//! Endpoint / region / credential resolution
//! (pi `bedrock-endpoint-resolution.test.ts` + `bedrock-credentials.test.ts`).

use super::*;

fn resolve(
    model: &Model,
    bedrock: &BedrockOptions,
    overlay: Option<&ProviderEnv>,
    ambient: &ProviderEnv,
) -> BedrockClientConfig {
    let opts = StreamOptions {
        env: overlay.cloned(),
        ..Default::default()
    };
    let auth = no_auth();
    resolve_client_config(model, &opts, bedrock, &auth, &env_source(overlay, ambient))
}

/// pi: "does not pin standard AWS endpoints when AWS_REGION is configured".
#[test]
fn aws_region_wins_and_suppresses_the_pinned_standard_endpoint() {
    let mut model = opus_48();
    model.id = "us.anthropic.claude-opus-4-8".into();
    let ambient = env_map(&[("AWS_REGION", "us-east-2")]);
    let config = resolve(&model, &BedrockOptions::default(), None, &ambient);
    assert_eq!(config.region.as_deref(), Some("us-east-2"));
    // Upstream leaves `config.endpoint` unset here and lets the SDK resolve the regional host;
    // cyrup materialises that same host, which is what "not pinned to model.baseUrl" means.
    assert_eq!(
        config.endpoint,
        "https://bedrock-runtime.us-east-2.amazonaws.com"
    );
}

/// pi: "derives region from a built-in EU endpoint when no region or profile is configured".
#[test]
fn endpoint_region_is_derived_when_nothing_else_is_configured() {
    let mut model = sonnet_45();
    model.id = "eu.anthropic.claude-sonnet-4-5-20250929-v1:0".into();
    model.base_url = "https://bedrock-runtime.eu-central-1.amazonaws.com".to_string();
    let ambient = ProviderEnv::new();
    let config = resolve(&model, &BedrockOptions::default(), None, &ambient);
    assert_eq!(
        config.endpoint,
        "https://bedrock-runtime.eu-central-1.amazonaws.com"
    );
    assert_eq!(config.region.as_deref(), Some("eu-central-1"));
}

/// pi: "handles missing regions for explicit, scoped, and ambient profiles" — the AMBIENT
/// profile is the one that must suppress both the pinned endpoint and the us-east-1 default.
#[test]
fn an_ambient_profile_suppresses_the_endpoint_pin_and_the_region_default() {
    let mut model = sonnet_45();
    model.base_url = "https://bedrock-runtime.eu-central-1.amazonaws.com".to_string();

    // Explicit profile: endpoint still pinned, region still derived.
    let ambient = ProviderEnv::new();
    let explicit = resolve(
        &model,
        &BedrockOptions {
            profile: Some("bedrock-profile".to_string()),
            ..Default::default()
        },
        None,
        &ambient,
    );
    assert_eq!(explicit.profile.as_deref(), Some("bedrock-profile"));
    assert_eq!(explicit.region.as_deref(), Some("eu-central-1"));

    // Scoped `AWS_PROFILE` (overlay only) behaves like the explicit option.
    let overlay = env_map(&[("AWS_PROFILE", "scoped-bedrock-profile")]);
    let scoped = resolve(&model, &BedrockOptions::default(), Some(&overlay), &ambient);
    assert_eq!(scoped.profile.as_deref(), Some("scoped-bedrock-profile"));
    assert_eq!(scoped.region.as_deref(), Some("eu-central-1"));

    // Ambient `AWS_PROFILE`: upstream leaves BOTH endpoint and region undefined.
    let ambient = env_map(&[("AWS_PROFILE", "ambient-bedrock-profile")]);
    let ambient_cfg = resolve(&model, &BedrockOptions::default(), None, &ambient);
    assert_eq!(
        ambient_cfg.profile.as_deref(),
        Some("ambient-bedrock-profile")
    );
    assert_eq!(ambient_cfg.region, None);
}

/// pi: "still passes custom Bedrock endpoints through to the SDK client".
#[test]
fn a_custom_endpoint_is_always_pinned() {
    let mut model = opus_48();
    model.base_url = "https://bedrock-vpc.example.com".to_string();
    let ambient = env_map(&[("AWS_REGION", "us-west-2")]);
    let config = resolve(&model, &BedrockOptions::default(), None, &ambient);
    assert_eq!(config.endpoint, "https://bedrock-vpc.example.com");
    assert_eq!(config.region.as_deref(), Some("us-west-2"));
}

/// pi: "extracts region from inference profile ARN regardless of AWS_REGION" (+ the GovCloud
/// partition form).
#[test]
fn an_arn_region_beats_aws_region() {
    let ambient = env_map(&[("AWS_REGION", "us-east-1")]);

    let mut model = opus_48();
    model.id =
        "arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/abc123".into();
    let config = resolve(&model, &BedrockOptions::default(), None, &ambient);
    assert_eq!(config.region.as_deref(), Some("us-west-2"));

    let mut gov = opus_48();
    gov.id =
        "arn:aws-us-gov:bedrock:us-gov-west-1:123456789012:application-inference-profile/abc"
            .into();
    let config = resolve(&gov, &BedrockOptions::default(), None, &ambient);
    assert_eq!(config.region.as_deref(), Some("us-gov-west-1"));
}

/// pi: "uses the generic API key option as a Bedrock bearer token".
#[test]
fn the_api_key_option_becomes_a_bearer_token() {
    let model = opus_48();
    let ambient = ProviderEnv::new();
    let opts = StreamOptions {
        api_key: Some("bedrock-api-key".to_string()),
        ..Default::default()
    };
    let config = resolve_client_config(
        &model,
        &opts,
        &BedrockOptions::default(),
        &no_auth(),
        &env_source(None, &ambient),
    );
    assert_eq!(config.bearer_token.as_deref(), Some("bedrock-api-key"));

    let mut headers = BTreeMap::new();
    authorize(&mut headers, &config, "https://x/y", b"{}").unwrap();
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("Bearer bedrock-api-key")
    );
    // A bearer request must NOT be SigV4-signed.
    assert!(!headers.contains_key("x-amz-date"));
}

/// pi `bedrock-credentials.test.ts`: an explicit or scoped profile must beat ambient access
/// keys; an ambient-only profile must not.
#[test]
fn a_configured_profile_beats_ambient_access_keys() {
    let model = opus_48();
    let ambient = env_map(&[
        ("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE"),
        ("AWS_SECRET_ACCESS_KEY", "secretexample"),
    ]);

    let explicit = resolve(
        &model,
        &BedrockOptions {
            profile: Some("explicit-profile".to_string()),
            ..Default::default()
        },
        None,
        &ambient,
    );
    assert_eq!(explicit.profile.as_deref(), Some("explicit-profile"));
    assert_eq!(explicit.credentials, None);

    let overlay = env_map(&[("AWS_PROFILE", "scoped-profile")]);
    let scoped = resolve(&model, &BedrockOptions::default(), Some(&overlay), &ambient);
    assert_eq!(scoped.profile.as_deref(), Some("scoped-profile"));
    assert_eq!(scoped.credentials, None);

    // No profile at all: the ambient keys are used.
    let plain = resolve(&model, &BedrockOptions::default(), None, &ambient);
    assert_eq!(plain.profile, None);
    assert_eq!(
        plain.credentials,
        Some(AwsCredentials {
            access_key_id: "AKIAEXAMPLE".to_string(),
            secret_access_key: "secretexample".to_string(),
            session_token: None,
        })
    );

    // An AMBIENT profile does not suppress the ambient keys (pi's third credentials case).
    let mut ambient_profile = ambient.clone();
    ambient_profile.insert("AWS_PROFILE".to_string(), "ambient-profile".to_string());
    let cfg = resolve(&model, &BedrockOptions::default(), None, &ambient_profile);
    assert_eq!(cfg.profile.as_deref(), Some("ambient-profile"));
    assert!(cfg.credentials.is_some());
}

#[test]
fn skip_auth_installs_the_dummy_credential_pair() {
    let model = opus_48();
    let ambient = env_map(&[
        ("AWS_BEDROCK_SKIP_AUTH", "1"),
        ("AWS_BEARER_TOKEN_BEDROCK", "tok"),
    ]);
    let config = resolve(&model, &BedrockOptions::default(), None, &ambient);
    assert_eq!(
        config.credentials.as_ref().map(|c| c.access_key_id.clone()),
        Some(SKIP_AUTH_ACCESS_KEY.to_string())
    );
    // `useBearerToken` is `bearerToken !== undefined && !skipAuth`.
    assert_eq!(config.bearer_token, None);
}

#[test]
fn only_standard_runtime_hosts_yield_an_endpoint_region() {
    assert_eq!(
        standard_bedrock_endpoint_region("https://bedrock-runtime.eu-central-1.amazonaws.com"),
        Some("eu-central-1".to_string())
    );
    assert_eq!(
        standard_bedrock_endpoint_region("https://bedrock-runtime-fips.us-east-1.amazonaws.com"),
        Some("us-east-1".to_string())
    );
    assert_eq!(
        standard_bedrock_endpoint_region("https://bedrock-runtime.cn-north-1.amazonaws.com.cn"),
        Some("cn-north-1".to_string())
    );
    assert_eq!(
        standard_bedrock_endpoint_region("https://bedrock-vpc.example.com"),
        None
    );
    // Anchored: an extra label must not match.
    assert_eq!(
        standard_bedrock_endpoint_region("https://bedrock-runtime.us-east-1.evil.amazonaws.com"),
        None
    );
}

#[test]
fn shared_credentials_files_are_read_for_a_configured_profile() {
    let dir = std::env::temp_dir().join(format!("cyrup-bedrock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("credentials");
    std::fs::write(
        &path,
        "[default]\naws_access_key_id = DEFAULTKEY\naws_secret_access_key = defaultsecret\n\n\
         [work]\naws_access_key_id = WORKKEY\naws_secret_access_key = worksecret\naws_session_token = worktoken\n",
    )
    .unwrap();

    let ambient = env_map(&[(
        "AWS_SHARED_CREDENTIALS_FILE",
        path.to_string_lossy().as_ref(),
    )]);
    let env = env_source(None, &ambient);
    assert_eq!(
        shared_profile_credentials("work", &env),
        Some(AwsCredentials {
            access_key_id: "WORKKEY".to_string(),
            secret_access_key: "worksecret".to_string(),
            session_token: Some("worktoken".to_string()),
        })
    );
    assert_eq!(
        shared_profile_credentials("default", &env)
            .map(|c| c.access_key_id),
        Some("DEFAULTKEY".to_string())
    );
    assert_eq!(shared_profile_credentials("absent", &env), None);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

//! Client configuration (pi `bedrock-converse-stream.ts:136-220`).

use super::env::EnvSource;
use super::options::BedrockOptions;
use super::url::url_host;
use crate::auth::AuthResult;
use crate::model::Model;
use crate::stream::StreamOptions;

/// The dummy credential pair upstream installs when `AWS_BEDROCK_SKIP_AUTH=1`
/// (`bedrock-converse-stream.ts:186-189`).
pub(super) const SKIP_AUTH_ACCESS_KEY: &str = "dummy-access-key";
const SKIP_AUTH_SECRET_KEY: &str = "dummy-secret-key";

/// Static AWS credentials (pi `BedrockRuntimeClientConfig["credentials"]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AwsCredentials {
    pub(super) access_key_id: String,
    pub(super) secret_access_key: String,
    pub(super) session_token: Option<String>,
}

/// The resolved `BedrockRuntimeClientConfig` (pi `config`, `bedrock-converse-stream.ts:140-220`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct BedrockClientConfig {
    /// `config.profile`.
    pub(super) profile: Option<String>,
    /// `config.region`.
    pub(super) region: Option<String>,
    /// `config.endpoint`, already defaulted to the standard regional runtime host when upstream
    /// leaves it unset (the SDK's endpoint resolver does that for pi).
    pub(super) endpoint: String,
    /// `config.credentials`.
    pub(super) credentials: Option<AwsCredentials>,
    /// `config.token.token` + `authSchemePreference: ["httpBearerAuth"]`.
    pub(super) bearer_token: Option<String>,
}

/// 1:1 port of pi's client-config block (`bedrock-converse-stream.ts:136-220`).
///
/// The precedence rules that matter, in upstream's own order:
/// 1. `options.profile || options.env.AWS_PROFILE || AWS_PROFILE` becomes `config.profile`.
/// 2. A standard `bedrock-runtime.<region>.amazonaws.com[.cn]` base URL is pinned as
///    `config.endpoint` **only** when neither a region nor an ambient `AWS_PROFILE` is configured;
///    a custom (VPC/proxy) endpoint is always pinned.
/// 3. Region: ARN-embedded > explicit/env > endpoint-derived (when pinned) > `us-east-1`, and the
///    last default is skipped entirely when an ambient `AWS_PROFILE` is set.
/// 4. Ambient access keys are used only when no profile was explicitly configured.
pub(super) fn resolve_client_config(
    model: &Model,
    opts: &StreamOptions,
    bedrock: &BedrockOptions,
    auth: &AuthResult,
    env: &EnvSource<'_>,
) -> BedrockClientConfig {
    let base_url = auth
        .auth
        .base_url
        .clone()
        .unwrap_or_else(|| model.base_url.clone());

    // pi `:139`: the explicit option, then the SCOPED `AWS_PROFILE` (overlay only).
    let options_profile = bedrock.profile.clone().or_else(|| {
        opts.env
            .as_ref()
            .or(auth.env.as_ref())
            .and_then(|m| m.get("AWS_PROFILE"))
            .filter(|v| !v.is_empty())
            .cloned()
    });
    let profile = options_profile
        .clone()
        .or_else(|| env.get("AWS_PROFILE"))
        .filter(|v| !v.is_empty());

    let configured_region = configured_bedrock_region(bedrock, env);
    let has_ambient_profile = env.ambient("AWS_PROFILE").is_some();
    let endpoint_region = standard_bedrock_endpoint_region(&base_url);
    let use_explicit_endpoint = should_use_explicit_bedrock_endpoint(
        &base_url,
        configured_region.as_deref(),
        has_ambient_profile,
    );

    let skip_auth = env.get("AWS_BEDROCK_SKIP_AUTH").as_deref() == Some("1");
    let bearer_token = bedrock
        .bearer_token
        .clone()
        .or_else(|| opts.api_key.clone())
        .or_else(|| auth.auth.api_key.clone())
        .or_else(|| env.get("AWS_BEARER_TOKEN_BEDROCK"))
        .filter(|t| !t.is_empty());
    let use_bearer_token = bearer_token.is_some() && !skip_auth;

    // pi `:173-182`.
    let region = if let Some(arn_region) = arn_region(model.id.as_str()) {
        Some(arn_region)
    } else if let Some(r) = configured_region.clone() {
        Some(r)
    } else if use_explicit_endpoint && endpoint_region.is_some() {
        endpoint_region.clone()
    } else if !has_ambient_profile {
        Some("us-east-1".to_string())
    } else {
        None
    };

    // pi `:185-195`.
    let credentials = if skip_auth {
        Some(AwsCredentials {
            access_key_id: SKIP_AUTH_ACCESS_KEY.to_string(),
            secret_access_key: SKIP_AUTH_SECRET_KEY.to_string(),
            session_token: None,
        })
    } else if options_profile.is_none() {
        configured_bedrock_credentials(env)
    } else {
        None
    };

    // The SDK resolves a bare `profile` through the shared config/credentials files; without the
    // SDK that resolution happens here so an explicit/scoped profile is not silently unauthenticated.
    let credentials = credentials.or_else(|| {
        profile
            .as_deref()
            .and_then(|p| shared_profile_credentials(p, env))
    });

    // Likewise for the endpoint: upstream leaves `config.endpoint` unset and lets the SDK's
    // endpoint resolver build `https://bedrock-runtime.<region>.amazonaws.com`.
    let endpoint = if use_explicit_endpoint {
        base_url.trim_end_matches('/').to_string()
    } else {
        let r = region.clone().unwrap_or_else(|| "us-east-1".to_string());
        format!("https://bedrock-runtime.{r}.amazonaws.com")
    };

    BedrockClientConfig {
        profile,
        region,
        endpoint,
        credentials,
        bearer_token: if use_bearer_token { bearer_token } else { None },
    }
}

/// pi `getConfiguredBedrockRegion` (`bedrock-converse-stream.ts:979-986`).
pub(super) fn configured_bedrock_region(
    bedrock: &BedrockOptions,
    env: &EnvSource<'_>,
) -> Option<String> {
    bedrock
        .region
        .clone()
        .filter(|v| !v.is_empty())
        .or_else(|| env.get("AWS_REGION"))
        .or_else(|| env.get("AWS_DEFAULT_REGION"))
}

/// pi `getConfiguredBedrockCredentials` (`bedrock-converse-stream.ts:988-1000`).
fn configured_bedrock_credentials(env: &EnvSource<'_>) -> Option<AwsCredentials> {
    let access_key_id = env.get("AWS_ACCESS_KEY_ID")?;
    let secret_access_key = env.get("AWS_SECRET_ACCESS_KEY")?;
    Some(AwsCredentials {
        access_key_id,
        secret_access_key,
        session_token: env.get("AWS_SESSION_TOKEN"),
    })
}

/// pi `getStandardBedrockEndpointRegion` (`bedrock-converse-stream.ts:1002-1014`): the region of a
/// `bedrock-runtime[-fips].<region>.amazonaws.com[.cn]` host, or `None` for any other host.
///
/// `[CYRUP-DELTA]` upstream applies the regex to `new URL(baseUrl).hostname`; cyrup has no `regex`
/// dependency, so the host is extracted by hand and the pattern is matched structurally. The
/// accepted set is identical: the `[a-z0-9-]+` region class is checked explicitly and a host with
/// extra labels (e.g. `bedrock-runtime.us-east-1.evil.amazonaws.com`) is rejected because the
/// suffix match is anchored.
pub(super) fn standard_bedrock_endpoint_region(base_url: &str) -> Option<String> {
    let host = url_host(base_url)?.to_lowercase();
    let rest = host
        .strip_suffix(".amazonaws.com.cn")
        .or_else(|| host.strip_suffix(".amazonaws.com"))?;
    let region = rest
        .strip_prefix("bedrock-runtime-fips.")
        .or_else(|| rest.strip_prefix("bedrock-runtime."))?;
    if region.is_empty()
        || region.contains('.')
        || !region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    Some(region.to_string())
}

/// pi `shouldUseExplicitBedrockEndpoint` (`bedrock-converse-stream.ts:1016-1027`).
fn should_use_explicit_bedrock_endpoint(
    base_url: &str,
    configured_region: Option<&str>,
    has_ambient_profile: bool,
) -> bool {
    if standard_bedrock_endpoint_region(base_url).is_none() {
        return true;
    }
    configured_region.is_none() && !has_ambient_profile
}

/// pi's inline ARN region capture (`bedrock-converse-stream.ts:173`):
/// `/^arn:aws(?:-[a-z0-9-]+)?:bedrock:([a-z0-9-]+):/`.
///
/// `[CYRUP-DELTA]` hand-rolled for the same no-`regex` reason as above. Greedy scanning is exact
/// here: both capture classes are terminated by a literal `:`, which is not in either class.
fn arn_region(model_id: &str) -> Option<String> {
    let rest = model_id.strip_prefix("arn:aws")?;
    // `(?:-[a-z0-9-]+)?` then `:bedrock:`.
    let rest = match rest.strip_prefix(':') {
        Some(r) => r,
        None => {
            let partition = rest.strip_prefix('-')?;
            let end = partition.find(':').filter(|i| *i > 0).filter(|i| {
                partition.get(..*i).is_some_and(|p| {
                    p.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                })
            })?;
            partition.get(end + 1..)?
        }
    };
    let rest = rest.strip_prefix("bedrock:")?;
    let end = rest.find(':').filter(|i| *i > 0)?;
    let region = rest.get(..end)?;
    if !region
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    Some(region.to_string())
}

/// Read static credentials for `profile` out of the shared credentials/config files.
///
/// This is the part of the SDK's default credential chain that a bare `profile` needs: upstream
/// sets `config.profile` and the SDK resolves it from `~/.aws/credentials` (falling back to
/// `[profile <name>]` in `~/.aws/config`). Honors `AWS_SHARED_CREDENTIALS_FILE` and
/// `AWS_CONFIG_FILE`, exactly as the SDK does. Role assumption / SSO / IMDS are **not** ported;
/// a profile that needs one resolves to `None` here and the request is sent unsigned-credentialed,
/// which surfaces as the provider's own auth error rather than a silent wrong-identity request.
pub(super) fn shared_profile_credentials(
    profile: &str,
    env: &EnvSource<'_>,
) -> Option<AwsCredentials> {
    let home = env.get("HOME").or_else(|| env.get("USERPROFILE"));
    let credentials_path = env.get("AWS_SHARED_CREDENTIALS_FILE").or_else(|| {
        home.as_ref()
            .map(|h| format!("{}/.aws/credentials", h.trim_end_matches('/')))
    });
    let config_path = env.get("AWS_CONFIG_FILE").or_else(|| {
        home.as_ref()
            .map(|h| format!("{}/.aws/config", h.trim_end_matches('/')))
    });

    for (path, section) in [
        (credentials_path, profile.to_string()),
        (config_path, format!("profile {profile}")),
    ] {
        let Some(path) = path else { continue };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(creds) = parse_ini_profile(&text, &section) {
            return Some(creds);
        }
    }
    None
}

/// Extract `aws_access_key_id` / `aws_secret_access_key` / `aws_session_token` from one INI
/// section. Returns `None` unless both required keys are present.
fn parse_ini_profile(text: &str, section: &str) -> Option<AwsCredentials> {
    let mut in_section = false;
    let mut access_key_id: Option<String> = None;
    let mut secret_access_key: Option<String> = None;
    let mut session_token: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = name.trim() == section;
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim().to_ascii_lowercase().as_str() {
            "aws_access_key_id" => access_key_id = Some(value),
            "aws_secret_access_key" => secret_access_key = Some(value),
            "aws_session_token" => session_token = Some(value),
            _ => {}
        }
    }

    Some(AwsCredentials {
        access_key_id: access_key_id.filter(|v| !v.is_empty())?,
        secret_access_key: secret_access_key.filter(|v| !v.is_empty())?,
        session_token: session_token.filter(|v| !v.is_empty()),
    })
}

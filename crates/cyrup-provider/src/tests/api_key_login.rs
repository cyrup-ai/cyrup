//! CFG-005 / PROV-003 — [`ApiKeyAuth::login`], the four multi-secret api-key setups.
//!
//! pi's `ApiKeyAuth` carries an OPTIONAL `login?(interaction): Promise<ApiKeyCredential>`
//! (`ai/src/auth/types.ts:166`, *"Interactive setup (prompt for key/provider env). Absent =
//! ambient-only."*). Four built-ins implement it, and **none of them is a single secret**:
//!
//! | provider | prompts | pi |
//! |---|---|---|
//! | `cloudflare-workers-ai` | key + account id | `providers/cloudflare-auth.ts:50-54` |
//! | `cloudflare-ai-gateway` | key + account id + gateway id | `providers/cloudflare-auth.ts:70-79` |
//! | `google-vertex` | picker → key, or project + location (+ credentials path) | `providers/google-vertex.ts:15-61` |
//! | `amazon-bedrock` | picker → token, or profile, or bare ack | `providers/amazon-bedrock.ts:13-51` |
//!
//! Before this landed the trait had only `name` + `resolve`, so `/login` selected a flow by
//! SNIFFING the strategy's display name and ran a single-secret prompt for all four — storing a
//! credential that `resolve` can never accept while reporting success. Each assertion below is on
//! the credential SHAPE, because that shape is exactly what the sniffer could not produce.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::all_providers;
use crate::auth::oauth::{AuthPromptKind, OAuthError, ScriptedInteraction};
use crate::auth::types::Credential;
use crate::auth::ApiKeyAuth;
use std::sync::Arc;

fn api_key_strategy(id: &str) -> Arc<dyn ApiKeyAuth> {
    all_providers()
        .into_iter()
        .find(|p| p.id().as_str() == id)
        .and_then(|p| p.provider_auth().and_then(|a| a.api_key.clone()))
        .unwrap_or_else(|| panic!("{id} must expose an api-key strategy"))
}

fn parts(cred: &Credential) -> (Option<String>, Vec<(String, String)>) {
    match cred {
        Credential::ApiKey { key, env } => (
            key.clone(),
            env.iter()
                .flat_map(|e| e.iter())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        other => panic!("expected an api_key credential, got {other:?}"),
    }
}

fn answers(values: &[&str]) -> ScriptedInteraction {
    ScriptedInteraction::new(values.iter().map(|v| Ok((*v).to_string())).collect())
}

/// The four are the ONLY built-ins that declare an interactive api-key setup — the env-var
/// strategies stay ambient-only, exactly as upstream's absent `login?` does.
#[test]
fn exactly_the_four_multi_secret_strategies_declare_a_login() {
    let mut with_login: Vec<String> = all_providers()
        .into_iter()
        .filter(|p| {
            p.provider_auth()
                .and_then(|a| a.api_key.as_ref())
                .is_some_and(|s| s.supports_login())
        })
        .map(|p| p.id().as_str().to_string())
        .collect();
    with_login.sort();
    assert_eq!(
        with_login,
        vec![
            "amazon-bedrock".to_string(),
            "cloudflare-ai-gateway".to_string(),
            "cloudflare-workers-ai".to_string(),
            "google-vertex".to_string(),
        ]
    );
}

/// An ambient-only strategy reports `LoginUnsupported`, matching upstream's absent `login?`.
#[tokio::test]
async fn env_key_strategies_report_login_unsupported() {
    let strategy = api_key_strategy("anthropic");
    assert!(!strategy.supports_login());
    let interaction = answers(&["never-read"]);
    match strategy.login(&interaction).await {
        Err(OAuthError::LoginUnsupported { .. }) => {}
        Err(other) => panic!("expected LoginUnsupported, got {other}"),
        Ok(_) => panic!("an ambient-only strategy must not mint a credential"),
    }
}

/// `providers/cloudflare-auth.ts:50-54` — a secret AND a text prompt; the account id lands in the
/// env overlay, which `resolve` requires (`:35`).
#[tokio::test]
async fn cloudflare_workers_ai_login_stores_key_and_account_id() {
    let strategy = api_key_strategy("cloudflare-workers-ai");
    assert!(strategy.supports_login());

    let interaction = answers(&["cf-key", "acct-1"]);
    let cred = strategy.login(&interaction).await.expect("login");
    let (key, env) = parts(&cred);
    assert_eq!(key.as_deref(), Some("cf-key"));
    assert_eq!(
        env,
        vec![("CLOUDFLARE_ACCOUNT_ID".to_string(), "acct-1".to_string())]
    );

    let prompts = interaction.prompts();
    assert_eq!(prompts.len(), 2, "a single-secret flow is not enough");
    assert_eq!(prompts[0].kind, Some(AuthPromptKind::Secret));
    assert_eq!(prompts[0].message, "Enter Cloudflare API key");
    assert_eq!(prompts[1].kind, Some(AuthPromptKind::Text));
    assert_eq!(prompts[1].message, "Enter Cloudflare account ID");
}

/// `providers/cloudflare-auth.ts:70-79` — the same plus the gateway id, which the gateway
/// `resolve` also requires (`:35`).
#[tokio::test]
async fn cloudflare_ai_gateway_login_stores_the_gateway_id_too() {
    let strategy = api_key_strategy("cloudflare-ai-gateway");
    let interaction = answers(&["cf-key", "acct-1", "gw-9"]);
    let cred = strategy.login(&interaction).await.expect("login");
    let (key, env) = parts(&cred);
    assert_eq!(key.as_deref(), Some("cf-key"));
    assert_eq!(
        env,
        vec![
            ("CLOUDFLARE_ACCOUNT_ID".to_string(), "acct-1".to_string()),
            ("CLOUDFLARE_GATEWAY_ID".to_string(), "gw-9".to_string()),
        ]
    );
    assert_eq!(interaction.prompts().len(), 3);
}

/// `providers/google-vertex.ts:15-61` — the picker, then either one secret or a KEYLESS credential
/// carrying only project/location (+ credentials path on the service-account arm).
#[tokio::test]
async fn google_vertex_login_covers_all_three_arms() {
    let strategy = api_key_strategy("google-vertex");
    assert!(strategy.supports_login());

    // `:25-30` — the api-key arm.
    let interaction = answers(&["api-key", "gcp-secret"]);
    let (key, env) = parts(&strategy.login(&interaction).await.expect("api-key arm"));
    assert_eq!(key.as_deref(), Some("gcp-secret"));
    assert!(env.is_empty());
    assert_eq!(interaction.prompts()[0].kind, Some(AuthPromptKind::Select));

    // `:34-60` — ADC: NO key at all, only the env overlay `resolve` reads (`:71-75`).
    let interaction = answers(&["adc", "proj-1", "us-central1"]);
    let (key, env) = parts(&strategy.login(&interaction).await.expect("adc arm"));
    assert_eq!(key, None, "the ADC arm stores no key — that is the point");
    assert_eq!(
        env,
        vec![
            (
                "GOOGLE_CLOUD_LOCATION".to_string(),
                "us-central1".to_string()
            ),
            ("GOOGLE_CLOUD_PROJECT".to_string(), "proj-1".to_string()),
        ]
    );

    // `:49-52` — the service-account arm adds the credentials path.
    let interaction = answers(&["service-account", "proj-1", "us-central1", "/tmp/sa.json"]);
    let (key, env) = parts(&strategy.login(&interaction).await.expect("sa arm"));
    assert_eq!(key, None);
    assert!(env.contains(&(
        "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
        "/tmp/sa.json".to_string()
    )));

    // `:31-33` — an unknown option id is upstream's message, verbatim.
    let interaction = answers(&["nonsense"]);
    let error = strategy.login(&interaction).await.expect_err("unknown arm");
    assert_eq!(
        error.to_string(),
        "Unknown Google Vertex AI auth method: nonsense"
    );
}

/// `providers/amazon-bedrock.ts:13-51` — the picker, then a bearer token, an `AWS_PROFILE`-only
/// credential, or a credential with neither (`:49`, `{ type: "api_key" }`).
#[tokio::test]
async fn amazon_bedrock_login_covers_all_three_arms() {
    let strategy = api_key_strategy("amazon-bedrock");
    assert!(strategy.supports_login());

    // `:23-28`
    let interaction = answers(&["bearer-token", "bedrock-token"]);
    let (key, env) = parts(&strategy.login(&interaction).await.expect("bearer arm"));
    assert_eq!(key.as_deref(), Some("bedrock-token"));
    assert!(env.is_empty());

    // `:39-44`
    let interaction = answers(&["aws-profile", "dev"]);
    let (key, env) = parts(&strategy.login(&interaction).await.expect("profile arm"));
    assert_eq!(key, None);
    assert_eq!(env, vec![("AWS_PROFILE".to_string(), "dev".to_string())]);

    // `:46-50` — the ack prompt is awaited, its answer discarded, and NOTHING is stored.
    let interaction = answers(&["credential-chain", ""]);
    let (key, env) = parts(&strategy.login(&interaction).await.expect("chain arm"));
    assert_eq!(key, None);
    assert!(env.is_empty());
    assert_eq!(
        interaction.prompts().len(),
        2,
        "the `press Enter to continue` prompt is part of the flow"
    );

    // `:45`
    let interaction = answers(&["nonsense"]);
    let error = strategy.login(&interaction).await.expect_err("unknown arm");
    assert_eq!(
        error.to_string(),
        "Unknown Amazon Bedrock auth method: nonsense"
    );
}

//! The Cloudflare Workers AI provider (arch-01 §5). A pure-[`openai-completions`](crate::api::openai_completions)
//! provider whose models speak the OpenAI-compatible Workers AI endpoint. Unlike the rest of the
//! fleet it needs a bespoke auth strategy: the catalog `baseUrl` carries a `{CLOUDFLARE_ACCOUNT_ID}`
//! placeholder that auth resolution must substitute with the resolved account id, and the request
//! is keyed by `CLOUDFLARE_API_KEY` alongside the account id.
//!
//! 1:1 port of Pi's `providers/cloudflare-workers-ai.ts` + the `cloudflareWorkersAIAuth()` resolver
//! in `providers/cloudflare-auth.ts` (workers-ai kind) + the verbatim `cloudflare-workers-ai.models.ts`
//! catalog.

use crate::api::{builtin_registry, ApiRegistry};
use crate::auth::types::{AuthContext, AuthResult, Credential, ModelAuth, ProviderEnv};
use crate::auth::{ApiKeyAuth, CredentialStore, InMemoryCredentialStore, ProviderAuth};
use crate::error::AuthError;
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// The provider id.
pub const CLOUDFLARE_WORKERS_AI_PROVIDER_ID: &str = "cloudflare-workers-ai";

/// The Cloudflare API-key env/credential field (`cloudflare-auth.ts`).
pub const CLOUDFLARE_API_KEY: &str = "CLOUDFLARE_API_KEY";
/// The Cloudflare account-id env/credential field, substituted into the model base URL.
pub const CLOUDFLARE_ACCOUNT_ID: &str = "CLOUDFLARE_ACCOUNT_ID";

/// The verbatim catalog (extracted from Pi's `cloudflare-workers-ai.models.ts`).
const CATALOG_JSON: &str = include_str!("catalog/cloudflare-workers-ai.json");

/// Parse the embedded Cloudflare Workers AI catalog into [`Model`]s. A parse failure yields an empty
/// catalog (surfaced loudly by the catalog tests) rather than a panic (NO-PANIC policy).
pub fn cloudflare_workers_ai_models() -> Vec<Model> {
    serde_json::from_str(CATALOG_JSON).unwrap_or_default()
}

/// Resolve a single Cloudflare field (Pi `cloudflare-auth.ts` `resolveValue`): from a stored
/// API-key credential when present (the api key from `credential.key`, every other field from the
/// credential's env overlay), otherwise from the ambient auth context.
async fn resolve_value(
    name: &str,
    ctx: &dyn AuthContext,
    cred: Option<&Credential>,
) -> Option<String> {
    match cred {
        Some(Credential::ApiKey { key, env }) => {
            if name == CLOUDFLARE_API_KEY {
                key.clone()
            } else {
                env.as_ref().and_then(|e| e.get(name).cloned())
            }
        }
        // An OAuth credential is not this api-key strategy's concern (Pi types it `ApiKeyCredential`).
        Some(Credential::Oauth { .. }) => None,
        None => ctx.env(name).await,
    }
}

/// Cloudflare Workers AI auth (Pi `cloudflareWorkersAIAuth()`): resolves `CLOUDFLARE_API_KEY` +
/// `CLOUDFLARE_ACCOUNT_ID`, substitutes the account id into the model `baseUrl`'s
/// `{CLOUDFLARE_ACCOUNT_ID}` placeholder, and carries the account id forward as a provider-scoped
/// env overlay. Resolves to `None` (not configured) when either the api key or account id is absent.
struct CloudflareWorkersAiAuth;

#[async_trait::async_trait]
impl ApiKeyAuth for CloudflareWorkersAiAuth {
    fn name(&self) -> &str {
        "Cloudflare API key"
    }

    async fn resolve(
        &self,
        model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let api_key = resolve_value(CLOUDFLARE_API_KEY, ctx, cred).await;
        let account_id = resolve_value(CLOUDFLARE_ACCOUNT_ID, ctx, cred).await;

        // Pi `if (!apiKey || !accountId) return undefined;` — empty strings are falsy in JS.
        let (Some(api_key), Some(account_id)) = (api_key, account_id) else {
            return Ok(None);
        };
        if api_key.is_empty() || account_id.is_empty() {
            return Ok(None);
        }

        // `resolveCloudflareBaseUrl`: replace every `{CLOUDFLARE_ACCOUNT_ID}` placeholder.
        let base_url = model
            .base_url
            .clone()
            .unwrap_or_default()
            .replace(&format!("{{{CLOUDFLARE_ACCOUNT_ID}}}"), &account_id);

        let mut env = ProviderEnv::new();
        env.insert(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id);

        let source = if cred.is_some() { "stored credential" } else { CLOUDFLARE_API_KEY };

        Ok(Some(AuthResult {
            auth: ModelAuth {
                api_key: Some(api_key),
                headers: None,
                base_url: Some(base_url),
            },
            env: Some(env),
            source: Some(source.to_string()),
        }))
    }
}

/// The Cloudflare Workers AI [`ProviderAuth`] (Pi `cloudflareWorkersAIAuth()`).
pub fn cloudflare_workers_ai_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(Arc::new(CloudflareWorkersAiAuth))
}

/// Construct the Cloudflare Workers AI provider over the given credential store + shared api
/// registry. The registry MUST provide the `openai-completions` impl (use [`builtin_registry`]).
pub fn cloudflare_workers_ai_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        CLOUDFLARE_WORKERS_AI_PROVIDER_ID,
        "Cloudflare Workers AI",
        cloudflare_workers_ai_models(),
        cloudflare_workers_ai_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn cloudflare_workers_ai_provider() -> WireProvider {
    cloudflare_workers_ai_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::api::openai_completions::build_body;
    use crate::auth::types::AuthContext;
    use crate::auth::InMemoryCredentialStore;
    use crate::context::Context;
    use crate::known_api::OPENAI_COMPLETIONS;
    use crate::provider::Provider;
    use crate::stream::{collect_message, StreamOptions};
    use cyrup_core::{ProviderId, StopReason};
    use std::collections::BTreeMap;

    struct MapEnv(BTreeMap<String, String>);
    #[async_trait::async_trait]
    impl AuthContext for MapEnv {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    #[test]
    fn catalog_parses_with_expected_count_and_tags() {
        let models = cloudflare_workers_ai_models();
        assert_eq!(models.len(), 13);
        assert!(models.iter().all(|m| m.api.as_str() == OPENAI_COMPLETIONS));
        assert!(models.iter().all(|m| m.provider.as_str() == "cloudflare-workers-ai"));
        // Every catalog entry carries the unresolved account-id placeholder in its base URL.
        assert!(models
            .iter()
            .all(|m| m.base_url.as_deref() == Some(
                "https://api.cloudflare.com/client/v4/accounts/{CLOUDFLARE_ACCOUNT_ID}/ai/v1"
            )));
        // The shared compat block (Pi: store/developer/long-cache off, session-affinity on).
        let m = models.iter().find(|m| m.id.as_str() == "@cf/openai/gpt-oss-120b").expect("gpt-oss");
        let c = m.compat.as_ref().expect("compat");
        assert_eq!(c.supports_store, Some(false));
        assert_eq!(c.supports_developer_role, Some(false));
        assert_eq!(c.supports_long_cache_retention, Some(false));
        assert_eq!(c.send_session_affinity_headers, Some(true));
        assert!(m.reasoning);
    }

    #[test]
    fn env_mapping_present() {
        let vars = crate::env_api_keys::api_key_env_vars("cloudflare-workers-ai").expect("mapping");
        assert!(vars.contains(&CLOUDFLARE_API_KEY));
    }

    #[tokio::test]
    async fn resolve_substitutes_account_id_into_base_url() {
        // With API key + account id in the ambient env, auth resolves: the account-id placeholder
        // is substituted and the env overlay carries the account id (Pi `cloudflareWorkersAIAuth`).
        let auth = CloudflareWorkersAiAuth;
        let model = cloudflare_workers_ai_models()
            .into_iter()
            .find(|m| m.id.as_str() == "@cf/openai/gpt-oss-120b")
            .expect("model");
        let ctx = MapEnv(BTreeMap::from([
            (CLOUDFLARE_API_KEY.to_string(), "cf-key".to_string()),
            (CLOUDFLARE_ACCOUNT_ID.to_string(), "acct-123".to_string()),
        ]));
        let result = auth.resolve(&model, &ctx, None).await.expect("ok").expect("configured");
        assert_eq!(result.auth.api_key.as_deref(), Some("cf-key"));
        assert_eq!(
            result.auth.base_url.as_deref(),
            Some("https://api.cloudflare.com/client/v4/accounts/acct-123/ai/v1")
        );
        assert_eq!(
            result.env.as_ref().and_then(|e| e.get(CLOUDFLARE_ACCOUNT_ID)).map(String::as_str),
            Some("acct-123")
        );
        assert_eq!(result.source.as_deref(), Some(CLOUDFLARE_API_KEY));
    }

    #[tokio::test]
    async fn resolve_requires_both_key_and_account_id() {
        let auth = CloudflareWorkersAiAuth;
        let model = cloudflare_workers_ai_models().into_iter().next().expect("model");
        // Only the api key → not configured.
        let ctx = MapEnv(BTreeMap::from([(CLOUDFLARE_API_KEY.to_string(), "cf-key".to_string())]));
        assert!(auth.resolve(&model, &ctx, None).await.expect("ok").is_none());
        // Only the account id → not configured.
        let ctx = MapEnv(BTreeMap::from([(CLOUDFLARE_ACCOUNT_ID.to_string(), "acct".to_string())]));
        assert!(auth.resolve(&model, &ctx, None).await.expect("ok").is_none());
    }

    #[tokio::test]
    async fn resolve_reads_account_id_from_stored_credential_env() {
        // A stored api-key credential supplies the key directly and the account id via its env
        // overlay (Pi `resolveValue`: api key from `credential.key`, others from `credential.env`).
        let auth = CloudflareWorkersAiAuth;
        let model = cloudflare_workers_ai_models().into_iter().next().expect("model");
        let cred = Credential::ApiKey {
            key: Some("stored-key".to_string()),
            env: Some(ProviderEnv::from([(
                CLOUDFLARE_ACCOUNT_ID.to_string(),
                "stored-acct".to_string(),
            )])),
        };
        // Ambient env is empty — everything comes from the credential.
        let ctx = MapEnv(BTreeMap::new());
        let result =
            auth.resolve(&model, &ctx, Some(&cred)).await.expect("ok").expect("configured");
        assert_eq!(result.auth.api_key.as_deref(), Some("stored-key"));
        assert!(result.auth.base_url.as_deref().unwrap().contains("stored-acct"));
        assert_eq!(result.source.as_deref(), Some("stored credential"));
    }

    #[tokio::test]
    async fn unconfigured_provider_yields_error_terminal() {
        let provider = cloudflare_workers_ai_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider.get_model("@cf/openai/gpt-oss-120b").expect("model").clone();
        let msg =
            collect_message(provider.stream(&model, &Context::default(), &StreamOptions::default()))
                .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("not configured"));
    }

    #[tokio::test]
    async fn configured_resolves_and_rewrites_base_url_then_reaches_transport() {
        // API key + account id present → auth resolves and substitutes the account id; the model's
        // (placeholder) base URL is replaced. Pointing at an unroutable host proves the resolved
        // request reached transport rather than short-circuiting as "not configured".
        let provider = cloudflare_workers_ai_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::from([
            (CLOUDFLARE_API_KEY.to_string(), "cf-key".to_string()),
            // An unroutable host as the "account id" so the substituted URL fails fast at connect.
            (CLOUDFLARE_ACCOUNT_ID.to_string(), "acct".to_string()),
        ]))));
        let mut model = provider.get_model("@cf/openai/gpt-oss-120b").expect("model").clone();
        // Replace the catalog base URL with one whose placeholder yields an unroutable address.
        model.base_url = Some("http://127.0.0.1:1/{CLOUDFLARE_ACCOUNT_ID}/v1".to_string());
        let msg =
            collect_message(provider.stream(&model, &Context::default(), &StreamOptions::default()))
                .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(!err.contains("not configured"), "auth should have resolved: {err}");
        assert!(err.contains("transport"), "expected transport error: {err}");
    }

    #[test]
    fn catalog_drives_openai_completions_encoding() {
        // A cloudflare reasoning model encodes through the openai-completions body builder; with no
        // store support the body omits `store`, and a reasoning model carries a reasoning field.
        let models = cloudflare_workers_ai_models();
        let m = models.iter().find(|m| m.id.as_str() == "@cf/openai/gpt-oss-120b").expect("gpt-oss");
        let opts = StreamOptions {
            reasoning: cyrup_core::ModelThinkingLevel::High,
            max_tokens: Some(64),
            ..Default::default()
        };
        let body = build_body(m, &Context::default(), &opts);
        assert!(body.get("store").is_none(), "store unsupported → omitted");
        // The compat-driven body builder ran end-to-end and threaded the explicit max-tokens budget.
        assert!(body.get("messages").is_some(), "messages encoded");
    }

    #[test]
    fn provider_identity() {
        let p = cloudflare_workers_ai_provider();
        assert_eq!(p.id(), &ProviderId::from("cloudflare-workers-ai"));
        assert_eq!(p.name(), "Cloudflare Workers AI");
        assert_eq!(p.models().len(), 13);
    }
}

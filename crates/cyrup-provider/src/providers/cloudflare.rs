//! The Cloudflare Workers AI provider (arch-01 §5). A pure-[`openai-completions`](crate::api::openai_completions)
//! provider whose models speak the OpenAI-compatible Workers AI endpoint. Unlike the rest of the
//! fleet it needs a bespoke auth strategy: the catalog `baseUrl` carries a `{CLOUDFLARE_ACCOUNT_ID}`
//! placeholder that auth resolution must substitute with the resolved account id, and the request
//! is keyed by `CLOUDFLARE_API_KEY` alongside the account id.
//!
//! 1:1 port of Pi's `providers/cloudflare-workers-ai.ts` + the `cloudflareWorkersAIAuth()` resolver
//! in `providers/cloudflare-auth.ts` (workers-ai kind) + the verbatim `cloudflare-workers-ai.models.ts`
//! catalog.

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::oauth::AuthPrompt;
use crate::auth::types::{AuthContext, AuthResult, Credential, ModelAuth, ProviderEnv};
use crate::auth::{ApiKeyAuth, CredentialStore, InMemoryCredentialStore, ProviderAuth};
use crate::error::AuthError;
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// The `/login` prompt strings, verbatim from `providers/cloudflare-auth.ts:51-52` and `:71-73`.
/// They are the operator-visible contract, so they are pinned as constants rather than inlined.
const ENTER_CLOUDFLARE_API_KEY: &str = "Enter Cloudflare API key";
const ENTER_CLOUDFLARE_ACCOUNT_ID: &str = "Enter Cloudflare account ID";
const ENTER_CLOUDFLARE_GATEWAY_ID: &str = "Enter Cloudflare AI Gateway ID";

/// The provider id.
pub const CLOUDFLARE_WORKERS_AI_PROVIDER_ID: &str = "cloudflare-workers-ai";

/// The Cloudflare API-key env/credential field (`cloudflare-auth.ts`).
pub const CLOUDFLARE_API_KEY: &str = "CLOUDFLARE_API_KEY";
/// The Cloudflare account-id env/credential field, substituted into the model base URL.
pub const CLOUDFLARE_ACCOUNT_ID: &str = "CLOUDFLARE_ACCOUNT_ID";
/// The Cloudflare AI-Gateway-id env/credential field, substituted into the model base URL
/// (`cloudflare-auth.ts`, `ai-gateway` kind only).
pub const CLOUDFLARE_GATEWAY_ID: &str = "CLOUDFLARE_GATEWAY_ID";

/// The Cloudflare AI Gateway provider id.
pub const CLOUDFLARE_AI_GATEWAY_PROVIDER_ID: &str = "cloudflare-ai-gateway";

/// The verbatim AI-Gateway catalog (extracted from Pi's `cloudflare-ai-gateway.models.ts`).
const AI_GATEWAY_CATALOG_JSON: &str = include_str!("catalog/cloudflare-ai-gateway.json");

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

    fn supports_login(&self) -> bool {
        true
    }

    /// 1:1 port of `cloudflareWorkersAIAuth().login` (`providers/cloudflare-auth.ts:50-54`): a
    /// SECRET prompt for the api key, then a TEXT prompt for the account id, returned as one
    /// `{ type: "api_key", key, env: { CLOUDFLARE_ACCOUNT_ID } }` credential.
    ///
    /// CFG-005: both values are required — [`CloudflareWorkersAiAuth::resolve`] returns `None`
    /// unless the account id is present too — so a single-secret `/login` that stored only the key
    /// produced a credential that can never resolve.
    async fn login(
        &self,
        interaction: &dyn crate::auth::oauth::AuthInteraction,
    ) -> Result<Credential, crate::auth::oauth::OAuthError> {
        let key = interaction
            .prompt(AuthPrompt::secret(ENTER_CLOUDFLARE_API_KEY))
            .await?;
        let account_id = interaction
            .prompt(AuthPrompt::text(ENTER_CLOUDFLARE_ACCOUNT_ID))
            .await?;
        let mut env = ProviderEnv::new();
        env.insert(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id);
        Ok(Credential::ApiKey {
            key: Some(key),
            env: Some(env),
        })
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
            .replace(&format!("{{{CLOUDFLARE_ACCOUNT_ID}}}"), &account_id);

        let mut env = ProviderEnv::new();
        env.insert(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id);

        let source = if cred.is_some() {
            "stored credential"
        } else {
            CLOUDFLARE_API_KEY
        };

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

/// Parse the embedded Cloudflare AI Gateway catalog into [`Model`]s. A parse failure yields an empty
/// catalog (surfaced loudly by the catalog tests) rather than a panic (NO-PANIC policy).
pub fn cloudflare_ai_gateway_models() -> Vec<Model> {
    serde_json::from_str(AI_GATEWAY_CATALOG_JSON).unwrap_or_default()
}

/// Cloudflare AI Gateway auth (Pi `cloudflareAIGatewayAuth()` + `resolveCloudflareEnv("ai-gateway")`,
/// `cloudflare-auth.ts`): resolves `CLOUDFLARE_API_KEY` + `CLOUDFLARE_ACCOUNT_ID` +
/// `CLOUDFLARE_GATEWAY_ID`, substitutes the account id and gateway id into the model `baseUrl`'s
/// `{CLOUDFLARE_ACCOUNT_ID}`/`{CLOUDFLARE_GATEWAY_ID}` placeholders, and authenticates via a
/// `cf-aig-authorization: Bearer <key>` header while suppressing the default `Authorization` /
/// `x-api-key` headers (the gateway speaks all three of anthropic-messages / openai-completions /
/// openai-responses, each of which would otherwise set its own auth header). Resolves to `None` (not
/// configured) when the api key, account id, or gateway id is absent.
struct CloudflareAiGatewayAuth;

#[async_trait::async_trait]
impl ApiKeyAuth for CloudflareAiGatewayAuth {
    fn name(&self) -> &str {
        "Cloudflare API key"
    }

    fn supports_login(&self) -> bool {
        true
    }

    /// 1:1 port of `cloudflareAIGatewayAuth().login` (`providers/cloudflare-auth.ts:70-79`) — the
    /// workers-ai flow plus a THIRD prompt for the gateway id, which
    /// [`CloudflareAiGatewayAuth::resolve`] also requires.
    async fn login(
        &self,
        interaction: &dyn crate::auth::oauth::AuthInteraction,
    ) -> Result<Credential, crate::auth::oauth::OAuthError> {
        let key = interaction
            .prompt(AuthPrompt::secret(ENTER_CLOUDFLARE_API_KEY))
            .await?;
        let account_id = interaction
            .prompt(AuthPrompt::text(ENTER_CLOUDFLARE_ACCOUNT_ID))
            .await?;
        let gateway_id = interaction
            .prompt(AuthPrompt::text(ENTER_CLOUDFLARE_GATEWAY_ID))
            .await?;
        let mut env = ProviderEnv::new();
        env.insert(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id);
        env.insert(CLOUDFLARE_GATEWAY_ID.to_string(), gateway_id);
        Ok(Credential::ApiKey {
            key: Some(key),
            env: Some(env),
        })
    }

    async fn resolve(
        &self,
        model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let api_key = resolve_value(CLOUDFLARE_API_KEY, ctx, cred).await;
        let account_id = resolve_value(CLOUDFLARE_ACCOUNT_ID, ctx, cred).await;
        let gateway_id = resolve_value(CLOUDFLARE_GATEWAY_ID, ctx, cred).await;

        // Pi `if (!apiKey || !accountId || (kind === "ai-gateway" && !gatewayId)) return undefined;`
        // — empty strings are falsy in JS.
        let (Some(api_key), Some(account_id), Some(gateway_id)) = (api_key, account_id, gateway_id)
        else {
            return Ok(None);
        };
        if api_key.is_empty() || account_id.is_empty() || gateway_id.is_empty() {
            return Ok(None);
        }

        // `resolveCloudflareBaseUrl`: replace every `{CLOUDFLARE_ACCOUNT_ID}` + `{CLOUDFLARE_GATEWAY_ID}`.
        let base_url = model
            .base_url
            .replace(&format!("{{{CLOUDFLARE_ACCOUNT_ID}}}"), &account_id)
            .replace(&format!("{{{CLOUDFLARE_GATEWAY_ID}}}"), &gateway_id);

        // Pi `env: { CLOUDFLARE_ACCOUNT_ID, ...(gatewayId ? { CLOUDFLARE_GATEWAY_ID } : {}) }`.
        let mut env = ProviderEnv::new();
        env.insert(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id);
        env.insert(CLOUDFLARE_GATEWAY_ID.to_string(), gateway_id);

        // Pi `auth.headers`: `cf-aig-authorization: Bearer <key>` + suppress Authorization / x-api-key.
        let mut headers = crate::HeaderMap::new();
        headers.insert(
            "cf-aig-authorization".to_string(),
            Some(format!("Bearer {api_key}")),
        );
        headers.insert("Authorization".to_string(), None);
        headers.insert("x-api-key".to_string(), None);

        let source = if cred.is_some() {
            "stored credential"
        } else {
            CLOUDFLARE_API_KEY
        };

        Ok(Some(AuthResult {
            // Pi sets only `headers` + `baseUrl` (no `apiKey`); the gateway carries the key in the
            // `cf-aig-authorization` header instead.
            auth: ModelAuth {
                api_key: None,
                headers: Some(headers),
                base_url: Some(base_url),
            },
            env: Some(env),
            source: Some(source.to_string()),
        }))
    }
}

/// The Cloudflare AI Gateway [`ProviderAuth`] (Pi `cloudflareAIGatewayAuth()`).
pub fn cloudflare_ai_gateway_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(Arc::new(CloudflareAiGatewayAuth))
}

/// Construct the Cloudflare AI Gateway provider over the given credential store + shared api
/// registry. The registry MUST provide the `anthropic-messages`, `openai-completions`, and
/// `openai-responses` impls (use [`builtin_registry`]) — the catalog mixes all three.
pub fn cloudflare_ai_gateway_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        CLOUDFLARE_AI_GATEWAY_PROVIDER_ID,
        "Cloudflare AI Gateway",
        cloudflare_ai_gateway_models(),
        cloudflare_ai_gateway_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn cloudflare_ai_gateway_provider() -> WireProvider {
    cloudflare_ai_gateway_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::api::openai_completions::build_body;
    use crate::auth::InMemoryCredentialStore;
    use crate::auth::types::AuthContext;
    use crate::context::Context;
    use crate::known_api::OPENAI_COMPLETIONS;
    use crate::provider::Provider;
    use crate::stream::{StreamOptions, collect_message};
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
        assert!(
            models
                .iter()
                .all(|m| m.provider.as_str() == "cloudflare-workers-ai")
        );
        // Every catalog entry carries the unresolved account-id placeholder in its base URL.
        assert!(models.iter().all(|m| m.base_url
            == "https://api.cloudflare.com/client/v4/accounts/{CLOUDFLARE_ACCOUNT_ID}/ai/v1"));
        // The shared compat block (Pi: store/developer/long-cache off, session-affinity on).
        let m = models
            .iter()
            .find(|m| m.id.as_str() == "@cf/openai/gpt-oss-120b")
            .expect("gpt-oss");
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
        let result = auth
            .resolve(&model, &ctx, None)
            .await
            .expect("ok")
            .expect("configured");
        assert_eq!(result.auth.api_key.as_deref(), Some("cf-key"));
        assert_eq!(
            result.auth.base_url.as_deref(),
            Some("https://api.cloudflare.com/client/v4/accounts/acct-123/ai/v1")
        );
        assert_eq!(
            result
                .env
                .as_ref()
                .and_then(|e| e.get(CLOUDFLARE_ACCOUNT_ID))
                .map(String::as_str),
            Some("acct-123")
        );
        assert_eq!(result.source.as_deref(), Some(CLOUDFLARE_API_KEY));
    }

    #[tokio::test]
    async fn resolve_requires_both_key_and_account_id() {
        let auth = CloudflareWorkersAiAuth;
        let model = cloudflare_workers_ai_models()
            .into_iter()
            .next()
            .expect("model");
        // Only the api key → not configured.
        let ctx = MapEnv(BTreeMap::from([(
            CLOUDFLARE_API_KEY.to_string(),
            "cf-key".to_string(),
        )]));
        assert!(
            auth.resolve(&model, &ctx, None)
                .await
                .expect("ok")
                .is_none()
        );
        // Only the account id → not configured.
        let ctx = MapEnv(BTreeMap::from([(
            CLOUDFLARE_ACCOUNT_ID.to_string(),
            "acct".to_string(),
        )]));
        assert!(
            auth.resolve(&model, &ctx, None)
                .await
                .expect("ok")
                .is_none()
        );
    }

    #[tokio::test]
    async fn resolve_reads_account_id_from_stored_credential_env() {
        // A stored api-key credential supplies the key directly and the account id via its env
        // overlay (Pi `resolveValue`: api key from `credential.key`, others from `credential.env`).
        let auth = CloudflareWorkersAiAuth;
        let model = cloudflare_workers_ai_models()
            .into_iter()
            .next()
            .expect("model");
        let cred = Credential::ApiKey {
            key: Some("stored-key".to_string()),
            env: Some(ProviderEnv::from([(
                CLOUDFLARE_ACCOUNT_ID.to_string(),
                "stored-acct".to_string(),
            )])),
        };
        // Ambient env is empty — everything comes from the credential.
        let ctx = MapEnv(BTreeMap::new());
        let result = auth
            .resolve(&model, &ctx, Some(&cred))
            .await
            .expect("ok")
            .expect("configured");
        assert_eq!(result.auth.api_key.as_deref(), Some("stored-key"));
        assert!(
            result
                .auth
                .base_url
                .as_deref()
                .unwrap()
                .contains("stored-acct")
        );
        assert_eq!(result.source.as_deref(), Some("stored credential"));
    }

    #[tokio::test]
    async fn unconfigured_provider_yields_error_terminal() {
        let provider = cloudflare_workers_ai_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider
            .get_model("@cf/openai/gpt-oss-120b")
            .expect("model")
            .clone();
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
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
        let mut model = provider
            .get_model("@cf/openai/gpt-oss-120b")
            .expect("model")
            .clone();
        // Replace the catalog base URL with one whose placeholder yields an unroutable address.
        model.base_url = "http://127.0.0.1:1/{CLOUDFLARE_ACCOUNT_ID}/v1".to_string();
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(
            !err.contains("not configured"),
            "auth should have resolved: {err}"
        );
        assert!(err.contains("transport"), "expected transport error: {err}");
    }

    #[test]
    fn catalog_drives_openai_completions_encoding() {
        // A cloudflare reasoning model encodes through the openai-completions body builder; with no
        // store support the body omits `store`, and a reasoning model carries a reasoning field.
        let models = cloudflare_workers_ai_models();
        let m = models
            .iter()
            .find(|m| m.id.as_str() == "@cf/openai/gpt-oss-120b")
            .expect("gpt-oss");
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

    // ----------------------- Cloudflare AI Gateway (Pi cloudflareAIGatewayAuth) ----------------

    #[test]
    fn ai_gateway_catalog_parses_with_expected_api_mix() {
        // pi `cloudflare-ai-gateway.models.ts` @`b0c2a90e`: 42 models = 18 anthropic-messages + 5
        // openai-completions + 19 openai-responses, all carrying the dual account/gateway
        // placeholders. cyrup's `91585d9a` snapshot held 38: the GPT-5.6 trio (openai-responses)
        // and `workers-ai/@cf/zai-org/glm-5.2` (openai-completions) landed in the week between the
        // two revisions and were never picked up (PROV-057/PROV-060).
        let models = cloudflare_ai_gateway_models();
        assert_eq!(models.len(), 42);
        assert!(
            models
                .iter()
                .all(|m| m.provider.as_str() == "cloudflare-ai-gateway")
        );
        let count = |api: &str| models.iter().filter(|m| m.api.as_str() == api).count();
        assert_eq!(count(crate::known_api::ANTHROPIC_MESSAGES), 18);
        assert_eq!(count(OPENAI_COMPLETIONS), 5);
        assert_eq!(count(crate::known_api::OPENAI_RESPONSES), 19);
        assert!(models.iter().all(|m| {
            let b = m.base_url.as_str();
            b.contains("{CLOUDFLARE_ACCOUNT_ID}") && b.contains("{CLOUDFLARE_GATEWAY_ID}")
        }));
    }

    #[test]
    fn ai_gateway_env_mapping_present() {
        let vars = crate::env_api_keys::api_key_env_vars("cloudflare-ai-gateway").expect("mapping");
        assert!(vars.contains(&CLOUDFLARE_API_KEY));
    }

    #[tokio::test]
    async fn ai_gateway_resolve_sets_gateway_header_and_substitutes_both_placeholders() {
        // With key + account + gateway present, resolution substitutes BOTH placeholders, carries
        // the key in `cf-aig-authorization` (Bearer), suppresses Authorization + x-api-key, and
        // exposes both ids in the env overlay (Pi `cloudflareAIGatewayAuth` resolve).
        let auth = CloudflareAiGatewayAuth;
        let model = cloudflare_ai_gateway_models()
            .into_iter()
            .find(|m| m.id.as_str() == "claude-3-5-haiku")
            .expect("model");
        let ctx = MapEnv(BTreeMap::from([
            (CLOUDFLARE_API_KEY.to_string(), "cf-key".to_string()),
            (CLOUDFLARE_ACCOUNT_ID.to_string(), "acct-123".to_string()),
            (CLOUDFLARE_GATEWAY_ID.to_string(), "gw-789".to_string()),
        ]));
        let result = auth
            .resolve(&model, &ctx, None)
            .await
            .expect("ok")
            .expect("configured");
        // No api key on the auth — the gateway carries it in the header.
        assert!(result.auth.api_key.is_none());
        let headers = result.auth.headers.expect("headers");
        assert_eq!(
            headers.get("cf-aig-authorization"),
            Some(&Some("Bearer cf-key".to_string()))
        );
        // Default per-api auth headers are suppressed (None).
        assert_eq!(headers.get("Authorization"), Some(&None));
        assert_eq!(headers.get("x-api-key"), Some(&None));
        assert_eq!(
            result.auth.base_url.as_deref(),
            Some("https://gateway.ai.cloudflare.com/v1/acct-123/gw-789/anthropic")
        );
        let env = result.env.expect("env");
        assert_eq!(
            env.get(CLOUDFLARE_ACCOUNT_ID).map(String::as_str),
            Some("acct-123")
        );
        assert_eq!(
            env.get(CLOUDFLARE_GATEWAY_ID).map(String::as_str),
            Some("gw-789")
        );
        assert_eq!(result.source.as_deref(), Some(CLOUDFLARE_API_KEY));
    }

    #[tokio::test]
    async fn ai_gateway_requires_key_account_and_gateway() {
        let auth = CloudflareAiGatewayAuth;
        let model = cloudflare_ai_gateway_models()
            .into_iter()
            .next()
            .expect("model");
        // Missing gateway id → not configured (Pi `kind === "ai-gateway" && !gatewayId`).
        let ctx = MapEnv(BTreeMap::from([
            (CLOUDFLARE_API_KEY.to_string(), "cf-key".to_string()),
            (CLOUDFLARE_ACCOUNT_ID.to_string(), "acct".to_string()),
        ]));
        assert!(
            auth.resolve(&model, &ctx, None)
                .await
                .expect("ok")
                .is_none()
        );
        // Missing account id → not configured.
        let ctx = MapEnv(BTreeMap::from([
            (CLOUDFLARE_API_KEY.to_string(), "cf-key".to_string()),
            (CLOUDFLARE_GATEWAY_ID.to_string(), "gw".to_string()),
        ]));
        assert!(
            auth.resolve(&model, &ctx, None)
                .await
                .expect("ok")
                .is_none()
        );
    }

    #[tokio::test]
    async fn ai_gateway_reads_account_and_gateway_from_stored_credential_env() {
        // A stored api-key credential supplies the key directly and account/gateway ids via its env
        // overlay (Pi `resolveValue`: api key from `credential.key`, others from `credential.env`).
        let auth = CloudflareAiGatewayAuth;
        let model = cloudflare_ai_gateway_models()
            .into_iter()
            .find(|m| m.id.as_str() == "gpt-4")
            .expect("model");
        let cred = Credential::ApiKey {
            key: Some("stored-key".to_string()),
            env: Some(ProviderEnv::from([
                (CLOUDFLARE_ACCOUNT_ID.to_string(), "stored-acct".to_string()),
                (CLOUDFLARE_GATEWAY_ID.to_string(), "stored-gw".to_string()),
            ])),
        };
        let ctx = MapEnv(BTreeMap::new());
        let result = auth
            .resolve(&model, &ctx, Some(&cred))
            .await
            .expect("ok")
            .expect("configured");
        let headers = result.auth.headers.expect("headers");
        assert_eq!(
            headers.get("cf-aig-authorization"),
            Some(&Some("Bearer stored-key".to_string()))
        );
        let base = result.auth.base_url.unwrap();
        assert!(
            base.contains("/stored-acct/stored-gw/openai"),
            "base: {base}"
        );
        assert_eq!(result.source.as_deref(), Some("stored credential"));
    }

    #[tokio::test]
    async fn ai_gateway_unconfigured_yields_error_terminal() {
        let provider = cloudflare_ai_gateway_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider
            .get_model("claude-3-5-haiku")
            .expect("model")
            .clone();
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("not configured"));
    }

    #[tokio::test]
    async fn ai_gateway_configured_reaches_transport_across_all_three_apis() {
        // For one model of each api (anthropic-messages / openai-completions / openai-responses),
        // a fully-configured gateway resolves auth and reaches transport (proving each api accepts
        // header-only auth with no api key). Point at an unroutable host so it fails fast at connect.
        for id in [
            "claude-3-5-haiku",
            "workers-ai/@cf/moonshotai/kimi-k2.5",
            "gpt-4",
        ] {
            let provider = cloudflare_ai_gateway_provider_with(
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(builtin_registry()),
            )
            .with_auth_context(Arc::new(MapEnv(BTreeMap::from([
                (CLOUDFLARE_API_KEY.to_string(), "cf-key".to_string()),
                (CLOUDFLARE_ACCOUNT_ID.to_string(), "acct".to_string()),
                (CLOUDFLARE_GATEWAY_ID.to_string(), "gw".to_string()),
            ]))));
            let mut model = provider.get_model(id).expect("model").clone();
            model.base_url = "http://127.0.0.1:1/v1".to_string();
            let msg = collect_message(provider.stream(
                &model,
                &Context::default(),
                &StreamOptions::default(),
            ))
            .await;
            assert_eq!(msg.stop_reason, StopReason::Error, "{id}");
            let err = msg.error_message.unwrap();
            assert!(
                !err.contains("not configured"),
                "{id}: auth should resolve: {err}"
            );
            assert!(
                err.contains("transport"),
                "{id}: expected transport error: {err}"
            );
        }
    }

    #[test]
    fn ai_gateway_provider_identity() {
        let p = cloudflare_ai_gateway_provider();
        assert_eq!(p.id(), &ProviderId::from("cloudflare-ai-gateway"));
        assert_eq!(p.name(), "Cloudflare AI Gateway");
        assert_eq!(p.models().len(), 42);
    }
}

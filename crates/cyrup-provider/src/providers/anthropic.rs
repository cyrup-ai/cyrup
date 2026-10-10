//! The Anthropic provider (arch-01 §5). Speaks the
//! [`anthropic-messages`](crate::api::anthropic_messages) wire protocol; differs from other
//! Anthropic-compatible providers only in base URL, env key, and catalog. Mirrors Pi's
//! `providers/anthropic.ts` + the generated `anthropic.models.ts` catalog.
//!
//! Auth is a 1:1 port of `anthropicApiKeyAuth()` (`providers/anthropic.ts:9-36` @v0.83.0), NOT the
//! generic `envApiKeyAuth` helper: the order is stored credential → `ANTHROPIC_AUTH_TOKEN` as an
//! `Authorization: Bearer` header → `ANTHROPIC_OAUTH_TOKEN` → `ANTHROPIC_API_KEY` as a literal api
//! key. The Pi provider's OAuth *login* clause (`lazyOAuth`/`loadAnthropicOAuth`,
//! `providers/anthropic.ts:45`) is wired too — see
//! [`super::builtin_oauth::builtin_provider_oauth`] — and the api-key `login` (`:12-15`) is
//! implemented on [`AnthropicApiKeyAuth`]. Resolution and both login paths are live.

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::{CredentialStore, InMemoryCredentialStore, ProviderAuth, env_key};
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// Anthropic's API base URL (the `/v1/messages` path is appended by the wire impl).
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// The provider id.
pub const ANTHROPIC_PROVIDER_ID: &str = "anthropic";

/// The verbatim catalog extracted from Pi's generated `anthropic.models.ts`.
const ANTHROPIC_CATALOG_JSON: &str = include_str!("catalog/anthropic.json");

/// The full Anthropic catalog (1:1 with Pi `ANTHROPIC_MODELS`). A parse failure yields an empty
/// catalog (surfaced loudly by the catalog-count test) rather than a panic (NO-PANIC policy).
pub fn anthropic_models() -> Vec<Model> {
    crate::catalog::load_catalog(ANTHROPIC_CATALOG_JSON).unwrap_or_default()
}

/// `ANTHROPIC_AUTH_TOKEN` — the bearer-token variable used by Anthropic-compatible gateways and
/// proxies that authenticate with `Authorization: Bearer` rather than `x-api-key`
/// (Pi `ANTHROPIC_AUTH_TOKEN_ENV`, `env-api-keys.ts:29` @v0.83.0).
pub const ANTHROPIC_AUTH_TOKEN_ENV: &str = "ANTHROPIC_AUTH_TOKEN";
/// Pi `ANTHROPIC_OAUTH_TOKEN_ENV` (`env-api-keys.ts:30` @v0.83.0).
pub const ANTHROPIC_OAUTH_TOKEN_ENV: &str = "ANTHROPIC_OAUTH_TOKEN";
/// Pi `ANTHROPIC_API_KEY_ENV` (`env-api-keys.ts:31` @v0.83.0).
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";

/// The prompt text for the interactive api-key setup, verbatim from
/// `providers/anthropic.ts:14` @v0.83.0.
const ENTER_ANTHROPIC_API_KEY: &str = "Enter Anthropic API key";

/// 1:1 port of `anthropicApiKeyAuth()` (`pi/packages/ai/src/providers/anthropic.ts:9-36`
/// @v0.83.0).
///
/// PROV-021: the resolve order is stored credential, then **`ANTHROPIC_AUTH_TOKEN` as an
/// `Authorization: Bearer` header** (`:21-27`), then `ANTHROPIC_OAUTH_TOKEN`/`ANTHROPIC_API_KEY` as
/// a literal api key (`:29-32`). The bearer arm cannot go through [`env_key`], which resolves every
/// variable into `ModelAuth.api_key` and therefore into `x-api-key` — a header the gateways this
/// variable exists for do not read.
///
/// PROV-003: `login` is the api-key half pi declares at `:12-15`.
struct AnthropicApiKeyAuth;

#[async_trait::async_trait]
impl crate::auth::ApiKeyAuth for AnthropicApiKeyAuth {
    fn name(&self) -> &str {
        // Pi `name: "Anthropic API key"` (providers/anthropic.ts:11).
        "Anthropic API key"
    }

    fn supports_login(&self) -> bool {
        true
    }

    /// `login: async (interaction) => ({ type: "api_key", key: await interaction.prompt({ type:
    /// "secret", message: "Enter Anthropic API key" }) })` (`providers/anthropic.ts:12-15`).
    async fn login(
        &self,
        interaction: &dyn crate::auth::oauth::AuthInteraction,
    ) -> Result<crate::auth::Credential, crate::auth::oauth::OAuthError> {
        let key = interaction
            .prompt(crate::auth::AuthPrompt::secret(ENTER_ANTHROPIC_API_KEY))
            .await?;
        Ok(crate::auth::Credential::ApiKey {
            key: Some(key),
            env: None,
        })
    }

    async fn resolve(
        &self,
        _model: &Model,
        ctx: &dyn crate::auth::AuthContext,
        cred: Option<&crate::auth::Credential>,
    ) -> Result<Option<crate::auth::AuthResult>, crate::error::AuthError> {
        // `if (credential?.key) return { auth: { apiKey: credential.key }, env: credential.env,
        // source: "stored credential" }` (:17-19). A stored credential owns the provider; env is
        // not consulted.
        if let Some(cred) = cred {
            return Ok(match cred {
                crate::auth::Credential::ApiKey { key, env } => Some(crate::auth::AuthResult {
                    auth: crate::auth::ModelAuth {
                        api_key: key.clone(),
                        ..Default::default()
                    },
                    env: env.clone(),
                    source: Some("stored".to_string()),
                }),
                // An OAuth credential is not this api-key strategy's concern.
                crate::auth::Credential::Oauth { .. } => None,
            });
        }

        // `const authToken = await ctx.env(ANTHROPIC_AUTH_TOKEN_ENV); if (authToken) return { auth:
        // { headers: { Authorization: `Bearer ${authToken}` } }, source: ANTHROPIC_AUTH_TOKEN_ENV }`
        // (:21-27). Note it resolves BEFORE the other two and produces NO `apiKey`, so no
        // `x-api-key` is emitted.
        if let Some(token) = ctx.env(ANTHROPIC_AUTH_TOKEN_ENV).await
            && !token.is_empty()
        {
            let mut headers = crate::HeaderMap::new();
            headers.insert("Authorization".to_string(), Some(format!("Bearer {token}")));
            return Ok(Some(crate::auth::AuthResult {
                auth: crate::auth::ModelAuth {
                    api_key: None,
                    headers: Some(headers),
                    base_url: None,
                },
                env: None,
                source: Some(ANTHROPIC_AUTH_TOKEN_ENV.to_string()),
            }));
        }

        // `for (const envVar of [ANTHROPIC_OAUTH_TOKEN_ENV, ANTHROPIC_API_KEY_ENV]) { … }` (:29-32).
        for var in [ANTHROPIC_OAUTH_TOKEN_ENV, ANTHROPIC_API_KEY_ENV] {
            if let Some(val) = ctx.env(var).await
                && !val.is_empty()
            {
                return Ok(Some(crate::auth::AuthResult {
                    auth: crate::auth::ModelAuth {
                        api_key: Some(val),
                        ..Default::default()
                    },
                    env: None,
                    source: Some(var.to_string()),
                }));
            }
        }

        // PROV-119 — workload identity federation, resolved LAST so every key and the bearer token
        // keep winning (pi `providers/anthropic.ts:49-69` @v1.0.0: "Last in line so keys and
        // ANTHROPIC_AUTH_TOKEN keep winning, as in the SDK").
        //
        // The ids are provider CONFIG, not a credential: they travel in `env` and the exchange
        // happens in `api::anthropic_messages::federation`, which is also where the four-variable
        // activation gate is documented. `auth` is left EMPTY on purpose — a federated access token
        // is `sk-ant-oat01-…`, and putting one in `api_key` would flip `is_oauth` and rewrite every
        // tool name on the wire (see that module's header).
        let mut federation = crate::auth::types::ProviderEnv::new();
        for var in [
            crate::env_api_keys::ANTHROPIC_FEDERATION_RULE_ID_ENV,
            crate::env_api_keys::ANTHROPIC_ORGANIZATION_ID_ENV,
            crate::env_api_keys::ANTHROPIC_SERVICE_ACCOUNT_ID_ENV,
        ] {
            match ctx.env(var).await {
                Some(value) if !value.is_empty() => {
                    federation.insert(var.to_string(), value);
                }
                // pi returns `undefined` the moment a REQUIRED id is missing, so a partially
                // configured workload resolves to no auth at all rather than to a half-built
                // federation config.
                _ => return Ok(None),
            }
        }
        // One of the two token sources is required; `_FILE` wins when both are set.
        let token_vars = [
            crate::env_api_keys::ANTHROPIC_IDENTITY_TOKEN_FILE_ENV,
            crate::env_api_keys::ANTHROPIC_IDENTITY_TOKEN_ENV,
        ];
        let mut have_token = false;
        for var in token_vars {
            if let Some(value) = ctx.env(var).await
                && !value.is_empty()
            {
                federation.insert(var.to_string(), value);
                have_token = true;
                break;
            }
        }
        if !have_token {
            return Ok(None);
        }
        // Optional, and it does not gate activation.
        if let Some(value) = ctx
            .env(crate::env_api_keys::ANTHROPIC_WORKSPACE_ID_ENV)
            .await
            && !value.is_empty()
        {
            federation.insert(
                crate::env_api_keys::ANTHROPIC_WORKSPACE_ID_ENV.to_string(),
                value,
            );
        }
        Ok(Some(crate::auth::AuthResult {
            auth: crate::auth::ModelAuth::default(),
            env: Some(federation),
            source: Some("workload identity federation".to_string()),
        }))
    }
}

/// The Anthropic [`ProviderAuth`]: `anthropicApiKeyAuth()` (`providers/anthropic.ts:9-36`
/// @v0.83.0) — `ANTHROPIC_AUTH_TOKEN` as a bearer header, then `ANTHROPIC_OAUTH_TOKEN`, then
/// `ANTHROPIC_API_KEY` — **plus** the Claude Pro/Max OAuth login
/// (`lazyOAuth({ name: "Anthropic (Claude Pro/Max)", load: loadAnthropicOAuth })`, `:45`) — see
/// [`super::builtin_oauth::builtin_provider_oauth`].
pub fn anthropic_auth() -> ProviderAuth {
    ProviderAuth {
        api_key: Some(Arc::new(AnthropicApiKeyAuth)),
        oauth: super::builtin_oauth::builtin_provider_oauth(ANTHROPIC_PROVIDER_ID),
    }
}

/// Construct the Anthropic provider over the given credential store + shared api registry. The
/// registry MUST provide the `anthropic-messages` impl (use [`builtin_registry`]).
pub fn anthropic_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        ANTHROPIC_PROVIDER_ID,
        "Anthropic",
        anthropic_models(),
        anthropic_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry. Resolves
/// auth from `$ANTHROPIC_OAUTH_TOKEN`/`$ANTHROPIC_API_KEY` (or a stored credential / per-request
/// override).
pub fn anthropic_provider() -> WireProvider {
    anthropic_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
}

// ---------------------------------------------------------------------------
// The Anthropic-compatible provider fleet (Pi providers/{kimi-coding,minimax,minimax-cn,
// vercel-ai-gateway}.ts). Every one speaks the `anthropic-messages` wire protocol with a single
// env-key auth and a verbatim generated catalog; they differ only in id/name/base URL/env-key.
// ---------------------------------------------------------------------------

/// Static metadata for one `anthropic-messages` fleet provider (Pi provider factory `id`/`name`/
/// env-key, plus the embedded `<id>.models.ts` catalog).
pub struct AnthropicFleetSpec {
    pub id: &'static str,
    pub name: &'static str,
    /// API-key env var (matches `env-api-keys.ts`).
    pub env_var: &'static str,
    /// Upstream's `envApiKeyAuth(<name>, …)` first argument — the user-facing api-key method label
    /// (`ai/src/auth/helpers.ts:9`). Not derivable from `name`: `kimi-coding` is
    /// `"Kimi API key"`, not `"Kimi For Coding API key"` (`providers/kimi-coding.ts:13`).
    pub auth_name: &'static str,
    /// The verbatim JSON catalog (extracted from Pi's `<id>.models.ts`).
    pub catalog_json: &'static str,
}

macro_rules! anthropic_fleet {
    ($($id:literal => ($const:ident, $name:literal, $env:literal, $auth:literal, $file:literal)),* $(,)?) => {
        $(
            pub const $const: AnthropicFleetSpec = AnthropicFleetSpec {
                id: $id,
                name: $name,
                env_var: $env,
                auth_name: $auth,
                catalog_json: include_str!(concat!("catalog/", $file, ".json")),
            };
        )*
        /// Every Anthropic-compatible fleet spec.
        pub const ANTHROPIC_FLEET: &[AnthropicFleetSpec] = &[$($const),*];
    };
}

anthropic_fleet! {
    "kimi-coding"       => (KIMI_CODING, "Kimi For Coding", "KIMI_API_KEY", "Kimi API key", "kimi-coding"),
    "minimax"           => (MINIMAX, "MiniMax", "MINIMAX_API_KEY", "MiniMax API key", "minimax"),
    "minimax-cn"        => (MINIMAX_CN, "MiniMax CN", "MINIMAX_CN_API_KEY", "MiniMax CN API key", "minimax-cn"),
    "vercel-ai-gateway" => (VERCEL_AI_GATEWAY, "Vercel AI Gateway", "AI_GATEWAY_API_KEY", "Vercel AI Gateway API key", "vercel-ai-gateway"),
}

impl AnthropicFleetSpec {
    /// Parse the embedded catalog into [`Model`]s (verbatim from Pi; a parse failure yields an empty
    /// catalog, surfaced loudly by the count tests, rather than a panic).
    pub fn models(&self) -> Vec<Model> {
        crate::catalog::load_catalog(self.catalog_json).unwrap_or_default()
    }

    /// The provider's [`ProviderAuth`]: an API key from its env var (Pi `envApiKeyAuth`), plus the
    /// `lazyOAuth` clause for the one fleet member that has one — `kimi-coding`
    /// (`providers/kimi-coding.ts:14-19`). See [`super::builtin_oauth::builtin_provider_oauth`].
    pub fn auth(&self) -> ProviderAuth {
        ProviderAuth {
            api_key: Some(env_key(self.auth_name, [self.env_var])),
            oauth: super::builtin_oauth::builtin_provider_oauth(self.id),
        }
    }

    /// Build this provider over an explicit credential store + shared api registry.
    pub fn provider_with(
        &self,
        store: Arc<dyn CredentialStore>,
        registry: Arc<ApiRegistry>,
    ) -> WireProvider {
        WireProvider::new(
            self.id,
            self.name,
            self.models(),
            self.auth(),
            store,
            registry,
        )
    }

    /// Build this provider with an in-memory store + the built-in api registry.
    pub fn provider(&self) -> WireProvider {
        self.provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
    }
}

/// Look up an Anthropic fleet spec by provider id.
pub fn anthropic_fleet_spec(id: &str) -> Option<&'static AnthropicFleetSpec> {
    ANTHROPIC_FLEET.iter().find(|s| s.id == id)
}

/// Construct every Anthropic-compatible fleet provider over a shared store + registry.
pub fn anthropic_fleet_providers_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> Vec<WireProvider> {
    ANTHROPIC_FLEET
        .iter()
        .map(|s| s.provider_with(store.clone(), registry.clone()))
        .collect()
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
    use crate::api::compat::ModelCompat;
    use crate::auth::types::AuthContext;
    use crate::context::Context;
    use crate::known_api::ANTHROPIC_MESSAGES;
    use crate::provider::Provider;
    use crate::stream::{StreamOptions, collect_message};
    use cyrup_core::StopReason;
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
    fn catalog_parses_verbatim_with_expected_count() {
        let models = anthropic_models();
        // Every entry in pi's `anthropic.models.ts`, live from
        // `pi.dev/api/models/providers/anthropic` since PROV-071. 14 while the catalog was frozen
        // at `b0c2a90e`; the refresh added Opus 5, Opus 5.5, Sonnet 5.5 and Fable 5.1 and retired
        // the two Opus 4.1 rows, which is two months of Anthropic releases the frozen floor could
        // not show. 17 since PROV-149's refresh added Claude Haiku 5.5 (pi `f76c1db66`).
        assert_eq!(models.len(), 17);
        assert!(models.iter().all(|m| m.api.as_str() == ANTHROPIC_MESSAGES));
        assert!(models.iter().all(|m| m.provider.as_str() == "anthropic"));
        assert!(models.iter().all(|m| m.base_url == ANTHROPIC_BASE_URL));
        // All Claude models accept image input.
        assert!(models.iter().all(|m| m.supports_image_input()));
    }

    #[test]
    fn flagship_compat_and_thinking_map_deserialize() {
        let models = anthropic_models();
        let find = |id: &str| {
            models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };

        // Opus 4.7: adaptive thinking + temperature unsupported + xhigh->xhigh AND max->max
        // (pi anthropic.models.ts:155 @91585d9a — `{"xhigh":"xhigh","max":"max"}`).
        let opus47 = find("claude-opus-4-7");
        let c: &ModelCompat = opus47.compat.as_ref().expect("compat");
        assert_eq!(c.force_adaptive_thinking, Some(true));
        assert_eq!(c.supports_temperature, Some(false));
        let m47 = opus47.thinking_level_map.as_ref().expect("opus-4-7 map");
        assert_eq!(m47.get("xhigh"), Some(&Some("xhigh".to_string())));
        assert_eq!(m47.get("max"), Some(&Some("max".to_string())));

        // Opus 4.6: adaptive, and the ONLY native rung is `max` (pi anthropic.models.ts:136
        // @91585d9a — `{"max":"max"}`). cyrup used to carry `{"xhigh":"max"}`, which made the
        // selector display `xhigh` while the wire effort was `max`; the label and the wire value
        // must now agree.
        let opus46 = find("claude-opus-4-6");
        assert_eq!(
            opus46
                .compat
                .as_ref()
                .and_then(|c| c.force_adaptive_thinking),
            Some(true)
        );
        let m46 = opus46.thinking_level_map.as_ref().expect("opus-4-6 map");
        assert_eq!(m46.get("max"), Some(&Some("max".to_string())));
        assert_eq!(m46.get("xhigh"), None, "4.6 has no native xhigh rung");

        // Opus 4.5: pricing + context window.
        let opus45 = find("claude-opus-4-5");
        assert_eq!(opus45.cost.input, 5.0);
        assert_eq!(opus45.cost.output, 25.0);
        assert_eq!(opus45.context_window, 200_000);
        assert_eq!(opus45.max_tokens, 64_000);

        // Sonnet 4.5: 1M from `cc2db980` (pi anthropic.models.ts:185 @91585d9a) until models.dev
        // moved it back to 200k, which PROV-149's refresh picked up from pi.dev.
        assert_eq!(find("claude-sonnet-4-5").context_window, 200_000);

        // After `cc2db980` retired the Claude 3.x entries, every remaining Anthropic model is a
        // reasoning model — there is no non-reasoning Claude left to pin.
        assert!(models.iter().all(|m| m.reasoning));
    }

    #[test]
    fn provider_identity() {
        let p = anthropic_provider();
        assert_eq!(p.id().as_str(), "anthropic");
        assert!(p.get_model("claude-opus-4-5").is_some());
        assert_eq!(p.models().len(), 17);
    }

    #[test]
    fn anthropic_fleet_catalogs_parse_with_expected_counts() {
        let expected = [
            ("kimi-coding", 4usize),
            ("minimax", 3),
            ("minimax-cn", 3),
            ("vercel-ai-gateway", 252),
        ];
        for (id, count) in expected {
            let spec = anthropic_fleet_spec(id).unwrap_or_else(|| panic!("no spec for {id}"));
            let models = spec.models();
            assert_eq!(models.len(), count, "catalog count mismatch for {id}");
            assert!(
                models.iter().all(|m| m.api.as_str() == ANTHROPIC_MESSAGES),
                "{id} api"
            );
            assert!(
                models.iter().all(|m| m.provider.as_str() == id),
                "{id} provider tag"
            );
            assert!(
                models.iter().all(|m| !m.base_url.is_empty()),
                "{id} baseUrl"
            );
            // Every fleet provider has an env-key mapping in env-api-keys.
            let vars = crate::env_api_keys::api_key_env_vars(id)
                .unwrap_or_else(|| panic!("no env mapping for {id}"));
            assert!(vars.contains(&spec.env_var), "{id} env var mismatch");
        }
    }

    #[test]
    fn vercel_gateway_flagship_compat_deserializes() {
        // The Vercel AI Gateway catalog includes adaptive-thinking Claude models whose compat block
        // carries `forceAdaptiveThinking`/`supportsTemperature` — proving the union compat reaches
        // the anthropic-messages catalog path.
        let models = VERCEL_AI_GATEWAY.models();
        let adaptive = models
            .iter()
            .find(|m| m.compat.as_ref().and_then(|c| c.force_adaptive_thinking) == Some(true));
        assert!(
            adaptive.is_some(),
            "expected at least one adaptive-thinking model"
        );
    }

    #[tokio::test]
    async fn unconfigured_without_env_yields_error_terminal() {
        let provider = anthropic_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider.get_model("claude-opus-4-5").unwrap().clone();
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
    async fn resolves_auth_then_fails_at_transport() {
        let env = MapEnv(BTreeMap::from([(
            "ANTHROPIC_API_KEY".to_string(),
            "sk-ant-api03-test".to_string(),
        )]));
        let provider = anthropic_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(env));
        let mut model = provider.get_model("claude-opus-4-5").unwrap().clone();
        model.base_url = "http://127.0.0.1:1".to_string();
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
            "auth should have resolved, got: {err}"
        );
        assert!(
            err.contains("transport"),
            "expected transport error, got: {err}"
        );
    }

    /// Live smoke test against the real Anthropic API. Ignored by default; run with
    /// `ANTHROPIC_API_KEY` set: `cargo test -p cyrup-provider -- --ignored live_anthropic`.
    #[tokio::test]
    #[ignore = "hits the real Anthropic API; requires ANTHROPIC_API_KEY"]
    async fn live_anthropic_returns_non_empty_done() {
        use cyrup_core::{Content, Message};
        if std::env::var("ANTHROPIC_API_KEY").is_err() {
            eprintln!("skipping: ANTHROPIC_API_KEY not set");
            return;
        }
        let provider = anthropic_provider();
        let model = provider.get_model("claude-haiku-4-5").unwrap().clone();
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text("Reply with exactly: pong")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let opts = StreamOptions {
            max_tokens: Some(64),
            ..Default::default()
        };
        let msg = collect_message(provider.stream(&model, &ctx, &opts)).await;
        assert_ne!(
            msg.stop_reason,
            StopReason::Error,
            "got error: {:?}",
            msg.error_message
        );
        let has_text = msg
            .content
            .iter()
            .any(|c| matches!(c, Content::Text { text, .. } if !text.trim().is_empty()));
        assert!(has_text, "expected non-empty text, got: {:?}", msg.content);
    }

    /// PROV-021. `ANTHROPIC_AUTH_TOKEN` resolves FIRST and travels as `Authorization: Bearer`,
    /// never as `x-api-key` (`providers/anthropic.ts:21-27` @v0.83.0). Red before the fix:
    /// `anthropic_auth()` used the generic `env_key(["ANTHROPIC_OAUTH_TOKEN","ANTHROPIC_API_KEY"])`
    /// helper, so the variable was unknown to the crate and a user with only it set got
    /// "not configured" in an environment where pi works.
    #[tokio::test]
    async fn anthropic_auth_token_resolves_as_a_bearer_header() {
        let auth = anthropic_auth();
        let strategy = auth.api_key.expect("anthropic has an api-key strategy");
        let m = anthropic_models().first().cloned().expect("catalog");

        let env = |pairs: &[(&str, &str)]| {
            MapEnv(
                pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            )
        };

        // Only the bearer token: `Authorization: Bearer t`, and NO api key.
        let r = strategy
            .resolve(&m, &env(&[("ANTHROPIC_AUTH_TOKEN", "t")]), None)
            .await
            .expect("resolve")
            .expect("configured");
        assert_eq!(r.auth.api_key, None, "must not become x-api-key");
        assert_eq!(
            r.auth.headers.as_ref().and_then(|h| h.get("Authorization")),
            Some(&Some("Bearer t".to_string()))
        );
        assert_eq!(r.source.as_deref(), Some("ANTHROPIC_AUTH_TOKEN"));

        // Both set: the bearer wins (it is tested before the loop).
        let r = strategy
            .resolve(
                &m,
                &env(&[("ANTHROPIC_AUTH_TOKEN", "t"), ("ANTHROPIC_API_KEY", "k")]),
                None,
            )
            .await
            .expect("resolve")
            .expect("configured");
        assert_eq!(r.auth.api_key, None);

        // Only the api key: unchanged behaviour.
        let r = strategy
            .resolve(&m, &env(&[("ANTHROPIC_API_KEY", "k")]), None)
            .await
            .expect("resolve")
            .expect("configured");
        assert_eq!(r.auth.api_key.as_deref(), Some("k"));
        assert!(r.auth.headers.is_none());
    }

    /// PROV-021, discovery half: all three variables are reported by `api_key_env_vars`
    /// (`env-api-keys.ts:73-76`), but `get_env_api_key` SKIPS `ANTHROPIC_AUTH_TOKEN` (`:147`) —
    /// turning it into a literal api key would send it as `x-api-key`.
    #[tokio::test]
    async fn anthropic_auth_token_is_discoverable_but_never_a_literal_key() {
        assert_eq!(
            crate::env_api_keys::api_key_env_vars("anthropic"),
            Some(
                &[
                    "ANTHROPIC_AUTH_TOKEN",
                    "ANTHROPIC_OAUTH_TOKEN",
                    "ANTHROPIC_API_KEY"
                ][..]
            )
        );
        let only_bearer = MapEnv(
            [("ANTHROPIC_AUTH_TOKEN".to_string(), "t".to_string())]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            crate::env_api_keys::get_env_api_key("anthropic", &only_bearer, None).await,
            None,
            "the bearer token must never be returned as an api key"
        );
        let both = MapEnv(
            [
                ("ANTHROPIC_AUTH_TOKEN".to_string(), "t".to_string()),
                ("ANTHROPIC_API_KEY".to_string(), "k".to_string()),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            crate::env_api_keys::get_env_api_key("anthropic", &both, None).await,
            Some("k".to_string())
        );
    }

    // ---- PROV-119: federation resolution order --------------------------------------------

    /// Every federation variable, so the arm's own gate is the only thing under test.
    fn federation_env() -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                crate::env_api_keys::ANTHROPIC_FEDERATION_RULE_ID_ENV.to_string(),
                "fdrl_rule".to_string(),
            ),
            (
                crate::env_api_keys::ANTHROPIC_ORGANIZATION_ID_ENV.to_string(),
                "org-uuid".to_string(),
            ),
            (
                crate::env_api_keys::ANTHROPIC_SERVICE_ACCOUNT_ID_ENV.to_string(),
                "svac_account".to_string(),
            ),
            (
                crate::env_api_keys::ANTHROPIC_IDENTITY_TOKEN_FILE_ENV.to_string(),
                "/var/run/secrets/anthropic.com/token".to_string(),
            ),
        ])
    }

    async fn resolve_with(env: BTreeMap<String, String>) -> Option<crate::auth::AuthResult> {
        let model = anthropic_models()
            .into_iter()
            .find(|m| m.id.as_str() == "claude-opus-4-5")
            .expect("catalog model");
        crate::auth::ApiKeyAuth::resolve(&AnthropicApiKeyAuth, &model, &MapEnv(env), None)
            .await
            .expect("resolution must not error")
    }

    /// The ids arrive in `env` and NOT as a credential: `auth` stays empty, because the exchange
    /// happens later and a federated token must never land in `api_key` (see
    /// `api::anthropic_messages::federation`).
    #[tokio::test]
    async fn federation_resolves_the_ids_into_env_with_no_credential() {
        let resolved = resolve_with(federation_env())
            .await
            .expect("federation applies");
        assert_eq!(
            resolved.source.as_deref(),
            Some("workload identity federation")
        );
        assert_eq!(
            resolved.auth.api_key, None,
            "federation yields config, not a key"
        );
        assert!(
            resolved.auth.headers.is_none(),
            "no header is minted at resolution time"
        );
        let env = resolved.env.expect("the ids travel in env");
        assert_eq!(
            env.get(crate::env_api_keys::ANTHROPIC_FEDERATION_RULE_ID_ENV)
                .map(String::as_str),
            Some("fdrl_rule")
        );
        assert_eq!(
            env.get(crate::env_api_keys::ANTHROPIC_SERVICE_ACCOUNT_ID_ENV)
                .map(String::as_str),
            Some("svac_account")
        );
        assert_eq!(
            env.get(crate::env_api_keys::ANTHROPIC_IDENTITY_TOKEN_FILE_ENV)
                .map(String::as_str),
            Some("/var/run/secrets/anthropic.com/token")
        );
    }

    /// Federation is LAST: each of the three key variables wins over a complete federation
    /// configuration (pi: "Last in line so keys and ANTHROPIC_AUTH_TOKEN keep winning").
    #[tokio::test]
    async fn every_key_variable_wins_over_federation() {
        for (var, expected_source) in [
            (ANTHROPIC_AUTH_TOKEN_ENV, ANTHROPIC_AUTH_TOKEN_ENV),
            (ANTHROPIC_OAUTH_TOKEN_ENV, ANTHROPIC_OAUTH_TOKEN_ENV),
            (ANTHROPIC_API_KEY_ENV, ANTHROPIC_API_KEY_ENV),
        ] {
            let mut env = federation_env();
            env.insert(var.to_string(), "secret-value".to_string());
            let resolved = resolve_with(env).await.expect("a key resolves");
            assert_eq!(
                resolved.source.as_deref(),
                Some(expected_source),
                "{var} must win over federation"
            );
            assert!(
                resolved.env.is_none(),
                "{var} must not also carry the federation ids"
            );
        }
    }

    /// A partially configured workload resolves to NOTHING rather than to a half-built federation
    /// config, so the failure is "not configured" and not an opaque 401 from the exchange.
    #[tokio::test]
    async fn a_partial_federation_configuration_resolves_to_nothing() {
        for missing in [
            crate::env_api_keys::ANTHROPIC_FEDERATION_RULE_ID_ENV,
            crate::env_api_keys::ANTHROPIC_ORGANIZATION_ID_ENV,
            crate::env_api_keys::ANTHROPIC_SERVICE_ACCOUNT_ID_ENV,
            crate::env_api_keys::ANTHROPIC_IDENTITY_TOKEN_FILE_ENV,
        ] {
            let mut env = federation_env();
            env.remove(missing);
            assert!(
                resolve_with(env).await.is_none(),
                "federation must not activate without {missing}"
            );
        }
    }

    /// `ANTHROPIC_IDENTITY_TOKEN` stands in for the file (CYRUP-DELTA: pi reads only the file).
    #[tokio::test]
    async fn the_inline_identity_token_also_activates_federation() {
        let mut env = federation_env();
        env.remove(crate::env_api_keys::ANTHROPIC_IDENTITY_TOKEN_FILE_ENV);
        env.insert(
            crate::env_api_keys::ANTHROPIC_IDENTITY_TOKEN_ENV.to_string(),
            "header.payload.signature".to_string(),
        );
        let resolved = resolve_with(env)
            .await
            .expect("the inline token activates federation");
        let carried = resolved.env.expect("ids in env");
        assert_eq!(
            carried
                .get(crate::env_api_keys::ANTHROPIC_IDENTITY_TOKEN_ENV)
                .map(String::as_str),
            Some("header.payload.signature")
        );
    }

    /// END TO END: resolve → exchange → the minted token on the actual `/v1/messages` request.
    ///
    /// This is the test that proves the feature is not inert. The unit tests cover the gate and the
    /// exchange separately; this one drives `provider.stream()` with only federation variables set
    /// and asserts that (a) the request is NOT rejected as "not configured", (b) the exchange was
    /// performed, and (c) the messages request carries `Authorization: Bearer <minted>` and no
    /// `x-api-key`.
    #[tokio::test]
    async fn a_federated_request_carries_the_minted_token_and_no_api_key() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let dir = std::env::temp_dir().join(format!(
            "cyrup-prov119-e2e-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let token_path = dir.join("token");
        std::fs::write(&token_path, "header.payload.signature").expect("write token");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let seen = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
        let recorded = std::sync::Arc::clone(&seen);
        tokio::spawn(async move {
            // 1: the token exchange. 2: the messages request, answered 500 so the turn ends fast.
            for i in 0..2 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = vec![0u8; 16384];
                let read = socket.read(&mut buf).await.unwrap_or(0);
                recorded
                    .lock()
                    .await
                    .push(String::from_utf8_lossy(&buf[..read]).to_string());
                let body = if i == 0 {
                    r#"{"access_token":"sk-ant-oat01-federated","token_type":"Bearer","expires_in":3600,"scope":"workspace:inference"}"#
                } else {
                    r#"{"type":"error","error":{"type":"api_error","message":"stop here"}}"#
                };
                let status = if i == 0 {
                    "200 OK"
                } else {
                    "500 Internal Server Error"
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });

        let mut env = federation_env();
        env.insert(
            crate::env_api_keys::ANTHROPIC_IDENTITY_TOKEN_FILE_ENV.to_string(),
            token_path.to_string_lossy().to_string(),
        );
        let provider = anthropic_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(env)));
        let mut model = provider.get_model("claude-opus-4-5").unwrap().clone();
        model.base_url = format!("http://127.0.0.1:{port}");

        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;

        let err = msg.error_message.unwrap_or_default();
        assert!(
            !err.contains("not configured"),
            "federation must resolve as configured auth, got: {err}"
        );

        let requests = seen.lock().await.clone();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            requests.len(),
            2,
            "expected the exchange and then the messages request, got {} — last: {:?}",
            requests.len(),
            requests.last()
        );
        assert!(
            requests[0].starts_with("POST /v1/oauth/token "),
            "the first call must be the token exchange:\n{}",
            requests[0]
        );
        let messages_request = &requests[1];
        assert!(
            messages_request.starts_with("POST /v1/messages "),
            "the second call must be the messages request:\n{messages_request}"
        );
        let lower = messages_request.to_ascii_lowercase();
        assert!(
            lower.contains("authorization: bearer sk-ant-oat01-federated"),
            "the minted token must be on the messages request:\n{messages_request}"
        );
        assert!(
            !lower.contains("x-api-key"),
            "a federated request must send no x-api-key:\n{messages_request}"
        );
    }
}

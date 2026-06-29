//! The Anthropic provider (arch-01 §5). Speaks the
//! [`anthropic-messages`](crate::api::anthropic_messages) wire protocol; differs from other
//! Anthropic-compatible providers only in base URL, env key, and catalog. Mirrors Pi's
//! `providers/anthropic.ts` + the generated `anthropic.models.ts` catalog.
//!
//! Auth: `ANTHROPIC_OAUTH_TOKEN` takes precedence over `ANTHROPIC_API_KEY` (Pi
//! `envApiKeyAuth("Anthropic API key", ["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"])`). The Pi
//! provider additionally wires an OAuth *login* flow (`lazyOAuth`/`loadAnthropicOAuth`); that login
//! subsystem is tracked as a separate blocker (it needs a local callback HTTP server + PKCE) — the
//! resolution path (explicit → stored → env) is fully wired here.

use crate::api::{builtin_registry, ApiRegistry};
use crate::auth::{env_key, CredentialStore, InMemoryCredentialStore, ProviderAuth};
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
    serde_json::from_str(ANTHROPIC_CATALOG_JSON).unwrap_or_default()
}

/// The Anthropic [`ProviderAuth`]: `ANTHROPIC_OAUTH_TOKEN` then `ANTHROPIC_API_KEY` (Pi
/// `envApiKeyAuth`, env-api-keys.ts:70).
pub fn anthropic_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key(["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]))
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
    /// The verbatim JSON catalog (extracted from Pi's `<id>.models.ts`).
    pub catalog_json: &'static str,
}

macro_rules! anthropic_fleet {
    ($($id:literal => ($const:ident, $name:literal, $env:literal, $file:literal)),* $(,)?) => {
        $(
            pub const $const: AnthropicFleetSpec = AnthropicFleetSpec {
                id: $id,
                name: $name,
                env_var: $env,
                catalog_json: include_str!(concat!("catalog/", $file, ".json")),
            };
        )*
        /// Every Anthropic-compatible fleet spec.
        pub const ANTHROPIC_FLEET: &[AnthropicFleetSpec] = &[$($const),*];
    };
}

anthropic_fleet! {
    "kimi-coding"       => (KIMI_CODING, "Kimi Coding", "KIMI_API_KEY", "kimi-coding"),
    "minimax"           => (MINIMAX, "MiniMax", "MINIMAX_API_KEY", "minimax"),
    "minimax-cn"        => (MINIMAX_CN, "MiniMax CN", "MINIMAX_CN_API_KEY", "minimax-cn"),
    "vercel-ai-gateway" => (VERCEL_AI_GATEWAY, "Vercel AI Gateway", "AI_GATEWAY_API_KEY", "vercel-ai-gateway"),
}

impl AnthropicFleetSpec {
    /// Parse the embedded catalog into [`Model`]s (verbatim from Pi; a parse failure yields an empty
    /// catalog, surfaced loudly by the count tests, rather than a panic).
    pub fn models(&self) -> Vec<Model> {
        serde_json::from_str(self.catalog_json).unwrap_or_default()
    }

    /// The provider's [`ProviderAuth`]: an API key from its env var (Pi `envApiKeyAuth`).
    pub fn auth(&self) -> ProviderAuth {
        ProviderAuth::with_api_key(env_key([self.env_var]))
    }

    /// Build this provider over an explicit credential store + shared api registry.
    pub fn provider_with(
        &self,
        store: Arc<dyn CredentialStore>,
        registry: Arc<ApiRegistry>,
    ) -> WireProvider {
        WireProvider::new(self.id, self.name, self.models(), self.auth(), store, registry)
    }

    /// Build this provider with an in-memory store + the built-in api registry.
    pub fn provider(&self) -> WireProvider {
        self.provider_with(Arc::new(InMemoryCredentialStore::new()), Arc::new(builtin_registry()))
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
    ANTHROPIC_FLEET.iter().map(|s| s.provider_with(store.clone(), registry.clone())).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::api::compat::ModelCompat;
    use crate::auth::types::AuthContext;
    use crate::context::Context;
    use crate::known_api::ANTHROPIC_MESSAGES;
    use crate::provider::Provider;
    use crate::stream::{collect_message, StreamOptions};
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
        // Every entry in Pi's `anthropic.models.ts` (25 models).
        assert_eq!(models.len(), 25);
        assert!(models.iter().all(|m| m.api.as_str() == ANTHROPIC_MESSAGES));
        assert!(models.iter().all(|m| m.provider.as_str() == "anthropic"));
        assert!(models.iter().all(|m| m.base_url.as_deref() == Some(ANTHROPIC_BASE_URL)));
        // All Claude models accept image input.
        assert!(models.iter().all(|m| m.supports_image_input()));
    }

    #[test]
    fn flagship_compat_and_thinking_map_deserialize() {
        let models = anthropic_models();
        let find = |id: &str| {
            models.iter().find(|m| m.id.as_str() == id).unwrap_or_else(|| panic!("missing {id}"))
        };

        // Opus 4.7: adaptive thinking + temperature unsupported + xhigh->xhigh map.
        let opus47 = find("claude-opus-4-7");
        let c: &ModelCompat = opus47.compat.as_ref().expect("compat");
        assert_eq!(c.force_adaptive_thinking, Some(true));
        assert_eq!(c.supports_temperature, Some(false));
        assert_eq!(
            opus47.thinking_level_map.as_ref().and_then(|m| m.get("xhigh")),
            Some(&Some("xhigh".to_string()))
        );

        // Opus 4.6: adaptive + xhigh->"max".
        let opus46 = find("claude-opus-4-6");
        assert_eq!(opus46.compat.as_ref().and_then(|c| c.force_adaptive_thinking), Some(true));
        assert_eq!(
            opus46.thinking_level_map.as_ref().and_then(|m| m.get("xhigh")),
            Some(&Some("max".to_string()))
        );

        // Opus 4.5: pricing + context window.
        let opus45 = find("claude-opus-4-5");
        assert_eq!(opus45.cost.input, 5.0);
        assert_eq!(opus45.cost.output, 25.0);
        assert_eq!(opus45.context_window, 200_000);
        assert_eq!(opus45.max_tokens, 64_000);

        // Haiku 3.5 is a non-reasoning model.
        assert!(!find("claude-3-5-haiku-latest").reasoning);
    }

    #[test]
    fn provider_identity() {
        let p = anthropic_provider();
        assert_eq!(p.id().as_str(), "anthropic");
        assert!(p.get_model("claude-opus-4-5").is_some());
        assert!(p.models().len() >= 20);
    }

    #[test]
    fn anthropic_fleet_catalogs_parse_with_expected_counts() {
        let expected = [
            ("kimi-coding", 3usize),
            ("minimax", 3),
            ("minimax-cn", 3),
            ("vercel-ai-gateway", 185),
        ];
        for (id, count) in expected {
            let spec = anthropic_fleet_spec(id).unwrap_or_else(|| panic!("no spec for {id}"));
            let models = spec.models();
            assert_eq!(models.len(), count, "catalog count mismatch for {id}");
            assert!(models.iter().all(|m| m.api.as_str() == ANTHROPIC_MESSAGES), "{id} api");
            assert!(models.iter().all(|m| m.provider.as_str() == id), "{id} provider tag");
            assert!(models.iter().all(|m| m.base_url.is_some()), "{id} baseUrl");
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
        let adaptive = models.iter().find(|m| {
            m.compat.as_ref().and_then(|c| c.force_adaptive_thinking) == Some(true)
        });
        assert!(adaptive.is_some(), "expected at least one adaptive-thinking model");
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
        model.base_url = Some("http://127.0.0.1:1".to_string());
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(!err.contains("not configured"), "auth should have resolved, got: {err}");
        assert!(err.contains("transport"), "expected transport error, got: {err}");
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
        let opts = StreamOptions { max_tokens: Some(64), ..Default::default() };
        let msg = collect_message(provider.stream(&model, &ctx, &opts)).await;
        assert_ne!(msg.stop_reason, StopReason::Error, "got error: {:?}", msg.error_message);
        let has_text = msg.content.iter().any(|c| matches!(c, Content::Text { text, .. } if !text.trim().is_empty()));
        assert!(has_text, "expected non-empty text, got: {:?}", msg.content);
    }
}

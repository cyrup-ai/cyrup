//! The Fireworks provider (arch-01 §5). A **mixed-API** provider: its catalog carries both
//! [`anthropic-messages`](crate::api::anthropic_messages) and
//! [`openai-completions`](crate::api::openai_completions) models, each routed to its own `ApiImpl`
//! per request via the shared [`ApiRegistry`]. Mirrors Pi's `providers/fireworks.ts` +
//! the generated `fireworks.models.ts` catalog (both already-implemented wire protocols — no new
//! API or dependency).

use crate::api::{builtin_registry, ApiRegistry};
use crate::auth::{env_key, CredentialStore, InMemoryCredentialStore, ProviderAuth};
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// Fireworks inference base URL (per-model `baseUrl` overrides distinguish the
/// `anthropic-messages` root from the `openai-completions` `/v1` root).
pub const FIREWORKS_BASE_URL: &str = "https://api.fireworks.ai/inference";

/// The provider id.
pub const FIREWORKS_PROVIDER_ID: &str = "fireworks";

/// The env var carrying the Fireworks API key (Pi `envApiKeyAuth("Fireworks API key",
/// ["FIREWORKS_API_KEY"])`, fireworks.ts:11).
pub const FIREWORKS_API_KEY_ENV: &str = "FIREWORKS_API_KEY";

/// The verbatim catalog extracted from Pi's generated `fireworks.models.ts`.
const FIREWORKS_CATALOG_JSON: &str = include_str!("catalog/fireworks.json");

/// The full Fireworks catalog (1:1 with Pi `FIREWORKS_MODELS`). A parse failure yields an empty
/// catalog (surfaced loudly by the count test) rather than a panic (NO-PANIC policy).
pub fn fireworks_models() -> Vec<Model> {
    serde_json::from_str(FIREWORKS_CATALOG_JSON).unwrap_or_default()
}

/// The Fireworks [`ProviderAuth`]: an API key from `$FIREWORKS_API_KEY` (Pi `envApiKeyAuth`).
pub fn fireworks_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key([FIREWORKS_API_KEY_ENV]))
}

/// Construct the Fireworks provider over the given credential store + shared api registry. The
/// registry MUST provide BOTH the `anthropic-messages` and `openai-completions` impls (use
/// [`builtin_registry`]).
pub fn fireworks_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        FIREWORKS_PROVIDER_ID,
        "Fireworks",
        fireworks_models(),
        fireworks_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn fireworks_provider() -> WireProvider {
    fireworks_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::known_api::{ANTHROPIC_MESSAGES, OPENAI_COMPLETIONS};
    use crate::provider::Provider;

    #[test]
    fn catalog_parses_verbatim_with_expected_count() {
        let models = fireworks_models();
        // Every entry in Pi's `fireworks.models.ts` (15 models).
        assert_eq!(models.len(), 15);
        assert!(models.iter().all(|m| m.provider.as_str() == "fireworks"));
        // Mixed-API: most models are anthropic-messages, glm-5p2 is openai-completions.
        assert!(models.iter().any(|m| m.api.as_str() == ANTHROPIC_MESSAGES));
        assert!(models.iter().any(|m| m.api.as_str() == OPENAI_COMPLETIONS));
    }

    #[test]
    fn anthropic_and_openai_models_route_per_api() {
        let models = fireworks_models();
        let find = |id: &str| {
            models.iter().find(|m| m.id.as_str() == id).unwrap_or_else(|| panic!("missing {id}"))
        };

        // glm-5p2 is the openai-completions model with a /v1 base URL + thinking level map.
        let glm = find("accounts/fireworks/models/glm-5p2");
        assert_eq!(glm.api.as_str(), OPENAI_COMPLETIONS);
        assert_eq!(glm.base_url.as_deref(), Some("https://api.fireworks.ai/inference/v1"));
        let gc = glm.compat.as_ref().expect("compat");
        assert_eq!(gc.supports_store, Some(false));
        assert_eq!(gc.supports_developer_role, Some(false));
        assert_eq!(
            glm.thinking_level_map.as_ref().and_then(|m| m.get("xhigh")),
            Some(&Some("max".to_string()))
        );

        // deepseek-v4-flash is anthropic-messages with session-affinity + no-eager-tool-streaming.
        let ds = find("accounts/fireworks/models/deepseek-v4-flash");
        assert_eq!(ds.api.as_str(), ANTHROPIC_MESSAGES);
        assert_eq!(ds.base_url.as_deref(), Some("https://api.fireworks.ai/inference"));
        let dc = ds.compat.as_ref().expect("compat");
        assert_eq!(dc.send_session_affinity_headers, Some(true));
        assert_eq!(dc.supports_eager_tool_input_streaming, Some(false));
        assert_eq!(dc.supports_cache_control_on_tools, Some(false));
        assert_eq!(dc.supports_long_cache_retention, Some(false));
    }

    #[test]
    fn provider_identity() {
        let p = fireworks_provider();
        assert_eq!(p.id().as_str(), "fireworks");
        assert!(p.get_model("accounts/fireworks/models/kimi-k2p6").is_some());
        // env mapping exists for the env-key auth.
        let vars = crate::env_api_keys::api_key_env_vars("fireworks").expect("env mapping");
        assert!(vars.contains(&FIREWORKS_API_KEY_ENV));
    }
}

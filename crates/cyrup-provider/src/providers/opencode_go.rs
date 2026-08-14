//! The OpenCode Go provider (arch-01 §5). A **mixed-API** provider whose catalog carries both
//! [`anthropic-messages`](crate::api::anthropic_messages) and
//! [`openai-completions`](crate::api::openai_completions) models, each routed to its own `ApiImpl`
//! per request via the shared [`ApiRegistry`]. Mirrors Pi's `providers/opencode-go.ts` + the
//! generated `opencode-go.models.ts` catalog (both already-implemented wire protocols — no new
//! API or dependency).

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::{CredentialStore, InMemoryCredentialStore, ProviderAuth, env_key};
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// The provider id.
pub const OPENCODE_GO_PROVIDER_ID: &str = "opencode-go";

/// The env var carrying the OpenCode API key (Pi `envApiKeyAuth("OpenCode API key",
/// ["OPENCODE_API_KEY"])`, opencode-go.ts:11).
pub const OPENCODE_API_KEY_ENV: &str = "OPENCODE_API_KEY";

/// The verbatim catalog extracted from Pi's generated `opencode-go.models.ts`.
const OPENCODE_GO_CATALOG_JSON: &str = include_str!("catalog/opencode-go.json");

/// The full OpenCode Go catalog (1:1 with Pi `OPENCODE_GO_MODELS`). A parse failure yields an
/// empty catalog (surfaced loudly by the count test) rather than a panic (NO-PANIC policy).
pub fn opencode_go_models() -> Vec<Model> {
    serde_json::from_str(OPENCODE_GO_CATALOG_JSON).unwrap_or_default()
}

/// The OpenCode [`ProviderAuth`]: an API key from `$OPENCODE_API_KEY` (Pi `envApiKeyAuth`).
pub fn opencode_go_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key("OpenCode API key", [OPENCODE_API_KEY_ENV]))
}

/// Construct the OpenCode Go provider over the given credential store + shared api registry.
/// The registry MUST provide BOTH the `anthropic-messages` and `openai-completions` impls (use
/// [`builtin_registry`]).
pub fn opencode_go_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        OPENCODE_GO_PROVIDER_ID,
        // VERSION LAG (v0.83.0 → v0.84.1): upstream renamed the display name from
        // "OpenCode Zen Go" to "OpenCode Go" (v0.84.1 `ai/src/providers/opencode-go.ts:11`;
        // v0.83.0 `…:11` still reads "OpenCode Zen Go"). The provider `id` is unchanged, and
        // nothing in the workspace keys off the display string.
        "OpenCode Go",
        opencode_go_models(),
        opencode_go_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn opencode_go_provider() -> WireProvider {
    opencode_go_provider_with(
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
    use crate::api::compat::ThinkingFormat;
    use crate::known_api::{ANTHROPIC_MESSAGES, OPENAI_COMPLETIONS};
    use crate::provider::Provider;

    #[test]
    fn catalog_parses_verbatim_with_expected_count() {
        let models = opencode_go_models();
        // Every entry in Pi's `opencode-go.models.ts` (13 models).
        assert_eq!(models.len(), 13);
        assert!(models.iter().all(|m| m.provider.as_str() == "opencode-go"));
        assert!(models.iter().any(|m| m.api.as_str() == ANTHROPIC_MESSAGES));
        assert!(models.iter().any(|m| m.api.as_str() == OPENAI_COMPLETIONS));
    }

    #[test]
    fn mixed_api_models_carry_their_compat() {
        let models = opencode_go_models();
        let find = |id: &str| {
            models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };

        // deepseek-v4-flash: openai-completions, deepseek thinking format + reasoning-content req.
        let ds = find("deepseek-v4-flash");
        assert_eq!(ds.api.as_str(), OPENAI_COMPLETIONS);
        let dc = ds.compat.as_ref().expect("compat");
        assert_eq!(dc.thinking_format, Some(ThinkingFormat::Deepseek));
        assert_eq!(
            dc.requires_reasoning_content_on_assistant_messages,
            Some(true)
        );

        // minimax-m3: anthropic-messages, no compat block (rides anthropic defaults).
        let mm = find("minimax-m3");
        assert_eq!(mm.api.as_str(), ANTHROPIC_MESSAGES);
        assert_eq!(mm.base_url, "https://opencode.ai/zen/go");
        assert!(mm.compat.is_none());

        // qwen3.6-plus: openai-completions, qwen thinking format.
        let q = find("qwen3.6-plus");
        assert_eq!(
            q.compat.as_ref().and_then(|c| c.thinking_format),
            Some(ThinkingFormat::Qwen)
        );
    }

    #[test]
    fn provider_identity() {
        let p = opencode_go_provider();
        assert_eq!(p.id().as_str(), "opencode-go");
        // v0.84.1 `ai/src/providers/opencode-go.ts:11` — renamed from "OpenCode Zen Go"
        // (v0.83.0 `…:11`). The sibling `opencode` provider is NOT renamed: it is still
        // `name: "OpenCode Zen"` at v0.84.1 `ai/src/providers/opencode.ts:14`.
        assert_eq!(p.name(), "OpenCode Go");
        assert!(p.get_model("kimi-k2.6").is_some());
        let vars = crate::env_api_keys::api_key_env_vars("opencode-go").expect("env mapping");
        assert!(vars.contains(&OPENCODE_API_KEY_ENV));
    }
}

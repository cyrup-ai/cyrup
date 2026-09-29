//! The Fireworks provider (arch-01 §5). A **mixed-API** provider: its catalog carries both
//! [`anthropic-messages`](crate::api::anthropic_messages) and
//! [`openai-completions`](crate::api::openai_completions) models, each routed to its own `ApiImpl`
//! per request via the shared [`ApiRegistry`]. Mirrors Pi's `providers/fireworks.ts` +
//! the generated `fireworks.models.ts` catalog (both already-implemented wire protocols — no new
//! API or dependency).

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::{CredentialStore, InMemoryCredentialStore, ProviderAuth, env_key};
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
    crate::catalog::load_catalog(FIREWORKS_CATALOG_JSON).unwrap_or_default()
}

/// The Fireworks [`ProviderAuth`]: an API key from `$FIREWORKS_API_KEY` (Pi `envApiKeyAuth`).
pub fn fireworks_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key("Fireworks API key", [FIREWORKS_API_KEY_ENV]))
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::known_api::{ANTHROPIC_MESSAGES, OPENAI_COMPLETIONS};
    use crate::provider::Provider;

    #[test]
    fn catalog_parses_verbatim_with_expected_count() {
        let models = fireworks_models();
        // Every entry in pi's `fireworks.models.ts`, live since PROV-071 (22 models; 16 while the
        // catalog was frozen at `b0c2a90e`).
        assert_eq!(models.len(), 22);
        assert!(models.iter().all(|m| m.provider.as_str() == "fireworks"));
        // Mixed-API: most models are anthropic-messages, glm-5p2 is openai-completions.
        assert!(models.iter().any(|m| m.api.as_str() == ANTHROPIC_MESSAGES));
        assert!(models.iter().any(|m| m.api.as_str() == OPENAI_COMPLETIONS));
    }

    #[test]
    fn anthropic_and_openai_models_route_per_api() {
        let models = fireworks_models();
        let find = |id: &str| {
            models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };

        // DRIFT-052, now CONVERGED rather than imposed. This test used to pin
        // `accounts/fireworks/models/glm-5p2` and its `routers/glm-5p2-fast` twin, whose four-key
        // `openAICompat` block cyrup forward-ported past `b0c2a90e` through the generator's DELTAS
        // table. Both ids are retired upstream — GLM 5.2 gave way to 5.3 — so the pin has nothing
        // left to act on and `xtask`'s CONVERGED table now asserts their ABSENCE instead. What the
        // forward-port was actually about survives on the successor rows, from upstream's own data:
        // pi `b9497c8c1` ("fix(ai): correct Fireworks GLM prompt caching, closes #7676") moved the
        // GLM rows off an inline `candidate.compat = { supportsStore, supportsDeveloperRole }`
        // ASSIGNMENT — which discarded the models.dev keys — onto the shared `openAICompat`
        // constant, and `glm-5p3` carries all four keys that fix produces.
        let glm = find("accounts/fireworks/models/glm-5p3");
        assert_eq!(glm.api.as_str(), OPENAI_COMPLETIONS);
        assert_eq!(glm.base_url, "https://api.fireworks.ai/inference/v1");
        let gc = glm.compat.as_ref().expect("compat");
        assert_eq!(gc.supports_store, Some(false));
        assert_eq!(gc.supports_developer_role, Some(false));
        assert_eq!(gc.send_session_affinity_headers, Some(true));
        assert_eq!(gc.supports_long_cache_retention, Some(false));
        // The declared keys are not inert, and the two do NOT behave alike on the wire — assert
        // the RESOLVED values too, because neither is auto-detected for fireworks:
        //   * `sendSessionAffinityHeaders` detects to `false` (`openai-completions.ts:1471`
        //     @v0.83.0), so ABSENT means no `x-session-affinity` header at all and every Fireworks
        //     prompt-cache lookup misses — Fireworks routes cache by replica affinity.
        //   * `supportsLongCacheRetention` detects to `!(isTogether || isCloudflareWorkersAI ||
        //     isCloudflareAiGateway || isNvidia || isAntLing)` (`…:1474-1480`) — all false for
        //     fireworks — so ABSENT resolves to **true** and cyrup asks for a retention Fireworks
        //     does not honour.
        let gr = crate::api::compat::get_compat(glm);
        assert!(gr.send_session_affinity_headers);
        assert!(!gr.supports_long_cache_retention);
        assert!(!gr.supports_store);
        assert!(!gr.supports_developer_role);
        // The top rung is `"max":"max"`, never `xhigh`.
        let gm = glm.thinking_level_map.as_ref().expect("glm map");
        assert_eq!(gm.get("max"), Some(&Some("max".to_string())));
        assert_eq!(gm.get("xhigh"), Some(&None));

        // The 5.2 ids are gone. Asserted, not assumed: if either comes back, DRIFT-052's
        // forward-port decision has to be re-taken against whatever compat block it comes back
        // with, and `xtask`'s CONVERGED table fails the generation run that brings it.
        for gone in [
            "accounts/fireworks/models/glm-5p2",
            "accounts/fireworks/routers/glm-5p2-fast",
        ] {
            assert!(
                !models.iter().any(|m| m.id.as_str() == gone),
                "{gone} is retired upstream"
            );
        }

        // The anthropic-messages half: session-affinity + no-eager-tool-streaming, on the
        // `/inference` base URL rather than `/inference/v1`.
        let ds = find("accounts/fireworks/models/deepseek-v4p1-flash");
        assert_eq!(ds.api.as_str(), ANTHROPIC_MESSAGES);
        assert_eq!(ds.base_url, "https://api.fireworks.ai/inference");
        let dc = ds.compat.as_ref().expect("compat");
        assert_eq!(dc.send_session_affinity_headers, Some(true));
        assert_eq!(dc.supports_eager_tool_input_streaming, Some(false));
        assert_eq!(dc.supports_cache_control_on_tools, Some(false));
        assert_eq!(dc.supports_long_cache_retention, Some(false));
        // pi #9323 — every Fireworks Messages row now allows an empty thinking signature. The
        // frozen floor had none of these, so cyrup dropped thinking blocks Fireworks does send.
        assert_eq!(dc.allow_empty_signature, Some(true));

        // MIRROR: the `routers/` twin takes the SAME openai compat, and the split is by API, not by
        // id — every `openai-completions` row has the block and no `anthropic-messages` row does.
        let fast = find("accounts/fireworks/routers/glm-5p3-fast");
        assert_eq!(fast.api.as_str(), OPENAI_COMPLETIONS);
        assert_eq!(fast.base_url, "https://api.fireworks.ai/inference/v1");
        let fr = crate::api::compat::get_compat(fast);
        assert!(fr.send_session_affinity_headers);
        assert!(!fr.supports_long_cache_retention);
        for m in &models {
            let c = m.compat.as_ref().expect("compat");
            if m.api.as_str() == OPENAI_COMPLETIONS {
                assert_eq!(
                    c.supports_store,
                    Some(false),
                    "{} is on Completions and must carry openAICompat",
                    m.id.as_str()
                );
            } else {
                assert_eq!(
                    c.supports_store,
                    None,
                    "{} is on Messages and must not take openAICompat",
                    m.id.as_str()
                );
                assert_eq!(
                    c.allow_empty_signature,
                    Some(true),
                    "{} is on Messages and must allow an empty signature (#9323)",
                    m.id.as_str()
                );
            }
        }
    }

    #[test]
    fn provider_identity() {
        let p = fireworks_provider();
        assert_eq!(p.id().as_str(), "fireworks");
        assert!(p.get_model("accounts/fireworks/models/kimi-k3").is_some());
        // env mapping exists for the env-key auth.
        let vars = crate::env_api_keys::api_key_env_vars("fireworks").expect("env mapping");
        assert!(vars.contains(&FIREWORKS_API_KEY_ENV));
    }
}

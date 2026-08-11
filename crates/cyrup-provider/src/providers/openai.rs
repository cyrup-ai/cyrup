//! The OpenAI provider (arch-01 §5). Speaks the
//! [`openai-responses`](crate::api::openai_responses) wire protocol. Mirrors Pi's
//! `providers/openai.ts` + the generated `openai.models.ts` catalog.

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::{CredentialStore, InMemoryCredentialStore, ProviderAuth, env_key};
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// OpenAI's API base URL (the `/responses` path is appended by the wire impl).
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// The provider id.
pub const OPENAI_PROVIDER_ID: &str = "openai";

/// The env var carrying the OpenAI API key (Pi `envApiKeyAuth("OpenAI API key",
/// ["OPENAI_API_KEY"])`, openai.ts:11).
pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

/// The verbatim catalog extracted from Pi's generated `openai.models.ts`.
const OPENAI_CATALOG_JSON: &str = include_str!("catalog/openai.json");

/// The full OpenAI catalog (1:1 with Pi `OPENAI_MODELS`). A parse failure yields an empty catalog
/// (surfaced loudly by the count test) rather than a panic (NO-PANIC policy).
pub fn openai_models() -> Vec<Model> {
    serde_json::from_str(OPENAI_CATALOG_JSON).unwrap_or_default()
}

/// The OpenAI [`ProviderAuth`]: an API key from `$OPENAI_API_KEY` (Pi `envApiKeyAuth`).
pub fn openai_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key([OPENAI_API_KEY_ENV]))
}

/// Construct the OpenAI provider over the given credential store + shared api registry. The
/// registry MUST provide the `openai-responses` impl (use [`builtin_registry`]).
pub fn openai_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        OPENAI_PROVIDER_ID,
        "OpenAI",
        openai_models(),
        openai_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn openai_provider() -> WireProvider {
    openai_provider_with(
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
    use crate::auth::types::AuthContext;
    use crate::context::Context;
    use crate::known_api::OPENAI_RESPONSES;
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
        let models = openai_models();
        // Every entry in Pi's `openai.models.ts` @91585d9a (45 models — the GPT-5.6 trio landed in
        // `7df2a94e`).
        assert_eq!(models.len(), 45);
        assert!(models.iter().all(|m| m.api.as_str() == OPENAI_RESPONSES));
        assert!(models.iter().all(|m| m.provider.as_str() == "openai"));
        assert!(models.iter().all(|m| m.base_url == OPENAI_BASE_URL));
    }

    /// Pi's generator wraps exactly the long-context OpenAI models in `withOpenAiLongContextPricing`
    /// (generate-models.ts:333-364, :2127-2130): a single tier above 272,000 input tokens at 2×
    /// input, 1.5× output, 2× cacheRead, 2× cacheWrite. Without it a 300k-token gpt-5.4-pro request
    /// is billed at half the real rate.
    #[test]
    fn long_context_models_carry_the_272k_pricing_tier() {
        let models = openai_models();
        // The GPT-5.6 trio (pi `7df2a94e`, openai.models.ts @91585d9a) carries the same 272k tier
        // with the same 2x/1.5x/2x/2x multipliers as the 5.4/5.5 family.
        let long_context = [
            "gpt-5.4",
            "gpt-5.4-pro",
            "gpt-5.5",
            "gpt-5.5-pro",
            "gpt-5.6-luna",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
        ];
        for id in long_context {
            let m = models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("{id} missing from catalog"));
            let tiers = m
                .cost
                .tiers
                .as_ref()
                .unwrap_or_else(|| panic!("{id} has no pricing tiers"));
            assert_eq!(tiers.len(), 1, "{id}");
            let t = &tiers[0];
            assert_eq!(t.input_tokens_above, 272_000, "{id}");
            assert!((t.input - m.cost.input * 2.0).abs() < 1e-9, "{id} input");
            assert!((t.output - m.cost.output * 1.5).abs() < 1e-9, "{id} output");
            assert!((t.cache_read - m.cost.cache_read * 2.0).abs() < 1e-9, "{id} cacheRead");
            assert!((t.cache_write - m.cost.cache_write * 2.0).abs() < 1e-9, "{id} cacheWrite");
        }
        // Every other OpenAI model stays flat-priced.
        for m in &models {
            if !long_context.contains(&m.id.as_str()) {
                assert!(m.cost.tiers.is_none(), "{} gained a tier", m.id.as_str());
            }
        }
    }

    /// VERSION LAG (v0.83.0 → v0.84.1): OpenAI cut GPT-5.6 Terra and Luna prices on 2026-07-30 and
    /// pi added the authoritative table `OPENAI_GPT_56_STANDARD_COSTS`
    /// (v0.84.1 `ai/scripts/generate-models.ts:387-393`), which the openai literals then consume
    /// (`:2371`, `:2383` — `cost: withOpenAiLongContextPricing(OPENAI_GPT_56_STANDARD_COSTS[...])`;
    /// `:2372`/`:2384` are the `contextWindow:` lines one row below).
    /// The table does not exist at v0.83.0 (`grep OPENAI_GPT_56_STANDARD_COSTS` finds nothing in
    /// `v0.83.0 ai/scripts/generate-models.ts`); v0.83.0 spelled the same two rows as inline
    /// literals `{1, 6, 0.1, 1.25}` / `{2.5, 15, 0.25, 3.125}`, which is exactly what cyrup carried.
    /// Overbilling by 5x (luna) and 1.25x (terra) is invisible without an absolute assertion, which
    /// is why [`long_context_models_carry_the_272k_pricing_tier`] — a RELATIVE multiplier check —
    /// stayed green through the whole divergence.
    #[test]
    fn gpt_5_6_luna_and_terra_use_the_post_cut_prices() {
        let models = openai_models();
        let find = |id: &str| {
            models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("{id} missing"))
                .clone()
        };

        let luna = find("gpt-5.6-luna");
        assert_eq!(luna.cost.input, 0.2);
        assert_eq!(luna.cost.output, 1.2);
        assert_eq!(luna.cost.cache_read, 0.02);
        assert_eq!(luna.cost.cache_write, 0.25);
        let t = &luna.cost.tiers.as_ref().expect("luna tiers")[0];
        assert_eq!(t.input, 0.4);
        assert_eq!(t.output, 1.8);
        assert_eq!(t.cache_read, 0.04);
        assert_eq!(t.cache_write, 0.5);

        let terra = find("gpt-5.6-terra");
        assert_eq!(terra.cost.input, 2.0);
        assert_eq!(terra.cost.output, 12.0);
        assert_eq!(terra.cost.cache_read, 0.2);
        assert_eq!(terra.cost.cache_write, 2.5);
        let t = &terra.cost.tiers.as_ref().expect("terra tiers")[0];
        assert_eq!(t.input, 4.0);
        assert_eq!(t.output, 18.0);
        assert_eq!(t.cache_read, 0.4);
        assert_eq!(t.cache_write, 5.0);

        // MIRROR: Sol is NOT in `OPENAI_GPT_56_STANDARD_COSTS` — its literal is still the inline
        // `{5, 30, 0.5, 6.25}` at BOTH tags (v0.84.1 `…:2360`), so it must not move.
        let sol = find("gpt-5.6-sol");
        assert_eq!(sol.cost.input, 5.0);
        assert_eq!(sol.cost.output, 30.0);
        assert_eq!(sol.cost.cache_read, 0.5);
        assert_eq!(sol.cost.cache_write, 6.25);
    }

    /// End-to-end: the catalog rate + the pricing function together bill a real long-context
    /// gpt-5.4-pro request at the long-context price.
    #[test]
    fn gpt_5_4_pro_bills_long_context_input_at_the_tier_rate() {
        let models = openai_models();
        let m = models.iter().find(|m| m.id.as_str() == "gpt-5.4-pro").expect("gpt-5.4-pro");
        let mut usage = cyrup_core::Usage { input: 300_000, output: 1_000, ..Default::default() };
        crate::usage::apply_cost(&m.cost, &mut usage);
        // Base $30/1e6 would be $9.00; the long-context tier is $60/1e6 => $18.00.
        assert!(
            (usage.cost.input - 18.0).abs() < 1e-6,
            "long-context input cost was {} (base-rate billing is 9.0)",
            usage.cost.input
        );
        // Output at 1.5x: 1_000 @ 270/1e6 = 0.27.
        assert!((usage.cost.output - 0.27).abs() < 1e-9, "output {}", usage.cost.output);
    }

    #[test]
    fn provider_identity_and_env_mapping() {
        let p = openai_provider();
        assert_eq!(p.id().as_str(), "openai");
        assert!(p.get_model("gpt-4").is_some());
        let vars = crate::env_api_keys::api_key_env_vars("openai").expect("env mapping");
        assert!(vars.contains(&OPENAI_API_KEY_ENV));
    }

    #[tokio::test]
    async fn unconfigured_without_env_yields_error_terminal() {
        let provider = openai_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider.get_model("gpt-4").unwrap().clone();
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
            OPENAI_API_KEY_ENV.to_string(),
            "sk-openai-test".to_string(),
        )]));
        let provider = openai_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(env));
        let mut model = provider.get_model("gpt-4").unwrap().clone();
        model.base_url = "http://127.0.0.1:1/v1".to_string();
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

    /// Live smoke test against the real OpenAI Responses API. Ignored by default; run with
    /// `OPENAI_API_KEY` set: `cargo test -p cyrup-provider -- --ignored live_openai`.
    #[tokio::test]
    #[ignore = "hits the real OpenAI API; requires OPENAI_API_KEY"]
    async fn live_openai_returns_non_empty_done() {
        use cyrup_core::{Content, Message};
        if std::env::var("OPENAI_API_KEY").is_err() {
            eprintln!("skipping: OPENAI_API_KEY not set");
            return;
        }
        let provider = openai_provider();
        let model = provider
            .get_model("gpt-5-mini")
            .or_else(|| provider.get_model("gpt-5"))
            .unwrap()
            .clone();
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text("Reply with exactly: pong")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let opts = StreamOptions {
            max_tokens: Some(256),
            ..Default::default()
        };
        let msg = collect_message(provider.stream(&model, &ctx, &opts)).await;
        assert_ne!(
            msg.stop_reason,
            StopReason::Error,
            "got error: {:?}",
            msg.error_message
        );
        let has_content = msg.content.iter().any(|c| match c {
            Content::Text { text, .. } => !text.trim().is_empty(),
            Content::Thinking { thinking, .. } => !thinking.trim().is_empty(),
            _ => false,
        });
        assert!(
            has_content,
            "expected non-empty content, got: {:?}",
            msg.content
        );
    }
}

//! The OpenAI-completions provider fleet (arch-01 §5). Every Pi provider whose models speak the
//! [`openai-completions`](crate::api::openai_completions) wire protocol, ported as
//! [`WireProvider`]s with their FULL catalogs extracted verbatim from Pi's generated
//! `providers/<id>.models.ts` files. They differ only in id/name/base URL, env-key, and catalog;
//! the shared compat matrix ([`crate::api::compat`]) drives every per-provider behavior.
//!
//! Mirrors `providers/{ant-ling,cerebras,deepseek,groq,huggingface,moonshotai,moonshotai-cn,nvidia,
//! openrouter,xai,xiaomi,xiaomi-token-plan-*,zai,zai-coding-cn}.ts` + their `.models.ts` catalogs.

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::{CredentialStore, InMemoryCredentialStore, ProviderAuth, env_key};
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// Static metadata for one openai-completions fleet provider (Pi provider factory `id`/`name`/
/// `auth`, plus the embedded `<id>.models.ts` catalog).
pub struct FleetSpec {
    pub id: &'static str,
    pub name: &'static str,
    /// API-key env var (matches `env-api-keys.ts` `getApiKeyEnvVars`).
    pub env_var: &'static str,
    /// The verbatim JSON catalog (extracted from Pi's `<id>.models.ts`).
    pub catalog_json: &'static str,
}

macro_rules! fleet {
    ($($id:literal => ($const:ident, $name:literal, $env:literal, $file:literal)),* $(,)?) => {
        $(
            pub const $const: FleetSpec = FleetSpec {
                id: $id,
                name: $name,
                env_var: $env,
                catalog_json: include_str!(concat!("catalog/", $file, ".json")),
            };
        )*

        /// Every fleet spec (stable order matching Pi's `builtinProviders()` listing).
        pub const FLEET: &[FleetSpec] = &[$($const),*];
    };
}

fleet! {
    "ant-ling"              => (ANT_LING, "Ant Ling", "ANT_LING_API_KEY", "ant-ling"),
    "cerebras"              => (CEREBRAS, "Cerebras", "CEREBRAS_API_KEY", "cerebras"),
    "deepseek"              => (DEEPSEEK, "DeepSeek", "DEEPSEEK_API_KEY", "deepseek"),
    "groq"                  => (GROQ, "Groq", "GROQ_API_KEY", "groq"),
    "huggingface"           => (HUGGINGFACE, "Hugging Face", "HF_TOKEN", "huggingface"),
    "moonshotai"            => (MOONSHOTAI, "Moonshot AI", "MOONSHOT_API_KEY", "moonshotai"),
    "moonshotai-cn"         => (MOONSHOTAI_CN, "Moonshot AI CN", "MOONSHOT_API_KEY", "moonshotai-cn"),
    "nvidia"                => (NVIDIA, "NVIDIA", "NVIDIA_API_KEY", "nvidia"),
    "openrouter"            => (OPENROUTER, "OpenRouter", "OPENROUTER_API_KEY", "openrouter"),
    "xai"                   => (XAI, "xAI", "XAI_API_KEY", "xai"),
    "xiaomi"                => (XIAOMI, "Xiaomi", "XIAOMI_API_KEY", "xiaomi"),
    "xiaomi-token-plan-ams" => (XIAOMI_TP_AMS, "Xiaomi Token Plan AMS", "XIAOMI_TOKEN_PLAN_AMS_API_KEY", "xiaomi-token-plan-ams"),
    "xiaomi-token-plan-cn"  => (XIAOMI_TP_CN, "Xiaomi Token Plan CN", "XIAOMI_TOKEN_PLAN_CN_API_KEY", "xiaomi-token-plan-cn"),
    "xiaomi-token-plan-sgp" => (XIAOMI_TP_SGP, "Xiaomi Token Plan SGP", "XIAOMI_TOKEN_PLAN_SGP_API_KEY", "xiaomi-token-plan-sgp"),
    "zai"                   => (ZAI, "Z.AI", "ZAI_API_KEY", "zai"),
    "zai-coding-cn"         => (ZAI_CODING_CN, "Z.AI Coding CN", "ZAI_CODING_CN_API_KEY", "zai-coding-cn"),
}

impl FleetSpec {
    /// Parse the embedded catalog into [`Model`]s. Catalogs are compile-time constants extracted
    /// verbatim from Pi; a parse failure yields an empty catalog (surfaced loudly by the
    /// catalog-count tests) rather than a panic (NO-PANIC policy).
    pub fn models(&self) -> Vec<Model> {
        serde_json::from_str(self.catalog_json).unwrap_or_default()
    }

    /// The provider's [`ProviderAuth`]: an API key from its env var (Pi `envApiKeyAuth`), plus the
    /// `lazyOAuth` clause for the two fleet members that have one — `xai`
    /// (`providers/xai.ts:15-20`) and `openrouter` (`providers/openrouter.ts:14-18`). See
    /// [`super::builtin_oauth::builtin_provider_oauth`].
    pub fn auth(&self) -> ProviderAuth {
        ProviderAuth {
            api_key: Some(env_key([self.env_var])),
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

/// Look up a fleet spec by provider id.
pub fn fleet_spec(id: &str) -> Option<&'static FleetSpec> {
    FLEET.iter().find(|s| s.id == id)
}

/// Construct every openai-completions fleet provider over a shared store + registry. Useful for
/// registering the whole fleet into a [`crate::collection::Models`] in one call.
pub fn fleet_providers_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> Vec<WireProvider> {
    FLEET
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
    use crate::api::openai_completions::build_body;
    use crate::context::Context;
    use crate::known_api::OPENAI_COMPLETIONS;
    use crate::provider::Provider;
    use crate::stream::StreamOptions;
    use cyrup_core::ModelThinkingLevel;

    /// Per-provider minimum catalog sizes (= the entry count in Pi's `<id>.models.ts`). A parse
    /// failure or a dropped entry makes this fail loudly.
    const EXPECTED_COUNTS: &[(&str, usize)] = &[
        ("ant-ling", 3),
        ("cerebras", 3),
        ("deepseek", 2),
        ("groq", 7),
        ("huggingface", 49),
        ("moonshotai", 9),
        ("moonshotai-cn", 9),
        ("nvidia", 20),
        ("openrouter", 270),
        ("xai", 8),
        ("xiaomi", 6),
        // The three token-plan catalogs dropped to 3 in pi `cc2db980`, which stopped cloning the
        // API-billing Xiaomi catalog into every region (see `catalog_data.rs`, PROV-004).
        ("xiaomi-token-plan-ams", 3),
        ("xiaomi-token-plan-cn", 3),
        ("xiaomi-token-plan-sgp", 3),
        ("zai", 6),
        ("zai-coding-cn", 6),
    ];

    #[test]
    fn every_catalog_parses_with_expected_count() {
        for (id, count) in EXPECTED_COUNTS {
            let spec = fleet_spec(id).unwrap_or_else(|| panic!("no spec for {id}"));
            let models = spec.models();
            assert_eq!(models.len(), *count, "catalog count mismatch for {id}");
            // Every model is openai-completions and tagged with the provider id.
            assert!(
                models.iter().all(|m| m.api.as_str() == OPENAI_COMPLETIONS),
                "{id} api"
            );
            assert!(
                models.iter().all(|m| m.provider.as_str() == *id),
                "{id} provider tag"
            );
            // baseUrl is always present in the generated catalog.
            assert!(
                models.iter().all(|m| !m.base_url.is_empty()),
                "{id} baseUrl"
            );
        }
    }

    #[test]
    fn fleet_has_sixteen_providers() {
        assert_eq!(FLEET.len(), 16);
        // Every fleet provider has an env-key mapping in env-api-keys.
        for spec in FLEET {
            let vars = crate::env_api_keys::api_key_env_vars(spec.id)
                .unwrap_or_else(|| panic!("no env mapping for {}", spec.id));
            assert!(vars.contains(&spec.env_var), "{} env var mismatch", spec.id);
        }
    }

    #[test]
    fn deepseek_catalog_carries_thinking_map_and_compat() {
        // DeepSeek models carry the deepseek thinking format + a thinkingLevelMap (high->"high",
        // max->"max" per pi deepseek.models.ts @91585d9a), proving the catalog's compat +
        // thinkingLevelMap deserialize 1:1.
        let models = DEEPSEEK.models();
        let m = models
            .iter()
            .find(|m| m.id.as_str() == "deepseek-v4-pro")
            .expect("v4-pro");
        let compat = m.compat.as_ref().expect("compat");
        assert_eq!(
            compat.thinking_format,
            Some(crate::api::compat::ThinkingFormat::Deepseek)
        );
        assert_eq!(
            compat.requires_reasoning_content_on_assistant_messages,
            Some(true)
        );
        let map = m.thinking_level_map.as_ref().expect("map");
        assert_eq!(map.get("high"), Some(&Some("high".to_string())));
        assert_eq!(map.get("max"), Some(&Some("max".to_string())));
        assert_eq!(map.get("xhigh"), None);
    }

    #[test]
    fn catalog_drives_reasoning_encoding_via_compat() {
        // A cerebras model (openai thinking format, reasoning_effort supported) encodes
        // reasoning_effort; max_tokens field per compat (cerebras is standard openai => max_completion_tokens).
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            max_tokens: Some(40),
            ..Default::default()
        };
        let cerebras = CEREBRAS.models();
        let gpt = cerebras
            .iter()
            .find(|m| m.id.as_str() == "gpt-oss-120b")
            .expect("gpt-oss");
        let body = build_body(gpt, &Context::default(), &opts);
        assert_eq!(body["reasoning_effort"], "high");

        // A deepseek model (deepseek thinking format) maps high->"high" via thinkingLevelMap and
        // sends reasoning_effort (deepseek supports it) — proving catalog compat reaches the encoder.
        let ds = DEEPSEEK.models();
        let m = ds
            .iter()
            .find(|m| m.id.as_str() == "deepseek-v4-pro")
            .expect("v4-pro");
        let body = build_body(m, &Context::default(), &opts);
        assert_eq!(body["reasoning_effort"], "high");
    }

    /// VERSION LAG (v0.83.0 → v0.84.1): Groq models get NO `thinkingLevelMap` from the generator
    /// itself (v0.84.1 `ai/scripts/generate-models.ts:1470-1492` builds the row without one) —
    /// the only source is a single override, which upstream RETARGETED from `qwen/qwen3-32b`
    /// (v0.83.0 `…:837`) to `qwen/qwen3.6-27b` (v0.84.1 `…:870`). cyrup still carried the map on
    /// the 3-32b row, so `high` mapped to the literal `"default"` and `low`/`medium` were pinned to
    /// `null` for a model upstream no longer special-cases.
    #[test]
    fn groq_qwen3_32b_no_longer_carries_the_retargeted_thinking_level_map() {
        let models = GROQ.models();
        let qwen = models
            .iter()
            .find(|m| m.id.as_str() == "qwen/qwen3-32b")
            .expect("qwen/qwen3-32b");
        assert_eq!(qwen.thinking_level_map, None);
        // MIRROR: no Groq row has a thinking-level map — the generator never sets one, and the sole
        // override now names an id this catalog does not contain.
        assert!(models.iter().all(|m| m.thinking_level_map.is_none()));
        // MIRROR: the row itself is untouched — this is a map removal, not a model removal.
        assert!(qwen.reasoning);
        assert_eq!(qwen.context_window, 131_072);
    }

    #[test]
    fn providers_construct_with_correct_identity() {
        let p = GROQ.provider();
        assert_eq!(p.id().as_str(), "groq");
        assert_eq!(p.name(), "Groq");
        assert!(p.get_model("openai/gpt-oss-20b").is_some() || !p.models().is_empty());

        let xai = XAI.provider();
        assert_eq!(xai.id().as_str(), "xai");
        assert!(xai.models().iter().all(|m| m.provider.as_str() == "xai"));
    }
}

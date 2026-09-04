//! The OpenAI-completions provider fleet (arch-01 §5). Every Pi provider whose models speak the
//! [`openai-completions`](crate::api::openai_completions) wire protocol, ported as
//! [`WireProvider`]s with their FULL catalogs extracted verbatim from Pi's generated
//! `providers/<id>.models.ts` files. They differ only in id/name/base URL, env-key, and catalog;
//! the shared compat matrix ([`crate::api::compat`]) drives every per-provider behavior.
//!
//! Mirrors `providers/{ant-ling,cerebras,deepseek,groq,huggingface,moonshotai,moonshotai-cn,nvidia,
//! openrouter,qwen-token-plan,qwen-token-plan-cn,qwen-token-plan-individual,xai,xiaomi,
//! xiaomi-token-plan-*,zai,zai-coding-cn}.ts` + their `.models.ts` catalogs.
//!
//! # Members without an embedded catalog (PROV-014)
//!
//! The three `qwen-token-plan*` members are registered with [`FleetCatalog::Dynamic`] — no
//! `catalog/*.json` — and that is a statement about EVIDENCE, not a shortcut. pi's rows for them
//! come from models.dev's `alibaba-token-plan[-cn]` records (`ai/scripts/generate-models.ts:2303-2380`
//! @v0.84.4), generated into a gitignored `providers/data/*.json`. The providers were added at
//! `bbb91fa8a` (v0.81.0~25, 2026-07-20) and `c03d78bdc` (2026-08-06), both AFTER `b0c2a90e` — the last
//! revision at which any `*.models.ts` was a data literal and the revision every other embedded
//! catalog is generated from (`xtask/src/main.rs::DEFAULT_REV`, PROV-060). So their catalog data is
//! in git at NO revision, `xtask gen-catalogs` cannot produce it, and hand-writing rows from memory is
//! exactly what the catalog rules forbid. What IS in git — the ids, `baseUrl`, compat, thinking maps
//! (`qwen-token-plan-models.test.ts` @v0.84.4) — is recorded on each member's doc comment for the
//! day the data becomes obtainable; the runtime catalog comes from the pi.dev overlay
//! ([`crate::remote_catalog`], which fetches `/api/models/providers/<id>` for every registered
//! provider) and from `models.json`.

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
    /// Upstream's `envApiKeyAuth(<name>, …)` first argument — the user-facing api-key method label
    /// `/login` lists and `login` interpolates into `Enter {name}` (`ai/src/auth/helpers.ts:9,12`).
    /// It is NOT `"{name} API key"` for every member: `huggingface` is `"Hugging Face token"`
    /// (`providers/huggingface.ts:11`) and `moonshotai-cn` is `"Moonshot AI API key"`, not
    /// `"Moonshot AI CN API key"` (`providers/moonshotai-cn.ts:11`) — which is why it is a table
    /// column rather than a format string.
    pub auth_name: &'static str,
    /// Where this member's rows come from — see [`FleetCatalog`].
    pub catalog: FleetCatalog,
    /// Upstream's `createProvider({ baseUrl })` (`Provider.baseUrl`, PROV-017) for the members
    /// whose catalog cannot carry it because they have none ([`FleetCatalog::Dynamic`]). `None` for
    /// every embedded-catalog member: each of their rows carries its own `baseUrl`, which is what
    /// the request path reads.
    pub base_url: Option<&'static str>,
}

/// The source of a fleet member's catalog rows. Two variants because "no embedded rows" is a
/// deliberate, evidence-driven state (see the module doc), not an empty string that a reader
/// could mistake for a broken `include_str!`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetCatalog {
    /// The verbatim JSON catalog extracted from Pi's `<id>.models.ts` at the pinned revision.
    Embedded(&'static str),
    /// No embedded rows: the member's models arrive at runtime (pi.dev overlay, `models.json`).
    Dynamic,
}

/// The catalog half of a `fleet!` row: a file stem embeds `catalog/<stem>.json`; `dynamic(<url>)`
/// declares a member with no embedded rows and the provider-level `baseUrl` upstream's
/// `createProvider` call carries.
macro_rules! fleet_catalog {
    (dynamic($base_url:literal)) => {
        (FleetCatalog::Dynamic, Some($base_url))
    };
    ($file:literal) => {
        (
            FleetCatalog::Embedded(include_str!(concat!("catalog/", $file, ".json"))),
            None::<&'static str>,
        )
    };
}

macro_rules! fleet {
    ($($id:literal => ($const:ident, $name:literal, $env:literal, $auth:literal, $($catalog:tt)+)),* $(,)?) => {
        $(
            pub const $const: FleetSpec = FleetSpec {
                id: $id,
                name: $name,
                env_var: $env,
                auth_name: $auth,
                catalog: fleet_catalog!($($catalog)+).0,
                base_url: fleet_catalog!($($catalog)+).1,
            };
        )*

        /// Every fleet spec (stable order matching Pi's `builtinProviders()` listing).
        pub const FLEET: &[FleetSpec] = &[$($const),*];
    };
}

fleet! {
    "ant-ling"              => (ANT_LING, "Ant Ling", "ANT_LING_API_KEY", "Ant Ling API key", "ant-ling"),
    "cerebras"              => (CEREBRAS, "Cerebras", "CEREBRAS_API_KEY", "Cerebras API key", "cerebras"),
    "deepseek"              => (DEEPSEEK, "DeepSeek", "DEEPSEEK_API_KEY", "DeepSeek API key", "deepseek"),
    "groq"                  => (GROQ, "Groq", "GROQ_API_KEY", "Groq API key", "groq"),
    "huggingface"           => (HUGGINGFACE, "Hugging Face", "HF_TOKEN", "Hugging Face token", "huggingface"),
    "moonshotai"            => (MOONSHOTAI, "Moonshot AI", "MOONSHOT_API_KEY", "Moonshot AI API key", "moonshotai"),
    "moonshotai-cn"         => (MOONSHOTAI_CN, "Moonshot AI CN", "MOONSHOT_API_KEY", "Moonshot AI API key", "moonshotai-cn"),
    "nvidia"                => (NVIDIA, "NVIDIA", "NVIDIA_API_KEY", "NVIDIA API key", "nvidia"),
    "openrouter"            => (OPENROUTER, "OpenRouter", "OPENROUTER_API_KEY", "OpenRouter API key", "openrouter"),
    // PROV-014 — `providers/qwen-token-plan.ts:6-15` @v0.84.4 (identical at v0.83.0), registered at
    // `all.ts:118`. models.dev source `alibaba-token-plan`; the ids upstream's own test pins as
    // present (`qwen-token-plan-models.test.ts:42-58` @v0.84.4): MiniMax-M2.5, deepseek-v3.2,
    // deepseek-v4-flash, deepseek-v4-pro, glm-5, glm-5.1, glm-5.2, kimi-k2.5, kimi-k2.6,
    // kimi-k2.7-code, qwen3.6-flash, qwen3.6-plus, qwen3.7-max, qwen3.7-plus, qwen3.8-max — every
    // row `compat: { thinkingFormat: "qwen", supportsDeveloperRole: false, supportsStore: false }`
    // (`generate-models.ts:2308-2313`), `reasoning_effort` only on the deepseek-v4-*/glm-5* rows
    // (`:306-316`). See the module doc for why the rows themselves are not embedded.
    "qwen-token-plan"       => (QWEN_TOKEN_PLAN, "Qwen Token Plan", "QWEN_TOKEN_PLAN_API_KEY", "Qwen Token Plan API key", dynamic("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1")),
    // PROV-014 — `providers/qwen-token-plan-cn.ts:6-15` @v0.84.4 (identical at v0.83.0),
    // `all.ts:119`. models.dev source `alibaba-token-plan-cn`; same id set as the international
    // plan, China endpoint, its own key.
    "qwen-token-plan-cn"    => (QWEN_TOKEN_PLAN_CN, "Qwen Token Plan CN", "QWEN_TOKEN_PLAN_CN_API_KEY", "Qwen Token Plan CN API key", dynamic("https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1")),
    // VERSION LAG (v0.83.0 → v0.84.4): `providers/qwen-token-plan-individual.ts:6-15`, added at
    // `c03d78bdc` (#7659), `all.ts:120`. The international endpoint and the SAME env var as
    // `qwen-token-plan` (`env-api-keys.ts:83`: `"qwen-token-plan-individual":
    // "QWEN_TOKEN_PLAN_API_KEY"`), narrowed to the eight-model personal allowlist
    // (`generate-models.ts:324-336`; `qwen-token-plan-models.test.ts:60-69`).
    "qwen-token-plan-individual" => (QWEN_TOKEN_PLAN_INDIVIDUAL, "Qwen Token Plan Individual", "QWEN_TOKEN_PLAN_API_KEY", "Qwen Token Plan Individual API key", dynamic("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1")),
    "xai"                   => (XAI, "xAI", "XAI_API_KEY", "xAI API key", "xai"),
    "xiaomi"                => (XIAOMI, "Xiaomi", "XIAOMI_API_KEY", "Xiaomi API key", "xiaomi"),
    "xiaomi-token-plan-ams" => (XIAOMI_TP_AMS, "Xiaomi Token Plan AMS", "XIAOMI_TOKEN_PLAN_AMS_API_KEY", "Xiaomi Token Plan AMS API key", "xiaomi-token-plan-ams"),
    "xiaomi-token-plan-cn"  => (XIAOMI_TP_CN, "Xiaomi Token Plan CN", "XIAOMI_TOKEN_PLAN_CN_API_KEY", "Xiaomi Token Plan CN API key", "xiaomi-token-plan-cn"),
    "xiaomi-token-plan-sgp" => (XIAOMI_TP_SGP, "Xiaomi Token Plan SGP", "XIAOMI_TOKEN_PLAN_SGP_API_KEY", "Xiaomi Token Plan SGP API key", "xiaomi-token-plan-sgp"),
    "zai"                   => (ZAI, "Z.AI", "ZAI_API_KEY", "Z.AI API key", "zai"),
    "zai-coding-cn"         => (ZAI_CODING_CN, "Z.AI Coding CN", "ZAI_CODING_CN_API_KEY", "Z.AI Coding CN API key", "zai-coding-cn"),
}

impl FleetSpec {
    /// Parse the embedded catalog into [`Model`]s. Catalogs are compile-time constants extracted
    /// verbatim from Pi; a parse failure yields an empty catalog (surfaced loudly by the
    /// catalog-count tests) rather than a panic (NO-PANIC policy). A [`FleetCatalog::Dynamic`]
    /// member has no embedded rows and yields an empty catalog by construction.
    pub fn models(&self) -> Vec<Model> {
        match self.catalog {
            FleetCatalog::Embedded(json) => serde_json::from_str(json).unwrap_or_default(),
            FleetCatalog::Dynamic => Vec::new(),
        }
    }

    /// `true` for the members whose rows are not embedded (see the module doc).
    pub fn is_dynamic(&self) -> bool {
        self.catalog == FleetCatalog::Dynamic
    }

    /// The provider's [`ProviderAuth`]: an API key from its env var (Pi `envApiKeyAuth`), plus the
    /// `lazyOAuth` clause for the two fleet members that have one — `xai`
    /// (`providers/xai.ts:15-20`) and `openrouter` (`providers/openrouter.ts:14-18`). See
    /// [`super::builtin_oauth::builtin_provider_oauth`].
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
        let provider = WireProvider::new(
            self.id,
            self.name,
            self.models(),
            self.auth(),
            store,
            registry,
        );
        match self.base_url {
            Some(base_url) => provider.with_base_url(base_url),
            None => provider,
        }
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
        ("moonshotai", 10),
        ("moonshotai-cn", 10),
        ("nvidia", 20),
        ("openrouter", 271),
        // PROV-058: pi's generator drops five xai ids via `XAI_BUILTIN_EXCLUDED_MODEL_IDS`
        // (`ai/scripts/generate-models.ts:379-385` @v0.83.0, consumed at `:2078`) — `grok-3`,
        // `grok-3-fast`, `grok-4.20-0309-non-reasoning`, `grok-4.20-0309-reasoning` and
        // `grok-code-fast-1`. cyrup shipped all five until the catalogs were regenerated.
        ("xai", 3),
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
            // Every model is openai-completions and tagged with the provider id — EXCEPT
            // `xai/grok-4.5`, which pi routes over the Responses API (PROV-054). The fleet is
            // named for the protocol its members mostly speak, not one they all speak:
            // `WireProvider` dispatches per `model.api` (`wire.rs:215`), so a single Responses row
            // inside an otherwise-completions catalog is exactly what upstream has and what cyrup
            // must reproduce. The carve-out is spelled as an id, not a count, so a SECOND row
            // drifting off the completions protocol still fails here.
            assert!(
                models.iter().all(|m| m.api.as_str() == OPENAI_COMPLETIONS
                    || (*id == "xai" && m.id.as_str() == "grok-4.5")),
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
    fn fleet_has_nineteen_providers() {
        assert_eq!(FLEET.len(), 19);
        // Every fleet provider has an env-key mapping in env-api-keys.
        for spec in FLEET {
            let vars = crate::env_api_keys::api_key_env_vars(spec.id)
                .unwrap_or_else(|| panic!("no env mapping for {}", spec.id));
            assert!(vars.contains(&spec.env_var), "{} env var mismatch", spec.id);
        }
    }

    /// PROV-014 — the three Qwen Token Plan members, field for field against
    /// `providers/qwen-token-plan{,-cn,-individual}.ts:6-15` @v0.84.4 and `env-api-keys.ts:81-83`.
    /// Their catalogs are `Dynamic` (module doc), so the provider-level `baseUrl` is the one
    /// `createProvider({ baseUrl })` carries, and every other embedded member stays `None`.
    #[test]
    fn qwen_token_plan_members_match_upstream() {
        let expected: &[(&str, &str, &str, &str, &str)] = &[
            (
                "qwen-token-plan",
                "Qwen Token Plan",
                "QWEN_TOKEN_PLAN_API_KEY",
                "Qwen Token Plan API key",
                "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
            ),
            (
                "qwen-token-plan-cn",
                "Qwen Token Plan CN",
                "QWEN_TOKEN_PLAN_CN_API_KEY",
                "Qwen Token Plan CN API key",
                "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            ),
            (
                "qwen-token-plan-individual",
                "Qwen Token Plan Individual",
                "QWEN_TOKEN_PLAN_API_KEY",
                "Qwen Token Plan Individual API key",
                "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
            ),
        ];
        for (id, name, env, auth, base_url) in expected {
            let spec = fleet_spec(id).unwrap_or_else(|| panic!("{id} is a fleet member"));
            assert_eq!(spec.name, *name);
            assert_eq!(spec.env_var, *env);
            assert_eq!(spec.auth_name, *auth);
            assert_eq!(spec.base_url, Some(*base_url));
            assert!(
                spec.is_dynamic(),
                "{id} ships no embedded rows (module doc)"
            );
            assert!(spec.models().is_empty());
            let p = spec.provider();
            assert_eq!(p.id().as_str(), *id);
            assert_eq!(Provider::name(&p), *name);
            assert_eq!(p.base_url(), Some(*base_url));
            let auth = p.provider_auth().expect("auth");
            assert_eq!(
                auth.api_key.as_ref().expect("apiKey").name(),
                spec.auth_name
            );
            assert!(auth.oauth.is_none(), "{id}: no lazyOAuth upstream");
        }
        // `all.ts:118-120` @v0.84.4 places them right after openrouter, in this order.
        let ids: Vec<&str> = FLEET.iter().map(|s| s.id).collect();
        let at = ids
            .iter()
            .position(|id| *id == "openrouter")
            .expect("openrouter");
        assert_eq!(
            &ids[at..at + 4],
            &[
                "openrouter",
                "qwen-token-plan",
                "qwen-token-plan-cn",
                "qwen-token-plan-individual"
            ]
        );
        // Exactly these three are dynamic; every embedded member carries its rows' own baseUrl.
        let dynamic: Vec<&str> = FLEET
            .iter()
            .filter(|s| s.is_dynamic())
            .map(|s| s.id)
            .collect();
        assert_eq!(
            dynamic,
            [
                "qwen-token-plan",
                "qwen-token-plan-cn",
                "qwen-token-plan-individual"
            ]
        );
        assert!(
            FLEET
                .iter()
                .filter(|s| !s.is_dynamic())
                .all(|s| s.base_url.is_none())
        );
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

    /// `[CYRUP-DELTA]` — pi `GROQ_MODELS["qwen/qwen3-32b"].thinkingLevelMap`
    /// (`groq.models.ts` @`b0c2a90e`; the override that puts it there is
    /// `ai/scripts/generate-models.ts:837` @v0.83.0). cyrup ships the v0.84.1 behaviour instead,
    /// and PROV-064 asked for this tag specifically so the divergence is findable by the mechanism
    /// the project uses to find accepted divergences. It is also load-bearing now: the catalogs are
    /// generated from `b0c2a90e` (PROV-060), which HAS the map, so the generator carries this as an
    /// explicit entry in its `DELTAS` table (`xtask/src/main.rs`) and refuses to run if upstream
    /// ever stops setting the key — a silently stale exception being exactly as dangerous as a
    /// silently reverted one.
    ///
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

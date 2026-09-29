//! The Pi provider fleet (arch-01 §5). Every Pi provider that is a plain [`WireProvider`] over an
//! embedded catalog plus an env-key auth. They differ only in id/name/base URL, env-key, catalog,
//! and the wire protocol their rows speak ([`FleetWire`]); the shared compat matrix
//! ([`crate::api::compat`]) drives every per-provider behavior.
//!
//! The set was named for `openai-completions` because nineteen of the twenty members speak it, and
//! for a while the twentieth had a single Responses row (PROV-054, `xai/grok-4.5`). That framing is
//! retired: pi moved xai's WHOLE catalog onto the Responses API
//! (`ai/scripts/generate-models.ts:1877` @v0.85.1 hardcodes `api: "openai-responses"` for every row
//! it emits for the provider). Membership never meant anything about protocol at runtime —
//! [`WireProvider`] dispatches per row on `model.api` (`wire.rs:215`) — so each member now DECLARES
//! its protocol in [`FleetSpec::wire`] and the test checks rows against that declaration instead of
//! a hand-kept exception (XAI_2).
//!
//! PROV-071's refresh then made a member speak TWO: `openrouter` is
//! `Provider<"anthropic-messages" | "openai-completions">` at v0.87.1
//! (`packages/ai/src/providers/openrouter.ts:8,21-25`), where it was single-API at `b0c2a90e`, and
//! its refreshed catalog carries 15 Messages rows among 393. The declaration became a SET rather
//! than one value for that ([`FleetWire::apis`]); nothing in the request path changed, because
//! per-row dispatch had always covered it.
//!
//! # Where a member's catalog comes from (PROV-014, DRIFT-009, PROV-071)
//!
//! Every fleet member with rows now ships an EMBEDDED catalog, and every one of those catalogs is
//! fetched live from `pi.dev/api/models/providers/<id>` by `xtask gen-catalogs` and committed under
//! `catalog/`. [`FleetCatalog::Dynamic`] — no `catalog/*.json` at all — is left with no member here;
//! `radius` is the only provider in the tree that still has none, and it is not a fleet member
//! because its rows come from its own gateway rather than from pi.
//!
//! **This block used to say the opposite, and the history is worth keeping** because it is the
//! reason to distrust "the data is unobtainable" as a conclusion. The three `qwen-token-plan*`
//! members and `baseten` were `Dynamic` on the reasoning that their rows are in git at NO
//! revision: the providers were added upstream at `bbb91fa8a` (2026-07-20), `c03d78bdc`
//! (2026-08-06) and `c1019d920` (2026-08-03), all AFTER `b0c2a90e` — the last revision at which any
//! `*.models.ts` was a data literal — and their data is generated into a gitignored
//! `providers/data/*.json`. Both halves of that were true and stayed true; DRIFT-009 showed the
//! CONCLUSION was still wrong, because pi publishes the same generated artifact over HTTP, and
//! PROV-071 then showed the same thing was true of the other 33 catalogs, which had been frozen at
//! `b0c2a90e` for exactly the same reason without anyone calling them blocked.
//!
//! What IS in git for these four — the ids, `baseUrl`, compat, thinking maps
//! (`qwen-token-plan-models.test.ts` @v0.84.4) — is recorded on each member's doc comment below,
//! and it is what the live rows are checked against. Registering `baseten` also required porting
//! the `baseten` thinking format ([`crate::api::compat::ThinkingFormat::Baseten`]): every reasoning
//! row it serves carries it, and an unknown format fails the whole row.
//!
//! The runtime catalog still arrives on top of all of this from the pi.dev overlay
//! ([`crate::remote_catalog`], which fetches the SAME endpoint per registered provider) and from
//! `models.json`. The embedded files are the offline and first-run floor, not a cache of it.

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
    /// The wire protocol this member's rows speak — see [`FleetWire`].
    pub wire: FleetWire,
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

/// The wire protocol every row of a fleet member's catalog speaks.
///
/// A DECLARATION, not a hint: the catalog test asserts each row's `api` against it, and the
/// `fleet!` macro cannot accept a member that does not state one.
///
/// It replaces an exception that decayed twice. It began as
/// `|| (*id == "xai" && m.id == "grok-4.5")`, which at least pinned WHICH row could deviate, and was
/// widened by XAI_1 to `|| *id == "xai"` — where `*id` is loop-constant, so `m.api` stopped being
/// read at all for xai and a row on ANY protocol passed. An exception list cannot assert a positive;
/// a declaration can. It is also release-proof: it names a protocol, never a model id or a count, so
/// it survives every future xAI release unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetWire {
    /// `openai-completions` — eighteen of the twenty members, including all four
    /// [`FleetCatalog::Dynamic`] ones, whose overlay rows are `openai-completions` upstream.
    Completions,
    /// `openai-responses` — `xai` alone (`ai/scripts/generate-models.ts:1877` @v0.85.1).
    Responses,
    /// `anthropic-messages` **and** `openai-completions` — `openrouter` alone.
    ///
    /// Upstream states it in the signature: `openrouterProvider(): Provider<"anthropic-messages" |
    /// "openai-completions">` with an `api` MAP rather than a single impl
    /// (`packages/ai/src/providers/openrouter.ts:8,21-25` @v0.87.1). At `b0c2a90e` — the revision
    /// every embedded catalog was frozen at until PROV-071 — the same file declared
    /// `Provider<"openai-completions">` with one impl, which is why a single-protocol declaration
    /// was ever enough. The refreshed catalog carries 15 `anthropic-messages` rows among 393, and
    /// they were the first thing the refresh surfaced.
    ///
    /// Nothing in the REQUEST PATH changed with it: [`WireProvider`] has always dispatched per row
    /// on `model.api` through the [`ApiRegistry`] (`wire.rs`), so both protocols already routed.
    /// What could not be expressed was the declaration, and a declaration that cannot say what
    /// upstream says is a declaration that has to be widened by exception next time.
    MessagesAndCompletions,
}

impl FleetWire {
    /// Every `Model::api` value a row of a member declaring this protocol may carry.
    ///
    /// A SET rather than one value, because upstream's own `api` field is a set for `openrouter`.
    /// The test still asserts the positive — each row's `api` must be IN this list — so a row that
    /// regressed to a protocol the member does not declare still fails.
    #[must_use]
    pub fn apis(self) -> &'static [&'static str] {
        match self {
            Self::Completions => &[crate::known_api::OPENAI_COMPLETIONS],
            Self::Responses => &[crate::known_api::OPENAI_RESPONSES],
            Self::MessagesAndCompletions => &[
                crate::known_api::ANTHROPIC_MESSAGES,
                crate::known_api::OPENAI_COMPLETIONS,
            ],
        }
    }
}

/// The catalog half of a `fleet!` row. Three forms, because "has rows" and "carries a
/// provider-level `baseUrl`" are INDEPENDENT facts and conflating them cost DRIFT-009 a regression:
///
/// * `<stem>` embeds `catalog/<stem>.json` and carries no provider-level `baseUrl` — every row of
///   such a catalog declares its own, which is what the request path reads.
/// * `embedded(<stem>, <url>)` embeds the catalog AND keeps `<url>`, for the members whose upstream
///   `createProvider({ baseUrl })` sets one explicitly (`Provider.baseUrl`, PROV-017). DRIFT-009's
///   four are exactly these: their rows became embeddable, but `providers/baseten.ts:6-14` and
///   `providers/qwen-token-plan*.ts:6-15` still pass a `baseUrl`, so dropping it while flipping
///   `Dynamic` -> `Embedded` would silently lose `Provider.baseUrl` for four providers.
/// * `dynamic(<url>)` declares a member with no embedded rows at all, keeping only that `baseUrl`.
macro_rules! fleet_catalog {
    (dynamic($base_url:literal)) => {
        (FleetCatalog::Dynamic, Some($base_url))
    };
    (embedded($file:literal, $base_url:literal)) => {
        (
            FleetCatalog::Embedded(include_str!(concat!("catalog/", $file, ".json"))),
            Some($base_url),
        )
    };
    ($file:literal) => {
        (
            FleetCatalog::Embedded(include_str!(concat!("catalog/", $file, ".json"))),
            None::<&'static str>,
        )
    };
}

macro_rules! fleet {
    ($($id:literal => ($const:ident, $name:literal, $env:literal, $auth:literal, $wire:ident, $($catalog:tt)+)),* $(,)?) => {
        $(
            pub const $const: FleetSpec = FleetSpec {
                id: $id,
                name: $name,
                env_var: $env,
                auth_name: $auth,
                wire: FleetWire::$wire,
                catalog: fleet_catalog!($($catalog)+).0,
                base_url: fleet_catalog!($($catalog)+).1,
            };
        )*

        /// Every fleet spec (stable order matching Pi's `builtinProviders()` listing).
        pub const FLEET: &[FleetSpec] = &[$($const),*];
    };
}

fleet! {
    "ant-ling"              => (ANT_LING, "Ant Ling", "ANT_LING_API_KEY", "Ant Ling API key", Completions, "ant-ling"),
    // DRIFT-009 — `providers/baseten.ts:6-14` @v0.84.4, registered `all.ts:95`; a v0.84.x
    // addition (`c1019d920`, 2026-08-03 — absent at the ported baseline v0.83.0). models.dev
    // source `baseten`, generated by `generate-models.ts::processBasetenModels` (`:1256-1345`),
    // whose `baseUrl` (`:1259`) is the literal below. Every row is `api: "openai-completions"`
    // with one of two compat blocks: `toggleReasoningCompat` / `toggleReasoningEffortCompat`
    // (`:1274-1283`, `thinkingFormat: "baseten"` plus `chatTemplateArgs: {enable_thinking:
    // {$var: "thinking.enabled"}}`) for a toggle model, `reasoningEffortCompat` / `baseCompat`
    // (`:1260-1273`) otherwise. The one row upstream's own test pins in full is
    // `zai-org/GLM-5.2` (`test/baseten-models.test.ts:19-54`), which is also cyrup's default
    // model for the provider (`cyrup-config/src/model/defaults.rs:37`). See the module doc for
    // why the rows themselves are not embedded.
    "baseten"               => (BASETEN, "Baseten", "BASETEN_API_KEY", "Baseten API key", Completions, embedded("baseten", "https://inference.baseten.co/v1")),
    "cerebras"              => (CEREBRAS, "Cerebras", "CEREBRAS_API_KEY", "Cerebras API key", Completions, "cerebras"),
    "deepseek"              => (DEEPSEEK, "DeepSeek", "DEEPSEEK_API_KEY", "DeepSeek API key", Completions, "deepseek"),
    "groq"                  => (GROQ, "Groq", "GROQ_API_KEY", "Groq API key", Completions, "groq"),
    "huggingface"           => (HUGGINGFACE, "Hugging Face", "HF_TOKEN", "Hugging Face token", Completions, "huggingface"),
    "moonshotai"            => (MOONSHOTAI, "Moonshot AI", "MOONSHOT_API_KEY", "Moonshot AI API key", Completions, "moonshotai"),
    "moonshotai-cn"         => (MOONSHOTAI_CN, "Moonshot AI CN", "MOONSHOT_API_KEY", "Moonshot AI API key", Completions, "moonshotai-cn"),
    "nvidia"                => (NVIDIA, "NVIDIA", "NVIDIA_API_KEY", "NVIDIA API key", Completions, "nvidia"),
    // `providers/openrouter.ts:8,21-25` @v0.87.1 declares TWO apis; at `b0c2a90e` it declared one
    // (`Provider<"openai-completions">`). See `FleetWire::MessagesAndCompletions`.
    "openrouter"            => (OPENROUTER, "OpenRouter", "OPENROUTER_API_KEY", "OpenRouter API key", MessagesAndCompletions, "openrouter"),
    // PROV-014 — `providers/qwen-token-plan.ts:6-15` @v0.84.4 (identical at v0.83.0), registered at
    // `all.ts:118`. models.dev source `alibaba-token-plan`; the ids upstream's own test pins as
    // present (`qwen-token-plan-models.test.ts:42-58` @v0.84.4): MiniMax-M2.5, deepseek-v3.2,
    // deepseek-v4-flash, deepseek-v4-pro, glm-5, glm-5.1, glm-5.2, kimi-k2.5, kimi-k2.6,
    // kimi-k2.7-code, qwen3.6-flash, qwen3.6-plus, qwen3.7-max, qwen3.7-plus, qwen3.8-max — every
    // row `compat: { thinkingFormat: "qwen", supportsDeveloperRole: false, supportsStore: false }`
    // (`generate-models.ts:2308-2313`), `reasoning_effort` only on the deepseek-v4-*/glm-5* rows
    // (`:306-316`). See the module doc for why the rows themselves are not embedded.
    "qwen-token-plan"       => (QWEN_TOKEN_PLAN, "Qwen Token Plan", "QWEN_TOKEN_PLAN_API_KEY", "Qwen Token Plan API key", Completions, embedded("qwen-token-plan", "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1")),
    // PROV-014 — `providers/qwen-token-plan-cn.ts:6-15` @v0.84.4 (identical at v0.83.0),
    // `all.ts:119`. models.dev source `alibaba-token-plan-cn`; same id set as the international
    // plan, China endpoint, its own key.
    "qwen-token-plan-cn"    => (QWEN_TOKEN_PLAN_CN, "Qwen Token Plan CN", "QWEN_TOKEN_PLAN_CN_API_KEY", "Qwen Token Plan CN API key", Completions, embedded("qwen-token-plan-cn", "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1")),
    // VERSION LAG (v0.83.0 → v0.84.4): `providers/qwen-token-plan-individual.ts:6-15`, added at
    // `c03d78bdc` (#7659), `all.ts:120`. The international endpoint and the SAME env var as
    // `qwen-token-plan` (`env-api-keys.ts:83`: `"qwen-token-plan-individual":
    // "QWEN_TOKEN_PLAN_API_KEY"`), narrowed to the eight-model personal allowlist
    // (`generate-models.ts:324-336`; `qwen-token-plan-models.test.ts:60-69`).
    "qwen-token-plan-individual" => (QWEN_TOKEN_PLAN_INDIVIDUAL, "Qwen Token Plan Individual", "QWEN_TOKEN_PLAN_API_KEY", "Qwen Token Plan Individual API key", Completions, embedded("qwen-token-plan-individual", "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1")),
    // XAI_2 — the one Responses member, and the one whose catalog is LIVE-FETCHED (XAI_1). The
    // declaration is what a freshly downloaded file gets checked against.
    "xai"                   => (XAI, "xAI", "XAI_API_KEY", "xAI API key", Responses, "xai"),
    "xiaomi"                => (XIAOMI, "Xiaomi", "XIAOMI_API_KEY", "Xiaomi API key", Completions, "xiaomi"),
    "xiaomi-token-plan-ams" => (XIAOMI_TP_AMS, "Xiaomi Token Plan AMS", "XIAOMI_TOKEN_PLAN_AMS_API_KEY", "Xiaomi Token Plan AMS API key", Completions, "xiaomi-token-plan-ams"),
    "xiaomi-token-plan-cn"  => (XIAOMI_TP_CN, "Xiaomi Token Plan CN", "XIAOMI_TOKEN_PLAN_CN_API_KEY", "Xiaomi Token Plan CN API key", Completions, "xiaomi-token-plan-cn"),
    "xiaomi-token-plan-sgp" => (XIAOMI_TP_SGP, "Xiaomi Token Plan SGP", "XIAOMI_TOKEN_PLAN_SGP_API_KEY", "Xiaomi Token Plan SGP API key", Completions, "xiaomi-token-plan-sgp"),
    "zai"                   => (ZAI, "Z.AI", "ZAI_API_KEY", "Z.AI API key", Completions, "zai"),
    "zai-coding-cn"         => (ZAI_CODING_CN, "Z.AI Coding CN", "ZAI_CODING_CN_API_KEY", "Z.AI Coding CN API key", Completions, "zai-coding-cn"),
}

impl FleetSpec {
    /// Parse the embedded catalog into [`Model`]s. Catalogs are compile-time constants extracted
    /// verbatim from Pi; a parse failure yields an empty catalog (surfaced loudly by the
    /// catalog-count tests) rather than a panic (NO-PANIC policy). A [`FleetCatalog::Dynamic`]
    /// member has no embedded rows and yields an empty catalog by construction.
    pub fn models(&self) -> Vec<Model> {
        match self.catalog {
            FleetCatalog::Embedded(json) => crate::catalog::load_catalog(json).unwrap_or_default(),
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
    use crate::provider::Provider;
    use crate::stream::StreamOptions;
    use cyrup_core::ModelThinkingLevel;

    /// Per-provider catalog sizes, one per fleet member that ships rows.
    ///
    /// # Why `xai` is no longer the exception, and why this table grew instead of shrinking
    ///
    /// This used to carry fifteen of the twenty members and a comment explaining that `xai` was
    /// DELIBERATELY absent (XAI_2): its catalog was re-downloaded on every `gen-catalogs` run, so
    /// "its row count is a property of what xAI is selling this week — pinning it schedules a red
    /// test for the next model launch". PROV-071 made every provider catalog live-fetched, so read
    /// literally that rule would empty the table.
    ///
    /// It empties nothing, because the premise was wrong in a way one live catalog was too small to
    /// show. A count here is a property of the COMMITTED file under `providers/catalog/`, not of
    /// the endpoint: `gen-catalogs` is run by hand, there is no CI (see `xtask`'s module docs), and
    /// the file changes only when a maintainer regenerates and reviews the diff. A launch upstream
    /// cannot turn this red. A regeneration can, and that is the moment the roster SHOULD be
    /// looked at — the refresh that closed PROV-071 moved openrouter 271 -> 393, amazon-bedrock
    /// 109 -> 174 and moonshotai 10 -> 4, and a table that opted out of counting would have let all
    /// three land unread. So `xai` gains the count it never had, and so does every other member.
    const EXPECTED_COUNTS: &[(&str, usize)] = &[
        ("ant-ling", 3),
        ("baseten", 21),
        ("cerebras", 2),
        ("deepseek", 2),
        ("groq", 7),
        ("huggingface", 76),
        ("moonshotai", 4),
        ("moonshotai-cn", 4),
        ("nvidia", 19),
        ("openrouter", 393),
        ("qwen-token-plan", 20),
        ("qwen-token-plan-cn", 20),
        ("qwen-token-plan-individual", 9),
        ("xai", 4),
        ("xiaomi", 6),
        ("xiaomi-token-plan-ams", 4),
        ("xiaomi-token-plan-cn", 4),
        ("xiaomi-token-plan-sgp", 4),
        ("zai", 7),
        ("zai-coding-cn", 4),
    ];

    #[test]
    fn every_catalog_parses_with_expected_count() {
        // Counts, for every member that ships rows — no exceptions, and the table must be TOTAL:
        // a member that gains a catalog without gaining a row here would otherwise be uncounted.
        let counted: Vec<&str> = EXPECTED_COUNTS.iter().map(|(id, _)| *id).collect();
        for spec in FLEET.iter().filter(|s| !s.is_dynamic()) {
            assert!(
                counted.contains(&spec.id),
                "{} ships an embedded catalog with no EXPECTED_COUNTS row",
                spec.id
            );
        }
        for (id, count) in EXPECTED_COUNTS {
            let spec = fleet_spec(id).unwrap_or_else(|| panic!("no spec for {id}"));
            assert_eq!(
                spec.models().len(),
                *count,
                "catalog count mismatch for {id}"
            );
        }

        // Provenance: every embedded catalog must say in `catalog_manifest.json` where its rows
        // came from, and a live-fetched one must carry the stamp its last successful fetch left.
        // A count says a catalog is the size somebody wrote down; this says its origin is on
        // record, which is the half a silently-stale catalog cannot fake (PROV-060).
        let manifest: serde_json::Value =
            serde_json::from_str(crate::providers::all::BUILTIN_CATALOG_MANIFEST_JSON)
                .expect("catalog_manifest.json parses");
        let catalogs = manifest
            .get("catalogs")
            .and_then(serde_json::Value::as_object)
            .expect("catalog_manifest.json has a `catalogs` map");
        for spec in FLEET.iter().filter(|s| !s.is_dynamic()) {
            let entry = catalogs
                .get(spec.id)
                .unwrap_or_else(|| panic!("{}: no catalog_manifest.json entry", spec.id));
            let source = entry.get("source").and_then(serde_json::Value::as_str);
            assert!(
                source.is_some_and(|src| !src.is_empty()),
                "{}: manifest entry names no source",
                spec.id
            );
            // Live-fetched catalogs carry their own stamp; a null one means the last `gen-catalogs`
            // run could not reach pi.dev and the file on disk is older than the manifest claims.
            if source.is_some_and(|src| src.starts_with("https://")) {
                for key in ["fetchedAt", "revision"] {
                    assert!(
                        entry.get(key).and_then(serde_json::Value::as_str).is_some(),
                        "{}: live catalog has no `{key}` — its provenance is unrecorded",
                        spec.id
                    );
                }
            }
        }

        // Invariants, for EVERY member that ships rows — pinned or live-fetched. None of these
        // names a model id or a count, so they hold across any future catalog refresh.
        for spec in FLEET.iter().filter(|s| !s.is_dynamic()) {
            let models = spec.models();
            // A parse failure yields an empty catalog silently (`FleetSpec::models` uses
            // `unwrap_or_default`), so this is the guard that a live download landing as garbage
            // fails loudly instead of shipping a provider with no models.
            assert!(!models.is_empty(), "{} parsed to an empty catalog", spec.id);
            // Every row speaks the protocol its member DECLARES. Asserts the POSITIVE: an xai row
            // that regressed to Completions fails here, which neither the original `grok-4.5`
            // carve-out nor XAI_1's whole-provider widening could see.
            let apis = spec.wire.apis();
            assert!(
                models.iter().all(|m| apis.contains(&m.api.as_str())),
                "{}: every row must speak one of {apis:?}, the protocols its FleetSpec::wire \
                 declares",
                spec.id
            );
            // And the declaration is checked in the other direction too: a member that declares a
            // protocol no row speaks is a stale declaration, which is how `Responses` would have
            // survived xai moving back.
            for api in apis {
                assert!(
                    models.iter().any(|m| m.api.as_str() == *api),
                    "{}: declares `{api}` but no row speaks it — the declaration is stale",
                    spec.id
                );
            }
            assert!(
                models.iter().all(|m| m.provider.as_str() == spec.id),
                "{} provider tag",
                spec.id
            );
            assert!(
                models.iter().all(|m| !m.base_url.is_empty()),
                "{} baseUrl",
                spec.id
            );
        }

        // The declaration is checkable in BOTH directions: exactly one member is on Responses
        // today, and it is xai. A second one arriving without a ledger entry is the signal that this
        // module's framing needs revisiting again.
        let responses: Vec<&str> = FLEET
            .iter()
            .filter(|s| s.wire == FleetWire::Responses)
            .map(|s| s.id)
            .collect();
        assert_eq!(responses, ["xai"]);
    }

    #[test]
    fn fleet_has_twenty_providers() {
        assert_eq!(FLEET.len(), 20);
        // Every fleet provider has an env-key mapping in env-api-keys.
        for spec in FLEET {
            let vars = crate::env_api_keys::api_key_env_vars(spec.id)
                .unwrap_or_else(|| panic!("no env mapping for {}", spec.id));
            assert!(vars.contains(&spec.env_var), "{} env var mismatch", spec.id);
        }
    }

    /// PROV-014 — the three Qwen Token Plan members, field for field against
    /// `providers/qwen-token-plan{,-cn,-individual}.ts:6-15` @v0.84.4 and `env-api-keys.ts:81-83`.
    ///
    /// DRIFT-009 moved all three from `Dynamic` to `Embedded`, so the assertions flipped with them:
    /// each now ships rows and each row must be tagged with its own provider id. The provider-level
    /// `baseUrl` is asserted UNCHANGED, and that is the point of the `embedded(<stem>, <url>)` macro
    /// arm: upstream's `createProvider({ baseUrl })` still passes one for these three (and for
    /// `baseten`) whether or not a catalog exists, so gaining a catalog must not cost
    /// `Provider.baseUrl`.
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
            assert_eq!(
                spec.base_url,
                Some(*base_url),
                "{id}: `createProvider({{ baseUrl }})` survives the flip to Embedded (DRIFT-009)"
            );
            assert!(
                !spec.is_dynamic(),
                "{id} ships embedded rows since DRIFT-009"
            );
            let rows = spec.models();
            assert!(!rows.is_empty(), "{id} parsed to an empty catalog");
            assert!(
                rows.iter().all(|m| m.provider.as_str() == *id),
                "{id}: every row must be tagged with its own provider id — `rows_from_body` \
                 enforces that at fetch time, this is the same check at LOAD time"
            );
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
        // DRIFT-009 — NO fleet member is dynamic any more. `radius` is the workspace's only
        // remaining catalog-less built-in and it is not a fleet member (its rows come from the
        // customer's own gateway, not from a published artifact), so this is an empty set and
        // asserting it empty is what makes a half-finished revert fail here rather than downstream.
        let dynamic: Vec<&str> = FLEET
            .iter()
            .filter(|s| s.is_dynamic())
            .map(|s| s.id)
            .collect();
        assert!(
            dynamic.is_empty(),
            "DRIFT-009 embedded the last four; {dynamic:?} is still Dynamic"
        );
        // The four DRIFT-009 members are the ONLY embedded members carrying a provider-level
        // `baseUrl`: upstream passes one to `createProvider` for exactly these four, and every other
        // member's rows carry their own.
        let with_base_url: Vec<&str> = FLEET
            .iter()
            .filter(|s| s.base_url.is_some())
            .map(|s| s.id)
            .collect();
        assert_eq!(
            with_base_url,
            [
                "baseten",
                "qwen-token-plan",
                "qwen-token-plan-cn",
                "qwen-token-plan-individual"
            ],
            "in `all.ts` order"
        );
    }

    /// DRIFT-009 — the fourth blocked provider, field for field against `providers/baseten.ts:6-14`
    /// @v0.84.4 (`all.ts:95`, `env-api-keys.ts:106`) and the hardcoded `baseUrl` its generator
    /// carries (`ai/scripts/generate-models.ts:1259`).
    ///
    /// It sits between `ant-ling` and `cerebras`, which is `all.ts`'s own order (`:92`, `:95`,
    /// `:96`). DRIFT-009 turned it from `Dynamic` into an `Embedded` member fetched from
    /// `pi.dev/api/models/providers/baseten`, so the emptiness assertion became a rows assertion —
    /// and the `thinkingFormat: "baseten"` check below stopped being hypothetical: 10 of the 21
    /// fetched rows really do carry it, and an unknown format fails the whole row, which would leave
    /// the provider registered and half empty rather than loudly broken.
    #[test]
    fn baseten_matches_upstream_and_ships_its_fetched_rows() {
        let spec = fleet_spec("baseten").expect("baseten is a fleet member");
        assert_eq!(spec.name, "Baseten");
        assert_eq!(spec.env_var, "BASETEN_API_KEY");
        assert_eq!(spec.auth_name, "Baseten API key");
        assert_eq!(
            spec.base_url,
            Some("https://inference.baseten.co/v1"),
            "generate-models.ts:1259 @v0.84.4"
        );
        assert!(
            !spec.is_dynamic(),
            "DRIFT-009 — baseten's rows are live-fetched into an embedded catalog"
        );

        let p = spec.provider();
        assert_eq!(p.id().as_str(), "baseten");
        assert_eq!(Provider::name(&p), "Baseten");
        assert_eq!(p.base_url(), Some("https://inference.baseten.co/v1"));
        let auth = p.provider_auth().expect("auth");
        assert_eq!(
            auth.api_key.as_ref().expect("apiKey").name(),
            "Baseten API key"
        );
        assert!(auth.oauth.is_none(), "no lazyOAuth in providers/baseten.ts");

        let ids: Vec<&str> = FLEET.iter().map(|s| s.id).collect();
        let at = ids
            .iter()
            .position(|id| *id == "ant-ling")
            .expect("ant-ling");
        assert_eq!(&ids[at..at + 3], &["ant-ling", "baseten", "cerebras"]);

        // The compat block Baseten's TOGGLE-reasoning rows carry must round-trip, or the overlay
        // delivers those rows as nothing. `processBasetenModels` selects one of four blocks per row
        // (`generate-models.ts:1310-1319` @v0.84.4); only `toggleReasoningCompat` /
        // `toggleReasoningEffortCompat` (`:1274-1283`) carry `thinkingFormat: "baseten"` — an
        // effort-only row carries `"openai"` (`:1269-1273`) and a non-reasoning row carries none
        // (`:1260-1268`).
        let compat: crate::api::compat::ModelCompat = serde_json::from_str(
            r#"{"thinkingFormat":"baseten","chatTemplateArgs":{"enable_thinking":{"$var":"thinking.enabled"}}}"#,
        )
        .expect("a Baseten compat block parses");
        assert_eq!(
            compat.thinking_format,
            Some(crate::api::compat::ThinkingFormat::Baseten)
        );
        assert!(compat.chat_template_args.is_some());

        // DRIFT-009 — and the SHIPPED catalog really exercises it. `zai-org/GLM-5.2` is the row
        // upstream's own `test/baseten-models.test.ts:19-54` pins in full and the id
        // `cyrup-config`'s `default_model_per_provider` names as this provider's default, so a
        // default that cannot resolve is a user-visible failure, not a test detail.
        let rows = spec.models();
        assert!(!rows.is_empty(), "baseten parsed to an empty catalog");
        assert!(
            rows.iter().all(|m| m.provider.as_str() == "baseten"),
            "every fetched row must be tagged `baseten`"
        );
        let glm = rows
            .iter()
            .find(|m| m.id.as_str() == "zai-org/GLM-5.2")
            .expect("the provider default `zai-org/GLM-5.2` must be in the shipped catalog");
        let glm_compat = glm.compat.as_ref().expect("GLM-5.2 declares a compat");
        assert_eq!(
            glm_compat.thinking_format,
            Some(crate::api::compat::ThinkingFormat::Baseten),
            "the toggle-reasoning block (`generate-models.ts:1274-1283`) survived the fetch"
        );
        assert!(
            glm_compat.chat_template_args.is_some(),
            "`chatTemplateArgs: {{enable_thinking: {{$var: thinking.enabled}}}}` survived the fetch"
        );
        assert!(
            rows.iter()
                .any(|m| m.compat.as_ref().and_then(|c| c.thinking_format)
                    == Some(crate::api::compat::ThinkingFormat::Baseten)),
            "no row carries the baseten thinking format"
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

    /// PROV-074, the half that actually protects users: the shipped `deepseek.json` rows carry a
    /// `compat` block with no `maxTokensField`, so the field each row resolves to is whatever
    /// `detect_compat` produces. At HEAD that was `max_completion_tokens` for both rows.
    #[test]
    fn deepseek_catalog_rows_resolve_to_max_tokens() {
        for m in DEEPSEEK.models().iter() {
            let resolved = crate::api::compat::get_compat(m);
            assert_eq!(
                resolved.max_tokens_field,
                crate::api::compat::MaxTokensField::MaxTokens,
                "{} must cap with max_tokens",
                m.id.as_str()
            );
        }
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
        // The row the retargeting moved the override OFF is gone from upstream entirely, so there
        // is nothing left for it to be wrongly re-attached to. `xtask`'s CONVERGED table asserts
        // the same absence at generation time and hard-errors if it comes back.
        assert!(
            !models.iter().any(|m| m.id.as_str() == "qwen/qwen3-32b"),
            "qwen/qwen3-32b is retired upstream; if it is back, PROV-064's override decision has \
             to be re-taken rather than inherited"
        );
        // The row the override moved ONTO carries it, from upstream's own data rather than from a
        // generator exception. This is the positive half the old assertion could not make: while
        // the catalog was frozen at b0c2a90e no Groq row had a map at all, so "no row has one" was
        // as true of a correct catalog as of an empty one.
        let retargeted = models
            .iter()
            .find(|m| m.id.as_str() == "qwen/qwen3.6-27b")
            .expect("qwen/qwen3.6-27b — the id v0.84.1 retargeted the override to");
        let map = retargeted
            .thinking_level_map
            .as_ref()
            .expect("the retargeted row carries the override");
        assert_eq!(map.get("off"), Some(&Some("none".to_string())));
        assert_eq!(map.get("high"), Some(&Some("default".to_string())));
        assert!(retargeted.reasoning);
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

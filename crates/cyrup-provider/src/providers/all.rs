//! The built-in provider registry — the L1 aggregator (1:1 port of Pi
//! `packages/ai/src/providers/all.ts`, `builtinProviders()` / `builtinModels()`).
//!
//! Pi's `all.ts` constructs EVERY built-in provider (`all.ts:70-108`) and registers each into a
//! `Models` collection (`all.ts:111-117`); the model pattern then resolves to the owning provider.
//! This module mirrors that for the providers actually implemented in this crate today.
//!
//! ## Pi `all.ts` `builtinProviders()` listing (line numbers from `all.ts:91-130` **@ `v0.84.4`**):
//!
//! | Pi line | provider id                  | here                                   |
//! |---------|------------------------------|----------------------------------------|
//! | 91      | `amazon-bedrock`             | ✓                                      |
//! | 92      | `ant-ling`                   | ✓ fleet                                |
//! | 93      | `anthropic`                  | ✓                                      |
//! | 94      | `azure-openai-responses`     | ✓                                      |
//! | 95      | `baseten`                    | ✓ fleet (dynamic catalog) — DRIFT-009  |
//! | 96      | `cerebras`                   | ✓ fleet                                |
//! | 97      | `cloudflare-ai-gateway`      | ✓                                      |
//! | 98      | `cloudflare-workers-ai`      | ✓                                      |
//! | 99      | `deepseek`                   | ✓ fleet                                |
//! | 100     | `fireworks`                  | ✓                                      |
//! | 101     | `github-copilot`             | ✓                                      |
//! | 102     | `google`                     | ✓                                      |
//! | 103     | `google-vertex`              | ✓                                      |
//! | 104     | `groq`                       | ✓ fleet                                |
//! | 105     | `huggingface`                | ✓ fleet                                |
//! | 106     | `kimi-coding`                | ✓ anthropic-compat fleet               |
//! | 107     | `minimax`                    | ✓ anthropic-compat fleet               |
//! | 108     | `minimax-cn`                 | ✓ anthropic-compat fleet               |
//! | 109     | `mistral`                    | ✓                                      |
//! | 110     | `moonshotai`                 | ✓ fleet                                |
//! | 111     | `moonshotai-cn`              | ✓ fleet                                |
//! | 112     | `nvidia`                     | ✓ fleet                                |
//! | 113     | `openai`                     | ✓                                      |
//! | 114     | `openai-codex`               | ✓                                      |
//! | 115     | `opencode`                   | ✓                                      |
//! | 116     | `opencode-go`                | ✓                                      |
//! | 117     | `openrouter`                 | ✓ fleet                                |
//! | 118     | `qwen-token-plan`            | ✓ fleet (dynamic catalog) — PROV-014   |
//! | 119     | `qwen-token-plan-cn`         | ✓ fleet (dynamic catalog) — PROV-014   |
//! | 120     | `qwen-token-plan-individual` | ✓ fleet (dynamic catalog) — PROV-014   |
//! | 121     | `radius`                     | ✓ [`super::radius`] — PROV-014         |
//! | 122     | `together`                   | ✓                                      |
//! | 123     | `vercel-ai-gateway`          | ✓ anthropic-compat fleet               |
//! | 124     | `xai`                        | ✓ fleet                                |
//! | 125     | `xiaomi`                     | ✓ fleet                                |
//! | 126-128 | `xiaomi-token-plan-*`        | ✓ fleet                                |
//! | 129     | `zai`                        | ✓ fleet                                |
//! | 130     | `zai-coding-cn`              | ✓ fleet                                |
//!
//! **All 40 of pi v0.84.4's built-in providers are registered below** (every one of the ported
//! baseline v0.83.0's 38, `all.ts:89-126` @v0.83.0, plus both v0.84.x additions —
//! `qwen-token-plan-individual` and `baseten`). Every api id the registered
//! providers' catalogs name has a registered impl — that half is not left to this comment:
//! `src/tests/catalog_data.rs`'s `every_catalog_api_has_a_registered_impl` walks all 35 catalogs and
//! asserts `builtin_registry().contains(&row.api)` for every row. Five registered providers ship
//! NO embedded catalog by design — `radius`, the three `qwen-token-plan*` members and `baseten` —
//! and `catalog_data.rs`'s `DYNAMIC_ONLY_PROVIDERS` pins that set in both directions.
//!
//! **DRIFT-009, 2026-09-05.** `baseten` (`all.ts:95` @v0.84.4, added upstream at `c1019d920`) was
//! the last unregistered built-in and the fourth of that item's four missing catalogs. It joins the
//! Qwen plans as a [`super::fleet`] member with a [`super::fleet::FleetCatalog::Dynamic`] catalog —
//! its rows are models.dev's `baseten` record, in git at no revision — and registering it required
//! porting the `baseten` thinking format it is the sole user of
//! ([`crate::api::compat::ThinkingFormat::Baseten`], `openai-completions.ts:888-904`), without which
//! every row the overlay delivers would fail to deserialize and the provider would offer nothing.
//!
//! **PROV-014, 2026-09-04.** `qwen-token-plan` / `qwen-token-plan-cn` (`all.ts:115-117` @v0.83.0)
//! and `radius` were the three v0.83.0 built-ins this file did not construct. They are now: the
//! two Qwen plans (plus v0.84.4's Individual plan) as [`super::fleet`] members whose catalog is
//! [`super::fleet::FleetCatalog::Dynamic`] — the rows are models.dev data that is in git at no
//! revision, see that module's doc — and `radius` as its own provider kind in [`super::radius`]:
//! `pi-messages`, api-key OR gateway-bound OAuth, catalog refreshed from the gateway's
//! `/v1/config` and published to the [`crate::models_store::ModelsStore`].
//!
//! **PROV-062, 2026-08-14 (sweep 9) — what this table used to say, and why it was worse than no
//! table.** It ended at `zai-coding-cn` with **no row for `all.ts:115`, `:116` or `:117`**, and then
//! asserted in prose that "Every provider pi's `builtinProviders()` constructs is registered below";
//! the guard test below went further and recorded that "Every built-in provider pi ships is now
//! ported, so there is no not-yet list left to assert against". Both were false, and `PROV-014` had
//! been open against exactly those three the whole time — so the file that documents the gap denied
//! it, in the same header that scolds an earlier sweep for this failure mode. Separately, **every
//! line number in the old table was a `91585d9a` offset carried under a declared `v0.83.0`
//! baseline**: at `91585d9a` `amazonBedrockProvider()` really is `all.ts:72`, but the three
//! providers above did not exist yet at that revision, which is precisely how their absence went
//! unnoticed when the table was transcribed. The offsets above are re-derived at `v0.83.0`.
//!
//! The table above was stale for four rows (`amazon-bedrock`, `github-copilot`, `google-vertex`,
//! `openai-codex` were marked *pending (NOT registered)* by the very sweep that registered them,
//! PROV-030), which read as "this file does not do what it does" to anyone who stopped at the
//! header. It is now accurate, and the residual it used to carry — `google-vertex` registered with
//! ten catalog rows and no wire api, so every request died with `no API implementation for
//! google-vertex` — is closed by [`crate::api::google_vertex`].
//!
//! **The one caveat left is not a registration gap.** `google-vertex`'s ADC arm mints its bearer in
//! [`crate::auth::google_adc`], which accepts `authorized_user` and `service_account` credentials
//! plus the GCE metadata server, and rejects `external_account` /
//! `impersonated_service_account` / `gdch_service_account` by name; see that module's
//! `[CYRUP-DELTA]`.
//!
//! Provider ids are unique, so the [`Models`] collection holds them in a `BTreeMap`; the `Vec`
//! ordering returned by [`all_providers`] is therefore informational only (grouped by constructor
//! for clarity) — resolution is by id, not by position.

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::{CredentialStore, InMemoryCredentialStore};
use crate::collection::{CreateModelsOptions, Models, create_models};
use crate::images::{ImagesModels, ImagesProvider, create_images_models};
use crate::provider::Provider;
use crate::providers::anthropic::anthropic_fleet_providers_with;
use crate::providers::fleet::fleet_providers_with;
use crate::providers::radius::radius_provider_with;
use crate::providers::{
    amazon_bedrock_provider_with, anthropic_provider_with, azure_openai_responses_provider_with,
    cloudflare_ai_gateway_provider_with, cloudflare_workers_ai_provider_with,
    fireworks_provider_with, github_copilot_provider_with, google_provider_with,
    google_vertex_provider_with, mistral_provider_with, openai_codex_provider_with,
    openai_provider_with, opencode_go_provider_with, opencode_provider_with,
    openrouter_images_provider, together_provider_with,
};
use crate::remote_catalog::CatalogOverlay;
use crate::utils::http_date::parse_iso8601_utc_ms;
use std::sync::Arc;

/// The generation manifest for the compiled-in catalogs under `providers/catalog/` (Pi
/// `providers/data/.manifest.json`, imported by `all.ts:12`).
pub const BUILTIN_CATALOG_MANIFEST_JSON: &str = include_str!("catalog_manifest.json");

/// Generation timestamp shared by all built-in provider catalogs, in epoch milliseconds (1:1 port of
/// Pi `getBuiltinModelDataGeneratedAt`, `all.ts:72-75`, including its `NaN` → `undefined` fold).
///
/// This is the staleness floor for the pi.dev overlay: [`crate::remote_catalog::remote_models`]
/// discards a persisted remote catalog that is not STRICTLY newer than this, so an upgrade that
/// refreshes the embedded JSON can never be shadowed by the pre-upgrade overlay (pi #7016). Before
/// DRIFT-007 cyrup carried no machine-readable stamp at all — provenance lived only in the prose of
/// `tests/catalog_data.rs`, which a program cannot compare against.
pub fn builtin_model_data_generated_at() -> Option<i64> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Manifest {
        generated_at: String,
    }
    serde_json::from_str::<Manifest>(BUILTIN_CATALOG_MANIFEST_JSON)
        .ok()
        .and_then(|m| parse_iso8601_utc_ms(&m.generated_at))
}

/// Every built-in provider that is implemented in this crate, freshly constructed over a shared
/// credential store + the built-in api registry (Pi `builtinProviders()`, `all.ts:70-108`).
///
/// The store defaults to an empty in-memory store and each [`crate::wire::WireProvider`] defaults to
/// the real-env auth context, so env API keys resolve when the provider is streamed directly.
pub fn all_providers() -> Vec<Arc<dyn Provider>> {
    all_providers_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
}

/// [`all_providers`] over an explicit credential store + shared api registry — so the whole registry
/// shares one catalog-parsing api registry (Pi constructs each provider fresh; we additionally share
/// the registry/store for cost).
pub fn all_providers_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> Vec<Arc<dyn Provider>> {
    all_providers_with_overlay(store, registry, None)
}

/// [`all_providers_with`] with an optional remote model-catalog overlay applied on top of each
/// provider's embedded catalog (Pi `builtinProviders().map(withRemoteCatalog)`,
/// `model-runtime.ts:145-151`; DRIFT-007).
///
/// The overlay is applied by wrapping, never by replacing: [`CatalogOverlay::apply`] merges by model
/// id over the embedded catalog and returns the provider UNCHANGED when it has nothing to
/// contribute. Passing `None` — which is what [`all_providers_with`] does — is byte-identical to the
/// pre-DRIFT-007 behavior, so the embedded catalogs remain the floor in every configuration.
pub fn all_providers_with_overlay(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
    overlay: Option<&CatalogOverlay>,
) -> Vec<Arc<dyn Provider>> {
    let providers = builtin_providers_with(store, registry);
    match overlay {
        None => providers,
        Some(overlay) => providers
            .into_iter()
            .map(|p| overlay.apply(p))
            .collect::<Vec<_>>(),
    }
}

fn builtin_providers_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> Vec<Arc<dyn Provider>> {
    let mut providers: Vec<Arc<dyn Provider>> = Vec::new();

    // openai-completions fleet: ant-ling, baseten, cerebras, deepseek, groq, huggingface,
    // moonshotai, moonshotai-cn, nvidia, openrouter, qwen-token-plan, qwen-token-plan-cn,
    // qwen-token-plan-individual, xai, xiaomi, xiaomi-token-plan-{ams,cn,sgp}, zai, zai-coding-cn
    // (Pi `all.ts` @v0.84.4 lines 92,95,96,99,104,105,110,111,112,117,118-120,124,125,126-128,129,130).
    for p in fleet_providers_with(store.clone(), registry.clone()) {
        providers.push(Arc::new(p));
    }

    // radius (Pi `all.ts:121` @v0.84.4; `:117` @v0.83.0) — PROV-014. Constructed WITHOUT a
    // `ModelsStore`, so through this entry point it is static: its persisted gateway catalog is
    // restored by the overlay `all_providers_with_overlay` applies (see `providers/radius.rs`).
    providers.push(Arc::new(radius_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // anthropic (Pi `all.ts:74`).
    providers.push(Arc::new(anthropic_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // azure-openai-responses (Pi `all.ts:75`).
    providers.push(Arc::new(azure_openai_responses_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // cloudflare-ai-gateway / cloudflare-workers-ai (Pi `all.ts:77-78`).
    providers.push(Arc::new(cloudflare_ai_gateway_provider_with(
        store.clone(),
        registry.clone(),
    )));
    providers.push(Arc::new(cloudflare_workers_ai_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // amazon-bedrock (pi `all.ts`) — the last of the seven that were unported.
    providers.push(Arc::new(amazon_bedrock_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // openai-codex and google-vertex (pi `all.ts`). Ported in the unported-work sweep.
    providers.push(Arc::new(openai_codex_provider_with(
        store.clone(),
        registry.clone(),
    )));
    providers.push(Arc::new(google_vertex_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // github-copilot (pi `all.ts`). Ported in the unported-work sweep; `all.rs`'s own port-status
    // table had it marked *pending*.
    providers.push(Arc::new(github_copilot_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // fireworks (Pi `all.ts:80`).
    providers.push(Arc::new(fireworks_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // google (Pi `all.ts:82`).
    providers.push(Arc::new(google_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // anthropic-compatible fleet: kimi-coding, minimax, minimax-cn, vercel-ai-gateway
    // (Pi `all.ts` lines 86,87,88,99).
    for p in anthropic_fleet_providers_with(store.clone(), registry.clone()) {
        providers.push(Arc::new(p));
    }

    // mistral (Pi `all.ts:89`).
    providers.push(Arc::new(mistral_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // openai (Pi `all.ts:93`).
    providers.push(Arc::new(openai_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // opencode / opencode-go (Pi `all.ts:95-96`).
    providers.push(Arc::new(opencode_provider_with(
        store.clone(),
        registry.clone(),
    )));
    providers.push(Arc::new(opencode_go_provider_with(
        store.clone(),
        registry.clone(),
    )));

    // together (Pi `all.ts:98`).
    providers.push(Arc::new(together_provider_with(store, registry)));

    providers
}

/// A [`Models`] collection with every implemented built-in provider registered (Pi `builtinModels`,
/// `all.ts:111-117`). Takes the same [`CreateModelsOptions`] as [`create_models`]; the default
/// (`CreateModelsOptions::default()`) uses the env-backed auth context so env API keys resolve.
/// The remote model-catalog overlay in `options`, if any, is applied to every built-in provider
/// (DRIFT-007). It defaults to `None`, so every existing caller keeps the embedded catalogs exactly
/// as before.
pub fn default_models(options: CreateModelsOptions) -> Models {
    let overlay = options.catalog_overlay.clone();
    let mut models = create_models(options);
    for provider in all_providers_with_overlay(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
        overlay.as_deref(),
    ) {
        models.set_provider(provider);
    }
    models
}

/// Every built-in image-generation provider, freshly constructed (Pi `builtinImagesProviders`,
/// `all.ts:120-122`). Currently just `openrouter-images` (Pi's only built-in image provider).
pub fn all_images_providers() -> Vec<Arc<dyn ImagesProvider>> {
    vec![Arc::new(openrouter_images_provider())]
}

/// An [`ImagesModels`] collection with every built-in image-generation provider registered (Pi
/// `builtinImagesModels`, `all.ts:125-131`).
pub fn default_images_models(options: CreateModelsOptions) -> ImagesModels {
    let mut models = create_images_models(options);
    for provider in all_images_providers() {
        models.set_provider(provider);
    }
    models
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

    /// Providers not yet ported. This is a NOT-YET list, not an exemption list: the project ports
    /// everything, and an id leaves this array by being implemented. It exists so a half-finished
    /// provider cannot be registered and silently answer requests it cannot serve — the assertion
    /// is "absent until real", never "must stay absent".
    ///
    /// PROV-062: this list was DELETED, with a comment claiming "every built-in provider pi ships
    /// is now ported, so there is no not-yet list left to assert against". That was false at the
    /// time it was written — PROV-014 was open against `qwen-token-plan`, `qwen-token-plan-cn` and
    /// `radius` — so the guard was removed in the same edit that made the claim it was guarding
    /// untrue. Restored, and it is the one place a future porter learns the set is incomplete
    /// without reading a backlog file. Each id leaves this array by being implemented, not by being
    /// reclassified: the three PROV-014 ids left on 2026-09-04, and `baseten` (`all.ts:95`
    /// @v0.84.4) left on 2026-09-05 under DRIFT-009.
    ///
    /// It is EMPTY today. Both guards below read it, and they read it in opposite directions:
    /// `registry_contains_implemented_provider_ids` asserts every id here is NOT registered, and
    /// `all_of_pis_v0_84_4_builtins_are_registered` SUBTRACTS it from the expected set so the two
    /// compose instead of contradicting each other. Parking an id therefore stays a one-line edit.
    const NOT_YET: &[&str] = &[];

    /// The registry must contain every ported built-in id and none of the not-yet-ported ones.
    #[test]
    fn registry_contains_implemented_provider_ids() {
        let models = default_models(CreateModelsOptions::default());
        let ids: Vec<String> = models
            .get_providers()
            .iter()
            .map(|p| p.id().as_str().to_string())
            .collect();

        // A representative cross-section of every constructor group.
        for expected in [
            "together",
            "anthropic",
            "google",
            "openai",
            "mistral",
            "opencode",
            "opencode-go",
            "azure-openai-responses",
            "cloudflare-ai-gateway",
            "cloudflare-workers-ai",
            "fireworks",
            // openai-completions fleet
            "groq",
            "xai",
            "deepseek",
            "openrouter",
            "moonshotai",
            // anthropic-compatible fleet
            "kimi-coding",
            "minimax",
            "vercel-ai-gateway",
            // ported in the unported-work sweep
            "github-copilot",
            "openai-codex",
            "google-vertex",
            "amazon-bedrock",
            // PROV-014 (2026-09-04): the three v0.83.0 built-ins this registry lacked, plus
            // v0.84.4's Individual plan.
            "qwen-token-plan",
            "qwen-token-plan-cn",
            "qwen-token-plan-individual",
            "radius",
            // DRIFT-009 (2026-09-05): the last unregistered built-in, `all.ts:95` @v0.84.4.
            "baseten",
        ] {
            assert!(
                ids.iter().any(|id| id == expected),
                "missing built-in provider '{expected}'"
            );
        }

        // `NOT_YET` is declared at the top of this module, because
        // `all_of_pis_v0_84_4_builtins_are_registered` subtracts the same array from its expected
        // set. Keep the loop: the day a provider is genuinely half-ported, this is where it is
        // parked, and parking it satisfies both guards at once.
        for not_yet in NOT_YET {
            assert!(
                !ids.iter().any(|id| id == not_yet),
                "'{not_yet}' is registered but has no working stream path. If it was genuinely \
                 ported, delete it from this array and from the NOT REGISTERED row in this \
                 module's header table in the same commit."
            );
        }

        // The count matches what `all_providers()` returns and has no duplicate ids.
        assert_eq!(ids.len(), all_providers().len());
    }

    /// DRIFT-009 — pi's `builtinProviders()` list in full, so the registry cannot disagree with
    /// upstream in silence.
    ///
    /// This is the lesson DRIFT-009 was filed for, applied to registrations instead of catalogs:
    /// the previous guard was a hand-kept NOT-YET array plus a prose table, and both went stale
    /// (`PROV-062` deleted the array while three providers were missing; the header claimed 39 of
    /// 40 while it listed one). Forty ids, transcribed from `providers/all.ts:91-130` @v0.84.4 in
    /// upstream's own order.
    ///
    /// **What this guard does and does not reach.** `PI_BUILTINS` is a hand transcription of the
    /// PINNED parity target, not a live read of upstream, so it catches exactly two things: a
    /// provider cyrup DROPS relative to v0.84.4, and a provider cyrup INVENTS that v0.84.4 does not
    /// ship (the direction the old prose table could not see at all). It CANNOT see a provider pi
    /// adds after v0.84.4 — such an id is in neither list and the comparison stays green — so it is
    /// exactly as hand-kept as the array it sits beside, and it must be refreshed when ADR-0006
    /// moves the parity target. `xtask gen-catalogs --roster <rev>` is the check that reads
    /// upstream live; this one pins the registry against the target that check names.
    ///
    /// It composes with `NOT_YET` rather than contradicting it: a parked id is subtracted from the
    /// expected set here and asserted absent there, so parking a half-ported provider keeps both
    /// guards green while it is parked and fails both the moment it is registered without leaving
    /// the array (or leaves the array without being registered).
    #[test]
    fn all_of_pis_v0_84_4_builtins_are_registered() {
        const PI_BUILTINS: &[&str] = &[
            "amazon-bedrock",
            "ant-ling",
            "anthropic",
            "azure-openai-responses",
            "baseten",
            "cerebras",
            "cloudflare-ai-gateway",
            "cloudflare-workers-ai",
            "deepseek",
            "fireworks",
            "github-copilot",
            "google",
            "google-vertex",
            "groq",
            "huggingface",
            "kimi-coding",
            "minimax",
            "minimax-cn",
            "mistral",
            "moonshotai",
            "moonshotai-cn",
            "nvidia",
            "openai",
            "openai-codex",
            "opencode",
            "opencode-go",
            "openrouter",
            "qwen-token-plan",
            "qwen-token-plan-cn",
            "qwen-token-plan-individual",
            "radius",
            "together",
            "vercel-ai-gateway",
            "xai",
            "xiaomi",
            "xiaomi-token-plan-ams",
            "xiaomi-token-plan-cn",
            "xiaomi-token-plan-sgp",
            "zai",
            "zai-coding-cn",
        ];
        assert_eq!(
            PI_BUILTINS.len(),
            40,
            "all.ts:91-130 @v0.84.4 is 40 entries"
        );

        let mut registered: Vec<String> = all_providers()
            .iter()
            .map(|p| p.id().as_str().to_string())
            .collect();
        registered.sort();
        // Subtract the parked ids so this guard and `registry_contains_implemented_provider_ids`
        // compose: that one asserts a parked id is NOT registered, so expecting it here would make
        // the two contradict each other and leave `NOT_YET` unusable.
        let mut expected: Vec<String> = PI_BUILTINS
            .iter()
            .filter(|id| !NOT_YET.contains(*id))
            .map(|s| (*s).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            registered, expected,
            "the registry and pi's builtinProviders() @v0.84.4 (minus anything parked in NOT_YET) \
             must name the same set — an id only pi has is an unported provider that belongs in \
             NOT_YET, an id only cyrup has is an invention. This list is pinned to v0.84.4: it \
             cannot see a provider pi adds later, and must be re-transcribed when the parity \
             target moves."
        );
    }

    /// The built-in images collection registers `openrouter-images` (Pi `builtinImagesModels`,
    /// `all.ts:125-131`) so an image model resolves out of the box.
    #[test]
    fn default_images_models_registers_openrouter() {
        let models = default_images_models(CreateModelsOptions::default());
        let ids: Vec<String> = models
            .get_providers()
            .iter()
            .map(|p| p.id().to_string())
            .collect();
        assert!(
            ids.iter().any(|id| id == "openrouter"),
            "missing built-in image provider 'openrouter'"
        );
        assert!(
            models
                .get_model("openrouter", "google/gemini-2.5-flash-image")
                .is_some(),
            "expected openrouter image model resolvable"
        );
    }

    /// PROV-014 — the registered `radius` is the real provider kind, not a fleet row: `pi-messages`
    /// is what its gateway rows speak, its auth carries both strategies, and a persisted gateway
    /// catalog reaches it through the overlay entry point exactly as every other built-in's does.
    #[test]
    fn radius_registers_with_both_auth_strategies_and_takes_the_overlay() {
        let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::new());
        let registry = Arc::new(builtin_registry());
        assert!(
            registry.contains(&crate::known_api::PI_MESSAGES.into()),
            "the registry must construct the api radius streams over"
        );
        let radius_row = crate::providers::radius::radius_models_from_config(
            "radius",
            &crate::providers::radius::RadiusGatewayConfig {
                base_url: "https://gw.example.test/v1".to_string(),
                models: vec![crate::providers::radius::RadiusGatewayModel {
                    id: "radius-1".to_string(),
                    name: "Radius One".to_string(),
                    reasoning: false,
                    thinking_level_map: None,
                    input: vec![crate::model::Modality::Text],
                    cost: crate::model::ModelCost::default(),
                    context_window: 128_000,
                    max_tokens: 8_192,
                }],
            },
        );
        let overlay = CatalogOverlay::from_entries([("radius".to_string(), radius_row)]);
        let providers = all_providers_with_overlay(store, registry, Some(&overlay));
        let radius = providers
            .iter()
            .find(|p| p.id().as_str() == "radius")
            .expect("radius registered");
        assert_eq!(radius.name(), "Radius");
        let auth = radius.provider_auth().expect("auth clause");
        assert!(auth.api_key.is_some(), "envApiKeyAuth on RADIUS_API_KEY");
        assert!(auth.oauth.is_some(), "lazyOAuth radius");
        let model = radius
            .get_model("radius-1")
            .expect("overlay restored the gateway row");
        assert_eq!(model.api.as_str(), crate::known_api::PI_MESSAGES);
        assert_eq!(model.base_url, "https://gw.example.test/v1");
    }

    /// Together's `moonshotai/Kimi-K2.6` resolves through the registry to the together provider.
    #[test]
    fn together_kimi_k2_6_resolves_through_registry() {
        let models = default_models(CreateModelsOptions::default());

        let together = models
            .get_provider("together")
            .expect("together provider registered");
        assert_eq!(together.id().as_str(), "together");

        let kimi = models
            .get_model("together", "moonshotai/Kimi-K2.6")
            .expect("Kimi-K2.6 resolvable via the together provider catalog");
        assert_eq!(kimi.provider.as_str(), "together");
        assert_eq!(kimi.id.as_str(), "moonshotai/Kimi-K2.6");
    }
}

//! The built-in provider registry — the L1 aggregator (1:1 port of Pi
//! `packages/ai/src/providers/all.ts`, `builtinProviders()` / `builtinModels()`).
//!
//! Pi's `all.ts` constructs EVERY built-in provider (`all.ts:70-108`) and registers each into a
//! `Models` collection (`all.ts:111-117`); the model pattern then resolves to the owning provider.
//! This module mirrors that for the providers actually implemented in this crate today.
//!
//! ## Pi `all.ts` `builtinProviders()` listing (line numbers from `all.ts:89-126` **@ `v0.83.0`**):
//!
//! | Pi line | provider id              | here                                   |
//! |---------|--------------------------|----------------------------------------|
//! | 89      | `amazon-bedrock`         | ✓                                      |
//! | 90      | `ant-ling`               | ✓ fleet                                |
//! | 91      | `anthropic`              | ✓                                      |
//! | 92      | `azure-openai-responses` | ✓                                      |
//! | 93      | `cerebras`               | ✓ fleet                                |
//! | 94      | `cloudflare-ai-gateway`  | ✓                                      |
//! | 95      | `cloudflare-workers-ai`  | ✓                                      |
//! | 96      | `deepseek`               | ✓ fleet                                |
//! | 97      | `fireworks`              | ✓                                      |
//! | 98      | `github-copilot`         | ✓                                      |
//! | 99      | `google`                 | ✓                                      |
//! | 100     | `google-vertex`          | ✓                                      |
//! | 101     | `groq`                   | ✓ fleet                                |
//! | 102     | `huggingface`            | ✓ fleet                                |
//! | 103     | `kimi-coding`            | ✓ anthropic-compat fleet               |
//! | 104     | `minimax`                | ✓ anthropic-compat fleet               |
//! | 105     | `minimax-cn`             | ✓ anthropic-compat fleet               |
//! | 106     | `mistral`                | ✓                                      |
//! | 107     | `moonshotai`             | ✓ fleet                                |
//! | 108     | `moonshotai-cn`          | ✓ fleet                                |
//! | 109     | `nvidia`                 | ✓ fleet                                |
//! | 110     | `openai`                 | ✓                                      |
//! | 111     | `openai-codex`           | ✓                                      |
//! | 112     | `opencode`               | ✓                                      |
//! | 113     | `opencode-go`            | ✓                                      |
//! | 114     | `openrouter`             | ✓ fleet                                |
//! | **115** | **`qwen-token-plan`**    | **✗ NOT REGISTERED — `PROV-014`**      |
//! | **116** | **`qwen-token-plan-cn`** | **✗ NOT REGISTERED — `PROV-014`**      |
//! | **117** | **`radius`**             | **✗ NOT REGISTERED — `PROV-014`**      |
//! | 118     | `together`               | ✓                                      |
//! | 119     | `vercel-ai-gateway`      | ✓ anthropic-compat fleet               |
//! | 120     | `xai`                    | ✓ fleet                                |
//! | 121     | `xiaomi`                 | ✓ fleet                                |
//! | 122-124 | `xiaomi-token-plan-*`    | ✓ fleet                                |
//! | 125     | `zai`                    | ✓ fleet                                |
//! | 126     | `zai-coding-cn`          | ✓ fleet                                |
//!
//! **33 of pi's 36 built-in providers are registered below.** Every api id the registered providers'
//! catalogs name has a registered impl — that half is not left to this comment:
//! `src/tests/catalog_data.rs`'s `every_catalog_api_has_a_registered_impl` walks all 35 catalogs and
//! asserts `builtin_registry().contains(&row.api)` for every row.
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
use crate::remote_catalog::CatalogOverlay;
use crate::utils::http_date::parse_iso8601_utc_ms;
use crate::providers::anthropic::anthropic_fleet_providers_with;
use crate::providers::fleet::fleet_providers_with;
use crate::providers::{
    anthropic_provider_with, azure_openai_responses_provider_with,
    cloudflare_ai_gateway_provider_with, cloudflare_workers_ai_provider_with,
    fireworks_provider_with, github_copilot_provider_with, google_provider_with,
    google_vertex_provider_with, openai_codex_provider_with,
    amazon_bedrock_provider_with, mistral_provider_with, openai_provider_with,
    opencode_go_provider_with, opencode_provider_with, openrouter_images_provider,
    together_provider_with,
};
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

    // openai-completions fleet: ant-ling, cerebras, deepseek, groq, huggingface, moonshotai,
    // moonshotai-cn, nvidia, openrouter, xai, xiaomi, xiaomi-token-plan-{ams,cn,sgp}, zai,
    // zai-coding-cn (Pi `all.ts` lines 73,76,79,84,85,90,91,92,97,100,101,102-104,105,106).
    for p in fleet_providers_with(store.clone(), registry.clone()) {
        providers.push(Arc::new(p));
    }

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
        ] {
            assert!(
                ids.iter().any(|id| id == expected),
                "missing built-in provider '{expected}'"
            );
        }

        // Providers not yet ported. This is a NOT-YET list, not an exemption list: the project
        // ports everything, and an id leaves this array by being implemented. It exists so a
        // half-finished provider cannot be registered and silently answer requests it cannot serve
        // — the assertion is "absent until real", never "must stay absent".
        //
        // PROV-062: this list was DELETED, with a comment claiming "every built-in provider pi ships
        // is now ported, so there is no not-yet list left to assert against". That was false at the
        // time it was written — PROV-014 has been open against these three since before it — so the
        // guard was removed in the same edit that made the claim it was guarding untrue. Restored,
        // and it is the one place a future porter learns the set is incomplete without reading a
        // backlog file. Each id leaves this array by being implemented (PROV-014), not by being
        // reclassified.
        for not_yet in ["qwen-token-plan", "qwen-token-plan-cn", "radius"] {
            assert!(
                !ids.iter().any(|id| id == not_yet),
                "'{not_yet}' is registered but has no working stream path (PROV-014, pi \
                 all.ts:115-117 @v0.83.0). If it was genuinely ported, delete it from this array \
                 and from the NOT REGISTERED rows in this module's header table in the same commit."
            );
        }

        // The count matches what `all_providers()` returns and has no duplicate ids.
        assert_eq!(ids.len(), all_providers().len());
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

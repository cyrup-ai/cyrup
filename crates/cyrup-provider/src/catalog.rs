//! Static model-catalog seam (arch-01 §3.1).
//!
//! Providers ship their catalog from embedded JSON (models.dev-shaped, in this crate's neutral
//! [`Model`] serde form); [`builtin_catalog`] is the union of all of them — the registry catalog
//! any consumer that needs model METADATA should read. The generation timestamp shared by every
//! embedded catalog lives in [`crate::providers::all::BUILTIN_CATALOG_MANIFEST_JSON`].
//!
//! **The runtime refresh is no longer deferred** (DRIFT-007): [`crate::remote_catalog`] ports Pi's
//! `withRemoteCatalog` — a persisted pi.dev overlay with `ETag` revalidation, a 4h freshness window
//! and an `<agent_dir>/models-store.json` cache. It is strictly an OVERLAY: the embedded catalogs
//! here remain the source of truth and the floor, and the overlay can only add or replace models by
//! id, never remove one.

use crate::classifier::{ClassifierModel, ImageModel, ModelType};
use crate::model::{
    Modality, Model, ModelImageInputLimits, ModelImageResizeOptions, ModelInputLimits,
};

/// Parse a catalog from a JSON array of [`Model`] records (the neutral, camelCase serde form),
/// then apply pi's generated-catalog compat metadata to every row.
///
/// This is the seam that stands in for pi's catalog GENERATOR. Upstream `890f92088` (#9816, v0.87.0)
/// moved the `supportsStrictMode` expression out of the runtime detector and into
/// `generate-models.ts`, where `applyOpenAICompletionsCompatMetadata` (`:802-809` @v0.87.1) bakes it
/// into each generated `openai-completions` row as explicit metadata, so that a BUILT-IN row keeps
/// strict JSON-schema tool support while a user-declared row falls back to the new conservative
/// runtime default of `false` (`openai-completions.ts:1666-1667`).
///
/// `[CYRUP-DELTA]` **Mechanism, at full parity.** cyrup computes that metadata when the frozen
/// catalog JSON is parsed rather than when it is generated, because cyrup's generator analogue
/// (`xtask gen-catalogs`) cannot be run at this pin: every `*.models.ts` upstream is a re-export of
/// gitignored generated data, and the pi.dev/models.dev fetches are unreachable (PROV-071). The
/// resolved compat of every catalog row is byte-identical to pi's either way — same expression, same
/// merge order, same `get_compat` precedence — and the frozen JSON and `gen-catalogs --check` stay
/// untouched. When a future generator run bakes the key into the JSON itself, this pass becomes a
/// no-op by construction: it only ever fills a key the row left unset.
///
/// A user `models.json` row never passes through here — the only callers are the embedded-catalog
/// parse sites — so it resolves `supports_strict_mode: false` exactly as upstream.
pub fn load_catalog(json: &str) -> Result<Vec<Model>, serde_json::Error> {
    let mut models: Vec<Model> = serde_json::from_str(json)?;
    for model in &mut models {
        apply_openai_completions_compat_metadata(model);
        apply_prompt_cache_metadata(model);
        apply_image_input_metadata(model);
    }
    Ok(models)
}

/// Pi `applyOpenAICompletionsCompatMetadata` (`generate-models.ts:802-809` @v0.87.1), narrowed to
/// the one key whose generated value differs from what cyrup's runtime detector derives.
///
/// Pi writes a DELTA against `OPENAI_COMPLETIONS_DEFAULT_COMPAT` (`:661`, whose `supportsStrictMode`
/// is `false`) — `openAICompletionsCompatDelta` (`:786-796`) emits only keys that differ from that
/// default — and then merges `{...detected, ...model.compat}` (`:805`), so a value the row already
/// declares WINS. Both halves are reproduced here: nothing is written when the expression yields the
/// default `false`, and an explicit row value is never overwritten. Every other key of the delta is
/// omitted deliberately: cyrup's [`crate::api::compat::detect_compat`] still derives those at
/// runtime from the same predicates, so materializing them would change no resolved value.
fn apply_openai_completions_compat_metadata(model: &mut Model) {
    if model.api.as_str() != crate::known_api::OPENAI_COMPLETIONS {
        return;
    }
    if !crate::api::compat::generated_supports_strict_mode(
        model.provider.as_str(),
        model.base_url.as_str(),
    ) {
        return;
    }
    let compat = model
        .compat
        .get_or_insert_with(crate::api::compat::ModelCompat::default);
    if compat.supports_strict_mode.is_none() {
        compat.supports_strict_mode = Some(true);
    }
}

/// Pi `applyPromptCacheMetadata` (`packages/ai/scripts/generate-models.ts:985-992` @v1.0.4).
///
/// Anthropic ephemeral cache entries have a hard five-minute lifetime, which `ttl: "1h"` extends to
/// one hour, so the two tiers are `{ short: 300, long: 3600 }` seconds
/// (`ANTHROPIC_PROMPT_CACHE`, `:983`).
///
/// **Only DIRECT Anthropic is annotated**, and that is upstream's explicit reasoning, not a
/// shortcut: *"Only direct Anthropic is annotated so cache warming does not assume equivalent
/// behavior through proxies."* A Claude model reached through OpenRouter or any other gateway is
/// `openai-completions` or a different provider id, keeps `prompt_cache: None`, and therefore never
/// gets a warm scheduled — which is the safe answer, because we do not know that the proxy's cache
/// behaves like Anthropic's.
///
/// Upstream also declines to annotate OpenAI, with its reasoning recorded at `:989-991`: a
/// documented TTL alone does not establish full cache loss, so warming there needs observed expiry,
/// replay and billing behaviour first. That omission is reproduced deliberately.
///
/// Like its sibling above, this only ever fills a key the row left unset, so a generated catalog
/// that one day bakes `promptCache` in makes this a no-op by construction.
pub(crate) fn apply_prompt_cache_metadata(model: &mut Model) {
    if model.provider.as_str() != "anthropic"
        || model.api.as_str() != crate::known_api::ANTHROPIC_MESSAGES
    {
        return;
    }
    if model.prompt_cache.is_none() {
        model.prompt_cache = Some(crate::model::ModelPromptCache {
            short: Some(300),
            long: Some(3600),
        });
    }
}

/// The resize profile every image-capable model resolves to when its row declares none — Pi
/// `DEFAULT_IMAGE_RESIZE` (`packages/ai/scripts/generate-models.ts:424-430` @v1.0.4).
///
/// Re-exported from its home in `cyrup-core` beside [`ModelImageResizeOptions`], so the generator
/// stamp here and the resizer in `cyrup-tools` read the SAME four numbers rather than two copies
/// that can drift. See [`cyrup_core::image`] for why the type is homed there.
pub use cyrup_core::DEFAULT_IMAGE_RESIZE;

/// Pi `applyImageInputMetadata` (`packages/ai/scripts/generate-models.ts:994-1020` @v1.0.4), the
/// third of this module's generated-catalog passes.
///
/// **A stamp here is warranted by direct evidence, not by analogy with
/// [`apply_prompt_cache_metadata`].** Upstream really does inject `inputLimits` at generation
/// time: `applyImageInputMetadata` is called once over the chat + classifier list (`:3487`) and
/// again over the openrouter IMAGE list (`:3505`). Its numbers are pinned by upstream's own
/// assertions (`packages/ai/test/providers.test.ts:111-136`).
///
/// It differs from its prompt-cache sibling in REACH, which is why copying that one's shape
/// without re-reading would under-port it: that pass is narrow (direct Anthropic only), this one
/// is broad (every image-capable row of four named providers, PLUS a universal resize default for
/// every other provider).
///
/// The shape, in full:
///
/// 1. Return unchanged unless `input` contains [`Modality::Image`] — upstream's
///    `if (!model.input.includes("image")) return;`. A text-only row keeps `input_limits: None`,
///    which is what makes the field's absence meaningful rather than merely unset.
/// 2. Pick the provider's hard limits: `anthropic` → 32 MiB and `maxPerRequest` **100** for a
///    non-image row whose `contextWindow` is exactly 200000, else **600**; `amazon-bedrock` →
///    `maxPerMessage: 20`; `openai` → 512 MiB and 1500; `google` → 20 MiB and 3600; anything else
///    → nothing.
/// 3. Merge so the ROW wins over the provider table (`{...providerLimits, ...model.inputLimits}`),
///    and fill `images.resize` PER KEY from [`DEFAULT_IMAGE_RESIZE`] under whatever the row
///    declared (`{...DEFAULT_IMAGE_RESIZE, ...configuredImages?.resize}`). So `resize` on an
///    image-capable row is ALWAYS complete afterwards, and a row that declares one key keeps it
///    and gains the other three.
///
/// `[CYRUP-DELTA]` **Mechanism, at full parity** — the same note [`load_catalog`] carries, for the
/// same reason (PROV-071: cyrup's generator analogue cannot be run at this pin). cyrup stamps when
/// the frozen JSON is parsed rather than when it is generated. Measured at HEAD over the 39
/// embedded catalogs: 1519 rows, 1065 image-capable, and 1011 of those already carry `inputLimits`
/// in exactly these shapes, so over the JSON CHAT catalogs this pass is already a no-op and merely
/// adds nothing a generator run would not. It earns its keep in two places where the data really
/// is absent: [`crate::providers::openrouter::openrouter_image_models`], whose
/// `catalog/openrouter-images.json` was frozen at v0.87.1 before the field existed and carries one
/// on **0 of its 54 image-capable rows**; and [`crate::providers::together::together_models`], the
/// one built-in catalog written as Rust literals rather than JSON, which therefore never reaches
/// `load_catalog` at all (6 image-capable rows).
///
/// Like both siblings, it only ever fills what the row left unset, so a future generator run that
/// bakes the key in makes it a no-op by construction. And like both siblings it runs ONLY at
/// catalog parse sites — never over a user `models.json`, which upstream's `modelFromJson` copies
/// verbatim (`provider-composer.ts:239`).
pub(crate) fn apply_image_input_metadata(model: &mut Model) {
    stamp_image_input_limits(
        model.provider.as_str(),
        &model.input,
        ModelType::Chat,
        Some(model.context_window),
        &mut model.input_limits,
    );
}

/// [`apply_image_input_metadata`] for an image row — upstream's second call site (`:3505`), where
/// `model.type === "image"` short-circuits Anthropic's `contextWindow` branch to 600.
pub(crate) fn apply_image_input_metadata_to_image(model: &mut ImageModel) {
    stamp_image_input_limits(
        model.provider.as_str(),
        &model.input,
        ModelType::Image,
        // pi's `ImageModel` extends `BaseModel`, which declares no `contextWindow`
        // (`types.ts:1097-1108`), so the `=== 200000` test is `undefined === 200000` → false.
        None,
        &mut model.input_limits,
    );
}

/// [`apply_image_input_metadata`] for a classifier row. Classifiers go through upstream's FIRST
/// call site with the chat models (`:3487`), so the `type !== "image"` half of Anthropic's branch
/// is true for them and their own `contextWindow` decides 100 vs 600.
pub(crate) fn apply_image_input_metadata_to_classifier(model: &mut ClassifierModel) {
    stamp_image_input_limits(
        model.provider.as_str(),
        &model.input,
        ModelType::Classifier,
        Some(model.context_window),
        &mut model.input_limits,
    );
}

/// The one body the three wrappers share, held apart from them because pi's `AnyModel` parameter
/// has no Rust counterpart that is `&mut` across all three concrete types.
fn stamp_image_input_limits(
    provider: &str,
    input: &[Modality],
    model_type: ModelType,
    context_window: Option<u64>,
    limits: &mut Option<ModelInputLimits>,
) {
    if !input.contains(&Modality::Image) {
        return;
    }

    // `providerLimits` (`generate-models.ts:997-1008`). Tuple order:
    // (maxRequestBytes, images.maxPerMessage, images.maxPerRequest).
    let (provider_request_bytes, provider_per_message, provider_per_request) = match provider {
        "anthropic" => {
            let per_request = if model_type != ModelType::Image && context_window == Some(200_000) {
                100
            } else {
                600
            };
            (Some(32 * 1024 * 1024), None, Some(per_request))
        }
        "amazon-bedrock" => (None, Some(20), None),
        "openai" => (Some(512 * 1024 * 1024), None, Some(1500)),
        "google" => (Some(20 * 1024 * 1024), None, Some(3600)),
        _ => (None, None, None),
    };

    let configured = limits.take();
    let configured_images = configured.as_ref().and_then(|l| l.images.as_ref());
    let configured_resize = configured_images.and_then(|i| i.resize.as_ref());

    *limits = Some(ModelInputLimits {
        // `{ ...providerLimits, ...model.inputLimits }`: the row's value wins.
        max_request_bytes: configured
            .as_ref()
            .and_then(|l| l.max_request_bytes)
            .or(provider_request_bytes),
        images: Some(ModelImageInputLimits {
            // `resize: { ...DEFAULT_IMAGE_RESIZE, ...configuredImages?.resize }` — per key, so a
            // row declaring only `jpegQuality` keeps it and gains the other three defaults.
            resize: Some(ModelImageResizeOptions {
                max_width: configured_resize
                    .and_then(|r| r.max_width)
                    .or(DEFAULT_IMAGE_RESIZE.max_width),
                max_height: configured_resize
                    .and_then(|r| r.max_height)
                    .or(DEFAULT_IMAGE_RESIZE.max_height),
                max_bytes: configured_resize
                    .and_then(|r| r.max_bytes)
                    .or(DEFAULT_IMAGE_RESIZE.max_bytes),
                jpeg_quality: configured_resize
                    .and_then(|r| r.jpeg_quality)
                    .or(DEFAULT_IMAGE_RESIZE.jpeg_quality),
            }),
            // `{ ...providerLimits?.images, ...configuredImages }`: again the row wins.
            max_per_message: configured_images
                .and_then(|i| i.max_per_message)
                .or(provider_per_message),
            max_per_request: configured_images
                .and_then(|i| i.max_per_request)
                .or(provider_per_request),
        }),
    });
}

/// Every model the implemented built-in providers ship — the real, whole model registry catalog
/// (Pi `Models.getModels()` over `builtinModels()`, `models.ts:135` @v0.83.0 / `all.ts:111-117`).
///
/// This is the credential-BLIND read: it is the complete synchronous catalog, exactly as Pi
/// documents `getModels()` ("`getModels()` remains the complete synchronous catalog", `models.ts:108`),
/// and it is what an embedder that only needs provider/model METADATA (cost, reasoning,
/// `context_window`, `max_tokens`) should consult. Pi's credential-FILTERED
/// `Models.getAvailable()` (`models.ts:394-409` @v0.83.0, provider auth checked per provider) is now
/// ported (PROV-031); a caller that wants availability must still layer its own auth check on top of
/// this crate yet; a caller that wants availability must layer its own auth check on top.
///
/// Composition matches [`crate::providers::all::default_models`] with default options: every
/// built-in provider, no remote overlay, ordered by provider id (the collection holds providers in
/// a `BTreeMap`) then by each provider's own catalog order. Parsed ONCE and cached — the embedded
/// catalogs are compile-time constants, so nothing here can change between calls.
///
/// Never panics: a provider whose catalog fails to parse simply contributes no models (Pi's
/// catch-and-skip contract, `models.ts:254-258` and `:263-267` @v0.83.0; PROV-041 corrected
/// `:99-101`, the `refreshModels?` docblock).
pub fn builtin_catalog() -> &'static [Model] {
    static CATALOG: std::sync::OnceLock<Vec<Model>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        crate::providers::all::default_models(crate::collection::CreateModelsOptions::default())
            .get_models(None)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::known_api;

    /// The registry catalog is the WHOLE built-in registry, not a hand-seeded subset: every
    /// registered provider contributes, and the metadata is the live embedded catalog's (so
    /// Sonnet 4.5's context window is the real 1M, not the retired 200k of the old seed stub).
    #[test]
    fn builtin_catalog_is_the_whole_registry() {
        let catalog = builtin_catalog();
        let providers: std::collections::BTreeSet<&str> =
            catalog.iter().map(|m| m.provider.as_str()).collect();
        assert!(
            providers.len() >= 25,
            "every built-in provider must contribute, got {} providers",
            providers.len()
        );
        // Providers that the retired 2-model seed stub could never answer for.
        for expected in ["google", "mistral", "groq", "openrouter", "together"] {
            assert!(
                providers.contains(expected),
                "{expected} missing from the registry catalog"
            );
        }
        let sonnet = catalog
            .iter()
            .find(|m| m.provider.as_str() == "anthropic" && m.id.as_str() == "claude-sonnet-4-5")
            .expect("anthropic claude-sonnet-4-5 present");
        assert_eq!(sonnet.api.as_str(), known_api::ANTHROPIC_MESSAGES);
        assert!(sonnet.reasoning);
        // 200000 since PROV-149: models.dev (and so pi.dev) moved Sonnet 4.5 back from 1M.
        assert_eq!(
            sonnet.context_window, 200_000,
            "metadata comes from the live embedded catalog"
        );
        // A `baseUrl` is an ORIGIN, never a full endpoint path (the seed stub stored
        // `https://api.anthropic.com/v1/messages`).
        assert!(
            !sonnet.base_url.ends_with("/v1/messages"),
            "baseUrl must be an origin, got {}",
            sonnet.base_url
        );
    }

    /// Cached: the same slice is returned every call (no re-parse of ~470 KB of embedded JSON).
    #[test]
    fn builtin_catalog_is_parsed_once() {
        assert!(std::ptr::eq(builtin_catalog(), builtin_catalog()));
    }

    #[test]
    fn load_catalog_rejects_garbage() {
        assert!(load_catalog("not json").is_err());
    }

    /// PROV-078 — a BUILT-IN `openai-completions` row carries strict support as EXPLICIT metadata,
    /// the way pi's generated catalogs do (`generate-models.ts:765-766`, `:802-809` @v0.87.1).
    ///
    /// This is the half that makes the conservative runtime default safe. With only
    /// `detect_compat`'s flat `false` in place, every one of the 460 embedded `openai-completions`
    /// rows would SILENTLY lose strict function calling, because 415 of them declare no
    /// `supportsStrictMode` at all. Upstream did not lose it: the generator bakes the expression into
    /// the row. So the assertion is on the row's own `compat`, not just on the resolved value —
    /// resolution alone would pass for the wrong reason if the metadata were ever dropped and the
    /// runtime default flipped back.
    #[test]
    fn builtin_openai_completions_rows_carry_strict_as_metadata() {
        for provider in ["groq", "huggingface"] {
            let rows: Vec<&Model> = builtin_catalog()
                .iter()
                .filter(|m| m.provider.as_str() == provider)
                .collect();
            assert!(!rows.is_empty(), "{provider} contributes no rows");
            for m in rows {
                assert_eq!(
                    m.api.as_str(),
                    known_api::OPENAI_COMPLETIONS,
                    "{provider}/{} changed wire api",
                    m.id
                );
                assert_eq!(
                    m.compat.as_ref().and_then(|c| c.supports_strict_mode),
                    Some(true),
                    "{provider}/{} lost its strict metadata",
                    m.id
                );
                assert!(
                    crate::api::compat::get_compat(m).supports_strict_mode,
                    "{provider}/{} does not resolve strict",
                    m.id
                );
            }
        }
    }

    /// PROV-078 — cerebras is excluded, matching `af7359b90` (#9804, v0.86.1), which added
    /// `!isCerebras` to the expression now living at `generate-models.ts:765-766` @v0.87.1.
    ///
    /// The exclusion is asserted on the RESOLVED value, because pi reaches `false` here by writing
    /// no key at all (the delta at `:786-796` omits anything equal to the
    /// `supportsStrictMode: false` default at `:661`) and letting the runtime default stand.
    #[test]
    fn cerebras_rows_are_excluded_like_pi_9804() {
        let rows: Vec<&Model> = builtin_catalog()
            .iter()
            .filter(|m| m.provider.as_str() == "cerebras")
            .collect();
        assert!(!rows.is_empty(), "cerebras contributes no rows");
        for m in rows {
            assert_eq!(m.api.as_str(), known_api::OPENAI_COMPLETIONS);
            assert_eq!(
                m.compat.as_ref().and_then(|c| c.supports_strict_mode),
                None,
                "cerebras/{} must carry no strict key, as pi's delta writes none",
                m.id
            );
            assert!(
                !crate::api::compat::get_compat(m).supports_strict_mode,
                "cerebras/{} must not resolve strict",
                m.id
            );
        }
    }

    /// PROV-078 — the seam is idempotent and never overwrites a row's own declaration: the
    /// embedded rows that spell `supportsStrictMode: false` out (moonshotai 4, moonshotai-cn 4,
    /// nvidia 19) keep it, per pi's `{...detected, ...model.compat}` merge order
    /// (`generate-models.ts:805`).
    ///
    /// The roster moved with PROV-071's live refresh and moved in BOTH directions, which is what
    /// makes it worth pinning: `cloudflare-ai-gateway` left it entirely — its Workers-AI
    /// passthrough rows now declare `supportsStrictMode: true` — while moonshotai and nvidia kept
    /// the explicit `false` on every row they still ship. A seam that quietly overwrote a row's own
    /// `false` would look identical to the gateway's legitimate move, so the providers are asserted
    /// as an exact set, not a lower bound.
    #[test]
    fn an_explicit_row_value_wins_over_the_generated_metadata() {
        let explicit: Vec<&Model> = builtin_catalog()
            .iter()
            .filter(|m| {
                m.api.as_str() == known_api::OPENAI_COMPLETIONS
                    && m.compat
                        .as_ref()
                        .and_then(|c| c.supports_strict_mode)
                        .is_some_and(|v| !v)
            })
            .collect();
        let mut per_provider: std::collections::BTreeMap<&str, usize> =
            std::collections::BTreeMap::new();
        for m in &explicit {
            *per_provider.entry(m.provider.as_str()).or_default() += 1;
        }
        // The embedded rosters that spell the key out, at their measured sizes. `together` also
        // declares `Some(false)`, but in CODE (`providers/together.rs:54`) for its hand-ported
        // rows, not in a generated catalog, so it is asserted as present rather than sized.
        assert_eq!(per_provider.get("moonshotai"), Some(&4));
        assert_eq!(per_provider.get("moonshotai-cn"), Some(&4));
        assert_eq!(per_provider.get("nvidia"), Some(&19));
        assert!(per_provider.contains_key("together"));
        assert_eq!(
            per_provider.keys().copied().collect::<Vec<_>>(),
            vec!["moonshotai", "moonshotai-cn", "nvidia", "together"],
            "a provider started or stopped declaring supportsStrictMode: false"
        );
        for m in explicit {
            assert!(!crate::api::compat::get_compat(m).supports_strict_mode);
        }
    }
}

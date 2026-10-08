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

use crate::model::Model;

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
        assert_eq!(
            sonnet.context_window, 1_000_000,
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

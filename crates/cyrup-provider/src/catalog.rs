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

/// Parse a catalog from a JSON array of [`Model`] records (the neutral, camelCase serde form).
pub fn load_catalog(json: &str) -> Result<Vec<Model>, serde_json::Error> {
    serde_json::from_str(json)
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
}

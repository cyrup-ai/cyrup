//! Static model-catalog seam (arch-01 §3.1).
//!
//! Providers ship a seed catalog from embedded JSON (models.dev-shaped, in this crate's neutral
//! [`Model`] serde form). The generation timestamp shared by every embedded catalog lives in
//! [`crate::providers::all::BUILTIN_CATALOG_MANIFEST_JSON`].
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

/// The embedded seed catalog (a hand-seeded subset; full models.dev generation is DEFERRED).
/// Never panics: a malformed embed yields an empty catalog (R-01-001 sync, non-throwing reads).
pub fn seed_catalog() -> Vec<Model> {
    load_catalog(include_str!("catalog/seed.json")).unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::known_api;

    #[test]
    fn seed_catalog_parses() {
        let catalog = seed_catalog();
        assert!(catalog.len() >= 2);
        let anthropic = catalog
            .iter()
            .find(|m| m.api.as_str() == known_api::ANTHROPIC_MESSAGES)
            .expect("anthropic model present");
        assert!(anthropic.reasoning);
        assert!((anthropic.cost.cache_write - 3.75).abs() < 1e-9);
        assert_eq!(anthropic.context_window, 200_000);
    }

    #[test]
    fn load_catalog_rejects_garbage() {
        assert!(load_catalog("not json").is_err());
    }
}

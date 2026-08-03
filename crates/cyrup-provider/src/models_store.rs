//! Persistent per-provider model catalogs (1:1 port of Pi `packages/ai/src/models-store.ts`).
//!
//! A [`ModelsStore`] is the durable half of the remote model-catalog overlay (DRIFT-007): it holds,
//! per provider id, the last catalog body fetched from the remote endpoint together with the HTTP
//! validators needed to revalidate it (`ETag`, `Last-Modified`) and the timestamp of the last
//! completed check. Persisting the body is what makes a restart *not* refetch, and what lets an
//! offline run still see the last overlay (Pi reads the store BEFORE the `allowNetwork` gate,
//! `remote-catalog-provider.ts:58-59`).
//!
//! The store is intentionally the only stateful piece: the merge itself is pure and happens at
//! provider-construction time (see [`crate::remote_catalog`]), so nothing here ever mutates a live
//! registry.
//!
//! Pi splits this the same way cyrup does: the interface + the in-memory implementation live in the
//! vendor-neutral `packages/ai` layer (here), while the locked, on-disk `FileModelsStore` lives in
//! the agent layer (here: `cyrup-config`, which owns `FileLock`/`write_atomic`).

use crate::error::ProviderError;
use crate::model::Model;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// One provider's persisted catalog plus its HTTP validators (Pi `ModelsStoreEntry`,
/// `models-store.ts:3-14`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsStoreEntry {
    /// The catalog body as last fetched. Empty means "no cached body" — which is exactly why the
    /// `If-None-Match` validator is suppressed in that state (a 304 against no body would leave the
    /// overlay empty).
    #[serde(default)]
    pub models: Vec<Model>,
    /// Unix milliseconds from the remote catalog's `Last-Modified` header (`0` when absent or
    /// unparseable, matching Pi's `Number.isNaN(lastModified) ? 0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<i64>,
    /// Unix milliseconds of the last COMPLETED remote check — the freshness window's anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<i64>,
    /// Opaque validator from the remote catalog's `ETag` header, stored VERBATIM (quotes included)
    /// and echoed back as `If-None-Match`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

/// Persistent model catalogs keyed by provider id (Pi `ModelsStore`, `models-store.ts:17-21`).
///
/// Every method is fallible and every caller on the refresh path treats a failure as "no cached
/// entry" rather than propagating it — a broken cache must never be worse than a cold one, and it
/// must never reduce the built-in catalogs (DRIFT-007's floor invariant).
#[async_trait::async_trait]
pub trait ModelsStore: Send + Sync {
    async fn read(&self, provider_id: &str) -> Result<Option<ModelsStoreEntry>, ProviderError>;
    async fn write(&self, provider_id: &str, entry: ModelsStoreEntry) -> Result<(), ProviderError>;
    async fn delete(&self, provider_id: &str) -> Result<(), ProviderError>;
}

/// A [`ModelsStore`] narrowed to a single provider id (Pi `ProviderModelsStore`,
/// `models-store.ts:24-28`): "providers cannot access other providers' catalogs".
#[derive(Clone)]
pub struct ProviderModelsStore {
    store: Arc<dyn ModelsStore>,
    provider_id: String,
}

impl ProviderModelsStore {
    pub fn new(store: Arc<dyn ModelsStore>, provider_id: impl Into<String>) -> Self {
        Self {
            store,
            provider_id: provider_id.into(),
        }
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub async fn read(&self) -> Result<Option<ModelsStoreEntry>, ProviderError> {
        self.store.read(&self.provider_id).await
    }

    pub async fn write(&self, entry: ModelsStoreEntry) -> Result<(), ProviderError> {
        self.store.write(&self.provider_id, entry).await
    }

    pub async fn delete(&self) -> Result<(), ProviderError> {
        self.store.delete(&self.provider_id).await
    }
}

/// Process-local store (Pi `InMemoryModelsStore`, `models-store.ts:30-45`). Used by tests and by
/// any run with no resolvable agent dir, where Pi falls back to
/// `InMemoryCodingAgentModelsStore` (`model-runtime.ts:143-144`).
#[derive(Default)]
pub struct InMemoryModelsStore {
    entries: Mutex<BTreeMap<String, ModelsStoreEntry>>,
}

impl InMemoryModelsStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl ModelsStore for InMemoryModelsStore {
    async fn read(&self, provider_id: &str) -> Result<Option<ModelsStoreEntry>, ProviderError> {
        // A poisoned lock degrades to "no cached entry" rather than panicking (NO-PANIC policy);
        // the overlay is optional by construction, so the built-in floor is unaffected.
        Ok(self
            .entries
            .lock()
            .ok()
            .and_then(|g| g.get(provider_id).cloned()))
    }

    async fn write(&self, provider_id: &str, entry: ModelsStoreEntry) -> Result<(), ProviderError> {
        if let Ok(mut g) = self.entries.lock() {
            g.insert(provider_id.to_string(), entry);
        }
        Ok(())
    }

    async fn delete(&self, provider_id: &str) -> Result<(), ProviderError> {
        if let Ok(mut g) = self.entries.lock() {
            g.remove(provider_id);
        }
        Ok(())
    }
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

    #[tokio::test]
    async fn in_memory_round_trips_and_scopes_by_provider() {
        let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
        assert!(store.read("groq").await.unwrap().is_none());

        let entry = ModelsStoreEntry {
            models: Vec::new(),
            last_modified: Some(7),
            checked_at: Some(9),
            etag: Some("\"abc\"".into()),
        };
        store.write("groq", entry.clone()).await.unwrap();
        assert_eq!(store.read("groq").await.unwrap(), Some(entry));
        // Scoping: another provider id is untouched.
        assert!(store.read("xai").await.unwrap().is_none());

        let scoped = ProviderModelsStore::new(store.clone(), "groq");
        assert_eq!(scoped.provider_id(), "groq");
        scoped.delete().await.unwrap();
        assert!(scoped.read().await.unwrap().is_none());
    }

    #[test]
    fn entry_serde_is_camel_case_and_omits_absent_validators() {
        let json = serde_json::to_string(&ModelsStoreEntry {
            models: Vec::new(),
            last_modified: Some(1),
            checked_at: Some(2),
            etag: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"models":[],"lastModified":1,"checkedAt":2}"#);
        // The etag is stored VERBATIM including its quotes (Pi `models-store.ts:9-13`).
        let back: ModelsStoreEntry =
            serde_json::from_str(r#"{"models":[],"etag":"W/\"v3\""}"#).unwrap();
        assert_eq!(back.etag.as_deref(), Some("W/\"v3\""));
        assert_eq!(back.checked_at, None);
    }
}

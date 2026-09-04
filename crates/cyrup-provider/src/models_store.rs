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
use cyrup_core::CancelToken;
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

/// Per-operation options for a [`ModelsStore`] call (Pi `ModelsStoreOperationOptions`,
/// `packages/ai/src/models-store.ts:16-18` @v0.84.1).
///
/// **Upstream drift, not a port gap (CFG-042).** The interface takes no options at all at v0.83.0
/// — the ported baseline — where the three methods are `read(providerId)`, `write(providerId,
/// entry)` and `delete(providerId)`. v0.84.1 adds this bag with a single member and threads it
/// through every method and every implementation. It is ported here rather than deferred because
/// it is the seam that makes a store call abortable at all, and pi's own production caller already
/// passes it (`packages/ai/src/models.ts:350`, `:352`, `:375` — the per-provider refresh
/// generation, which aborts a superseded refresh mid-flight).
///
/// A one-field struct rather than a bare token, deliberately: that is the shape upstream chose so
/// the surface can grow without re-breaking three signatures, and pi's `signal?:` being optional
/// *inside* an optional bag is why cyrup's parameter is `Option<&ModelsStoreOperationOptions>` —
/// `None` is pi's `undefined`, i.e. "no caller passed anything", which is what every cyrup call
/// site does today and what pi's own `remote-catalog-provider.ts` call sites do.
#[derive(Clone, Debug, Default)]
pub struct ModelsStoreOperationOptions {
    /// Pi `signal?: AbortSignal`. cyrup's single `AbortSignal` equivalent is
    /// [`CancelToken`] (arch-00 §3.2 — "no subsystem invents its own abort
    /// flag").
    pub signal: Option<CancelToken>,
}

impl ModelsStoreOperationOptions {
    /// Pi `options?.signal?.throwIfAborted()` — the FIRST statement of every implementation method
    /// at v0.84.1 (`packages/ai/src/models-store.ts:31`, `:37`, `:42`;
    /// `packages/coding-agent/src/core/models-store.ts:120-122`, `:127-137`, `:139-149`).
    ///
    /// Both halves of the optionality chain are honoured: no options *or* options carrying no
    /// signal is a no-op, exactly like the two `?.`s. An already-cancelled token yields
    /// [`ProviderError::Aborted`], whose code is `aborted` — the taxonomy counterpart of the
    /// `AbortError` `throwIfAborted` raises.
    ///
    /// Placement matters and is upstream's: the check runs BEFORE the lock is taken and before any
    /// I/O, so a cancelled operation neither contends for the cross-process file lock nor half-runs.
    /// It is a one-shot check, not a race — pi's `throwIfAborted` is likewise a synchronous test of
    /// the flag's current state, not a subscription.
    pub fn throw_if_aborted(options: Option<&Self>) -> Result<(), ProviderError> {
        match options.and_then(|o| o.signal.as_ref()) {
            Some(token) if token.is_cancelled() => Err(ProviderError::Aborted),
            _ => Ok(()),
        }
    }
}

/// Persistent model catalogs keyed by provider id (Pi `ModelsStore`, `models-store.ts:17-21`
/// @v0.83.0, `:20-25` @v0.84.1).
///
/// Every method is fallible and every caller on the refresh path treats a failure as "no cached
/// entry" rather than propagating it — a broken cache must never be worse than a cold one, and it
/// must never reduce the built-in catalogs (DRIFT-007's floor invariant). The `options` parameter
/// is v0.84.1's addition; see [`ModelsStoreOperationOptions`] for why `Option<&_>` is the port of
/// `options?:`.
#[async_trait::async_trait]
pub trait ModelsStore: Send + Sync {
    async fn read(
        &self,
        provider_id: &str,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<Option<ModelsStoreEntry>, ProviderError>;
    async fn write(
        &self,
        provider_id: &str,
        entry: ModelsStoreEntry,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError>;
    async fn delete(
        &self,
        provider_id: &str,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError>;
}

/// A [`ModelsStore`] narrowed to a single provider id (Pi `ProviderModelsStore`,
/// `models-store.ts:24-28` @v0.83.0): "providers cannot access other providers' catalogs".
///
/// **Upstream removed this interface at v0.84.1** — `git -C pi grep -n ProviderModelsStore v0.84.1
/// -- packages/` returns nothing; the narrowing moved into the `refreshModels` context object that
/// `packages/ai/src/models.ts` hands each provider. cyrup keeps it, which is a shape divergence
/// that is NOT part of CFG-042 and is left to whoever ports v0.84.1's `Models.refresh` generation
/// machinery. Noted here so the next reader does not close that drift by finding this type.
///
/// It forwards the v0.84.1 `options` bag unchanged, so scoping a store cannot become a
/// cancellation hole.
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

    pub async fn read(
        &self,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<Option<ModelsStoreEntry>, ProviderError> {
        self.store.read(&self.provider_id, options).await
    }

    pub async fn write(
        &self,
        entry: ModelsStoreEntry,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.store.write(&self.provider_id, entry, options).await
    }

    pub async fn delete(
        &self,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.store.delete(&self.provider_id, options).await
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
    async fn read(
        &self,
        provider_id: &str,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<Option<ModelsStoreEntry>, ProviderError> {
        // `options?.signal?.throwIfAborted()` (`models-store.ts:31` @v0.84.1) — first statement.
        ModelsStoreOperationOptions::throw_if_aborted(options)?;
        // A poisoned lock degrades to "no cached entry" rather than panicking (NO-PANIC policy);
        // the overlay is optional by construction, so the built-in floor is unaffected.
        //
        // `.cloned()` is pi's `structuredClone(entry)` (`:33` @v0.84.1, also new at that tag): the
        // caller must not be handed an alias it can mutate the store through. In Rust the clone is
        // forced by the borrow anyway, so this half of the v0.84.1 diff is already satisfied.
        Ok(self
            .entries
            .lock()
            .ok()
            .and_then(|g| g.get(provider_id).cloned()))
    }

    async fn write(
        &self,
        provider_id: &str,
        entry: ModelsStoreEntry,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        // `models-store.ts:37` @v0.84.1. Checked BEFORE the map is touched, so an aborted write
        // leaves the previous entry intact rather than half-applied.
        ModelsStoreOperationOptions::throw_if_aborted(options)?;
        if let Ok(mut g) = self.entries.lock() {
            // pi `structuredClone(entry)` (`:38`); cyrup takes the entry BY VALUE, so the caller
            // has already given up its copy and no alias can exist.
            g.insert(provider_id.to_string(), entry);
        }
        Ok(())
    }

    async fn delete(
        &self,
        provider_id: &str,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        // `models-store.ts:42` @v0.84.1.
        ModelsStoreOperationOptions::throw_if_aborted(options)?;
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
        assert!(store.read("groq", None).await.unwrap().is_none());

        let entry = ModelsStoreEntry {
            models: Vec::new(),
            last_modified: Some(7),
            checked_at: Some(9),
            etag: Some("\"abc\"".into()),
        };
        store.write("groq", entry.clone(), None).await.unwrap();
        assert_eq!(store.read("groq", None).await.unwrap(), Some(entry));
        // Scoping: another provider id is untouched.
        assert!(store.read("xai", None).await.unwrap().is_none());

        let scoped = ProviderModelsStore::new(store.clone(), "groq");
        assert_eq!(scoped.provider_id(), "groq");
        scoped.delete(None).await.unwrap();
        assert!(scoped.read(None).await.unwrap().is_none());
    }

    /// CFG-042's residual — the `signal` half of v0.84.1's `ModelsStoreOperationOptions`.
    ///
    /// pi calls `options?.signal?.throwIfAborted()` as the first statement of all three methods
    /// (`packages/ai/src/models-store.ts:31`, `:37`, `:42` @v0.84.1; the interface at `:16-18`,
    /// `:22-24`). None of it exists at v0.83.0, where the three methods take `(providerId)`,
    /// `(providerId, entry)` and `(providerId)`.
    ///
    /// RED before this pass for the strongest possible reason: `ModelsStoreOperationOptions` did
    /// not exist and the trait methods took no options argument, so this test did not COMPILE.
    /// The behavioural claims — that an aborted call is `ProviderError::Aborted`, and that it does
    /// not mutate — are new with the parameter.
    #[tokio::test]
    async fn cfg042_an_aborted_signal_rejects_the_operation_without_mutating() {
        let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
        let kept = ModelsStoreEntry {
            checked_at: Some(1),
            ..ModelsStoreEntry::default()
        };
        store.write("groq", kept.clone(), None).await.unwrap();

        let cancelled = ModelsStoreOperationOptions {
            signal: Some(CancelToken::new()),
        };
        if let Some(t) = cancelled.signal.as_ref() {
            t.cancel();
        }

        // All three refuse, with `Aborted` — the taxonomy counterpart of `AbortError`.
        for err in [
            store.read("groq", Some(&cancelled)).await.unwrap_err(),
            store
                .write("groq", ModelsStoreEntry::default(), Some(&cancelled))
                .await
                .unwrap_err(),
            store.delete("groq", Some(&cancelled)).await.unwrap_err(),
        ] {
            assert!(matches!(err, ProviderError::Aborted), "got: {err:?}");
            assert_eq!(err.code(), "aborted");
        }

        // The check runs BEFORE the map is touched: the aborted write did not overwrite and the
        // aborted delete did not remove.
        assert_eq!(store.read("groq", None).await.unwrap(), Some(kept));
    }

    /// Both `?.`s in `options?.signal?.throwIfAborted()` are optional, so two shapes are no-ops:
    /// no options at all, and options carrying no signal. A LIVE (uncancelled) token is a third.
    /// Every cyrup call site today passes the first shape, which is what pi's own
    /// `remote-catalog-provider.ts` call sites pass, so this pins that the parameter did not
    /// quietly make the common path fallible.
    ///
    /// RED before this pass: did not compile (no such type, no such parameter).
    #[tokio::test]
    async fn cfg042_absent_signal_and_live_signal_are_both_no_ops() {
        let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
        let no_signal = ModelsStoreOperationOptions::default();
        let live = ModelsStoreOperationOptions {
            signal: Some(CancelToken::new()),
        };

        for options in [None, Some(&no_signal), Some(&live)] {
            let entry = ModelsStoreEntry {
                checked_at: Some(3),
                ..ModelsStoreEntry::default()
            };
            store.write("groq", entry.clone(), options).await.unwrap();
            assert_eq!(store.read("groq", options).await.unwrap(), Some(entry));
            store.delete("groq", options).await.unwrap();
            assert!(store.read("groq", options).await.unwrap().is_none());
        }
    }

    /// The scoped wrapper must not be a cancellation hole: `ProviderModelsStore` forwards the bag
    /// rather than dropping it. (cyrup keeps this type; upstream deleted it at v0.84.1 — see the
    /// type's own doc. Whichever way that drift resolves, forwarding is the correct behaviour.)
    ///
    /// RED before this pass: did not compile.
    #[tokio::test]
    async fn cfg042_the_scoped_wrapper_forwards_the_signal() {
        let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
        let scoped = ProviderModelsStore::new(store, "groq");
        let cancelled = ModelsStoreOperationOptions {
            signal: Some(CancelToken::new()),
        };
        if let Some(t) = cancelled.signal.as_ref() {
            t.cancel();
        }

        assert!(matches!(
            scoped.read(Some(&cancelled)).await.unwrap_err(),
            ProviderError::Aborted
        ));
        assert!(matches!(
            scoped
                .write(ModelsStoreEntry::default(), Some(&cancelled))
                .await
                .unwrap_err(),
            ProviderError::Aborted
        ));
        assert!(matches!(
            scoped.delete(Some(&cancelled)).await.unwrap_err(),
            ProviderError::Aborted
        ));
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

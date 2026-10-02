//! H2 — the host-side contract an extension reaches a provider catalog refresh through:
//! `HostServices::refresh_provider` (pi `modelRegistry.refresh({ providers: [id], allowNetwork,
//! signal })`, `packages/ai/src/models.ts:546-606` @v0.99.2-17) and the `RefreshModelsContext` a
//! provider's `refresh_models` reads (`:74-90`).
//!
//! The registry that implements the verb is `cyrup-session-svc`'s `GuestProviderRegistry`, tested
//! there. What this crate owns, and pins here, is the default answer of a host with no registry, the
//! object safety of the verb on `Arc<dyn HostServices>`, and the scoping of the per-refresh context.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::host::HostServices;
use crate::host::services::{
    CannedResponses, DenyServices, ModelsPersist, ModelsPublication, ModelsPublisher,
    ProviderRefreshContext, RecordingServices,
};
use cyrup_core::CancelToken;
use cyrup_provider::{ModelsStoreEntry, ProviderError};

/// A publisher that records what it is handed and answers a fixed verdict.
struct Recording {
    verdict: Result<bool, &'static str>,
    persisted: Mutex<Vec<bool>>,
    updates: AtomicUsize,
}

#[async_trait::async_trait]
impl ModelsPublisher for Recording {
    async fn publish(&self, publication: ModelsPublication) -> Result<bool, ProviderError> {
        self.persisted.lock().unwrap().push(matches!(
            publication.persist,
            Some(ModelsPersist::Write { .. })
        ));
        if let Some(update) = publication.update {
            update();
            self.updates.fetch_add(1, Ordering::SeqCst);
        }
        self.verdict
            .map_err(|message| ProviderError::ModelSource(message.into()))
    }
}

fn context(publisher: Arc<dyn ModelsPublisher>, allow_network: bool) -> ProviderRefreshContext {
    ProviderRefreshContext::new(
        None,
        Some(ModelsStoreEntry::default()),
        Vec::new(),
        allow_network,
        false,
        CancelToken::new(),
        publisher,
    )
}

/// A host with no model registry must not answer `refresh_provider` with a clean result: an
/// extension that asked for a refresh would read "nothing failed" as "the catalog is current". It
/// reports an error under the provider's id instead — through `Arc<dyn HostServices>`, which is how a
/// native extension holds the backend.
#[tokio::test]
async fn a_host_without_a_registry_reports_the_provider_as_failed() {
    let backends: Vec<Arc<dyn HostServices>> = vec![
        Arc::new(DenyServices),
        Arc::new(RecordingServices::new(CannedResponses::default())),
    ];
    for services in backends {
        let result = services
            .refresh_provider("llama.cpp", true, CancelToken::new())
            .await;

        assert!(!result.aborted);
        assert_eq!(result.errors.len(), 1, "{result:?}");
        assert!(
            result.error_for("llama.cpp").is_some(),
            "the error is keyed by the provider id, as pi's `errors.get(id)` reads it: {result:?}"
        );
        assert!(!result.is_clean());
    }
}

/// An abort is an abort even on a host with nothing to refresh (pi `models.ts:550`): `aborted`, and
/// no error — a cancelled caller does not also get a provider failure.
#[tokio::test]
async fn a_host_without_a_registry_still_reports_an_abort_as_an_abort() {
    let cancel = CancelToken::new();
    cancel.cancel();

    let result = DenyServices
        .refresh_provider("llama.cpp", true, cancel)
        .await;

    assert!(result.aborted);
    assert!(result.errors.is_empty(), "{result:?}");
}

/// The context exists only for the duration of the refresh the registry runs: outside a scope a
/// provider finds none (so it has nothing to restore from or publish to), inside it finds exactly the
/// one it was given, and after the scope it is gone again.
#[tokio::test]
async fn the_refresh_context_is_visible_only_inside_its_scope() {
    assert!(ProviderRefreshContext::current().is_none());
    let publisher = Arc::new(Recording {
        verdict: Ok(true),
        persisted: Mutex::new(Vec::new()),
        updates: AtomicUsize::new(0),
    });

    let seen = context(publisher, true)
        .scope(async {
            ProviderRefreshContext::current().map(|c| (c.allow_network, c.stored.is_some()))
        })
        .await;

    assert_eq!(seen, Some((true, true)));
    assert!(ProviderRefreshContext::current().is_none());
}

/// Two refreshes running at once on one runtime each see THEIR OWN context (the registry refreshes
/// providers concurrently): the scope is per task, not per thread or process.
#[tokio::test]
async fn concurrent_refresh_contexts_do_not_leak_into_each_other() {
    let publisher: Arc<dyn ModelsPublisher> = Arc::new(Recording {
        verdict: Ok(true),
        persisted: Mutex::new(Vec::new()),
        updates: AtomicUsize::new(0),
    });
    let network = context(Arc::clone(&publisher), true).scope(async {
        tokio::task::yield_now().await;
        ProviderRefreshContext::current().map(|c| c.allow_network)
    });
    let cache = context(publisher, false).scope(async {
        tokio::task::yield_now().await;
        ProviderRefreshContext::current().map(|c| c.allow_network)
    });

    let (network, cache) = tokio::join!(network, cache);

    assert_eq!(network, Some(true));
    assert_eq!(cache, Some(false));
}

/// `publish` is the registry's publisher: the publication reaches it unchanged and its verdict —
/// `Ok(false)` for a superseded refresh, an error for a store failure — is what the provider sees
/// (pi `publish(): Promise<boolean>`, `models.ts:83`).
#[tokio::test]
async fn publish_forwards_the_publication_and_returns_the_registrys_verdict() {
    let applied = Arc::new(Recording {
        verdict: Ok(true),
        persisted: Mutex::new(Vec::new()),
        updates: AtomicUsize::new(0),
    });
    let ctx = context(Arc::clone(&applied) as Arc<dyn ModelsPublisher>, true);
    let verdict = ctx
        .publish(ModelsPublication {
            persist: Some(ModelsPersist::Write {
                entry: ModelsStoreEntry::default(),
                classifiers: Vec::new(),
            }),
            update: Some(Box::new(|| {})),
        })
        .await;
    assert!(matches!(verdict, Ok(true)));
    assert_eq!(*applied.persisted.lock().unwrap(), vec![true]);
    assert_eq!(applied.updates.load(Ordering::SeqCst), 1);

    let stale = context(
        Arc::new(Recording {
            verdict: Ok(false),
            persisted: Mutex::new(Vec::new()),
            updates: AtomicUsize::new(0),
        }),
        true,
    );
    assert!(matches!(
        stale.publish(ModelsPublication::default()).await,
        Ok(false)
    ));

    let broken = context(
        Arc::new(Recording {
            verdict: Err("store write failed"),
            persisted: Mutex::new(Vec::new()),
            updates: AtomicUsize::new(0),
        }),
        true,
    );
    let error = broken
        .publish(ModelsPublication::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("store write failed"));
}

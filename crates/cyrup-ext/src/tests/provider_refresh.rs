//! H2 — the host-side contract an extension reaches a provider catalog refresh through:
//! `HostServices::refresh_provider` (pi `modelRegistry.refresh({ providers: [id], allowNetwork,
//! signal })`, `packages/ai/src/models.ts:546-606` @v0.99.2-17) and the `RefreshModelsContext` a
//! provider's `refresh_models` reads (`:74-90`).
//!
//! The registry that implements the verb is `cyrup-session-svc`'s `GuestProviderRegistry`, tested
//! there. What this crate owns, and pins here, is the default answer of a host with no registry, the
//! object safety of the verb on `Arc<dyn HostServices>`, and the publication types that cross it
//! (re-exported from `cyrup-provider`, where the per-refresh context lives, PROV-111).
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
    RecordingServices,
};
use cyrup_core::CancelToken;
use cyrup_provider::{Credential, ModelsStoreEntry, ProviderError, RefreshModelsContext};

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

fn context(publisher: Arc<dyn ModelsPublisher>, allow_network: bool) -> RefreshModelsContext {
    RefreshModelsContext {
        credential: Some(Credential::api_key("sk-secret")),
        stored: Some(ModelsStoreEntry::default()),
        publisher: Some(publisher),
        allow_network,
        ..RefreshModelsContext::default()
    }
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
            .refresh_provider("llama.cpp", true, false, CancelToken::new())
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
        .refresh_provider("llama.cpp", true, false, cancel)
        .await;

    assert!(result.aborted);
    assert!(result.errors.is_empty(), "{result:?}");
}

/// The context is a plain owned value: a provider that does its work in a spawned task hands the
/// task a clone and the task sees the same credential, stored snapshot, phase flag and publisher
/// (PROV-111; the task-local it replaced did not cross a `tokio::spawn`).
#[tokio::test]
async fn a_clone_of_the_refresh_context_carries_everything_into_a_spawned_task() {
    let publisher = Arc::new(Recording {
        verdict: Ok(true),
        persisted: Mutex::new(Vec::new()),
        updates: AtomicUsize::new(0),
    });
    let ctx = context(Arc::clone(&publisher) as Arc<dyn ModelsPublisher>, true);

    let seen = tokio::spawn({
        let ctx = ctx.clone();
        async move {
            let published = ctx.publish(ModelsPublication::default()).await;
            (
                ctx.allow_network,
                ctx.stored.is_some(),
                matches!(ctx.credential, Some(Credential::ApiKey { .. })),
                published.ok(),
            )
        }
    })
    .await
    .unwrap();

    assert_eq!(seen, (true, true, true, Some(true)));
    assert_eq!(publisher.persisted.lock().unwrap().len(), 1);
}

/// Without a publisher (an engine with no store, such as `Models::refresh_with`) a publication is
/// not applied and the answer is `Ok(false)` — "stop", as for a superseded refresh — rather than a
/// panic or a silent success; the `update` never runs.
#[tokio::test]
async fn publishing_without_a_publisher_applies_nothing() {
    let ran = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&ran);
    let verdict = RefreshModelsContext::default()
        .publish(ModelsPublication {
            persist: None,
            update: Some(Box::new(move || {
                flag.fetch_add(1, Ordering::SeqCst);
            })),
        })
        .await;

    assert!(matches!(verdict, Ok(false)), "{verdict:?}");
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}

/// The context's `Debug` says whether a credential is present, never what it is.
#[test]
fn the_refresh_contexts_debug_output_does_not_leak_the_credential() {
    let publisher: Arc<dyn ModelsPublisher> = Arc::new(Recording {
        verdict: Ok(true),
        persisted: Mutex::new(Vec::new()),
        updates: AtomicUsize::new(0),
    });

    let shown = format!("{:?}", context(publisher, true));

    assert!(!shown.contains("sk-secret"), "{shown}");
    assert!(shown.contains("credential"), "{shown}");
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

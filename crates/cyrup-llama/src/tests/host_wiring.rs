//! What the llama.cpp extension asks of its host, and what it does with the answers (EXT-027).
//!
//! Two seams a production session exposed, neither reachable from the upstream test file because pi
//! has no host to be late and no store whose write can fail behind `publish`:
//!
//! * the provider's credential store is the HOST's, asked at every call. A native is built before
//!   the host attaches the credential store (`LiveHostServices::attach_provider_auth` needs the
//!   session's provider registry, which does not exist yet at `init`), so a store taken once at
//!   `init` was the empty fallback for the whole session, and the stored key never reached a stream.
//! * a failed catalog write fails the refresh (`await context.publish(..)` rejects,
//!   `extensions/llama/provider.ts:213-222`, `:251-258` @v0.99.2-17), which is how `/llama` learns
//!   the catalog was not saved (`index.ts:61-62`). The host's publisher answered a `bool`, so the
//!   refresh ended quietly with nothing installed.
//!
//! **No network beyond the loopback fake router.**
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use cyrup_ext::host::HostServices;
use cyrup_ext::host::services::{ModelsPublication, ModelsPublisher};
use cyrup_provider::auth::{Credential, CredentialStore, InMemoryCredentialStore};
use cyrup_provider::{Provider, ProviderError, RefreshModelsContext};
use serde_json::json;

use crate::LLAMA_PROVIDER_ID;
use crate::error::LlamaError;
use crate::extension::{ContextRefreshHost, HostCredentials};
use crate::provider::{
    CatalogEntry, CatalogPublication, LlamaController, LlamaControllerOptions, LlamaRefreshHost,
};
use crate::tests::fake_server::{FakeLlamaServer, model_with};

// ------------------------------------------------------------------------------ the host's store

/// A host whose credential store is attached some time after the extension was handed the host.
#[derive(Default)]
struct LateStoreHost {
    store: Mutex<Option<Arc<dyn CredentialStore>>>,
}

impl LateStoreHost {
    fn attach(&self, store: Arc<dyn CredentialStore>) {
        *self.store.lock().unwrap() = Some(store);
    }
}

impl HostServices for LateStoreHost {
    fn provider_credentials(&self, provider_id: &str) -> Option<Arc<dyn CredentialStore>> {
        assert_eq!(provider_id, LLAMA_PROVIDER_ID, "asked for its own provider");
        self.store.lock().unwrap().clone()
    }
}

fn llama() -> cyrup_core::ProviderId {
    cyrup_core::ProviderId::from(LLAMA_PROVIDER_ID)
}

fn api_key(key: &str) -> Credential {
    Credential::ApiKey {
        key: Some(key.to_string()),
        env: None,
    }
}

async fn store_with(key: &str) -> Arc<dyn CredentialStore> {
    let store = InMemoryCredentialStore::new();
    store.insert(llama(), api_key(key));
    Arc::new(store)
}

fn key_of(credential: Option<Credential>) -> Option<String> {
    match credential {
        Some(Credential::ApiKey { key, .. }) => key,
        other => panic!("expected an api-key credential or none, got {other:?}"),
    }
}

/// The store the extension hands its provider, over a services cell the host fills in.
fn credentials_over(host: Option<Arc<LateStoreHost>>) -> HostCredentials {
    let cell: Arc<OnceLock<Arc<dyn HostServices>>> = Arc::new(OnceLock::new());
    if let Some(host) = host {
        let _ = cell.set(host as Arc<dyn HostServices>);
    }
    HostCredentials::new(cell)
}

/// The store attached AFTER the host was handed over is the one reads reach: `init` runs before the
/// attachment, and the provider must still see the credential.
#[tokio::test]
async fn a_credential_store_attached_after_the_host_was_handed_over_is_read() {
    let host = Arc::new(LateStoreHost::default());
    let credentials = credentials_over(Some(Arc::clone(&host)));
    assert!(
        credentials.read(&llama()).await.unwrap().is_none(),
        "nothing attached yet: the provider simply has no credential"
    );

    host.attach(store_with("sk-stored").await);

    assert_eq!(
        key_of(credentials.read(&llama()).await.unwrap()).as_deref(),
        Some("sk-stored"),
        "the store the host attached later answers the very next read"
    );
}

/// The host's store is asked every time, so a login stored after the first read is seen, and a
/// replaced store is followed.
#[tokio::test]
async fn every_read_asks_the_host_again() {
    let host = Arc::new(LateStoreHost::default());
    let credentials = credentials_over(Some(Arc::clone(&host)));
    host.attach(store_with("sk-one").await);
    assert_eq!(
        key_of(credentials.read(&llama()).await.unwrap()).as_deref(),
        Some("sk-one")
    );

    host.attach(store_with("sk-two").await);

    assert_eq!(
        key_of(credentials.read(&llama()).await.unwrap()).as_deref(),
        Some("sk-two")
    );
}

/// Writes and deletes go to the host's store too, not to a private copy: a credential the provider
/// stores (an OAuth-style refresh inside `modify`) is the host's credential.
#[tokio::test]
async fn modify_list_and_delete_reach_the_hosts_store() {
    let host = Arc::new(LateStoreHost::default());
    let hosts_store = store_with("sk-host").await;
    host.attach(Arc::clone(&hosts_store));
    let credentials = credentials_over(Some(host));

    let listed = credentials.list().await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|i| i.provider.as_str())
            .collect::<Vec<_>>(),
        [LLAMA_PROVIDER_ID]
    );

    credentials
        .modify(
            &llama(),
            Box::new(|_| Box::pin(async { Ok(Some(api_key("sk-modified"))) })),
        )
        .await
        .unwrap();
    assert_eq!(
        key_of(hosts_store.read(&llama()).await.unwrap()).as_deref(),
        Some("sk-modified")
    );

    credentials.delete(&llama()).await.unwrap();
    assert!(hosts_store.read(&llama()).await.unwrap().is_none());
}

/// A host with no credential store (the default host, or no host at all) leaves the provider with a
/// process-local store: registered, working, and holding nothing until something is written.
#[tokio::test]
async fn without_a_host_store_the_provider_has_a_working_local_one() {
    for host in [None, Some(Arc::new(LateStoreHost::default()))] {
        let credentials = credentials_over(host);
        assert!(credentials.read(&llama()).await.unwrap().is_none());

        credentials
            .modify(
                &llama(),
                Box::new(|_| Box::pin(async { Ok(Some(api_key("sk-local"))) })),
            )
            .await
            .unwrap();

        assert_eq!(
            key_of(credentials.read(&llama()).await.unwrap()).as_deref(),
            Some("sk-local")
        );
    }
}

// ------------------------------------------------------------------------------ the publication

/// The host's publisher behind `context.publish`.
struct Recorder {
    /// `Err` text, or `None` to apply the publication.
    failure: Option<&'static str>,
    persisted: Mutex<Vec<(Vec<String>, Vec<String>)>>,
}

#[async_trait::async_trait]
impl ModelsPublisher for Recorder {
    async fn publish(&self, publication: ModelsPublication) -> Result<bool, ProviderError> {
        if let Some(reason) = self.failure {
            return Err(ProviderError::ModelSource(reason.into()));
        }
        if let Some(cyrup_ext::host::services::ModelsPersist::Write { entry, classifiers }) =
            publication.persist
        {
            self.persisted.lock().unwrap().push((
                entry
                    .models
                    .iter()
                    .map(|m| m.id.as_str().to_string())
                    .collect(),
                classifiers
                    .iter()
                    .map(|m| m.id.as_str().to_string())
                    .collect(),
            ));
        }
        if let Some(update) = publication.update {
            update();
        }
        Ok(true)
    }
}

/// The context the host engine hands `refresh_models`: the credential, the stored catalog and the
/// publisher are all on the argument (PROV-111).
fn refresh_context(
    credential: Option<Credential>,
    stored: Option<cyrup_provider::ModelsStoreEntry>,
    stored_classifiers: Vec<cyrup_provider::ClassifierModel>,
    publisher: Arc<dyn ModelsPublisher>,
) -> RefreshModelsContext {
    RefreshModelsContext {
        credential,
        stored,
        stored_classifiers,
        publisher: Some(publisher),
        ..RefreshModelsContext::default()
    }
}

/// One refresh through the extension's own publisher ([`ContextRefreshHost`]), the way the host
/// engine runs it: the provider's `refresh_models` over a [`RefreshModelsContext`] whose
/// credential names the fake router.
async fn refresh_through_the_host(
    server: &FakeLlamaServer,
    recorder: Arc<Recorder>,
) -> (Option<Result<(), ProviderError>>, LlamaController) {
    let controller = LlamaController::new(
        LlamaControllerOptions::new(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(|_provider| Ok(())),
        )
        .with_refresh_host(Arc::new(ContextRefreshHost) as Arc<dyn LlamaRefreshHost>),
    );
    let mut env = BTreeMap::new();
    env.insert("LLAMA_BASE_URL".to_string(), server.url().to_string());
    let context = refresh_context(
        Some(Credential::ApiKey {
            key: None,
            env: Some(env),
        }),
        None,
        Vec::new(),
        recorder,
    );
    let provider = controller.provider();
    let outcome = Provider::refresh_models(provider.as_ref(), &context).await;
    (outcome, controller)
}

async fn router() -> FakeLlamaServer {
    FakeLlamaServer::with_models(vec![model_with(
        "tiny",
        "loaded",
        json!({ "meta": { "n_ctx": 4096 } }),
    )])
    .await
}

/// A catalog the store refused to save fails the refresh, with the store's own reason, instead of
/// ending it as if nothing needed doing; and the catalog is not installed (`update` runs only after
/// persistence succeeded, `models.ts:516-518`).
#[tokio::test]
async fn a_catalog_the_store_refused_fails_the_refresh_with_its_reason() {
    let server = router().await;
    let recorder = Arc::new(Recorder {
        failure: Some("disk full"),
        persisted: Mutex::new(Vec::new()),
    });

    let (outcome, controller) = refresh_through_the_host(&server, recorder).await;

    let error = outcome
        .expect("the provider refreshes")
        .expect_err("a refused write fails the refresh");
    assert!(error.to_string().contains("disk full"), "{error}");
    assert!(
        controller.provider().models().is_empty(),
        "nothing was installed over the catalog the store refused"
    );
}

/// The control: the same refresh over a store that accepts the write succeeds, persists the chat
/// models and their classifier twins, and installs the catalog.
#[tokio::test]
async fn a_catalog_the_store_accepts_is_persisted_and_installed() {
    let server = router().await;
    let recorder = Arc::new(Recorder {
        failure: None,
        persisted: Mutex::new(Vec::new()),
    });

    let (outcome, controller) = refresh_through_the_host(&server, Arc::clone(&recorder)).await;

    outcome.expect("the provider refreshes").expect("clean");
    assert_eq!(
        *recorder.persisted.lock().unwrap(),
        [(vec!["tiny".to_string()], vec!["tiny".to_string()])]
    );
    assert_eq!(controller.provider().models().len(), 1);
}

/// The error survives as a [`LlamaError`] on the trait-level seam too: `publish` is where the
/// host's failure is reported, as pi's `context.publish` rejects (`models.ts:498-518`), and it is
/// not folded into the `false` that means "superseded".
#[tokio::test]
async fn publish_reports_the_stores_failure() {
    let recorder = Arc::new(Recorder {
        failure: Some("read-only file system"),
        persisted: Mutex::new(Vec::new()),
    });
    let context = refresh_context(None, None, Vec::new(), recorder);
    let publication = || CatalogPublication {
        persist: Some(CatalogEntry {
            models: Vec::new(),
            checked_at: 1,
        }),
        update: None,
    };

    let published = ContextRefreshHost.publish(&context, publication()).await;

    match published {
        Err(LlamaError::Message(text)) => assert!(text.contains("read-only"), "{text}"),
        other => panic!("expected the store's failure, got {other:?}"),
    }
}

/// An aborted publication stays an abort: `ProviderError::Aborted` is `LlamaError::Cancelled`, so a
/// refresh the user stopped is not reported as a failed write.
#[tokio::test]
async fn an_aborted_publication_is_a_cancellation_not_a_failure() {
    struct Aborts;
    #[async_trait::async_trait]
    impl ModelsPublisher for Aborts {
        async fn publish(&self, _p: ModelsPublication) -> Result<bool, ProviderError> {
            Err(ProviderError::Aborted)
        }
    }
    let context = refresh_context(None, None, Vec::new(), Arc::new(Aborts));

    let published = ContextRefreshHost
        .publish(
            &context,
            CatalogPublication {
                persist: None,
                update: None,
            },
        )
        .await;

    assert!(
        matches!(published, Err(LlamaError::Cancelled)),
        "{published:?}"
    );
}

/// Given a context with no publisher (a refresh no host-run engine made) there is no store:
/// nothing is applied, neither the write nor the `update`, and it answers "do not continue"
/// (`false`), as a superseded refresh does.
#[tokio::test]
async fn publishing_outside_a_host_run_refresh_applies_nothing() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let ran = Arc::new(AtomicBool::new(false));
    let publication = |ran: &Arc<AtomicBool>| {
        let ran = Arc::clone(ran);
        CatalogPublication {
            persist: None,
            update: Some(Box::new(move || ran.store(true, Ordering::SeqCst))),
        }
    };

    let bare = RefreshModelsContext::default();
    let published = ContextRefreshHost.publish(&bare, publication(&ran)).await;
    assert!(matches!(published, Ok(false)), "{published:?}");
    assert!(!ran.load(Ordering::SeqCst), "the update never ran");
    assert!(ContextRefreshHost.credential(&bare).await.is_none());
    assert!(ContextRefreshHost.stored(&bare).await.is_none());
}

/// The persisted entry the host snapshots is read back as chat models followed by the classifier
/// models stored beside them, with the time of the check (`0` when it has none).
#[tokio::test]
async fn the_stored_catalog_is_the_chat_models_then_the_classifiers() {
    use crate::client::LlamaModelInfo;
    use crate::model::{to_classifier_model, to_model};
    use cyrup_provider::{AnyModel, ModelsStoreEntry};

    let info: LlamaModelInfo = serde_json::from_value(model_with(
        "tiny",
        "loaded",
        json!({ "meta": { "n_ctx": 4096 } }),
    ))
    .unwrap();
    let chat = to_model(&info, "http://127.0.0.1:8080", None, None).unwrap();
    let classifier = to_classifier_model(&info, "http://127.0.0.1:8080", None);
    let stored = |checked_at| {
        refresh_context(
            None,
            Some(ModelsStoreEntry {
                models: vec![chat.clone()],
                checked_at,
                ..ModelsStoreEntry::default()
            }),
            vec![classifier.clone()],
            Arc::new(Recorder {
                failure: None,
                persisted: Mutex::new(Vec::new()),
            }),
        )
    };

    let entry = ContextRefreshHost
        .stored(&stored(Some(1234)))
        .await
        .expect("a snapshot was taken");
    assert_eq!(
        entry.models,
        vec![
            AnyModel::Chat(chat.clone()),
            AnyModel::Classifier(classifier.clone())
        ]
    );
    assert_eq!(entry.checked_at, 1234);

    let unchecked = ContextRefreshHost
        .stored(&stored(None))
        .await
        .expect("a snapshot was taken");
    assert_eq!(unchecked.checked_at, 0, "no check time reads as 0");
}

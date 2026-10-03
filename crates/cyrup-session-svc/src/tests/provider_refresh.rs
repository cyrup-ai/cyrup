//! H2 — the host drives `Provider::refresh_models` on an extension-registered provider: the startup
//! cache-only restore, the network refresh and its persistence, the offline switch, a stale refresh
//! that must not publish after a newer one, abort, and per-provider errors.
//!
//! pi: `ModelsImpl.refresh({ providers, allowNetwork, signal })` (`packages/ai/src/models.ts:546-606`
//! @v0.99.2-17) restores each provider's stored catalog before any network call, runs the network
//! phase only when allowed and authenticated, and hands the provider a generation-checked
//! `publish({ persist, update })` (`:498-525`). The llama extension is the one provider that uses it
//! (`extensions/llama/provider.ts:201-259`), and its `/llama` command reaches it through
//! `modelRegistry.refresh({ providers: [id], allowNetwork: true, signal })` (`index.ts:54-59`), which
//! cyrup spells `HostServices::refresh_provider`.
//!
//! **No network.** The providers here are scripted in memory; "network" is a phase flag.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, EventStream, ProviderId};
use cyrup_ext::host::HostServices;
use cyrup_ext::host::services::{
    ModelsPersist, ModelsPublication, ProviderRefreshRequest, ProviderRefresher,
};
use cyrup_ext::provider::ModelRegistrySink;
use cyrup_provider::{
    ApiKeyAuth, AuthContext, AuthError, AuthResult, ClassifierModel, ConfigProvider, Context,
    Credential, CredentialStore, InMemoryCredentialStore, InMemoryModelsStore, Modality, Model,
    ModelAuth, ModelCost, ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions, Provider,
    ProviderAuth, ProviderError, RefreshModelsContext, StreamEvent, StreamOptions,
};
use futures::future::BoxFuture;
use tokio::sync::Notify;

use crate::GuestProviderRegistry;
use crate::host_services::LiveHostServices;

const LLAMA: &str = "llama.cpp";

type Script = Arc<
    dyn Fn(RefreshModelsContext) -> BoxFuture<'static, Result<(), ProviderError>> + Send + Sync,
>;

/// What one `refresh_models` call observed.
#[derive(Clone, Debug)]
struct Seen {
    /// The trait argument's `allow_network`.
    allow_network: bool,
    /// The `allow_network` of the context clone handed to the script — must agree with the argument.
    context_network: Option<bool>,
    /// The trait argument's `force` and the context's — must agree.
    force: bool,
    context_force: bool,
    credential: Option<Credential>,
    stored_ids: Option<Vec<String>>,
    stored_checked_at: Option<i64>,
    stored_last_modified: Option<i64>,
    stored_classifier_ids: Vec<String>,
}

/// A refreshable provider whose `refresh_models` records what it was handed and then runs `script`.
struct Scripted {
    id: ProviderId,
    inner: Arc<dyn Provider>,
    auth: Option<ProviderAuth>,
    script: Script,
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[async_trait::async_trait]
impl Provider for Scripted {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        self.inner.models()
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        self.auth.as_ref()
    }
    fn has_refresh_models(&self) -> bool {
        true
    }
    async fn refresh_models(
        &self,
        ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        let context = ctx.clone();
        self.seen.lock().unwrap().push(Seen {
            allow_network: ctx.allow_network,
            context_network: Some(context.allow_network),
            force: ctx.force,
            context_force: context.force,
            credential: context.credential.clone(),
            stored_ids: context
                .stored
                .as_ref()
                .map(|e| e.models.iter().map(|m| m.id.as_str().to_string()).collect()),
            stored_checked_at: context.stored.as_ref().and_then(|e| e.checked_at),
            stored_last_modified: context.stored.as_ref().and_then(|e| e.last_modified),
            stored_classifier_ids: context
                .stored_classifiers
                .iter()
                .map(|c| c.id.as_str().to_string())
                .collect(),
        });
        Some((self.script)(context).await)
    }
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream(model, context, options)
    }
}

fn catalog(provider: &str, ids: &[&str]) -> Vec<Model> {
    ids.iter()
        .map(|id| Model {
            id: (*id).into(),
            name: (*id).to_string(),
            api: "openai-completions".into(),
            provider: provider.into(),
            base_url: "http://127.0.0.1:8080/v1".to_string(),
            reasoning: false,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 4096,
            max_tokens: 4096,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        })
        .collect()
}

fn classifier(provider: &str, id: &str) -> ClassifierModel {
    ClassifierModel {
        id: id.into(),
        name: id.to_string(),
        api: "llama-cpp-classify".into(),
        provider: provider.into(),
        base_url: "http://127.0.0.1:8080".to_string(),
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        headers: None,
        context_window: 4096,
    }
}

fn plain_provider(provider: &str, ids: &[&str]) -> Arc<dyn Provider> {
    ConfigProvider::new(provider, provider, None, catalog(provider, ids)).into_arc()
}

fn scripted(
    id: &str,
    ids: &[&str],
    auth: Option<ProviderAuth>,
    script: Script,
) -> (Arc<Scripted>, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(Scripted {
        id: id.into(),
        inner: plain_provider(id, ids),
        auth,
        script,
        seen: Arc::clone(&seen),
    });
    (provider, seen)
}

fn script<F>(f: F) -> Script
where
    F: Fn(RefreshModelsContext) -> BoxFuture<'static, Result<(), ProviderError>>
        + Send
        + Sync
        + 'static,
{
    Arc::new(f)
}

fn phases(seen: &Arc<Mutex<Vec<Seen>>>) -> Vec<bool> {
    seen.lock()
        .unwrap()
        .iter()
        .map(|s| s.allow_network)
        .collect()
}

fn registry_over(store: Arc<dyn ModelsStore>) -> Arc<GuestProviderRegistry> {
    let registry = Arc::new(GuestProviderRegistry::new());
    registry.attach_models_store(store);
    registry
}

fn request(providers: Option<&[&str]>, allow_network: Option<bool>) -> ProviderRefreshRequest {
    ProviderRefreshRequest {
        providers: providers.map(|ids| ids.iter().map(|s| (*s).to_string()).collect()),
        allow_network,
        force: false,
        cancel: CancelToken::new(),
    }
}

/// Wait until every detached refresh task of `registry` has ended: the per-provider operations and
/// the publications a superseded or aborted refresh left running (they are detached by design, pi's
/// `raceWithAbortSignal` does not cancel the queued publication, only the wait). An assertion that
/// nothing landed is then not satisfied merely because a stale write had not happened yet: the wait
/// is for the event, the tasks' end, not for a stretch of the clock.
///
/// Call it only once every gate a task may be parked on has been released.
async fn settle(registry: &GuestProviderRegistry) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while registry.refresh_tasks_in_flight() > 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the detached refresh tasks ended");
}

/// Let every task that can run, run, until all are parked. For asserting that something parked on
/// a gate did NOT proceed: on a current-thread runtime, enough yields are a full pass over every
/// runnable task, with no clock involved.
async fn run_every_runnable_task() {
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
}

/// A store that never looks at the operation's abort signal (pi's `ModelsStore` takes `signal?`
/// as an option an implementation may ignore) and can hold one `write` open. With it a publication
/// is stopped only by the registry's own checks, not by the store refusing an aborted call.
struct BlindStore {
    inner: InMemoryModelsStore,
    /// `(entered, release)`: a `write` announces itself on `entered` and waits for `release`.
    write_gate: Option<(Arc<Notify>, Arc<Notify>)>,
}

impl BlindStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: InMemoryModelsStore::new(),
            write_gate: None,
        })
    }

    fn gated(entered: Arc<Notify>, release: Arc<Notify>) -> Arc<Self> {
        Arc::new(Self {
            inner: InMemoryModelsStore::new(),
            write_gate: Some((entered, release)),
        })
    }
}

#[async_trait::async_trait]
impl ModelsStore for BlindStore {
    async fn read(
        &self,
        provider_id: &str,
        _options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<Option<ModelsStoreEntry>, ProviderError> {
        self.inner.read(provider_id, None).await
    }
    async fn write(
        &self,
        provider_id: &str,
        entry: ModelsStoreEntry,
        _options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        if let Some((entered, release)) = &self.write_gate {
            entered.notify_one();
            release.notified().await;
        }
        self.inner.write(provider_id, entry, None).await
    }
    async fn delete(
        &self,
        provider_id: &str,
        _options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.inner.delete(provider_id, None).await
    }
    async fn read_classifier_models(
        &self,
        provider_id: &str,
        _options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<Vec<ClassifierModel>, ProviderError> {
        self.inner.read_classifier_models(provider_id, None).await
    }
    async fn write_classifier_models(
        &self,
        provider_id: &str,
        models: Vec<ClassifierModel>,
        _options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.inner
            .write_classifier_models(provider_id, models, None)
            .await
    }
}

fn ids(models: &[Model]) -> Vec<String> {
    models.iter().map(|m| m.id.as_str().to_string()).collect()
}

/// An API-key strategy shaped like the llama provider's `resolve` (`provider.ts:187-196`): the
/// server URL comes from the stored credential's env, else the ambient `LLAMA_BASE_URL`.
struct UrlAuth;

#[async_trait::async_trait]
impl ApiKeyAuth for UrlAuth {
    fn name(&self) -> &str {
        "llama.cpp server"
    }
    async fn resolve(
        &self,
        _model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let (key, stored_env) = match cred {
            Some(Credential::ApiKey { key, env }) => (key.clone(), env.clone()),
            _ => (None, None),
        };
        let url = match stored_env.as_ref().and_then(|e| e.get("LLAMA_BASE_URL")) {
            Some(url) => Some(url.clone()),
            None => ctx.env("LLAMA_BASE_URL").await,
        };
        Ok(url.map(|url| AuthResult {
            auth: ModelAuth {
                api_key: key.or_else(|| Some("local".to_string())),
                ..ModelAuth::default()
            },
            env: Some(BTreeMap::from([("LLAMA_BASE_URL".to_string(), url)])),
            source: Some("test".to_string()),
        }))
    }
}

fn url_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(Arc::new(UrlAuth))
}

struct MapCtx(BTreeMap<String, String>);

#[async_trait::async_trait]
impl AuthContext for MapCtx {
    async fn env(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

/// A script that publishes `ids` as the provider's catalog: persisted only in the network phase
/// (the llama provider's second publish, `provider.ts:252-258`), and `update` swaps the live
/// provider through the registry's replace path — a cyrup provider's catalog is an immutable borrow.
fn publish_catalog(
    registry: &Arc<GuestProviderRegistry>,
    id: &'static str,
    ids_: &'static [&'static str],
    log: &Arc<Mutex<Vec<String>>>,
) -> Script {
    let registry = Arc::clone(registry);
    let log = Arc::clone(log);
    script(move |ctx| {
        let registry = Arc::clone(&registry);
        let log = Arc::clone(&log);
        Box::pin(async move {
            let persist = ctx.allow_network.then(|| ModelsPersist::Write {
                entry: ModelsStoreEntry {
                    models: catalog(id, ids_),
                    checked_at: Some(100),
                    ..ModelsStoreEntry::default()
                },
                classifiers: ids_.iter().map(|m| classifier(id, m)).collect(),
            });
            let published = ctx
                .publish(ModelsPublication {
                    persist,
                    update: Some(Box::new(move || {
                        log.lock()
                            .unwrap()
                            .push(format!("update:{}", ids_.join(",")));
                        registry.upsert_live_provider(id, plain_provider(id, ids_));
                    })),
                })
                .await?;
            assert!(
                published,
                "this refresh is current, so its publication applies"
            );
            Ok(())
        })
    })
}

fn register(registry: &GuestProviderRegistry, provider: Arc<Scripted>) {
    let id = provider.id().as_str().to_string();
    registry.upsert_live_provider(&id, provider);
}

// -------------------------------------------------------------------------------------------------
// cache-only restore
// -------------------------------------------------------------------------------------------------

/// pi `models.ts:570-571` + `provider.ts:203-226`: the startup restore hands the provider the STORED
/// catalog and classifier models with `allowNetwork: false`, and the provider's own `update` makes
/// the live catalog the stored one — with no network phase after it, whatever the credential.
#[tokio::test]
async fn cache_only_restore_hands_the_stored_catalog_to_the_provider_and_stays_off_the_network() {
    let store: Arc<dyn ModelsStore> = Arc::new(InMemoryModelsStore::new());
    // The entry shape pi's llama provider persists: `{ models, checkedAt }` and no `lastModified`.
    store
        .write(
            LLAMA,
            ModelsStoreEntry {
                models: catalog(LLAMA, &["cached-a", "cached-b"]),
                checked_at: Some(5),
                last_modified: None,
                etag: None,
            },
            None,
        )
        .await
        .unwrap();
    store
        .write_classifier_models(LLAMA, vec![classifier(LLAMA, "cached-a")], None)
        .await
        .unwrap();
    let registry = registry_over(store);
    let log = Arc::new(Mutex::new(Vec::new()));
    let restore_log = Arc::clone(&log);
    let reg = Arc::clone(&registry);
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script(move |ctx| {
            let reg = Arc::clone(&reg);
            let log = Arc::clone(&restore_log);
            Box::pin(async move {
                // `provider.ts:203-226`: restore what was stored, publish `update` only.
                let stored = ctx.stored.clone().expect("a stored entry exists");
                let restored: Vec<String> = stored
                    .models
                    .iter()
                    .map(|m| m.id.as_str().to_string())
                    .collect();
                let applied = ctx
                    .publish(ModelsPublication {
                        persist: None,
                        update: Some(Box::new(move || {
                            log.lock()
                                .unwrap()
                                .push(format!("restored:{}", restored.join(",")));
                            reg.upsert_live_provider(
                                LLAMA,
                                plain_provider(LLAMA, &["cached-a", "cached-b"]),
                            );
                        })),
                    })
                    .await?;
                assert!(applied);
                Ok(())
            })
        }),
    );
    register(&registry, provider);

    let result = registry.restore_cached(CancelToken::new()).await;

    assert!(result.is_clean(), "{result:?}");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "exactly the cache-only phase ran: {seen:?}");
    let phase = &seen[0];
    assert!(!phase.allow_network);
    assert_eq!(phase.context_network, Some(false));
    assert_eq!(
        phase.stored_ids.as_deref(),
        Some(&["cached-a".to_string(), "cached-b".to_string()][..])
    );
    assert_eq!(phase.stored_checked_at, Some(5));
    assert_eq!(phase.stored_classifier_ids, vec!["cached-a".to_string()]);
    assert_eq!(
        *log.lock().unwrap(),
        vec!["restored:cached-a,cached-b".to_string()]
    );
    assert_eq!(
        ids(&registry.models()),
        vec!["cached-a".to_string(), "cached-b".to_string()],
        "the provider's update replaced the live catalog with the stored one"
    );
}

/// The `lastModified` staleness guard that makes the pi.dev overlay loader discard an entry without
/// one (`remote_catalog`, pi #7016; the reason `RadiusProvider` stamps `last_modified`) does NOT sit
/// on this path: an extension provider's entry is restored as written. Read back through the real
/// `FileModelsStore`, so the on-disk round trip is covered too.
#[tokio::test]
async fn a_stored_entry_without_last_modified_survives_the_restore_through_the_file_store() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("models-store.json");
    let file_store: Arc<dyn ModelsStore> = Arc::new(
        cyrup_config::models_store::FileModelsStore::new(path.clone()),
    );
    file_store
        .write(
            LLAMA,
            ModelsStoreEntry {
                models: catalog(LLAMA, &["on-disk"]),
                checked_at: Some(9),
                last_modified: None,
                etag: None,
            },
            None,
        )
        .await
        .unwrap();
    // A fresh store over the same file, as a new process would have.
    let registry = registry_over(Arc::new(cyrup_config::models_store::FileModelsStore::new(
        path,
    )));
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        None,
        script(|_| Box::pin(async { Ok(()) })),
    );
    register(&registry, provider);

    let result = registry.restore_cached(CancelToken::new()).await;

    assert!(result.is_clean(), "{result:?}");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].stored_ids, Some(vec!["on-disk".to_string()]));
    assert_eq!(seen[0].stored_checked_at, Some(9));
    assert_eq!(
        seen[0].stored_last_modified, None,
        "restored exactly as written, not discarded"
    );
}

// -------------------------------------------------------------------------------------------------
// network refresh persists
// -------------------------------------------------------------------------------------------------

/// pi `provider.ts:228-258`: with network allowed and a credential, the provider runs a second phase
/// whose publication persists `{ models, checkedAt }` AND replaces the live catalog. The stored
/// entry and the classifier models are what the NEXT startup's restore will read; a registry
/// generation bump is what makes `/model` see the new catalog.
#[tokio::test]
async fn a_network_refresh_persists_the_catalog_and_replaces_the_live_one() {
    let store = Arc::new(InMemoryModelsStore::new());
    let registry = registry_over(store.clone());
    let credentials = Arc::new(InMemoryCredentialStore::new());
    credentials.insert(
        ProviderId::from(LLAMA),
        Credential::ApiKey {
            key: Some("sk-local".to_string()),
            env: Some(BTreeMap::from([(
                "LLAMA_BASE_URL".to_string(),
                "http://127.0.0.1:9999".to_string(),
            )])),
        },
    );
    registry.attach_refresh_auth(credentials, Arc::new(MapCtx(BTreeMap::new())));
    let log = Arc::new(Mutex::new(Vec::new()));
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        publish_catalog(&registry, LLAMA, &["fresh-a", "fresh-b"], &log),
    );
    register(&registry, provider);
    let generation_before = registry.generation();

    let result = registry.refresh(request(Some(&[LLAMA]), Some(true))).await;

    assert!(result.is_clean(), "{result:?}");
    assert_eq!(
        phases(&seen),
        vec![false, true],
        "restore first, then the network phase"
    );
    let network = seen.lock().unwrap()[1].clone();
    match network.credential {
        Some(Credential::ApiKey { key, env }) => {
            assert_eq!(key.as_deref(), Some("sk-local"));
            assert_eq!(
                env.unwrap().get("LLAMA_BASE_URL").map(String::as_str),
                Some("http://127.0.0.1:9999"),
                "the provider reads the server URL off the resolved credential's env"
            );
        }
        other => panic!("the network phase carries the resolved api-key credential: {other:?}"),
    }
    let entry = store.read(LLAMA, None).await.unwrap().expect("persisted");
    assert_eq!(
        ids(&entry.models),
        vec!["fresh-a".to_string(), "fresh-b".to_string()]
    );
    assert_eq!(entry.checked_at, Some(100));
    let persisted_classifiers = store.read_classifier_models(LLAMA, None).await.unwrap();
    assert_eq!(
        persisted_classifiers
            .iter()
            .map(|c| c.id.as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["fresh-a".to_string(), "fresh-b".to_string()]
    );
    // Both phases published an `update`; the first (restore) one re-registered the provider and the
    // network phase still landed, so a replacement does not supersede the refresh that published it.
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "update:fresh-a,fresh-b".to_string(),
            "update:fresh-a,fresh-b".to_string()
        ]
    );
    assert_ne!(
        registry.generation(),
        generation_before,
        "the catalog swap moved the snapshot cache key"
    );
    assert_eq!(
        ids(&registry.models()),
        vec!["fresh-a".to_string(), "fresh-b".to_string()]
    );
}

/// The same refresh against the real on-disk store: what the network phase persisted is readable by
/// a store opened afterwards (the next process), classifier models excluded — see the file store.
#[tokio::test]
async fn a_persisted_chat_catalog_is_readable_by_a_second_file_store() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("models-store.json");
    let registry = registry_over(Arc::new(cyrup_config::models_store::FileModelsStore::new(
        path.clone(),
    )));
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script(|ctx| {
            Box::pin(async move {
                if ctx.allow_network {
                    ctx.publish(ModelsPublication {
                        persist: Some(ModelsPersist::Write {
                            entry: ModelsStoreEntry {
                                models: catalog(LLAMA, &["persisted"]),
                                checked_at: Some(42),
                                ..ModelsStoreEntry::default()
                            },
                            classifiers: Vec::new(),
                        }),
                        update: None,
                    })
                    .await?;
                }
                Ok(())
            })
        }),
    );
    register(&registry, provider);

    let result = registry.refresh(request(None, Some(true))).await;

    assert!(result.is_clean(), "{result:?}");
    let reopened = cyrup_config::models_store::FileModelsStore::new(path);
    let entry = reopened.read(LLAMA, None).await.unwrap().expect("on disk");
    assert_eq!(ids(&entry.models), vec!["persisted".to_string()]);
    assert_eq!(entry.checked_at, Some(42));
}

/// A `persist: null` publication deletes the stored entry (pi `models.ts:509-510`).
#[tokio::test]
async fn a_null_persist_deletes_the_stored_entry() {
    let store = Arc::new(InMemoryModelsStore::new());
    store
        .write(
            LLAMA,
            ModelsStoreEntry {
                models: catalog(LLAMA, &["old"]),
                ..ModelsStoreEntry::default()
            },
            None,
        )
        .await
        .unwrap();
    let registry = registry_over(store.clone());
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        None,
        script(|ctx| {
            Box::pin(async move {
                ctx.publish(ModelsPublication {
                    persist: Some(ModelsPersist::Delete),
                    update: None,
                })
                .await?;
                Ok(())
            })
        }),
    );
    register(&registry, provider);

    registry.restore_cached(CancelToken::new()).await;

    assert!(store.read(LLAMA, None).await.unwrap().is_none());
}

// -------------------------------------------------------------------------------------------------
// the offline switch
// -------------------------------------------------------------------------------------------------

/// pi `allowNetwork: options.allowNetwork ?? this.modelNetworkEnabled` (`core/model-runtime.ts:850`):
/// with the host's network switch OFF (PI_OFFLINE), a request that names no `allowNetwork` is
/// cache-only, and ONLY an explicit `allowNetwork: true` reaches the network — which is how pi's
/// `/llama` stays live offline. The converse holds too: an explicit `false` beats a switch that is on.
#[tokio::test]
async fn the_offline_switch_is_overridden_only_by_an_explicit_allow_network() {
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script(|_| Box::pin(async { Ok(()) })),
    );
    register(&registry, provider);

    registry.set_network_enabled(false);
    registry.refresh(request(Some(&[LLAMA]), None)).await;
    assert_eq!(
        phases(&seen),
        vec![false],
        "offline + unspecified: cache-only"
    );

    seen.lock().unwrap().clear();
    registry.refresh(request(Some(&[LLAMA]), Some(false))).await;
    assert_eq!(
        phases(&seen),
        vec![false],
        "offline + explicit false: cache-only"
    );

    seen.lock().unwrap().clear();
    let result = registry.refresh(request(Some(&[LLAMA]), Some(true))).await;
    assert!(result.is_clean(), "{result:?}");
    assert_eq!(
        phases(&seen),
        vec![false, true],
        "offline + explicit true: the network phase runs"
    );

    registry.set_network_enabled(true);
    seen.lock().unwrap().clear();
    registry.refresh(request(Some(&[LLAMA]), None)).await;
    assert_eq!(
        phases(&seen),
        vec![false, true],
        "online + unspecified: the network phase runs"
    );

    seen.lock().unwrap().clear();
    registry.refresh(request(Some(&[LLAMA]), Some(false))).await;
    assert_eq!(
        phases(&seen),
        vec![false],
        "online + explicit false: cache-only"
    );
}

/// pi `force: allowNetwork ? force : undefined` (`models.ts:541`): `force` reaches the provider in
/// the network phase only; the cache-only restore never carries it, and neither does a refresh that
/// did not ask for it.
#[tokio::test]
async fn force_reaches_only_the_network_phase() {
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script(|_| Box::pin(async { Ok(()) })),
    );
    register(&registry, provider);

    let mut forced = request(Some(&[LLAMA]), Some(true));
    forced.force = true;
    registry.refresh(forced).await;
    let calls = seen.lock().unwrap().clone();
    assert_eq!(
        calls
            .iter()
            .map(|c| (c.allow_network, c.force, c.context_force))
            .collect::<Vec<_>>(),
        vec![(false, false, false), (true, true, true)]
    );

    seen.lock().unwrap().clear();
    registry.refresh(request(Some(&[LLAMA]), Some(true))).await;
    let calls = seen.lock().unwrap().clone();
    assert!(
        calls.iter().all(|c| !c.force && !c.context_force),
        "{calls:?}"
    );

    // `force` without the network is dropped, not forwarded.
    seen.lock().unwrap().clear();
    let mut forced_offline = request(Some(&[LLAMA]), Some(false));
    forced_offline.force = true;
    registry.refresh(forced_offline).await;
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|c| !c.force && !c.context_force)
    );
}

/// pi `models.ts:575-576`: no resolvable credential, no network phase — a provider without auth, or
/// one whose auth finds nothing configured, is restored from cache and left alone.
#[tokio::test]
async fn no_credential_means_no_network_phase() {
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::new())),
    );
    let (no_auth, no_auth_seen) = scripted(
        "no-auth",
        &["a"],
        None,
        script(|_| Box::pin(async { Ok(()) })),
    );
    let (unconfigured, unconfigured_seen) = scripted(
        "unconfigured",
        &["b"],
        Some(url_auth()),
        script(|_| Box::pin(async { Ok(()) })),
    );
    register(&registry, no_auth);
    register(&registry, unconfigured);

    let result = registry.refresh(request(None, Some(true))).await;

    assert!(result.is_clean(), "{result:?}");
    assert_eq!(phases(&no_auth_seen), vec![false]);
    assert_eq!(
        phases(&unconfigured_seen),
        vec![false],
        "UrlAuth resolves nothing without a URL"
    );
}

/// The cache-only phase carries the STORED credential (pi `storedCredential`, `models.ts:571`); the
/// network phase carries the RESOLVED one (`:575-577`) — here the ambient `LLAMA_BASE_URL`.
#[tokio::test]
async fn the_restore_phase_sees_the_stored_credential_and_the_network_phase_the_resolved_one() {
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    let credentials = Arc::new(InMemoryCredentialStore::new());
    credentials.insert(ProviderId::from(LLAMA), Credential::api_key("stored-key"));
    registry.attach_refresh_auth(
        credentials,
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://ambient:8080".to_string(),
        )]))),
    );
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script(|_| Box::pin(async { Ok(()) })),
    );
    register(&registry, provider);

    registry.refresh(request(Some(&[LLAMA]), Some(true))).await;

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(
        matches!(&seen[0].credential, Some(Credential::ApiKey { key: Some(k), env: None }) if k == "stored-key")
    );
    match &seen[1].credential {
        Some(Credential::ApiKey { key, env }) => {
            assert_eq!(key.as_deref(), Some("stored-key"));
            assert_eq!(
                env.as_ref()
                    .and_then(|e| e.get("LLAMA_BASE_URL"))
                    .map(String::as_str),
                Some("http://ambient:8080")
            );
        }
        other => panic!("{other:?}"),
    }
}

// -------------------------------------------------------------------------------------------------
// stale refresh
// -------------------------------------------------------------------------------------------------

/// pi 0.84.0 (`coding-agent` CHANGELOG:760): "stale llama.cpp catalog refreshes could publish after a
/// newer refresh". Refresh 1 is held inside its network phase; refresh 2 starts, supersedes it and
/// publishes `fresh`; refresh 1 is then released and tries to publish `stale`. Neither its persist
/// nor its `update` may land, so storage and the live catalog stay on `fresh`.
#[tokio::test]
async fn a_stale_refresh_cannot_overwrite_a_newer_one() {
    // A store that ignores the abort signal, so only the registry's own gate can stop the write.
    let store = BlindStore::new();
    let registry = registry_over(store.clone());
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let log = Arc::new(Mutex::new(Vec::new()));
    let network_calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (stale_done_tx, stale_done_rx) = tokio::sync::oneshot::channel::<Result<bool, String>>();
    let stale_done = Arc::new(Mutex::new(Some(stale_done_tx)));

    let reg = Arc::clone(&registry);
    let publish_log = Arc::clone(&log);
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script({
            let (entered, release, network_calls, stale_done) = (
                Arc::clone(&entered),
                Arc::clone(&release),
                Arc::clone(&network_calls),
                Arc::clone(&stale_done),
            );
            move |ctx| {
                let (entered, release, network_calls, stale_done) = (
                    Arc::clone(&entered),
                    Arc::clone(&release),
                    Arc::clone(&network_calls),
                    Arc::clone(&stale_done),
                );
                let (reg, publish_log) = (Arc::clone(&reg), Arc::clone(&publish_log));
                Box::pin(async move {
                    if !ctx.allow_network {
                        return Ok(());
                    }
                    let call = network_calls.fetch_add(1, Ordering::SeqCst);
                    let (name, entry_ids): (&'static str, &'static [&'static str]) = if call == 0 {
                        ("stale", &["stale"])
                    } else {
                        ("fresh", &["fresh"])
                    };
                    if call == 0 {
                        entered.notify_one();
                        release.notified().await;
                    }
                    let outcome = ctx
                        .publish(ModelsPublication {
                            persist: Some(ModelsPersist::Write {
                                entry: ModelsStoreEntry {
                                    models: catalog(LLAMA, entry_ids),
                                    checked_at: Some(1),
                                    ..ModelsStoreEntry::default()
                                },
                                classifiers: Vec::new(),
                            }),
                            update: Some(Box::new(move || {
                                publish_log.lock().unwrap().push(format!("update:{name}"));
                                reg.upsert_live_provider(LLAMA, plain_provider(LLAMA, entry_ids));
                            })),
                        })
                        .await;
                    if call == 0
                        && let Some(tx) = stale_done.lock().unwrap().take()
                    {
                        let _ = tx.send(
                            outcome
                                .as_ref()
                                .map(|applied| *applied)
                                .map_err(ToString::to_string),
                        );
                    }
                    outcome.map(|_| ())
                })
            }
        }),
    );
    register(&registry, provider);

    let first = {
        let registry = Arc::clone(&registry);
        tokio::spawn(async move { registry.refresh(request(Some(&[LLAMA]), Some(true))).await })
    };
    entered.notified().await;
    // The newer refresh supersedes the held one, and completes.
    let second = registry.refresh(request(Some(&[LLAMA]), Some(true))).await;
    assert!(second.is_clean(), "{second:?}");
    assert_eq!(
        ids(&store.read(LLAMA, None).await.unwrap().unwrap().models),
        vec!["fresh".to_string()]
    );

    release.notify_one();
    let stale_outcome = stale_done_rx.await.expect("the stale publish settled");
    assert!(
        !matches!(stale_outcome, Ok(true)),
        "a superseded refresh's publish must not report success: {stale_outcome:?}"
    );
    let first = first.await.unwrap();
    assert!(
        first.errors.is_empty(),
        "a superseded refresh is a cancellation, not a provider failure: {first:?}"
    );

    settle(&registry).await;
    assert_eq!(
        ids(&store.read(LLAMA, None).await.unwrap().unwrap().models),
        vec!["fresh".to_string()],
        "the stale refresh did not overwrite the newer persisted catalog"
    );
    // Phase 1 of each refresh published nothing here, so the only updates are the network ones.
    assert_eq!(*log.lock().unwrap(), vec!["update:fresh".to_string()]);
    assert_eq!(
        ids(&registry.models()),
        vec!["fresh".to_string()],
        "nor did it replace the live catalog"
    );
}

/// pi `models.ts:515`: the generation is checked AGAIN after the persistence step. A refresh
/// superseded while its `write` is in flight has already touched the store (that cannot be undone),
/// but its `update` must not run: the live catalog never changes for a publication that lost.
#[tokio::test]
async fn a_publication_superseded_while_it_persists_does_not_run_its_update() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let store = BlindStore::gated(Arc::clone(&entered), Arc::clone(&release));
    let registry = registry_over(store.clone());
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let log = Arc::new(Mutex::new(Vec::new()));
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        publish_catalog(&registry, LLAMA, &["lost"], &log),
    );
    register(&registry, provider);
    let refresh = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move { registry.refresh(request(Some(&[LLAMA]), Some(true))).await }
    });
    // The restore phase's publication has no `persist`, so its `update` ran before the gate; the
    // network phase's `write` is the one held open.
    entered.notified().await;
    assert_eq!(*log.lock().unwrap(), vec!["update:lost".to_string()]);

    // The replacement supersedes the refresh while its write is in flight.
    registry.upsert_live_provider(LLAMA, plain_provider(LLAMA, &["replacement"]));
    release.notify_one();
    let result = refresh.await.unwrap();
    settle(&registry).await;

    assert!(result.errors.is_empty(), "{result:?}");
    assert_eq!(
        *log.lock().unwrap(),
        vec!["update:lost".to_string()],
        "the superseded publication's update did not run a second time"
    );
    assert_eq!(
        ids(&registry.models()),
        vec!["replacement".to_string()],
        "the live catalog is the replacement, not the lost refresh's"
    );
}

/// pi `setProvider` / `deleteProvider` both start with `supersedeProviderRefresh` (`models.ts:399-407`):
/// re-registering a provider while its refresh is in flight cancels that refresh, so the replaced
/// provider's late publication cannot write into the new one's storage or catalog.
#[tokio::test]
async fn re_registering_a_provider_supersedes_its_in_flight_refresh() {
    let store = Arc::new(InMemoryModelsStore::new());
    let registry = registry_over(store.clone());
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<bool>();
    let done = Arc::new(Mutex::new(Some(done_tx)));
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script({
            let (entered, release, done) = (
                Arc::clone(&entered),
                Arc::clone(&release),
                Arc::clone(&done),
            );
            move |ctx| {
                let (entered, release, done) = (
                    Arc::clone(&entered),
                    Arc::clone(&release),
                    Arc::clone(&done),
                );
                Box::pin(async move {
                    if !ctx.allow_network {
                        return Ok(());
                    }
                    entered.notify_one();
                    release.notified().await;
                    let applied = ctx
                        .publish(ModelsPublication {
                            persist: Some(ModelsPersist::Write {
                                entry: ModelsStoreEntry {
                                    models: catalog(LLAMA, &["from-replaced-provider"]),
                                    ..ModelsStoreEntry::default()
                                },
                                classifiers: Vec::new(),
                            }),
                            update: None,
                        })
                        .await
                        .unwrap_or(false);
                    if let Some(tx) = done.lock().unwrap().take() {
                        let _ = tx.send(applied);
                    }
                    Ok(())
                })
            }
        }),
    );
    register(&registry, provider);
    let refresh = {
        let registry = Arc::clone(&registry);
        tokio::spawn(async move { registry.refresh(request(Some(&[LLAMA]), Some(true))).await })
    };
    entered.notified().await;

    registry.upsert_live_provider(LLAMA, plain_provider(LLAMA, &["replacement"]));
    release.notify_one();

    assert!(
        !done_rx.await.unwrap(),
        "the replaced provider's publication was refused"
    );
    let result = refresh.await.unwrap();
    assert!(result.errors.is_empty(), "{result:?}");
    settle(&registry).await;
    assert!(
        store.read(LLAMA, None).await.unwrap().is_none(),
        "nothing was persisted"
    );
    assert_eq!(ids(&registry.models()), vec!["replacement".to_string()]);
}

// -------------------------------------------------------------------------------------------------
// abort and errors
// -------------------------------------------------------------------------------------------------

/// pi `models.ts:550` + `:605`: an already-aborted caller gets `{ aborted: true }` and no provider is
/// called at all.
#[tokio::test]
async fn an_already_aborted_refresh_calls_no_provider() {
    // A store that ignores the abort signal, so the provider is stopped only by the engine's own
    // early return and not by the store refusing an aborted read.
    let registry = registry_over(BlindStore::new());
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        None,
        script(|_| Box::pin(async { Ok(()) })),
    );
    register(&registry, provider);
    let req = request(None, Some(true));
    req.cancel.cancel();

    let result = registry.refresh(req).await;

    assert!(result.aborted);
    assert!(result.errors.is_empty());
    // A refresh task the engine had wrongly started would run on the next yields; wait for it so
    // "nothing was called" is not satisfied merely because it had not been polled yet.
    settle(&registry).await;
    assert!(seen.lock().unwrap().is_empty(), "no provider was called");
}

/// pi `models.ts:581-589` + `:605`: aborting a refresh that is mid-network returns promptly with
/// `aborted: true`, records NO error for the provider, and persists nothing — even when the provider
/// answers the abort with an error of its own.
#[tokio::test]
async fn an_abort_during_the_network_phase_is_reported_and_is_not_a_provider_error() {
    let store = Arc::new(InMemoryModelsStore::new());
    let registry = registry_over(store.clone());
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let entered = Arc::new(Notify::new());
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script({
            let entered = Arc::clone(&entered);
            move |ctx| {
                let entered = Arc::clone(&entered);
                Box::pin(async move {
                    if !ctx.allow_network {
                        return Ok(());
                    }
                    entered.notify_one();
                    // The provider honours the signal the way the llama one does (`client.list({ signal })`).
                    ctx.cancel.cancelled().await;
                    Err(ProviderError::Aborted)
                })
            }
        }),
    );
    register(&registry, provider);
    let cancel = CancelToken::new();
    let mut req = request(Some(&[LLAMA]), Some(true));
    req.cancel = cancel.clone();
    let refresh = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move { registry.refresh(req).await }
    });
    entered.notified().await;

    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), refresh)
        .await
        .expect("an aborted refresh returns promptly")
        .unwrap();

    assert!(result.aborted, "{result:?}");
    assert!(
        result.errors.is_empty(),
        "an abort is not a provider failure: {result:?}"
    );
    assert!(store.read(LLAMA, None).await.unwrap().is_none());
}

/// pi `models.ts:584-590`: a provider's failure is recorded under ITS id and nothing is thrown; the
/// other providers refresh normally, in the same call.
#[tokio::test]
async fn a_provider_failure_is_reported_per_provider_and_does_not_stop_the_others() {
    let store = Arc::new(InMemoryModelsStore::new());
    let registry = registry_over(store.clone());
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let (failing, _) = scripted(
        "failing",
        &["x"],
        Some(url_auth()),
        script(|ctx| {
            Box::pin(async move {
                if ctx.allow_network {
                    return Err(ProviderError::ModelSource("server unreachable".into()));
                }
                Ok(())
            })
        }),
    );
    let (healthy, healthy_seen) = scripted(
        "healthy",
        &["y"],
        Some(url_auth()),
        script(|ctx| {
            Box::pin(async move {
                if ctx.allow_network {
                    ctx.publish(ModelsPublication {
                        persist: Some(ModelsPersist::Write {
                            entry: ModelsStoreEntry {
                                models: catalog("healthy", &["y2"]),
                                ..ModelsStoreEntry::default()
                            },
                            classifiers: Vec::new(),
                        }),
                        update: None,
                    })
                    .await?;
                }
                Ok(())
            })
        }),
    );
    register(&registry, failing);
    register(&registry, healthy);

    let result = registry.refresh(request(None, Some(true))).await;

    assert!(!result.aborted);
    assert_eq!(
        result.errors.keys().collect::<Vec<_>>(),
        vec!["failing"],
        "{result:?}"
    );
    assert!(
        result
            .error_for("failing")
            .unwrap()
            .to_string()
            .contains("server unreachable")
    );
    assert_eq!(phases(&healthy_seen), vec![false, true]);
    assert_eq!(
        ids(&store.read("healthy", None).await.unwrap().unwrap().models),
        vec!["y2".to_string()],
        "the healthy provider's refresh published despite its sibling's failure"
    );
    assert!(store.read("failing", None).await.unwrap().is_none());
}

/// A refresh restricted to ids refreshes only those (pi `providers` option, `models.ts:551-555`);
/// an unknown id is ignored rather than an error.
#[tokio::test]
async fn a_provider_selection_restricts_the_refresh_and_ignores_unknown_ids() {
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    let (a, a_seen) = scripted("a", &["a"], None, script(|_| Box::pin(async { Ok(()) })));
    let (b, b_seen) = scripted("b", &["b"], None, script(|_| Box::pin(async { Ok(()) })));
    register(&registry, a);
    register(&registry, b);

    let result = registry
        .refresh(request(Some(&["b", "nope"]), Some(false)))
        .await;

    assert!(result.is_clean(), "{result:?}");
    assert!(a_seen.lock().unwrap().is_empty());
    assert_eq!(phases(&b_seen), vec![false]);
}

/// A static provider (its `refresh_models` answers `None`) is a clean no-op, and gets no network
/// phase — pi's `refreshModels === undefined` filter (`models.ts:552-554`).
#[tokio::test]
async fn a_static_provider_is_left_alone() {
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    registry.upsert_live_provider("static", plain_provider("static", &["s"]));

    let result = registry.refresh(request(None, Some(true))).await;

    assert!(result.is_clean(), "{result:?}");
    assert_eq!(ids(&registry.models()), vec!["s".to_string()]);
}

// -------------------------------------------------------------------------------------------------
// the host verb
// -------------------------------------------------------------------------------------------------

fn live_services(registry: Option<&Arc<GuestProviderRegistry>>) -> LiveHostServices {
    let services = LiveHostServices::new(
        plain_provider("session", &["m"]),
        cyrup_tools::Backend::default().proc,
        std::env::temp_dir(),
    );
    if let Some(registry) = registry {
        services.attach_provider_refresher(Arc::clone(registry) as Arc<dyn ProviderRefresher>);
    }
    services
}

/// `HostServices::refresh_provider` is pi's `modelRegistry.refresh({ providers: [id], allowNetwork,
/// signal })` for one provider: an explicit `true` reaches the network phase under an OFFLINE
/// registry (what `/llama` relies on), `false` stays on the cache, and only the named provider runs.
#[tokio::test]
async fn the_host_verb_overrides_the_offline_switch_only_with_an_explicit_allow_network() {
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    registry.set_network_enabled(false);
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let (llama, llama_seen) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script(|_| Box::pin(async { Ok(()) })),
    );
    let (other, other_seen) = scripted(
        "other",
        &["o"],
        Some(url_auth()),
        script(|_| Box::pin(async { Ok(()) })),
    );
    register(&registry, llama);
    register(&registry, other);
    let services = live_services(Some(&registry));

    let result = services
        .refresh_provider(LLAMA, false, false, CancelToken::new())
        .await;
    assert!(result.is_clean(), "{result:?}");
    assert_eq!(phases(&llama_seen), vec![false]);

    llama_seen.lock().unwrap().clear();
    let result = services
        .refresh_provider(LLAMA, true, false, CancelToken::new())
        .await;
    assert!(result.is_clean(), "{result:?}");
    assert_eq!(
        phases(&llama_seen),
        vec![false, true],
        "allow_network: true beat the offline switch"
    );
    assert!(
        other_seen.lock().unwrap().is_empty(),
        "only the named provider was refreshed"
    );
}

/// EXT-104: the verb carries pi's `force` (`ModelsRefreshOptions.force`, `models.ts:91-98`): a native
/// asking for a forced refresh reaches the provider's context with `force` set, in the network phase
/// only (`models.ts:541`); a refresh that does not ask for it, or asks without the network, does not.
#[tokio::test]
async fn the_host_verb_carries_force_to_the_providers_network_phase() {
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let (llama, seen) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script(|_| Box::pin(async { Ok(()) })),
    );
    register(&registry, llama);
    let services = live_services(Some(&registry));
    let phases = |seen: &Arc<Mutex<Vec<Seen>>>| {
        seen.lock()
            .unwrap()
            .iter()
            .map(|c| (c.allow_network, c.force))
            .collect::<Vec<_>>()
    };

    let result = services
        .refresh_provider(LLAMA, true, true, CancelToken::new())
        .await;
    assert!(result.is_clean(), "{result:?}");
    assert_eq!(phases(&seen), vec![(false, false), (true, true)]);

    seen.lock().unwrap().clear();
    services
        .refresh_provider(LLAMA, true, false, CancelToken::new())
        .await;
    assert!(phases(&seen).iter().all(|(_, force)| !force));

    seen.lock().unwrap().clear();
    services
        .refresh_provider(LLAMA, false, true, CancelToken::new())
        .await;
    assert!(phases(&seen).iter().all(|(_, force)| !force));
}

/// The verb reports abort and the per-provider error in pi's result shape (`index.ts:60-62` reads
/// `result.aborted` and `result.errors.get(id)`).
#[tokio::test]
async fn the_host_verb_reports_abort_and_per_provider_errors() {
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let (llama, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script(|ctx| {
            Box::pin(async move {
                if ctx.allow_network {
                    return Err(ProviderError::ModelSource("catalog fetch failed".into()));
                }
                Ok(())
            })
        }),
    );
    register(&registry, llama);
    let services = live_services(Some(&registry));

    let failed = services
        .refresh_provider(LLAMA, true, false, CancelToken::new())
        .await;
    assert!(!failed.aborted);
    assert!(
        failed
            .error_for(LLAMA)
            .unwrap()
            .to_string()
            .contains("catalog fetch failed")
    );

    let cancel = CancelToken::new();
    cancel.cancel();
    let aborted = services.refresh_provider(LLAMA, true, false, cancel).await;
    assert!(aborted.aborted);
    assert!(aborted.errors.is_empty());
}

/// A host with no refresh engine attached does not pretend the catalog is current: the provider
/// reports an error (and an abort is still an abort).
#[tokio::test]
async fn the_host_verb_without_an_attached_registry_reports_an_error() {
    let services = live_services(None);

    let result = services
        .refresh_provider(LLAMA, true, false, CancelToken::new())
        .await;

    assert!(!result.aborted);
    assert!(result.error_for(LLAMA).is_some(), "{result:?}");
}

// -------------------------------------------------------------------------------------------------
// supersession by removal, the begin() cancel, publication order, OAuth credential resolution
// -------------------------------------------------------------------------------------------------

/// pi `deleteProvider` starts with `supersedeProviderRefresh` (`models.ts:405-407`) exactly as
/// `setProvider` does: removing a provider while its refresh is in flight cancels that refresh, so
/// the removed provider's late publication cannot write its catalog into the store.
///
/// **Red** if `remove_provider` does not supersede the in-flight refresh: the late publication is
/// then applied and persisted.
#[tokio::test]
async fn removing_a_provider_supersedes_its_in_flight_refresh() {
    let store = Arc::new(InMemoryModelsStore::new());
    let registry = registry_over(store.clone());
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<bool>();
    let done = Arc::new(Mutex::new(Some(done_tx)));
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script({
            let (entered, release, done) = (
                Arc::clone(&entered),
                Arc::clone(&release),
                Arc::clone(&done),
            );
            move |ctx| {
                let (entered, release, done) = (
                    Arc::clone(&entered),
                    Arc::clone(&release),
                    Arc::clone(&done),
                );
                Box::pin(async move {
                    if !ctx.allow_network {
                        return Ok(());
                    }
                    entered.notify_one();
                    release.notified().await;
                    let applied = ctx
                        .publish(ModelsPublication {
                            persist: Some(ModelsPersist::Write {
                                entry: ModelsStoreEntry {
                                    models: catalog(LLAMA, &["from-removed-provider"]),
                                    ..ModelsStoreEntry::default()
                                },
                                classifiers: Vec::new(),
                            }),
                            update: None,
                        })
                        .await
                        .unwrap_or(false);
                    if let Some(tx) = done.lock().unwrap().take() {
                        let _ = tx.send(applied);
                    }
                    Ok(())
                })
            }
        }),
    );
    register(&registry, provider);
    let refresh = {
        let registry = Arc::clone(&registry);
        tokio::spawn(async move { registry.refresh(request(Some(&[LLAMA]), Some(true))).await })
    };
    entered.notified().await;

    registry.remove_provider(LLAMA);
    release.notify_one();

    assert!(
        !done_rx.await.unwrap(),
        "the removed provider's publication was refused"
    );
    let result = refresh.await.unwrap();
    assert!(result.errors.is_empty(), "{result:?}");
    settle(&registry).await;
    assert!(
        store.read(LLAMA, None).await.unwrap().is_none(),
        "nothing was persisted"
    );
    assert!(ids(&registry.models()).is_empty(), "the provider is gone");
}

/// pi `beginProviderRefresh` aborts the previous controller (`models.ts:491-496`): a newer refresh of
/// a provider cancels the older one's signal, which is what lets an older refresh stop its own
/// network work and not merely lose its publication to the generation check.
///
/// **Red** if `begin` replaces the previous controller without cancelling it.
#[tokio::test]
async fn a_newer_refresh_cancels_the_older_refreshs_signal() {
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let first_signal: Arc<Mutex<Option<CancelToken>>> = Arc::new(Mutex::new(None));
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script({
            let (entered, release, calls, first_signal) = (
                Arc::clone(&entered),
                Arc::clone(&release),
                Arc::clone(&calls),
                Arc::clone(&first_signal),
            );
            move |ctx| {
                let (entered, release, calls, first_signal) = (
                    Arc::clone(&entered),
                    Arc::clone(&release),
                    Arc::clone(&calls),
                    Arc::clone(&first_signal),
                );
                Box::pin(async move {
                    if !ctx.allow_network {
                        return Ok(());
                    }
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        *first_signal.lock().unwrap() = Some(ctx.cancel.clone());
                        entered.notify_one();
                        release.notified().await;
                    }
                    Ok(())
                })
            }
        }),
    );
    register(&registry, provider);
    let first = {
        let registry = Arc::clone(&registry);
        tokio::spawn(async move { registry.refresh(request(Some(&[LLAMA]), Some(true))).await })
    };
    entered.notified().await;

    let second = registry.refresh(request(Some(&[LLAMA]), Some(true))).await;

    assert!(second.errors.is_empty(), "{second:?}");
    let signal = first_signal
        .lock()
        .unwrap()
        .clone()
        .expect("the first refresh reached its network phase");
    assert!(
        signal.is_cancelled(),
        "starting a newer refresh cancelled the older one's signal"
    );
    release.notify_one();
    let first = first.await.unwrap();
    assert!(
        first.errors.is_empty(),
        "a superseded refresh is a cancellation, not a provider failure: {first:?}"
    );
}

/// A models store whose `write` holds until released and counts how many writes have begun, so a
/// test can see whether a second publication started while the first was still persisting.
struct SerialStore {
    inner: InMemoryModelsStore,
    started: AtomicUsize,
    release: Arc<Notify>,
}

#[async_trait::async_trait]
impl ModelsStore for SerialStore {
    async fn read(
        &self,
        provider_id: &str,
        _options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<Option<ModelsStoreEntry>, ProviderError> {
        self.inner.read(provider_id, None).await
    }
    async fn write(
        &self,
        provider_id: &str,
        entry: ModelsStoreEntry,
        _options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        self.inner.write(provider_id, entry, None).await
    }
    async fn delete(
        &self,
        provider_id: &str,
        _options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.inner.delete(provider_id, None).await
    }
    async fn read_classifier_models(
        &self,
        provider_id: &str,
        _options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<Vec<ClassifierModel>, ProviderError> {
        self.inner.read_classifier_models(provider_id, None).await
    }
    async fn write_classifier_models(
        &self,
        provider_id: &str,
        models: Vec<ClassifierModel>,
        _options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.inner
            .write_classifier_models(provider_id, models, None)
            .await
    }
}

/// pi `publicationChains` (`models.ts:391`, `:504-518`): one provider's publications apply one at a
/// time, in arrival order — the second does not begin persisting until the first has finished, and
/// the later one is what the store ends up holding.
///
/// **Red** if publications are not serialised per provider: both writes start while the first is
/// still held open.
#[tokio::test]
async fn a_providers_publications_persist_one_at_a_time_in_arrival_order() {
    let release = Arc::new(Notify::new());
    let store = Arc::new(SerialStore {
        inner: InMemoryModelsStore::new(),
        started: AtomicUsize::new(0),
        release: Arc::clone(&release),
    });
    let registry = registry_over(store.clone());
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script(|ctx| {
            Box::pin(async move {
                if !ctx.allow_network {
                    return Ok(());
                }
                let publication = |id: &'static str| ModelsPublication {
                    persist: Some(ModelsPersist::Write {
                        entry: ModelsStoreEntry {
                            models: catalog(LLAMA, &[id]),
                            ..ModelsStoreEntry::default()
                        },
                        classifiers: Vec::new(),
                    }),
                    update: None,
                };
                let (first, second) = futures::join!(
                    ctx.publish(publication("first")),
                    ctx.publish(publication("second"))
                );
                assert!(first?, "the first publication applied");
                assert!(second?, "the second publication applied");
                Ok(())
            })
        }),
    );
    register(&registry, provider);
    let refresh = {
        let registry = Arc::clone(&registry);
        tokio::spawn(async move { registry.refresh(request(Some(&[LLAMA]), Some(true))).await })
    };

    // The first write announced itself, by an event rather than by a count of milliseconds.
    tokio::time::timeout(Duration::from_secs(30), async {
        while store.started.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first publication began persisting");
    run_every_runnable_task().await;
    assert_eq!(
        store.started.load(Ordering::SeqCst),
        1,
        "the second publication waits for the first to finish persisting"
    );

    release.notify_one();
    tokio::time::timeout(Duration::from_secs(30), async {
        while store.started.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the second publication began persisting");
    assert_eq!(
        store.started.load(Ordering::SeqCst),
        2,
        "the second publication began once the first was done"
    );
    release.notify_one();

    let result = refresh.await.unwrap();
    assert!(result.errors.is_empty(), "{result:?}");
    let stored = store.read(LLAMA, None).await.unwrap().expect("persisted");
    assert_eq!(ids(&stored.models), vec!["second".to_string()]);
}

/// An OAuth strategy that counts refreshes and always yields a fresh, far-future credential.
struct CountingOauth {
    refreshes: Arc<AtomicUsize>,
}

fn unix_ms(offset_ms: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    i64::try_from(now).unwrap() + offset_ms
}

fn oauth_credential(access: &str, expires: i64) -> Credential {
    Credential::Oauth {
        refresh: "refresh-token".to_string(),
        access: access.to_string(),
        expires,
        ext: serde_json::Map::new(),
    }
}

#[async_trait::async_trait]
impl cyrup_provider::OAuthAuth for CountingOauth {
    fn name(&self) -> &str {
        "counting oauth"
    }
    async fn refresh(&self, _cred: &Credential) -> Result<Credential, AuthError> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        Ok(oauth_credential("refreshed-access", unix_ms(3_600_000)))
    }
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        Ok(ModelAuth {
            api_key: match cred {
                Credential::Oauth { access, .. } => Some(access.clone()),
                Credential::ApiKey { .. } => None,
            },
            ..ModelAuth::default()
        })
    }
}

/// Run one network refresh of `LLAMA` whose provider authenticates with `auth` and whose store
/// holds `stored`; return the credential the network phase was handed, the phases that ran, and
/// the credential store (so a caller can read back what the refresh left in it).
async fn network_credential_for(
    auth: ProviderAuth,
    stored: Credential,
) -> (Option<Credential>, Vec<bool>, Arc<InMemoryCredentialStore>) {
    let credentials = Arc::new(InMemoryCredentialStore::new());
    credentials
        .modify(
            &LLAMA.into(),
            Box::new(move |_| Box::pin(async move { Ok(Some(stored)) })),
        )
        .await
        .unwrap();
    let registry = registry_over(Arc::new(InMemoryModelsStore::new()));
    registry.attach_refresh_auth(
        Arc::clone(&credentials) as Arc<dyn CredentialStore>,
        Arc::new(MapCtx(BTreeMap::new())),
    );
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        Some(auth),
        script(|_| Box::pin(async { Ok(()) })),
    );
    register(&registry, provider);

    let result = registry.refresh(request(Some(&[LLAMA]), Some(true))).await;

    assert!(result.errors.is_empty(), "{result:?}");
    let credential = seen
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.allow_network)
        .and_then(|s| s.credential.clone());
    (credential, phases(&seen), credentials)
}

fn access_of(credential: Option<&Credential>) -> Option<&str> {
    match credential {
        Some(Credential::Oauth { access, .. }) => Some(access.as_str()),
        _ => None,
    }
}

/// pi `resolveRefreshCredential` (`models.ts:608-635`): a stored OAuth credential that has not
/// expired is handed to the network phase as it is, with no refresh.
///
/// **Red** if the expiry test is inverted: a live credential is then refreshed (and replaced).
#[tokio::test]
async fn a_valid_stored_oauth_credential_reaches_the_network_phase_unrefreshed() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let (credential, phases, credentials) = network_credential_for(
        ProviderAuth::with_oauth(Arc::new(CountingOauth {
            refreshes: Arc::clone(&refreshes),
        })),
        oauth_credential("stored-access", unix_ms(3_600_000)),
    )
    .await;

    assert_eq!(phases, vec![false, true]);
    assert_eq!(access_of(credential.as_ref()), Some("stored-access"));
    assert_eq!(refreshes.load(Ordering::SeqCst), 0, "nothing was refreshed");
    let kept = credentials.read(&LLAMA.into()).await.unwrap();
    assert_eq!(
        access_of(kept.as_ref()),
        Some("stored-access"),
        "the store still holds the untouched credential"
    );
}

/// The same resolution for an EXPIRED stored OAuth credential: it is refreshed first (the strategy's
/// `refresh`, under the store's lock) and the network phase is handed the refreshed credential.
///
/// **Red** if the expiry test is inverted: the stale credential reaches the network phase.
#[tokio::test]
async fn an_expired_stored_oauth_credential_is_refreshed_before_the_network_phase() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let (credential, phases, credentials) = network_credential_for(
        ProviderAuth::with_oauth(Arc::new(CountingOauth {
            refreshes: Arc::clone(&refreshes),
        })),
        oauth_credential("stale-access", 1),
    )
    .await;

    assert_eq!(phases, vec![false, true]);
    assert_eq!(access_of(credential.as_ref()), Some("refreshed-access"));
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    let kept = credentials.read(&LLAMA.into()).await.unwrap();
    assert_eq!(
        access_of(kept.as_ref()),
        Some("refreshed-access"),
        "the store holds the refreshed credential, not the stale one"
    );
}

/// A stored OAuth credential for a provider whose auth has no OAuth strategy cannot authenticate the
/// network phase (pi `if (!auth.oauth) return undefined`, `models.ts:614`): the phase is skipped,
/// the cache-only restore still ran.
///
/// **Red** if that guard is removed: the unexpired credential is handed to the network phase anyway.
#[tokio::test]
async fn a_stored_oauth_credential_without_an_oauth_strategy_skips_the_network_phase() {
    let (credential, phases, credentials) = network_credential_for(
        url_auth(),
        oauth_credential("stored-access", unix_ms(3_600_000)),
    )
    .await;

    assert_eq!(phases, vec![false], "only the cache-only restore ran");
    assert!(credential.is_none());
    let kept = credentials.read(&LLAMA.into()).await.unwrap();
    assert_eq!(access_of(kept.as_ref()), Some("stored-access"));
}

// -------------------------------------------------------------------------------------------------
// one store operation for both halves; a dropped refresh
// -------------------------------------------------------------------------------------------------

/// A store that records which mutating operations it is asked for.
struct RecordingStore {
    inner: InMemoryModelsStore,
    calls: Mutex<Vec<&'static str>>,
}

impl RecordingStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: InMemoryModelsStore::new(),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ModelsStore for RecordingStore {
    async fn read(
        &self,
        provider_id: &str,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<Option<ModelsStoreEntry>, ProviderError> {
        self.inner.read(provider_id, options).await
    }
    async fn write(
        &self,
        provider_id: &str,
        entry: ModelsStoreEntry,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.calls.lock().unwrap().push("write");
        self.inner.write(provider_id, entry, options).await
    }
    async fn delete(
        &self,
        provider_id: &str,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.calls.lock().unwrap().push("delete");
        self.inner.delete(provider_id, options).await
    }
    async fn read_classifier_models(
        &self,
        provider_id: &str,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<Vec<ClassifierModel>, ProviderError> {
        self.inner
            .read_classifier_models(provider_id, options)
            .await
    }
    async fn write_classifier_models(
        &self,
        provider_id: &str,
        models: Vec<ClassifierModel>,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.calls.lock().unwrap().push("write_classifier_models");
        self.inner
            .write_classifier_models(provider_id, models, options)
            .await
    }
    async fn write_with_classifiers(
        &self,
        provider_id: &str,
        entry: ModelsStoreEntry,
        classifiers: Vec<ClassifierModel>,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<(), ProviderError> {
        self.calls.lock().unwrap().push("write_with_classifiers");
        self.inner
            .write_with_classifiers(provider_id, entry, classifiers, options)
            .await
    }
}

/// pi persists `{ models: [...refreshed, ...refreshedClassifiers] }` in ONE `write`
/// (`extensions/llama/provider.ts:251-253`). The registry asks its store for ONE operation too, so
/// a crash or a second process can never find the new chat models beside the old classifier
/// models.
///
/// **Red** if the publication goes back to two store calls: the recorded calls are then `write`
/// and `write_classifier_models`.
#[tokio::test]
async fn a_catalog_publication_persists_both_halves_as_one_store_operation() {
    let store = RecordingStore::new();
    let registry = registry_over(store.clone());
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let log = Arc::new(Mutex::new(Vec::new()));
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        publish_catalog(&registry, LLAMA, &["fresh"], &log),
    );
    register(&registry, provider);

    let result = registry.refresh(request(Some(&[LLAMA]), Some(true))).await;

    assert!(result.is_clean(), "{result:?}");
    assert_eq!(store.calls(), vec!["write_with_classifiers"]);
    assert_eq!(
        ids(&store.read(LLAMA, None).await.unwrap().unwrap().models),
        vec!["fresh".to_string()]
    );
    let classifiers = store.read_classifier_models(LLAMA, None).await.unwrap();
    assert_eq!(
        classifiers
            .iter()
            .map(|c| c.id.as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["fresh".to_string()]
    );
}

/// When the caller's refresh future is DROPPED (a task abort, a command future dropped on session
/// teardown) the detached operation is told, through the signal its publication gate reads: it stops
/// its network work, and the publication that would have followed it is refused. Before, nothing
/// ever cancelled it, so it ran to the end and persisted a catalog for a refresh nobody was waiting
/// for.
///
/// **Red** without the drop guard: the operation waits for a cancellation that never comes, and
/// `settle` times out.
#[tokio::test]
async fn dropping_a_refresh_cancels_its_detached_operation_so_nothing_publishes() {
    let store = Arc::new(InMemoryModelsStore::new());
    let registry = registry_over(store.clone());
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let entered = Arc::new(Notify::new());
    let applied: Arc<Mutex<Option<bool>>> = Arc::default();
    let (provider, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        script({
            let entered = Arc::clone(&entered);
            let applied = Arc::clone(&applied);
            move |ctx| {
                let entered = Arc::clone(&entered);
                let applied = Arc::clone(&applied);
                Box::pin(async move {
                    if !ctx.allow_network {
                        return Ok(());
                    }
                    entered.notify_one();
                    // The network request: it runs until it is told to stop.
                    ctx.cancel.cancelled().await;
                    // The publication that would follow a fetch that completed.
                    let published = ctx
                        .publish(ModelsPublication {
                            persist: Some(ModelsPersist::Write {
                                entry: ModelsStoreEntry {
                                    models: catalog(LLAMA, &["stale"]),
                                    ..ModelsStoreEntry::default()
                                },
                                classifiers: Vec::new(),
                            }),
                            update: None,
                        })
                        .await
                        .unwrap_or(false);
                    *applied.lock().unwrap() = Some(published);
                    Ok(())
                })
            }
        }),
    );
    register(&registry, provider);
    let refresh = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move { registry.refresh(request(Some(&[LLAMA]), Some(true))).await }
    });
    entered.notified().await;

    refresh.abort();
    let _ = refresh.await;
    settle(&registry).await;

    assert_eq!(
        *applied.lock().unwrap(),
        Some(false),
        "the operation was cancelled and its publication refused"
    );
    assert!(
        store.read(LLAMA, None).await.unwrap().is_none(),
        "nothing was persisted for a refresh nobody waited for"
    );
}

// -------------------------------------------------------------------------------------------------
// PROV-111: the refresh context is the argument, so it survives a spawned task
// -------------------------------------------------------------------------------------------------

/// What the task a [`Spawning`] provider spawns saw on its clone of the context.
#[derive(Clone, Debug, Default)]
struct SpawnedSaw {
    credential: Option<Credential>,
    stored_ids: Option<Vec<String>>,
    published: Option<bool>,
}

/// A provider that does all its work in a task it `tokio::spawn`s, handing the task a clone of the
/// context — the shape that the task-local `ProviderRefreshContext` could not serve, because a
/// task-local does not cross a spawn.
struct Spawning {
    id: ProviderId,
    inner: Arc<dyn Provider>,
    saw: Arc<Mutex<Vec<SpawnedSaw>>>,
    updates: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Provider for Spawning {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        self.inner.models()
    }
    fn has_refresh_models(&self) -> bool {
        true
    }
    async fn refresh_models(
        &self,
        ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        let ctx = ctx.clone();
        let updates = Arc::clone(&self.updates);
        let saw = tokio::spawn(async move {
            let published = ctx
                .publish(ModelsPublication {
                    persist: None,
                    update: Some(Box::new(move || {
                        updates.fetch_add(1, Ordering::SeqCst);
                    })),
                })
                .await
                .ok();
            SpawnedSaw {
                credential: ctx.credential.clone(),
                stored_ids: ctx
                    .stored
                    .as_ref()
                    .map(|e| e.models.iter().map(|m| m.id.as_str().to_string()).collect()),
                published,
            }
        })
        .await
        .expect("the spawned task ran");
        self.saw.lock().unwrap().push(saw);
        Some(Ok(()))
    }
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream(model, context, options)
    }
}

/// PROV-111: a `refresh_models` that spawns a task still sees its credential, its stored catalog and
/// its publisher. Before the context was the argument these travelled in a tokio task-local, which
/// the spawned task did not inherit: `ProviderRefreshContext::current()` answered `None` there
/// (red run: "the spawned task saw None").
#[tokio::test]
async fn a_refresh_models_that_spawns_a_task_still_sees_its_credential_and_can_publish() {
    let store = Arc::new(InMemoryModelsStore::new());
    store
        .write(
            LLAMA,
            ModelsStoreEntry {
                models: catalog(LLAMA, &["cached"]),
                checked_at: Some(5),
                ..ModelsStoreEntry::default()
            },
            None,
        )
        .await
        .unwrap();
    let registry = registry_over(store);
    let credentials = Arc::new(InMemoryCredentialStore::new());
    credentials.insert(ProviderId::from(LLAMA), Credential::api_key("stored-key"));
    registry.attach_refresh_auth(credentials, Arc::new(MapCtx(BTreeMap::new())));
    let saw = Arc::new(Mutex::new(Vec::new()));
    let updates = Arc::new(AtomicUsize::new(0));
    registry.upsert_live_provider(
        LLAMA,
        Arc::new(Spawning {
            id: LLAMA.into(),
            inner: plain_provider(LLAMA, &["boot"]),
            saw: Arc::clone(&saw),
            updates: Arc::clone(&updates),
        }),
    );

    let result = registry.refresh(request(Some(&[LLAMA]), Some(false))).await;

    assert!(result.is_clean(), "{result:?}");
    let saw = saw.lock().unwrap().clone();
    assert_eq!(saw.len(), 1, "{saw:?}");
    assert!(
        matches!(&saw[0].credential, Some(Credential::ApiKey { key: Some(k), .. }) if k == "stored-key"),
        "the spawned task saw {:?}",
        saw[0].credential
    );
    assert_eq!(saw[0].stored_ids, Some(vec!["cached".to_string()]));
    assert_eq!(
        saw[0].published,
        Some(true),
        "the spawned task's publication was applied"
    );
    assert_eq!(updates.load(Ordering::SeqCst), 1, "and its update ran");
}

// -------------------------------------------------------------------------------------------------
// SEAM-142: a static provider is filtered BEFORE a refresh begins
// -------------------------------------------------------------------------------------------------

/// pi filters `refreshModels === undefined` BEFORE `beginProviderRefresh` (`models.ts:552-559`), so
/// refreshing a static provider's id cannot supersede a refresh of that id that is still running.
///
/// Here the id's provider becomes static in the middle of a refresh: the cache-only phase publishes
/// an `update` that swaps a static provider in under the id (the registry's own replace path, which
/// does not supersede the publishing refresh), and the refresh is then held in its network phase.
/// A refresh request for the id must find the static provider and do nothing — the held refresh
/// still publishes its catalog afterwards. Before `Provider::has_refresh_models` the engine began a
/// refresh first and only then learned the provider answered `None`, which cancelled the held one.
#[tokio::test]
async fn refreshing_a_static_providers_id_leaves_an_in_flight_refresh_running() {
    let store = Arc::new(InMemoryModelsStore::new());
    let registry = registry_over(store.clone());
    registry.attach_refresh_auth(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(MapCtx(BTreeMap::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://127.0.0.1:1".to_string(),
        )]))),
    );
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let log = Arc::new(Mutex::new(Vec::new()));
    let (provider, _) = scripted(LLAMA, &["boot"], Some(url_auth()), {
        let (registry, entered, release, log) = (
            Arc::clone(&registry),
            Arc::clone(&entered),
            Arc::clone(&release),
            Arc::clone(&log),
        );
        script(move |ctx| {
            let (registry, entered, release, log) = (
                Arc::clone(&registry),
                Arc::clone(&entered),
                Arc::clone(&release),
                Arc::clone(&log),
            );
            Box::pin(async move {
                if !ctx.allow_network {
                    // The restore swaps a STATIC provider in under the id.
                    ctx.publish(ModelsPublication {
                        persist: None,
                        update: Some(Box::new(move || {
                            registry
                                .upsert_live_provider(LLAMA, plain_provider(LLAMA, &["swapped"]));
                        })),
                    })
                    .await?;
                    return Ok(());
                }
                entered.notify_one();
                release.notified().await;
                let published = ctx
                    .publish(ModelsPublication {
                        persist: Some(ModelsPersist::Write {
                            entry: ModelsStoreEntry {
                                models: catalog(LLAMA, &["fresh"]),
                                checked_at: Some(7),
                                ..ModelsStoreEntry::default()
                            },
                            classifiers: Vec::new(),
                        }),
                        update: Some(Box::new(move || {
                            log.lock().unwrap().push("update:fresh".to_string());
                        })),
                    })
                    .await?;
                assert!(published, "the held refresh was not superseded");
                Ok(())
            })
        })
    });
    register(&registry, provider);

    let held = {
        let registry = Arc::clone(&registry);
        tokio::spawn(async move { registry.refresh(request(Some(&[LLAMA]), Some(true))).await })
    };
    entered.notified().await;
    assert_eq!(
        ids(&registry.models()),
        vec!["swapped".to_string()],
        "the id now holds the static provider"
    );

    let static_refresh = registry.refresh(request(Some(&[LLAMA]), Some(true))).await;
    assert!(static_refresh.is_clean(), "{static_refresh:?}");

    release.notify_one();
    let held = held.await.unwrap();
    assert!(held.is_clean(), "{held:?}");
    settle(&registry).await;
    assert_eq!(
        ids(&store.read(LLAMA, None).await.unwrap().unwrap().models),
        vec!["fresh".to_string()],
        "the held refresh's network publication landed"
    );
    assert_eq!(*log.lock().unwrap(), vec!["update:fresh".to_string()]);
}

// -------------------------------------------------------------------------------------------------
// SEAM-141: a provider registered after startup restores its cached catalog
// -------------------------------------------------------------------------------------------------

/// A script that does what the llama provider does in its cache-only phase: the stored catalog
/// becomes the live one, through an `update` that swaps a new provider in under the id.
fn restore_stored_catalog(registry: &Arc<GuestProviderRegistry>, id: &'static str) -> Script {
    let registry = Arc::clone(registry);
    script(move |ctx| {
        let registry = Arc::clone(&registry);
        Box::pin(async move {
            let Some(stored) = ctx.stored.clone() else {
                return Ok(());
            };
            let stored_ids = ids(&stored.models);
            ctx.publish(ModelsPublication {
                persist: None,
                update: Some(Box::new(move || {
                    let refs: Vec<&str> = stored_ids.iter().map(String::as_str).collect();
                    registry.upsert_live_provider(id, plain_provider(id, &refs));
                })),
            })
            .await?;
            Ok(())
        })
    })
}

async fn store_with(id: &str, models: &[&str]) -> Arc<InMemoryModelsStore> {
    let store = Arc::new(InMemoryModelsStore::new());
    store
        .write(
            id,
            ModelsStoreEntry {
                models: catalog(id, models),
                checked_at: Some(5),
                ..ModelsStoreEntry::default()
            },
            None,
        )
        .await
        .unwrap();
    store
}

/// pi `registerNativeProvider` / `registerProvider` end in `void this.refresh({ allowNetwork:
/// false })` (`core/model-runtime.ts:893`, `:939`), so a provider registered after startup lists its
/// cached catalog at once. Here: the startup restore has run (over nothing), and then a provider
/// whose id has a stored entry registers. With no refresh requested by anyone, its models are the
/// cached ones, and nothing touched the network.
#[tokio::test]
async fn a_provider_registered_late_with_a_stored_entry_lists_its_cached_models_without_a_refresh()
{
    let registry = registry_over(store_with(LLAMA, &["cached"]).await);
    registry.restore_cached(CancelToken::new()).await;
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        restore_stored_catalog(&registry, LLAMA),
    );

    register(&registry, provider);
    settle(&registry).await;

    assert_eq!(ids(&registry.models()), vec!["cached".to_string()]);
    assert_eq!(
        phases(&seen),
        vec![false],
        "one cache-only phase, no network"
    );
}

/// The caution behind the id restriction: a catalog change IS a re-registration here, so a
/// registration of an id that already has a provider must not restore. A `/llama` `set_catalog`
/// swaps in the live router catalog this way; restoring the stored one over it would be a clobber.
#[tokio::test]
async fn re_registering_an_id_does_not_restore_the_stored_catalog_over_the_new_one() {
    let registry = registry_over(store_with(LLAMA, &["cached"]).await);
    registry.restore_cached(CancelToken::new()).await;
    let (first, _) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        restore_stored_catalog(&registry, LLAMA),
    );
    register(&registry, first);
    settle(&registry).await;
    assert_eq!(ids(&registry.models()), vec!["cached".to_string()]);

    // The provider's own catalog change: a new value under the same id, carrying the live catalog.
    let (live, live_seen) = scripted(
        LLAMA,
        &["live"],
        Some(url_auth()),
        restore_stored_catalog(&registry, LLAMA),
    );
    register(&registry, live);
    settle(&registry).await;

    assert_eq!(ids(&registry.models()), vec!["live".to_string()]);
    assert!(
        phases(&live_seen).is_empty(),
        "no refresh was begun for a re-registration"
    );
}

/// Before the startup restore has begun nothing restores on registration: the startup restore is
/// what covers every provider registered up to then, and it must do so exactly once.
#[tokio::test]
async fn a_provider_registered_before_the_startup_restore_is_restored_once_by_it() {
    let registry = registry_over(store_with(LLAMA, &["cached"]).await);
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        restore_stored_catalog(&registry, LLAMA),
    );

    register(&registry, provider);
    settle(&registry).await;
    assert!(
        phases(&seen).is_empty(),
        "registration alone restores nothing yet"
    );

    registry.restore_cached(CancelToken::new()).await;
    settle(&registry).await;

    assert_eq!(phases(&seen), vec![false]);
    assert_eq!(ids(&registry.models()), vec!["cached".to_string()]);
}

/// A provider that is static is not restored (and begins nothing), and an id removed and
/// registered again is a new id: it restores again.
#[tokio::test]
async fn a_static_late_provider_begins_nothing_and_a_re_registered_removed_id_restores_again() {
    let registry = registry_over(store_with(LLAMA, &["cached"]).await);
    registry.restore_cached(CancelToken::new()).await;

    registry.upsert_live_provider(LLAMA, plain_provider(LLAMA, &["static"]));
    assert_eq!(
        registry.refresh_tasks_in_flight(),
        0,
        "a static provider begins nothing"
    );
    assert_eq!(ids(&registry.models()), vec!["static".to_string()]);

    registry.remove_provider(LLAMA);
    let (provider, seen) = scripted(
        LLAMA,
        &["boot"],
        Some(url_auth()),
        restore_stored_catalog(&registry, LLAMA),
    );
    register(&registry, provider);
    settle(&registry).await;

    assert_eq!(phases(&seen), vec![false]);
    assert_eq!(ids(&registry.models()), vec!["cached".to_string()]);
}

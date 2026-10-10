//! Tests for the `provider` module.
//!
//! Upstream: `packages/coding-agent/test/llama-extension.test.ts` @v0.99.2-17 (the provider cases,
//! numbered below by their `it(...)` title) against `extensions/llama/provider.ts`. The router is
//! the loopback fake of `tests/fake_server.rs`; the chat and classifier endpoints are served by
//! [`InferenceFake`] below, because the router fake does not speak them.
//!
//! pi's `publish` callback of those tests (persist, then run `update`, answer `true`) is
//! [`TestHost`], which is also the [`LlamaRefreshHost`] for the trait-level refresh tests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use cyrup_core::{CancelToken, Content, Message};
use cyrup_provider::auth::oauth::{AuthPromptKind, OAuthError, ScriptedInteraction};
use cyrup_provider::auth::{
    ApiKeyAuth, AuthContext, Credential, CredentialStore, InMemoryCredentialStore, ProviderEnv,
};
use cyrup_provider::collection::AuthType;
use cyrup_provider::stream::StreamOptions;
use cyrup_provider::stream::collect_message;
use cyrup_provider::{
    AnyModel, ClassifierAnswer, ClassifierContext, ClassifierOptions, ClassifierQuestion,
    ClassifierStopReason, Context, CreateModelsOptions, Model, OrderedMap, Provider, ProviderError,
    RefreshModelsContext, create_models,
};

use crate::LLAMA_PROVIDER_ID;
use crate::client::{LlamaModelInfo, llama_inference_url};
use crate::error::LlamaError;
use crate::provider::{
    CatalogEntry, CatalogPublication, CatalogPublisher, DEFAULT_LLAMA_BASE_URL,
    DEFAULT_LLAMA_SERVER_URL, LlamaApiKeyAuth, LlamaController, LlamaControllerOptions,
    LlamaRefreshContext, LlamaRefreshHost, RegisterProviderFn, SetCatalogOptions,
};
use crate::tests::fake_server::{FakeLlamaServer, Reply, model_with, model_with_status};

// --------------------------------------------------------------------------------------- harness --

/// pi's `publish` of the upstream tests: persist, run `update`, answer `true`; plus the stored
/// catalog and credential a [`LlamaRefreshHost`] offers.
#[derive(Default)]
struct TestHost {
    entry: Mutex<Option<CatalogEntry>>,
    credential: Mutex<Option<Credential>>,
    /// How many publications ran their `update`.
    updates: AtomicUsize,
    /// Answer `false` without persisting or updating, as a superseded or aborted refresh does.
    refuse: std::sync::atomic::AtomicBool,
}

impl TestHost {
    fn entry(&self) -> Option<CatalogEntry> {
        lock(&self.entry).clone()
    }

    fn set_credential(&self, credential: Credential) {
        *lock(&self.credential) = Some(credential);
    }

    fn refuse(&self, refuse: bool) {
        self.refuse.store(refuse, Ordering::SeqCst);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[async_trait::async_trait]
impl CatalogPublisher for TestHost {
    async fn publish(&self, publication: CatalogPublication) -> Result<bool, LlamaError> {
        if self.refuse.load(Ordering::SeqCst) {
            return Ok(false);
        }
        if let Some(entry) = publication.persist {
            *lock(&self.entry) = Some(entry);
        }
        if let Some(update) = publication.update {
            update();
            self.updates.fetch_add(1, Ordering::SeqCst);
        }
        Ok(true)
    }
}

#[async_trait::async_trait]
impl LlamaRefreshHost for TestHost {
    async fn credential(&self, _ctx: &RefreshModelsContext) -> Option<Credential> {
        lock(&self.credential).clone()
    }

    async fn stored(&self, _ctx: &RefreshModelsContext) -> Option<CatalogEntry> {
        self.entry()
    }

    async fn publish(
        &self,
        _ctx: &RefreshModelsContext,
        publication: CatalogPublication,
    ) -> Result<bool, LlamaError> {
        CatalogPublisher::publish(self, publication).await
    }
}

/// A controller whose registration callback records every provider it is handed.
struct Harness {
    controller: LlamaController,
    registered: Arc<Mutex<Vec<Arc<dyn Provider>>>>,
}

fn harness(host: Option<Arc<TestHost>>) -> Harness {
    harness_with_store(host, Arc::new(InMemoryCredentialStore::new()))
}

fn harness_with_store(host: Option<Arc<TestHost>>, store: Arc<dyn CredentialStore>) -> Harness {
    let registered: Arc<Mutex<Vec<Arc<dyn Provider>>>> = Arc::default();
    let sink = registered.clone();
    let register: RegisterProviderFn = Arc::new(move |provider| {
        lock(&sink).push(provider);
        Ok(())
    });
    let mut options = LlamaControllerOptions::new(store, register)
        .with_auth_context(Arc::new(MapContext::default()));
    if let Some(host) = host {
        options = options.with_refresh_host(host);
    }
    Harness {
        controller: LlamaController::new(options),
        registered,
    }
}

/// An [`AuthContext`] over a fixed environment.
#[derive(Default, Clone)]
struct MapContext(BTreeMap<String, String>);

impl MapContext {
    fn with(name: &str, value: &str) -> Self {
        Self(BTreeMap::from([(name.to_string(), value.to_string())]))
    }
}

#[async_trait::async_trait]
impl AuthContext for MapContext {
    async fn env(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }

    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

fn credential(url: &str) -> Credential {
    let mut env = ProviderEnv::new();
    env.insert("LLAMA_BASE_URL".to_string(), url.to_string());
    Credential::ApiKey {
        key: Some("local".to_string()),
        env: Some(env),
    }
}

/// One refresh as the upstream tests call it, over the harness's provider.
async fn refresh(
    harness: &Harness,
    host: &TestHost,
    credential: Option<&Credential>,
    stored: Option<&CatalogEntry>,
    allow_network: bool,
) -> Result<(), LlamaError> {
    let cancel = CancelToken::new();
    harness
        .controller
        .provider()
        .refresh(&LlamaRefreshContext {
            credential,
            stored,
            publisher: host,
            allow_network,
            cancel: &cancel,
        })
        .await
}

fn info(value: Value) -> LlamaModelInfo {
    serde_json::from_value(value).unwrap()
}

fn ids(models: &[Model]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

fn persisted(entry: Option<CatalogEntry>) -> Vec<(String, String)> {
    entry
        .unwrap()
        .models
        .iter()
        .map(|model| (model.id().to_string(), model.api().as_str().to_string()))
        .collect()
}

fn pair(id: &str, api: &str) -> (String, String) {
    (id.to_string(), api.to_string())
}

fn context_windows(entry: &Option<CatalogEntry>) -> Vec<u64> {
    entry
        .as_ref()
        .unwrap()
        .models
        .iter()
        .map(|model| match model {
            AnyModel::Chat(model) => model.context_window,
            AnyModel::Classifier(model) => model.context_window,
            // PROV-128 added `AnyModel::Image`; an image row has no `contextWindow`
            // (`BaseModel` has none, types.ts:1097-1108) and llama.cpp lists none.
            AnyModel::Image(_) => 0,
        })
        .collect()
}

fn get_props_queries(server: &FakeLlamaServer) -> Vec<Option<String>> {
    server
        .requests_to("GET", "/props")
        .iter()
        .map(|request| request.query().map(str::to_string))
        .collect()
}

// ------------------------------------------------------------------------------------ set_catalog --

/// Upstream `exposes loaded and sleeping models with router metadata`.
#[test]
fn set_catalog_exposes_loaded_and_sleeping_models_with_router_metadata() {
    let harness = harness(None);
    let catalog = vec![
        info(json!({
            "id": "loaded",
            "status": { "value": "loaded", "args": ["llama-server", "--n-gpu-layers", "999"] },
            "architecture": { "input_modalities": ["text", "image"] },
            "meta": { "n_ctx": 65536, "n_ctx_train": 131072 },
        })),
        info(json!({ "id": "sleeping", "status": { "value": "sleeping" } })),
        info(json!({ "id": "unloaded", "status": { "value": "unloaded" } })),
        info(json!({ "id": "loading", "status": { "value": "loading" } })),
    ];
    harness
        .controller
        .set_catalog(
            &catalog,
            "http://localhost:8080",
            SetCatalogOptions::default(),
        )
        .unwrap();

    let provider = harness.controller.provider();
    let models = provider.models();
    assert_eq!(ids(models), ["loaded", "sleeping"]);
    let loaded = &models[0];
    assert_eq!(loaded.base_url, "http://localhost:8080/v1");
    assert_eq!(loaded.context_window, 65536);
    assert_eq!(loaded.max_tokens, 65536);
    assert_eq!(
        serde_json::to_value(&loaded.input).unwrap(),
        json!(["text", "image"])
    );
    assert_eq!(models[1].base_url, "http://localhost:8080/v1");
    assert_eq!(models[1].context_window, 128_000);
    assert!(!loaded.reasoning, "set_catalog reads no props");
}

/// `getAllModels` (`provider.ts:200`): chat models, then the classifier twins, which sit on the
/// server ROOT (`toPiClassifierModel`, `:91`).
#[test]
fn get_all_models_lists_chat_models_then_classifier_twins_on_the_server_root() {
    let harness = harness(None);
    let catalog = vec![
        info(json!({ "id": "a", "status": { "value": "loaded" }, "meta": { "n_ctx": 4096 } })),
        info(json!({ "id": "b", "status": { "value": "sleeping" } })),
    ];
    harness
        .controller
        .set_catalog(
            &catalog,
            "http://localhost:8080",
            SetCatalogOptions::default(),
        )
        .unwrap();

    let all = harness.controller.provider().get_all_models();
    let summary: Vec<(&str, &str, &str)> = all
        .iter()
        .map(|model| {
            let base_url = match model {
                AnyModel::Chat(model) => model.base_url.as_str(),
                AnyModel::Classifier(model) => model.base_url.as_str(),
                // PROV-128 added `AnyModel::Image`; llama.cpp lists no image rows.
                AnyModel::Image(model) => model.base_url.as_str(),
            };
            (model.id(), model.api().as_str(), base_url)
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("a", "openai-completions", "http://localhost:8080/v1"),
            ("b", "openai-completions", "http://localhost:8080/v1"),
            ("a", "llama-cpp-classify", "http://localhost:8080"),
            ("b", "llama-cpp-classify", "http://localhost:8080"),
        ]
    );
}

/// `setCatalog`'s `routerAutoload` option (`provider.ts:134`, `:147`).
#[test]
fn set_catalog_offers_unloaded_presets_only_with_router_autoload() {
    let catalog = vec![
        info(json!({ "id": "preset", "status": { "value": "unloaded" }, "source": "preset" })),
        info(json!({ "id": "cache", "status": { "value": "unloaded" }, "source": "cache" })),
    ];
    let without = harness(None);
    without
        .controller
        .set_catalog(&catalog, "http://h:1", SetCatalogOptions::default())
        .unwrap();
    assert!(without.controller.provider().models().is_empty());

    let with = harness(None);
    with.controller
        .set_catalog(
            &catalog,
            "http://h:1",
            SetCatalogOptions {
                router_autoload: true,
            },
        )
        .unwrap();
    assert_eq!(ids(with.controller.provider().models()), ["preset"]);
}

/// A catalog change makes a NEW provider and hands it to the host; the old handle keeps the old
/// catalog (`Provider::models` returns a slice, so the catalog is replaced, not mutated).
#[test]
fn set_catalog_registers_a_fresh_provider_each_time() {
    let harness = harness(None);
    let before = harness.controller.provider();
    assert!(before.models().is_empty());
    assert!(
        lock(&harness.registered).is_empty(),
        "construction registers nothing"
    );

    let one = vec![info(
        json!({ "id": "one", "status": { "value": "loaded" } }),
    )];
    harness
        .controller
        .set_catalog(&one, "http://h:1", SetCatalogOptions::default())
        .unwrap();
    let two = vec![info(
        json!({ "id": "two", "status": { "value": "loaded" } }),
    )];
    harness
        .controller
        .set_catalog(&two, "http://h:1", SetCatalogOptions::default())
        .unwrap();

    let registered = lock(&harness.registered);
    assert_eq!(registered.len(), 2);
    assert_eq!(ids(registered[0].models()), ["one"]);
    assert_eq!(ids(registered[1].models()), ["two"]);
    assert_eq!(ids(harness.controller.provider().models()), ["two"]);
    assert!(before.models().is_empty(), "the old handle is not mutated");
    assert_eq!(registered[1].id().as_str(), LLAMA_PROVIDER_ID);
}

#[test]
fn a_bad_server_url_changes_and_registers_nothing() {
    let harness = harness(None);
    let catalog = vec![info(json!({ "id": "m", "status": { "value": "loaded" } }))];
    let error = harness
        .controller
        .set_catalog(&catalog, "file:///tmp/llama", SetCatalogOptions::default())
        .unwrap_err();
    assert!(error.to_string().contains("http or https"), "{error}");
    assert!(harness.controller.provider().models().is_empty());
    assert!(lock(&harness.registered).is_empty());
}

#[test]
fn a_host_that_refuses_the_provider_is_reported_not_swallowed() {
    let register: RegisterProviderFn = Arc::new(|_| Err("registry is closed".to_string()));
    let controller = LlamaController::new(LlamaControllerOptions::new(
        Arc::new(InMemoryCredentialStore::new()),
        register,
    ));
    let catalog = vec![info(json!({ "id": "m", "status": { "value": "loaded" } }))];
    let error = controller
        .set_catalog(&catalog, "http://h:1", SetCatalogOptions::default())
        .unwrap_err();
    assert_eq!(error.to_string(), "registry is closed");
    assert_eq!(
        controller.register_error().as_deref(),
        Some("registry is closed")
    );
}

#[test]
fn the_provider_describes_itself_as_pi_does() {
    let harness = harness(None);
    let provider = harness.controller.provider();
    assert_eq!(provider.id().as_str(), "llama.cpp");
    assert_eq!(provider.name(), "llama.cpp");
    assert_eq!(provider.base_url(), Some("http://127.0.0.1:8080/v1"));
    assert_eq!(DEFAULT_LLAMA_SERVER_URL, "http://127.0.0.1:8080");
    assert_eq!(
        DEFAULT_LLAMA_BASE_URL,
        llama_inference_url(DEFAULT_LLAMA_SERVER_URL).unwrap()
    );
    let auth = provider.provider_auth().unwrap().api_key.as_ref().unwrap();
    assert_eq!(auth.name(), "llama.cpp server");
    assert!(auth.supports_login());
    assert!(auth.supports_check());
}

// ----------------------------------------------------------------------------- refresh: discovery --

/// Upstream `discovers chat-template thinking support for loaded models` (`#9528`): exactly one
/// `GET /props?model=qwen&autoload=false`, and the template turns reasoning on.
#[tokio::test]
async fn refresh_discovers_chat_template_thinking_support_for_loaded_models() {
    let server = FakeLlamaServer::with_models(vec![model_with(
        "qwen",
        "loaded",
        json!({ "meta": { "n_ctx": 32768 } }),
    )])
    .await;
    server.set_props(json!({ "chat_template": "{% if enable_thinking %}think{% endif %}" }));
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));

    refresh(&harness, &host, Some(&credential(server.url())), None, true)
        .await
        .unwrap();

    let queries = get_props_queries(&server);
    assert_eq!(
        queries,
        [Some("model=qwen&autoload=false".to_string())],
        "exactly one /props"
    );
    let provider = harness.controller.provider();
    let [qwen] = provider.models() else {
        panic!("one model: {:?}", ids(provider.models()))
    };
    assert_eq!(qwen.id.as_str(), "qwen");
    assert!(qwen.reasoning);
    let map: Vec<(&str, Option<&str>)> = qwen
        .thinking_level_map
        .as_ref()
        .unwrap()
        .iter()
        .map(|(level, value)| (level.as_str(), value.as_deref()))
        .collect();
    assert_eq!(
        map,
        [
            ("high", None),
            ("low", None),
            ("medium", Some("medium")),
            ("minimal", None),
            ("off", Some("off")),
            ("xhigh", None),
        ]
    );
    assert_eq!(
        serde_json::to_value(&qwen.compat).unwrap()["thinkingFormat"],
        "qwen-chat-template"
    );
    // The replacement reached the host: the last provider it was handed carries the catalog.
    {
        let registered = lock(&harness.registered);
        let last = registered
            .last()
            .expect("the update registers the provider");
        assert_eq!(ids(last.models()), ["qwen"]);
    }
    assert_eq!(qwen.context_window, 32768);
    assert_eq!(qwen.base_url, format!("{}/v1", server.url()));
}

/// A template without `enable_thinking` leaves a loaded model a plain chat model.
#[tokio::test]
async fn refresh_does_not_mark_a_template_without_thinking_as_reasoning() {
    let server = FakeLlamaServer::with_models(vec![model_with("plain", "loaded", json!({}))]).await;
    server.set_props(json!({ "chat_template": "{{ messages }}" }));
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));

    refresh(&harness, &host, Some(&credential(server.url())), None, true)
        .await
        .unwrap();

    let provider = harness.controller.provider();
    assert!(!provider.models()[0].reasoning);
    assert_eq!(provider.models()[0].thinking_level_map, None);
}

/// `provider.ts:240-247`: `/props?model=` is asked for LOADED models only. Sleeping models may wake
/// and unloaded presets would have to be loaded, so they stay unclassified.
#[tokio::test]
async fn refresh_reads_props_only_for_loaded_models() {
    let server = FakeLlamaServer::with_models(vec![
        model_with("loaded", "loaded", json!({ "source": "preset" })),
        model_with("sleeping", "sleeping", json!({ "source": "preset" })),
        model_with("unloaded", "unloaded", json!({ "source": "preset" })),
    ])
    .await;
    server.set_props(json!({ "models_autoload": true, "chat_template": "enable_thinking" }));
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));

    refresh(&harness, &host, Some(&credential(server.url())), None, true)
        .await
        .unwrap();

    let mut queries = get_props_queries(&server);
    queries.sort();
    assert_eq!(
        queries,
        [None, Some("model=loaded&autoload=false".to_string())],
        "the router's own /props once, and the loaded model's"
    );
    let provider = harness.controller.provider();
    let reasoning: Vec<(&str, bool)> = provider
        .models()
        .iter()
        .map(|model| (model.id.as_str(), model.reasoning))
        .collect();
    assert_eq!(
        reasoning,
        [("loaded", true), ("sleeping", false), ("unloaded", false)]
    );
}

/// `Promise.all` over the per-model reads (`provider.ts:237-247`): one failing read fails the
/// refresh and leaves the previous catalog, which `refreshModels` documents ("retain their
/// previous list on failure", `models.ts:178`).
#[tokio::test]
async fn a_failing_props_read_fails_the_refresh_and_keeps_the_previous_catalog() {
    let server = FakeLlamaServer::with_models(vec![model_with("qwen", "loaded", json!({}))]).await;
    server.respond(
        "GET",
        "/props",
        Reply::Json(500, json!({ "error": { "message": "props exploded" } })),
    );
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));
    harness
        .controller
        .set_catalog(
            &[info(
                json!({ "id": "earlier", "status": { "value": "loaded" } }),
            )],
            "http://h:1",
            SetCatalogOptions::default(),
        )
        .unwrap();

    let error = refresh(&harness, &host, Some(&credential(server.url())), None, true)
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "props exploded");
    assert_eq!(ids(harness.controller.provider().models()), ["earlier"]);
    assert_eq!(host.entry(), None, "nothing was persisted");
}

// ------------------------------------------------------------------------ refresh: autoload gating --

/// Upstream `exposes unloaded presets only when router autoload is enabled`: one `GET /props`, the
/// Bearer key on every request, and only the usable preset survives.
#[tokio::test]
async fn refresh_exposes_unloaded_presets_only_when_router_autoload_is_enabled() {
    let server = FakeLlamaServer::with_models(vec![
        model_with(
            "preset",
            "unloaded",
            json!({ "source": "preset", "meta": { "n_ctx": 65536 } }),
        ),
        json!({
            "id": "failed-preset",
            "status": { "value": "unloaded", "failed": true },
            "source": "preset",
        }),
        model_with("cache", "unloaded", json!({ "source": "cache" })),
        model_with("models-dir", "unloaded", json!({ "source": "models_dir" })),
    ])
    .await;
    server.set_props(json!({ "role": "router", "models_autoload": true }));
    server.require_bearer("local");
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));

    refresh(&harness, &host, Some(&credential(server.url())), None, true)
        .await
        .unwrap();

    assert_eq!(
        get_props_queries(&server),
        [None],
        "one /props, the router's own"
    );
    assert_eq!(ids(harness.controller.provider().models()), ["preset"]);
    assert_eq!(
        persisted(host.entry()),
        [
            pair("preset", "openai-completions"),
            pair("preset", "llama-cpp-classify")
        ]
    );
    for request in server.requests() {
        assert_eq!(
            request.header("authorization"),
            Some("Bearer local"),
            "{}",
            request.target
        );
    }
}

/// Upstream `hides unloaded presets when router autoload is disabled`.
#[tokio::test]
async fn refresh_hides_unloaded_presets_when_router_autoload_is_disabled() {
    let server = FakeLlamaServer::with_models(vec![model_with(
        "preset",
        "unloaded",
        json!({ "source": "preset" }),
    )])
    .await;
    server.set_props(json!({ "role": "router", "models_autoload": false }));
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));

    refresh(&harness, &host, Some(&credential(server.url())), None, true)
        .await
        .unwrap();

    assert!(harness.controller.provider().models().is_empty());
    assert_eq!(get_props_queries(&server), [None]);
}

/// `models_autoload` must be the boolean `true` (`provider.ts:52`): anything else hides them.
#[tokio::test]
async fn refresh_requires_models_autoload_to_be_true() {
    for props in [
        json!({}),
        json!({ "models_autoload": "true" }),
        json!({ "models_autoload": 1 }),
    ] {
        let server = FakeLlamaServer::with_models(vec![model_with(
            "preset",
            "unloaded",
            json!({ "source": "preset" }),
        )])
        .await;
        server.set_props(props.clone());
        let host = Arc::new(TestHost::default());
        let harness = harness(Some(host.clone()));
        refresh(&harness, &host, Some(&credential(server.url())), None, true)
            .await
            .unwrap();
        assert!(harness.controller.provider().models().is_empty(), "{props}");
    }
}

/// A failing router `/props` reads as "autoload off" (`provider.ts:53-55`); the refresh still
/// succeeds.
#[tokio::test]
async fn refresh_treats_a_failing_router_props_as_autoload_off() {
    let server = FakeLlamaServer::with_models(vec![
        model_with("preset", "unloaded", json!({ "source": "preset" })),
        model_with("loaded", "loaded", json!({})),
    ])
    .await;
    server.respond_times("GET", "/props", Reply::Json(500, json!({})), 1);
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));

    refresh(&harness, &host, Some(&credential(server.url())), None, true)
        .await
        .unwrap();

    assert_eq!(ids(harness.controller.provider().models()), ["loaded"]);
}

/// `routerAutoloadEnabled` asks only when an unloaded PRESET exists (`provider.ts:50`).
#[tokio::test]
async fn refresh_does_not_ask_the_router_unless_an_unloaded_preset_exists() {
    let server = FakeLlamaServer::with_models(vec![
        model_with("sleeping", "sleeping", json!({ "source": "preset" })),
        model_with("cache", "unloaded", json!({ "source": "cache" })),
    ])
    .await;
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));

    refresh(&harness, &host, Some(&credential(server.url())), None, true)
        .await
        .unwrap();

    assert!(get_props_queries(&server).is_empty(), "no /props at all");
    assert_eq!(ids(harness.controller.provider().models()), ["sleeping"]);
}

// ------------------------------------------------------------------- refresh: persist and restore --

/// Upstream `persists and restores selectable models for cache-only startup refreshes`.
#[tokio::test]
async fn refresh_persists_the_catalog_and_a_cache_only_refresh_restores_it() {
    let server = FakeLlamaServer::with_models(vec![
        model_with("loaded", "loaded", json!({ "meta": { "n_ctx": 32768 } })),
        model_with(
            "sleeping",
            "sleeping",
            json!({ "meta": { "n_ctx": 32768 } }),
        ),
        model_with("unloaded", "unloaded", json!({})),
    ])
    .await;
    server.set_props(json!({}));
    let host = Arc::new(TestHost::default());
    let first = harness(Some(host.clone()));

    refresh(&first, &host, Some(&credential(server.url())), None, true)
        .await
        .unwrap();

    assert_eq!(
        ids(first.controller.provider().models()),
        ["loaded", "sleeping"]
    );
    assert_eq!(
        persisted(host.entry()),
        [
            pair("loaded", "openai-completions"),
            pair("sleeping", "openai-completions"),
            pair("loaded", "llama-cpp-classify"),
            pair("sleeping", "llama-cpp-classify"),
        ]
    );
    assert!(
        host.entry().unwrap().checked_at > 0,
        "stamped with the time of the check"
    );
    let requests_before = server.requests().len();

    let second = harness(Some(host.clone()));
    let stored = host.entry();
    refresh(
        &second,
        &host,
        Some(&credential(server.url())),
        stored.as_ref(),
        false,
    )
    .await
    .unwrap();

    assert_eq!(
        server.requests().len(),
        requests_before,
        "cache-only: no network"
    );
    let provider = second.controller.provider();
    let url = server.url();
    let restored: Vec<(&str, &str, u64)> = provider
        .models()
        .iter()
        .map(|model| {
            (
                model.id.as_str(),
                model.base_url.as_str(),
                model.context_window,
            )
        })
        .collect();
    let v1 = format!("{url}/v1");
    assert_eq!(
        restored,
        [
            ("loaded", v1.as_str(), 32768),
            ("sleeping", v1.as_str(), 32768)
        ]
    );
    let classifiers: Vec<(&str, &str, &str, u64)> = provider
        .classifier_models()
        .iter()
        .map(|model| {
            (
                model.id.as_str(),
                model.api.as_str(),
                model.base_url.as_str(),
                model.context_window,
            )
        })
        .collect();
    assert_eq!(
        classifiers,
        [
            ("loaded", "llama-cpp-classify", url, 32768),
            ("sleeping", "llama-cpp-classify", url, 32768),
        ]
    );
    // `getAllModels` reports the restored classifiers too.
    assert_eq!(
        provider
            .get_all_models()
            .iter()
            .filter(|model| model.as_classifier().is_some())
            .count(),
        2
    );
}

/// Only `llama.cpp` models of the matching api come back from storage (`provider.ts:204-212`).
#[tokio::test]
async fn restore_keeps_only_llama_models_of_the_matching_api() {
    let server = FakeLlamaServer::with_models(vec![model_with("kept", "loaded", json!({}))]).await;
    server.set_props(json!({}));
    let host = Arc::new(TestHost::default());
    let source = harness(Some(host.clone()));
    refresh(&source, &host, Some(&credential(server.url())), None, true)
        .await
        .unwrap();
    let mut entry = host.entry().unwrap();
    let kept_chat = match &entry.models[0] {
        AnyModel::Chat(model) => model.clone(),
        // PROV-128 added `AnyModel::Image`; neither non-chat variant may appear first here.
        AnyModel::Classifier(_) | AnyModel::Image(_) => panic!("chat first"),
    };
    let kept_classifier = entry.models[1].as_classifier().unwrap().clone();

    let mut other_provider = kept_chat.clone();
    other_provider.id = "foreign".into();
    other_provider.provider = "someone-else".into();
    let mut other_api = kept_chat.clone();
    other_api.id = "wrong-api".into();
    other_api.api = "anthropic-messages".into();
    let mut classifier_api = kept_classifier.clone();
    classifier_api.id = "wrong-classifier-api".into();
    classifier_api.api = "something-else".into();
    let mut foreign_classifier = kept_classifier.clone();
    foreign_classifier.id = "foreign-classifier".into();
    foreign_classifier.provider = "someone-else".into();
    entry.models.extend([
        AnyModel::Chat(other_provider),
        AnyModel::Chat(other_api),
        AnyModel::Classifier(classifier_api),
        AnyModel::Classifier(foreign_classifier),
    ]);

    let target = harness(None);
    let sink = TestHost::default();
    refresh(&target, &sink, None, Some(&entry), false)
        .await
        .unwrap();

    let provider = target.controller.provider();
    assert_eq!(ids(provider.models()), ["kept"]);
    assert_eq!(
        provider
            .classifier_models()
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["kept"]
    );
}

/// Upstream `preserves cached llama.cpp context for unloaded autoload presets` (`#10077`/`#10158`):
/// the window a loaded model reported survives its unloading, until a command line pins another.
#[tokio::test]
async fn refresh_preserves_the_cached_context_window_for_unloaded_autoload_presets() {
    let server = FakeLlamaServer::with_models(vec![model_with(
        "qwen",
        "loaded",
        json!({ "source": "preset", "meta": { "n_ctx": 65536, "n_ctx_train": 128000 } }),
    )])
    .await;
    server.set_props(json!({ "role": "router", "models_autoload": true }));
    let host = Arc::new(TestHost::default());
    let cred = credential(server.url());

    let first = harness(Some(host.clone()));
    refresh(&first, &host, Some(&cred), None, true)
        .await
        .unwrap();
    assert_eq!(context_windows(&host.entry()), [65536, 65536]);

    server.set_model(model_with(
        "qwen",
        "unloaded",
        json!({ "source": "preset", "meta": { "n_ctx_train": 128000 } }),
    ));
    let second = harness(Some(host.clone()));
    let stored = host.entry();
    refresh(&second, &host, Some(&cred), stored.as_ref(), true)
        .await
        .unwrap();
    let provider = second.controller.provider();
    assert_eq!(ids(provider.models()), ["qwen"]);
    assert_eq!(provider.models()[0].context_window, 65536);
    assert_eq!(context_windows(&host.entry()), [65536, 65536]);

    server.set_model(model_with_status(
        "qwen",
        "unloaded",
        json!({ "args": ["llama-server", "--ctx-size", "32768"] }),
    ));
    let mut preset = server.models().remove(0);
    preset["source"] = json!("preset");
    preset["meta"] = json!({ "n_ctx_train": 128000 });
    server.set_model(preset);
    let stored = host.entry();
    refresh(&second, &host, Some(&cred), stored.as_ref(), true)
        .await
        .unwrap();
    assert_eq!(context_windows(&host.entry()), [32768, 32768]);
}

// ---------------------------------------------------------------------------- refresh: the gates --

/// `if (!context.allowNetwork || signal.aborted || credential?.type !== "api_key") return`
/// (`provider.ts:228`): nothing leaves the process.
#[tokio::test]
async fn refresh_stays_offline_without_network_permission_or_with_an_aborted_signal() {
    let server = FakeLlamaServer::with_models(vec![model_with("m", "loaded", json!({}))]).await;
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));
    let cred = credential(server.url());

    refresh(&harness, &host, Some(&cred), None, false)
        .await
        .unwrap();

    let cancel = CancelToken::new();
    cancel.cancel();
    harness
        .controller
        .provider()
        .refresh(&LlamaRefreshContext {
            credential: Some(&cred),
            stored: None,
            publisher: host.as_ref(),
            allow_network: true,
            cancel: &cancel,
        })
        .await
        .unwrap();

    assert!(server.requests().is_empty());
    assert_eq!(host.entry(), None);
    assert!(harness.controller.provider().models().is_empty());
}

#[tokio::test]
async fn refresh_stays_offline_for_anything_but_an_api_key_credential() {
    let server = FakeLlamaServer::with_models(vec![model_with("m", "loaded", json!({}))]).await;
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));
    let oauth = Credential::Oauth {
        refresh: "r".to_string(),
        access: "a".to_string(),
        expires: i64::MAX,
        ext: serde_json::Map::new(),
    };

    refresh(&harness, &host, Some(&oauth), None, true)
        .await
        .unwrap();
    refresh(&harness, &host, None, None, true).await.unwrap();

    assert!(server.requests().is_empty());
    assert_eq!(host.entry(), None);
}

/// UPSTREAM QUIRK, replicated: `refreshModels` takes the server URL ONLY from the credential's
/// `env.LLAMA_BASE_URL` (`credentialServerUrl`, `provider.ts:24-27`, `:229-230`), while `check` and
/// `resolve` also accept the `LLAMA_BASE_URL` environment variable (`resolveServerUrl`,
/// `:29-35`). A credential with a key and no URL therefore refreshes nothing, in a setting where
/// the very same credential resolves.
#[tokio::test]
async fn upstream_quirk_refresh_needs_the_credentials_own_url_while_auth_also_reads_the_environment()
 {
    let server = FakeLlamaServer::with_models(vec![model_with("m", "loaded", json!({}))]).await;
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));
    let no_url = Credential::ApiKey {
        key: Some("local".to_string()),
        env: None,
    };
    let blank_url = Credential::ApiKey {
        key: Some("local".to_string()),
        env: Some(ProviderEnv::from([(
            "LLAMA_BASE_URL".to_string(),
            "   ".to_string(),
        )])),
    };

    refresh(&harness, &host, Some(&no_url), None, true)
        .await
        .unwrap();
    refresh(&harness, &host, Some(&blank_url), None, true)
        .await
        .unwrap();

    assert!(
        server.requests().is_empty(),
        "no request without the credential's own URL"
    );
    assert_eq!(host.entry(), None);

    // The same credentials do resolve against an environment that names the server.
    let ctx = MapContext::with("LLAMA_BASE_URL", server.url());
    let auth = LlamaApiKeyAuth::default();
    for cred in [&no_url, &blank_url] {
        let resolved = auth.resolve(&any_model(), &ctx, Some(cred)).await.unwrap();
        assert_eq!(
            resolved.unwrap().auth.base_url.as_deref(),
            Some(format!("{}/v1", server.url()).as_str())
        );
    }
}

/// A publication the host refuses (superseded or aborted) ends the refresh right after the restore
/// (`provider.ts:216-225`).
#[tokio::test]
async fn a_refused_restore_ends_the_refresh_before_any_network_access() {
    let server = FakeLlamaServer::with_models(vec![model_with("m", "loaded", json!({}))]).await;
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));
    host.refuse(true);
    let stored = CatalogEntry {
        models: Vec::new(),
        checked_at: 1,
    };

    refresh(
        &harness,
        &host,
        Some(&credential(server.url())),
        Some(&stored),
        true,
    )
    .await
    .unwrap();

    assert!(server.requests().is_empty());
    assert!(harness.controller.provider().models().is_empty());
}

/// A fetch the caller aborts mid-request fails with the abort and publishes nothing.
#[tokio::test]
async fn an_abort_during_the_catalog_request_fails_the_refresh_and_publishes_nothing() {
    let server = FakeLlamaServer::with_models(vec![model_with("m", "loaded", json!({}))]).await;
    server.respond(
        "GET",
        "/models",
        Reply::After(
            Duration::from_secs(30),
            Box::new(Reply::Json(200, json!({ "data": [] }))),
        ),
    );
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));
    let cred = credential(server.url());
    let cancel = CancelToken::new();
    let aborter = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        aborter.cancel();
    });

    let error = harness
        .controller
        .provider()
        .refresh(&LlamaRefreshContext {
            credential: Some(&cred),
            stored: None,
            publisher: host.as_ref(),
            allow_network: true,
            cancel: &cancel,
        })
        .await
        .unwrap_err();

    assert_eq!(error, LlamaError::Cancelled);
    assert_eq!(host.entry(), None);
}

/// `if (context.signal.aborted) return` after the autoload check (`provider.ts:235`): a signal
/// that fires while the router's `/props` is in flight ends the refresh without publishing, even
/// though the autoload check itself only reads as "off".
#[tokio::test]
async fn an_abort_after_the_catalog_returns_publishes_nothing() {
    let server = FakeLlamaServer::with_models(vec![model_with(
        "preset",
        "unloaded",
        json!({ "source": "preset" }),
    )])
    .await;
    server.respond(
        "GET",
        "/props",
        Reply::After(
            Duration::from_secs(30),
            Box::new(Reply::Json(200, json!({ "models_autoload": true }))),
        ),
    );
    let host = Arc::new(TestHost::default());
    let harness = harness(Some(host.clone()));
    let cred = credential(server.url());
    let cancel = CancelToken::new();
    let aborter = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        aborter.cancel();
    });

    harness
        .controller
        .provider()
        .refresh(&LlamaRefreshContext {
            credential: Some(&cred),
            stored: None,
            publisher: host.as_ref(),
            allow_network: true,
            cancel: &cancel,
        })
        .await
        .unwrap();

    assert_eq!(host.entry(), None, "nothing persisted");
    assert_eq!(host.updates.load(Ordering::SeqCst), 0, "nothing replaced");
}

// ------------------------------------------------------------------- Provider::refresh_models --

/// Without a host there is nowhere to read a credential from or publish to: a static provider.
#[tokio::test]
async fn refresh_models_is_none_without_a_refresh_host() {
    let harness = harness(None);
    let outcome = harness
        .controller
        .provider()
        .refresh_models(&RefreshModelsContext::default())
        .await;
    assert!(outcome.is_none());
}

#[tokio::test]
async fn refresh_models_runs_the_refresh_through_the_host() {
    let server =
        FakeLlamaServer::with_models(vec![model_with("qwen", "sleeping", json!({}))]).await;
    let host = Arc::new(TestHost::default());
    host.set_credential(credential(server.url()));
    let harness = harness(Some(host.clone()));

    let outcome = harness
        .controller
        .provider()
        .refresh_models(&RefreshModelsContext::default())
        .await;

    assert!(matches!(outcome, Some(Ok(()))), "{outcome:?}");
    assert_eq!(ids(harness.controller.provider().models()), ["qwen"]);
    assert_eq!(persisted(host.entry()).len(), 2);
}

/// The host's `allow_network: false` is the cache-only restore.
#[tokio::test]
async fn refresh_models_honours_a_cache_only_context() {
    let server =
        FakeLlamaServer::with_models(vec![model_with("qwen", "sleeping", json!({}))]).await;
    let host = Arc::new(TestHost::default());
    host.set_credential(credential(server.url()));
    let harness = harness(Some(host.clone()));

    let outcome = harness
        .controller
        .provider()
        .refresh_models(&RefreshModelsContext::cache_only())
        .await;

    assert!(matches!(outcome, Some(Ok(()))));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn refresh_models_reports_failures_in_the_hosts_taxonomy() {
    let server = FakeLlamaServer::with_models(Vec::new()).await;
    server.respond(
        "GET",
        "/models",
        Reply::Json(500, json!({ "error": { "message": "boom" } })),
    );
    let host = Arc::new(TestHost::default());
    host.set_credential(credential(server.url()));
    let harness = harness(Some(host.clone()));

    let outcome = harness
        .controller
        .provider()
        .refresh_models(&RefreshModelsContext::default())
        .await
        .unwrap();
    let error = outcome.unwrap_err();
    assert_eq!(error.code(), "model_source");
    assert!(error.to_string().contains("boom"), "{error}");

    // An abort stays an abort.
    server.clear_overrides();
    server.respond(
        "GET",
        "/models",
        Reply::After(
            Duration::from_secs(30),
            Box::new(Reply::Json(200, json!({ "data": [] }))),
        ),
    );
    let ctx = RefreshModelsContext::default();
    let aborter = ctx.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        aborter.cancel();
    });
    let outcome = harness
        .controller
        .provider()
        .refresh_models(&ctx)
        .await
        .unwrap();
    assert!(
        matches!(outcome, Err(ProviderError::Aborted)),
        "{outcome:?}"
    );
}

// ------------------------------------------------------------------------------------ auth ----

fn any_model() -> Model {
    let harness = harness(None);
    harness
        .controller
        .set_catalog(
            &[info(json!({ "id": "m", "status": { "value": "loaded" } }))],
            "http://127.0.0.1:1",
            SetCatalogOptions::default(),
        )
        .unwrap();
    harness.controller.provider().models()[0].clone()
}

fn credential_json(credential: &Credential) -> Value {
    serde_json::to_value(credential).unwrap()
}

/// Upstream `stays dormant until configured and stores URL plus optional key` (first half): with
/// no credential and no `LLAMA_BASE_URL`, `check` and `resolve` answer nothing.
#[tokio::test]
async fn auth_is_dormant_until_a_server_is_configured() {
    let auth = LlamaApiKeyAuth::default();
    let empty = MapContext::default();
    assert_eq!(auth.check(&empty, None).await.unwrap(), None);
    assert!(
        auth.resolve(&any_model(), &empty, None)
            .await
            .unwrap()
            .is_none()
    );
    // A blank variable is no configuration either.
    let blank = MapContext::with("LLAMA_BASE_URL", "   ");
    assert_eq!(auth.check(&blank, None).await.unwrap(), None);
    assert!(
        auth.resolve(&any_model(), &blank, None)
            .await
            .unwrap()
            .is_none()
    );
}

/// Upstream `stays dormant until configured and stores URL plus optional key` (second half): login
/// prompts the URL, then the key, validates with `GET /models` carrying the Bearer key, and stores
/// the URL beside the key.
#[tokio::test]
async fn login_prompts_url_then_key_validates_with_bearer_and_stores_both() {
    let server = FakeLlamaServer::with_models(Vec::new()).await;
    server.require_bearer("secret");
    let auth = LlamaApiKeyAuth::with_process_env(Arc::new(|_| None));
    let interaction =
        ScriptedInteraction::new(vec![Ok(server.url().to_string()), Ok("secret".to_string())]);

    let credential = auth.login(&interaction).await.unwrap();

    assert_eq!(
        credential_json(&credential),
        json!({ "type": "api_key", "key": "secret", "env": { "LLAMA_BASE_URL": server.url() } })
    );
    let prompts = interaction.prompts();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[0].kind, Some(AuthPromptKind::Text));
    assert_eq!(prompts[0].message, "llama.cpp server URL");
    assert_eq!(
        prompts[0].placeholder.as_deref(),
        Some("http://127.0.0.1:8080")
    );
    assert_eq!(prompts[1].kind, Some(AuthPromptKind::Secret));
    assert_eq!(prompts[1].message, "API key (optional)");
    let checks = server.requests_to("GET", "/models");
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].header("authorization"), Some("Bearer secret"));

    // ... and the stored credential resolves (upstream's last assertion).
    let ctx = MapContext::default();
    let resolved = auth
        .resolve(&any_model(), &ctx, Some(&credential))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.auth.api_key.as_deref(), Some("secret"));
    assert_eq!(resolved.auth.base_url, Some(format!("{}/v1", server.url())));
    assert_eq!(
        resolved.env,
        Some(ProviderEnv::from([(
            "LLAMA_BASE_URL".to_string(),
            server.url().to_string()
        )]))
    );
    assert_eq!(resolved.source.as_deref(), Some("stored credential"));
}

/// The key is optional: blank stores no key and sends no `Authorization` (`apiKey || undefined`,
/// `provider.ts:176`, `:174`).
#[tokio::test]
async fn login_with_a_blank_key_stores_none_and_sends_no_authorization() {
    let server = FakeLlamaServer::with_models(Vec::new()).await;
    let auth = LlamaApiKeyAuth::with_process_env(Arc::new(|_| None));
    let interaction = ScriptedInteraction::new(vec![
        Ok(format!("{}/v1/", server.url())),
        Ok("   ".to_string()),
    ]);

    let credential = auth.login(&interaction).await.unwrap();

    assert_eq!(
        credential_json(&credential),
        json!({ "type": "api_key", "env": { "LLAMA_BASE_URL": server.url() } }),
        "the URL is stored normalised, and no key at all"
    );
    assert_eq!(
        server.requests_to("GET", "/models")[0].header("authorization"),
        None
    );
}

/// A blank URL answer falls back to `LLAMA_BASE_URL` (`provider.ts:165-167`), which is also the
/// prompt's placeholder (`:163`).
#[tokio::test]
async fn login_falls_back_to_the_environment_url_and_shows_it_as_the_placeholder() {
    let server = FakeLlamaServer::with_models(Vec::new()).await;
    let url = server.url().to_string();
    let auth = LlamaApiKeyAuth::with_process_env(Arc::new(move |name| {
        (name == "LLAMA_BASE_URL").then(|| url.clone())
    }));
    let interaction = ScriptedInteraction::new(vec![Ok(String::new()), Ok(String::new())]);

    let credential = auth.login(&interaction).await.unwrap();

    assert_eq!(
        credential_json(&credential)["env"]["LLAMA_BASE_URL"],
        json!(server.url())
    );
    assert_eq!(
        interaction.prompts()[0].placeholder.as_deref(),
        Some(server.url())
    );
}

/// A server that rejects the key fails the login with the server's message, and a URL that is not
/// http(s) fails before the key is even asked for.
#[tokio::test]
async fn login_fails_when_the_server_refuses_or_the_url_is_unusable() {
    let server = FakeLlamaServer::with_models(Vec::new()).await;
    server.require_bearer("right");
    let auth = LlamaApiKeyAuth::with_process_env(Arc::new(|_| None));
    let interaction =
        ScriptedInteraction::new(vec![Ok(server.url().to_string()), Ok("wrong".to_string())]);
    let error = auth.login(&interaction).await.unwrap_err();
    assert!(
        matches!(&error, OAuthError::Failed(message) if message == "Invalid API Key"),
        "{error:?}"
    );

    let interaction = ScriptedInteraction::new(vec![Ok("ftp://example.com".to_string())]);
    let error = auth.login(&interaction).await.unwrap_err();
    assert!(
        matches!(&error, OAuthError::Failed(message) if message.contains("http or https")),
        "{error:?}"
    );
    assert_eq!(interaction.prompts().len(), 1, "the key is not asked for");
}

#[tokio::test]
async fn login_stops_when_a_prompt_is_cancelled() {
    let auth = LlamaApiKeyAuth::with_process_env(Arc::new(|_| None));
    let interaction = ScriptedInteraction::new(vec![Err(OAuthError::Cancelled)]);
    assert!(matches!(
        auth.login(&interaction).await,
        Err(OAuthError::Cancelled)
    ));
}

/// `check` (`provider.ts:181-186`): active with a URL, labelled by where it came from.
#[tokio::test]
async fn check_labels_the_source_of_the_configuration() {
    let auth = LlamaApiKeyAuth::default();
    let stored = credential("http://127.0.0.1:9");
    let check = auth
        .check(&MapContext::default(), Some(&stored))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(check.auth_type, AuthType::ApiKey);
    assert_eq!(check.source.as_deref(), Some("stored credential"));

    let ambient = MapContext::with("LLAMA_BASE_URL", "http://127.0.0.1:9");
    let check = auth.check(&ambient, None).await.unwrap().unwrap();
    assert_eq!(check.auth_type, AuthType::ApiKey);
    assert_eq!(check.source.as_deref(), Some("LLAMA_BASE_URL"));
}

/// `resolve` (`provider.ts:187-196`): key from the credential, else `LLAMA_API_KEY`, else
/// `local`; base URL `<server>/v1`; env overlay is the credential's plus the normalised URL.
#[tokio::test]
async fn resolve_picks_the_key_from_the_credential_then_the_environment_then_local() {
    let auth = LlamaApiKeyAuth::default();
    let model = any_model();
    let with_key = Credential::ApiKey {
        key: Some("stored".to_string()),
        env: Some(ProviderEnv::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://h:1".to_string(),
        )])),
    };
    let no_key = Credential::ApiKey {
        key: None,
        env: with_key.env().cloned(),
    };
    let ambient_key = MapContext(BTreeMap::from([(
        "LLAMA_API_KEY".to_string(),
        "from-env".to_string(),
    )]));
    let none = MapContext::default();

    let key =
        |resolved: Option<cyrup_provider::AuthResult>| resolved.unwrap().auth.api_key.unwrap();
    assert_eq!(
        key(auth
            .resolve(&model, &ambient_key, Some(&with_key))
            .await
            .unwrap()),
        "stored"
    );
    assert_eq!(
        key(auth
            .resolve(&model, &ambient_key, Some(&no_key))
            .await
            .unwrap()),
        "from-env"
    );
    assert_eq!(
        key(auth.resolve(&model, &none, Some(&no_key)).await.unwrap()),
        "local"
    );
}

#[tokio::test]
async fn resolve_builds_the_base_url_and_env_overlay_from_the_normalised_server() {
    let auth = LlamaApiKeyAuth::default();
    let cred = Credential::ApiKey {
        key: Some("k".to_string()),
        env: Some(ProviderEnv::from([
            ("LLAMA_BASE_URL".to_string(), "http://h:1/v1/".to_string()),
            ("OTHER".to_string(), "kept".to_string()),
        ])),
    };

    let resolved = auth
        .resolve(&any_model(), &MapContext::default(), Some(&cred))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(resolved.auth.base_url.as_deref(), Some("http://h:1/v1"));
    assert_eq!(resolved.auth.headers, None);
    assert_eq!(
        resolved.env,
        Some(ProviderEnv::from([
            ("LLAMA_BASE_URL".to_string(), "http://h:1".to_string()),
            ("OTHER".to_string(), "kept".to_string()),
        ]))
    );
}

/// With no credential the environment's URL and `LLAMA_API_KEY` configure the provider, and the
/// source says so.
#[tokio::test]
async fn resolve_from_the_environment_alone() {
    let auth = LlamaApiKeyAuth::default();
    let ctx = MapContext(BTreeMap::from([
        (
            "LLAMA_BASE_URL".to_string(),
            "  http://h:2/v1  ".to_string(),
        ),
        ("LLAMA_API_KEY".to_string(), "env-key".to_string()),
    ]));

    let resolved = auth
        .resolve(&any_model(), &ctx, None)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(resolved.auth.api_key.as_deref(), Some("env-key"));
    assert_eq!(resolved.auth.base_url.as_deref(), Some("http://h:2/v1"));
    assert_eq!(
        resolved.env,
        Some(ProviderEnv::from([(
            "LLAMA_BASE_URL".to_string(),
            "http://h:2".to_string()
        )]))
    );
    assert_eq!(resolved.source.as_deref(), Some("LLAMA_BASE_URL"));
}

/// A server URL that is not http(s) is an auth error, not a silent "unconfigured".
#[tokio::test]
async fn an_unusable_server_url_is_an_auth_error() {
    let auth = LlamaApiKeyAuth::default();
    let ctx = MapContext::with("LLAMA_BASE_URL", "file:///tmp/llama");
    assert_eq!(auth.check(&ctx, None).await.unwrap_err().code(), "auth");
    assert_eq!(
        auth.resolve(&any_model(), &ctx, None)
            .await
            .unwrap_err()
            .code(),
        "auth"
    );
}

// ------------------------------------------------------------------------ inference endpoints --

/// What the inference fake saw.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A loopback server for the endpoints the router fake does not speak: `llama-server`'s
/// `/tokenize`, `/apply-template` and `/completion` (what `llama-cpp-classify` calls) and the
/// OpenAI-compatible `/v1/chat/completions`. It is upstream's `classifies with selectable models
/// through llama-server` server: the tokenizer maps a character to its code point and the
/// completion ranks `B` over `A`.
struct InferenceFake {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl InferenceFake {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(Self::serve(socket, log.clone()));
            }
        });
        Self { url, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        lock(&self.seen).clone()
    }

    fn paths(&self) -> Vec<String> {
        self.seen().into_iter().map(|seen| seen.path).collect()
    }

    async fn serve(mut socket: TcpStream, log: Arc<Mutex<Vec<Seen>>>) {
        let mut data: Vec<u8> = Vec::new();
        let head_end = loop {
            if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                break position;
            }
            let mut chunk = [0_u8; 4096];
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(read) => data.extend_from_slice(&chunk[..read]),
            }
        };
        let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
        let mut lines = head.split("\r\n");
        let mut request_line = lines.next().unwrap_or_default().split(' ');
        let method = request_line.next().unwrap_or_default().to_string();
        let path = request_line.next().unwrap_or_default().to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        let length = headers
            .iter()
            .find(|(name, _)| name == "content-length")
            .and_then(|(_, value)| value.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = data[head_end + 4..].to_vec();
        while body.len() < length {
            let mut chunk = [0_u8; 4096];
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(read) => body.extend_from_slice(&chunk[..read]),
            }
        }
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        lock(&log).push(Seen {
            method,
            path: path.clone(),
            headers,
            body: body.clone(),
        });

        let (content_type, reply) = match path.as_str() {
            "/tokenize" => {
                let content = body["content"].as_str().unwrap_or_default();
                let tokens: Vec<u32> = content.chars().map(u32::from).collect();
                ("application/json", json!({ "tokens": tokens }).to_string())
            }
            "/apply-template" => (
                "application/json",
                json!({ "prompt": "<|im_start|>assistant\n" }).to_string(),
            ),
            "/completion" => (
                "application/json",
                json!({
                    "completion_probabilities": [{
                        "top_logprobs": [
                            { "id": 66, "token": "B", "logprob": -0.1 },
                            { "id": 65, "token": "A", "logprob": -2.4 },
                        ],
                    }],
                })
                .to_string(),
            ),
            "/v1/chat/completions" => {
                let chunk = |delta: Value, finish: Value| {
                    format!(
                        "data: {}\n\n",
                        json!({
                            "id": "chatcmpl-1",
                            "object": "chat.completion.chunk",
                            "created": 1,
                            "model": "qwen",
                            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
                        })
                    )
                };
                let stream = format!(
                    "{}{}data: [DONE]\n\n",
                    chunk(
                        json!({ "role": "assistant", "content": "hello from llama" }),
                        Value::Null
                    ),
                    chunk(json!({}), json!("stop")),
                );
                ("text/event-stream", stream)
            }
            _ => (
                "application/json",
                json!({ "error": { "message": "not found" } }).to_string(),
            ),
        };
        let status = if reply.contains("not found") {
            "404 Not Found"
        } else {
            "200 OK"
        };
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.shutdown().await;
    }
}

fn user_context(text: &str) -> Context {
    Context {
        system_prompt: None,
        messages: vec![Message::User {
            content: vec![Content::Text {
                text: text.into(),
                text_signature: None,
            }],
            timestamp: 0,
        }],
        tools: Vec::new(),
    }
}

/// Upstream `classifies with selectable models through llama-server`: the classifier twin answers
/// through `/tokenize`, `/apply-template` and `/completion` of the SERVER ROOT, with the model id
/// in every request.
#[tokio::test]
async fn the_classifier_twin_classifies_through_llama_server() {
    let fake = InferenceFake::start().await;
    let harness = harness(None);
    harness
        .controller
        .set_catalog(
            &[info(
                json!({ "id": "qwen", "status": { "value": "loaded" } }),
            )],
            &fake.url,
            SetCatalogOptions::default(),
        )
        .unwrap();
    let provider = harness.controller.provider();
    let mut classifier = provider
        .get_all_models()
        .into_iter()
        .find_map(AnyModel::into_classifier)
        .expect("a classifier model");
    assert_eq!(
        classifier.base_url, fake.url,
        "the twin is built on the server root"
    );
    // Provider auth resolves the OpenAI-compatible /v1 URL, which replaces the model's base URL.
    classifier.base_url = format!("{}/v1", fake.url);

    let context = ClassifierContext {
        images: None,
        state: serde_json::Map::from_iter([(
            "message".to_string(),
            json!("The build is red again."),
        )]),
        questions: OrderedMap::from_iter([(
            "kind",
            ClassifierQuestion::Choice {
                instructions: "What is this about?".to_string(),
                criteria: OrderedMap::from_iter([
                    ("billing", String::new()),
                    ("ci", String::new()),
                ]),
            },
        )]),
    };
    let result = provider
        .classify(
            &classifier,
            &context,
            &ClassifierOptions {
                api_key: Some("local".to_string()),
                ..ClassifierOptions::default()
            },
        )
        .await;

    assert_eq!(result.error_message, None);
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
    assert!(
        matches!(result.answers.get("kind"), Some(ClassifierAnswer::Choice { choice, .. }) if choice == "ci"),
        "{:?}",
        result.answers
    );
    let paths = fake.paths();
    for expected in ["/tokenize", "/apply-template", "/completion"] {
        assert!(
            paths.iter().any(|path| path == expected),
            "{expected} in {paths:?}"
        );
    }
    for seen in fake.seen() {
        assert_eq!(seen.body["model"], "qwen", "{}", seen.path);
        assert_eq!(seen.method, "POST");
    }
}

/// Streaming goes through the `openai-completions` api against `<server>/v1`: the wire body honours
/// the model's compat (`max_tokens`, no `store`) and the Bearer key.
#[tokio::test]
async fn chat_streams_through_the_openai_completions_api_with_the_models_compat() {
    let fake = InferenceFake::start().await;
    let harness = harness(None);
    harness
        .controller
        .set_catalog(
            &[info(
                json!({ "id": "qwen", "status": { "value": "loaded" }, "meta": { "n_ctx": 8192 } }),
            )],
            &fake.url,
            SetCatalogOptions::default(),
        )
        .unwrap();
    let provider = harness.controller.provider();
    let model = provider.models()[0].clone();
    // What `Models::stream` hands a provider after resolving auth: the key and the env overlay.
    let options = StreamOptions {
        api_key: Some("local".to_string()),
        env: Some(ProviderEnv::from([(
            "LLAMA_BASE_URL".to_string(),
            fake.url.clone(),
        )])),
        max_tokens: Some(100),
        ..StreamOptions::default()
    };

    let message = collect_message(provider.stream(&model, &user_context("hi"), &options)).await;

    let text = message.content.iter().find_map(|content| match content {
        Content::Text { text, .. } => Some(text.to_string()),
        _ => None,
    });
    assert_eq!(text.as_deref(), Some("hello from llama"), "{message:?}");
    let requests: Vec<Seen> = fake
        .seen()
        .into_iter()
        .filter(|seen| seen.path == "/v1/chat/completions")
        .collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("authorization"), Some("Bearer local"));
    let body = &requests[0].body;
    assert_eq!(body["model"], "qwen");
    assert_eq!(body["max_tokens"], 100, "{body}");
    assert_eq!(body.get("max_completion_tokens"), None, "{body}");
    assert_eq!(body.get("store"), None, "{body}");
}

/// The path a session takes: the agent streams through [`Provider::stream`] with no options of
/// its own, and the provider resolves the STORED credential (`<server>/v1`, its key) from the
/// credential store it was built with. The request lands on that server, not on the model's own
/// base URL.
#[tokio::test]
async fn a_stored_credential_routes_the_stream_to_its_server() {
    let fake = InferenceFake::start().await;
    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::new().with_credential(
        LLAMA_PROVIDER_ID.into(),
        Credential::ApiKey {
            key: Some("secret".to_string()),
            env: Some(ProviderEnv::from([(
                "LLAMA_BASE_URL".to_string(),
                fake.url.clone(),
            )])),
        },
    ));
    let harness = harness_with_store(None, store);
    harness
        .controller
        .set_catalog(
            &[info(
                json!({ "id": "qwen", "status": { "value": "loaded" } }),
            )],
            "http://127.0.0.1:1",
            SetCatalogOptions::default(),
        )
        .unwrap();
    let provider = harness.controller.provider();
    let model = provider.models()[0].clone();
    assert_eq!(
        model.base_url, "http://127.0.0.1:1/v1",
        "the model points nowhere useful"
    );

    let message =
        collect_message(provider.stream(&model, &user_context("hi"), &StreamOptions::default()))
            .await;

    let text = message.content.iter().find_map(|content| match content {
        Content::Text { text, .. } => Some(text.to_string()),
        _ => None,
    });
    assert_eq!(text.as_deref(), Some("hello from llama"), "{message:?}");
    let requests: Vec<Seen> = fake
        .seen()
        .into_iter()
        .filter(|seen| seen.path == "/v1/chat/completions")
        .collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("authorization"), Some("Bearer secret"));
}

/// Before any server is configured the provider is dormant: `Models` finds no auth and the stream
/// fails as "not configured" instead of calling the default address.
#[tokio::test]
async fn an_unconfigured_provider_does_not_stream() {
    let harness = harness(None);
    harness
        .controller
        .set_catalog(
            &[info(
                json!({ "id": "qwen", "status": { "value": "loaded" } }),
            )],
            "http://127.0.0.1:1",
            SetCatalogOptions::default(),
        )
        .unwrap();
    let provider = harness.controller.provider();
    let mut models = create_models(CreateModelsOptions {
        credentials: Some(Arc::new(InMemoryCredentialStore::new())),
        auth_context: Some(Arc::new(MapContext::default())),
        catalog_overlay: None,
    });
    models.set_provider(provider);

    let check = models.check_auth(LLAMA_PROVIDER_ID).await.unwrap();
    assert_eq!(
        check, None,
        "check_auth reports nothing until a server is configured"
    );
}

// ---------------------------------------------------------------------- install atomicity --

/// `current` and the provider the host registered are two copies of one fact, so two installs (a
/// `/llama` `set_catalog` and a refresh's `update`) must not interleave: the second may not even
/// reach the host until the first has handed over its provider. Upstream reassigns one pair of
/// closure arrays (`provider.ts:147-149`, `:254-257`), so last-write-wins is always consistent.
#[test]
fn two_installs_never_interleave_between_the_current_provider_and_the_host() {
    let events: Arc<Mutex<Vec<String>>> = Arc::default();
    let (inside_tx, inside_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let first = Arc::new(AtomicUsize::new(0));
    let register: RegisterProviderFn = {
        let events = events.clone();
        Arc::new(move |provider| {
            let ids: Vec<String> = provider
                .models()
                .iter()
                .map(|model| model.id.to_string())
                .collect();
            let call = first.fetch_add(1, Ordering::SeqCst);
            lock(&events).push(format!("enter {ids:?}"));
            if call == 0 {
                // The first install parks inside the host until the test lets it go.
                inside_tx.send(()).unwrap();
                let _ = lock(&release_rx).recv_timeout(Duration::from_secs(10));
            }
            lock(&events).push(format!("exit {ids:?}"));
            Ok(())
        })
    };
    let controller = LlamaController::new(LlamaControllerOptions::new(
        Arc::new(InMemoryCredentialStore::new()),
        register,
    ));
    let catalog = |id: &str| vec![info(json!({ "id": id, "status": { "value": "loaded" } }))];

    let one = {
        let controller = controller.clone();
        let catalog = catalog("a");
        std::thread::spawn(move || {
            controller
                .set_catalog(&catalog, "http://h:1", SetCatalogOptions::default())
                .unwrap();
        })
    };
    inside_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let two = {
        let controller = controller.clone();
        let catalog = catalog("b");
        std::thread::spawn(move || {
            controller
                .set_catalog(&catalog, "http://h:1", SetCatalogOptions::default())
                .unwrap();
        })
    };
    // Give the second install every chance to run ahead; a correct controller keeps it waiting.
    std::thread::sleep(Duration::from_millis(150));
    let ids = |controller: &LlamaController| -> Vec<String> {
        controller
            .provider()
            .models()
            .iter()
            .map(|model| model.id.to_string())
            .collect()
    };
    assert_eq!(
        ids(&controller),
        ["a"],
        "the second install must not replace the current provider while the first is still being \
         handed to the host"
    );
    release_tx.send(()).unwrap();
    one.join().unwrap();
    two.join().unwrap();

    assert_eq!(
        *lock(&events),
        [
            r#"enter ["a"]"#,
            r#"exit ["a"]"#,
            r#"enter ["b"]"#,
            r#"exit ["b"]"#
        ]
    );
    assert_eq!(
        ids(&controller),
        ["b"],
        "the host's last provider is the current one"
    );
}

// ------------------------------------------------------------------------ classification --

/// pi attaches `classify` to the llama provider unconditionally (`provider.ts:262`), so the host's
/// pre-auth support check passes even while the catalog lists no classifier twin yet.
#[test]
fn the_llama_provider_supports_classification_without_any_classifier_model() {
    let harness = harness(None);
    let provider = harness.controller.provider();
    assert!(provider.models().is_empty());
    assert!(provider.classifier_models().is_empty());
    assert!(provider.supports_classification());
}

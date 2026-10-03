//! Tests for the `extension` module (`index.ts`), over the loopback router fake in
//! [`super::fake_server`], a loopback Hugging Face fake defined here, and a fake [`HostServices`].
//!
//! Upstream tests only the registration of `index.ts` ("registers a native provider and /llama
//! command", `llama-extension.test.ts:42-54`); everything else here pins behaviour upstream
//! implements and does not test. The command flow ([`Flow`]) is driven two ways: against a scripted
//! [`LlamaUi`] that records every dialog it is asked for (so a title or an option list is asserted
//! exactly), and, for the wiring, through the real overlay with keys fed by a fake host and the
//! command routed by a real [`ExtensionHost`].
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cyrup_core::{CancelToken, ExtensionId};
use cyrup_ext::host::services::HostProviderAuth;
use cyrup_ext::registry::CommandDescriptor;
use cyrup_ext::{
    ExtError, ExtMode, ExtensionHost, HostConfig, HostCtx, HostServices, InitApi,
    InteractiveOverlay, LateRegistrar, ModelRegistrySink, NativeExtension, NotifyKind, OverlayKey,
    OverlayKeyCode, ProviderRegistration,
};
use cyrup_provider::auth::InMemoryCredentialStore;
use cyrup_provider::{ModelsRefreshResult, Provider, ProviderError};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::fake_server::{FakeLlamaServer, Reply, Step, event, model, model_with_status};
use crate::LLAMA_PROVIDER_ID;
use crate::client::{LlamaClient, LlamaModelInfo};
use crate::error::LlamaError;
use crate::extension::{
    Endpoints, Flow, FlowError, LlamaExtension, connection_error_message, is_connection_error,
    model_is_loaded, parse_huggingface_model,
};
use crate::huggingface::HuggingFaceModel;
use crate::llama_extension_for_env;
use crate::provider::{LlamaController, LlamaControllerOptions};
use crate::ui::{LlamaManagerAction, LlamaUi, ProgressState, SearchFn};

// =================================================================================================
// Fixtures
// =================================================================================================

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn info(id: &str, status: &str) -> LlamaModelInfo {
    serde_json::from_value(model(id, status)).unwrap()
}

fn press(code: OverlayKeyCode) -> OverlayKey {
    OverlayKey::plain(code)
}

/// Reads the `"METHOD /path"` of every request the llama server has received.
type RequestProbe = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

/// What a Hugging Face request looked like to the fake: target and lower-cased headers.
type HfRequest = (String, BTreeMap<String, String>);

/// The model list the UI was shown: server URL and `(id, status)` rows.
type ShownList = (String, Vec<(String, String)>);

/// What the host was told to show, and what the server had been asked by then.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Notice {
    message: String,
    kind: NotifyKind,
    /// `"METHOD /path"` of every request the llama server had received when this was shown.
    requests_seen: Vec<String>,
}

/// What the host's refresh does for `llama.cpp`.
#[derive(Clone)]
enum RefreshBehavior {
    Clean,
    /// Answers `aborted` at once.
    Aborted,
    /// Waits for the caller's signal, then answers `aborted`.
    WaitForCancel,
    /// Records this error for the provider.
    Fail(LlamaError),
}

/// One `refresh_provider` call.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RefreshCall {
    provider: String,
    allow_network: bool,
    /// The model ids of the provider registered at that moment.
    catalog: Vec<String>,
}

struct FakeHost {
    notices: Mutex<Vec<Notice>>,
    requests: Mutex<Option<RequestProbe>>,
    auth: Mutex<Result<Option<HostProviderAuth>, String>>,
    auth_calls: AtomicUsize,
    behavior: Mutex<RefreshBehavior>,
    refreshes: Mutex<Vec<RefreshCall>>,
    registered: Arc<Mutex<Vec<Arc<dyn Provider>>>>,
    accept_overlay: bool,
    script: Mutex<VecDeque<(String, Vec<OverlayKey>)>>,
    frames: Mutex<Vec<Vec<String>>>,
    opened: AtomicBool,
}

impl FakeHost {
    fn new(registered: Arc<Mutex<Vec<Arc<dyn Provider>>>>) -> Arc<Self> {
        Arc::new(Self {
            notices: Mutex::new(Vec::new()),
            requests: Mutex::new(None),
            auth: Mutex::new(Ok(None)),
            auth_calls: AtomicUsize::new(0),
            behavior: Mutex::new(RefreshBehavior::Clean),
            refreshes: Mutex::new(Vec::new()),
            registered,
            accept_overlay: true,
            script: Mutex::new(VecDeque::new()),
            frames: Mutex::new(Vec::new()),
            opened: AtomicBool::new(false),
        })
    }

    fn standalone() -> Arc<Self> {
        Self::new(Arc::new(Mutex::new(Vec::new())))
    }

    fn with_script(self: Arc<Self>, script: Vec<(&str, Vec<OverlayKey>)>) -> Arc<Self> {
        *self.script.lock().unwrap() = script
            .into_iter()
            .map(|(text, keys)| (text.to_string(), keys))
            .collect();
        self
    }

    fn set_auth(&self, auth: HostProviderAuth) {
        *self.auth.lock().unwrap() = Ok(Some(auth));
    }

    fn set_behavior(&self, behavior: RefreshBehavior) {
        *self.behavior.lock().unwrap() = behavior;
    }

    fn notices(&self) -> Vec<Notice> {
        self.notices.lock().unwrap().clone()
    }

    fn messages(&self) -> Vec<(String, NotifyKind)> {
        self.notices()
            .into_iter()
            .map(|notice| (notice.message, notice.kind))
            .collect()
    }

    fn refreshes(&self) -> Vec<RefreshCall> {
        self.refreshes.lock().unwrap().clone()
    }

    fn saw_frame_with(&self, text: &str) -> bool {
        self.frames
            .lock()
            .unwrap()
            .iter()
            .any(|frame| frame.iter().any(|line| line.contains(text)))
    }
}

impl HostServices for FakeHost {
    fn notify(&self, message: &str, kind: NotifyKind) {
        let probe = self.requests.lock().unwrap().clone();
        let requests_seen = probe.map(|probe| probe()).unwrap_or_default();
        self.notices.lock().unwrap().push(Notice {
            message: message.to_string(),
            kind,
            requests_seen,
        });
    }

    fn provider_auth<'a>(
        &'a self,
        provider_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<HostProviderAuth>, String>> {
        Box::pin(async move {
            assert_eq!(provider_id, LLAMA_PROVIDER_ID);
            self.auth_calls.fetch_add(1, Ordering::SeqCst);
            self.auth.lock().unwrap().clone()
        })
    }

    fn refresh_provider<'a>(
        &'a self,
        provider_id: &'a str,
        allow_network: bool,
        cancel: CancelToken,
    ) -> BoxFuture<'a, ModelsRefreshResult> {
        Box::pin(async move {
            let catalog = self
                .registered
                .lock()
                .unwrap()
                .last()
                .map(|provider| {
                    provider
                        .models()
                        .iter()
                        .map(|model| model.id.as_str().to_string())
                        .collect()
                })
                .unwrap_or_default();
            self.refreshes.lock().unwrap().push(RefreshCall {
                provider: provider_id.to_string(),
                allow_network,
                catalog,
            });
            let behavior = self.behavior.lock().unwrap().clone();
            let mut result = ModelsRefreshResult::default();
            match behavior {
                RefreshBehavior::Clean => {}
                RefreshBehavior::Aborted => result.aborted = true,
                RefreshBehavior::WaitForCancel => {
                    cancel.cancelled().await;
                    result.aborted = true;
                }
                RefreshBehavior::Fail(error) => {
                    result.errors.insert(
                        provider_id.to_string(),
                        ProviderError::ModelSource(Box::new(error)),
                    );
                }
            }
            result
        })
    }

    fn open_overlay(&self, mut overlay: Box<dyn InteractiveOverlay>) -> bool {
        if !self.accept_overlay {
            return false;
        }
        self.opened.store(true, Ordering::SeqCst);
        let mut turns = 0usize;
        loop {
            let frame: Vec<String> = overlay
                .render(100, 30)
                .iter()
                .map(|line| line.plain_text().trim_end().to_string())
                .collect();
            let next_keys = {
                let mut script = self.script.lock().unwrap();
                if script.front().is_some_and(|(wanted, _)| {
                    frame.iter().any(|line| line.contains(wanted.as_str()))
                }) {
                    script.pop_front().map(|(_, keys)| keys)
                } else {
                    None
                }
            };
            self.frames.lock().unwrap().push(frame);
            for key in next_keys.into_iter().flatten() {
                let _ = overlay.handle_key(key);
            }
            let _ = overlay.tick();
            if overlay.should_close() {
                return true;
            }
            turns += 1;
            // A host nobody closes must not hang the suite.
            if turns >= 3_000 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// The registrar a native is handed: records every provider it is asked to register.
struct FakeRegistrar {
    owner: ExtensionId,
    providers: Arc<Mutex<Vec<Arc<dyn Provider>>>>,
}

impl LateRegistrar for FakeRegistrar {
    fn register_tool(&self, _tool: Arc<dyn cyrup_core::Tool>) -> Result<(), ExtError> {
        Ok(())
    }

    fn register_command(&self, _name: String, _desc: CommandDescriptor) -> Result<(), ExtError> {
        Ok(())
    }

    fn register_tool_renderer(&self, _tool_name: String) -> Result<(), ExtError> {
        Ok(())
    }

    fn register_provider_live(
        &self,
        id: String,
        provider: Arc<dyn Provider>,
    ) -> Result<(), ExtError> {
        assert_eq!(id, LLAMA_PROVIDER_ID);
        self.providers.lock().unwrap().push(provider);
        Ok(())
    }

    /// The extension never unregisters its provider; a call is a bug the test must see, not a
    /// retraction the fake pretends to have made.
    fn unregister_provider(&self, id: &str) -> Result<bool, ExtError> {
        Err(ExtError::Component(format!(
            "FakeRegistrar was asked to unregister `{id}`"
        )))
    }

    fn owner(&self) -> ExtensionId {
        self.owner.clone()
    }
}

/// A [`LlamaUi`] that answers from queues and records what it was asked.
#[derive(Default)]
struct ScriptedUi {
    models: Mutex<VecDeque<LlamaManagerAction>>,
    selects: Mutex<VecDeque<Option<String>>>,
    search_answer: Mutex<Option<String>>,
    search_query: Mutex<Option<String>>,
    search_result: Mutex<Option<Result<Vec<HuggingFaceModel>, LlamaError>>>,
    search_calls: AtomicUsize,
    stop_once: AtomicBool,
    shown: Mutex<Vec<ShownList>>,
    asked: Mutex<Vec<(String, Vec<String>)>>,
    statuses: Mutex<Vec<(String, String)>>,
    progress: Mutex<Vec<ProgressState>>,
}

impl ScriptedUi {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn push_model(&self, action: LlamaManagerAction) -> &Self {
        self.models.lock().unwrap().push_back(action);
        self
    }

    fn pick(&self, model: LlamaModelInfo) -> &Self {
        self.push_model(LlamaManagerAction::Model(Box::new(model)))
    }

    fn answer(&self, choice: Option<&str>) -> &Self {
        self.selects
            .lock()
            .unwrap()
            .push_back(choice.map(str::to_string));
        self
    }

    fn asked(&self) -> Vec<(String, Vec<String>)> {
        self.asked.lock().unwrap().clone()
    }

    fn titles(&self) -> Vec<String> {
        self.asked().into_iter().map(|(title, _)| title).collect()
    }

    fn shown(&self) -> Vec<ShownList> {
        self.shown.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlamaUi for ScriptedUi {
    async fn show_models(&self, server_url: &str, models: &[LlamaModelInfo]) -> LlamaManagerAction {
        self.shown.lock().unwrap().push((
            server_url.to_string(),
            models
                .iter()
                .map(|model| (model.id.clone(), model.status.value.as_str().to_string()))
                .collect(),
        ));
        self.models
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(LlamaManagerAction::Close)
    }

    async fn select(&self, title: &str, options: &[String]) -> Option<String> {
        self.asked
            .lock()
            .unwrap()
            .push((title.to_string(), options.to_vec()));
        self.selects.lock().unwrap().pop_front().flatten()
    }

    async fn search_models(&self, search: SearchFn) -> Option<String> {
        self.search_calls.fetch_add(1, Ordering::SeqCst);
        let query = self.search_query.lock().unwrap().clone();
        if let Some(query) = query {
            let found = search(query, CancellationToken::new()).await;
            *self.search_result.lock().unwrap() = Some(found);
        }
        self.search_answer.lock().unwrap().clone()
    }

    fn show_status(&self, title: &str, message: &str) {
        self.statuses
            .lock()
            .unwrap()
            .push((title.to_string(), message.to_string()));
    }

    async fn progress(&self, state: &ProgressState) {
        self.progress.lock().unwrap().push(state.clone());
        if self.stop_once.swap(false, Ordering::SeqCst) {
            // The user presses Escape a moment after the run started.
            tokio::time::sleep(Duration::from_millis(30)).await;
            return;
        }
        std::future::pending::<()>().await;
    }

    fn update_progress(&self, _state: &ProgressState) {}
}

// ------------------------------------------------------------------------------ Hugging Face --

/// A loopback Hugging Face API: `GET /api/models?...` answers `search`, `GET /api/models/<id>`
/// answers the details registered for `<id>`.
struct FakeHuggingFace {
    url: String,
    seen: Arc<Mutex<Vec<HfRequest>>>,
}

impl FakeHuggingFace {
    async fn spawn(search: Value, details: Vec<(&str, Value)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<HfRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let details: BTreeMap<String, Value> = details
            .into_iter()
            .map(|(id, body)| (id.to_string(), body))
            .collect();
        let recorded = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let recorded = Arc::clone(&recorded);
                let search = search.clone();
                let details = details.clone();
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                        match socket.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buffer).to_string();
                    let mut lines = head.split("\r\n");
                    let target = lines
                        .next()
                        .and_then(|line| line.split(' ').nth(1))
                        .unwrap_or_default()
                        .to_string();
                    let headers = lines
                        .filter_map(|line| line.split_once(':'))
                        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
                        .collect();
                    recorded.lock().unwrap().push((target.clone(), headers));
                    let path = target.split('?').next().unwrap_or_default();
                    let (status, body) = if path == "/api/models" {
                        (200, search)
                    } else if let Some(id) = path.strip_prefix("/api/models/") {
                        match details.get(id) {
                            Some(body) => (200, body.clone()),
                            None => (404, json!({ "error": "not found" })),
                        }
                    } else {
                        (404, json!({ "error": "not found" }))
                    };
                    let body = body.to_string();
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        Self { url, seen }
    }

    fn requests(&self) -> Vec<HfRequest> {
        self.seen.lock().unwrap().clone()
    }
}

/// The details body of a repository: its id, its gate and one `.gguf` per `(quantization, size)`.
fn repository(id: &str, gated: Value, files: &[(&str, Option<u64>)]) -> Value {
    let siblings: Vec<Value> = files
        .iter()
        .map(|(quantization, size)| match size {
            Some(size) => {
                json!({ "rfilename": format!("model-{quantization}.gguf"), "size": size })
            }
            None => json!({ "rfilename": format!("model-{quantization}.gguf") }),
        })
        .collect();
    json!({ "id": id, "gated": gated, "siblings": siblings })
}

const GIB: u64 = 1024 * 1024 * 1024;

// ------------------------------------------------------------------------------------- the rig --

#[derive(Default)]
struct RigOptions {
    request_timeout: Option<Duration>,
    sync_timeout: Option<Duration>,
    hugging_face: Option<String>,
    environment: BTreeMap<String, String>,
    behavior: Option<RefreshBehavior>,
}

/// A fake llama-server, a fake host, a scripted UI and a [`Flow`] wired to all three.
struct Rig {
    server: Arc<FakeLlamaServer>,
    host: Arc<FakeHost>,
    ui: Arc<ScriptedUi>,
    flow: Flow,
}

impl Rig {
    async fn new(models: Vec<Value>) -> Self {
        Self::with(models, RigOptions::default()).await
    }

    async fn with(models: Vec<Value>, options: RigOptions) -> Self {
        let server = Arc::new(FakeLlamaServer::with_models(models).await);
        let registered: Arc<Mutex<Vec<Arc<dyn Provider>>>> = Arc::new(Mutex::new(Vec::new()));
        let host = FakeHost::new(Arc::clone(&registered));
        if let Some(behavior) = options.behavior {
            host.set_behavior(behavior);
        }
        let probe_server = Arc::clone(&server);
        *host.requests.lock().unwrap() = Some(Arc::new(move || {
            probe_server
                .requests()
                .iter()
                .map(|request| format!("{} {}", request.method, request.path()))
                .collect()
        }));

        let sink = Arc::clone(&registered);
        let controller = LlamaController::new(LlamaControllerOptions::new(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(move |provider| {
                sink.lock().unwrap().push(provider);
                Ok(())
            }),
        ));
        let mut client = LlamaClient::with_http_client(server.url(), None, http()).unwrap();
        if let Some(timeout) = options.request_timeout {
            client = client.with_request_timeout(timeout);
        }
        let mut flow = Flow::new(
            Arc::clone(&host) as Arc<dyn HostServices>,
            controller,
            client,
            Endpoints {
                http: Some(http()),
                hugging_face_url: options.hugging_face,
                environment: Some(options.environment),
            },
        );
        if let Some(timeout) = options.sync_timeout {
            flow = flow.with_sync_timeout(timeout);
        }
        Self {
            server,
            host,
            ui: ScriptedUi::new(),
            flow,
        }
    }

    fn ui(&self) -> Arc<dyn LlamaUi> {
        Arc::clone(&self.ui) as Arc<dyn LlamaUi>
    }

    /// `"METHOD /path"` plus the posted model id, for the requests that change the router.
    fn posts(&self) -> Vec<String> {
        self.server
            .requests()
            .into_iter()
            .filter(|request| request.method == "POST")
            .map(|request| {
                let body = request.json();
                format!(
                    "{} {}",
                    request.path(),
                    body.get("model")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                )
            })
            .collect()
    }

    fn catalog_reads(&self) -> usize {
        self.server.requests_to("GET", "/models").len()
    }

    async fn run(&self) -> Result<(), FlowError> {
        self.flow.run(self.ui()).await
    }
}

fn error_of(result: Result<(), FlowError>) -> FlowError {
    result.expect_err("the step must fail")
}

// =================================================================================================
// Pure helpers
// =================================================================================================

/// `parseHuggingFaceModel` (`index.ts:22-27`): the colon that separates the quantization is the
/// first one after the first `/`.
#[test]
fn parses_owner_repo_and_an_optional_quantization() {
    let parse = |value: &str| {
        let parsed = parse_huggingface_model(value);
        (parsed.repository, parsed.quantization)
    };
    assert_eq!(parse("owner/repo"), ("owner/repo".to_string(), None));
    assert_eq!(
        parse("owner/repo:Q4_K_M"),
        ("owner/repo".to_string(), Some("Q4_K_M".to_string()))
    );
    // No slash: the search starts at the beginning.
    assert_eq!(
        parse("repo:Q4"),
        ("repo".to_string(), Some("Q4".to_string()))
    );
    // A colon before the slash is part of the repository.
    assert_eq!(
        parse("own:er/repo:Q8_0"),
        ("own:er/repo".to_string(), Some("Q8_0".to_string()))
    );
    // `owner/repo:` has an empty quantization, which is falsy upstream.
    assert_eq!(
        parse("owner/repo:"),
        ("owner/repo".to_string(), Some(String::new()))
    );
}

/// `isConnectionError` / `connectionErrorMessage` (`index.ts:11-20`).
#[test]
fn classifies_connection_errors_by_message() {
    let connection = [
        FlowError::from(LlamaError::Transport("fetch failed: refused".to_string())),
        FlowError::from(LlamaError::Timeout),
        FlowError::from(LlamaError::Message("Network is unreachable".to_string())),
        FlowError::from(LlamaError::Timeout),
        FlowError::from(LlamaError::Transport("fetch failed".to_string())),
    ];
    for error in &connection {
        assert!(is_connection_error(error), "{error:?}");
        assert_eq!(
            connection_error_message(error),
            "Could not connect to the server."
        );
    }
    let other = [
        FlowError::from(LlamaError::Cancelled),
        FlowError::from(LlamaError::Message("out of memory".to_string())),
        FlowError::from(LlamaError::Message(
            "Hugging Face returned HTTP 500".to_string(),
        )),
    ];
    for error in &other {
        assert!(!is_connection_error(error), "{error:?}");
        assert_eq!(connection_error_message(error), error.message());
    }
}

/// `modelIsLoaded` (`index.ts:7-9`): a sleeping model counts as loaded.
#[test]
fn loaded_and_sleeping_models_are_loaded() {
    assert!(model_is_loaded(&info("a", "loaded")));
    assert!(model_is_loaded(&info("a", "sleeping")));
    for status in ["unloaded", "loading", "downloading"] {
        assert!(!model_is_loaded(&info("a", status)), "{status}");
    }
}

// =================================================================================================
// syncCatalog (`index.ts:46-64`)
// =================================================================================================

/// The catalog reaches the provider before the host refreshes it, and the refresh is asked for the
/// llama.cpp provider only with the network allowed (`index.ts:53-58`).
#[tokio::test]
async fn sync_catalog_hands_the_catalog_over_then_refreshes_with_the_network_allowed() {
    let rig = Rig::new(vec![model("a", "loaded"), model("b", "unloaded")]).await;
    let catalog = rig.flow.sync_catalog(None).await.unwrap();
    assert_eq!(
        catalog.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(
        rig.host.refreshes(),
        vec![RefreshCall {
            provider: "llama.cpp".to_string(),
            allow_network: true,
            // The provider already offered the loaded model when the refresh ran; an unloaded one
            // is not selectable without router autoload.
            catalog: vec!["a".to_string()],
        }]
    );
}

/// A catalog the caller already holds is not read again (`catalog ?? await client.list(...)`,
/// `index.ts:52`).
#[tokio::test]
async fn sync_catalog_uses_the_given_catalog_without_listing() {
    let rig = Rig::new(vec![model("server-side", "loaded")]).await;
    let given = vec![info("given", "loaded")];
    let returned = rig.flow.sync_catalog(Some(given.clone())).await.unwrap();
    assert_eq!(returned, given);
    assert_eq!(rig.catalog_reads(), 0);
    assert_eq!(rig.host.refreshes()[0].catalog, vec!["given".to_string()]);
}

/// An aborted refresh is the 15 s timeout: `Model catalog refresh timed out.` (`index.ts:60`),
/// which is not a connection error.
#[tokio::test]
async fn an_aborted_refresh_reports_a_timeout_message() {
    let rig = Rig::with(
        vec![model("a", "loaded")],
        RigOptions {
            sync_timeout: Some(Duration::from_millis(60)),
            behavior: Some(RefreshBehavior::WaitForCancel),
            ..RigOptions::default()
        },
    )
    .await;
    let error = rig.flow.sync_catalog(None).await.unwrap_err();
    assert_eq!(error.message(), "Model catalog refresh timed out.");
    assert!(!is_connection_error(&error));
    // Whatever the host reports, an abort wins even when it carries no error.
    rig.host.set_behavior(RefreshBehavior::Aborted);
    let error = rig.flow.sync_catalog(None).await.unwrap_err();
    assert_eq!(error.message(), "Model catalog refresh timed out.");
}

/// The provider's own refresh error is rethrown (`index.ts:61-62`), not the host's wrapper of it.
#[tokio::test]
async fn sync_catalog_rethrows_the_providers_refresh_error() {
    let rig = Rig::new(vec![model("a", "loaded")]).await;
    rig.host
        .set_behavior(RefreshBehavior::Fail(LlamaError::Message(
            "boom".to_string(),
        )));
    let error = rig.flow.sync_catalog(None).await.unwrap_err();
    assert_eq!(error.message(), "boom");
    assert!(!is_connection_error(&error));

    rig.host
        .set_behavior(RefreshBehavior::Fail(LlamaError::Timeout));
    let error = rig.flow.sync_catalog(None).await.unwrap_err();
    assert!(is_connection_error(&error), "{error:?}");
}

/// `AbortSignal.timeout` fails a pending fetch with a `TimeoutError`; the timer cancelling the
/// token must read as a timeout, which is a connection error (`index.ts:14`, `:51`).
#[tokio::test]
async fn a_list_that_outlives_the_timer_is_a_connection_error() {
    let rig = Rig::with(
        vec![model("a", "loaded")],
        RigOptions {
            sync_timeout: Some(Duration::from_millis(60)),
            ..RigOptions::default()
        },
    )
    .await;
    rig.server.respond("GET", "/models", Reply::Hang);
    let error = rig.flow.sync_catalog(None).await.unwrap_err();
    assert!(is_connection_error(&error), "{error:?}");
    assert_eq!(error.message(), "The operation was aborted due to timeout");
}

// =================================================================================================
// The flow: reading the catalog (`index.ts:193-203`)
// =================================================================================================

/// A failed read offers Retry / Close with the server and the generic connection message, and Retry
/// reads again.
#[tokio::test]
async fn a_connection_error_offers_retry_and_a_retry_reads_again() {
    let rig = Rig::with(
        vec![model("a", "loaded")],
        RigOptions {
            request_timeout: Some(Duration::from_millis(100)),
            ..RigOptions::default()
        },
    )
    .await;
    rig.server.respond_times("GET", "/models", Reply::Hang, 1);
    rig.ui.answer(Some("Retry"));
    rig.run().await.unwrap();

    assert_eq!(
        rig.ui.asked(),
        vec![(
            format!(
                "llama.cpp unavailable\n{}\n\nCould not connect to the server.",
                rig.server.url()
            ),
            vec!["Retry".to_string(), "Close".to_string()],
        )]
    );
    assert_eq!(rig.catalog_reads(), 2);
    assert_eq!(
        rig.ui.shown(),
        vec![(
            rig.server.url().to_string(),
            vec![("a".to_string(), "loaded".to_string())]
        )]
    );
}

/// Close ends the command without ever showing the model list.
#[tokio::test]
async fn closing_the_connection_dialog_ends_the_command() {
    let rig = Rig::with(
        vec![],
        RigOptions {
            request_timeout: Some(Duration::from_millis(100)),
            ..RigOptions::default()
        },
    )
    .await;
    rig.server.respond("GET", "/models", Reply::Hang);
    rig.ui.answer(Some("Close"));
    rig.run().await.unwrap();
    assert_eq!(rig.ui.asked().len(), 1);
    assert!(rig.ui.shown().is_empty());
    assert_eq!(rig.catalog_reads(), 1);
}

/// An error that is not a connection error shows its own message in the same dialog.
#[tokio::test]
async fn the_connection_dialog_shows_other_errors_verbatim() {
    let rig = Rig::new(vec![]).await;
    rig.server.respond(
        "GET",
        "/models",
        Reply::Json(500, json!({ "error": { "message": "router exploded" } })),
    );
    rig.ui.answer(Some("Close"));
    rig.run().await.unwrap();
    assert_eq!(
        rig.ui.titles(),
        vec![format!(
            "llama.cpp unavailable\n{}\n\nrouter exploded",
            rig.server.url()
        )]
    );
}

/// The refresh timeout is surfaced through the same dialog.
#[tokio::test]
async fn the_refresh_abort_message_reaches_the_dialog() {
    let rig = Rig::with(
        vec![model("a", "loaded")],
        RigOptions {
            sync_timeout: Some(Duration::from_millis(60)),
            behavior: Some(RefreshBehavior::WaitForCancel),
            ..RigOptions::default()
        },
    )
    .await;
    rig.ui.answer(Some("Close"));
    rig.run().await.unwrap();
    assert_eq!(
        rig.ui.titles(),
        vec![format!(
            "llama.cpp unavailable\n{}\n\nModel catalog refresh timed out.",
            rig.server.url()
        )]
    );
}

/// A provider's refresh failure is surfaced through the dialog too.
#[tokio::test]
async fn a_providers_refresh_failure_reaches_the_dialog() {
    let rig = Rig::new(vec![model("a", "loaded")]).await;
    rig.host
        .set_behavior(RefreshBehavior::Fail(LlamaError::Message(
            "boom".to_string(),
        )));
    rig.ui.answer(Some("Close"));
    rig.run().await.unwrap();
    assert_eq!(
        rig.ui.titles(),
        vec![format!(
            "llama.cpp unavailable\n{}\n\nboom",
            rig.server.url()
        )]
    );
}

// =================================================================================================
// The flow: the model list (`index.ts:204-227`)
// =================================================================================================

/// A loaded model asks to be unloaded, unloads, and the list is read again.
#[tokio::test]
async fn a_loaded_model_is_unloaded_after_confirmation() {
    let rig = Rig::new(vec![model("a", "loaded")]).await;
    rig.server
        .on_unload("a", vec![Step::SetModel(model("a", "unloaded"))]);
    rig.ui.pick(info("a", "loaded"));
    rig.ui.answer(Some("Yes"));
    rig.run().await.unwrap();

    assert_eq!(
        rig.ui.asked(),
        vec![(
            "Unload model?\na".to_string(),
            vec!["Yes".to_string(), "No".to_string()]
        )]
    );
    assert_eq!(rig.posts(), vec!["/models/unload a".to_string()]);
    assert_eq!(
        rig.host.messages(),
        vec![("Unloaded a".to_string(), NotifyKind::Info)]
    );
    // The list was shown again with the catalog read after the action.
    let shown = rig.ui.shown();
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[1].1, vec![("a".to_string(), "unloaded".to_string())]);
}

/// Declining the confirmation unloads nothing and says nothing.
#[tokio::test]
async fn declining_the_unload_confirmation_changes_nothing() {
    let rig = Rig::new(vec![model("a", "loaded")]).await;
    rig.ui.pick(info("a", "loaded"));
    rig.ui.answer(Some("No"));
    rig.run().await.unwrap();
    assert!(rig.posts().is_empty());
    assert!(rig.host.notices().is_empty());
}

/// A sleeping model is loaded in the router's eyes: it is unloaded, not loaded.
#[tokio::test]
async fn a_sleeping_model_is_unloaded_not_loaded() {
    let rig = Rig::new(vec![model("s", "sleeping")]).await;
    rig.server
        .on_unload("s", vec![Step::SetModel(model("s", "unloaded"))]);
    rig.ui.pick(info("s", "sleeping"));
    rig.ui.answer(Some("Yes"));
    rig.run().await.unwrap();
    assert_eq!(rig.ui.titles(), vec!["Unload model?\ns".to_string()]);
    assert_eq!(rig.posts(), vec!["/models/unload s".to_string()]);
}

/// An unloaded model is loaded.
#[tokio::test]
async fn an_unloaded_model_is_loaded() {
    let rig = Rig::new(vec![model("a", "unloaded")]).await;
    rig.server
        .on_load("a", vec![Step::SetModel(model("a", "loaded"))]);
    rig.ui.pick(info("a", "unloaded"));
    rig.run().await.unwrap();
    assert_eq!(rig.posts(), vec!["/models/load a".to_string()]);
    assert_eq!(
        rig.host.messages(),
        vec![("Loaded a".to_string(), NotifyKind::Info)]
    );
    let shown = rig.ui.shown();
    assert_eq!(shown[1].1, vec![("a".to_string(), "loaded".to_string())]);
}

/// A model in any other state is only reported, as a warning (`index.ts:216`).
#[tokio::test]
async fn a_busy_model_is_reported_as_a_warning() {
    let rig = Rig::new(vec![model("b", "loading"), model("c", "downloading")]).await;
    rig.ui.pick(info("b", "loading"));
    rig.ui.pick(info("c", "downloading"));
    rig.run().await.unwrap();
    assert!(rig.posts().is_empty());
    assert_eq!(
        rig.host.messages(),
        vec![
            ("b is loading".to_string(), NotifyKind::Warning),
            ("c is downloading".to_string(), NotifyKind::Warning),
        ]
    );
    assert_eq!(rig.ui.shown().len(), 3);
}

/// The "Download model…" row starts the download flow (`index.ts:212`).
#[tokio::test]
async fn the_download_row_starts_the_download_flow() {
    let rig = Rig::new(vec![]).await;
    rig.ui.push_model(LlamaManagerAction::Download);
    rig.run().await.unwrap();
    assert_eq!(rig.ui.search_calls.load(Ordering::SeqCst), 1);
}

/// A failed action is reported as an error, and only after the catalog has been read again, so the
/// list the user returns to is current (`index.ts:217-225`).
#[tokio::test]
async fn an_action_failure_is_notified_after_the_catalog_is_read_again() {
    let rig = Rig::new(vec![model("a", "loaded")]).await;
    rig.server.respond(
        "POST",
        "/models/unload",
        Reply::Json(500, json!({ "error": { "message": "cannot unload" } })),
    );
    rig.ui.pick(info("a", "loaded"));
    rig.ui.answer(Some("Yes"));
    rig.run().await.unwrap();

    let notices = rig.host.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].message, "cannot unload");
    assert_eq!(notices[0].kind, NotifyKind::Error);
    let seen = &notices[0].requests_seen;
    let failed = seen
        .iter()
        .position(|request| request == "POST /models/unload")
        .unwrap();
    assert!(
        seen.iter()
            .skip(failed + 1)
            .any(|request| request == "GET /models"),
        "the catalog was read again before the error was shown: {seen:?}"
    );
    assert_eq!(rig.ui.shown().len(), 2);
}

/// A connection error out of an action is not reported on its own: the re-read that follows shows
/// the Retry / Close dialog if the server is really gone (`index.ts:223`).
#[tokio::test]
async fn a_connection_error_from_an_action_is_not_notified() {
    let rig = Rig::with(
        vec![model("a", "loaded")],
        RigOptions {
            request_timeout: Some(Duration::from_millis(100)),
            ..RigOptions::default()
        },
    )
    .await;
    rig.server.respond("POST", "/models/unload", Reply::Hang);
    rig.ui.pick(info("a", "loaded"));
    rig.ui.answer(Some("Yes"));
    rig.run().await.unwrap();
    assert!(
        rig.host
            .notices()
            .iter()
            .all(|notice| notice.kind != NotifyKind::Error),
        "{:?}",
        rig.host.notices()
    );
    assert_eq!(rig.ui.shown().len(), 2, "the re-read still happened");
}

// =================================================================================================
// loadModel (`index.ts:66-123`)
// =================================================================================================

/// Nothing else loaded: no question, the load runs under the progress view and ends with `Loaded`.
#[tokio::test]
async fn load_with_nothing_else_loaded_asks_nothing() {
    let rig = Rig::new(vec![model("a", "unloaded")]).await;
    rig.server
        .on_load("a", vec![Step::SetModel(model("a", "loaded"))]);
    let catalog = vec![info("a", "unloaded")];
    rig.flow
        .load_model(&rig.ui(), &catalog, &catalog[0])
        .await
        .unwrap();

    assert!(rig.ui.asked().is_empty());
    assert_eq!(rig.posts(), vec!["/models/load a".to_string()]);
    assert_eq!(
        rig.host.messages(),
        vec![("Loaded a".to_string(), NotifyKind::Info)]
    );
    let first = rig.ui.progress.lock().unwrap()[0].clone();
    assert_eq!(first.title, "Loading model");
    assert_eq!(first.model, "a");
    assert_eq!(first.message, "Starting…");
}

/// A load the router has accepted but that the catalog does not show as loaded yet reads
/// `Load started` (`index.ts:110-112`).
#[tokio::test]
async fn load_that_is_not_loaded_in_the_catalog_says_load_started() {
    let rig = Rig::new(vec![model("a", "loading")]).await;
    // The wait sees the model loaded once; the catalog read after it does not.
    rig.server.respond_times(
        "GET",
        "/models",
        Reply::Json(200, json!({ "data": [model("a", "loaded")] })),
        1,
    );
    let catalog = vec![info("a", "unloaded")];
    rig.flow
        .load_model(&rig.ui(), &catalog, &catalog[0])
        .await
        .unwrap();
    assert_eq!(
        rig.host.messages(),
        vec![("Load started for a".to_string(), NotifyKind::Info)]
    );
}

fn two_loaded_and_a_target() -> (Vec<Value>, Vec<LlamaModelInfo>) {
    (
        vec![
            model("b", "loaded"),
            model("c", "sleeping"),
            model("a", "unloaded"),
        ],
        vec![
            info("b", "loaded"),
            info("c", "sleeping"),
            info("a", "unloaded"),
        ],
    )
}

fn script_unload_and_load(rig: &Rig, ids: &[&str]) {
    for id in ids {
        rig.server
            .on_unload(id, vec![Step::SetModel(model(id, "unloaded"))]);
        rig.server
            .on_load(id, vec![Step::SetModel(model(id, "loaded"))]);
    }
}

/// Models already loaded: the question counts them, "Unload all and load" unloads each of them
/// before it loads the target.
#[tokio::test]
async fn load_can_replace_the_loaded_models() {
    let (models, catalog) = two_loaded_and_a_target();
    let rig = Rig::new(models).await;
    script_unload_and_load(&rig, &["a", "b", "c"]);
    rig.ui.answer(Some("Unload all and load"));
    rig.flow
        .load_model(&rig.ui(), &catalog, &catalog[2])
        .await
        .unwrap();

    assert_eq!(
        rig.ui.asked(),
        vec![(
            "2 models are loaded".to_string(),
            vec![
                "Unload all and load".to_string(),
                "Keep loaded and load".to_string(),
                "Cancel".to_string()
            ]
        )]
    );
    assert_eq!(
        rig.posts(),
        vec![
            "/models/unload b".to_string(),
            "/models/unload c".to_string(),
            "/models/load a".to_string()
        ]
    );
    assert_eq!(
        rig.host.messages(),
        vec![("Loaded a".to_string(), NotifyKind::Info)]
    );
}

/// The target is not one of "the loaded models", even when the catalog snapshot lists it as loaded.
#[tokio::test]
async fn load_does_not_count_the_target_among_the_loaded_models() {
    let rig = Rig::new(vec![model("b", "loaded"), model("a", "unloaded")]).await;
    script_unload_and_load(&rig, &["a"]);
    rig.ui.answer(Some("Keep loaded and load"));
    let catalog = vec![info("a", "loaded"), info("b", "loaded")];
    rig.flow
        .load_model(&rig.ui(), &catalog, &info("a", "unloaded"))
        .await
        .unwrap();
    assert_eq!(rig.ui.titles(), vec!["1 model is loaded".to_string()]);
}

/// One loaded model reads "1 model is loaded"; "Keep loaded and load" unloads nothing.
#[tokio::test]
async fn load_can_keep_the_loaded_models() {
    let rig = Rig::new(vec![model("b", "loaded"), model("a", "unloaded")]).await;
    script_unload_and_load(&rig, &["a"]);
    rig.ui.answer(Some("Keep loaded and load"));
    let catalog = vec![info("b", "loaded"), info("a", "unloaded")];
    rig.flow
        .load_model(&rig.ui(), &catalog, &catalog[1])
        .await
        .unwrap();
    assert_eq!(rig.ui.titles(), vec!["1 model is loaded".to_string()]);
    assert_eq!(rig.posts(), vec!["/models/load a".to_string()]);
}

/// "Cancel" and a dismissed question both end the load before anything is sent.
#[tokio::test]
async fn load_stops_when_the_question_is_cancelled() {
    for answer in [Some("Cancel"), None] {
        let (models, catalog) = two_loaded_and_a_target();
        let rig = Rig::new(models).await;
        rig.ui.answer(answer);
        rig.flow
            .load_model(&rig.ui(), &catalog, &catalog[2])
            .await
            .unwrap();
        assert!(rig.posts().is_empty(), "{answer:?}");
        assert!(rig.host.notices().is_empty(), "{answer:?}");
    }
}

/// Stopping a load after "Unload all and load" unloads the target and loads the models that were
/// unloaded again.
#[tokio::test]
async fn stopping_a_replacing_load_restores_the_previous_models() {
    let rig = Rig::new(vec![model("b", "loaded"), model("a", "unloaded")]).await;
    script_unload_and_load(&rig, &["b"]);
    rig.ui.stop_once.store(true, Ordering::SeqCst);
    rig.ui
        .answer(Some("Unload all and load"))
        .answer(Some("Yes"));
    let catalog = vec![info("b", "loaded"), info("a", "unloaded")];
    rig.flow
        .load_model(&rig.ui(), &catalog, &catalog[1])
        .await
        .unwrap();

    assert_eq!(
        rig.ui.asked()[1],
        (
            "Stop loading?\na".to_string(),
            vec!["Yes".to_string(), "No".to_string()]
        )
    );
    assert_eq!(
        rig.posts(),
        vec![
            "/models/unload b".to_string(),
            "/models/load a".to_string(),
            "/models/unload a".to_string(),
            "/models/load b".to_string(),
        ]
    );
    assert_eq!(
        rig.host.messages(),
        vec![(
            "Restoring previously loaded models".to_string(),
            NotifyKind::Info
        )]
    );
    // The restore ends with a catalog sync.
    assert_eq!(rig.host.refreshes().len(), 1);
}

/// Without "Unload all", stopping a load restores nothing.
#[tokio::test]
async fn stopping_a_non_replacing_load_restores_nothing() {
    let rig = Rig::new(vec![model("b", "loaded"), model("a", "unloaded")]).await;
    rig.ui.stop_once.store(true, Ordering::SeqCst);
    rig.ui
        .answer(Some("Keep loaded and load"))
        .answer(Some("Yes"));
    let catalog = vec![info("b", "loaded"), info("a", "unloaded")];
    rig.flow
        .load_model(&rig.ui(), &catalog, &catalog[1])
        .await
        .unwrap();
    assert_eq!(
        rig.posts(),
        vec!["/models/load a".to_string(), "/models/unload a".to_string()]
    );
    assert!(rig.host.notices().is_empty());
}

/// Answering "No" to "Stop loading?" keeps loading.
#[tokio::test]
async fn declining_to_stop_the_load_keeps_loading() {
    let rig = Rig::new(vec![model("a", "unloaded")]).await;
    rig.server.on_load(
        "a",
        vec![Step::delay_ms(300), Step::SetModel(model("a", "loaded"))],
    );
    rig.ui.stop_once.store(true, Ordering::SeqCst);
    rig.ui.answer(Some("No"));
    let catalog = vec![info("a", "unloaded")];
    rig.flow
        .load_model(&rig.ui(), &catalog, &catalog[0])
        .await
        .unwrap();
    assert_eq!(rig.posts(), vec!["/models/load a".to_string()]);
    assert_eq!(
        rig.host.messages(),
        vec![("Loaded a".to_string(), NotifyKind::Info)]
    );
}

fn failing_load(id: &str, exit_code: i64) -> Vec<Step> {
    vec![Step::SetModel(model_with_status(
        id,
        "unloaded",
        json!({ "failed": true, "exit_code": exit_code }),
    ))]
}

/// A failed load after "Unload all and load" loads the previous models again and still fails with
/// the load's own error.
#[tokio::test]
async fn a_failed_replacing_load_restores_and_reports_the_load_error() {
    let rig = Rig::new(vec![model("b", "loaded"), model("a", "unloaded")]).await;
    script_unload_and_load(&rig, &["b"]);
    rig.server.on_load("a", failing_load("a", 1));
    rig.ui.answer(Some("Unload all and load"));
    let catalog = vec![info("b", "loaded"), info("a", "unloaded")];
    let error = error_of(rig.flow.load_model(&rig.ui(), &catalog, &catalog[1]).await);

    assert_eq!(error.message(), "Model exited with code 1");
    assert_eq!(
        rig.posts(),
        vec![
            "/models/unload b".to_string(),
            "/models/load a".to_string(),
            "/models/load b".to_string(),
        ]
    );
    assert_eq!(
        rig.host.messages(),
        vec![(
            "Restoring previously loaded models".to_string(),
            NotifyKind::Info
        )]
    );
}

/// When the restore fails as well, the original load error is the one reported.
#[tokio::test]
async fn a_failing_restore_does_not_replace_the_load_error() {
    let rig = Rig::new(vec![model("b", "loaded"), model("a", "unloaded")]).await;
    rig.server
        .on_unload("b", vec![Step::SetModel(model("b", "unloaded"))]);
    rig.server.on_load("a", failing_load("a", 1));
    rig.server.on_load("b", failing_load("b", 2));
    rig.ui.answer(Some("Unload all and load"));
    let catalog = vec![info("b", "loaded"), info("a", "unloaded")];
    let error = error_of(rig.flow.load_model(&rig.ui(), &catalog, &catalog[1]).await);
    assert_eq!(error.message(), "Model exited with code 1");
    assert!(rig.posts().contains(&"/models/load b".to_string()));
}

/// A failed load that unloaded nothing restores nothing.
#[tokio::test]
async fn a_failed_non_replacing_load_restores_nothing() {
    let rig = Rig::new(vec![model("b", "loaded"), model("a", "unloaded")]).await;
    rig.server.on_load("a", failing_load("a", 3));
    rig.ui.answer(Some("Keep loaded and load"));
    let catalog = vec![info("b", "loaded"), info("a", "unloaded")];
    let error = error_of(rig.flow.load_model(&rig.ui(), &catalog, &catalog[1]).await);
    assert_eq!(error.message(), "Model exited with code 3");
    assert_eq!(rig.posts(), vec!["/models/load a".to_string()]);
    assert!(rig.host.notices().is_empty());
}

/// Unloading the loaded models happens before the guarded part: a failure there is reported as it
/// is, with no load and no restore.
#[tokio::test]
async fn a_failed_unload_before_the_load_is_not_restored() {
    let rig = Rig::new(vec![model("b", "loaded"), model("a", "unloaded")]).await;
    rig.server.respond(
        "POST",
        "/models/unload",
        Reply::Json(500, json!({ "error": { "message": "stuck" } })),
    );
    rig.ui.answer(Some("Unload all and load"));
    let catalog = vec![info("b", "loaded"), info("a", "unloaded")];
    let error = error_of(rig.flow.load_model(&rig.ui(), &catalog, &catalog[1]).await);
    assert_eq!(error.message(), "stuck");
    assert_eq!(rig.posts(), vec!["/models/unload b".to_string()]);
    assert!(rig.host.notices().is_empty());
}

// =================================================================================================
// unloadModel (`index.ts:125-135`)
// =================================================================================================

/// Confirmed: unload, wait, sync, `Unloaded`.
#[tokio::test]
async fn unload_waits_for_the_model_and_syncs_the_catalog() {
    let rig = Rig::new(vec![model("a", "loaded")]).await;
    rig.server
        .on_unload("a", vec![Step::SetModel(model("a", "unloaded"))]);
    rig.ui.answer(Some("Yes"));
    rig.flow
        .unload_model(&rig.ui(), &info("a", "loaded"))
        .await
        .unwrap();
    assert_eq!(rig.posts(), vec!["/models/unload a".to_string()]);
    assert_eq!(rig.host.refreshes().len(), 1);
    assert_eq!(
        rig.host.messages(),
        vec![("Unloaded a".to_string(), NotifyKind::Info)]
    );
}

/// A dismissed confirmation unloads nothing.
#[tokio::test]
async fn unload_does_nothing_when_the_confirmation_is_dismissed() {
    let rig = Rig::new(vec![model("a", "loaded")]).await;
    rig.ui.answer(None);
    rig.flow
        .unload_model(&rig.ui(), &info("a", "loaded"))
        .await
        .unwrap();
    assert!(rig.posts().is_empty());
    assert!(rig.host.refreshes().is_empty());
}

// =================================================================================================
// downloadModel (`index.ts:137-181`)
// =================================================================================================

fn download_script(model_id: &str) -> Vec<Step> {
    vec![
        Step::WaitForSse(1),
        Step::SetModel(model(model_id, "downloading")),
        Step::delay_ms(20),
        Step::Sse(event(
            model_id,
            "download_progress",
            json!({ "progress": { "https://example/m.gguf": { "done": 512, "total": 1024 } } }),
        )),
        Step::SetModel(model(model_id, "unloaded")),
        Step::Sse(event(model_id, "download_finished", json!({}))),
    ]
}

async fn download_rig(search: Value, details: Vec<(&str, Value)>) -> (Rig, FakeHuggingFace) {
    let hugging_face = FakeHuggingFace::spawn(search, details).await;
    let rig = Rig::with(
        vec![],
        RigOptions {
            hugging_face: Some(hugging_face.url.clone()),
            ..RigOptions::default()
        },
    )
    .await;
    (rig, hugging_face)
}

/// The quantization picker lists `Q4_K_M` first (recommended), then by size with unknown sizes
/// last; the chosen one is downloaded and the catalog synced with what the download returned.
#[tokio::test]
async fn download_asks_for_a_quantization_and_downloads_it() {
    let (rig, hugging_face) = download_rig(
        json!([]),
        vec![(
            "owner/repo",
            repository(
                "owner/repo",
                json!(false),
                &[
                    ("Q8_0", Some(8 * GIB)),
                    ("Q4_K_M", Some(4 * GIB + GIB / 2)),
                    ("Q2_K", Some(2 * GIB)),
                    ("Q3_K_S", None),
                ],
            ),
        )],
    )
    .await;
    rig.server
        .on_download("owner/repo:Q2_K", download_script("owner/repo:Q2_K"));
    *rig.ui.search_answer.lock().unwrap() = Some("owner/repo".to_string());
    rig.ui.answer(Some("Q2_K · 2.00 GiB"));
    rig.flow.download_model(&rig.ui()).await.unwrap();

    assert_eq!(
        rig.ui.asked(),
        vec![(
            "Select quantization\nowner/repo".to_string(),
            vec![
                "Q4_K_M · 4.50 GiB · recommended".to_string(),
                "Q2_K · 2.00 GiB".to_string(),
                "Q8_0 · 8.00 GiB".to_string(),
                "Q3_K_S".to_string(),
            ]
        )]
    );
    assert_eq!(
        *rig.ui.statuses.lock().unwrap(),
        vec![(
            "Loading model details".to_string(),
            "owner/repo".to_string()
        )]
    );
    assert_eq!(rig.posts(), vec!["/models owner/repo:Q2_K".to_string()]);
    let first = rig.ui.progress.lock().unwrap()[0].clone();
    assert_eq!(first.title, "Downloading model");
    assert_eq!(first.model, "owner/repo:Q2_K");
    assert_eq!(first.message, "Starting…");
    assert_eq!(
        rig.host.messages(),
        vec![("Downloaded owner/repo:Q2_K".to_string(), NotifyKind::Info)]
    );
    // The catalog the download returned was synced, not read again by the command.
    assert_eq!(rig.host.refreshes().len(), 1);
    assert_eq!(
        hugging_face.requests().len(),
        1,
        "details only; no search ran"
    );
}

/// A quantization in the selection skips the picker, and the details are asked for the repository
/// alone.
#[tokio::test]
async fn download_with_a_quantization_in_the_selection_does_not_ask() {
    let (rig, hugging_face) = download_rig(
        json!([]),
        vec![(
            "owner/repo",
            repository("owner/repo", json!(false), &[("Q4_K_M", Some(GIB))]),
        )],
    )
    .await;
    rig.server
        .on_download("owner/repo:Q8_0", download_script("owner/repo:Q8_0"));
    *rig.ui.search_answer.lock().unwrap() = Some("owner/repo:Q8_0".to_string());
    rig.flow.download_model(&rig.ui()).await.unwrap();

    assert!(rig.ui.asked().is_empty());
    assert_eq!(rig.posts(), vec!["/models owner/repo:Q8_0".to_string()]);
    assert_eq!(
        rig.ui.statuses.lock().unwrap()[0].1,
        "owner/repo",
        "the status names the repository, not the quantization"
    );
    assert_eq!(
        hugging_face.requests()[0].0.split('?').next(),
        Some("/api/models/owner/repo")
    );
}

/// An empty quantization (`owner/repo:`) is falsy, so the picker is shown.
#[tokio::test]
async fn download_with_an_empty_quantization_still_asks() {
    let (rig, _hugging_face) = download_rig(
        json!([]),
        vec![(
            "owner/repo",
            repository("owner/repo", json!(false), &[("Q4_K_M", Some(GIB))]),
        )],
    )
    .await;
    *rig.ui.search_answer.lock().unwrap() = Some("owner/repo:".to_string());
    rig.ui.answer(None);
    rig.flow.download_model(&rig.ui()).await.unwrap();
    assert_eq!(
        rig.ui.titles(),
        vec!["Select quantization\nowner/repo".to_string()]
    );
}

/// A repository with no recognised quantization is downloaded by its id, which is the id the
/// details report.
#[tokio::test]
async fn download_without_quantizations_uses_the_reported_id() {
    let (rig, _hugging_face) = download_rig(
        json!([]),
        vec![("owner/repo", repository("Owner/Repo", json!(false), &[]))],
    )
    .await;
    rig.server
        .on_download("Owner/Repo", download_script("Owner/Repo"));
    *rig.ui.search_answer.lock().unwrap() = Some("owner/repo".to_string());
    rig.flow.download_model(&rig.ui()).await.unwrap();
    assert!(rig.ui.asked().is_empty());
    assert_eq!(rig.posts(), vec!["/models Owner/Repo".to_string()]);
    assert_eq!(
        rig.host.messages(),
        vec![("Downloaded Owner/Repo".to_string(), NotifyKind::Info)]
    );
}

/// Dismissing the search ends the download before anything is asked of Hugging Face.
#[tokio::test]
async fn download_stops_when_the_search_is_dismissed() {
    let (rig, hugging_face) = download_rig(json!([]), vec![]).await;
    rig.flow.download_model(&rig.ui()).await.unwrap();
    assert!(hugging_face.requests().is_empty());
    assert!(rig.posts().is_empty());
}

/// Dismissing the quantization picker ends the download.
#[tokio::test]
async fn download_stops_when_the_quantization_picker_is_dismissed() {
    let (rig, _hugging_face) = download_rig(
        json!([]),
        vec![(
            "owner/repo",
            repository("owner/repo", json!(false), &[("Q4_K_M", Some(GIB))]),
        )],
    )
    .await;
    *rig.ui.search_answer.lock().unwrap() = Some("owner/repo".to_string());
    rig.ui.answer(None);
    rig.flow.download_model(&rig.ui()).await.unwrap();
    assert!(rig.posts().is_empty());
    assert!(rig.host.notices().is_empty());
}

/// A gated repository asks for confirmation, with the wording for the kind of gate.
#[tokio::test]
async fn download_of_a_gated_repository_asks_before_continuing() {
    for (gate, approval) in [
        ("manual", "Manual approval is required"),
        ("auto", "Accept the access terms"),
    ] {
        let (rig, _hugging_face) = download_rig(
            json!([]),
            vec![(
                "owner/gated",
                repository("owner/gated", json!(gate), &[("Q4_K_M", Some(GIB))]),
            )],
        )
        .await;
        rig.server
            .on_download("owner/gated:Q4_K_M", download_script("owner/gated:Q4_K_M"));
        *rig.ui.search_answer.lock().unwrap() = Some("owner/gated".to_string());
        rig.ui
            .answer(Some("Continue"))
            .answer(Some("Q4_K_M · 1.00 GiB · recommended"));
        rig.flow.download_model(&rig.ui()).await.unwrap();

        assert_eq!(
            rig.ui.asked()[0],
            (
                format!(
                    "Hugging Face access required\nowner/gated\n\n{approval} at:\nhttps://huggingface.co/owner/gated\n\nThe llama.cpp server needs HF_TOKEN with access."
                ),
                vec!["Continue".to_string(), "Back".to_string()]
            ),
            "{gate}"
        );
        assert_eq!(
            rig.posts(),
            vec!["/models owner/gated:Q4_K_M".to_string()],
            "{gate}"
        );
    }
}

/// Anything but "Continue" on a gated repository ends the download.
#[tokio::test]
async fn download_of_a_gated_repository_stops_unless_continued() {
    for answer in [Some("Back"), None] {
        let (rig, _hugging_face) = download_rig(
            json!([]),
            vec![(
                "owner/gated",
                repository("owner/gated", json!("auto"), &[("Q4_K_M", Some(GIB))]),
            )],
        )
        .await;
        *rig.ui.search_answer.lock().unwrap() = Some("owner/gated:Q4_K_M".to_string());
        rig.ui.answer(answer);
        rig.flow.download_model(&rig.ui()).await.unwrap();
        assert_eq!(rig.ui.asked().len(), 1, "{answer:?}");
        assert!(rig.posts().is_empty(), "{answer:?}");
    }
}

/// An ungated repository is not asked about access.
#[tokio::test]
async fn download_of_an_open_repository_does_not_ask_about_access() {
    let (rig, _hugging_face) = download_rig(
        json!([]),
        vec![("owner/repo", repository("owner/repo", json!(false), &[]))],
    )
    .await;
    rig.server
        .on_download("owner/repo", download_script("owner/repo"));
    *rig.ui.search_answer.lock().unwrap() = Some("owner/repo".to_string());
    rig.flow.download_model(&rig.ui()).await.unwrap();
    assert!(rig.ui.asked().is_empty());
}

/// Stopping a download asks the router to unload the model and ends without a sync or a notice.
#[tokio::test]
async fn stopping_a_download_unloads_the_model_and_ends_quietly() {
    let (rig, _hugging_face) = download_rig(
        json!([]),
        vec![(
            "owner/repo",
            repository("owner/repo", json!(false), &[("Q4_K_M", Some(GIB))]),
        )],
    )
    .await;
    rig.ui.stop_once.store(true, Ordering::SeqCst);
    *rig.ui.search_answer.lock().unwrap() = Some("owner/repo:Q4_K_M".to_string());
    rig.ui.answer(Some("Yes"));
    rig.flow.download_model(&rig.ui()).await.unwrap();

    assert_eq!(
        rig.ui.asked(),
        vec![(
            "Stop download?\nowner/repo:Q4_K_M".to_string(),
            vec!["Yes".to_string(), "No".to_string()]
        )]
    );
    assert_eq!(
        rig.posts(),
        vec![
            "/models owner/repo:Q4_K_M".to_string(),
            "/models/unload owner/repo:Q4_K_M".to_string()
        ]
    );
    assert!(rig.host.notices().is_empty());
    assert!(rig.host.refreshes().is_empty());
}

/// The search box is wired to the Hugging Face client: it searches GGUF repositories by downloads.
#[tokio::test]
async fn download_search_queries_hugging_face() {
    let (rig, hugging_face) =
        download_rig(json!([{ "id": "owner/repo", "downloads": 42 }]), vec![]).await;
    *rig.ui.search_query.lock().unwrap() = Some("qwen".to_string());
    rig.flow.download_model(&rig.ui()).await.unwrap();

    assert_eq!(
        rig.ui.search_result.lock().unwrap().clone(),
        Some(Ok(vec![HuggingFaceModel {
            id: "owner/repo".to_string(),
            downloads: 42.0
        }]))
    );
    assert_eq!(
        hugging_face.requests()[0].0,
        "/api/models?search=qwen&filter=gguf&sort=downloads&direction=-1&limit=20"
    );
}

/// The Hugging Face token (`HF_TOKEN`) is sent with the requests, and without one no credential is.
#[tokio::test]
async fn download_sends_the_hugging_face_token() {
    for (environment, expected) in [
        (
            BTreeMap::from([("HF_TOKEN".to_string(), "hf_secret".to_string())]),
            Some("Bearer hf_secret".to_string()),
        ),
        (BTreeMap::new(), None),
    ] {
        let hugging_face = FakeHuggingFace::spawn(json!([]), vec![]).await;
        let rig = Rig::with(
            vec![],
            RigOptions {
                hugging_face: Some(hugging_face.url.clone()),
                environment,
                ..RigOptions::default()
            },
        )
        .await;
        *rig.ui.search_query.lock().unwrap() = Some("qwen".to_string());
        rig.flow.download_model(&rig.ui()).await.unwrap();
        assert_eq!(
            hugging_face.requests()[0].1.get("authorization").cloned(),
            expected
        );
    }
}

/// A failing details request is the command's error, with Hugging Face's message.
#[tokio::test]
async fn a_failing_details_request_is_reported() {
    let (rig, _hugging_face) = download_rig(json!([]), vec![]).await;
    *rig.ui.search_answer.lock().unwrap() = Some("owner/unknown".to_string());
    let error = error_of(rig.flow.download_model(&rig.ui()).await);
    assert_eq!(error.message(), "not found");
    assert!(rig.posts().is_empty());
}

// =================================================================================================
// The extension: registration and the command handler
// =================================================================================================

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: PathBuf::from("."),
    }
}

/// Records the live providers the host hands its model registry.
#[derive(Default)]
struct Sink {
    events: Mutex<Vec<String>>,
    live: Mutex<BTreeMap<String, Arc<dyn Provider>>>,
}

impl Sink {
    fn live(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.live.lock().unwrap().get(id).cloned()
    }

    fn model_ids(&self, id: &str) -> Vec<String> {
        self.live(id)
            .map(|provider| {
                provider
                    .models()
                    .iter()
                    .map(|model| model.id.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl ModelRegistrySink for Sink {
    fn upsert_provider(&self, reg: &ProviderRegistration) {
        self.events
            .lock()
            .unwrap()
            .push(format!("upsert:{}", reg.id));
    }

    fn upsert_live_provider(&self, id: &str, provider: Arc<dyn Provider>) {
        self.live.lock().unwrap().insert(id.to_string(), provider);
        self.events
            .lock()
            .unwrap()
            .push(format!("upsert-live:{id}"));
    }

    fn remove_provider(&self, id: &str) {
        self.live.lock().unwrap().remove(id);
        self.events.lock().unwrap().push(format!("remove:{id}"));
    }
}

fn extension(endpoints: Endpoints) -> Arc<LlamaExtension> {
    Arc::new(LlamaExtension::new(PathBuf::from("/nonexistent/agent")).with_endpoints(endpoints))
}

/// The extension as the host leaves it before `init`: services and registrar bound.
fn bound_extension(host: &Arc<FakeHost>) -> Arc<LlamaExtension> {
    let ext = extension(endpoints());
    ext.set_host_services(host.clone());
    ext.set_late_registrar(Arc::new(FakeRegistrar {
        owner: ExtensionId::from("llama.cpp"),
        providers: Arc::clone(&host.registered),
    }));
    ext
}

fn endpoints() -> Endpoints {
    Endpoints {
        http: Some(http()),
        hugging_face_url: None,
        environment: Some(BTreeMap::new()),
    }
}

fn auth_for(server: &FakeLlamaServer) -> HostProviderAuth {
    HostProviderAuth {
        api_key: Some("secret".to_string()),
        base_url: Some(format!("{}/v1", server.url())),
        env: BTreeMap::from([("LLAMA_BASE_URL".to_string(), server.url().to_string())]),
        source: Some("stored credential".to_string()),
    }
}

fn tui_ctx() -> HostCtx {
    HostCtx::command(ExtMode::Tui, true, PathBuf::from("."))
}

/// Port of "registers a native provider and /llama command" (`llama-extension.test.ts:42-54`),
/// through a real host: one live provider `llama.cpp` reaches the model registry and the command
/// `llama` is registered with its description; the extension is hidden and ambient and does not
/// vote on project trust.
#[tokio::test]
async fn registers_the_llama_provider_and_the_llama_command() {
    let host = ExtensionHost::new(cfg());
    let sink = Arc::new(Sink::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();
    let ext = extension(endpoints());
    host.load_native_with_services(ext.clone(), FakeHost::standalone())
        .await
        .unwrap();

    let provider = sink.live("llama.cpp").expect("the provider was registered");
    assert_eq!(provider.id().as_str(), "llama.cpp");
    assert!(provider.models().is_empty(), "a fresh catalog is empty");
    assert_eq!(
        *sink.events.lock().unwrap(),
        vec!["upsert-live:llama.cpp".to_string()]
    );
    let commands = host.registry().command_descriptions().unwrap();
    let llama = commands
        .iter()
        .find(|(name, _)| name == "llama")
        .expect("the /llama command is registered");
    assert_eq!(llama.1.description, "Manage llama.cpp router models");

    let id = ExtensionId::from("llama.cpp");
    assert_eq!(ext.id(), id);
    assert!(host.is_extension_hidden(&id));
    assert!(ext.is_ambient());
    assert!(!ext.decides_project_trust());
}

/// `ctx.mode !== "tui"` (`index.ts:186-189`): in any other mode the command warns and does nothing
/// else, not even reading the provider's auth.
#[tokio::test]
async fn the_command_warns_outside_the_interactive_mode() {
    for mode in [ExtMode::Rpc, ExtMode::Json, ExtMode::Print] {
        let host = FakeHost::standalone();
        let ext = extension(endpoints());
        ext.set_host_services(host.clone());
        let ctx = HostCtx::command(mode, false, PathBuf::from("."));
        let out = ext.execute_command("llama", "", &ctx).await.unwrap();
        assert_eq!(out, None);
        assert_eq!(
            host.messages(),
            vec![(
                "/llama is available in interactive mode".to_string(),
                NotifyKind::Warning
            )],
            "{mode:?}"
        );
        assert_eq!(host.auth_calls.load(Ordering::SeqCst), 0, "{mode:?}");
        assert!(!host.opened.load(Ordering::SeqCst), "{mode:?}");
    }
}

/// No provider auth: tell the user how to configure it (`index.ts:31-34`).
#[tokio::test]
async fn the_command_asks_to_configure_when_the_provider_has_no_auth() {
    let host = FakeHost::standalone();
    let ext = extension(endpoints());
    ext.set_host_services(host.clone());
    ext.execute_command("llama", "", &tui_ctx()).await.unwrap();
    assert_eq!(
        host.messages(),
        vec![(
            "Configure llama.cpp with /login llama.cpp".to_string(),
            NotifyKind::Warning
        )]
    );
    assert!(!host.opened.load(Ordering::SeqCst));
}

/// A failure reading the auth leaves the handler as an error (`await ctx.modelRegistry
/// .getProviderAuth(..)` rejects, `index.ts:30`); the command runner reports it as
/// `command:llama: <message>`. The handler issues no notice of its own, and `Display` is the bare
/// message, so the runner's text is pi's.
#[tokio::test]
async fn the_command_reports_an_auth_failure() {
    let host = FakeHost::standalone();
    *host.auth.lock().unwrap() = Err("credential store exploded".to_string());
    let ext = extension(endpoints());
    ext.set_host_services(host.clone());
    let error = ext
        .execute_command("llama", "", &tui_ctx())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "credential store exploded");
    assert!(host.messages().is_empty(), "the handler notifies nothing");
    assert!(!host.opened.load(Ordering::SeqCst));
}

/// `LLAMA_BASE_URL` of the auth environment names the server, even when `auth.baseUrl` names
/// another; the stored key is sent (`index.ts:35-39`). The list closes on Escape.
#[tokio::test]
async fn the_command_prefers_the_environment_url_and_sends_the_key() {
    let server = FakeLlamaServer::with_models(vec![model("a", "loaded")]).await;
    server.require_bearer("secret");
    let host = FakeHost::standalone().with_script(vec![(
        "Download model…",
        vec![press(OverlayKeyCode::Escape)],
    )]);
    let mut auth = auth_for(&server);
    auth.base_url = Some("http://127.0.0.1:9/v1".to_string());
    host.set_auth(auth);
    let ext = bound_extension(&host);
    ext.execute_command("llama", "", &tui_ctx()).await.unwrap();

    let reads = server.requests_to("GET", "/models");
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].header("authorization"), Some("Bearer secret"));
    assert!(
        host.saw_frame_with("Download model…"),
        "the model list was shown"
    );
    assert_eq!(
        host.refreshes(),
        vec![RefreshCall {
            provider: "llama.cpp".to_string(),
            allow_network: true,
            catalog: vec!["a".to_string()],
        }]
    );
}

/// Without an environment URL, `auth.baseUrl` is the server (its `/v1` is dropped by
/// normalisation); an empty environment value does not count.
#[tokio::test]
async fn the_command_falls_back_to_the_auth_base_url() {
    let server = FakeLlamaServer::with_models(vec![model("a", "loaded")]).await;
    let host = FakeHost::standalone().with_script(vec![(
        "Download model…",
        vec![press(OverlayKeyCode::Escape)],
    )]);
    let mut auth = auth_for(&server);
    auth.env = BTreeMap::from([("LLAMA_BASE_URL".to_string(), String::new())]);
    host.set_auth(auth);
    let ext = bound_extension(&host);
    ext.execute_command("llama", "", &tui_ctx()).await.unwrap();
    assert_eq!(server.requests_to("GET", "/models").len(), 1);
    assert!(
        host.saw_frame_with("Download model…"),
        "the model list was shown"
    );
}

/// A server URL that cannot be used leaves the handler as an error (`normalizeLlamaServerUrl`
/// throws, `index.ts:30-40`), and no overlay opens.
#[tokio::test]
async fn the_command_reports_an_unusable_server_url() {
    let host = FakeHost::standalone();
    host.set_auth(HostProviderAuth {
        api_key: None,
        base_url: Some("file:///tmp/llama".to_string()),
        env: BTreeMap::new(),
        source: None,
    });
    let ext = extension(endpoints());
    ext.set_host_services(host.clone());
    let error = ext
        .execute_command("llama", "", &tui_ctx())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Server URL must use http or https");
    assert!(host.messages().is_empty(), "the handler notifies nothing");
    assert!(!host.opened.load(Ordering::SeqCst));
}

/// A host with no interactive surface never starts the flow, so the server is not contacted.
#[tokio::test]
async fn the_command_does_not_contact_the_server_without_a_surface() {
    let server = FakeLlamaServer::with_models(vec![model("a", "loaded")]).await;
    let mut host = FakeHost::standalone();
    Arc::get_mut(&mut host).unwrap().accept_overlay = false;
    host.set_auth(auth_for(&server));
    let ext = extension(endpoints());
    ext.set_host_services(host.clone());
    ext.execute_command("llama", "", &tui_ctx()).await.unwrap();
    assert!(server.requests().is_empty());
    assert!(host.notices().is_empty());
}

/// Only the `llama` command is this extension's.
#[tokio::test]
async fn an_unknown_command_is_refused() {
    let ext = extension(endpoints());
    ext.set_host_services(FakeHost::standalone());
    assert!(ext.execute_command("other", "", &tui_ctx()).await.is_err());
}

/// The whole path through a real host: the command is routed by name, the overlay is driven by
/// keys, the model is unloaded, and the provider the host holds follows the catalog.
#[tokio::test]
async fn the_command_runs_end_to_end_through_the_host_and_overlay() {
    let server = FakeLlamaServer::with_models(vec![model("a", "loaded")]).await;
    server.on_unload("a", vec![Step::SetModel(model("a", "unloaded"))]);
    let host_services = FakeHost::standalone().with_script(vec![
        ("Download model…", vec![press(OverlayKeyCode::Enter)]),
        ("Unload model?", vec![press(OverlayKeyCode::Enter)]),
        ("Download model…", vec![press(OverlayKeyCode::Escape)]),
    ]);
    host_services.set_auth(auth_for(&server));

    let host = ExtensionHost::new(cfg());
    let sink = Arc::new(Sink::default());
    host.registry().bind_model_registry(sink.clone()).unwrap();
    host.load_native_with_services(extension(endpoints()), host_services.clone())
        .await
        .unwrap();
    assert!(sink.model_ids("llama.cpp").is_empty());

    let outcome = host
        .execute_native_command("llama", "", &CancelToken::new())
        .await
        .unwrap()
        .expect("the llama extension owns the command");
    assert_eq!(outcome.unwrap(), None);

    assert_eq!(
        server.requests_to("POST", "/models/unload")[0].json(),
        json!({ "model": "a" })
    );
    assert_eq!(
        host_services.messages(),
        vec![("Unloaded a".to_string(), NotifyKind::Info)]
    );
    // The loaded model was registered when the command read the catalog, and dropped again once it
    // was unloaded: the host's provider follows the server.
    let events = sink.events.lock().unwrap().clone();
    assert!(
        events
            .iter()
            .filter(|e| *e == "upsert-live:llama.cpp")
            .count()
            >= 3,
        "{events:?}"
    );
    assert!(sink.model_ids("llama.cpp").is_empty());
}

/// Every catalog change reaches the registrar as a replacement provider with the new models.
#[tokio::test]
async fn the_catalog_is_registered_through_the_late_registrar() {
    let server =
        FakeLlamaServer::with_models(vec![model("a", "loaded"), model("z", "loaded")]).await;
    let host_services = FakeHost::standalone().with_script(vec![(
        "Download model…",
        vec![press(OverlayKeyCode::Escape)],
    )]);
    host_services.set_auth(auth_for(&server));
    let providers = Arc::new(Mutex::new(Vec::new()));
    let ext = extension(endpoints());
    ext.set_host_services(host_services.clone());
    ext.set_late_registrar(Arc::new(FakeRegistrar {
        owner: ExtensionId::from("llama.cpp"),
        providers: Arc::clone(&providers),
    }));
    ext.init(&mut InitApi::new()).await.unwrap();
    ext.execute_command("llama", "", &tui_ctx()).await.unwrap();

    let providers = providers.lock().unwrap();
    let last = providers.last().expect("the catalog was registered");
    let mut ids: Vec<String> = last
        .models()
        .iter()
        .map(|model| model.id.as_str().to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["a".to_string(), "z".to_string()]);
}

/// A host that never bound a registrar cannot take the catalog: the refusal is the flow's error,
/// not a silent success, and `/llama` shows its reason.
#[tokio::test]
async fn a_catalog_with_no_registrar_is_refused_with_its_reason() {
    let server = FakeLlamaServer::with_models(vec![model("a", "loaded")]).await;
    let host_services = FakeHost::standalone().with_script(vec![(
        "llama.cpp unavailable",
        vec![press(OverlayKeyCode::Escape)],
    )]);
    host_services.set_auth(auth_for(&server));
    let ext = extension(endpoints());
    ext.set_host_services(host_services.clone());
    ext.init(&mut InitApi::new()).await.unwrap();
    ext.execute_command("llama", "", &tui_ctx()).await.unwrap();

    assert!(
        host_services.saw_frame_with("the host bound no provider registrar"),
        "the dialog names the refusal"
    );
}

/// The constructor mirrors its siblings: always `Some`, and what it returns is the hidden, ambient
/// `llama.cpp` extension.
#[test]
fn the_constructor_returns_the_hidden_ambient_extension() {
    let ext = llama_extension_for_env(std::path::Path::new("/nonexistent/agent"))
        .expect("the llama extension is always attached");
    assert_eq!(ext.id(), ExtensionId::from("llama.cpp"));
    assert!(ext.is_ambient());
    assert!(ext.is_hidden());
    assert!(!ext.decides_project_trust());
}

/// `AbortSignal.timeout` has no owner to outlive; here the timer does, so dropping it must cancel
/// the token: whatever still holds it (a refresh the host detached) is working for a caller that is
/// gone, and with the timer stopped would otherwise have no deadline at all.
#[tokio::test]
async fn dropping_a_timeout_signal_cancels_its_token() {
    let signal = crate::extension::TimeoutSignal::after(Duration::from_secs(3600));
    let token = signal.token.clone();
    assert!(!token.is_cancelled());

    drop(signal);

    assert!(token.is_cancelled());
}

/// The timer itself still fires.
#[tokio::test]
async fn a_timeout_signal_cancels_its_token_when_the_delay_passes() {
    let signal = crate::extension::TimeoutSignal::after(Duration::from_millis(20));
    tokio::time::timeout(Duration::from_secs(5), signal.token.cancelled())
        .await
        .expect("the timer fires");
}

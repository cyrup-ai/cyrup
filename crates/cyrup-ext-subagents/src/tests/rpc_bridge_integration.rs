//! PB-8 — the REACHABILITY proof for the subagent RPC bridge.
//!
//! Every hop below is production code. Nothing here constructs the bridge, the tool or the
//! executor by hand: the only test-authored object is the `HostServices` impl, and its
//! `emit_event` body is the same single line the live backend's is
//! (`cyrup-session-svc/src/host_services.rs:1523-1526`).
//!
//! ```text
//! ExtensionHost::new(HostConfig{ mode, has_ui, cwd })                  (cyrup-ext/src/facade.rs)
//!   → host.load_native_with_services(SubagentsExtension, services)     (facade.rs:370)
//!       → NativeExtension::set_host_services                           (native_impl.rs)   REAL
//!       → NativeExtension::init, RegistrationMode::Full                (native_impl.rs)   REAL
//!           → InitApi::subscribe_bus(SUBAGENT_RPC_REQUEST_EVENT)       (native.rs:466)    REAL
//!           → host turns it into SharedBus::subscribe                  (facade.rs:532)    REAL
//!   → host.bus().subscribe(<client>, "subagents:rpc:v1:reply:pb8-1")   — how a client attaches
//!   → host.bus().emit("subagents:rpc:v1:request", <envelope>)          (bus.rs:89)        REAL
//!   → host.deliver_bus_events(&CancelToken::new())                     (facade.rs:1850)   REAL
//!       → BusFanout::drain_bus round 1                                 (facade.rs:2719)   REAL
//!           → SubagentsExtension::on_bus_event                         (facade.rs:2785)   THE FEATURE
//!               → SubagentTool::execute / fleet_state / async snapshot                    REAL
//!               → HostServices::emit_event → SharedBus::emit                              REAL
//!       → BusFanout::drain_bus round 2 delivers the reply to the client                   REAL
//! ```
//!
//! **The user action.** A host, an editor, or a sibling extension emits
//! `subagents:rpc:v1:request` on the inter-extension bus and reads the reply off
//! `subagents:rpc:v1:reply:<its own requestId>`. That is how a delegating agent or a graph
//! orchestrator drives cyrup's subagents without being the model.
//!
//! **Timing.** `BusFanout::drain_bus` loops rounds inside the ONE `deliver_bus_events(..)` await
//! (`facade.rs:2731`'s `MAX_ROUNDS`), so a reply emitted from inside `on_bus_event` reaches a
//! subscriber in the same call. No `sleep`, no `timeout`, no polling — except in
//! [`a_completion_is_announced_on_the_inter_extension_bus`], where what is being waited on is the
//! real `notify::PollWatcher`'s 500 ms filesystem tick and not the bus at all.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use cyrup_core::{CancelToken, ExtensionId};
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{ExtError, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostEvent};

use crate::background::async_status_snapshot::{
    ASYNC_STATUS_SNAPSHOT_KIND, ASYNC_STATUS_SNAPSHOT_VERSION,
};
use crate::extension::rpc::{
    SUBAGENT_RPC_READY_EVENT, SUBAGENT_RPC_REPLY_EVENT_PREFIX, SUBAGENT_RPC_REQUEST_EVENT,
    subagent_rpc_reply_event,
};
use crate::extension::{RegistrationMode, SubagentsExtension};
use crate::paths::Roots;
use crate::registration::SubagentExtensionConfig;

/// The session id this harness's backend reports. Asserted VERBATIM in
/// [`a_host_drives_ping_over_the_inter_extension_bus_and_gets_a_reply`]: a hardcoded reply cannot
/// satisfy it, because only a real consultation of
/// `SubagentExecutor::host_services()` can produce it.
const TEST_SESSION_ID: &str = "pb8session01";

/// `config.max_active_async_runs_per_session` for the harness — see [`sandboxed`]. Deliberately
/// not a round number and not `0` (`0` resolves to "unlimited", which renders as the same `0`
/// `empty_fleet()` hardcodes and would prove nothing).
const TEST_ASYNC_CAPACITY_LIMIT: u32 = 7;

// =================================================================================================
// The two harness objects
// =================================================================================================

/// The live capability backend, reduced to the three methods this surface reads.
///
/// `emit_event`'s body is production's, verbatim: the real
/// `LiveHostServices::emit_event` is one line — `bus.emit(topic.to_string(), payload.clone())`
/// (`crates/cyrup-session-svc/src/host_services.rs:1523-1526`) — over the SAME
/// [`cyrup_ext::bus::SharedBus`] the builder attaches at `builder.rs:1186`, before the native load
/// loop. `cyrup_ext::host::RecordingServices` cannot be reused here: it does not override
/// `emit_event`, so it would silently drop every reply.
struct BusForwardingServices {
    bus: Arc<cyrup_ext::bus::SharedBus>,
    session_file: PathBuf,
    /// Every `set_widget(key, lines, _)` this extension made, in call order. The host's own
    /// `LiveHostServices` forwards the identical triple to the terminal
    /// (`cyrup-session-svc/src/host_services.rs`); recording it is how a test sees which SLOT a
    /// document landed in, which is the whole subject of
    /// [`the_rpc_mode_machine_document_gets_its_own_slot_and_leaves_the_fleet_widget_alone`].
    widgets: Mutex<Vec<(String, Option<Vec<String>>)>>,
}

impl cyrup_ext::host::HostServices for BusForwardingServices {
    fn emit_event(&self, topic: &str, payload: &Value) {
        self.bus.emit(topic.to_string(), payload.clone());
    }

    fn session_id(&self) -> Option<String> {
        Some(TEST_SESSION_ID.to_string())
    }

    fn session_file(&self) -> Option<PathBuf> {
        Some(self.session_file.clone())
    }

    fn set_widget(
        &self,
        key: &str,
        lines: Option<&[String]>,
        _placement: cyrup_ext::host::WidgetPlacement,
    ) {
        self.widgets
            .lock()
            .unwrap()
            .push((key.to_string(), lines.map(<[String]>::to_vec)));
    }
}

/// A sibling extension that records what the bus delivered to it — i.e. exactly what an editor,
/// a host, or another extension is on the far end of this surface.
///
/// It subscribes through [`cyrup_ext::bus::SharedBus::subscribe`] AFTER load, per request, which is
/// the attach pattern [`crate::extension::rpc`]'s module doc prescribes: the bus has no prefix
/// matching, so a client names the full reply topic for the `requestId` it is about to use.
struct RecordingClient {
    id: ExtensionId,
    seen: Arc<Mutex<Vec<(String, Value)>>>,
}

impl RecordingClient {
    fn new() -> Self {
        Self {
            id: ExtensionId::from("pb8-rpc-client"),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn deliveries(&self) -> Vec<(String, Value)> {
        self.seen.lock().unwrap().clone()
    }

    /// The one delivery on `topic`, or a panic naming everything that DID arrive.
    fn one(&self, topic: &str) -> Value {
        let seen = self.deliveries();
        let matching: Vec<&(String, Value)> = seen.iter().filter(|(t, _)| t == topic).collect();
        assert_eq!(
            matching.len(),
            1,
            "expected exactly one delivery on {topic}, saw: {seen:?}"
        );
        matching[0].1.clone()
    }
}

#[async_trait::async_trait]
impl NativeExtension for RecordingClient {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    async fn on_bus_event(
        &self,
        topic: &str,
        payload: &Value,
        _ctx: &HostCtx,
    ) -> Result<(), ExtError> {
        self.seen
            .lock()
            .unwrap()
            .push((topic.to_string(), payload.clone()));
        Ok(())
    }
}

// =================================================================================================
// Harness
// =================================================================================================

/// `roots` is what keeps `init`'s startup housekeeping (and every artifact path below it) off the
/// real machine — the same sandbox `crates/cyrup-it/tests/subagents/cyrup_home_env_sandboxed_tests.rs:35`
/// uses.
fn sandboxed(home: &Path) -> SubagentExtensionConfig {
    SubagentExtensionConfig {
        roots: Roots::sandboxed(home),
        // A DISTINCTIVE capacity limit, so `fleet.topLevelAsyncCapacity` cannot be satisfied by
        // `fleet::empty_fleet()`'s hardcoded `{used: 0, limit: 0}`. Every fleet assertion below
        // leans on it: the number can only appear if `build_fleet_status` ran, passed its
        // fail-closed session gate, and was handed a capacity the dispatcher actually measured.
        max_active_async_runs_per_session: Some(TEST_ASYNC_CAPACITY_LIMIT),
        ..SubagentExtensionConfig::default()
    }
}

struct Harness {
    host: Arc<ExtensionHost>,
    client: Arc<RecordingClient>,
    extension: Arc<SubagentsExtension>,
    services: Arc<BusForwardingServices>,
    cwd: PathBuf,
    config: SubagentExtensionConfig,
    _home: tempfile::TempDir,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn start(mode: RegistrationMode) -> Self {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let config = sandboxed(home.path());

        let host = Arc::new(ExtensionHost::new(HostConfig {
            mode: ExtMode::Tui,
            has_ui: true,
            cwd: cwd.clone(),
        }));
        let services = Arc::new(BusForwardingServices {
            bus: Arc::clone(host.bus()),
            session_file: home.path().join("session.jsonl"),
            widgets: Mutex::new(Vec::new()),
        });
        let extension = Arc::new(SubagentsExtension::with_mode(
            config.clone(),
            cwd.clone(),
            mode,
        ));
        let backend: Arc<dyn cyrup_ext::host::HostServices> = Arc::clone(&services) as _;
        host.load_native_with_services(extension.clone(), backend)
            .await
            .expect("the subagents extension loads");

        let client = Arc::new(RecordingClient::new());
        host.load_native(client.clone())
            .await
            .expect("the client extension loads");

        Self {
            host,
            client,
            extension,
            services,
            cwd,
            config,
            _home: home,
            _dir: dir,
        }
    }

    /// Every `set_widget` the extension made, in call order.
    fn widgets(&self) -> Vec<(String, Option<Vec<String>>)> {
        self.services.widgets.lock().unwrap().clone()
    }

    /// The distinct widget KEYS touched, in first-touch order.
    fn widget_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for (key, _) in self.widgets() {
            if !keys.contains(&key) {
                keys.push(key.clone());
            }
        }
        keys
    }

    /// Track one live background run owned by [`TEST_SESSION_ID`], by writing the `status.json` a
    /// real runner writes and letting the PRODUCTION `JobTracker` tick load it. After this,
    /// `SubagentExecutor::fleet_state` reports one active job — which is what gives BOTH widgets
    /// something to say.
    async fn track_a_live_run(&self, run_id: &str) {
        use crate::background::{RunId, RunMode, RunPaths, RunState, RunStatus, StepState};

        let roots = crate::background::run_artifact_roots_in(&self.config.roots, &self.cwd);
        let id = RunId::from_token(run_id.to_string());
        let paths = RunPaths::for_run(&roots.async_root, &roots.results_dir, &id);
        tokio::fs::create_dir_all(&paths.run_dir)
            .await
            .expect("the run directory");

        let mut status = RunStatus::queued(id.clone(), RunMode::Single, None);
        status.state = RunState::Running;
        status.session_id = crate::identity::SessionId::parse(TEST_SESSION_ID);
        status.started_at = crate::time::now_epoch_millis();
        status.last_update = status.started_at;
        status.steps = vec![crate::background::StepStatus {
            status: StepState::Running,
            ..crate::background::StepStatus::pending("pb8worker")
        }];
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status.json");

        let tracker = self.extension.executor().tracker().clone();
        tracker.track(id, paths, None).await;
        tracker.tick_once().await;
    }

    /// Seed ONE genuinely releasable active-async capacity slot into [`TEST_SESSION_ID`]'s pool,
    /// bypassing `acquire` the way `background/active_async_capacity/tests.rs`'s `seed_slot` does.
    ///
    /// "Releasable" is the full [`crate::background::active_async_capacity::runner_release_verdict`]
    /// ladder, satisfied for real: a terminal `status.json` owned by this session and this run, an
    /// owner record whose runner pid is bound, and a pid the KERNEL reports as `ESRCH` (a spawned
    /// child that was waited on — the `background/reconcile.rs` idiom, not a stubbed probe; the
    /// production `CapacityOptions` this path builds installs no probe override, so a fake pid
    /// would be at the mercy of whatever really owns that number).
    ///
    /// That is the ONE state in which the two readers disagree: `snapshot_for` counts the slot,
    /// `reconcile_active_async_capacity` deletes it.
    ///
    /// Returns the `slot-0` directory, so the caller can ask the filesystem whether it survived.
    async fn seed_a_releasable_capacity_slot(&self, run_id: &str) -> PathBuf {
        use crate::background::active_async_capacity::{
            ActiveAsyncCapacityKind, ActiveAsyncCapacityOwner, CapacityOwnerVersion,
            session_pool_dir, slot_dir,
        };
        use crate::background::{RunId, RunMode, RunPaths, RunState, RunStatus};

        let session = crate::identity::SessionId::parse(TEST_SESSION_ID).expect("a session id");
        let roots = crate::background::run_artifact_roots_in(&self.config.roots, &self.cwd);
        let id = RunId::from_token(run_id.to_string());
        let paths = RunPaths::for_run(&roots.async_root, &roots.results_dir, &id);
        tokio::fs::create_dir_all(&paths.run_dir)
            .await
            .expect("the run directory");

        // Terminal, this session's, this run's — the first four rungs of the ladder.
        let mut status = RunStatus::queued(id.clone(), RunMode::Single, None);
        status.state = RunState::Complete;
        status.session_id = Some(session.clone());
        status.last_update = crate::time::now_epoch_millis();
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status.json");

        let owner = ActiveAsyncCapacityOwner {
            version: CapacityOwnerVersion,
            reservation_token: "pb8-capacity-token".to_string(),
            owner_session_id: session.clone(),
            owner_session_key: crate::identity::IndexSegment::encode(session.as_str()).to_string(),
            slot: 0,
            run_id: id,
            source_run_id: None,
            generation: 0,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: paths.run_dir.clone(),
            reserved_at: 0,
            // THE PROOF rung: a bound runner pid the kernel says is gone.
            runner_pid: Some(a_reaped_pid()),
            runner_started_at: Some(1),
        };
        let pool = session_pool_dir(
            &crate::background::active_async_capacity_root_in(&self.config.roots),
            &session,
        );
        let dir = slot_dir(&pool, owner.slot);
        tokio::fs::create_dir_all(&dir).await.expect("the slot");
        crate::background::atomic::write_atomic_json(
            &dir.join(crate::background::active_async_capacity::key::OWNER_FILE),
            &owner,
        )
        .await
        .expect("write owner.json");
        assert!(dir.is_dir(), "the seeded slot is on disk before the read");
        dir
    }

    /// The LAST `set_widget` call made against `key`, or `None` when the slot was never touched.
    /// The outer `Option` is "was it touched"; the inner one is upstream's `undefined` — a CLEAR.
    fn last_widget(&self, key: &str) -> Option<Option<Vec<String>>> {
        self.widgets()
            .into_iter()
            .rfind(|(k, _)| k == key)
            .map(|(_, lines)| lines)
    }

    async fn full() -> Self {
        Self::start(RegistrationMode::Full).await
    }

    /// Attach to a reply topic the way the module doc says a client must: the FULL topic, before
    /// the request is emitted.
    fn listen(&self, topic: &str) {
        self.host
            .bus()
            .subscribe(self.client.id.clone(), topic.to_string());
    }

    /// Emit one request and drain. The reply is delivered inside this one call.
    async fn request(&self, envelope: Value) {
        self.host
            .bus()
            .emit(SUBAGENT_RPC_REQUEST_EVENT.to_string(), envelope);
        self.host.deliver_bus_events(&CancelToken::new()).await;
    }

    /// Subscribe to `pb8-<n>`'s reply topic, emit the request, drain, and return the reply.
    async fn round_trip(&self, request_id: &str, method: &str, params: Option<Value>) -> Value {
        let topic = subagent_rpc_reply_event(request_id);
        self.listen(&topic);
        let mut envelope = json!({
            "version": 1,
            "requestId": request_id,
            "method": method,
        });
        if let Some(params) = params {
            envelope["params"] = params;
        }
        self.request(envelope).await;
        self.client.one(&topic)
    }
}

/// A pid that is GENUINELY dead — a real child, spawned and reaped, so `kill(pid, 0)` returns
/// `ESRCH` as an OS fact. Verbatim the idiom `background/reconcile.rs`'s
/// `spawn_and_reap_dead_pid` and `background/active_async_capacity/tests.rs`'s `reaped_pid` use,
/// duplicated rather than shared because both are `#[cfg(test)]`-private to their own modules.
///
/// It has to be real here: [`Harness::seed_a_releasable_capacity_slot`] drives the PRODUCTION
/// `SubagentExecutor::capacity_options` (`extension/executor/background.rs:386-401`), which
/// installs no `with_pid_liveness` override, so the verdict ladder probes the actual kernel.
fn a_reaped_pid() -> u32 {
    let mut child = std::process::Command::new("true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("`true` spawns");
    let pid = child.id();
    let status = child.wait().expect("wait reaps the child");
    assert!(status.success(), "`true` must exit 0");
    pid
}

/// The `error.code` of a failed reply, with the whole reply in the panic message.
fn error_code(reply: &Value) -> String {
    assert_eq!(
        reply["success"],
        Value::Bool(false),
        "expected a failure reply: {reply}"
    );
    reply["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("a failure reply carries error.code: {reply}"))
        .to_string()
}

// =================================================================================================
// 1 — THE proof
// =================================================================================================

/// A host emits `ping` on the bus and gets an answer back, through the real registration, the real
/// fan-out and the real reply emit.
///
/// The load-bearing assertion is `data.session.sessionId`: it can only be produced by consulting
/// the live backend through `SubagentExecutor::host_services()`, so a hardcoded or absent reply
/// cannot satisfy it. Removing `api.subscribe_bus`, removing the `emit_event`, or replacing
/// `on_bus_event` with `Ok(())` each produce ZERO deliveries and fail at [`RecordingClient::one`].
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_drives_ping_over_the_inter_extension_bus_and_gets_a_reply() {
    let harness = Harness::full().await;
    let reply = harness.round_trip("pb8-1", "ping", None).await;

    assert_eq!(reply["version"], json!(1));
    assert_eq!(reply["requestId"], json!("pb8-1"));
    assert_eq!(reply["method"], json!("ping"));
    assert_eq!(reply["success"], json!(true));

    let data = &reply["data"];
    // pi `SUBAGENT_RPC_METHODS` (`rpc.ts:34`) — eight, in upstream's order.
    assert_eq!(
        data["methods"],
        json!([
            "ping",
            "status",
            "manage",
            "spawn",
            "steer",
            "interrupt",
            "stop",
            "resume"
        ])
    );
    assert_eq!(
        data["events"]["replyPrefix"],
        json!(SUBAGENT_RPC_REPLY_EVENT_PREFIX)
    );
    assert_eq!(data["events"]["request"], json!(SUBAGENT_RPC_REQUEST_EVENT));
    assert_eq!(data["events"]["ready"], json!(SUBAGENT_RPC_READY_EVENT));
    // §4.5's promise, and the observer that pays for it.
    assert_eq!(
        data["events"]["asyncComplete"],
        json!("subagent:async-complete")
    );
    // The killer: the LIVE backend's session id.
    assert_eq!(data["session"]["sessionId"], json!(TEST_SESSION_ID));
    assert_eq!(
        data["capabilities"]["asyncStatusSnapshot"]["kind"],
        json!(ASYNC_STATUS_SNAPSHOT_KIND)
    );
    // `pingData` must not advertise a capability nothing implements — see `rpc/ping.rs`.
    for dropped in [
        "nonRecoveringSteer",
        "launchResolvedExtensions",
        "runtimeAcknowledgedExtensions",
        "processTerminalProof",
    ] {
        assert!(
            data["capabilities"].get(dropped).is_none(),
            "{dropped} has no backing seam and must not be advertised: {data}"
        );
    }
    for dropped in ["childStatus", "processTerminal"] {
        assert!(
            data["events"].get(dropped).is_none(),
            "{dropped} is emitted by nothing and must not be advertised: {data}"
        );
    }
}

// =================================================================================================
// 2 — every request is REPLIED, never dropped
// =================================================================================================

/// A bad method, a bad version and an unusable `requestId` each produce a reply rather than
/// silence. A bridge that returned `Err` from `on_bus_event` would have its error CONTAINED by
/// `drain_bus` (`facade.rs:2740-2750`) and the caller would hang; this test sees that as zero
/// deliveries.
///
/// The `…reply:unknown` delivery is the only thing that proves `safeReplyRequestId`
/// (`rpc.ts:789-795`) was ported rather than skipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_method_and_a_bad_version_are_replied_not_dropped() {
    let harness = Harness::full().await;

    let reply = harness.round_trip("pb8-2", "teleport", None).await;
    assert_eq!(error_code(&reply), "unsupported_method");
    // `errorReply` (`rpc.ts:799-801`) only echoes a method it recognises.
    assert!(reply.get("method").is_none(), "{reply}");

    let topic = subagent_rpc_reply_event("pb8-3");
    harness.listen(&topic);
    harness
        .request(json!({ "version": 2, "requestId": "pb8-3", "method": "ping" }))
        .await;
    let reply = harness.client.one(&topic);
    assert_eq!(error_code(&reply), "unsupported_version");
    assert_eq!(reply["method"], json!("ping"));

    // A `requestId` carrying a newline could forge a second reply topic, so it is rejected at the
    // very top (`rpc.ts:773`) and answered on the literal fallback topic.
    let unknown = subagent_rpc_reply_event("unknown");
    harness.listen(&unknown);
    harness
        .request(json!({ "version": 1, "requestId": "bad\nid", "method": "ping" }))
        .await;
    let reply = harness.client.one(&unknown);
    assert_eq!(error_code(&reply), "invalid_request");
    assert_eq!(reply["requestId"], json!("unknown"));
    assert!(
        !harness
            .client
            .deliveries()
            .iter()
            .any(|(t, _)| t.contains('\n')),
        "a forged reply topic must never be emitted"
    );
}

// =================================================================================================
// 3 + 4 — the UW-21 proof, on BOTH status tiers
// =================================================================================================

/// An untargeted `status` takes the IN-MEMORY tier (`rpc.ts:721-736`) and carries both public
/// projections.
///
/// `data.asyncSnapshot` is producible ONLY by
/// `background::async_status_snapshot::build_async_status_snapshot_for_state` — the call that had
/// no production caller at all before this task. The assertion is on the KIND constant, not on
/// "some object is present", so gutting the call cannot pass it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_rpc_status_carries_the_fleet_and_the_async_status_snapshot() {
    let harness = Harness::full().await;
    let reply = harness.round_trip("pb8-4", "status", None).await;
    assert_eq!(reply["success"], json!(true), "{reply}");
    let data = &reply["data"];

    // The in-memory tier's own summary sentence (`rpc.ts:426-429`) — this is what proves the fast
    // path, not the executor, answered.
    assert_eq!(
        data["text"],
        json!("In-memory subagent status: 0 active children.")
    );
    assert_eq!(data["details"]["mode"], json!("management"));

    // The fleet half, asserted on values only the REAL projection can produce. `version: 1`,
    // `entries: []` and a numeric capacity are all satisfied verbatim by `fleet::empty_fleet()`,
    // which is a hardcoded literal — so the load-bearing assertion is the LIMIT: `empty_fleet()`
    // reports `{used: 0, limit: 0}`, and only a projection that passed the fail-closed session
    // gate and was handed the dispatcher's measurement reports the configured 7.
    assert_eq!(data["fleet"]["version"], json!(1));
    assert_eq!(
        data["fleet"]["topLevelAsyncCapacity"],
        json!({ "used": 0, "limit": TEST_ASYNC_CAPACITY_LIMIT }),
        "a hardcoded empty fleet would report limit 0: {data}"
    );
    assert_eq!(
        data["asyncSnapshot"]["kind"],
        json!(ASYNC_STATUS_SNAPSHOT_KIND)
    );
    assert_eq!(
        data["asyncSnapshot"]["version"],
        json!(ASYNC_STATUS_SNAPSHOT_VERSION)
    );
}

/// The same untargeted `status`, with a LIVE background run in the session: the fleet entry and
/// the async snapshot both have to describe it.
///
/// This is the half `empty_fleet()` cannot fake at all. The entry's `key` is asserted to be the
/// OPAQUE `fleet-1` while the `agent` is the run's real step agent, which together pin the two
/// properties `rpc.ts:93` states as a contract: the projection is real, and the internal
/// `async:<runId>[:<offset>]` addressing never crosses the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_rpc_status_describes_a_live_run_without_leaking_its_id() {
    let harness = Harness::full().await;
    harness.track_a_live_run("pb8liverunaaaaaa").await;

    let reply = harness.round_trip("pb8-20", "status", None).await;
    assert_eq!(reply["success"], json!(true), "{reply}");
    let data = &reply["data"];

    assert_eq!(
        data["text"],
        json!("In-memory subagent status: 1 active child."),
        "the in-memory summary counts the live child, with pi's SINGULAR noun: {data}"
    );
    assert_eq!(data["fleet"]["totalActive"], json!(1), "{data}");
    let entries = data["fleet"]["entries"]
        .as_array()
        .expect("the fleet carries entries");
    assert_eq!(entries.len(), 1, "{data}");
    assert_eq!(
        entries[0]["key"],
        json!("fleet-1"),
        "the key is the opaque alias, never the run id"
    );
    assert_eq!(entries[0]["agent"], json!("pb8worker"), "{data}");
    assert!(
        !data["fleet"].to_string().contains("pb8liverunaaaaaa"),
        "`rpc.ts:93`: the entry key is never a run identifier — {}",
        data["fleet"]
    );

    // The snapshot half names the run, because THAT document is the addressable one.
    assert_eq!(
        data["asyncSnapshot"]["kind"],
        json!(ASYNC_STATUS_SNAPSHOT_KIND)
    );
    assert!(
        data["asyncSnapshot"]
            .to_string()
            .contains("pb8liverunaaaaaa"),
        "the async snapshot describes the live run: {}",
        data["asyncSnapshot"]
    );
}

/// `status {view:"fleet"}` makes `hasStatusTarget` (`rpc.ts:406-413`) true, so the request takes
/// the EXECUTOR tier (`rpc.ts:738-754`) — through the real `SubagentTool::execute`.
///
/// Wiring the snapshot into only the in-memory fast path — the easy half-job — fails here.
/// Upstream attaches both projections to both tiers and so must cyrup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_rpc_status_with_a_target_still_carries_the_fleet_and_snapshot() {
    let harness = Harness::full().await;
    let reply = harness
        .round_trip("pb8-5", "status", Some(json!({ "view": "fleet" })))
        .await;
    assert_eq!(reply["success"], json!(true), "{reply}");
    let data = &reply["data"];

    // NOT the in-memory sentence: this answer came from `control_status_view`'s fleet branch.
    assert_ne!(
        data["text"],
        json!("In-memory subagent status: 0 active children.")
    );
    assert!(data["text"].is_string(), "{data}");
    // Same discriminator as the in-memory tier: `version: 1` is `empty_fleet()`'s own literal, the
    // configured limit is not.
    assert_eq!(data["fleet"]["version"], json!(1));
    assert_eq!(
        data["fleet"]["topLevelAsyncCapacity"],
        json!({ "used": 0, "limit": TEST_ASYNC_CAPACITY_LIMIT }),
        "the executor tier carries the SAME measured projection, not a hardcoded blank: {data}"
    );
    assert_eq!(
        data["asyncSnapshot"]["kind"],
        json!(ASYNC_STATUS_SNAPSHOT_KIND)
    );
    assert_eq!(
        data["asyncSnapshot"]["version"],
        json!(ASYNC_STATUS_SNAPSHOT_VERSION)
    );
}

/// A `status` read COUNTS the session's capacity pool; it does not SWEEP it.
///
/// `extension/rpc/mod.rs`'s `active_async_capacity` takes `capacity::snapshot_for`
/// (`background/active_async_capacity/sweep.rs:25-48`, *"counts, does not reconcile"*) rather than
/// `capacity::get_active_async_capacity_snapshot` (`sweep.rs:105-119`), which is a bare alias of
/// `reconcile_active_async_capacity` and DELETES every slot it can prove released. `status` is the
/// one method an external bus client may drive in a loop, and upstream's is a plain field read on
/// the live `SubagentState` (`src/extension/rpc.ts:301` @`v0.68.0`) — a read with no side effect.
/// Sweeping belongs to the admission path, where `acquire` already runs it (§D2,
/// `background/active_async_capacity/mod.rs`).
///
/// The harness seeds ONE genuinely releasable slot — the single state in which the two readers
/// disagree — and this test pins BOTH halves of that disagreement:
///
/// * the reported `used` is `1`, the count of what is in the pool, not `0`, the count of what a
///   sweep would have left behind; and
/// * `slot-0` is still on disk afterwards, which is the state mutation itself rather than a number
///   derived from it.
///
/// Swapping the call back to the reconciling variant fails on both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_rpc_status_counts_the_capacity_pool_and_does_not_sweep_it() {
    let harness = Harness::full().await;
    let slot = harness
        .seed_a_releasable_capacity_slot("pb8sweeprunaaaaa")
        .await;

    let reply = harness.round_trip("pb8-30", "status", None).await;
    assert_eq!(reply["success"], json!(true), "{reply}");
    let data = &reply["data"];

    assert_eq!(
        data["fleet"]["topLevelAsyncCapacity"],
        json!({ "used": 1, "limit": TEST_ASYNC_CAPACITY_LIMIT }),
        "a counting read reports the occupied slot; a reconciling one would have deleted it and \
         reported `used: 0`: {data}"
    );
    assert!(
        slot.is_dir(),
        "the `status` read swept the slot pool — {} is gone, so a bus client polling `status` in \
         a loop is driving slot deletion",
        slot.display()
    );
    assert!(
        slot.join(crate::background::active_async_capacity::key::OWNER_FILE)
            .is_file(),
        "the owner record must survive a read: {}",
        slot.display()
    );
}

// =================================================================================================
// 5 — the RPC-local spawn gates
// =================================================================================================

/// `spawn` refuses an `action` (`rpc.ts:518-520`) and refuses a foreground launch
/// (`rpc.ts:521-523`), with upstream's own sentences.
///
/// Dropping either guard turns this surface into a way to run a management/control verb under a
/// method that claims to launch, or into a BLOCKING foreground launch on a fire-and-forget
/// channel. Both refusals happen in the bridge, ahead of dispatch, so nothing is spawned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_spawn_refuses_an_action_and_refuses_a_non_detached_launch() {
    let harness = Harness::full().await;

    let reply = harness
        .round_trip(
            "pb8-6",
            "spawn",
            Some(json!({ "agent": "x", "action": "steer" })),
        )
        .await;
    assert_eq!(error_code(&reply), "invalid_params");
    assert_eq!(
        reply["error"]["message"],
        json!(
            "RPC spawn does not accept management/control actions. Use status or interrupt RPC \
             methods instead."
        )
    );

    let reply = harness
        .round_trip(
            "pb8-7",
            "spawn",
            Some(json!({ "agent": "x", "async": false })),
        )
        .await;
    assert_eq!(error_code(&reply), "invalid_params");
    assert_eq!(
        reply["error"]["message"],
        json!("RPC spawn only supports detached async launches; omit async or set async: true.")
    );
}

// =================================================================================================
// 6 — the ready announcement
// =================================================================================================

/// `SessionStart` announces the surface on [`SUBAGENT_RPC_READY_EVENT`], carrying the same document
/// a `ping` reply carries.
///
/// Driven through the REAL `NativeExtension::on_event`, the same call the host dispatcher makes.
/// Remove the `emit_rpc_ready()` line from the `SessionStart` arm and nothing is delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bridge_announces_ready_on_session_start() {
    let harness = Harness::full().await;
    harness.listen(SUBAGENT_RPC_READY_EVENT);

    harness
        .extension
        .on_event(
            &HostEvent::SessionStart {
                reason: "test".to_string(),
                previous_session_file: None,
            },
            &HostCtx::event(ExtMode::Tui, true, harness.cwd.clone()),
        )
        .await;
    harness.host.deliver_bus_events(&CancelToken::new()).await;

    let ready = harness.client.one(SUBAGENT_RPC_READY_EVENT);
    assert_eq!(
        ready["methods"],
        json!([
            "ping",
            "status",
            "manage",
            "spawn",
            "steer",
            "interrupt",
            "stop",
            "resume"
        ])
    );
    assert_eq!(ready["events"]["ready"], json!(SUBAGENT_RPC_READY_EVENT));
    assert_eq!(ready["session"]["sessionId"], json!(TEST_SESSION_ID));
}

// =================================================================================================
// 7 — the child-safe gate
// =================================================================================================

/// A [`RegistrationMode::ChildSafe`] fanout child subscribes to NO bus topic, so it answers no RPC
/// at all.
///
/// Subscribing from the wrong arm of `match self.mode` would let a fanout child expose the
/// orchestrator's spawn/stop/manage surface on the bus — from inside a child, where the whole point
/// of the mode is that it cannot orchestrate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_safe_registration_answers_no_rpc() {
    // The POSITIVE control first, on the identical envelope: without it, "nothing was delivered"
    // is equally satisfied by the whole feature being deleted, and this test would survive a
    // gutting it exists to prevent.
    let full = Harness::full().await;
    full.listen(&subagent_rpc_reply_event("pb8-8"));
    full.request(json!({ "version": 1, "requestId": "pb8-8", "method": "ping" }))
        .await;
    let answered = full.client.one(&subagent_rpc_reply_event("pb8-8"));
    assert_eq!(answered["success"], json!(true), "{answered}");
    assert_eq!(
        answered["data"]["session"]["sessionId"],
        json!(TEST_SESSION_ID)
    );

    // Same request, same envelope, a ChildSafe registration: silence.
    let harness = Harness::start(RegistrationMode::ChildSafe).await;
    harness.listen(&subagent_rpc_reply_event("pb8-8"));
    harness
        .request(json!({ "version": 1, "requestId": "pb8-8", "method": "ping" }))
        .await;
    assert!(
        harness.client.deliveries().is_empty(),
        "a child-safe registration must answer nothing: {:?}",
        harness.client.deliveries()
    );
}

// =================================================================================================
// 8 — the completion announcement (§4.5)
// =================================================================================================

/// A background completion observed by the PRODUCTION completion watcher is republished on the
/// inter-extension bus, so `pingData`'s `events.asyncComplete` advertisement is not a lie and a
/// delegating host does not have to poll `status` in a loop.
///
/// The watcher is installed through the production `install_completion_watcher` — the same call
/// `native_impl.rs`'s `SessionStart` arm makes — so the composite observer under test is the
/// production one. Remove the fifth member and this test sees nothing.
///
/// The bounded wait is for the real `notify::PollWatcher`'s
/// [`crate::background::watch::RESULTS_DIR_POLL_INTERVAL`] tick, not for the bus: the same wait
/// `background/watch/install.rs:771-773` already uses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_completion_is_announced_on_the_inter_extension_bus() {
    use crate::background::watch::SUBAGENT_ASYNC_COMPLETE_EVENT;
    use crate::background::watch::tests::{child_result, publish_result, result_with_children};
    use crate::background::{ResultFile, RunState};

    let harness = Harness::full().await;
    harness.listen(SUBAGENT_ASYNC_COMPLETE_EVENT);

    // The production install — `SubagentExecutor::install_completion_watcher`, which builds the
    // five-member composite in `extension/executor/notices.rs`.
    let executor = harness.extension.executor();
    executor.install_completion_watcher(&harness.cwd).await;
    let results_dir =
        crate::background::run_artifact_roots_in(&harness.config.roots, &harness.cwd).results_dir;

    // A completing background run's terminal `ResultFile` — the runner's last file-writing act
    // (R-SA-077), and the only signal the orchestrator half ever gets.
    let mut result = result_with_children(
        "pb8completion",
        RunState::Complete,
        true,
        None,
        vec![child_result("worker", Some("all done"), 0)],
    );
    // The delivery-ownership pair this instance actually holds: the harness backend's session and
    // THIS process's completion-owner id. Anything else is another instance's result and is
    // deliberately never consumed.
    result.session_id = crate::identity::SessionId::parse(TEST_SESSION_ID);
    result.completion_owner_id = Some(crate::identity::current_completion_owner_id());
    let result: ResultFile = result;
    publish_result(&results_dir, &result).await;

    // Bounded wait on the FILESYSTEM tick, then one drain for the bus.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        harness.host.deliver_bus_events(&CancelToken::new()).await;
        if !harness.client.deliveries().is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the completion was never announced on {SUBAGENT_ASYNC_COMPLETE_EVENT}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    let announced = harness.client.deliveries();
    let (topic, payload) = &announced[0];
    assert_eq!(topic, SUBAGENT_ASYNC_COMPLETE_EVENT);
    assert_eq!(payload["runId"], json!("pb8completion"));
    assert_eq!(payload["success"], json!(true));
}

// =================================================================================================
// 9 — the five methods the first pass left untested, end to end over the bus
// =================================================================================================

/// `manage` (`rpc.ts:712-714`, `manageParams` at `:486-512`): the SEVEN-action allowlist and the
/// `id` requirement, asserted on the wire rather than only at the normalizer.
///
/// The allowlist is a narrowing upstream chose deliberately — `schedule.create` and
/// `schedule.run-due` ARE dispatchable by this crate's schedule tool
/// (`background/scheduled_runs/tool.rs`) — so widening it here would silently hand a bus client
/// two verbs upstream withholds. The refusal SENTENCE is asserted verbatim because it is the
/// contract an integrator reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_manage_enforces_the_seven_action_allowlist_and_the_id_requirement() {
    let harness = Harness::full().await;
    let allowlist = crate::extension::rpc::SUBAGENT_RPC_MANAGEMENT_ACTIONS.join(", ");

    for (n, rejected) in ["schedule.create", "schedule.run-due", "stop", ""]
        .into_iter()
        .enumerate()
    {
        let reply = harness
            .round_trip(
                &format!("pb8-manage-{n}"),
                "manage",
                Some(json!({ "action": rejected, "id": "s1" })),
            )
            .await;
        assert_eq!(error_code(&reply), "invalid_params");
        assert_eq!(
            reply["error"]["message"],
            json!(format!("RPC manage action must be one of: {allowlist}.")),
            "`{rejected}` must not be dispatchable over RPC"
        );
    }

    // Six of the seven require an `id`, and the refusal names the action.
    let reply = harness
        .round_trip(
            "pb8-manage-id",
            "manage",
            Some(json!({ "action": "schedule.delete" })),
        )
        .await;
    assert_eq!(error_code(&reply), "invalid_params");
    assert_eq!(
        reply["error"]["message"],
        json!("RPC manage schedule.delete requires id.")
    );

    // `schedule.list` is the one that does not — and it REACHES the tool: the answer below is the
    // SCHEDULER's own refusal (`scheduledRuns.enabled` defaults off in this sandbox), a sentence
    // the bridge has no way to produce. That is the proof the `manage` arm dispatches rather than
    // merely validating.
    let reply = harness
        .round_trip(
            "pb8-manage-list",
            "manage",
            Some(json!({ "action": "schedule.list" })),
        )
        .await;
    assert_eq!(error_code(&reply), "execution_failed", "{reply}");
    assert_eq!(
        reply["error"]["message"],
        json!("Scheduled runs are disabled by scheduledRuns.enabled=false."),
        "the reply came from the scheduled-runs tool, not from the RPC normalizer: {reply}"
    );
}

/// `steer` (`rpc.ts:756-758`, `steerParams` at `:527-541`) — the message gate, the target gate and
/// the three-value mode allowlist, on the wire.
///
/// The final case is the one that proves the method is WIRED rather than merely validated: a
/// well-formed steer at a run that does not exist gets `execution_failed`, which can only come
/// from `SubagentTool::execute` having actually been entered (`executeChecked`, `:472-484`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_steer_gates_message_target_and_mode_then_dispatches() {
    let harness = Harness::full().await;

    let reply = harness
        .round_trip(
            "pb8-steer-1",
            "steer",
            Some(json!({ "id": "r", "message": "  " })),
        )
        .await;
    assert_eq!(error_code(&reply), "invalid_params");
    assert_eq!(
        reply["error"]["message"],
        json!("RPC steer requires a non-empty message.")
    );

    let reply = harness
        .round_trip("pb8-steer-2", "steer", Some(json!({ "message": "go" })))
        .await;
    assert_eq!(
        reply["error"]["message"],
        json!("RPC steer requires id, runId, or dir.")
    );

    let reply = harness
        .round_trip(
            "pb8-steer-3",
            "steer",
            Some(json!({ "message": "go", "id": "r", "mode": "shout" })),
        )
        .await;
    assert_eq!(
        reply["error"]["message"],
        json!("RPC steer mode must be steer, follow_up, or auto.")
    );

    let reply = harness
        .round_trip(
            "pb8-steer-4",
            "steer",
            Some(json!({ "message": "go", "id": "pb8nosuchrun0001", "mode": "auto" })),
        )
        .await;
    assert_eq!(
        error_code(&reply),
        "execution_failed",
        "a well-formed steer must reach the tool, not stop at the bridge: {reply}"
    );
}

/// `resume` (`rpc.ts:765-767`, `resumeParams` at `:543-559`) — including the restriction the task
/// calls out: `outputMode` may ONLY be `"file-only"`.
///
/// That is what keeps an RPC resume from being asked to stream a run's output back through a
/// channel with no reader: the result goes to a file and the caller is told where.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_resume_gates_message_target_and_output_mode_then_dispatches() {
    let harness = Harness::full().await;

    let reply = harness
        .round_trip("pb8-res-1", "resume", Some(json!({ "id": "r" })))
        .await;
    assert_eq!(
        reply["error"]["message"],
        json!("RPC resume requires a non-empty message.")
    );

    let reply = harness
        .round_trip("pb8-res-2", "resume", Some(json!({ "message": "again" })))
        .await;
    assert_eq!(
        reply["error"]["message"],
        json!("RPC resume requires id, runId, or dir.")
    );

    for mode in ["inline", "both"] {
        let reply = harness
            .round_trip(
                &format!("pb8-res-{mode}"),
                "resume",
                Some(json!({ "message": "again", "id": "r", "outputMode": mode })),
            )
            .await;
        assert_eq!(error_code(&reply), "invalid_params");
        assert_eq!(
            reply["error"]["message"],
            json!("RPC resume supports only file-only output mode."),
            "`{mode}` is not an RPC-resume output mode"
        );
    }

    let reply = harness
        .round_trip(
            "pb8-res-3",
            "resume",
            Some(json!({ "message": "again", "id": "r", "output": "  " })),
        )
        .await;
    assert_eq!(
        reply["error"]["message"],
        json!("RPC resume output must be a non-empty path.")
    );

    let reply = harness
        .round_trip(
            "pb8-res-4",
            "resume",
            Some(json!({ "message": "again", "id": "pb8nosuchrun0001" })),
        )
        .await;
    assert_eq!(
        error_code(&reply),
        "execution_failed",
        "a well-formed resume must reach the tool: {reply}"
    );
}

/// `stop` (`rpc.ts:762-764`) and `interrupt` (`:759-761`) — the TARGET WHITELIST
/// (`normalizeTargetParams`, `:385-396`) and `stop`'s extra `childId` key.
///
/// The whitelist is the security property on these two verbs: an RPC caller names a run, it does
/// not smuggle an `agent`/`task`/`action` into a control verb and turn it into a launch. That is
/// asserted here by the *absence* of a spawn — a smuggled `agent` that reached the tool would make
/// this a LAUNCH, and the reply would not be the target-resolution failure it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_stop_and_interrupt_take_only_target_keys() {
    let harness = Harness::full().await;

    for method in ["stop", "interrupt"] {
        let reply = harness
            .round_trip(
                &format!("pb8-{method}-smuggle"),
                method,
                Some(json!({
                    "id": "pb8nosuchrun0001",
                    "agent": "pb8-smuggled-agent",
                    "task": "launch me",
                    "action": "spawn",
                    "async": true,
                })),
            )
            .await;
        assert_eq!(
            error_code(&reply),
            "execution_failed",
            "the verb reached the tool and failed on the unknown TARGET: {reply}"
        );
        assert!(
            !reply.to_string().contains("pb8-smuggled-agent"),
            "the smuggled agent must never reach the dispatched object: {reply}"
        );

        // A non-object `params` is refused by `assertRecordParams` before any of that.
        let reply = harness
            .round_trip(&format!("pb8-{method}-bad"), method, Some(json!(7)))
            .await;
        assert_eq!(error_code(&reply), "invalid_params");
        assert_eq!(
            reply["error"]["message"],
            json!(format!("RPC {method} params must be an object."))
        );
    }

    // `stop` carries `childId` on top of the whitelist (SUBA-087; upstream's own `stopAsyncRun`
    // reads the same key at `rpc.ts:567`). Against a REAL target the key is forwarded far enough
    // for `background::control::validate_stop_child_id` to answer with its own sentence — which
    // is what proves the bridge threads `childId` through rather than dropping it on the four-key
    // whitelist.
    harness.track_a_live_run("pb8stoptargetaaa").await;
    let reply = harness
        .round_trip(
            "pb8-stop-child",
            "stop",
            Some(json!({ "id": "pb8stoptargetaaa", "childId": "bad\nchild" })),
        )
        .await;
    assert_eq!(error_code(&reply), "execution_failed", "{reply}");
    assert!(
        reply["error"]["message"]
            .as_str()
            .is_some_and(|m| m.to_ascii_lowercase().contains("child")),
        "the childId gate names the field it refused: {reply}"
    );
}

/// pi `assertRecordParams` (`rpc.ts:349-353`) on the WIRE: an explicit `params: null` is a present
/// non-record and is refused, where an ABSENT `params` is `{}`.
///
/// `parseRequest` (`:784`) carries `params` through whenever it is not `undefined`, so JSON `null`
/// reaches the normalizer as a value. Folding it into `{}` would make
/// `{"method":"spawn","params":null}` a real dispatch into [`crate::extension::SubagentTool`] —
/// taking a single-dispatch slot and a spawn-budget read — where upstream answers `invalid_params`
/// without entering the tool at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_null_params_is_refused_on_the_wire() {
    let harness = Harness::full().await;

    for (n, method) in [
        "status",
        "spawn",
        "manage",
        "steer",
        "resume",
        "stop",
        "interrupt",
    ]
    .into_iter()
    .enumerate()
    {
        let topic = subagent_rpc_reply_event(&format!("pb8-null-{n}"));
        harness.listen(&topic);
        harness
            .request(json!({
                "version": 1,
                "requestId": format!("pb8-null-{n}"),
                "method": method,
                "params": Value::Null,
            }))
            .await;
        let reply = harness.client.one(&topic);
        assert_eq!(error_code(&reply), "invalid_params", "{method}: {reply}");
        assert_eq!(
            reply["error"]["message"],
            json!(format!("RPC {method} params must be an object.")),
            "`params: null` must be refused for {method}"
        );
    }

    // …and the same methods with params ABSENT are not refused for that reason.
    let reply = harness.round_trip("pb8-null-ok", "status", None).await;
    assert_eq!(
        reply["success"],
        json!(true),
        "an absent params is the empty record: {reply}"
    );
}

// =================================================================================================
// 10 — the RPC-mode widget slot (pi `renderWidget`, tui/render.ts:2991-3008)
// =================================================================================================

/// In `ExtMode::Rpc` the machine document goes to its OWN slot — pi's `WIDGET_KEY`
/// (`"subagent-async"`, `shared/types.ts:2789`) — and the always-on fleet-status widget
/// (`"subagent-fleet-status"`, `tui/fleet-status.ts:14`) keeps publishing its human rows.
///
/// These are two different widgets upstream and `renderWidget` only ever touches the first
/// (`:2995`, `:3000`, `:3007`). Publishing the snapshot into the fleet-status slot instead would
/// REPLACE the human widget for every `--acp`/`--rpc` client — and, with no async jobs, CLEAR it —
/// which is a default-on regression to a shipped feature. `cyrup_ext::HostServices::set_widget`
/// takes the key as its first argument (`cyrup-ext/src/host/services.rs:376`), so there is nothing
/// forcing them to share one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_rpc_mode_machine_document_gets_its_own_slot_and_leaves_the_fleet_widget_alone() {
    use crate::background::async_status_snapshot::{
        ASYNC_STATUS_SNAPSHOT_WIDGET_KEY, ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX,
    };
    use crate::tui::fleet_status::FLEET_STATUS_WIDGET_KEY;

    let harness = Harness::full().await;
    harness.track_a_live_run("pb8widgetrunaaaa").await;

    // The repaint edge `native_impl.rs`'s `AgentEnd` arm drives, in RPC mode with a UI — i.e.
    // exactly `cyrup --acp` (`cyrup-session-svc/src/builder.rs`'s `ext_mode()` yields
    // `(ExtMode::Rpc, has_ui = true)`).
    harness
        .extension
        .on_event(
            &HostEvent::AgentEnd {
                messages: Vec::new(),
            },
            &HostCtx::event(ExtMode::Rpc, true, harness.cwd.clone()),
        )
        .await;

    let widgets = harness.widgets();
    assert!(
        harness
            .widget_keys()
            .contains(&ASYNC_STATUS_SNAPSHOT_WIDGET_KEY.to_string()),
        "the machine document needs its own slot: {widgets:?}"
    );
    assert!(
        harness
            .widget_keys()
            .contains(&FLEET_STATUS_WIDGET_KEY.to_string()),
        "the RPC branch must NOT return past the always-on fleet-status widget: {widgets:?}"
    );

    // The machine document is on the async key, and carries the live run.
    let machine: Vec<&Vec<String>> = widgets
        .iter()
        .filter(|(key, _)| key == ASYNC_STATUS_SNAPSHOT_WIDGET_KEY)
        .filter_map(|(_, lines)| lines.as_ref())
        .collect();
    assert_eq!(machine.len(), 1, "{widgets:?}");
    assert!(
        machine[0][0].starts_with(ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX),
        "the async slot carries the prefixed JSON line: {:?}",
        machine[0]
    );
    assert!(
        machine[0][0].contains("pb8widgetrunaaaa"),
        "…describing the live run: {:?}",
        machine[0]
    );

    // The fleet-status slot carries HUMAN rows, and never the machine line.
    for (key, lines) in &widgets {
        if key != FLEET_STATUS_WIDGET_KEY {
            continue;
        }
        let Some(lines) = lines else {
            panic!("the fleet-status widget was CLEARED while a run is live: {widgets:?}");
        };
        assert!(
            !lines
                .iter()
                .any(|line| line.starts_with(ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX)),
            "the machine document must never land in the human slot: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("1 active agent")),
            "the human widget still renders the live roster: {lines:?}"
        );
    }
}

/// The EMPTY-roster half of the same defect, and the one that made it a default-on regression:
/// pi's `jobs.length === 0` branch CLEARS the widget (`tui/render.ts:2992-2997`), and a clear sent
/// to the wrong key removes the always-on fleet status outright.
///
/// An `--acp` session with live FOREGROUND work and no async runs is the common case —
/// `async_status_snapshot_jobs_for_state` reads only `state.tracked_jobs`
/// (`background/async_status_snapshot/state.rs`), never `foreground_controls` — so the empty
/// branch fires constantly there. This test pins that the clear lands on `"subagent-async"` and
/// that the fleet-status slot is not touched by this path at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_rpc_roster_clears_only_the_async_slot() {
    use crate::background::async_status_snapshot::ASYNC_STATUS_SNAPSHOT_WIDGET_KEY;
    use crate::tui::fleet_status::FLEET_STATUS_WIDGET_KEY;

    let harness = Harness::full().await;
    // Deliberately NO `track_a_live_run`: the async roster is empty.
    harness
        .extension
        .on_event(
            &HostEvent::AgentEnd {
                messages: Vec::new(),
            },
            &HostCtx::event(ExtMode::Rpc, true, harness.cwd.clone()),
        )
        .await;

    assert_eq!(
        harness.widgets(),
        vec![(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY.to_string(), None)],
        "an empty roster clears the ASYNC slot and nothing else"
    );
    assert!(
        !harness
            .widget_keys()
            .contains(&FLEET_STATUS_WIDGET_KEY.to_string()),
        "the always-on fleet-status widget must never be cleared by the async path: {:?}",
        harness.widgets()
    );
}

/// The mirror image: in `ExtMode::Tui` the async slot is never touched at all, because upstream's
/// branch is `ctx.mode === "rpc"` (`tui/render.ts:2999`) and a human gets the mounted component,
/// not a `PI_SUBAGENT_ASYNC_JSON:` line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_mode_publishes_only_the_human_fleet_widget() {
    use crate::background::async_status_snapshot::ASYNC_STATUS_SNAPSHOT_WIDGET_KEY;
    use crate::tui::fleet_status::FLEET_STATUS_WIDGET_KEY;

    let harness = Harness::full().await;
    harness.track_a_live_run("pb8tuirunaaaaaaa").await;
    harness
        .extension
        .on_event(
            &HostEvent::AgentEnd {
                messages: Vec::new(),
            },
            &HostCtx::event(ExtMode::Tui, true, harness.cwd.clone()),
        )
        .await;

    assert_eq!(
        harness.widget_keys(),
        vec![FLEET_STATUS_WIDGET_KEY.to_string()],
        "a TUI session gets exactly the one always-on widget: {:?}",
        harness.widgets()
    );
    assert!(
        !harness
            .widget_keys()
            .contains(&ASYNC_STATUS_SNAPSHOT_WIDGET_KEY.to_string())
    );
}

/// Session shutdown CLEARS BOTH of this extension's widget slots.
///
/// Upstream's cleanup block does exactly two widget removals: `fleetStatus?.dispose()`
/// (`src/extension/index.ts:1063` @`v0.68.0`, whose `dispose` clears the fleet-status key) and
/// `state.lastUiContext.ui.setWidget(WIDGET_KEY, undefined)` (`:1098`) for the async-jobs key.
/// cyrup's `SessionShutdown` arm (`extension/host/native_impl.rs`) has to make both calls,
/// because `refresh_fleet_status_widget` publishes the machine document into that second slot in
/// `ExtMode::Rpc` — so the previous `AgentEnd` repaint below is what puts a
/// `PI_SUBAGENT_ASYNC_JSON:` line in it.
///
/// Dropping the async clear does not fail loudly: it strands that line — a document describing a
/// session that no longer exists, with a run it says is live — in an `--acp` client's widget slot,
/// where it survives the session that wrote it. This test is the discriminator: it asserts the
/// LAST call on each key is a removal, so a slot left holding stale content is a failure even
/// though `set_widget` was called on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_shutdown_clears_both_widget_slots() {
    use crate::background::async_status_snapshot::{
        ASYNC_STATUS_SNAPSHOT_WIDGET_KEY, ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX,
    };
    use crate::tui::fleet_status::FLEET_STATUS_WIDGET_KEY;

    let harness = Harness::full().await;
    harness.track_a_live_run("pb8shutdownrunaa").await;

    // `cyrup --acp`'s repaint edge, which is what OCCUPIES both slots in the first place.
    harness
        .extension
        .on_event(
            &HostEvent::AgentEnd {
                messages: Vec::new(),
            },
            &HostCtx::event(ExtMode::Rpc, true, harness.cwd.clone()),
        )
        .await;
    let published = harness
        .last_widget(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY)
        .expect("the RPC repaint published the machine document")
        .expect("…as content, not a clear");
    assert!(
        published[0].starts_with(ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX)
            && published[0].contains("pb8shutdownrunaa"),
        "the async slot holds a document naming the live run before shutdown: {published:?}"
    );

    harness
        .extension
        .on_event(
            &HostEvent::SessionShutdown {
                reason: "pb8-shutdown".to_string(),
                target_session_file: None,
            },
            &HostCtx::event(ExtMode::Rpc, true, harness.cwd.clone()),
        )
        .await;

    assert_eq!(
        harness.last_widget(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY),
        Some(None),
        "`src/extension/index.ts:1098` — shutdown must REMOVE the async slot, not leave it \
         describing a dead session: {:?}",
        harness.widgets()
    );
    assert_eq!(
        harness.last_widget(FLEET_STATUS_WIDGET_KEY),
        Some(None),
        "`src/extension/index.ts:1063` — `fleetStatus?.dispose()` clears the human slot too: {:?}",
        harness.widgets()
    );
}

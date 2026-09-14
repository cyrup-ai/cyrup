//! The workflowScript runtime — pi `runWorkflowScript` (`scripted-workflow.ts:1717-2284`) with the
//! Node worker replaced by a V8 isolate embedded through `deno_core` (SCOPE_3f §3).
//!
//! # Why the op model deletes most of the old runtime
//!
//! `deno_core` ops are natively async on tokio, so a child launch is an op the script `await`s and
//! twelve concurrent `await`s are twelve concurrent tokio futures. There is no handle registry, no
//! `wasi:io/poll` bridge, no guest pump and no epoch bookkeeping, because the op future IS the
//! operation's lifetime (§3.2). Upstream's 963-line promise-graph detached-child detector likewise
//! collapses to one `observed` bool per launch key (§3.5).
//!
//! # The three blockers this module is organised around (§2)
//!
//! * **BLOCKER-1 — `JsRuntime` is `!Send`.** A V8 isolate is thread-confined, so each workflow owns
//!   one named OS thread running a current-thread runtime; ops forward to cyrup's main multi-thread
//!   runtime through a captured `Handle`, so the isolate is confined but the WORK is not. Same shape
//!   as R-ARCH-EXT-013's single-thread-per-`Store` confinement in `cyrup-ext`.
//! * **BLOCKER-2 — `terminate_execution` is the only preemption and it poisons the isolate.** The
//!   supervising task holds a `v8::IsolateHandle`. Because a poisoned isolate raises on every
//!   later entry, the terminate is ordered to be the LAST touch: a cancelled run is not stopped
//!   until the assembly-flush window (which re-enters the guest) has closed, exactly as upstream
//!   decides `mayFlushAssembly` inside `onAbort` before `finish` calls `worker.terminate()`.
//!   `cancel_terminate_execution()` is therefore never needed here — there is no subsequent entry
//!   to repair, and the partial is read from host-side state, never back out of the guest.
//! * **BLOCKER-3 — the near-heap-limit callback must RAISE the limit.** V8's OOM path is `abort()`,
//!   which would take the host process with it. The callback grants slack, records the fatal and
//!   terminates the script — `cyrup-ext`'s rule holds here too: the host never crashes.
//!
//! # Liveness (§6)
//!
//! Every wait is bounded and every bound has one owner: running JS by the supervisor's
//! `terminate_execution`, a child/host/state call by the whole-run `tokio::time::timeout`
//! ([`WORKFLOW_DEFAULT_TIMEOUT_MS`] when the caller supplies none), and settlement by
//! [`WORKFLOW_SETTLE_DRAIN_TIMEOUT_MS`]. The teardown order is fixed and is stated at
//! [`run_workflow_script`].
//!
//! # The snapshot and the ops crate (§7)
//!
//! The `#[op2]` ops, the `extension!` declaration and `js/prelude.js` live in the
//! `cyrup-workflow-runtime` crate, not here: a `build.rs` can never import from the crate whose
//! build it is running, and this crate's own `build.rs` (new, alongside this file) needs exactly
//! that extension to produce `WORKFLOW_SNAPSHOT`. [`RunSharedBridge`] is the thin adapter from
//! [`RunShared`] to that crate's `WorkflowOpsBridge` trait, seeded into `OpState` directly after
//! `JsRuntime::new` — never through the extension's `options`/`state` — because a snapshot reused
//! across many different workflow runs must never have any one run's state baked into it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_core::cancel::CancelToken;
use cyrup_workflow_runtime::{ObservationKind, WorkflowOpsBridge};
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use super::analyzer::{AnalyzerOptions, analyze_workflow_script};
use super::git_ref::{BASE_REF_VALIDATION_ERROR, valid_git_ref};
use super::json_value::assert_workflow_json_value;
use super::recovery::{
    is_acceptance_metadata_recovery, is_explicit_read_only_recovery_review,
    recovery_barrier_message,
};
use super::settlement::CompletionSettlement;
use super::types::{
    WorkflowConsoleEntry, WorkflowConsoleLevel, WorkflowLanePlan, WorkflowScriptError,
    WorkflowScriptErrorKind, WorkflowScriptPartial, WorkflowScriptResult,
    WorkflowScriptValidationResult, WorkflowSteerOptions, WorkflowSteerResult, WorkflowSteerState,
};
use crate::spawn::parallel::DEFAULT_GLOBAL_CONCURRENCY_LIMIT;
use crate::workflows::{
    HOST_STEP_MAX_COUNT, HostStepKind, HostStepMonitorKind, HostStepNode, HostStepState,
    HostStepVerdict, HostStepVersion, WorkflowContinuation, WorkflowHostCommandParams,
    WorkflowHostCommandResult, WorkflowHostCommandState, WorkflowKey, WorkflowScriptChildResult,
    WorkflowScriptOperation, WorkflowScriptTraceEntry, WorkflowScriptTraceState,
    normalize_workflow_host_command_params, stable_json,
};

// ------------------------------------------------------------------------------------------------
// Host callbacks (pi `RunWorkflowScriptOptions`, `:1102-1128`)
// ------------------------------------------------------------------------------------------------

/// One admitted launch call — pi's `admit` argument element (`:1113`).
#[derive(Clone, Debug)]
pub struct WorkflowRunCall {
    /// The child key.
    pub key: String,
    /// The launch params object.
    pub params: Map<String, Value>,
}

/// Admission metadata handed to `launch` — pi's fourth `launch` argument (`:1114`).
#[derive(Clone, Copy, Debug)]
pub struct WorkflowLaunchAdmission {
    /// Always `true` here: the engine admits before launching.
    pub admitted: bool,
    /// Whether this launch is part of a `runs.all` batch.
    pub batch: bool,
}

/// A resume reference on its way to the resolver — pi's `resolveResume` first argument
/// (`:1115`): either a retained run id string or a keyed receipt reference.
#[derive(Clone, Debug)]
pub enum WorkflowResumeInput {
    /// A retained run id.
    RunId(String),
    /// A keyed workflow receipt reference.
    Reference(super::types::WorkflowReceiptResumeReference),
}

/// A resolved resume — pi `string | WorkflowResolvedResumeReference` (`:1115`).
#[derive(Clone, Debug)]
pub enum WorkflowResolvedResume {
    /// Just the run id.
    RunId(String),
    /// The run id plus known lineage.
    Reference(super::types::WorkflowResolvedResumeReference),
}

/// The embedder-implemented host surface. Optional capabilities default to upstream's exact
/// "unavailable" refusals, so an embedder implements only what it has.
#[async_trait::async_trait]
pub trait WorkflowScriptHost: Send + Sync {
    /// pi `options.admit` (`:1112`) — runs once per launch/batch BEFORE any child spawns.
    async fn admit(&self, _calls: &[WorkflowRunCall]) -> Result<(), String> {
        Ok(())
    }

    /// pi `options.launch` (`:1114`) — launch one child and resolve its settled result.
    async fn launch(
        &self,
        key: &str,
        params: Map<String, Value>,
        cancel: CancelToken,
        admission: WorkflowLaunchAdmission,
    ) -> Result<WorkflowScriptChildResult, String>;

    /// pi `options.status` (`:1116`).
    async fn status(
        &self,
        key_or_run_id: &str,
        cancel: CancelToken,
    ) -> Result<WorkflowScriptChildResult, String>;

    /// Whether `resolveResume` exists (pi checks `options.resolveResume` presence).
    fn supports_resolve_resume(&self) -> bool {
        false
    }

    /// pi `options.resolveResume` (`:1115`).
    async fn resolve_resume(
        &self,
        _reference: WorkflowResumeInput,
        _cancel: CancelToken,
        _index: Option<u64>,
    ) -> Result<WorkflowResolvedResume, String> {
        Err("Keyed workflow receipt resume is unavailable in this host.".to_string())
    }

    /// Whether `steer` exists (pi checks `options.steer` presence, `:2044`).
    fn supports_steer(&self) -> bool {
        false
    }

    /// pi `options.steer` (`:1117`).
    async fn steer(
        &self,
        _key: &str,
        _message: &str,
        _options: WorkflowSteerOptions,
        _cancel: CancelToken,
    ) -> Result<WorkflowSteerResult, String> {
        Err("Workflow steering is unavailable in this host.".to_string())
    }

    /// Whether `host` exists (pi checks `options.host` presence, `:2070`).
    fn supports_host(&self) -> bool {
        false
    }

    /// pi `options.host` (`:1118`).
    async fn host_command(
        &self,
        _key: &str,
        _params: WorkflowHostCommandParams,
        _cancel: CancelToken,
    ) -> Result<WorkflowHostCommandResult, String> {
        Err("runs.host is unavailable in this host context.".to_string())
    }
}

/// Mission workflow state — pi `options.state` (`:1119-1122`); absent means the verbatim
/// "Workflow state is unavailable without a mission." refusal.
#[async_trait::async_trait]
pub trait WorkflowStateStore: Send + Sync {
    /// `state.get(key)`.
    async fn get(&self, key: &str) -> Result<Option<Value>, String>;
    /// `state.set(key, value)`.
    async fn set(&self, key: &str, value: Value) -> Result<(), String>;
}

/// A stop-child handle — pi's `registerStopChild` argument (`:1123`): returns `true` when the key
/// named a live launch that was stopped.
pub type WorkflowStopChild = Arc<dyn Fn(&str, Option<&str>) -> bool + Send + Sync>;

/// pi `oneUsePermit.claim` (`:1105`): the verbatim permit error, or `None` to admit.
pub type WorkflowPermitClaim = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
/// pi `continueAfterAbortWhenChildrenSettled` (`:1109`).
pub type WorkflowFlushEligibility = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// pi `onTrace` (`:1124`).
pub type WorkflowTraceCallback = Arc<dyn Fn(&WorkflowScriptTraceEntry, usize) + Send + Sync>;
/// pi `onLanePlan` (`:1125`).
pub type WorkflowLanePlanCallback = Arc<dyn Fn(&[WorkflowLanePlan]) + Send + Sync>;
/// pi `onEmit` (`:1126`) — fallible: a persistence failure aborts the run.
pub type WorkflowEmitCallback = Arc<
    dyn Fn(
            Vec<Value>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;
/// pi `onHostStep` (`:1119`).
pub type WorkflowHostStepCallback = Arc<dyn Fn(&HostStepNode) + Send + Sync>;
/// pi `registerStopChild` (`:1123`) — `Some(stop)` before the run, `None` at settlement.
pub type WorkflowStopChildRegistrar = Arc<dyn Fn(Option<WorkflowStopChild>) + Send + Sync>;

/// pi `RunWorkflowScriptOptions` (`scripted-workflow.ts:1102-1128`).
pub struct RunWorkflowScriptOptions {
    /// The workflowScript body.
    pub script: String,
    /// pi `oneUsePermit.claim` (`:1105`) — host-only first-slice admission; never sent to the
    /// guest. Returns the verbatim permit error, or `None` to admit.
    pub one_use_permit: Option<WorkflowPermitClaim>,
    /// pi `timeoutMs` — bounds the WHOLE run via `tokio::time::timeout` (BLOCKER-1: epoch cannot
    /// bound a guest waiting in a host call).
    pub timeout_ms: Option<u64>,
    /// pi `signal` — cancellation, as the workspace's one abort flag (`cyrup-core/src/cancel.rs`:
    /// "No subsystem invents its own abort flag").
    pub cancel: Option<CancelToken>,
    /// pi `continueAfterAbortWhenChildrenSettled` (`:1109`). The callback cannot throw here
    /// (upstream's throwing-callback arm is unrepresentable for an infallible `fn`).
    pub continue_after_abort_when_children_settled: Option<WorkflowFlushEligibility>,
    /// pi `globalConcurrencyLimit` (`:1111`), default [`DEFAULT_GLOBAL_CONCURRENCY_LIMIT`].
    pub global_concurrency_limit: Option<usize>,
    /// The launch/status/steer/host/admit/resolve surface.
    pub host: Arc<dyn WorkflowScriptHost>,
    /// Mission state, when the run has one.
    pub state: Option<Arc<dyn WorkflowStateStore>>,
    /// pi `registerStopChild` (`:1123`) — called with `Some(stop)` before the run and `None` at
    /// settlement.
    pub register_stop_child: Option<WorkflowStopChildRegistrar>,
    /// pi `onTrace` (`:1124`) — telemetry; never decides workflow outcomes.
    pub on_trace: Option<WorkflowTraceCallback>,
    /// pi `onLanePlan` (`:1125`).
    pub on_lane_plan: Option<WorkflowLanePlanCallback>,
    /// pi `onEmit` (`:1126`) — the ONE callback whose failure aborts the run (upstream
    /// `:1936-1944`), hence fallible.
    pub on_emit: Option<WorkflowEmitCallback>,
    /// pi `onHostStep` (`:1119`).
    pub on_host_step: Option<WorkflowHostStepCallback>,
}

// ------------------------------------------------------------------------------------------------
// Shared run state
// ------------------------------------------------------------------------------------------------

/// A pending failure recorded by a host import (emit persistence, abort) that must preempt the
/// guest's own outcome.
#[derive(Clone, Debug)]
struct PendingFailure {
    message: String,
    error_kind: Option<WorkflowScriptErrorKind>,
}

/// One queued telemetry event (§5.5). Delivered on the drain task, never on the isolate thread.
#[derive(Debug)]
enum TelemetryEvent {
    /// A newly appended trace entry and its index — the delta, not the journal.
    ///
    /// Boxed like [`Self::HostStep`]: this is much the largest variant, and the enum is moved
    /// through a channel on every single trace push.
    Trace {
        /// The entry that was just appended.
        entry: Box<WorkflowScriptTraceEntry>,
        /// Its 0-based index in the run's trace journal.
        index: usize,
    },
    /// A `runs.lanes` plan advisory.
    LanePlan(Vec<WorkflowLanePlan>),
    /// A host-step transition. Boxed: `HostStepNode` is much larger than the other variants.
    HostStep(Box<HostStepNode>),
    /// Settlement reached: deliver everything already queued, then stop.
    ///
    /// An explicit sentinel rather than "drop the last sender", because `Arc<RunShared>` — and so
    /// the sender — may still be held by a task that outlives settlement. This terminates the
    /// drain deterministically without depending on who is still holding a clone.
    Flush,
}

#[derive(Clone, Debug)]
struct LaunchRecord {
    fingerprint: String,
    observed: bool,
    generated_lane_key: Option<String>,
}

#[derive(Clone, Debug)]
struct OpRecord {
    key: String,
    observed: bool,
}

#[derive(Default)]
pub(crate) struct RunInner {
    emits: Vec<Value>,
    console: Vec<WorkflowConsoleEntry>,
    trace: Vec<WorkflowScriptTraceEntry>,
    children: HashMap<String, WorkflowScriptChildResult>,
    child_order: Vec<String>,
    launches: HashMap<String, LaunchRecord>,
    launch_order: Vec<String>,
    steers: HashMap<u64, OpRecord>,
    steer_order: Vec<u64>,
    hosts: HashMap<u64, OpRecord>,
    host_order: Vec<u64>,
    /// Monotonic id for steer/host records. Not a handle: nothing outside this process ever sees
    /// it, because an op future IS the operation (§3.2) and there is no poll protocol to key.
    next_op_id: u64,
    stopped_launches: HashSet<String>,
    child_stop_tokens: HashMap<String, CancelToken>,
    batch_admissions: HashMap<String, Arc<tokio::sync::OnceCell<Result<(), String>>>>,
    recovery_barrier: Option<String>,
    finishing: bool,
    assembly_abort: bool,
    abort_error: Option<String>,
}

pub(crate) struct RunShared {
    host: Arc<dyn WorkflowScriptHost>,
    state_store: Option<Arc<dyn WorkflowStateStore>>,
    one_use_permit: Option<WorkflowPermitClaim>,
    on_trace: Option<WorkflowTraceCallback>,
    on_lane_plan: Option<WorkflowLanePlanCallback>,
    on_emit: Option<WorkflowEmitCallback>,
    on_host_step: Option<WorkflowHostStepCallback>,
    semaphore: Arc<tokio::sync::Semaphore>,
    /// The `childController` — aborts every child at settlement/cancel.
    child_cancel: CancelToken,
    /// Fires when a host import records a fatal failure the run loop must observe.
    fatal_signal: CancelToken,
    fatal: Mutex<Option<PendingFailure>>,
    /// Bumped whenever a child result lands — wakes the reuse waiters in [`await_child`].
    settle_tx: watch::Sender<u64>,
    /// BLOCKER-1 / §5.1: cyrup's MAIN multi-thread runtime.
    ///
    /// The isolate thread runs a current-thread runtime (deno_core's op driver requires it —
    /// `deno_unsync::spawn` asserts the flavour), so every host call is forwarded here instead of
    /// being awaited on the isolate thread. That is what makes the isolate confined but the WORK
    /// parallel: a 12-way `runs.all` becomes 12 tasks on the main pool.
    main_handle: tokio::runtime::Handle,
    /// §5.5: the telemetry queue. Unbounded by design — a slow journal writer must never apply
    /// back-pressure to a running workflow, and it must never touch the isolate thread.
    telemetry: tokio::sync::mpsc::UnboundedSender<TelemetryEvent>,
    /// §6.2: the last moment any operation made progress. Telemetry for 3g's live card, not a
    /// safety net — with ops there is no unsettleable handle for a stall sweep to discover.
    last_progress: Mutex<Instant>,
    inner: Mutex<RunInner>,
}

impl RunShared {
    fn bump(&self) {
        // §6.2: progress is touched in exactly one place, which is the same place that wakes the
        // reuse waiters — so "a child landed" and "the run made progress" cannot drift apart.
        if let Ok(mut last) = self.last_progress.lock() {
            *last = Instant::now();
        }
        self.settle_tx.send_modify(|generation| *generation += 1);
    }

    /// Seconds since any operation last made progress — 3g's live card reads this (§6.2).
    #[allow(dead_code, reason = "consumed by SCOPE_3g's progress projection")]
    fn idle_for(&self) -> Duration {
        self.last_progress
            .lock()
            .map(|last| last.elapsed())
            .unwrap_or_default()
    }

    fn record_fatal(&self, message: String, error_kind: Option<WorkflowScriptErrorKind>) {
        let mut fatal = self
            .fatal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if fatal.is_none() {
            *fatal = Some(PendingFailure {
                message,
                error_kind,
            });
        }
        drop(fatal);
        self.fatal_signal.cancel();
    }

    /// Queue the newest trace entry for the telemetry drain (§5.5).
    ///
    /// Two rules, both load-bearing. **Telemetry must not decide workflow outcomes** (upstream
    /// `:1746-1755` — a throwing `onTrace` there would reject the child promise the script is
    /// awaiting, so one failed status write could mark a completed child failed); sending on an
    /// unbounded channel makes that structurally impossible, and it also keeps the embedder off
    /// the isolate thread entirely. **And the event is the DELTA, not a snapshot**: cloning the
    /// whole trace per push is quadratic, and a 32-lane × 16-stage workflow would perform on the
    /// order of 10⁶ entry-clones under the state lock.
    fn trace_changed(&self) {
        if self.on_trace.is_none() {
            return;
        }
        let delta = {
            let inner = self.lock();
            inner
                .trace
                .last()
                .cloned()
                .map(|entry| (entry, inner.trace.len().saturating_sub(1)))
        };
        if let Some((entry, index)) = delta {
            self.emit_telemetry(TelemetryEvent::Trace {
                entry: Box::new(entry),
                index,
            });
        }
    }

    /// Hand an event to the drain. Never blocks and never fails the run: a closed channel means
    /// settlement already finished, and a late trace entry is not worth an error.
    fn emit_telemetry(&self, event: TelemetryEvent) {
        let _ = self.telemetry.send(event);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RunInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn push_trace(&self, entry: WorkflowScriptTraceEntry) {
        self.lock().trace.push(entry);
        self.trace_changed();
    }

    fn host_step_changed(&self, step: &HostStepNode) {
        if self.on_host_step.is_some() {
            self.emit_telemetry(TelemetryEvent::HostStep(Box::new(step.clone())));
        }
    }

    /// Run a host call on the MAIN runtime and await it from the isolate thread (BLOCKER-1).
    ///
    /// `JoinError` here means the task was cancelled or panicked; both surface as a plain failure
    /// rather than propagating a panic across the V8 FFI boundary, where an unwind would abort the
    /// process rather than fail the run.
    async fn on_main<F, T>(&self, future: F) -> Result<T, String>
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.main_handle
            .spawn(future)
            .await
            .map_err(|error| format!("Workflow host call did not complete: {error}"))
    }

    /// Allocate the next steer/host record id. Deliberately NOT a handle: §3.2's op model means
    /// the future is the operation, so this id never leaves the host — it exists only so the
    /// settlement scan can name which `runs.steer`/`runs.host` call went unobserved.
    #[allow(
        dead_code,
        reason = "the guest allocates correlation ids; kept for host-side callers"
    )]
    fn next_op_id(&self) -> u64 {
        let mut inner = self.lock();
        inner.next_op_id += 1;
        inner.next_op_id
    }

    /// §3.5 / §6 DL-9 — record that the script consumed a launch/steer/host promise.
    ///
    /// Keyed PER LAUNCH, not latched over a captured array: `runs.lanes` pushes later-stage
    /// observations after its aggregate has already been awaited, and a one-shot latch would mark
    /// only the first stage — making every multi-stage lane fail settlement with a spurious
    /// "unawaited runs.run launch(es)". Upstream cannot key this way (it has no host-side identity
    /// for a promise); here the launch key IS the identity.
    pub(crate) fn mark_observed(&self, kind: ObservationKind, key: &str, call_id: u64) {
        let mut inner = self.lock();
        match kind {
            ObservationKind::Run => {
                if let Some(launch) = inner.launches.get_mut(key) {
                    launch.observed = true;
                }
            }
            ObservationKind::Steer => {
                if let Some(record) = inner.steers.get_mut(&call_id) {
                    record.observed = true;
                }
            }
            ObservationKind::Host => {
                if let Some(record) = inner.hosts.get_mut(&call_id) {
                    record.observed = true;
                }
            }
        }
    }

    /// `runs.lanes` plan advisory — queued telemetry, never awaited (§5.5).
    pub(crate) fn record_lane_plan(&self, lanes_json: &str) {
        if self.on_lane_plan.is_some()
            && let Ok(lanes) = serde_json::from_str::<Vec<WorkflowLanePlan>>(lanes_json)
        {
            self.emit_telemetry(TelemetryEvent::LanePlan(lanes));
        }
    }

    /// Captured `console.*` (`:770-776`). An unknown level is dropped, as upstream drops it.
    pub(crate) fn record_console(&self, level: &str, text: String) {
        if let Some(level) = WorkflowConsoleLevel::parse(level) {
            self.lock()
                .console
                .push(WorkflowConsoleEntry { level, text });
        }
    }

    /// `emit(value)` — upstream `:1929-1946`. The one callback whose failure aborts the run, so the
    /// emit is rolled back before the fatal is recorded and the wording is upstream's.
    pub(crate) async fn emit(&self, value: Value) -> Result<(), String> {
        if let Err(error) = assert_workflow_json_value(&value, "emit") {
            let message = format!("Workflow emit could not be persisted: {error}");
            self.record_fatal(message.clone(), None);
            return Err(message);
        }
        let snapshot = {
            let mut inner = self.lock();
            inner.emits.push(value);
            inner.emits.clone()
        };
        let Some(on_emit) = &self.on_emit else {
            return Ok(());
        };
        // Awaited, not blocked on: §5.5. A sync callback here would force the embedder to block
        // the isolate thread inside a running op.
        if let Err(error) = on_emit(snapshot).await {
            self.lock().emits.pop();
            let message = format!("Workflow emit could not be persisted: {error}");
            self.record_fatal(message.clone(), None);
            return Err(message);
        }
        Ok(())
    }

    /// `state.get(key)` — the verbatim refusal when the run has no mission.
    pub(crate) async fn state_get(&self, key: &str) -> Result<Option<Value>, String> {
        let Some(store) = self.state_store.clone() else {
            return Err("Workflow state is unavailable without a mission.".to_string());
        };
        let key = validate_key(Some(&Value::String(key.to_string())), "state")?;
        store.get(&key).await
    }

    /// `state.set(key, value)`.
    pub(crate) async fn state_set(&self, key: &str, value: Value) -> Result<(), String> {
        let Some(store) = self.state_store.clone() else {
            return Err("Workflow state is unavailable without a mission.".to_string());
        };
        let key = validate_key(Some(&Value::String(key.to_string())), "state")?;
        {
            let inner = self.lock();
            if let Some(barrier) = &inner.recovery_barrier {
                return Err(recovery_barrier_message(
                    barrier,
                    &format!("state.set('{key}')"),
                ));
            }
        }
        assert_workflow_json_value(&value, &format!("state.set('{key}') value"))?;
        store.set(&key, value).await
    }
}

/// Bridges [`RunShared`] to `cyrup_workflow_runtime`'s `WorkflowOpsBridge` (§7) — the ONLY thing
/// that crate's ops can reach outside their own crate. A thin newtype rather than an
/// `impl … for RunShared` directly: several of `RunShared`'s own operations (`run_launch`,
/// `run_status`, …) take `&Arc<RunShared>`, not `&RunShared`, because they clone the `Arc` to move
/// an owned handle into a spawned task — a bridge trait's `&self` cannot manufacture that `Arc`
/// back out of a bare reference, so this wrapper simply holds it.
#[derive(Clone)]
struct RunSharedBridge(Arc<RunShared>);

#[async_trait::async_trait]
impl WorkflowOpsBridge for RunSharedBridge {
    async fn launch(&self, envelope: Value) -> Result<Value, String> {
        let envelope: LaunchEnvelope = serde_json::from_value(envelope)
            .map_err(|error| format!("Invalid workflow launch envelope: {error}"))?;
        let delivery = run_launch(&self.0, envelope).await?;
        serde_json::to_value(&delivery)
            .map_err(|error| format!("Workflow launch result could not be persisted: {error}"))
    }

    async fn status(&self, key_or_run_id: String) -> Result<Value, String> {
        let result = run_status(&self.0, &key_or_run_id).await?;
        serde_json::to_value(&result)
            .map_err(|error| format!("Workflow status result could not be persisted: {error}"))
    }

    async fn steer(
        &self,
        call_id: u64,
        key: String,
        message: String,
        options: Value,
    ) -> Result<Value, String> {
        let options: WorkflowSteerOptions = serde_json::from_value(options)
            .map_err(|error| format!("Invalid runs.steer options: {error}"))?;
        let result = run_steer(&self.0, call_id, &key, &message, options).await?;
        serde_json::to_value(&result)
            .map_err(|error| format!("Workflow steer result could not be persisted: {error}"))
    }

    async fn host_command(
        &self,
        call_id: u64,
        key: String,
        params: Value,
    ) -> Result<Value, String> {
        let result = run_host_command(&self.0, call_id, &key, params).await?;
        serde_json::to_value(&result)
            .map_err(|error| format!("Workflow host result could not be persisted: {error}"))
    }

    fn observe(&self, kind: ObservationKind, key: String, call_id: u64) {
        self.0.mark_observed(kind, &key, call_id);
    }

    fn record_lane_plan(&self, lanes_json: String) {
        self.0.record_lane_plan(&lanes_json);
    }

    async fn emit(&self, value: Value) -> Result<(), String> {
        self.0.emit(value).await
    }

    fn record_console(&self, level: String, text: String) {
        self.0.record_console(&level, text);
    }

    async fn state_get(&self, key: String) -> Result<Option<Value>, String> {
        self.0.state_get(&key).await
    }

    async fn state_set(&self, key: String, value: Value) -> Result<(), String> {
        self.0.state_set(&key, value).await
    }
}

/// pi `stoppedChildResult` (`:1786`).
fn stopped_child_result(key: &str, message: &str) -> WorkflowScriptChildResult {
    WorkflowScriptChildResult {
        key: key.to_string(),
        ok: false,
        stopped: true,
        output: message.to_string(),
        error: Some(message.to_string()),
        ..Default::default()
    }
}

/// pi `workflowStringMetadata` (`:1628-1636`): the display metadata copied onto trace entries.
fn workflow_string_metadata(
    params: &Map<String, Value>,
) -> (Option<String>, Option<String>, Option<String>) {
    let field = |name: &str| {
        params
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToString::to_string)
    };
    (field("phase"), field("label"), field("agent"))
}

fn trace_entry(
    operation: WorkflowScriptOperation,
    key: &str,
    state: WorkflowScriptTraceState,
) -> WorkflowScriptTraceEntry {
    WorkflowScriptTraceEntry {
        operation,
        key: key.to_string(),
        state,
        agent: None,
        run_id: None,
        duration_ms: None,
        phase: None,
        label: None,
        error: None,
        generated_lane_key: None,
        lane: None,
        warning: None,
    }
}

/// pi `validateKey` (`:1401-1405`) — the grammar via [`WorkflowKey::parse`] (SCOPE_3 §A.3's one
/// declaration), the wording owned by this call site.
fn validate_key(value: Option<&Value>, owner: &str) -> Result<String, String> {
    let text = value.and_then(Value::as_str);
    match text {
        Some(key) if WorkflowKey::parse(key).is_ok() => Ok(key.to_string()),
        _ => Err(format!(
            "{owner} key must be 1-128 characters using letters, numbers, '.', '_' or '-', and start with a letter or number."
        )),
    }
}

/// pi `canonicalRunParams` (`:1393-1397`).
fn canonical_run_params(params: &Map<String, Value>) -> Map<String, Value> {
    if !params.contains_key("gate") || params.get("acceptance") != Some(&Value::Bool(false)) {
        return params.clone();
    }
    let mut without = params.clone();
    without.remove("acceptance");
    without
}

/// pi `isZeroUsage` (`:1683-1691`).
fn is_zero_usage(usage: Option<&Value>) -> bool {
    let Some(usage) = usage.and_then(Value::as_object) else {
        return false;
    };
    let zero = |field: &str| {
        usage
            .get(field)
            .map_or(0.0, |v| v.as_f64().unwrap_or(f64::NAN))
            == 0.0
    };
    let cost_zero = match usage.get("cost") {
        Some(Value::Object(cost)) => {
            cost.get("total")
                .map_or(0.0, |v| v.as_f64().unwrap_or(f64::NAN))
                == 0.0
        }
        _ => true,
    };
    zero("input") && zero("output") && zero("cacheRead") && zero("cacheWrite") && cost_zero
}

/// pi `AUTO_RESUME_PARAM_KEYS` (`:1679`).
const AUTO_RESUME_PARAM_KEYS: [&str; 18] = [
    "acceptance",
    "agentContract",
    "baseRef",
    "index",
    "intercomBridge",
    "label",
    "lane",
    "maxRuntimeMs",
    "output",
    "outputMode",
    "outputSchema",
    "phase",
    "skill",
    "skills",
    "task",
    "timeoutMs",
    "toolBudget",
    "worktree",
];

/// pi `setupAbortResumeParams` (`:1693-1715`): detect the setup-abort shape (aborted before any
/// model output, zero usage) and build the one-shot auto-resume params.
fn setup_abort_resume_params(
    params: &Map<String, Value>,
    result: &WorkflowScriptChildResult,
    cancelled: bool,
) -> Option<Map<String, Value>> {
    if cancelled || result.ok || result.stopped || result.interrupted {
        return None;
    }
    let run_id = result.run_id.as_deref().filter(|id| !id.is_empty())?;
    let child_result = (result.results.len() == 1)
        .then(|| result.results.first())
        .flatten()
        .and_then(Value::as_object);
    let error = child_result
        .and_then(|child| child.get("error").and_then(Value::as_str))
        .or(result.error.as_deref());
    if error != Some("This operation was aborted")
        || !is_zero_usage(child_result.and_then(|child| child.get("usage")))
    {
        return None;
    }
    let messages = child_result
        .and_then(|child| child.get("messages").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    let message = messages
        .iter()
        .rev()
        .find(|entry| entry.get("role").and_then(Value::as_str) == Some("assistant"));
    if let Some(message) = message {
        let message = message.as_object()?;
        if message.get("stopReason").and_then(Value::as_str) != Some("error")
            || message.get("errorMessage").and_then(Value::as_str) != error
        {
            return None;
        }
        let content_empty = message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        if !content_empty || !is_zero_usage(message.get("usage")) {
            return None;
        }
        if message.contains_key("diagnostics") || message.contains_key("responseId") {
            return None;
        }
    }
    let task = params
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("Continue after the setup abort.")
        .to_string();
    let mut resume_params = Map::new();
    resume_params.insert("resume".into(), json!(run_id));
    resume_params.insert("task".into(), json!(task.clone()));
    for key in AUTO_RESUME_PARAM_KEYS {
        if let Some(value) = params.get(key) {
            resume_params.insert(key.to_string(), value.clone());
        }
    }
    resume_params.insert("resume".into(), json!(run_id));
    resume_params.insert("task".into(), json!(task));
    Some(resume_params)
}

/// pi `describeGateAcceptanceConflict` (`runs/shared/acceptance.ts`, re-declared inline in the
/// worker at `:626-637`): render both conflicting fields, JSON-encoded, each cut to 120 UTF-16
/// units with a literal `"..."` suffix when longer.
fn describe_gate_acceptance_conflict(gate: &Value, acceptance: &Value) -> String {
    fn render(value: &Value) -> String {
        let encoded = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
        if encoded.chars().map(char::len_utf16).sum::<usize>() > 120 {
            format!("{}...", truncate_chars(&encoded, 120))
        } else {
            encoded
        }
    }
    format!(
        " Both fields were present: gate={} acceptance={}.",
        render(gate),
        render(acceptance)
    )
}

/// pi `parseWorkflowResumeReference` (`:1300-1308`) — messages verbatim; the launch path prefixes
/// them with `runs.run('{key}') `.
fn parse_workflow_resume_reference(
    value: &Value,
) -> Result<super::types::WorkflowReceiptResumeReference, String> {
    let Some(reference) = value.as_object() else {
        return Err("resume must be a retained run id or keyed workflow receipt reference.".into());
    };
    if reference
        .keys()
        .any(|field| field != "workflowRunId" && field != "key" && field != "latest")
    {
        return Err("keyed resume contains unsupported fields.".into());
    }
    let workflow_run_id = reference
        .get("workflowRunId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or("keyed resume workflowRunId must be non-empty.")?;
    let key = validate_key(reference.get("key"), "keyed resume")?;
    if reference.get("latest") != Some(&Value::Bool(true)) {
        return Err("keyed resume requires latest: true.".into());
    }
    Ok(super::types::WorkflowReceiptResumeReference {
        workflow_run_id: workflow_run_id.to_string(),
        key,
        latest: true,
    })
}

/// pi `omitNonJsonWorkflowResultMetadata` (`:1310-1320`): a child result whose `results` cannot
/// cross the JSON boundary loses `results` and keeps everything else. Structurally, a
/// [`serde_json::Value`] is always JSON, so the assert can only fail on a non-finite number — the
/// drop-`results` arm is retained because it is upstream's behaviour at this exact seam.
fn omit_non_json_workflow_result_metadata(
    result: &WorkflowScriptChildResult,
) -> WorkflowScriptChildResult {
    let mut normalized = result.clone();
    let results_ok = normalized
        .results
        .iter()
        .all(|value| assert_workflow_json_value(value, "runs.run result.results").is_ok());
    if !results_ok {
        normalized.results = Vec::new();
    }
    normalized
}

/// pi `workflowReturnRecoveryHint` (`:1385-1394` of the host section): appended to a
/// return-serialization failure so the model can still recover child outputs.
fn workflow_return_recovery_hint(children: &[WorkflowScriptChildResult]) -> String {
    if children.is_empty() {
        return " Return only plain JSON data. For a child result, select fields such as { runId: child.runId, ok: child.ok, outputReference: child.outputReference }.".to_string();
    }
    let references: Vec<String> = children
        .iter()
        .take(10)
        .map(|child| {
            let mut fields = Vec::new();
            if let Some(run_id) = &child.run_id {
                fields.push(format!("runId={}", truncate_chars(run_id, 500)));
            }
            if let Some(reference) = &child.output_reference {
                fields.push(format!(
                    "outputReference={}",
                    truncate_chars(reference, 500)
                ));
            }
            if let Some(artifact) = child.artifact_paths.first() {
                fields.push(format!("artifact={}", truncate_chars(artifact, 500)));
            }
            if fields.is_empty() {
                format!("'{}'", child.key)
            } else {
                format!("'{}' ({})", child.key, fields.join(", "))
            }
        })
        .collect();
    let more = if children.len() > references.len() {
        format!(", and {} more", children.len() - references.len())
    } else {
        String::new()
    };
    format!(
        " Child work completed before return serialization failed. Recover outputs from: {}{}. Return a plain projection such as {{ runId: child.runId, ok: child.ok, outputReference: child.outputReference }}.",
        references.join(", "),
        more
    )
}

/// JS `.slice(0, n)` on identifier-ish strings (run ids, paths): UTF-16 slicing via the shared
/// truncating helper.
fn truncate_chars(value: &str, max: usize) -> String {
    crate::workflows::truncate_display(value, max)
}

// ------------------------------------------------------------------------------------------------
// The operations behind the ops (§3.2)
//
// Each is a plain `async fn`: the future IS the operation's lifetime, so `runs.all` of twelve is
// twelve concurrent futures on the parent runtime. The thin `#[op2]` layer over these lives in the
// `cyrup-workflow-runtime` crate (WORKFLOW_1 §7), never in this one: a `build.rs` can never import
// from the crate whose build it is running, and `cyrup-ext-subagents/build.rs` needs that
// extension to produce the V8 startup snapshot. See `scripted/mod.rs`'s note for the full move.
// ------------------------------------------------------------------------------------------------

/// The `launch` envelope the guest posts — upstream's worker `run` call args (`:283-291`).
#[derive(Debug, serde::Deserialize)]
pub(crate) struct LaunchEnvelope {
    key: Value,
    params: Value,
    #[serde(default, rename = "collectFailure")]
    collect_failure: bool,
    #[serde(default)]
    batch: Option<Value>,
    #[serde(default, rename = "generatedLaneKey")]
    generated_lane_key: Option<String>,
}

/// `runs.status(keyOrRunId)` — upstream `:2011-2027`.
///
/// An op, so the future IS the status call: no handle, no poll. Upstream does not await status at
/// settlement (`:1826`), and neither does this — a hung status must not hold the run hostage.
pub(crate) async fn run_status(
    shared: &Arc<RunShared>,
    key_or_run_id: &str,
) -> Result<WorkflowScriptChildResult, String> {
    if key_or_run_id.trim().is_empty() {
        return Err("runs.status(keyOrRunId) requires a non-empty string.".to_string());
    }
    let key = key_or_run_id.to_string();
    let (target, known_run_id, finishing) = {
        let inner = shared.lock();
        let known = inner.children.get(&key);
        (
            known
                .and_then(|child| child.run_id.clone())
                .unwrap_or_else(|| key.clone()),
            known.and_then(|child| child.run_id.clone()),
            inner.finishing,
        )
    };
    let mut started = trace_entry(
        WorkflowScriptOperation::Status,
        &key,
        WorkflowScriptTraceState::Started,
    );
    started.run_id = known_run_id;
    shared.push_trace(started);
    if finishing {
        // The run is tearing down. Upstream's equivalent handle simply never settles; here the op
        // resolves with the abort wording instead, because a never-resolving op is a hang (§6).
        return Err("Workflow script aborted.".to_string());
    }
    let outcome = {
        let host = shared.host.clone();
        let cancel = shared.child_cancel.clone();
        shared
            .on_main(async move { host.status(&target, cancel).await })
            .await
            .and_then(|outcome| outcome)
    };
    match outcome {
        Ok(result) => {
            if !shared.lock().finishing {
                let mut entry = trace_entry(
                    WorkflowScriptOperation::Status,
                    &key,
                    if result.ok {
                        WorkflowScriptTraceState::Completed
                    } else {
                        WorkflowScriptTraceState::Failed
                    },
                );
                entry.run_id = result.run_id.clone();
                if !result.ok {
                    entry.error = Some(result.output.clone());
                }
                shared.push_trace(entry);
            }
            shared.bump();
            if result.ok {
                Ok(omit_non_json_workflow_result_metadata(&result))
            } else {
                Err(format!("Status '{key}' failed: {}", result.output))
            }
        }
        Err(error) => Err(error),
    }
}

/// `runs.steer` — upstream `:2028-2059`.
/// `runs.steer(key, message, options)` — upstream `:2028-2059`.
///
/// `call_id` is allocated by the GUEST, not the host: upstream marks a steer observed when the
/// script first attaches `then`/`catch`/`finally`, which happens synchronously at `await` time and
/// therefore before any result exists. A guest-owned correlation id lets the prelude mark
/// observation at exactly that moment without reintroducing a handle protocol (§3.5).
pub(crate) async fn run_steer(
    shared: &Arc<RunShared>,
    call_id: u64,
    key: &str,
    message: &str,
    options: WorkflowSteerOptions,
) -> Result<WorkflowSteerResult, String> {
    let key = validate_key(Some(&Value::String(key.to_string())), "runs.steer")?;
    if message.trim().is_empty() {
        return Err(format!("runs.steer('{key}') requires a non-empty message."));
    }
    {
        let inner = shared.lock();
        if let Some(barrier) = &inner.recovery_barrier {
            return Err(recovery_barrier_message(
                barrier,
                &format!("runs.steer('{key}')"),
            ));
        }
    }
    shared.push_trace(trace_entry(
        WorkflowScriptOperation::Steer,
        &key,
        WorkflowScriptTraceState::Started,
    ));
    let has_launch = shared.lock().launches.contains_key(&key);
    let supports = shared.host.supports_steer();
    {
        let mut inner = shared.lock();
        inner.steers.insert(
            call_id,
            OpRecord {
                key: key.clone(),
                observed: false,
            },
        );
        inner.steer_order.push(call_id);
    }
    let started_at = Instant::now();
    let message = message.trim().to_string();
    let outcome: Result<WorkflowSteerResult, String> = if !has_launch {
        Err(format!(
            "runs.steer('{key}') requires a prior runs.run/runs.all launch with that key."
        ))
    } else if !supports {
        Err("Workflow steering is unavailable in this host.".to_string())
    } else {
        let host = shared.host.clone();
        let cancel = shared.child_cancel.clone();
        let steer_key = key.clone();
        let steer_message = message.clone();
        shared
            .on_main(async move {
                host.steer(&steer_key, &steer_message, options, cancel)
                    .await
            })
            .await
            .and_then(|outcome| outcome)
    };
    match outcome {
        Ok(receipt) => {
            let mut entry = trace_entry(
                WorkflowScriptOperation::Steer,
                &key,
                match receipt.state {
                    WorkflowSteerState::Queued => WorkflowScriptTraceState::Queued,
                    WorkflowSteerState::Delivered => WorkflowScriptTraceState::Delivered,
                    WorkflowSteerState::Missed => WorkflowScriptTraceState::Missed,
                    WorkflowSteerState::Failed => WorkflowScriptTraceState::Failed,
                },
            );
            entry.duration_ms = Some(started_at.elapsed().as_millis() as u64);
            entry.error = receipt.error.clone();
            shared.push_trace(entry);
            shared.bump();
            Ok(receipt)
        }
        Err(error) => {
            let mut entry = trace_entry(
                WorkflowScriptOperation::Steer,
                &key,
                WorkflowScriptTraceState::Failed,
            );
            entry.duration_ms = Some(started_at.elapsed().as_millis() as u64);
            entry.error = Some(error.clone());
            shared.push_trace(entry);
            shared.bump();
            Err(error)
        }
    }
}

/// `runs.host` — upstream `:2060-2110`.
/// `runs.host(key, params)` — upstream `:2060-2110`.
///
/// Registered as an op ONLY for `workflow`-provenance runs (§0.4): a raw `workflowScript` isolate
/// never links this, so `runs.host` is `undefined` there rather than refused. `call_id` is
/// guest-allocated for the same reason as [`run_steer`]'s.
#[allow(clippy::too_many_lines)]
pub(crate) async fn run_host_command(
    shared: &Arc<RunShared>,
    call_id: u64,
    key: &str,
    raw_params: Value,
) -> Result<WorkflowHostCommandResult, String> {
    let key = validate_key(Some(&Value::String(key.to_string())), "runs.host")?;
    let params =
        normalize_workflow_host_command_params(&raw_params, &format!("runs.host('{key}') params"))?;
    {
        let inner = shared.lock();
        if let Some(barrier) = &inner.recovery_barrier {
            return Err(recovery_barrier_message(
                barrier,
                &format!("runs.host('{key}')"),
            ));
        }
        if inner.hosts.len() >= HOST_STEP_MAX_COUNT {
            return Err(format!(
                "workflowScript supports at most {HOST_STEP_MAX_COUNT} runs.host calls."
            ));
        }
    }
    if !shared.host.supports_host() {
        return Err("runs.host is unavailable in this host context.".to_string());
    }
    {
        let mut inner = shared.lock();
        inner.hosts.insert(
            call_id,
            OpRecord {
                key: key.clone(),
                observed: false,
            },
        );
        inner.host_order.push(call_id);
    }
    let started_at_ms = now_ms();
    let started_step = HostStepNode {
        version: HostStepVersion,
        kind: HostStepKind,
        monitor_kind: HostStepMonitorKind::Command,
        id: key.clone(),
        label: key.clone(),
        role: params.role.map(|role| match role {
            crate::workflows::WorkflowHostCommandRole::Ci => "ci".to_string(),
            crate::workflows::WorkflowHostCommandRole::Gate => "gate".to_string(),
        }),
        provider: params.provider.clone(),
        state: HostStepState::Running,
        verdict: None,
        reason_code: None,
        detail: None,
        target: None,
        freshness: None,
        report_path: None,
        exit_code: None,
        updated_at: started_at_ms.into(),
        deadline_at: Some((started_at_ms + params.timeout_ms).into()),
    };
    shared.host_step_changed(&started_step);
    shared.push_trace(trace_entry(
        WorkflowScriptOperation::Host,
        &key,
        WorkflowScriptTraceState::Started,
    ));
    let started = Instant::now();
    let outcome = {
        let host = shared.host.clone();
        let cancel = shared.child_cancel.clone();
        let host_key = key.clone();
        let host_params = params.clone();
        shared
            .on_main(async move { host.host_command(&host_key, host_params, cancel).await })
            .await
            .and_then(|outcome| outcome)
    };
    match outcome {
        Ok(result) => {
            let detail_raw = [result.error.clone().unwrap_or_default(), {
                let stderr = result.stderr.trim();
                if stderr.is_empty() {
                    result.stdout.trim().to_string()
                } else {
                    stderr.to_string()
                }
            }]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            let detail: String = truncate_chars(
                detail_raw
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim(),
                200,
            );
            let state = match result.state {
                WorkflowHostCommandState::Stopped => HostStepState::Cancelled,
                _ if result.ok => HostStepState::Done,
                _ => HostStepState::Error,
            };
            let mut step = started_step.clone();
            step.state = state;
            step.verdict = (state == HostStepState::Done).then_some(HostStepVerdict::Pass);
            step.reason_code = (!result.ok).then(|| match result.state {
                WorkflowHostCommandState::TimedOut => "timed_out".to_string(),
                WorkflowHostCommandState::Stopped => "aborted".to_string(),
                _ => "command_failed".to_string(),
            });
            step.detail = (!detail.is_empty()).then(|| detail.clone());
            step.report_path = params.output.clone().or_else(|| {
                result
                    .output_path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
            });
            step.exit_code = result.exit_code.map(Into::into);
            step.updated_at = now_ms().into();
            shared.host_step_changed(&step);
            let mut entry = trace_entry(
                WorkflowScriptOperation::Host,
                &key,
                if result.ok {
                    WorkflowScriptTraceState::Completed
                } else if result.state == WorkflowHostCommandState::Stopped {
                    WorkflowScriptTraceState::Stopped
                } else {
                    WorkflowScriptTraceState::Failed
                },
            );
            entry.duration_ms = Some(result.duration_ms);
            if !result.ok {
                entry.error = Some(
                    result
                        .error
                        .clone()
                        .unwrap_or_else(|| "Host command failed.".to_string()),
                );
            }
            shared.push_trace(entry);
            shared.bump();
            if result.ok {
                Ok(result)
            } else {
                let fallback = result.error.clone().unwrap_or_else(|| {
                    format!(
                        "exit code {}",
                        result
                            .exit_code
                            .map_or_else(|| "unknown".to_string(), |code| code.to_string())
                    )
                });
                let text = if detail.is_empty() { fallback } else { detail };
                Err(format!("Host command '{key}' failed: {text}"))
            }
        }
        Err(error) => {
            let mut step = started_step.clone();
            step.state = HostStepState::Error;
            step.reason_code = Some("execution_failed".to_string());
            step.detail = Some(truncate_chars(
                &error.split_whitespace().collect::<Vec<_>>().join(" "),
                200,
            ));
            step.updated_at = now_ms().into();
            shared.host_step_changed(&step);
            let mut entry = trace_entry(
                WorkflowScriptOperation::Host,
                &key,
                WorkflowScriptTraceState::Failed,
            );
            entry.duration_ms = Some(started.elapsed().as_millis() as u64);
            entry.error = Some(error.clone());
            shared.push_trace(entry);
            shared.bump();
            Err(error)
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// `runs.run` / `runs.all` member launch — upstream `:2112-2281`, the heart of the runtime.
#[allow(clippy::too_many_lines)]
pub(crate) async fn run_launch(
    shared: &Arc<RunShared>,
    envelope: LaunchEnvelope,
) -> Result<LaunchDelivery, String> {
    {
        let inner = shared.lock();
        if inner.assembly_abort {
            // Upstream: any NEW call after an assembly-only abort fails the whole run (`:1957`).
            let message = inner
                .abort_error
                .clone()
                .unwrap_or_else(|| "Workflow context was replaced or reloaded.".to_string());
            drop(inner);
            shared.record_fatal(message.clone(), None);
            return Err(message.clone());
        }
    }
    let key = match validate_key(Some(&envelope.key), "runs.run") {
        Ok(key) => key,
        Err(error) => return Err(error),
    };
    let Some(params) = envelope.params.as_object().cloned() else {
        return Err(format!(
            "runs.run('{key}', params) requires a params object."
        ));
    };
    let generated_lane_key = envelope.generated_lane_key.as_deref().and_then(|lane| {
        (WorkflowKey::parse(lane).is_ok() && key.starts_with(&format!("{lane}.")))
            .then(|| lane.to_string())
    });
    let collect_failure = envelope.collect_failure;
    let fingerprint = stable_json(&Value::Object(canonical_run_params(&params)));
    let (phase, label, agent) = workflow_string_metadata(&params);

    // Reuse path (`:2129-2137`). The decision is taken under the lock and the lock is released
    // BY SCOPE before the await — not by a hand-placed `drop`, which a later edit could reorder
    // past the suspension point.
    enum ReusePlan {
        Fresh,
        Conflict,
        Reuse,
    }
    let plan = {
        let mut inner = shared.lock();
        match inner.launches.get(&key).cloned() {
            None => ReusePlan::Fresh,
            Some(existing) if existing.fingerprint != fingerprint => ReusePlan::Conflict,
            Some(existing) => {
                let mut entry = trace_entry(
                    WorkflowScriptOperation::Run,
                    &key,
                    WorkflowScriptTraceState::Reused,
                );
                entry.phase = phase.clone();
                entry.label = label.clone();
                entry.agent = agent.clone();
                entry.generated_lane_key = existing.generated_lane_key.clone();
                inner.trace.push(entry);
                ReusePlan::Reuse
            }
        }
    };
    match plan {
        ReusePlan::Conflict => {
            return Err(format!(
                "Duplicate workflow key '{key}' used with incompatible launch params."
            ));
        }
        ReusePlan::Reuse => {
            shared.trace_changed();
            let result = await_child(shared, &key).await;
            return Ok(deliver_launch(shared, &key, &result, collect_failure));
        }
        ReusePlan::Fresh => {}
    }

    // Permit + shape rejections (`:2138-2177`), every message verbatim.
    if let Some(permit) = &shared.one_use_permit {
        if let Some(error) = permit(&key) {
            return Err(error);
        }
        if envelope.batch.is_some() {
            return Err("Workflow child permit does not support runs.all.".to_string());
        }
        if params.contains_key("resume") {
            return Err("Workflow child permit does not support retained resume.".to_string());
        }
    }
    if params.contains_key("action") {
        return Err(format!(
            "runs.run('{key}') accepts execution params only; management action is not allowed."
        ));
    }
    if params.contains_key("workflowScript") {
        return Err(format!(
            "runs.run('{key}') cannot start a nested workflow script."
        ));
    }
    if ["tasks", "chain", "parallel", "concurrency", "chainDir"]
        .iter()
        .any(|field| params.contains_key(*field))
    {
        return Err(format!(
            "runs.run('{key}') accepts one child via {{ agent, task }}; use runs.all(...) and JavaScript control flow for orchestration."
        ));
    }
    if params
        .get("worktree")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(format!("runs.run('{key}') worktree must be true or false."));
    }
    if let Some(base_ref) = params.get("baseRef")
        && !base_ref.as_str().is_some_and(valid_git_ref)
    {
        return Err(format!("runs.run('{key}') {BASE_REF_VALIDATION_ERROR}"));
    }
    if let Some(gate) = params.get("gate") {
        if !gate.as_str().is_some_and(|text| !text.trim().is_empty()) {
            return Err(format!(
                "runs.run('{key}') gate must be a non-empty command string."
            ));
        }
        let acceptance = params.get("acceptance");
        if acceptance.is_some() && acceptance != Some(&Value::Bool(false)) {
            return Err(format!(
                "runs.run('{key}') gate cannot be combined with acceptance; use one gate command or acceptance.verify.{}",
                describe_gate_acceptance_conflict(gate, acceptance.unwrap_or(&Value::Null))
            ));
        }
        if params.contains_key("resume") {
            return Err(format!(
                "runs.run('{key}') gate is not supported with retained resume."
            ));
        }
    }
    let mut resume_reference = None;
    if let Some(resume) = params.get("resume") {
        if !resume.is_string() {
            match parse_workflow_resume_reference(resume) {
                Ok(reference) => resume_reference = Some(reference),
                Err(error) => return Err(format!("runs.run('{key}') {error}")),
            }
        }
        if resume.as_str().is_some_and(|text| text.trim().is_empty()) {
            return Err(format!(
                "runs.run('{key}') resume must be a non-empty retained run id."
            ));
        }
        if params.contains_key("agent") {
            return Err(format!(
                "runs.run('{key}') resume and agent are mutually exclusive."
            ));
        }
        if !params
            .get("task")
            .and_then(Value::as_str)
            .is_some_and(|task| !task.trim().is_empty())
        {
            return Err(format!(
                "runs.run('{key}') resume requires a non-empty task follow-up."
            ));
        }
    }

    // Batch admission dedup (`:2180-2196`).
    let batch = envelope.batch.as_ref().and_then(|batch| {
        let id = batch.get("id").and_then(Value::as_str)?.to_string();
        let calls = batch
            .get("calls")
            .and_then(Value::as_array)?
            .iter()
            .filter_map(|call| {
                let call_key = call.get("key").and_then(Value::as_str)?.to_string();
                let call_params = call.get("params").and_then(Value::as_object)?.clone();
                Some(WorkflowRunCall {
                    key: call_key,
                    params: call_params,
                })
            })
            .collect::<Vec<_>>();
        Some((id, calls))
    });
    let is_batch = batch.is_some();
    let admission_cell = {
        let mut inner = shared.lock();

        match &batch {
            Some((id, _)) => inner
                .batch_admissions
                .entry(id.clone())
                .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
                .clone(),
            None => Arc::new(tokio::sync::OnceCell::new()),
        }
    };
    let admission_calls: Vec<WorkflowRunCall> = {
        let inner = shared.lock();
        let mut seen = HashSet::new();
        batch
            .map(|(_, calls)| calls)
            .unwrap_or_else(|| {
                vec![WorkflowRunCall {
                    key: key.clone(),
                    params: params.clone(),
                }]
            })
            .into_iter()
            .filter(|call| {
                if seen.contains(&call.key) || inner.launches.contains_key(&call.key) {
                    false
                } else {
                    seen.insert(call.key.clone());
                    true
                }
            })
            .collect()
    };

    // Register the launch, then run the child (`:2198-2279`). The op future IS the child's
    // lifetime, so there is nothing to register a handle for.
    {
        let mut inner = shared.lock();
        inner.launches.insert(
            key.clone(),
            LaunchRecord {
                fingerprint,
                observed: false,
                generated_lane_key: generated_lane_key.clone(),
            },
        );
        inner.launch_order.push(key.clone());
        inner.child_order.push(key.clone());
        let mut entry = trace_entry(
            WorkflowScriptOperation::Run,
            &key,
            WorkflowScriptTraceState::Started,
        );
        entry.phase = phase.clone();
        entry.label = label.clone();
        entry.agent = agent.clone();
        entry.generated_lane_key = generated_lane_key.clone();
        inner.trace.push(entry);
    }
    shared.trace_changed();

    let shared_task = shared.clone();
    let task_key = key.clone();
    let task_params = params;
    let task_lane_key = generated_lane_key;
    {
        let shared = &shared_task;
        let started_at = Instant::now();
        let result = run_child(
            &shared_task,
            &task_key,
            task_params.clone(),
            resume_reference,
            admission_cell,
            admission_calls,
            is_batch,
        )
        .await;
        let normalized = match result {
            Ok(result) => {
                let mut normalized = if !result.ok && result.error.is_none() {
                    let mut with_error = result.clone();
                    with_error.error = Some(result.output.clone());
                    with_error
                } else {
                    result
                };
                normalized.key = task_key.clone();
                Ok(normalized)
            }
            Err(error) => Err(error),
        };
        let (phase, label, agent) = workflow_string_metadata(&task_params);
        match normalized {
            Ok(result) => {
                let stopped_override = {
                    let mut inner = shared_task.lock();
                    inner.child_stop_tokens.remove(&task_key);
                    if inner.stopped_launches.contains(&task_key) {
                        inner.children.get(&task_key).cloned()
                    } else {
                        inner.children.insert(task_key.clone(), result.clone());
                        None
                    }
                };
                let was_stopped = stopped_override.is_some();
                let effective = stopped_override.unwrap_or_else(|| result.clone());
                if !was_stopped {
                    {
                        let mut inner = shared_task.lock();
                        if is_acceptance_metadata_recovery(&effective)
                            && inner.recovery_barrier.is_none()
                        {
                            inner.recovery_barrier = Some(task_key.clone());
                        }
                    }
                    let state = if effective.ok {
                        WorkflowScriptTraceState::Completed
                    } else if effective.stopped {
                        WorkflowScriptTraceState::Stopped
                    } else if effective.detached {
                        WorkflowScriptTraceState::Detached
                    } else {
                        WorkflowScriptTraceState::Failed
                    };
                    let mut entry = trace_entry(WorkflowScriptOperation::Run, &task_key, state);
                    entry.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                    entry.phase = phase;
                    entry.label = label;
                    entry.agent = effective.agent.clone().or(agent);
                    entry.run_id = effective.run_id.clone();
                    entry.generated_lane_key = task_lane_key;
                    if !effective.ok {
                        entry.error = effective
                            .error
                            .clone()
                            .or_else(|| Some(effective.output.clone()));
                    }
                    shared_task.push_trace(entry);
                }
                shared_task.bump();
                Ok(deliver_launch(
                    shared,
                    &task_key,
                    &effective,
                    collect_failure,
                ))
            }
            Err(error) => {
                let failure = WorkflowScriptChildResult {
                    key: task_key.clone(),
                    ok: false,
                    output: error.clone(),
                    error: Some(error.clone()),
                    ..Default::default()
                };
                let effective = {
                    let mut inner = shared_task.lock();
                    inner.child_stop_tokens.remove(&task_key);
                    if inner.stopped_launches.contains(&task_key) {
                        inner.children.get(&task_key).cloned().unwrap_or_else(|| {
                            let mut stopped = failure.clone();
                            stopped.stopped = true;
                            stopped
                        })
                    } else {
                        inner.children.insert(task_key.clone(), failure.clone());
                        let mut entry = trace_entry(
                            WorkflowScriptOperation::Run,
                            &task_key,
                            WorkflowScriptTraceState::Failed,
                        );
                        entry.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        entry.phase = phase;
                        entry.label = label;
                        entry.agent = agent;
                        entry.generated_lane_key = task_lane_key;
                        entry.error = Some(error.clone());
                        inner.trace.push(entry);
                        failure.clone()
                    }
                };
                shared_task.trace_changed();
                shared_task.bump();
                Ok(deliver_launch(
                    shared,
                    &task_key,
                    &effective,
                    collect_failure,
                ))
            }
        }
    }
}

/// Wait until `children[key]` exists (the reuse path's second consumer).
async fn await_child(shared: &Arc<RunShared>, key: &str) -> WorkflowScriptChildResult {
    let mut rx = shared.settle_tx.subscribe();
    loop {
        {
            let inner = shared.lock();
            if let Some(child) = inner.children.get(key) {
                return child.clone();
            }
        }
        if rx.changed().await.is_err() {
            // The run is tearing down; report the stopped shape.
            return stopped_child_result(key, "Workflow script aborted.");
        }
    }
}

/// What a launch delivers to the script — pi's `deliver` (`:2121-2133`).
///
/// A tagged outcome rather than a `Result`, because upstream's rejection carries a *kind*
/// (`workflowErrorKind = "detached-child"`, `:903-906`) that a bare error string cannot. The
/// prelude reconstructs the exact Error shape from this, so the model sees byte-identical wording
/// and an identical `.workflowErrorKind` to upstream.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(crate) enum LaunchDelivery {
    /// Resolve the launch promise with the child result.
    Resolved {
        /// The child result, already stripped of non-JSON metadata (`:1310-1320`).
        result: Box<WorkflowScriptChildResult>,
    },
    /// Reject the launch promise.
    Rejected {
        /// The verbatim rejection wording.
        message: String,
        /// `true` when the prelude must set `error.workflowErrorKind = "detached-child"`.
        detached: bool,
    },
}

/// pi's `deliver` (`:2121-2133`): `collectFailure` returns the raw result; otherwise a failed,
/// unstopped, non-recoverable child REJECTS the launch with the detached/failed wording.
fn deliver_launch(
    shared: &Arc<RunShared>,
    key: &str,
    result: &WorkflowScriptChildResult,
    collect_failure: bool,
) -> LaunchDelivery {
    if !collect_failure {
        let recoverable = result.recovery.as_ref().is_some_and(|recovery| {
            recovery.status == "available-for-review"
                && recovery.reason == "acceptance-metadata-rejected"
        });
        if !result.ok && !result.stopped && !recoverable {
            let reason = result
                .error
                .clone()
                .unwrap_or_else(|| result.output.clone());
            let message = if result.detached {
                format!("Run '{key}' detached: {reason}")
            } else {
                format!("Run '{key}' failed: {reason}")
            };
            return LaunchDelivery::Rejected {
                message,
                detached: result.detached,
            };
        }
    }
    let boundary = omit_non_json_workflow_result_metadata(result);
    // Upstream's response-boundary check (`:2130`). `serde_v8` will carry any `serde_json::Value`,
    // so this can only fail on a shape serde itself rejects; the arm is kept because it is
    // upstream's behaviour at this exact seam, and because the recorded child result matters.
    match serde_json::to_value(&boundary) {
        Ok(_) => LaunchDelivery::Resolved {
            result: Box::new(boundary),
        },
        Err(error) => {
            let response_path = format!("runs.run('{key}') result");
            let failure = WorkflowScriptChildResult {
                key: key.to_string(),
                ok: false,
                output: error.to_string(),
                error: Some(error.to_string()),
                ..Default::default()
            };
            shared.lock().children.insert(key.to_string(), failure);
            LaunchDelivery::Rejected {
                message: format!(
                    "{response_path} must contain only JSON data before it can be returned from workflowScript. Return a plain projection such as {{ runId, ok, output }}. {error}"
                ),
                detached: false,
            }
        }
    }
}

/// The per-child pipeline (`:2198-2262`): admission → stop checks → resume resolution → semaphore
/// → launch → setup-abort auto-resume → continuation lineage.
async fn run_child(
    shared: &Arc<RunShared>,
    key: &str,
    params: Map<String, Value>,
    resume_reference: Option<super::types::WorkflowReceiptResumeReference>,
    admission_cell: Arc<tokio::sync::OnceCell<Result<(), String>>>,
    admission_calls: Vec<WorkflowRunCall>,
    is_batch: bool,
) -> Result<WorkflowScriptChildResult, String> {
    let admitted = admission_cell
        .get_or_init(|| async {
            {
                let inner = shared.lock();
                if inner.finishing {
                    return Ok(());
                }
                for call in &admission_calls {
                    if let Some(barrier) = &inner.recovery_barrier
                        && !is_explicit_read_only_recovery_review(&call.params)
                    {
                        return Err(recovery_barrier_message(barrier, &call.key));
                    }
                }
            }
            let host = shared.host.clone();
            let calls = admission_calls.clone();
            shared
                .on_main(async move { host.admit(&calls).await })
                .await
                .unwrap_or_else(Err)
        })
        .await
        .clone();
    admitted?;

    let stop_state = {
        let mut inner = shared.lock();
        if inner.finishing || inner.stopped_launches.contains(key) {
            Some(
                inner
                    .children
                    .get(key)
                    .and_then(|child| child.error.clone())
                    .unwrap_or_else(|| "Workflow script aborted.".to_string()),
            )
        } else {
            let token = shared.child_cancel.child_token();
            inner.child_stop_tokens.insert(key.to_string(), token);
            None
        }
    };
    if let Some(text) = stop_state {
        return Ok(stopped_child_result(key, &text));
    }
    let child_cancel = shared
        .lock()
        .child_stop_tokens
        .get(key)
        .cloned()
        .unwrap_or_else(|| shared.child_cancel.child_token());

    // Resume resolution (`:2208-2233`).
    let resume_input = match (
        &resume_reference,
        params.get("resume").and_then(Value::as_str),
    ) {
        (Some(reference), _) => Some(WorkflowResumeInput::Reference(reference.clone())),
        (None, Some(resume)) if shared.host.supports_resolve_resume() => {
            Some(WorkflowResumeInput::RunId(resume.to_string()))
        }
        _ => None,
    };
    let mut resolved_lineage: Option<Vec<String>> = None;
    let mut launch_params = params.clone();
    // §6.3 / DL-14: every child is marked as being inside a workflow, so the subagent tool can
    // refuse a nested `workflowScript` at the one boundary `runs.run`'s own check cannot see.
    launch_params.insert(WORKFLOW_CHILD_MARKER.to_string(), Value::Bool(true));
    if let Some(input) = resume_input {
        if !shared.host.supports_resolve_resume() {
            return Err("Keyed workflow receipt resume is unavailable in this host.".to_string());
        }
        let index = params.get("index").and_then(Value::as_u64);
        let resolved = {
            let host = shared.host.clone();
            let resume_input = input.clone();
            let cancel = child_cancel.clone();
            shared
                .on_main(async move { host.resolve_resume(resume_input, cancel, index).await })
                .await
                .and_then(|outcome| outcome)?
        };
        let (resolved_run_id, lineage) = match &resolved {
            WorkflowResolvedResume::RunId(run_id) => (run_id.trim().to_string(), Vec::new()),
            WorkflowResolvedResume::Reference(reference) => (
                reference.run_id.trim().to_string(),
                reference
                    .run_ids
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|id| id.trim().to_string())
                    .filter(|id| !id.is_empty())
                    .collect(),
            ),
        };
        if resume_reference.is_some() && resolved_run_id.is_empty() {
            return Err(
                "Keyed workflow receipt resume resolved without a retained run id.".to_string(),
            );
        }
        if matches!(resolved, WorkflowResolvedResume::Reference(_)) {
            let mut seen = HashSet::new();
            let mut list: Vec<String> = if lineage.is_empty() {
                vec![resolved_run_id.clone()]
            } else {
                lineage
            }
            .into_iter()
            .filter(|id| seen.insert(id.clone()))
            .collect();
            if list.last().map(String::as_str) != Some(resolved_run_id.as_str()) {
                list.push(resolved_run_id.clone());
            }
            if matches!(input, WorkflowResumeInput::RunId(_)) {
                let inner = shared.lock();
                let predecessor = inner
                    .children
                    .values()
                    .find(|child| child.run_id.as_deref() == Some(resolved_run_id.as_str()));
                if let Some(continuation) = predecessor.and_then(|child| child.continuation.clone())
                    && continuation.run_ids.last().map(String::as_str)
                        == Some(resolved_run_id.as_str())
                {
                    list = continuation.run_ids;
                }
            }
            resolved_lineage = Some(list);
        }
        if !resolved_run_id.is_empty() {
            launch_params.insert("resume".into(), json!(resolved_run_id));
        }
    }

    // Bounded launch (`:2234-2258`) under the run's ONE semaphore.
    let permit = shared
        .semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| "Workflow script aborted.".to_string())?;
    let result = {
        let recheck = {
            let inner = shared.lock();
            inner.finishing || inner.stopped_launches.contains(key) || child_cancel.is_cancelled()
        };
        if recheck {
            let text = {
                let inner = shared.lock();
                inner
                    .children
                    .get(key)
                    .and_then(|child| child.error.clone())
                    .unwrap_or_else(|| "Workflow script aborted.".to_string())
            };
            drop(permit);
            return Ok(stopped_child_result(key, &text));
        }
        let first = {
            let host = shared.host.clone();
            let launch_key = key.to_string();
            let params = launch_params.clone();
            let cancel = child_cancel.clone();
            shared
                .on_main(async move {
                    host.launch(
                        &launch_key,
                        params,
                        cancel,
                        WorkflowLaunchAdmission {
                            admitted: true,
                            batch: is_batch,
                        },
                    )
                    .await
                })
                .await
                .and_then(|outcome| outcome)?
        };
        let auto_resume = setup_abort_resume_params(&params, &first, child_cancel.is_cancelled());
        match auto_resume {
            None => first,
            Some(resume_params) => {
                let run_id = first.run_id.clone().unwrap_or_default();
                let mut lineage = resolved_lineage.clone().unwrap_or_default();
                if !lineage.contains(&run_id) {
                    lineage.push(run_id.clone());
                }
                resolved_lineage = Some(lineage);
                let (phase, label, agent) = workflow_string_metadata(&resume_params);
                let mut entry = trace_entry(
                    WorkflowScriptOperation::Run,
                    key,
                    WorkflowScriptTraceState::Started,
                );
                entry.phase = Some("auto-resume".to_string());
                let _ = phase;
                entry.label = label;
                entry.agent = agent;
                entry.run_id = first.run_id.clone();
                shared.push_trace(entry);
                {
                    let host = shared.host.clone();
                    let launch_key = key.to_string();
                    let cancel = child_cancel.clone();
                    shared
                        .on_main(async move {
                            host.launch(
                                &launch_key,
                                resume_params,
                                cancel,
                                WorkflowLaunchAdmission {
                                    admitted: true,
                                    batch: is_batch,
                                },
                            )
                            .await
                        })
                        .await
                        .and_then(|outcome| outcome)?
                }
            }
        }
    };
    drop(permit);
    let mut normalized = result;
    if let Some(lineage) = resolved_lineage
        && !lineage.is_empty()
        && let Some(run_id) = normalized.run_id.clone()
    {
        let mut seen = HashSet::new();
        let run_ids: Vec<String> = lineage
            .into_iter()
            .chain(std::iter::once(run_id))
            .filter(|id| seen.insert(id.clone()))
            .collect();
        normalized.continuation = Some(WorkflowContinuation { run_ids });
    }
    Ok(normalized)
}

// ------------------------------------------------------------------------------------------------
// The isolate thread (§5.1–§5.3)
// ------------------------------------------------------------------------------------------------

/// §6.3 / DL-14 — the marker every workflow child carries in its launch params.
///
/// `runs.run` already refuses `workflowScript` outright (*"cannot start a nested workflow
/// script."*), but a **child agent** can call the subagent tool with `workflowScript`, and that
/// path never goes through `runs.run`. Nesting is therefore refused at the boundary that actually
/// leaks: the engine stamps this marker onto every child launch, and the tool checks for it.
///
/// This matters beyond tidiness. Any future admission control that holds a permit for a run's
/// lifetime turns nesting into textbook hold-and-wait — parent holds a slot while awaiting a child
/// whose agent is blocked acquiring the next one. Refusing at the boundary removes the edge rather
/// than sizing around it.
pub const WORKFLOW_CHILD_MARKER: &str = "__cyrupWorkflowChild";

/// The verbatim refusal a nested `workflowScript` attempt receives — a re-prompt naming the fix
/// (§0.1.3), not a bare denial.
pub const NESTED_WORKFLOW_REFUSAL: &str = "workflowScript cannot be started from inside a workflow child. Express the nested plan as additional runs.run/runs.all/runs.lanes calls in the parent script.";

/// Refuse a `workflowScript` request that originated inside a workflow child (§6.3).
///
/// Returns the verbatim refusal when the request carries [`WORKFLOW_CHILD_MARKER`] **and** asks to
/// start a script.
///
/// WORKFLOW_2 wires the call site, in `extension/tool/mod.rs`'s `Tool::execute`, BEFORE the
/// request is deserialized — `SubagentToolParams` carries neither `workflowScriptPath` nor
/// `workflow` nor the marker, so a post-parse check could not see two of the three
/// `starts_a_script` disjuncts below.
///
/// Note the transport, because it is not the request: this function's marker conjunct is
/// satisfied only because the dispatch site stamps [`WORKFLOW_CHILD_MARKER`] onto the request map
/// when `WORKFLOW_CHILD_ENV` (`extension::executor::workflow`, a private module — hence no
/// intra-doc link) is present in the process
/// environment. A model-authored tool call can never carry the marker itself, and the only other
/// producer is this engine (in-process, when building `launch_params`), so without that env
/// transport the refusal could only ever fire on a request the host had synthesized.
#[must_use]
pub fn refuse_nested_workflow(request: &Map<String, Value>) -> Option<&'static str> {
    let inside_workflow_child = request
        .get(WORKFLOW_CHILD_MARKER)
        .is_some_and(|marker| marker == &Value::Bool(true));
    let starts_a_script = request.contains_key("workflowScript")
        || request.contains_key("workflowScriptPath")
        || request.contains_key("workflow");
    (inside_workflow_child && starts_a_script).then_some(NESTED_WORKFLOW_REFUSAL)
}

/// pi `WORKFLOW_ASSEMBLY_FLUSH_TIMEOUT_MS` (`scripted-workflow.ts:22`) — how long the isolate stays
/// alive for pure result assembly after an abort whose children had all already settled.
pub const WORKFLOW_ASSEMBLY_FLUSH_TIMEOUT_MS: u64 = 5_000;

/// §6 DL-2: the whole-run bound when the caller supplies none.
///
/// Upstream leaves `timeoutMs` optional (`scripted-workflow.ts:1894`) because a hung Node worker
/// costs only itself. Here an unbounded run pins a live V8 isolate and an OS thread, so BLOCKER-1's
/// "epoch cannot bound a waiting guest, `tokio::time::timeout` must" has to have a default to bind.
/// 30 minutes matches upstream's own foreground-workflow default (`tool-description.ts:58`).
pub const WORKFLOW_DEFAULT_TIMEOUT_MS: u64 = 30 * 60 * 1000;

/// §6 DL-3: settlement may not outlive this.
///
/// Upstream's `Promise.allSettled([...steers, ...hostCalls])` (`:1826`) is bounded only in
/// practice, by an AbortSignal-driven executor and `runs.host`'s required `timeoutMs` (up to 24h).
/// [`WorkflowScriptHost`] is a trait an embedder implements, so "the impl honours cancel" is an
/// assumption; the bound is enforced here instead of assumed.
pub const WORKFLOW_SETTLE_DRAIN_TIMEOUT_MS: u64 = 5_000;

/// §5.3 / BLOCKER-3: headroom granted when V8 nears its heap limit.
///
/// The callback must RAISE the limit — V8's OOM path is `abort()`, which would take the host
/// process down with it. The slack exists only so the script can be terminated cleanly.
const WORKFLOW_HEAP_SLACK_BYTES: usize = 16 * 1024 * 1024;

/// §5.3: the per-workflow heap cap. Sized deliberately rather than inherited: a workflow script is
/// a few dozen await-dominated statements, but it holds child results, the trace journal and the
/// emit log, all of which are the model's own data.
const WORKFLOW_HEAP_LIMIT_BYTES: usize = 256 * 1024 * 1024;

/// The V8 startup snapshot for the workflow extension (§7). Built by this crate's OWN `build.rs`,
/// which depends on `cyrup-workflow-runtime` (a normal, non-circular `[build-dependencies]` edge —
/// see that crate's module doc for why the extension itself cannot live in THIS crate) and writes
/// the bytes to this crate's `OUT_DIR`.
static WORKFLOW_SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/workflow.snapshot"));

/// A thread-safe stop button for a running isolate (BLOCKER-2).
///
/// `terminate_execution` is the only preemption V8 offers and it must be called from OUTSIDE the
/// isolate's own thread, so the supervising task holds this while the script runs.
pub(crate) struct Terminator {
    handle: deno_core::v8::IsolateHandle,
}

impl Terminator {
    /// Interrupt whatever the isolate is executing — including a tight `while (true)`.
    ///
    /// Verified against deno_core's own regression test for exactly this shape
    /// (`libs/core/runtime/tests/misc.rs:309-346`).
    fn terminate(&self) {
        self.handle.terminate_execution();
    }
}

/// One workflow's isolate, confined to one named OS thread (BLOCKER-1).
struct IsolateThread {
    join: std::thread::JoinHandle<Result<String, WorkflowGuestError>>,
    terminator: Terminator,
}

/// What the guest returned, or why it did not.
#[derive(Debug)]
pub(crate) struct WorkflowGuestError {
    message: String,
    error_kind: Option<String>,
    error_phase: Option<String>,
}

impl IsolateThread {
    fn spawn(shared: Arc<RunShared>, script: String, state_enabled: bool) -> Result<Self, String> {
        // Ops forward here so children run on the multi-thread pool while the isolate stays put.
        let main = tokio::runtime::Handle::current();
        let (handle_tx, handle_rx) = std::sync::mpsc::channel::<deno_core::v8::IsolateHandle>();
        let join = std::thread::Builder::new()
            .name("cyrup-workflow-isolate".to_string())
            .spawn(move || run_isolate(&main, shared, script, state_enabled, &handle_tx))
            .map_err(|error| format!("Workflow isolate thread could not start: {error}"))?;
        let handle = handle_rx
            .recv()
            .map_err(|_| "Workflow isolate thread stopped before it started.".to_string())?;
        Ok(Self {
            join,
            terminator: Terminator { handle },
        })
    }

    fn terminator(&self) -> Terminator {
        Terminator {
            handle: self.terminator.handle.clone(),
        }
    }

    /// Await the isolate thread without blocking a worker.
    async fn join(self) -> Result<Result<String, WorkflowGuestError>, String> {
        let Self { join, .. } = self;
        tokio::task::spawn_blocking(move || join.join())
            .await
            .map_err(|error| format!("Workflow isolate join failed: {error}"))?
            .map_err(|_| "Workflow isolate thread panicked.".to_string())
    }
}

/// The isolate thread body: build the runtime, install the sandbox, evaluate, drive the event loop.
/// Serializes V8 ISOLATE CONSTRUCTION across threads.
///
/// `deno_core` 0.411.0 guards concurrent snapshot-deserializing isolate creation for
/// [denoland/deno#15590](https://github.com/denoland/deno/issues/15590) — but only on Windows
/// (`runtime/setup.rs:271`'s `if cfg!(windows) && has_snapshot && ...`). On every other platform
/// `v8::Isolate::new(params)` with a `snapshot_blob` runs unguarded, and two threads deserializing
/// the same snapshot at once corrupt V8 internals. The observable symptom is an ABORT, not an
/// error: `vector.h: libc++ Hardening assertion __n < size() failed: vector[] index out of bounds`,
/// with the whole process killed by SIGABRT — a panic across the V8 FFI boundary cannot unwind, so
/// there is nothing to catch.
///
/// This is not a test-only concern. [`IsolateThread::spawn`] gives every workflow run its OWN OS
/// thread, so two concurrent `workflowScript` tool calls in one process take exactly the racing
/// path.
///
/// ⚠ The lock covers CONSTRUCTION ONLY and must never be held across script execution or an
/// `.await`. Isolate creation is microseconds and is the only racing region; holding it longer
/// would serialize every workflow run in the process and silently undo `runs.all`'s concurrency.
fn v8_isolate_init_lock() -> std::sync::MutexGuard<'static, ()> {
    static ISOLATE_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // `unwrap`/`expect` are DENY workspace-wide. A poisoned lock here carries no data to be
    // inconsistent — the guard protects a V8-internal critical section, not a Rust value — so
    // recovering is strictly better than aborting a workflow that would otherwise succeed.
    ISOLATE_INIT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn run_isolate(
    main: &tokio::runtime::Handle,
    shared: Arc<RunShared>,
    script: String,
    state_enabled: bool,
    handle_tx: &std::sync::mpsc::Sender<deno_core::v8::IsolateHandle>,
) -> Result<String, WorkflowGuestError> {
    let guest_error = |message: String| WorkflowGuestError {
        message,
        error_kind: None,
        error_phase: None,
    };
    // Deliberately NOT `main.enter()`. deno_core drives ops with `deno_unsync::spawn`, which
    // asserts the CURRENT runtime is current-thread; entering the multi-thread runtime here makes
    // every op abort the process (a panic across the V8 FFI boundary cannot unwind). Host calls
    // reach the main pool explicitly, through `RunShared::on_main`.
    let _ = main;
    let local = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| guest_error(format!("Workflow isolate runtime failed: {error}")))?;

    // EVERYTHING that can dispatch an op runs inside `block_on`, including construction and the
    // synchronous `execute_script`: the prelude and the agent's script both call ops during
    // evaluation, and `deno_unsync::spawn` needs a live runtime context at that moment.
    local.block_on(async move {
        // ⚠ The guard is scoped to THIS BLOCK — construction only. A bare
        // `let _isolate_init = ...;` here would bind it to the enclosing `async move` block and
        // hold a `std` mutex across every `.await` below, serializing all concurrent workflow runs
        // and pinning a non-`Send` guard across suspension points. See `v8_isolate_init_lock`.
        let mut runtime = {
            let _isolate_init = v8_isolate_init_lock();
            deno_core::JsRuntime::new(deno_core::RuntimeOptions {
                // §7: the identical extension declaration used to build `WORKFLOW_SNAPSHOT` is
                // supplied again here — `deno_core` recognises it by name against the loaded
                // snapshot and skips re-executing its JS; ops still bind normally either way (see
                // `cyrup_workflow_runtime`'s module doc).
                extensions: vec![cyrup_workflow_runtime::cyrup_workflow::init()],
                startup_snapshot: Some(WORKFLOW_SNAPSHOT),
                // §5.3: a deliberate cap, not an inherited default.
                create_params: Some(
                    deno_core::v8::CreateParams::default()
                        .heap_limits(0, WORKFLOW_HEAP_LIMIT_BYTES),
                ),
                ..Default::default()
            })
        };

        // The isolate must be DISPOSED under the same lock it was CREATED under: V8 mutates
        // process-global state in both `Isolate::New` and `Isolate::Dispose`, and deno_core
        // guards neither off-Windows. Serializing construction alone left a create-vs-dispose
        // race that still aborted the process ~10% of the time under load.
        //
        // The body is wrapped in an inner `async` block purely so it has exactly ONE exit: its
        // `?`s and early `return`s resolve to the block's value instead of jumping past the
        // guarded disposal below. Execution is NOT inside the lock — only creation and
        // disposal are, so two concurrent workflow runs still execute fully in parallel.
        let outcome = async {
            // §7 Part A: the bridge is seeded into `OpState` here, AFTER construction, rather than
            // through the extension's `options`/`state` — a snapshot reused across many different
            // workflow runs must never have any one run's state baked into it, and the build-time
            // snapshot pass that produced `WORKFLOW_SNAPSHOT` has no live run to build one from at all.
            {
                let bridge: Arc<dyn WorkflowOpsBridge> = Arc::new(RunSharedBridge(shared.clone()));
                runtime.op_state().borrow_mut().put(bridge);
            }

            let isolate_handle = runtime.v8_isolate().thread_safe_handle();
            if handle_tx.send(isolate_handle.clone()).is_err() {
                return Err(guest_error("Workflow supervisor stopped.".to_string()));
            }

            // BLOCKER-3: RAISE the limit and stop the script; never let V8 reach its own OOM `abort()`.
            // cyrup-ext's rule holds here too — the host never crashes.
            {
                let shared = shared.clone();
                let handle = isolate_handle.clone();
                runtime.add_near_heap_limit_callback(move |current, _initial| {
                    shared.record_fatal(
                        "workflowScript exceeded its memory budget.".to_string(),
                        None,
                    );
                    handle.terminate_execution();
                    current + WORKFLOW_HEAP_SLACK_BYTES
                });
            }

            let host_enabled = shared.host.supports_host();
            install_sandbox(&mut runtime, state_enabled, host_enabled).map_err(guest_error)?;

            // The host compiles; the guest may not. Upstream's asymmetry (`new vm.Script` against
            // `codeGeneration: { strings: false }`, `:916`) reproduced exactly.
            let wrapped =
                format!("globalThis.__cyrupWorkflowRun((async () => {{\n{script}\n}})())");
            let promise = runtime
                .execute_script("workflow-script.js", wrapped)
                .map_err(|error| guest_error(format_guest_error(&error.to_string())))?;

            let resolved = runtime.resolve(promise);
            let value = runtime
                .with_event_loop_promise(resolved, deno_core::PollEventLoopOptions::default())
                .await
                .map_err(|error| workflow_guest_error(&error))?;

            // §6 DL-3 — upstream's `Promise.allSettled([...steers, ...hostCalls])` (`:1826`).
            //
            // In the op model that IS the event loop: a `runs.steer`/`runs.host` the script never
            // awaited is a still-pending op, and running the loop to completion settles exactly the set
            // upstream awaits. Bounded, because `WorkflowScriptHost` is a trait an embedder implements
            // — "the impl honours cancel" is an assumption, not a guarantee, and `runs.host`'s own
            // timeoutMs may be up to 24h. On expiry the ops are dropped with the runtime, which is
            // upstream's `worker.terminate()` shape.
            let _ = tokio::time::timeout(
                Duration::from_millis(WORKFLOW_SETTLE_DRAIN_TIMEOUT_MS),
                runtime.run_event_loop(deno_core::PollEventLoopOptions::default()),
            )
            .await;

            let outcome = {
                deno_core::scope!(scope, runtime);
                let local_value = deno_core::v8::Local::new(scope, value);
                deno_core::serde_v8::from_v8::<GuestOutcomeEnvelope>(scope, local_value).map_err(
                    |error| WorkflowGuestError {
                        message: format!("Workflow return could not be persisted: {error}"),
                        error_kind: None,
                        error_phase: Some("return-serialization".to_string()),
                    },
                )?
            };
            if !outcome.ok {
                return Err(WorkflowGuestError {
                    message: outcome
                        .message
                        .unwrap_or_else(|| "Workflow script failed.".to_string()),
                    error_kind: outcome.error_kind,
                    error_phase: outcome.error_phase,
                });
            }
            serde_json::to_string(&outcome.value.unwrap_or(Value::Null)).map_err(|error| {
                WorkflowGuestError {
                    message: format!("Workflow return could not be persisted: {error}"),
                    error_kind: None,
                    error_phase: Some("return-serialization".to_string()),
                }
            })
        }
        .await;
        {
            let _isolate_dispose = v8_isolate_init_lock();
            drop(runtime);
        }
        outcome
    })
}

/// Deny `WebAssembly` compilation from source — upstream's `codeGeneration: { wasm: false }`.
///
/// V8 calls this through its C ABI, hence the `extern "C"` signature. It is a pure predicate: no
/// allocation, no unwinding, no host state — so there is nothing here that could cross the FFI
/// boundary badly.
extern "C" fn deny_wasm_codegen(
    _context: deno_core::v8::Local<'_, deno_core::v8::Context>,
    _source: deno_core::v8::Local<'_, deno_core::v8::String>,
) -> bool {
    false
}

/// Assemble §3.3's realm: exactly the granted capabilities, `Deno` removed, dynamic code disabled.
///
/// A capability the run may not use is **absent, not present-and-refused** — `state` without a
/// mission (the schema's *"mission state when enabled"*) and `runs.host` without `workflow`
/// provenance (§0.4: raw `workflowScript` *"cannot use runs.host"*). `undefined` is what the model
/// can actually discover; a member that exists and throws teaches it nothing.
///
/// Ordering is deliberate: both code-generation switches are thrown BEFORE the install script runs,
/// and the install hook deletes itself in the same script that uses it, so no agent script can ever
/// observe a partially-built realm.
fn install_sandbox(
    runtime: &mut deno_core::JsRuntime,
    state_enabled: bool,
    host_enabled: bool,
) -> Result<(), String> {
    // upstream `codeGeneration: { strings: false, wasm: false }` (`:916`), exactly. The HOST may
    // still compile the script (as upstream does with `new vm.Script`); the guest may not.
    runtime
        .v8_isolate()
        .set_allow_wasm_code_generation_callback(deny_wasm_codegen);
    {
        let context = runtime.main_context();
        deno_core::scope!(scope, runtime);
        let context = deno_core::v8::Local::new(scope, context);
        context.set_allow_generation_from_strings(false);
    }
    let install = format!(
        "globalThis.__cyrupWorkflowInstall({{ stateEnabled: {state_enabled}, hostEnabled: {host_enabled} }});\n\
         delete globalThis.__cyrupWorkflowInstall;\n\
         delete globalThis.Deno;"
    );
    runtime
        .execute_script("workflow-sandbox.js", install)
        .map(|_| ())
        .map_err(|error| format!("Workflow sandbox setup failed: {error}"))
}

/// The tagged outcome `__cyrupWorkflowRun` resolves with — upstream's worker `error`/`result`
/// message, as data. A thrown JS Error cannot carry `workflowErrorKind` across the boundary
/// (V8→Rust conversion drops arbitrary error properties), so the tag travels as a field.
#[derive(Debug, serde::Deserialize)]
struct GuestOutcomeEnvelope {
    ok: bool,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default, rename = "errorKind")]
    error_kind: Option<String>,
    #[serde(default, rename = "errorPhase")]
    error_phase: Option<String>,
}

/// Format a V8 error the way upstream formats a worker error, so the model sees one shape.
fn format_guest_error(raw: &str) -> String {
    raw.strip_prefix("Uncaught ").unwrap_or(raw).to_string()
}

/// Turn a thrown JS error into the host's error, preserving `workflowErrorKind`.
///
/// Upstream tags a detached-child rejection by hanging `workflowErrorKind` on the `Error`
/// (`scripted-workflow.ts:903-906`) and reads it back at settlement. `deno_core`'s [`JsError`](deno_core::error::JsError)
/// captures exactly that in `additional_properties`, so the tag survives the boundary instead of
/// being flattened into a message the host would have to re-parse.
fn workflow_guest_error(error: &deno_core::error::CoreError) -> WorkflowGuestError {
    use deno_core::error::CoreErrorKind;
    let js_error = match error.as_kind() {
        CoreErrorKind::Js(js) => Some(js),
        _ => None,
    };
    let error_kind = js_error.and_then(|js| {
        js.additional_properties
            .iter()
            .find(|(name, _)| name == "workflowErrorKind")
            .map(|(_, value)| value.clone())
    });
    let message = js_error
        .and_then(|js| js.message.clone())
        .unwrap_or_else(|| format_guest_error(&error.to_string()));
    WorkflowGuestError {
        message: format_guest_error(&message),
        error_kind,
        error_phase: None,
    }
}

// ------------------------------------------------------------------------------------------------
// The public entry points
// ------------------------------------------------------------------------------------------------

/// Validate a workflowScript without running it — upstream `validateWorkflowScript`
/// (`scripted-workflow.ts:1568-1626`), with the authority split of §3.6.
///
/// **V8 owns syntax** (it is the executor, so it is authoritative about what parses) and the
/// **Rust analyzer owns structure** — nested-async plus the enumerated-globals check that upstream
/// cannot perform at all. Compile first: the analyzer then only ever sees source known to parse,
/// so the two can never disagree.
///
/// # Errors
///
/// Engine/instantiation failure text. A script that merely fails validation is an `Ok` result with
/// `ok: false` — that is a finding for the model, not an error for the host.
pub fn validate_workflow_script(
    script: &str,
    state_enabled: bool,
) -> Result<WorkflowScriptValidationResult, String> {
    if script.trim().is_empty() {
        return Ok(WorkflowScriptValidationResult {
            ok: false,
            errors: vec![super::types::WorkflowScriptValidationError {
                message: "workflowScript must not be empty.".to_string(),
                line: None,
                column: None,
            }],
        });
    }
    // 1. Syntax, by the engine that would run it.
    if let Some(error) = compile_check(script) {
        return Ok(WorkflowScriptValidationResult {
            ok: false,
            errors: vec![error],
        });
    }
    // 2. Structure, by the one analyzer.
    let report = analyze_workflow_script(script, AnalyzerOptions { state_enabled })
        .map_err(|error| error.message)?;
    Ok(WorkflowScriptValidationResult {
        ok: report.ok(),
        errors: report.errors,
    })
}

/// Compile the wrapped body in a throwaway isolate purely to learn whether V8 accepts it.
///
/// No ops, no prelude, no snapshot: nothing here can run the script, only parse it. `execute_script`
/// on an `(async () => { … })()` wrapper would *evaluate* it, so the body is wrapped in a function
/// expression that is compiled and immediately discarded.
fn compile_check(script: &str) -> Option<super::types::WorkflowScriptValidationError> {
    // ⚠ Held for this isolate's WHOLE life, unlike `run_isolate`'s create/dispose-only guard.
    //
    // Not an inconsistency — the two are different shapes. `run_isolate` executes arbitrary guest
    // script for up to the workflow's whole timeout, so locking its execution would serialize
    // concurrent workflow runs. This function only PARSES one script in a throwaway isolate: no
    // ops, no prelude, no snapshot, no I/O, no await, microseconds end to end. Serializing it
    // costs nothing measurable and buys the last piece of correctness — measured, this was the
    // final residual abort (~4% of full-suite runs) once create-vs-create and create-vs-dispose
    // were closed, because even a bare `execute_script` mutates process-global V8 state that a
    // concurrent `Isolate::New`/`Dispose` on another thread is also touching.
    let _isolate_whole_life = v8_isolate_init_lock();
    let mut runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions::default());
    // Compiling `(async function () { <body> })` parses the body without executing a statement of
    // it, which is exactly the check upstream gets from acorn.
    let probe = format!("(async function () {{\n{script}\n}});");
    let compiled = runtime.execute_script("workflow-script-validate.js", probe);
    let outcome = match compiled {
        Ok(_) => None,
        Err(error) => {
            let raw = error.to_string();
            // V8 prefixes "Uncaught SyntaxError: "; strip it so the model sees the same shape
            // upstream's acorn message has.
            let message = raw
                .strip_prefix("Uncaught ")
                .unwrap_or(&raw)
                .lines()
                .next()
                .unwrap_or(&raw)
                .to_string();
            Some(super::types::WorkflowScriptValidationError {
                message,
                line: None,
                column: None,
            })
        }
    };
    // Explicit, so the isolate is disposed while `_isolate_whole_life` is still held rather than
    // at the closing brace, where the guard would already have been released.
    drop(runtime);
    outcome
}

/// pi `runWorkflowScript` (`scripted-workflow.ts:1717-2284`).
///
/// # Errors
///
/// [`WorkflowScriptError`] carrying the partial (trace, children, console, emits) — the partial is
/// the point; SCOPE_3g's settlement reads it.
///
/// It is BOXED, matching `runner_main/executor.rs`'s and `cyrup-mcp/src/runtime.rs`'s convention
/// for the same reason: [`WorkflowScriptError`] carries a whole [`WorkflowScriptPartial`] — four
/// `Vec`s at 24 bytes each, plus a 24-byte `String` and the padded `Option` — which lands on
/// clippy's 128-byte `result_large_err` threshold, and it (rightly) refuses to widen the `Ok`
/// return of the workflow's one hot path by that much for the sake of nine cold failure arms. `Box<WorkflowScriptError>` derefs to the same fields, so callers reading
/// `error.message` / `error.partial` are unchanged.
#[allow(clippy::too_many_lines)]
pub async fn run_workflow_script(
    options: RunWorkflowScriptOptions,
) -> Result<WorkflowScriptResult, Box<WorkflowScriptError>> {
    let fail = |message: String, kind: Option<WorkflowScriptErrorKind>| {
        Box::new(WorkflowScriptError {
            message,
            partial: WorkflowScriptPartial::default(),
            error_kind: kind,
        })
    };
    if options.script.trim().is_empty() {
        return Err(fail("workflowScript must not be empty.".into(), None));
    }
    if options.timeout_ms == Some(0) {
        return Err(fail(
            "workflow script timeout must be a positive integer.".into(),
            None,
        ));
    }
    if options.global_concurrency_limit == Some(0) {
        return Err(fail(
            "workflow script global concurrency limit must be a positive integer.".into(),
            None,
        ));
    }

    let (settle_tx, _settle_rx) = watch::channel(0u64);
    // §5.5: one queue, one drain task. Telemetry callbacks run HERE — never on the isolate thread
    // and never inside a running op — so an embedder that persists cannot stall the workflow.
    let (telemetry_tx, mut telemetry_rx) = tokio::sync::mpsc::unbounded_channel::<TelemetryEvent>();
    let telemetry_drain = {
        let on_trace = options.on_trace.clone();
        let on_lane_plan = options.on_lane_plan.clone();
        let on_host_step = options.on_host_step.clone();
        tokio::spawn(async move {
            while let Some(event) = telemetry_rx.recv().await {
                match event {
                    TelemetryEvent::Trace { entry, index } => {
                        if let Some(on_trace) = &on_trace {
                            on_trace(&entry, index);
                        }
                    }
                    TelemetryEvent::LanePlan(lanes) => {
                        if let Some(on_lane_plan) = &on_lane_plan {
                            on_lane_plan(&lanes);
                        }
                    }
                    TelemetryEvent::HostStep(step) => {
                        if let Some(on_host_step) = &on_host_step {
                            on_host_step(&step);
                        }
                    }
                    TelemetryEvent::Flush => break,
                }
            }
        })
    };
    let shared = Arc::new(RunShared {
        host: options.host.clone(),
        state_store: options.state.clone(),
        one_use_permit: options.one_use_permit.clone(),
        on_trace: options.on_trace.clone(),
        on_lane_plan: options.on_lane_plan.clone(),
        on_emit: options.on_emit.clone(),
        on_host_step: options.on_host_step.clone(),
        semaphore: Arc::new(tokio::sync::Semaphore::new(
            options
                .global_concurrency_limit
                .unwrap_or(DEFAULT_GLOBAL_CONCURRENCY_LIMIT),
        )),
        child_cancel: CancelToken::new(),
        fatal_signal: CancelToken::new(),
        fatal: Mutex::new(None),
        settle_tx,
        main_handle: tokio::runtime::Handle::current(),
        telemetry: telemetry_tx,
        last_progress: Mutex::new(Instant::now()),
        inner: Mutex::new(RunInner::default()),
    });

    // pi `registerStopChild` (`:1793-1816`).
    if let Some(register) = &options.register_stop_child {
        let stop_shared = shared.clone();
        let stop: WorkflowStopChild = Arc::new(move |key: &str, message: Option<&str>| {
            let message = message
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("Workflow child '{key}' stopped by user."));
            let mut inner = stop_shared.lock();
            if !inner.launches.contains_key(key) || inner.children.contains_key(key) {
                return false;
            }
            inner.stopped_launches.insert(key.to_string());
            inner
                .children
                .insert(key.to_string(), stopped_child_result(key, &message));
            if let Some(token) = inner.child_stop_tokens.get(key) {
                token.cancel();
            }
            let started = inner
                .trace
                .iter()
                .rev()
                .find(|entry| {
                    entry.operation == WorkflowScriptOperation::Run
                        && entry.key == key
                        && entry.state == WorkflowScriptTraceState::Started
                })
                .cloned();
            let mut entry = trace_entry(
                WorkflowScriptOperation::Run,
                key,
                WorkflowScriptTraceState::Stopped,
            );
            if let Some(started) = started {
                entry.agent = started.agent;
                entry.phase = started.phase;
                entry.label = started.label;
                entry.generated_lane_key = started.generated_lane_key;
            }
            entry.error = Some(message);
            inner.trace.push(entry);
            drop(inner);
            stop_shared.trace_changed();
            stop_shared.bump();
            true
        });
        register(Some(stop));
    }

    let partial = |shared: &Arc<RunShared>| -> WorkflowScriptPartial {
        let inner = shared.lock();
        WorkflowScriptPartial {
            emits: inner.emits.clone(),
            console: inner.console.clone(),
            trace: inner.trace.clone(),
            children: inner
                .child_order
                .iter()
                .filter_map(|key| inner.children.get(key).cloned())
                .collect(),
        }
    };

    // BLOCKER-1: the isolate is thread-confined, so it gets its own named OS thread with a
    // current-thread runtime. The WORK is not confined — ops forward to this runtime through the
    // captured `Handle`, so children run on the main multi-thread pool and `runs.all` fans out for
    // real. Same shape as R-ARCH-EXT-013's single-thread-per-`Store` confinement in `cyrup-ext`.
    let script = options.script.clone();
    let state_enabled = options.state.is_some();
    let isolate = match IsolateThread::spawn(shared.clone(), script, state_enabled) {
        Ok(isolate) => isolate,
        Err(error) => return Err(fail(error, None)),
    };
    let terminator = isolate.terminator();
    let guest_call = isolate.join();
    tokio::pin!(guest_call);

    enum GuestOutcome {
        Finished(Result<Result<String, WorkflowGuestError>, String>),
        TimedOut,
        Cancelled,
        Fatal,
    }

    let cancel = options.cancel.clone().unwrap_or_default();
    // §6 DL-2: upstream leaves `timeoutMs` optional (`:1894`) because a hung Node worker costs only
    // itself; here an unbounded run holds a live V8 isolate and an OS thread, so the default is real.
    let effective_timeout_ms = options.timeout_ms.unwrap_or(WORKFLOW_DEFAULT_TIMEOUT_MS);
    let mut outcome = {
        let timer = tokio::time::sleep(Duration::from_millis(effective_timeout_ms));
        tokio::pin!(timer);
        // §6 DL-13: `biased;` in upstream's precedence. `finish()` is strictly first-call-wins and
        // its callers cannot fire simultaneously in a single-threaded event loop; four ready arms in
        // an unbiased `select!` would make the SAME script return `Ok` or `timed out` at random.
        // Completion first: a script that returned has already produced its value.
        tokio::select! {
            biased;
            result = &mut guest_call => GuestOutcome::Finished(result),
            () = shared.fatal_signal.cancelled() => GuestOutcome::Fatal,
            () = cancel.cancelled() => GuestOutcome::Cancelled,
            () = &mut timer => GuestOutcome::TimedOut,
        }
    };
    // §6 DL-8: cancel children BEFORE the isolate goes down. Dropping the isolate first would
    // leave launched children running with no consumer.
    //
    // BLOCKER-2 governs the *other* half: `terminate_execution` poisons the isolate for every
    // later entry, so it must not fire while a re-entry is still possible. The assembly flush
    // below re-enters the guest, so a cancelled run is NOT terminated here — upstream orders it
    // the same way, deciding `mayFlushAssembly` inside `onAbort` before `finish` ever calls
    // `worker.terminate()`. A timeout or a fatal has no flush path, so those stop immediately.
    match outcome {
        GuestOutcome::Finished(_) => {}
        GuestOutcome::Cancelled => shared.child_cancel.cancel(),
        GuestOutcome::TimedOut | GuestOutcome::Fatal => {
            shared.child_cancel.cancel();
            terminator.terminate();
        }
    }

    // pi `onAbort` (`:1848-1892`): assembly-only flush when every launched child settled.
    if matches!(outcome, GuestOutcome::Cancelled) {
        let abort_message = "Workflow script aborted.".to_string();
        let all_settled = {
            let inner = shared.lock();
            !inner.launches.is_empty()
                && inner
                    .launches
                    .keys()
                    .all(|key| inner.children.contains_key(key))
        };
        let may_flush = options
            .continue_after_abort_when_children_settled
            .as_ref()
            .is_some_and(|callback| callback(&abort_message));
        if may_flush && all_settled {
            {
                let mut inner = shared.lock();
                inner.assembly_abort = true;
                inner.abort_error = Some(abort_message.clone());
            }
            shared.child_cancel.cancel();
            let flush = tokio::time::timeout(
                Duration::from_millis(WORKFLOW_ASSEMBLY_FLUSH_TIMEOUT_MS),
                &mut guest_call,
            )
            .await;
            outcome = match flush {
                Ok(result) => GuestOutcome::Finished(result),
                Err(_elapsed) => GuestOutcome::Cancelled,
            };
        }
    }
    // The flush window is closed, so no re-entry remains: stop a still-running isolate now.
    // Because this is the LAST touch, `cancel_terminate_execution` is unnecessary — there is no
    // subsequent entry for the poisoned state to break, and the partial is read from host-side
    // state (`RunShared`), never back out of the guest.
    if matches!(outcome, GuestOutcome::Cancelled) {
        terminator.terminate();
    }

    // Settlement: abort children, then wait for every outstanding op task (upstream's
    // `Promise.allSettled([...steers, ...hostCalls])`, `:1826`).
    {
        let mut inner = shared.lock();
        inner.finishing = true;
    }
    shared.child_cancel.cancel();
    // Upstream awaits ONLY steers+hosts (`:1826`); launch/status tasks are terminated, not
    // awaited — a hung child launcher must not hold settlement hostage (the deadlock a
    // cancel-deaf `launch` impl would otherwise cause).
    // §6 DL-15: release the stop closure BEFORE anything that can block. It captures the run
    // state, so a slow drain would otherwise keep the whole partial reachable from the embedder —
    // and the child token is already cancelled, so a late `stop()` has nothing left to stop.
    if let Some(register) = &options.register_stop_child {
        register(None);
    }
    // The `:1826` drain already happened, bounded, on the isolate thread where the ops live — see
    // `run_isolate`. Nothing to await here: dropping the isolate drops any op that outlived it.
    //
    // §5.5: flush the telemetry queue so a journal writer sees the final entries. Dropping the
    // last sender closes the channel, which ends the drain loop; the join is bounded for the same
    // reason the op drain is — the callbacks are embedder code.
    shared.emit_telemetry(TelemetryEvent::Flush);
    let _ = tokio::time::timeout(
        Duration::from_millis(WORKFLOW_SETTLE_DRAIN_TIMEOUT_MS),
        telemetry_drain,
    )
    .await;

    let fatal = shared
        .fatal
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let finish_error = |message: String, kind: Option<WorkflowScriptErrorKind>| {
        Box::new(WorkflowScriptError {
            message,
            partial: partial(&shared),
            error_kind: kind,
        })
    };

    match outcome {
        GuestOutcome::Fatal => {
            let failure = fatal.unwrap_or(PendingFailure {
                message: "Workflow script failed.".to_string(),
                error_kind: None,
            });
            Err(finish_error(failure.message, failure.error_kind))
        }
        GuestOutcome::TimedOut => Err(finish_error(
            format!("Workflow script timed out after {effective_timeout_ms}ms.",),
            Some(WorkflowScriptErrorKind::Timeout),
        )),
        GuestOutcome::Cancelled => {
            // §5.4: the instance is DROPPED (store goes down with this frame); children were
            // cancelled above and reaped by their own select arms. Mark still-running launches
            // stopped, as upstream's onAbort does (`:1874-1890`).
            let abort_message = "Workflow script aborted.".to_string();
            let stop_keys: Vec<String> = {
                let inner = shared.lock();
                inner
                    .launch_order
                    .iter()
                    .filter(|key| !inner.children.contains_key(*key))
                    .cloned()
                    .collect()
            };
            for key in stop_keys {
                let mut inner = shared.lock();
                inner.stopped_launches.insert(key.clone());
                let started = inner
                    .trace
                    .iter()
                    .rev()
                    .find(|entry| {
                        entry.operation == WorkflowScriptOperation::Run
                            && entry.key == key
                            && entry.state == WorkflowScriptTraceState::Started
                    })
                    .cloned();
                let mut entry = trace_entry(
                    WorkflowScriptOperation::Run,
                    &key,
                    WorkflowScriptTraceState::Stopped,
                );
                if let Some(started) = started {
                    entry.agent = started.agent;
                    entry.phase = started.phase;
                    entry.label = started.label;
                    entry.generated_lane_key = started.generated_lane_key;
                }
                entry.error = Some(abort_message.clone());
                inner.trace.push(entry);
            }
            shared.trace_changed();
            Err(finish_error(abort_message, None))
        }
        GuestOutcome::Finished(call_result) => {
            if let Some(failure) = fatal {
                // A fatal recorded by an import (emit persistence) preempts the guest outcome.
                return Err(finish_error(failure.message, failure.error_kind));
            }
            match call_result {
                Err(engine_error) => Err(finish_error(engine_error, None)),
                Ok(Err(envelope)) => {
                    let message = if envelope.error_phase.as_deref() == Some("return-serialization")
                    {
                        format!(
                            "{}{}",
                            envelope.message,
                            workflow_return_recovery_hint(&partial(&shared).children)
                        )
                    } else {
                        envelope.message
                    };
                    let kind = match envelope.error_kind.as_deref() {
                        Some("detached-child") => Some(WorkflowScriptErrorKind::DetachedChild),
                        _ => None,
                    };
                    Err(finish_error(message, kind))
                }
                Ok(Ok(value_json)) => {
                    let value: Value = match serde_json::from_str(&value_json) {
                        Ok(value) => value,
                        Err(error) => {
                            return Err(finish_error(
                                format!("Workflow return could not be persisted: {error}"),
                                None,
                            ));
                        }
                    };
                    if let Err(error) = assert_workflow_json_value(&value, "return") {
                        return Err(finish_error(
                            format!("Workflow return could not be persisted: {error}"),
                            None,
                        ));
                    }
                    // The settlement decision (SCOPE_3f §4.3): handle bookkeeping, one pure
                    // decision, three verbatim messages in run ▸ steer ▸ host precedence.
                    let (unobserved_runs, unobserved_steers, unobserved_hosts) = {
                        let inner = shared.lock();
                        (
                            inner
                                .launch_order
                                .iter()
                                .filter(|key| {
                                    inner
                                        .launches
                                        .get(*key)
                                        .is_some_and(|launch| !launch.observed)
                                })
                                .cloned()
                                .collect::<Vec<_>>(),
                            inner
                                .steer_order
                                .iter()
                                .filter_map(|handle| inner.steers.get(handle))
                                .filter(|record| !record.observed)
                                .map(|record| record.key.clone())
                                .collect::<Vec<_>>(),
                            inner
                                .host_order
                                .iter()
                                .filter_map(|handle| inner.hosts.get(handle))
                                .filter(|record| !record.observed)
                                .map(|record| record.key.clone())
                                .collect::<Vec<_>>(),
                        )
                    };
                    let settlement = CompletionSettlement::decide(
                        unobserved_runs,
                        unobserved_steers,
                        unobserved_hosts,
                    );
                    if let Some(message) = settlement.message() {
                        return Err(finish_error(message, None));
                    }
                    let partial = partial(&shared);
                    Ok(WorkflowScriptResult {
                        value,
                        emits: partial.emits,
                        console: partial.console,
                        trace: partial.trace,
                        children: partial.children,
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A launcher that records observed concurrency and settles children after a short hold, so
    /// the 12-way fan-out is MEASURED (BLOCKER-2 makes the failure silent otherwise).
    struct FakeHost {
        live: AtomicUsize,
        peak: AtomicUsize,
        hold: Duration,
        detach_keys: Vec<String>,
        fail_keys: Vec<String>,
    }

    impl FakeHost {
        fn new(hold: Duration) -> Self {
            Self {
                live: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                hold,
                detach_keys: Vec::new(),
                fail_keys: Vec::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl WorkflowScriptHost for FakeHost {
        async fn launch(
            &self,
            key: &str,
            params: Map<String, Value>,
            cancel: CancelToken,
            _admission: WorkflowLaunchAdmission,
        ) -> Result<WorkflowScriptChildResult, String> {
            let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(live, Ordering::SeqCst);
            // A well-behaved launcher observes the child token — the engine additionally
            // guarantees settlement is not hostage to one that does not (other_tasks abort).
            let cancelled = tokio::select! {
                () = tokio::time::sleep(self.hold) => false,
                () = cancel.cancelled() => true,
            };
            self.live.fetch_sub(1, Ordering::SeqCst);
            if cancelled {
                return Ok(WorkflowScriptChildResult {
                    key: key.to_string(),
                    ok: false,
                    stopped: true,
                    output: "Workflow script aborted.".into(),
                    error: Some("Workflow script aborted.".into()),
                    ..Default::default()
                });
            }
            let key_owned = key.to_string();
            if self.detach_keys.contains(&key_owned) {
                return Ok(WorkflowScriptChildResult {
                    key: key_owned.clone(),
                    ok: false,
                    detached: true,
                    output: "child detached".into(),
                    error: Some("child detached".into()),
                    ..Default::default()
                });
            }
            if self.fail_keys.contains(&key_owned) {
                return Ok(WorkflowScriptChildResult {
                    key: key_owned.clone(),
                    ok: false,
                    output: "child broke".into(),
                    error: Some("child broke".into()),
                    ..Default::default()
                });
            }
            let agent = params
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or("worker");
            Ok(WorkflowScriptChildResult {
                key: key_owned.clone(),
                ok: true,
                agent: Some(agent.to_string()),
                run_id: Some(format!("run-{key_owned}")),
                output: format!("output of {key_owned}"),
                ..Default::default()
            })
        }

        async fn status(
            &self,
            key_or_run_id: &str,
            _cancel: CancelToken,
        ) -> Result<WorkflowScriptChildResult, String> {
            Ok(WorkflowScriptChildResult {
                key: key_or_run_id.to_string(),
                ok: true,
                output: "running".into(),
                ..Default::default()
            })
        }
    }

    fn options(host: Arc<FakeHost>, script: &str) -> RunWorkflowScriptOptions {
        RunWorkflowScriptOptions {
            script: script.to_string(),
            one_use_permit: None,
            timeout_ms: Some(120_000),
            cancel: None,
            continue_after_abort_when_children_settled: None,
            global_concurrency_limit: None,
            host,
            state: None,
            register_stop_child: None,
            on_trace: None,
            on_lane_plan: None,
            on_emit: None,
            on_host_step: None,
        }
    }

    #[test]
    fn validate_reports_the_verbatim_findings() {
        let empty = validate_workflow_script("   ", false).unwrap();
        assert!(!empty.ok);
        assert_eq!(empty.errors[0].message, "workflowScript must not be empty.");

        let nested =
            validate_workflow_script("const f = async () => 1; return await f();", false).unwrap();
        assert!(!nested.ok);
        assert!(
            nested.errors[0]
                .message
                .starts_with("workflowScript does not support nested async functions."),
            "got: {}",
            nested.errors[0].message
        );

        let ok = validate_workflow_script(
            "const r = await runs.run(\"a\", { agent: \"worker\", task: \"T\" }); return { out: r.output };",
            false,
        )
        .unwrap();
        assert!(ok.ok, "expected clean validation, got {:?}", ok.errors);
    }

    /// §3.6 — the check upstream cannot perform. A script reaching for a global the sandbox does
    /// not have is caught at VALIDATE time, with a re-prompt, before a child is spent; upstream
    /// returns `ok: true` here and dies later as a runtime `ReferenceError`.
    #[test]
    fn validate_catches_unavailable_globals_before_a_child_is_spent() {
        let timer = validate_workflow_script(
            "await new Promise((r) => setTimeout(r, 1000)); return 1;",
            false,
        )
        .unwrap();
        assert!(!timer.ok);
        let message = &timer.errors[0].message;
        assert!(message.contains("'setTimeout'"), "got: {message}");
        assert!(
            message.contains("Workflows have no timer"),
            "the message must teach the rule: {message}"
        );

        let fetching =
            validate_workflow_script("const r = await fetch(\"http://x\"); return r;", false)
                .unwrap();
        assert!(!fetching.ok);
        assert!(fetching.errors[0].message.contains("'fetch'"));

        // Scope-aware: a local binding that shadows a global is NOT a missing global.
        let shadowed = validate_workflow_script(
            "const fetch = (u) => runs.run(\"f\", { agent: \"w\", task: u }); return await fetch(\"x\");",
            false,
        )
        .unwrap();
        assert!(shadowed.ok, "got: {:?}", shadowed.errors);
    }

    /// `state` exists only with a mission — the schema's "mission state when enabled".
    #[test]
    fn validate_admits_state_only_with_a_mission() {
        let script = "await state.set(\"k\", 1); return await state.get(\"k\");";
        assert!(validate_workflow_script(script, true).unwrap().ok);
        let without = validate_workflow_script(script, false).unwrap();
        assert!(!without.ok);
        assert!(without.errors[0].message.contains("require a mission"));
    }

    /// V8 owns syntax (§3.6): a body that does not parse is reported before the analyzer runs.
    #[test]
    fn validate_reports_syntax_from_the_engine_that_would_run_it() {
        let broken = validate_workflow_script("return {", false).unwrap();
        assert!(!broken.ok);
        assert!(!broken.errors[0].message.is_empty());
    }

    /// STRESS: many workflow isolates EXECUTING concurrently, overlapping each other and a stream
    /// of short-lived `validate` isolates being created and disposed on other threads.
    ///
    /// This is the shape production takes when a model emits two `subagent workflowScript` calls in
    /// one assistant message: the tool inherits `ExecMode::Parallel`, so `execute_parallel`
    /// `joinset.spawn`s both into the SAME process, and each `run_isolate` takes its own OS thread.
    /// Separate cwds and separate per-run tokio runtimes do not isolate V8 — `Isolate::New` and
    /// `Isolate::Dispose` mutate process-global state shared by every thread.
    ///
    /// An unsynchronized overlap does not fail an assertion here — it ABORTS the process (SIGABRT,
    /// libc++ `vector[] index out of bounds`), because a panic cannot unwind across the V8 FFI
    /// boundary. So the property under test is "the process survives", and the only way to observe
    /// a regression is that this test takes the whole harness down with it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_workflow_isolates_do_not_race_v8_global_state() {
        let script = r#"
const items = [];
for (let i = 0; i < 4; i++) items.push({ key: "c" + i, agent: "worker", task: "T" + i });
const results = await runs.all(items);
return { count: results.length };
"#;
        for round in 0..6 {
            let mut tasks: Vec<tokio::task::JoinHandle<Result<(), String>>> = Vec::new();
            // Long-lived isolates, executing guest script concurrently with each other.
            for _ in 0..6 {
                let host = Arc::new(FakeHost::new(Duration::from_millis(15)));
                let opts = options(host, script);
                tasks.push(tokio::spawn(async move {
                    run_workflow_script(opts)
                        .await
                        .map(|_| ())
                        .map_err(|error| error.message)
                }));
            }
            // Short-lived validate isolates created AND disposed while the above are mid-execution.
            for _ in 0..6 {
                tasks.push(tokio::spawn(async {
                    for _ in 0..4 {
                        validate_workflow_script("return 1;", false).map_err(|e| e.to_string())?;
                        validate_workflow_script("return {", false).map_err(|e| e.to_string())?;
                    }
                    Ok(())
                }));
            }
            for task in tasks {
                let settled = task.await.map_err(|e| e.to_string());
                assert!(settled.is_ok(), "round {round}: isolate task panicked");
                assert!(
                    settled.unwrap_or(Ok(())).is_ok(),
                    "round {round}: a concurrent workflow isolate failed"
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn twelve_children_run_concurrently_and_branching_works() {
        let host = Arc::new(FakeHost::new(Duration::from_millis(400)));
        let script = r#"
const items = [];
for (let i = 0; i < 12; i++) items.push({ key: "c" + i, agent: "worker", task: "T" + i });
const results = await runs.all(items);
emit({ first: results[0].output });
console.log("fanout", results.length);
if (results[11].ok) {
  const fix = await runs.run("fix", { agent: "worker", task: "Fix: " + results[11].output });
  return { shipped: true, fix: fix.output, count: results.length };
}
return { shipped: false };
"#;
        let result = run_workflow_script(options(host.clone(), script))
            .await
            .unwrap();
        assert_eq!(
            result.value,
            json!({ "shipped": true, "fix": "output of fix", "count": 12 })
        );
        // BLOCKER-2's regression: a suspending launch import would serialize this to peak == 1.
        let peak = host.peak.load(Ordering::SeqCst);
        assert!(
            peak >= 12,
            "expected 12 concurrent children, saw peak {peak}"
        );
        assert_eq!(result.emits, vec![json!({ "first": "output of c0" })]);
        assert_eq!(result.console.len(), 1);
        assert_eq!(result.console[0].level, WorkflowConsoleLevel::Log);
        assert_eq!(result.console[0].text, "fanout 12");
        assert_eq!(result.children.len(), 13);
        assert!(
            result
                .trace
                .iter()
                .filter(|entry| entry.state == WorkflowScriptTraceState::Completed)
                .count()
                >= 13
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unawaited_launches_fail_settlement_with_the_verbatim_message() {
        let host = Arc::new(FakeHost::new(Duration::from_millis(10)));
        let script = r#"
runs.run("orphan", { agent: "worker", task: "T" });
const kept = await runs.run("kept", { agent: "worker", task: "T" });
return { kept: kept.ok };
"#;
        let error = run_workflow_script(options(host, script))
            .await
            .unwrap_err();
        assert_eq!(
            error.message,
            "workflowScript completed with unawaited runs.run launch(es): 'orphan'. For ordinary parallel fanout use await runs.all([{key, agent, task}, ...]); do not read .output from unawaited launches."
        );
        assert_eq!(error.error_kind, None, "completion errors carry no kind");
        // The partial is the point: the trace and children survive the failure.
        assert!(
            error
                .partial
                .trace
                .iter()
                .any(|entry| entry.key == "orphan")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detached_child_rejects_with_the_kind_and_partial() {
        let mut host = FakeHost::new(Duration::from_millis(10));
        host.detach_keys.push("d".into());
        let host = Arc::new(host);
        let script = r#"return await runs.run("d", { agent: "worker", task: "T" });"#;
        let error = run_workflow_script(options(host, script))
            .await
            .unwrap_err();
        assert!(
            error.message.contains("Run 'd' detached: child detached"),
            "got: {}",
            error.message
        );
        assert_eq!(
            error.error_kind,
            Some(WorkflowScriptErrorKind::DetachedChild)
        );
        assert_eq!(error.partial.children.len(), 1);
        assert!(error.partial.children[0].detached);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn duplicate_keys_and_guest_validation_reject_synchronously() {
        let host = Arc::new(FakeHost::new(Duration::from_millis(10)));
        let script = r#"
let firstError = "";
try {
  await runs.run("k", { agent: "worker", task: "A" });
  await runs.run("k", { agent: "worker", task: "B" });
} catch (error) {
  firstError = error.message;
}
let badKey = "";
try {
  runs.run(".bad", { agent: "worker", task: "T" });
} catch (error) {
  badKey = error.message;
}
return { firstError, badKey };
"#;
        let result = run_workflow_script(options(host, script)).await.unwrap();
        let object = result.value.as_object().unwrap();
        assert_eq!(
            object.get("firstError").unwrap().as_str().unwrap(),
            "Duplicate workflow key 'k' used with incompatible launch params."
        );
        assert_eq!(
            object.get("badKey").unwrap().as_str().unwrap(),
            "runs.run has an invalid key."
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn workflow_timeout_is_tokio_not_epoch() {
        // A child that never settles: the guest blocks in poll (which epoch deliberately does not
        // interrupt, BLOCKER-1) and the run's tokio timeout is what fires.
        let host = Arc::new(FakeHost::new(Duration::from_secs(3600)));
        let mut opts = options(
            host,
            r#"return await runs.run("slow", { agent: "worker", task: "T" });"#,
        );
        opts.timeout_ms = Some(600);
        let error = run_workflow_script(opts).await.unwrap_err();
        assert_eq!(error.message, "Workflow script timed out after 600ms.");
        assert_eq!(error.error_kind, Some(WorkflowScriptErrorKind::Timeout));
        // The partial survived: the launch was traced before the deadline.
        assert!(error.partial.trace.iter().any(|entry| entry.key == "slow"));
    }

    /// A runaway is stopped by `terminate_execution`, which is the ONLY preemption V8 offers
    /// (BLOCKER-2). There is no epoch here: the wasm substrate had a 5 ms ticker and a ~5 s budget,
    /// and this design deliberately replaced it with the real thing upstream uses
    /// (`worker.terminate()`, `:1832`), driven by the run's own deadline (SCOPE_3f §6's liveness
    /// table: "running JS | the supervising task's IsolateHandle | terminate_execution").
    ///
    /// The assertion that matters is PROMPTNESS: a tight `for (;;) {}` never yields, so if the
    /// terminate did not actually interrupt running JS the isolate thread would spin forever and
    /// this test would hang rather than fail.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_runaway_loop_is_stopped_by_terminate_execution() {
        let host = Arc::new(FakeHost::new(Duration::from_millis(10)));
        let mut opts = options(host, "for (;;) {} return 1;");
        opts.timeout_ms = Some(600);
        let started = Instant::now();
        let error = run_workflow_script(opts).await.unwrap_err();
        let elapsed = started.elapsed();
        assert_eq!(error.message, "Workflow script timed out after 600ms.");
        assert_eq!(error.error_kind, Some(WorkflowScriptErrorKind::Timeout));
        assert!(
            elapsed < Duration::from_secs(20),
            "a non-yielding loop must be interrupted, not waited out; took {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn recovery_barrier_blocks_followups_after_rejected_acceptance() {
        struct RecoveryHost(FakeHost);
        #[async_trait::async_trait]
        impl WorkflowScriptHost for RecoveryHost {
            async fn launch(
                &self,
                key: &str,
                params: Map<String, Value>,
                cancel: CancelToken,
                admission: WorkflowLaunchAdmission,
            ) -> Result<WorkflowScriptChildResult, String> {
                if key == "gated" {
                    return Ok(WorkflowScriptChildResult {
                        key: key.to_string(),
                        ok: false,
                        output: "rejected".into(),
                        error: Some("rejected".into()),
                        recovery: Some(crate::workflows::AcceptanceRecoveryMetadata {
                            status: "available-for-review".into(),
                            reason: "acceptance-metadata-rejected".into(),
                            report_path: "report.md".into(),
                            report_hash: "hash".into(),
                        }),
                        ..Default::default()
                    });
                }
                self.0.launch(key, params, cancel, admission).await
            }
            async fn status(
                &self,
                key_or_run_id: &str,
                cancel: CancelToken,
            ) -> Result<WorkflowScriptChildResult, String> {
                self.0.status(key_or_run_id, cancel).await
            }
        }
        let host = Arc::new(RecoveryHost(FakeHost::new(Duration::from_millis(10))));
        let script = r#"
const gated = await runs.run("gated", { agent: "worker", task: "T" });
let barred = "";
try {
  await runs.run("after", { agent: "worker", task: "Fix everything." });
} catch (error) {
  barred = error.message;
}
const review = await runs.run("review1", { agent: "reviewer", task: "Review the saved report. Read-only: return findings only.", acceptance: false });
return { gatedOk: gated.ok, barred, reviewOk: review.ok };
"#;
        let mut opts = RunWorkflowScriptOptions {
            script: script.to_string(),
            one_use_permit: None,
            timeout_ms: Some(120_000),
            cancel: None,
            continue_after_abort_when_children_settled: None,
            global_concurrency_limit: None,
            host,
            state: None,
            register_stop_child: None,
            on_trace: None,
            on_lane_plan: None,
            on_emit: None,
            on_host_step: None,
        };
        opts.timeout_ms = Some(120_000);
        let result = run_workflow_script(opts).await.unwrap();
        let object = result.value.as_object().unwrap();
        assert_eq!(object.get("gatedOk"), Some(&json!(false)));
        // Upstream wraps the barrier rejection: the admission throw resolves into a FAILURE child
        // and `deliver` (`:2121-2133`) rethrows it as `Run '{key}' failed: {barrier message}` —
        // only `state.set`/`runs.steer`/`runs.host` barrier rejections surface bare.
        assert_eq!(
            object.get("barred").unwrap().as_str().unwrap(),
            format!(
                "Run 'after' failed: {}",
                recovery_barrier_message("gated", "after")
            )
        );
        assert_eq!(object.get("reviewOk"), Some(&json!(true)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn state_without_a_mission_uses_the_verbatim_refusal() {
        let host = Arc::new(FakeHost::new(Duration::from_millis(10)));
        // stateEnabled is false, so `state` is not even a global; a mission-less run that still
        // tries `state` sees a ReferenceError, matching upstream's sandbox omission of `state`.
        let script = r#"
let text = "";
try { await state.get("k"); } catch (error) { text = String(error && error.name); }
return { text };
"#;
        let result = run_workflow_script(options(host, script)).await.unwrap();
        assert_eq!(result.value, json!({ "text": "ReferenceError" }));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mission_state_round_trips_through_the_store() {
        struct MemoryState(Mutex<HashMap<String, Value>>);
        #[async_trait::async_trait]
        impl WorkflowStateStore for MemoryState {
            async fn get(&self, key: &str) -> Result<Option<Value>, String> {
                Ok(self.0.lock().unwrap().get(key).cloned())
            }
            async fn set(&self, key: &str, value: Value) -> Result<(), String> {
                self.0.lock().unwrap().insert(key.to_string(), value);
                Ok(())
            }
        }
        let host = Arc::new(FakeHost::new(Duration::from_millis(10)));
        let state = Arc::new(MemoryState(Mutex::new(HashMap::new())));
        let mut opts = options(
            host,
            r#"
await state.set("progress", { step: 2 });
const read = await state.get("progress");
const missing = await state.get("absent");
return { read, missing: missing === undefined ? "undefined" : "present" };
"#,
        );
        opts.state = Some(state);
        let result = run_workflow_script(opts).await.unwrap();
        assert_eq!(
            result.value,
            json!({ "read": { "step": 2 }, "missing": "undefined" })
        );
    }
}

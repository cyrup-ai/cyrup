//! The `workflowScript` `deno_core` extension — ops, the extension declaration, and `prelude.js` —
//! split out of `cyrup-ext-subagents` into its own crate so a `build.rs` can snapshot it
//! (WORKFLOW_1 SCOPE_3f §7).
//!
//! # Why this crate exists at all
//!
//! A Cargo build script is compiled and run as its own standalone binary **before** its own
//! crate's `src/` is compiled — it can never import a type or function from the crate whose build
//! it is running. The workflow ops need `cyrup-ext-subagents` internals (`RunShared`,
//! `LaunchEnvelope`, …) to do anything useful, so those ops cannot live in a crate that snapshots
//! itself. Splitting the ops + extension declaration out here, with zero dependency on
//! `cyrup-ext-subagents`, is what makes [`build_snapshot`] callable from a build script at all: a
//! **consumer's** `build.rs` (`cyrup-ext-subagents/build.rs`) adds this crate as a
//! `[build-dependencies]` entry — an entirely ordinary, non-circular edge — calls
//! [`build_snapshot`], and writes the bytes to its own `OUT_DIR`; its `src/` then `include_bytes!`s
//! them back for `RuntimeOptions.startup_snapshot`. This crate itself has no `build.rs`.
//!
//! # The crate boundary is [`WorkflowOpsBridge`], not embedder types
//!
//! The op bodies below never see `RunShared`, `LaunchEnvelope`, `WorkflowScriptChildResult`, or any
//! other `cyrup-ext-subagents` type — only `serde_json::Value` and primitives. The embedder
//! (`cyrup-ext-subagents`'s `engine.rs`) implements [`WorkflowOpsBridge`] on a thin wrapper around
//! its own `RunShared` and seeds it into [`deno_core::OpState`] itself, **after** `JsRuntime::new`
//! — never through this extension's `options`/`state`, because a snapshot reused across many
//! different workflow runs must never have any one run's state baked into it, and a build-time
//! snapshot pass has no live run to build one from in the first place.
//!
//! # No `options`/`state` on the `extension!` invocation — on purpose
//!
//! The identical [`cyrup_workflow`] value is used both to build the snapshot (no live run exists)
//! and at real runtime (a live run exists) — see [`build_snapshot`] and the `deno_core` mechanics
//! it relies on. `deno_core` matches extensions against a loaded snapshot's own sidecar data BY
//! NAME and automatically skips re-executing `js`/`esm` sources it already finds baked in
//! (`deno_core-0.411.0/extension_set.rs:316-401`), so the runtime side does not need — and must
//! not use — a JS-free variant of this same declaration.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{OpState, op2};
use deno_error::JsErrorBox;

/// Which kind of call an observation advisory refers to (SCOPE_3f §3.5).
///
/// Upstream marks a launch/steer/host observed the moment the script first attaches
/// `then`/`catch`/`finally` — synchronously, *before* any result exists. So observation cannot ride
/// on an op's return value; it is its own synchronous op, and the prelude calls it from the
/// facade's `then`. The host still owns the verdict: this only reports *that* the script consumed
/// the promise, and the embedder's own settlement logic decides what that means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObservationKind {
    /// A `runs.run` / `runs.all` / `runs.lanes` launch, keyed by workflow key.
    Run,
    /// A `runs.steer` call, keyed by the guest's correlation id.
    Steer,
    /// A `runs.host` call, keyed by the guest's correlation id.
    Host,
}

/// The capability surface an embedder grants a workflow run — the ONLY way these ops reach outside
/// this crate. Every method mirrors one `runs.*`/`state.*`/`emit`/`console.*` call the prelude
/// makes; structured payloads cross as `serde_json::Value` because the concrete types
/// (`LaunchEnvelope`, `WorkflowScriptChildResult`, …) belong to the embedder, not to this crate.
///
/// Object-safe by construction (`async_trait` boxes the async methods) because the embedder hands
/// this to `OpState` as `Arc<dyn WorkflowOpsBridge>`.
#[async_trait::async_trait]
pub trait WorkflowOpsBridge: Send + Sync + 'static {
    /// `runs.run(key, params)`, and each member of `runs.all([…])` / `runs.lanes([…])`. Returns the
    /// settled delivery — resolved child result, or a tagged rejection — as JSON; the prelude
    /// reconstructs the exact `Error` shape from it.
    async fn launch(&self, envelope: serde_json::Value) -> Result<serde_json::Value, String>;

    /// `runs.status(keyOrRunId)`.
    async fn status(&self, key_or_run_id: String) -> Result<serde_json::Value, String>;

    /// `runs.steer(key, message, options)`. `call_id` is guest-allocated (the prelude marks a
    /// steer observed synchronously, before any result exists, so the host needs a correlation id
    /// it did not mint itself).
    async fn steer(
        &self,
        call_id: u64,
        key: String,
        message: String,
        options: serde_json::Value,
    ) -> Result<serde_json::Value, String>;

    /// `runs.host(key, params)`. Registered as an op only for `workflow`-provenance runs; a raw
    /// `workflowScript` isolate never links this extension's host op path at all, which is how
    /// `runs.host` stays `undefined` rather than refused for that provenance.
    async fn host_command(
        &self,
        call_id: u64,
        key: String,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String>;

    /// Mark a launch/steer/host observed (§3.5). Synchronous and infallible: observing must never
    /// be able to fail a script for a transport error it did not cause. An unknown id is silently
    /// ignored for the same reason.
    fn observe(&self, kind: ObservationKind, key: String, call_id: u64);

    /// `runs.lanes` plan metadata — telemetry, queued rather than awaited, cannot fail the run.
    fn record_lane_plan(&self, lanes_json: String);

    /// `emit(value)` — the one telemetry-shaped call whose failure aborts the run.
    async fn emit(&self, value: serde_json::Value) -> Result<(), String>;

    /// Captured `console.*`. `level` is one of `log|info|warn|error`; anything else is dropped.
    fn record_console(&self, level: String, text: String);

    /// `state.get(key)`.
    async fn state_get(&self, key: String) -> Result<Option<serde_json::Value>, String>;

    /// `state.set(key, value)`.
    async fn state_set(&self, key: String, value: serde_json::Value) -> Result<(), String>;
}

/// Fetch the embedder's bridge out of `OpState`. Cloning an `Arc` rather than holding the borrow
/// across an `.await` — `OpState` is a `RefCell`, and holding it across a suspension point would
/// panic the moment two ops overlapped, which is the normal case here.
fn bridge(state: &Rc<RefCell<OpState>>) -> Arc<dyn WorkflowOpsBridge> {
    state
        .borrow()
        .borrow::<Arc<dyn WorkflowOpsBridge>>()
        .clone()
}

/// `runs.run(key, params)` / one member of `runs.all([...])`.
///
/// Returns the child's settled result. There is no handle, no poll, and no `subscribe`: the op
/// future IS the child's lifetime, and V8 resolves the JS promise when it completes. A 12-way
/// `runs.all` is 12 concurrent op futures on the parent runtime — real parallelism, with no
/// bookkeeping to get wrong.
#[op2]
#[serde]
pub async fn op_runs_launch(
    state: Rc<RefCell<OpState>>,
    #[serde] envelope: serde_json::Value,
) -> Result<serde_json::Value, JsErrorBox> {
    bridge(&state)
        .launch(envelope)
        .await
        .map_err(JsErrorBox::generic)
}

/// `runs.status(keyOrRunId)`.
#[op2]
#[serde]
pub async fn op_runs_status(
    state: Rc<RefCell<OpState>>,
    #[string] key_or_run_id: String,
) -> Result<serde_json::Value, JsErrorBox> {
    bridge(&state)
        .status(key_or_run_id)
        .await
        .map_err(JsErrorBox::generic)
}

/// `runs.steer(key, message, options)`.
#[op2]
#[serde]
pub async fn op_runs_steer(
    state: Rc<RefCell<OpState>>,
    #[bigint] call_id: u64,
    #[string] key: String,
    #[string] message: String,
    #[serde] options: serde_json::Value,
) -> Result<serde_json::Value, JsErrorBox> {
    bridge(&state)
        .steer(call_id, key, message, options)
        .await
        .map_err(JsErrorBox::generic)
}

/// `runs.host(key, params)`. Registered only for `workflow` provenance.
#[op2]
#[serde]
pub async fn op_runs_host(
    state: Rc<RefCell<OpState>>,
    #[bigint] call_id: u64,
    #[string] key: String,
    #[serde] params: serde_json::Value,
) -> Result<serde_json::Value, JsErrorBox> {
    bridge(&state)
        .host_command(call_id, key, params)
        .await
        .map_err(JsErrorBox::generic)
}

/// Mark a launch/steer/host observed (§3.5).
#[op2]
pub fn op_workflow_observe(
    state: &mut OpState,
    #[serde] kind: ObservationKind,
    #[string] key: &str,
    #[bigint] call_id: u64,
) {
    state
        .borrow::<Arc<dyn WorkflowOpsBridge>>()
        .clone()
        .observe(kind, key.to_string(), call_id);
}

/// `runs.lanes` plan metadata — upstream's `lanePlan` worker message.
///
/// Telemetry, so it is queued rather than awaited and cannot fail the run.
#[op2(fast)]
pub fn op_runs_lane_plan(state: &mut OpState, #[string] lanes_json: &str) {
    state
        .borrow::<Arc<dyn WorkflowOpsBridge>>()
        .clone()
        .record_lane_plan(lanes_json.to_string());
}

/// `emit(value)` — upstream's `emit` worker message.
///
/// The ONE telemetry-shaped callback that is awaited, because its failure aborts the run.
#[op2]
pub async fn op_workflow_emit(
    state: Rc<RefCell<OpState>>,
    #[serde] value: serde_json::Value,
) -> Result<(), JsErrorBox> {
    bridge(&state)
        .emit(value)
        .await
        .map_err(JsErrorBox::generic)
}

/// Captured `console.*`. `level` is one of `log|info|warn|error`; anything else is dropped, as
/// upstream drops it.
#[op2(fast)]
pub fn op_workflow_log(state: &mut OpState, #[string] level: &str, #[string] text: String) {
    state
        .borrow::<Arc<dyn WorkflowOpsBridge>>()
        .clone()
        .record_console(level.to_string(), text);
}

/// `state.get(key)`.
#[op2]
#[serde]
pub async fn op_workflow_state_get(
    state: Rc<RefCell<OpState>>,
    #[string] key: String,
) -> Result<Option<serde_json::Value>, JsErrorBox> {
    bridge(&state)
        .state_get(key)
        .await
        .map_err(JsErrorBox::generic)
}

/// `state.set(key, value)`.
#[op2]
pub async fn op_workflow_state_set(
    state: Rc<RefCell<OpState>>,
    #[string] key: String,
    #[serde] value: serde_json::Value,
) -> Result<(), JsErrorBox> {
    bridge(&state)
        .state_set(key, value)
        .await
        .map_err(JsErrorBox::generic)
}

deno_core::extension!(
    cyrup_workflow,
    ops = [
        op_runs_launch,
        op_runs_status,
        op_runs_steer,
        op_runs_host,
        op_workflow_observe,
        op_runs_lane_plan,
        op_workflow_emit,
        op_workflow_log,
        op_workflow_state_get,
        op_workflow_state_set,
    ],
    // `include_js_files!` resolves `dir` against THIS crate's `CARGO_MANIFEST_DIR` — true both when
    // `cyrup-ext-subagents/build.rs` calls `build_snapshot` (which passes it explicitly, below) and
    // at ordinary compilation (cargo always sets it for the crate being built).
    js = [ dir "src/js", "prelude.js" ],
    // Deliberately no `options`/`state`: see the module doc. The bridge to the embedding host is
    // seeded into `OpState` by the caller, as an `Arc<dyn WorkflowOpsBridge>`, after
    // `JsRuntime::new` — never here.
    docs = "The workflowScript runtime (SCOPE_3f §3.2 / WORKFLOW_1 §7). These ops are the ONLY \
            capability the agent's script can reach; `deno_runtime` is deliberately not a \
            dependency, so there is no fetch/fs/net/setTimeout to remove.",
);

/// Build the V8 startup snapshot for this extension (WORKFLOW_1 §7).
///
/// Call this from a **consumer's** `build.rs` — never from this crate's own, because a build
/// script can never import from the crate whose build it is running, which is the one fact this
/// whole crate split exists to work around (see the module doc). The consumer writes the returned
/// bytes to its OWN `OUT_DIR` and its runtime code `include_bytes!`s them back for
/// `RuntimeOptions.startup_snapshot`, supplying `extensions: vec![cyrup_workflow::init()]` again at
/// load time (`deno_core` recognises the already-snapshotted extension by name and skips
/// re-executing its JS; ops still bind normally either way — see the module doc).
///
/// # Errors
///
/// Returns `Err` only if V8 itself fails to produce a snapshot — a `deno_core`/V8 defect, not a
/// caller mistake.
pub fn build_snapshot() -> Result<(Box<[u8]>, Vec<std::path::PathBuf>), deno_core::error::CoreError>
{
    let output = deno_core::snapshot::create_snapshot(
        deno_core::snapshot::CreateSnapshotOptions {
            cargo_manifest_dir: env!("CARGO_MANIFEST_DIR"),
            startup_snapshot: None,
            skip_op_registration: false,
            extensions: vec![cyrup_workflow::init()],
            extension_transpiler: None,
            with_runtime_cb: None,
        },
        None,
    )?;
    Ok((output.output, output.files_loaded_during_snapshot))
}

//! The `deno_core` op layer — the ONLY capability the agent's script can reach (SCOPE_3f §3.2).
//!
//! # Why this file is thin
//!
//! `deno_core` ops are natively async on tokio, so each op here is a two-line adapter over the
//! `async fn` in [`super::engine`]. There is no handle registry, no `subscribe`/`result` protocol
//! and no guest pump, because **the op future is the operation's lifetime**: a `runs.all` of twelve
//! children is twelve concurrent futures on the parent runtime, with nothing to get wrong.
//!
//! # The capability boundary
//!
//! What is registered here is the whole of what a workflowScript can do. `deno_runtime` is not a
//! dependency, so there is no `fetch`, no `fs`, no `net` and no `setTimeout` to remove — the isolate
//! starts where upstream's `vm.createContext` starts (§0.3), and this table is the only thing added.
//!
//! [`op_runs_host`] is registered **only** for `workflow`-provenance runs (§0.4): a raw
//! `workflowScript` never links it, so `runs.host` is `undefined` there rather than refused, which
//! is upstream's rule (*"raw workflowScript/workflowScriptPath … cannot use runs.host"*).

use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpState, op2};
use deno_error::JsErrorBox;

use super::engine::{
    LaunchDelivery, LaunchEnvelope, WorkflowOpState, run_host_command, run_launch, run_status,
    run_steer,
};
use super::types::{WorkflowSteerOptions, WorkflowSteerResult};
use crate::workflows::{WorkflowHostCommandResult, WorkflowScriptChildResult};

/// Which kind of call an observation advisory refers to (§3.5).
///
/// Upstream marks a launch/steer/host observed the moment the script first attaches
/// `then`/`catch`/`finally` — synchronously, *before* any result exists. So observation cannot ride
/// on the op's return value; it is its own synchronous op, and the prelude calls it from the
/// facade's `then`. The host still owns the verdict: this only reports *that* the script consumed
/// the promise, and [`super::settlement`] decides what that means.
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

fn ctx(state: &Rc<RefCell<OpState>>) -> WorkflowOpState {
    state.borrow().borrow::<WorkflowOpState>().clone()
}

/// `runs.run(key, params)`, and each member of `runs.all([…])` / `runs.lanes([…])`.
///
/// Returns the settled delivery — resolved child result, or a rejection carrying upstream's
/// `detached-child` kind. The prelude turns the rejection back into an `Error` with the exact
/// wording and `.workflowErrorKind` upstream produces.
#[op2]
#[serde]
pub async fn op_runs_launch(
    state: Rc<RefCell<OpState>>,
    #[serde] envelope: LaunchEnvelope,
) -> Result<LaunchDelivery, JsErrorBox> {
    let ctx = ctx(&state);
    run_launch(ctx.shared(), envelope)
        .await
        .map_err(JsErrorBox::generic)
}

/// `runs.status(keyOrRunId)` — upstream `:2011-2027`.
#[op2]
#[serde]
pub async fn op_runs_status(
    state: Rc<RefCell<OpState>>,
    #[string] key_or_run_id: String,
) -> Result<WorkflowScriptChildResult, JsErrorBox> {
    let ctx = ctx(&state);
    run_status(ctx.shared(), &key_or_run_id)
        .await
        .map_err(JsErrorBox::generic)
}

/// `runs.steer(key, message, options)` — upstream `:2028-2059`.
#[op2]
#[serde]
pub async fn op_runs_steer(
    state: Rc<RefCell<OpState>>,
    #[bigint] call_id: u64,
    #[string] key: String,
    #[string] message: String,
    #[serde] options: WorkflowSteerOptions,
) -> Result<WorkflowSteerResult, JsErrorBox> {
    let ctx = ctx(&state);
    run_steer(ctx.shared(), call_id, &key, &message, options)
        .await
        .map_err(JsErrorBox::generic)
}

/// `runs.host(key, params)` — upstream `:2060-2110`. Registered only for `workflow` provenance.
#[op2]
#[serde]
pub async fn op_runs_host(
    state: Rc<RefCell<OpState>>,
    #[bigint] call_id: u64,
    #[string] key: String,
    #[serde] params: serde_json::Value,
) -> Result<WorkflowHostCommandResult, JsErrorBox> {
    let ctx = ctx(&state);
    run_host_command(ctx.shared(), call_id, &key, params)
        .await
        .map_err(JsErrorBox::generic)
}

/// Mark a launch/steer/host observed (§3.5).
///
/// Synchronous and infallible: it must not be possible for the act of observing to fail, or a
/// script would be punished at settlement for a transport error it did not cause. An unknown id is
/// silently ignored for the same reason.
#[op2]
pub fn op_workflow_observe(
    state: &mut OpState,
    #[serde] kind: ObservationKind,
    #[string] key: &str,
    #[bigint] call_id: u64,
) {
    let ctx = state.borrow::<WorkflowOpState>().clone();
    ctx.shared().mark_observed(kind, key, call_id);
}

/// `runs.lanes` plan metadata — upstream's `lanePlan` worker message (`:539-551`).
///
/// Telemetry, so it is queued rather than awaited and cannot fail the run (§5.5).
#[op2(fast)]
pub fn op_runs_lane_plan(state: &mut OpState, #[string] lanes_json: &str) {
    let ctx = state.borrow::<WorkflowOpState>().clone();
    ctx.shared().record_lane_plan(lanes_json);
}

/// `emit(value)` — upstream's `emit` worker message.
///
/// The ONE telemetry-shaped callback that is awaited, because its failure aborts the run
/// (`:1936-1944`). Async for exactly that reason (§5.5): a synchronous `Fn` would force an embedder
/// to block the isolate thread here.
#[op2]
pub async fn op_workflow_emit(
    state: Rc<RefCell<OpState>>,
    #[serde] value: serde_json::Value,
) -> Result<(), JsErrorBox> {
    let ctx = ctx(&state);
    ctx.shared().emit(value).await.map_err(JsErrorBox::generic)
}

/// Captured `console.*`. `level` is one of `log|info|warn|error`; anything else is dropped, as
/// upstream drops it.
#[op2(fast)]
pub fn op_workflow_log(state: &mut OpState, #[string] level: &str, #[string] text: String) {
    let ctx = state.borrow::<WorkflowOpState>().clone();
    ctx.shared().record_console(level, text);
}

/// `state.get(key)` — backed by `missions/workflow_state.rs`, whose module doc records that it was
/// built for this call site.
#[op2]
#[serde]
pub async fn op_workflow_state_get(
    state: Rc<RefCell<OpState>>,
    #[string] key: String,
) -> Result<Option<serde_json::Value>, JsErrorBox> {
    let ctx = ctx(&state);
    ctx.shared()
        .state_get(&key)
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
    let ctx = ctx(&state);
    ctx.shared()
        .state_set(&key, value)
        .await
        .map_err(JsErrorBox::generic)
}

//! The real [`WorkflowScriptHost`] — the bridge from a `workflowScript`'s `runs.*` calls to this
//! crate's own child execution (WORKFLOW_2).
//!
//! This is the file whose absence made the whole workflow runtime unreachable. It implements the
//! two REQUIRED trait methods and nothing else: every optional capability keeps the trait's
//! default, which is upstream's own "unavailable in this host" refusal.
//!
//! Note precisely what that buys, because it differs per capability: `runs.host` and `state` are
//! genuinely ABSENT from the guest realm (`cyrup-workflow-runtime`'s `js/prelude.js:574,578`),
//! while keyed resume is PRESENT AND REFUSING (`engine.rs:1105`, `:1908`). `runs.steer` is now
//! wired (WORKFLOW_14). Do not describe all four as "absent".

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cyrup_core::{CancelToken, ModelId, ToolUpdateSink};
use serde_json::{Map, Value};

use crate::background::control::{SteerAckState, SteerDeliveryMode};
use crate::exec::SingleResult;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::foreground_control::ForegroundChildSteerHandle;
use crate::extension::executor::requests::ForegroundRunRequest;
use crate::extension::tool::params::{SubagentToolParams, resolve_execution_agent_scope};
use crate::extension::tool::text::STEER_ACK_TIMEOUT;
use crate::fork_context::ContextRequest;
use crate::workflows::scripted::{
    WORKFLOW_CHILD_MARKER, WorkflowLaunchAdmission, WorkflowResolvedResume,
    WorkflowResolvedResumeReference, WorkflowResumeInput, WorkflowScriptHost, WorkflowSteerMode,
    WorkflowSteerOptions, WorkflowSteerResult, WorkflowSteerState, WorkflowSteerTarget,
};
use crate::workflows::{
    WorkflowContinuation, WorkflowKey, WorkflowLaneMetadata, WorkflowRequestedContext,
    WorkflowResumability, WorkflowScriptChildResult, assert_workflow_lane_key,
    normalize_workflow_lane_metadata, workflow_terminal_outcome_for_result,
};

/// Set on every workflow child's environment so the child's own `subagent` tool can refuse a
/// nested `workflowScript` (WORKFLOW_1 §6.3).
///
/// The marker cannot travel in the request — a model-authored tool call will never carry it — so
/// the env is the transport and `extension/tool/mod.rs` stamps it back onto the request map before
/// [`crate::workflows::scripted::refuse_nested_workflow`] runs. This is the half of §6.3 that
/// WORKFLOW_1 left unwired: as shipped, the only producer of the marker was the engine itself
/// (`engine.rs:1905`), so `refuse_nested_workflow` could only ever fire on a request the host had
/// synthesized in-process.
///
/// It rides on the CHILD's `Command`, never on this process's environment — which is both
/// [`crate::exec::RunOptions::child_env`]'s own documented contract (`exec/agent_config.rs:686-706`)
/// and the reason `clippy.toml` bans `std::env::set_var` outright.
pub const WORKFLOW_CHILD_ENV: &str = "CYRUP_WORKFLOW_CHILD";

/// The host's share of the one host [`ToolUpdateSink`] this tool call owns.
///
/// [`ToolUpdateSink`] is `Box<dyn FnMut(ToolUpdate) + Send + 'static>` (`cyrup-core/src/tool.rs:123`)
/// — NOT `Clone` and NOT `Sync` — but [`WorkflowScriptHost`] requires `Send + Sync`
/// (`engine.rs:122`) and every child launch needs its OWN sink because
/// `run_foreground_streaming` consumes one by value. `Arc<Mutex<…>>` is what makes the single sink
/// both shareable and `Sync`; [`child_sink`] mints a fresh forwarding `Box` per child that
/// re-locks on each update.
type SharedUpdateSink = Arc<Mutex<ToolUpdateSink>>;

/// Mint a fresh forwarding [`ToolUpdateSink`] for one child, sharing the host's single real sink.
///
/// The lock is held only for the duration of one synchronous `FnMut` call and is never held across
/// an `.await`, so N concurrent children serialise their UI updates and nothing else.
fn child_sink(shared: &SharedUpdateSink) -> ToolUpdateSink {
    let shared = Arc::clone(shared);
    Box::new(move |update| {
        // `unwrap_used`/`expect_used` are DENY workspace-wide (`Cargo.toml:101-104`): a poisoned
        // sink drops the update rather than panicking inside a child's progress fold.
        if let Ok(mut sink) = shared.lock() {
            sink(update);
        }
    })
}

/// The identity fields the engine's auto-resume relaunch drops.
///
/// The engine relaunches the SAME key on auto-resume (`engine.rs:2045`) with params rebuilt from
/// an 18-key whitelist (`engine.rs:755-773`) that contains NEITHER `agent` NOR
/// `model`/`context`/`agentScope`. A host that reads those only from the params therefore refuses
/// its own retry, turning a recoverable transient abort into a hard child failure.
#[derive(Clone)]
struct LaunchIdentity {
    agent: String,
    model: Option<String>,
    agent_scope: Option<String>,
    context: Option<ContextRequest>,
    /// Stable flat child index for steer routing (WORKFLOW_14). Minted once per key on first
    /// launch and reused on auto-resume relaunch of the same key.
    index: usize,
}

/// One workflow run's launch bridge.
pub(crate) struct WorkflowRunHost {
    /// Shared with the tool — `SubagentTool` already holds an `Arc<SubagentExecutor>`, so the host
    /// clones the same handle rather than borrowing.
    executor: Arc<SubagentExecutor>,
    /// The request cwd, resolved once by `Tool::execute`'s `resolve_requested_cwd`
    /// (`extension/tool/mod.rs:205`) — never re-derived here.
    cwd: PathBuf,
    /// The host update sink, wrapped per [`SharedUpdateSink`], so a workflow child streams live
    /// progress exactly as a SINGLE run's child does (C19, `run_foreground_streaming`). Note this
    /// makes WORKFLOW the SECOND streaming mode: `route_parallel_mode`/`route_chain_mode` still
    /// take no `on_update` at all.
    on_update: SharedUpdateSink,
    /// Settled children by key, in launch order — the ONLY thing [`WorkflowScriptHost::status`]
    /// can answer from in a foreground host, because a launch does not return until its child
    /// settles. A `Vec`, not a map, because `runs.status` may be asked by key OR by run id
    /// (`prelude.js:311`'s `status(keyOrRunId)`) and the order is the launch order the engine's
    /// own `child_order` records.
    ///
    /// `std::sync::Mutex`, not `tokio`'s: every access is a short synchronous read/upsert with no
    /// `.await` inside the critical section.
    ///
    /// ⚠ UPSERTED BY KEY, never blindly pushed. The engine relaunches the SAME key on auto-resume
    /// (`engine.rs:2045`), and a duplicate entry would make `status` answer from the stale first
    /// attempt forever.
    settled: Mutex<Vec<WorkflowScriptChildResult>>,
    /// What each key was FIRST launched with — see [`LaunchIdentity`].
    launched: Mutex<HashMap<String, LaunchIdentity>>,
    /// WORKFLOW_6 §4.3 — the ONE workflow run id for the whole call (minted by `routing.rs` BEFORE
    /// this host is constructed), stamped onto every child this host launches as
    /// `parent_workflow_run_id` (pi `prepareWorkflowChildLaunchParams`'s `parentWorkflowRunId:
    /// workflowRunId`, `subagent-executor.ts:5633`).
    workflow_run_id: crate::background::RunId,

    /// The run record this workflow is writing (§3.1). SHARED with `routing.rs`, because §3.2's
    /// terminal write must start from the SAME status this host has been refreshing — not a second
    /// one that would clobber it.
    status: Arc<Mutex<crate::background::RunStatus>>,
    /// Where that record lives: `run_dir_name.resolve_in(&async_root)`, already created by §3.1.
    run_dir: PathBuf,
    /// The async root under which *any* workflow receipt may be found. Required by
    /// `resolve_resume` because the reference may name a *different* workflow's run dir
    /// (W13 §3.1 already resolves it once per `route_workflow_mode`).
    async_root: PathBuf,
    /// Monotonic flat child index source for steer handles (WORKFLOW_14). Assigned at first launch
    /// of a key and reused on auto-resume; never re-assigned.
    next_child_index: AtomicUsize,
    /// Serializes republish WRITES. `runs.all` settles children concurrently and the inventory grows
    /// monotonically — without this, an older snapshot can land after a newer one and drop a step.
    /// `tokio::sync::Mutex` because it is held across the `.await` on the write; the two `std`
    /// mutexes above are not.
    publish_lock: tokio::sync::Mutex<()>,
}

impl WorkflowRunHost {
    /// Build the host for one `workflowScript` tool call.
    ///
    /// `on_update` is MOVED in, not cloned: the tool call owns exactly one host sink, and the
    /// `Arc<Mutex<…>>` wrap performed here is what lets every child borrow a forwarding share of
    /// it (see [`SharedUpdateSink`]).
    pub(crate) fn new(
        executor: Arc<SubagentExecutor>,
        cwd: PathBuf,
        on_update: ToolUpdateSink,
        workflow_run_id: crate::background::RunId,
        status: Arc<Mutex<crate::background::RunStatus>>,
        run_dir: PathBuf,
        async_root: PathBuf,
    ) -> Self {
        Self {
            executor,
            cwd,
            on_update: Arc::new(Mutex::new(on_update)),
            settled: Mutex::new(Vec::new()),
            launched: Mutex::new(HashMap::new()),
            workflow_run_id,
            status,
            run_dir,
            async_root,
            next_child_index: AtomicUsize::new(0),
            publish_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// The ONE place a child's steer handle is built — used by both `launch` (which hands it to
    /// the spawn, so `RunOptions::steer_inbox_dir` IS `handle.inbox_dir`) and `steer` (which writes
    /// into it). A single construction point is what makes "the parent writes where the child
    /// reads" true by construction rather than by two derivations agreeing.
    fn child_steer_handle(&self, index: usize) -> ForegroundChildSteerHandle {
        ForegroundChildSteerHandle {
            inbox_dir: crate::background::control::step_steer_inbox_dir(&self.run_dir, index),
            run_dir: self.run_dir.clone(),
            index,
        }
    }

    /// Republish `status.steps` from the settled inventory — pi's `syncCurrentChild`
    /// (`foreground-control.ts:39-60`) for a workflow's discovered children.
    ///
    /// Best-effort: a workflow whose progress write fails still returns its child's real result. This
    /// is a progress record, never an outcome — §3.2's terminal write is the outcome, and it rebuilds
    /// `steps` from the engine's own complete child list regardless of what landed here.
    async fn publish_steps(&self) {
        // Held across the write: snapshot and write must be ONE critical section, or concurrent
        // `runs.all` children can reorder their writes and a later write can carry fewer steps.
        let _write = self.publish_lock.lock().await;
        let snapshot = {
            let Ok(settled) = self.settled.lock() else {
                return;
            };
            let steps = crate::workflows::workflow_step_statuses(&settled);
            let Ok(mut status) = self.status.lock() else {
                return;
            };
            status.steps = steps;
            status.current_step = status.steps.len().checked_sub(1);
            // NOT `advance_state` — the state is not changing. `touch()` (`records.rs:471`) is the
            // sanctioned "progress without transition" refresh; assign directly only if `touch` is
            // unavailable at this visibility.
            status.touch();
            status.clone()
            // BOTH std::sync guards drop HERE, before the await below.
        };
        let _ = crate::background::atomic::write_atomic_json(
            &self.run_dir.join("status.json"),
            &snapshot,
        )
        .await;
    }

    /// Guest `params.lane` → typed metadata. Invalid lane fails the launch BEFORE spawn so a
    /// mismatched key cannot reach `build_workflow_receipt` Rule 5. Absent lane is `Ok(None)`.
    fn parse_child_lane(
        key: &str,
        raw: Option<&serde_json::Value>,
    ) -> Result<Option<WorkflowLaneMetadata>, String> {
        let lane = normalize_workflow_lane_metadata(raw, &format!("runs.run('{key}') lane"))
            .map_err(|e| e.to_string())?;
        if let Some(ref lane) = lane {
            let parsed = WorkflowKey::parse(key)
                .map_err(|_| format!("runs.run('{key}') has an invalid key."))?;
            assert_workflow_lane_key(
                Some(lane),
                Some(&parsed),
                &format!("runs.run('{key}') lane"),
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(lane)
    }

    /// First-launch `context` plus the auto-resume remembered request. `Profile` has no
    /// `WorkflowRequestedContext` counterpart — `None` is correct.
    fn requested_context_for(
        &self,
        key: &str,
        child: &SubagentToolParams,
    ) -> Option<WorkflowRequestedContext> {
        let request = child.context_override().or_else(|| {
            self.launched
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(key)
                .and_then(|id| id.context)
        })?;
        match request {
            ContextRequest::Fresh => Some(WorkflowRequestedContext::Fresh),
            ContextRequest::Fork => Some(WorkflowRequestedContext::Fork),
            ContextRequest::Profile => None,
        }
    }

    /// Map one settled [`SingleResult`] onto the wire shape the engine hands back to the guest.
    fn map_child_result(
        &self,
        key: &str,
        run_id: &crate::background::RunId,
        child: &SubagentToolParams,
        result: &SingleResult,
        lane: Option<WorkflowLaneMetadata>,
    ) -> WorkflowScriptChildResult {
        WorkflowScriptChildResult {
            key: key.to_string(),
            // `applyDetachedChildSettlement`'s own `succeeded` predicate. Deliberately NOT
            // `crate::tui::intercom::resolve_subagent_result_status`, which answers a different
            // question (it classifies for RENDERING, not for settlement).
            ok: result.exit_code == 0 && result.error.is_none() && !result.interrupted,
            stopped: result.stopped,
            // `SingleResult::agent` is a `String`, not an `Option<String>`.
            agent: Some(result.agent.clone()),
            // The run's own real id, returned alongside the result by `run_foreground_streaming`.
            // NOT `result.child_run_id`, which is `None` on every foreground run by construction.
            run_id: Some(run_id.to_string()),
            // `final_output` IS `assemble_delivered_output`'s output — that function is a pure
            // tail which already ran inside `run_sync`, is `pub(crate)` to `exec`, and needs
            // ladder state a settled `SingleResult` does not carry. So it cannot be called here,
            // and does not need to be.
            output: result.final_output.clone().unwrap_or_default(),
            error: result.error.clone(),
            detached: result.detached,
            interrupted: result.interrupted,
            structured_output: result.structured_output.clone(),
            lane,
            terminal_outcome: workflow_terminal_outcome_for_result(
                crate::workflows::WorkflowBudgetSignals::from_single_result(result),
            ),
            requested_context: self.requested_context_for(key, child),
            // CUT: SingleResult has no per-attempt context flag and ModelAttempt
            // (exec/fallback.rs:1144-1150) has none either. WorkflowResolvedContext::Mixed
            // (types.rs:653-665) is underivable. A two-valued fill that can never produce Mixed
            // would be trusted and wrong. See WORKFLOW_15 §1.6.
            resolved_context: None,
            output_reference: result.saved_output_path.clone(),
            // CUT: AcceptanceRecoveryMetadata requires report_path + report_hash (types.rs:618-630).
            // AcceptanceLedger (exec/acceptance/model/types.rs:575) has neither; the only writers
            // of those two fields in this crate are tests (scripted/recovery.rs:735, engine.rs:3492).
            // See WORKFLOW_15 §1.6.
            recovery: None,
            // Settled None: this host never relocates a child's output. saved_output_path already
            // populates output_reference.
            output_path_mapping: None,
            // Unported family; the field's own doc says so.
            external_adapter: None,
            resumability: Some(match result.session_file.as_ref() {
                Some(_) => WorkflowResumability::Resumable,
                None if result.stopped => WorkflowResumability::NotResumable {
                    reason: "child was stopped before a session was persisted".to_string(),
                },
                None if result.interrupted => WorkflowResumability::NotResumable {
                    reason: "child was interrupted before a session was persisted".to_string(),
                },
                None => WorkflowResumability::NotResumable {
                    reason: "child persisted no session file".to_string(),
                },
            }),
            continuation: Some(WorkflowContinuation {
                run_ids: vec![run_id.to_string()],
            }),
            // `SingleResult::artifact_paths` is `Option<ArtifactPaths>` — five NAMED `PathBuf`s,
            // not a list. Flattened in the struct's own declaration order (input, output, jsonl,
            // metadata, transcript) so the wire list is stable across runs. `to_string_lossy`
            // rather than `to_str` + filter: a non-UTF-8 artifact path must still be reported,
            // not silently dropped from the inventory.
            artifact_paths: result
                .artifact_paths
                .as_ref()
                .map(|paths| {
                    [
                        &paths.input_path,
                        &paths.output_path,
                        &paths.jsonl_path,
                        &paths.metadata_path,
                        &paths.transcript_path,
                    ]
                    .into_iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
                })
                .unwrap_or_default(),
            // `unwrap_or_default()`, never `?` or `.unwrap()`: `unwrap_used`/`expect_used` are
            // DENY (`Cargo.toml:101-102`), and a serialization failure must degrade the summary,
            // not fail the child. The engine's summary builder reads
            // `sessionName`/`model`/`thinking`/`acceptance.status` off this first element.
            results: serde_json::to_value(result)
                .map(|v| vec![v])
                .unwrap_or_default(),
        }
    }
}

#[async_trait::async_trait]
impl WorkflowScriptHost for WorkflowRunHost {
    async fn launch(
        &self,
        key: &str,
        params: Map<String, Value>,
        cancel: CancelToken,
        _admission: WorkflowLaunchAdmission,
    ) -> Result<WorkflowScriptChildResult, String> {
        // Strip the engine's own marker first: it is host metadata (inserted at `engine.rs:1905`),
        // not a child param, and it must reach the child as ENV below, never as a task argument.
        let mut params = params;
        params.remove(WORKFLOW_CHILD_MARKER);

        let lane = Self::parse_child_lane(key, params.get("lane"))?;

        // The guest has already rejected `workflowScript` (`prelude.js:186`), `action` (`:189`)
        // and `tasks`/`chain`/`parallel`/`concurrency`/`chainDir` (`:191-195`) on a launch, so
        // what remains IS this tool's own SINGLE param shape. Parsing it as `SubagentToolParams`
        // is what lets `single_run_overrides` apply verbatim, so every SINGLE-mode feature
        // reaches a workflow child for free rather than through a narrower hand-rolled struct.
        let child: SubagentToolParams = serde_json::from_value(Value::Object(params))
            .map_err(|e| format!("runs.run('{key}') params are invalid: {e}"))?;

        if child.r#async == Some(true) {
            return Err(format!(
                "runs.run('{key}') cannot request async:true inside a workflow; the workflow owns \
                 its children's lifetime. Omit async or pass async:false."
            ));
        }

        // ⚠ THE AUTO-RESUME FALLBACK. The engine relaunches this key (`engine.rs:2045`) with
        // params rebuilt from an 18-key whitelist (`engine.rs:755-773`) that carries NO `agent`,
        // `model`, `context` or `agentScope`. Reading those from the params alone refuses the
        // engine's own retry and turns a transient provider abort into a failed child.
        //
        // Why this path is genuinely reachable, and why it may get MORE reachable without warning:
        // the decision gate is `error == "This operation was aborted" && is_zero_usage(
        // results[0].usage)` (`engine.rs:794-795`). Its message-shape guards (`:807-824`) are
        // BYPASSED here because they read `messages` off `results[0]` and `SingleResult` is the
        // compacted shape with no `messages` field at all — so the path is strictly more reachable
        // in cyrup than upstream. The usage half passes today because `Usage` is
        // `rename_all = "camelCase"` and so serializes the exact five keys `is_zero_usage` reads
        // (`input`/`output`/`cacheRead`/`cacheWrite`/`cost.total`). That helper resolves a MISSING
        // key to zero, so if `Usage`'s serde renaming ever changes, every lookup misses,
        // `is_zero_usage` becomes unconditionally `true`, and auto-resume fires for EVERY aborted
        // child rather than only zero-usage setup aborts. This fallback must be correct, not
        // merely present.
        let remembered = self
            .launched
            .lock()
            .ok()
            .and_then(|map| map.get(key).cloned());
        let Some(agent) = child
            .agent
            .clone()
            .or_else(|| remembered.as_ref().map(|id| id.agent.clone()))
        else {
            // `route_single`'s own message shape, retargeted at the guest call that caused it.
            return Err(format!("runs.run('{key}') requires an 'agent' name."));
        };
        let model = child
            .model
            .clone()
            .or_else(|| remembered.as_ref().and_then(|id| id.model.clone()));
        let agent_scope = child
            .agent_scope
            .clone()
            .or_else(|| remembered.as_ref().and_then(|id| id.agent_scope.clone()));
        let context = child
            .context_override()
            .or_else(|| remembered.as_ref().and_then(|id| id.context));

        // Record the identity on the FIRST launch only — never overwrite, so an auto-resume retry
        // (`engine.rs`'s relaunch of the SAME key) cannot erase what it is trying to recover.
        //
        // WORKFLOW_14: the index is minted AND READ BACK inside ONE lock section. A `fetch_add`
        // outside the lock can hand this launch an index that `or_insert_with` then declines to
        // store — `runs.all` launches concurrently, so another launch of this key can land between
        // a lock-free `remembered` read and the insert. The child would then be spawned against
        // `steer-targets/<minted>/` while `runs.steer(key)` resolves the map's different index:
        // one logical child, two control identities. `remembered` above stays exactly as it is for
        // the agent/model/scope/context fallbacks; only the index resolution is atomic.
        let index = {
            let mut launched = self.launched.lock().unwrap_or_else(PoisonError::into_inner);
            match launched.entry(key.to_string()) {
                Entry::Occupied(existing) => existing.get().index,
                Entry::Vacant(slot) => {
                    let idx = self.next_child_index.fetch_add(1, Ordering::SeqCst);
                    slot.insert(LaunchIdentity {
                        agent: agent.clone(),
                        model: model.clone(),
                        agent_scope: agent_scope.clone(),
                        context,
                        index: idx,
                    })
                    .index
                }
            }
        };

        let mut overrides = crate::extension::tool::SubagentTool::single_run_overrides(&child)
            .map_err(|e| e.to_string())?;
        // The child-marker transport: it rides on the CHILD's `Command`, never on this process's
        // environment. This is `RunOptions::child_env`'s first production caller.
        overrides
            .child_env
            .insert(WORKFLOW_CHILD_ENV.to_string(), "1".to_string());

        // `ForegroundRunRequest<'a>` BORROWS `agent_name`/`task`/`cwd`, so both owned locals must
        // outlive the `.await` below.
        let task = child.task.clone().unwrap_or_default();

        let (result, run_id) = self
            .executor
            .run_foreground_streaming(
                ForegroundRunRequest {
                    overrides,
                    cwd: &self.cwd,
                    // `&agent`, NOT `agent`: the field is `&'a str`. `route_single` writes a bare
                    // `agent` only because its `agent` is already a `&str` from
                    // `p.agent.as_deref()`; the auto-resume fallback above makes this one an owned
                    // `String`, so it must be borrowed here.
                    agent_name: &agent,
                    task: &task,
                    // From the resolved bindings above, NOT from `child.*` — the auto-resume
                    // relaunch carries none of these four.
                    agent_scope: resolve_execution_agent_scope(agent_scope.as_deref()),
                    context,
                    model_override: model.map(ModelId::from),
                    // The RAW alias fold, deliberately NOT `resolve_foreground_timeout(p,
                    // foreground_timeout_default(...))` as `route_single` uses: that ladder ends in
                    // `.or(Some(DEFAULT_FOREGROUND_TIMEOUT_MS))`, which would give a workflow child
                    // a longer budget than the workflow that owns it. A workflow child's ceiling is
                    // the workflow's own `timeout_ms`, applied by the engine's whole-run
                    // `tokio::time::timeout`.
                    timeout_ms: child.timeout_ms.or(child.max_runtime_ms),
                    // The workflow's own token, threaded down: `runs.stopChild` and the run-level
                    // timeout both cancel through it (the engine hands `launch` a per-key CHILD
                    // token), and the engine guarantees settlement is not hostage to a launcher
                    // that ignores it.
                    cancel,
                    // WORKFLOW_6 §4.3 — pi `prepareWorkflowChildLaunchParams({ parentWorkflowRunId:
                    // workflowRunId, workflowKey: key, ... })` (`subagent-executor.ts:5633`). This is
                    // what makes `controlIsLiveInWorkflow` (WORKFLOW_7) and S6's filter (WORKFLOW_11)
                    // able to find this child at all.
                    parent_workflow_run_id: Some(self.workflow_run_id.clone()),
                    // `key` is the engine's lane key. `WorkflowKey::parse` is the only constructor
                    // and it is fallible (`workflows/key.rs:21`); a key the engine generated always
                    // parses, and `None` on the impossible branch degrades the entry's provenance
                    // rather than failing a child that is otherwise fine.
                    workflow_key: crate::workflows::WorkflowKey::parse(key).ok(),
                    // WORKFLOW_14 — built at the ONE construction point, so the path the child is
                    // spawned watching and the path a steer is written into are the same value
                    // rather than two agreeing derivations. Set in the SAME literal as
                    // `parent_workflow_run_id` so the "`Some` iff workflow-owned" invariant is
                    // visible at a glance rather than asserted elsewhere.
                    workflow_steer: Some(self.child_steer_handle(index)),
                },
                // A FRESH forwarding sink per child, because the real sink is `Box<dyn FnMut>` and
                // `run_foreground_streaming` consumes one.
                child_sink(&self.on_update),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mapped = self.map_child_result(key, &run_id, &child, &result, lane);

        // Upsert in place, never push. Upsert-IN-PLACE rather than remove+push, because launch
        // order is what `status` and the engine's own `child_order` both assume.
        if let Ok(mut settled) = self.settled.lock() {
            match settled.iter_mut().find(|c| c.key == mapped.key) {
                Some(existing) => *existing = mapped.clone(),
                None => settled.push(mapped.clone()),
            }
        }

        // §3.3 — republish immediately after each child settles (before returning to engine).
        self.publish_steps().await;
        Ok(mapped)
    }

    /// A foreground host has no live child registry — a launch does not return until its child
    /// settles — so this answers from the settled list only, by key first and then by run id (the
    /// guest passes either, `prelude.js:311`'s `status(keyOrRunId)`, which coerces a non-string
    /// to `""`).
    ///
    /// An unknown key is an ERROR, not an empty result: `runs.status` on a child that was never
    /// launched is a script bug, and the message is a re-prompt.
    async fn status(
        &self,
        key_or_run_id: &str,
        _cancel: CancelToken,
    ) -> Result<WorkflowScriptChildResult, String> {
        let found = self.settled.lock().ok().and_then(|settled| {
            settled
                .iter()
                .find(|c| c.key == key_or_run_id)
                .or_else(|| {
                    settled
                        .iter()
                        .find(|c| c.run_id.as_deref() == Some(key_or_run_id))
                })
                .cloned()
        });
        found.ok_or_else(|| {
            format!("runs.status('{key_or_run_id}') names no launched child in this workflow.")
        })
    }

    /// Keyed receipt resume is available because this host writes receipts into a real run dir
    /// (WORKFLOW_13) and records per-child resumability (WORKFLOW_15). Both are load-bearing: without
    /// the first there is no receipt to read, without the second every entry is `not-resumable`.
    fn supports_resolve_resume(&self) -> bool {
        true
    }

    async fn resolve_resume(
        &self,
        reference: WorkflowResumeInput,
        _cancel: CancelToken,
        _index: Option<u64>,
    ) -> Result<WorkflowResolvedResume, String> {
        match reference {
            WorkflowResumeInput::RunId(run_id) => Ok(WorkflowResolvedResume::RunId(run_id)),

            WorkflowResumeInput::Reference(reference) => {
                let async_root = self.async_root.clone();
                let resolved_entry = tokio::task::spawn_blocking(move || {
                    crate::workflows::resolve_workflow_receipt_resume_entry(
                        crate::workflows::ResolveWorkflowReceiptResume {
                            reference: &reference,
                            async_root: &async_root,
                            assert_resumable: None,
                        },
                    )
                })
                .await
                .map_err(|e| format!("workflow receipt resume resolution panicked: {e}"))
                .and_then(|inner| {
                    inner.map_err(|e| format!("workflow receipt resume resolution failed: {e}"))
                })?;

                let resolved_ref = WorkflowResolvedResumeReference {
                    run_id: resolved_entry.latest_run_id().to_string(),
                    run_ids: Some(resolved_entry.entry().continuation.run_ids.clone()),
                };
                Ok(WorkflowResolvedResume::Reference(resolved_ref))
            }
        }
    }

    /// WORKFLOW_14 — the workflow owns a run directory (WORKFLOW_13), so the file-drop transport
    /// has an address and `runs.steer` is real in this host. The engine checks this flag BEFORE the
    /// method (`engine.rs`'s `supports_steer()` gate), so overriding only [`Self::steer`] would
    /// change nothing.
    fn supports_steer(&self) -> bool {
        true
    }

    /// `runs.steer(key, message, options)` from inside the script.
    ///
    /// The same file-drop-then-await-ack pair the async action and the tool-side workflow route
    /// use, against this workflow's own run directory — never a second transport.
    async fn steer(
        &self,
        key: &str,
        message: &str,
        options: WorkflowSteerOptions,
        _cancel: CancelToken,
    ) -> Result<WorkflowSteerResult, String> {
        // The engine has already proved `key` names a launched child (its own "requires a prior
        // runs.run/runs.all launch with that key" gate), so this resolves the index the host minted
        // for that key in `launch` — never a fresh one, which is why `next_child_index` is not
        // touched here.
        let index = {
            let launched = self.launched.lock().unwrap_or_else(PoisonError::into_inner);
            launched
                .get(key)
                .map(|identity| identity.index)
                .ok_or_else(|| {
                    format!("runs.steer('{key}') names no launched child in this workflow.")
                })?
        };

        // A workflow key names exactly ONE child, at the index minted for that key. Honouring an
        // explicit `index` that addresses a different one is impossible, and quietly steering this
        // key's child instead would be the same silent-retarget failure the mode mapping below
        // avoids.
        if let Some(requested) = options.index
            && usize::try_from(requested).ok() != Some(index)
        {
            return Err(format!(
                "runs.steer('{key}') index {requested} does not address this child (index {index})."
            ));
        }

        // A TOTAL mapping, not `SteerDeliveryMode::parse`: `WorkflowSteerOptions::mode` is an ENUM,
        // and an unrecognised wire value is already refused twice before this host is reached — by
        // the guest validator's own allow-list and then by serde's `snake_case` rename on
        // `WorkflowSteerMode`. So there is no unrecognised case left to refuse here. An OMITTED
        // mode is NOT an unrecognised one: it means the caller did not say, which is
        // `SteerDeliveryMode::Steer` (its `#[default]`).
        let mode = match options.mode {
            None | Some(WorkflowSteerMode::Steer) => SteerDeliveryMode::Steer,
            Some(WorkflowSteerMode::FollowUp) => SteerDeliveryMode::FollowUp,
            Some(WorkflowSteerMode::Auto) => SteerDeliveryMode::Auto,
        };

        // The child's OWN inbox, via the same handle `launch` spawned it against — pi's
        // `child.steer(...)` direct line, not the runner intake queue this process never drains.
        let handle = self.child_steer_handle(index);
        let request_id = handle
            .deliver(message, Some(mode), "workflow-script-steer")
            .await
            .map_err(|e| e.to_string())?;

        // The caller's own budget when they set one — see `await_steer_ack_within`'s doc.
        let budget = options
            .ack_timeout_ms
            .map_or(STEER_ACK_TIMEOUT, std::time::Duration::from_millis);
        let ack = SubagentExecutor::await_steer_ack_within(
            &self.run_dir,
            &request_id,
            Some(index),
            budget,
        )
        .await;

        // ⚠ No acknowledgment inside the budget is `Queued`, NEVER `Missed`. `Missed` means the
        // message missed its target; a pending file drop has not missed — it is on disk, and a
        // child that reaches a safe point later still takes it (SUBA-049).
        //
        // ⚠ A `Failed` ack is returned as `Ok(receipt)` here, NOT `Err` — the opposite of the
        // tool-side arm in `workflow_steering.rs`, and deliberately so. The engine's own `Ok` arm
        // maps `WorkflowSteerState::Failed` onto a failed trace entry and reads `receipt.error`,
        // and `WorkflowSteerResult::error` exists precisely so a failed steer is a VALUE a script
        // can inspect. `Err` would throw in the guest and discard the request id. The tool surface
        // has no receipt to carry the failure, so it uses `Err`: two surfaces, two conventions,
        // each correct for its own caller.
        let (state, delivery_status, error) = match ack.as_ref() {
            None => (WorkflowSteerState::Queued, Some("pending"), None),
            Some(ack) => match ack.state {
                SteerAckState::Delivered => {
                    (WorkflowSteerState::Delivered, Some("delivered"), None)
                }
                SteerAckState::Queued => (WorkflowSteerState::Queued, Some("queued"), None),
                SteerAckState::Failed => {
                    (WorkflowSteerState::Failed, None, Some(ack.message.clone()))
                }
            },
        };

        Ok(WorkflowSteerResult {
            key: key.to_string(),
            state,
            request_id: Some(request_id),
            delivery_status: delivery_status.map(str::to_string),
            // The one child this key addresses. `try_from` rather than `as`: the field is `u32` and
            // a saturating conversion is honest where a truncating cast is not.
            targets: Some(vec![WorkflowSteerTarget {
                index: u32::try_from(index).unwrap_or(u32::MAX),
                state: ack
                    .as_ref()
                    .map_or("pending", |ack| ack.state.as_str())
                    .to_string(),
                reason: error.clone(),
            }]),
            error,
        })
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

    use super::*;

    /// A host whose `launched` map already carries `key` at `index`, as `launch` would have left it.
    fn host_with(run_dir: PathBuf, key: &str, index: usize) -> WorkflowRunHost {
        let workflow_run_id = crate::background::RunId::new();
        let status = Arc::new(Mutex::new(crate::background::RunStatus::queued(
            workflow_run_id.clone(),
            crate::background::RunMode::Workflow,
            None,
        )));
        let host = WorkflowRunHost::new(
            Arc::new(SubagentExecutor::new()),
            PathBuf::from("/proj"),
            Box::new(|_| {}),
            workflow_run_id,
            status,
            run_dir.clone(),
            run_dir,
        );
        if let Ok(mut launched) = host.launched.lock() {
            launched.insert(
                key.to_string(),
                LaunchIdentity {
                    agent: "worker".to_string(),
                    model: None,
                    agent_scope: None,
                    context: None,
                    index,
                },
            );
        }
        host
    }

    /// DoD manual-confirmation 1: `runs.steer` returns a RECEIPT, never upstream's
    /// "Workflow steering is unavailable in this host." (which `supports_steer() == true` makes
    /// unreachable at the engine's own gate). With no child running, no ack can arrive, so the
    /// receipt is `Queued` + `pending` — the request is on disk and is still live.
    #[tokio::test]
    async fn steer_returns_a_queued_receipt_and_delivers_to_the_childs_inbox() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_dir = dir.path().join("wf");
        let host = host_with(run_dir.clone(), "lane", 2);

        assert!(
            host.supports_steer(),
            "the engine gates on this BEFORE calling steer"
        );

        let receipt = host
            .steer(
                "lane",
                "narrow the diff",
                WorkflowSteerOptions::default(),
                CancelToken::new(),
            )
            .await
            .expect("a launched key must yield a receipt");

        assert_eq!(receipt.key, "lane");
        assert_eq!(receipt.state, WorkflowSteerState::Queued);
        assert_eq!(receipt.delivery_status.as_deref(), Some("pending"));
        assert!(
            receipt.request_id.is_some(),
            "the receipt must carry its correlation id"
        );
        assert!(receipt.error.is_none());
        let targets = receipt.targets.expect("one target, this key's child");
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets[0].index, 2,
            "the WORKFLOW-flat index this key was launched at"
        );

        // Delivered into the child's own inbox at the same index — not the runner intake queue.
        let inbox = crate::background::control::step_steer_inbox_dir(&run_dir, 2);
        let count = std::fs::read_dir(&inbox)
            .expect("the child's inbox must exist")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
            .count();
        assert_eq!(count, 1, "exactly one request in {inbox:?}");
    }

    /// The engine proves the key was launched before calling us, but a host that trusted that
    /// blindly would mint a fresh index for an unknown key and steer a child that does not exist.
    #[tokio::test]
    async fn steer_refuses_a_key_this_host_never_launched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = host_with(dir.path().join("wf"), "lane", 0);
        let error = host
            .steer(
                "other",
                "hi",
                WorkflowSteerOptions::default(),
                CancelToken::new(),
            )
            .await
            .expect_err("an unlaunched key must refuse");
        assert_eq!(
            error,
            "runs.steer('other') names no launched child in this workflow."
        );
    }

    /// An explicit `index` that names a different child cannot be honoured. Quietly steering this
    /// key's child instead is the silent-retarget failure the mode mapping also refuses to make.
    #[tokio::test]
    async fn steer_refuses_an_explicit_index_that_addresses_another_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = host_with(dir.path().join("wf"), "lane", 2);
        let options = WorkflowSteerOptions {
            index: Some(5),
            ..WorkflowSteerOptions::default()
        };
        let error = host
            .steer("lane", "hi", options, CancelToken::new())
            .await
            .expect_err("a mismatched explicit index must refuse");
        assert_eq!(
            error,
            "runs.steer('lane') index 5 does not address this child (index 2)."
        );
    }

    /// An OMITTED mode is not an unrecognised one: it means "the caller did not say", which is
    /// `SteerDeliveryMode::Steer`. `Steer` is normalised OFF the wire, so the request carries no
    /// `mode` field at all — while `follow_up` is carried verbatim.
    #[tokio::test]
    async fn an_omitted_mode_defaults_to_steer_and_follow_up_is_carried() {
        for (mode, expected) in [
            (None, None),
            (Some(WorkflowSteerMode::Steer), None),
            (Some(WorkflowSteerMode::FollowUp), Some("follow_up")),
            (Some(WorkflowSteerMode::Auto), Some("auto")),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let run_dir = dir.path().join("wf");
            let host = host_with(run_dir.clone(), "lane", 0);
            let options = WorkflowSteerOptions {
                mode,
                ..WorkflowSteerOptions::default()
            };
            host.steer("lane", "go", options, CancelToken::new())
                .await
                .expect("every mode must deliver, none may refuse");

            let inbox = crate::background::control::step_steer_inbox_dir(&run_dir, 0);
            let entry = std::fs::read_dir(&inbox)
                .expect("inbox")
                .filter_map(Result::ok)
                .find(|e| e.file_name().to_string_lossy().ends_with(".json"))
                .expect("one request");
            let body = std::fs::read_to_string(entry.path()).expect("read");
            let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
            assert_eq!(
                parsed.get("mode").and_then(|m| m.as_str()),
                expected,
                "mode {mode:?}"
            );
        }
    }
}

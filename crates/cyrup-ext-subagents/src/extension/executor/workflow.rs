//! The real [`WorkflowScriptHost`] — the bridge from a `workflowScript`'s `runs.*` calls to this
//! crate's own child execution (WORKFLOW_2).
//!
//! This is the file whose absence made the whole workflow runtime unreachable. It implements the
//! two REQUIRED trait methods and nothing else: every optional capability keeps the trait's
//! default, which is upstream's own "unavailable in this host" refusal.
//!
//! Note precisely what that buys, because it differs per capability: `state` is genuinely ABSENT
//! from the guest realm (`cyrup-workflow-runtime`'s `js/prelude.js:578`), while keyed resume is
//! PRESENT AND REFUSING (`engine.rs:1105`, `:1908`). `runs.steer` is now wired (WORKFLOW_14) and
//! `runs.host` is now wired (WORKFLOW_19) — [`WorkflowScriptHost::supports_host`] is what installs
//! the guest property at all (`prelude.js:574` deletes it when the flag is false), so flipping it
//! is not a refinement of the refusal, it is the difference between a verb and a `TypeError`. Do
//! not describe all four as "absent".

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
use crate::extension::executor::foreground_control::{
    ChildSteerReadiness, ForegroundChildSteerHandle,
};
use crate::extension::executor::notices::ForegroundControlEntry;
use crate::extension::executor::requests::ForegroundRunRequest;
use crate::extension::tool::params::{SubagentToolParams, resolve_execution_agent_scope};
use crate::extension::tool::text::{CHILD_SESSION_NOT_RUNNING_YET, STEER_ACK_TIMEOUT};
use crate::fork_context::ContextRequest;
use crate::workflows::scripted::{
    WORKFLOW_CHILD_MARKER, WorkflowLaunchAdmission, WorkflowResolvedResume,
    WorkflowResolvedResumeReference, WorkflowResumeInput, WorkflowScriptHost, WorkflowSteerMode,
    WorkflowSteerOptions, WorkflowSteerResult, WorkflowSteerState, WorkflowSteerTarget,
};
use crate::workflows::{
    WorkflowContinuation, WorkflowHostCommandParams, WorkflowHostCommandResult, WorkflowKey,
    WorkflowLaneMetadata, WorkflowRequestedContext, WorkflowResumability,
    WorkflowScriptChildResult, assert_workflow_lane_key, execute_workflow_host_command,
    normalize_workflow_lane_metadata, resolve_workflow_host_output_claim_path,
    workflow_terminal_outcome_for_result,
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

/// The `output` a LIVE child carries out of [`WorkflowScriptHost::status`] (WORKFLOW_17).
///
/// The live arm reports `ok: true`, meaning *the status query succeeded* — NOT that the child
/// succeeded, which is not yet knowable. It has no choice: `run_status` re-wraps any host result
/// with `ok == false` as `Err(format!("Status '{key}' failed: {output}"))`
/// (`workflows/scripted/engine.rs:1046-1050`) and files a `Failed` trace entry (`:1036`), so a
/// running child answered with `ok: false` would still reach the script as a THROWN ERROR and the
/// arm would have swapped one misleading failure for another. Running-ness therefore has to ride in
/// the payload, and this line is it: the word `running` comes first so a script that prints nothing
/// but `status.output` still says the true thing.
///
/// Absent counters are OMITTED, never rendered as `0`. The fields are `Option<u64>`
/// (`notices.rs:48-52`) precisely because "no control event has folded a count yet" is not "zero
/// turns", and a fabricated `0 turns` is exactly the confident-but-wrong evidence this change
/// exists to remove. The separator is the inline stats line's own (`tui/events.rs:329-340`), so the
/// two live progress renderings read alike.
///
/// The parent-level fields are read, not `active_children[&0]`: `sync_current_child`
/// (`foreground_control.rs:171-176`) keeps them derived from that child, and the two index spaces
/// are NOT the same — an `active_children` key is the child's index within its own foreground run
/// (always `0` for a workflow child, `foreground.rs:989-993`) while [`LaunchIdentity::index`] is
/// the workflow-flat index. Reading the derived fields cannot conflate them.
fn live_activity_line(control: &ForegroundControlEntry) -> String {
    let mut parts: Vec<String> = vec!["running".to_string()];
    if let Some(state) = control.current_activity_state {
        // The wire word, matching `ActivityState`'s own `rename_all = "snake_case"`
        // (`background/telemetry.rs:22-31`), so the line and the status JSON agree.
        parts.push(
            match state {
                crate::background::ActivityState::ActiveLongRunning => "active_long_running",
                crate::background::ActivityState::NeedsAttention => "needs_attention",
            }
            .to_string(),
        );
    }
    if let Some(tool) = control.current_tool.as_deref() {
        parts.push(match control.current_path.as_deref() {
            Some(path) => format!("⚒ {tool} {path}"),
            None => format!("⚒ {tool}"),
        });
    }
    if let Some(turns) = control.turn_count {
        parts.push(format!("{turns} turns"));
    }
    if let Some(tools) = control.tool_count {
        parts.push(format!("{tools} tools"));
    }
    if let Some(tokens) = control.tokens {
        parts.push(format!("{tokens} tokens"));
    }
    parts.join(" · ")
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

/// One workflow child that detached, carried out of [`WorkflowScriptHost::launch`] with the
/// settled result the drive loop returned for it.
///
/// The three fields are exactly what
/// [`reconcile_detached_workflow_child_completion`](crate::extension::executor::workflow_detach::reconcile_detached_workflow_child_completion)
/// needs and cannot recover from disk: the child's run id (a foreground child writes no
/// `ResultFile`, so nothing on disk names it), its settled [`SingleResult`], and the lane it was
/// launched under. Everything else that reconciler reads — the workflow's status, the prior
/// payload, the receipt — it resolves from the run directory itself.
#[derive(Clone)]
pub(crate) struct DetachedWorkflowChild {
    /// The engine's lane key, and this record's upsert identity.
    pub(crate) key: String,
    /// The child's real run id — [`WorkflowScriptHost::launch`]'s own
    /// `run_foreground_streaming` return, never `SingleResult::child_run_id` (`None` on every
    /// foreground run by construction).
    pub(crate) run_id: crate::background::RunId,
    /// `key` parsed as a lane key, for the reconciler's by-key settlement rung. `None` on the
    /// impossible branch (`WorkflowKey::parse` is fallible and the engine generated this key), and
    /// `None` there simply disables the identity back-fill, which is the safe verdict.
    pub(crate) workflow_key: Option<WorkflowKey>,
    /// The settled result, cloned out of the launch.
    pub(crate) result: SingleResult,
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
    /// Settled children by key, in launch order — [`WorkflowScriptHost::status`]'s FIRST arm, and
    /// the only one that can carry a child's real OUTCOME, because a launch does not return until
    /// its child settles.
    ///
    /// ⚠ It is no longer the only thing `status` can answer from, and this doc used to say it was.
    /// WORKFLOW_17 added the live arm: a still-running child is answered out of the executor's
    /// `foreground_controls` registry by [`WorkflowRunHost::live_child_status`], so polling an
    /// in-flight child reports progress instead of claiming the key was never launched. "A launch
    /// does not return until its child settles" constrains what THIS struct can hold; it never
    /// constrained what the host can observe.
    ///
    /// A `Vec`, not a map, because `runs.status` may be asked by key OR by run id
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
    /// The children that DETACHED, by key, in launch order — see [`DetachedWorkflowChild`].
    ///
    /// This is cyrup's `onDetachedExit` (pi `subagent-executor.ts:3951-3976`, fired by
    /// `execution.ts:2364`). Upstream needs a callback because its `detachForeground` returns the
    /// tool call early and leaves the child running; cyrup's drive loop keeps driving a detached
    /// child to its real exit (`exec/drive_attempt.rs:345-367` fires the clarify ask and continues,
    /// the answer riding back over the broker), so the detached child's COMPLETION is observed
    /// right here, in `launch`'s return, with its settled [`SingleResult`] in hand. Recording it is
    /// the whole difference between a hook and no hook.
    ///
    /// The same `Mutex` discipline and the same ⚠ UPSERT-BY-KEY rule as `settled` above, for the
    /// same reason: an auto-resume relaunch of a key mints a NEW child run id, and a pushed
    /// duplicate would hand the reconciler the abandoned attempt's run id as well as the live one.
    detached: Mutex<Vec<DetachedWorkflowChild>>,
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
            detached: Mutex::new(Vec::new()),
            launched: Mutex::new(HashMap::new()),
            workflow_run_id,
            status,
            run_dir,
            async_root,
            next_child_index: AtomicUsize::new(0),
            publish_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Every child that detached during this workflow run, in launch order.
    ///
    /// Drained by value rather than borrowed for the same reason [`Self::update_sink`] is an
    /// accessor: `routing.rs` is a different module, and the settlement path needs an owned
    /// snapshot it can hold across the reconciler's `.await`s — which the field's
    /// `std::sync::Mutex` guard must never be alive across.
    ///
    /// Empty for every workflow whose children all ran to a normal exit, which is the overwhelming
    /// majority: a detach is a child blocking on a `contact_supervisor` clarify ask (R-SA-037).
    pub(crate) fn detached_children(&self) -> Vec<DetachedWorkflowChild> {
        self.detached
            .lock()
            .map(|detached| detached.to_vec())
            .unwrap_or_default()
    }

    /// The host's share of the one real sink, for the live `emit()` forwarder `routing.rs` passes
    /// as `RunWorkflowScriptOptions::on_emit` (WORKFLOW_21).
    ///
    /// An accessor rather than `pub(crate)` on the field or on [`SharedUpdateSink`]: `routing.rs`
    /// is a different module, the alias' doc is written for a type that stays private to this one,
    /// and the only thing the caller legitimately needs is a share to lock. It spells the type out
    /// for exactly that reason.
    ///
    /// The forwarder built on this must hold the lock the same way [`child_sink`] does — across one
    /// synchronous `sink(update)` and never across an `.await` — because it is the SAME mutex the
    /// per-child forwarding sinks minted by [`WorkflowScriptHost::launch`] hold from the main pool
    /// while children stream, and unlike them it is polled on the isolate thread inside a live op.
    pub(crate) fn update_sink(&self) -> Arc<Mutex<ToolUpdateSink>> {
        Arc::clone(&self.on_update)
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
            let mut steps = crate::workflows::workflow_step_statuses(&settled);
            let Ok(mut status) = self.status.lock() else {
                return;
            };
            // The child that just settled is stamped `ended_at` now; earlier rows keep theirs.
            crate::workflows::carry_step_settle_times(
                &status.steps,
                &mut steps,
                crate::time::now_epoch_millis(),
            );
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

    /// [`WorkflowScriptHost::status`]'s LIVE arm: the still-running child this workflow launched
    /// under `key_or_run_id`, or `None` when no such child is in flight (WORKFLOW_17).
    ///
    /// The registry is the executor's `foreground_controls` (`mod.rs:137`), which holds an entry
    /// for EXACTLY the duration of a child's run: `register_foreground_controls` inserts it keyed
    /// by the child's real run id (`foreground.rs:1022-1025`) and `settle_foreground_run` `remove`s
    /// it (`foreground.rs:1063-1069`). Presence therefore IS liveness — nothing empties a
    /// surviving entry, because this crate has no `end_foreground_child` counterpart to
    /// `begin_foreground_child` — which is why this predicate needs neither
    /// `control_is_live_in_workflow`'s `!active_children.is_empty()` term nor its session term
    /// (`workflow_steering.rs:84-92`), even though it copies that resolver's lock → filter → clone
    /// shape verbatim. The session term is redundant here for a second reason: the scope is
    /// `self.workflow_run_id`, which is process-local and minted by this very tool call
    /// (`routing.rs:583`), so a foreign instance's control entry can never match it.
    ///
    /// ⚠ The run id is the MAP KEY, not a field on the entry — the same reason
    /// `resolve_workflow_foreground_steering_target` carries it out as `(k.clone(), c.clone())`
    /// (`workflow_steering.rs:210-222`). That is also why [`LaunchIdentity`] grows no `run_id`
    /// field: `RunId::new()` is called inside `resolve_run_channels` (`foreground.rs:641`) and
    /// reaches `launch` only in `run_foreground_streaming`'s return tuple — i.e. AFTER the child
    /// has settled, at which point arm 1 already answers. A field filled there would be `None` for
    /// every running child's entire lifetime, dead in exactly the window a live status needs it.
    ///
    /// The guest addresses either form (`prelude.js:311`'s `status(keyOrRunId)`), so both the lane
    /// key and the map key are accepted.
    /// pi `steerWorkflowChildByKey`'s own control find (`subagent-executor.ts:4491-4493`) — the
    /// liveness half of [`WorkflowScriptHost::steer`]'s poll:
    ///
    /// ```ts
    /// const control = [...state.foregroundControls.values()].find((candidate) =>
    ///     candidate.parentWorkflowRunId === input.workflowRunId
    ///     && candidate.workflowKey === input.key
    ///     && (candidate.activeChildren?.size ?? 0) > 0);
    /// ```
    ///
    /// All three terms, and note the middle one: `workflowKey`, NOT `sessionId`. That is the
    /// difference between this predicate and `workflow_steering.rs`'s `control_is_live_in_workflow`,
    /// and it is the whole point — the session term identifies an OWNER, the key term identifies a
    /// LANE. Reusing the session predicate here would match any live child of this workflow and
    /// steer the wrong one on a multi-lane script. The session term is not needed in exchange:
    /// `self.workflow_run_id` is minted by this very tool call (`routing.rs`), so it is unique to
    /// this process and no foreign instance's entry can carry it.
    ///
    /// This is the LIVENESS gate only; the INDEX still comes from [`Self::launched`], which is
    /// strictly stronger on identity (a `WorkflowRunHost` exists for exactly one workflow and
    /// `launched` is keyed by lane key, so both identity terms hold by construction there) and is
    /// the only place the flat index a handle addresses was ever minted. Deriving the index from a
    /// scan would be a second minting site for the one number that must agree between the spawn and
    /// the write.
    ///
    /// Cloned nothing and held nothing: a `bool` out of a short synchronous section, because
    /// `foreground_controls` is a `std::sync::Mutex` and this is polled from an async loop.
    fn lane_has_live_child(&self, key: &str) -> bool {
        let controls = self
            .executor
            .foreground_controls
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        controls.values().any(|control| {
            control.parent_workflow_run_id.as_ref() == Some(&self.workflow_run_id)
                && control.workflow_key.as_ref().map(WorkflowKey::as_str) == Some(key)
                && !control.active_children.is_empty()
        })
    }

    fn live_child_status(&self, key_or_run_id: &str) -> Option<WorkflowScriptChildResult> {
        // Cloned OUT of the lock, never read through it: `foreground_controls` is a
        // `std::sync::Mutex` shared with every live run's notifier pump, and this is called from an
        // async fn. `unwrap_or_else(PoisonError::into_inner)` because `unwrap_used`/`expect_used`
        // are DENY workspace-wide (`Cargo.toml:101-104`) and a poisoned registry must degrade a
        // status poll, not panic the workflow that is polling.
        let (run_id, control) = {
            let controls = self
                .executor
                .foreground_controls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            controls
                .iter()
                .find(|(run_id, control)| {
                    control.parent_workflow_run_id.as_ref() == Some(&self.workflow_run_id)
                        && (control.workflow_key.as_ref().map(WorkflowKey::as_str)
                            == Some(key_or_run_id)
                            || run_id.as_str() == key_or_run_id)
                })
                .map(|(run_id, control)| (run_id.clone(), control.clone()))?
        };

        // The lane key comes off the ENTRY, not from the argument, so a run-id-addressed poll still
        // answers with the key the script launched: `WorkflowScriptChildResult::key` is a KEY, and
        // echoing a run id into it would hand the script a second, wrong identity for the child.
        // `None` is `launch`'s documented degraded-provenance branch (`WorkflowKey::parse(key).ok()`
        // on a key the engine generated, which always parses); with no lane key there is nothing
        // truthful to name the child by, so it falls through to the unknown arm rather than guess.
        let key = control.workflow_key.as_ref()?.as_str().to_string();

        // `launched` is the launch-side half of the identity, and it is written BEFORE the spawn
        // await — the `launched.entry(key)` insert precedes the `run_foreground_streaming().await`
        // in `launch` — so it is populated for the child's whole running lifetime. That ordering is
        // what makes this arm possible at all.
        let agent = self
            .launched
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .map(|identity| identity.agent.clone());

        // `..Default::default()` (the derive is on the struct itself, `workflows/types.rs:309`)
        // rather than twenty hand-written `None`/`false` fields. Every one of them is absent for the
        // same single reason: the child HAS NOT SETTLED. `stopped`/`interrupted`/`detached` are
        // false because no terminal event has happened, `error`/`structured_output`/`resumability`/
        // `continuation`/`artifact_paths`/`results` are empty because there is no outcome yet — and
        // inventing one is the failure mode this whole change removes. Arm 1 replaces all of it the
        // moment there IS an outcome.
        Some(WorkflowScriptChildResult {
            key,
            // See [`live_activity_line`]: `ok` answers "did the status QUERY succeed", not "did the
            // child succeed". `engine.rs:1046-1050` leaves no other option that meets the objective
            // without widening the engine's own gate.
            ok: true,
            agent,
            run_id: Some(run_id),
            output: live_activity_line(&control),
            ..WorkflowScriptChildResult::default()
        })
    }
}

/// How one `runs.steer` attempt ended — the input [`workflow_steer_receipt`] classifies.
///
/// pi's `steerWorkflowChildByKey` hands `workflowSteerReceipt` a loose object and lets the receipt
/// builder read whichever fields are present (`subagent-executor.ts:4319-4334`). These three
/// variants are the same three shapes it actually passes, made exhaustive so a new outcome cannot
/// be added without deciding which of the four receipt states it is.
enum WorkflowSteerOutcome {
    /// The request was WRITTEN into the child's inbox. `ack` is the child's answer within the
    /// remaining budget, or `None` when none arrived.
    Acked {
        request_id: String,
        ack: Option<crate::background::control::SteerAck>,
    },
    /// A live route existed and refused — today only a child that published
    /// `supported: false`. Terminal, and distinct from [`Self::NoRoute`]: the child answered.
    Refused(String),
    /// No live steering route inside the budget, or the call was cancelled — pi `:4537`.
    NoRoute(String),
}

/// pi `workflowSteerReceipt` (`subagent-executor.ts:4319-4334`) — the ONE place a `runs.steer`
/// receipt is built, so its state word, its `deliveryStatus` refinement and its per-target record
/// cannot drift apart.
///
/// The four states and where each comes from:
///
/// | condition | state | pi |
/// |---|---|---|
/// | ack `delivered` | [`WorkflowSteerState::Delivered`] | `:4325` |
/// | ack `queued`, **or no ack inside the budget** | [`WorkflowSteerState::Queued`] | `:4325` |
/// | ack `failed`, or a live route refused | [`WorkflowSteerState::Failed`] | `:4323-4324` |
/// | no live route before the deadline / cancelled | [`WorkflowSteerState::Missed`] | `:4537` |
///
/// ⚠ No acknowledgment inside the budget is `Queued`, NEVER `Missed`. `Missed` means the message
/// missed its target; a pending file drop has not missed — it is on disk, addressed, and a child
/// that reaches a safe point later still takes it (SUBA-049). Collapsing the two would tell a
/// script its guidance was lost when it is in fact in flight.
///
/// ⚠ A `Failed` outcome is returned as `Ok(receipt)` by [`WorkflowScriptHost::steer`], NOT `Err` —
/// the opposite of the tool-side arm in `workflow_steering.rs`, and deliberately so. The engine's
/// `Ok` arm maps [`WorkflowSteerState::Failed`] onto a failed trace entry and reads
/// [`WorkflowSteerResult::error`], which exists precisely so a failed steer is a VALUE a script can
/// inspect; `Err` would throw in the guest and discard the request id. The tool surface has no
/// receipt to carry a failure, so it uses `Err`. Two surfaces, two conventions, each correct for
/// its own caller.
///
/// The per-target `state` word is derived from the receipt state rather than re-read off the ack:
/// the field is loose upstream (any string), and two independent renderings of one outcome is the
/// drift this helper exists to remove. `delivery_status` is pi's `"delivered"`/`"queued"` pair
/// (`workflow-foreground-steering.ts:158`) and is OMITTED where nothing was delivered at all —
/// `null` there would read as a fourth delivery word.
fn workflow_steer_receipt(
    key: &str,
    index: usize,
    outcome: WorkflowSteerOutcome,
) -> WorkflowSteerResult {
    let (state, request_id, error) = match outcome {
        WorkflowSteerOutcome::Acked { request_id, ack } => match ack {
            None => (WorkflowSteerState::Queued, Some(request_id), None),
            Some(ack) => match ack.state {
                SteerAckState::Delivered => (WorkflowSteerState::Delivered, Some(request_id), None),
                SteerAckState::Queued => (WorkflowSteerState::Queued, Some(request_id), None),
                // The ack's own `message` IS the reason for a refusal — e.g. upstream's
                // `Follow-up queue is full (20 messages).` — so it is what the script reads.
                SteerAckState::Failed => (
                    WorkflowSteerState::Failed,
                    Some(request_id),
                    Some(ack.message),
                ),
            },
        },
        // No request was ever written on either arm, so there is no id to correlate against.
        WorkflowSteerOutcome::Refused(reason) => (WorkflowSteerState::Failed, None, Some(reason)),
        WorkflowSteerOutcome::NoRoute(reason) => (WorkflowSteerState::Missed, None, Some(reason)),
    };

    let word = match state {
        WorkflowSteerState::Queued => "queued",
        WorkflowSteerState::Delivered => "delivered",
        WorkflowSteerState::Missed => "missed",
        WorkflowSteerState::Failed => "failed",
    };

    WorkflowSteerResult {
        key: key.to_string(),
        state,
        request_id,
        delivery_status: matches!(
            state,
            WorkflowSteerState::Queued | WorkflowSteerState::Delivered
        )
        .then(|| word.to_string()),
        // The one child this key addresses. `try_from` rather than `as`: the field is `u32` and a
        // saturating conversion is honest where a truncating cast is not.
        targets: Some(vec![WorkflowSteerTarget {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            state: word.to_string(),
            reason: error.clone(),
        }]),
        error,
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
        // SUBA-100 — pi `resolveWorkflowChildCwd` (`subagent-executor.ts:3548-3560` @v0.68.0): a
        // placed child's typed `cwd` names a directory ON THE MACHINE. It is only ever read when
        // the launch resolves a placement (the call's `machine` or the agent's own), so an
        // unplaced child is unaffected.
        overrides.machine_cwd = child.cwd.clone();

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

        // THE DETACH HOOK. `result.detached` is the drive loop's settled R-SA-037 observation
        // (`exec/fallback.rs:1527`'s field, carried through `SettledAttempt`), and this line is the
        // moment it becomes actionable: the detached child has now EXITED, and its result is the
        // evidence `reconcile_detached_workflow_child_completion` settles the paused workflow from.
        // Recorded here rather than derived at settlement from `mapped.detached`, because the
        // reconciler takes a `SingleResult` and `WorkflowScriptChildResult` is the lossy guest-wire
        // projection of one — `exit_code`, `interrupted` and `session_file` are all absent from it.
        //
        // R-VLS11b-01: `result` is a TERMINAL result on every path that reaches here, never a
        // `/subagents-detach` receipt. `hand_off_detached_foreground_run` (`foreground.rs`) is
        // upstream's `workflowAwaitDetached` fork (`subagent-executor.ts:4008-4012`, `:4123`): a
        // user detach of a workflow child keeps driving the child in THIS task and returns its
        // real exit, so the only producer that can set `detached` on what `run_foreground_streaming`
        // hands back here is the intercom one — which is exactly what this hook is written for.
        //
        // Same upsert-in-place rule as `settled` above, and for the same reason.
        if result.detached
            && let Ok(mut detached) = self.detached.lock()
        {
            let entry = DetachedWorkflowChild {
                key: key.to_string(),
                run_id: run_id.clone(),
                workflow_key: WorkflowKey::parse(key).ok(),
                result: result.clone(),
            };
            match detached.iter_mut().find(|c| c.key == entry.key) {
                Some(existing) => *existing = entry,
                None => detached.push(entry),
            }
        }

        // §3.3 — republish immediately after each child settles (before returning to engine).
        self.publish_steps().await;
        Ok(mapped)
    }

    /// `runs.status(keyOrRunId)` — THREE arms: settled, then LIVE, then unknown (WORKFLOW_17).
    ///
    /// This answered from the settled list alone, and its own doc justified that with *"a
    /// foreground host has no live child registry — a launch does not return until its child
    /// settles"*. The premise is true of the child's RESULT and false of its STATUS, which is the
    /// entire point of a status verb. The host has no live registry; the EXECUTOR does, and the host
    /// holds an `Arc` to it.
    ///
    /// What the settled-only host did to a script that polled an in-flight child was emit
    /// *"names no launched child in this workflow"* — wrong twice over, because the key HAS been
    /// launched and the sentence invites the author to add a launch that already exists. The engine
    /// makes that worse rather than catching it: for an in-flight child `run_status` cannot resolve
    /// a run id (it reads settled children only, `engine.rs:996-1006`) and hands this method the raw
    /// KEY, so there is deliberately no engine-side notion of "running" — that answer comes entirely
    /// from here.
    ///
    /// The guest passes either form (`prelude.js:311`'s `status(keyOrRunId)`, which coerces a
    /// non-string to `""`); both arms accept both.
    ///
    /// An unknown key is still an ERROR, not an empty result: `runs.status` on a child that was
    /// never launched is a script bug, and the message is a re-prompt. It is now reached only when
    /// the key is in neither the settled list nor the live registry, which is the only case the
    /// wording was ever true for.
    async fn status(
        &self,
        key_or_run_id: &str,
        _cancel: CancelToken,
    ) -> Result<WorkflowScriptChildResult, String> {
        // Arm 1 — SETTLED, unchanged, and it wins whenever it exists: a real outcome is strictly
        // more informative than a live snapshot. The one window where both arms can match is an
        // auto-resume relaunch of a key that already failed once (`engine.rs:2045`), which answers
        // from the first attempt until the retry settles and `launch`'s upsert-in-place replaces it.
        let settled = self.settled.lock().ok().and_then(|settled| {
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
        if let Some(settled) = settled {
            return Ok(settled);
        }

        // Arm 2 — LIVE. The child is still running, so it has no outcome to report; what it has is
        // evidence, and [`Self::live_child_status`] carries it (real run id, agent, activity line,
        // live counters) as an `Ok` the script can inspect rather than an error it must catch.
        if let Some(live) = self.live_child_status(key_or_run_id) {
            return Ok(live);
        }

        // Arm 3 — UNKNOWN.
        Err(format!(
            "runs.status('{key_or_run_id}') names no launched child in this workflow."
        ))
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

    /// `runs.steer(key, message, options)` from inside the script — pi `steerWorkflowChildByKey`
    /// (`subagent-executor.ts:4477-4541`).
    ///
    /// The same file-drop-then-await-ack pair the async action and the tool-side workflow route
    /// use, against this workflow's own run directory — never a second transport.
    ///
    /// # Why this POLLS, and why that is what makes the method non-vacuous
    ///
    /// [`Self::launch`] does not return until its child settles, so `runs.steer` is only ever
    /// reachable from a CONCURRENT lane of the guest script. The steered lane can therefore be
    /// anywhere in its life when the call lands, including the ordinary window between "the engine
    /// recorded the launch" and "the spawned child reached its runtime" — which is precisely what
    /// upstream's own loop exists for (`:4490`, `:4539`), and its comment at `:4501` says so:
    /// *"the control registers before its child session exists; keep polling until the steer can
    /// route."* One budget covers the whole loop, not each attempt.
    async fn steer(
        &self,
        key: &str,
        message: &str,
        options: WorkflowSteerOptions,
        cancel: CancelToken,
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

        // pi `:4488-4489` — ONE budget for the whole loop, not one per attempt. The caller's own
        // when they set it; see `await_steer_ack_within`'s doc for why accepting `ackTimeoutMs` and
        // then waiting the default anyway would be a silent retarget of their request.
        let budget = options
            .ack_timeout_ms
            .map_or(STEER_ACK_TIMEOUT, std::time::Duration::from_millis);
        let deadline = std::time::Instant::now() + budget;

        loop {
            // pi `:4491-4493` — the lane must have a LIVE child before a request is written for it.
            // Two independent facts, checked in order, because they become true at different times
            // and a single check would conflate them: the control registry says this lane has a
            // running child at all, and the child's own capability record says that child's runtime
            // is up and can take an injection.
            if self.lane_has_live_child(key) {
                match handle.readiness().await {
                    ChildSteerReadiness::Ready => {
                        let request_id = handle
                            .deliver(message, Some(mode), "workflow-script-steer")
                            .await
                            .map_err(|e| e.to_string())?;
                        // The REMAINING budget, not a fresh one: the deadline bounds finding the
                        // route AND hearing back, exactly as upstream's single `deadline` does.
                        // `await_steer_ack_within` always performs one read before testing its own
                        // deadline, so a zero remainder still gets an honest look rather than an
                        // automatic `queued`.
                        let ack = SubagentExecutor::await_steer_ack_within(
                            &self.run_dir,
                            &request_id,
                            Some(index),
                            deadline.saturating_duration_since(std::time::Instant::now()),
                        )
                        .await;
                        return Ok(workflow_steer_receipt(
                            key,
                            index,
                            WorkflowSteerOutcome::Acked { request_id, ack },
                        ));
                    }
                    // pi `:4633` — the ONLY retryable outcome, and retryable only while budget
                    // remains. The control registers before the child's runtime does, which is
                    // ordinary and short-lived; anything else in this `match` is the answer.
                    //
                    // At the deadline upstream returns the LAST ATTEMPT'S receipt rather than
                    // falling through to `:4668`'s `missed`, and the distinction it draws is worth
                    // keeping: a lane whose control was never found had no route at all
                    // (`Missed`), while one whose control was found and whose child never booted
                    // has a knowable reason (`Failed` carrying it). Collapsing the two would throw
                    // away the only sentence that says WHY.
                    ChildSteerReadiness::NotRunningYet => {
                        if std::time::Instant::now() >= deadline {
                            return Ok(workflow_steer_receipt(
                                key,
                                index,
                                WorkflowSteerOutcome::Refused(
                                    CHILD_SESSION_NOT_RUNNING_YET.to_string(),
                                ),
                            ));
                        }
                    }
                    // Terminal: a host that cannot inject messages will not start being able to,
                    // so waiting out the budget would only turn a knowable `failed` into a `missed`.
                    ChildSteerReadiness::Unsupported => {
                        return Ok(workflow_steer_receipt(
                            key,
                            index,
                            WorkflowSteerOutcome::Refused(format!(
                                "Workflow child '{key}' cannot be steered: its host cannot inject \
                                 messages."
                            )),
                        ));
                    }
                }
            }

            // pi `:4667-4668`. cyrup has no async status file for a FOREGROUND child, so upstream's
            // three status-derived `missed` arms (`:4643`, `:4662`, `:4665`) collapse into this one
            // — the lane had no live steering route inside the budget, which is the same fact those
            // three report by three different routes.
            if cancel.is_cancelled() || std::time::Instant::now() >= deadline {
                return Ok(workflow_steer_receipt(
                    key,
                    index,
                    WorkflowSteerOutcome::NoRoute(format!(
                        "Workflow child '{key}' had no live steering route."
                    )),
                ));
            }
            // pi `:4539` — `Math.min(10, …)`. The same 10 ms, deliberately NOT
            // `STEER_ACK_POLL_INTERVAL`: that one paces the ACK read inside `await_steer_ack_within`,
            // a different loop waiting on a different event.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// WORKFLOW_19 — the guest sees `runs.host` ONLY because of this flag.
    ///
    /// `prelude.js:574` is `if (!hostEnabled) delete surface.host;`, and `hostEnabled` is read off
    /// this method (`engine.rs`'s `shared.host.supports_host()` at install time). So while
    /// [`Self::host_command`] without this would be dead code, this without
    /// [`Self::host_command`] would be worse than dead: the property would exist and every call
    /// would take the trait default's `runs.host is unavailable in this host context.` The two
    /// land together, as `supports_steer`/`steer` did.
    ///
    /// It is true unconditionally because its two prerequisites are unconditional for a
    /// foreground workflow: a resolved request cwd (`self.cwd`, `extension/tool/mod.rs:205`) to
    /// run the command in, and a real run directory (`self.run_dir`, WORKFLOW_13) to write the
    /// default capture into. Neither is optional at this point in the call.
    fn supports_host(&self) -> bool {
        true
    }

    /// `runs.host(key, params)` — run a gate/CI command on the host and hand the script its
    /// verdict.
    ///
    /// `params` arrives ALREADY NORMALIZED: `run_host_command` calls
    /// [`crate::workflows::normalize_workflow_host_command_params`] before it reaches this host
    /// (and before the `supports_host` gate, so a malformed-params script gets the normalizer's
    /// message even in a host that refuses the verb), and that function's own doc names itself the
    /// ONLY validating boundary. Re-validating here would be a second, divergent boundary; this
    /// method therefore treats `params` as plain data and adds exactly the two arguments a
    /// normalized param set cannot carry — where the command runs, and where its capture lands.
    async fn host_command(
        &self,
        key: &str,
        params: WorkflowHostCommandParams,
        cancel: CancelToken,
    ) -> Result<WorkflowHostCommandResult, String> {
        // The REQUEST cwd, the one `Tool::execute`'s `resolve_requested_cwd` already resolved and
        // the same one every child of this workflow is launched in — never `std::env::current_dir`,
        // which is this process's ambient state and would silently disagree with the `cwd` the
        // caller asked for (the same reason `clippy.toml` bans `std::env::set_var`).
        //
        // For an EXPLICIT `params.output` this is also the containment root:
        // `execute_workflow_host_command` runs `ContainedPath::assert_within(cwd, …)` against it
        // before and after the spawn. Handing it a different root than the command's own working
        // directory would make "relative to the cwd" mean two things at once.
        let cwd = self.cwd.as_path();

        // The default capture destination, which nothing but this host can mint: it is used only
        // when the script named no `output`, and in that branch the runner does a plain
        // `create_dir_all` + `write_default_output` with NO containment check — so the path must be
        // one this host owns outright rather than anything derived from guest input. `self.run_dir`
        // is exactly that: the workflow's own directory, created up front by `route_workflow_mode`
        // and already the home of `status.json` and the receipt.
        //
        // `key` is safe as a file name by construction and is re-proved on the way in: the guest
        // pattern is `/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/` (`prelude.js:87`) and the engine
        // re-checks it through `validate_key` → `WorkflowKey::parse` before this method is
        // reached. No separator, no leading dot, so the `join` cannot escape `host/`.
        let default_output_path = self.run_dir.join("host").join(format!("{key}.log"));

        // Claim the DEFAULT destination and only the default one.
        //
        // The claim is the post-run equality check `resolve_workflow_host_output_claim_path(
        // &output_path) == claimed` (`host_command.rs:907-912`), which catches a command that
        // replaced its own output directory with a symlink between the claim and the write. The
        // default branch has no other guard at all — it is a plain write — so this is the whole of
        // its protection, and the host can state the claim exactly because in that branch
        // `output_path` IS `default_output_path`.
        //
        // The EXPLICIT branch gets `None`, deliberately. There the runner already re-runs
        // `ContainedPath::assert_within` and requires it to EQUAL the pre-spawn result, which is
        // the same symlink-swap defence; claiming as well would buy nothing and would force this
        // host to re-derive the runner's private `cwd.join(output)` normalization. A claim that
        // disagrees with the runner's own derivation fails CLOSED — a hard
        // `output path changed after it was claimed.` on a command that did nothing wrong — so
        // duplicating that derivation is a liability, not a belt.
        let claimed_output_path = params
            .output
            .is_none()
            .then(|| resolve_workflow_host_output_claim_path(&default_output_path));

        // `&params` / `&cancel`: the trait hands this host OWNED values (the engine clones them per
        // call at `engine.rs`'s `host_params`/`cancel` bindings) while the runner borrows.
        //
        // `cancel` is the engine's `child_cancel`, the token it cancels at settlement — so a
        // command still running when the workflow settles drains as `Stopped`
        // (→ `HostStepState::Cancelled`) rather than hanging the settlement behind its timeout.
        execute_workflow_host_command(
            key,
            &params,
            cwd,
            &default_output_path,
            claimed_output_path.as_deref(),
            &cancel,
        )
        .await
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

    /// A bare host rooted at a REAL `cwd` and `run_dir` — what `runs.host` needs, since it spawns
    /// a process in the one and writes its capture into the other.
    fn host_rooted_at(cwd: PathBuf, run_dir: PathBuf) -> WorkflowRunHost {
        let workflow_run_id = crate::background::RunId::new();
        let status = Arc::new(Mutex::new(crate::background::RunStatus::queued(
            workflow_run_id.clone(),
            crate::background::RunMode::Workflow,
            None,
        )));
        WorkflowRunHost::new(
            Arc::new(SubagentExecutor::new()),
            cwd,
            Box::new(|_| {}),
            workflow_run_id,
            status,
            run_dir.clone(),
            run_dir,
        )
    }

    /// The normalizer's OUTPUT, as the engine hands it to the host — never a hand-rolled shape the
    /// normalizer would have rejected, because `host_command`'s contract is "already normalized".
    fn host_params(
        command: &str,
        timeout_ms: u64,
        output: Option<&str>,
    ) -> WorkflowHostCommandParams {
        // The key is OMITTED, never `null`: the normalizer refuses a present-but-null `output`
        // ("must be a non-empty relative path"), so `json!({"output": None::<&str>})` would build
        // a param set the engine could never hand this host.
        let mut raw = serde_json::json!({
            "kind": "command",
            "command": command,
            "timeoutMs": timeout_ms,
        });
        if let Some(output) = output
            && let Some(map) = raw.as_object_mut()
        {
            map.insert("output".to_string(), Value::String(output.to_string()));
        }
        crate::workflows::normalize_workflow_host_command_params(&raw, "runs.host('gate') params")
            .expect("the fixture params must normalize")
    }

    /// A host whose `launched` map already carries `key` at `index`, as `launch` would have left it.
    fn host_with(run_dir: PathBuf, key: &str, index: usize) -> WorkflowRunHost {
        let host = host_rooted_at(PathBuf::from("/proj"), run_dir);
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
    /// unreachable at the engine's own gate). The lane is live and the child has published its
    /// capability, so the request is written on the first pass of the poll; no ack can arrive from a
    /// fixture, so the receipt is `Queued` + `queued` — the request is on disk and is still live.
    #[tokio::test]
    async fn steer_returns_a_queued_receipt_and_delivers_to_the_childs_inbox() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_dir = dir.path().join("wf");
        let host = host_with_live_lane(run_dir.clone(), "lane", 2);
        publish_capability(&run_dir, 2, true).await;

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
        // `queued`, never `pending`: a fourth delivery word on a receipt whose state is already
        // `queued` says nothing the state does not, and `pending` is the ASYNC surface's own text.
        assert_eq!(receipt.delivery_status.as_deref(), Some("queued"));
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
        assert_eq!(
            targets[0].state, "queued",
            "the per-target word is DERIVED from the receipt state, so the two cannot drift"
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

    /// §5.2's poll, and the state it produces. A lane whose control entry never appears has no live
    /// steering route, and the honest answer at the deadline is `Missed` — the FOURTH receipt state,
    /// which nothing in this crate could produce before.
    ///
    /// ⚠ And nothing is written. That is the difference between `Missed` and `Queued`: a queued
    /// receipt promises a file is on disk for the child to find, so answering `Queued` here would
    /// promise a delivery to a child that does not exist.
    ///
    /// `ack_timeout_ms` is the caller's own budget and it bounds the WHOLE loop, so a 60 ms budget
    /// makes this test cost 60 ms rather than `STEER_ACK_TIMEOUT`'s three seconds.
    #[tokio::test]
    async fn steer_reports_missed_when_the_lane_never_becomes_live() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_dir = dir.path().join("wf");
        // Launched (so the index resolves) but NO foreground control — the lane is registered and
        // not yet spawned, which is precisely the window the poll exists for.
        let host = host_with(run_dir.clone(), "lane", 0);
        let options = WorkflowSteerOptions {
            ack_timeout_ms: Some(60),
            ..WorkflowSteerOptions::default()
        };

        let receipt = host
            .steer("lane", "hi", options, CancelToken::new())
            .await
            .expect("a launched key must yield a receipt, never an Err");

        assert_eq!(receipt.state, WorkflowSteerState::Missed);
        assert_eq!(
            receipt.error.as_deref(),
            Some("Workflow child 'lane' had no live steering route.")
        );
        assert!(
            receipt.request_id.is_none(),
            "no request was ever written, so there is no id to correlate against"
        );
        assert!(receipt.delivery_status.is_none(), "nothing was delivered");
        let targets = receipt.targets.expect("one target, this key's child");
        assert_eq!(targets[0].state, "missed");
        assert_eq!(
            requests_in(&crate::background::control::step_steer_inbox_dir(
                &run_dir, 0
            )),
            0,
            "a missed steer must not leave a file promising a delivery"
        );
    }

    /// The OTHER deadline arm, and the reason the two are not one. A lane whose control IS live but
    /// whose child never reaches its runtime does not report "no live steering route" — there WAS a
    /// route, and the reason it could not be used is knowable. pi returns the last attempt's receipt
    /// at `:4633` for exactly this, rather than falling through to `:4668`'s `missed`.
    #[tokio::test]
    async fn steer_reports_failed_with_the_reason_when_a_live_lane_never_boots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_dir = dir.path().join("wf");
        // Live control, and NO capability ever published.
        let host = host_with_live_lane(run_dir.clone(), "lane", 0);
        let options = WorkflowSteerOptions {
            ack_timeout_ms: Some(60),
            ..WorkflowSteerOptions::default()
        };

        let receipt = host
            .steer("lane", "hi", options, CancelToken::new())
            .await
            .expect("a launched key must yield a receipt");

        assert_eq!(receipt.state, WorkflowSteerState::Failed);
        assert_eq!(
            receipt.error.as_deref(),
            Some("Child session is not running yet."),
            "the reason is the whole value of this arm over a bare `Missed`"
        );
        assert_eq!(
            requests_in(&crate::background::control::step_steer_inbox_dir(
                &run_dir, 0
            )),
            0,
            "the readiness gate runs before the write on every pass of the poll"
        );
    }

    /// The poll RETRIES rather than answering, which is the whole of pi `:4501-4502`. The lane is
    /// live but its child has not published a capability when the call starts; it publishes 50 ms
    /// in, and the steer delivers instead of reporting `Missed`.
    ///
    /// Without the retry this is a `Missed` (or, before the readiness gate, a file dropped for a
    /// child that might never read it) — a concurrent lane steering a sibling that is still booting
    /// is the ordinary case, not the exotic one, because `launch` blocks until its child settles and
    /// `runs.steer` is therefore only ever called from a concurrent lane.
    #[tokio::test]
    async fn steer_polls_until_the_child_publishes_its_capability() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_dir = dir.path().join("wf");
        let host = host_with_live_lane(run_dir.clone(), "lane", 0);

        let late = run_dir.clone();
        let publisher = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            publish_capability(&late, 0, true).await;
        });

        let options = WorkflowSteerOptions {
            // Twenty times the 50 ms boot — ample headroom for the 10 ms poll to see the
            // capability land — and well under `STEER_ACK_TIMEOUT`, so the remainder spent waiting
            // for an ack no fixture can send stays a fraction of a second.
            ack_timeout_ms: Some(1_000),
            ..WorkflowSteerOptions::default()
        };
        let receipt = host
            .steer("lane", "narrow the diff", options, CancelToken::new())
            .await
            .expect("a launched key must yield a receipt");
        publisher.await.expect("the publisher task must not panic");

        assert_eq!(
            receipt.state,
            WorkflowSteerState::Queued,
            "the poll must outlast the child's boot, not answer Missed at the first look"
        );
        assert!(receipt.request_id.is_some());
        assert_eq!(
            requests_in(&crate::background::control::step_steer_inbox_dir(
                &run_dir, 0
            )),
            1,
            "exactly one request — a retry loop must not write on every pass"
        );
    }

    /// `supported: false` is TERMINAL and is answered immediately. Polling it to the deadline would
    /// turn a knowable `Failed` into a `Missed` and cost the script the reason.
    #[tokio::test]
    async fn steer_reports_failed_for_a_child_whose_host_cannot_inject() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_dir = dir.path().join("wf");
        let host = host_with_live_lane(run_dir.clone(), "lane", 0);
        publish_capability(&run_dir, 0, false).await;

        let receipt = host
            .steer(
                "lane",
                "hi",
                WorkflowSteerOptions::default(),
                CancelToken::new(),
            )
            .await
            .expect("a failed steer is a RECEIPT here, never an Err — `Err` throws in the guest");

        assert_eq!(receipt.state, WorkflowSteerState::Failed);
        assert_eq!(
            receipt.error.as_deref(),
            Some("Workflow child 'lane' cannot be steered: its host cannot inject messages.")
        );
        assert!(
            receipt.delivery_status.is_none(),
            "nothing was delivered, so the queued/delivered refinement is omitted, not null"
        );
        let targets = receipt.targets.expect("one target");
        assert_eq!(targets[0].state, "failed");
        assert_eq!(targets[0].reason.as_deref(), receipt.error.as_deref());
        assert_eq!(
            requests_in(&crate::background::control::step_steer_inbox_dir(
                &run_dir, 0
            )),
            0,
            "a child that cannot be injected into gets no file"
        );
    }

    /// pi `:4536`'s first term. An already-cancelled call answers on the first pass instead of
    /// burning the budget — the guest asked to stop, and a receipt is still owed.
    #[tokio::test]
    async fn steer_answers_missed_immediately_when_the_call_is_cancelled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_dir = dir.path().join("wf");
        let host = host_with(run_dir, "lane", 0);
        let cancel = CancelToken::new();
        cancel.cancel();

        let started = std::time::Instant::now();
        let receipt = host
            .steer("lane", "hi", WorkflowSteerOptions::default(), cancel)
            .await
            .expect("a cancelled steer still owes a receipt");

        assert_eq!(receipt.state, WorkflowSteerState::Missed);
        assert!(
            started.elapsed() < STEER_ACK_TIMEOUT,
            "cancellation must short-circuit the budget, not wait it out"
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
            let host = host_with_live_lane(run_dir.clone(), "lane", 0);
            publish_capability(&run_dir, 0, true).await;
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

    /// Register a live foreground control exactly as `register_foreground_controls` would
    /// (`foreground.rs:1022-1025`): keyed by the child's real run id, stamped with the owning
    /// workflow and lane key. Returns that run id, because it is the MAP KEY and not a field — the
    /// same shape `resolve_workflow_foreground_steering_target` has to carry out by hand.
    fn register_live_child(
        host: &WorkflowRunHost,
        parent_workflow_run_id: Option<&crate::background::RunId>,
        key: &str,
        counters: (Option<u64>, Option<u64>, Option<u64>),
    ) -> String {
        let run_id = crate::background::RunId::new().as_str().to_string();
        let (turn_count, tool_count, tokens) = counters;
        let entry = ForegroundControlEntry {
            detach: None,
            interrupt: CancelToken::new(),
            current_agent: Some("worker".to_string()),
            current_index: Some(0),
            current_activity_state: Some(crate::background::ActivityState::ActiveLongRunning),
            mode: crate::background::RunMode::Single,
            description: Some("narrow the diff".to_string()),
            current_tool: Some("Edit".to_string()),
            current_path: Some("src/lib.rs".to_string()),
            turn_count,
            tool_count,
            tokens,
            started_at: 0,
            updated_at: 0,
            session_id: None,
            parent_workflow_run_id: parent_workflow_run_id.cloned(),
            workflow_key: WorkflowKey::parse(key).ok(),
            cwd: None,
            session_name: None,
            // NON-EMPTY, exactly as `register_foreground_controls` leaves it: it calls
            // `begin_foreground_child` with the run's one child at index 0 before the entry is ever
            // visible. `lane_has_live_child` reads this term (pi `:4493`'s
            // `activeChildren.size > 0`), so an empty map here would describe a control entry this
            // crate never actually publishes.
            active_children: one_active_child(),
        };
        host.executor
            .foreground_controls
            .lock()
            .unwrap()
            .insert(run_id.clone(), entry);
        run_id
    }

    /// The single child a foreground SINGLE run has, at its own run-local index `0` — NOT the flat
    /// workflow index, which is a different namespace and lives on the steer handle
    /// (`foreground_control.rs`'s `ForegroundChildSteerHandle::index` doc).
    fn one_active_child() -> std::collections::BTreeMap<
        usize,
        crate::extension::executor::foreground_control::ForegroundChildEntry,
    > {
        let mut children = std::collections::BTreeMap::new();
        children.insert(
            0,
            crate::extension::executor::foreground_control::ForegroundChildEntry {
                index: 0,
                agent: "worker".to_string(),
                session_name: None,
                description: None,
                started_at: 0,
                updated_at: 0,
                current_activity_state: None,
                current_tool: None,
                current_path: None,
                turn_count: None,
                tool_count: None,
                tokens: None,
                interrupt: CancelToken::new(),
                // `runs.steer` never reads this — it mints its own handle from the index it
                // launched the key at (`child_steer_handle`) — so `None` is the honest fixture.
                steer: None,
            },
        );
        children
    }

    /// Publish the child's own steering capability, as a booted child's `prompt_runtime` does.
    /// `steer` will not write a request without one — that is the whole of the readiness gate.
    async fn publish_capability(run_dir: &std::path::Path, index: usize, supported: bool) {
        crate::background::control::write_steer_capability_at(
            &crate::background::control::steer_capability_path(run_dir, index),
            &crate::background::control::SteerCapability {
                kind: "steer-capability".to_string(),
                protocol_version: 1,
                index,
                // A real pid: `write_steer_capability_at` refuses a zero one.
                pid: 4242,
                ready_at: 1,
                supported,
            },
        )
        .await
        .expect("the child's capability must publish");
    }

    /// How many steer requests reached `inbox`. `0` for a directory that was never created, which
    /// is what every non-delivering arm must assert.
    fn requests_in(inbox: &std::path::Path) -> usize {
        std::fs::read_dir(inbox)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// A host with `key` launched at `index` AND a live foreground control for that lane — the two
    /// halves `steer` requires before it will write anything: the launch ledger supplies the index,
    /// the control registry supplies liveness.
    fn host_with_live_lane(run_dir: PathBuf, key: &str, index: usize) -> WorkflowRunHost {
        let host = host_with(run_dir, key, index);
        let workflow_run_id = host.workflow_run_id.clone();
        register_live_child(&host, Some(&workflow_run_id), key, (None, None, None));
        host
    }

    /// THE OBJECTIVE. A poll against a running child must return `Ok` carrying live evidence, not
    /// the "names no launched child" error — and `ok` must be `true`, because
    /// `engine.rs:1046-1050` re-wraps an `ok == false` host result as
    /// `Err("Status '<key>' failed: …")` and the script would still see a thrown error.
    #[tokio::test]
    async fn status_reports_a_live_child_with_ok_true_and_its_real_run_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = host_with(dir.path().join("wf"), "lane", 0);
        let run_id = register_live_child(
            &host,
            Some(&host.workflow_run_id),
            "lane",
            (Some(3), Some(12), Some(4096)),
        );

        let result = host
            .status("lane", CancelToken::new())
            .await
            .expect("a LIVE child must not be reported as never launched");

        assert!(
            result.ok,
            "ok means the status QUERY succeeded; false becomes a thrown error"
        );
        assert_eq!(result.key, "lane");
        assert_eq!(
            result.agent.as_deref(),
            Some("worker"),
            "from the launch identity"
        );
        assert_eq!(
            result.run_id.as_deref(),
            Some(run_id.as_str()),
            "the live child's REAL run id, read as the foreground_controls map key"
        );
        assert_eq!(
            result.output,
            "running · active_long_running · ⚒ Edit src/lib.rs · 3 turns · 12 tools · 4096 tokens"
        );
        // Not settled: there is no outcome to report yet, and inventing one is the failure this
        // change removes.
        assert!(!result.stopped && !result.interrupted && !result.detached);
        assert!(result.error.is_none());
        assert!(result.results.is_empty());
    }

    /// The guest may address either form (`prelude.js:311`'s `status(keyOrRunId)`), and for a live
    /// child the run id IS the map key. The answer still names the LANE KEY, never the run id —
    /// `WorkflowScriptChildResult::key` is a key.
    #[tokio::test]
    async fn status_answers_a_live_child_addressed_by_its_run_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = host_with(dir.path().join("wf"), "lane", 0);
        let run_id = register_live_child(
            &host,
            Some(&host.workflow_run_id),
            "lane",
            (None, None, None),
        );

        let result = host
            .status(&run_id, CancelToken::new())
            .await
            .expect("a run-id-addressed poll must resolve the same child");

        assert_eq!(result.key, "lane", "the launched key, not the run id");
        assert_eq!(result.run_id.as_deref(), Some(run_id.as_str()));
        // Absent counters are OMITTED, never rendered as `0`: `None` is "nothing folded yet", and
        // a fabricated `0 turns` would be confident and wrong.
        assert_eq!(
            result.output,
            "running · active_long_running · ⚒ Edit src/lib.rs"
        );
    }

    /// The live arm is scoped to THIS workflow. A concurrent workflow's child — or a plain
    /// foreground run that happens to share a lane name — must not be reported as this host's.
    #[tokio::test]
    async fn status_ignores_a_live_child_owned_by_another_workflow() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = host_with(dir.path().join("wf"), "lane", 0);
        let other = crate::background::RunId::new();
        register_live_child(&host, Some(&other), "lane", (Some(1), None, None));
        register_live_child(&host, None, "lane", (Some(1), None, None));

        let error = host
            .status("lane", CancelToken::new())
            .await
            .expect_err("neither entry belongs to this workflow");
        assert_eq!(
            error,
            "runs.status('lane') names no launched child in this workflow."
        );
    }

    /// Arm 3 still refuses: a key in neither the settled list nor the live registry is a script
    /// bug, and the message is the re-prompt. This is the only case its wording was ever true for.
    #[tokio::test]
    async fn status_still_refuses_a_key_that_was_never_launched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = host_with(dir.path().join("wf"), "lane", 0);
        register_live_child(
            &host,
            Some(&host.workflow_run_id),
            "lane",
            (None, None, None),
        );

        let error = host
            .status("other", CancelToken::new())
            .await
            .expect_err("an unknown key must still refuse");
        assert_eq!(
            error,
            "runs.status('other') names no launched child in this workflow."
        );
    }

    /// Arm 1 wins over arm 2: a settled result is strictly more informative than a live snapshot,
    /// so a stale control entry can never mask a real outcome.
    #[tokio::test]
    async fn a_settled_result_wins_over_a_live_control_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = host_with(dir.path().join("wf"), "lane", 0);
        register_live_child(
            &host,
            Some(&host.workflow_run_id),
            "lane",
            (Some(9), None, None),
        );
        host.settled
            .lock()
            .unwrap()
            .push(WorkflowScriptChildResult {
                key: "lane".to_string(),
                ok: true,
                agent: Some("worker".to_string()),
                run_id: Some("settled-run".to_string()),
                output: "done".to_string(),
                ..WorkflowScriptChildResult::default()
            });

        let result = host
            .status("lane", CancelToken::new())
            .await
            .expect("settled answers");
        assert_eq!(result.output, "done");
        assert_eq!(result.run_id.as_deref(), Some("settled-run"));
    }

    // --------------------------------------------------------------------------------------
    // WORKFLOW_19 — `runs.host`
    // --------------------------------------------------------------------------------------

    /// DoD 1: the flag is what installs the guest property at all. With it false `prelude.js:574`
    /// deletes `runs.host` and the script gets a `TypeError`, not upstream's refusal string — so
    /// this assertion is the difference between the verb existing and not existing.
    #[test]
    fn the_host_surface_is_advertised() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = host_rooted_at(dir.path().to_path_buf(), dir.path().join("wf"));
        assert!(host.supports_host());
    }

    /// The happy path, end to end through the real runner: a passing command settles `passed`, and
    /// its capture lands at the DEFAULT destination this host minted — under the workflow's own
    /// run directory, not under the request cwd, because the script named no `output` and the
    /// runner's default branch does no containment check at all.
    #[tokio::test]
    async fn a_passing_command_captures_into_the_workflows_own_run_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().join("proj");
        let run_dir = dir.path().join("wf");
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&run_dir).expect("run dir");
        let host = host_rooted_at(cwd.clone(), run_dir.clone());

        let result = host
            .host_command(
                "gate",
                host_params("printf 'out'; printf 'err' >&2", 30_000, None),
                CancelToken::new(),
            )
            .await
            .expect("a command that runs resolves, whatever its exit code");

        assert_eq!(
            result.state,
            crate::workflows::WorkflowHostCommandState::Passed
        );
        assert!(result.ok);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "out");
        assert_eq!(result.stderr, "err");
        assert_eq!(result.output_path, run_dir.join("host").join("gate.log"));
        assert_eq!(
            std::fs::read_to_string(&result.output_path)
                .expect("the capture must be written")
                .len(),
            6,
            "the capture holds both streams"
        );
    }

    /// The command runs in the REQUEST cwd — `self.cwd`, the one `Tool::execute` resolved — and
    /// never in this process's ambient `std::env::current_dir`, which is what a workflow launched
    /// with an explicit `cwd` would otherwise silently get.
    #[tokio::test]
    async fn the_command_runs_in_the_request_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().join("proj");
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::write(cwd.join("marker.txt"), "here").expect("marker");
        let host = host_rooted_at(cwd, dir.path().join("wf"));

        let result = host
            .host_command(
                "gate",
                host_params("cat marker.txt", 30_000, None),
                CancelToken::new(),
            )
            .await
            .expect("resolves");

        assert!(result.ok, "{:?}", result.error);
        assert_eq!(result.stdout, "here");
    }

    /// A non-zero exit is a business OUTCOME, not a technical failure: it comes back as `Ok` with
    /// `state: failed`, so the engine can map it onto a terminal `HostStepNode` and only THEN
    /// reject the guest promise. An `Err` here would skip that mapping and leave the receipt
    /// carrying a `Running` step for a command that has plainly finished.
    #[tokio::test]
    async fn a_failing_command_is_a_result_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().join("proj");
        std::fs::create_dir_all(&cwd).expect("cwd");
        let host = host_rooted_at(cwd, dir.path().join("wf"));

        let result = host
            .host_command(
                "bad",
                host_params("exit 3", 30_000, None),
                CancelToken::new(),
            )
            .await
            .expect("a failing command still RESOLVES");

        assert_eq!(
            result.state,
            crate::workflows::WorkflowHostCommandState::Failed
        );
        assert!(!result.ok);
        assert_eq!(result.exit_code, Some(3));
        assert_eq!(result.error.as_deref(), Some("Command exited with code 3."));
    }

    /// DoD 3: the timeout path is real, not simulated — a command that outlives its budget settles
    /// `timed-out` (which the engine renders as `state: "error"` + `reasonCode: "timed_out"`;
    /// there is no `timedOut` host-step state).
    #[tokio::test]
    async fn a_command_that_outlives_its_budget_settles_timed_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().join("proj");
        std::fs::create_dir_all(&cwd).expect("cwd");
        let host = host_rooted_at(cwd, dir.path().join("wf"));

        let result = host
            .host_command(
                "slow",
                host_params("sleep 30", 250, None),
                CancelToken::new(),
            )
            .await
            .expect("a timeout still RESOLVES");

        assert_eq!(
            result.state,
            crate::workflows::WorkflowHostCommandState::TimedOut
        );
        assert!(!result.ok);
        assert!(
            result.exit_code.is_none(),
            "the ladder killed it, so there is no exit code to report"
        );
        // The STATE is asserted, not the message. `settle_workflow_host_command`'s precedence puts
        // a process-tree cleanup fault ahead of the timeout sentence, and whether the group sweep
        // can verify itself empty is a property of the sandbox this test runs in, not of this
        // host — `workflows::host_command::tests::a_timeout_settles_timed_out` pins that exact
        // sentence at the layer that owns it.
        assert!(result.error.is_some(), "a timeout always carries a reason");
    }

    /// An EXPLICIT `output` is resolved against the request cwd, not the run directory — and this
    /// host passes no claim on that branch, so a correctly-behaving command must not trip
    /// `output path changed after it was claimed.`
    #[tokio::test]
    async fn an_explicit_output_lands_under_the_request_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().join("proj");
        std::fs::create_dir_all(cwd.join("reports")).expect("cwd");
        let host = host_rooted_at(cwd.clone(), dir.path().join("wf"));

        let result = host
            .host_command(
                "gate",
                host_params("printf 'x'", 30_000, Some("reports/gate.log")),
                CancelToken::new(),
            )
            .await
            .expect("resolves");

        assert!(result.ok, "{:?}", result.error);
        assert_eq!(result.output_path, cwd.join("reports/gate.log"));
        assert_eq!(
            std::fs::read_to_string(&result.output_path).expect("written"),
            "x"
        );
        assert!(
            !dir.path().join("wf").join("host").exists(),
            "an explicit output must not also mint the default destination"
        );
    }

    /// The engine's `child_cancel` reaches the child: a workflow aborted mid-command settles
    /// `stopped` (→ `HostStepState::Cancelled`) instead of holding settlement hostage to the
    /// command's own timeout.
    #[tokio::test]
    async fn an_aborted_workflow_stops_the_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().join("proj");
        std::fs::create_dir_all(&cwd).expect("cwd");
        let host = host_rooted_at(cwd, dir.path().join("wf"));
        let cancel = CancelToken::new();

        let ticket = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            ticket.cancel();
        });

        let result = host
            .host_command("slow", host_params("sleep 30", 60_000, None), cancel)
            .await
            .expect("an abort still RESOLVES");

        assert_eq!(
            result.state,
            crate::workflows::WorkflowHostCommandState::Stopped
        );
        assert!(!result.ok);
    }
}

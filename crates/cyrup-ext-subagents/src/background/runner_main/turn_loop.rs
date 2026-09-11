//! The step loop itself ([`run_inner`], arch-SA §6.5 main-loop shape; R-SA-084/095/096): the
//! G77 terminal-flag gates in pi's inbox-drain order (stop → timeout → interrupt), append
//! absorption, and per-step dispatch. Split out of `background/runner_main.rs`; ports
//! pi `runs/background/subagent-runner.ts`.

use super::config::RunnerConfig;
use super::control_watcher::ControlFlags;
use super::events::append_event;
use super::executor::ExecSingleStepExecutor;
use super::settle::{
    StepDisposition, cascade_to_descendants, imported_root_to_single_result, settle_step_result,
    skip_child_stopped_step,
};
use super::status::{
    SharedStatus, TelemetryMsg, lock_status, mark_remaining_paused, mark_remaining_stopped,
    mark_remaining_timed_out, mark_step_running, promote_interrupted_results_to_stopped,
    record_step_outcome, refresh_workflow_graph, step_display_agent, step_elapsed_ms,
    timeout_message, write_shared_status,
};
use crate::background::cascade;
use crate::background::child_stop::{ChildStatusWord, child_status_event};
use crate::background::control;
use crate::background::control::ChainAppendRequest;
use crate::background::flat_index::{flat_base, flat_range, flat_total, pending_step_statuses_for};
use crate::background::{RunId, RunPaths, RunStatus};
use crate::error::SubagentError;
use crate::exec::SingleResult;
use crate::jsonl::BoundedJsonlWriter;
use crate::spawn::chain_graph::{
    ChainRunContext, OutputRegistry, RunnerStep, SingleStepExecutor, StepResult, walk_chain,
};
use crate::spawn::depth::DepthEnvelope;
use crate::spawn::parallel::GlobalConcurrencyLimit;
use std::sync::Arc;

// =================================================================================================
// run_inner — the step loop itself
// =================================================================================================

/// The step loop's own outcome, BEFORE `run`'s tail maps it into a terminal [`RunState`](crate::background::RunState) — kept
/// distinct from a bare `Vec<SingleResult>` so the interrupted-vs-completed distinction (R-SA-084:
/// `Paused`, never `Failed`) survives without `run_inner` itself needing to know how its caller
/// will map either variant onto [`RunState`](crate::background::RunState).
pub(super) enum LoopOutcome {
    /// The step cursor was exhausted without an interrupt — every step in `results` ran to its
    /// own completion (success or failure; `run`'s tail decides `Complete` vs. `Failed` overall
    /// from `results`' own exit codes).
    Completed { results: Vec<SingleResult> },
    /// An interrupt was observed and consumed before the step cursor was exhausted — `results`
    /// holds every step that DID complete before the interrupt landed; steps that never got to
    /// run are left `Pending` in `status.steps` (R-SA-084: "mark every currently-running step
    /// Paused... before signaling its own actively-spawned child subprocess(es)" — this phase has
    /// no live child to signal mid-step since interrupts are only checked BETWEEN steps, see this
    /// function's own doc note on that scope boundary).
    Interrupted { results: Vec<SingleResult> },
    /// A wall-clock deadline expired — either this run's own (`config.deadline_at_ms`, observed as
    /// a step whose child was killed by the deadline) or an ancestor's, delivered as a
    /// `control/timeout.json` request (pi `timeoutRunner`, `subagent-runner.ts:2987-3025`
    /// @v0.34.0).
    ///
    /// Deliberately NOT folded into `Interrupted`: an interrupt is a resumable pause (`Paused`,
    /// every unfinished step `Paused`, `resume` can pick it up), whereas a timeout is terminal
    /// failure (`Failed`, `timedOut`, every unfinished step `Failed` with the timeout message,
    /// nothing to resume). Collapsing the two would make an expired deadline look resumable and
    /// leave a permanently-`Paused` record nothing ever revisits.
    TimedOut {
        results: Vec<SingleResult>,
        /// The message stamped onto the run's terminal error and every step it failed.
        message: String,
    },
    /// G77 — an explicit stop request (`control/stop.json`, pi `StopRequest`) was observed and
    /// consumed: pi `stopRunner` (`subagent-runner.ts:2955-2984` @v0.43.0). `results` holds every
    /// step that DID complete before the stop landed.
    ///
    /// Deliberately NOT folded into `Interrupted` or `TimedOut`. Against `Interrupted`: a stop is
    /// terminal and `resume` MUST refuse it (`async-resume.ts:406`), where a pause is exactly what
    /// `resume` exists for. Against `TimedOut`: the terminal `state` is `Stopped`, not `Failed`, and
    /// every downstream reader — the notify status word (`notify.ts:210`), the grouped intercom
    /// verdict (`result-intercom.ts:84-87`), the `status` action's `State:` line
    /// (`run-status.ts:478-479`) — prints a different word for it. Collapsing either way loses a
    /// user-visible distinction upstream maintains at four separate sites.
    Stopped {
        results: Vec<SingleResult>,
        /// The message stamped onto the run's terminal error and every step it stopped — always
        /// the request's own `reason` when it carried one, else [`control::STOP_MESSAGE`].
        message: String,
    },
}

/// Drive the step-execution loop itself (R-SA-076 write-ordering per iteration, R-SA-084 interrupt
/// check, R-SA-095/096 append-request consumption, dispatch via the Phase-3 spawn boundary).
///
/// # Interrupt-check granularity (a deliberate, documented scope boundary)
///
/// This loop re-checks `interrupted` BETWEEN steps (the "should I even START the next step" gate),
/// but a step's OWN live child is ALSO interruptible mid-flight: the run-wide shared
/// [`cyrup_core::CancelToken`] (`interrupt_cancel`) is threaded into every dispatched step's
/// [`exec::RunOptions::interrupt`](crate::exec::RunOptions::interrupt) (see [`ExecSingleStepExecutor::run_single`]), so when the
/// control-inbox watcher cancels it the in-flight child is torn down via the normal
/// `exec::run_sync` -> `SpawnedChild::terminate` signal-escalation path (R-SA-036/059/084) rather
/// than merely noticed after the step finishes — and an interrupted step ends the run `Paused`.
///
/// # Errors
///
/// Returns `Err` only for a genuine I/O failure writing `status.json` mid-loop (R-SA-076) — a
/// single step's own failure (nonzero exit, timeout, etc.) is NOT an `Err` here; it is recorded as
/// a `SingleResult` with a nonzero `exit_code` and the loop continues to the next step exactly as
/// R-SA-052's chain-walk semantics dictate (a chain does not abort on one step's failure unless
/// the group itself is `fail_fast`, which `walk_chain`/`run_bounded` already enforce internally).
// One over the limit, from the in-process `spawn_command` hand-down `run_with` needs; the
// alternative is a parameter struct that exists only to satisfy the lint.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_inner(
    roots: &crate::paths::Roots,
    child_env: &std::collections::HashMap<String, String>,
    spawn_command: Option<&crate::spawn::SpawnCommand>,
    config: &RunnerConfig,
    run_paths: &RunPaths,
    status: &SharedStatus,
    flags: &ControlFlags,
    interrupt_cancel: &cyrup_core::CancelToken,
    telemetry: tokio::sync::mpsc::UnboundedSender<TelemetryMsg>,
    events: &mut Option<BoundedJsonlWriter>,
) -> Result<LoopOutcome, SubagentError> {
    let mut steps = config.steps.clone();
    let mut cursor = 0usize;
    let mut results: Vec<SingleResult> = Vec::new();
    let mut registry = OutputRegistry::new();

    let depth = crate::spawn::depth::resolve_effective_depth(config.max_subagent_depth);
    ensure_depth_available(&depth)?;

    let (executor, mut ctx) = build_chain_context(
        child_env,
        spawn_command,
        config,
        run_paths,
        flags,
        interrupt_cancel,
        telemetry,
        depth,
    );

    let mut io = TurnLoopIo {
        roots,
        config,
        run_paths,
        status,
        events,
        flags,
    };

    loop {
        // G77 — the three terminal-flag checks below run in pi's own inbox-drain order, and MUST
        // stay in it: stop FIRST, then timeout, then interrupt
        // (`runs/background/control-channel.ts:653-655` @v0.43.0). Each one returns `Some` only
        // when it actually consumed a pending request, so the FIRST that does wins and ends the
        // run: a stop outranks a timeout outranks an interrupt, and the terminal record is always
        // the hardest, least-resumable verdict. Each check carries its own rationale on its `fn`.
        if let Some(outcome) = check_stop_flag(&mut io, &steps, cursor, &mut results).await? {
            return Ok(outcome);
        }
        if let Some(outcome) = check_timeout_flag(&mut io, &steps, cursor, &mut results).await? {
            return Ok(outcome);
        }
        if let Some(outcome) = check_interrupt_flag(&mut io, &steps, cursor, &mut results).await? {
            return Ok(outcome);
        }

        absorb_pending_appends(&mut io, &mut steps).await?;

        if cursor >= steps.len() {
            return Ok(LoopOutcome::Completed { results });
        }

        let step = steps.get(cursor).cloned().ok_or_else(|| {
            SubagentError::Spawn(std::io::Error::other("step cursor out of range"))
        })?;

        // SUBA-093 — the FLAT status slots this top-level step occupies. A `ParallelGroup` owns
        // one slot per member; every other shape owns exactly one.
        let flat_slots = flat_range(&steps, cursor);

        // SUBA-087 — pi `if (childStopRequests.has(flatIndex)) { results.push(childStopResult(…));
        // flatIndex++; continue; }` (`subagent-runner.ts:4937-4941` @v0.64.0): a child-scoped stop
        // that landed while this step was still `pending` (`subagent.step.stop_queued`) is applied
        // HERE, before dispatch — the step is marked `stopped` without ever spawning a child, and
        // the loop moves on to the next step. The run itself stays alive.
        //
        // SUBA-093: only for a step that occupies exactly ONE flat slot, which is pi's own
        // sequential branch. A wider `ParallelGroup` is addressed per member instead, inside the
        // fan-out (`ExecSingleStepExecutor::run_single`, pi `:4221`) — resolving a member's stop
        // here would tear down the whole group, which is precisely the defect this item closes.
        if flat_slots.len() == 1
            && let Some(record) = io.flags.child_stops.recorded(flat_slots.start)
        {
            skip_child_stopped_step(
                &mut io,
                &steps,
                flat_slots.start,
                &step,
                &record,
                &mut results,
            )
            .await?;
            cursor += 1;
            continue;
        }

        // SUBA-093 — publish this step's flat base on the context BEFORE dispatch. The dispatch
        // adapter reads it back for the live-telemetry tag, the per-child steer paths, the child's
        // intercom label and its stop handle; `dispatch_group` re-stamps it per member for a
        // parallel fan-out (pi's per-step `ctx.flatIndex`, `subagent-runner.ts:1294`).
        ctx.step_slot = crate::spawn::chain_graph::StepSlot::Exclusive(flat_slots.start);

        {
            let mut guard = lock_status(status);
            let s = &mut *guard;
            // SUBA-093: every member of a group goes `Running` when the group is dispatched.
            // [CYRUP-DELTA, granularity only] pi marks each member at the moment its own worker
            // claims it (`subagent-runner.ts:4236-4238`); cyrup's fan-out happens behind
            // `walk_chain`, which reports no per-member start, so a concurrency-limited group can
            // show a member `Running` slightly before its worker claims a permit. Nothing keys on
            // the distinction: `is_stoppable_step_state` accepts `Pending` and `Running` alike.
            for flat in flat_slots.clone() {
                mark_step_running(s, flat);
            }
            s.current_step = Some(flat_slots.start);
            refresh_workflow_graph(s, &steps);
            s.touch();
        }
        write_shared_status(run_paths, status)
            .await
            .map_err(SubagentError::Spawn)?;
        append_event(
            io.events,
            "subagent.step.started",
            Some(serde_json::json!({
                "runId": config.run_id.as_str(),
                "stepIndex": flat_slots.start,
                "agent": step_display_agent(&step),
            })),
        )
        .await;

        if let RunnerStep::ImportAsyncRoot(spec) = &step {
            run_import_async_root(
                &mut io,
                &steps,
                flat_slots.start,
                &step,
                spec,
                &mut registry,
                &mut results,
            )
            .await?;
            cursor += 1;
            continue;
        }

        // Dispatch via the Phase-3 spawn boundary (chain_graph::walk_chain over a ONE-element
        // graph for this single cursor position — reusing the exact same SingleStep/ParallelGroup/
        // DynamicGroup dispatch `walk_chain` already implements, rather than re-implementing group
        // fan-out inline here). `ChainGraph` is a plain `Vec<RunnerStep>` type alias, so the
        // one-element "graph" is just a fresh one-element `Vec`.
        let one_step: Vec<RunnerStep> = vec![step.clone()];
        let walked = walk_chain(&one_step, &mut registry, &executor, &ctx).await;
        let (step_results, group_results) = walked?;

        let step_result = step_results.into_iter().next().ok_or_else(|| {
            SubagentError::Spawn(std::io::Error::other(
                "walk_chain produced no result for a single dispatched step",
            ))
        })?;

        match settle_step_result(
            &mut io,
            &steps,
            flat_slots,
            step,
            step_result,
            group_results,
            &mut results,
        )
        .await?
        {
            StepDisposition::Advance => cursor += 1,
            StepDisposition::Requeue => {}
            StepDisposition::Finish(outcome) => return Ok(outcome),
        }
    }
}

/// The run-wide handles every turn-loop helper below writes through: the one-shot config, this
/// run's [`RunPaths`], the shared [`RunStatus`] the telemetry pump also mutates, the `events.jsonl`
/// writer, and the three control-inbox flags. Grouped into one struct purely so each helper takes
/// a readable argument list instead of five threaded parameters — no helper stores it.
pub(super) struct TurnLoopIo<'a> {
    /// The roots this run resolves against, so the terminal-flag checks and the step settler all
    /// read ONE resolution rather than each re-deriving it from the environment mid-run.
    pub(super) roots: &'a crate::paths::Roots,
    pub(super) config: &'a RunnerConfig,
    pub(super) run_paths: &'a RunPaths,
    pub(super) status: &'a SharedStatus,
    pub(super) events: &'a mut Option<BoundedJsonlWriter>,
    pub(super) flags: &'a ControlFlags,
}

/// R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST in this loop's own setup — before
/// any step's discovery-free-but-still-real worktree setup (`chain_graph::assign_worktree_cwds`
/// -> `spawn::worktree::setup_worktree_group`, which shells out to real `git` subprocesses) or
/// any child OS process is spawned for ANY step in this run's chain. This hop-2 runner process
/// is itself already one recursion hop deep (its own `depth.current_depth` reflects however
/// many ancestors spawned it, propagated via `CYRUP_SUBAGENT_DEPTH`/`_MAX_DEPTH`, R-SA-054) —
/// if that inherited envelope is already at its ceiling, this run must reject EVERY one of its
/// configured steps up front rather than dispatching the first one and only then discovering
/// `ExecSingleStepExecutor::run_single` -> `exec::run_sync`'s own independent re-check rejects
/// it (which would still be correct per R-SA-055's letter for that one step, since no spawn
/// would have happened yet, but would incorrectly leave every LATER step in `steps` looking
/// like it was simply never reached rather than explicitly blocked, and would run any
/// `worktree: true` group's real `git worktree add` setup for nothing before the per-child
/// dispatch inside `run_bounded` ever reached `run_sync`'s own guard). Failing the whole run
/// here, before the loop even starts, keeps the rejection uniform across every step shape
/// (`SingleStep`/`ParallelGroup`/`DynamicGroup`) and guarantees zero worktrees and zero child
/// processes are ever created for a run whose own depth is already exhausted.
fn ensure_depth_available(depth: &DepthEnvelope) -> Result<(), SubagentError> {
    if crate::spawn::depth::is_blocked(depth) {
        return Err(SubagentError::DepthExceeded {
            current: depth.current_depth,
            max: depth.max_depth,
        });
    }
    Ok(())
}

/// Build the two values every dispatched step is driven through: the [`SingleStepExecutor`] that
/// carries this run's depth envelope, interrupt token, resolved personas and per-step policy into
/// each spawned child, and the [`ChainRunContext`] `walk_chain` resolves each step against.
// As `run_inner` above: one over, for the same threaded value.
#[allow(clippy::too_many_arguments)]
fn build_chain_context(
    child_env: &std::collections::HashMap<String, String>,
    spawn_command: Option<&crate::spawn::SpawnCommand>,
    config: &RunnerConfig,
    run_paths: &RunPaths,
    flags: &ControlFlags,
    interrupt_cancel: &cyrup_core::CancelToken,
    telemetry: tokio::sync::mpsc::UnboundedSender<TelemetryMsg>,
    depth: DepthEnvelope,
) -> (Arc<dyn SingleStepExecutor>, ChainRunContext) {
    let global_limit = GlobalConcurrencyLimit::new(config.global_concurrency_limit.max(1));
    let cancel_root = cyrup_core::CancelToken::new();
    // T0.1 / C13: the per-agent resolved-persona map the orchestrator baked into the one-shot
    // config is threaded straight into the executor so every dispatched step runs its REAL named
    // persona (never re-discovered, never a placeholder). `Arc`-shared so a parallel/dynamic
    // group's fanned-out children share one map rather than cloning it per child.
    let resolved_agents = Arc::new(config.resolved_agents.clone());
    let executor: Arc<dyn SingleStepExecutor> = Arc::new(ExecSingleStepExecutor {
        // `None` on the REAL detached hop-2 runner: it reaches its steps through a `RunnerConfig`
        // written to disk as JSON, so nothing in-process can be handed down and these steps resolve
        // their command from the environment they inherited, exactly as before. `Some` only when a
        // caller drives `run_with` in-process and therefore IS the runner.
        spawn_command: spawn_command.cloned(),
        child_env: child_env.clone(),
        depth,
        interrupted: Arc::clone(&flags.interrupted),
        interrupt_cancel: interrupt_cancel.clone(),
        child_stops: Some(flags.child_stops.clone()),
        telemetry: Some(telemetry),
        resolved_agents,
        // Intercom child-bridge (pi `subagent-runner.ts:779-783`): the orchestrator's presence target
        // + this run's id, carried in the one-shot config, so every step's spawned child activates
        // its `contact_supervisor` bridge addressed at the launching supervisor.
        orchestrator_intercom_target: config.orchestrator_intercom_target.clone(),
        run_id: Some(config.run_id.clone()),
        // Session-model inheritance (pi `ctx.model`): the live parent session model the orchestrator
        // captured at plan time, carried through the one-shot config (this detached process has no
        // host-services backend of its own), so an inheriting step resolves the parent's model.
        inherited_session_model: config.inherited_session_model.clone(),
        // SCOPE_19/A1: and the parent session's reasoning level, carried the same way and for the
        // same reason — this detached process cannot probe the parent's thinking level itself.
        inherited_session_thinking: config.inherited_session_thinking.clone(),
        // pi `ctx.hostAvailableBuiltins` (`subagent-runner.ts:3703`): the LAUNCHING orchestrator's
        // host observation, carried the same way and for the same reason as the two fields above —
        // this detached process has no host-services backend, and its own tool registry is not the
        // parent's, so re-reading here would answer a different question.
        host_available_builtins: config.host_available_builtins.clone(),
        // SUBA-003: the model-scope policy the orchestrator authorized this run under, carried in
        // the one-shot config for the same reason as the two fields above — this process performs
        // no discovery and reads no settings.
        model_scope: config.model_scope.clone(),
        // SUBA-N05 (pi `const controlConfig = config.controlConfig ?? DEFAULT_CONTROL_CONFIG`,
        // `subagent-runner.ts:1802` @v0.34.0): the live-control config resolved parent-side and
        // carried in the one-shot config, so an async run's `control` override genuinely changes
        // the thresholds each step's child stream is judged against instead of being dropped.
        control: config.control.clone(),
        // SUBA-N06: R-SA-043 compaction's opt-out, carried the same way and for the same reason.
        include_progress: config.include_progress,
        // SUBA-N03: the run's `share` opt-in and artifact destination/selection, carried from the
        // one-shot config so an async run honours `share`/`artifacts` and leaves the same artifact
        // quadruple a foreground run does (pi `subagent-runner.ts:879-890,1117-1125` @v0.34.0).
        share: config.share,
        // SUBA-008 — the run-level turn budget reaches every step from the one-shot config.
        turn_budget: config.turn_budget,
        // SUBA-073 — and so does the run-level, fully-merged permission policy, for the same
        // reason and by the same route: this process performs no discovery and reads no live
        // extension config.
        permission_rules: config.permission_rules.clone(),
        // SUBA-021 — and so does the run-level usage budget, for the same reason and by the same
        // route (pi `ctx.usageBudget` ← `config.usageBudget`, `subagent-runner.ts:172`).
        usage_budget: config.usage_budget,
        artifacts_dir: config.artifacts_dir.clone(),
        artifact_config: config.artifact_config,
        // G90 (pi `subagent-runner.ts:2313,2600,2797` @v0.34.0): the async run dir every
        // dispatched step derives its own `steer-targets/<flatIndex>/` inbox from. This is the
        // detached hop-2 runner, so it is exactly the process upstream gives `steerInboxDir` to.
        run_dir: Some(run_paths.run_dir.clone()),
    });
    // SUBA-N03 — pi `subagent-runner.ts:2078-2081`: `const remainingMs = Math.max(0,
    // config.deadlineAt - Date.now())`. The orchestrator stamped an ABSOLUTE epoch deadline into
    // the one-shot config; convert it back to a local `Instant` ONCE here, charging the elapsed
    // hop-1 spawn + hop-2 startup time against the budget rather than refunding it. An
    // already-passed deadline collapses to `now` (`max(0, …)`), so the first step is refused
    // immediately instead of the subtraction wrapping into a far-future instant.
    //
    // This replaces a hardcoded `None` justified as "R-SA-036: background runs have no built-in
    // wall-clock timeout". That remains true of the DEFAULT — `timeout_ms`/`deadline_at_ms` are
    // `None` unless the caller asked for a timeout — but it was never a reason to DROP an explicit
    // one, and upstream has always honoured `timeoutMs` on the async path (`schemas.ts:265-266`
    // and `tool-description.ts:25,:73` @v0.34.0 both say it applies to "foreground and
    // async/background runs"; `async-execution.ts:1302-1305` arms the deadline).
    let deadline_at = config.deadline_at_ms.map(|deadline_ms| {
        let remaining_ms =
            deadline_ms.saturating_sub(u64::try_from(crate::time::now_epoch_millis()).unwrap_or(0));
        std::time::Instant::now() + std::time::Duration::from_millis(remaining_ms)
    });
    let ctx = ChainRunContext {
        cwd: config.cwd.clone(),
        deadline_at,
        // The NOMINAL budget, rendered into a timed-out step's message and never re-derived per
        // step (pi's `timeoutMessage = \`Subagent timed out after ${config.timeoutMs}ms.\``,
        // `subagent-runner.ts:1339`).
        timeout_ms: config.timeout_ms,
        cancel: cancel_root.clone(),
        global_limit,
        worktree_base_dir: config.worktree_base_dir.clone(),
        // The registry (shared across every one-step `walk_chain` call in the loop below) carries
        // the rolling `{previous}` text, so step-to-step piping works even though each `walk_chain`
        // invocation walks a single step. `{task}`/`{chain_dir}` resolve from the run-wide values the
        // orchestrator serialized into the one-shot config (A: pi `originalTask`/`chainDir`), so the
        // detached runner substitutes the SAME values the foreground `/chain` path does.
        original_task: config.original_task.clone(),
        chain_dir: config.chain_dir.clone(),
        // C16 / pi `config.chain.dynamicFanout.maxItems`: the orchestrator resolves this ONCE at
        // plan time (`config_snapshot().dynamic_fanout_max_items()`) and bakes it into the one-shot
        // `RunnerConfig`, so a background dynamic-fanout step whose own `expand.maxItems` is absent
        // falls back to the SAME run-wide cap the foreground path applies, rather than always
        // failing materialization.
        dynamic_fanout_max_items: config.dynamic_fanout_max_items,
        // SUBA-093: re-stamped by `run_inner` before every dispatch (and again per member by
        // `dispatch_group`); the initial value is only the first step's own base.
        step_slot: crate::spawn::chain_graph::StepSlot::Exclusive(0),
    };
    (executor, ctx)
}

/// G77 — the stop flag, read at the very top of every loop iteration ahead of the other two.
///
/// G77 — STOP is checked before BOTH of the others, matching pi's own inbox-drain order
/// (`runs/background/control-channel.ts:653-655` @v0.43.0: `consumeStopRequest` → `consumeTimeoutRequest` →
/// `consumeInterruptRequest`) and `stopRunner`'s mutual-exclusion guard
/// (`subagent-runner.ts:2955-2986`: `if (stopped || timedOut || interrupted || …) return`). The
/// order is load-bearing when several land together: a stop outranks a timeout outranks an
/// interrupt, so the terminal record is always the hardest, least-resumable verdict.
///
/// Unlike the interrupt arm below there is no `cursor < steps.len()` moot-signal guard: a
/// stop that lands after the last step finished still ends the run `Stopped` upstream
/// (`stopRunner` only checks `statusPayload.state === "running"`, which is still true until
/// `finish_run` writes the terminal record), and — unlike the interrupt case that guard
/// exists for — that is not a downgrade to a permanently-wrong non-terminal record: `Stopped`
/// IS terminal, so nothing is left waiting to be resumed.
async fn check_stop_flag(
    io: &mut TurnLoopIo<'_>,
    steps: &[RunnerStep],
    cursor: usize,
    results: &mut Vec<SingleResult>,
) -> Result<Option<LoopOutcome>, SubagentError> {
    let config = io.config;
    let run_paths = io.run_paths;
    let status = io.status;
    let events = &mut *io.events;
    let stopped = &io.flags.stopped;
    if stopped.load(std::sync::atomic::Ordering::SeqCst) {
        if let Some(request) = control::consume_stop_request(run_paths).await? {
            let message = request
                .reason
                .clone()
                .unwrap_or_else(|| control::STOP_MESSAGE.to_string());
            let recorded_child_stops = io.flags.child_stops.recorded_indexes();
            let stopped_children: Vec<(usize, String, String)> = {
                let mut guard = lock_status(status);
                let s = &mut *guard;
                mark_remaining_stopped(s, flat_base(steps, cursor), flat_total(steps), &message);
                refresh_workflow_graph(s, steps);
                s.touch();
                recorded_child_stops
                    .into_iter()
                    .map(|(index, record)| {
                        let agent = s
                            .steps
                            .get(index)
                            .map(|step| step.agent.clone())
                            .unwrap_or_default();
                        (index, record.child_id, agent)
                    })
                    .collect()
            };
            write_shared_status(run_paths, status)
                .await
                .map_err(SubagentError::Spawn)?;
            // SUBA-087 — pi `appendTerminalChildStatusEvent` (`subagent-runner.ts:2975-2978`,
            // called at `:4340` under `stopped || childStopped`): a child whose OWN stop was
            // recorded gets its terminal `subagent.child-status` `stopped` even when the whole
            // run stopped first.
            let now = crate::time::now_epoch_millis();
            for (index, child_id, agent) in stopped_children {
                append_event(
                    events,
                    "subagent.child-status",
                    Some(child_status_event(
                        config.run_id.as_str(),
                        index,
                        &child_id,
                        &agent,
                        ChildStatusWord::Stopped,
                        now,
                    )),
                )
                .await;
            }
            // pi `stopNestedAsyncDescendants()` (`subagent-runner.ts:2984`) — stop the whole
            // subtree, not just this run, or every background run this one spawned keeps going
            // detached and unreachable after the user asked for it to stop.
            cascade_to_descendants(io.roots, config, events, cascade::CascadeVerb::Stop).await;
            promote_interrupted_results_to_stopped(results, &message);
            return Ok(Some(LoopOutcome::Stopped {
                results: std::mem::take(results),
                message,
            }));
        }
        // Same idempotent absorption the other two arms document: a watch notification with
        // nothing actually pending clears the flag rather than looping.
        stopped.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(None)
}

/// Timeout is checked BEFORE interrupt, matching pi's own inbox-drain order
/// (`runs/background/control-channel.ts:654-655` @v0.43.0: `if (consumeTimeoutRequest(...)) onTimeout();`
/// then `if (consumeInterruptRequest(...)) onInterrupt();`). The order is load-bearing when
/// both land together — an ancestor that timed out cascades a timeout to this run while a
/// user may simultaneously be interrupting it, and the terminal record must be the harder
/// of the two verdicts (`Failed`/timed-out, not a resumable `Paused`).
async fn check_timeout_flag(
    io: &mut TurnLoopIo<'_>,
    steps: &[RunnerStep],
    cursor: usize,
    results: &mut Vec<SingleResult>,
) -> Result<Option<LoopOutcome>, SubagentError> {
    let config = io.config;
    let run_paths = io.run_paths;
    let status = io.status;
    let events = &mut *io.events;
    let timed_out = &io.flags.timed_out;
    if timed_out.load(std::sync::atomic::Ordering::SeqCst) {
        if let Some(request) = control::consume_timeout_request(run_paths).await? {
            let message = request
                .reason
                .clone()
                .unwrap_or_else(|| timeout_message(config.timeout_ms, &request.source));
            {
                let mut guard = lock_status(status);
                let s = &mut *guard;
                mark_remaining_timed_out(s, flat_base(steps, cursor), flat_total(steps), &message);
                refresh_workflow_graph(s, steps);
                s.touch();
            }
            write_shared_status(run_paths, status)
                .await
                .map_err(SubagentError::Spawn)?;
            // Fail the whole subtree, not just this run — see `background::cascade`.
            cascade_to_descendants(io.roots, config, events, cascade::CascadeVerb::Timeout).await;
            return Ok(Some(LoopOutcome::TimedOut {
                results: std::mem::take(results),
                message,
            }));
        }
        // Same idempotent absorption the interrupt branch below documents: a watch
        // notification with nothing actually pending clears the flag rather than looping.
        timed_out.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(None)
}

/// R-SA-084: check interrupted FIRST, before consuming appends or dispatching further
/// work — an interrupt that lands must stop new-step dispatch as soon as this loop next
/// observes it, not after one more (possibly append-extended) step has already started.
///
/// Race guard (found in second-pass adversarial review): a natural completion and an
/// interrupt delivery can land in the same instant — `interrupt()` reads `status.json` and
/// sees `state: Running` (which stays true right up until `finish_run` writes the terminal
/// record), so it can successfully write a control-inbox request and set `interrupted` in
/// the tiny window AFTER this loop's last step already finished (`cursor` already advanced
/// past the final index) but BEFORE this loop's next top-of-iteration check. Without the
/// `cursor < steps.len()` guard below, that late, moot interrupt would still be consumed
/// and reported as `LoopOutcome::Interrupted`, downgrading a run whose every step actually
/// completed into a non-terminal `Paused` `ResultFile` (`success: false`) with no step left
/// to resume from — a permanently-wrong terminal record, since nothing ever reconciles a
/// `Paused` run back to `Complete` after the fact. Only treat the interrupt as a genuine
/// pause when there is still unstarted/unfinished step work for it to actually pause;
/// otherwise silently absorb it (matching R-SA-083's own "duplicate/stale signal MUST be
/// silently absorbed" idempotency principle, applied here to a signal that is stale relative
/// to the run's own already-finished work rather than stale relative to a prior consumption)
/// and let the loop fall through to its normal `Completed` exit on this same iteration.
async fn check_interrupt_flag(
    io: &mut TurnLoopIo<'_>,
    steps: &[RunnerStep],
    cursor: usize,
    results: &mut Vec<SingleResult>,
) -> Result<Option<LoopOutcome>, SubagentError> {
    let config = io.config;
    let run_paths = io.run_paths;
    let status = io.status;
    let events = &mut *io.events;
    let interrupted = &io.flags.interrupted;
    if interrupted.load(std::sync::atomic::Ordering::SeqCst) && cursor < steps.len() {
        if let Some(request) = control::consume_interrupt_request(run_paths).await? {
            {
                let mut guard = lock_status(status);
                let s = &mut *guard;
                mark_remaining_paused(s, flat_base(steps, cursor), flat_total(steps));
                refresh_workflow_graph(s, steps);
                s.touch();
            }
            write_shared_status(run_paths, status)
                .await
                .map_err(SubagentError::Spawn)?;
            let _ = request; // consumed; contents already reflected via status/event log.
            // R-SA-084 stops THIS run; without the cascade every background run this one
            // spawned would keep running, detached and unreachable — see `background::cascade`.
            cascade_to_descendants(io.roots, config, events, cascade::CascadeVerb::Interrupt).await;
            return Ok(Some(LoopOutcome::Interrupted {
                results: std::mem::take(results),
            }));
        }
        // The watcher observed a notification but a synchronous re-check found nothing
        // pending (already consumed by a race, or a stale wake-up) — R-SA-083's idempotent
        // absorption, restated here: clear the flag and keep going rather than looping forever
        // treating a one-shot notification as sticky.
        interrupted.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(None)
}

/// R-SA-095/096: consume pending append requests EVERY iteration, before checking whether
/// the step cursor is exhausted — re-scans disk (never trusts the in-memory `steps` list as
/// the source of truth for what is pending), per R-SA-096's explicit "MUST re-scan disk,
/// not cache" requirement.
async fn absorb_pending_appends(
    io: &mut TurnLoopIo<'_>,
    steps: &mut Vec<RunnerStep>,
) -> Result<(), SubagentError> {
    let run_paths = io.run_paths;
    let status = io.status;
    let pending = control::list_pending_appends(&run_paths.append_dir).await?;
    if !pending.is_empty() {
        for (path, parsed) in pending {
            if let Some(request) = parsed {
                let mut guard = lock_status(status);
                append_steps(steps, &mut guard, &request);
            }
            // Delete-then-act, at-most-once (R-SA-095: "MUST list, read, and DELETE all
            // pending request files... and only then extend its own in-loop step list").
            let _ = tokio::fs::remove_file(&path).await;
        }
        let pending_count = control::count_pending_appends(&run_paths.append_dir).await?;
        {
            let mut guard = lock_status(status);
            let s = &mut *guard;
            s.pending_appends = Some(pending_count);
            s.chain_step_count = Some(steps.len());
            refresh_workflow_graph(s, steps);
            s.touch();
        }
        write_shared_status(run_paths, status)
            .await
            .map_err(SubagentError::Spawn)?;
    }
    Ok(())
}

/// R-SA-097 root attachment (chain-root-attachment.ts): an `ImportAsyncRoot` step is NOT
/// dispatched by spawning a child — it is synthesized by POLLING another already-launched
/// run's terminal files (mirroring pi's `runSingleStep` short-circuit `if (step.importAsyncRoot)`,
/// `subagent-runner.ts:1153`). Intercept it here, before the `walk_chain` dispatch, so the
/// runner "calls the poll" (`control::wait_for_imported_async_root`) rather than routing it
/// through the `SingleStepExecutor` spawn seam that would (correctly) have no idea how to run
/// it.
pub(super) async fn run_import_async_root(
    io: &mut TurnLoopIo<'_>,
    steps: &[RunnerStep],
    cursor: usize,
    step: &RunnerStep,
    spec: &crate::spawn::chain_graph::ImportAsyncRootSpec,
    registry: &mut OutputRegistry,
    results: &mut Vec<SingleResult>,
) -> Result<(), SubagentError> {
    let config = io.config;
    let run_paths = io.run_paths;
    let status = io.status;
    let events = &mut *io.events;
    let target_run_id = RunId::from_token(spec.run_id.clone());
    let target_paths = RunPaths::for_run(&spec.async_root, &spec.results_dir, &target_run_id);
    let imported = control::wait_for_imported_async_root(
        &target_paths,
        &spec.run_id,
        spec.index,
        &spec.agent,
        control::ROOT_ATTACHMENT_POLL_INTERVAL,
    )
    .await?;

    let step_result = StepResult {
        success: imported.success,
        structured_output: imported.structured_output.clone(),
        final_output: Some(imported.output.clone()),
        error: imported.error.clone(),
        interrupted: false,
        // An IMPORTED async root's control events belong to the run that was attached, and
        // are already recorded on ITS own terminal `ResultFile` — `ImportedAsyncRootResult`
        // deliberately carries only the identity/output fields
        // `imported_root_to_single_result` reproduces, so there is nothing to re-attribute
        // here (matching pi's `runSingleStep`, `subagent-runner.ts:1162-1181`).
        control_events: Vec::new(),
        // Same reasoning for the per-child detail fields: an imported root's real exit code
        // is carried on `ImportedAsyncRootResult` and reproduced by
        // `imported_root_to_single_result`; an `ImportAsyncRoot` step can never be a
        // dynamic-fanout child, so no collect record ever reads these.
        exit_code: None,
        timed_out: false,
        // The imported child's own terminal classification (pi `contextOverflow:
        // imported.contextOverflow`, `subagent-runner.ts:767`, flowing into the status write at
        // `:4570`) — an identity field like `model`/`session_file` below, NOT per-child collect
        // detail: `record_step_outcome` stamps it onto this step's `StepStatus`, keeping the
        // status surface in agreement with the `ResultFile` side
        // (`imported_root_to_single_result`).
        context_overflow: imported.context_overflow,
        saved_output_path: None,
        artifact_paths: None,
        // Usage/turns stay zero for the same reason `imported_root_to_single_result` leaves them
        // zero: the target run's spend lives on its OWN terminal `ResultFile`, and re-counting it
        // here would double-bill the importing chain. Identity fields (model, ladder, transcript)
        // ARE the imported child's own, so `record_step_outcome` stamps the true ones onto this
        // step's `StepStatus`.
        usage: Default::default(),
        turns: 0,
        model: imported.model.clone(),
        attempted_models: imported.attempted_models.clone(),
        session_file: imported.session_file.clone(),
        output_state: crate::exec::output_state::derive_output_state(
            Some(imported.output.as_str()),
            imported.structured_output.as_ref(),
            None,
        ),
        structured_output_path: None,
        // An imported async root's recovery evidence lives on ITS own terminal `ResultFile`
        // (pi's import copy, `subagent-runner.ts:760-790`, carries no `timeoutRecovery`).
        timeout_recovery: None,
    };
    // Register the imported output under its named key (pi's `outputName`/`as`) so a later
    // `{outputs.name}` reference in this chain resolves to it — a validated structured
    // output when present, otherwise the imported text (R-SA-053).
    if let Some(name) = &spec.output {
        let value = imported
            .structured_output
            .clone()
            .unwrap_or_else(|| serde_json::Value::String(imported.output.clone()));
        registry.register(name.clone(), value);
    }

    let step_duration_ms;
    {
        let mut guard = lock_status(status);
        let s = &mut *guard;
        record_step_outcome(s, &(cursor..cursor + 1), step, &step_result, None);
        step_duration_ms = step_elapsed_ms(s, cursor);
        refresh_workflow_graph(s, steps);
        s.touch();
    }
    append_event(
        events,
        if step_result.success {
            "subagent.step.completed"
        } else {
            "subagent.step.failed"
        },
        Some(serde_json::json!({
            "runId": config.run_id.as_str(),
            "stepIndex": cursor,
            "agent": step_display_agent(step),
            "exitCode": i32::from(!step_result.success),
            "durationMs": step_duration_ms,
        })),
    )
    .await;
    results.push(imported_root_to_single_result(spec, &imported));

    write_shared_status(run_paths, status)
        .await
        .map_err(SubagentError::Spawn)?;

    Ok(())
}

/// Append a [`ChainAppendRequest`]'s steps to the in-loop `steps` list AND `status.steps`
/// (R-SA-095's "only then extend its own in-loop step list/`status.json`'s `steps`/
/// `chain_step_count`" — both updated together so they never observably diverge).
pub(super) fn append_steps(
    steps: &mut Vec<RunnerStep>,
    status: &mut RunStatus,
    request: &ChainAppendRequest,
) {
    for step in &request.steps {
        // SUBA-093: an appended step extends the FLAT list by its own width, and only at the tail,
        // so no already-published flat base is disturbed.
        status.steps.extend(pending_step_statuses_for(step));
        steps.push(step.clone());
    }
}

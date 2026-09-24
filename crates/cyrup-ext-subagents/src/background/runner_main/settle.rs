//! Per-step settlement and result shaping (SUBA-087/093): folding a dispatched step's outcome
//! into the status ledger and the [`SingleResult`] list, including child-scoped stop skips and
//! imported-async-root conversion. Split out of `background/runner_main.rs`; ports
//! pi `runs/background/subagent-runner.ts`.

use super::config::RunnerConfig;
use super::events::append_event;
use super::status::{
    lock_status, mark_remaining_paused, mark_remaining_timed_out,
    promote_interrupted_results_to_stopped, record_step_outcome, refresh_workflow_graph,
    step_display_agent, step_elapsed_ms, timeout_message, write_shared_status,
};
use super::turn_loop::{LoopOutcome, TurnLoopIo};
use crate::background::StepState;
use crate::background::cascade;
use crate::background::child_stop::{
    ChildStatusWord, ChildStopRecord, child_status_event, mark_child_stopped,
};
use crate::background::control;
use crate::background::flat_index::flat_total;
use crate::error::SubagentError;
use crate::exec::SingleResult;
use crate::jsonl::RunEventLog;
use crate::spawn::chain_graph::{ParallelGroupSpec, RunnerStep, StepResult};

/// What [`run_inner`](super::turn_loop::run_inner)'s loop does next once a dispatched step's outcome has been recorded.
pub(super) enum StepDisposition {
    /// Advance the step cursor and dispatch the next step.
    Advance,
    /// Re-enter the loop WITHOUT advancing the cursor, so a loop-top terminal-flag check produces
    /// the terminal record for the verb that actually tore this step's child down.
    Requeue,
    /// End the run with this terminal outcome.
    Finish(LoopOutcome),
}

/// Which control verb tore a step's child down mid-flight, in pi's precedence
/// (`subagent-runner.ts:4219-4222,4336` @v0.64.0: `stopped` → `timedOut` → `childStopRequests.has`
/// → `interrupted`). cyrup drives every verb off one interrupt token (a run-wide verb cancels the
/// parent, a child-scoped stop cancels the step's child token), so `step_result.interrupted` alone
/// cannot say which fired; the pending-request files and the child-stop registry can.
pub(super) enum MidFlightVerb {
    /// A whole-run `control/stop-requests/*` is pending — the loop-top stop branch owns the record.
    RunStop,
    /// A `control/timeout.json` is pending — the loop-top timeout branch owns the record.
    RunTimeout,
    /// SUBA-087 — a child-scoped stop was recorded against THIS step: it ends `Stopped` and the
    /// run continues (pi `childStopped`, `:4285-4342`).
    ChildStop(ChildStopRecord),
    /// A plain interrupt — the run pauses.
    Interrupt,
}

/// R-SA-084 mid-flight interrupt (`subagent-runner.ts:1583-1609`): a step whose child was
/// signalled and torn down mid-flight (the shared `interrupt_cancel` token this run threaded
/// into `RunOptions::interrupt` fired) is the pause point — the run ends `Paused`, never
/// `Complete`, even though an interrupted `run_sync` reports a paused-success (exit 0).
///
/// SUBA-087: unless the token that fired was this step's OWN child-scoped stop handle, in which
/// case the step ends [`StepState::Stopped`] with pi's stop message, `subagent.step.stopped` +
/// `subagent.child-status` are emitted, and the loop ADVANCES exactly as it does past a failed
/// step (pi `childStopped`, `subagent-runner.ts:4336-4342` @v0.64.0).
pub(super) async fn settle_step_result(
    io: &mut TurnLoopIo<'_>,
    steps: &[RunnerStep],
    flat_slots: std::ops::Range<usize>,
    step: RunnerStep,
    step_result: StepResult,
    group_results: Vec<crate::spawn::chain_graph::GroupStepResult>,
    results: &mut Vec<SingleResult>,
) -> Result<StepDisposition, SubagentError> {
    let config = io.config;
    let run_paths = io.run_paths;
    let status = io.status;
    let cursor = flat_slots.start;
    let flat_end = flat_slots.end;
    // SUBA-093 — a group settles per MEMBER; every other shape settles as one child. The group
    // aggregate carries no `interrupted` flag of its own (`chain_graph::collapse_fan_out`), so the
    // mid-flight disambiguation below is a non-group concern by construction.
    let is_group = matches!(
        step,
        RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_)
    );
    let flat_step_total = flat_total(steps);
    let interrupted_mid_flight = step_result.interrupted;
    // Disambiguate WHICH verb tore this child down, in pi's precedence. Both run-wide verbs are
    // checked against their FILE rather than their flag so a stale flag can never spin this
    // loop: only the loop-top branches' own consumption removes those files.
    //
    // G77: the STOP inbox is probed before the timeout inbox, for the identical reason and in
    // pi's identical order (`runs/background/control-channel.ts:653-655`). Returning `Interrupted`
    // for either would end an explicitly-stopped or timed-out run as a resumable `Paused`.
    let mid_flight = if interrupted_mid_flight {
        if control::check_stop_inbox_now(run_paths).await?.is_some() {
            Some(MidFlightVerb::RunStop)
        } else if control::check_timeout_inbox_now(run_paths).await?.is_some() {
            Some(MidFlightVerb::RunTimeout)
        } else if let Some(record) = io.flags.child_stops.recorded(cursor) {
            Some(MidFlightVerb::ChildStop(record))
        } else {
            Some(MidFlightVerb::Interrupt)
        }
    } else if !is_group && let Some(record) = io.flags.child_stops.recorded(cursor) {
        // SUBA-093 — the stop was applied at the dispatch GATE rather than mid-flight
        // (`ExecSingleStepExecutor::run_single`'s `is_requested` short-circuit, pi `:4937`), so
        // no child ever ran and nothing reports `interrupted`. The step is still `Stopped`.
        Some(MidFlightVerb::ChildStop(record))
    } else {
        None
    };
    let step_duration_ms;
    // SCOPE_17 — the per-MEMBER child run ids, captured under the SAME status lock as the
    // whole-step `child_run_id` below and addressed by the identical `flat_slots` offsets
    // `record_step_outcome`'s per-member arm uses. Captured here rather than re-locking at the
    // push site: the lock is released before the terminal push, and a second acquisition could
    // observe a status a concurrent control op had already moved on.
    let member_child_run_ids: Vec<Option<crate::background::RunId>>;
    // SCOPE_3d — the child's own run id, mirrored off the status step this settle just recorded
    // (`StepStatus::run_id`, pi `AsyncStatus.steps[].runId`) onto the terminal per-step
    // `SingleResult::child_run_id`, which is `WaitCompletionChild.runId`'s producer.
    let child_run_id;
    let mut child_stopped: Option<crate::background::child_stop::ChildStoppedSummary> = None;
    // SUBA-093 — per-MEMBER child stops settled inside a fan-out, each of which gets pi's two
    // terminal events of its own.
    let mut group_child_stops: Vec<(usize, crate::background::child_stop::ChildStoppedSummary)> =
        Vec::new();
    {
        let mut guard = lock_status(status);
        let s = &mut *guard;
        record_step_outcome(s, &flat_slots, &step, &step_result, group_results.first());
        // SUBA-093 — a `ParallelGroup` member whose OWN child-scoped stop fired is `Stopped`, not
        // the `Failed` its non-zero exit would otherwise make it (pi's per-member
        // `childStopped` settle, `subagent-runner.ts:4285-4342` @v0.64.0). Its siblings keep the
        // outcomes `record_step_outcome` just wrote, and the run stays alive.
        if is_group {
            for flat in flat_slots.clone() {
                if let Some(record) = io.flags.child_stops.recorded(flat)
                    && let Some(summary) =
                        mark_child_stopped(s, flat, Some(&record), crate::time::now_epoch_millis())
                {
                    group_child_stops.push((flat, summary));
                }
            }
        }
        match &mid_flight {
            Some(MidFlightVerb::ChildStop(record)) => {
                // pi `markChildStopped` (`subagent-runner.ts:2992-3010`): `record_step_outcome`
                // marked this step `Complete` (paused-success exits 0); it is `Stopped`.
                child_stopped =
                    mark_child_stopped(s, cursor, Some(record), crate::time::now_epoch_millis());
            }
            Some(_) => {
                // `record_step_outcome` marked this step `Complete` (paused-success exits 0);
                // override it (and every not-yet-run later step) to `Paused` per R-SA-084.
                if let Some(entry) = s.steps.get_mut(cursor) {
                    entry.status = StepState::Paused;
                    entry.error = None;
                }
                mark_remaining_paused(s, flat_end, flat_step_total);
            }
            None => {}
        }
        step_duration_ms = step_elapsed_ms(s, cursor);
        member_child_run_ids = flat_slots
            .clone()
            .map(|flat| s.steps.get(flat).and_then(|entry| entry.run_id.clone()))
            .collect();
        child_run_id = s.steps.get(cursor).and_then(|entry| entry.run_id.clone());
        refresh_workflow_graph(s, steps);
        s.touch();
    }
    for (flat, summary) in &group_child_stops {
        append_child_stopped_events(io.events, config, *flat, summary).await;
    }
    if let Some(summary) = &child_stopped {
        // pi `subagent-runner.ts:4335-4340`: `subagent.step.stopped` with `exitCode: 1`, then
        // `appendTerminalChildStatusEvent` → `subagent.child-status` `stopped`.
        append_child_stopped_events(io.events, config, cursor, summary).await;
        let mut single = step_result_to_single_result(&step, &step_result);
        single.child_run_id = child_run_id.clone();
        // The child's own record is `stopped` whether the stop tore it down mid-flight or was
        // applied at its dispatch gate (pi's `stoppedAfterAcceptance`, `:1642,1722`), so the
        // promotion below runs either way.
        single.interrupted = true;
        promote_interrupted_results_to_stopped(
            std::slice::from_mut(&mut single),
            control::STOP_MESSAGE,
        );
        results.push(single);
        write_shared_status(run_paths, status)
            .await
            .map_err(SubagentError::Spawn)?;
        return Ok(StepDisposition::Advance);
    }
    let events = &mut *io.events;
    let event_type = if interrupted_mid_flight {
        "subagent.step.paused"
    } else if step_result.success {
        "subagent.step.completed"
    } else {
        "subagent.step.failed"
    };
    append_event(
        events,
        event_type,
        Some(serde_json::json!({
            "runId": config.run_id.as_str(),
            "stepIndex": cursor,
            "agent": step_display_agent(&step),
            "exitCode": if interrupted_mid_flight { 0 } else { i32::from(!step_result.success) },
            "durationMs": step_duration_ms,
        })),
    )
    .await;
    // SCOPE_17 — pi flattens a fan-out into ONE `results[]` ENTRY PER CHILD, for both group kinds:
    // `subagent-runner.ts:4229-4230` (static parallel) and `:3768-3771` (dynamic). cyrup recorded a
    // single aggregate whose `final_output` is `None` by construction, so a parallel background
    // run's completion notification read `(no output)` and every child's usage, model, session file
    // and artifact trail was destroyed at this line.
    //
    // This also RESTORES `ResultFile::results`' own documented invariant — "the same fixed order as
    // `RunStatus::steps`". `flat_index::pending_step_statuses_for` seeds one status step PER MEMBER
    // for a `ParallelGroup`, so a 3-task fan-out has had 3 status steps and 1 result entry all
    // along.
    //
    // The traversal is deliberately identical to `record_step_outcome`'s per-member arm
    // (`status.rs`): `children.iter().enumerate()`, offset onto `flat_slots.start`. Two different
    // walks over the same positional array is how the status and the payload would drift.
    //
    // A `DynamicGroup` falls to the aggregate arm on purpose: it owns exactly ONE flat status slot
    // (its width is unknown until walk time), so flattening it would push `results` past
    // `status.steps` and break the same ordering contract in the opposite direction. Its aggregate
    // now carries the children's joined text — see `chain_graph::collapse_fan_out`.
    match (&step, group_results.first()) {
        (RunnerStep::ParallelGroup(group), Some(collapsed)) => {
            for (offset, child) in collapsed.children.iter().enumerate() {
                let identity = identity_for_member(group, offset);
                let mut single = match child {
                    Some(outcome) => step_result_to_single_result_with(identity, outcome),
                    None => skipped_member_result(
                        identity,
                        collapsed
                            .fail_fast_skipped
                            .get(offset)
                            .copied()
                            .unwrap_or(false),
                    ),
                };
                single.child_run_id = member_child_run_ids.get(offset).cloned().flatten();
                results.push(single);
            }
        }
        _ => {
            let mut single = step_result_to_single_result(&step, &step_result);
            single.child_run_id = child_run_id;
            results.push(single);
        }
    }

    write_shared_status(run_paths, status)
        .await
        .map_err(SubagentError::Spawn)?;

    match mid_flight {
        // Fall through to the top of the loop WITHOUT advancing the cursor and let the stop /
        // timeout branch there produce the terminal record (it re-marks this same step, since
        // `Paused` is not terminal).
        Some(MidFlightVerb::RunStop | MidFlightVerb::RunTimeout) => {
            return Ok(StepDisposition::Requeue);
        }
        Some(MidFlightVerb::Interrupt) => {
            // Consume the interrupt request file (idempotent) so it is not left dangling on the
            // run dir, then end the run `Paused` — the child was already torn down mid-flight.
            let _ = control::consume_interrupt_request(run_paths).await;
            cascade_to_descendants(io.roots, config, events, cascade::CascadeVerb::Interrupt).await;
            return Ok(StepDisposition::Finish(LoopOutcome::Interrupted {
                results: std::mem::take(results),
            }));
        }
        Some(MidFlightVerb::ChildStop(_)) | None => {}
    }

    // A step whose child was killed by the wall clock means the RUN-WIDE deadline
    // (`config.deadline_at_ms`, converted to `ctx.deadline_at` once above) has passed — it is
    // not a per-step budget, so every remaining step would be born already over its deadline.
    // pi ends the whole run here (`timeoutRunner` marks the run `failed`/`timedOut` and fails
    // every still-running-or-pending step, `subagent-runner.ts:2029-2062` @v0.34.0) rather than
    // marching the cursor through steps that cannot succeed. This is the ORIGIN of the timeout
    // cascade: the run whose own deadline expired is what turns a bounded background run into
    // a bounded background SUBTREE.
    if step_result.timed_out {
        let message = timeout_message(config.timeout_ms, "deadline");
        {
            let mut guard = lock_status(status);
            let s = &mut *guard;
            mark_remaining_timed_out(s, flat_end, flat_step_total, &message);
            refresh_workflow_graph(s, steps);
            s.touch();
        }
        write_shared_status(run_paths, status)
            .await
            .map_err(SubagentError::Spawn)?;
        cascade_to_descendants(io.roots, config, events, cascade::CascadeVerb::Timeout).await;
        return Ok(StepDisposition::Finish(LoopOutcome::TimedOut {
            results: std::mem::take(results),
            message,
        }));
    }

    Ok(StepDisposition::Advance)
}

/// SUBA-087 — pi's sequential-branch `childStopResult` (`subagent-runner.ts:3011-3014,4937-4941`
/// @v0.64.0): the step at `cursor` had a child-scoped stop queued against it before it was
/// dispatched, so it is marked `stopped` WITHOUT spawning a child, its stopped result is recorded
/// (`stoppedStepResult`, `:3182-3190`: output/error = the stop message, `exitCode: 1`, `stopped`),
/// and the events are appended.
pub(super) async fn skip_child_stopped_step(
    io: &mut TurnLoopIo<'_>,
    steps: &[RunnerStep],
    cursor: usize,
    step: &RunnerStep,
    record: &ChildStopRecord,
    results: &mut Vec<SingleResult>,
) -> Result<(), SubagentError> {
    let config = io.config;
    let run_paths = io.run_paths;
    let status = io.status;
    let (summary, child_run_id) = {
        let mut guard = lock_status(status);
        let s = &mut *guard;
        let summary = mark_child_stopped(s, cursor, Some(record), crate::time::now_epoch_millis());
        // SCOPE_3d — same mirror as `settle_step_result`: the stopped step's own child run id
        // (when the workflow runtime assigned one before the stop landed) still names the child.
        let child_run_id = s.steps.get(cursor).and_then(|entry| entry.run_id.clone());
        refresh_workflow_graph(s, steps);
        s.touch();
        (summary, child_run_id)
    };
    write_shared_status(run_paths, status)
        .await
        .map_err(SubagentError::Spawn)?;
    if let Some(summary) = &summary {
        append_child_stopped_events(io.events, config, cursor, summary).await;
    }
    let mut single = stopped_single_result(step);
    single.child_run_id = child_run_id;
    results.push(single);
    Ok(())
}

/// SUBA-093 — pi `childStopResult`/`stoppedStepResult` (`subagent-runner.ts:3011-3014,3182-3190`
/// @v0.64.0) as the [`StepResult`] a dispatch returns when a child-scoped stop was already queued
/// against its slot: the stop message as both output and error, exit code 1, nothing interrupted
/// (no child ever ran) and nothing timed out.
pub(super) fn child_stopped_step_result() -> StepResult {
    StepResult {
        execution: None,
        native_machine: None,
        runtime_acknowledged_extensions: None,
        success: false,
        structured_output: None,
        final_output: Some(control::STOP_MESSAGE.to_string()),
        error: Some(control::STOP_MESSAGE.to_string()),
        interrupted: false,
        control_events: Vec::new(),
        exit_code: Some(1),
        timed_out: false,
        context_overflow: false,
        saved_output_path: None,
        artifact_paths: None,
        // No child ever ran (the stop was queued before dispatch), so there is no usage, no
        // model, no transcript — and `Absent` is known, not unknown: a stopped step produced
        // nothing.
        usage: Default::default(),
        turns: 0,
        model: None,
        attempted_models: Vec::new(),
        session_file: None,
        output_state: crate::exec::output_state::SubagentOutputState::Absent,
        structured_output_path: None,
        // No child ever ran — there is no worktree evidence to summarize (upstream's
        // `childStopResult` carries no `timeoutRecovery` either, `subagent-runner.ts:3011-3014`).
        timeout_recovery: None,
        // No child ran, so no transcript writer ever existed.
        transcript_path: None,
        transcript_error: None,
        watchdog: None,
    }
}

/// pi `stoppedStepResult` (`subagent-runner.ts:3182-3190`) as a [`SingleResult`]: the stop message
/// as both output and error, `exitCode: 1`, `stopped: true`, nothing interrupted or timed out.
pub(super) fn stopped_single_result(step: &RunnerStep) -> SingleResult {
    let message = control::STOP_MESSAGE.to_string();
    let mut single = step_result_to_single_result(
        step,
        &StepResult {
            execution: None,
            native_machine: None,
            runtime_acknowledged_extensions: None,
            success: false,
            structured_output: None,
            final_output: Some(message.clone()),
            error: Some(message),
            interrupted: false,
            control_events: Vec::new(),
            exit_code: Some(1),
            timed_out: false,
            context_overflow: false,
            saved_output_path: None,
            artifact_paths: None,
            // A stopped step produced nothing: zero usage, no model, no transcript, and a
            // known-`Absent` output state (see `child_stopped_step_result`).
            usage: Default::default(),
            turns: 0,
            model: None,
            attempted_models: Vec::new(),
            session_file: None,
            output_state: crate::exec::output_state::SubagentOutputState::Absent,
            structured_output_path: None,
            // pi `stoppedStepResult` (`subagent-runner.ts:3182-3190`) carries no
            // `timeoutRecovery`: the stop was applied outside the child (or before it spawned),
            // so there is no run-scoped snapshot to measure against.
            timeout_recovery: None,
            // No child ran, so no transcript writer ever existed.
            transcript_path: None,
            transcript_error: None,
            watchdog: None,
        },
    );
    single.exit_code = 1;
    single.stopped = true;
    single
}

/// The two lines pi appends when a child-stopped step settles — `subagent.step.stopped`
/// (`subagent-runner.ts:3008`/`:4335-4339`: `exitCode: 1`, `durationMs`) and the terminal
/// `subagent.child-status` `stopped` (`appendTerminalChildStatusEvent`, `:2975-2978`).
pub(super) async fn append_child_stopped_events(
    events: &mut Option<RunEventLog>,
    config: &RunnerConfig,
    index: usize,
    summary: &crate::background::child_stop::ChildStoppedSummary,
) {
    append_event(
        events,
        "subagent.step.stopped",
        Some(serde_json::json!({
            "runId": config.run_id.as_str(),
            "stepIndex": index,
            "childId": summary.child_id,
            "agent": summary.agent,
            "exitCode": 1,
            "durationMs": summary.duration_ms,
        })),
    )
    .await;
    append_event(
        events,
        "subagent.child-status",
        Some(child_status_event(
            config.run_id.as_str(),
            index,
            &summary.child_id,
            &summary.agent,
            ChildStatusWord::Stopped,
            crate::time::now_epoch_millis(),
        )),
    )
    .await;
}

/// Deliver `verb` to every live nested async descendant of this run, logging each failed delivery
/// into this run's own `events.jsonl` under pi's `subagent.nested.{interrupt,timeout}_failed`
/// event types (`subagent-runner.ts:1539-1573` @v0.34.0).
///
/// A run with no `nested_route` has no descendants to reach and this is a no-op — that is the
/// common case (a leaf background run), so the cascade costs nothing on the path that does not
/// need it.
pub(super) async fn cascade_to_descendants(
    roots: &crate::paths::Roots,
    config: &RunnerConfig,
    events: &mut Option<RunEventLog>,
    verb: cascade::CascadeVerb,
) {
    let Some(route) = config.nested_route.as_ref() else {
        return;
    };
    // SUBA-115: a nested runner reaches only its own subtree; a root runner (no `nested_self`) the
    // whole route, as pi's `isNestedControlDescendant` decides.
    let issuer = config.nested_self.as_ref().map(|_| config.run_id.as_str());
    let report = cascade::cascade_to_nested_async_descendants(roots, route, issuer, verb).await;
    for failure in report.failures {
        let mut payload = serde_json::json!({
            "runId": config.run_id.as_str(),
            "message": failure.message,
        });
        if let (Some(target), Some(map)) = (failure.target_run_id, payload.as_object_mut()) {
            map.insert("targetRunId".to_string(), serde_json::Value::String(target));
        }
        append_event(events, verb.failure_event_type(), Some(payload)).await;
    }
}

/// Collapse one [`StepResult`] (this file's narrow, chain-graph-local result shape) into a full
/// [`SingleResult`] (func-SA §4.3's canonical per-run record, the shape [`ResultFile::results`](crate::background::ResultFile::results)
/// actually stores) — a group step's aggregate is likewise represented as one [`SingleResult`]
/// entry (per-child detail already folded into `status.parallel_groups` by
/// [`record_step_outcome`]; the terminal [`ResultFile`](crate::background::ResultFile) carries the same one-entry-per-top-level-
/// step shape `status.steps` does, not a flattened per-child list).
pub(super) fn step_result_to_single_result(step: &RunnerStep, result: &StepResult) -> SingleResult {
    step_result_to_single_result_with(identity_for_step(step), result)
}

/// The display identity a [`SingleResult`] is recorded under — the one thing that differs between
/// projecting a whole step and projecting ONE MEMBER of a fan-out (SCOPE_17).
///
/// A named pair rather than two `&str`/`String` parameters: `agent` and `task` are both strings,
/// so a transposed call site would compile happily and produce a payload naming the task as the
/// agent.
pub(super) struct ResultIdentity {
    /// The display name this result is attributed to.
    pub agent: String,
    /// The task prompt the step (or member) was given; empty where the shape has none.
    pub task: String,
}

/// The whole-step identity — the derivation [`step_result_to_single_result`] has always used,
/// lifted out verbatim so the per-member path can supply its own without duplicating the field
/// copying below.
fn identity_for_step(step: &RunnerStep) -> ResultIdentity {
    let agent = match step {
        RunnerStep::SingleStep(spec) => spec.agent.clone(),
        RunnerStep::ParallelGroup(group) => format!("<parallel:{} tasks>", group.steps.len()),
        RunnerStep::DynamicGroup(dynamic) => format!("<dynamic:{}>", dynamic.collect),
        // Never reached: `run_inner` intercepts `ImportAsyncRoot` and builds its `SingleResult`
        // directly via `imported_root_to_single_result` (the imported result's own agent, not this
        // step's display name). Kept for exhaustiveness only.
        RunnerStep::ImportAsyncRoot(spec) => spec.agent.clone(),
    };
    let task = match step {
        RunnerStep::SingleStep(spec) => spec.task.clone(),
        RunnerStep::ImportAsyncRoot(spec) => format!("Attach async root {}", spec.run_id),
        RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_) => String::new(),
    };
    ResultIdentity { agent, task }
}

/// SCOPE_17 — one member of a static fan-out, addressed positionally (R-SA-051 order).
///
/// `agent` comes from the member's own [`crate::spawn::chain_graph::SingleStepSpec`], which is
/// exactly what [`crate::background::flat_index::pending_step_statuses_for`] already seeds
/// `status.steps` from — so the payload entry and its status entry report the SAME agent by
/// construction rather than by coincidence.
fn identity_for_member(group: &ParallelGroupSpec, offset: usize) -> ResultIdentity {
    match group.steps.get(offset) {
        Some(spec) => ResultIdentity {
            agent: spec.agent.clone(),
            task: spec.task.clone(),
        },
        // Unreachable: `children` is pre-sized from `group.steps` by `run_bounded`. Degrade to a
        // positional label rather than dropping the member — a result that names itself oddly is
        // strictly better than a child whose output vanishes, which is the defect this fixes.
        None => ResultIdentity {
            agent: format!("<member {offset}>"),
            task: String::new(),
        },
    }
}

/// SCOPE_17 — pi's fail-fast skip record (`chain-execution.ts:321-330`): a real entry, not a hole.
///
/// [`crate::spawn::chain_graph::FAIL_FAST_SKIPPED_EXIT_CODE`] (`-1`) is upstream's own sentinel and
/// is deliberately NOT `1` — a reader must be able to tell "never ran" from "ran and failed". The
/// two skip REASONS are distinguished because pi distinguishes them
/// ([`crate::spawn::chain_graph::GroupStepResult::fail_fast_skipped`]): a fail-fast skip has an
/// upstream analogue, a cancellation skip has none.
///
/// Built from [`StepResult::failure`] rather than a hand-written [`SingleResult`] literal so every
/// other field takes the SAME defaults the real path uses — a future field added to `SingleResult`
/// cannot silently skip this arm.
fn skipped_member_result(identity: ResultIdentity, fail_fast: bool) -> SingleResult {
    let error = if fail_fast {
        crate::spawn::chain_graph::FAIL_FAST_SKIPPED_ERROR.to_string()
    } else {
        "Skipped (cancelled)".to_string()
    };
    let mut result = StepResult::failure(error);
    result.exit_code = i32::try_from(crate::spawn::chain_graph::FAIL_FAST_SKIPPED_EXIT_CODE).ok();
    step_result_to_single_result_with(identity, &result)
}

/// [`step_result_to_single_result`] with the identity supplied explicitly — the single
/// field-copying body both the whole-step and the per-member (SCOPE_17) projections share.
pub(super) fn step_result_to_single_result_with(
    identity: ResultIdentity,
    result: &StepResult,
) -> SingleResult {
    let ResultIdentity { agent, task } = identity;
    SingleResult {
        // SUBA-100 — the step's result carries both onto the terminal result payload
        // (`subagent-runner.ts:1579,928` @v0.68.0).
        execution: result.execution,
        native_machine: result.native_machine.clone(),
        // SUBA-063 — pi's step results carry `runtimeAcknowledgedExtensions:
        // r.runtimeAcknowledgedExtensions` onto the terminal result payload
        // (`subagent-runner.ts:5068` @v0.68.0).
        runtime_acknowledged_extensions: result.runtime_acknowledged_extensions.clone(),
        skills_warning: None,
        // UW-3 — pi's step results carry `watchdog: pr.watchdog` (`subagent-runner.ts:3565`).
        watchdog: result.watchdog.clone(),
        // SUBA-021: no usage budget on this path (see the field doc).
        usage_budget: None,
        turn_budget: None,
        turn_budget_exceeded: false,
        wrap_up_requested: false,
        agent,
        task,
        // The child's real code when the executor ran one; the success/failure mapping only as the
        // fallback for a step whose executor spawned nothing (mocks, and every group aggregate).
        // pi's async runner records the real code too (`subagent-runner.ts` stores the
        // `SingleResult` its step produced, exit code and all), so a `ResultFile` reader sees `2`
        // or `137` rather than a flattened `1`.
        exit_code: result
            .exit_code
            .unwrap_or_else(|| i32::from(!result.success)),
        // The telemetry `StepResult` now carries across the waist (SCOPE_3a): before these fields
        // existed on `StepResult`, every line below was a hard-coded
        // `Usage::default()`/`0`/`None`, so an async run's terminal `ResultFile` reported zero
        // tokens, zero cost, zero turns and no model for every child.
        usage: result.usage.clone(),
        turns: result.turns,
        model: result.model.clone(),
        attempted_models: result.attempted_models.clone(),
        model_attempts: Vec::new(),
        final_output: result.final_output.clone(),
        structured_output: result.structured_output.clone(),
        // SCOPE_3d — assigned by the settle callers (`settle_step_result`,
        // `skip_child_stopped_step`) from `StepStatus::run_id`; this projection has no status
        // access of its own.
        child_run_id: None,
        session_file: result.session_file.clone(),
        output_state: result.output_state,
        structured_output_path: result.structured_output_path.clone(),
        artifact_paths: result.artifact_paths.clone(),
        // pi `transcriptPath` / `transcriptError` on the runner's results copy
        // (`subagent-runner.ts:1590-1591`): the live transcript's path and its writer's error,
        // carried onto the terminal `ResultFile` so a `wait`/`collect` reader can open the file.
        transcript_path: result.transcript_path.clone(),
        transcript_error: result.transcript_error.clone(),
        acceptance: None,
        detached: false,
        detached_reason: None,
        // R-SA-084: carry the mid-flight interrupt flag through to the terminal per-step
        // `SingleResult` (pi's `interrupted` field), so a `ResultFile` reader sees which step was
        // the pause point rather than a hard-coded `false`.
        interrupted: result.interrupted,
        // Same rationale as `interrupted` above, one field over: a deadline kill is now visible on
        // the terminal per-step `SingleResult` instead of being flattened into an anonymous
        // non-zero exit.
        timed_out: result.timed_out,
        // SUBA-3c — same rationale one field over: pi's terminal chain results carry the step's
        // own `timeoutRecovery` (`subagent-runner.ts:4946`), and this projection is the only path
        // onto an async run's `ResultFile`; dropping it here is exactly how the evidence used to
        // die at the waist.
        timeout_recovery: result.timeout_recovery.clone(),
        // Same rationale again: pi's terminal result carries the step's own `contextOverflow`
        // (`subagent-runner.ts:4486`), and this projection is the only path onto an async run's
        // `ResultFile`.
        context_overflow: result.context_overflow,
        stopped: false,
        process_signal: None,
        error: result.error.clone(),
        saved_output_path: result.saved_output_path.clone(),
        tool_calls: Vec::new(),
        // [`crate::spawn::chain_graph::StepResult`] is the chain walker's own compact projection and
        // carries no tool surface, so a step projected back into a `SingleResult` has none either.
        // Deliberately NOT re-resolved from the agent here: this function is pure and has neither
        // the persona nor the run options, and a surface guessed from a name is exactly the kind of
        // unverified claim `tool_surface` exists to replace.
        tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
        output_truncated: false,
        // SUBA-N05: the step's raised control events, carried through `StepResult` rather than
        // dropped — this is the only channel by which an ASYNC run's control events reach the
        // orchestrator, which reads them off the terminal `ResultFile`.
        control_events: result.control_events.clone(),
        progress: None,
        // SUBA-074: `StepResult` carries no external-runner receipt, so an ASYNC external run's
        // `runner`/`externalProcess` do not survive this projection. Recorded as a residual on the
        // item rather than papered over — the run itself executes identically on both paths; only
        // the receipt fields are absent from the async `ResultFile`.
        runner: None,
        external_process: None,
    }
}

/// Collapse one [`control::ImportedAsyncRootResult`] (the product of polling an attached async root
/// to a terminal state, R-SA-097) into the [`SingleResult`] this chain records for its synthesized
/// first step. Unlike [`step_result_to_single_result`], the agent/model/attempted-models here come
/// from the IMPORTED result (the target child's own identity), not the `ImportAsyncRoot` step's
/// display spec — matching pi's `runSingleStep` returning `imported.agent`/`imported.model`/… rather
/// than the step's declared values (`subagent-runner.ts:1162-1181`).
pub(super) fn imported_root_to_single_result(
    spec: &crate::spawn::chain_graph::ImportAsyncRootSpec,
    imported: &control::ImportedAsyncRootResult,
) -> SingleResult {
    SingleResult {
        execution: None,
        native_machine: None,
        runtime_acknowledged_extensions: None,
        skills_warning: None,
        watchdog: None,
        // SUBA-021: no usage budget on this path (see the field doc).
        usage_budget: None,
        turn_budget: None,
        turn_budget_exceeded: false,
        wrap_up_requested: false,
        agent: imported.agent.clone(),
        task: format!("Attach async root {}", spec.run_id),
        exit_code: imported.exit_code,
        // Deliberately zero, and correct — NOT the projection loss `step_result_to_single_result`
        // used to have: an imported async root's usage (and turn count) lives on its OWN terminal
        // `ResultFile`, and double-counting it into the importing chain would inflate the parent.
        usage: cyrup_core::Usage::default(),
        turns: 0,
        model: imported.model.clone(),
        attempted_models: imported.attempted_models.clone(),
        model_attempts: Vec::new(),
        final_output: Some(imported.output.clone()),
        structured_output: imported.structured_output.clone(),
        // The target child's own transcript — `ImportedAsyncRootResult` has carried it all along
        // (`control.rs`); it simply died at this projection.
        session_file: imported.session_file.clone(),
        // An imported async ROOT's identity is the run-level id of its OWN `ResultFile`
        // (`spec.run_id` names the attach target, not a child this chain launched), and
        // `StepStatus::run_id` is never populated for an `ImportAsyncRoot` step.
        child_run_id: None,
        output_state: crate::exec::output_state::derive_output_state(
            Some(imported.output.as_str()),
            imported.structured_output.as_ref(),
            None,
        ),
        structured_output_path: None,
        artifact_paths: None,
        transcript_path: None,
        transcript_error: None,
        acceptance: None,
        detached: false,
        detached_reason: None,
        interrupted: false,
        timed_out: false,
        // pi's import copy (`subagent-runner.ts:760-790`) carries `sessionFile`/`contextOverflow`
        // and NOT `timeoutRecovery`: the target run's recovery evidence lives on its OWN terminal
        // `ResultFile`, same reasoning as `usage` above.
        timeout_recovery: None,
        // pi `contextOverflow: imported.contextOverflow` (`subagent-runner.ts:767`) — the target
        // child's own terminal classification, re-published by the importing chain.
        context_overflow: imported.context_overflow,
        stopped: false,
        process_signal: None,
        error: imported.error.clone(),
        saved_output_path: None,
        tool_calls: Vec::new(),
        // R-SA-097 chain-root attachment POLLS an already-launched async run and never spawns a
        // child of its own, so this process planned no surface.
        tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
        output_truncated: false,
        control_events: Vec::new(),
        progress: None,
        // An imported async ROOT is a cyrup run, never a foreign process.
        runner: None,
        external_process: None,
    }
}

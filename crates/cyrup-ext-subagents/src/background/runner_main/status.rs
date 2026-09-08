//! The live [`RunStatus`] ledger: the shared handle both the step loop and the telemetry pump
//! mutate (pi `subagent-runner.ts:1430-1581`), its atomic publication, and every status-ledger
//! mutation (mark/record/refresh helpers). Split out of `background/runner_main.rs`; ports
//! pi `runs/background/subagent-runner.ts`.

use crate::background::{ParallelGroupStatus, RunPaths, RunStatus, StepState, StepStatus};
use crate::background::atomic::write_atomic_json;
use crate::exec::SingleResult;
use crate::spawn::chain_graph::{RunnerStep, StepResult};
use std::sync::Arc;


/// The agent name shown for one [`RunnerStep`] in a `subagent.step.*` `events.jsonl` line — the
/// step's own agent for a single/import step, or a synthesized group label.
///
/// The events are one-per-TOP-LEVEL-step, so the group label survives here. `RunStatus::steps` is
/// the other index space and its entries are one-per-FLAT-CHILD, which is why
/// [`crate::background::flat_index::pending_step_statuses_for`] names a group's members
/// individually instead. (SUBA-093 deleted `pending_step_status_for`, the collapsed builder this
/// comment used to point at.)
pub(super) fn step_display_agent(step: &RunnerStep) -> String {
    match step {
        RunnerStep::SingleStep(spec) => spec.agent.clone(),
        RunnerStep::ImportAsyncRoot(spec) => spec.agent.clone(),
        RunnerStep::ParallelGroup(group) => format!("<parallel:{} tasks>", group.steps.len()),
        RunnerStep::DynamicGroup(dynamic) => format!("<dynamic:{}>", dynamic.collect),
    }
}

/// The elapsed wall-clock milliseconds of the step at `flat_index`, from its recorded
/// `started_at`/`ended_at` (pi's `taskEndTime - taskStartTime` on a `subagent.step.*` event).
/// `0` when either timestamp is missing.
pub(super) fn step_elapsed_ms(status: &RunStatus, flat_index: usize) -> i64 {
    status
        .steps
        .get(flat_index)
        .and_then(|s| s.started_at.zip(s.ended_at))
        .map(|(start, end)| (end - start).max(0))
        .unwrap_or(0)
}

/// Recompute + embed this run's workflow-graph snapshot (pi's `refreshWorkflowGraph`,
/// `subagent-runner.ts:2160-2192`) from the current step list + live per-step statuses, so any
/// `status.json` reader always sees a graph consistent with the run's current progress.
pub(super) fn refresh_workflow_graph(status: &mut RunStatus, steps: &[RunnerStep]) {
    let graph = crate::background::workflow_graph_from_run(steps, status);
    status.telemetry.workflow_graph = Some(graph);
}

// =================================================================================================
// Shared status handle + live-telemetry pump (pi `subagent-runner.ts:1430-1581`)
// =================================================================================================

/// The one [`RunStatus`] both the step loop ([`run_inner`](super::turn_loop::run_inner), lifecycle transitions) and the live
/// telemetry task ([`spawn_telemetry_task`], per-child-event folds) mutate — a plain
/// `std::sync::Mutex` written atomically to `status.json` via [`write_shared_status`]. Every
/// critical section against it is a short, synchronous read-modify-write with no `.await` held
/// across the guard (the atomic file write clones under the lock, then writes with the lock
/// released), mirroring `background/tracker.rs`'s identical `std::sync::Mutex` discipline.
pub(super) type SharedStatus = Arc<std::sync::Mutex<RunStatus>>;

/// One raw child NDJSON line, tagged with the flat step index it belongs to, sent from a dispatched
/// step's [`crate::exec::RunOptions::live_events`] sink to the runner's telemetry task.
pub(crate) struct TelemetryMsg {
    /// The flat index of the step whose child produced this line.
    pub(super) flat_index: usize,
    /// The raw NDJSON line, exactly as read from the child's stdout.
    pub(super) raw: String,
}

/// Lock the shared status, recovering the guard on a poisoned mutex rather than propagating the
/// panic (the map's contents stay structurally valid), matching `background/tracker.rs`.
pub(super) fn lock_status(shared: &SharedStatus) -> std::sync::MutexGuard<'_, RunStatus> {
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Atomically write the current shared status to `status.json` (R-SA-076): clone under the lock,
/// then write with the lock RELEASED so no `std::sync::Mutex` guard is ever held across the `.await`.
pub(super) async fn write_shared_status(run_paths: &RunPaths, shared: &SharedStatus) -> std::io::Result<()> {
    let snapshot = lock_status(shared).clone();
    write_atomic_json(&run_paths.status, &snapshot).await
}

/// Spawn the live-telemetry pump (pi's `updateStepFromChildEvent` per-event fold +
/// `activityTimer`'s 1s cadence, `subagent-runner.ts:1430-1581`): drains raw child NDJSON lines off
/// `rx`, parses each into a [`crate::exec::ndjson::SubagentEvent`], folds it into the addressed
/// step's live [`crate::background::StepTelemetry`] plus the top-level roll-ups, and writes
/// `status.json` — both per event AND on a 1s timer — so a reader watching the file sees
/// `currentTool`/`recentTools`/token telemetry advance live during the run. The task ends when every
/// telemetry sender is dropped (the step loop finished and released the executor), which the caller
/// awaits BEFORE writing the terminal record so no late telemetry write races the terminal
/// `status.json`.
pub(super) fn spawn_telemetry_task(
    run_paths: RunPaths,
    shared: SharedStatus,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<TelemetryMsg>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                message = rx.recv() => {
                    let Some(TelemetryMsg { flat_index, raw }) = message else {
                        break; // every sender dropped — the run's step loop has finished
                    };
                    let Some(event) = crate::exec::ndjson::parse_line(&raw) else {
                        continue; // R-SA-026: a non-event line is tolerated, never fatal
                    };
                    {
                        let mut status = lock_status(&shared);
                        let now = crate::time::now_epoch_millis();
                        if let Some(step) = status.steps.get_mut(flat_index) {
                            crate::background::apply_child_event_to_step(step, &event, now);
                        }
                        status.telemetry.last_activity_at = Some(now);
                        status.sync_top_level_telemetry(flat_index);
                    }
                    let _ = write_shared_status(&run_paths, &shared).await;
                }
                _ = ticker.tick() => {
                    // pi's 1s `activityTimer` cadence: re-flush the current live status so a reader
                    // sees a fresh `lastUpdate` even during a quiet stretch between child events.
                    {
                        let mut status = lock_status(&shared);
                        status.touch();
                    }
                    let _ = write_shared_status(&run_paths, &shared).await;
                }
            }
        }
    })
}

/// Mark every step from `from_index` (inclusive) through `total` as `Paused` with an end
/// timestamp (R-SA-084: "mark every currently-running step Paused... with an end timestamp"),
/// including the step at `from_index` itself (the one that was `Running` — or about to be — at
/// the moment the interrupt was observed) — steps strictly before `from_index` are left however
/// [`record_step_outcome`] already left them (their own genuine terminal/paused state from having
/// actually run), and steps at or after `from_index` that were never even started are likewise
/// moved out of `Pending` into `Paused` rather than left looking like they simply never got a
/// turn, since R-SA-084 does not distinguish "was mid-flight" from "was about to start" for the
/// purpose of this marking.
/// pi's `timeoutMessage` (`subagent-runner.ts:1339` @v0.34.0): `Subagent timed out after
/// ${config.timeoutMs}ms.`, falling back to the bare `"Subagent timed out."` upstream's
/// `timeoutRunner` uses when no nominal budget is known — which is exactly the ancestor-cascade
/// case, where the deadline that expired belonged to a different run and this one has no
/// `timeout_ms` of its own to name.
pub(super) fn timeout_message(timeout_ms: Option<u64>, source: &str) -> String {
    match timeout_ms {
        Some(ms) => format!("Subagent timed out after {ms}ms."),
        None if source == "ancestor-timeout" => {
            "Subagent timed out: an ancestor run's deadline expired.".to_string()
        }
        None => "Subagent timed out.".to_string(),
    }
}

/// The timeout counterpart of [`mark_remaining_paused`] (pi `timeoutRunner`'s step sweep,
/// `subagent-runner.ts:2029-2067` @v0.34.0): every step from `from_index` that is not already
/// terminal becomes `Failed` with the timeout `message` and an end timestamp.
///
/// `Failed`, not `Paused`, is the whole point — see [`LoopOutcome::TimedOut`](super::turn_loop::LoopOutcome::TimedOut). A reader must be
/// able to tell "this run stopped and can be resumed" from "this run ran out of time and is over".
pub(super) fn mark_remaining_timed_out(
    status: &mut RunStatus,
    from_index: usize,
    total: usize,
    message: &str,
) {
    let now = crate::time::now_epoch_millis();
    for index in from_index..total {
        if let Some(step) = status.steps.get_mut(index)
            && !step.status.is_terminal()
        {
            step.status = StepState::Failed;
            step.error = Some(message.to_string());
            step.ended_at.get_or_insert(now);
        }
    }
    if let Some(groups) = &mut status.parallel_groups {
        for group in groups {
            if group.group_step_index >= from_index {
                for child in &mut group.children {
                    if !child.status.is_terminal() {
                        child.status = StepState::Failed;
                        child.error = Some(message.to_string());
                        child.ended_at.get_or_insert(now);
                    }
                }
            }
        }
    }
}

/// G77 — re-label the child that was torn down BY the stop as `stopped` rather than `interrupted`.
///
/// cyrup drives a step's live child off one shared [`cyrup_core::CancelToken`] for every control
/// verb, so a child killed by a stop comes back from `run_sync` carrying `interrupted: true` — the
/// same shape an actual `interrupt` produces. pi does not have that ambiguity because it hands the
/// child two separate abort controllers, and it resolves the child's own record against the STOP
/// signal specifically: `const stoppedAfterAcceptance = finalResult?.stopped === true ||
/// ctx.stopSignal?.aborted === true;` … `stopped: stoppedAfterAcceptance ? true :
/// finalResult?.stopped` (`subagent-runner.ts:1642,1722` @v0.43.0). This function is that same
/// promotion, applied at the one place cyrup knows the stop signal is what fired.
///
/// The rest of each promoted field follows `runSubagent`'s own stopped-result shape
/// (`subagent-runner.ts:1937-4576` @v0.43.0): `exitCode: 1`, `error: stopMessage`, and — only when the
/// child produced no output of its own — `finalOutput: stopMessage`. Children that had ALREADY
/// completed before the stop landed are untouched, exactly as upstream leaves them (their records
/// settled while `stopSignal.aborted` was still false).
pub(super) fn promote_interrupted_results_to_stopped(results: &mut [SingleResult], message: &str) {
    for result in results.iter_mut().filter(|r| r.interrupted) {
        result.interrupted = false;
        result.stopped = true;
        result.exit_code = 1;
        result.error = Some(message.to_string());
        if result
            .final_output
            .as_deref()
            .is_none_or(|text| text.trim().is_empty())
        {
            result.final_output = Some(message.to_string());
        }
    }
}

/// G77 — the STOP counterpart of [`mark_remaining_timed_out`]/[`mark_remaining_paused`], ported
/// from pi `stopRunner`'s own step sweep (`subagent-runner.ts:2955-2986` @v0.43.0):
///
/// ```text
/// for (const step of statusPayload.steps) {
///     if (step.status !== "running" && step.status !== "pending") continue;
///     step.status = "stopped";
///     step.error = stopMessage;
///     step.exitCode = 1;
///     step.stopped = true;
///     …
/// }
/// ```
///
/// Three differences from the timeout sweep, all upstream's:
/// * the terminal step status is [`StepState::Stopped`], never `Failed` — a reader must be able to
///   tell "someone stopped this" from "this crashed";
/// * upstream sweeps EVERY `running`-or-`pending` step in the payload, not only those from the
///   cursor onward — which is the same set here, since a step before the cursor is already
///   terminal and `is_terminal()` skips it either way;
/// * the message is the fixed [`control::STOP_MESSAGE`](crate::background::control::STOP_MESSAGE), not a computed one.
pub(super) fn mark_remaining_stopped(status: &mut RunStatus, from_index: usize, total: usize, message: &str) {
    let now = crate::time::now_epoch_millis();
    for index in from_index..total {
        if let Some(step) = status.steps.get_mut(index)
            && !step.status.is_terminal()
        {
            step.status = StepState::Stopped;
            step.error = Some(message.to_string());
            // SUBA-087 — pi `step.stopped = true` (`subagent-runner.ts:3842` @v0.64.0).
            step.stopped = true;
            step.ended_at.get_or_insert(now);
        }
    }
    if let Some(groups) = &mut status.parallel_groups {
        for group in groups {
            if group.group_step_index >= from_index {
                for child in &mut group.children {
                    if !child.status.is_terminal() {
                        child.status = StepState::Stopped;
                        child.error = Some(message.to_string());
                        child.stopped = true;
                        child.ended_at.get_or_insert(now);
                    }
                }
            }
        }
    }
}

pub(super) fn mark_remaining_paused(status: &mut RunStatus, from_index: usize, total: usize) {
    let now = crate::time::now_epoch_millis();
    for index in from_index..total {
        if let Some(step) = status.steps.get_mut(index)
            && !step.status.is_terminal()
        {
            step.status = StepState::Paused;
            step.ended_at.get_or_insert(now);
        }
    }
    if let Some(groups) = &mut status.parallel_groups {
        for group in groups {
            if group.group_step_index >= from_index {
                for child in &mut group.children {
                    if !child.status.is_terminal() {
                        child.status = StepState::Paused;
                        child.ended_at.get_or_insert(now);
                    }
                }
            }
        }
    }
}

pub(super) fn mark_step_running(status: &mut RunStatus, index: usize) {
    if let Some(step) = status.steps.get_mut(index) {
        step.status = StepState::Running;
        step.started_at
            .get_or_insert(crate::time::now_epoch_millis());
    }
}

/// Fold one completed step's [`StepResult`] (and, for a group step, its
/// [`crate::spawn::chain_graph::GroupStepResult`]'s own per-child detail) back into the flat
/// `status.steps` slots `slots` names, plus `status.parallel_groups`.
///
/// SUBA-093 — a [`RunnerStep::ParallelGroup`] owns one flat slot PER MEMBER, so each member's own
/// outcome lands on its own entry (pi's per-member settle, `subagent-runner.ts:4286-4295`
/// @v0.64.0) instead of every member collapsing onto the group's single entry. A member with no
/// result at all (fail-fast-skipped or cancelled) is `Failed` with the same sentence the aggregate
/// error counts it under. Every other shape — including an un-spliced
/// [`RunnerStep::DynamicGroup`], whose members share one slot (a recorded SUBA-093 residual) —
/// records the aggregate on its single entry, exactly as before.
pub(super) fn record_step_outcome(
    status: &mut RunStatus,
    slots: &std::ops::Range<usize>,
    step: &RunnerStep,
    result: &StepResult,
    group_result: Option<&crate::spawn::chain_graph::GroupStepResult>,
) {
    let now = crate::time::now_epoch_millis();
    let index = slots.start;
    let per_member = match (step, group_result) {
        (RunnerStep::ParallelGroup(_), Some(group)) if slots.len() > 1 => Some(group),
        _ => None,
    };
    match per_member {
        Some(group) => {
            for (offset, child) in group.children.iter().enumerate() {
                let Some(entry) = status.steps.get_mut(index + offset) else {
                    continue;
                };
                if entry.status.is_terminal() {
                    continue;
                }
                entry.ended_at = Some(now);
                match child {
                    Some(outcome) => {
                        entry.status = if outcome.success {
                            StepState::Complete
                        } else {
                            StepState::Failed
                        };
                        entry.error = outcome.error.clone();
                        // The member's real telemetry (SCOPE_3a): pi's per-member settle writes
                        // the child's own usage/model/session onto its step entry
                        // (`subagent-runner.ts:4286-4295`); before `StepResult` carried these,
                        // every status reader — including `/subagent-cost` — saw zeros here.
                        entry.usage = outcome.usage.clone();
                        entry.turns = outcome.turns;
                        entry.model = outcome.model.clone();
                        entry.attempted_models = outcome.attempted_models.clone();
                        entry.session_file = outcome.session_file.clone();
                        // pi `setOptionalProperty(requiredStatusStep(…), "contextOverflow",
                        // singleResult.contextOverflow)` (`subagent-runner.ts:4083`).
                        entry.context_overflow = outcome.context_overflow;
                        // pi `setOptionalProperty(requiredStatusStep(…), "timeoutRecovery",
                        // singleResult.timeoutRecovery)` (`subagent-runner.ts:4165`).
                        entry.timeout_recovery = outcome.timeout_recovery.clone();
                    }
                    None => {
                        entry.status = StepState::Failed;
                        entry.error = Some("skipped (fail-fast or cancellation)".to_string());
                    }
                }
            }
        }
        // A zero-width group owns NO flat slot, so `slots.start` is the next step's entry rather
        // than one of its own: writing this step's outcome there would settle a step that has not
        // run. No constructor produces a zero-task `ParallelGroup` today (`flat_step_width`'s own
        // doc says a zero-width group contributes nothing), so this is a guard, not a live branch.
        None if !slots.is_empty() => {
            if let Some(entry) = status.steps.get_mut(index) {
                entry.status = if result.success {
                    StepState::Complete
                } else {
                    StepState::Failed
                };
                entry.ended_at = Some(now);
                entry.error = result.error.clone();
                // Same six lines as the per-member arm above — the aggregate/single-slot shape
                // records its own step's real telemetry rather than leaving the
                // `StepStatus::pending` zeros in place.
                entry.usage = result.usage.clone();
                entry.turns = result.turns;
                entry.model = result.model.clone();
                entry.attempted_models = result.attempted_models.clone();
                entry.session_file = result.session_file.clone();
                // pi `subagent-runner.ts:3668`/`:4570` — the single-step and chain-step status
                // writes of `contextOverflow`.
                entry.context_overflow = result.context_overflow;
                // pi `subagent-runner.ts:3750`/`:4645` — the single-step and chain-step status
                // writes of `timeoutRecovery` (the FULL summary; a status step is local state).
                entry.timeout_recovery = result.timeout_recovery.clone();
            }
        }
        None => {}
    }

    if let (RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_), Some(group)) =
        (step, group_result)
    {
        let children: Vec<StepStatus> = group
            .children
            .iter()
            .map(|child| {
                let mut s = StepStatus::pending("<group-child>");
                s.started_at = Some(now);
                s.ended_at = Some(now);
                match child {
                    Some(outcome) => {
                        s.status = if outcome.success {
                            StepState::Complete
                        } else {
                            StepState::Failed
                        };
                        s.error = outcome.error.clone();
                        // The same six fields the per-member arm writes onto `status.steps` —
                        // without them a `parallel_groups` reader sees zeros while `steps` sees
                        // the truth.
                        s.usage = outcome.usage.clone();
                        s.turns = outcome.turns;
                        s.model = outcome.model.clone();
                        s.attempted_models = outcome.attempted_models.clone();
                        s.session_file = outcome.session_file.clone();
                        s.context_overflow = outcome.context_overflow;
                        s.timeout_recovery = outcome.timeout_recovery.clone();
                    }
                    None => {
                        s.status = StepState::Failed;
                        s.error = Some("skipped (fail-fast or cancellation)".to_string());
                    }
                }
                s
            })
            .collect();
        let entry = ParallelGroupStatus {
            group_step_index: index,
            children,
        };
        status
            .parallel_groups
            .get_or_insert_with(Vec::new)
            .push(entry);
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
    use super::super::tests::single_step;
    use crate::background::control;
    use crate::background::flat_index::{flat_base, flat_range, flat_total, pending_step_statuses_for};
    use crate::background::{RunId, RunMode, RunState};


    /// SUBA-093 — a `ParallelGroup`'s per-member outcomes land on the members' OWN flat status
    /// entries, not collapsed onto one entry for the whole group (pi's per-member settle,
    /// `subagent-runner.ts:4286-4295` @v0.64.0). This is what gives a `tasks[]` fan-out a live
    /// per-child status at all, and therefore what a `childId` resolves against.
    ///
    /// Fails at the parent commit by construction: `record_step_outcome` took a single `usize`
    /// there and `status.steps` carried one `<parallel:3 tasks>` entry for the whole group.
    #[test]
    fn a_parallel_groups_member_outcomes_land_on_their_own_flat_status_entries() {
        let mut status = RunStatus::queued(
            RunId::from_token("flatgroup001".to_string()),
            RunMode::Parallel,
            Some(1),
        );
        status.state = RunState::Running;
        let group_step = RunnerStep::ParallelGroup(crate::spawn::chain_graph::ParallelGroupSpec {
            steps: vec![
                single_step("alpha", "a"),
                single_step("beta", "b"),
                single_step("gamma", "c"),
            ],
            concurrency: 3,
            fail_fast: false,
            worktree: false,
        });
        status.steps = crate::background::flat_index::pending_step_statuses_for(&group_step);
        assert_eq!(
            status
                .steps
                .iter()
                .map(|s| s.agent.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta", "gamma"],
            "the declaration itself is per member"
        );

        let group_result = crate::spawn::chain_graph::GroupStepResult {
            aggregate: StepResult::failure("1 of 3 group step(s) failed or were skipped"),
            children: vec![
                Some(StepResult::success(Some("out-a".to_string()), None)),
                Some(StepResult::failure("beta blew up")),
                Some(StepResult::success(Some("out-c".to_string()), None)),
            ],
            fail_fast_skipped: vec![false, false, false],
        };
        let aggregate = group_result.aggregate.clone();
        record_step_outcome(
            &mut status,
            &(0..3),
            &group_step,
            &aggregate,
            Some(&group_result),
        );

        assert_eq!(status.steps[0].status, StepState::Complete);
        assert_eq!(
            status.steps[1].status,
            StepState::Failed,
            "only the member that failed is Failed: {:?}",
            status.steps
        );
        assert_eq!(status.steps[1].error.as_deref(), Some("beta blew up"));
        assert_eq!(status.steps[2].status, StepState::Complete);
        assert!(
            status.steps[0].error.is_none() && status.steps[2].error.is_none(),
            "a sibling never picks up the aggregate's own error text"
        );
        // The settled-detail record is still written, now keyed by the group's FLAT base.
        let groups = status.parallel_groups.as_ref().expect("group recorded");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_step_index, 0);
        assert_eq!(groups[0].children.len(), 3);
    }

    /// SUBA-3c — the OTHER two `record_step_outcome` arms carry `timeoutRecovery` as well: the
    /// per-member arm writes it onto each member's flat `steps` entry, and the group-children
    /// sweep writes it onto `parallel_groups[..].children`. Missing the latter reproduces exactly
    /// the bug that arm's own comment records: "a `parallel_groups` reader sees zeros while
    /// `steps` sees the truth".
    #[test]
    fn a_members_timeout_recovery_lands_on_both_the_flat_entry_and_the_group_children() {
        let mut status = RunStatus::queued(
            RunId::from_token("flatgrouprec1".to_string()),
            RunMode::Parallel,
            Some(1),
        );
        status.state = RunState::Running;
        let group_step = RunnerStep::ParallelGroup(crate::spawn::chain_graph::ParallelGroupSpec {
            steps: vec![single_step("alpha", "a"), single_step("beta", "b")],
            concurrency: 2,
            fail_fast: false,
            worktree: false,
        });
        status.steps = crate::background::flat_index::pending_step_statuses_for(&group_step);

        let evidence = crate::exec::mutation_evidence::TrackedMutationEvidence {
            source: Default::default(),
            tracked_only: true,
            changed_files: vec!["src/half-written.rs".to_string()],
            attempted_mutation: true,
            truncated: false,
            unavailable: None,
        };
        let summary = crate::exec::mutation_evidence::build_timeout_recovery_summary(
            crate::exec::mutation_evidence::TimeoutRecoveryInput {
                termination: crate::exec::mutation_evidence::Termination::TimedOut,
                evidence: &evidence,
                required_output_missing: Some(true),
                current_tool: None,
                current_tool_args: None,
                current_path: None,
                session_file: None,
                transcript_path: None,
                artifact_paths: None,
            },
        );
        let mut timed_out_member = StepResult::failure("deadline");
        timed_out_member.timed_out = true;
        timed_out_member.timeout_recovery = Some(summary.clone());

        let group_result = crate::spawn::chain_graph::GroupStepResult {
            aggregate: StepResult::failure("1 of 2 group step(s) failed or were skipped"),
            children: vec![
                Some(StepResult::success(Some("out-a".to_string()), None)),
                Some(timed_out_member),
            ],
            fail_fast_skipped: vec![false, false],
        };
        let aggregate = group_result.aggregate.clone();
        record_step_outcome(&mut status, &(0..2), &group_step, &aggregate, Some(&group_result));

        // Per-member arm (pi `subagent-runner.ts:4165`).
        assert_eq!(status.steps[0].timeout_recovery, None);
        assert_eq!(status.steps[1].timeout_recovery.as_ref(), Some(&summary));
        // Group-children sweep — the `parallel_groups` reader sees the same truth.
        let groups = status.parallel_groups.as_ref().expect("group recorded");
        assert_eq!(groups[0].children[0].timeout_recovery, None);
        assert_eq!(groups[0].children[1].timeout_recovery.as_ref(), Some(&summary));
    }

    /// SUBA-093 review fix — a step that owns NO flat slot settles nothing.
    ///
    /// `flat_range` returns `base..base` for a zero-width `ParallelGroup`, whose `slots.start` is
    /// the NEXT step's status entry. Before the guard, `record_step_outcome` took the non-group
    /// arm (`per_member` needs `slots.len() > 1`) and wrote the empty group's aggregate over a
    /// step that had not run. No constructor produces a zero-task group today, which is exactly
    /// why the invariant needs pinning rather than trusting.
    #[test]
    fn a_zero_width_group_settles_nothing_and_leaves_the_next_step_pending() {
        let steps = vec![
            RunnerStep::ParallelGroup(crate::spawn::chain_graph::ParallelGroupSpec {
                steps: Vec::new(),
                concurrency: 2,
                fail_fast: false,
                worktree: false,
            }),
            RunnerStep::SingleStep(single_step("tail", "t")),
        ];
        assert_eq!(flat_total(&steps), 1);
        assert_eq!(flat_range(&steps, 0), 0..0, "the group owns no slot");
        assert_eq!(flat_range(&steps, 1), 0..1, "the tail owns slot 0");

        let mut status = RunStatus::queued(
            RunId::from_token("flatempty001".to_string()),
            RunMode::Chain,
            Some(1),
        );
        status.steps = steps.iter().flat_map(pending_step_statuses_for).collect();
        assert_eq!(status.steps.len(), 1);
        record_step_outcome(
            &mut status,
            &flat_range(&steps, 0),
            &steps[0],
            &StepResult::failure("the empty group"),
            None,
        );
        assert_eq!(
            status.steps[0].status,
            StepState::Pending,
            "the tail must not have been settled by the group ahead of it: {:?}",
            status.steps
        );
        assert!(status.steps[0].error.is_none(), "{:?}", status.steps);
    }

    /// SUBA-093 — a step that occupies one flat slot still records the aggregate on that slot, and
    /// a chain's later steps are numbered past a group's whole width.
    #[test]
    fn a_single_step_records_on_its_own_slot_and_flat_bases_skip_a_groups_width() {
        let steps = vec![
            RunnerStep::SingleStep(single_step("lead", "l")),
            RunnerStep::ParallelGroup(crate::spawn::chain_graph::ParallelGroupSpec {
                steps: vec![single_step("x", "x"), single_step("y", "y")],
                concurrency: 2,
                fail_fast: false,
                worktree: false,
            }),
            RunnerStep::SingleStep(single_step("tail", "t")),
        ];
        assert_eq!(flat_total(&steps), 4);
        assert_eq!(flat_base(&steps, 2), 3);
        assert_eq!(flat_range(&steps, 1), 1..3);

        let mut status = RunStatus::queued(
            RunId::from_token("flatchain001".to_string()),
            RunMode::Chain,
            Some(1),
        );
        status.steps = steps.iter().flat_map(pending_step_statuses_for).collect();
        assert_eq!(status.steps.len(), 4);
        record_step_outcome(
            &mut status,
            &(3..4),
            &steps[2],
            &StepResult::failure("tail failed"),
            None,
        );
        assert_eq!(status.steps[3].status, StepState::Failed);
        assert_eq!(status.steps[3].error.as_deref(), Some("tail failed"));
        assert!(
            status.steps[..3]
                .iter()
                .all(|s| s.status == StepState::Pending),
            "recording the tail step touches nothing ahead of it"
        );
    }

    /// G77 widening 2 — `mark_remaining_stopped`'s PARALLEL-GROUP child sweep.
    ///
    /// A parallel group's children live on `RunStatus::parallel_groups`, not in the flat `steps`
    /// list, and upstream's `stopRunner` sweep marks every non-terminal one `"stopped"` with the
    /// stop message (`subagent-runner.ts:2955-2986`, whose `statusPayload.steps` walk covers the
    /// normalized parallel children too). The flat half was covered by the mid-flight stop
    /// integration test; the group half never was — a single-step run has no groups at all.
    ///
    /// Three properties, all upstream's: already-terminal children are LEFT ALONE (a child that
    /// genuinely completed before the stop landed is not relabelled), groups strictly before the
    /// cursor are untouched, and every swept child gets [`control::STOP_MESSAGE`] plus an end
    /// timestamp.
    #[test]
    fn mark_remaining_stopped_sweeps_parallel_group_children_without_relabelling_finished_ones() {
        let mut status = RunStatus::queued(
            RunId::from_token("stopgroups01".to_string()),
            RunMode::Chain,
            Some(1),
        );
        status.state = RunState::Running;
        status.steps = vec![
            crate::background::StepStatus::pending("group-a"),
            crate::background::StepStatus::pending("group-b"),
        ];
        let group = |index: usize, statuses: &[StepState]| crate::background::ParallelGroupStatus {
            group_step_index: index,
            children: statuses
                .iter()
                .map(|s| {
                    let mut child = crate::background::StepStatus::pending("kid");
                    child.status = *s;
                    child
                })
                .collect(),
        };
        status.parallel_groups = Some(vec![
            // Strictly BEFORE the cursor: already settled, must not be touched.
            group(0, &[StepState::Complete]),
            // At the cursor: one mid-flight, one never started, one already finished.
            group(
                1,
                &[StepState::Running, StepState::Pending, StepState::Complete],
            ),
        ]);

        mark_remaining_stopped(&mut status, 1, 2, control::STOP_MESSAGE);

        let groups = status
            .parallel_groups
            .as_ref()
            .expect("groups survive the sweep");
        assert_eq!(
            groups[0].children[0].status,
            StepState::Complete,
            "a group before the cursor is not part of the sweep at all"
        );
        assert!(
            groups[0].children[0].error.is_none(),
            "and picks up no stop message either"
        );

        let swept = &groups[1].children;
        assert_eq!(
            swept[0].status,
            StepState::Stopped,
            "the mid-flight parallel child must be marked Stopped, never Failed and never Paused"
        );
        assert_eq!(
            swept[1].status,
            StepState::Stopped,
            "a never-started parallel child is swept too (upstream sweeps `running` OR `pending`)"
        );
        assert_eq!(
            swept[2].status,
            StepState::Complete,
            "a child that genuinely finished before the stop landed keeps its own verdict"
        );
        assert_eq!(swept[0].error.as_deref(), Some(control::STOP_MESSAGE));
        assert_eq!(swept[1].error.as_deref(), Some(control::STOP_MESSAGE));
        assert!(
            swept[2].error.is_none(),
            "the finished child is not restamped with a stop message it never earned"
        );
        assert!(
            swept[0].ended_at.is_some(),
            "a swept child gets an end timestamp"
        );
        assert!(
            swept[1].ended_at.is_some(),
            "a swept child gets an end timestamp"
        );

        // The flat step list is swept by the same call, from the same cursor.
        assert_eq!(
            status.steps[0].status,
            StepState::Pending,
            "before the cursor"
        );
        assert_eq!(status.steps[1].status, StepState::Stopped);
        assert_eq!(
            status.steps[1].error.as_deref(),
            Some(control::STOP_MESSAGE)
        );
    }

    /// G77 widening 3, half one — `promote_interrupted_results_to_stopped` must NOT touch a child
    /// that had already settled before the stop landed.
    ///
    /// pi's own promotion is `stopped: stoppedAfterAcceptance ? true : finalResult?.stopped`
    /// (`subagent-runner.ts:1642,1722` @v0.43.0), applied to the child the stop signal tore down — a child
    /// whose record settled while `stopSignal.aborted` was still false keeps its own verdict.
    /// cyrup's witness for "torn down by the stop" is `interrupted` (all three control verbs share
    /// one cancellation token), so this asserts the filter, not just the rewrite.
    #[test]
    fn promoting_stopped_children_leaves_already_settled_ones_alone() {
        let settled = |agent: &str, interrupted: bool, output: Option<&str>| SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            turn_budget_exceeded: false,
            wrap_up_requested: false,
            child_run_id: None,
            agent: agent.to_string(),
            task: String::new(),
            exit_code: 0,
            usage: cyrup_core::Usage::default(),
            turns: 0,
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: output.map(str::to_string),
            structured_output: None,
            session_file: None,
            output_state: Default::default(),
            structured_output_path: None,
            artifact_paths: None,
            acceptance: None,
            detached: false,
            interrupted,
            timed_out: false,
            timeout_recovery: None,
            context_overflow: false,
            stopped: false,
            process_signal: None,
            error: None,
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
            runner: None,
            external_process: None,
        };
        let mut results = vec![
            settled(
                "finished-first",
                false,
                Some("I completed before the stop."),
            ),
            settled("torn-down", true, None),
            settled("torn-down-with-output", true, Some("partial work")),
        ];

        promote_interrupted_results_to_stopped(&mut results, control::STOP_MESSAGE);

        assert!(
            !results[0].stopped && !results[0].interrupted && results[0].exit_code == 0,
            "a child that settled before the stop keeps its own clean record: {:?}",
            results[0]
        );
        assert_eq!(
            results[0].final_output.as_deref(),
            Some("I completed before the stop.")
        );

        for promoted in &results[1..] {
            assert!(promoted.stopped, "{promoted:?}");
            assert!(
                !promoted.interrupted,
                "`interrupted` must be CLEARED, or the run reads as resumable: {promoted:?}"
            );
            assert_eq!(promoted.exit_code, 1, "pi `subagent-runner.ts:909`");
            assert_eq!(promoted.error.as_deref(), Some(control::STOP_MESSAGE));
        }
        assert_eq!(
            results[1].final_output.as_deref(),
            Some(control::STOP_MESSAGE),
            "a torn-down child with NO output of its own gets the stop message as its output \
             (pi `subagent-runner.ts:917`)"
        );
        assert_eq!(
            results[2].final_output.as_deref(),
            Some("partial work"),
            "a torn-down child that DID produce output keeps it (pi's `!finalOutput.trim()` guard)"
        );
    }
}

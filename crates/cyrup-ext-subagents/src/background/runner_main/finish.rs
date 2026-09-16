//! The R-SA-077 terminal funnel: [`settle_loop_outcome`] classifies the loop's outcome and
//! [`finish_run`] performs the `status.json`-THEN-`ResultFile` write ordering every exit path
//! routes through. Split out of `background/runner_main.rs`; ports
//! pi `runs/background/subagent-runner.ts`.

use super::config::RunnerConfig;
use super::events::append_event;
use super::turn_loop::LoopOutcome;
use crate::background::atomic::write_atomic_json;
use crate::background::result_index::{self, ResultWrite};
use crate::background::{ResultFile, RunMode, RunPaths, RunState, RunStatus};
use crate::error::SubagentError;
use crate::exec::SingleResult;
use crate::jsonl::BoundedJsonlWriter;
use std::path::PathBuf;

/// Fold [`run_inner`](super::turn_loop::run_inner)'s outcome into the terminal `(state, results, error)` triple [`finish_run`]
/// records, appending the matching terminal `subagent.run.*` event for each shape on the way.
pub(super) async fn settle_loop_outcome(
    loop_outcome: Result<LoopOutcome, SubagentError>,
    config: &RunnerConfig,
    events: &mut Option<BoundedJsonlWriter>,
    duration_ms: i64,
) -> (RunState, Vec<SingleResult>, Option<String>) {
    let run_id_str = config.run_id.as_str().to_string();
    match loop_outcome {
        Ok(LoopOutcome::Completed { results }) => {
            let all_ok = results.iter().all(|r| r.exit_code == 0);
            append_event(
                events,
                "subagent.run.completed",
                Some(serde_json::json!({
                    "runId": run_id_str,
                    "status": if all_ok { "complete" } else { "failed" },
                    "durationMs": duration_ms,
                })),
            )
            .await;
            (
                if all_ok {
                    RunState::Complete
                } else {
                    RunState::Failed
                },
                results,
                None,
            )
        }
        Ok(LoopOutcome::Interrupted { results }) => {
            append_event(
                events,
                "subagent.run.paused",
                Some(serde_json::json!({ "runId": run_id_str })),
            )
            .await;
            (RunState::Paused, results, None)
        }
        Ok(LoopOutcome::TimedOut { results, message }) => {
            // pi `subagent.run.timed_out` (`subagent-runner.ts:2053-2060` @v0.34.0), carrying both
            // the nominal budget and the absolute deadline so a reader can tell a run that used
            // its whole budget from one an ancestor cut short.
            append_event(
                events,
                "subagent.run.timed_out",
                Some(serde_json::json!({
                    "runId": run_id_str,
                    "timeoutMs": config.timeout_ms,
                    "deadlineAt": config.deadline_at_ms,
                    "message": message,
                    "durationMs": duration_ms,
                })),
            )
            .await;
            (RunState::Failed, results, Some(message))
        }
        Ok(LoopOutcome::Stopped { results, message }) => {
            // G77 — pi `subagent.run.stopped` (`subagent-runner.ts:2977-2982` @v0.43.0), carrying
            // the stop message so a reader of `events.jsonl` sees WHY the run ended without having
            // to reconstruct it from the terminal record. `durationMs` follows the same shape the
            // sibling terminal events use here.
            append_event(
                events,
                "subagent.run.stopped",
                Some(serde_json::json!({
                    "runId": run_id_str,
                    "message": message,
                    "durationMs": duration_ms,
                })),
            )
            .await;
            (RunState::Stopped, results, Some(message))
        }
        Err(err) => {
            append_event(
                events,
                "subagent.run.completed",
                Some(serde_json::json!({
                    "runId": run_id_str,
                    "status": "failed",
                    "durationMs": duration_ms,
                    "error": err.to_string(),
                })),
            )
            .await;
            (RunState::Failed, Vec::new(), Some(err.to_string()))
        }
    }
}

// =================================================================================================
// finish_run — R-SA-077's write-ordering invariant, the ONE funnel every exit path routes through
// =================================================================================================

/// Compute the terminal [`RunState`], write the final `status.json`, THEN write the terminal
/// [`ResultFile`] — in that EXACT order, unconditionally, on every single call site (R-SA-077).
///
/// This is the sole function in this module that writes a run's TERMINAL records. Every call site
/// in [`run`](super::run) funnels through here rather than writing either file directly, which is what makes
/// the ordering invariant structural rather than a convention every future edit must remember to
/// preserve: adding a new early-return branch to [`run`](super::run) in the future still cannot skip this
/// ordering unless that branch also skips calling this function entirely (in which case NEITHER
/// file is written, which is a strictly safer failure mode than writing them out of order — a
/// caller polling this run id sees "still Queued/Running" rather than an observably-ahead-of-
/// itself `ResultFile` with no matching terminal `status.json`).
///
/// `error` is folded into every result's own `error` field (if `results` is non-empty) OR, if
/// `results` is empty (the run never got far enough to produce even one step outcome), into a
/// single synthesized placeholder [`SingleResult`] so [`ResultFile::results`] is never silently
/// empty for a run that reached a terminal Failed state — a downstream reader walking `results`
/// should always find at least one entry explaining what happened, mirroring
/// `reconcile.rs::synthesize_step_results`'s identical "never leave results empty" contract for
/// the stale-dead-reconciliation path (this is the runner's OWN, first-hand analogue of that same
/// contract, not a re-derivation of `reconcile.rs`'s logic).
///
/// # Double-invocation idempotency (a `finish_run`-level guard, not just `read_and_delete_config`'s)
///
/// Before writing anything, this function checks whether `run_paths.legacy_result_root` ALREADY exists on
/// disk. If it does, some earlier call — this same process's own [`run`](super::run) invocation, or (per this
/// module's documented double-invocation scenario) a wholly separate, later `run()` invocation
/// against the same `--config`/`run_paths` pair after the first has already reached a terminal
/// write — has already produced the authoritative terminal record for this run id, and this call
/// is a no-op: neither `status.json` nor the `ResultFile` is touched. Without this guard, a second
/// `run()` invocation whose `read_and_delete_config` call observes
/// [`ConfigConsumeOutcome::AlreadyConsumed`](super::ConfigConsumeOutcome::AlreadyConsumed) (module docs: "degrades gracefully instead of
/// crashing") would still reach `finish_run` and — since the terminal-transition write below is
/// deliberately unconditional/guard-bypassing precisely so it can ALWAYS reach a terminal state —
/// silently overwrite a genuinely-completed run's `Complete`/`success: true` result with a
/// synthesized `Failed`/`success: false` one. R-SA-077 already establishes that `ResultFile`
/// presence is the single authoritative "truly done" signal for every OTHER reader in this
/// subsystem (`reconcile.rs`, `control.rs`); this guard applies that identical principle
/// reflexively to the runner's own terminal-write path, so "no panic on double-invocation" (this
/// module's literal contract) also means "no silent data corruption of an already-final result"
/// (the property that contract exists to protect in the first place).
/// Whether this run has ALREADY recorded a terminal [`ResultFile`].
///
/// The guard behind `finish_run`'s double-invocation refusal, and it must resolve rather than
/// probe a fixed path: a promoted payload lives under `result-owned/<enc(session)>/`, so testing
/// the legacy root would answer "no result yet" for every run this build completes — and the
/// unconditional terminal write that follows would overwrite a genuine `Complete` result with a
/// synthesized failure.
async fn terminal_result_exists(run_paths: &RunPaths, status: &RunStatus) -> bool {
    match status.session_id.as_ref() {
        Some(session_id) => run_paths
            .resolve_result(session_id, &status.run_id)
            .await
            .is_some(),
        None => matches!(
            tokio::fs::try_exists(&run_paths.legacy_result_root).await,
            Ok(true)
        ),
    }
}

/// The two [`ResultFile`] fields a settled workflow contributes to a terminal write — pi's
/// `status.workflowChildren`/`publicResult.workflowReceipt` spreads
/// (`workflow-settlement.ts:231`, `:235`).
///
/// [`Default`] (both `None`) for every one of [`finish_run`]'s six current callers, none of which
/// is a workflow — §0.6 cut the detached async runner; the foreground path (§3.2) never calls this
/// (it writes the terminal status itself). The NON-default value is produced by
/// [`apply_workflow_settlement_plan`] below, whose caller
/// ([`crate::extension::executor::workflow_detach`], SCOPE_8) assembles its own
/// [`ResultFile`] rather than going through `finish_run`. Pointer: §4.
#[derive(Clone, Debug, Default)]
pub(crate) struct WorkflowResultFields {
    /// [`ResultFile::workflow_children`].
    pub workflow_children: Option<crate::workflows::WorkflowChildSummary>,
    /// [`ResultFile::workflow_receipt`] — built only when BOTH a receipt and a path exist
    /// (`workflow-settlement.ts:235`), never inserted-then-removed (`:238`).
    pub workflow_receipt: Option<crate::workflows::WorkflowReceiptRef>,
}

/// Stamp a [`crate::workflows::WorkflowSettlementPlan`] onto the terminal records — the ONE
/// function a workflow terminal path calls instead of open-coding the assembly (upstream
/// open-codes it twice, at `runs/foreground/subagent-executor.ts:5722-5727` and `:5768-5777`; the
/// deduplicated form is `planWorkflowSettlement`, whose sole upstream caller is
/// `workflow-detach-reconcile.ts:228`).
///
/// Lives beside [`finish_run`] rather than inside it: `finish_run` is R-SA-077's write-ordering
/// funnel and has six callers today, none of which is a workflow. The async arm was cut (§0.6);
/// the foreground path writes its own terminal status via `settle_foreground_workflow` (§3.2).
/// Pointer: §4.
///
/// **The "future async workflow arm" that doc anticipated has arrived**: SCOPE_8's detached-child
/// reconciler ([`crate::extension::executor::workflow_detach::reconcile_detached_workflow_child_completion`])
/// is this function's first production caller, which is why the
/// `#[cfg_attr(not(test), allow(dead_code))]` that used to sit here is gone. It calls this and
/// then performs its OWN `write_atomic_json` → `update_active_run_index` →
/// `write_async_result_file` sequence — the same ordering `finish_run` performs — rather than
/// calling `finish_run`, whose double-invocation guard (`:237-245`) refuses precisely the
/// superseding second write over an already-published `Paused` result that a detached-child
/// settlement IS. Hence `finish_run` stays `pub(super)`; only this stamper is re-exported
/// (`runner_main/mod.rs`).
pub(crate) fn apply_workflow_settlement_plan(
    plan: &crate::workflows::WorkflowSettlementPlan,
    status: &mut RunStatus,
) -> WorkflowResultFields {
    *status = plan.status.clone();
    WorkflowResultFields {
        workflow_children: plan.status.workflow_children.clone(),
        workflow_receipt: match (&plan.receipt, &plan.receipt_path) {
            (Some(receipt), Some(path)) => Some(crate::workflows::WorkflowReceiptRef {
                path: path.clone(),
                receipt: receipt.clone(),
            }),
            _ => None,
        },
    }
}

// `finish_run` was already at clippy's `too_many_arguments` ceiling (seven) before WORKFLOW_3;
// `workflow` is the one addition this task's own doc anticipated ("a struct is the better
// shape") — it IS a struct, bundling two fields into one parameter rather than two, but the
// pre-existing seven were out of this task's scope to redesign.
#[allow(clippy::too_many_arguments)]
pub(super) async fn finish_run(
    run_paths: &RunPaths,
    mut status: RunStatus,
    terminal_state: RunState,
    mut results: Vec<SingleResult>,
    cwd: PathBuf,
    session_file: Option<PathBuf>,
    error: String,
    workflow: WorkflowResultFields,
) {
    if terminal_result_exists(run_paths, &status).await {
        tracing::warn!(
            run_id = %status.run_id,
            "finish_run called again after a terminal ResultFile already exists on disk \
             (double-invocation of the runner); leaving the existing authoritative result \
             untouched rather than overwriting it"
        );
        return;
    }

    // Force the terminal transition directly (mirrors `reconcile.rs::synthesize_failure`'s own
    // rationale): `finish_run` must be able to reach ANY of Complete/Failed/Paused regardless of
    // which (possibly already-illegal-from-here) state `status` currently holds, since this is the
    // authoritative "this run is now over" write, not a normal forward-progress transition subject
    // to the ordinary transition guard.
    let now = crate::time::now_epoch_millis();
    status.state = terminal_state;
    status.last_update = now;
    status.ended_at = Some(now);
    // pi `statusPayload.cwd`/`statusPayload.sessionFile` (`subagent-runner.ts:3021` @v0.34.0): the
    // terminal `status.json` write carries the SAME `cwd`/`sessionFile` the terminal `ResultFile`
    // below does, so `resume`'s terminal-revival branch (R-SA-085) can read `status.cwd ??
    // result.cwd` (`background/async-resume.ts:323,345,373`) straight off the reconciled status
    // without needing a second, separate ResultFile read.
    status.cwd = Some(cwd.clone());
    status.session_file = session_file.clone();

    if !error.is_empty() && results.is_empty() {
        results.push(SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            turn_budget_exceeded: false,
            wrap_up_requested: false,
            agent: status
                .steps
                .first()
                .map(|s| s.agent.clone())
                .unwrap_or_else(|| status.run_id.as_str().to_string()),
            task: String::new(),
            exit_code: 1,
            usage: cyrup_core::Usage::default(),
            turns: 0,
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            // A synthesized run-level failure names no child run.
            child_run_id: None,
            // G77 — pi's `stoppedStepResult` fills BOTH halves (`subagent-runner.ts:2358-2365`:
            // `output: stopMessage, error: stopMessage`), and the sibling live-child path here
            // ([`promote_interrupted_results_to_stopped`]) already does the same. Without it a
            // stopped run whose stop landed before any step produced a result delivers an EMPTY
            // output alongside a populated error, which every output-shaped reader (the notify
            // completion message, the intercom payload's `outputs`, the status report) renders as
            // "the run produced nothing" rather than "the run was stopped". Only for `Stopped`:
            // upstream's other synthesized shapes carry their own messages and cyrup's `finish_run`
            // has no way to tell a plain `Failed` apart from a timed-out one.
            final_output: (terminal_state == RunState::Stopped).then(|| error.clone()),
            structured_output: None,
            // A synthesized terminal placeholder: no step ever produced a result, so nothing was
            // produced (`Absent`, known), and there is no transcript or capture to name. The
            // run-level `session_file` is stamped onto the terminal STATUS above, not onto this
            // per-child placeholder — it is the parent run's session, not a child's.
            session_file: None,
            output_state: crate::exec::output_state::SubagentOutputState::Absent,
            structured_output_path: None,
            artifact_paths: None,
            acceptance: None,
            detached: false,
            interrupted: terminal_state == RunState::Paused,
            timed_out: false,
            // A synthesized placeholder for a run that produced no step results at all — no child
            // ran, so there is no worktree evidence to summarize either.
            timeout_recovery: None,
            // A synthesized placeholder for a run that produced no step results at all — no ladder
            // ran, so there is no overflow classification to report.
            context_overflow: false,
            // G77 — pi `runSubagent`'s stopped result carries `stopped: true` and
            // `exitCode: 1` (`subagent-runner.ts:2358-2365`), which is what
            // `resolveSubagentResultStatus`/`buildCompletionDetails`/`resultState` all read to
            // classify the child as stopped rather than merely failed.
            stopped: terminal_state == RunState::Stopped,
            process_signal: None,
            error: Some(error.clone()),
            saved_output_path: None,
            tool_calls: Vec::new(),
            // A SYNTHESIZED run-level failure: no step ran, so no child was planned and no surface
            // exists to publish.
            tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
            runner: None,
            external_process: None,
        });
    }

    // `success` iff the run reached `Complete` AND every recorded result exited cleanly.
    // `Iterator::all` is vacuously `true` over an empty `results` list (a `Complete` run that
    // produced zero step results — e.g. a `Chain` run whose `steps` list was itself empty — is
    // treated as a success, matching this crate's general "no work attempted, no work failed"
    // convention rather than requiring a nonsensical "at least one result" precondition).
    let success = terminal_state == RunState::Complete && results.iter().all(|r| r.exit_code == 0);

    // R-SA-077: status.json THEN ResultFile, in that exact order. Both writes are best-effort at
    // the OUTER level (a failure writing `status.json` here still attempts the `ResultFile` write,
    // rather than leaving the run in an indefinite non-terminal state on disk merely because ONE
    // of the two writes hit a transient I/O error) — but the ORDER between the two calls is never
    // reordered, which is the actual invariant R-SA-077 requires; `write_atomic_json`'s own
    // temp-then-rename guarantee (R-SA-076) means a reader never observes a torn write of either
    // individual file, only ever "old status, no result" or "new status, no result yet" or "new
    // status, new result" — never "new result, old status", since the result write is issued
    // strictly after the status write is issued here.
    let status_write = write_atomic_json(&run_paths.status, &status).await;

    // SCOPE_9/SUBTASK5 — the index router (pi `updateActiveRunIndex`, `subagent-runner.ts:2137`),
    // whose TERMINAL arm is what reaches `updateTerminalRunIndex`. This call site's comment used
    // to name that indirection as a fact about upstream while calling the terminal writer
    // directly; the router now exists, so the loop is closed and the active marker this run wrote
    // at startup (`runner_main/entry.rs`) is released here rather than outliving the run.
    //
    // Best-effort, in the same "both writes are attempted" spirit as the two writes around it: an
    // advisory index that failed to write must never fail the run that was finishing. The router
    // writes the terminal marker BEFORE releasing the active one, so a process dying between them
    // leaves the run in both indexes rather than in neither.
    if let Err(err) =
        crate::background::active_run_index::update_active_run_index(&run_paths.run_dir, &status)
            .await
    {
        tracing::warn!(
            run_id = %status.run_id,
            error = %err,
            "failed to update the async run index; the run's own terminal record is unaffected"
        );
    }

    // ensureAccessibleDir-equivalent, final guard (C7): the terminal ResultFile's directory MUST
    // exist for the authoritative "done" signal to land. This covers every exit path — including
    // the config-less pre-read error branches that pass in the caller-derived `run_paths`, whose
    // results dir the orchestrator may or may not have created — so a run can never silently fail
    // to record its terminal result merely because its results dir was absent (the exact C7
    // failure mode: the runner's divergent, never-created results dir).
    let _ = crate::background::ensure_accessible_dir(&run_paths.results_dir).await;

    // SCOPE_17 — pi `agentName` (`subagent-runner.ts:4765-4770`), ported verbatim:
    //
    //   const finalFlatAgents = statusPayload.steps.map((step) => step.agent);
    //   const agentName = finalFlatAgents.length === 1
    //       ? finalFlatAgents[0]!
    //       : resultMode === "parallel"
    //           ? `parallel:${finalFlatAgents.join("+")}`
    //           : `chain:${finalFlatAgents.join("->")}`;
    //
    // This field is what `background::watch::format_completion_message` renders as
    // `Background task completed: **<agent>**`. Taking `steps.first()` alone made a 2-child fan-out
    // announce itself as `**delegate**`, because `flat_index::pending_step_statuses_for` seeds one
    // status step per MEMBER, so `steps[0].agent` is the first member's name.
    //
    // The width-1 arm comes FIRST and is why a single-agent run is still named for its agent rather
    // than `parallel:agent` — the notification this task never touched stays byte-identical.
    //
    // `status.mode` is the whole of pi's `resultMode` (`:4757`, `config.resultMode ??
    // statusPayload.mode`): cyrup's `RunnerConfig` carries no result-mode override.
    let flat_agents: Vec<&str> = status.steps.iter().map(|s| s.agent.as_str()).collect();
    let agent = match flat_agents.as_slice() {
        [] => status.run_id.as_str().to_string(),
        [only] => (*only).to_string(),
        many if status.mode == RunMode::Parallel => format!("parallel:{}", many.join("+")),
        many => format!("chain:{}", many.join("->")),
    };

    let result_file = ResultFile {
        id: status.run_id.clone(),
        run_id: status.run_id.clone(),
        agent,
        mode: status.mode,
        state: terminal_state,
        success,
        cwd,
        session_file,
        // Both identities are sourced from `status`, never from a `RunnerConfig`: `finish_run`
        // has six production callers and two of them (`config.rs`'s config-load-failure paths)
        // have no config at all. `status` is the one input every caller supplies.
        session_id: status.session_id.clone(),
        completion_owner_id: status.completion_owner_id.clone(),
        results,
        workflow_children: workflow.workflow_children,
        workflow_receipt: workflow.workflow_receipt,
        // `None` structurally: the detached runner executes an agent/chain graph, and
        // `RunMode::Workflow` is never emitted here (`background/state.rs:22-37`) — so no run this
        // function settles can be a scheduled fire. A schedule's origin is stamped by the one
        // writer that has it, `extension/executor/scheduled_runs.rs`.
        schedule_origin: None,
    };
    let result_write = write_result_file(run_paths, &status, &result_file).await;

    if let Err(err) = status_write {
        tracing::warn!(
            run_id = %status.run_id,
            error = %err,
            "failed to write terminal status.json (R-SA-077); ResultFile write was still \
             attempted per this function's own best-effort-both-writes contract"
        );
    }
    if let Err(err) = result_write {
        tracing::warn!(
            run_id = %status.run_id,
            error = %err,
            "failed to write terminal ResultFile (R-SA-077)"
        );
    }

    // Best-effort run-history recording (pi's `recordRun`, `run-history.ts`): one line per
    // top-level result appended to `<agent_dir>/run-history.jsonl` (pi `getHistoryPath()`,
    // `runs/shared/run-history.ts:23-25` @v0.43.0 — the DURABLE agent dir, deliberately not the
    // disposable `temp_root_dir` scratch tree). Placed AFTER the
    // authoritative status/ResultFile writes (and inside the double-invocation guard above, so a
    // no-op re-invocation never double-records) — a history-write failure never affects the run.
    //
    // The run's OWN async root (`run_dir`'s parent) is handed over rather than re-derived, so a run
    // whose roots were redirected records its history with them instead of in the real user's agent
    // dir — see [`crate::background::run_history_path_for`].
    let async_root = run_paths.run_dir.parent().unwrap_or(&run_paths.run_dir);
    crate::background::record_run_history(async_root, status.started_at, &result_file.results)
        .await;
}

/// Write the terminal [`ResultFile`] through the session-partitioned index.
///
/// pi `writeAsyncResultFile` (`result-files.ts:178-183`) replaces what used to be a bare
/// `write_atomic_json` to the public path. The payload is now staged session-private, indexed, and
/// only then promoted, so no result is publicly visible before it can be attributed.
///
/// # A result with no session is a hard failure, not a silent raw write
///
/// pi throws for exactly this case (`result-files.ts:166`). An unattributable result cannot be
/// indexed, and an unindexed result is invisible to every reader: it would be written, never
/// delivered, never found by a lookup, and never cleaned up except by the age sweep. Falling back
/// to the old unindexed write would recreate precisely the orphan this change exists to remove.
///
/// The failure is not silent to the user either — `status.json` is written first and independently
/// (R-SA-077's ordering, above), and that is what `/subagents` reads, so a run that cannot record
/// a result still reports its terminal state.
///
/// # Errors
///
/// A staging or session-index write failure, or [`SubagentError::Management`] when the run has no
/// recorded [`crate::identity::SessionId`].
async fn write_result_file(
    run_paths: &RunPaths,
    status: &RunStatus,
    result_file: &ResultFile,
) -> Result<(), SubagentError> {
    let Some(session_id) = status.session_id.as_ref() else {
        return Err(SubagentError::Management(format!(
            "cannot write async result for run '{}' without a session id: an unattributable \
             result cannot be indexed and would never be delivered",
            status.run_id
        )));
    };
    let results_dir = run_paths.results_dir.as_path();

    let state = result_index::write_async_result_file(
        &ResultWrite {
            results_dir,
            session_id,
            run_id: &status.run_id,
            written_at: crate::time::now_epoch_millis(),
            async_dir: Some(run_paths.run_dir.as_path()),
            tool_call_id: None,
        },
        result_file,
    )
    .await
    .map_err(SubagentError::Spawn)?;

    // `Staged` is not a failure — the payload is safely written and the first reader that resolves
    // it will promote it — but it is not the ordinary outcome either, and reporting it as plain
    // success (what `.map(|_state| ())` did) made a promotion that never happened invisible.
    if state == result_index::PayloadState::Staged {
        tracing::warn!(
            run_id = %status.run_id,
            "terminal result stayed staged: promotion did not publish it, so delivery waits for a \
             reader to promote it on read"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::run;
    use super::super::tests::{run_paths_in, single_step};
    use super::*;
    use crate::background::control;
    use crate::background::{RunId, RunMode};
    use crate::spawn::chain_graph::RunnerStep;
    use std::collections::BTreeMap;

    /// Where `finish_run` actually publishes a terminal result for these fixtures: the owning
    /// session's partition, never the shared results root.
    fn terminal_result_path(run_paths: &RunPaths, run_id: &RunId) -> std::path::PathBuf {
        crate::background::result_index::owned_payload_path(
            &run_paths.results_dir,
            &crate::identity::SessionId::parse("test-session").expect("non-empty"),
            run_id,
        )
    }

    // ---------------------------------------------------------------------------------------
    // Second-pass adversarial-review regression: `run()`'s control-inbox-directory creation step
    // (between the initial status.json write and `run_inner`) must route ANY failure through
    // `finish_run` exactly like every other pre-loop fallible step, never bypass it via a bare
    // `?`. This is provable WITHOUT the fixture binary (never reaches `run_inner`/subprocess
    // dispatch at all): pre-creating a plain FILE at the exact path `run()` needs to
    // `create_dir_all` as a directory forces that call to fail deterministically on every
    // platform (`ENOTDIR`/`AlreadyExists`-as-non-directory), with no timing dependency.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn control_inbox_dir_creation_failure_still_reaches_a_terminal_failed_state_via_finish_run()
     {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-badcontrol");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir)
            .await
            .expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        // `run_paths.control_inbox` is `<run_dir>/control/interrupt.json`, so its parent is
        // `<run_dir>/control`. Pre-create a plain FILE at exactly that path so `run()`'s own
        // `tokio::fs::create_dir_all(.../control)` call is guaranteed to fail.
        tokio::fs::write(run_paths.run_dir.join("control"), b"not a directory")
            .await
            .expect("pre-create a blocking file where the control dir needs to go");

        let config = RunnerConfig {
            // SUBA-021: unbudgeted on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            permission_rules: None,
            // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
            // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
            timeout_ms: None,
            deadline_at_ms: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            run_id: run_id.clone(),
            mode: RunMode::Single,
            steps: vec![RunnerStep::SingleStep(single_step("worker", "do it"))],
            cwd: dir.path().to_path_buf(),
            session_file: None,
            session_id: Some("test-session".to_string()),
            completion_owner_id: None,
            global_concurrency_limit: 20,
            worktree_base_dir: None,
            max_subagent_depth: 2,
            // Empty roots => `run` falls back to the caller-derived `run_paths` (these unit tests'
            // pre-C7 behavior). The C7 config-driven-rebuild path is exercised end to end in
            // `tests/background_runner_main_integration.rs`.
            async_root: PathBuf::new(),
            results_dir: PathBuf::new(),
            resolved_agents: BTreeMap::new(),
            original_task: String::new(),
            chain_dir: None,
            orchestrator_intercom_target: None,
            inherited_session_model: None,
            inherited_session_thinking: None,
            host_available_builtins: None,
            model_scope: None,
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            control: None,
            include_progress: None,
        };
        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &config)
            .await
            .expect("write config");

        let outcome = run(&cfg_path, &run_paths).await;
        assert!(
            outcome.is_ok(),
            "run() itself never returns Err to its own caller, even when the control-inbox \
             directory cannot be created: {outcome:?}"
        );

        let status_bytes = tokio::fs::read(&run_paths.status).await.expect(
            "status.json must exist and be terminal — a bare `?` bypassing finish_run would \
             leave it permanently stuck at the initial Running record written earlier in run()",
        );
        let status: RunStatus = serde_json::from_slice(&status_bytes).expect("valid JSON");
        assert_eq!(
            status.state,
            RunState::Failed,
            "the control-inbox-directory-creation failure must reach a terminal Failed status \
             via finish_run, not leave the run stuck Running forever: {status:?}"
        );

        let result_bytes = tokio::fs::read(&terminal_result_path(&run_paths, &run_id)).await.expect(
            "ResultFile must exist too — finish_run's own status-then-result ordering must still \
             hold on this exit path, not skip both writes entirely",
        );
        let result_file: ResultFile = serde_json::from_slice(&result_bytes).expect("valid JSON");
        assert_eq!(result_file.state, RunState::Failed);
        assert!(!result_file.success);
    }

    #[tokio::test]
    async fn finish_run_second_call_after_terminal_result_exists_does_not_overwrite_it() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-double-invoke");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir)
            .await
            .expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(111));
        // A real run always carries the launching session: `write_async_result_file` refuses an
        // unattributable result (pi `result-files.ts:166`), because it could never be delivered.
        status.session_id = crate::identity::SessionId::parse("test-session");
        status
            .advance_state(RunState::Running)
            .expect("Queued -> Running");

        // First call: a genuine successful completion.
        finish_run(
            &run_paths,
            status.clone(),
            RunState::Complete,
            vec![SingleResult {
                // SUBA-021: no usage budget on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                turn_budget_exceeded: false,
                wrap_up_requested: false,
                child_run_id: None,
                agent: "researcher".to_string(),
                task: "do the thing".to_string(),
                exit_code: 0,
                usage: cyrup_core::Usage::default(),
                turns: 0,
                model: None,
                attempted_models: Vec::new(),
                model_attempts: Vec::new(),
                final_output: Some("done".to_string()),
                structured_output: None,
                session_file: None,
                output_state: Default::default(),
                structured_output_path: None,
                artifact_paths: None,
                acceptance: None,
                detached: false,
                interrupted: false,
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
                // Test fixture: no child was planned, so there is no surface to report.
                tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
            }],
            dir.path().to_path_buf(),
            None,
            String::new(),
            WorkflowResultFields::default(),
        )
        .await;

        let first_result_bytes = tokio::fs::read(&terminal_result_path(&run_paths, &run_id))
            .await
            .expect("ResultFile exists after the first finish_run call");
        let first_result: ResultFile =
            serde_json::from_slice(&first_result_bytes).expect("valid JSON");
        assert_eq!(first_result.state, RunState::Complete);
        assert!(
            first_result.success,
            "first call recorded a genuine success"
        );

        let first_status_bytes = tokio::fs::read(&run_paths.status)
            .await
            .expect("status.json exists after the first finish_run call");

        // Second call: simulates a double-invocation of `run()` against the same config/run_paths
        // (e.g. `read_and_delete_config` observing `AlreadyConsumed` and `run`'s own tail routing
        // that outcome to `finish_run` with a freshly synthesized `Failed` status, exactly as
        // `run`'s `AlreadyConsumed` match arm does). This must NOT clobber the already-terminal,
        // already-successful result with a spurious failure.
        let mut second_status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(222));
        second_status.session_id = crate::identity::SessionId::parse("test-session");
        finish_run(
            &run_paths,
            second_status,
            RunState::Failed,
            Vec::new(),
            PathBuf::new(),
            None,
            "runner-config.json was already consumed".to_string(),
            WorkflowResultFields::default(),
        )
        .await;

        let result_bytes_after_second_call =
            tokio::fs::read(&terminal_result_path(&run_paths, &run_id))
                .await
                .expect("ResultFile still exists after the second finish_run call");
        let result_after_second_call: ResultFile =
            serde_json::from_slice(&result_bytes_after_second_call).expect("valid JSON");
        assert_eq!(
            result_after_second_call, first_result,
            "a second finish_run call against a run id with an already-terminal ResultFile must \
             leave it byte-for-byte untouched, never overwrite a genuine success with a \
             synthesized double-invocation failure"
        );

        let status_bytes_after_second_call = tokio::fs::read(&run_paths.status)
            .await
            .expect("status.json still exists after the second finish_run call");
        assert_eq!(
            status_bytes_after_second_call, first_status_bytes,
            "status.json must likewise be left untouched by a no-op double-invocation finish_run call"
        );
    }

    #[tokio::test]
    async fn finish_run_first_call_writes_normally_when_no_result_file_exists_yet() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-first-call");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir)
            .await
            .expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(1));
        status.session_id = crate::identity::SessionId::parse("test-session");

        finish_run(
            &run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            dir.path().to_path_buf(),
            None,
            "boom".to_string(),
            WorkflowResultFields::default(),
        )
        .await;

        assert!(
            tokio::fs::try_exists(&terminal_result_path(&run_paths, &run_id))
                .await
                .expect("check exists"),
            "the double-invocation guard must not block a genuine FIRST terminal write"
        );
        let result: ResultFile = serde_json::from_slice(
            &tokio::fs::read(&terminal_result_path(&run_paths, &run_id))
                .await
                .expect("read result"),
        )
        .expect("valid JSON");
        assert_eq!(result.state, RunState::Failed);
        assert!(!result.success);
    }

    #[tokio::test]
    async fn a_run_with_no_session_writes_status_but_refuses_the_result() {
        // pi `result-files.ts:166` throws for an unattributable result, and `finish_run` must not
        // fall back to a raw unindexed write: an unindexed result is invisible to every reader, so
        // it could only ever become garbage in a directory shared by every cyrup instance.
        //
        // The failure is still observable — `status.json` records the terminal state, and that is
        // what `/subagents` reads.
        let dir = tempfile::tempdir().expect("tempdir");
        let run_id = RunId::from_token("run-no-session");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir)
            .await
            .expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(1));
        assert!(
            status.session_id.is_none(),
            "precondition: no launching session"
        );

        finish_run(
            &run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            dir.path().to_path_buf(),
            None,
            "boom".to_string(),
            WorkflowResultFields::default(),
        )
        .await;

        assert!(
            tokio::fs::try_exists(&run_paths.status)
                .await
                .expect("check status"),
            "the terminal status must still be recorded"
        );
        assert!(
            !tokio::fs::try_exists(&terminal_result_path(&run_paths, &run_id))
                .await
                .expect("check result"),
            "an unattributable result must NOT be written, not even unindexed"
        );
    }

    // ---------------------------------------------------------------------------------------
    // WORKFLOW_3 §3c — apply_workflow_settlement_plan
    // ---------------------------------------------------------------------------------------

    /// `apply_workflow_settlement_plan` overwrites the caller's status with the plan's settled
    /// one and projects exactly the two `ResultFile` fields a workflow contributes — both only
    /// when BOTH a receipt and a path exist, never inserted-then-removed.
    #[test]
    fn apply_workflow_settlement_plan_projects_workflow_children_and_receipt_together() {
        let run_id = RunId::from_token("wf-run-0001");
        let mut status = RunStatus::queued(
            run_id.clone(),
            crate::background::RunMode::Workflow,
            Some(1),
        );
        status.state = RunState::Running;

        let run_dir_name = crate::identity::RunDirName::for_run(&run_id);
        let receipt =
            crate::workflows::build_workflow_receipt(crate::workflows::BuildWorkflowReceipt {
                workflow_run_id: &run_dir_name,
                state: crate::workflows::WorkflowReceiptState::Complete,
                children: &[],
                host_steps: &[],
                workflow_children: None,
                resource: None,
                terminal_outcome: None,
                created_at: Some(1_000),
            })
            .expect("builds");

        let plan =
            crate::workflows::plan_workflow_settlement(crate::workflows::PlanWorkflowSettlement {
                status: &status,
                summary: "done".to_string(),
                trace: &[],
                receipt: Some(receipt),
                receipt_path: Some(PathBuf::from("/tmp/workflow-receipt.json")),
                receipt_persistence_error: None,
                resolution: None,
                terminal_outcome: None,
                now: Some(2_000),
                event_metadata: serde_json::Map::new(),
            });

        let mut caller_status = status;
        let fields = apply_workflow_settlement_plan(&plan, &mut caller_status);

        assert_eq!(caller_status.state, plan.status.state);
        assert!(
            fields.workflow_children.is_some(),
            "the settled inventory is projected"
        );
        assert_eq!(
            fields.workflow_receipt.as_ref().map(|r| r.path.clone()),
            Some(PathBuf::from("/tmp/workflow-receipt.json")),
            "a receipt with a path projects both together"
        );
    }

    /// pi `statusPayload.cwd`/`statusPayload.sessionFile` (`subagent-runner.ts:3021` @v0.34.0): the
    /// terminal `status.json` write must carry the SAME `cwd`/`sessionFile` the terminal
    /// `ResultFile` does, so `resume`'s terminal-revival branch (R-SA-085,
    /// `background/async-resume.ts:323,345,373`) can read `status.cwd ?? result.cwd` straight off
    /// the reconciled status. Pre-fix, `finish_run` stamped `cwd`/`session_file` only onto the
    /// `ResultFile`, leaving `status.json`'s own (newly added) fields permanently `None`.
    #[tokio::test]
    async fn finish_run_stamps_cwd_and_session_file_onto_the_terminal_status_too() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let run_id = RunId::from_token("run-cwd-stamp");
        let run_paths = run_paths_in(dir.path(), &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir)
            .await
            .expect("mkdir run_dir");
        tokio::fs::create_dir_all(dir.path().join("results"))
            .await
            .expect("mkdir results_dir");

        let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(1));
        status.session_id = crate::identity::SessionId::parse("test-session");
        let run_cwd = dir.path().join("the-actual-run-cwd");
        let session_file = dir.path().join("session.jsonl");

        finish_run(
            &run_paths,
            status,
            RunState::Complete,
            Vec::new(),
            run_cwd.clone(),
            Some(session_file.clone()),
            String::new(),
            WorkflowResultFields::default(),
        )
        .await;

        let written_status: RunStatus = serde_json::from_slice(
            &tokio::fs::read(&run_paths.status)
                .await
                .expect("read status"),
        )
        .expect("valid JSON");
        assert_eq!(
            written_status.cwd,
            Some(run_cwd),
            "the terminal status.json write must carry the run's own cwd, matching the ResultFile"
        );
        assert_eq!(
            written_status.session_file,
            Some(session_file),
            "the terminal status.json write must carry the run's own session_file, matching the \
             ResultFile"
        );
    }

    // ---------------------------------------------------------------------------------------
    // G77 — the `stopped` widenings that had no coverage of their own: `finish_run`'s synthesized
    // child, `mark_remaining_stopped`'s parallel-GROUP child sweep (the flat step sweep was
    // covered; the group half never was), `promote_interrupted_results_to_stopped`'s
    // already-settled-child filter, and the stop message itself. The fourth claimed ordering — a
    // stop landing together with a timeout must win — needs a real runner and lives in
    // `tests/run_state_signal_and_stop_parity.rs`.
    // ---------------------------------------------------------------------------------------

    /// G77 — pi's stop message, pinned VERBATIM.
    ///
    /// `"Subagent stopped by user."` is the literal `stopMessage` upstream defines once
    /// (`runs/background/subagent-runner.ts:1972` @v0.43.0) and then repeats as the `??` default at
    /// `:779`, `:915`, `:917`, `:952`, `:1151`, `:1596`, `:1636`, `:1658`,
    /// `runs/shared/external-cli-runner.ts:108` and `runs/background/chain-root-attachment.ts:100,147`.
    /// It is stamped onto a stopped run's terminal `error`, onto every step the stop swept, and onto
    /// the child's `finalOutput` when the child produced none of its own — so a drifting copy would
    /// silently change three separate observable records at once. Nothing pinned the text itself;
    /// every existing assertion compared it against the constant, which cannot catch a drifted
    /// constant.
    #[test]
    fn the_stop_message_is_pis_verbatim_text() {
        assert_eq!(
            control::STOP_MESSAGE,
            "Subagent stopped by user.",
            "pi `subagent-runner.ts:1972`'s literal `stopMessage`"
        );
    }

    /// G77 widening 1 — `finish_run`'s SYNTHESIZED child.
    ///
    /// When a run reaches a terminal state having produced no step results at all, `finish_run`
    /// invents one placeholder [`SingleResult`] so the `ResultFile` is never silently empty. That
    /// placeholder must carry `stopped: true` for a `Stopped` run (pi `runSubagent`'s stopped result
    /// shape, `subagent-runner.ts:4448-4454`: `stopped: true`, `exitCode: 1`) — it is the ONLY thing
    /// that lets `resolveSubagentResultStatus` classify the child as stopped rather than merely
    /// failed, and therefore the only thing that makes the grouped intercom verdict `stopped`.
    ///
    /// The three sibling terminal states are asserted in the same test so the flag cannot be
    /// widened into an unconditional `true`.
    #[tokio::test]
    async fn finish_runs_synthesized_child_carries_stopped_only_for_a_stopped_run() {
        for (terminal_state, expect_stopped, expect_interrupted) in [
            (RunState::Stopped, true, false),
            (RunState::Paused, false, true),
            (RunState::Failed, false, false),
        ] {
            let dir = tempfile::tempdir().expect("real tempdir");
            let run_id = RunId::from_token(format!("synth-{terminal_state:?}").to_lowercase());
            let run_paths = run_paths_in(dir.path(), &run_id);
            tokio::fs::create_dir_all(&run_paths.run_dir)
                .await
                .expect("mkdir run_dir");
            tokio::fs::create_dir_all(dir.path().join("results"))
                .await
                .expect("mkdir results_dir");

            let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(1));
            status.session_id = crate::identity::SessionId::parse("test-session");
            status.steps = vec![crate::background::StepStatus::pending("scout")];

            finish_run(
                &run_paths,
                status,
                terminal_state,
                // Empty: this is exactly the input that makes `finish_run` synthesize a child.
                Vec::new(),
                dir.path().to_path_buf(),
                None,
                control::STOP_MESSAGE.to_string(),
                WorkflowResultFields::default(),
            )
            .await;

            let result: ResultFile = serde_json::from_slice(
                &tokio::fs::read(&terminal_result_path(&run_paths, &run_id))
                    .await
                    .expect("read result"),
            )
            .expect("valid JSON");
            assert_eq!(result.state, terminal_state);
            assert_eq!(
                result.results.len(),
                1,
                "a terminal run with an error and no step results must still explain itself"
            );
            let child = &result.results[0];
            assert_eq!(
                child.agent, "scout",
                "the placeholder inherits the first step's agent"
            );
            assert_eq!(child.exit_code, 1);
            assert_eq!(
                child.error.as_deref(),
                Some(control::STOP_MESSAGE),
                "{terminal_state:?}: the terminal error is folded onto the placeholder"
            );
            // pi `stoppedStepResult` fills `output: stopMessage` alongside `error: stopMessage`
            // (`subagent-runner.ts:2358-2365`). Only the stopped shape does — a plain `Failed`
            // placeholder carries no output, exactly as before.
            assert_eq!(
                child.final_output.as_deref(),
                if expect_stopped {
                    Some(control::STOP_MESSAGE)
                } else {
                    None
                },
                "{terminal_state:?}: {child:?}"
            );
            assert_eq!(
                child.stopped, expect_stopped,
                "{terminal_state:?}: the synthesized child's `stopped` flag must track the terminal \
                 state, not be hard-coded either way: {child:?}"
            );
            assert_eq!(
                child.interrupted, expect_interrupted,
                "{terminal_state:?}: `interrupted` is the PAUSED verdict and must never coincide \
                 with `stopped`: {child:?}"
            );

            // The downstream consequence, on the same real record: the grouped intercom verdict.
            let payload = crate::tui::intercom::IntercomPayload::from_result(&result);
            let expected = if expect_stopped {
                crate::tui::intercom::SubagentResultStatus::Stopped
            } else if expect_interrupted {
                crate::tui::intercom::SubagentResultStatus::Paused
            } else {
                crate::tui::intercom::SubagentResultStatus::Failed
            };
            assert_eq!(
                payload.status, expected,
                "{terminal_state:?}: the synthesized child's flags are what `resolveGroupedStatus` \
                 reads: {payload:?}"
            );
        }
    }
}

//! `workflowResultChildren` and its two helpers — pi
//! `runs/foreground/workflow-detach-reconcile.ts:44-95` (@v0.67.0).
//!
//! Upstream rebuilds an OPEN `WorkflowPublicChild` map. cyrup's terminal payload is the typed
//! [`ResultFile`] with `results: Vec<SingleResult>` (`background/records.rs:483`), and SCOPE_8 §0
//! is explicit that no `PublicChild` type is to be introduced — so every upstream key lands on the
//! `SingleResult` field that already carries that meaning, and the three keys with no field
//! (`workflowKey`, `sessionName`, `outputPathMapping`) are recorded at their seam below rather
//! than silently dropped.

use cyrup_core::Usage;

use crate::background::{ResultFile, RunId, RunStatus, StepState, StepStatus};
use crate::exec::SingleResult;
use crate::exec::output_state::SubagentOutputState;
use crate::workflows::{
    WorkflowBudgetSignals, WorkflowOutputPathMapping, WorkflowReceipt, WorkflowTerminalOutcome,
    WorkflowTerminalOutcomeReason, workflow_terminal_outcome_for_result,
};

/// pi `usageWithValue` (`:44-48`) — suppress an all-zero usage block so a reconciled child never
/// overwrites a previously recorded figure with zeros.
///
/// Upstream tests six named fields (`input|output|cacheRead|cacheWrite|cost|turns`) because its
/// `usage` is `Usage | undefined`. cyrup's carrier is a non-`Option` [`Usage`] value, so the
/// question "does this usage carry a value" is exactly "is it something other than
/// [`Usage::default`]" — identical observable behaviour, different representation, and the
/// `undefined` result becomes "do not overwrite the existing entry's usage".
#[must_use]
pub(crate) fn usage_with_value(usage: &Usage) -> Option<&Usage> {
    (*usage != Usage::default()).then_some(usage)
}

/// pi `requestedOutputPathFromTask` (`runs/shared/single-output.ts:110-118` @v0.67.0) — recover the
/// output path a task string was instructed to write to.
///
/// Ported here rather than left to a caller (SCOPE_8 §Y-3): the two instruction sentences this
/// parser matches are produced in-tree, verbatim, by
/// [`crate::exec::output::format_output_path_instruction`] (`exec/output.rs:1089` and the
/// read-only-agent branch immediately below it), so the parser has a real producer and the
/// alternative — pushing the derivation onto a caller that does not exist yet — would guarantee
/// the mapping is never populated.
///
/// Scans **last line first**, exactly as upstream does: a task may be re-injected with a second,
/// superseding instruction (`inject_single_output_instruction` appends), and the last one wins.
#[must_use]
pub(crate) fn requested_output_path_from_task(task: &str) -> Option<String> {
    const PREFIXES: [&str; 3] = [
        "write your findings to exactly this path:",
        "write your findings to:",
        "the runtime will persist it to exactly this path:",
    ];
    for line in task.lines().rev() {
        let trimmed = line.trim();
        let lowered = trimmed.to_ascii_lowercase();
        let Some(prefix) = PREFIXES.iter().find(|prefix| lowered.starts_with(**prefix)) else {
            continue;
        };
        let requested = trimmed
            .get(prefix.len()..)
            .unwrap_or_default()
            .trim()
            .to_string();
        if requested.is_empty() {
            continue;
        }
        // Upstream strips a single pair of wrapping backticks (`:116`) — an operator who wrote the
        // path as inline code in the task must not have the backticks become part of the path.
        return Some(
            match requested
                .strip_prefix('`')
                .and_then(|r| r.strip_suffix('`'))
            {
                Some(unquoted) if !unquoted.is_empty() => unquoted.to_string(),
                _ => requested,
            },
        );
    }
    None
}

/// pi `outputPathMappingFromTask` (`single-output.ts:120-125`) — the requested→saved remap, or
/// `None` when there is nothing to remap.
///
/// The third guard is upstream's and is the whole point: an ABSOLUTE requested path that
/// normalizes equal to the saved path is not a remap at all, and publishing it would tell an
/// operator their output moved when it did not.
#[must_use]
pub(crate) fn output_path_mapping_from_task(
    task: &str,
    saved_path: Option<&str>,
) -> Option<WorkflowOutputPathMapping> {
    let requested_path = requested_output_path_from_task(task)?;
    let saved_path = saved_path?;
    let requested = std::path::Path::new(&requested_path);
    if requested.is_absolute()
        && normalize_path(requested) == normalize_path(std::path::Path::new(saved_path))
    {
        return None;
    }
    Some(WorkflowOutputPathMapping {
        requested_path,
        saved_path: saved_path.to_string(),
    })
}

/// `path.normalize` for the one comparison above: collapse `.` and resolve `..` lexically, without
/// touching the filesystem (upstream's `path.normalize` is lexical too, so a symlink-resolving
/// `canonicalize` would be a DIFFERENT predicate and would additionally fail for a path that no
/// longer exists).
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// pi's `success` key (`:63`) — `exitCode === 0 && !error && !interrupted`.
///
/// It does **not** consult `stopped`, matching
/// [`crate::workflows::apply_detached_child_settlement`]'s own documented quirk
/// (`workflows/settlement.rs:264-270`): a stopped child that exited cleanly reads as SUCCEEDED
/// here and is caught by the `interrupted || stopped` arm only for its flags. Ported as written.
#[must_use]
fn detached_child_succeeded(result: &SingleResult) -> bool {
    result.exit_code == 0 && result.error.is_none() && !result.interrupted
}

/// The published exit code for a child whose success verdict is [`detached_child_succeeded`].
///
/// `SingleResult` has no `success` field (SCOPE_8 §0: cyrup publishes the typed result, not
/// upstream's open `WorkflowPublicChild` map), so `exit_code` IS the success carrier — it is what
/// `finish_run` (`runner_main/finish.rs:338`) and the completion classifier both read. Forcing a
/// non-zero code for a child that did not succeed keeps that equivalence exact; a child that DID
/// succeed publishes `0` even when it was stopped, which is precisely how the quirk above stays
/// observable.
fn published_exit_code(result: &SingleResult) -> i32 {
    if detached_child_succeeded(result) {
        0
    } else if result.exit_code == 0 {
        1
    } else {
        result.exit_code
    }
}

/// pi `getSingleResultOutput(result)` (`:51`) — unported upstream helper; the crate idiom is
/// `final_output.as_deref().unwrap_or("")` (`background/watch/message.rs`).
fn single_result_output(result: &SingleResult) -> &str {
    result.final_output.as_deref().unwrap_or("")
}

fn output_state_for(output: &str) -> SubagentOutputState {
    if output.trim().is_empty() {
        SubagentOutputState::Absent
    } else {
        SubagentOutputState::Present
    }
}

/// Carry a [`WorkflowTerminalOutcome`] onto the two `SingleResult` signals it was derived FROM.
///
/// `SingleResult` has no `terminal_outcome` field; the outcome is a projection of `timed_out` /
/// `turn_budget_exceeded` ([`workflow_terminal_outcome_for_result`],
/// `workflows/settlement.rs:466-484`). Writing the projection back onto its own inputs is the only
/// representation cyrup has for upstream's `...(terminalOutcome ? { terminalOutcome } : {})`
/// spread, and it round-trips: re-deriving the outcome from the published child yields the same
/// value.
fn apply_terminal_outcome(child: &mut SingleResult, outcome: &WorkflowTerminalOutcome) {
    let WorkflowTerminalOutcome::Partial { reason } = outcome;
    match reason {
        WorkflowTerminalOutcomeReason::Timeout => child.timed_out = true,
        WorkflowTerminalOutcomeReason::BudgetExhausted => child.turn_budget_exceeded = true,
    }
}

/// pi `workflowResultChildren` (`:50-95`) — the settled workflow's published child array.
///
/// **Arm A** (`existing` present, `:56-77`): the prior result file's `results` is rewritten in
/// place — only the entry whose `child_run_id` matches is touched, every sibling is preserved
/// byte-for-byte. This is what upstream's *"preserves sibling output path mappings when rebuilding
/// from workflow status"* case protects.
///
/// **Arm B** (`existing` absent, `:79-94`): the array is rebuilt from `status.steps`, with the
/// receipt supplying the `outputReference` (`:88`) and `terminalOutcome` (`:91`) of every step
/// that is NOT the settling child. Dropping those fallbacks loses sibling evidence whenever the
/// paused result file was already collected.
///
/// The join key is [`SingleResult::child_run_id`] (`exec/run_result.rs:202`), not `run_id` —
/// `SingleResult`'s run id names the CHILD this entry describes, and `run_id` on the enclosing
/// [`ResultFile`] names the workflow.
pub(crate) fn workflow_result_children(
    status: &RunStatus,
    child_run_id: &str,
    result: &SingleResult,
    existing: Option<&ResultFile>,
    receipt: Option<&WorkflowReceipt>,
) -> Vec<SingleResult> {
    let output = single_result_output(result);
    let output_reference = result.saved_output_path.clone();
    let terminal_outcome =
        workflow_terminal_outcome_for_result(WorkflowBudgetSignals::from_single_result(result));

    if let Some(existing) = existing {
        // Arm A. `Array.isArray(existingResults)` upstream; here the typed `results` vector is
        // always an array, so "an existing result file was readable" IS the arm's condition —
        // including the empty-array case, which upstream maps to an empty array too.
        return existing
            .results
            .iter()
            .map(|child| {
                if child.child_run_id.as_ref().map(RunId::as_str) != Some(child_run_id) {
                    // `:58` — every non-matching entry passes through untouched.
                    return child.clone();
                }
                let mut next = child.clone();
                next.exit_code = published_exit_code(result);
                next.final_output = Some(output.to_string());
                next.output_state = output_state_for(output);
                // `:66` — `detached: undefined`. The field is a plain `bool` here, so "clear"
                // means `false`, not "remove".
                next.detached = false;
                if usage_with_value(&result.usage).is_some() {
                    next.usage = result.usage.clone();
                }
                if let Some(reference) = &output_reference {
                    next.saved_output_path = Some(reference.clone());
                }
                if result.interrupted || result.stopped {
                    next.interrupted = true;
                }
                if result.stopped {
                    next.stopped = true;
                }
                if let Some(outcome) = &terminal_outcome {
                    apply_terminal_outcome(&mut next, outcome);
                }
                if let Some(session_file) = &result.session_file {
                    next.session_file = Some(session_file.clone());
                }
                if let Some(error) = &result.error {
                    next.error = Some(error.clone());
                }
                next
            })
            .collect();
    }

    // Arm B.
    status
        .steps
        .iter()
        .map(|step| {
            let settling = step.run_id.as_ref().map(RunId::as_str) == Some(child_run_id);
            let entry = step
                .workflow_key
                .as_ref()
                .and_then(|key| receipt.and_then(|receipt| receipt.entry(key)));
            let step_output = if settling { output } else { "" };
            let mut next = step_child(step, step_output);
            if settling {
                if usage_with_value(&result.usage).is_some() {
                    next.usage = result.usage.clone();
                }
                if let Some(session_file) = &result.session_file {
                    next.session_file = Some(session_file.clone());
                }
            }
            // `:88` — the settling child's own reference wins; otherwise the receipt entry's.
            next.saved_output_path = if settling && output_reference.is_some() {
                output_reference.clone()
            } else {
                entry.and_then(|entry| entry.output_reference.clone())
            };
            // `:91` — same precedence for the terminal outcome.
            let step_outcome = if settling && terminal_outcome.is_some() {
                terminal_outcome
            } else {
                entry.and_then(|entry| entry.terminal_outcome)
            };
            if let Some(outcome) = &step_outcome {
                apply_terminal_outcome(&mut next, outcome);
            }
            next
        })
        .collect()
}

/// One Arm-B child, built from its [`StepStatus`].
///
/// Three upstream keys have no `SingleResult` home and are recorded here rather than dropped
/// silently (SCOPE_8 §W-7's "publish only the fields that exist", extended to this function):
/// `workflowKey` — cyrup's keyed inventory is [`ResultFile::workflow_children`], which
/// `plan_workflow_settlement` stamps from the same steps; `sessionName` — carried on the step and
/// on the summary row, not on the result; `outputPathMapping` — carried on
/// [`StepStatus::output_path_mapping`], which the driver stamps and which reaches the operator
/// through [`crate::workflows::workflow_output_path_mapping_summary`].
fn step_child(step: &StepStatus, output: &str) -> SingleResult {
    SingleResult {
        execution: None,
        native_machine: None,
        runtime_acknowledged_extensions: None,
        skills_warning: None,
        watchdog: None,
        agent: step.agent.clone(),
        task: String::new(),
        // `:84` — `success: step.status === "completed" || step.status === "complete"`. cyrup has
        // ONE spelling, and `exit_code` is the success carrier (see `published_exit_code`).
        exit_code: i32::from(step.status != StepState::Complete),
        // Upstream omits `usage` for every non-settling child because its `WorkflowStatusStep`
        // carries none. cyrup's `StepStatus` carries the real per-step figure, and publishing
        // zeros where a real number exists would be a regression against a field this port
        // already maintains — so the step's own usage is used.
        usage: step.usage.clone(),
        turns: step.turns,
        model: step.model.clone(),
        attempted_models: step.attempted_models.clone(),
        model_attempts: Vec::new(),
        final_output: Some(output.to_string()),
        structured_output: None,
        acceptance: None,
        detached: false,
        detached_reason: None,
        interrupted: step.interrupted,
        timed_out: false,
        timeout_recovery: None,
        context_overflow: step.context_overflow,
        stopped: step.stopped,
        process_signal: None,
        turn_budget: None,
        turn_budget_exceeded: false,
        wrap_up_requested: false,
        usage_budget: None,
        error: step.error.clone(),
        saved_output_path: None,
        session_file: step.session_file.clone(),
        child_run_id: step.run_id.clone(),
        output_state: output_state_for(output),
        structured_output_path: None,
        artifact_paths: None,
        // The step's own live-transcript stamps, exactly as `session_file` above is the step's.
        transcript_path: step.transcript_path.clone(),
        transcript_error: step.transcript_error.clone(),
        tool_calls: Vec::new(),
        output_truncated: false,
        tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
        control_events: Vec::new(),
        progress: None,
        runner: None,
        external_process: None,
    }
}

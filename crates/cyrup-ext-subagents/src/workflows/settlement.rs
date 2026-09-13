//! The workflow settlement composer — ports pi `workflows/workflow-settlement.ts` (249 LOC): how
//! a detached/paused workflow's child settlement promotes the run, how the terminal writers
//! classify and report it, and the one function ([`plan_workflow_settlement`]) that assembles a
//! settled workflow's terminal payload.
//!
//! Not [`crate::workflows::scripted::CompletionSettlement`] (`workflows/scripted/settlement.rs`)
//! — that is a DIFFERENT concept (the run ▸ steer ▸ host unawaited-work precedence decision inside
//! `finish()`), unrelated to this module and untouched by it.

use std::path::PathBuf;

use crate::background::{
    ActivityState, RunId, RunMode, RunState, RunStatus, StepState, StepStatus,
};
use crate::exec::SingleResult;

use super::child_summary::{WorkflowChildSummaryInput, workflow_child_summary};
use super::key::WorkflowKey;
use super::receipt::{
    WorkflowReceipt, WorkflowReceiptState, WorkflowRecoveryAction, WorkflowRecoveryCall,
    WorkflowRecoveryResume, WorkflowTerminalResolution,
};
use super::types::{
    WorkflowOutputPathMapping, WorkflowScriptChildResult, WorkflowScriptTraceEntry, WorkflowState,
    WorkflowTerminalOutcome, WorkflowTerminalOutcomeReason,
};

/// pi `UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION` (`workflow-settlement.ts:11`).
///
/// The wording is the remediation instruction, not a log line: it tells the operator to resume the
/// workflow explicitly instead of treating a completed child as workflow completion. Paraphrasing
/// it removes the only actionable thing in the message.
pub const UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION: &str = "unsupported-continuation: detached \
    workflow child settled, but JavaScript workflow continuation was not persisted. Resume the \
    workflow explicitly instead of treating the completed child as top-level workflow completion.";

/// pi `INTERRUPTED_DETACHED_CHILD` (`:12`). Also a MATCHED value, not only a rendered one:
/// [`classify_workflow_settlement`] tests `candidate.error === INTERRUPTED_DETACHED_CHILD`
/// (`:158`), so a drifted copy would silently reclassify an interrupted child as
/// `settled-awaiting-resume`.
pub const INTERRUPTED_DETACHED_CHILD: &str = "Interrupted. Waiting for explicit next action.";

/// pi `EVIDENCE_PERSISTENCE_FAILED` (`:13`) — the diagnostic prefix on a receipt-write failure.
pub const EVIDENCE_PERSISTENCE_FAILED: &str = "evidence-persistence-failed";

/// A [`RunStatus`] that has already had [`with_workflow_children`] applied — pi's
/// `AsyncStatus | undefined` return shape from `promoteSettledPausedWorkflow`/
/// `applyDetachedChildSettlement` (`:101`, `:148`), given its own name here because every value of
/// this type has ALREADY been through the ONE site that stamps the settled inventory complete.
///
/// A plain type alias, not a wrapper: the distinction is for the reader (a caller does not have to
/// re-derive "has this been re-stamped yet"), not a runtime invariant the type itself enforces.
pub type SettledWorkflowStatus = RunStatus;

/// pi `workflowState` (`workflow-settlement.ts:58-60`).
///
/// Exhaustive by design: `Queued`/`Running`/`Failed` all collapse to `Failed`, which is upstream's
/// catch-all — but written out, so a seventh [`RunState`] breaks the build instead of silently
/// reporting a failure. (`RunState` has no `Partial`/`Rejected`; `background/wait.rs:729` is the
/// precedent for that collapse — cited, not re-derived.)
fn workflow_state(state: RunState) -> WorkflowState {
    match state {
        RunState::Complete => WorkflowState::Completed,
        RunState::Paused => WorkflowState::Paused,
        RunState::Stopped => WorkflowState::Stopped,
        RunState::Queued | RunState::Running | RunState::Failed => WorkflowState::Failed,
    }
}

/// `RunState -> WorkflowReceiptState` — pi's SECOND, near-identical map inside
/// `planWorkflowSettlement` (`:216`). Its OWN exhaustive `match`, never bridged through
/// [`workflow_state`]: the two enums disagree on their first word (`"complete"` vs `"completed"`),
/// so a bridge would be a silent wire-format bug no Rust type catches.
fn workflow_receipt_state(state: RunState) -> WorkflowReceiptState {
    match state {
        RunState::Complete => WorkflowReceiptState::Complete,
        RunState::Paused => WorkflowReceiptState::Paused,
        RunState::Stopped => WorkflowReceiptState::Stopped,
        RunState::Queued | RunState::Running | RunState::Failed => WorkflowReceiptState::Failed,
    }
}

/// pi `withWorkflowChildren` (`:62-74`) — the **only** site that stamps the inventory-complete
/// flag true. Every in-flight stamp elsewhere passes `false`; this flag is the reader's signal
/// that the child list is final.
///
/// Takes the trace explicitly (`status.workflow?.trace` has no [`RunStatus`] home): WORKFLOW_2's
/// engine returns `WorkflowScriptResult::trace` complete on both arms, so the caller always holds
/// it and this never needs to be threaded through `RunStatus` itself.
///
/// # Errors
///
/// [`workflow_child_summary`]'s own `requiredId` rejection — unreachable in practice (the inputs
/// are the run's own `run_id`/`tool_call_id`), but propagated rather than unwrapped; the ONE
/// caller that can reach the top of a public API ([`plan_workflow_settlement`]) degrades to a
/// `Failed` status carrying the message rather than panicking.
pub fn with_workflow_children(
    status: &RunStatus,
    trace: &[WorkflowScriptTraceEntry],
) -> Result<RunStatus, String> {
    let mut next = status.clone();
    let parent_tool_call_id = status
        .tool_call_id
        .clone()
        .unwrap_or_else(|| status.run_id.as_str().to_string());
    let summary = workflow_child_summary(WorkflowChildSummaryInput {
        parent_tool_call_id: &parent_tool_call_id,
        workflow_run_id: status.run_id.as_str(),
        workflow_state: workflow_state(status.state),
        inventory_complete: true,
        trace,
        steps: &status.steps,
        children: &[],
        progress: &[],
    })
    .map_err(|e| e.message().to_string())?;
    next.workflow_children = Some(summary);
    Ok(next)
}

/// pi `findWorkflowSettlementStep` (`workflow-settlement.ts:76-84`).
///
/// Exact `runId` first; only if that misses **and both `workflow_key` and `session_file` were
/// supplied** does it fall back to matching un-launched steps by key — dropping either
/// precondition widens the fallback to claim a step the caller could not actually identify.
/// `candidates.len() == 1` is a THIRD guard: two same-key un-launched steps resolve to nothing,
/// never the first.
///
/// Returns an INDEX, not `&mut StepStatus`: the two callers mutate the step and then read
/// siblings, which is a borrow conflict over a mutable reference but not over an index.
#[must_use]
pub fn find_workflow_settlement_step(
    steps: &[StepStatus],
    child_run_id: &str,
    workflow_key: Option<&WorkflowKey>,
    session_file: Option<&std::path::Path>,
) -> Option<usize> {
    let exact = steps
        .iter()
        .position(|candidate| candidate.run_id.as_ref().map(RunId::as_str) == Some(child_run_id));
    if exact.is_some() {
        return exact;
    }
    let (Some(workflow_key), Some(session_file)) = (workflow_key, session_file) else {
        return None;
    };
    let mut candidates = steps.iter().enumerate().filter(|(_, candidate)| {
        candidate.run_id.is_none()
            && candidate.workflow_key.as_ref() == Some(workflow_key)
            && candidate.session_file.as_deref() == Some(session_file)
    });
    let first = candidates.next()?;
    if candidates.next().is_none() {
        Some(first.0)
    } else {
        None
    }
}

/// pi `promoteSettledPausedWorkflow` (`workflow-settlement.ts:85-101`).
///
/// # Errors
///
/// See [`with_workflow_children`].
pub fn promote_settled_paused_workflow(
    status: &RunStatus,
    trace: &[WorkflowScriptTraceEntry],
    now: i64,
) -> Result<Option<SettledWorkflowStatus>, String> {
    if status.mode != RunMode::Workflow || status.state != RunState::Paused {
        // Paused-gated: only a detached workflow reaches Paused (§0.6 cut the async runner;
        // §4 lists this as still dead). Pointer: §0.6, §4.
        return Ok(None);
    }
    let mut next = status.clone();
    let still_open = next.steps.iter().any(|candidate| {
        candidate.status == StepState::Running
            || (candidate.status == StepState::Paused
                && candidate.telemetry.activity_state == Some(ActivityState::NeedsAttention))
    });
    if still_open || next.steps.is_empty() {
        return Ok(None);
    }
    let failed = next
        .steps
        .iter()
        .any(|candidate| candidate.status == StepState::Failed);
    // Force the terminal transition directly, mirroring `finish_run`'s own rationale
    // (`background/runner_main/finish.rs`): this is the authoritative "this workflow settled"
    // write, not a normal forward-progress transition subject to the ordinary transition guard.
    next.last_update = now;
    next.state = RunState::Failed;
    next.ended_at = Some(now);
    next.telemetry.activity_state = None;
    if !failed {
        next.error = Some(UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION.to_string());
    }
    with_workflow_children(&next, trace).map(Some)
}

/// The settled-child result [`apply_detached_child_settlement`] applies — pi's inline `input`
/// object (`workflow-settlement.ts:103-104`).
pub struct DetachedChildSettlement<'a> {
    /// The child run this settlement is for.
    pub child_run_id: &'a str,
    /// The child's exit code, when known.
    pub exit_code: Option<i32>,
    /// The child's error text, when it failed.
    pub error: Option<&'a str>,
    /// The child was interrupted mid-flight.
    pub interrupted: bool,
    /// The child's persisted session-transcript path, when resolved.
    pub session_file: Option<&'a std::path::Path>,
    /// The child's display session name, when derived.
    pub session_name: Option<&'a str>,
    /// The child was stopped by an explicit stop request.
    pub stopped: bool,
    /// The workflow lane key this settlement is scoped to, for the by-key fallback rung.
    pub workflow_key: Option<&'a WorkflowKey>,
    /// Wall-clock time to stamp onto every mutation this call makes.
    pub now: i64,
}

/// Which error a promoted-to-failed workflow reports — pi's four-arm chain (`:143-147`).
///
/// A decision enum rather than four `else if`s: the arms are mutually exclusive, the precedence is
/// load-bearing, and the caller has one `match` to execute.
enum PromotedWorkflowError<'a> {
    /// A sibling step failed for a reason of its own (`:143`).
    FailedSibling(&'a str),
    /// This child was interrupted or stopped (`:144`) — falls back to
    /// [`INTERRUPTED_DETACHED_CHILD`] when the child carried no error.
    ThisChildInterrupted(Option<&'a str>),
    /// This child failed with an error (`:145`).
    ThisChild(&'a str),
    /// Some other step is a recorded interrupted child (`:146`).
    InterruptedSibling(&'a str),
    /// None of the four — keep whatever [`promote_settled_paused_workflow`] set.
    Keep,
}

fn promoted_workflow_error<'a>(
    failed_sibling_error: Option<&'a str>,
    this_child_interrupted_or_stopped: bool,
    this_child_error: Option<&'a str>,
    interrupted_sibling_error: Option<&'a str>,
) -> PromotedWorkflowError<'a> {
    if let Some(error) = failed_sibling_error {
        PromotedWorkflowError::FailedSibling(error)
    } else if this_child_interrupted_or_stopped {
        PromotedWorkflowError::ThisChildInterrupted(this_child_error)
    } else if let Some(error) = this_child_error {
        PromotedWorkflowError::ThisChild(error)
    } else if let Some(error) = interrupted_sibling_error {
        PromotedWorkflowError::InterruptedSibling(error)
    } else {
        PromotedWorkflowError::Keep
    }
}

/// pi `applyDetachedChildSettlement` (`workflow-settlement.ts:102-148`) — the order is the
/// behaviour: both sibling-error scans happen BEFORE the step is mutated, and they differ
/// (`failedSiblingError` excludes the settling step and requires `!interrupted`;
/// `interruptedChildError` does not exclude it and requires `interrupted`).
///
/// The `succeeded` predicate is `exitCode === 0 && !error && !interrupted` — it does NOT consult
/// `stopped`, so a stopped child with `exitCode: 0` and no error reads as succeeded here and is
/// caught by the `interrupted || stopped` arm only for its error text. Ported as written.
///
/// # Errors
///
/// See [`with_workflow_children`].
pub fn apply_detached_child_settlement(
    status: &RunStatus,
    input: DetachedChildSettlement<'_>,
    trace: &[WorkflowScriptTraceEntry],
) -> Result<Option<SettledWorkflowStatus>, String> {
    if status.mode != RunMode::Workflow || status.state != RunState::Paused {
        // Paused-gated: only a detached workflow reaches Paused (§0.6 cut the async runner;
        // §4 lists this as still dead). Pointer: §0.6, §4.
        return Ok(None);
    }
    let mut next = status.clone();
    let Some(step_index) = find_workflow_settlement_step(
        &next.steps,
        input.child_run_id,
        input.workflow_key,
        input.session_file,
    ) else {
        return Ok(None);
    };

    // Both sibling scans BEFORE the step is mutated (`:110-116`).
    let failed_sibling_error = next
        .steps
        .iter()
        .enumerate()
        .find(|(index, candidate)| {
            *index != step_index && candidate.status == StepState::Failed && !candidate.interrupted
        })
        .and_then(|(_, candidate)| candidate.error.clone());
    let interrupted_child_error = next
        .steps
        .iter()
        .find(|candidate| candidate.status == StepState::Failed && candidate.interrupted)
        .and_then(|candidate| candidate.error.clone());

    let succeeded = input.exit_code == Some(0) && input.error.is_none() && !input.interrupted;
    let now = input.now;

    if let Some(step) = next.steps.get_mut(step_index) {
        if step.run_id.is_none() {
            step.run_id = Some(RunId::from_token(input.child_run_id.to_string()));
        }
        step.status = if succeeded {
            StepState::Complete
        } else {
            StepState::Failed
        };
        step.ended_at = Some(now);
        step.telemetry.activity_state = None;
        step.telemetry.current_tool = None;
        step.telemetry.current_tool_started_at = None;
        if let Some(session_file) = input.session_file {
            step.session_file = Some(session_file.to_path_buf());
        }
        if let Some(session_name) = input.session_name {
            step.session_name = Some(session_name.to_string());
        }
        if succeeded {
            step.error = None;
            step.interrupted = false;
        } else if input.interrupted || input.stopped {
            step.error = Some(
                input
                    .error
                    .map(str::to_string)
                    .unwrap_or_else(|| INTERRUPTED_DETACHED_CHILD.to_string()),
            );
            step.interrupted = true;
            if input.stopped {
                step.stopped = true;
            }
        } else {
            step.interrupted = false;
            step.stopped = false;
            if let Some(error) = input.error {
                step.error = Some(error.to_string());
            }
        }
    }
    next.last_update = now;

    let promoted = promote_settled_paused_workflow(&next, trace, now)?;
    let settled = if let Some(mut promoted_status) = promoted {
        if promoted_status.state == RunState::Failed {
            promoted_status.error = match promoted_workflow_error(
                failed_sibling_error.as_deref(),
                input.interrupted || input.stopped,
                input.error,
                interrupted_child_error.as_deref(),
            ) {
                PromotedWorkflowError::FailedSibling(error)
                | PromotedWorkflowError::ThisChild(error)
                | PromotedWorkflowError::InterruptedSibling(error) => Some(error.to_string()),
                PromotedWorkflowError::ThisChildInterrupted(error) => Some(
                    error
                        .map(str::to_string)
                        .unwrap_or_else(|| INTERRUPTED_DETACHED_CHILD.to_string()),
                ),
                PromotedWorkflowError::Keep => promoted_status.error.clone(),
            };
        }
        promoted_status
    } else {
        next
    };
    // pi calls `withWorkflowChildren` again here regardless of the `promoted` branch (`:148`) —
    // a second, harmless re-stamp when `promoted` is `Some` (the summary is deterministic and
    // `.error` does not feed it). Ported as written, not "optimized" away.
    with_workflow_children(&settled, trace).map(Some)
}

/// pi `classifyWorkflowSettlement` (`workflow-settlement.ts:149-161`).
#[must_use]
pub fn classify_workflow_settlement(
    status: &RunStatus,
    interrupted: bool,
) -> Option<WorkflowTerminalResolution> {
    if status.state != RunState::Complete && status.state != RunState::Failed {
        return None;
    }
    if status
        .steps
        .iter()
        .any(|step| step.status == StepState::Failed && !step.interrupted)
    {
        return Some(WorkflowTerminalResolution::FailedChild);
    }
    if interrupted
        || status.steps.iter().any(|step| {
            step.status == StepState::Stopped
                || step.stopped
                || step.interrupted
                || step.error.as_deref() == Some(INTERRUPTED_DETACHED_CHILD)
        })
    {
        return Some(WorkflowTerminalResolution::InterruptedChild);
    }
    Some(WorkflowTerminalResolution::SettledAwaitingResume)
}

/// pi `workflowRecoveryActions` (`workflow-settlement.ts:162-168`) — the ONE sanctioned producer
/// of [`WorkflowRecoveryAction`]s; [`super::receipt`]'s own reader re-validates every one it reads
/// back against this same predicate (`entry.resume.is_resumable()`).
#[must_use]
pub fn workflow_recovery_actions(receipt: Option<&WorkflowReceipt>) -> Vec<WorkflowRecoveryAction> {
    let Some(receipt) = receipt else {
        return Vec::new();
    };
    receipt
        .entries
        .iter()
        .filter(|entry| entry.resume.is_resumable())
        .map(|entry| WorkflowRecoveryAction {
            key: entry.key.clone(),
            call: WorkflowRecoveryCall::RunsRun,
            resume: WorkflowRecoveryResume {
                workflow_run_id: receipt.workflow_run_id.clone(),
                key: entry.key.clone(),
                latest: true,
            },
            task_required: true,
        })
        .collect()
}

/// The three budget/deadline signals `workflowTerminalOutcomeForResult` reads
/// (`workflow-settlement.ts:169-174`). A named input rather than `&SingleResult`, because
/// `tool_budget_blocked` has NO carrier on [`SingleResult`] yet: the crate does block on a tool
/// budget but never reports it in the result. Landing that field later is then a change at the two
/// call sites, not to this signature.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkflowBudgetSignals {
    /// pi `result.timedOut` — [`SingleResult::timed_out`].
    pub timed_out: bool,
    /// pi `result.turnBudgetExceeded` — [`SingleResult::turn_budget_exceeded`].
    pub turn_budget_exceeded: bool,
    /// pi `result.toolBudgetBlocked` — no carrier yet; always `false` from
    /// [`Self::from_single_result`].
    pub tool_budget_blocked: bool,
}

impl WorkflowBudgetSignals {
    /// Reads the two signals [`SingleResult`] carries today.
    #[must_use]
    pub fn from_single_result(result: &SingleResult) -> Self {
        Self {
            timed_out: result.timed_out,
            turn_budget_exceeded: result.turn_budget_exceeded,
            tool_budget_blocked: false,
        }
    }
}

/// pi `workflowTerminalOutcomeForResult` (`workflow-settlement.ts:169-174`). Precedence:
/// `timed_out ⇒ timeout`, else `turn_budget_exceeded || tool_budget_blocked ⇒ budget_exhausted`,
/// else `None`.
#[must_use]
pub fn workflow_terminal_outcome_for_result(
    signals: WorkflowBudgetSignals,
) -> Option<WorkflowTerminalOutcome> {
    if signals.timed_out {
        Some(WorkflowTerminalOutcome::Partial {
            reason: WorkflowTerminalOutcomeReason::Timeout,
        })
    } else if signals.turn_budget_exceeded || signals.tool_budget_blocked {
        Some(WorkflowTerminalOutcome::Partial {
            reason: WorkflowTerminalOutcomeReason::BudgetExhausted,
        })
    } else {
        None
    }
}

/// pi `workflowOutputPathMappingSummary` — BOTH upstream copies
/// (`workflow-settlement.ts:175-185` over an untyped list, and
/// `runs/foreground/subagent-executor.ts:4308-4318` over `WorkflowScriptChildResult[]`).
/// Identical output; the only difference is where the key comes from, so it is a parameter here.
///
/// Leading space and trailing period are upstream's — the string is CONCATENATED onto a summary,
/// so dropping either changes the rendered sentence.
#[must_use]
pub fn workflow_output_path_mapping_summary<'a>(
    mappings: impl IntoIterator<Item = (Option<&'a str>, &'a WorkflowOutputPathMapping)>,
) -> String {
    let rendered: Vec<String> = mappings
        .into_iter()
        .map(|(key, mapping)| {
            format!(
                "'{}': requested {} -> saved {}",
                key.unwrap_or("child"),
                mapping.requested_path,
                mapping.saved_path
            )
        })
        .collect();
    if rendered.is_empty() {
        String::new()
    } else {
        format!(" Output path mappings: {}.", rendered.join("; "))
    }
}

/// Adapter for the script-result list: the key is always present (`child.key`), so the `"child"`
/// fallback inside [`workflow_output_path_mapping_summary`] is never reached through this adapter
/// — it belongs only to a future untyped-list adapter, where a caller can have a mapping with no
/// key.
pub fn output_path_mappings_of(
    children: &[WorkflowScriptChildResult],
) -> impl Iterator<Item = (Option<&str>, &WorkflowOutputPathMapping)> {
    children.iter().filter_map(|child| {
        child
            .output_path_mapping
            .as_ref()
            .map(|mapping| (Some(child.key.as_str()), mapping))
    })
}

/// The `subagent.workflow.completed` completion event a terminal [`WorkflowSettlementPlan`]
/// emits — pi's inline object literal (`workflow-settlement.ts:240-248`). A typed struct, not a
/// bare map: the six computed keys (`type`, `state`, `workflowResolution`, `terminalOutcome`,
/// `error`, `activityState`) are known and `type` is matched downstream — but
/// `input.eventMetadata` is spread LAST upstream (`:247`), so a caller key can override a computed
/// one; [`Self::extra`] is merged last for the identical reason, via a hand-written `Serialize`.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowCompletionEvent {
    /// The settled run state.
    pub state: RunState,
    /// The three-word resolution, when one applies.
    pub workflow_resolution: Option<WorkflowTerminalResolution>,
    /// The partial-terminal outcome the caller supplied.
    pub terminal_outcome: Option<WorkflowTerminalOutcome>,
    /// The settled run's own error, when any.
    pub error: Option<String>,
    /// The settled run's activity state, when any.
    pub activity_state: Option<ActivityState>,
    /// The caller's own event metadata, merged LAST so a caller key can override a computed one.
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl serde::Serialize for WorkflowCompletionEvent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        let mut map = serde_json::Map::new();
        map.insert(
            "type".to_string(),
            serde_json::json!("subagent.workflow.completed"),
        );
        map.insert(
            "state".to_string(),
            serde_json::to_value(self.state).map_err(S::Error::custom)?,
        );
        if let Some(resolution) = self.workflow_resolution {
            map.insert(
                "workflowResolution".to_string(),
                serde_json::to_value(resolution).map_err(S::Error::custom)?,
            );
        }
        if let Some(outcome) = &self.terminal_outcome {
            map.insert(
                "terminalOutcome".to_string(),
                serde_json::to_value(outcome).map_err(S::Error::custom)?,
            );
        }
        if let Some(error) = &self.error {
            map.insert("error".to_string(), serde_json::json!(error));
        }
        if let Some(activity_state) = self.activity_state {
            map.insert(
                "activityState".to_string(),
                serde_json::to_value(activity_state).map_err(S::Error::custom)?,
            );
        }
        for (key, value) in &self.extra {
            map.insert(key.clone(), value.clone());
        }
        map.serialize(serializer)
    }
}

/// The input to [`plan_workflow_settlement`] — pi `planWorkflowSettlement`'s single argument
/// (`workflow-settlement.ts:186-197`), narrowed to what THIS composer decides: `baseResult`/
/// `children` stay the caller's (§2), since cyrup's terminal payload is the TYPED
/// [`crate::background::ResultFile`], not an open `publicResult` map to splat.
pub struct PlanWorkflowSettlement<'a> {
    /// The status entering settlement.
    pub status: &'a RunStatus,
    /// The run summary text.
    pub summary: String,
    /// The script's execution trace, for [`with_workflow_children`]'s inventory pass.
    pub trace: &'a [WorkflowScriptTraceEntry],
    /// The receipt built for this settlement, when persistence succeeded.
    pub receipt: Option<WorkflowReceipt>,
    /// Where the receipt was written, when it was.
    pub receipt_path: Option<PathBuf>,
    /// Set when the receipt failed to persist — promotes the status to `Failed` and discards
    /// `receipt` (`:204-210`).
    pub receipt_persistence_error: Option<String>,
    /// The three-word resolution, when the caller already determined one.
    pub resolution: Option<WorkflowTerminalResolution>,
    /// The partial-terminal outcome, when the run ended that way.
    pub terminal_outcome: Option<WorkflowTerminalOutcome>,
    /// Overrides the crate's own clock; `None` uses [`crate::time::now_epoch_millis`].
    pub now: Option<i64>,
    /// Extra completion-event fields, merged last.
    pub event_metadata: serde_json::Map<String, serde_json::Value>,
}

/// pi `WorkflowSettlementPlan` (`workflow-settlement.ts:40-46`), with `publicResult` typed.
///
/// Upstream's `publicResult: Record<string, unknown>` is an open JSON object because its writer
/// (`writeAsyncResultFile(resultPath, plan.publicResult)`) takes one. cyrup's terminal payload is
/// the TYPED [`crate::background::ResultFile`], which is what a later wait-projector reads — so
/// this plan returns the fields to stamp rather than a map to splat, and `baseResult` stays the
/// caller's (it is the previous payload plus run identity, none of which this composer decides).
pub struct WorkflowSettlementPlan {
    /// The settled status: state/error/ended_at/last_update/activity_state/workflow_children/
    /// workflow_receipt_path all applied.
    pub status: RunStatus,
    /// `success` — `state == Complete` (`:223`).
    pub success: bool,
    /// The run summary (`:225`).
    pub summary: String,
    /// `error` — `None` when complete, else the status error or the summary (`:226`).
    pub error: Option<String>,
    /// The rebuilt receipt, or `None` when persistence failed (`:208`).
    pub receipt: Option<WorkflowReceipt>,
    /// The receipt's path, present only when a receipt was actually written (`:214`).
    pub receipt_path: Option<PathBuf>,
    /// Derived from the receipt; empty when there is none (`:212`).
    pub recovery: Vec<WorkflowRecoveryAction>,
    /// The three-word resolution, when one applies.
    pub resolution: Option<WorkflowTerminalResolution>,
    /// The partial-terminal outcome the caller supplied (`:234`).
    pub terminal_outcome: Option<WorkflowTerminalOutcome>,
    /// `Some` iff `status.state.is_terminal()` (`:237`, `:240`).
    pub completion_event: Option<WorkflowCompletionEvent>,
}

/// pi `planWorkflowSettlement` (`workflow-settlement.ts:186-249`) — the composer.
#[must_use]
pub fn plan_workflow_settlement(input: PlanWorkflowSettlement<'_>) -> WorkflowSettlementPlan {
    let now = input.now.unwrap_or_else(crate::time::now_epoch_millis);
    let mut status = match with_workflow_children(input.status, input.trace) {
        Ok(status) => status,
        Err(message) => {
            // Unreachable in practice (the run's own ids are always valid) — degrades the run
            // rather than panicking, per this module's own doc on `with_workflow_children`.
            let mut degraded = input.status.clone();
            degraded.state = RunState::Failed;
            degraded.error = Some(message);
            degraded.ended_at = Some(now);
            degraded.last_update = now;
            degraded
        }
    };
    let mut summary = input.summary;
    let mut receipt = input.receipt;
    let mut resolution = input.resolution;

    if let Some(receipt_persistence_error) = &input.receipt_persistence_error {
        let diagnostic = format!("{EVIDENCE_PERSISTENCE_FAILED}: {receipt_persistence_error}");
        let mut failed_status = status.clone();
        failed_status.state = RunState::Failed;
        failed_status.error = Some(diagnostic.clone());
        failed_status.telemetry.activity_state = None;
        failed_status.ended_at = Some(now);
        failed_status.last_update = now;
        status = with_workflow_children(&failed_status, input.trace).unwrap_or(failed_status);
        summary = format!(
            "{diagnostic} Available child evidence was preserved, but workflow completion was \
             not accepted."
        );
        receipt = None;
        if resolution.is_none() {
            resolution = classify_workflow_settlement(&status, false);
        }
    }

    let recovery = workflow_recovery_actions(receipt.as_ref());
    status.workflow_receipt_path = None;
    if receipt.is_some() && input.receipt_path.is_some() {
        status.workflow_receipt_path = input.receipt_path.clone();
    }
    if let Some(existing_receipt) = receipt {
        let mut next_receipt = existing_receipt;
        next_receipt.state = workflow_receipt_state(status.state);
        next_receipt.workflow_children = status.workflow_children.clone();
        if let Some(resolution) = resolution {
            next_receipt.workflow_resolution = Some(resolution);
            next_receipt.recovery = Some(recovery.clone());
        }
        receipt = Some(next_receipt);
    }

    let success = status.state == RunState::Complete;
    let error = if success {
        None
    } else {
        status.error.clone().or_else(|| Some(summary.clone()))
    };
    // cyrup has no `Partial` `RunState`, so `terminal` (upstream's `complete | failed | partial |
    // stopped`) is exactly `RunState::is_terminal()` (`Complete | Failed | Stopped`) — reuse it,
    // cite `background/wait.rs:729` for the collapse rather than re-deriving it.
    let terminal = status.state.is_terminal();
    // Built conditionally, never inserted-then-removed (`:238`'s `delete` dissolves into `None`
    // simply never being set).
    let receipt_path = status.workflow_receipt_path.clone();

    let completion_event = terminal.then(|| WorkflowCompletionEvent {
        state: status.state,
        workflow_resolution: resolution,
        terminal_outcome: input.terminal_outcome,
        error: status.error.clone(),
        activity_state: status.telemetry.activity_state,
        extra: input.event_metadata,
    });

    WorkflowSettlementPlan {
        status,
        success,
        summary,
        error,
        receipt,
        receipt_path,
        recovery,
        resolution,
        terminal_outcome: input.terminal_outcome,
        completion_event,
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
    use crate::workflows::WorkflowKey as Key;

    fn key(raw: &str) -> Key {
        Key::parse(raw).expect("valid key")
    }

    fn workflow_status(state: RunState, steps: Vec<StepStatus>) -> RunStatus {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Workflow, Some(1));
        status.state = state;
        status.steps = steps;
        status
    }

    #[test]
    fn workflow_state_maps_every_run_state_and_collapses_the_catch_all() {
        assert_eq!(workflow_state(RunState::Complete), WorkflowState::Completed);
        assert_eq!(workflow_state(RunState::Paused), WorkflowState::Paused);
        assert_eq!(workflow_state(RunState::Stopped), WorkflowState::Stopped);
        for other in [RunState::Queued, RunState::Running, RunState::Failed] {
            assert_eq!(workflow_state(other), WorkflowState::Failed);
        }
    }

    #[test]
    fn workflow_receipt_state_disagrees_with_workflow_state_on_the_settled_word() {
        assert_eq!(
            workflow_receipt_state(RunState::Complete),
            WorkflowReceiptState::Complete
        );
        assert_ne!(
            serde_json::to_value(workflow_receipt_state(RunState::Complete)).expect("ser"),
            serde_json::to_value(workflow_state(RunState::Complete)).expect("ser"),
            "one letter apart: complete vs completed"
        );
    }

    #[test]
    fn find_workflow_settlement_step_matches_by_run_id_first() {
        let mut steps = vec![StepStatus::pending("a"), StepStatus::pending("b")];
        steps[1].run_id = Some(RunId::from_token("child-1"));
        assert_eq!(
            find_workflow_settlement_step(&steps, "child-1", None, None),
            Some(1)
        );
    }

    #[test]
    fn find_workflow_settlement_step_falls_back_by_key_and_session_file_only_when_both_supplied() {
        let path = std::path::Path::new("/tmp/session.jsonl");
        let mut steps = vec![StepStatus::pending("a")];
        steps[0].workflow_key = Some(key("lane.a"));
        steps[0].session_file = Some(path.to_path_buf());
        assert_eq!(
            find_workflow_settlement_step(&steps, "unrelated-run", Some(&key("lane.a")), Some(path)),
            Some(0)
        );
        // Missing either precondition must not fall back.
        assert_eq!(
            find_workflow_settlement_step(&steps, "unrelated-run", Some(&key("lane.a")), None),
            None
        );
        assert_eq!(
            find_workflow_settlement_step(&steps, "unrelated-run", None, Some(path)),
            None
        );
    }

    #[test]
    fn find_workflow_settlement_step_resolves_two_same_key_candidates_to_nothing() {
        let path = std::path::Path::new("/tmp/session.jsonl");
        let mut steps = vec![StepStatus::pending("a"), StepStatus::pending("b")];
        for step in &mut steps {
            step.workflow_key = Some(key("lane.a"));
            step.session_file = Some(path.to_path_buf());
        }
        assert_eq!(
            find_workflow_settlement_step(&steps, "unrelated-run", Some(&key("lane.a")), Some(path)),
            None,
            "two candidates resolve to nothing, never the first"
        );
    }

    #[test]
    fn find_workflow_settlement_step_never_claims_a_step_that_already_has_a_run_id() {
        let path = std::path::Path::new("/tmp/session.jsonl");
        let mut steps = vec![StepStatus::pending("a")];
        steps[0].workflow_key = Some(key("lane.a"));
        steps[0].session_file = Some(path.to_path_buf());
        steps[0].run_id = Some(RunId::from_token("already-launched"));
        assert_eq!(
            find_workflow_settlement_step(&steps, "unrelated-run", Some(&key("lane.a")), Some(path)),
            None
        );
    }

    #[test]
    fn promote_settled_paused_workflow_ignores_a_non_workflow_or_non_paused_run() {
        let single = workflow_status(RunState::Paused, vec![StepStatus::pending("a")]);
        let mut single = single;
        single.mode = RunMode::Single;
        assert_eq!(
            promote_settled_paused_workflow(&single, &[], 0).expect("ok"),
            None
        );

        let running = workflow_status(RunState::Running, vec![StepStatus::pending("a")]);
        assert_eq!(
            promote_settled_paused_workflow(&running, &[], 0).expect("ok"),
            None
        );
    }

    #[test]
    fn promote_settled_paused_workflow_stays_none_while_a_step_is_still_open() {
        let mut still_running = StepStatus::pending("a");
        still_running.status = StepState::Running;
        let status = workflow_status(RunState::Paused, vec![still_running]);
        assert_eq!(
            promote_settled_paused_workflow(&status, &[], 0).expect("ok"),
            None
        );
    }

    #[test]
    fn promote_settled_paused_workflow_promotes_to_failed_with_the_continuation_message() {
        let mut settled = StepStatus::pending("a");
        settled.status = StepState::Complete;
        let status = workflow_status(RunState::Paused, vec![settled]);
        let promoted = promote_settled_paused_workflow(&status, &[], 1_000)
            .expect("ok")
            .expect("promoted");
        assert_eq!(promoted.state, RunState::Failed);
        assert_eq!(
            promoted.error.as_deref(),
            Some(UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION)
        );
        assert!(promoted.workflow_children.is_some(), "stamped inventory");
    }

    #[test]
    fn promote_settled_paused_workflow_does_not_set_the_continuation_message_when_a_step_failed() {
        let mut failed_step = StepStatus::pending("a");
        failed_step.status = StepState::Failed;
        failed_step.error = Some("boom".to_string());
        let status = workflow_status(RunState::Paused, vec![failed_step]);
        let promoted = promote_settled_paused_workflow(&status, &[], 1_000)
            .expect("ok")
            .expect("promoted");
        assert_eq!(promoted.state, RunState::Failed);
        assert_eq!(
            promoted.error, None,
            "the continuation message is only for an all-succeeded settlement"
        );
    }

    #[test]
    fn apply_detached_child_settlement_promotes_on_the_last_open_child() {
        let mut step = StepStatus::pending("a");
        step.status = StepState::Running;
        step.run_id = Some(RunId::from_token("child-1"));
        let status = workflow_status(RunState::Paused, vec![step]);
        let settled = apply_detached_child_settlement(
            &status,
            DetachedChildSettlement {
                child_run_id: "child-1",
                exit_code: Some(0),
                error: None,
                interrupted: false,
                session_file: None,
                session_name: None,
                stopped: false,
                workflow_key: None,
                now: 1_000,
            },
            &[],
        )
        .expect("ok")
        .expect("settled");
        assert_eq!(settled.state, RunState::Failed, "promoted, all-succeeded");
        assert_eq!(
            settled.error.as_deref(),
            Some(UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION)
        );
        assert_eq!(settled.steps[0].status, StepState::Complete);
    }

    #[test]
    fn apply_detached_child_settlement_prefers_a_failed_sibling_error_over_this_childs_own() {
        let mut sibling = StepStatus::pending("sibling");
        sibling.status = StepState::Failed;
        sibling.error = Some("sibling boom".to_string());
        let mut step = StepStatus::pending("a");
        step.status = StepState::Running;
        step.run_id = Some(RunId::from_token("child-1"));
        let status = workflow_status(RunState::Paused, vec![sibling, step]);
        let settled = apply_detached_child_settlement(
            &status,
            DetachedChildSettlement {
                child_run_id: "child-1",
                exit_code: Some(1),
                error: Some("this child's own error"),
                interrupted: false,
                session_file: None,
                session_name: None,
                stopped: false,
                workflow_key: None,
                now: 1_000,
            },
            &[],
        )
        .expect("ok")
        .expect("settled");
        assert_eq!(settled.error.as_deref(), Some("sibling boom"));
    }

    #[test]
    fn apply_detached_child_settlement_returns_none_when_the_step_cannot_be_found() {
        let step = StepStatus::pending("a");
        let status = workflow_status(RunState::Paused, vec![step]);
        assert_eq!(
            apply_detached_child_settlement(
                &status,
                DetachedChildSettlement {
                    child_run_id: "no-such-child",
                    exit_code: Some(0),
                    error: None,
                    interrupted: false,
                    session_file: None,
                    session_name: None,
                    stopped: false,
                    workflow_key: None,
                    now: 0,
                },
                &[],
            )
            .expect("ok"),
            None
        );
    }

    #[test]
    fn classify_workflow_settlement_prefers_failed_child_over_interrupted() {
        let mut failed = StepStatus::pending("a");
        failed.status = StepState::Failed;
        let mut interrupted = StepStatus::pending("b");
        interrupted.status = StepState::Stopped;
        let status = workflow_status(RunState::Failed, vec![failed, interrupted]);
        assert_eq!(
            classify_workflow_settlement(&status, false),
            Some(WorkflowTerminalResolution::FailedChild)
        );
    }

    #[test]
    fn classify_workflow_settlement_is_none_for_a_non_terminal_state() {
        let status = workflow_status(RunState::Running, vec![]);
        assert_eq!(classify_workflow_settlement(&status, false), None);
    }

    #[test]
    fn classify_workflow_settlement_matches_the_interrupted_sentinel_by_value() {
        let mut interrupted_error = StepStatus::pending("a");
        interrupted_error.status = StepState::Complete;
        interrupted_error.error = Some(INTERRUPTED_DETACHED_CHILD.to_string());
        let status = workflow_status(RunState::Complete, vec![interrupted_error]);
        assert_eq!(
            classify_workflow_settlement(&status, false),
            Some(WorkflowTerminalResolution::InterruptedChild)
        );
    }

    #[test]
    fn workflow_terminal_outcome_for_result_precedence_is_timeout_then_budget() {
        assert_eq!(
            workflow_terminal_outcome_for_result(WorkflowBudgetSignals {
                timed_out: true,
                turn_budget_exceeded: true,
                tool_budget_blocked: true,
            }),
            Some(WorkflowTerminalOutcome::Partial {
                reason: WorkflowTerminalOutcomeReason::Timeout
            })
        );
        assert_eq!(
            workflow_terminal_outcome_for_result(WorkflowBudgetSignals {
                timed_out: false,
                turn_budget_exceeded: true,
                tool_budget_blocked: false,
            }),
            Some(WorkflowTerminalOutcome::Partial {
                reason: WorkflowTerminalOutcomeReason::BudgetExhausted
            })
        );
        assert_eq!(
            workflow_terminal_outcome_for_result(WorkflowBudgetSignals::default()),
            None
        );
    }

    #[test]
    fn workflow_output_path_mapping_summary_is_empty_string_when_there_is_nothing_to_report() {
        assert_eq!(workflow_output_path_mapping_summary(std::iter::empty()), "");
    }

    #[test]
    fn workflow_output_path_mapping_summary_renders_every_mapping() {
        let mapping = WorkflowOutputPathMapping {
            requested_path: "requested.txt".to_string(),
            saved_path: "saved.txt".to_string(),
        };
        let rendered = workflow_output_path_mapping_summary([(Some("a"), &mapping)]);
        assert_eq!(
            rendered,
            " Output path mappings: 'a': requested requested.txt -> saved saved.txt."
        );
    }

    #[test]
    fn completion_event_extra_overrides_a_computed_key() {
        let mut extra = serde_json::Map::new();
        extra.insert("state".to_string(), serde_json::json!("overridden"));
        let event = WorkflowCompletionEvent {
            state: RunState::Complete,
            workflow_resolution: None,
            terminal_outcome: None,
            error: None,
            activity_state: None,
            extra,
        };
        let value = serde_json::to_value(&event).expect("serializes");
        assert_eq!(value["type"], serde_json::json!("subagent.workflow.completed"));
        assert_eq!(
            value["state"],
            serde_json::json!("overridden"),
            "a caller's own event metadata key overrides a computed one"
        );
    }

    #[test]
    fn plan_workflow_settlement_marks_success_only_when_complete() {
        let status = workflow_status(RunState::Complete, vec![]);
        let plan = plan_workflow_settlement(PlanWorkflowSettlement {
            status: &status,
            summary: "done".to_string(),
            trace: &[],
            receipt: None,
            receipt_path: None,
            receipt_persistence_error: None,
            resolution: None,
            terminal_outcome: None,
            now: Some(1_000),
            event_metadata: serde_json::Map::new(),
        });
        assert!(plan.success);
        assert_eq!(plan.error, None);
        assert!(plan.completion_event.is_some(), "Complete is terminal");
        assert!(plan.receipt.is_none());
        assert!(plan.receipt_path.is_none());
    }

    #[test]
    fn plan_workflow_settlement_falls_back_error_to_the_summary() {
        let status = workflow_status(RunState::Failed, vec![]);
        let plan = plan_workflow_settlement(PlanWorkflowSettlement {
            status: &status,
            summary: "it broke".to_string(),
            trace: &[],
            receipt: None,
            receipt_path: None,
            receipt_persistence_error: None,
            resolution: None,
            terminal_outcome: None,
            now: Some(1_000),
            event_metadata: serde_json::Map::new(),
        });
        assert!(!plan.success);
        assert_eq!(plan.error.as_deref(), Some("it broke"));
    }

    #[test]
    fn plan_workflow_settlement_promotes_a_receipt_persistence_failure_to_failed() {
        let status = workflow_status(RunState::Complete, vec![]);
        let plan = plan_workflow_settlement(PlanWorkflowSettlement {
            status: &status,
            summary: "done".to_string(),
            trace: &[],
            receipt: None,
            receipt_path: None,
            receipt_persistence_error: Some("disk full".to_string()),
            resolution: None,
            terminal_outcome: None,
            now: Some(1_000),
            event_metadata: serde_json::Map::new(),
        });
        assert_eq!(plan.status.state, RunState::Failed);
        assert!(
            plan.summary
                .starts_with("evidence-persistence-failed: disk full"),
        );
        assert!(plan.receipt.is_none());
    }

    #[test]
    fn plan_workflow_settlement_never_leaves_a_receipt_without_a_path_or_vice_versa() {
        let status = workflow_status(RunState::Complete, vec![]);
        let run_id_str = status.run_id.as_str().to_string();
        let run = crate::identity::RunDirName::parse(&run_id_str).expect("valid");
        let receipt = super::super::receipt::build_workflow_receipt(
            super::super::receipt::BuildWorkflowReceipt {
                workflow_run_id: &run,
                state: WorkflowReceiptState::Complete,
                children: &[],
                host_steps: &[],
                workflow_children: None,
                resource: None,
                terminal_outcome: None,
                created_at: Some(1_000),
            },
        )
        .expect("builds");

        // A receipt with NO path: the key is built conditionally, never inserted-then-removed.
        let plan = plan_workflow_settlement(PlanWorkflowSettlement {
            status: &status,
            summary: "done".to_string(),
            trace: &[],
            receipt: Some(receipt.clone()),
            receipt_path: None,
            receipt_persistence_error: None,
            resolution: None,
            terminal_outcome: None,
            now: Some(1_000),
            event_metadata: serde_json::Map::new(),
        });
        assert!(plan.receipt_path.is_none());

        // A receipt WITH a path: both are present, and the receipt's own state/workflowChildren
        // reflect the settled status.
        let plan = plan_workflow_settlement(PlanWorkflowSettlement {
            status: &status,
            summary: "done".to_string(),
            trace: &[],
            receipt: Some(receipt),
            receipt_path: Some(PathBuf::from("/tmp/workflow-receipt.json")),
            receipt_persistence_error: None,
            resolution: None,
            terminal_outcome: None,
            now: Some(1_000),
            event_metadata: serde_json::Map::new(),
        });
        assert_eq!(
            plan.receipt_path,
            Some(PathBuf::from("/tmp/workflow-receipt.json"))
        );
        assert_eq!(plan.receipt.as_ref().map(|r| r.state), Some(WorkflowReceiptState::Complete));
    }
}

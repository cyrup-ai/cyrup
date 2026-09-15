//! The live-identity backfill — pi `workflow-detach-reconcile.ts:182-191` (@v0.67.0).
//!
//! Before settling, upstream may write the settling child's run id onto a workflow step that has
//! none. Every clause below exists to REFUSE a claim the caller cannot actually justify: two
//! same-key live attempts resolve to nothing (never "the first one"), and a completion that
//! matches only on `workflowKey` with no confirmed live control is stale and is ignored. Relaxing
//! any clause admits cross-lane corruption that no later stage can detect.

use crate::background::{ActivityState, RunId, RunStatus, StepState};
use crate::workflows::WorkflowKey;

use super::DetachedWorkflowChildCompletion;

/// One row of the caller's `state.foregroundControls` snapshot — pi's
/// `ForegroundRunControl` narrowed to the three fields `:182-185` reads.
///
/// The run id is a FIELD here and a MAP KEY upstream in cyrup
/// (`extension/executor/mod.rs:146`: `HashMap<String, ForegroundControlEntry>`, documented
/// `targetRunId -> {…}`); [`ForegroundControlEntry`](crate::extension::executor::notices::ForegroundControlEntry)
/// itself has no `run_id`. Snapshotting `(key, entry)` pairs into this row is what lets the
/// comparison at `:185` be written at all, and it is why the registry's `std::sync::Mutex` is
/// never held across an `.await` in this module — the caller drops the guard before calling in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveForegroundControl {
    /// The live run this control drives — upstream's map key.
    pub(crate) run_id: RunId,
    /// pi `control.parentWorkflowRunId` (`notices.rs:73`).
    pub(crate) parent_workflow_run_id: Option<RunId>,
    /// pi `control.workflowKey` (`notices.rs:78`).
    pub(crate) workflow_key: Option<WorkflowKey>,
}

/// pi `workflow-detach-reconcile.ts:182-191` — back-fill `input.child_run_id` onto the one step
/// that provably belongs to it, or change nothing.
///
/// Takes `status` **by value and returns it** because upstream mutates `candidate.runId` in place
/// on the very object it then hands to `applyDetachedChildToPausedWorkflow` (`:190` → `:192`); a
/// `&mut` signature would work equally well but would let a caller forget to use the mutated
/// value, which is exactly the bug this shape makes unrepresentable.
///
/// The four guards, in upstream's order:
///
/// 0. `input.workflow_key` is `None` ⇒ `matchingControls` is empty (`:182`), so
///    `confirmedLiveIdentity` is false and nothing is back-filled. A completion with no workflow
///    key can never claim a step.
/// 1. Exactly ONE live control matches this `parent_workflow_run_id` + `workflow_key`, and its run
///    id is the settling child (`:182-185`).
/// 2. Exactly ONE status step carries that `workflow_key` (`:187-188`).
/// 3. That step has no `run_id`, no `session_file`, is `Paused`, and its activity state is
///    `NeedsAttention` (`:189-190`).
pub(crate) fn backfill_live_child_identity(
    mut status: RunStatus,
    input: &DetachedWorkflowChildCompletion<'_>,
) -> RunStatus {
    // Clause 0.
    let Some(workflow_key) = input.workflow_key else {
        return status;
    };

    // Clause 1. `filter` then `len() == 1` — never `find`, because "two candidates" must resolve
    // to nothing rather than to the first.
    let mut matching = input.live_controls.iter().filter(|control| {
        control.parent_workflow_run_id.as_ref() == Some(&status.run_id)
            && control.workflow_key.as_ref() == Some(workflow_key)
    });
    let Some(control) = matching.next() else {
        return status;
    };
    if matching.next().is_some() {
        return status;
    }
    if control.run_id.as_str() != input.child_run_id {
        return status;
    }

    // Clause 2 — same "exactly one" discipline over the status steps.
    let mut candidates = status
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.workflow_key.as_ref() == Some(workflow_key));
    let Some((index, _)) = candidates.next() else {
        return status;
    };
    if candidates.next().is_some() {
        return status;
    }

    // Clause 3 — all four sub-clauses, every one of them a refusal the caller cannot override.
    let Some(step) = status.steps.get_mut(index) else {
        return status;
    };
    if step.run_id.is_none()
        && step.session_file.is_none()
        && step.status == StepState::Paused
        && step.telemetry.activity_state == Some(ActivityState::NeedsAttention)
    {
        // The same constructor `apply_detached_child_settlement` uses for this exact write
        // (`workflows/settlement.rs:313`), so the two paths cannot mint different ids.
        step.run_id = Some(RunId::from_token(input.child_run_id.to_string()));
    }
    status
}

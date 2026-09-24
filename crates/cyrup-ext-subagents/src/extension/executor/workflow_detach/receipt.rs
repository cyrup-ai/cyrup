//! `reconcileWorkflowReceipt` — pi `workflow-detach-reconcile.ts:97-159` (@v0.67.0).
//!
//! The settling child's receipt entry is re-derived from its own terminal evidence and the receipt
//! is rewritten to disk BEFORE the settlement plan exists. That ordering is upstream's and is
//! load-bearing: `plan_workflow_settlement` discards its `receipt` entirely when a
//! `receipt_persistence_error` is set (`workflows/settlement.rs:688`), so the on-disk receipt is
//! the only copy that survives a fault, and the driver captures — never propagates — every fault
//! raised here.

use std::path::{Path, PathBuf};

use crate::background::control::read_status_file;
use crate::background::{RunId, RunPaths, RunState, RunStatus, StepStatus};
use crate::exec::SingleResult;
use crate::identity::RunDirName;
use crate::runner::status::{external_cli_receipt_metadata, normalize_external_cli_runner_status};
use crate::workflows::{
    Bounded, WorkflowBudgetSignals, WorkflowReceipt, WorkflowReceiptError, WorkflowReceiptResume,
    WorkflowReceiptState, WorkflowTerminalResolution, read_workflow_receipt,
    workflow_recovery_actions, workflow_terminal_outcome_for_result, write_workflow_receipt,
};

/// What a successful receipt reconcile published — pi's `{ receipt, path }` return (`:97`).
#[derive(Debug)]
pub(crate) struct PublishedWorkflowReceipt {
    /// The rewritten receipt, as it was written.
    pub(crate) receipt: WorkflowReceipt,
    /// Where it was written.
    pub(crate) path: PathBuf,
}

/// The reason attached to a not-resumable verdict when the probe found the child still live —
/// upstream's literal (`:111`).
const CHILD_STILL_RUNNING: &str = "child is still running";

/// The fallback reason for an external-CLI override whose metadata carries no
/// `nonResumableReason`. [`WorkflowReceiptResume::NotResumable::reason`] must never be blank
/// (`workflows/receipt.rs:176`), and the metadata is an opaque `serde_json::Value`
/// (`receipt.rs:253`), so the key CAN be absent on a receipt written by another build.
const EXTERNAL_ADAPTER_NOT_RESUMABLE: &str = "external CLI adapter does not support resume";

/// `RunState -> WorkflowReceiptState` (`:144`).
///
/// Re-derived locally rather than widening `workflows::settlement`'s private `workflow_receipt_state`
/// (`settlement.rs:74`): this task does not own that module, and the map is four exhaustive arms
/// whose whole content is the wire vocabulary. The composer applies the SAME map a second time to
/// `plan.receipt` (`settlement.rs:693`) — upstream double-applies it too (`:144` then
/// `workflow-settlement.ts:216`), and the duplication is not removable, because this copy stamps
/// the receipt that goes to DISK and the composer's stamps the copy that goes into the plan.
fn receipt_state_for(state: RunState) -> WorkflowReceiptState {
    match state {
        RunState::Complete => WorkflowReceiptState::Complete,
        RunState::Paused => WorkflowReceiptState::Paused,
        RunState::Stopped => WorkflowReceiptState::Stopped,
        RunState::Queued | RunState::Running | RunState::Failed | RunState::Partial => {
            WorkflowReceiptState::Failed
        }
    }
}

/// The child's own run directory.
///
/// Upstream hard-codes `path.join(DIRS.async, childRunId)` (`:115`) — the PROCESS-wide async root,
/// not the workflow's job directory. cyrup's equivalent root is the workflow run directory's
/// parent (`background/artifact_roots.rs`'s per-cwd async root, which is what
/// `RunPaths::for_run(async_root, …)` is handed), and a workflow child is launched under the same
/// per-cwd root as the workflow that launched it. Deriving it therefore reproduces upstream
/// exactly rather than assuming something upstream does not — which is why the child's `RunPaths`
/// is NOT a separate input field (SCOPE_8 §Y-2).
fn child_run_paths(workflow: &RunPaths, child_run_id: &str) -> RunPaths {
    let async_root: &Path = workflow.run_dir.parent().unwrap_or(&workflow.run_dir);
    RunPaths::for_run(
        async_root,
        &workflow.results_dir,
        &RunId::from_token(child_run_id.to_string()),
    )
}

/// Upstream's `resolveAsyncResumeTarget(…, { requireSessionFile: true, sessionId: status.sessionId })`
/// probe (`:107-113`), rebuilt from the child's own terminal record.
///
/// cyrup has no `resolve_async_resume_target`. The nearest surface, `background/control.rs`'s
/// `resume()`, is a PERFORMING call — it revives the child — so probing with it would restart the
/// very run this reconciler is settling. What `requireSessionFile: true` actually asks is "did
/// this child leave a resumable transcript", and the child's own `status.json` answers it: a
/// terminal state plus a recorded session file. A child whose status is missing, non-terminal, or
/// transcript-less is `not-resumable` with upstream's own reason; an I/O fault maps to upstream's
/// `catch` arm, which carries the error text as the reason.
///
/// `sessionId` scoping (§2 ref 1) is structural here rather than a parameter: the child's run
/// directory is resolved under the workflow's own async root, so a child belonging to another
/// instance's run is not reachable through this path at all.
async fn probe_child_resumability(
    workflow: &RunPaths,
    child_run_id: &str,
    result: &SingleResult,
    latest_run_id: Option<Bounded<256>>,
) -> WorkflowReceiptResume {
    let child = child_run_paths(workflow, child_run_id);
    let child_status = match read_status_file(&child.status).await {
        Ok(status) => status,
        Err(error) => {
            return WorkflowReceiptResume::NotResumable {
                latest_run_id,
                reason: error.to_string(),
            };
        }
    };
    let resumable = child_status.is_some_and(|child_status| {
        child_status.state.is_terminal()
            && (child_status.session_file.is_some() || result.session_file.is_some())
    });
    match (resumable, latest_run_id) {
        // `:127` — `latestRunId: entry.latestRunId ?? childRunId`. The `Resumable` arm
        // STRUCTURALLY requires the id (`workflows/receipt.rs:164-170`), which is why the
        // fallback is applied by the caller before this point and re-checked here.
        (true, Some(latest_run_id)) => WorkflowReceiptResume::Resumable { latest_run_id },
        (true, None) => WorkflowReceiptResume::NotResumable {
            latest_run_id: None,
            reason: "child produced no run id".to_string(),
        },
        (false, latest_run_id) => WorkflowReceiptResume::NotResumable {
            latest_run_id,
            reason: CHILD_STILL_RUNNING.to_string(),
        },
    }
}

/// pi `reconcileWorkflowReceipt` (`:97-159`).
///
/// Returns `Ok(None)` for a workflow that never wrote a receipt (`:100`) — that reconciles fine.
/// Every other fault is an `Err(String)` the driver turns into a
/// `receipt_persistence_error`, which `plan_workflow_settlement` then converts into an
/// `EVIDENCE_PERSISTENCE_FAILED` status; the available child evidence is still published.
///
/// # Errors
///
/// The two verbatim identity refusals (`:104`, `:106`), an unreadable/invalid receipt, or a write
/// failure.
pub(crate) async fn reconcile_workflow_receipt(
    status: &RunStatus,
    child_run_id: &str,
    result: &SingleResult,
    run_paths: &RunPaths,
    resolution: Option<WorkflowTerminalResolution>,
) -> Result<Option<PublishedWorkflowReceipt>, String> {
    // The asymmetry between the two IO calls is upstream's (`:98` `path.dirname(asyncDir)` for the
    // read, `:158` `asyncDir` for the write) and is deliberate: `read_workflow_receipt` takes the
    // async ROOT plus a `RunDirName`, `write_workflow_receipt` takes the already-resolved run dir.
    let async_root: &Path = run_paths.run_dir.parent().unwrap_or(&run_paths.run_dir);
    let run = RunDirName::for_run(&status.run_id);
    let receipt = match read_workflow_receipt(async_root, &run) {
        Ok(receipt) => receipt,
        // `:100` — `if (!fs.existsSync(receiptPath)) return undefined`. BOTH absent-file variants
        // map here: for a LIVE paused workflow `status.json` exists, so the absent receipt arrives
        // as `MayStillBeActive`, never `NotFound` (`workflows/receipt.rs:684-700`). Probing the
        // path first and then reading would be the TOCTOU this typed error removes.
        Err(
            WorkflowReceiptError::NotFound { .. } | WorkflowReceiptError::MayStillBeActive { .. },
        ) => {
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    };

    let step: Option<&StepStatus> = status
        .steps
        .iter()
        .find(|candidate| candidate.run_id.as_ref().map(RunId::as_str) == Some(child_run_id));
    let Some(key) = step.and_then(|step| step.workflow_key.clone()) else {
        // `:104` — verbatim, and matched by upstream's own tests.
        return Err(format!(
            "Workflow receipt '{}' cannot identify detached child '{child_run_id}' by stable key.",
            status.run_id
        ));
    };
    let Some(entry) = receipt.entry(&key) else {
        // `:106` — verbatim.
        return Err(format!(
            "Workflow receipt '{}' has no detached child key '{}'.",
            status.run_id,
            key.as_str()
        ));
    };
    let entry = entry.clone();

    // `:127` — the retained id the resumable arm needs, with upstream's fallback to the settling
    // child's own id.
    let latest_run_id = entry
        .resume
        .latest_run_id()
        .cloned()
        .or_else(|| Bounded::<256>::parse(child_run_id));
    let mut resume =
        probe_child_resumability(run_paths, child_run_id, result, latest_run_id.clone()).await;

    // `:119` — `savedOutputPath ?? outputReference?.path ?? entry.outputReference`. cyrup's
    // `SingleResult` has no `output_reference`, so the chain is two rungs.
    let output_reference = result
        .saved_output_path
        .clone()
        .or_else(|| entry.output_reference.clone());

    // `:115-117` — the external adapter. Upstream ALSO falls back to the child status's
    // single-step `runner`/`externalProcess`; cyrup's `StepStatus` carries neither field
    // (`background/records.rs:24-180`), so the settling result — which does carry both
    // (`exec/run_result.rs:313`, `:319`) — is the only source, and the fallback has no carrier to
    // read rather than being dropped by choice.
    let external_runner = result
        .runner
        .as_ref()
        .and_then(|runner| serde_json::to_value(runner).ok())
        .as_ref()
        .and_then(normalize_external_cli_runner_status);
    let external_adapter = match &external_runner {
        Some(runner) => Some(external_cli_receipt_metadata(
            runner,
            result.external_process.as_ref(),
            output_reference.as_deref(),
        )),
        None => entry.external_adapter.clone(),
    };

    // `:120` — the outcome MERGES onto the entry, it does not replace: a detached timeout stays
    // local to its own entry and is never promoted workflow-wide.
    let child_terminal_outcome =
        workflow_terminal_outcome_for_result(WorkflowBudgetSignals::from_single_result(result))
            .or(entry.terminal_outcome);

    // `:121` — last and absolute, REGARDLESS of the probe above. This is what keeps mixed
    // pi/external entries on their own computed resumability instead of a blanket verdict: only
    // the entries that actually carry an adapter are forced.
    if let Some(adapter) = &external_adapter {
        resume = WorkflowReceiptResume::NotResumable {
            latest_run_id: latest_run_id.clone(),
            reason: adapter
                .get("nonResumableReason")
                .and_then(serde_json::Value::as_str)
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or(EXTERNAL_ADAPTER_NOT_RESUMABLE)
                .to_string(),
        };
    }

    let mut updated = entry;
    // `:123`/`:135` — `...(step.agent ? { agent: step.agent } : {})`. `StepStatus::agent` is a
    // plain `String`, so "present" is "non-blank"; `Bounded::parse` already rejects blank.
    if let Some(agent) = step.and_then(|step| Bounded::<256>::parse(&step.agent)) {
        updated.agent = Some(agent);
    }
    // Upstream also copies `step.context` onto `resolvedContext` (`:124`); cyrup's `StepStatus`
    // has no context field, so there is nothing to copy.
    updated.resume = resume;
    if output_reference.is_some() {
        updated.output_reference = output_reference;
    }
    if child_terminal_outcome.is_some() {
        updated.terminal_outcome = child_terminal_outcome;
    }
    if external_adapter.is_some() {
        updated.external_adapter = external_adapter;
    }

    let mut next = receipt;
    if let Some(slot) = next.entries.iter_mut().find(|slot| slot.key == key) {
        *slot = updated;
    }
    // `:144` / `:149` — the receipt's own state and child inventory come from the SETTLED status.
    next.state = receipt_state_for(status.state);
    next.workflow_children = status.workflow_children.clone();
    // `:153-157` — set-or-delete, both halves together: a receipt with a resolution carries
    // recovery, one without carries neither.
    if let Some(resolution) = resolution {
        next.workflow_resolution = Some(resolution);
        next.recovery = Some(workflow_recovery_actions(Some(&next)));
    } else {
        next.workflow_resolution = None;
        next.recovery = None;
    }

    let path = write_workflow_receipt(&run_paths.run_dir, &next).map_err(|e| e.to_string())?;
    Ok(Some(PublishedWorkflowReceipt {
        receipt: next,
        path,
    }))
}

//! Detached-workflow-child completion reconciliation — ports pi
//! `runs/foreground/workflow-detach-reconcile.ts` (304 LOC @v0.67.0).
//!
//! The lifecycle this module owns: a **paused** workflow whose **detached foreground child**
//! finally exits must be settled — neither stranded `paused` forever, nor silently promoted to
//! "complete" on evidence that its JavaScript continuation never actually persisted.
//!
//! # Why this is a disk-driven unit rather than a callback
//!
//! Upstream's caller is the `onDetachedExit` closure (`subagent-executor.ts:3951-3976`), fired by
//! `execution.ts:2364` when a child detached through `detachForeground` eventually terminates.
//! **cyrup has none of that machinery**: [`crate::exec::SingleResult::detached`] is a FLAG on a
//! returned result (set by the drive loop when a blocking `contact_supervisor` ask is surfaced),
//! not a "return the tool call early and keep the child running" mechanism, and nothing fires on
//! the detached child's later exit. Building that mechanism is foreground-lifecycle work this task
//! does not own.
//!
//! So the reconciler's input is the run DIRECTORY plus a settled child result. Every value
//! upstream reads off in-process state has a durable source — `state.asyncJobs.get(id)?.asyncDir`
//! becomes [`RunPaths`], `readStatus(asyncDir)` becomes
//! [`read_status_file`](crate::background::control::read_status_file), the result file becomes
//! [`RunPaths::resolve_result`] — and upstream itself already falls back to the disk path when the
//! job is absent, so the durable source is the PRIMARY, not a degradation. Wiring this to a future
//! detach hook is then a one-line call rather than a rewrite.
//!
//! The one piece that cannot come off disk is `state.foregroundControls`, which the identity
//! back-fill (`identity.rs`) needs. SCOPE_8 §Y-1 named two shapes; this module takes **(a)** — the
//! caller snapshots the registry into
//! [`LiveForegroundControl`](identity::LiveForegroundControl) rows and hands them in. Shape (b)
//! (the driver reading `SubagentExecutor::foreground_controls` by field, legal for a descendant
//! module) is closer to upstream but gives up the testability posture above for the ONE input that
//! has no durable form — and it would also couple every future caller to a live executor handle it
//! may not have. The registry's `std::sync::Mutex` is documented as "every access is a short
//! synchronous read" (`extension/executor/mod.rs:152-159`), so snapshotting in the caller is also
//! what keeps the guard from ever being alive across the `.await`s below.
//!
//! # Two rungs of upstream's write path are deliberately absent
//!
//! * **`updateActiveRunIndex` (`:256`).** cyrup has only the TERMINAL run index
//!   (`background/terminal_run_index/`). Every state this reconciler publishes is terminal or
//!   `paused`, and `is_indexed_state` admits `paused` (`terminal_run_index/entry.rs:103-105`), so
//!   the terminal index is called unconditionally — exactly as `finish_run` calls it
//!   (`runner_main/finish.rs:352`) for the `Paused` runs it also writes. An active-run index is
//!   SCOPE_9's surface and is not created or touched here (SCOPE_8 §Y-7).
//! * **The bus emit (`:281-301`).** cyrup has no write-site completion emit, and adding one would
//!   DOUBLE-deliver every reconciled workflow. pi's completion signal is an in-process event
//!   because pi's runner is in-process; cyrup's runner is a detached OS process whose only signal
//!   is the terminal `ResultFile` it writes, which `ResultsWatcher` observes and republishes as a
//!   `CompletionEvent` (`background/watch/observer.rs:1-5`). **Writing the result file IS the
//!   emit.** A later reader must not "restore the missing emit". The
//!   `subagent.workflow.completed` `events.jsonl` append (`:274-280`) is NOT the bus emit and IS
//!   still performed.
//!
//! # The job mirror (`:257-263`) has no analogue either
//!
//! Upstream copies the settled status onto its in-memory `asyncJobs` entry. cyrup's tracker is
//! refreshed by RE-READING `status.json`, not by in-place mutation, so there is nothing to mirror.

pub(crate) mod children;
pub(crate) mod identity;
pub(crate) mod receipt;

use crate::background::control::read_status_file;
use crate::background::result_index::{self, PayloadState, ResultWrite};
use crate::background::runner_main::{WorkflowResultFields, apply_workflow_settlement_plan};
use crate::background::{ResultFile, RunMode, RunPaths, RunState, RunStatus, StepStatus};
use crate::error::SubagentError;
use crate::exec::SingleResult;
use crate::identity::SessionId;
use crate::jsonl::BoundedJsonlWriter;
use crate::workflows::{
    DetachedChildSettlement, PlanWorkflowSettlement, SettledWorkflowStatus, WorkflowKey,
    WorkflowReceipt, WorkflowScriptTraceEntry, WorkflowSettlementPlan, WorkflowTerminalResolution,
    apply_detached_child_settlement, classify_workflow_settlement, find_workflow_settlement_step,
    plan_workflow_settlement, promote_settled_paused_workflow,
    workflow_output_path_mapping_summary, workflow_recovery_actions,
};

pub(crate) use children::workflow_result_children;
pub(crate) use identity::{LiveForegroundControl, backfill_live_child_identity};
pub(crate) use receipt::reconcile_workflow_receipt;

/// pi `applyDetachedChildToPausedWorkflow` (`workflow-detach-reconcile.ts:33-38`) — the entry
/// point the detach lifecycle calls, delegating to the settlement module that owns the logic.
///
/// Kept as an indirection exactly as upstream keeps it: the settlement module owns the rule, this
/// module owns the entry point, and a call site can move between the two without touching
/// `workflows/`.
///
/// # Errors
///
/// See [`apply_detached_child_settlement`].
pub(crate) fn apply_detached_child_to_paused_workflow(
    status: &RunStatus,
    input: DetachedChildSettlement<'_>,
    trace: &[WorkflowScriptTraceEntry],
) -> Result<Option<SettledWorkflowStatus>, String> {
    apply_detached_child_settlement(status, input, trace)
}

/// pi `promotePausedWorkflowIfSettled` (`:40-42`) — the SECOND, thinner entry point, used by the
/// workflow's own settlement path rather than by the driver below.
///
/// Upstream's caller (`subagent-executor.ts:5763`) runs right after a workflow fails with
/// `errorKind: "detached-child"` and is therefore parked at `paused`: it asks whether the workflow
/// is ALREADY settled (every detached child having exited before the engine unwound) and promotes
/// it if so.
///
/// `#[cfg_attr(not(test), allow(dead_code))]`, the same discipline `finish.rs:203` used and for
/// the same reason: [`crate::workflows::scripted::WorkflowScriptErrorKind::DetachedChild`] exists
/// (`workflows/scripted/types.rs:250`) but nothing in-tree yet PARKS a run at `Paused` with it, so
/// this wrapper has no production caller until the workflow engine's own detached-child unwind arm
/// lands. The driver below does not reach it — it goes through
/// [`apply_detached_child_to_paused_workflow`], which promotes internally.
///
/// # Errors
///
/// See [`promote_settled_paused_workflow`].
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn promote_paused_workflow_if_settled(
    status: &RunStatus,
    trace: &[WorkflowScriptTraceEntry],
) -> Result<Option<SettledWorkflowStatus>, String> {
    promote_settled_paused_workflow(status, trace, crate::time::now_epoch_millis())
}

/// The reconciler's input — pi's `reconcileDetachedWorkflowChildCompletion` argument object
/// (`:170-177`), with the two in-process reads replaced by their durable equivalents.
pub(crate) struct DetachedWorkflowChildCompletion<'a> {
    /// The WORKFLOW run's directory layout. Stands in for `state.asyncJobs.get(workflowRunId)?
    /// .asyncDir ?? path.join(DIRS.async, workflowRunId)` (`:178-179`) — and carries the run id,
    /// so upstream's separate `workflowRunId` argument is redundant here: the status read back
    /// from `run_paths.status` is the authority for it.
    pub(crate) run_paths: &'a RunPaths,
    /// The detached child that just exited.
    pub(crate) child_run_id: &'a str,
    /// Its settled result.
    pub(crate) result: &'a SingleResult,
    /// The workflow lane the child was launched under, when the caller knows it. `None` disables
    /// the identity back-fill entirely (clause 0, `:182`).
    pub(crate) workflow_key: Option<&'a WorkflowKey>,
    /// A snapshot of `state.foregroundControls` (`:182-184`) — see this module's doc for why it is
    /// a snapshot and not a live handle. Empty is legal and simply means "no confirmed live
    /// identity", which is the safe verdict.
    pub(crate) live_controls: &'a [LiveForegroundControl],
    /// The workflow script's execution trace, for
    /// [`crate::workflows::with_workflow_children`]'s inventory pass.
    ///
    /// An INPUT FIELD rather than a hard-coded `&[]` (SCOPE_8 §Y-4): [`RunStatus`] has no trace
    /// and no script-state field, so a disk-driven reconciler genuinely cannot recover it, and
    /// passing `&[]` unconditionally would skip pass 1 of `workflow_child_summary`
    /// (`workflows/child_summary.rs:205-212`) and lose every trace-only `agent`/`sessionName`/
    /// `model` carry-forward. Passes 2 (steps) and 3 (children) still populate the inventory, so
    /// `&[]` degrades rather than breaks — which is exactly why the caller, who may hold the trace
    /// from the engine, decides.
    pub(crate) trace: &'a [WorkflowScriptTraceEntry],
}

/// pi `reconcileDetachedWorkflowChildCompletion` (`:170-304`).
///
/// Returns `false` — WITHOUT writing anything — when the run has no status, when the status is not
/// a `paused` workflow, or when the settling child matches no step. Upstream's three early
/// `return false`s ARE the idempotency guarantee: replaying this after the workflow already
/// settled is a no-op, because [`apply_detached_child_settlement`] refuses a non-`Paused` status
/// (`workflows/settlement.rs:278-282`). A crash between the identity back-fill and the publish
/// therefore replays safely.
///
/// The pipeline order below is upstream's and the order IS the behaviour: a receipt failure must
/// be able to demote a status that was already promoted, which it can only do if classification
/// and the receipt pass both happen before the plan is composed.
///
/// `#[cfg_attr(not(test), allow(dead_code))]`: cyrup has no detach hook to fire this (see the
/// module doc), so this function's own tests are its only callers until a foreground
/// detach-lifecycle task builds one. The attribute is on the DRIVER precisely so it can come OFF
/// [`crate::background::runner_main::apply_workflow_settlement_plan`], which this function is the
/// first production caller of.
///
/// # Errors
///
/// [`SubagentError::Management`] when a settlement invariant refuses (the settlement helpers yield
/// `String`), or when the reconciled result has no session to be attributed to; [`SubagentError::Spawn`]
/// for an I/O fault reading the status or the prior result file.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn reconcile_detached_workflow_child_completion(
    input: DetachedWorkflowChildCompletion<'_>,
) -> Result<bool, SubagentError> {
    // `:181`.
    let Some(status) = read_status_file(&input.run_paths.status).await? else {
        return Ok(false);
    };

    // `:182-191` — live-identity back-fill, BEFORE settlement.
    let status = backfill_live_child_identity(status, &input);

    // `:192-197`.
    let Some(next) = apply_detached_child_to_paused_workflow(
        &status,
        DetachedChildSettlement {
            child_run_id: input.child_run_id,
            // `DetachedChildSettlement::exit_code` is `Option<i32>` (a child may not have one);
            // `SingleResult::exit_code` is not (`exec/run_result.rs:28`).
            exit_code: Some(input.result.exit_code),
            error: input.result.error.as_deref(),
            interrupted: input.result.interrupted,
            session_file: input.result.session_file.as_deref(),
            // `SingleResult` has no `session_name`; the step keeps whatever it already carried.
            session_name: None,
            stopped: input.result.stopped,
            workflow_key: input.workflow_key,
            now: crate::time::now_epoch_millis(),
        },
        input.trace,
    )
    .map_err(SubagentError::Management)?
    else {
        return Ok(false);
    };
    let mut next = next;

    // `:198-201` — the settled step keeps the mapping derived from THIS result's task/output.
    // `find_workflow_settlement_step(.., None, None)` is exact-`run_id`-only, and that is correct,
    // not an omission: the by-key rung needs BOTH a key and a session file
    // (`workflows/settlement.rs:145-147`), and upstream's second call passes neither either.
    if let Some(mapping) = children::output_path_mapping_from_task(
        &input.result.task,
        input.result.saved_output_path.as_deref(),
    ) && let Some(index) =
        find_workflow_settlement_step(&next.steps, input.child_run_id, None, None)
        && let Some(step) = next.steps.get_mut(index)
    {
        step.output_path_mapping = Some(mapping);
    }

    // `:202-209` — the prior result file, read BEFORE the receipt pass because both the child
    // rebuild and the session fallback need it. Upstream re-throws every non-`ENOENT` error; the
    // cyrup equivalent is absent ⇒ `None`, unparseable ⇒ propagate.
    let existing = read_existing_result_file(input.run_paths, &next).await?;

    // `:213` — classification is computed on the SETTLED status, before the receipt pass.
    let resolution = classify_workflow_settlement(&next, input.result.interrupted);

    // `:214-220` — a receipt fault is CAPTURED, never propagated: the available child evidence
    // must still be published, and `plan_workflow_settlement` is what turns the fault into a
    // failed status.
    let (receipt, receipt_path, receipt_error) = match reconcile_workflow_receipt(
        &next,
        input.child_run_id,
        input.result,
        input.run_paths,
        resolution,
    )
    .await
    {
        Ok(Some(published)) => (Some(published.receipt), Some(published.path), None),
        Ok(None) => (None, None, None),
        Err(error) => (
            None,
            None,
            Some(format!(
                "Failed to reconcile async workflow receipt: {error}"
            )),
        ),
    };

    // `:221-227`.
    let children = workflow_result_children(
        &next,
        input.child_run_id,
        input.result,
        existing.as_ref(),
        receipt.as_ref(),
    );
    let summary =
        detached_settlement_summary(&next, input.child_run_id, resolution, receipt.as_ref());

    // `:252` — the terminal outcome is read off the RECEIPT, not recomputed from the result:
    // `reconcile_workflow_receipt` has already folded `entry.terminal_outcome` into it, and
    // upstream deliberately lets that merged value win. Bound before the literal below, because
    // `receipt` is moved into it.
    let terminal_outcome = receipt.as_ref().and_then(|r| r.terminal_outcome);

    // `:228-254`.
    let plan = plan_workflow_settlement(PlanWorkflowSettlement {
        status: &next,
        summary,
        trace: input.trace,
        receipt,
        receipt_path,
        receipt_persistence_error: receipt_error.clone(),
        resolution,
        terminal_outcome,
        now: None,
        event_metadata: reconciled_from_detached_child(input.child_run_id),
    });

    // SUBTASK3 — WORKFLOW_3's deferred seam. `apply_workflow_settlement_plan` is the ONE stamper a
    // workflow terminal path calls instead of open-coding the assembly (upstream open-codes it
    // twice), and SCOPE_8 is the "future async workflow arm" its doc named.
    let mut settled = next;
    let workflow_fields = apply_workflow_settlement_plan(&plan, &mut settled);

    publish_reconciled_settlement(
        &input,
        &plan,
        &settled,
        children,
        existing.as_ref(),
        workflow_fields,
        receipt_error.as_deref(),
    )
    .await?;
    Ok(true)
}

/// `{ reconciledFromDetachedChild: <childRunId> }` (`:253`).
///
/// [`crate::workflows::WorkflowCompletionEvent::extra`] merges caller keys LAST
/// (`workflows/settlement.rs:582-584`) for exactly this purpose.
fn reconciled_from_detached_child(
    child_run_id: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "reconciledFromDetachedChild".to_string(),
        serde_json::json!(child_run_id),
    );
    metadata
}

/// pi's summary ternary plus its concatenated mapping clause (`:222-227`).
///
/// Two details that must not be simplified away:
///
/// * [`workflow_recovery_actions`] is called TWICE — here for the summary's branch, and again
///   inside [`plan_workflow_settlement`] (`settlement.rs:711`) for `plan.recovery`. That is not
///   redundant: the summary is an INPUT to the plan, so it cannot read `plan.recovery`.
/// * Upstream's last arm falls back to `existing?.summary` (`:227`). [`ResultFile`] has **no
///   `summary` field** (SCOPE_8 §W-7), and widening its wire shape — read by `wait`, the watcher
///   and the projector — is out of this task's scope, so the chain collapses to
///   `next.error ?? "Workflow failed."`. The settled summary itself is not lost: it is carried on
///   the `subagent.workflow.completed` line this reconciler appends (SCOPE_8 §Y-5), which is the
///   one operator-facing signal cyrup keeps from upstream's bus emit.
fn detached_settlement_summary(
    next: &RunStatus,
    child_run_id: &str,
    resolution: Option<WorkflowTerminalResolution>,
    receipt: Option<&WorkflowReceipt>,
) -> String {
    let recovery = workflow_recovery_actions(receipt);
    let head = if resolution == Some(WorkflowTerminalResolution::SettledAwaitingResume) {
        format!(
            "Workflow lanes settled after detached child {child_run_id} finished. JavaScript \
             workflow continuation was not persisted.{}",
            if recovery.is_empty() {
                " No retained child is resumable."
            } else {
                " Use the listed keyed recovery action to continue a child."
            }
        )
    } else if next.state == RunState::Complete {
        format!("Workflow completed after detached child {child_run_id} finished.")
    } else {
        next.error
            .clone()
            .unwrap_or_else(|| "Workflow failed.".to_string())
    };
    // The leading space and trailing period belong to the mapping summary and are CONCATENATED
    // (`settlement.rs:487-492`); trimming either changes the rendered sentence.
    //
    // Fed from `next.steps`, not from the published children: `output_path_mappings_of`
    // (`settlement.rs:519`) adapts `&[WorkflowScriptChildResult]`, and this reconciler's children
    // are `SingleResult`s that carry no mapping at all — the generic `IntoIterator` signature
    // exists for precisely this caller, and its `"child"` key fallback (`settlement.rs:503`)
    // becomes reachable here for the first time.
    let mappings = next.steps.iter().filter_map(|step: &StepStatus| {
        step.output_path_mapping
            .as_ref()
            .map(|mapping| (step.workflow_key.as_ref().map(WorkflowKey::as_str), mapping))
    });
    format!("{head}{}", workflow_output_path_mapping_summary(mappings))
}

/// The prior published payload (`:202-209`).
///
/// Resolved rather than probed at a fixed path: a promoted payload lives under
/// `result-owned/<enc(session)>/`, which a bare path test would miss
/// (`background/run_paths.rs:155-168`'s own rationale).
async fn read_existing_result_file(
    run_paths: &RunPaths,
    status: &RunStatus,
) -> Result<Option<ResultFile>, SubagentError> {
    let resolved = match status.session_id.as_ref() {
        Some(session_id) => run_paths.resolve_result(session_id, &status.run_id).await,
        None => None,
    };
    // Upstream's `resultFilePath(DIRS.results, workflowRunId)` (`:200`) is NOT session-partitioned.
    // cyrup's owned and staged locations are, so a status carrying no session cannot address them
    // — and the legacy root (`<results>/<run_id>.json`) is then the only payload this run could
    // have. Reading it is also what makes upstream's `sessionId: next.sessionId ??
    // existing.sessionId` fallback (`:244`) reachable at all: with no session on the status there
    // is no other way to recover one.
    let path = resolved.unwrap_or_else(|| run_paths.legacy_result_root.clone());
    match tokio::fs::read(&path).await {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            SubagentError::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SubagentError::Spawn(e)),
    }
}

/// The write path (`:255-280`), in order — **the ordering is the guarantee**.
///
/// `status.json` → terminal index → result file → events, which is the same order `finish_run`
/// itself performs (`runner_main/finish.rs:349`, `:352`, `:489`). `finish_run` is deliberately NOT
/// called: its first statement refuses any run that already published a terminal `ResultFile`
/// (`finish.rs:237-245`), and a paused workflow HAS published one — this reconciler's entire job
/// is the superseding second write that guard exists to refuse. It would also synthesize the wrong
/// payload (`agent` from the steps, `success` from the exit codes) where upstream publishes
/// `agent: "workflow"`, `mode: "workflow"` and `plan.success`.
async fn publish_reconciled_settlement(
    input: &DetachedWorkflowChildCompletion<'_>,
    plan: &WorkflowSettlementPlan,
    settled: &RunStatus,
    children: Vec<SingleResult>,
    existing: Option<&ResultFile>,
    workflow_fields: WorkflowResultFields,
    receipt_error: Option<&str>,
) -> Result<(), SubagentError> {
    // 1 — `:255`.
    crate::background::atomic::write_atomic_json(&input.run_paths.status, settled)
        .await
        .map_err(SubagentError::Spawn)?;

    // 2 — `:256`'s cyrup counterpart. Self-gated on `is_indexed_state` and on `session_id`, so it
    // is called unconditionally with no branch of our own, and best-effort exactly as
    // `finish.rs:352-366` treats it: an advisory index that failed to write must never fail the
    // settlement that was completing.
    if let Err(error) = crate::background::terminal_run_index::update_terminal_run_index(
        &input.run_paths.run_dir,
        settled,
    )
    .await
    {
        tracing::warn!(
            run_id = %settled.run_id,
            %error,
            "failed to write terminal-run index marker for a reconciled detached workflow; the \
             run's own terminal record is unaffected"
        );
    }

    // 3 — `:264`. The session refusal is `write_result_file`'s (`finish.rs:478-484`), reproduced
    // rather than re-derived: an unattributable result cannot be indexed and would never be
    // delivered. It is raised AFTER the status write on purpose — `status.json` is what
    // `/subagents` reads, so a settlement that cannot record a result still reports its state
    // (R-SA-077's independence).
    let session_id = settled
        .session_id
        .clone()
        // `:244` — `next.sessionId ?? existing?.sessionId`, in that order. Reversing it, or
        // dropping the fallback, loses attribution for a workflow whose paused result file was
        // collected before the child exited.
        .or_else(|| existing.and_then(|existing| existing.session_id.clone()));
    let Some(session_id) = session_id else {
        return Err(SubagentError::Management(format!(
            "cannot write reconciled workflow result for run '{}' without a session id: an \
             unattributable result cannot be indexed and would never be delivered",
            settled.run_id
        )));
    };
    let result_file = reconciled_result_file(
        plan,
        settled,
        &session_id,
        children,
        existing,
        workflow_fields,
    );
    write_reconciled_result_file(input.run_paths, settled, &session_id, &result_file).await?;

    // 4/5 — `:265-280`. One writer, reused for both lines. A journal failure is logged and
    // swallowed, matching upstream's `appendDetachedWorkflowEvent` try/catch (`:161-168`): the
    // settlement is already durable on disk, and losing an advisory event line must not undo it.
    // `write_line` is additionally a silent no-op once the file's byte budget is reached
    // (R-SA-136), so a dropped event is not an error either.
    if receipt_error.is_some() || plan.completion_event.is_some() {
        match BoundedJsonlWriter::create(&input.run_paths.events).await {
            Ok(mut writer) => {
                if let Some(error) = receipt_error {
                    let mut event = event_envelope(settled);
                    event.insert(
                        "type".to_string(),
                        serde_json::json!("subagent.workflow.receipt_write_failed"),
                    );
                    event.insert("error".to_string(), serde_json::json!(error));
                    event.insert(
                        "reconciledFromDetachedChild".to_string(),
                        serde_json::json!(input.child_run_id),
                    );
                    append_event(&mut writer, &input.run_paths.events, event).await;
                }
                // `:274` — gated on the plan's own completion event, which exists iff
                // `status.state.is_terminal()` (`settlement.rs:715-728`; `Paused` is excluded).
                // While another detached child is still open the workflow stays `paused`, the plan
                // produces no event, and this reconciler writes status + result and stops.
                // Emitting a completion for a still-paused workflow would publish a terminal
                // verdict the workflow has not reached.
                if let Some(completion) = &plan.completion_event {
                    let mut event = event_envelope(settled);
                    if let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(completion)
                    {
                        event.extend(fields);
                    }
                    event.insert(
                        "recovery".to_string(),
                        serde_json::to_value(&plan.recovery).unwrap_or(serde_json::Value::Null),
                    );
                    // SCOPE_8 §Y-5: the settled summary's only surviving home (see
                    // `detached_settlement_summary`). Upstream carries it on the bus emit, which
                    // cyrup does not have.
                    event.insert("summary".to_string(), serde_json::json!(plan.summary));
                    append_event(&mut writer, &input.run_paths.events, event).await;
                }
            }
            Err(error) => tracing::warn!(
                events = %input.run_paths.events.display(),
                %error,
                "failed to open the detached-workflow event journal; the reconciled settlement is \
                 already durable in status.json and the result file"
            ),
        }
    }
    Ok(())
}

/// `{ ts, runId, … }` — upstream's two fixed keys on every appended event (`:266-267`, `:275-276`).
fn event_envelope(settled: &RunStatus) -> serde_json::Map<String, serde_json::Value> {
    let mut event = serde_json::Map::new();
    event.insert(
        "ts".to_string(),
        serde_json::json!(crate::time::now_epoch_millis()),
    );
    event.insert(
        "runId".to_string(),
        serde_json::json!(settled.run_id.as_str()),
    );
    event
}

async fn append_event(
    writer: &mut BoundedJsonlWriter,
    path: &std::path::Path,
    event: serde_json::Map<String, serde_json::Value>,
) {
    let line = serde_json::Value::Object(event).to_string();
    if let Err(error) = writer.write_line(&line).await {
        tracing::warn!(
            events = %path.display(),
            %error,
            "failed to append a detached-workflow event line"
        );
    }
}

/// The published payload (`:232-247`), narrowed to the fields [`ResultFile`] actually has.
///
/// Six upstream keys have no home and are NOT invented here (SCOPE_8 §W-7): `toolCallId`,
/// `asyncDir`, `workflow` (script state), `reconciledFromDetachedChild` (which survives on the
/// completion event's `extra`), `scheduleOrigin` (no field anywhere in the crate) and `summary`.
/// Widening `ResultFile` is out of scope — its wire shape is read by `wait`, the results watcher
/// and the completion projector.
fn reconciled_result_file(
    plan: &WorkflowSettlementPlan,
    settled: &RunStatus,
    session_id: &SessionId,
    children: Vec<SingleResult>,
    existing: Option<&ResultFile>,
    workflow_fields: WorkflowResultFields,
) -> ResultFile {
    ResultFile {
        id: settled.run_id.clone(),
        run_id: settled.run_id.clone(),
        // `:237-238` — literal, not synthesized from the steps.
        agent: "workflow".to_string(),
        mode: RunMode::Workflow,
        state: settled.state,
        // `plan.success` is `state == Complete` (`settlement.rs:707`), never the all-exit-codes-zero
        // rule `finish_run` uses.
        success: plan.success,
        // `RunStatus::cwd` is `Option<PathBuf>` while `ResultFile::cwd` is not, and upstream's
        // `cwd: next.cwd` may likewise be undefined. The prior payload is the better fallback than
        // any synthesized directory; an empty path is what "the status never recorded one"
        // honestly serializes to.
        cwd: settled
            .cwd
            .clone()
            .or_else(|| existing.map(|existing| existing.cwd.clone()))
            .unwrap_or_default(),
        session_file: settled
            .session_file
            .clone()
            .or_else(|| existing.and_then(|existing| existing.session_file.clone())),
        session_id: Some(session_id.clone()),
        completion_owner_id: settled
            .completion_owner_id
            .clone()
            .or_else(|| existing.and_then(|existing| existing.completion_owner_id.clone())),
        results: children,
        workflow_children: workflow_fields.workflow_children,
        workflow_receipt: workflow_fields.workflow_receipt,
    }
}

/// `writeAsyncResultFile` (`:264`) through the session-partitioned index, mirroring
/// `finish.rs:485-500` — including its `Staged` warn-not-fail arm: a staged payload IS written and
/// will be promoted by the first reader that resolves it, so reporting it as a failure would be
/// wrong, and reporting it as plain success would make a promotion that never happened invisible.
async fn write_reconciled_result_file(
    run_paths: &RunPaths,
    settled: &RunStatus,
    session_id: &SessionId,
    result_file: &ResultFile,
) -> Result<(), SubagentError> {
    let state = result_index::write_async_result_file(
        &ResultWrite {
            results_dir: run_paths.results_dir.as_path(),
            session_id,
            run_id: &settled.run_id,
            written_at: crate::time::now_epoch_millis(),
            async_dir: Some(run_paths.run_dir.as_path()),
            tool_call_id: None,
        },
        result_file,
    )
    .await
    .map_err(SubagentError::Spawn)?;
    if state == PayloadState::Staged {
        tracing::warn!(
            run_id = %settled.run_id,
            "reconciled workflow result stayed staged: promotion did not publish it, so delivery \
             waits for a reader to promote it on read"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;

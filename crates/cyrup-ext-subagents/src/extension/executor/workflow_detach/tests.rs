//! SCOPE_8 §X — the behaviours pinned as tests.
//!
//! Every fixture is a `tempfile::tempdir()` holding a `paused` workflow `status.json` under an
//! async root, plus a sibling results dir, which is exactly the shape
//! [`RunPaths::for_run`](crate::background::RunPaths::for_run) addresses. Nothing here needs a live
//! executor — that is the whole point of this module's disk-driven posture.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

use serde_json::json;
use tempfile::TempDir;

use super::children::{output_path_mapping_from_task, usage_with_value, workflow_result_children};
use super::identity::{LiveForegroundControl, backfill_live_child_identity};
use super::receipt::reconcile_workflow_receipt;
use super::{
    DetachedWorkflowChildCompletion, apply_detached_child_to_paused_workflow,
    promote_paused_workflow_if_settled, reconcile_detached_workflow_child_completion,
};
use crate::background::atomic::write_atomic_json;
use crate::background::{
    ActivityState, ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus, StepState, StepStatus,
};
use crate::exec::SingleResult;
use crate::exec::output_state::SubagentOutputState;
use crate::identity::{CompletionOwnerId, RunDirName, SessionId};
use crate::workflows::{
    BuildWorkflowReceipt, DetachedChildSettlement, UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION,
    WORKFLOW_RECEIPT_FILE, WorkflowKey, WorkflowReceipt, WorkflowReceiptError,
    WorkflowReceiptResume, WorkflowReceiptState, WorkflowScriptChildResult,
    WorkflowTerminalOutcome, WorkflowTerminalOutcomeReason, WorkflowTerminalResolution,
    build_workflow_receipt, read_workflow_receipt, write_workflow_receipt,
};

const CHILD: &str = "childrun0001";

// -------------------------------------------------------------------------------------------
// Fixtures
// -------------------------------------------------------------------------------------------

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    run_paths: RunPaths,
    run_id: RunId,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let run_id = RunId::from_token("wfrun0001");
        let run_paths = RunPaths::for_run(&root.join("async"), &root.join("results"), &run_id);
        Self {
            _dir: dir,
            root,
            run_paths,
            run_id,
        }
    }

    fn async_root(&self) -> PathBuf {
        self.root.join("async")
    }

    async fn seed(&self, status: &RunStatus) {
        tokio::fs::create_dir_all(&self.run_paths.run_dir)
            .await
            .expect("run dir");
        write_atomic_json(&self.run_paths.status, status)
            .await
            .expect("status.json");
    }

    /// A terminal child run directory, so the receipt's resumability probe can find one.
    async fn seed_child(&self, child_run_id: &str, state: RunState, with_session_file: bool) {
        let child = RunPaths::for_run(
            &self.async_root(),
            &self.root.join("results"),
            &RunId::from_token(child_run_id.to_string()),
        );
        tokio::fs::create_dir_all(&child.run_dir)
            .await
            .expect("child run dir");
        let mut status = RunStatus::queued(
            RunId::from_token(child_run_id.to_string()),
            RunMode::Single,
            None,
        );
        status.state = state;
        if with_session_file {
            status.session_file = Some(self.root.join("session.jsonl"));
        }
        write_atomic_json(&child.status, &status)
            .await
            .expect("child status");
    }

    async fn read_status(&self) -> RunStatus {
        let bytes = tokio::fs::read(&self.run_paths.status)
            .await
            .expect("status.json");
        serde_json::from_slice(&bytes).expect("valid status")
    }

    async fn read_receipt_json(&self) -> serde_json::Value {
        let bytes = tokio::fs::read(self.run_paths.run_dir.join(WORKFLOW_RECEIPT_FILE))
            .await
            .expect("receipt");
        serde_json::from_slice(&bytes).expect("valid receipt json")
    }

    async fn read_result(&self) -> ResultFile {
        let path = self
            .run_paths
            .resolve_result(&session(), &self.run_id)
            .await
            .expect("published result");
        let bytes = tokio::fs::read(&path).await.expect("result payload");
        serde_json::from_slice(&bytes).expect("valid result")
    }

    async fn events(&self) -> String {
        tokio::fs::read_to_string(&self.run_paths.events)
            .await
            .unwrap_or_default()
    }
}

fn session() -> SessionId {
    SessionId::parse("scope8-session").expect("non-empty")
}

fn owner() -> CompletionOwnerId {
    CompletionOwnerId::parse("scope8-owner").expect("non-empty")
}

fn key(raw: &str) -> WorkflowKey {
    WorkflowKey::parse(raw).expect("valid key")
}

/// A paused workflow status carrying `steps`, attributed to [`session`]/[`owner`].
fn paused_workflow(run_id: &RunId, steps: Vec<StepStatus>) -> RunStatus {
    let mut status = RunStatus::queued(run_id.clone(), RunMode::Workflow, None);
    status.state = RunState::Paused;
    status.session_id = Some(session());
    status.completion_owner_id = Some(owner());
    status.cwd = Some(PathBuf::from("/tmp/scope8"));
    status.steps = steps;
    status
}

/// A step parked exactly where a detached child leaves it: `paused` + `needs_attention`.
fn detached_step(agent: &str, lane: &str, run_id: Option<&str>) -> StepStatus {
    let mut step = StepStatus::pending(agent);
    step.status = StepState::Paused;
    step.workflow_key = Some(key(lane));
    step.telemetry.activity_state = Some(ActivityState::NeedsAttention);
    step.run_id = run_id.map(|id| RunId::from_token(id.to_string()));
    step
}

fn child_result(child_run_id: &str) -> SingleResult {
    SingleResult {
        execution: None,
        native_machine: None,
        runtime_acknowledged_extensions: None,
        skills_warning: None,
        watchdog: None,
        agent: "researcher".to_string(),
        task: String::new(),
        exit_code: 0,
        usage: cyrup_core::Usage::default(),
        turns: 0,
        model: None,
        attempted_models: Vec::new(),
        model_attempts: Vec::new(),
        final_output: Some("done".to_string()),
        structured_output: None,
        acceptance: None,
        detached: true,
        detached_reason: None,
        interrupted: false,
        timed_out: false,
        timeout_recovery: None,
        context_overflow: false,
        stopped: false,
        process_signal: None,
        turn_budget: None,
        turn_budget_exceeded: false,
        wrap_up_requested: false,
        usage_budget: None,
        error: None,
        saved_output_path: None,
        session_file: None,
        child_run_id: Some(RunId::from_token(child_run_id.to_string())),
        output_state: SubagentOutputState::Present,
        structured_output_path: None,
        artifact_paths: None,
        transcript_path: None,
        transcript_error: None,
        tool_calls: Vec::new(),
        output_truncated: false,
        tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
        control_events: Vec::new(),
        progress: None,
        runner: None,
        external_process: None,
    }
}

fn completion<'a>(
    run_paths: &'a RunPaths,
    result: &'a SingleResult,
    workflow_key: Option<&'a WorkflowKey>,
    live_controls: &'a [LiveForegroundControl],
) -> DetachedWorkflowChildCompletion<'a> {
    DetachedWorkflowChildCompletion {
        run_paths,
        child_run_id: CHILD,
        result,
        workflow_key,
        live_controls,
        trace: &[],
    }
}

fn script_child(lane: &str, run_id: Option<&str>) -> WorkflowScriptChildResult {
    WorkflowScriptChildResult {
        key: lane.to_string(),
        ok: true,
        stopped: false,
        agent: Some("researcher".to_string()),
        run_id: run_id.map(str::to_string),
        output: String::new(),
        error: None,
        detached: true,
        interrupted: false,
        structured_output: None,
        lane: None,
        terminal_outcome: None,
        requested_context: None,
        resolved_context: None,
        output_reference: None,
        recovery: None,
        output_path_mapping: None,
        external_adapter: None,
        resumability: None,
        continuation: None,
        artifact_paths: Vec::new(),
        results: Vec::new(),
    }
}

/// Persist a receipt for `run_id` covering `children`.
fn seed_receipt(run_paths: &RunPaths, run_id: &RunId, children: &[WorkflowScriptChildResult]) {
    let receipt = build_workflow_receipt(BuildWorkflowReceipt {
        workflow_run_id: &RunDirName::for_run(run_id),
        state: WorkflowReceiptState::Paused,
        children,
        host_steps: &[],
        workflow_children: None,
        resource: None,
        terminal_outcome: None,
        created_at: Some(1),
    })
    .expect("receipt builds");
    write_workflow_receipt(&run_paths.run_dir, &receipt).expect("receipt writes");
}

fn external_runner() -> crate::runner::status::ExternalCliRunnerStatus {
    crate::runner::status::normalize_external_cli_runner_status(&json!({
        "type": "external-cli",
        "command": "codex",
        "args": ["exec"],
    }))
    .expect("normalizes")
}

// -------------------------------------------------------------------------------------------
// Driver / idempotency
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn no_status_file_returns_false_without_writing() {
    let fixture = Fixture::new();
    tokio::fs::create_dir_all(&fixture.run_paths.run_dir)
        .await
        .expect("run dir");
    let result = child_result(CHILD);

    let reconciled = reconcile_detached_workflow_child_completion(completion(
        &fixture.run_paths,
        &result,
        None,
        &[],
    ))
    .await
    .expect("no status is not an error");

    assert!(!reconciled, "upstream `:181` returns false for no status");
    assert!(!fixture.run_paths.status.exists(), "no status.json written");
    assert!(!fixture.run_paths.events.exists(), "no events appended");
    assert!(
        !fixture.root.join("results").exists(),
        "no result published"
    );
}

#[tokio::test]
async fn a_non_paused_workflow_returns_false_and_writes_nothing() {
    let fixture = Fixture::new();
    let mut status = paused_workflow(
        &fixture.run_id,
        vec![detached_step("a", "lane.a", Some(CHILD))],
    );
    status.state = RunState::Complete;
    fixture.seed(&status).await;
    let before = tokio::fs::read(&fixture.run_paths.status)
        .await
        .expect("read");
    let result = child_result(CHILD);

    let reconciled = reconcile_detached_workflow_child_completion(completion(
        &fixture.run_paths,
        &result,
        None,
        &[],
    ))
    .await
    .expect("guarded, not an error");

    assert!(!reconciled, "the `settlement.rs:278-282` non-Paused guard");
    assert_eq!(
        tokio::fs::read(&fixture.run_paths.status)
            .await
            .expect("read"),
        before,
        "status.json untouched"
    );
    assert!(!fixture.run_paths.events.exists(), "no events appended");
}

#[tokio::test]
async fn reconciling_twice_is_a_no_op() {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("first reconcile")
    );
    let status_bytes = tokio::fs::read(&fixture.run_paths.status)
        .await
        .expect("read");
    let result_path = fixture
        .run_paths
        .resolve_result(&session(), &fixture.run_id)
        .await
        .expect("published");
    let result_bytes = tokio::fs::read(&result_path).await.expect("read");

    let again = reconcile_detached_workflow_child_completion(completion(
        &fixture.run_paths,
        &result,
        None,
        &[],
    ))
    .await
    .expect("replay is not an error");

    assert!(
        !again,
        "a settled workflow is no longer Paused, so it replays as a no-op"
    );
    assert_eq!(
        tokio::fs::read(&fixture.run_paths.status)
            .await
            .expect("read"),
        status_bytes,
        "status.json is byte-identical after the replay"
    );
    assert_eq!(
        tokio::fs::read(&result_path).await.expect("read"),
        result_bytes,
        "the published payload is byte-identical after the replay"
    );
}

#[tokio::test]
async fn a_child_matching_no_step_returns_false() {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some("someone-else"))],
        ))
        .await;
    let result = child_result(CHILD);

    let reconciled = reconcile_detached_workflow_child_completion(completion(
        &fixture.run_paths,
        &result,
        None,
        &[],
    ))
    .await
    .expect("no match is not an error");

    assert!(!reconciled, "upstream `:197`");
    assert!(!fixture.run_paths.events.exists());
}

// -------------------------------------------------------------------------------------------
// Settlement semantics — these are what wake the previously dead helpers
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_successful_detached_child_fails_the_workflow_closed_with_the_unsupported_continuation_error()
 {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );

    let settled = fixture.read_status().await;
    assert_eq!(
        settled.state,
        RunState::Failed,
        "a clean detached child must NEVER promote the workflow to complete"
    );
    assert_eq!(
        settled.error.as_deref(),
        Some(UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION)
    );
    assert!(!fixture.read_result().await.success);
}

#[tokio::test]
async fn another_open_detached_child_keeps_the_workflow_paused_and_logs_no_completion() {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![
                detached_step("a", "lane.a", Some(CHILD)),
                detached_step("b", "lane.b", Some("otherchild")),
            ],
        ))
        .await;
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );

    let settled = fixture.read_status().await;
    assert_eq!(
        settled.state,
        RunState::Paused,
        "the still-open sibling keeps the workflow paused"
    );
    assert!(
        !fixture
            .events()
            .await
            .contains("subagent.workflow.completed"),
        "publishing a completion for a still-paused workflow would announce a verdict the \
         workflow has not reached"
    );
    // The result IS still published — the settled lane's evidence must reach a reader.
    assert_eq!(fixture.read_result().await.state, RunState::Paused);
}

/// Item 1 of the TAIL batch: the reconciler's `subagent.workflow.completed` line is a LIFECYCLE
/// line (pi `appendDetachedWorkflowEvent`, a bare `fs.appendFileSync`,
/// `workflow-detach-reconcile.ts:161-168` @v0.68.0) and so is never capped — exactly as the runner's
/// `RunEventLog::write_line` treats its own lifecycle trail. The run's `events.jsonl` is pre-grown
/// (sparsely) to the full default 50 MiB budget, the state in which the runner's own lifecycle
/// trail has consumed it: the old silent `BoundedJsonlWriter` dropped the workflow's terminal
/// completion line there without a trace.
#[tokio::test]
async fn a_full_event_log_still_receives_the_workflows_terminal_completion_line() {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    let full = crate::jsonl::DEFAULT_JSONL_CAP_BYTES;
    std::fs::File::create(&fixture.run_paths.events)
        .expect("events.jsonl")
        .set_len(full)
        .expect("grow events.jsonl to the full budget");
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );

    let mut file = tokio::fs::File::open(&fixture.run_paths.events)
        .await
        .expect("events.jsonl");
    let len = file.metadata().await.expect("metadata").len();
    assert!(len > full, "a line was appended past the budget");
    file.seek(std::io::SeekFrom::Start(full))
        .await
        .expect("seek");
    let mut tail = String::new();
    file.read_to_string(&mut tail).await.expect("tail");
    let line: serde_json::Value =
        serde_json::from_str(tail.trim_end()).expect("one whole JSON line after the budget");
    assert_eq!(line["type"], "subagent.workflow.completed");
    assert_eq!(line["runId"], fixture.run_id.as_str());
    assert!(
        !tail.contains(crate::jsonl::TRUNCATED_EVENT_TYPE),
        "a lifecycle line is never refused, so no truncation marker is written for it"
    );
}

#[tokio::test]
async fn a_failed_sibling_outranks_this_childs_interruption() {
    let fixture = Fixture::new();
    let mut sibling = detached_step("b", "lane.b", Some("otherchild"));
    sibling.status = StepState::Failed;
    sibling.interrupted = false;
    sibling.error = Some("sibling exploded".to_string());
    sibling.telemetry.activity_state = None;
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD)), sibling],
        ))
        .await;
    let mut result = child_result(CHILD);
    result.interrupted = true;
    result.exit_code = 1;

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );

    assert_eq!(
        fixture.read_status().await.error.as_deref(),
        Some("sibling exploded"),
        "`PromotedWorkflowError`'s precedence: failed sibling outranks this child's interruption"
    );
}

#[tokio::test]
async fn a_stopped_detached_child_classifies_as_interrupted_evidence() {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    seed_receipt(
        &fixture.run_paths,
        &fixture.run_id,
        &[script_child("lane.a", Some(CHILD))],
    );
    let mut result = child_result(CHILD);
    // G77's stopped shape: `stopped: true` WITH `exitCode: 1` (`subagent-runner.ts:2358-2365`).
    // The exit code matters — `apply_detached_child_settlement`'s `succeeded` predicate ignores
    // `stopped` (`settlement.rs:264-270`), so a stopped child that also exited 0 settles as
    // COMPLETE and classifies `settled-awaiting-resume`; it is the non-zero exit that routes it
    // through the `interrupted || stopped` arm and leaves the interrupted evidence on the step
    // that `classify_workflow_settlement` then reads.
    result.stopped = true;
    result.exit_code = 1;

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );

    let receipt = fixture.read_receipt_json().await;
    assert_eq!(
        receipt["workflowResolution"],
        json!("interrupted-child"),
        "`classify_workflow_settlement` reached from production for the first time"
    );
    assert_eq!(receipt["state"], json!("failed"));

    // The complement: the same stop with a clean exit is NOT interrupted evidence.
    let clean = Fixture::new();
    clean
        .seed(&paused_workflow(
            &clean.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    seed_receipt(
        &clean.run_paths,
        &clean.run_id,
        &[script_child("lane.a", Some(CHILD))],
    );
    let mut clean_stop = child_result(CHILD);
    clean_stop.stopped = true;
    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &clean.run_paths,
            &clean_stop,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );
    assert_eq!(
        clean.read_receipt_json().await["workflowResolution"],
        json!("settled-awaiting-resume")
    );
}

// -------------------------------------------------------------------------------------------
// Identity back-fill (`identity.rs`)
// -------------------------------------------------------------------------------------------

fn live(run_id: &str, parent: &RunId, lane: &str) -> LiveForegroundControl {
    LiveForegroundControl {
        run_id: RunId::from_token(run_id.to_string()),
        parent_workflow_run_id: Some(parent.clone()),
        workflow_key: Some(key(lane)),
    }
}

#[test]
fn a_live_control_backfills_a_step_that_has_no_run_id() {
    let run_id = RunId::from_token("wf1");
    let status = paused_workflow(&run_id, vec![detached_step("a", "lane.a", None)]);
    let result = child_result(CHILD);
    let paths = RunPaths::for_run(Path::new("/tmp/a"), Path::new("/tmp/r"), &run_id);
    let controls = [live(CHILD, &run_id, "lane.a")];
    let lane = key("lane.a");

    let next =
        backfill_live_child_identity(status, &completion(&paths, &result, Some(&lane), &controls));

    assert_eq!(
        next.steps[0].run_id.as_ref().map(RunId::as_str),
        Some(CHILD)
    );
}

#[test]
fn two_ambiguous_same_key_live_attempts_backfill_nothing() {
    let run_id = RunId::from_token("wf1");
    let status = paused_workflow(&run_id, vec![detached_step("a", "lane.a", None)]);
    let result = child_result(CHILD);
    let paths = RunPaths::for_run(Path::new("/tmp/a"), Path::new("/tmp/r"), &run_id);
    let controls = [
        live(CHILD, &run_id, "lane.a"),
        live("rival", &run_id, "lane.a"),
    ];
    let lane = key("lane.a");

    let next =
        backfill_live_child_identity(status, &completion(&paths, &result, Some(&lane), &controls));

    assert!(
        next.steps[0].run_id.is_none(),
        "two candidates resolve to NOTHING, never to the first"
    );
}

#[test]
fn a_completion_matching_only_on_workflow_key_is_stale_and_ignored() {
    let run_id = RunId::from_token("wf1");
    let status = paused_workflow(&run_id, vec![detached_step("a", "lane.a", None)]);
    let result = child_result(CHILD);
    let paths = RunPaths::for_run(Path::new("/tmp/a"), Path::new("/tmp/r"), &run_id);
    let lane = key("lane.a");

    // No live control at all…
    let next = backfill_live_child_identity(
        status.clone(),
        &completion(&paths, &result, Some(&lane), &[]),
    );
    assert!(next.steps[0].run_id.is_none());

    // …and a live control for the lane whose run is somebody ELSE.
    let controls = [live("a-different-child", &run_id, "lane.a")];
    let next =
        backfill_live_child_identity(status, &completion(&paths, &result, Some(&lane), &controls));
    assert!(
        next.steps[0].run_id.is_none(),
        "`:185` compares the control's run id to the settling child"
    );
}

#[test]
fn a_completion_with_no_workflow_key_never_backfills() {
    let run_id = RunId::from_token("wf1");
    let status = paused_workflow(&run_id, vec![detached_step("a", "lane.a", None)]);
    let result = child_result(CHILD);
    let paths = RunPaths::for_run(Path::new("/tmp/a"), Path::new("/tmp/r"), &run_id);
    let controls = [live(CHILD, &run_id, "lane.a")];

    let next = backfill_live_child_identity(status, &completion(&paths, &result, None, &controls));

    assert!(
        next.steps[0].run_id.is_none(),
        "clause 0 (`:182`): no workflow key ⇒ no matching controls ⇒ no back-fill"
    );
}

#[test]
fn a_step_that_already_has_a_session_file_is_not_backfilled() {
    let run_id = RunId::from_token("wf1");
    let paths = RunPaths::for_run(Path::new("/tmp/a"), Path::new("/tmp/r"), &run_id);
    let result = child_result(CHILD);
    let controls = [live(CHILD, &run_id, "lane.a")];
    let lane = key("lane.a");

    // Clause 3, one sub-clause at a time: each of the four alone must refuse the claim.
    let mut already_run = detached_step("a", "lane.a", Some("already"));
    already_run.session_file = None;

    let mut with_session_file = detached_step("a", "lane.a", None);
    with_session_file.session_file = Some(PathBuf::from("/tmp/session.jsonl"));

    let mut not_paused = detached_step("a", "lane.a", None);
    not_paused.status = StepState::Running;

    let mut not_needing_attention = detached_step("a", "lane.a", None);
    not_needing_attention.telemetry.activity_state = None;

    for (label, step, expected) in [
        ("run_id already present", already_run, Some("already")),
        ("session_file present", with_session_file, None),
        ("status is not paused", not_paused, None),
        (
            "activity state is not needs_attention",
            not_needing_attention,
            None,
        ),
    ] {
        let status = paused_workflow(&run_id, vec![step]);
        let next = backfill_live_child_identity(
            status,
            &completion(&paths, &result, Some(&lane), &controls),
        );
        assert_eq!(
            next.steps[0].run_id.as_ref().map(RunId::as_str),
            expected,
            "clause 3 sub-clause: {label}"
        );
    }
}

// -------------------------------------------------------------------------------------------
// Receipt (`receipt.rs`)
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_absent_receipt_reconciles_to_none_not_an_error() {
    let fixture = Fixture::new();
    let status = paused_workflow(
        &fixture.run_id,
        vec![detached_step("a", "lane.a", Some(CHILD))],
    );
    fixture.seed(&status).await;
    let result = child_result(CHILD);

    // A LIVE paused workflow has a status.json, so the typed reader reports `MayStillBeActive`,
    // NOT `NotFound` — and both must map to `Ok(None)`.
    let raw = read_workflow_receipt(&fixture.async_root(), &RunDirName::for_run(&fixture.run_id));
    assert!(
        matches!(raw, Err(WorkflowReceiptError::MayStillBeActive { .. })),
        "the absent-receipt arm for a live paused run is MayStillBeActive: {raw:?}"
    );

    let published = reconcile_workflow_receipt(&status, CHILD, &result, &fixture.run_paths, None)
        .await
        .expect("an absent receipt reconciles fine");
    assert!(published.is_none());
}

#[tokio::test]
async fn a_step_with_no_workflow_key_raises_the_verbatim_cannot_identify_message() {
    let fixture = Fixture::new();
    let mut step = detached_step("a", "lane.a", Some(CHILD));
    step.workflow_key = None;
    let status = paused_workflow(&fixture.run_id, vec![step]);
    fixture.seed(&status).await;
    seed_receipt(
        &fixture.run_paths,
        &fixture.run_id,
        &[script_child("lane.a", Some(CHILD))],
    );
    let result = child_result(CHILD);

    let error = reconcile_workflow_receipt(&status, CHILD, &result, &fixture.run_paths, None)
        .await
        .expect_err("an unidentifiable child is a fault, never a silent skip");

    assert_eq!(
        error,
        format!(
            "Workflow receipt '{}' cannot identify detached child '{CHILD}' by stable key.",
            fixture.run_id
        )
    );
}

#[tokio::test]
async fn a_receipt_with_no_entry_for_the_key_raises_the_verbatim_no_detached_child_key_message() {
    let fixture = Fixture::new();
    let status = paused_workflow(
        &fixture.run_id,
        vec![detached_step("a", "lane.a", Some(CHILD))],
    );
    fixture.seed(&status).await;
    seed_receipt(
        &fixture.run_paths,
        &fixture.run_id,
        &[script_child("lane.z", Some("someone-else"))],
    );
    let result = child_result(CHILD);

    let error = reconcile_workflow_receipt(&status, CHILD, &result, &fixture.run_paths, None)
        .await
        .expect_err("a missing entry is a fault");

    assert_eq!(
        error,
        format!(
            "Workflow receipt '{}' has no detached child key 'lane.a'.",
            fixture.run_id
        )
    );
}

/// Write a structurally invalid receipt, so the reader fails with `Invalid` rather than the two
/// absent-file variants.
async fn seed_malformed_receipt(fixture: &Fixture) {
    tokio::fs::write(
        fixture.run_paths.run_dir.join(WORKFLOW_RECEIPT_FILE),
        b"{\"version\":1}",
    )
    .await
    .expect("write malformed receipt");
}

#[tokio::test]
async fn a_malformed_receipt_still_publishes_the_completion_with_evidence_persistence_failed() {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    seed_malformed_receipt(&fixture).await;
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("a receipt fault is captured, never propagated")
    );

    let settled = fixture.read_status().await;
    assert_eq!(settled.state, RunState::Failed);
    assert!(
        settled
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with(crate::workflows::EVIDENCE_PERSISTENCE_FAILED)),
        "the fault demotes the status: {:?}",
        settled.error
    );
    // …and the available child evidence is STILL published.
    let published = fixture.read_result().await;
    assert_eq!(published.results.len(), 1);
    assert!(published.workflow_receipt.is_none());
}

#[tokio::test]
async fn a_receipt_write_failure_is_journalled_and_does_not_fail_the_reconcile() {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    seed_malformed_receipt(&fixture).await;
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("the reconcile succeeds despite the receipt fault")
    );

    let events = fixture.events().await;
    let line = events
        .lines()
        .find(|line| line.contains("subagent.workflow.receipt_write_failed"))
        .expect("the receipt fault is journalled (`:265-273`)");
    let event: serde_json::Value = serde_json::from_str(line).expect("valid json line");
    assert_eq!(event["reconciledFromDetachedChild"], json!(CHILD));
    assert!(
        event["error"]
            .as_str()
            .is_some_and(|e| e.starts_with("Failed to reconcile async workflow receipt:")),
        "{event}"
    );
}

#[tokio::test]
async fn an_external_cli_adapter_overrides_a_resumable_verdict() {
    let fixture = Fixture::new();
    let status = paused_workflow(
        &fixture.run_id,
        vec![detached_step("a", "lane.a", Some(CHILD))],
    );
    fixture.seed(&status).await;
    // A terminal child WITH a transcript: the probe alone would say `resumable`.
    fixture.seed_child(CHILD, RunState::Complete, true).await;
    seed_receipt(
        &fixture.run_paths,
        &fixture.run_id,
        &[script_child("lane.a", Some(CHILD))],
    );
    let mut result = child_result(CHILD);
    result.session_file = Some(fixture.root.join("session.jsonl"));
    result.runner = Some(external_runner());

    let published = reconcile_workflow_receipt(&status, CHILD, &result, &fixture.run_paths, None)
        .await
        .expect("reconciles")
        .expect("a receipt exists");

    let entry = published.receipt.entry(&key("lane.a")).expect("entry");
    let adapter_reason = entry
        .external_adapter
        .as_ref()
        .and_then(|adapter| adapter.get("nonResumableReason"))
        .and_then(serde_json::Value::as_str)
        .expect("the adapter metadata carries the reason");
    match &entry.resume {
        WorkflowReceiptResume::NotResumable { reason, .. } => assert_eq!(
            reason, adapter_reason,
            "`:121` — the adapter override is last and absolute"
        ),
        other => panic!("external adapters are never resumable: {other:?}"),
    }
}

#[tokio::test]
async fn mixed_pi_and_external_entries_keep_their_own_computed_resumability() {
    let fixture = Fixture::new();
    let status = paused_workflow(
        &fixture.run_id,
        vec![
            detached_step("a", "lane.a", Some(CHILD)),
            detached_step("b", "lane.b", Some("nativechild")),
        ],
    );
    fixture.seed(&status).await;
    fixture.seed_child(CHILD, RunState::Complete, true).await;
    let mut native = script_child("lane.b", Some("nativechild"));
    native.resumability = Some(crate::workflows::WorkflowResumability::Resumable);
    seed_receipt(
        &fixture.run_paths,
        &fixture.run_id,
        &[script_child("lane.a", Some(CHILD)), native],
    );
    let mut result = child_result(CHILD);
    result.runner = Some(external_runner());

    let published = reconcile_workflow_receipt(&status, CHILD, &result, &fixture.run_paths, None)
        .await
        .expect("reconciles")
        .expect("a receipt exists");

    assert!(
        !published
            .receipt
            .entry(&key("lane.a"))
            .expect("settling entry")
            .resume
            .is_resumable(),
        "only the entry that carries an adapter is forced"
    );
    assert!(
        published
            .receipt
            .entry(&key("lane.b"))
            .expect("sibling entry")
            .resume
            .is_resumable(),
        "the native sibling keeps the verdict it was built with"
    );
}

#[tokio::test]
async fn a_detached_timeout_outcome_stays_on_its_own_entry_and_is_not_promoted_workflow_wide() {
    let fixture = Fixture::new();
    let status = paused_workflow(
        &fixture.run_id,
        vec![detached_step("a", "lane.a", Some(CHILD))],
    );
    fixture.seed(&status).await;
    seed_receipt(
        &fixture.run_paths,
        &fixture.run_id,
        &[script_child("lane.a", Some(CHILD))],
    );
    let mut result = child_result(CHILD);
    result.timed_out = true;

    let published = reconcile_workflow_receipt(
        &status,
        CHILD,
        &result,
        &fixture.run_paths,
        Some(WorkflowTerminalResolution::InterruptedChild),
    )
    .await
    .expect("reconciles")
    .expect("a receipt exists");

    assert_eq!(
        published
            .receipt
            .entry(&key("lane.a"))
            .expect("entry")
            .terminal_outcome,
        Some(WorkflowTerminalOutcome::Partial {
            reason: WorkflowTerminalOutcomeReason::Timeout
        })
    );
    assert_eq!(
        published.receipt.terminal_outcome, None,
        "`:120` merges onto the ENTRY; the workflow-wide outcome is untouched"
    );
}

#[tokio::test]
async fn a_resumable_entry_keeps_its_existing_latest_run_id_and_falls_back_to_the_child() {
    let fixture = Fixture::new();
    let status = paused_workflow(
        &fixture.run_id,
        vec![detached_step("a", "lane.a", Some(CHILD))],
    );
    fixture.seed(&status).await;
    fixture.seed_child(CHILD, RunState::Complete, true).await;

    // The entry already records a retained run — upstream's `entry.latestRunId ?? childRunId`
    // keeps it.
    let mut retained = script_child("lane.a", Some("earlier-attempt"));
    retained.resumability = Some(crate::workflows::WorkflowResumability::Resumable);
    seed_receipt(&fixture.run_paths, &fixture.run_id, &[retained]);
    let result = child_result(CHILD);
    let published = reconcile_workflow_receipt(&status, CHILD, &result, &fixture.run_paths, None)
        .await
        .expect("reconciles")
        .expect("a receipt exists");
    assert_eq!(
        published
            .receipt
            .entry(&key("lane.a"))
            .expect("entry")
            .resume
            .latest_run_id()
            .map(|id| id.as_str().to_string()),
        Some("earlier-attempt".to_string())
    );

    // With no retained run recorded, the settling child's own id is the fallback.
    seed_receipt(
        &fixture.run_paths,
        &fixture.run_id,
        &[script_child("lane.a", None)],
    );
    let published = reconcile_workflow_receipt(&status, CHILD, &result, &fixture.run_paths, None)
        .await
        .expect("reconciles")
        .expect("a receipt exists");
    let entry = published.receipt.entry(&key("lane.a")).expect("entry");
    assert_eq!(
        entry
            .resume
            .latest_run_id()
            .map(|id| id.as_str().to_string()),
        Some(CHILD.to_string())
    );
    assert!(
        entry.resume.is_resumable(),
        "a terminal child with a transcript is resumable"
    );
}

// -------------------------------------------------------------------------------------------
// Children (`children.rs`)
// -------------------------------------------------------------------------------------------

fn existing_result(run_id: &RunId, results: Vec<SingleResult>) -> ResultFile {
    ResultFile {
        schedule_origin: None,
        id: run_id.clone(),
        run_id: run_id.clone(),
        agent: "workflow".to_string(),
        mode: RunMode::Workflow,
        state: RunState::Paused,
        success: false,
        cwd: PathBuf::from("/tmp/scope8"),
        session_file: None,
        session_id: Some(session()),
        completion_owner_id: Some(owner()),
        results,
        workflow_children: None,
        workflow_receipt: None,
    }
}

#[test]
fn arm_a_rewrites_only_the_matching_child_and_clears_its_detached_flag() {
    let run_id = RunId::from_token("wf1");
    let status = paused_workflow(&run_id, vec![detached_step("a", "lane.a", Some(CHILD))]);
    let mut sibling = child_result("siblingrun");
    sibling.final_output = Some("sibling output".to_string());
    let target = child_result(CHILD);
    let existing = existing_result(&run_id, vec![target.clone(), sibling.clone()]);
    let mut result = child_result(CHILD);
    result.final_output = Some("fresh output".to_string());

    let children = workflow_result_children(&status, CHILD, &result, Some(&existing), None);

    assert_eq!(children.len(), 2);
    assert_eq!(children[0].final_output.as_deref(), Some("fresh output"));
    assert!(!children[0].detached, "`:66` — `detached: undefined`");
    assert_eq!(children[0].output_state, SubagentOutputState::Present);
    assert_eq!(
        children[1], sibling,
        "`:58` — every sibling passes through untouched"
    );
}

#[test]
fn arm_a_preserves_sibling_output_path_mappings() {
    // The mapping's carrier in cyrup is `StepStatus::output_path_mapping` (SCOPE_8 §W-7:
    // `SingleResult` has no such field), so "preserved" means the rebuild touches neither the
    // sibling's published entry nor any step's mapping.
    let run_id = RunId::from_token("wf1");
    let mut sibling_step = detached_step("b", "lane.b", Some("siblingrun"));
    sibling_step.output_path_mapping = Some(crate::workflows::WorkflowOutputPathMapping {
        requested_path: "report.md".to_string(),
        saved_path: "/tmp/out/report.md".to_string(),
    });
    let status = paused_workflow(
        &run_id,
        vec![
            detached_step("a", "lane.a", Some(CHILD)),
            sibling_step.clone(),
        ],
    );
    let sibling = child_result("siblingrun");
    let existing = existing_result(&run_id, vec![child_result(CHILD), sibling.clone()]);
    let result = child_result(CHILD);

    let children = workflow_result_children(&status, CHILD, &result, Some(&existing), None);

    assert_eq!(children[1], sibling);
    assert_eq!(
        status.steps[1].output_path_mapping,
        sibling_step.output_path_mapping
    );
}

/// A receipt whose one entry carries an output reference and a timeout outcome, for the Arm-B
/// back-fill cases.
fn receipt_with_entry_evidence(run_id: &RunId) -> WorkflowReceipt {
    let mut child = script_child("lane.b", Some("siblingrun"));
    child.output_reference = Some("/tmp/out/sibling.md".to_string());
    child.terminal_outcome = Some(WorkflowTerminalOutcome::Partial {
        reason: WorkflowTerminalOutcomeReason::Timeout,
    });
    build_workflow_receipt(BuildWorkflowReceipt {
        workflow_run_id: &RunDirName::for_run(run_id),
        state: WorkflowReceiptState::Paused,
        children: &[child],
        host_steps: &[],
        workflow_children: None,
        resource: None,
        terminal_outcome: None,
        created_at: Some(1),
    })
    .expect("receipt builds")
}

#[test]
fn arm_b_rebuilds_from_steps_and_backfills_output_reference_from_the_receipt_entry() {
    let run_id = RunId::from_token("wf1");
    let status = paused_workflow(
        &run_id,
        vec![
            detached_step("a", "lane.a", Some(CHILD)),
            detached_step("b", "lane.b", Some("siblingrun")),
        ],
    );
    let receipt = receipt_with_entry_evidence(&run_id);
    let result = child_result(CHILD);

    let children = workflow_result_children(&status, CHILD, &result, None, Some(&receipt));

    assert_eq!(children.len(), 2, "rebuilt from `status.steps`");
    assert_eq!(
        children[1].saved_output_path.as_deref(),
        Some("/tmp/out/sibling.md"),
        "`:88` — the sibling's reference comes off its receipt entry"
    );
    assert_eq!(
        children[0].final_output.as_deref(),
        Some("done"),
        "only the settling child carries the fresh output (`:86`)"
    );
    assert_eq!(children[1].final_output.as_deref(), Some(""));
}

#[test]
fn arm_b_backfills_terminal_outcome_from_the_receipt_entry() {
    let run_id = RunId::from_token("wf1");
    let status = paused_workflow(
        &run_id,
        vec![
            detached_step("a", "lane.a", Some(CHILD)),
            detached_step("b", "lane.b", Some("siblingrun")),
        ],
    );
    let receipt = receipt_with_entry_evidence(&run_id);
    let result = child_result(CHILD);

    let children = workflow_result_children(&status, CHILD, &result, None, Some(&receipt));

    // `SingleResult` has no `terminal_outcome`; the outcome is carried on the two signals it is
    // derived FROM, so a reader re-deriving it gets the same value back.
    assert!(
        children[1].timed_out,
        "`:91` — Partial{{timeout}} off the entry"
    );
    assert!(
        !children[0].timed_out,
        "the settling child did not time out"
    );
}

/// `:89` + `:222-227` — a NON-settling step's `output_path_mapping` (cyrup's carrier for
/// upstream's `outputPathMapping` key; [`SingleResult`] has no such field, SCOPE_8 §W-7) survives
/// the Arm-B rebuild and reaches the operator through the settled summary this reconciler
/// journals.
///
/// Driven through the PRODUCTION reconcile. The earlier shape of this test called
/// `workflow_output_path_mapping_summary` over the very `status.steps` it had just built by hand
/// — proving the formatter's own arithmetic, never the rebuild — and asserted nothing about the
/// children it computed beyond their count.
///
/// The sibling lane is already settled so that this child is the LAST open one: a still-open
/// sibling keeps the workflow `paused`, and a paused workflow journals no completion line at all
/// (`another_open_detached_child_keeps_the_workflow_paused_and_logs_no_completion`).
#[tokio::test]
async fn a_sibling_steps_output_path_mapping_survives_the_rebuild_and_reaches_the_summary() {
    let fixture = Fixture::new();
    let mapping = crate::workflows::WorkflowOutputPathMapping {
        requested_path: "report.md".to_string(),
        saved_path: "/tmp/out/report.md".to_string(),
    };
    let mut sibling = detached_step("b", "lane.b", Some("siblingrun"));
    sibling.status = StepState::Complete;
    sibling.telemetry.activity_state = None;
    sibling.output_path_mapping = Some(mapping.clone());
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD)), sibling],
        ))
        .await;
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );

    // Arm B ran in production: with no prior payload on disk the children are rebuilt from
    // `status.steps`, one per step, only the settling child carrying this result's output.
    let published = fixture.read_result().await;
    assert_eq!(published.results.len(), 2, "rebuilt from `status.steps`");
    assert_eq!(published.results[0].final_output.as_deref(), Some("done"));
    assert_eq!(
        published.results[1]
            .child_run_id
            .as_ref()
            .map(RunId::as_str),
        Some("siblingrun")
    );
    assert_eq!(published.results[1].final_output.as_deref(), Some(""));
    assert_eq!(
        published.results[1].exit_code, 0,
        "`:84` — a `Complete` step publishes as a success"
    );

    // The settled status kept the sibling's mapping — nothing in the rebuild clears it…
    let settled = fixture.read_status().await;
    assert_eq!(
        settled.steps[1].output_path_mapping.as_ref(),
        Some(&mapping)
    );

    // …and the mapping clause is concatenated onto the summary the completion line carries, which
    // is the only place an operator ever reads it (SCOPE_8 §Y-5).
    let events = fixture.events().await;
    let line = events
        .lines()
        .find(|line| line.contains("subagent.workflow.completed"))
        .expect("a promoted workflow journals its completion");
    let event: serde_json::Value = serde_json::from_str(line).expect("valid json line");
    let summary = event["summary"]
        .as_str()
        .expect("the completion line carries the settled summary");
    assert!(
        summary.ends_with(
            " Output path mappings: 'lane.b': requested report.md -> saved /tmp/out/report.md."
        ),
        "{summary}"
    );
}

#[test]
fn an_all_zero_usage_is_suppressed() {
    assert!(usage_with_value(&cyrup_core::Usage::default()).is_none());
    let usage = cyrup_core::Usage {
        input: 7,
        ..cyrup_core::Usage::default()
    };
    assert!(usage_with_value(&usage).is_some());

    // …and the suppression is observable: an all-zero result must not overwrite a recorded figure.
    let run_id = RunId::from_token("wf1");
    let status = paused_workflow(&run_id, vec![detached_step("a", "lane.a", Some(CHILD))]);
    let mut recorded = child_result(CHILD);
    recorded.usage = usage.clone();
    let existing = existing_result(&run_id, vec![recorded]);
    let result = child_result(CHILD);

    let children = workflow_result_children(&status, CHILD, &result, Some(&existing), None);
    assert_eq!(children[0].usage, usage);
}

#[test]
fn the_success_predicate_ignores_stopped() {
    let run_id = RunId::from_token("wf1");
    let status = paused_workflow(&run_id, vec![detached_step("a", "lane.a", Some(CHILD))]);
    let existing = existing_result(&run_id, vec![child_result(CHILD)]);
    let mut result = child_result(CHILD);
    result.stopped = true;

    let children = workflow_result_children(&status, CHILD, &result, Some(&existing), None);

    assert_eq!(
        children[0].exit_code, 0,
        "`:63`'s predicate does not consult `stopped`, so a clean stopped child still SUCCEEDS"
    );
    assert!(
        children[0].stopped,
        "…while the stop itself is still recorded"
    );
    assert!(
        children[0].interrupted,
        "…as is the interruption it implies"
    );
}

/// Upstream `:198-201` — the settled step keeps the mapping derived from THIS result's
/// task/output, which is what makes `workflow_output_path_mapping_summary` (dead in-tree until
/// now) render anything at all.
#[tokio::test]
async fn the_settled_step_keeps_the_output_path_mapping_derived_from_this_results_task() {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    let mut result = child_result(CHILD);
    result.task =
        "Investigate the crash.\n\nWrite your findings to exactly this path: report.md".to_string();
    result.saved_output_path = Some("/tmp/out/report.md".to_string());

    assert_eq!(
        output_path_mapping_from_task(&result.task, result.saved_output_path.as_deref()),
        Some(crate::workflows::WorkflowOutputPathMapping {
            requested_path: "report.md".to_string(),
            saved_path: "/tmp/out/report.md".to_string(),
        })
    );

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );

    let settled = fixture.read_status().await;
    assert_eq!(
        settled.steps[0]
            .output_path_mapping
            .as_ref()
            .map(|mapping| mapping.saved_path.as_str()),
        Some("/tmp/out/report.md")
    );
}

/// An ABSOLUTE requested path that normalizes onto the saved path is not a remap at all.
#[test]
fn an_absolute_requested_path_equal_to_the_saved_path_is_not_a_mapping() {
    assert_eq!(
        output_path_mapping_from_task(
            "Write your findings to exactly this path: /tmp/out/./report.md",
            Some("/tmp/out/report.md"),
        ),
        None
    );
}

// -------------------------------------------------------------------------------------------
// Identity propagation (§2)
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_published_result_carries_session_id_and_completion_owner_id() {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );

    let published = fixture.read_result().await;
    assert_eq!(published.session_id, Some(session()));
    assert_eq!(published.completion_owner_id, Some(owner()));
    assert_eq!(published.agent, "workflow");
    assert_eq!(published.mode, RunMode::Workflow);
}

#[tokio::test]
async fn the_session_falls_back_to_the_existing_result_file_when_the_status_has_none() {
    let fixture = Fixture::new();
    let mut status = paused_workflow(
        &fixture.run_id,
        vec![detached_step("a", "lane.a", Some(CHILD))],
    );
    status.session_id = None;
    fixture.seed(&status).await;
    // The prior payload — written where a build predating the owned partition published it, which
    // is the only location addressable without a session.
    tokio::fs::create_dir_all(&fixture.run_paths.results_dir)
        .await
        .expect("results dir");
    tokio::fs::write(
        &fixture.run_paths.legacy_result_root,
        serde_json::to_vec(&existing_result(&fixture.run_id, vec![child_result(CHILD)]))
            .expect("serialize"),
    )
    .await
    .expect("write prior payload");
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("the `next -> existing` fallback keeps the result attributable")
    );

    assert_eq!(
        fixture.read_result().await.session_id,
        Some(session()),
        "`:244` — `next.sessionId ?? existing.sessionId`, in that order"
    );
}

#[tokio::test]
async fn a_reconciled_result_with_no_session_anywhere_is_a_hard_error() {
    let fixture = Fixture::new();
    let mut status = paused_workflow(
        &fixture.run_id,
        vec![detached_step("a", "lane.a", Some(CHILD))],
    );
    status.session_id = None;
    fixture.seed(&status).await;
    let result = child_result(CHILD);

    let error = reconcile_detached_workflow_child_completion(completion(
        &fixture.run_paths,
        &result,
        None,
        &[],
    ))
    .await
    .expect_err("an unattributable result cannot be indexed and must fail loudly");

    assert!(
        matches!(error, crate::error::SubagentError::Management(_)),
        "{error:?}"
    );
    assert_eq!(
        fixture.read_status().await.state,
        RunState::Failed,
        "R-SA-077's independence: status.json is written FIRST, so the settlement is still visible"
    );
}

// -------------------------------------------------------------------------------------------
// Write ordering / SUBTASK3
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_terminal_index_marker_is_written_for_a_settled_workflow() {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );

    let index_root = fixture.async_root().join(".terminal-runs");
    let mut session_dirs = tokio::fs::read_dir(&index_root)
        .await
        .expect("the terminal index root exists");
    let session_dir = session_dirs
        .next_entry()
        .await
        .expect("read")
        .expect("one session partition");
    let mut markers = tokio::fs::read_dir(session_dir.path())
        .await
        .expect("session partition");
    assert!(
        markers.next_entry().await.expect("read").is_some(),
        "one marker per settled run"
    );
}

/// SCOPE_8 deferred the ACTIVE run index to SCOPE_9 and guarded the deferral with
/// `no_active_run_index_is_created`. SCOPE_9 has landed it: this call site now routes through
/// `update_active_run_index`, so the index root DOES appear — and what must be asserted instead is
/// the property that matters, that the settled run holds NO active marker once the terminal one is
/// written.
#[tokio::test]
async fn the_settlement_releases_the_active_run_index_marker() {
    let fixture = Fixture::new();
    fixture
        .seed(&paused_workflow(
            &fixture.run_id,
            vec![detached_step("a", "lane.a", Some(CHILD))],
        ))
        .await;
    let result = child_result(CHILD);

    assert!(
        reconcile_detached_workflow_child_completion(completion(
            &fixture.run_paths,
            &result,
            None,
            &[]
        ))
        .await
        .expect("reconciles")
    );

    let mut entries = tokio::fs::read_dir(fixture.async_root())
        .await
        .expect("async root");
    let mut names: Vec<String> = Vec::new();
    while let Some(entry) = entries.next_entry().await.expect("read") {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    assert_eq!(
        names,
        vec![
            ".active-runs".to_string(),
            ".terminal-runs".to_string(),
            fixture.run_id.as_str().to_string()
        ],
        "both index roots, and nothing else, sit beside the run directory"
    );
    assert_eq!(
        crate::background::active_run_index::read_active_run_index(&fixture.async_root()).await,
        Some(Vec::new()),
        "a settled run must hold no active marker — the terminal index is written FIRST and the \
         active entry released after it, so a process dying between the two leaves the run in both \
         indexes rather than in neither"
    );
}

// -------------------------------------------------------------------------------------------
// SUBTASK4 — the two thin entry points
// -------------------------------------------------------------------------------------------

#[test]
fn promote_paused_workflow_if_settled_delegates_with_the_crate_clock() {
    let run_id = RunId::from_token("wf1");
    let mut settled_step = detached_step("a", "lane.a", Some(CHILD));
    settled_step.status = StepState::Complete;
    settled_step.telemetry.activity_state = None;
    let status = paused_workflow(&run_id, vec![settled_step]);
    let before = crate::time::now_epoch_millis();

    let promoted = promote_paused_workflow_if_settled(&status, &[])
        .expect("promotes")
        .expect("every lane settled");

    assert_eq!(promoted.state, RunState::Failed);
    assert_eq!(
        promoted.error.as_deref(),
        Some(UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION)
    );
    assert!(
        promoted.ended_at.is_some_and(|ended| ended >= before),
        "the wrapper supplies the crate clock rather than taking a `now` argument"
    );
}

#[test]
fn apply_detached_child_to_paused_workflow_delegates_unchanged() {
    let run_id = RunId::from_token("wf1");
    let status = paused_workflow(&run_id, vec![detached_step("a", "lane.a", Some(CHILD))]);
    let input = DetachedChildSettlement {
        child_run_id: CHILD,
        exit_code: Some(0),
        error: None,
        interrupted: false,
        session_file: None,
        session_name: None,
        stopped: false,
        workflow_key: None,
        now: 1234,
    };
    let direct = crate::workflows::apply_detached_child_settlement(
        &status,
        DetachedChildSettlement {
            child_run_id: CHILD,
            exit_code: Some(0),
            error: None,
            interrupted: false,
            session_file: None,
            session_name: None,
            stopped: false,
            workflow_key: None,
            now: 1234,
        },
        &[],
    )
    .expect("settles");

    let through_wrapper =
        apply_detached_child_to_paused_workflow(&status, input, &[]).expect("settles");

    assert_eq!(
        through_wrapper, direct,
        "the wrapper adds nothing of its own"
    );
}

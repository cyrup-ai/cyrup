//! G82 — authorship comes from the CHILD'S OWN successful write, not from the output file on disk.
//!
//! `pi-subagents:v0.43.0:src/runs/shared/single-output.ts:14-52` `extractChildWrittenOutput`, wired
//! at `runs/foreground/execution.ts:1680-1701` into `evaluateAcceptance`'s `fileOutput` input
//! (`runs/shared/acceptance.ts:1214-1220`), where `parseAcceptanceReportSources`
//! (`acceptance.ts:753-771`) searches it for the acceptance report — BEFORE the assistant output
//! when `outputMode: "file-only"` makes it `authoritative`.
//!
//! Upstream's own doc on the function states why it cannot be replaced by reading the path:
//!
//! > Unlike reading the path from disk, this cannot be polluted by a sibling run writing the same
//! > path (#420); requiring the successful tool result keeps failed, cancelled, or unanswered write
//! > calls from counting as authored output.
//!
//! cyrup had only the disk side: `exec/output.rs::resolve_output_handoff` is an mtime/size stat
//! heuristic (its own doc says so), so the acceptance gate saw whatever happened to be at the path
//! and never what THIS child actually authored. A `file-only` child whose artifact carried the
//! `acceptance-report` block therefore had it ignored, and its run was `rejected` for a missing
//! attestation it had in fact produced.
//!
//! Both halves are asserted against a REAL fixture child subprocess driven over NDJSON:
//! the negative control (a child that emits no write call at all is rejected) and the positive case
//! (the same run, with the report delivered through the child's own successful `write` call,
//! reaches `attested`).
//!
//! Separate compilation unit from `lib.rs`, so NOT bound by that crate's `#![forbid(unsafe_code)]`;
//! the `unsafe` env mutation (Rust 2024 requires it for `std::env::set_var`/`remove_var`) is scoped
//! and serialized under [`ENV_MUTATION_LOCK`], exactly like every other integration test here.
//!
//! Gated on `test-fixtures` (the `cyrup-subagent-fixture` `[[bin]]`'s own `required-features`
//! gate): without it the fixture is never built and this file compiles to an empty, passing test
//! list.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::Mutex;

use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::acceptance::AcceptanceStatus;
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions, SingleResult, run_sync};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;

static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

/// The artifact the child authors: prose plus the fenced `acceptance-report` block the inferred
/// `attested` contract asks for.
const CHILD_ARTIFACT: &str = "The flake is a timing race in the reaper.\n\n\
     ```acceptance-report\n\
     {\"reviewFindings\": [\"blocker: reaper.rs:44 - unsynchronized wait\"], \"residualRisks\": [\"none\"]}\n\
     ```";

/// The receipt the child leaves in its final assistant message. Deliberately carries NO
/// `acceptance-report` block: in `file-only` mode the ARTIFACT is the answer, and the receipt is
/// just a pointer. This is what makes the test discriminating — the only place the report exists
/// is inside the child's own `write` call.
const CHILD_RECEIPT: &str = "Wrote the review to the configured output path.";

fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
}

fn agent_config(name: &str) -> AgentConfig {
    AgentConfig {
        name: name.to_string(),
        model: Some(ModelId::from("fixture-model")),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        // The completion-mutation guard is a separate gate; disabled so this run's outcome is
        // decided by the child's exit code and the acceptance ledger alone.
        completion_guard: Some(false),
        max_output: OutputCap::default(),
        max_subagent_depth: None,
        memory: None,
        tool_budget: None,
        depth: DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
    }
}

/// `acceptance: None` — no explicit policy, so `run_sync` infers the contract through
/// `AcceptanceContract::heuristic_default`, which for a research agent yields `attested` and
/// therefore genuinely requires an `acceptance-report` block from somewhere.
fn run_options(cwd: &Path, output_path: &Path) -> RunOptions {
    RunOptions {
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        output_path: Some(output_path.to_path_buf()),
        output_mode: OutputMode::FileOnly,
        reads: None,
        structured_output_schema: None,
        model_override: ModelOverride::Inherit,
        preferred_provider: None,
        available_models: vec![ModelId::from("fixture-model")],
        cancel: CancelToken::new(),
        interrupt: CancelToken::new(),
        share: None,
        session_dir: None,
        skills: None,
        runtime_cwd: None,
        include_progress: None,
        agent_scope: None,
        acceptance: None,
        fork_context: ForkContext::fresh(),
        live_events: None,
        parent_session_id: None,
        clarify: None,
        orchestrator_intercom_target: None,
        run_id: None,
        child_index: None,
        steer_inbox_dir: None,
        control_config: None,
        on_control_event: None,
        model_scope: None,
        artifacts_dir: None,
    }
}

fn message_end_line(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 2,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

fn write_start_line(call_id: &str, path: &Path, content: &str) -> String {
    serde_json::json!({
        "type": "tool_execution_start",
        "toolCallId": call_id,
        "toolName": "write",
        "args": {"path": path.display().to_string(), "content": content}
    })
    .to_string()
}

fn tool_end_line(call_id: &str, is_error: bool) -> String {
    serde_json::json!({
        "type": "tool_execution_end",
        "toolCallId": call_id,
        "toolName": "write",
        "result": {"ok": !is_error},
        "isError": is_error
    })
    .to_string()
}

/// Drive one REAL fixture child subprocess over NDJSON and return its settled result.
async fn run_fixture(dir: &Path, output_path: &Path, lines: Vec<String>) -> SingleResult {
    let steps: Vec<serde_json::Value> = lines
        .into_iter()
        .map(|line| serde_json::json!({"kind": "emit", "line": line}))
        .collect();
    let script = serde_json::json!({ "steps": steps, "exit_code": 0 });
    let script_path = dir.join("script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");
    let fixture = fixture_binary_path();

    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc. Every caller
    // holds `ENV_MUTATION_LOCK` for the whole call.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_sync(
            &agent_config("researcher"),
            "Investigate the flake and report what you find",
            &run_options(dir, output_path),
        ),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }
    result
}

/// Negative control: the same receipt prose, with NO write call in the transcript, has no
/// acceptance report anywhere and is rejected on the ledger. Without this half the positive case
/// below would pass even if the gate simply stopped requiring a report.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receipt_with_no_child_write_has_no_report_and_is_rejected() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");
    let output_path = dir.path().join("review.md");

    let result = run_fixture(
        dir.path(),
        &output_path,
        vec![message_end_line(CHILD_RECEIPT)],
    )
    .await;

    let ledger = result
        .acceptance
        .as_ref()
        .expect("a clean run always publishes a ledger");
    assert_eq!(
        ledger.status,
        AcceptanceStatus::Rejected,
        "no acceptance-report exists in either source: {result:?}"
    );
}

/// The positive case: the report is delivered ONLY through the child's own successful `write` tool
/// call. Nothing ever puts it on disk — the fixture child emits NDJSON, it does not touch the
/// filesystem — so the ledger can only reach `attested` if authorship is read from the transcript,
/// which is exactly what `extractChildWrittenOutput` does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_report_inside_the_childs_own_successful_write_satisfies_the_gate() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");
    let output_path = dir.path().join("review.md");

    let result = run_fixture(
        dir.path(),
        &output_path,
        vec![
            write_start_line("w1", &output_path, CHILD_ARTIFACT),
            tool_end_line("w1", false),
            message_end_line(CHILD_RECEIPT),
        ],
    )
    .await;

    let ledger = result
        .acceptance
        .as_ref()
        .expect("a clean run always publishes a ledger");
    assert_eq!(
        ledger.status,
        AcceptanceStatus::Attested,
        "the child-authored artifact is an admissible — and, in file-only mode, the PRIMARY — \
         acceptance-report source: {result:?}"
    );
    assert_eq!(result.exit_code, 0, "{result:?}");
}

/// A write call whose tool result reported an ERROR is not authorship: the artifact never landed,
/// so its content must not be credited to the child. Same transcript as the passing case above,
/// with `isError: true` on the result — the one bit that differs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_write_result_does_not_supply_the_report() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");
    let output_path = dir.path().join("review.md");

    let result = run_fixture(
        dir.path(),
        &output_path,
        vec![
            write_start_line("w1", &output_path, CHILD_ARTIFACT),
            tool_end_line("w1", true),
            message_end_line(CHILD_RECEIPT),
        ],
    )
    .await;

    let ledger = result
        .acceptance
        .as_ref()
        .expect("a clean run always publishes a ledger");
    assert_eq!(
        ledger.status,
        AcceptanceStatus::Rejected,
        "a failed write is not authored output, so the report it carried is inadmissible: \
         {result:?}"
    );
}

/// A write to a DIFFERENT path is not authorship of the configured output either — the path
/// comparison is what keeps a child's unrelated scratch file out of the acceptance evidence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_to_another_path_does_not_supply_the_report() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");
    let output_path = dir.path().join("review.md");
    let other_path = dir.path().join("scratch.md");

    let result = run_fixture(
        dir.path(),
        &output_path,
        vec![
            write_start_line("w1", &other_path, CHILD_ARTIFACT),
            tool_end_line("w1", false),
            message_end_line(CHILD_RECEIPT),
        ],
    )
    .await;

    let ledger = result
        .acceptance
        .as_ref()
        .expect("a clean run always publishes a ledger");
    assert_eq!(
        ledger.status,
        AcceptanceStatus::Rejected,
        "only the CONFIGURED output path's content counts: {result:?}"
    );
}

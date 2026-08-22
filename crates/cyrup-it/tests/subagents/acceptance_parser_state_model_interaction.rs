//! CROSS-CUTTING (batch 9): the acceptance PARSER (G79) and the acceptance STATUS MODEL (G78) were
//! ported and verified separately. This file drives a REAL fixture child subprocess over NDJSON,
//! through `run_sync`, and asserts that the report the parser yields is the one the state model
//! actually settles on — the interaction neither group's isolated review could observe.
//!
//! Both groups' own tests call `evaluate_acceptance` (or `model::parse_acceptance_report`) directly
//! with a hand-written `&str`. That proves each half correct in isolation and proves nothing about
//! the seam between them, which in this crate runs:
//!
//! ```text
//! run_sync
//!   -> exec::output::extract_child_written_output   (G82: authorship from the child's own write)
//!   -> exec::acceptance::evaluate_acceptance        (the LATTICE gate — the live one)
//!        -> select_acceptance_report_source         (G82: file-first when `outputMode: file-only`)
//!             -> model::parse_acceptance_report     (G79: the parser)
//!        -> self_report_floor / declared_structural_failures
//!        -> AcceptanceStatus                        (G78: the status model)
//! ```
//!
//! The two cases below are chosen because each one FAILS for a different reason if the seam is
//! wrong, and neither is reachable from a unit test of either half:
//!
//! 1. `an_aliased_child_report_survives_the_whole_live_pipeline` — G79's normalization (snake_case
//!    field names, status aliases, singleton-to-array coercion) has to survive being carried by
//!    G82's authorship extraction and then satisfy G78's evidence rungs. A parser that normalizes
//!    correctly but is handed the wrong SOURCE, or a model that reads a field the normalizer
//!    renamed, both show up here and nowhere else.
//!
//! 2. `a_truncated_report_in_the_authoritative_file_is_never_papered_over_by_the_receipt` — the
//!    load-bearing rule of `parseAcceptanceReportSources` (`acceptance.ts:753-771`): a DEFECT in
//!    the primary source is surfaced, and only a genuinely ABSENT report falls through to the
//!    secondary. The single discriminator is the exact string `ACCEPTANCE_REPORT_NOT_FOUND`
//!    (`acceptance.ts:699`), so any parse path that collapses a defect onto "not found" silently
//!    converts a rejection into a pass. This test is what makes that rule observable in production.
//!
//! Gated on `test-fixtures` exactly like the other real-subprocess suites here: without the flag
//! the `cyrup-subagent-fixture` `[[bin]]` is never built and this file compiles to an empty,
//! passing test list.

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
        // The completion-mutation guard is a separate gate; disabled so each run's outcome is
        // decided by the acceptance ledger alone.
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
/// `AcceptanceContract::heuristic_default`. `outputMode: file-only` is what makes the child's
/// artifact the AUTHORITATIVE report source (`execution.ts:1680-1701`).
fn run_options(cwd: &Path, output_path: &Path) -> RunOptions {
    RunOptions {
        turn_budget: None,
        // SUBA-021 — pi's `usageBudget` is an OPTIONAL param (`extension/schemas.ts:330`) with no
        // upstream default: a run that does not ask for a budget runs unbudgeted. This fixture asks
        // for none, so `None` is what keeps every assertion below measuring what it measured before
        // the field existed (and `skip_serializing_if` keeps the on-disk config byte-identical).
        usage_budget: None,
        enforce_hard_turn_limit: false,
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
        // SUBA-049 — the RETURN half of G90's steer channel. `None` for the same reason
        // `steer_inbox_dir` above is `None`: pi mints both paths only where an async run
        // directory exists (`subagent-runner.ts:3820-3821`), and this fixture's run has neither.
        // Both sides gate the env keys on presence, so `None` leaves the child's spawn env
        // byte-identical to what this test was written against.
        steer_ack_dir: None,
        steer_capability_path: None,
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

fn tool_end_line(call_id: &str) -> String {
    serde_json::json!({
        "type": "tool_execution_end",
        "toolCallId": call_id,
        "toolName": "write",
        "result": {"ok": true},
        "isError": false
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

    // SAFETY: scoped, mutex-serialized env mutation — every caller holds `ENV_MUTATION_LOCK` for
    // the whole call, exactly as the sibling real-subprocess suites in this directory do.
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

/// G79 × G78, end to end. The child authors an artifact whose acceptance report uses EVERY
/// normalization G79 added and none of the canonical spellings:
///
/// * the wrapper key `acceptance_report` (one of the four spellings, `acceptance.ts:484-489`),
/// * snake_case field names throughout (`acceptance.ts:486-508`),
/// * a status ALIAS (`"Done"` for `satisfied`, `acceptance.ts:520`),
/// * a lone object where an array belongs, and a bare string where a `string[]` belongs
///   (the singleton coercions, `acceptance.ts:596-620`).
///
/// Before G79 every one of those was a hard mismatch and this run would be `Rejected` for a purely
/// cosmetic difference. After it, the report has to survive being carried by G82's authorship
/// extraction out of the child's own `write` call and then satisfy G78's evidence rungs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_aliased_child_report_survives_the_whole_live_pipeline() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");
    let output_path = dir.path().join("review.md");

    let artifact = "The flake is a timing race in the reaper.\n\n\
         ```acceptance_report\n\
         {\"acceptance_report\": {\
            \"review_findings\": \"blocker: reaper.rs:44 - unsynchronized wait\", \
            \"residual_risks\": [\"none\"], \
            \"criteria_satisfied\": {\"id\": \"c_1\", \"status\": \"Done\", \"evidence\": \"traced the wait\"}\
         }}\n\
         ```";

    let result = run_fixture(
        dir.path(),
        &output_path,
        vec![
            write_start_line("call-1", &output_path, artifact),
            tool_end_line("call-1"),
            message_end_line("Wrote the review to the configured output path."),
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
        "an aliased/snake_case report must reach the SAME status a canonical one does; detail was \
         {:?}",
        ledger.detail
    );
}

/// The load-bearing source-selection rule, made observable in production.
///
/// The child's authoritative artifact opens an `acceptance-report` fence and is then CUT OFF — no
/// newline, no body, no closing fence. That is a DEFECT of the primary source, and
/// `parseAcceptanceReportSources` (`acceptance.ts:753-771`) surfaces it; only a genuinely ABSENT
/// report may fall through to the secondary.
///
/// The test is discriminating because the assistant receipt carries a COMPLETE, VALID report. If
/// the truncated fence is misread as "no report here" the run falls through to that receipt and
/// settles `Attested` — a silently PASSING run whose real artifact is garbage. It must reject.
///
/// This is exactly the case `parse_acceptance_report` got wrong: `acceptance.ts:702` tests
/// `/```acceptance[-_]report\b/i` (tag presence), and reusing the offset-finding opener helper —
/// which requires `[^\n]*\n` per `acceptance.ts:671` — collapsed a cut-off opener onto
/// `ACCEPTANCE_REPORT_NOT_FOUND`, the one value that means "fall through".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_truncated_report_in_the_authoritative_file_is_never_papered_over_by_the_receipt() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");
    let output_path = dir.path().join("review.md");

    // The artifact: cut off mid-opener. No newline after the tag.
    let truncated_artifact = "The flake is a timing race in the reaper.\n\n```acceptance-report";

    // The receipt: a COMPLETE and VALID report. If the defect above is misread as an absent
    // report, THIS is what the gate would use, and the run would pass.
    let receipt = "Wrote the review.\n\n\
         ```acceptance-report\n\
         {\"reviewFindings\": [\"blocker: reaper.rs:44 - unsynchronized wait\"], \
           \"residualRisks\": [\"none\"]}\n\
         ```";

    let result = run_fixture(
        dir.path(),
        &output_path,
        vec![
            write_start_line("call-1", &output_path, truncated_artifact),
            tool_end_line("call-1"),
            message_end_line(receipt),
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
        "a truncated report in the AUTHORITATIVE source is a defect to surface, and must never be \
         replaced by the receipt's valid one; detail was {:?}",
        ledger.detail
    );

    // Control: the very same receipt, with the child's artifact carrying no fence at all, IS a
    // genuine miss and DOES fall through — so the assertion above is pinning the defect/miss
    // distinction, not merely "file-only runs reject".
    let dir2 = tempfile::tempdir().expect("real tempdir");
    let output_path2 = dir2.path().join("review.md");
    let control = run_fixture(
        dir2.path(),
        &output_path2,
        vec![
            write_start_line("call-1", &output_path2, "Prose with no report at all."),
            tool_end_line("call-1"),
            message_end_line(receipt),
        ],
    )
    .await;
    let control_ledger = control
        .acceptance
        .as_ref()
        .expect("a clean run always publishes a ledger");
    assert_eq!(
        control_ledger.status,
        AcceptanceStatus::Attested,
        "an ABSENT report in the primary source falls through to the secondary; detail was {:?}",
        control_ledger.detail
    );
}

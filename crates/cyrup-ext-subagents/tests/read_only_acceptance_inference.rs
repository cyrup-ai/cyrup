//! A read-only / research subagent must still get an acceptance contract — pi's `inferLevel` has
//! no `"none"` branch.
//!
//! `pi-subagents:v0.34.0:src/runs/shared/acceptance.ts:69-125` `inferLevel` returns exactly four
//! shapes and none of them is `none`: `reviewed` (`:88-96`), `checked` (`:98-105`), `attested` for
//! a reviewer/scout/context-builder/researcher/analyst agent or read-only task wording
//! (`:107-116`), and `attested` as the final fallthrough (`:118-124`). `formatAcceptancePrompt`
//! returns `""` only for `level === "none"` (`:305`), and `execution.ts:1037-1038` appends its
//! result unconditionally, so essentially every child is told to end with a fenced
//! `acceptance-report` block — and `evaluateAcceptance` (`:787-816`) then produces a real ledger:
//! `attested` when the block is there, `rejected` when it is not.
//!
//! Before this change `AcceptanceContract::heuristic_default` classified with the enum-lattice
//! `completion_guard::expects_implementation_mutation` instead and returned
//! `AcceptanceStatus::NotRequired` for anything that did not read as implementation-expecting.
//! `inject_acceptance_contract` then returned the task VERBATIM for such a contract
//! (`is_no_op()`), and `evaluate_acceptance` short-circuited to `AcceptanceLedger::not_required()`.
//! For every reviewer / scout / researcher / summariser child, cyrup therefore sent a materially
//! different prompt from pi's and reported `acceptance: not-required` where pi reports
//! `attested`/`rejected`.
//!
//! Both halves are asserted here against the REAL scripted fixture child: the ledger the run
//! publishes, and — the load-bearing safety property — that an inferred rejection does NOT fail
//! the run. pi gates its post-hoc exit-code correction on `result.acceptance.explicit`
//! (`execution.ts:1229`) and an inferred contract is never explicit, so always-inferring changes
//! provenance, not outcomes.
//!
//! Separate compilation unit from `lib.rs`, so NOT bound by that crate's `#![forbid(unsafe_code)]`;
//! the `unsafe` env mutation (Rust 2024 requires it for `std::env::set_var`/`remove_var`) is scoped
//! and serialized under [`ENV_MUTATION_LOCK`], exactly like every other integration test here.
//!
//! Gated on `test-fixtures` (the `cyrup-subagent-fixture` `[[bin]]`'s own `required-features`
//! gate): without it the fixture is never built and this file compiles to an empty, passing test
//! list.

#![cfg(feature = "test-fixtures")]
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
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions, SingleResult, run_sync};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;

static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

fn fixture_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))
}

fn write_script(dir: &Path, name: &str, script_json: &serde_json::Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, script_json.to_string()).expect("write fixture script");
    path
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

/// `acceptance: None` is the whole point: no explicit policy, so `run_sync` resolves the contract
/// through `AcceptanceContract::heuristic_default` — pi's `level: "auto"` (`acceptance.ts:127`).
fn run_options(cwd: &Path) -> RunOptions {
    RunOptions {
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        output_path: None,
        output_mode: OutputMode::Inline,
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
    }
}

/// Run one real fixture child under `agent`/`task` and return its settled result.
async fn run_fixture(dir: &Path, agent: &str, task: &str, output: &str) -> SingleResult {
    let script = serde_json::json!({
        "steps": [ {"kind": "emit", "line": message_end_line(output)} ],
        "exit_code": 0
    });
    let script_path = write_script(dir, "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc. Every caller
    // holds `ENV_MUTATION_LOCK` for the whole call.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_sync(&agent_config(agent), task, &run_options(dir)),
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

/// The contract itself, with no subprocess involved: pi's read-only-agent branch
/// (`acceptance.ts:107-116`) infers `attested`, never a no-op.
#[test]
fn a_research_agent_infers_a_real_contract_rather_than_none() {
    let contract = AcceptanceContract::heuristic_default("researcher", "Investigate the flake");
    assert_eq!(contract.required_level, AcceptanceStatus::Attested);
    assert!(
        !contract.is_no_op(),
        "pi's `inferLevel` has no `none` branch: {contract:?}"
    );
}

/// End to end: a research child that emits ordinary prose and no `acceptance-report` block gets a
/// REJECTED ledger (pi `evaluateAcceptance`'s missing-attestation branch, `acceptance.ts:808-814`)
/// — not the `not-required` cyrup previously reported — and, because the contract was INFERRED
/// rather than explicit, the run still exits 0 with no error (pi `execution.ts:1229` gates its
/// exit-code correction on `result.acceptance.explicit`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_research_child_with_no_report_is_rejected_on_the_ledger_but_still_exits_clean() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    let result = run_fixture(
        dir.path(),
        "researcher",
        "Investigate the flake and report what you find",
        "I looked at the logs; the flake is a timing race in the reaper.",
    )
    .await;

    let ledger = result
        .acceptance
        .as_ref()
        .expect("a clean run always publishes a ledger");
    assert_eq!(
        ledger.status,
        AcceptanceStatus::Rejected,
        "no acceptance-report block was emitted, so the gate rejects: {result:?}"
    );
    assert_eq!(
        result.exit_code, 0,
        "an INFERRED rejection must never flip the exit code: {result:?}"
    );
    assert!(
        result.error.is_none(),
        "...nor attach an error: {:?}",
        result.error
    );
}

/// The same child, this time emitting the fenced `acceptance-report` block pi's injected contract
/// asks for, reaches `attested` (`acceptance.ts:806-807`). This is the half that proves the gate
/// is a real evaluation rather than an unconditional rejection — and that the delivered output has
/// the machine report stripped back off it (pi `stripAcceptanceReport`, `execution.ts:823`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_research_child_that_attests_reaches_attested() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    const PROSE: &str = "The flake is a timing race in the reaper.";
    let report = serde_json::json!({
        "reviewFindings": ["blocker: reaper.rs:44 - unsynchronized wait"],
        "residualRisks": ["none"],
    });
    let output = format!("{PROSE}\n\n```acceptance-report\n{report}\n```");

    let result = run_fixture(
        dir.path(),
        "researcher",
        "Investigate the flake and report what you find",
        &output,
    )
    .await;

    let ledger = result
        .acceptance
        .as_ref()
        .expect("a clean run always publishes a ledger");
    assert_eq!(
        ledger.status,
        AcceptanceStatus::Attested,
        "an attested child report satisfies an `attested` contract: {result:?}"
    );
    assert_eq!(result.exit_code, 0, "{result:?}");
    let delivered = result.final_output.as_deref().unwrap_or_default();
    assert!(
        delivered.contains(PROSE),
        "the prose answer survives: {delivered:?}"
    );
    assert!(
        !delivered.contains("reviewFindings"),
        "the machine report is stripped from the delivered output: {delivered:?}"
    );
}

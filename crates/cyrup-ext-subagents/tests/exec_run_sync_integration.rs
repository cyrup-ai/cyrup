//! Integration test: `exec::run_sync` end to end against the scripted-NDJSON test-double binary
//! (`cyrup-subagent-fixture`, arch-SA §11) — exercising the full prompt-construction ->
//! real-subprocess-spawn -> NDJSON-consumption -> final-output-extraction ->
//! completion-mutation-guard -> acceptance-gate pipeline (func-SA §5.2; arch-SA §6.3; this
//! crate's task-brief testing obligation for R-SA-027/028/036/043).
//!
//! No mocking anywhere in this file (this codebase's standing convention, restated in this
//! crate's own task brief): every run below spawns the REAL `cyrup-subagent-fixture` binary as a
//! genuine OS subprocess via `CYRUP_SUBAGENT_BINARY` (R-SA-045 tier 1's documented override
//! escape hatch), reads its REAL piped stdout, and asserts on the REAL observed `SingleResult`.
//!
//! This file is a separate compilation unit from `cyrup-ext-subagents`'s own `lib.rs` (ordinary
//! Cargo integration-test placement), so it is NOT bound by that crate's own
//! `#![forbid(unsafe_code)]` — the one `unsafe` block below (Rust 2024 requires `unsafe` for
//! `std::env::set_var`/`remove_var`, since process environment is de facto shared mutable state)
//! is scoped to exactly the two calls needed to point `CYRUP_SUBAGENT_BINARY` at the fixture
//! binary for the duration of one test, executed under a process-wide mutex
//! ([`ENV_MUTATION_LOCK`]) so this file's tests never race each other over that global state even
//! when `cargo test` runs them concurrently within the same test-binary process.
//!
//! Gated on the `test-fixtures` Cargo feature (matching the `cyrup-subagent-fixture` `[[bin]]`
//! target's own `required-features` gate, `Cargo.toml`): without that feature the fixture binary
//! is never built at all, so this whole file compiles to an empty test list (`cargo test`
//! reports it as a normal, zero-test pass) rather than every test failing at spawn time with a
//! confusing "No such file or directory".

#![cfg(feature = "test-fixtures")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::Mutex;

use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;

/// Serializes every test in this file that mutates `CYRUP_SUBAGENT_BINARY` (process-global
/// state) — `cargo test` runs a test binary's own `#[test]` functions concurrently by default, so
/// without this lock two tests in this file could observe or clobber each other's override value
/// mid-run.
static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";

/// Path to the real, already-built `cyrup-subagent-fixture` binary — Cargo sets
/// `CARGO_BIN_EXE_<name>` for every `[[bin]]` target in this same package that is part of the
/// current test run's build graph, which requires running this test file with
/// `--features test-fixtures` (this crate's `[[bin]]` entry's own `required-features` gate,
/// `Cargo.toml`) so the fixture binary actually gets built at all.
fn fixture_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))
}

/// Write `script_json` to a fresh temp file and return its path, for
/// `CYRUP_SUBAGENT_FIXTURE_SCRIPT` to point at.
fn write_script(dir: &std::path::Path, name: &str, script_json: &serde_json::Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, script_json.to_string()).expect("write fixture script");
    path
}

fn base_agent_config(model: &str) -> AgentConfig {
    AgentConfig {
        name: "worker".to_string(),
        model: Some(ModelId::from(model)),
        fallback_models: Vec::new(),
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        output: None,
        completion_guard: Some(false), // isolate this test from R-SA-034's own separate gate
        max_output: OutputCap::default(),
        max_subagent_depth: None,
        depth: DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
    }
}

fn base_run_options(cwd: &std::path::Path, model: &str) -> RunOptions {
    RunOptions {
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        output_path: None,
        output_mode: OutputMode::Inline,
        structured_output_schema: None,
        model_override: ModelOverride::Inherit,
        preferred_provider: None,
        available_models: vec![ModelId::from(model)],
        cancel: CancelToken::new(),
        interrupt: CancelToken::new(),
        share: None,
        session_dir: None,
        include_progress: None,
        agent_scope: None,
        acceptance: Some(AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![])),
        fork_context: ForkContext::fresh(),
    }
}

/// One `message_end` NDJSON line whose assistant message carries the given usage + text parts —
/// mirrors exactly what a real `cyrup --print --mode json` invocation emits on its wire
/// (`exec/ndjson.rs`'s own documented wire shape: `message.usage`, `message.content[].text`,
/// camelCase payload fields).
fn message_end_line(text: &str, input: u64, output: u64) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": input, "output": output, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": input + output,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

fn tool_execution_start_line(tool_call_id: &str, tool_name: &str) -> String {
    serde_json::json!({"type": "tool_execution_start", "toolCallId": tool_call_id, "toolName": tool_name}).to_string()
}

fn tool_execution_end_line(tool_call_id: &str, tool_name: &str) -> String {
    serde_json::json!({
        "type": "tool_execution_end", "toolCallId": tool_call_id, "toolName": tool_name,
        "result": "ok", "isError": false
    })
    .to_string()
}

// -------------------------------------------------------------------------------------------
// Full pipeline: prompt construction -> spawn -> NDJSON consumption -> final-output extraction
// -> acceptance gate, on a clean, successful attempt.
// -------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_end_to_end_against_the_scripted_fixture_extracts_output_and_reaches_checked() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "emit", "line": tool_execution_start_line("c1", "edit")},
            {"kind": "emit", "line": tool_execution_end_line("c1", "edit")},
            {"kind": "emit", "line": message_end_line(
                "I implemented the fix.\n```acceptance-report\n{\"criteriaSatisfied\": true, \"changedFiles\": [\"a.rs\"]}\n```",
                42, 17,
            )},
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_end"}"#.to_string())}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);

    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT", &script_path);
    }

    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    opts.acceptance = Some(AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]));

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Implement the approved fix", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT");
    }

    assert_eq!(result.exit_code, 0, "clean run must report exit code 0: {result:?}");
    assert_eq!(result.model.as_ref().map(ModelId::as_str), Some("fixture-model"));
    assert_eq!(result.attempted_models.len(), 1, "no fallback should have been needed");
    assert!(!result.timed_out);
    assert!(!result.detached);
    assert!(!result.interrupted);

    // R-SA-029: final-output extraction must have picked up the acceptance-report-shaped text.
    let output = result.final_output.expect("final_output must be extracted");
    assert!(output.contains("acceptance-report"), "got: {output}");
    assert!(output.contains("criteriaSatisfied"));

    // R-SA-027: usage must have been accumulated from the child's message_end event.
    assert_eq!(result.usage.input, 42);
    assert_eq!(result.usage.output, 17);

    // R-SA-043: result compaction — summarized tool_calls, not raw messages.
    assert_eq!(result.tool_calls, vec!["edit".to_string()]);

    // R-SA-032: acceptance ledger reached at least Checked given the real (non-triggered)
    // completion-mutation guard and the self-reported acceptance-report block.
    let ledger = result.acceptance.expect("acceptance ledger must be populated");
    assert!(
        ledger.status.satisfies(AcceptanceStatus::Checked),
        "expected at least Checked, got {:?}",
        ledger.status
    );
}

// -------------------------------------------------------------------------------------------
// R-SA-028: bounded recent-output buffer — the fixture emits far more than 50 lines; run_sync's
// own AgentProgress fold must never retain more than RECENT_OUTPUT_CAP raw lines at any point
// (asserted indirectly here via a successful, non-hanging completion — the cap's own unit-level
// eviction behavior is covered directly in exec/mod.rs's own #[cfg(test)] module; this
// integration test's job is to prove the REAL spawn+consume pipeline actually drives
// AgentProgress::record_raw_line for every line a real child emits, not just a hand-built event
// list).
// -------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_survives_a_real_child_emitting_more_than_fifty_lines() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");

    let mut steps = vec![serde_json::json!({"kind": "emit", "line": r#"{"type":"agent_start"}"#})];
    for i in 0..80 {
        steps.push(serde_json::json!({
            "kind": "emit",
            "line": serde_json::json!({"type": "unknown", "n": i}).to_string()
        }));
    }
    steps.push(serde_json::json!({
        "kind": "emit",
        "line": message_end_line("plain final answer, no acceptance report", 1, 1)
    }));
    let script = serde_json::json!({"steps": steps, "exit_code": 0});
    let script_path = write_script(dir.path(), "script-many-lines.json", &script);

    let fixture = fixture_binary_path();
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT", &script_path);
    }

    let agent = base_agent_config("fixture-model");
    let opts = base_run_options(dir.path(), "fixture-model");

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Review only: return findings", &opts),
    )
    .await
    .expect("run_sync must not hang draining 80+ lines");

    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT");
    }

    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(
        result.final_output.as_deref(),
        Some("plain final answer, no acceptance report")
    );
}

// -------------------------------------------------------------------------------------------
// R-SA-036: timeout terminates the fallback ladder outright (soft interrupt via the real
// SIGINT->SIGTERM->SIGKILL escalation ladder) rather than advancing to a next candidate model.
// -------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_timeout_terminates_the_ladder_without_advancing_to_a_fallback_model() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");

    // A fixture that ignores SIGINT and sleeps far longer than the test's own deadline, so the
    // real signal-escalation ladder must walk past SIGINT to SIGTERM (a plain sleep, unlike a
    // full ignore-everything trap, still dies to SIGTERM) before this attempt's deadline-driven
    // termination is confirmed.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": r#"{"type":"agent_start"}"#},
            {"kind": "sleep_ms", "ms": 30_000}
        ],
        "ignore_sigint": true,
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-hang.json", &script);

    let fixture = fixture_binary_path();
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT", &script_path);
    }

    let mut agent = base_agent_config("primary-model");
    agent.fallback_models = vec![ModelId::from("fallback-model")]; // must NEVER be attempted
    let mut opts = base_run_options(dir.path(), "primary-model");
    opts.available_models = vec![ModelId::from("primary-model"), ModelId::from("fallback-model")];
    opts.deadline_at = Some(std::time::Instant::now() + Duration::from_millis(300));

    let result = tokio::time::timeout(
        Duration::from_secs(15), // generous outer bound for the real SIGINT->SIGTERM escalation
        cyrup_ext_subagents::exec::run_sync(&agent, "long running task", &opts),
    )
    .await
    .expect("run_sync itself must return once the real signal escalation confirms termination");

    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT");
    }

    assert!(result.timed_out, "expected timed_out: true, got {result:?}");
    assert_eq!(
        result.attempted_models.len(),
        1,
        "R-SA-036: a timeout MUST NOT advance to the next fallback candidate, got {:?}",
        result.attempted_models
    );
    assert_eq!(result.attempted_models[0].as_str(), "primary-model");
}

// -------------------------------------------------------------------------------------------
// R-SA-055 (SAFETY-CRITICAL): the recursion-depth guard runs FIRST in `run_sync`, rejecting a
// blocked attempt without EVER spawning the real fixture child — proven with
// `CYRUP_SUBAGENT_BINARY` correctly pointed at the real, working fixture binary (never a bogus
// path), so a failure to observe the fixture's own scripted marker output can only be attributed
// to the depth guard itself, not to the fixture being unreachable for some unrelated reason.
// -------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_rejects_a_blocked_depth_without_spawning_the_real_fixture_child() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");

    const NEVER_SPAWNED_MARKER: &str = "THIS-FIXTURE-MUST-NEVER-ACTUALLY-RUN";
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line(NEVER_SPAWNED_MARKER, 1, 1)},
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-depth-blocked.json", &script);

    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc. Deliberately a
    // REAL, working fixture (not a bogus path), so this test proves the depth guard is what
    // prevents the spawn, not merely that a bad binary path would have failed anyway.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT", &script_path);
    }

    let mut agent = base_agent_config("fixture-model");
    // current_depth == max_depth: is_blocked() must be true.
    agent.depth = DepthEnvelope {
        current_depth: 4,
        max_depth: 4,
    };
    let opts = base_run_options(dir.path(), "fixture-model");

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        cyrup_ext_subagents::exec::run_sync(&agent, "do something", &opts),
    )
    .await
    .expect("a depth-blocked run_sync call must return near-instantly, never hang");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT");
    }

    assert_eq!(result.exit_code, 1, "a blocked depth attempt must report failure: {result:?}");
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("depth limit exceeded"),
        "expected a DepthExceeded-shaped error, got: {:?}",
        result.error
    );
    assert!(
        result.attempted_models.is_empty(),
        "no model attempt (and therefore no real child process) may ever be made"
    );
    assert_ne!(
        result.final_output.as_deref(),
        Some(NEVER_SPAWNED_MARKER),
        "the fixture's marker output must never appear — proving the real child was never spawned"
    );
    // The load-bearing proof, independent of the assertions above: `run_sync`'s spawn-scratch
    // directory (the first filesystem side effect ANY spawn attempt, real or fixture, would ever
    // create) must never have been created.
    assert!(
        !dir.path().join(".cyrup-subagent-scratch").exists(),
        "the depth guard must reject before the spawn-scratch directory is ever created, i.e. \
         before any child process attempt"
    );
}

// -------------------------------------------------------------------------------------------
// R-SA-030: structured-output extraction + parent-side JSON-Schema re-validation, against a
// REAL scripted child process (no mocking, matching this file's own standing convention).
// -------------------------------------------------------------------------------------------

/// The declared JSON Schema shared by both R-SA-030 tests below: an object requiring a string
/// `summary` and an integer `count`.
fn sample_structured_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string"},
            "count": {"type": "integer"}
        },
        "required": ["summary", "count"]
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_validates_a_schema_valid_structured_output_and_populates_the_field() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": r#"{"type":"agent_start"}"#},
            {"kind": "emit", "line": message_end_line(
                "Here is my structured result:\n```json\n{\"summary\": \"all good\", \"count\": 3}\n```",
                10, 5,
            )},
            {"kind": "emit", "line": r#"{"type":"agent_end"}"#}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-structured-valid.json", &script);

    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT", &script_path);
    }

    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    opts.structured_output_schema = Some(sample_structured_output_schema());

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Produce the structured summary", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT");
    }

    assert_eq!(result.exit_code, 0, "a schema-valid structured output must not fail the run: {result:?}");
    assert!(result.error.is_none(), "got: {:?}", result.error);
    assert_eq!(
        result.structured_output,
        Some(serde_json::json!({"summary": "all good", "count": 3})),
        "R-SA-030: SingleResult::structured_output must be populated with the validated value, \
         got {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_rejects_a_schema_invalid_structured_output_and_fails_the_run() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");

    // "count" is a string here, not the schema-required integer — this must fail parent-side
    // re-validation even though the child exited 0 and produced SOME structured-looking output.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": r#"{"type":"agent_start"}"#},
            {"kind": "emit", "line": message_end_line(
                "Here is my structured result:\n```json\n{\"summary\": \"all good\", \"count\": \"three\"}\n```",
                10, 5,
            )},
            {"kind": "emit", "line": r#"{"type":"agent_end"}"#}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-structured-invalid.json", &script);

    let fixture = fixture_binary_path();
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT", &script_path);
    }

    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    opts.structured_output_schema = Some(sample_structured_output_schema());

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Produce the structured summary", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");

    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT");
    }

    assert_ne!(
        result.exit_code, 0,
        "R-SA-030: a structured output that fails parent-side schema validation MUST fail the \
         run, got {result:?}"
    );
    assert!(
        result.structured_output.is_none(),
        "an invalid value must never be surfaced as the validated structured_output, got {:?}",
        result.structured_output
    );
    let error = result.error.expect("a clear validation-error message must be present");
    assert!(
        error.contains("structured output validation failed") && error.contains("count"),
        "expected a clear validation-error message naming the offending field, got: {error}"
    );
}

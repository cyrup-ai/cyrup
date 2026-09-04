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
//! Mutates no process environment: each run names its fixture binary and script through
//! `RunOptions::spawn_command`, so this file needs no `unsafe` and no lock. Every file in
//! this `--test` target shares ONE process, so a global set here would be global for all.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::time::Duration;


use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions, ToolCallSummary};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;


/// Path to the real, already-built `cyrup-subagent-fixture` binary.
///
/// MIGRATION: this used to be `PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))`, which
/// worked only while this file lived in `cyrup-ext-subagents` — Cargo sets `CARGO_BIN_EXE_<name>`
/// only for test targets in the SAME package as that binary. In `cyrup-it` it does not resolve at
/// all, so the path now comes from this crate's `build.rs`, which builds the fixture (with the
/// owning crate's `--features test-fixtures`) and exports `CYRUP_IT_BIN_CYRUP_SUBAGENT_FIXTURE`.
fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
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
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        exclude_tools: Vec::new(),
        allow_nested_subagents: None,
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false), // isolate this test from R-SA-034's own separate gate
        max_output: OutputCap::default(),
        max_subagent_depth: None,
        memory: None,
        tool_budget: None,
        runner: None, // SUBA-074: the native child, as before
        depth: DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
    }
}

fn base_run_options(cwd: &std::path::Path, model: &str) -> RunOptions {
    RunOptions {
        spawn_command: None,
        child_env: std::collections::HashMap::new(),
        turn_budget: None,
        permission_rules: None, // SUBA-073: no policy — the pre-field behaviour
        // SUBA-078: this fixture exercises no reasoning ceiling — `None` is "no ceiling
        // configured, so the bound is off", matching `runner_main.rs`'s own hop-2 default.
        thinking_ceiling: None,
        // SUBA-021: pi's `usageBudget` is an OPTIONAL param — upstream has no default budget, so a
        // call that does not ask for one runs unbudgeted. This fixture asks for none.
        usage_budget: None,
        enforce_hard_turn_limit: false,
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        output_path: None,
        output_mode: OutputMode::Inline,
        reads: None,
        structured_output_schema: None,
        model_override: ModelOverride::Inherit,
        preferred_provider: None,
        available_models: vec![ModelId::from(model)],
        cancel: CancelToken::new(),
        interrupt: CancelToken::new(),
        share: None,
        session_dir: None,
        skills: None,
        runtime_cwd: None,
        include_progress: None,
        agent_scope: None,
        acceptance: Some(AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![])),
        fork_context: ForkContext::fresh(),
        live_events: None,
        parent_session_id: None,
        clarify: None,
        orchestrator_intercom_target: None,
        run_id: None,
        child_index: None,
        steer_inbox_dir: None,
        // SUBA-049: the RETURN half of G90's steer channel. Both paths exist only under a background
        // run directory; a foreground fixture like this one has none. Load-bearing:
        // `build_attempt_spawn_plan` gates both env keys on presence (exec/mod.rs:2227-2250), so
        // `None` keeps the child's env overlay byte-identical to a real foreground child's.
        steer_ack_dir: None,
        steer_capability_path: None,
        control_config: None,
        on_control_event: None,
        artifacts_dir: None,
        // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
        model_scope: None,
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


    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.acceptance = Some(AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]));

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Implement the approved fix", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");


    assert_eq!(result.exit_code, 0, "clean run must report exit code 0: {result:?}");
    assert_eq!(result.model.as_ref().map(ModelId::as_str), Some("fixture-model"));
    assert_eq!(result.attempted_models.len(), 1, "no fallback should have been needed");
    assert!(!result.timed_out);
    assert!(!result.detached);
    assert!(!result.interrupted);

    // R-SA-029 + pi `stripAcceptanceReport` (execution.ts:823): the acceptance gate consumes the
    // acceptance-report block for provenance, but it is STRIPPED from the DELIVERED output — the
    // caller sees the human answer, never the machine report JSON (previously shown verbatim, the
    // bug this closes).
    let output = result.final_output.expect("final_output must be extracted");
    assert_eq!(
        output, "I implemented the fix.",
        "the trailing acceptance-report fence must be stripped from the delivered output, got: {output}"
    );
    assert!(!output.contains("acceptance-report"), "report fence must not leak into output: {output}");
    assert!(!output.contains("criteriaSatisfied"));

    // R-SA-027: usage must have been accumulated from the child's message_end event.
    assert_eq!(result.usage.input, 42);
    assert_eq!(result.usage.output, 17);

    // R-SA-043: result compaction — one `{text, expandedText}` tool-call PREVIEW (pi
    // `ToolCallSummary`), sourced from the `tool_execution_start` request, NOT a bare tool name.
    // The scripted `edit` call carried no path argument, so the preview is pi's `edit ` (the tool
    // name followed by the empty shortened path), identical for the short and expanded forms.
    assert_eq!(
        result.tool_calls,
        vec![ToolCallSummary {
            text: "edit ".to_string(),
            expanded_text: "edit ".to_string(),
        }]
    );

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


    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Review only: return findings", &opts),
    )
    .await
    .expect("run_sync must not hang draining 80+ lines");


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


    let mut agent = base_agent_config("primary-model");
    agent.fallback_models = vec![ModelId::from("fallback-model")]; // must NEVER be attempted
    let mut opts = base_run_options(dir.path(), "primary-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.available_models = vec![ModelId::from("primary-model"), ModelId::from("fallback-model")];
    opts.deadline_at = Some(std::time::Instant::now() + Duration::from_millis(300));

    let result = tokio::time::timeout(
        Duration::from_secs(15), // generous outer bound for the real SIGINT->SIGTERM escalation
        cyrup_ext_subagents::exec::run_sync(&agent, "long running task", &opts),
    )
    .await
    .expect("run_sync itself must return once the real signal escalation confirms termination");


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
    let dir = tempfile::tempdir().expect("tempdir");

    const NEVER_SPAWNED_MARKER: &str = "THIS-FIXTURE-MUST-NEVER-ACTUALLY-RUN";
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line(NEVER_SPAWNED_MARKER, 1, 1)},
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-depth-blocked.json", &script);


    let mut agent = base_agent_config("fixture-model");
    // current_depth == max_depth: is_blocked() must be true.
    agent.depth = DepthEnvelope {
        current_depth: 4,
        max_depth: 4,
    };
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        cyrup_ext_subagents::exec::run_sync(&agent, "do something", &opts),
    )
    .await
    .expect("a depth-blocked run_sync call must return near-instantly, never hang");


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

/// SUBA-S01 (pi `structured-output.ts:156-173`, `subagent-prompt-runtime.ts`): the child delivers a
/// structured-output value by CALLING the `structured_output` tool, which writes it to the private
/// capture file the parent named in `CYRUP_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE`. A fenced
/// ` ```json ` block in prose is explicitly NOT that channel (a missing capture file is a hard
/// failure "even when prose was produced"), so both R-SA-030 tests below script the fixture child
/// to make that real write — the `write_structured_output` step is the fixture's stand-in for the
/// tool call, not a shortcut around it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_validates_a_schema_valid_structured_output_and_populates_the_field() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": r#"{"type":"agent_start"}"#},
            {"kind": "write_structured_output", "value": {"summary": "all good", "count": 3}},
            {"kind": "emit", "line": message_end_line(
                "Here is my structured result:\n```json\n{\"summary\": \"all good\", \"count\": 3}\n```",
                10, 5,
            )},
            {"kind": "emit", "line": r#"{"type":"agent_end"}"#}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-structured-valid.json", &script);


    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.structured_output_schema = Some(sample_structured_output_schema());

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Produce the structured summary", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");


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
    let dir = tempfile::tempdir().expect("tempdir");

    // "count" is a string here, not the schema-required integer — this must fail parent-side
    // re-validation even though the child exited 0, CALLED `structured_output` (wrote the capture
    // file) and produced prose.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": r#"{"type":"agent_start"}"#},
            {"kind": "write_structured_output", "value": {"summary": "all good", "count": "three"}},
            {"kind": "emit", "line": message_end_line(
                "Here is my structured result:\n```json\n{\"summary\": \"all good\", \"count\": \"three\"}\n```",
                10, 5,
            )},
            {"kind": "emit", "line": r#"{"type":"agent_end"}"#}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-structured-invalid.json", &script);


    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.structured_output_schema = Some(sample_structured_output_schema());

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Produce the structured summary", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");


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

/// pi `execution.ts:1189-1193`: the empty-output (cold-start) gate's structured-presence leg is
/// literally `!existsSync(options.structuredOutput.outputPath)` — the CAPTURE FILE, not the
/// transcript.
///
/// The USER ACTION this protects: an agent whose whole job is to emit a structured record calls
/// `structured_output` and then stops WITHOUT any prose (a perfectly ordinary, and for a
/// schema-declared step arguably the ideal, ending). pi passes that run. A transcript-scanning
/// presence test classifies it `Missing`, flips the attempt to the retryable "Subagent produced no
/// output" error, and burns a fallback model on a run that already succeeded — and, because every
/// fallback attempt ends the same way, fails the whole task.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_accepts_a_structured_only_child_that_produced_no_prose_at_all() {
    let dir = tempfile::tempdir().expect("tempdir");

    // The child calls `structured_output` (writes the capture file) and its ONLY assistant message
    // is empty — no prose anywhere in the transcript, and in particular no fenced ```json block.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "write_structured_output", "value": {"summary": "all good", "count": 3}},
            {"kind": "emit", "line": empty_message_end_line()},
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_end"}"#.to_string())}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-structured-only.json", &script);


    let mut agent = base_agent_config("primary-model");
    agent.fallback_models = vec![ModelId::from("fallback-model")];
    let mut opts = base_run_options(dir.path(), "primary-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.available_models = vec![ModelId::from("primary-model"), ModelId::from("fallback-model")];
    opts.structured_output_schema = Some(sample_structured_output_schema());

    let result = tokio::time::timeout(
        Duration::from_secs(15),
        cyrup_ext_subagents::exec::run_sync(&agent, "Produce the structured summary", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");


    assert_eq!(
        result.exit_code, 0,
        "a child that CALLED structured_output has not 'produced no output', prose or not: \
         {result:?}"
    );
    assert!(result.error.is_none(), "got: {:?}", result.error);
    assert_eq!(
        result.structured_output,
        Some(serde_json::json!({"summary": "all good", "count": 3})),
        "the captured value must still be surfaced, got {result:?}"
    );
    assert_eq!(
        result.attempted_models,
        vec![ModelId::from("primary-model")],
        "the ladder must NOT advance — this attempt succeeded, so no fallback model may be burned"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_missing_structured_output_fails_the_run_even_with_prose() {
    // pi `readStructuredOutput` (structured-output.ts:156-159, execution.ts:791-805): a declared
    // `outputSchema` with NO captured structured value is a HARD failure EVEN WHEN the child
    // produced prose — prose is never an exemption. The child here emits a non-empty prose answer
    // but NO structured-output value at all.
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": r#"{"type":"agent_start"}"#},
            {"kind": "emit", "line": message_end_line(
                "Here is a perfectly nice prose answer with no structured output block at all.",
                10, 5,
            )},
            {"kind": "emit", "line": r#"{"type":"agent_end"}"#}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-structured-missing-with-prose.json", &script);


    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.structured_output_schema = Some(sample_structured_output_schema());

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Produce the structured summary", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");


    assert_ne!(
        result.exit_code, 0,
        "a declared schema with no structured value MUST fail even though prose was produced: {result:?}"
    );
    assert!(result.structured_output.is_none());
    let error = result.error.expect("a missing-structured-output error must be present");
    assert!(
        error.contains("must finish by calling structured_output"),
        "expected the pi 'Missing structured_output call' message, got: {error}"
    );
}

// -------------------------------------------------------------------------------------------
// Tier T3 (group A) — exit-0 re-diagnosis, interrupt semantics, empty-output/fallback — driven
// against the REAL scripted fixture child (no mocking), mirroring `single-execution.test.ts`
// scenarios: a trailing tool failure after a zero exit becomes a failure; a soft interrupt is a
// paused success; an empty (cold-start) output is a *retryable* failure that advances the ladder.
// -------------------------------------------------------------------------------------------

/// A `tool_execution_end` NDJSON line carrying the given text as its result, in cyrup's real tool-
/// result wire shape (`{"content":[{"type":"text","text":…}],…}`, `agent.rs:113-115`).
fn tool_execution_end_result_line(
    tool_call_id: &str,
    tool_name: &str,
    text: &str,
    is_error: bool,
) -> String {
    serde_json::json!({
        "type": "tool_execution_end",
        "toolCallId": tool_call_id,
        "toolName": tool_name,
        "result": {
            "content": [{"type": "text", "text": text}],
            "details": serde_json::Value::Null,
            "terminate": false
        },
        "isError": is_error
    })
    .to_string()
}

/// An empty terminal assistant `message_end` (clean `stopReason: "stop"`, no text) — the cold-start/
/// empty-response shape pi's empty-output check classifies as a retryable failure.
fn empty_message_end_line() -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": ""}],
            "usage": {
                "input": 7, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 7,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_re_diagnoses_a_trailing_tool_failure_after_a_zero_exit_and_does_not_retry() {
    let dir = tempfile::tempdir().expect("tempdir");

    // The child EXITS ZERO, but its final activity was a failed `bash` call reporting a non-zero
    // exit code, with NO assistant text recovering from it — pi `detectSubagentError`
    // (`utils.ts:481-519`) flips this to a failure at the parsed exit code (127). Because
    // "exit 127" matches NO retryable pattern, the fallback model must never be attempted.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "emit", "line": tool_execution_start_line("c1", "bash")},
            {"kind": "emit", "line": tool_execution_end_result_line("c1", "bash", "process exited with code 127", false)}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-trailing-error.json", &script);


    let mut agent = base_agent_config("primary-model");
    agent.fallback_models = vec![ModelId::from("fallback-model")]; // must NEVER be attempted
    let mut opts = base_run_options(dir.path(), "primary-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.available_models = vec![ModelId::from("primary-model"), ModelId::from("fallback-model")];

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Run the build", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast fixture child");


    assert_eq!(
        result.exit_code, 127,
        "detectSubagentError must flip a trailing bash exit 127 to a failure at that code: {result:?}"
    );
    let error = result.error.expect("a re-diagnosed error must be surfaced");
    assert!(
        error.contains("bash") && error.contains("127"),
        "expected a bash exit-127 error message, got: {error}"
    );
    assert!(!result.timed_out);
    assert!(!result.interrupted);
    assert_eq!(
        result.attempted_models.len(),
        1,
        "an exit-127 failure is NOT retryable, so the fallback model must never be attempted, got {:?}",
        result.attempted_models
    );
    assert_eq!(result.attempted_models[0].as_str(), "primary-model");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_soft_interrupt_returns_a_paused_success_not_an_exit_1_failure() {
    let dir = tempfile::tempdir().expect("tempdir");

    // A child that starts, then sleeps far longer than the test — the interrupt fires mid-run, and
    // pi's soft-interrupt semantics (`execution.ts:722-761`) make this a PAUSED SUCCESS: exit 0,
    // `interrupted: true`, a cleared error, and the "Interrupted. Waiting…" sentinel output — NOT
    // an exit-1 failure.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "sleep_ms", "ms": 30_000}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-interrupt.json", &script);


    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    let interrupt = CancelToken::new();
    opts.interrupt = interrupt.clone();

    // Fire the interrupt shortly after the run begins (the child is by then blocked in its sleep).
    let canceller = {
        let interrupt = interrupt.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            interrupt.cancel();
        })
    };

    let result = tokio::time::timeout(
        Duration::from_secs(15), // generous bound for the real SIGINT termination
        cyrup_ext_subagents::exec::run_sync(&agent, "long running task", &opts),
    )
    .await
    .expect("run_sync must return once the interrupt terminates the child");
    let _ = canceller.await;


    assert_eq!(
        result.exit_code, 0,
        "a soft interrupt is a PAUSED SUCCESS (exit 0), not an exit-1 failure: {result:?}"
    );
    assert!(result.interrupted, "the interrupted flag must be set: {result:?}");
    assert!(!result.timed_out, "an interrupt must not be reported as a timeout: {result:?}");
    assert!(
        result.error.is_none(),
        "a paused-success interrupt must clear the error, got: {:?}",
        result.error
    );
    assert!(
        result
            .final_output
            .as_deref()
            .unwrap_or_default()
            .contains("Interrupted"),
        "expected the paused-success sentinel output, got: {:?}",
        result.final_output
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_empty_output_is_a_retryable_failure_that_advances_the_fallback_ladder() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Every attempt (the single static script is reused for both models) emits a clean terminal
    // stop with EMPTY text — pi's empty-output (cold-start) classification (`execution.ts:781-789`)
    // makes this an exit-1 failure whose message ("no output") is RETRYABLE
    // (`model-fallback.ts:129-131`), so the ladder ADVANCES to the fallback model (two attempts) —
    // the observable proof that empty output triggers fallback, in contrast to the non-retryable
    // trailing-tool failure above.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "emit", "line": empty_message_end_line()}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-empty-output.json", &script);


    let mut agent = base_agent_config("primary-model");
    agent.fallback_models = vec![ModelId::from("fallback-model")];
    let mut opts = base_run_options(dir.path(), "primary-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.available_models = vec![ModelId::from("primary-model"), ModelId::from("fallback-model")];

    let result = tokio::time::timeout(
        Duration::from_secs(15),
        cyrup_ext_subagents::exec::run_sync(&agent, "Produce a summary", &opts),
    )
    .await
    .expect("run_sync must not hang against fast fixture children");


    assert_eq!(
        result.attempted_models.len(),
        2,
        "an empty-output failure is RETRYABLE, so the ladder MUST advance to the fallback model, \
         got {:?}",
        result.attempted_models
    );
    assert_eq!(
        result.attempted_models,
        vec![ModelId::from("primary-model"), ModelId::from("fallback-model")]
    );
    assert_eq!(result.model_attempts.len(), 2);
    assert!(
        result.model_attempts.iter().all(|attempt| !attempt.success),
        "both empty-output attempts must be recorded as failures: {:?}",
        result.model_attempts
    );
    assert_ne!(result.exit_code, 0, "the whole run fails once every attempt is empty: {result:?}");
    let error = result.error.expect("a cold-start/empty-output error must be surfaced");
    assert!(
        error.to_lowercase().contains("no output") || error.to_lowercase().contains("cold-start"),
        "expected the empty-output/cold-start error message, got: {error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_empty_output_with_a_declared_schema_but_no_structured_value_is_retryable() {
    let dir = tempfile::tempdir().expect("tempdir");

    // pi `execution.ts:786`: the empty-output gate is
    // `!finalText?.trim() && (!options.structuredOutput || missingStructuredOutput)`. When a
    // structured-output schema IS declared but the child produced NEITHER prose NOR any
    // structured-output value (`missingStructuredOutput` true), pi surfaces the RETRYABLE
    // "no output" cold-start error at the per-attempt level, so the fallback ladder ADVANCES —
    // it does NOT defer to a post-ladder, non-retryable "structured output missing" verdict. This
    // is the observable proof that a schema-declared cold-start empty run still retries, matching
    // the no-schema empty-output case above rather than short-circuiting on `structuredOutput`.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "emit", "line": empty_message_end_line()}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-empty-structured.json", &script);


    let mut agent = base_agent_config("primary-model");
    agent.fallback_models = vec![ModelId::from("fallback-model")];
    let mut opts = base_run_options(dir.path(), "primary-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.available_models = vec![ModelId::from("primary-model"), ModelId::from("fallback-model")];
    opts.structured_output_schema = Some(sample_structured_output_schema());

    let result = tokio::time::timeout(
        Duration::from_secs(15),
        cyrup_ext_subagents::exec::run_sync(&agent, "Produce a structured summary", &opts),
    )
    .await
    .expect("run_sync must not hang against fast fixture children");


    assert_eq!(
        result.attempted_models.len(),
        2,
        "a schema-declared but structured-missing AND empty-prose run is RETRYABLE, so the ladder \
         MUST advance to the fallback model, got {:?}",
        result.attempted_models
    );
    assert_eq!(
        result.attempted_models,
        vec![ModelId::from("primary-model"), ModelId::from("fallback-model")]
    );
    assert_ne!(result.exit_code, 0, "the whole run still fails once every attempt is empty: {result:?}");
    let error = result.error.expect("a cold-start/empty-output error must be surfaced");
    assert!(
        error.to_lowercase().contains("no output") || error.to_lowercase().contains("cold-start"),
        "expected the RETRYABLE empty-output/cold-start error (not a non-retryable \
         structured-missing verdict), got: {error}"
    );
}

// -------------------------------------------------------------------------------------------
// T3 group C — timeout contract: a timed-out run's acceptance ledger is `rejected` (NOT
// `not-required`), and its delivered output leads with the timeout message. Proven against the
// REAL signal-escalation ladder (the fixture ignores SIGINT and sleeps far past the deadline, so
// termination is confirmed only after SIGTERM), never a mock. pi `buildTimedOutAcceptanceLedger`
// (`execution.ts:101-113` @v0.34.0) + timeout preamble (`execution.ts:824-829`).
// -------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_timeout_yields_a_rejected_acceptance_ledger_and_a_timeout_message() {
    let dir = tempfile::tempdir().expect("tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": r#"{"type":"agent_start"}"#},
            {"kind": "sleep_ms", "ms": 30_000}
        ],
        "ignore_sigint": true,
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script-timeout-ledger.json", &script);


    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    // A contract that REQUIRES acceptance (Checked) — so a timed-out run is `rejected`, not
    // `not-required`. Both the nominal budget (for the message) and the wall-clock deadline are set.
    opts.acceptance = Some(AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]));
    opts.timeout_ms = Some(300);
    opts.deadline_at = Some(std::time::Instant::now() + Duration::from_millis(300));

    let result = tokio::time::timeout(
        Duration::from_secs(15),
        cyrup_ext_subagents::exec::run_sync(&agent, "long running task", &opts),
    )
    .await
    .expect("run_sync must return once the real signal escalation confirms termination");


    assert!(result.timed_out, "expected timed_out: true, got {result:?}");
    assert_ne!(result.exit_code, 0, "a timed-out run must fail: {result:?}");

    // The load-bearing assertion: the ledger is `rejected`, with a failed timeout runtime check —
    // NOT the `not-required` a non-clean gate would otherwise yield.
    let ledger = result
        .acceptance
        .expect("a timed-out run whose contract required acceptance must carry a ledger");
    assert_eq!(
        ledger.status,
        AcceptanceStatus::Rejected,
        "a timed-out run whose contract required acceptance must be REJECTED, got {ledger:?}"
    );
    assert!(
        ledger
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("timed out"),
        "the ledger must record the timeout as the reason acceptance was not evaluated: {ledger:?}"
    );

    // The delivered output leads with the timeout message (nominal budget), pi `formatTimeoutMessage`.
    let output = result.final_output.expect("a timed-out run still delivers a message");
    assert!(
        output.contains("timed out after 300ms"),
        "the delivered output must lead with the timeout message: {output}"
    );
}

// -------------------------------------------------------------------------------------------
// T3 group C — stderr surfacing: a child that exits non-zero with content on STDERR has that
// stderr surfaced into `SingleResult::error` (pi `execution.ts:686`), not drained-and-discarded.
// Proven against a REAL child that writes to its real stderr pipe and exits 2.
// -------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_sync_surfaces_a_failed_childs_stderr_into_the_result_error() {
    let dir = tempfile::tempdir().expect("tempdir");

    const STDERR_DETAIL: &str = "fatal: the child could not open the workspace";
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": r#"{"type":"agent_start"}"#},
            {"kind": "emit", "line": message_end_line("partial work before failing", 4, 2)},
            {"kind": "emit_stderr", "line": STDERR_DETAIL},
        ],
        "exit_code": 2
    });
    let script_path = write_script(dir.path(), "script-stderr.json", &script);


    let agent = base_agent_config("fixture-model");
    // Single model, no fallback — the failure must not advance a ladder; assert on the one attempt.
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "do the work", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, non-zero-exit fixture child");


    assert_ne!(result.exit_code, 0, "the child exited non-zero: {result:?}");
    let error = result
        .error
        .expect("a non-zero-exit run with stderr must carry an error");
    assert!(
        error.contains(STDERR_DETAIL),
        "the child's stderr must be surfaced into the result error (pi execution.ts:686), not \
         drained-and-discarded — got: {error}"
    );
}

// -------------------------------------------------------------------------------------------
// SUBA-N06 — `includeProgress`: R-SA-043 compaction's ONE documented opt-out.
//
// pi gates its `details.progress` array on the flag and nothing else
// (`progress: params.includeProgress ? allProgress : undefined`,
// `runs/foreground/subagent-executor.ts:3819` @v0.43.0 for SINGLE, `:2679` for PARALLEL). cyrup's
// SINGLE-mode `details` IS the serialized `SingleResult`, so the snapshot lands on
// `SingleResult::progress` and surfaces at the same JSON path a pi caller reads.
//
// These three tests fail against the pre-fix tree, where `run_sync` had `let _ =
// opts.include_progress;` and a hardcoded `progress: None`:
//   * the flag-ON test finds `None` where a snapshot must be;
//   * the byte-identity test passes there but is the guard that keeps the default path clean;
//   * the interrupt test finds `None` where pi's uncompacted `running` snapshot must be.
// -------------------------------------------------------------------------------------------

/// A three-tool, two-turn script whose child text is deliberately chatty, so the settled
/// snapshot's counters are non-trivial AND the compaction assertions have something to erase.
fn progress_script() -> serde_json::Value {
    serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "emit", "line": tool_execution_start_line("c1", "read")},
            {"kind": "emit", "line": tool_execution_end_line("c1", "read")},
            {"kind": "emit", "line": message_end_line("thinking out loud", 10, 5)},
            {"kind": "emit", "line": tool_execution_start_line("c2", "edit")},
            {"kind": "emit", "line": tool_execution_end_line("c2", "edit")},
            {"kind": "emit", "line": tool_execution_start_line("c3", "bash")},
            {"kind": "emit", "line": tool_execution_end_line("c3", "bash")},
            {"kind": "emit", "line": message_end_line("Done: the change is applied.", 30, 7)},
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_end"}"#.to_string())}
        ],
        "exit_code": 0
    })
}

/// Run `progress_script()` once with the given `include_progress` value, returning the result.
async fn run_progress_fixture(
    dir: &std::path::Path,
    script_name: &str,
    include_progress: Option<bool>,
) -> cyrup_ext_subagents::exec::SingleResult {
    let script_path = write_script(dir, script_name, &progress_script());
    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir, "fixture-model");
    // The fixture named for THIS run rather than moved into the process environment every
    // concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.include_progress = include_progress;
    // A stable child index + skill list, so the snapshot's launch-context fields are assertable
    // rather than all-default (pi seeds `progress.index`/`progress.skills` at construction,
    // `runs/foreground/execution.ts:259,263` @v0.34.0).
    opts.child_index = Some(3);
    tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "chatty task", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn include_progress_true_returns_a_compacted_pi_shaped_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let result = run_progress_fixture(dir.path(), "script-progress-on.json", Some(true)).await;

    assert_eq!(result.exit_code, 0, "the fixture child exits clean: {result:?}");
    let progress = result
        .progress
        .clone()
        .expect("includeProgress: true must populate SingleResult::progress");

    // pi's launch-context fields, seeded at construction (`execution.ts:258-270` @v0.34.0).
    assert_eq!(progress.index, 3, "pi `progress.index` ← options.index");
    assert_eq!(progress.agent.as_deref(), Some("worker"));
    assert_eq!(progress.task, "chatty task");
    assert_eq!(
        progress.status,
        cyrup_ext_subagents::tui::events::LiveProgressStatus::Complete,
        "exit 0 with no error is pi's `completed` (`execution.ts:907`)"
    );

    // pi's live counters: three `tool_execution_start`s, two ASSISTANT turns, and
    // `tokens = usage.input + usage.output` (`execution.ts:646`).
    assert_eq!(progress.tool_count, 3);
    assert_eq!(progress.tokens, 10 + 5 + 30 + 7);
    assert!(progress.error.is_none(), "a clean run names no error");
    assert!(
        progress.failed_tool.is_none(),
        "pi sets `failedTool` only when there is BOTH an error and a tool in flight"
    );

    // pi `compactCompletedProgress` (`shared/utils.ts:329-345`): a SETTLED snapshot keeps eleven
    // keys and empties the two growth terms, dropping every other field from its object literal.
    assert!(
        progress.recent_tools.is_empty(),
        "compactCompletedProgress resets recentTools to []: {:?}",
        progress.recent_tools
    );
    assert!(
        progress.recent_output.is_empty(),
        "compactCompletedProgress resets recentOutput to []: {:?}",
        progress.recent_output
    );
    assert!(progress.current_tool.is_none(), "absent from pi's literal");
    assert_eq!(progress.turn_count, 0, "absent from pi's literal");
    assert!(progress.model.is_none(), "absent from pi's literal");
    assert!(progress.input_tokens.is_none(), "absent from pi's literal");
    assert!(progress.output_tokens.is_none(), "absent from pi's literal");

    // ...and it really is on the wire, at the JSON path a pi caller reads for a SINGLE run.
    let wire = serde_json::to_value(&result).expect("SingleResult serializes");
    assert_eq!(wire["progress"]["agent"], serde_json::json!("worker"));
    assert_eq!(wire["progress"]["status"], serde_json::json!("completed"));
    assert_eq!(wire["progress"]["toolCount"], serde_json::json!(3));
    assert!(
        wire["progress"].get("recentTools").is_none(),
        "an emptied ring is skipped on the wire, not serialized as []"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn include_progress_omitted_or_false_is_byte_identical_to_the_pre_flag_result() {
    let dir = tempfile::tempdir().expect("tempdir");

    // R-SA-043 compaction is the DEFAULT and `includeProgress` is precisely its opt-out, so both
    // falsy forms must produce a result whose serialization is byte-for-byte what it was before
    // the `progress` field existed — i.e. the key is absent entirely, not `null`, not `{}`.
    let omitted = run_progress_fixture(dir.path(), "script-progress-omitted.json", None).await;
    let explicit_false =
        run_progress_fixture(dir.path(), "script-progress-false.json", Some(false)).await;

    assert!(omitted.progress.is_none(), "an omitted flag populates nothing");
    assert!(
        explicit_false.progress.is_none(),
        "pi's gate is truthiness — `includeProgress: false` is the same as omitting it"
    );

    // Byte identity, asserted on the real serialized bytes. `duration_ms` is wall-clock and lives
    // on the snapshot (which is absent here), never on `SingleResult` itself, so the two runs of
    // the same script serialize identically.
    let a = serde_json::to_string(&omitted).expect("serialize");
    let b = serde_json::to_string(&explicit_false).expect("serialize");
    assert_eq!(
        a, b,
        "an omitted and an explicitly-false includeProgress must serialize identically"
    );
    assert!(
        !a.contains("\"progress\""),
        "the compacted default must not carry a `progress` key at all: {a}"
    );

    // The flag must change NOTHING else about the result — the same run with the flag ON differs
    // in exactly one key.
    let on = run_progress_fixture(dir.path(), "script-progress-on2.json", Some(true)).await;
    let mut on_wire = serde_json::to_value(&on).expect("serialize");
    let off_wire = serde_json::to_value(&omitted).expect("serialize");
    assert!(
        on_wire
            .as_object_mut()
            .expect("object")
            .remove("progress")
            .is_some(),
        "the flag-ON run must carry a progress key to remove"
    );
    assert_eq!(
        on_wire, off_wire,
        "includeProgress must add the `progress` key and change nothing else"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn include_progress_on_an_interrupt_paused_run_keeps_pis_uncompacted_running_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");

    // pi leaves an interrupt-paused run's progress at `"running"` (`execution.ts:828`, returning at
    // `:861` before the `completed`/`failed` assignment at `:907`), and
    // `compactCompletedProgress` deliberately refuses to compact a `running` snapshot — the caller
    // is expected to RESUME the run, so its live detail is exactly what it needs. This asserts
    // cyrup reproduces that, and — the adversarial half — that the rings it therefore keeps are
    // BOUNDED, which upstream's are not.
    let mut steps = vec![serde_json::json!({"kind": "emit", "line": r#"{"type":"agent_start"}"#})];
    // Far more tool calls than RECENT_TOOLS_CAP (32) and far more output lines than
    // RECENT_OUTPUT_CAP (50), all before the child blocks.
    for i in 0..80 {
        steps.push(serde_json::json!({"kind": "emit", "line": tool_execution_start_line(&format!("c{i}"), "bash")}));
        steps.push(serde_json::json!({"kind": "emit", "line": tool_execution_end_line(&format!("c{i}"), "bash")}));
        steps.push(serde_json::json!({"kind": "emit", "line": message_end_line(&format!("chatter line {i}"), 1, 1)}));
    }
    steps.push(serde_json::json!({"kind": "sleep_ms", "ms": 30_000}));
    let script = serde_json::json!({ "steps": steps, "exit_code": 0 });
    let script_path = write_script(dir.path(), "script-progress-interrupt.json", &script);


    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.include_progress = Some(true);
    let interrupt = CancelToken::new();
    opts.interrupt = interrupt.clone();
    let canceller = tokio::spawn({
        let interrupt = interrupt.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            interrupt.cancel();
        }
    });

    let result = tokio::time::timeout(
        Duration::from_secs(15),
        cyrup_ext_subagents::exec::run_sync(&agent, "chatty long task", &opts),
    )
    .await
    .expect("run_sync must return once the interrupt terminates the child");
    let _ = canceller.await;


    assert!(result.interrupted, "the run must be interrupt-paused: {result:?}");
    let progress = result
        .progress
        .expect("includeProgress: true must populate progress even on a paused run");
    assert_eq!(
        progress.status,
        cyrup_ext_subagents::tui::events::LiveProgressStatus::Running,
        "pi leaves an interrupt-paused run at `running`, which is why it is not compacted"
    );

    // ADVERSARIAL: a chatty long-running child cannot inflate this snapshot without bound. pi's
    // live arrays grow unbounded here (it slices only when streaming); this port evicts at push.
    assert!(
        progress.recent_tools.len() <= cyrup_ext_subagents::tui::events::RECENT_TOOLS_CAP,
        "recentTools must stay within MAX_STREAMED_RECENT_TOOLS, got {}",
        progress.recent_tools.len()
    );
    assert!(
        progress.recent_output.len() <= cyrup_ext_subagents::exec::RECENT_OUTPUT_CAP,
        "recentOutput must stay within pi's 50-line window, got {}",
        progress.recent_output.len()
    );
    for line in &progress.recent_output {
        assert!(
            line.chars().count()
                <= cyrup_ext_subagents::exec::RECENT_OUTPUT_LINE_CHARS + "… [truncated]".chars().count(),
            "no single recentOutput line may exceed the per-line cap: {} chars",
            line.chars().count()
        );
        assert!(
            !line.contains("\"type\":\"message_end\""),
            "recentOutput carries EXTRACTED text, never the raw NDJSON envelope: {line}"
        );
    }
    assert!(
        progress.recent_output.iter().any(|l| l.starts_with("chatter line ")),
        "the child's own extracted text must be what survived: {:?}",
        progress.recent_output
    );
}

// -------------------------------------------------------------------------------------------
// SUBA-008: the turn budget, end to end against a real child process.
//
// This is the item's own Verify recipe, corrected on one point: the item writes
// `turnBudget:{hard:2}`, which is the TOOL budget's key shape. Upstream's turn budget takes
// `{maxTurns, graceTurns}` (`extension/schemas.ts:104-107` @v0.43.0) and has no `hard`.
// -------------------------------------------------------------------------------------------

/// One NON-terminal assistant `message_end` — `stopReason` is `toolUse`, so pi's
/// `terminalAssistantStop` (`execution.ts:921`) is FALSE and the budget's first decision arm does
/// not short-circuit. A child that stops cleanly is never aborted however far over budget it is,
/// which is exactly why the enforcement test must not use [`message_end_line`].
fn working_message_end_line(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "toolUse"
        }
    })
    .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_budget_wraps_up_at_max_turns_and_aborts_the_child_after_the_grace_turn() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Four working turns are scripted, then a long sleep. With maxTurns 2 / graceTurns 1 the
    // supervisor must request wrap-up on turn 2 and ABORT on turn 3 — so the fourth turn and the
    // sleep must never be observed, which is what makes this a test of enforcement rather than of
    // bookkeeping.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "emit", "line": working_message_end_line("still thinking 1")},
            {"kind": "emit", "line": working_message_end_line("still thinking 2")},
            {"kind": "emit", "line": working_message_end_line("still thinking 3")},
            {"kind": "sleep_ms", "ms": 30000},
            {"kind": "emit", "line": working_message_end_line("never reached")}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "turn-budget.json", &script);


    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.turn_budget = Some(cyrup_ext_subagents::exec::turn_budget::ResolvedTurnBudget {
        max_turns: 2,
        grace_turns: 1,
    });

    let result = tokio::time::timeout(
        Duration::from_secs(20),
        cyrup_ext_subagents::exec::run_sync(&agent, "Work on this forever", &opts),
    )
    .await
    .expect("the turn-budget abort must end the run well inside the child's 30s sleep");


    // pi `result.turnBudgetExceeded = true` + `turnBudgetState(budget, turnCount, true)`
    // (`execution.ts:737-739`).
    assert!(
        result.turn_budget_exceeded,
        "the third assistant turn crosses maxTurns(2)+graceTurns(1) and must abort: {result:?}"
    );
    assert!(result.wrap_up_requested, "the soft limit was reached, so wrap-up was requested");
    let state = result.turn_budget.expect("an aborted run must publish its budget state");
    assert_eq!(
        state.outcome,
        cyrup_ext_subagents::exec::turn_budget::TurnBudgetOutcome::Exceeded
    );
    assert_eq!(state.turn_count, 3, "the abort fires ON the third assistant turn");
    assert_eq!(state.max_turns, 2);
    assert_eq!(state.grace_turns, 1);
    // `wrapUpRequestedAtTurn` is the THRESHOLD (pi's literal `budget.maxTurns`), while
    // `exceededAtTurn` is the OBSERVED turn — the two differ here, which is the point.
    assert_eq!(state.wrap_up_requested_at_turn, Some(2));
    assert_eq!(state.exceeded_at_turn, Some(3));
    assert_eq!(state.termination_deferred_at_turn, None);

    // pi `result.error = turnBudgetExceededMessage(...)` (`execution.ts:740`), verbatim.
    assert_eq!(
        result.error.as_deref(),
        Some("Subagent exceeded turn budget after 3 assistant turns (soft limit 2 + grace 1)."),
        "the abort message must be upstream's, and must outrank every other diagnosis: {result:?}"
    );
    assert_ne!(result.exit_code, 0, "a budget abort is a failure, not a clean exit");
    assert!(!result.timed_out, "this is a budget abort, not the orchestrator's deadline");
    assert!(!result.interrupted);

    // pi `formatTurnBudgetOutput(message, fullOutput)` (`execution.ts:1252`): the message leads,
    // and whatever the child DID produce follows under upstream's own heading.
    let output = result.final_output.expect("an aborted run still delivers its partial output");
    assert!(
        output.starts_with("Subagent exceeded turn budget after 3 assistant turns"),
        "the abort message must lead the delivered output: {output}"
    );
    assert!(
        output.contains("Partial output before turn-budget abort:"),
        "upstream's partial-output heading must be present: {output}"
    );
    assert!(
        output.contains("still thinking 3"),
        "the child's real partial output must survive the abort: {output}"
    );
    assert!(
        !output.contains("never reached"),
        "the child must have been killed before its post-sleep turn: {output}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_that_finishes_inside_its_turn_budget_is_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");

    // ADVERSARIAL, and the reason this test exists next to the one above: the SAME budget must be
    // completely inert for a child that stops on its own. Without this, an enforcement bug that
    // aborted every budgeted run would still pass the abort test.
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "emit", "line": message_end_line("All done.", 5, 3)},
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_end"}"#.to_string())}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "within-budget.json", &script);


    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The fixture named for THIS run rather than moved into the process
    // environment every concurrently-running test in this binary shares.
    opts.spawn_command = Some(SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });
    opts.turn_budget = Some(cyrup_ext_subagents::exec::turn_budget::ResolvedTurnBudget {
        max_turns: 2,
        grace_turns: 1,
    });

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "Answer briefly", &opts),
    )
    .await
    .expect("run_sync must not hang");


    assert_eq!(result.exit_code, 0, "an in-budget run is untouched: {result:?}");
    assert!(!result.turn_budget_exceeded);
    assert!(!result.wrap_up_requested);
    assert_eq!(result.error, None);
    assert_eq!(
        result.final_output.as_deref(),
        Some("All done."),
        "no turn-budget note may be folded onto an in-budget run's output"
    );
    let state = result
        .turn_budget
        .expect("pi stamps `initialTurnBudgetState` on any budgeted run (execution.ts:399)");
    assert_eq!(
        state.outcome,
        cyrup_ext_subagents::exec::turn_budget::TurnBudgetOutcome::WithinBudget
    );
    assert_eq!(state.turn_count, 1);
}

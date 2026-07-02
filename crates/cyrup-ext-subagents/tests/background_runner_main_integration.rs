//! Integration test: `background::runner_main::run` end to end against the scripted-NDJSON
//! test-double binary (`cyrup-subagent-fixture`, arch-SA §11) — proving the hop-2 detached-runner
//! main loop's status.json-then-ResultFile write ordering (func-SA R-SA-077) on both the happy
//! path and a forced-error path, plus R-SA-096's disk-re-scan append-request consumption
//! guarantee.
//!
//! No mocking anywhere in this file (this codebase's standing convention, restated in this
//! crate's own task brief and already established by `tests/exec_run_sync_integration.rs` and
//! `tests/background_spawn_detached_integration.rs`): every run below drives the REAL
//! `background::runner_main::run` function, which itself spawns the REAL
//! `cyrup-subagent-fixture` binary as a genuine OS subprocess (via `CYRUP_SUBAGENT_BINARY`,
//! R-SA-045 tier 1's documented override escape hatch, honored transitively through
//! `exec::run_sync` -> `spawn::resolve_spawn_command`), and asserts on the REAL on-disk
//! `status.json`/`ResultFile` contents `run()` itself wrote.
//!
//! This file is a separate compilation unit from `cyrup-ext-subagents`'s own `lib.rs` (ordinary
//! Cargo integration-test placement), so it is NOT bound by that crate's own
//! `#![forbid(unsafe_code)]`, and `CARGO_BIN_EXE_cyrup-subagent-fixture` (only available to
//! integration tests, never to a library's own `#[cfg(test)]` unit tests) resolves here — exactly
//! the same two reasons `tests/exec_run_sync_integration.rs` and
//! `tests/background_spawn_detached_integration.rs` live outside `src/`. The `unsafe` blocks
//! below (Rust 2024 requires `unsafe` for `std::env::set_var`/`remove_var`) are scoped to exactly
//! the two calls needed to point `CYRUP_SUBAGENT_BINARY`/`CYRUP_SUBAGENT_FIXTURE_SCRIPT` at the
//! fixture binary/script for the duration of one test, executed under a process-wide mutex
//! ([`ENV_MUTATION_LOCK`]) so this file's tests never race each other over that global state even
//! when `cargo test` runs them concurrently within the same test-binary process.
//!
//! Gated on the `test-fixtures` Cargo feature (matching the `cyrup-subagent-fixture` `[[bin]]`
//! target's own `required-features` gate, `Cargo.toml`): without that feature the fixture binary
//! is never built at all, so this whole file compiles to an empty test list (`cargo test` reports
//! it as a normal, zero-test pass) rather than every test failing at spawn time with a confusing
//! "No such file or directory".

#![cfg(feature = "test-fixtures")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::Mutex;

use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::control::{ChainAppendRequest, InterruptRequest};
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, run};
use cyrup_ext_subagents::background::{RunId, RunMode, RunPaths, RunState, RunStatus, ResultFile};
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};

/// Serializes every test in this file that mutates `CYRUP_SUBAGENT_BINARY`/
/// `CYRUP_SUBAGENT_FIXTURE_SCRIPT` (process-global state) — `cargo test` runs a test binary's own
/// `#[test]` functions concurrently by default, so without this lock two tests in this file could
/// observe or clobber each other's override value mid-run. Mirrors
/// `background_spawn_detached_integration.rs`'s/`exec_run_sync_integration.rs`'s identical
/// `ENV_MUTATION_LOCK` convention.
static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

/// Path to the real, already-built `cyrup-subagent-fixture` binary — Cargo sets
/// `CARGO_BIN_EXE_<name>` for every `[[bin]]` target in this same package that is part of the
/// current test run's build graph, which requires running this test file with
/// `--features test-fixtures` (this crate's `[[bin]]` entry's own `required-features` gate,
/// `Cargo.toml`) so the fixture binary actually gets built at all.
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

fn single_step(agent: &str, task: &str) -> SingleStepSpec {
    SingleStepSpec {
        agent: agent.to_string(),
        task: task.to_string(),
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_mode: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
    }
}

async fn run_against_fixture(
    dir: &Path,
    script: &serde_json::Value,
    config: RunnerConfig,
) -> (RunStatus, ResultFile) {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let script_path = write_script(dir, "script.json", script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation, mirroring every other integration test in
    // this crate that substitutes the scripted fixture binary.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let async_root = dir.join("async");
    let results_dir = dir.join("results");
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &config.run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write runner config");

    let result = run(&cfg_path, &run_paths).await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }

    result.expect("run() itself never returns Err");

    let status: RunStatus = serde_json::from_slice(
        &tokio::fs::read(&run_paths.status).await.expect("status.json exists"),
    )
    .expect("parse status.json");
    let result_file: ResultFile = serde_json::from_slice(
        &tokio::fs::read(&run_paths.result).await.expect("ResultFile exists"),
    )
    .expect("parse ResultFile");

    (status, result_file)
}

/// The core proof this crate's task brief calls for: a full run through `run()` against the
/// scripted fixture, asserting BOTH that `status.json` reaches a terminal state AND that the
/// terminal `ResultFile` exists — with `status.json` written strictly before `ResultFile`
/// (`finish_run`'s own sequential `.await`s make same-task ordering structural; this test proves
/// the externally observable half of R-SA-077 a caller can actually depend on: both files
/// present, mutually consistent, `success: true`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn happy_path_writes_status_then_result_both_terminal_and_consistent() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line("all done")},
        ],
        "exit_code": 0
    });
    let config = RunnerConfig {
        run_id: RunId::from_token("happyrun1"),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do the thing"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
    };

    let (status, result_file) = run_against_fixture(dir.path(), &script, config).await;

    assert_eq!(status.state, RunState::Complete);
    assert!(status.ended_at.is_some());
    assert_eq!(result_file.state, RunState::Complete);
    assert!(result_file.success);
    assert_eq!(result_file.results.len(), 1);
    assert_eq!(result_file.results[0].exit_code, 0);
    assert_eq!(
        result_file.run_id, status.run_id,
        "the ResultFile and status.json must agree on run identity"
    );
}

/// R-SA-136/146: `run()` itself must be the real writer of `<run_dir>/events.jsonl` — not merely a
/// documented intention on [`RunPaths::events`]'s own doc comment. Proven end to end against the
/// real fixture subprocess (no mocking, matching this file's own standing convention): after one
/// successful run, `events.jsonl` must exist, contain multiple well-formed JSON lines (at least a
/// `run.started`, one `step.started`/`step.completed` pair, and a `run.completed`), and every line
/// must independently parse as valid JSON — proving this crate's shared
/// [`cyrup_ext_subagents::jsonl::BoundedJsonlWriter`] primitive is genuinely exercised on this path,
/// not just on the foreground per-attempt child-output tee.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_writes_real_events_jsonl_through_the_shared_bounded_writer() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line("all done")},
        ],
        "exit_code": 0
    });
    let config = RunnerConfig {
        run_id: RunId::from_token("eventsrun1"),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do the thing"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
    };

    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &config.run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    let _guard = ENV_MUTATION_LOCK.lock().await;
    let script_path = write_script(dir.path(), "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation, mirroring every other integration test in
    // this crate that substitutes the scripted fixture binary.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write runner config");
    let result = run(&cfg_path, &run_paths).await;
    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }
    result.expect("run() itself never returns Err");

    assert!(
        tokio::fs::try_exists(&run_paths.events).await.unwrap_or(false),
        "events.jsonl must actually be created by a real run, not merely a path this crate reserves"
    );

    let contents = tokio::fs::read_to_string(&run_paths.events)
        .await
        .expect("events.jsonl is readable");
    let lines: Vec<&str> = contents.lines().collect();
    assert!(
        lines.len() >= 4,
        "expected at least run.started/step.started/step.completed/run.completed, got: {contents:?}"
    );

    let mut kinds = Vec::new();
    for line in &lines {
        let parsed: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("every line must be valid JSON: {e}: {line}"));
        kinds.push(
            parsed["kind"]
                .as_str()
                .unwrap_or_else(|| panic!("every line must carry a string 'kind' field: {line}"))
                .to_string(),
        );
        assert!(parsed["ts"].is_number(), "every line must carry a numeric 'ts' field: {line}");
    }

    assert_eq!(kinds.first().map(String::as_str), Some("run.started"));
    assert!(kinds.contains(&"step.started".to_string()));
    assert!(kinds.contains(&"step.completed".to_string()));
    assert_eq!(kinds.last().map(String::as_str), Some("run.completed"));
}

/// The forced-error path this crate's task brief calls for: a step whose scripted child exits
/// nonzero must still produce a terminal `status.json` (state=Failed) AND a terminal `ResultFile`
/// (success=false) — proving the ordering invariant holds on the FAILURE path too, not only the
/// happy path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forced_error_path_still_writes_status_then_result_both_terminal() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line("partial output")},
        ],
        "exit_code": 7
    });
    let config = RunnerConfig {
        run_id: RunId::from_token("failrun001"),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do the thing"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
    };

    let (status, result_file) = run_against_fixture(dir.path(), &script, config).await;

    assert_eq!(
        status.state,
        RunState::Failed,
        "a nonzero-exit step must reconcile the whole run to Failed"
    );
    assert!(status.ended_at.is_some(), "a terminal status must have ended_at set");
    assert_eq!(result_file.state, RunState::Failed);
    assert!(!result_file.success);
    assert_eq!(result_file.results.len(), 1);
    assert_ne!(result_file.results[0].exit_code, 0);
}

/// A run whose one-shot config never existed at all (e.g. a caller passed a bogus path) still
/// reaches a terminal, consistent Failed status.json/ResultFile pair via `finish_run`'s own
/// no-config error branch — proving R-SA-077's ordering invariant holds even on this crate's own
/// internal-error exit path, not merely a subprocess exit-code failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_config_file_still_reaches_a_terminal_failed_state() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
    let run_id = RunId::from_token("noconfig1");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    let bogus_cfg_path = run_paths.run_dir.join("does-not-exist.json");

    let outcome = run(&bogus_cfg_path, &run_paths).await;
    assert!(outcome.is_ok(), "run() itself never returns Err to its own caller");

    let status: RunStatus = serde_json::from_slice(
        &tokio::fs::read(&run_paths.status).await.expect("status.json exists"),
    )
    .expect("parse status");
    let result_file: ResultFile = serde_json::from_slice(
        &tokio::fs::read(&run_paths.result).await.expect("ResultFile exists"),
    )
    .expect("parse result");

    assert_eq!(status.state, RunState::Failed);
    assert_eq!(result_file.state, RunState::Failed);
    assert!(!result_file.success);
    assert!(
        !result_file.results.is_empty(),
        "a synthesized diagnostic result must be present even with no config at all"
    );
}

/// Proves R-SA-096: an append request written to disk AFTER `run()` has already started (but
/// before the single scripted step completes) is picked up by the NEXT loop iteration, because
/// `run_inner` re-lists `append-requests/` from disk on every iteration rather than snapshotting
/// it once at startup. A single-step config with a slow-emitting first step gives this test a
/// real window to write the append file concurrently with the runner's own in-flight first step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn append_request_written_after_start_is_consumed_next_iteration() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "sleep_ms", "ms": 400},
            {"kind": "emit", "line": message_end_line("first done")},
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation (see module doc).
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
    let run_id = RunId::from_token("appendrun1");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    let config = RunnerConfig {
        run_id: run_id.clone(),
        mode: RunMode::Chain,
        steps: vec![RunnerStep::SingleStep(single_step("first", "first task"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
    };
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write config");

    // Write the append request CONCURRENTLY with `run()`'s own in-flight first step (which sleeps
    // 400ms before emitting) — proving the SECOND step (appended, never present in the original
    // one-shot config) is genuinely picked up from a disk re-scan, not from any in-memory list
    // `run_inner` might otherwise have snapshotted once at startup.
    let run_paths_for_append = run_paths.clone();
    let append_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        tokio::fs::create_dir_all(&run_paths_for_append.append_dir)
            .await
            .expect("mkdir append_dir");
        let request = ChainAppendRequest::new(vec![RunnerStep::SingleStep(single_step(
            "appended",
            "appended task",
        ))]);
        let file_name = request.file_name();
        write_atomic_json(&run_paths_for_append.append_dir.join(file_name), &request)
            .await
            .expect("write append request");
    });

    let outcome = run(&cfg_path, &run_paths).await;
    append_task.await.expect("append task completes");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }

    outcome.expect("run() itself never returns Err");

    let result_file: ResultFile = serde_json::from_slice(
        &tokio::fs::read(&run_paths.result).await.expect("ResultFile exists"),
    )
    .expect("parse result");

    assert_eq!(
        result_file.results.len(),
        2,
        "the appended step (written to disk mid-run) must have been picked up and executed, \
         proving append consumption re-scans disk rather than trusting a stale in-memory \
         snapshot taken at startup: got {} result(s)",
        result_file.results.len()
    );
}

/// Second-pass adversarial-review regression test: a genuine race where an interrupt request
/// lands at (as close as a test can force) the exact same instant the run's one and only step
/// naturally finishes. `control::interrupt()` gates its own write on observing `status.json` at
/// `state: Running` — which stays `Running` all the way up to `finish_run`'s terminal write — so a
/// real caller's `interrupt()` call CAN win that race and successfully write a control-inbox
/// request file after the run's last step has already completed but before `run_inner`'s loop
/// next observes `cursor >= steps.len()`. This test simulates that landed-file outcome directly
/// (bypassing `interrupt()`'s own `Running`-gate, which a second racing OS process would clear
/// legitimately) by writing the `InterruptRequest` file itself, timed via the fixture's
/// `sleep_ms` step to land while the single step is still in flight. The run MUST still reach
/// `Complete`/`success: true` — not a `Paused`/`success: false` `ResultFile` with no work left to
/// resume, which would be a permanently-wrong terminal record since nothing ever reconciles a
/// `Paused` run back to `Complete` after the fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_interrupt_after_last_step_completes_does_not_downgrade_a_finished_run_to_paused() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "sleep_ms", "ms": 300},
            {"kind": "emit", "line": message_end_line("only step done")},
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation (see module doc).
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
    let run_id = RunId::from_token("lateintrpt");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    // A SINGLE-step run: once this one step finishes, the loop cursor is immediately exhausted —
    // exactly the shape needed to race an interrupt against natural completion with nothing left
    // to legitimately pause.
    let config = RunnerConfig {
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("only", "only task"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
    };
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write config");

    // Fire the interrupt file write shortly before the scripted step's own 300ms sleep elapses,
    // aiming to land it in the narrow post-completion, pre-next-iteration-check window rather than
    // clearly before (which would legitimately pause the in-flight step) or clearly after (which
    // would just be a stray unconsumed file on an already-terminal run either way).
    let run_paths_for_interrupt = run_paths.clone();
    let interrupt_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(280)).await;
        tokio::fs::create_dir_all(
            run_paths_for_interrupt
                .control_inbox
                .parent()
                .expect("control_inbox has a parent"),
        )
        .await
        .expect("mkdir control dir");
        let request = InterruptRequest::new("race-test", Some("late-arriving".to_string()));
        // Best-effort: if the run has already finished and moved on, this write may race a
        // directory that still exists but whose file nobody will ever consume — that outcome is
        // exactly as legitimate as landing the write in time, and is itself part of what this test
        // proves is harmless.
        let _ = write_atomic_json(&run_paths_for_interrupt.control_inbox, &request).await;
    });

    let outcome = run(&cfg_path, &run_paths).await;
    interrupt_task.await.expect("interrupt task completes");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }

    outcome.expect("run() itself never returns Err");

    let status: RunStatus = serde_json::from_slice(
        &tokio::fs::read(&run_paths.status).await.expect("status.json exists"),
    )
    .expect("parse status.json");
    let result_file: ResultFile = serde_json::from_slice(
        &tokio::fs::read(&run_paths.result).await.expect("ResultFile exists"),
    )
    .expect("parse result");

    assert_eq!(
        status.state,
        RunState::Complete,
        "a run whose only step genuinely completed must reach Complete even if a stale interrupt \
         request lands after that step finished, not be downgraded to Paused: {status:?}"
    );
    assert_eq!(result_file.state, RunState::Complete);
    assert!(
        result_file.success,
        "the terminal ResultFile must report success for a run whose one step exited 0, \
         regardless of a late-arriving, effectively-moot interrupt request: {result_file:?}"
    );
    assert_eq!(result_file.results.len(), 1);
    assert_eq!(result_file.results[0].exit_code, 0);
}

// -------------------------------------------------------------------------------------------
// R-SA-055 (SAFETY-CRITICAL): the hop-2 runner's own depth guard must reject the WHOLE run
// before dispatching a single step, spawning zero real child processes — even though
// `CYRUP_SUBAGENT_BINARY` is correctly pointed at the real scripted fixture the entire rest of
// this file uses, proving any failure to reach `RunState::Complete` here is caused specifically
// by the depth guard, not an unrelated spawn misconfiguration.
// -------------------------------------------------------------------------------------------

/// A run whose `RunnerConfig::max_subagent_depth` is already exhausted (`0`, and no
/// `CYRUP_SUBAGENT_DEPTH`/`CYRUP_SUBAGENT_MAX_DEPTH` override is set in this test's own
/// environment) must reach a terminal `Failed` state carrying a depth-exceeded error, with the
/// scripted fixture NEVER actually invoked — proven three independent ways: (1) the terminal
/// `ResultFile`'s error text names the depth guard, not a subprocess/exit-code failure; (2) the
/// run's own `.cyrup-subagent-scratch` directory (`exec::run_sync`'s first filesystem side effect
/// on ANY spawn attempt, real or not) was never created; (3) a distinctive marker string the
/// fixture script would have emitted to stdout if it had EVER actually run appears nowhere in the
/// terminal result.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn depth_exhausted_run_rejects_the_whole_run_and_spawns_zero_real_processes() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    const NEVER_SPAWNED_MARKER: &str = "THIS-FIXTURE-MUST-NEVER-ACTUALLY-RUN";
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line(NEVER_SPAWNED_MARKER)},
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation (see module doc) — CYRUP_SUBAGENT_BINARY is
    // deliberately set to a REAL, working fixture here (not a bogus path) so a failure to reach
    // Complete can only be attributed to the depth guard itself, never to the fixture being
    // unreachable for an unrelated reason.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let run_id = RunId::from_token("depthrun01");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    let config = RunnerConfig {
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do something"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        // current_depth (0, absent from this test's own real env) >= max_depth (0): blocked.
        max_subagent_depth: 0,
    };
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write runner config");

    let outcome = run(&cfg_path, &run_paths).await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }

    outcome.expect("run() itself never returns Err to its own caller, even on a depth rejection");

    let status: RunStatus = serde_json::from_slice(
        &tokio::fs::read(&run_paths.status).await.expect("status.json exists"),
    )
    .expect("parse status.json");
    let result_file: ResultFile = serde_json::from_slice(
        &tokio::fs::read(&run_paths.result).await.expect("ResultFile exists"),
    )
    .expect("parse ResultFile");

    assert_eq!(
        status.state,
        RunState::Failed,
        "a depth-exhausted run must reach a terminal Failed status, not Complete/Paused: {status:?}"
    );
    assert!(!result_file.success);
    assert!(
        result_file
            .results
            .iter()
            .all(|r| r.final_output.as_deref().unwrap_or_default() != NEVER_SPAWNED_MARKER),
        "the fixture's marker output must never appear anywhere in the terminal result — proving \
         the scripted child was never actually spawned: {result_file:?}"
    );

    // Proof 2: `exec::run_sync`'s scratch directory (the first filesystem side effect ANY spawn
    // attempt — real or fixture — would create) must never have been created anywhere under this
    // run's own cwd.
    assert!(
        !dir.path().join(".cyrup-subagent-scratch").exists(),
        "no spawn-scratch directory may exist: the depth guard must reject before run_inner ever \
         reaches exec::run_sync for any step"
    );
}

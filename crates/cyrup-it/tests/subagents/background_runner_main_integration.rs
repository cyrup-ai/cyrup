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

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::Mutex;

use cyrup_core::ModelId;
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::control::{ChainAppendRequest, InterruptRequest};
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, run};
use cyrup_ext_subagents::background::{RunId, RunMode, RunPaths, RunState, RunStatus, ResultFile};
use cyrup_ext_subagents::discovery::types::SystemPromptMode;
use cyrup_ext_subagents::exec::ResolvedAgentPersona;
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};

/// A minimal resolved persona for a fixture-driven test (T0.1 / C13): a real model so
/// `build_model_candidates` yields a non-empty ladder (the scripted fixture ignores `--model`
/// anyway), the completion-mutation guard disabled to isolate these ordering/lifecycle tests from
/// R-SA-034, and `Replace` system-prompt mode. Every step's agent MUST have a persona in the
/// run's `resolved_agents` map now that the runner dispatches the REAL named persona and rejects
/// an unresolved agent as `Unknown agent` rather than synthesizing a placeholder.
fn fixture_persona(name: &str) -> ResolvedAgentPersona {
    ResolvedAgentPersona {
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
        completion_guard: Some(false),
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
    }
}

/// The plan-time `resolved_agents` map covering every agent name any step in this file dispatches
/// (including the mid-run `appended` step consumed from disk) — mirroring the orchestrator's own
/// eager, up-front persona resolution.
fn all_personas() -> BTreeMap<String, ResolvedAgentPersona> {
    ["worker", "first", "only", "appended"]
        .into_iter()
        .map(|name| (name.to_string(), fixture_persona(name)))
        .collect()
}

/// Serializes every test in this file that mutates `CYRUP_SUBAGENT_BINARY`/
/// `CYRUP_SUBAGENT_FIXTURE_SCRIPT` (process-global state) — `cargo test` runs a test binary's own
/// `#[test]` functions concurrently by default, so without this lock two tests in this file could
/// observe or clobber each other's override value mid-run. Mirrors
/// `background_spawn_detached_integration.rs`'s/`exec_run_sync_integration.rs`'s identical
/// `ENV_MUTATION_LOCK` convention.
static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

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
        skills: None,
        session_dir: None,
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
        output_path: None,
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
        turn_budget: None,
        // SUBA-021 — pi's `usageBudget` is an OPTIONAL param (`extension/schemas.ts:330`) with no
        // upstream default: a run that does not ask for a budget runs unbudgeted. This fixture asks
        // for none, so `None` is what keeps every assertion below measuring what it measured before
        // the field existed (and `skip_serializing_if` keeps the on-disk config byte-identical).
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: RunId::from_token("happyrun1"),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do the thing"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        // C7: carry the orchestrator's absolute roots so `run` rebuilds RunPaths from THESE (they
        // equal the async/results dirs this test creates), never re-derives them.
        async_root: dir.path().join("async"),
        results_dir: dir.path().join("results"),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
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

    // T4 workflow-graph snapshot: `status.json` must carry a `workflowGraph` whose single node is
    // the `step-0` node for this one-step run (pi `buildWorkflowGraphSnapshot`, `workflow-graph.ts`).
    let graph = status
        .telemetry
        .workflow_graph
        .as_ref()
        .expect("status.json must carry a workflowGraph snapshot");
    assert_eq!(graph.nodes.len(), 1, "one-step run has one graph node");
    assert_eq!(graph.nodes[0].id, "step-0", "the node id is the pi `step-<N>` shape");
    assert_eq!(graph.nodes[0].agent.as_deref(), Some("worker"));
}

/// C7 regression (root-cause T0.4): the detached runner must write its terminal `ResultFile` into
/// the SAME results dir the ORCHESTRATOR derived and created — never a re-derived, divergent one.
/// Before the fix, the orchestrator's results dir was `<temp_root_dir>/results/<cwd_key>` while the
/// runner re-derived `<temp_root_dir>/async/results` from the config-file path's own structure, so
/// every real background run's result write targeted a directory that never existed and the run
/// appeared to hang forever.
///
/// This drives the REAL `run()` against the REAL fixture subprocess with the exact production
/// topology: the orchestrator's two sibling roots come from the shared
/// [`cyrup_ext_subagents::background::run_artifact_roots`] derivation (keyed off `CYRUP_HOME` for
/// hermeticity), the config carries those ABSOLUTE roots, and — to prove the runner HONORS the
/// config rather than a path re-derivation — `run()` is deliberately handed a provisional
/// `RunPaths` whose result path points at the OLD, buggy, never-created location. The run's
/// `status.json` AND `ResultFile` must both be found in the ORCHESTRATOR's dirs, and NOTHING must
/// land at the divergent location.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn result_file_lands_in_the_orchestrator_results_dir_not_a_re_derived_one() {
    let home = tempfile::tempdir().expect("real tempdir");

    let _guard = ENV_MUTATION_LOCK.lock().await;
    // SAFETY: mutex-serialized, scoped env mutation — same convention as the fixture-binary
    // override below. `CYRUP_HOME` makes the shared `run_artifact_roots` derivation hermetic.
    unsafe {
        std::env::set_var("CYRUP_HOME", home.path());
    }

    let cwd = home.path().join("project");
    tokio::fs::create_dir_all(&cwd).await.expect("mkdir cwd");

    // Orchestrator side: derive the two sibling roots from the ONE shared source of truth and
    // create them (ensureAccessibleDir-equivalent), exactly as `spawn_background_steps` does.
    let roots = cyrup_ext_subagents::background::run_artifact_roots(&cwd);
    cyrup_ext_subagents::background::ensure_accessible_dir(&roots.async_root)
        .await
        .expect("orchestrator creates async_root");
    cyrup_ext_subagents::background::ensure_accessible_dir(&roots.results_dir)
        .await
        .expect("orchestrator creates results_dir");

    let run_id = RunId::from_token("c7run0001");
    let orchestrator_paths = RunPaths::for_run(&roots.async_root, &roots.results_dir, &run_id);
    tokio::fs::create_dir_all(&orchestrator_paths.run_dir)
        .await
        .expect("orchestrator creates run_dir");

    // The config carries the orchestrator's ABSOLUTE roots — the T0.4 fix.
    let config = RunnerConfig {
        turn_budget: None,
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do the thing"))],
        cwd: cwd.clone(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: roots.async_root.clone(),
        results_dir: roots.results_dir.clone(),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
};
    let cfg_path = orchestrator_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config)
        .await
        .expect("write runner config");

    // Deliberately hand run() a PROVISIONAL RunPaths whose result path points at the OLD, buggy,
    // never-created location (`<temp_root_dir>/async/results/...`) — exactly what the pre-fix
    // derivation computed. The C7 fix means run() ignores this for its terminal writes and honors
    // the config's absolute roots instead.
    let buggy_results_dir = roots
        .async_root
        .parent()
        .expect("async_root has a parent")
        .join("results");
    let provisional = RunPaths::for_run(&roots.async_root, &buggy_results_dir, &run_id);
    assert_ne!(
        provisional.result, orchestrator_paths.result,
        "the buggy provisional result path must differ from the orchestrator's, else this test \
         proves nothing"
    );

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line("all done")},
        ],
        "exit_code": 0
    });
    let script_path = write_script(home.path(), "script.json", &script);
    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation (the lock is already held above).
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let result = run(&cfg_path, &provisional).await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
        std::env::remove_var("CYRUP_HOME");
    }
    result.expect("run() itself never returns Err");

    // BOTH the status.json and the terminal ResultFile must be found in the ORCHESTRATOR's dirs.
    assert!(
        tokio::fs::try_exists(&orchestrator_paths.status)
            .await
            .unwrap_or(false),
        "status.json must be written into the orchestrator's run dir: {:?}",
        orchestrator_paths.status
    );
    assert!(
        tokio::fs::try_exists(&orchestrator_paths.result)
            .await
            .unwrap_or(false),
        "the terminal ResultFile must land in the orchestrator's results dir (the whole point of \
         C7): {:?}",
        orchestrator_paths.result
    );

    // And NOTHING may land at the divergent, re-derived location.
    assert!(
        !tokio::fs::try_exists(&provisional.result)
            .await
            .unwrap_or(false),
        "the ResultFile must NOT be written to the divergent re-derived results dir (C7): {:?}",
        provisional.result
    );

    // Ordering + consistency: status.json THEN ResultFile, both terminal and mutually consistent.
    let status: RunStatus = serde_json::from_slice(
        &tokio::fs::read(&orchestrator_paths.status)
            .await
            .expect("status.json exists"),
    )
    .expect("parse status.json");
    let result_file: ResultFile = serde_json::from_slice(
        &tokio::fs::read(&orchestrator_paths.result)
            .await
            .expect("ResultFile exists"),
    )
    .expect("parse ResultFile");
    assert_eq!(status.state, RunState::Complete);
    assert_eq!(result_file.state, RunState::Complete);
    assert!(result_file.success);
    assert_eq!(
        result_file.run_id, run_id,
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
        turn_budget: None,
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: RunId::from_token("eventsrun1"),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do the thing"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        // C7: carry the orchestrator's absolute roots so `run` rebuilds RunPaths from THESE (they
        // equal the async/results dirs this test creates), never re-derives them.
        async_root: dir.path().join("async"),
        results_dir: dir.path().join("results"),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
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

    // events.jsonl field parity with pi (`subagent-runner.ts`): the field name is `type` (NOT
    // `kind`) and the event-type strings are `subagent.*`, with step events carrying `agent`.
    let mut types = Vec::new();
    for line in &lines {
        let parsed: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("every line must be valid JSON: {e}: {line}"));
        types.push(
            parsed["type"]
                .as_str()
                .unwrap_or_else(|| panic!("every line must carry a string 'type' field: {line}"))
                .to_string(),
        );
        assert!(parsed["ts"].is_number(), "every line must carry a numeric 'ts' field: {line}");
    }

    assert_eq!(types.first().map(String::as_str), Some("subagent.run.started"));
    assert!(types.contains(&"subagent.step.started".to_string()));
    assert!(types.contains(&"subagent.step.completed".to_string()));
    assert_eq!(types.last().map(String::as_str), Some("subagent.run.completed"));

    // Step events carry `agent`; the completion event carries `exitCode`/`durationMs` (pi parity).
    let step_completed = lines
        .iter()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("valid JSON"))
        .find(|v| v["type"] == "subagent.step.completed")
        .expect("a subagent.step.completed event must be present");
    assert_eq!(step_completed["agent"], "worker");
    assert_eq!(step_completed["exitCode"], 0);
    assert!(step_completed["durationMs"].is_number());
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
        turn_budget: None,
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: RunId::from_token("failrun001"),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do the thing"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        // C7: carry the orchestrator's absolute roots so `run` rebuilds RunPaths from THESE (they
        // equal the async/results dirs this test creates), never re-derives them.
        async_root: dir.path().join("async"),
        results_dir: dir.path().join("results"),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
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
        turn_budget: None,
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Chain,
        steps: vec![RunnerStep::SingleStep(single_step("first", "first task"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        // C7: carry the orchestrator's absolute roots so `run` rebuilds RunPaths from THESE (they
        // equal the async/results dirs this test creates), never re-derives them.
        async_root: dir.path().join("async"),
        results_dir: dir.path().join("results"),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
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
        turn_budget: None,
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("only", "only task"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        // C7: carry the orchestrator's absolute roots so `run` rebuilds RunPaths from THESE (they
        // equal the async/results dirs this test creates), never re-derives them.
        async_root: dir.path().join("async"),
        results_dir: dir.path().join("results"),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
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
        turn_budget: None,
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do something"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        // current_depth (0, absent from this test's own real env) >= max_depth (0): blocked.
        max_subagent_depth: 0,
        // C7: carry the orchestrator's absolute roots (equal to this test's async/results dirs).
        async_root: dir.path().join("async"),
        results_dir: dir.path().join("results"),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
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

/// A `tool_execution_start` NDJSON line (pi's live-tool wire event) naming `tool`, a call id, and a
/// path argument — the shape [`cyrup_ext_subagents`]'s `exec::ndjson::parse_line` folds into a
/// step's live `currentTool`/`currentPath` telemetry.
fn tool_start_line(tool: &str, call_id: &str) -> String {
    serde_json::json!({
        "type": "tool_execution_start",
        "toolCallId": call_id,
        "toolName": tool,
        "args": {"path": "/tmp/fixture-target.txt"}
    })
    .to_string()
}

/// T4 live telemetry (`subagent-runner.ts:1430-1517`): while a step's child is mid-flight holding a
/// tool call open, a CONCURRENT reader of `status.json` must observe the live `currentTool` —
/// proving the runner's per-event/1s-timer telemetry pump folds child NDJSON events into
/// `status.json` DURING the run, not merely at completion. Drives the REAL `run()` against the REAL
/// fixture subprocess (no mocking): the fixture emits a `tool_execution_start` then sleeps 800ms
/// holding that tool open, and this test polls `status.json` throughout that window.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_json_carries_live_current_tool_during_a_run() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": tool_start_line("grep_search", "call-1")},
            {"kind": "sleep_ms", "ms": 800},
            {"kind": "emit", "line": message_end_line("all done")},
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
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let run_id = RunId::from_token("livetool01");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    let config = RunnerConfig {
        turn_budget: None,
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "grep the code"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: async_root.clone(),
        results_dir: results_dir.clone(),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
};
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write runner config");

    // Drive `run()` concurrently with a poller of status.json.
    let status_path = run_paths.status.clone();
    let run_handle = {
        let cfg_path = cfg_path.clone();
        let run_paths = run_paths.clone();
        tokio::spawn(async move { run(&cfg_path, &run_paths).await })
    };

    let mut saw_current_tool = false;
    for _ in 0..160 {
        if let Ok(bytes) = tokio::fs::read(&status_path).await
            && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
        {
            let top = v.get("currentTool").and_then(|x| x.as_str());
            let step0 = v
                .get("steps")
                .and_then(|s| s.get(0))
                .and_then(|s| s.get("currentTool"))
                .and_then(|x| x.as_str());
            if top == Some("grep_search") || step0 == Some("grep_search") {
                saw_current_tool = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let outcome = run_handle.await.expect("run task joins");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }
    outcome.expect("run() itself never returns Err");

    assert!(
        saw_current_tool,
        "a concurrent status.json reader must observe the live `currentTool` while the child holds \
         a tool call open mid-run — proving the telemetry pump folds child events into status.json \
         DURING the run, not just at completion"
    );
}

/// T4 mid-flight interrupt (`subagent-runner.ts:1333,2002-2005,2069` @v0.34.0): interrupting a SINGLE-step run
/// must actually reach and tear down the in-flight child — not be a no-op that only takes effect
/// BETWEEN steps (a single-step run has no next step). Drives the REAL `run()` against the REAL
/// fixture subprocess: the fixture sleeps 6000ms, and this test drops a control-inbox interrupt
/// request ~400ms in. The run MUST finish FAR sooner than the child's 6s sleep (proving the child was
/// signalled and torn down, not waited out), reach `Paused`, and record the step as interrupted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupting_a_single_step_run_actually_signals_the_mid_flight_child() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "sleep_ms", "ms": 6000},
            {"kind": "emit", "line": message_end_line("SHOULD-NOT-REACH")},
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
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let run_id = RunId::from_token("midintrpt1");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    let config = RunnerConfig {
        turn_budget: None,
        usage_budget: None,
        // SUBA-N03: this fixture exercises neither the run-level timeout nor `share`/artifacts, so it
        // carries the same values an older on-disk config deserializes to (`#[serde(default)]`).
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("only", "sleep a long time"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: async_root.clone(),
        results_dir: results_dir.clone(),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
    nested_route: None,
    nested_self: None,
    dynamic_fanout_max_items: None,
    // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
    model_scope: None,
    control: None,
    include_progress: None,
};
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write config");

    // Deliver the interrupt ~400ms into the child's 6s sleep — squarely mid-flight.
    let run_paths_for_interrupt = run_paths.clone();
    let interrupt_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        tokio::fs::create_dir_all(
            run_paths_for_interrupt
                .control_inbox
                .parent()
                .expect("control_inbox has a parent"),
        )
        .await
        .expect("mkdir control dir");
        let request = InterruptRequest::new("mid-flight-test", Some("stop the child".to_string()));
        write_atomic_json(&run_paths_for_interrupt.control_inbox, &request)
            .await
            .expect("write interrupt request");
    });

    let started = std::time::Instant::now();
    let outcome = run(&cfg_path, &run_paths).await;
    let elapsed = started.elapsed();
    interrupt_task.await.expect("interrupt task completes");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }
    outcome.expect("run() itself never returns Err");

    assert!(
        elapsed < Duration::from_millis(4000),
        "the run must finish FAR sooner than the child's 6s sleep — proving the mid-flight child was \
         actually signalled and torn down, not waited out: elapsed={elapsed:?}"
    );

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
        RunState::Paused,
        "an interrupted single-step run must end Paused (R-SA-084 soft pause), not Complete: {status:?}"
    );
    assert_eq!(result_file.state, RunState::Paused);
    assert!(!result_file.success);
    assert_eq!(result_file.results.len(), 1);
    assert!(
        result_file.results[0].interrupted,
        "the interrupted step's SingleResult must carry interrupted=true: {:?}",
        result_file.results[0]
    );
    assert_ne!(
        result_file.results[0].final_output.as_deref(),
        Some("SHOULD-NOT-REACH"),
        "the child's post-sleep emit must never have run — it was torn down mid-sleep"
    );
}

/// SUBA-N05, the hop-2 half: [`RunnerConfig::control`] must reach every dispatched step's
/// `exec::RunOptions::control_config`, so an ASYNC run's `control` override genuinely changes the
/// thresholds the child's stream is judged against — and the raised events travel back to the
/// orchestrator on the terminal `ResultFile`.
///
/// Upstream: the async runner reads `const controlConfig = config.controlConfig ??
/// DEFAULT_CONTROL_CONFIG` (`subagent-runner.ts:1802` @v0.34.0) and drives its per-child attention
/// tracking from it. Before this, cyrup's `ExecSingleStepExecutor::run_single` hard-coded
/// `control_config: None`, so a background run could only ever use the stock 60 s window no matter
/// what the caller (or `subagents.control`) said.
///
/// Both directions are asserted in ONE run pair against the SAME child script, so the only
/// independent variable is the config: with `needsAttentionAfterMs: 1` the child's ~1.8 s silent
/// window raises `needs_attention`; with the config absent (pi's `?? DEFAULT_CONTROL_CONFIG`) the
/// identical child is far inside the stock 60 s window and raises nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runner_config_control_reaches_every_step_and_raises_real_events() {
    // A child that says nothing for longer than one 1 s activity tick, then answers cleanly.
    let script = serde_json::json!({
        "steps": [
            {"kind": "sleep_ms", "ms": 1_800},
            {"kind": "emit", "line": message_end_line("done after a long silence")},
        ],
        "exit_code": 0
    });

    fn config_for(dir: &Path, token: &str, control: Option<cyrup_ext_subagents::exec::control::ResolvedControlConfig>) -> RunnerConfig {
        RunnerConfig {
            turn_budget: None,
            usage_budget: None,
            timeout_ms: None,
            deadline_at_ms: None,
            share: None,
            artifacts_dir: None,
            artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
            run_id: RunId::from_token(token),
            mode: RunMode::Single,
            steps: vec![RunnerStep::SingleStep(single_step("worker", "idle a while"))],
            cwd: dir.to_path_buf(),
            session_file: None,
            session_id: None,
            global_concurrency_limit: 20,
            worktree_base_dir: None,
            max_subagent_depth: 2,
            async_root: dir.join("async"),
            results_dir: dir.join("results"),
            resolved_agents: all_personas(),
            original_task: String::new(),
            chain_dir: None,
            orchestrator_intercom_target: None,
            inherited_session_model: None,
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
            model_scope: None,
            control,
            include_progress: None,
        }
    }

    let armed_dir = tempfile::tempdir().expect("real tempdir");
    let (_status, armed) = run_against_fixture(
        armed_dir.path(),
        &script,
        config_for(
            armed_dir.path(),
            "ctrlarmed",
            Some(cyrup_ext_subagents::exec::control::ResolvedControlConfig {
                needs_attention_after_ms: 1,
                ..cyrup_ext_subagents::exec::control::ResolvedControlConfig::default()
            }),
        ),
    )
    .await;

    assert_eq!(armed.results.len(), 1);
    let events = &armed.results[0].control_events;
    assert!(
        events.iter().any(|e| {
            e.event_type == cyrup_ext_subagents::registration::ControlEventType::NeedsAttention
        }),
        "RunnerConfig::control must reach the step's RunOptions and make the idle window trip; \
         got {events:?}"
    );
    assert_eq!(
        events[0].agent, "worker",
        "the event must be attributed to the step's real persona"
    );

    // The discriminator: same child, no config => pi's `?? DEFAULT_CONTROL_CONFIG` (60 s window).
    let stock_dir = tempfile::tempdir().expect("real tempdir");
    let (_status, stock) =
        run_against_fixture(stock_dir.path(), &script, config_for(stock_dir.path(), "ctrlstock", None))
            .await;
    assert!(
        stock.results[0].control_events.is_empty(),
        "with no control config the stock 60s attention window applies, so the same ~1.8s idle \
         must raise nothing; got {:?}",
        stock.results[0].control_events
    );
}

/// SUBA-N03, the hop-2 half, end to end: the detached runner must actually CONSUME the new
/// `RunnerConfig`/`SingleStepSpec` fields — write the artifact quadruple, create the child's
/// session directory, and pass `--session-dir` on the child's real argv.
///
/// The orchestrator-side tests (`extension::tests::a_background_single_run_honours_the_nine_*`)
/// prove these values reach `runner-config.json`. That is only half the contract: a value that
/// lands in the handoff file and is then ignored by the runner is still an
/// advertised-and-silently-dropped param, which is the exact defect SUBA-041 names. This drives
/// the REAL `run()` against the REAL fixture subprocess and asserts on real on-disk side effects.
///
/// Upstream equivalents @v0.34.0: the artifact quadruple is written by the async runner itself
/// (`runs/background/subagent-runner.ts:877-889` writes `_input.md` BEFORE the child spawns, then
/// `:1117-1133` writes `_output.md`/`_meta.json` after), and `sessionDir` becomes the child's
/// `--session-dir` after an `fs.mkdirSync` (`runs/shared/pi-args.ts:109-111`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_runner_writes_the_artifact_quadruple_and_honours_session_dir_and_share() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let artifacts_dir = dir.path().join("arts");
    let session_dir = dir.path().join("sessions").join("run-0");

    let script = serde_json::json!({
        // Echo the child's real argv into its stdout stream, which `exec::run_sync` tees to
        // `<cwd>/.cyrup-subagent-scratch/attempt-0.jsonl` (R-SA-058) — the crate's own standing
        // observation channel for "what argv did the child actually receive".
        "echo_argv": true,
        "steps": [
            {"kind": "emit", "line": message_end_line("delivered answer")},
        ],
        "exit_code": 0
    });

    let mut step = single_step("worker", "do the thing");
    step.session_dir = Some(session_dir.clone());
    step.skills = Some(Vec::new());

    let config = RunnerConfig {
        turn_budget: None,
        usage_budget: None,
        // The three fields under test, plus the timeout pair left at its default.
        timeout_ms: None,
        deadline_at_ms: None,
        share: Some(true),
        artifacts_dir: Some(artifacts_dir.clone()),
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::foreground(),
        run_id: RunId::from_token("artifactsrun1"),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(step)],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: dir.path().join("async"),
        results_dir: dir.path().join("results"),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        model_scope: None,
        control: None,
        include_progress: None,
    };

    let (_status, result_file) = run_against_fixture(dir.path(), &script, config).await;
    assert!(result_file.success, "the fixture run must succeed: {result_file:?}");

    // --- the artifact quadruple (pi `getArtifactPaths(ctx.artifactsDir, ctx.id, step.agent, index)`)
    let paths = cyrup_ext_subagents::artifacts::artifact_paths(
        &artifacts_dir,
        "artifactsrun1",
        "worker",
        Some(0),
    );
    assert!(
        paths.input_path.exists(),
        "the hop-2 runner must write `_input.md` BEFORE the child spawns (crash safety), at {}",
        paths.input_path.display()
    );
    let input = std::fs::read_to_string(&paths.input_path).expect("read _input.md");
    assert!(
        input.contains("# Task for worker") && input.contains("do the thing"),
        "the input artifact carries pi's `# Task for <agent>\\n\\n<task>` body: {input}"
    );
    assert!(paths.output_path.exists(), "`_output.md` must be written after the run");
    assert_eq!(
        std::fs::read_to_string(&paths.output_path).expect("read _output.md"),
        "delivered answer",
        "the output artifact carries the child's delivered answer"
    );
    assert!(paths.metadata_path.exists(), "`_meta.json` must be written after the run");
    let meta: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&paths.metadata_path).expect("read _meta.json"),
    )
    .expect("_meta.json parses");
    assert_eq!(
        meta["runId"], "artifactsrun1",
        "the metadata shape is the SAME one the foreground path emits: {meta}"
    );
    assert_eq!(meta["agent"], "worker");
    assert_eq!(meta["exitCode"], 0);
    assert!(paths.jsonl_path.exists(), "`.jsonl` must be written (foreground() enables it)");

    // --- sessionDir: created up front and handed to the child as real argv.
    assert!(
        session_dir.exists(),
        "the step's session directory must be created before the child spawns (pi's `mkdirSync` \
         ahead of `--session-dir`), at {}",
        session_dir.display()
    );
    let tee = std::fs::read_to_string(dir.path().join(".cyrup-subagent-scratch/attempt-0.jsonl"))
        .expect("the R-SA-058 raw-stdout tee must exist");
    assert!(
        tee.contains("--session-dir"),
        "the child's real argv must carry `--session-dir`: {tee}"
    );
    assert!(
        tee.contains(&session_dir.display().to_string()),
        "…pointing at THIS step's resolved directory: {tee}"
    );
    // `share: true` is one of the two `sessionEnabled` terms, and a session-enabled child must NOT
    // be spawned `--no-session` (pi `execution.ts:1027,1039` @v0.34.0).
    assert!(
        !tee.contains("--no-session"),
        "a run with `share: true` / an explicit sessionDir must not spawn the child \
         `--no-session`: {tee}"
    );
}

/// SUBA-N03: `artifacts: false` reaching hop 2 as an absent `artifacts_dir` must leave NO artifact
/// files behind — pi's own first gate term (`if (ctx.artifactsDir && …)`,
/// `runs/background/subagent-runner.ts:1192`). The complement of the test directly above; without
/// it, that one would still pass if the runner wrote artifacts unconditionally into some default
/// location.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_runner_writes_no_artifacts_when_the_run_disabled_them() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let artifacts_dir = dir.path().join("arts");
    let script = serde_json::json!({
        "steps": [{"kind": "emit", "line": message_end_line("done")}],
        "exit_code": 0
    });
    let config = RunnerConfig {
        turn_budget: None,
        usage_budget: None,
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        // Both halves of pi's two-term gate say "off", which is exactly what an explicit
        // `artifacts: false` produces on the orchestrator side.
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig {
            enabled: false,
            ..cyrup_ext_subagents::artifacts::ArtifactConfig::foreground()
        },
        run_id: RunId::from_token("noartifacts1"),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do the thing"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: dir.path().join("async"),
        results_dir: dir.path().join("results"),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        model_scope: None,
        control: None,
        include_progress: None,
    };

    let (_status, result_file) = run_against_fixture(dir.path(), &script, config).await;
    assert!(result_file.success);
    assert!(
        !artifacts_dir.exists(),
        "a run with artifacts disabled must leave no artifact directory at all"
    );
}

/// SUBA-N03: an ABSOLUTE `deadline_at_ms` in the one-shot config really arms the run's deadline in
/// the detached process — pi `const remainingMs = Math.max(0, config.deadlineAt - Date.now())`
/// (`runs/background/subagent-runner.ts:2078-2081` @v0.34.0).
///
/// Proven with an ALREADY-PASSED deadline, which is the case that distinguishes a real port from a
/// field that merely deserializes: `saturating_sub` must collapse it to "no time left" so the step
/// times out, rather than the subtraction wrapping into a far-future instant (which would look
/// exactly like the pre-SUBA-N03 "background runs have no timeout" behaviour and pass any test
/// using a generous future deadline). The fixture is scripted to sleep well past the budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_already_passed_deadline_in_the_config_times_the_run_out_rather_than_wrapping() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script = serde_json::json!({
        "steps": [
            {"kind": "sleep_ms", "ms": 30_000},
            {"kind": "emit", "line": message_end_line("should never be delivered")},
        ],
        "exit_code": 0
    });
    let config = RunnerConfig {
        turn_budget: None,
        usage_budget: None,
        timeout_ms: Some(1),
        // One hour in the PAST: `deadline_at_ms - now` underflows, and only a saturating
        // subtraction yields the correct "already expired" deadline.
        deadline_at_ms: Some(1),
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: RunId::from_token("expiredrun1"),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do the thing"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: dir.path().join("async"),
        results_dir: dir.path().join("results"),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        model_scope: None,
        control: None,
        include_progress: None,
    };

    let (_status, result_file) = run_against_fixture(dir.path(), &script, config).await;
    assert!(
        !result_file.success,
        "an expired deadline must fail the run, not be silently ignored: {result_file:?}"
    );
    let rendered = serde_json::to_string(&result_file).expect("serialize ResultFile");
    assert!(
        rendered.contains("timed out") || rendered.contains("timeout"),
        "the failure must be reported as a TIMEOUT (pi's `Subagent timed out after {{n}}ms.`), \
         not some other error: {rendered}"
    );
}

// =================================================================================================
// G77 — `stopped` as a first-class terminal state, driven through the REAL detached-runner loop
// =================================================================================================

/// G77, the live path: a `control/stop.json` delivered mid-flight must tear the child down and end
/// the run in the terminal [`RunState::Stopped`] state — **not** `Paused` (what an `interrupt`
/// produces) and **not** `Failed` (what a `timeout` produces).
///
/// Upstream: pi `stopRunner` (`runs/background/subagent-runner.ts:2955-2984` @v0.43.0) sets
/// `statusPayload.state = "stopped"`, `statusPayload.error = stopMessage`, marks every
/// `running`-or-`pending` step `"stopped"` with `error = stopMessage`, appends
/// `subagent.run.stopped` to `events.jsonl`, aborts the live children and stops the nested async
/// descendants. Every one of those observable effects is asserted below against the REAL files
/// `run()` itself wrote, with a REAL `cyrup-subagent-fixture` OS subprocess as the child.
///
/// The paired `interrupt` test directly above this one (`interrupting_a_single_step_run_actually_
/// signals_the_mid_flight_child`) is deliberately the same shape with the same 6 s child script, so
/// the ONLY independent variable between the two is which control file was written — which is
/// exactly the distinction G77 exists to make observable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_a_mid_flight_run_ends_it_stopped_not_paused_and_not_failed() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "sleep_ms", "ms": 6000},
            {"kind": "emit", "line": message_end_line("SHOULD-NOT-REACH")},
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
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let run_id = RunId::from_token("midstop001");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");

    let config = RunnerConfig {
        turn_budget: None,
        usage_budget: None,
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("only", "sleep a long time"))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: async_root.clone(),
        results_dir: results_dir.clone(),
        resolved_agents: all_personas(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        model_scope: None,
        control: None,
        include_progress: None,
    };
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write config");

    // Deliver the stop ~400ms into the child's 6s sleep — squarely mid-flight, through the REAL
    // parent-side `deliver_stop_request` primitive (never a hand-rolled file write), so this test
    // covers the writer as well as the runner's reader.
    let stop_dir = run_paths.run_dir.clone();
    let stop_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        cyrup_ext_subagents::background::control::deliver_stop_request(
            &stop_dir,
            "stop-action",
            None,
        )
        .await
        .expect("write stop request");
    });

    let started = std::time::Instant::now();
    let outcome = run(&cfg_path, &run_paths).await;
    let elapsed = started.elapsed();
    stop_task.await.expect("stop task completes");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }
    outcome.expect("run() itself never returns Err");

    assert!(
        elapsed < Duration::from_millis(4000),
        "the run must finish FAR sooner than the child's 6s sleep — proving the mid-flight child was \
         actually torn down by the stop, not waited out: elapsed={elapsed:?}"
    );

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
        RunState::Stopped,
        "a stopped run must end `Stopped` — NOT `Paused` (interrupt) and NOT `Failed` (timeout): {status:?}"
    );
    assert_eq!(result_file.state, RunState::Stopped);
    assert!(
        RunState::Stopped.is_terminal(),
        "`Stopped` must be terminal in its own right (pi `chain-root-attachment.ts:60` TERMINAL_STATES)"
    );
    assert!(!result_file.success);

    // pi `subagent-runner.ts:2966-2973`: every running-or-pending step becomes `"stopped"` with the
    // stop message, never `"failed"` and never `"paused"`.
    assert_eq!(status.steps.len(), 1);
    assert_eq!(
        status.steps[0].status,
        cyrup_ext_subagents::background::StepState::Stopped,
        "the mid-flight step must be marked Stopped: {:?}",
        status.steps[0]
    );
    assert_eq!(
        status.steps[0].error.as_deref(),
        Some(cyrup_ext_subagents::background::control::STOP_MESSAGE),
        "the stopped step carries pi's literal stop message"
    );

    // pi `subagent-runner.ts:1642,1722` @v0.43.0 — the child's own record is promoted to `stopped`, and
    // `:909`/`:915`/`:917` give it exitCode 1, the stop message as `error`, and the stop message as
    // `finalOutput` when it produced none of its own.
    assert_eq!(result_file.results.len(), 1);
    let child = &result_file.results[0];
    assert!(child.stopped, "the torn-down child's SingleResult must carry stopped=true: {child:?}");
    assert!(
        !child.interrupted,
        "a stop is NOT an interrupt — `interrupted` must be cleared so the run does not read as resumable: {child:?}"
    );
    assert_eq!(child.exit_code, 1);
    assert_eq!(
        child.error.as_deref(),
        Some(cyrup_ext_subagents::background::control::STOP_MESSAGE)
    );
    assert_ne!(
        child.final_output.as_deref(),
        Some("SHOULD-NOT-REACH"),
        "the child's post-sleep emit must never have run — it was torn down mid-sleep"
    );

    // pi `subagent-runner.ts:2977-2982`: the terminal event is `subagent.run.stopped`, not
    // `subagent.run.paused` and not `subagent.run.completed`.
    let events = tokio::fs::read_to_string(&run_paths.events)
        .await
        .expect("events.jsonl exists");
    let types: Vec<String> = events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .collect();
    assert!(
        types.iter().any(|t| t == "subagent.run.stopped"),
        "events.jsonl must carry `subagent.run.stopped`: {types:?}"
    );
    assert!(
        !types.iter().any(|t| t == "subagent.run.paused" || t == "subagent.run.completed"),
        "a stopped run must NOT also report itself paused or completed: {types:?}"
    );

    // The stop request is consumed exactly once (pi `consumeStopRequest`'s
    // read-then-unconditionally-delete discipline).
    assert!(
        !cyrup_ext_subagents::background::control::stop_request_path(&run_paths.run_dir).exists(),
        "the consumed stop request file must be gone"
    );

    // G77's downstream contract, asserted on the SAME real record rather than a hand-built one:
    // the grouped intercom payload reports `stopped`, the notify header word is `stopped`, and
    // `resume` refuses the run outright.
    let payload = cyrup_ext_subagents::tui::intercom::IntercomPayload::from_result(&result_file);
    assert_eq!(
        payload.status,
        cyrup_ext_subagents::tui::intercom::SubagentResultStatus::Stopped,
        "pi `resolveGroupedStatus` must report the whole grouped run as stopped: {payload:?}"
    );
    assert_eq!(payload.summary, "1 stopped");

    let notify = cyrup_ext_subagents::background::watch::format_completion_message(&result_file);
    assert!(
        notify.content.contains("Background task stopped:"),
        "pi `notify.ts:210`'s fourth status word must be rendered: {}",
        notify.content
    );

    let resume_err = cyrup_ext_subagents::background::control::resume(
        &async_root,
        &results_dir,
        run_id.as_str(),
        None,
    )
    .await
    .expect_err("resume must refuse a stopped run");
    assert_eq!(
        resume_err.to_string(),
        format!("Async run '{}' was stopped and cannot be resumed. Start a new run instead.", run_id.as_str()),
        "pi `async-resume.ts:406`'s verbatim refusal"
    );

    // …and the `status` action renders the stopped run's own state word plus pi's
    // not-resumable guidance rather than a `Revive:` line.
    let rendered = cyrup_ext_subagents::background::run_status::inspect_status_by_id(
        &async_root,
        &results_dir,
        run_id.as_str(),
    )
    .await
    .expect("status renders")
    .expect("status is present");
    assert!(
        rendered.contains("State: stopped"),
        "the status report must print `stopped`, not `failed`: {rendered}"
    );
    assert!(
        rendered.contains("Resume: unavailable; stopped runs are not resumable. Start a new run instead."),
        "pi `run-status.ts:52`'s verbatim stopped-resume guidance: {rendered}"
    );
    assert!(
        !rendered.contains("Revive:"),
        "a stopped run must never be offered a Revive line: {rendered}"
    );
}

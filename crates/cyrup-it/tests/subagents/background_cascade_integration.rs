//! Integration test: the ancestor→descendant control cascade and the `timeout` control-inbox verb,
//! driven through the REAL production entry point [`cyrup_ext_subagents::background::runner_main::run`].
//!
//! # What these prove
//!
//! **The interrupt cascade.** A background run is a detached OS process in its own process group,
//! and so is every background run IT spawns. Signalling cannot cross that boundary. Before the
//! cascade existed, interrupting a background run paused only that run's own steps and wrote only
//! its own `status.json` — every background descendant kept running, unreachable. These tests
//! register a live descendant in the nested-run registry exactly the way a real fan-out does, run
//! the real runner, and assert the descendant's OWN control inbox received the request file.
//!
//! **The timeout verb.** `control/timeout.json` (pi `TimeoutRequest`,
//! `src/runs/background/control-channel.ts:41,209,257` @v0.34.0) is the second control-inbox verb,
//! deliberately distinct from `interrupt.json`: an interrupt is a resumable `Paused`, a timeout is a
//! terminal `Failed`. These tests assert BOTH halves — that the runner consumes a delivered timeout
//! and ends `Failed`/timed-out rather than `Paused`, and that it cascades that timeout onward.
//!
//! # No mocking, and no subprocess either
//!
//! Every assertion below is against files the real `run()` wrote, on a real filesystem. Unlike this
//! crate's other `runner_main` integration tests, these do not need the scripted fixture binary at
//! all: a control request that is already pending when the runner starts is consumed at the very
//! top of the step loop, BEFORE any step is dispatched, so no child is ever spawned. That is also
//! what makes these tests fast and free of any timing race — the outcome does not depend on
//! winning a race against a live child.
//!
//! This file is a separate compilation unit from `cyrup-ext-subagents`'s own `lib.rs` (ordinary
//! Cargo integration-test placement), so it is NOT bound by that crate's `#![forbid(unsafe_code)]`;
//! the `unsafe` blocks below exist only because Rust 2024 requires `unsafe` for
//! `std::env::set_var`/`remove_var`, and they are serialized by [`ENV_MUTATION_LOCK`] because
//! `CYRUP_SUBAGENTS_TEMP_ROOT` is process-global state.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tokio::sync::Mutex;

use cyrup_core::ModelId;
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::control::{InterruptRequest, StopRequest, TimeoutRequest};
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, run};
use cyrup_ext_subagents::background::{ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus};
use cyrup_ext_subagents::discovery::types::SystemPromptMode;
use cyrup_ext_subagents::exec::ResolvedAgentPersona;
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};
use cyrup_ext_subagents::spawn::nested_events::{
    NestedEventInput, NestedRoute, NestedRunSummary, TEMP_ROOT_ENV, create_nested_route,
    write_nested_event,
};

/// `CYRUP_SUBAGENTS_TEMP_ROOT` is process-global and every nested-events path derives from it, so
/// the two tests in this file must not overlap. Held for each test's whole body — including the
/// `run()` call itself, which resolves the nested-runs root from this same variable while it
/// cascades. Mirrors the `ENV_MUTATION_LOCK` convention every other integration test in this crate
/// uses.
static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

fn persona(name: &str) -> ResolvedAgentPersona {
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

/// Register one live background descendant in `route`'s nested-run registry, exactly the way a
/// real fan-out child does (a `subagent.nested.started`-shaped event carrying its own summary), and
/// create its run directory. Returns the descendant's async dir — the directory whose `control/`
/// inbox the cascade must reach.
fn register_live_descendant(route: &NestedRoute, temp_root: &Path, child_id: &str) -> PathBuf {
    let async_dir = temp_root
        .join("nested-subagent-runs")
        .join(&route.root_run_id)
        .join(child_id);
    std::fs::create_dir_all(&async_dir).expect("create the descendant's run dir");

    let mut child = NestedRunSummary {
        id: child_id.to_string(),
        parent_run_id: route.root_run_id.clone(),
        parent_step_index: Some(0),
        parent_agent: None,
        depth: 1,
        path: Vec::new(),
        async_dir: Some(async_dir.display().to_string()),
        pid: None,
        session_id: None,
        session_file: None,
        intercom_target: None,
        owner_intercom_target: None,
        leaf_intercom_target: None,
        owner_state: None,
        control_inbox: None,
        capability_token: None,
        mode: Some("single".to_string()),
        // The state that makes it a cascade target at all.
        state: "running".to_string(),
        agent: Some("worker".to_string()),
        agents: None,
        current_step: None,
        chain_step_count: None,
        activity_state: None,
        last_activity_at: None,
        current_tool: None,
        current_tool_started_at: None,
        current_path: None,
        turn_count: None,
        tool_count: None,
        total_tokens: None,
        total_cost: None,
        started_at: None,
        ended_at: None,
        last_update: None,
        error: None,
        steps: None,
        children: None,
    };
    // Deliberately left absent: a descendant whose pid is unknown must still be reached through the
    // file inbox. The signal is only ever a latency optimization.
    child.pid = None;

    write_nested_event(
        route,
        &NestedEventInput {
            event_type: "subagent.nested.started".to_string(),
            ts: 1,
            parent_run_id: route.root_run_id.clone(),
            parent_step_index: Some(0),
            child,
        },
    )
    .expect("write the nested-started event");

    async_dir
}

struct Harness {
    run_paths: RunPaths,
    descendant_dir: PathBuf,
    config_path: PathBuf,
}

/// Build a one-step background run that owns a nested route with one live descendant, and write its
/// one-shot config to disk. Nothing is started yet — the caller plants a control request first.
async fn build_run(dir: &Path, temp_root: &Path, run_token: &str, child_id: &str) -> Harness {
    let route = create_nested_route(run_token).expect("mint a nested route");
    let descendant_dir = register_live_descendant(&route, temp_root, child_id);

    let async_root = dir.join("async");
    let results_dir = dir.join("results");
    tokio::fs::create_dir_all(&async_root).await.unwrap();
    tokio::fs::create_dir_all(&results_dir).await.unwrap();

    let run_id = RunId::from_token(run_token);
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.unwrap();

    let config = RunnerConfig {
        turn_budget: None,
        // SUBA-021 — pi's `usageBudget` is an OPTIONAL param (`extension/schemas.ts:330`) with no
        // upstream default: a run that does not ask for a budget runs unbudgeted. This fixture asks
        // for none, so `None` is what keeps every assertion below measuring what it measured before
        // the field existed (and `skip_serializing_if` keeps the on-disk config byte-identical).
        usage_budget: None,
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id,
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", "do the thing"))],
        cwd: dir.to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root,
        results_dir,
        resolved_agents: BTreeMap::from([("worker".to_string(), persona("worker"))]),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        // The whole point: this run owns descendants, so the cascade has somewhere to go.
        nested_route: Some(route),
        nested_self: None,
        dynamic_fanout_max_items: None,
        model_scope: None,
        control: None,
        include_progress: None,
    };

    let config_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&config_path, &config)
        .await
        .expect("write the one-shot runner config");

    Harness {
        run_paths,
        descendant_dir,
        config_path,
    }
}

async fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let bytes = tokio::fs::read(path)
        .await
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// [49] — interrupting a background run must reach every live background descendant, not just the
/// run's own steps.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupting_a_background_run_cascades_to_its_live_async_descendants() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");
    let temp_root = dir.path().join("subagents-temp");
    std::fs::create_dir_all(&temp_root).unwrap();
    // SAFETY: mutex-serialized, scoped env mutation — the standing convention in this crate's
    // integration tests. Held for the whole test body because `run()` reads it while cascading.
    unsafe {
        std::env::set_var(TEMP_ROOT_ENV, &temp_root);
    }

    let harness = build_run(dir.path(), &temp_root, "cascadeintr1", "childintr1").await;

    // The interrupt a caller's `control::interrupt()` would have written, already pending when the
    // runner starts — the runner's mandatory synchronous startup check picks it up and the step
    // loop consumes it on its first iteration, before any step is dispatched.
    std::fs::create_dir_all(harness.run_paths.control_inbox.parent().unwrap()).unwrap();
    write_atomic_json(
        &harness.run_paths.control_inbox,
        &InterruptRequest::new("user", None),
    )
    .await
    .expect("plant the interrupt request");

    run(&harness.config_path, &harness.run_paths)
        .await
        .expect("run() never returns Err");

    // SAFETY: scoped cleanup inside the same mutex-held critical section.
    unsafe {
        std::env::remove_var(TEMP_ROOT_ENV);
    }

    // The run itself paused — the pre-existing half of the behavior, asserted so a regression that
    // broke it while keeping the cascade would still be caught.
    let result: ResultFile = read_json(&harness.run_paths.result).await;
    assert_eq!(result.state, RunState::Paused);

    // The half this test exists for: the DESCENDANT's own control inbox.
    let descendant_inbox = harness.descendant_dir.join("control").join("interrupt.json");
    assert!(
        descendant_inbox.exists(),
        "the live async descendant at {} received no interrupt — it would keep running forever \
         after its ancestor was stopped",
        harness.descendant_dir.display()
    );
    let delivered: InterruptRequest = read_json(&descendant_inbox).await;
    assert_eq!(delivered.kind, "interrupt");
    assert_eq!(
        delivered.source, "ancestor-interrupt",
        "pi stamps this exact source so a descendant can tell an ancestor cascade from a direct \
         user interrupt"
    );
}

/// [48] — a delivered `control/timeout.json` must be consumed as a TERMINAL deadline failure (not a
/// resumable pause), and must cascade onward to this run's own live descendants.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivered_timeout_request_fails_the_run_and_cascades_to_descendants() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");
    let temp_root = dir.path().join("subagents-temp");
    std::fs::create_dir_all(&temp_root).unwrap();
    // SAFETY: see the sibling test above.
    unsafe {
        std::env::set_var(TEMP_ROOT_ENV, &temp_root);
    }

    let harness = build_run(dir.path(), &temp_root, "cascadetmo1", "childtmo1").await;

    // What an ancestor whose own deadline expired drops into this run's inbox.
    let inbox_dir = harness.run_paths.control_inbox.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&inbox_dir).unwrap();
    let timeout_path = inbox_dir.join("timeout.json");
    write_atomic_json(
        &timeout_path,
        &TimeoutRequest::new("ancestor-timeout", None),
    )
    .await
    .expect("plant the timeout request");

    run(&harness.config_path, &harness.run_paths)
        .await
        .expect("run() never returns Err");

    // SAFETY: scoped cleanup inside the same mutex-held critical section.
    unsafe {
        std::env::remove_var(TEMP_ROOT_ENV);
    }

    // 1. The verb was actually consumed (at-most-once: the file is gone).
    assert!(
        !timeout_path.exists(),
        "the timeout request was never consumed — it is still sitting in the inbox"
    );

    // 2. Terminal FAILURE, not a resumable pause. This is the distinction the whole second verb
    //    exists for: `Paused` here would mean an expired deadline left a run something could
    //    legitimately resume.
    let result: ResultFile = read_json(&harness.run_paths.result).await;
    assert_eq!(
        result.state,
        RunState::Failed,
        "a timeout is terminal failure, never a resumable Paused"
    );
    assert!(!result.success);
    let error = result
        .results
        .first()
        .and_then(|r| r.error.clone())
        .unwrap_or_default();
    assert!(
        error.to_lowercase().contains("timed out"),
        "the terminal record must name the timeout; got {error:?}"
    );
    assert!(
        !result.results.iter().any(|r| r.interrupted),
        "a timed-out run must not be recorded as interrupted (that would read as resumable)"
    );

    // The never-dispatched step is FAILED with the timeout message, not left Pending or Paused.
    let status: RunStatus = read_json(&harness.run_paths.status).await;
    assert!(
        status
            .steps
            .iter()
            .all(|s| s.status == cyrup_ext_subagents::background::StepState::Failed),
        "every unfinished step must be failed by the timeout; got {:?}",
        status.steps.iter().map(|s| s.status).collect::<Vec<_>>()
    );

    // 3. pi's `subagent.run.timed_out` lifecycle event.
    let events = tokio::fs::read_to_string(&harness.run_paths.events)
        .await
        .expect("events.jsonl exists");
    assert!(
        events.contains("subagent.run.timed_out"),
        "no subagent.run.timed_out event in:\n{events}"
    );

    // 4. And the cascade: the descendant gets a TIMEOUT (not an interrupt), so its own subtree
    //    fails too rather than sitting paused-but-unresumable.
    let descendant_inbox = harness.descendant_dir.join("control").join("timeout.json");
    assert!(
        descendant_inbox.exists(),
        "the live async descendant at {} received no timeout",
        harness.descendant_dir.display()
    );
    let delivered: TimeoutRequest = read_json(&descendant_inbox).await;
    assert_eq!(delivered.kind, "timeout");
    assert_eq!(delivered.source, "ancestor-timeout");
    assert!(
        !harness
            .descendant_dir
            .join("control")
            .join("interrupt.json")
            .exists(),
        "a timeout must not be downgraded into an interrupt on the way down"
    );
}

/// G77 — the STOP verb's own cascade: a `control/stop.json` must end the run `Stopped` (a THIRD
/// terminal verdict, distinct from the interrupt's `Paused` and the timeout's `Failed` asserted by
/// the two tests above) and must reach every live async descendant as a STOP, never downgraded into
/// an interrupt or a timeout on the way down.
///
/// Upstream: `stopRunner` (`runs/background/subagent-runner.ts:2955-2984` @v0.43.0) sets
/// `statusPayload.state = "stopped"`, marks every running-or-pending step `"stopped"`, appends
/// `subagent.run.stopped`, and calls `stopNestedAsyncDescendants()` (`:2984`), which delivers
/// `deliverStopRequest({ …, source: "ancestor-stop" })` to each `running`/`queued` descendant
/// (`:2281-2310`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivered_stop_request_stops_the_run_and_cascades_to_descendants() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("real tempdir");
    let temp_root = dir.path().join("subagents-temp");
    std::fs::create_dir_all(&temp_root).unwrap();
    // SAFETY: see the sibling tests above.
    unsafe {
        std::env::set_var(TEMP_ROOT_ENV, &temp_root);
    }

    let harness = build_run(dir.path(), &temp_root, "cascadestp1", "childstp1").await;

    // Planted through the REAL parent-side primitive, so this covers the writer too.
    let stop_path = cyrup_ext_subagents::background::control::stop_request_path(
        &harness.run_paths.run_dir,
    );
    cyrup_ext_subagents::background::control::deliver_stop_request(
        &harness.run_paths.run_dir,
        "stop-action",
        None,
    )
    .await
    .expect("plant the stop request");

    run(&harness.config_path, &harness.run_paths)
        .await
        .expect("run() never returns Err");

    // SAFETY: scoped cleanup inside the same mutex-held critical section.
    unsafe {
        std::env::remove_var(TEMP_ROOT_ENV);
    }

    // 1. Consumed at most once.
    assert!(
        !stop_path.exists(),
        "the stop request was never consumed — it is still sitting in the inbox"
    );

    // 2. A THIRD terminal verdict, not either of the other two.
    let result: ResultFile = read_json(&harness.run_paths.result).await;
    assert_eq!(
        result.state,
        RunState::Stopped,
        "a stop is its own terminal state — never `Paused` (interrupt) and never `Failed` (timeout)"
    );
    assert!(!result.success);
    assert!(
        !result.results.iter().any(|r| r.interrupted),
        "a stopped run must not be recorded as interrupted (that would read as resumable)"
    );

    let status: RunStatus = read_json(&harness.run_paths.status).await;
    assert!(
        status
            .steps
            .iter()
            .all(|s| s.status == cyrup_ext_subagents::background::StepState::Stopped),
        "every unfinished step must be marked Stopped by the stop; got {:?}",
        status.steps.iter().map(|s| s.status).collect::<Vec<_>>()
    );

    // 3. pi's `subagent.run.stopped` lifecycle event.
    let events = tokio::fs::read_to_string(&harness.run_paths.events)
        .await
        .expect("events.jsonl exists");
    assert!(
        events.contains("subagent.run.stopped"),
        "no subagent.run.stopped event in:\n{events}"
    );

    // 4. The cascade: the descendant gets a STOP with pi's own `ancestor-stop` source.
    let descendant_inbox = harness.descendant_dir.join("control").join("stop.json");
    assert!(
        descendant_inbox.exists(),
        "the live async descendant at {} received no stop",
        harness.descendant_dir.display()
    );
    let delivered: StopRequest = read_json(&descendant_inbox).await;
    assert_eq!(delivered.kind, "stop");
    assert_eq!(delivered.source, "ancestor-stop");
    for downgrade in ["interrupt.json", "timeout.json"] {
        assert!(
            !harness.descendant_dir.join("control").join(downgrade).exists(),
            "a stop must not be downgraded into {downgrade} on the way down"
        );
    }
}

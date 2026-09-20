//! Integration test: the process-terminal proof over a REAL background run — ledger rows VL-S4's
//! positive half.
//!
//! Everything here is real. `run_with` drives the actual hop-2 runner main loop in process; that
//! loop spawns the actual `cyrup-subagent-fixture` binary as a genuine OS child in its own process
//! group; the assertions read the files the runner itself wrote. Nothing is stubbed, and nothing
//! about the process-terminal artifact is synthesized by the test.
//!
//! # What this file proves that a unit test cannot
//!
//! `background::process_terminal`'s own tests drive the ladder over hand-written candidates. They
//! cannot show that the candidate a real run writes is CORRECT — that the writer records in it
//! describe children that really existed, that their process groups were really verified torn
//! down, and that the identity the orchestrator minted is the identity the proof ends up carrying.
//! That chain crosses four modules and one process boundary, and only a real child can close it.
//!
//! It is also the file that makes the `[CYRUP-DELTA]` on cyrup's writer records falsifiable. pi
//! writes `writers[i] = []` and `expectedWriters[i] = 0` for every step, unconditionally
//! (`subagent-runner.ts:5168-5190`), because its children run inside the runner process — its own
//! comment at `:5169` says so. If cyrup's candidate ever came back with an empty `writers` map,
//! the proof would still read `observed` and nothing else in the suite would notice; the
//! assertions below are what notice.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::Path;

use cyrup_core::ModelId;
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::process_terminal::{
    InstanceKind, ProcessInstanceExit, ProcessTerminal, ProcessTerminalState, ProcessTreeTerminal,
    ProofExpectation, RunnerProcessInstanceId, read_process_terminal,
    read_process_terminal_candidate, valid_process_instance,
};
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, RunnerOverrides, run_with};
use cyrup_ext_subagents::background::{RunDir, RunId, RunMode, RunPaths, RunState, RunStatus};
use cyrup_ext_subagents::discovery::types::SystemPromptMode;
use cyrup_ext_subagents::exec::ResolvedAgentPersona;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};

fn fixture_persona(name: &str) -> ResolvedAgentPersona {
    ResolvedAgentPersona {
        file_path: None,
        acceptance_role: None,
        default_acceptance: None,
        name: name.to_string(),
        model: Some(ModelId::from("fixture-model")),
        model_provider: None,
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
        completion_guard: Some(false),
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
        runner: None,
    }
}

fn fixture_cmd(script_path: &Path) -> SpawnCommand {
    SpawnCommand {
        binary: crate::support::bins::subagent_fixture(),
        base_args: vec![
            "--fixture-script".to_string(),
            script_path.display().to_string(),
        ],
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

/// The run config every test here launches, mirroring
/// `background_runner_main_integration.rs`'s own literal field for field. A session id is
/// mandatory: the session-partitioned result index refuses a session-less result outright
/// (`write_result_file`, pi `result-files.ts:166`), so a runner expected to land a terminal
/// `ResultFile` must carry the launching session's identity.
fn base_config(run_id: RunId, steps: Vec<RunnerStep>, dir: &Path) -> RunnerConfig {
    RunnerConfig {
        host_available_builtins: None,
        completion_owner_id: None,
        turn_budget: None,
        permission_rules: None,
        usage_budget: None,
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id,
        runner_process_instance_id: None,
        revival_lease: None,
        mode: RunMode::Chain,
        steps,
        cwd: dir.to_path_buf(),
        session_file: None,
        session_id: Some("it-session".to_string()),
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: dir.join("async"),
        results_dir: dir.join("results"),
        resolved_agents: [("worker".to_string(), fixture_persona("worker"))]
            .into_iter()
            .collect(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        inherited_session_thinking: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        model_scope: None,
        control: None,
        include_progress: None,
    }
}

/// The run this file drives: N steps, each dispatching one real child that prints one assistant
/// message and exits 0.
struct Run {
    _tmp: tempfile::TempDir,
    run_paths: RunPaths,
    run_dir: RunDir,
    run_id: RunId,
    instance: RunnerProcessInstanceId,
    roots: Roots,
}

impl Run {
    async fn execute(step_count: usize) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        // Every root this run touches is confined to the tempdir — including the session-lease
        // root the process-terminal ladder consults, which is resolved from these `Roots`.
        let roots = Roots::sandboxed(dir);

        let script_path = dir.join("script.json");
        std::fs::write(
            &script_path,
            serde_json::json!({
                "steps": [{ "kind": "emit", "line": message_end_line("done") }],
                "exit_code": 0,
            })
            .to_string(),
        )
        .expect("write fixture script");

        let async_root = dir.join("async");
        let results_dir = dir.join("results");
        tokio::fs::create_dir_all(&async_root)
            .await
            .expect("mkdir async");
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir results");

        let run_id = RunId::new();
        let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir)
            .await
            .expect("mkdir run dir");
        let run_dir = RunDir::for_existing(&run_paths.run_dir);

        // The ORCHESTRATOR's half, exactly as `spawn_background_steps` performs it: mint the
        // identity, put it on the config, and establish ownership BEFORE the runner is authorized
        // to touch any child session (pi `process-terminal.ts:118`).
        let instance = RunnerProcessInstanceId::new();
        cyrup_ext_subagents::background::process_terminal::initialize_process_terminal(
            &run_dir, &run_id, &instance,
        )
        .await
        .expect("initialize process terminal");

        let steps: Vec<RunnerStep> = (0..step_count)
            .map(|i| RunnerStep::SingleStep(single_step("worker", &format!("task {i}"))))
            .collect();
        let personas: BTreeMap<String, ResolvedAgentPersona> =
            [("worker".to_string(), fixture_persona("worker"))]
                .into_iter()
                .collect();

        let mut config = base_config(run_id.clone(), steps, dir);
        config.runner_process_instance_id = Some(instance.clone());
        config.resolved_agents = personas;

        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &config)
            .await
            .expect("write runner config");

        run_with(
            &cfg_path,
            &run_paths,
            RunnerOverrides {
                spawn_command: Some(fixture_cmd(&script_path)),
                roots: Some(roots.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("run() itself never returns Err");

        Self {
            _tmp: tmp,
            run_paths,
            run_dir,
            run_id,
            instance,
            roots,
        }
    }

    async fn status(&self) -> RunStatus {
        serde_json::from_slice(
            &tokio::fs::read(&self.run_paths.status)
                .await
                .expect("read status"),
        )
        .expect("parse status")
    }

    fn event_lines(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.run_dir.events())
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}

/// THE END-TO-END PROOF.
///
/// A real two-step background run leaves a `process-terminal.json` that says `observed`, carries
/// the identity the orchestrator minted, and — the part upstream cannot do — names one real writer
/// child per step whose process GROUP was verified torn down.
#[tokio::test]
async fn a_real_background_run_leaves_an_observed_proof_naming_its_real_children() {
    let run = Run::execute(2).await;

    // ---- the candidate the runner wrote -------------------------------------------------------
    let candidate = read_process_terminal_candidate(&run.run_dir)
        .await
        .expect("the candidate reads")
        .expect("the runner wrote a candidate");
    assert_eq!(candidate.run_id, run.run_id);
    assert_eq!(candidate.runner_process_instance_id, run.instance);

    // pi writes `writers[i] = []` / `expectedWriters[i] = 0` for every step
    // (`subagent-runner.ts:5168-5190`) because its children run in-process. cyrup's do not, and
    // these two assertions are what stop this port quietly regressing to upstream's empty shape.
    let expected = candidate
        .expected_writers
        .as_ref()
        .expect("cyrup always declares its writer counts");
    assert_eq!(expected.len(), 2, "one entry per flat step: {expected:?}");
    for index in ["0", "1"] {
        assert_eq!(
            expected.get(index).copied(),
            Some(1),
            "step {index} launched exactly one child: {expected:?}"
        );
        let records = candidate
            .writers
            .get(index)
            .unwrap_or_else(|| panic!("step {index} has a writers entry: {:?}", candidate.writers));
        assert_eq!(records.len(), 1, "step {index}: {records:?}");
        let ProcessInstanceExit::PiWriter {
            attempt,
            exit_code,
            process_tree,
            process_instance_id,
            ..
        } = &records[0]
        else {
            panic!("a step's writer record is a pi-writer exit: {records:?}");
        };
        assert_eq!(*attempt, 0, "the first attempt of step {index}");
        assert_eq!(
            *exit_code,
            Some(0),
            "the fixture child exits cleanly: {records:?}"
        );
        assert!(!process_instance_id.is_empty());
        // The claim the whole delta rests on: a REAL posix process group, verified gone.
        assert!(
            matches!(
                process_tree,
                ProcessTreeTerminal::ObservedProcessGroup { process_group_id, .. }
                    if *process_group_id > 0
            ),
            "step {index}'s child must have had its process group verified torn down: \
             {process_tree:?}"
        );
        // ...and it must satisfy upstream's own validator, which is what
        // `finalizeProcessTerminal` reads the candidate back through.
        let raw = serde_json::to_value(&records[0]).expect("encode writer record");
        assert!(
            valid_process_instance(&raw, Some(InstanceKind::PiWriter)),
            "pi's own `validProcessInstance` must accept the record cyrup writes: {raw}"
        );
    }

    // ---- the proof ---------------------------------------------------------------------------
    let proof = read_process_terminal(
        &run.run_dir,
        ProofExpectation::new(&run.run_id, &run.instance),
    )
    .await
    .expect("the sidecar exists");
    let ProcessTerminal::Observed {
        instances,
        observed_at,
        ..
    } = &proof
    else {
        panic!("a clean run's close is observed, got {proof:?}");
    };
    assert!(*observed_at > 0);
    assert_eq!(proof.run_id(), &run.run_id);
    assert_eq!(*proof.runner_process_instance_id(), run.instance);
    // pi `:290` — the runner first, then every writer, flattened in step order.
    assert_eq!(
        instances.len(),
        3,
        "runner + two writer children: {instances:?}"
    );
    let ProcessInstanceExit::Runner {
        process_instance_id,
        exit_code,
        signal,
        ..
    } = &instances[0]
    else {
        panic!("the first instance is the runner's own close: {instances:?}");
    };
    assert_eq!(*process_instance_id, run.instance);
    assert_eq!(*exit_code, Some(0));
    // A runner that died of a signal never reached its close observation at all, so this half can
    // only ever be `None` — see `finalize_own_process_terminal`'s doc.
    assert_eq!(*signal, None);

    // ---- the overlay -------------------------------------------------------------------------
    let status = run.status().await;
    assert_eq!(status.state, RunState::Complete, "{:?}", status.error);
    assert_eq!(
        status.process_terminal.as_ref().map(ProcessTerminal::state),
        Some(ProcessTerminalState::Observed),
        "the run-level overlay carries the finalized proof: {:?}",
        status.process_terminal
    );
    assert_eq!(
        status
            .process_terminal
            .as_ref()
            .map(ProcessTerminal::runner_process_instance_id),
        Some(&run.instance),
        "and the identity a reader matches on"
    );
    for (index, step) in status.steps.iter().enumerate() {
        let step_proof = step
            .process_terminal
            .as_ref()
            .unwrap_or_else(|| panic!("step {index} has an overlay: {step:?}"));
        assert_eq!(
            step_proof.state(),
            ProcessTerminalState::Observed,
            "step {index} launched one child and its close was observed"
        );
        assert_eq!(step_proof.base().child_index, Some(index));
    }

    // ---- the event ---------------------------------------------------------------------------
    let events: Vec<serde_json::Value> = run
        .event_lines()
        .into_iter()
        .filter(|line| line["type"] == serde_json::json!("subagent.run.process_terminal"))
        .collect();
    assert_eq!(events.len(), 1, "exactly one lifecycle event: {events:?}");
    assert_eq!(events[0]["lifecycleArtifactVersion"], serde_json::json!(3));
    assert_eq!(
        events[0]["runId"],
        serde_json::json!(run.run_id.as_str()),
        "{:?}",
        events[0]
    );
    assert_eq!(
        events[0]["processTerminal"]["state"],
        serde_json::json!("observed")
    );

    // The lease root is the run-scratch one, and this run — which held no session file — never
    // created a lease directory under it.
    let lease_root = cyrup_ext_subagents::background::session_leases_root_in(&run.roots);
    assert!(
        !lease_root.exists(),
        "a run with no session file takes no lease: {lease_root:?}"
    );
}

/// P6 — pi `subagent-runner.ts:2102` / `async-execution.ts:839`: the runner stamps the PENDING
/// proof onto its very first `status.json` write, from the identity the orchestrator minted.
///
/// THE USER ACTION this exists for: an agent asks about a background run that has been quiet for a
/// while. `status.state` says `running` whether the runner is mid-step or was SIGKILLed ten
/// minutes ago — that is the ambiguity VL-S4 names. `status.processTerminal` is what closes it: it
/// reads `pending` for the whole run and is replaced at the close, so a proof that STAYS pending
/// after the process is gone is the crash signal.
///
/// The read has to happen MID-RUN, because the close overlays the same key — which is exactly why
/// this test drives the runner as a task and polls rather than asserting after the fact.
#[tokio::test]
async fn the_runner_publishes_a_pending_proof_on_its_first_status_write() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().to_path_buf();
    let roots = Roots::sandboxed(&dir);

    // A child that lingers, so `status.json` is observable while the run is genuinely `running`.
    let script_path = dir.join("script.json");
    std::fs::write(
        &script_path,
        serde_json::json!({
            "steps": [
                { "kind": "sleep_ms", "ms": 2000 },
                { "kind": "emit", "line": message_end_line("done") },
            ],
            "exit_code": 0,
        })
        .to_string(),
    )
    .expect("write fixture script");

    let async_root = dir.join("async");
    let results_dir = dir.join("results");
    tokio::fs::create_dir_all(&async_root)
        .await
        .expect("mkdir async");
    tokio::fs::create_dir_all(&results_dir)
        .await
        .expect("mkdir results");
    let run_id = RunId::new();
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir)
        .await
        .expect("mkdir run dir");

    let instance = RunnerProcessInstanceId::new();
    let mut config = base_config(
        run_id.clone(),
        vec![RunnerStep::SingleStep(single_step("worker", "task"))],
        &dir,
    );
    config.runner_process_instance_id = Some(instance.clone());
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config)
        .await
        .expect("write runner config");

    let status_path = run_paths.status.clone();
    let driver = tokio::spawn({
        let cfg_path = cfg_path.clone();
        let run_paths = run_paths.clone();
        let spawn_command = fixture_cmd(&script_path);
        async move {
            run_with(
                &cfg_path,
                &run_paths,
                RunnerOverrides {
                    spawn_command: Some(spawn_command),
                    roots: Some(roots),
                    ..Default::default()
                },
            )
            .await
            .expect("run() itself never returns Err");
        }
    });

    // Poll for the runner's own first status write.
    let mut observed: Option<RunStatus> = None;
    for _ in 0..400 {
        if let Ok(bytes) = tokio::fs::read(&status_path).await
            && let Ok(status) = serde_json::from_slice::<RunStatus>(&bytes)
            && status.state == RunState::Running
        {
            observed = Some(status);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let running = observed.expect("the runner publishes a `running` status while its child works");
    let pending = running
        .process_terminal
        .as_ref()
        .expect("the runner's first status write carries the pending proof");
    assert_eq!(
        pending.state(),
        ProcessTerminalState::Pending,
        "a live run has observed nothing yet: {pending:?}"
    );
    assert_eq!(
        *pending.runner_process_instance_id(),
        instance,
        "and it carries the identity the LAUNCH minted, never one the runner chose"
    );
    assert_eq!(pending.run_id(), &run_id);

    driver.await.expect("the runner task joins");
}

/// The refusal half, on the same real path: a proof belonging to another runner is never accepted
/// as this one's.
///
/// This is the check the whole `RunnerProcessInstanceId` mint exists to make meaningful, and the
/// reason it is a fresh v4 uuid rather than the reusable `(runner_pid, runner_started_at)` pair
/// `active_async_capacity` had to substitute while no identity existed.
#[tokio::test]
async fn a_readers_expectation_refuses_a_proof_from_another_runner() {
    let run = Run::execute(1).await;

    let foreign = RunnerProcessInstanceId::new();
    let refused = read_process_terminal(&run.run_dir, ProofExpectation::new(&run.run_id, &foreign))
        .await
        .expect("the sidecar exists");
    assert_eq!(
        refused.state(),
        ProcessTerminalState::Unknown,
        "a proof for a different runner must not read as observed: {refused:?}"
    );
    let ProcessTerminal::Unknown { diagnostic, .. } = &refused else {
        panic!("expected unknown");
    };
    let message = diagnostic.as_deref().unwrap_or_default();
    assert!(
        message.starts_with("Process-terminal proof in '")
            && message.contains("belongs to runner '"),
        "the refusal carries upstream's own sentence: {message}"
    );

    // ...and with the right identity, the same file reads as the positive proof it is.
    let accepted = read_process_terminal(
        &run.run_dir,
        ProofExpectation::new(&run.run_id, &run.instance),
    )
    .await
    .expect("the sidecar exists");
    assert_eq!(accepted.state(), ProcessTerminalState::Observed);
}

/// A run launched by a build that minted no identity writes NO candidate and leaves the launch's
/// own artifacts untouched.
///
/// `RunnerConfig::runner_process_instance_id` is `Option` for exactly this case — a
/// `runner-config.json` written before the field existed. Inventing an identity here would produce
/// a proof no reader holds the key to, which is worse than no proof at all.
#[tokio::test]
async fn a_run_with_no_minted_identity_writes_no_proof_rather_than_inventing_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let roots = Roots::sandboxed(dir);
    let script_path = dir.join("script.json");
    std::fs::write(
        &script_path,
        serde_json::json!({
            "steps": [{ "kind": "emit", "line": message_end_line("done") }],
            "exit_code": 0,
        })
        .to_string(),
    )
    .expect("write fixture script");

    let async_root = dir.join("async");
    let results_dir = dir.join("results");
    tokio::fs::create_dir_all(&async_root)
        .await
        .expect("mkdir async");
    tokio::fs::create_dir_all(&results_dir)
        .await
        .expect("mkdir results");
    let run_id = RunId::new();
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir)
        .await
        .expect("mkdir run dir");
    let run_dir = RunDir::for_existing(&run_paths.run_dir);

    let config = base_config(
        run_id.clone(),
        vec![RunnerStep::SingleStep(single_step("worker", "task"))],
        dir,
    );
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config)
        .await
        .expect("write runner config");

    run_with(
        &cfg_path,
        &run_paths,
        RunnerOverrides {
            spawn_command: Some(fixture_cmd(&script_path)),
            roots: Some(roots),
            ..Default::default()
        },
    )
    .await
    .expect("run() itself never returns Err");

    assert!(
        !run_dir.process_terminal_candidate().exists(),
        "no identity means no candidate"
    );
    assert!(
        !run_dir.process_terminal().exists(),
        "no identity means no proof"
    );
    let status: RunStatus = serde_json::from_slice(
        &tokio::fs::read(&run_paths.status)
            .await
            .expect("read status"),
    )
    .expect("parse status");
    assert_eq!(
        status.state,
        RunState::Complete,
        "the run itself still succeeds"
    );
    assert!(status.process_terminal.is_none());
}

/// VL-S4's WHOLE OBSERVABLE, from the crash side: a runner that dies mid-run leaves a sidecar that
/// is still `pending`, where the same run allowed to finish leaves `observed`.
///
/// Nothing else in the suite asserts this. Every other test here drives a run to completion, so
/// deleting `finalize_own_process_terminal` from `runner_main::entry`'s tail — or moving it ABOVE
/// `finish_run` — would leave the whole suite green while the crash-vs-clean discriminator this
/// ledger row exists for was gone. Three things are pinned, and each one fails a different
/// mutation:
///
/// 1. **A runner that never reached its tail leaves `pending`.** The driver task is ABORTED while
///    its real child is still working, which is what a `SIGKILL` leaves behind on disk: the
///    launch's `pending` sidecar, and the launch's EMPTY candidate. (`abort` is the faithful
///    stand-in here because `run_with` is driven in-process; the artifact state it produces — a
///    runner that stopped between its status write and its close — is byte-for-byte the state a
///    killed detached runner leaves, and `debug_run_lifecycle_integration.rs` covers the same
///    `pending` sidecar under a genuinely dead OS pid with the capacity ladder reading it.)
/// 2. **The clean twin leaves `observed`.** Same config, same fixture, allowed to finish.
/// 3. **The proof is finalized AFTER `finish_run`, not before.** Its `resumeDisposition` is
///    computed from the status on disk at finalize time (`process-terminal.ts:144-148`), so a
///    finalize hoisted above the terminal write would see `running` and answer `unavailable`;
///    after it, with the transcript present, it answers `resumable`. That single word is the
///    ordering, and R-SA-077 forbids a third write landing between the status and the result.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_killed_runner_leaves_a_pending_proof_where_a_clean_one_leaves_observed() {
    /// One run, driven either to completion or to an abort mid-child.
    async fn drive(
        kill_mid_run: bool,
    ) -> (
        // Held so the run's artifacts outlive this call — every assertion below reads them.
        tempfile::TempDir,
        RunDir,
        RunId,
        RunnerProcessInstanceId,
        ProcessTerminal,
    ) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let roots = Roots::sandboxed(&dir);

        // A child that lingers long enough to be caught mid-run, then finishes cleanly.
        let script_path = dir.join("script.json");
        std::fs::write(
            &script_path,
            serde_json::json!({
                "steps": [
                    { "kind": "sleep_ms", "ms": 1500 },
                    { "kind": "emit", "line": message_end_line("done") },
                ],
                "exit_code": 0,
            })
            .to_string(),
        )
        .expect("write fixture script");

        let async_root = dir.join("async");
        let results_dir = dir.join("results");
        tokio::fs::create_dir_all(&async_root)
            .await
            .expect("mkdir async");
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir results");
        let run_id = RunId::new();
        let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&run_paths.run_dir)
            .await
            .expect("mkdir run dir");
        let run_dir = RunDir::for_existing(&run_paths.run_dir);

        // The transcript the run writes, so `resumeDisposition` has a file to test for — rung 3
        // above needs `fs.existsSync(sessionFile)` to hold (pi `:147`).
        let session_file = dir.join("session.jsonl");
        std::fs::write(&session_file, b"{}\n").expect("write session transcript");

        let instance = RunnerProcessInstanceId::new();
        cyrup_ext_subagents::background::process_terminal::initialize_process_terminal(
            &run_dir, &run_id, &instance,
        )
        .await
        .expect("initialize process terminal");

        let mut config = base_config(
            run_id.clone(),
            vec![RunnerStep::SingleStep(single_step("worker", "task"))],
            &dir,
        );
        config.runner_process_instance_id = Some(instance.clone());
        config.session_file = Some(session_file.clone());
        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &config)
            .await
            .expect("write runner config");

        let status_path = run_paths.status.clone();
        let driver = tokio::spawn({
            let cfg_path = cfg_path.clone();
            let run_paths = run_paths.clone();
            let spawn_command = fixture_cmd(&script_path);
            async move {
                run_with(
                    &cfg_path,
                    &run_paths,
                    RunnerOverrides {
                        spawn_command: Some(spawn_command),
                        roots: Some(roots),
                        ..Default::default()
                    },
                )
                .await
                .expect("run() itself never returns Err");
            }
        });

        if kill_mid_run {
            // Wait until the runner is genuinely running a child, then stop it dead. Waiting for
            // `Running` is what makes this a KILL and not a race against startup.
            for _ in 0..400 {
                if let Ok(bytes) = tokio::fs::read(&status_path).await
                    && let Ok(status) = serde_json::from_slice::<RunStatus>(&bytes)
                    && status.state == RunState::Running
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            driver.abort();
            let _ = driver.await;
        } else {
            driver.await.expect("the runner task joins");
        }

        let proof = read_process_terminal(&run_dir, ProofExpectation::new(&run_id, &instance))
            .await
            .expect("the launch wrote a sidecar before the spawn, so one always exists");
        (tmp, run_dir, run_id, instance, proof)
    }

    // ---- 1. the killed runner -----------------------------------------------------------------
    let (_killed_tmp, killed_dir, killed_run, killed_instance, killed) = drive(true).await;
    assert_eq!(
        killed.state(),
        ProcessTerminalState::Pending,
        "a runner that never reached its close proved nothing, and the record must say so \
         rather than guess: {killed:?}"
    );
    assert_eq!(killed.run_id(), &killed_run);
    assert_eq!(*killed.runner_process_instance_id(), killed_instance);
    // And the candidate is still the launch's EMPTY one — the crash discriminator the ladder
    // reads if anything ever does finalize this run (`process-terminal.ts:277`).
    let candidate = read_process_terminal_candidate(&killed_dir)
        .await
        .expect("the candidate reads")
        .expect("the launch wrote one");
    assert!(
        candidate.writers.is_empty() && candidate.expected_writers.is_none(),
        "a runner that died before its tail declared nothing: {candidate:?}"
    );

    // ---- 2 and 3. the clean twin --------------------------------------------------------------
    let (_clean_tmp, _clean_dir, clean_run, clean_instance, clean) = drive(false).await;
    assert_eq!(
        clean.state(),
        ProcessTerminalState::Observed,
        "the same run allowed to finish DOES prove its close: {clean:?}"
    );
    assert_eq!(clean.run_id(), &clean_run);
    assert_eq!(*clean.runner_process_instance_id(), clean_instance);
    assert_eq!(
        clean.base().resume_disposition,
        Some(cyrup_ext_subagents::background::process_terminal::ResumeDisposition::Resumable),
        "the proof is finalized AFTER `finish_run` wrote the terminal status: a finalize hoisted \
         above it would read `running` and answer `unavailable`: {clean:?}"
    );
}

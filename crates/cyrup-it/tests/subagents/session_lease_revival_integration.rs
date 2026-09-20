//! Integration test: the canonical-session revival lease over a REAL background run — ledger row
//! VL-S3.
//!
//! Everything here is real. `run_with` drives the actual hop-2 runner main loop in process, which
//! is the process that acquires, holds and releases the lease (pi `subagent-runner.ts:5241-5293`);
//! that loop spawns the actual `cyrup-subagent-fixture` binary as a genuine OS child in its own
//! process group, which is what puts a REAL pid into the lease's `writerState: "running"`; and
//! every assertion reads a file the runner itself wrote.
//!
//! # What this file proves that a unit test cannot
//!
//! `background::session_lease`'s own tests drive the ladder with both ambient probes injected —
//! a foreign hostname, a dead pid, a recycled start identity. They prove the ALGORITHM. They
//! cannot prove that any production path ever acquires a lease at all, that the runner releases
//! it on the way out, that the release reaches the process-terminal candidate, or that a second
//! revival of one session file is actually turned away with upstream's sentence instead of
//! quietly running beside the first. That chain crosses the orchestrator, the one-shot config, the
//! runner's startup, its step loop and its close, and only a real run closes it.
//!
//! VL-S3's whole claim is the last of those: **two revivals of one session file cannot run at
//! once.** `a_second_revival_of_one_session_file_is_refused_with_upstreams_sentence` is that
//! claim, stated against a lease held by a process that is genuinely alive.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cyrup_core::ModelId;
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::process_terminal::{
    ProcessTerminal, ProcessTerminalState, ProofExpectation, RunnerProcessInstanceId,
    read_process_terminal, read_process_terminal_candidate,
};
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, RunnerOverrides, run_with};
use cyrup_ext_subagents::background::session_lease::{
    LeaseToken, LeaseWriter, SessionLeaseOptions, SessionLeaseRequest, SessionLeaseState,
    acquire_session_lease, inspect_session_lease, session_lease_dir,
};
use cyrup_ext_subagents::background::{
    RunDir, RunId, RunMode, RunPaths, RunState, RunStatus, session_leases_root_in,
};
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

/// One revival run, staged exactly as the orchestrator stages it: mint the identity, establish
/// ownership BEFORE the runner is authorized, put the lease REQUEST on the one-shot config, and
/// drive `run_with`.
struct Revival {
    tmp: tempfile::TempDir,
    dir: PathBuf,
    roots: Roots,
    run_paths: RunPaths,
    run_dir: RunDir,
    run_id: RunId,
    instance: RunnerProcessInstanceId,
    session_file: PathBuf,
    cfg_path: PathBuf,
    script_path: PathBuf,
}

impl Revival {
    /// Stage everything up to the spawn, without running. `sleep_ms` holds the fixture child alive
    /// so a concurrent reader can observe the lease MID-RUN.
    async fn stage(sleep_ms: u64) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        // Every root, including the session-lease root, is confined here — `paths.rs:186-191`
        // records what an unconfined one costs, and a leaked lease directory under a shared root
        // refuses every later revival of that session file on the machine.
        let roots = Roots::sandboxed(&dir);

        let mut steps = vec![serde_json::json!({
            "kind": "emit", "line": message_end_line("revived")
        })];
        if sleep_ms > 0 {
            steps.insert(0, serde_json::json!({"kind": "sleep_ms", "ms": sleep_ms}));
        }
        let script_path = dir.join("script.json");
        std::fs::write(
            &script_path,
            serde_json::json!({ "steps": steps, "exit_code": 0 }).to_string(),
        )
        .expect("write fixture script");

        // The session transcript the revival reopens. It must EXIST: the lease is keyed on its
        // realpath, which is what makes two paths to one file take one lease.
        let session_file = dir.join("sessions").join("revived.jsonl");
        std::fs::create_dir_all(session_file.parent().expect("parent")).expect("mkdir sessions");
        std::fs::write(&session_file, b"{}\n").expect("write session file");

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

        let instance = RunnerProcessInstanceId::new();
        cyrup_ext_subagents::background::process_terminal::initialize_process_terminal(
            &run_dir, &run_id, &instance,
        )
        .await
        .expect("initialize process terminal");

        let personas: BTreeMap<String, ResolvedAgentPersona> =
            [("worker".to_string(), fixture_persona("worker"))]
                .into_iter()
                .collect();
        let mut config = base_config(
            run_id.clone(),
            vec![RunnerStep::SingleStep(single_step("worker", "follow up"))],
            &dir,
        );
        config.runner_process_instance_id = Some(instance.clone());
        config.resolved_agents = personas;
        config.session_file = Some(session_file.clone());
        // P4 — exactly the record `control.rs::revive_from_transcript` puts on the spec, carried
        // onto the one-shot config by `spawn_background_steps`.
        config.revival_lease = Some(SessionLeaseRequest {
            session_file: session_file.clone(),
            run_id: run_id.clone(),
            source_run_id: RunId::from_token("source-run".to_string()),
            parent_session_id: Some("it-session".to_string()),
        });

        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &config)
            .await
            .expect("write runner config");

        Self {
            tmp,
            dir,
            roots,
            run_paths,
            run_dir,
            run_id,
            instance,
            session_file,
            cfg_path,
            script_path,
        }
    }

    async fn run(&self) {
        run_with(
            &self.cfg_path,
            &self.run_paths,
            RunnerOverrides {
                spawn_command: Some(fixture_cmd(&self.script_path)),
                roots: Some(self.roots.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("run() itself never returns Err");
    }

    fn lease_root(&self) -> PathBuf {
        session_leases_root_in(&self.roots)
    }

    fn lease_dir(&self) -> PathBuf {
        session_lease_dir(&self.session_file, &self.lease_root()).expect("lease dir")
    }

    async fn status(&self) -> RunStatus {
        serde_json::from_slice(
            &tokio::fs::read(&self.run_paths.status)
                .await
                .expect("read status"),
        )
        .expect("parse status")
    }
}

/// The happy path, end to end: the RUNNER acquires the lease for its whole run, records its real
/// OS child as the writer, releases it at the close, and the acknowledged release reaches the
/// process-terminal candidate — which is what lets the proof reach `observed` at all.
///
/// Gutted anywhere along P5→P7→P9: the lease directory survives the run (no release), or the
/// candidate carries no token (the release never reached it), or the proof comes back
/// `canonical-session-release-unverified` (release after finalize instead of before).
#[tokio::test]
async fn a_revival_run_holds_the_lease_for_its_whole_run_and_releases_it_at_the_close() {
    let revival = Revival::stage(0).await;

    assert!(
        inspect_session_lease(&revival.session_file, &revival.lease_root())
            .expect("inspect")
            .is_free(),
        "nothing holds the session file before the run"
    );

    revival.run().await;

    // P9 — the lease is released, and the release is DEMONSTRABLE: the directory is gone.
    assert!(
        !revival.lease_dir().exists(),
        "the runner releases its lease on the way out"
    );
    assert!(
        inspect_session_lease(&revival.session_file, &revival.lease_root())
            .expect("inspect")
            .is_free(),
        "and the next revival of this session file is free to take it"
    );

    // P8/P9 — the candidate carries the token this run held and an ACKNOWLEDGED release. Without
    // both, `finalizeProcessTerminal`'s `:275-276` rung refuses the proof.
    let candidate = read_process_terminal_candidate(&revival.run_dir)
        .await
        .expect("candidate reads")
        .expect("the runner wrote one");
    let token = candidate
        .revival_lease_token
        .as_ref()
        .expect("the candidate names the lease token this run held");
    assert!(!token.as_str().is_empty());
    assert_eq!(
        candidate.revival_lease_release_acknowledged,
        Some(true),
        "the release was observed, not merely attempted"
    );
    // This assertion is also what makes `mark_process_terminal_candidate_lease_release` a REAL
    // call rather than a dispatched no-op. The acknowledgement has exactly one writer
    // (`process-terminal.ts:134-138`, `runner_main::release_and_record_lease`), and it only lands
    // because the candidate write happens FIRST, carrying the token the stamp matches on
    // (`:136`). Invert those two — or delete the stamp — and this line goes red while everything
    // else about the run stays identical.
    assert_eq!(candidate.run_id, revival.run_id);
    assert_eq!(candidate.runner_process_instance_id, revival.instance);

    // P10 — and because the release happened BEFORE the finalize, the proof reaches `observed`.
    let proof = read_process_terminal(
        &revival.run_dir,
        ProofExpectation::new(&revival.run_id, &revival.instance),
    )
    .await
    .expect("the runner wrote a sidecar");
    assert_eq!(
        proof.state(),
        ProcessTerminalState::Observed,
        "a released lease and an acknowledged release let the ladder reach observed: {proof:?}"
    );
    let ProcessTerminal::Observed {
        canonical_session, ..
    } = &proof
    else {
        panic!("observed");
    };
    let canonical_session = canonical_session
        .as_ref()
        .expect("the run held a session file, so the projection can be made");
    assert!(
        canonical_session.free_at_observation,
        "the session file really was free when the close was observed"
    );

    let status = revival.status().await;
    assert_eq!(status.state, RunState::Complete);
    drop(revival.tmp);
}

/// **VL-S3's whole claim.** A second revival of one session file, while a LIVE process holds its
/// lease, is refused — with upstream's own sentence — and never runs a single step.
///
/// The incumbent's pid is this test process's own, so the liveness probe is real: the owner is
/// genuinely alive, on this genuine hostname, under this process's genuine start identity. Nothing
/// about the refusal is arranged by an injected probe.
///
/// Gutted (P5 dropped, or the acquire's result ignored): the run proceeds, two runners write one
/// session transcript and interleave their turns — the exact hazard the ledger row names. The
/// `Failed` state and the sentence below both flip.
#[tokio::test]
async fn a_second_revival_of_one_session_file_is_refused_with_upstreams_sentence() {
    let revival = Revival::stage(0).await;

    // A live incumbent, taken through the real acquire: this process's pid, this machine's
    // hostname, this process's start identity. `demonstrablyStale` can find no evidence of death
    // anywhere, because there is none.
    let incumbent = acquire_session_lease(
        &SessionLeaseRequest {
            session_file: revival.session_file.clone(),
            run_id: RunId::from_token("incumbent-run".to_string()),
            source_run_id: RunId::from_token("incumbent-source".to_string()),
            parent_session_id: Some("incumbent-session".to_string()),
        },
        &revival.lease_root(),
        &SessionLeaseOptions::default(),
    )
    .await
    .expect("the incumbent takes the lease");
    let incumbent_token: LeaseToken = incumbent.token().clone();
    let canonical = incumbent
        .owner()
        .canonical_session_file
        .display()
        .to_string();

    revival.run().await;

    // R-SA-077's invariant holds on the refusal path too: status.json THEN the ResultFile, both
    // terminal, on an exit path that never ran a step.
    let status = revival.status().await;
    assert_eq!(
        status.state,
        RunState::Failed,
        "a revival that cannot take the lease does not run"
    );
    assert_eq!(
        status.error.as_deref(),
        Some(
            format!(
                "Direct revival of session '{canonical}' is already owned by run \
                 'incumbent-run' (source run 'incumbent-source', parent session \
                 'incumbent-session', pid {} on {}). Wait for that revival to finish or start a \
                 separate continuation without reusing this session file.",
                std::process::id(),
                cyrup_ext_subagents::background::async_retention::machine_hostname(),
            )
            .as_str()
        ),
        "pi `conflictMessage` (`session-lease.ts:158-159`), byte for byte, on the run's own record"
    );
    assert!(
        status.steps.is_empty() || status.steps.iter().all(|step| step.run_id.is_none()),
        "no step was dispatched: {:?}",
        status.steps
    );
    // pi `persistPreProceedStartupFailure` (`async-execution.ts:636-640`): a run that never got as
    // far as a child publishes `not-started`, carrying its OWN run id and the launch's minted
    // instance. That triple is the input to the capacity pool's early-failure carve-out, which is
    // what releases this run's slot at once rather than after the abandoned timeout.
    let ProcessTerminal::NotStarted { base } = status
        .process_terminal
        .as_ref()
        .expect("a refused revival still publishes a process-terminal record")
    else {
        panic!(
            "a run that never started is `not-started`: {:?}",
            status.process_terminal
        );
    };
    assert_eq!(base.run_id, revival.run_id);
    assert_eq!(base.runner_process_instance_id, revival.instance);
    // …and the SIDECAR stays `pending`. Nothing was observed, so nothing claims to have been.
    let sidecar = read_process_terminal(
        &revival.run_dir,
        ProofExpectation::new(&revival.run_id, &revival.instance),
    )
    .await
    .expect("the launch wrote a pending sidecar");
    assert_eq!(
        sidecar.state(),
        ProcessTerminalState::Pending,
        "a refused revival observes no close and must not claim one"
    );
    // No child transcript was written, because no child was spawned.
    assert!(
        !revival.dir.join("async").join("children").exists(),
        "a refused revival spawns nothing"
    );

    // The incumbent's lease is UNTOUCHED — not stolen, not tombstoned.
    let SessionLeaseState::Owned { owner, .. } =
        inspect_session_lease(&revival.session_file, &revival.lease_root()).expect("inspect")
    else {
        panic!("the incumbent still holds it");
    };
    assert_eq!(owner.token, incumbent_token);
    assert_eq!(owner.run_id, "incumbent-run");
    drop(revival.tmp);
}

/// P7 — the lease's `writerState` really tracks a REAL OS child, read MID-RUN.
///
/// This is the `[CYRUP-DELTA]` made falsifiable: upstream never calls `updateWriter` at v0.68.0
/// (`git grep -n updateWriter v0.68.0 -- 'src/**/*.ts'` finds the definition and nothing else),
/// because its children run inside the runner process. cyrup's do not, so the field upstream
/// defined and never fed carries a genuine pid here — which is what makes the `spawning` and
/// `running` staleness rungs reachable rather than serde-only.
///
/// The child sleeps, so the assertion below runs while it is alive. Gutted (the writer channel not
/// installed, or `update_writer` never called): the owner record stays `writerState: "none"` for
/// the whole run, and a crashed runner's lease would then be reclaimable while its child is still
/// writing the session file.
#[tokio::test]
async fn the_lease_records_the_running_child_as_its_writer_mid_run() {
    let revival = Revival::stage(4_000).await;
    let lease_root = revival.lease_root();
    let session_file = revival.session_file.clone();

    let cfg_path = revival.cfg_path.clone();
    let run_paths = revival.run_paths.clone();
    let roots = revival.roots.clone();
    let script_path = revival.script_path.clone();
    let runner = tokio::spawn(async move {
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
    });

    // Poll for the `running` writer while the child sleeps.
    let mut observed: Option<u32> = None;
    for _ in 0..200 {
        if let Ok(SessionLeaseState::Owned { owner, .. }) =
            inspect_session_lease(&session_file, &lease_root)
            && let LeaseWriter::Running { pid, .. } = owner.writer
        {
            observed = Some(pid);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let pid = observed.expect("the lease records the dispatched child's pid as its writer");
    assert!(pid > 0);
    // And the pid is a REAL process — the one `SpawnedChild` reported, still alive mid-sleep.
    assert_ne!(
        pid,
        std::process::id(),
        "the writer is the CHILD, never this runner"
    );

    runner.await.expect("the runner task does not panic");

    // The lease is gone at the close, writer state and all.
    assert!(!revival.lease_dir().exists());
    drop(revival.tmp);
}

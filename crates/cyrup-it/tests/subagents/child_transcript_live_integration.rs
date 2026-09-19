//! Live child transcript (pi `src/shared/child-transcript.ts` @v0.68.0): a child's
//! `_transcript.jsonl` MUST grow WHILE the child runs, on both the foreground (`exec::run_sync`)
//! and the background (`runner_main::run_with`) path, from the parsed child-event stream.
//!
//! Every test here spawns the REAL `cyrup-subagent-fixture` binary as a genuine OS subprocess —
//! named through `RunOptions::spawn_command` / `RunnerOverrides::spawn_command`, never through
//! the process environment — and reads the REAL file the production writer
//! (`exec::child_transcript::ChildTranscriptWriter`, fed from `exec::drive_attempt`) appends to.
//!
//! # Why the read is provably MID-RUN
//!
//! The fixture emits three events and then parks itself behind a 30 s `sleep_ms` before emitting
//! its terminal events. Its first turn ends with `stopReason: "toolUse"`, not `"stop"`, so the
//! drive loop's final-stop grace drain is never armed; the loop then leaves only on stdout EOF,
//! a terminal assistant stop, or cancel/interrupt (`exec/drive_attempt.rs`), none of which can
//! happen while the child sleeps. The run future is pinned and raced
//! (`tokio::select! { biased; }`, run arm FIRST) against the file poll, so a run that had settled
//! before the poll succeeded would win the race and fail the test outright. Only after the mid-run
//! assertions pass is the run cancelled/interrupted. A test that read the file only after
//! completion would prove nothing about "live".
//!
//! # The mutations these tests exist to catch (all run, all observed)
//!
//! Delete the `write_child_event` call in `handle_child_line` (`exec/drive_attempt.rs`) and only
//! the sentinel ever lands, so the `>= 3 lines` poll times out after 10 s and A and B fail.
//! Delete the sentinel call in `run_sync` and only two records land before the sleep, so the same
//! poll times out (and, were the file read, record 0 would be the assistant message).
//!
//! Sections D and E drive the PRODUCTION producer, `SubagentExecutor::run_foreground` (the path
//! the `subagent` tool takes), because A and C set `RunOptions::transcript` by hand on `run_sync`
//! and so never reach `build_foreground_run_options`' gate. D fails when that gate is mutated to
//! `None`; E fails when any hop that carries the writer's latched error to a result surface
//! (`run_sync`'s result, `build_step_result`, `record_step_outcome`, the `ResultFile` projection)
//! is dropped.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::artifacts::{ArtifactConfig, artifact_paths, project_artifacts_dir};
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::control::InterruptRequest;
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, RunnerOverrides, run_with};
use cyrup_ext_subagents::background::{
    ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus, StepState,
};
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::child_transcript::{INITIAL_PROMPT_SENTINEL, TranscriptSource};
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{
    AgentConfig, ResolvedAgentPersona, RunOptions, SingleResult, run_sync,
};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;

/// A task text that must never reach the transcript: the first record is the redacted sentinel.
const TASK: &str = "TASK-TEXT-MUST-NEVER-APPEAR-IN-THE-TRANSCRIPT";
/// The line the parked fixture emits only AFTER its 30 s sleep — seeing it means the read was
/// not mid-run.
const AFTER_THE_SLEEP: &str = "SHOULD-NOT-REACH";
/// How long a mid-run poll may take before the test gives up (far under the fixture's sleep).
const POLL_BUDGET: Duration = Duration::from_secs(10);

fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
}

fn fixture_cmd(script_path: &Path) -> SpawnCommand {
    SpawnCommand {
        binary: fixture_binary_path(),
        base_args: vec![
            "--fixture-script".to_string(),
            script_path.display().to_string(),
        ],
    }
}

fn write_script(dir: &Path, name: &str, script_json: &serde_json::Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, script_json.to_string()).expect("write fixture script");
    path
}

/// One assistant `message_end` line. `stop_reason` matters: `"stop"` (with no tool call) is a
/// TERMINAL assistant stop that arms the drive loop's 1 s final-stop grace drain
/// (`exec/drive_attempt.rs`, pi `FINAL_STOP_GRACE_MS`), after which the child is force-drained
/// with SIGINT — so a child meant to stay parked must end its first turn with `"toolUse"`.
fn message_end_line(text: &str, stop_reason: &str) -> String {
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
            "stopReason": stop_reason
        }
    })
    .to_string()
}

/// The child that parks itself MID-CONVERSATION: three events, a 30 s sleep, then the rest. The
/// first turn ends with `"toolUse"` (it is about to run `bash`), so NO final-stop drain is armed
/// and nothing but cancel/interrupt can end the run while the child sleeps.
fn parked_child_script(dir: &Path) -> PathBuf {
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": "{\"type\":\"agent_start\"}"},
            {"kind": "emit", "line": message_end_line("FIRST: the child is talking", "toolUse")},
            {"kind": "emit", "line": serde_json::json!({
                "type": "tool_execution_start", "toolCallId": "call-1", "toolName": "bash",
                "args": {"command": "sleep 30"}
            }).to_string()},
            {"kind": "sleep_ms", "ms": 30_000},
            {"kind": "emit", "line": serde_json::json!({
                "type": "tool_execution_end", "toolCallId": "call-1", "toolName": "bash",
                "isError": false, "result": {"content": [{"type": "text", "text": "done"}]}
            }).to_string()},
            {"kind": "emit", "line": message_end_line(AFTER_THE_SLEEP, "stop")},
            {"kind": "emit", "line": "{\"type\":\"agent_end\"}"}
        ],
        "exit_code": 0
    });
    write_script(dir, "parked.json", &script)
}

/// A child that finishes at once, for the gate-off cases.
fn quick_child_script(dir: &Path) -> PathBuf {
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": "{\"type\":\"agent_start\"}"},
            {"kind": "emit", "line": message_end_line("done", "stop")},
            {"kind": "emit", "line": "{\"type\":\"agent_end\"}"}
        ],
        "exit_code": 0
    });
    write_script(dir, "quick.json", &script)
}

/// Write a trivial agent persona `.md` to `<cwd>/.cyrup/agents/<name>.md` — the project-scope
/// discovery root `SubagentExecutor::discovery_config` scans (copied from
/// `artifacts_run_integration.rs`, like every other fixture helper in this directory).
fn write_fixture_persona(cwd: &Path, name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: a trivial fixture persona for the transcript tests\n\
             model: fixture/model\n---\n\nYou are a trivial test persona.\n"
        ),
    )
    .expect("write fixture persona");
}

/// The PRODUCTION foreground path: `SubagentExecutor::run_foreground` — `run_foreground_impl` →
/// `build_foreground_run_options` → `exec::run_sync` — exactly as the `subagent` tool drives it,
/// with the default `ArtifactConfig` (`enabled: true`, `include_transcript: true`) and the
/// project artifacts dir `project_artifacts_dir(cwd)`. The fixture binary and the sandboxed home
/// root are named on the extension config, never on the process environment.
async fn run_foreground_through_the_executor(
    work_dir: &Path,
    home_dir: &Path,
    script_path: &Path,
) -> SingleResult {
    write_fixture_persona(work_dir, "worker");
    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            spawn_command: Some(fixture_cmd(script_path)),
            roots: Roots::sandboxed(home_dir),
            ..SubagentExtensionConfig::default()
        },
        work_dir.to_path_buf(),
    );
    let executor = extension.executor().clone();
    executor
        .run_foreground(work_dir, "worker", TASK, None, None, None)
        .await
        .expect("the foreground run completes without an orchestration-level error")
}

fn base_agent_config(model: &str) -> AgentConfig {
    AgentConfig {
        acceptance_role: None,
        default_acceptance: None,
        name: "worker".to_string(),
        model: Some(ModelId::from(model)),
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
        max_output: OutputCap::default(),
        max_subagent_depth: None,
        memory: None,
        tool_budget: None,
        runner: None,
        depth: DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
    }
}

fn base_run_options(cwd: &Path, model: &str) -> RunOptions {
    RunOptions {
        model_exclusions: None,
        host_available_builtins: None,
        structured_output_dir: None,
        spawn_command: None,
        child_env: std::collections::HashMap::new(),
        turn_budget: None,
        permission_rules: None,
        thinking_ceiling: None,
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
        acceptance: Some(AcceptanceContract::explicit(
            AcceptanceStatus::NotRequired,
            vec![],
        )),
        fork_context: ForkContext::fresh(),
        live_events: None,
        parent_session_id: None,
        clarify: None,
        orchestrator_intercom_target: None,
        run_id: None,
        child_index: None,
        steer_inbox_dir: None,
        steer_ack_dir: None,
        steer_capability_path: None,
        control_config: None,
        on_control_event: None,
        artifacts_dir: None,
        transcript: None,
        model_scope: None,
    }
}

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

/// The per-test background run layout: async root, results dir, run dir, all under `dir`.
struct BackgroundRun {
    run_id: RunId,
    run_paths: RunPaths,
    cfg_path: PathBuf,
}

async fn prepare_background_run(
    dir: &Path,
    run_token: &str,
    art_dir: &Path,
    artifact_config: ArtifactConfig,
) -> BackgroundRun {
    let async_root = dir.join("async");
    let results_dir = dir.join("results");
    tokio::fs::create_dir_all(&async_root)
        .await
        .expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir)
        .await
        .expect("mkdir results_dir");
    let run_id = RunId::from_token(run_token);
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir)
        .await
        .expect("mkdir run_dir");

    let config = RunnerConfig {
        host_available_builtins: None,
        completion_owner_id: None,
        turn_budget: None,
        permission_rules: None,
        usage_budget: None,
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: Some(art_dir.to_path_buf()),
        artifact_config,
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step("worker", TASK))],
        cwd: dir.to_path_buf(),
        session_file: None,
        // The session-partitioned result index refuses a session-less result outright.
        session_id: Some("it-session".to_string()),
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: async_root.clone(),
        results_dir: results_dir.clone(),
        resolved_agents: [("worker".to_string(), fixture_persona("worker"))]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
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
    };
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config)
        .await
        .expect("write config");
    BackgroundRun {
        run_id,
        run_paths,
        cfg_path,
    }
}

async fn read_status(run_paths: &RunPaths) -> RunStatus {
    serde_json::from_slice(
        &tokio::fs::read(&run_paths.status)
            .await
            .expect("status.json exists"),
    )
    .expect("parse status.json")
}

async fn read_terminal_result(run_paths: &RunPaths, status: &RunStatus) -> ResultFile {
    let path = run_paths
        .resolve_result(
            status
                .session_id
                .as_ref()
                .expect("this file's fixtures always launch with a session id"),
            &status.run_id,
        )
        .await
        .expect("terminal result file exists");
    serde_json::from_slice(&tokio::fs::read(&path).await.expect("read ResultFile"))
        .expect("parse ResultFile")
}

/// Poll `path` every 25 ms until it holds at least `min` lines, inside [`POLL_BUDGET`].
async fn poll_lines(path: &Path, min: usize) -> Vec<String> {
    tokio::time::timeout(POLL_BUDGET, async {
        loop {
            if let Ok(text) = tokio::fs::read_to_string(path).await {
                let lines: Vec<String> = text.lines().map(str::to_string).collect();
                if lines.len() >= min {
                    return lines;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the transcript at {} never reached {min} lines within {POLL_BUDGET:?} — the writer \
             is not being fed while the child runs",
            path.display()
        )
    })
}

/// Poll `status.json` every 25 ms until the run reports `Running`, inside [`POLL_BUDGET`].
async fn poll_until_running(run_paths: &RunPaths) -> RunStatus {
    tokio::time::timeout(POLL_BUDGET, async {
        loop {
            if let Ok(bytes) = tokio::fs::read(&run_paths.status).await
                && let Ok(status) = serde_json::from_slice::<RunStatus>(&bytes)
                && status.state == RunState::Running
            {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("status.json never reported Running within {POLL_BUDGET:?}"))
}

fn parse_records(lines: &[String]) -> Vec<serde_json::Value> {
    lines
        .iter()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("every transcript line is JSON: {e}: {line}"))
        })
        .collect()
}

/// The three records a parked child leaves on disk, in pi's vocabulary, with the sentinel first
/// and neither the task text nor the post-sleep line anywhere.
fn assert_mid_run_records(lines: &[String], source: &str, run_token: &str) {
    for line in lines {
        assert!(
            !line.contains(TASK),
            "the transcript must never carry the prompt: {line}"
        );
        assert!(
            !line.contains(AFTER_THE_SLEEP),
            "a post-sleep record on disk means the read was NOT mid-run: {line}"
        );
    }
    let records = parse_records(lines);
    assert!(records.len() >= 3, "{records:#?}");

    let sentinel = &records[0];
    assert_eq!(sentinel["recordType"], "message", "{sentinel}");
    assert_eq!(sentinel["sourceEventType"], "initial_prompt", "{sentinel}");
    assert_eq!(sentinel["role"], "user", "{sentinel}");
    assert_eq!(sentinel["text"], INITIAL_PROMPT_SENTINEL, "{sentinel}");
    assert_eq!(sentinel["version"], 1);
    assert_eq!(sentinel["source"], source);
    assert_eq!(sentinel["runId"], run_token);
    assert_eq!(sentinel["agent"], "worker");
    assert_eq!(sentinel["childIndex"], 0);

    let first = &records[1];
    assert_eq!(first["recordType"], "message", "{first}");
    assert_eq!(first["role"], "assistant", "{first}");
    assert_eq!(first["text"], "FIRST: the child is talking", "{first}");
    assert_eq!(first["sourceEventType"], "message_end", "{first}");

    let tool = &records[2];
    assert_eq!(tool["recordType"], "tool_start", "{tool}");
    assert_eq!(tool["toolName"], "bash", "{tool}");
    assert_eq!(tool["toolCallId"], "call-1", "{tool}");
    assert_eq!(tool["argsPreview"], "sleep 30", "{tool}");
}

// =================================================================================================
// A. Foreground: `exec::run_sync` direct
// =================================================================================================

/// The foreground path: `run_sync` creates the writer from its own artifact bundle, writes the
/// sentinel before the child spawns, and `drive_attempt` appends every parsed event — readable
/// while the child is parked, and published on the result once it is cancelled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_foreground_transcript_grows_while_the_child_is_still_running() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script_path = parked_child_script(dir.path());
    let art_dir = dir.path().join("artifacts");
    let cancel = CancelToken::new();

    let mut opts = base_run_options(dir.path(), "fixture-model");
    opts.spawn_command = Some(fixture_cmd(&script_path));
    opts.artifacts_dir = Some(art_dir.clone());
    opts.run_id = Some(RunId::from_token("livetx0001"));
    opts.child_index = Some(0);
    opts.transcript = Some(TranscriptSource::Foreground);
    opts.cancel = cancel.clone();
    let agent = base_agent_config("fixture-model");
    let expected = artifact_paths(&art_dir, "livetx0001", "worker", Some(0)).transcript_path;

    let mut run = std::pin::pin!(run_sync(&agent, TASK, &opts));
    let lines = tokio::select! {
        biased;
        result = &mut run => panic!("the run settled before the transcript was read: {result:?}"),
        lines = poll_lines(&expected, 3) => lines,
    };
    // The run future is still pending: the child is parked behind its 30 s sleep and nothing has
    // cancelled it. Everything below is asserted against a RUNNING child.
    assert_mid_run_records(&lines, "foreground", "livetx0001");

    cancel.cancel();
    let result = run.await;
    assert_eq!(
        result.transcript_path.as_deref(),
        Some(expected.as_path()),
        "the result publishes the writer's path"
    );
    assert!(
        result.transcript_error.is_none(),
        "no writer error: {:?}",
        result.transcript_error
    );
    assert_ne!(
        result.final_output.as_deref(),
        Some(AFTER_THE_SLEEP),
        "the child was torn down mid-sleep, never reaching its post-sleep emit"
    );
    let after = std::fs::read_to_string(&expected).expect("transcript still on disk");
    assert!(
        !after.contains(AFTER_THE_SLEEP),
        "the post-sleep record must never have landed: {after}"
    );
}

// =================================================================================================
// B. Background: `runner_main::run_with`
// =================================================================================================

/// The background path: the runner stamps `steps[0].transcript_path` at DECLARATION (before the
/// child spawns), its dispatched `run_sync` writes the same file live, and the terminal
/// `status.json` and `ResultFile` both carry the path after an interrupt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_background_transcript_is_stamped_at_declaration_and_grows_while_the_child_runs() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script_path = parked_child_script(dir.path());
    let art_dir = dir.path().join("artifacts");
    let bg = prepare_background_run(
        dir.path(),
        "livetx0002",
        &art_dir,
        ArtifactConfig::default(),
    )
    .await;
    let expected = artifact_paths(&art_dir, "livetx0002", "worker", Some(0)).transcript_path;

    let mut run = std::pin::pin!(run_with(
        &bg.cfg_path,
        &bg.run_paths,
        RunnerOverrides {
            spawn_command: Some(fixture_cmd(&script_path)),
            ..Default::default()
        },
    ));

    // (i) The declaration-time stamp: present on the FIRST `Running` status, which the runner
    // writes before dispatching the step (`runner_main/entry.rs`), i.e. before any record exists.
    let running = tokio::select! {
        biased;
        outcome = &mut run => panic!("the run settled before it was observed Running: {outcome:?}"),
        status = poll_until_running(&bg.run_paths) => status,
    };
    assert_eq!(
        running.steps[0].transcript_path.as_deref(),
        Some(expected.as_path()),
        "steps[0].transcript_path must be stamped at declaration: {:?}",
        running.steps
    );
    assert!(running.steps[0].transcript_error.is_none());

    // (ii) The live file, while the child is parked.
    let lines = tokio::select! {
        biased;
        outcome = &mut run => panic!("the run settled before the transcript was read: {outcome:?}"),
        lines = poll_lines(&expected, 3) => lines,
    };
    assert_mid_run_records(&lines, "async", "livetx0002");

    // (iii) Interrupt the still-running child through the control inbox, then read the terminal
    // surfaces.
    tokio::fs::create_dir_all(
        bg.run_paths
            .control_inbox
            .parent()
            .expect("control_inbox has a parent"),
    )
    .await
    .expect("mkdir control dir");
    let request = InterruptRequest::new("live-transcript-test", Some("stop".to_string()));
    write_atomic_json(&bg.run_paths.control_inbox, &request)
        .await
        .expect("write interrupt request");
    let started = std::time::Instant::now();
    run.await.expect("run() itself never returns Err");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the interrupt must tear the parked child down, not wait out its 30 s sleep"
    );

    let status = read_status(&bg.run_paths).await;
    assert_eq!(status.run_id, bg.run_id);
    assert_eq!(status.state, RunState::Paused, "{status:?}");
    assert_eq!(
        status.steps[0].status,
        StepState::Paused,
        "{:?}",
        status.steps
    );
    assert_eq!(
        status.steps[0].transcript_path.as_deref(),
        Some(expected.as_path()),
        "the terminal status keeps the transcript path"
    );
    assert!(status.steps[0].transcript_error.is_none());

    let result_file = read_terminal_result(&bg.run_paths, &status).await;
    assert_eq!(result_file.results.len(), 1);
    assert_eq!(
        result_file.results[0].transcript_path.as_deref(),
        Some(expected.as_path()),
        "the terminal ResultFile carries the transcript path: {:?}",
        result_file.results[0]
    );
    assert!(result_file.results[0].transcript_error.is_none());
    assert!(result_file.results[0].interrupted);
}

// =================================================================================================
// C. The gate: `transcript: None` / `include_transcript: false`
// =================================================================================================

/// Foreground with the switch off: the bundle is still minted, but no `_transcript.jsonl` is
/// created and the result publishes no path (`Some` means "a writer existed", not "the bundle
/// names a file").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_transcript_is_written_when_the_foreground_switch_is_off() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script_path = quick_child_script(dir.path());
    let art_dir = dir.path().join("artifacts");

    let mut opts = base_run_options(dir.path(), "fixture-model");
    opts.spawn_command = Some(fixture_cmd(&script_path));
    opts.artifacts_dir = Some(art_dir.clone());
    opts.run_id = Some(RunId::from_token("livetx0003"));
    opts.child_index = Some(0);
    opts.transcript = None;
    let agent = base_agent_config("fixture-model");
    let bundle = artifact_paths(&art_dir, "livetx0003", "worker", Some(0));

    let result = run_sync(&agent, TASK, &opts).await;
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(result.transcript_path, None);
    assert_eq!(result.transcript_error, None);
    assert_eq!(
        result.artifact_paths.as_ref().map(|p| &p.transcript_path),
        Some(&bundle.transcript_path),
        "the bundle still names the file; only the writer is absent"
    );
    assert!(
        !bundle.transcript_path.exists(),
        "no transcript file may be created with the switch off"
    );
}

/// Background with `include_transcript: false`: no declaration stamp, no file, no path on the
/// terminal surfaces — while the artifact quadruple's `_input.md` still lands under the same dir,
/// proving the transcript switch is independent of the artifacts switch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_transcript_is_written_when_the_background_config_excludes_it() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script_path = quick_child_script(dir.path());
    let art_dir = dir.path().join("artifacts");
    let bg = prepare_background_run(
        dir.path(),
        "livetx0004",
        &art_dir,
        ArtifactConfig {
            include_transcript: false,
            ..ArtifactConfig::default()
        },
    )
    .await;
    let bundle = artifact_paths(&art_dir, "livetx0004", "worker", Some(0));

    run_with(
        &bg.cfg_path,
        &bg.run_paths,
        RunnerOverrides {
            spawn_command: Some(fixture_cmd(&script_path)),
            ..Default::default()
        },
    )
    .await
    .expect("run() itself never returns Err");

    let status = read_status(&bg.run_paths).await;
    assert_eq!(status.state, RunState::Complete, "{status:?}");
    assert_eq!(status.steps[0].transcript_path, None, "{:?}", status.steps);
    assert_eq!(status.steps[0].transcript_error, None);
    let result_file = read_terminal_result(&bg.run_paths, &status).await;
    assert_eq!(result_file.results[0].transcript_path, None);
    assert!(
        !bundle.transcript_path.exists(),
        "no transcript file may be created with include_transcript: false"
    );
    assert!(
        bundle.input_path.exists(),
        "the artifact quadruple is still written: {}",
        bundle.input_path.display()
    );
}

// =================================================================================================
// D. The PRODUCTION foreground producer: `SubagentExecutor::run_foreground`
// =================================================================================================

/// The hop every real `subagent` tool call takes. `build_foreground_run_options`
/// (`extension/executor/foreground.rs`) turns the writer on with `transcript: (artifacts_enabled
/// && include_transcript).then_some(TranscriptSource::Foreground)`; tests A and C bypass that
/// gate by setting `RunOptions::transcript` themselves, so this is the one test that reaches it.
/// Mutate the gate to `None` and this fails: no path on the result, no file under the project
/// artifacts dir. The header record's `source` is asserted because the label is the foreground
/// producer's own contribution (pi `execution.ts:1843`), and the file is the SAME one the
/// FleetView pane and `/subagents status` mint (`artifact_paths(.., Some(0)).transcript_path`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_production_foreground_run_writes_a_transcript_labelled_foreground() {
    let work_dir = tempfile::tempdir().expect("real tempdir for the persona + cwd");
    let home_dir = tempfile::tempdir().expect("real tempdir to isolate the home roots");
    let script_path = quick_child_script(work_dir.path());

    let result =
        run_foreground_through_the_executor(work_dir.path(), home_dir.path(), &script_path).await;
    assert_eq!(result.exit_code, 0, "{result:?}");

    let path = result.transcript_path.clone().unwrap_or_else(|| {
        panic!(
            "the production foreground run must publish the writer's path — the producer gate \
             in `build_foreground_run_options` is off: {result:?}"
        )
    });
    assert!(
        result.transcript_error.is_none(),
        "no writer error: {:?}",
        result.transcript_error
    );
    let art_dir = project_artifacts_dir(work_dir.path());
    assert_eq!(
        path.parent(),
        Some(art_dir.as_path()),
        "the transcript lives beside the artifact quadruple"
    );
    let name = path.file_name().unwrap().to_str().unwrap();
    let run_token = name
        .strip_suffix("_worker_0_transcript.jsonl")
        .unwrap_or_else(|| panic!("the file is `<run>_worker_0_transcript.jsonl`: {name}"));
    assert_eq!(
        path,
        artifact_paths(&art_dir, run_token, "worker", Some(0)).transcript_path,
        "the same quadruple the FleetView pane and `/subagents status` mint"
    );

    let text = std::fs::read_to_string(&path).expect("the transcript exists on disk");
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    for line in &lines {
        assert!(
            !line.contains(TASK),
            "the transcript must never carry the prompt: {line}"
        );
    }
    let records = parse_records(&lines);
    assert!(records.len() >= 2, "{records:#?}");
    let header = &records[0];
    assert_eq!(header["source"], "foreground", "{header}");
    assert_eq!(header["sourceEventType"], "initial_prompt", "{header}");
    assert_eq!(header["role"], "user", "{header}");
    assert_eq!(header["text"], INITIAL_PROMPT_SENTINEL, "{header}");
    assert_eq!(header["runId"], run_token, "{header}");
    assert_eq!(header["agent"], "worker", "{header}");
    assert_eq!(header["childIndex"], 0, "{header}");
    assert!(
        records
            .iter()
            .any(|r| r["role"] == "assistant" && r["text"] == "done"),
        "the child's own message follows the sentinel: {records:#?}"
    );
}

// =================================================================================================
// E. DoD #4 — a writer error SURFACES on the result, and the run still completes
// =================================================================================================

/// The foreground production path with the project artifacts root pre-created as a regular
/// FILE: every best-effort quadruple write fails silently and the writer cannot initialise
/// (`mkdir -p` of a file). The run must still complete (a transcript failure never fails the
/// child) and `SingleResult::transcript_error` must carry the writer's latched error in
/// upstream's own sentence (pi `transcriptError: transcriptWriter?.getError()`,
/// `execution.ts:1967`), beside the path it would have written (`:167-172`: the path stays
/// published). Mutate `run_sync`'s `transcript_error:` projection to `None` and this fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_error_surfaces_on_the_foreground_result_and_the_run_still_completes() {
    let work_dir = tempfile::tempdir().expect("real tempdir for the persona + cwd");
    let home_dir = tempfile::tempdir().expect("real tempdir to isolate the home roots");
    let script_path = quick_child_script(work_dir.path());
    let art_dir = project_artifacts_dir(work_dir.path());
    std::fs::create_dir_all(art_dir.parent().expect("artifacts dir has a parent"))
        .expect("mkdir the artifacts root's parent");
    std::fs::write(
        &art_dir,
        b"a regular file where the artifacts dir should be",
    )
    .expect("block the artifacts dir with a file");

    let result =
        run_foreground_through_the_executor(work_dir.path(), home_dir.path(), &script_path).await;
    assert_eq!(
        result.exit_code, 0,
        "a transcript failure must never fail the child: {result:?}"
    );
    assert!(
        result
            .final_output
            .as_deref()
            .unwrap_or("")
            .contains("done"),
        "the child's output still arrives: {:?}",
        result.final_output
    );
    let error = result.transcript_error.clone().unwrap_or_else(|| {
        panic!("the writer's initialisation failure must surface on the result: {result:?}")
    });
    assert!(
        error.starts_with("Failed to initialize child transcript '"),
        "upstream's sentence: {error}"
    );
    assert!(error.contains("_worker_0_transcript.jsonl"), "{error}");
    let path = result
        .transcript_path
        .as_ref()
        .expect("the path stays published beside the error");
    assert_eq!(path.parent(), Some(art_dir.as_path()));
    assert!(!path.exists(), "nothing could be written under a file");
}

/// The background path with the artifacts root beneath a regular FILE: the declaration stamp
/// names the file, the dispatched `run_sync` cannot initialise the writer, and the latched error
/// must cross every hop to BOTH terminal surfaces — `status.json`'s step (`record_step_outcome`,
/// pi `:3829-3830`) and the `ResultFile`'s result (`step_result_to_single_result_with`, pi
/// `:1590-1591`) — while the run completes. Dropping `build_step_result`'s carry,
/// `record_step_outcome`'s assignment or the `ResultFile` projection each fails this.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_error_surfaces_on_the_background_status_and_result_file() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let script_path = quick_child_script(dir.path());
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"a regular file").expect("write the blocker");
    let art_dir = blocker.join("artifacts");
    let bg = prepare_background_run(
        dir.path(),
        "livetx0005",
        &art_dir,
        ArtifactConfig::default(),
    )
    .await;
    let expected = artifact_paths(&art_dir, "livetx0005", "worker", Some(0)).transcript_path;

    run_with(
        &bg.cfg_path,
        &bg.run_paths,
        RunnerOverrides {
            spawn_command: Some(fixture_cmd(&script_path)),
            ..Default::default()
        },
    )
    .await
    .expect("run() itself never returns Err");

    let status = read_status(&bg.run_paths).await;
    assert_eq!(
        status.state,
        RunState::Complete,
        "a transcript failure must never fail the run: {status:?}"
    );
    assert_eq!(
        status.steps[0].status,
        StepState::Complete,
        "{:?}",
        status.steps
    );
    assert_eq!(
        status.steps[0].transcript_path.as_deref(),
        Some(expected.as_path()),
        "the declaration stamp stays published beside the error"
    );
    let status_error = status.steps[0].transcript_error.clone().unwrap_or_else(|| {
        panic!(
            "the writer's failure must reach status.json's step: {:?}",
            status.steps
        )
    });
    assert!(
        status_error.starts_with("Failed to initialize child transcript '"),
        "upstream's sentence: {status_error}"
    );
    assert!(
        status_error.contains("livetx0005_worker_0_transcript.jsonl"),
        "{status_error}"
    );

    let result_file = read_terminal_result(&bg.run_paths, &status).await;
    assert_eq!(result_file.results.len(), 1);
    assert_eq!(result_file.results[0].exit_code, 0);
    assert_eq!(
        result_file.results[0].transcript_path.as_deref(),
        Some(expected.as_path())
    );
    assert_eq!(
        result_file.results[0].transcript_error.as_deref(),
        Some(status_error.as_str()),
        "the ResultFile carries the SAME sentence: {:?}",
        result_file.results[0]
    );
    assert!(!expected.exists(), "nothing could be written under a file");
}

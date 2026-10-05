//! SUBA-128 — `checkpointBeforeDeadlineMs` end to end through the detached runner's own entry
//! (`background::runner_main::run_with`), with a REAL child process: a shell script standing in
//! for a child whose steer inbox (`CYRUP_SUBAGENT_STEER_INBOX`) receives the runner's
//! `deadline-checkpoint` request before the deadline kills it.
//!
//! Upstream: `subagent-runner.ts:3379-3406` @v0.71.0 (the checkpoint timer beside the deadline
//! timer, routed "like any external steer so its lifecycle records the receipt").

#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use crate::background::atomic::write_atomic_json;
use crate::background::runner_main::{RunnerConfig, RunnerOverrides, run_with};
use crate::background::{RunId, RunMode, RunPaths, RunState};
use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};

/// A child that waits for the first request in its steer inbox, keeps a copy, and answers; it
/// answers `no checkpoint` if none arrives within ten seconds.
fn child(dir: &Path) -> std::path::PathBuf {
    let script = dir.join("checkpoint-child.sh");
    let received = dir.join("received.json");
    let line = |text: &str| {
        serde_json::json!({
            "type": "message_end",
            "message": { "role": "assistant", "content": [{ "type": "text", "text": text }] },
        })
        .to_string()
    };
    let body = format!(
        "#!/bin/sh\n\
         printf '%s\\n' '{{\"type\":\"agent_start\"}}'\n\
         i=0\n\
         while [ $i -lt 100 ]; do\n\
         \x20 for f in \"$CYRUP_SUBAGENT_STEER_INBOX\"/*.json; do\n\
         \x20   if [ -f \"$f\" ]; then\n\
         \x20     cp \"$f\" '{received}'\n\
         \x20     printf '%s\\n' '{handed_off}'\n\
         \x20     printf '%s\\n' '{{\"type\":\"agent_settled\"}}'\n\
         \x20     exit 0\n\
         \x20   fi\n\
         \x20 done\n\
         \x20 sleep 0.1\n\
         \x20 i=$((i+1))\n\
         done\n\
         printf '%s\\n' '{none}'\n\
         exit 0\n",
        received = received.display(),
        handed_off = line("handed off"),
        none = line("no checkpoint"),
    );
    std::fs::write(&script, body).expect("write child");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

fn single_step() -> SingleStepSpec {
    SingleStepSpec {
        worktree: crate::spawn::worktree::WorktreeRequest::Shared,
        machine: None,
        skills: None,
        session_dir: None,
        agent: "helper".to_string(),
        task: "long work".to_string(),
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
        fast: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
        label: None,
        session_name: None,
    }
}

fn config(dir: &Path, run_id: &RunId, deadline_at_ms: u64) -> RunnerConfig {
    let mut helper = crate::discovery::management::test_support::sample_agent(
        crate::discovery::types::AgentSource::Project,
        dir.join("helper.md"),
    );
    helper.name = "helper".to_string();
    helper.local_name = "helper".to_string();
    helper.model = Some(cyrup_core::ModelId::from("m1"));
    let mut resolved_agents = BTreeMap::new();
    resolved_agents.insert(
        "helper".to_string(),
        crate::exec::resolve_step_agent_config(&helper),
    );
    RunnerConfig {
        model_response_aliases: None,
        runner_process_instance_id: None,
        revival_lease: None,
        usage_budget: None,
        turn_budget: None,
        permission_rules: None,
        timeout_ms: Some(4_000),
        deadline_at_ms: Some(deadline_at_ms),
        checkpoint_before_deadline_ms: Some(2_500),
        tool_timeout: Default::default(),
        share: None,
        artifacts_dir: None,
        artifact_config: crate::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step())],
        cwd: dir.to_path_buf(),
        session_file: None,
        session_id: Some("checkpoint-session".to_string()),
        completion_owner_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: dir.join("async"),
        results_dir: dir.join("results"),
        resolved_agents,
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        inherited_session_thinking: None,
        model_scope: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        control: None,
        include_progress: None,
    }
}

/// pi's "Verify": a run with `timeoutMs` and `checkpointBeforeDeadlineMs` receives the checkpoint
/// steer about `checkpointBeforeDeadlineMs` before the timeout, so the child can hand off and
/// finish instead of being killed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_child_receives_the_deadline_checkpoint_and_hands_off_before_the_kill() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_id = RunId::from_token("checkpointrun1");
    let run_paths = RunPaths::for_run(
        &dir.path().join("async"),
        &dir.path().join("results"),
        &run_id,
    );
    std::fs::create_dir_all(&run_paths.run_dir).expect("mkdir run dir");
    std::fs::create_dir_all(dir.path().join("results")).expect("mkdir results");
    let started = std::time::Instant::now();
    let deadline_at_ms = u64::try_from(crate::time::now_epoch_millis()).unwrap_or(0) + 4_000;
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config(dir.path(), &run_id, deadline_at_ms))
        .await
        .expect("write config");

    run_with(
        &cfg_path,
        &run_paths,
        RunnerOverrides {
            spawn_command: Some(crate::spawn::SpawnCommand {
                binary: child(dir.path()),
                base_args: Vec::new(),
            }),
            roots: Some(crate::paths::Roots::sandboxed(dir.path())),
            ..RunnerOverrides::default()
        },
    )
    .await
    .expect("run_with never returns Err");

    assert!(
        started.elapsed() < std::time::Duration::from_millis(3_900),
        "the child handed off before the deadline kill: {:?}",
        started.elapsed()
    );
    let received: crate::background::control::SteerRequest = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("received.json"))
            .expect("the child's inbox received a request"),
    )
    .expect("a steer request");
    assert_eq!(received.source.as_deref(), Some("deadline-checkpoint"));
    assert!(
        received
            .message
            .starts_with("Deadline checkpoint from the runner: this run is killed in about "),
        "{}",
        received.message
    );
    assert!(
        received.message.ends_with(
            "Finish the current tool call only, then stop and reply with a handoff: changed files, build/test state, remaining work, and commit/PR state. Do not start new work."
        ),
        "{}",
        received.message
    );

    let session = crate::identity::SessionId::parse("checkpoint-session").expect("non-empty");
    let result_path = run_paths
        .resolve_result(&session, &run_id)
        .await
        .expect("terminal result");
    let result: crate::background::ResultFile =
        serde_json::from_slice(&std::fs::read(result_path).expect("read result"))
            .expect("parse result");
    assert_eq!(result.state, RunState::Complete, "{result:?}");
    assert_eq!(
        result
            .results
            .first()
            .and_then(|r| r.final_output.as_deref()),
        Some("handed off")
    );
    let events = std::fs::read_to_string(&run_paths.events).expect("events.jsonl");
    assert!(
        events
            .lines()
            .any(|line| line.contains("subagent.steer.requested")
                && line.contains("deadline-checkpoint")),
        "the checkpoint's receipt is on the steering lifecycle: {events}"
    );
}

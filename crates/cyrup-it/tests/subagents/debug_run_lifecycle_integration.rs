//! `debug.run` — THE REACHABILITY PROOF for
//! `cyrup_ext_subagents::background::run_lifecycle_debug` (pi `run-status.ts:47-108` @v0.68.0),
//! driven end to end through the production dispatch over a run every datum of which is real:
//!
//! 1. `SubagentExecutor::spawn_background_steps` — the PRODUCTION claim path — acquires the
//!    session's active-capacity slot (`slot-0/owner.json`), spawns the detached hop-1 runner (the
//!    scripted fixture with no script, which exits 0 at once), and binds the runner's real pid onto
//!    the slot with `mark_started`.
//! 2. The runner's terminal `status.json` is written into the run directory the spawn created,
//!    carrying that same pid — the record the fixture-as-runner does not write itself.
//! 3. `{ action: "debug.run", id }` and `{ action: "debug.run", dir }` go through
//!    `SubagentTool::execute` → `route_action` → `route_control_action` →
//!    `SubagentExecutor::control_debug_run`, which reconciles the status, inspects the slot, probes
//!    the pid and renders the dump — and the assertions pin the slot, the pid and the liveness the
//!    production spawn produced, line by line.
//!
//! The dump prints the process-terminal proof this build HAS (the pid probe) and says the sidecar
//! is not recorded; the negative assertion at the end pins that it never names a
//! `process-terminal.json` no code path writes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_core::{CancelToken, Tool, ToolCallId};
use cyrup_ext::HostServices;
use cyrup_ext_subagents::background::active_async_capacity::{
    read_owner, session_pool_dir, slot_dir,
};
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::reconcile::{Liveness, check_pid_liveness};
use cyrup_ext_subagents::background::{
    RunId, RunMode, RunPaths, RunState, RunStatus, StepState, StepStatus,
    active_async_capacity_root_in, run_artifact_roots_in,
};
use cyrup_ext_subagents::extension::{BackgroundStepsSpec, SubagentsExtension};
use cyrup_ext_subagents::identity::SessionId;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};

/// The session every record in this test is attributed to: the host reports it, the capacity
/// pool is keyed by it, and the status file carries it.
const SESSION: &str = "debug-session";

/// A `HostServices` that reports a fixed session id — what makes `spawn_background_steps`'s
/// capacity gate apply (a session-less spawn belongs to no pool and claims nothing).
struct FixedSessionHost;

impl HostServices for FixedSessionHost {
    fn session_id(&self) -> Option<String> {
        Some(SESSION.to_string())
    }
}

fn worker_step() -> RunnerStep {
    RunnerStep::SingleStep(SingleStepSpec {
        skills: None,
        session_dir: None,
        agent: "worker".to_string(),
        task: "debug me".to_string(),
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
    })
}

fn background_spec(run_id: RunId) -> BackgroundStepsSpec {
    BackgroundStepsSpec {
        steps: vec![worker_step()],
        mode: RunMode::Single,
        session_file: None,
        resolved_agents: BTreeMap::new(),
        original_task: "debug me".to_string(),
        chain_dir: None,
        control: None,
        include_progress: None,
        run_id,
        timeout_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        turn_budget: None,
        permission_rules: None,
        usage_budget: None,
        transfer_from: None,
        thinking_ceiling: None,
        capability_ceiling: None,
        model_origin: None,
    }
}

/// Wait, bounded, until the kernel confirms `pid` is gone. The fixture exits at once, but the
/// exit is asynchronous to this process.
fn wait_until_dead(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if check_pid_liveness(pid) == Liveness::Dead {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "runner pid {pid} did not exit within 10s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The terminal record the detached runner writes when it finishes: `Queued` → `Running` →
/// `Complete`, attributed to `SESSION`, carrying the runner's own pid, with its one step complete.
async fn write_terminal_status(paths: &RunPaths, run_id: &RunId, pid: u32) {
    let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(pid));
    status
        .advance_state(RunState::Running)
        .expect("Queued -> Running");
    let mut step = StepStatus::pending("worker");
    step.status = StepState::Complete;
    status.steps = vec![step];
    status
        .advance_state(RunState::Complete)
        .expect("Running -> Complete");
    status.session_id = SessionId::parse(SESSION);
    write_atomic_json(&paths.status, &status)
        .await
        .expect("write the terminal status.json");
}

async fn debug_run(ext: &SubagentsExtension, params: serde_json::Value) -> String {
    let result = ext
        .subagent_tool()
        .execute(
            ToolCallId::from("debug-run"),
            params,
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect("debug.run answers for a real async run");
    result
        .content
        .iter()
        .filter_map(|c| match c {
            cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_line(text: &str, expected: &str) {
    assert!(
        text.lines().any(|line| line == expected),
        "expected the line {expected:?} in:\n{text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_run_prints_the_real_capacity_slot_for_a_real_async_run() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    let roots = Roots::sandboxed(home.path());

    let cfg = SubagentExtensionConfig {
        // No script: the fixture's env-fallback default exits 0 at once, so the hop-1 process is
        // genuinely spawned and confirmed, and its pid is genuinely dead by the time we ask.
        spawn_command: Some(SpawnCommand {
            binary: crate::support::bins::subagent_fixture(),
            base_args: Vec::new(),
        }),
        // The cap is what makes the spawn CLAIM a slot at all.
        max_active_async_runs_per_session: Some(1),
        roots: roots.clone(),
        ..SubagentExtensionConfig::default()
    };
    let ext = SubagentsExtension::with_config_and_cwd(cfg, cwd.path().to_path_buf());
    ext.executor().set_host_services(Arc::new(FixedSessionHost));

    // (1) The production claim path: `acquire` writes `slot-0/owner.json`, the spawn is confirmed,
    // `mark_started(pid)` binds the runner pid onto the slot.
    let run_id = RunId::new();
    let spawned = ext
        .executor()
        .spawn_background_steps(cwd.path(), background_spec(run_id.clone()))
        .await
        .expect("spawn_background_steps confirms the detached hop-1 spawn");
    assert_eq!(spawned, run_id);

    let session = SessionId::parse(SESSION).expect("a valid session id");
    let pool = session_pool_dir(&active_async_capacity_root_in(&roots), &session);
    let owner = read_owner(&slot_dir(&pool, 0))
        .await
        .expect("the spawn claimed slot 0 of this session's pool");
    assert_eq!(owner.run_id, run_id);
    let pid = owner
        .runner_pid
        .expect("mark_started bound the runner pid onto the slot");
    assert!(owner.runner_started_at.is_some());
    wait_until_dead(pid);

    // (2) The runner's terminal record, in the run dir the spawn created.
    let artifact_roots = run_artifact_roots_in(&roots, cwd.path());
    let paths = RunPaths::for_run(
        &artifact_roots.async_root,
        &artifact_roots.results_dir,
        &run_id,
    );
    assert!(
        paths.run_dir.is_dir(),
        "the spawn created {}",
        paths.run_dir.display()
    );
    write_terminal_status(&paths, &run_id, pid).await;

    // (3) The id form, through the production tool.
    let text = debug_run(
        &ext,
        serde_json::json!({ "action": "debug.run", "id": run_id.as_str() }),
    )
    .await;

    assert_eq!(text.lines().next(), Some("Run lifecycle debug"), "{text}");
    assert_line(&text, &format!("Run: {run_id}"));
    assert_line(&text, &format!("Dir: {}", paths.run_dir.display()));
    assert_line(&text, &format!("Status file: {}", paths.status.display()));
    assert_line(&text, &format!("Session: {SESSION}"));
    assert_line(&text, "State: complete");
    assert_line(&text, "Mode: single");
    assert!(
        text.lines()
            .any(|line| line.starts_with("Process terminal: not recorded")),
        "{text}"
    );
    assert_line(&text, &format!("Runner pid: {pid} (dead)"));
    // The proof rung's own sentence (`active_async_capacity/inspect.rs`, `runner_release_verdict`).
    assert_line(
        &text,
        &format!(
            "Active capacity: releasable — runner pid {pid} is confirmed gone and the run is terminal"
        ),
    );
    assert_line(
        &text,
        "Capacity owner: current slot 0, runner, generation 0",
    );
    assert_line(&text, &format!("Capacity session: {SESSION}"));
    assert_line(
        &text,
        &format!("Capacity async dir: {}", paths.run_dir.display()),
    );
    assert_line(&text, &format!("Capacity runner pid: {pid}"));
    assert!(
        text.lines()
            .any(|line| line.starts_with("Capacity runner started: 20")),
        "{text}"
    );
    assert!(
        !text
            .lines()
            .any(|line| line.starts_with("Process terminal file:")),
        "the dump must not name a sidecar this build never writes:\n{text}"
    );
    assert!(
        !text.contains("Workflow children:"),
        "a single-mode run has no workflow block:\n{text}"
    );

    // (4) The `dir` form (`run-status.ts:406-410`'s `resolveAsyncRunLocation` over `params.dir`)
    // reaches the same slot.
    let by_dir = debug_run(
        &ext,
        serde_json::json!({ "action": "debug.run", "dir": paths.run_dir.to_string_lossy() }),
    )
    .await;
    assert_line(
        &by_dir,
        "Capacity owner: current slot 0, runner, generation 0",
    );
    assert_line(&by_dir, &format!("Run: {run_id}"));
}

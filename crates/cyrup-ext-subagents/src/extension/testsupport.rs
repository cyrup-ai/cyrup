//! Shared fixtures for this module tree's unit tests: capturing sinks, stub host services and
//! the run/agent seeding helpers more than one submodule's tests need.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

// NOTE: the `CYRUP_HOME`-sandboxed tests that used to live in this module tree
// (`child_env_gate_controls_what_is_registered`, `top_level_with_optin_attaches_full`,
// `init_registers_the_tool_and_all_thirteen_commands`,
// `teardown_session_stops_the_tracker_and_clears_the_parent_session_anchor`) live in
// `tests/cyrup_home_env_sandboxed_tests.rs`: they need `std::env::set_var`/`remove_var`, which Rust
// requires `unsafe` for, and this crate's `src/lib.rs` is `#![forbid(unsafe_code)]` — see that
// file's module doc for the full rationale (matches every other `tests/*_integration.rs` file's
// identical env-mutation convention in this crate).

use super::*;
use cyrup_core::Tool;
use crate::background::RunId;
use crate::background::RunMode;
use crate::background::RunPaths;
use crate::background::RunState;
use crate::background::StepState;
use crate::extension::executor::paths::default_async_root;
use crate::extension::executor::paths::default_results_dir;
use cyrup_core::CancelToken;
use cyrup_core::ToolCallId;
use cyrup_core::ToolError;
use cyrup_core::ToolResult;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// A [`SingleStepSpec`](crate::spawn::chain_graph::SingleStepSpec) with nothing set beyond the
/// agent + task the lowered slash graphs in `executor::spawn_budget` and `host` need, so the
/// spawn-budget assertions stay about the COUNT and not about step configuration.
pub(crate) fn bare_single_step(agent: &str, task: &str) -> crate::spawn::chain_graph::SingleStepSpec {
    crate::spawn::chain_graph::SingleStepSpec {
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

/// A minimal [`cyrup_ext::host::HostServices`] double reporting a canned live session id/file — the
/// analog of the `FixedModelHost` double in `executor`'s own tests, for proving `run_doctor` reads
/// the SAME live handle [`SubagentExecutor::resolve_context`] already uses (P-1) instead of a
/// per-cwd mtime guess.
pub(crate) struct FixedSessionIdHost {
    pub(crate) id: Option<String>,
    pub(crate) file: Option<PathBuf>,
}

impl cyrup_ext::host::HostServices for FixedSessionIdHost {
    fn session_id(&self) -> Option<String> {
        self.id.clone()
    }
    fn session_file(&self) -> Option<PathBuf> {
        self.file.clone()
    }
}

// ---------------------------------------------------------------------------------------
// SUBA-057 — `action: "dismiss"` (pi `dismissRecoveredWorkflow`,
// `runs/foreground/async-dismiss-action.ts` @v0.47.1).
//
// The READ half of this feature (the `displayDismissedAt` field, the reconciler's
// `DisplayDismissed` action, `list_active_runs`'s `continue`, the `State: display-dismissed`
// report) landed with the field. This block covers the half that was missing: the only PRODUCER
// of the marker, plus the verb that reaches it.
// ---------------------------------------------------------------------------------------

/// A [`cyrup_ext::host::HostServices`] that reports a fixed session id, so
/// [`SubagentExecutor::current_session_id`] answers the value `dismiss`'s third refusal
/// compares against (pi `state.currentSessionId`).
pub(crate) struct FixedSessionHost(pub(crate) &'static str);

impl cyrup_ext::host::HostServices for FixedSessionHost {
    fn session_id(&self) -> Option<String> {
        Some(self.0.to_string())
    }
}

/// Seed a run that looks exactly like pi's reload-orphaned workflow: `status.json` still claims
/// `Running`, it is attributed to `session`, and it carries `pid` — `None` for the orphaned
/// case (nothing left to probe, cyrup's analogue of "no live controller"), or a real live pid
/// for the still-controlled case.
///
/// A pid-less `Running` record is precisely the run reconciliation can never advance:
/// `reconcile`'s step 3 (`background/reconcile.rs`) falls through to `NoneNeeded` for
/// `(Running, None)`, so before this change such a run stayed in `/subagents-fleet` and in
/// `{action:"status"}` forever with no supported way to clear it.
pub(crate) fn seed_orphaned_run(cwd: &Path, run_id: &str, session: Option<&str>, pid: Option<u32>) -> RunPaths {
    let async_root = default_async_root(cwd);
    let results_dir = default_results_dir(cwd);
    let id = RunId::from_token(run_id.to_string());
    let paths = RunPaths::for_run(&async_root, &results_dir, &id);
    std::fs::create_dir_all(&paths.run_dir).expect("mkdir run dir");
    let mut status = crate::background::RunStatus::queued(id, RunMode::Chain, pid);
    status.state = RunState::Running;
    status.session_id = session.map(str::to_string);
    let mut step = crate::background::StepStatus::pending("builder");
    step.status = StepState::Running;
    status.steps = vec![step];
    std::fs::write(&paths.status, serde_json::to_string(&status).expect("serialize status"))
        .expect("write status.json");
    paths
}

// ---------------------------------------------------------------------------------------
// G90 (`action: "steer"`) + G92 (`view`/`lines`, `/subagents-fleet`) — driven end to end
// through the USER-REACHABLE surfaces: the `subagent` tool call and the slash command.
// ---------------------------------------------------------------------------------------

/// Write a `Running` `status.json` for one run under `cwd`'s async root, returning its
/// [`RunPaths`]. This is the on-disk state a real detached runner would have produced by the time a
/// user types `subagent({ action: "status" | "steer", … })`, so every test built on it drives the
/// same code path a live run does.
pub(crate) fn seed_running_run(cwd: &Path, run_id: &str, agents: &[&str]) -> RunPaths {
    let async_root = default_async_root(cwd);
    let results_dir = default_results_dir(cwd);
    let id = RunId::from_token(run_id.to_string());
    let paths = RunPaths::for_run(&async_root, &results_dir, &id);
    std::fs::create_dir_all(&paths.run_dir).expect("mkdir run dir");
    let mut status = crate::background::RunStatus::queued(
        id,
        if agents.len() > 1 { RunMode::Parallel } else { RunMode::Single },
        // A pid this process can definitely signal, so R-SA-079's reconciliation gate does not
        // rewrite `Running` to a dead-pid terminal state before the report is even rendered.
        Some(std::process::id()),
    );
    status.state = RunState::Running;
    status.current_step = Some(0);
    status.steps = agents
        .iter()
        .enumerate()
        .map(|(i, agent)| {
            let mut step = crate::background::StepStatus::pending(*agent);
            if i == 0 {
                step.status = StepState::Running;
            }
            step
        })
        .collect();
    std::fs::write(
        &paths.status,
        serde_json::to_string(&status).expect("serialize status"),
    )
    .expect("write status.json");
    paths
}

/// The GLOBAL mission pointer index defaults to `agent_dir()/missions/index` — the
/// developer's real `~/.cyrup/agent` (faithful to pi `missions/store.ts:265`). A test that
/// creates a mission, directly or through the tool, MUST scope it into its own tempdir; the
/// only production lever for that is `config.missions.globalIndexDir`.
///
/// There are TWO ways a test reaches that launch path, and both need scoping:
///
/// * a bare [`SubagentTool`] over its own executor — use [`scoped_tool`], which arms this
///   config on the executor's live config for you;
/// * a [`SubagentsExtension`] built from a [`SubagentExtensionConfig`] literal, whose
///   `subagent_tool()` inherits that config verbatim — set `missions: Some(scoped_missions(…))`
///   in the literal. This second route is NOT covered by [`scoped_tool`] and is what leaked
///   `~/.cyrup/agent/missions/index` pointers titled `"a"`/`"c"` from the three spawn-budget /
///   chain-billing tests: their dispatches carry a `task`, and a task-bearing dispatch
///   auto-creates a mission (`missions/lifecycle.rs::prepare_mission_launch`) whether or not
///   the test mentions missions at all.
pub(crate) fn scoped_missions(root: &Path) -> crate::missions::MissionStoreConfig {
    crate::missions::MissionStoreConfig {
        global_index_dir: Some(
            root.join("agent").join("missions").join("index").to_string_lossy().into_owned(),
        ),
        ..Default::default()
    }
}

/// Install [`scoped_missions`] on the executor's live config, which is what the `subagent`
/// tool's launch path reads (`extension.rs`'s `cfg.missions`).
pub(crate) async fn arm_scoped_missions(executor: &SubagentExecutor, root: &Path) {
    executor.config_cell().lock().await.missions = Some(scoped_missions(root));
}

/// A [`SubagentTool`] over a fresh executor whose mission pointer index is scoped into `dir`
/// ([`scoped_missions`]). Missions are ON by default, so ANY dispatch carrying a `task`
/// auto-creates one (`missions/lifecycle.rs::prepare_mission_launch`) and writes a pointer to
/// `agent_dir()/missions/index` — the developer's real `~/.cyrup/agent`. This is therefore the
/// constructor every dispatch test must use, not `SubagentTool::new` directly.
pub(crate) async fn scoped_tool(dir: &Path) -> SubagentTool {
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir).await;
    SubagentTool::new(executor, dir.to_path_buf())
}

pub(crate) async fn dispatch_tool(
    tool: &SubagentTool,
    params: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    tool.execute(
        ToolCallId::from("t"),
        params,
        CancelToken::new(),
        Box::new(|_u: cyrup_core::ToolUpdate| {}),
    )
    .await
}

pub(crate) fn tool_text(result: &ToolResult) -> String {
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

// ---------------------------------------------------------------------------------------
// The two "Mission tracking unavailable" degradation paths (pi `subagent-executor.ts`
// `:5101-5111` pre-launch and `:5115-5128` post-launch). Both were entirely uncovered.
// ---------------------------------------------------------------------------------------

/// A [`crate::missions::MissionStoreConfig`] whose global index lands INSIDE `root`, so no test
/// here ever writes to the real agent dir under `$HOME`.
pub(crate) fn scoped_mission_config(root: &Path) -> crate::missions::MissionStoreConfig {
    crate::missions::MissionStoreConfig {
        global_index_dir: Some(
            root.join("agent").join("missions").join("index").to_string_lossy().into_owned(),
        ),
        ..Default::default()
    }
}

// =====================================================================================
// Tier-2 (a): fork default-mode + per-index branch (`apply_fork_contexts`).
// =====================================================================================

pub(crate) fn fork_user_msg(text: &str) -> cyrup_core::Message {
    cyrup_core::Message::User {
        content: vec![cyrup_core::Content::text(text)],
        timestamp: 0,
    }
}

pub(crate) fn fork_assistant_msg(text: &str) -> cyrup_core::Message {
    cyrup_core::Message::Assistant(cyrup_core::AssistantMessage {
        content: vec![cyrup_core::Content::text(text)],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: cyrup_core::Usage::default(),
        stop_reason: cyrup_core::StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    })
}

// =========================================================================================
// SUBA-003: `subagents.modelScope` enforcement
// (pi `runs/shared/model-scope.ts` + `model-fallback.ts:200-212`)
// =========================================================================================

/// Seed a cwd with one discoverable agent and (optionally) a `subagents` settings block.
pub(crate) fn seed_scope_fixture(cwd: &Path, agent: &str, settings_json: Option<&str>) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir agents dir");
    std::fs::write(
        agents_dir.join(format!("{agent}.md")),
        format!("---\nname: {agent}\ndescription: Model-scope fixture agent\n---\nBody.\n"),
    )
    .expect("write agent fixture");
    if let Some(json) = settings_json {
        std::fs::write(agents_dir.join("settings.json"), json).expect("write settings.json");
    }
}

//! `children.list` — THE REACHABILITY PROOF for
//! [`crate::background::retained_children::list_retained_children`] and its predicate, driven
//! end to end through the production dispatch: a real `workflowScript` launches a real child
//! through `WorkflowRunHost::launch` → `run_foreground_streaming`; the child settles; the
//! workflow's terminal write persists it as a step row of the workflow's own `status.json`
//! (`workflow_step_statuses`, now carrying `sessionFile`); and `{ action: "children.list" }`
//! reads that row back off the FILESYSTEM through `collect_async_runs_by_scan` and answers with
//! the real resumability verdict.
//!
//! Nothing here touches the listing directly, and no in-memory record can satisfy it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;
use crate::background::{
    LaunchInputs, RecoveryDescriptor, RunDir, RunId, RunMode, runner_main::RunnerConfig,
};
use crate::extension::SubagentExecutor;
use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
use crate::extension::testsupport::{
    FixedSessionHost, arm_scoped_missions, bare_single_step, dispatch_tool, tool_text,
    write_completing_child_binary, write_failing_child_binary,
};
use crate::spawn::chain_graph::RunnerStep;
use std::path::Path;
use std::sync::Arc;

/// pi's empty-listing sentence (`retained-children.ts:111`).
const NO_RETAINED_CHILDREN: &str = "No retained workflow children in the active parent session. \
                                    If a retained-writer challenge is required, launch a same-role \
                                    fallback challenge and label it as fallback.";
/// pi's no-resumable trailer (`:133`).
const NO_RESUMABLE_TRAILER: &str = "No resumable retained child is listed. Launch a same-role \
                                    fallback challenge and label it as fallback.";

/// Write a recovery descriptor for `(workflow run, agent)` at the workflow run's directory with
/// the PRODUCTION writer — `RecoveryDescriptor::for_single_launch` over a resolved persona —
/// exactly as the async SINGLE launch does (`extension/executor/background.rs`). The verb
/// fabricates nothing; this is the fixture that stands in for the per-child descriptor location
/// a workflow launch does not have yet (residual R1).
///
/// `descriptor_cwd` is the cwd the descriptor records (`RunnerConfig::cwd`), which the listing's
/// `:66-72` rung stats; `cwd` is where the persona resolves from.
async fn write_descriptor_with_the_production_writer(
    executor: &SubagentExecutor,
    cwd: &Path,
    descriptor_cwd: &Path,
    async_root: &Path,
    results_dir: &Path,
    workflow_run_id: &str,
    agent: &str,
) {
    let cfg = executor.config_snapshot().await;
    let personas = executor
        .resolve_plan_personas(
            cwd,
            [agent.to_string()],
            crate::discovery::types::AgentReadScope::Both,
            &cfg.roots,
        )
        .expect("the builtin persona resolves");
    let runner_config = RunnerConfig {
        run_id: RunId::from_token(workflow_run_id.to_string()),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(bare_single_step(agent, "T"))],
        cwd: descriptor_cwd.to_path_buf(),
        session_file: None,
        session_id: Some("session-children".to_string()),
        completion_owner_id: None,
        global_concurrency_limit: 4,
        worktree_base_dir: None,
        max_subagent_depth: 4,
        async_root: async_root.to_path_buf(),
        results_dir: results_dir.to_path_buf(),
        resolved_agents: personas,
        original_task: "T".to_string(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        inherited_session_thinking: None,
        host_available_builtins: None,
        turn_budget: None,
        permission_rules: None,
        usage_budget: None,
        model_scope: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        control: None,
        include_progress: None,
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: crate::artifacts::ArtifactConfig::default(),
    };
    let descriptor = RecoveryDescriptor::for_single_launch(
        &runner_config,
        LaunchInputs {
            thinking_ceiling: None,
            capability_ceiling: None,
            stored_model_origin: None,
        },
    )
    .expect("a single-step Single launch writes a descriptor");
    descriptor
        .write(
            &RunDir::new(async_root, &RunId::from_token(workflow_run_id.to_string()))
                .recovery_descriptor(),
        )
        .await
        .expect("the descriptor persists");
}

/// The reachability test, five cases over ONE settled workflow child.
///
/// Gutted, in turn: stub `list_retained_children` to `Ok(vec![])` → case A's row assertion fails
/// on the empty sentence; drop `workflow_step_statuses`'s `sessionFile` fill → the `status.json`
/// pin fails and case A's reason becomes `no persisted session file`; drop the descriptor rung →
/// case A reads `resumable`; drop `assert_belongs_to` → the agent-mismatch sub-case of B reads
/// `resumable`; revert the resume hint to the child id → case B's hint line fails; revert the
/// candidate source to the indexed fleet history → case A′ answers the empty sentence; drop
/// `carry_step_settle_times` → the `endedAt` pin fails; collapse the cwd rung's ENOENT arm →
/// case D prints `required cwd is missing`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn children_list_lists_a_settled_workflow_child_with_the_real_predicate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_completing_child_binary(dir.path());

    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    // pi `resolveCurrentSessionId(ctx.sessionManager)` (`subagent-executor.ts:6468`): the
    // listing is scoped to the session that owns the workflow, and the workflow's `status.json`
    // is stamped with it, so a session-less host would list nothing.
    executor.set_host_services(Arc::new(FixedSessionHost("session-children")));
    {
        let mut cfg = executor.config_cell().lock().await;
        cfg.spawn_command = Some(crate::spawn::SpawnCommand {
            binary: script,
            base_args: Vec::new(),
        });
    }
    let tool = SubagentTool::new(executor.clone(), dir.path().to_path_buf());

    // A real, regular `.jsonl` where the child's session resolves: an explicit `sessionDir` is
    // used AS-IS (`tool/task_items.rs::resolve_single_run_session_root`), `resolve_run_channels`
    // appends the `run-0` leaf (`foreground.rs`), and `share: true` makes
    // `resolve_result_session_file` (`exec/mod.rs`) pick the newest `.jsonl` in that directory
    // as `SingleResult::session_file` — which `workflow_step_statuses` now copies onto the row.
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(sessions.join("run-0")).expect("mkdir sessions/run-0");
    let session_file = sessions.join("run-0").join("child.jsonl");
    std::fs::write(&session_file, "{}\n").expect("plant the session file");
    let sessions_json = serde_json::to_string(&sessions.to_string_lossy()).unwrap();
    let script_text = format!(
        "await runs.run(\"a\", {{ agent: \"worker\", task: \"T\", model: \"sonnet\", sessionDir: \
         {sessions_json}, share: true }});\nreturn \"done\";"
    );
    let result = dispatch_tool(&tool, serde_json::json!({ "workflowScript": script_text }))
        .await
        .expect("a completing child must not fail the workflow");
    let workflow_run_id = result.details.as_ref().expect("details")["workflowRunId"]
        .as_str()
        .expect("the settlement stamps the run id")
        .to_string();

    // The SAME roots arithmetic the dispatch used, so this reads the tree the run really wrote to.
    let cfg = executor.config_snapshot().await;
    let async_root = default_async_root_in(&cfg.roots, dir.path());
    let results_dir = default_results_dir_in(&cfg.roots, dir.path());
    let status_path = async_root.join(&workflow_run_id).join("status.json");
    let status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&status_path).expect("the workflow's own status.json is on disk"),
    )
    .unwrap();
    // The `workflow_step_statuses` fix's own pin: the settled row carries the session file.
    assert_eq!(
        status["steps"][0]["sessionFile"],
        serde_json::json!(session_file.to_string_lossy()),
        "the terminal write must persist the child's session file onto its step row: {status}"
    );
    let child_run_id = status["steps"][0]["runId"]
        .as_str()
        .expect("the settled row carries the child's run id")
        .to_string();
    assert_ne!(child_run_id, workflow_run_id);
    // pi `:90` — `completedAt` is the CHILD's `endedAt`: the step row's own, stamped by
    // `carry_step_settle_times` when the child settled, not the workflow's.
    let completed_at = status["steps"][0]["endedAt"]
        .as_i64()
        .expect("the settled row carries the child's own endedAt");
    assert!(
        completed_at
            <= status["endedAt"]
                .as_i64()
                .expect("a settled workflow carries endedAt"),
        "the child settled no later than its workflow: {status}"
    );
    let completed_iso = crate::time::format_iso8601_millis(completed_at);

    // ---- Case A: the predicate is live and the descriptor is absent ------------------------
    let result = dispatch_tool(&tool, serde_json::json!({ "action": "children.list" }))
        .await
        .expect("children.list is a read and dispatches");
    assert_eq!(
        result.details,
        Some(serde_json::json!({ "mode": "management", "results": [] })),
        "pi `subagent-executor.ts:6471` — a management reply"
    );
    let text = tool_text(&result);
    assert_ne!(
        text, NO_RETAINED_CHILDREN,
        "the settled child must be listed"
    );
    assert!(
        text.starts_with(
            "Retained workflow children (up to 10; newest first, with a resumable child retained \
             when available):\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "- {child_run_id} | worker | complete | {completed_iso}"
        )),
        "the row must name the CHILD's id, agent, and the child's own state and endedAt (pi \
         `:97` `run.state`, `:90` `run.endedAt`, `run` being the child): {text}"
    );
    assert!(
        text.contains(&format!("\n  workflow: {workflow_run_id} (a)")),
        "{text}"
    );
    assert!(text.contains("\n  task: (no task summary)"), "{text}");
    assert!(
        text.contains("\n  resumability: not resumable (missing recovery descriptor)"),
        "with a real session file and no descriptor the verdict is the descriptor rung's, pi's \
         `missing recovery descriptor` (`resume` itself refuses the same absence with \
         `RecoveryDescriptorError::Missing`'s own sentence): {text}"
    );
    assert!(text.ends_with(NO_RESUMABLE_TRAILER), "{text}");
    assert!(!text.contains("  session: "), "{text}");

    // ---- Case A′: the run indexes are non-empty and the child is STILL listed ---------------
    // A foreground workflow run is in neither run index, so a candidate source that reads the
    // indexes whenever they are non-empty (`collect_fleet_history`) would hide every workflow
    // status file the moment any async single run of this session had settled. Stage exactly
    // that with the production index writer.
    {
        let single = RunId::from_token("single-settled");
        let mut status =
            crate::background::RunStatus::queued(single.clone(), RunMode::Single, None);
        status.session_id = crate::identity::SessionId::parse_opt(Some("session-children"));
        status
            .advance_state(crate::background::RunState::Running)
            .unwrap();
        status
            .advance_state(crate::background::RunState::Complete)
            .unwrap();
        let paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &single);
        std::fs::create_dir_all(&paths.run_dir).unwrap();
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .unwrap();
        crate::background::active_run_index::update_active_run_index(&paths.run_dir, &status)
            .await
            .expect("the production index writer files the terminal single run");
    }
    let text = tool_text(
        &dispatch_tool(&tool, serde_json::json!({ "action": "children.list" }))
            .await
            .expect("children.list dispatches"),
    );
    assert!(
        text.contains(&format!(
            "- {child_run_id} | worker | complete | {completed_iso}"
        )),
        "a settled async single run in the indexes must not hide the workflow's retained \
         child: {text}"
    );

    // ---- Case B: a descriptor written by the production writer makes the child resumable ---
    write_descriptor_with_the_production_writer(
        &executor,
        dir.path(),
        dir.path(),
        &async_root,
        &results_dir,
        &workflow_run_id,
        "worker",
    )
    .await;
    let text = tool_text(
        &dispatch_tool(&tool, serde_json::json!({ "action": "children.list" }))
            .await
            .expect("children.list dispatches"),
    );
    assert!(text.contains("\n  resumability: resumable\n"), "{text}");
    assert!(
        text.contains(&format!("\n  session: {}\n", session_file.display())),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "\n  resume: subagent({{ action: \"resume\", id: \"{workflow_run_id}\", index: 0, \
             message: \"...\" }})"
        )),
        "the resume hint addresses the child as (workflow id, step index), which is how `resume` \
         reaches a workflow child here: {text}"
    );
    assert!(
        !text.contains(NO_RESUMABLE_TRAILER),
        "a resumable child in the window suppresses the fallback trailer: {text}"
    );

    // B′: a descriptor for ANOTHER agent at the same slot is refused by name — the
    // `assert_belongs_to` rung, not a generic "invalid".
    write_descriptor_with_the_production_writer(
        &executor,
        dir.path(),
        dir.path(),
        &async_root,
        &results_dir,
        &workflow_run_id,
        "reviewer",
    )
    .await;
    let text = tool_text(
        &dispatch_tool(&tool, serde_json::json!({ "action": "children.list" }))
            .await
            .expect("children.list dispatches"),
    );
    assert!(
        text.contains(
            "\n  resumability: not resumable (recovery descriptor belongs to agent reviewer)"
        ),
        "{text}"
    );

    // ---- Case D: the descriptor's cwd — pi `:66-72` — deleted, then a file in its place ------
    let descriptor_cwd = dir.path().join("child-cwd");
    std::fs::create_dir_all(&descriptor_cwd).expect("mkdir child-cwd");
    write_descriptor_with_the_production_writer(
        &executor,
        dir.path(),
        &descriptor_cwd,
        &async_root,
        &results_dir,
        &workflow_run_id,
        "worker",
    )
    .await;
    let text = tool_text(
        &dispatch_tool(&tool, serde_json::json!({ "action": "children.list" }))
            .await
            .expect("children.list dispatches"),
    );
    assert!(
        text.contains("\n  resumability: resumable\n"),
        "an existing directory passes the cwd rung: {text}"
    );
    std::fs::remove_dir(&descriptor_cwd).expect("rm child-cwd");
    let text = tool_text(
        &dispatch_tool(&tool, serde_json::json!({ "action": "children.list" }))
            .await
            .expect("children.list dispatches"),
    );
    assert!(
        text.contains(&format!(
            "\n  resumability: not resumable (resume dependency unavailable: ENOENT: no such \
             file or directory, stat '{}')",
            descriptor_cwd.display()
        )),
        "a deleted cwd is upstream's `catch` at `:70-71` — `statSync` throws ENOENT — not the \
         `required cwd is missing` sentence: {text}"
    );
    std::fs::write(&descriptor_cwd, "not a directory").expect("plant a file at the cwd");
    let text = tool_text(
        &dispatch_tool(&tool, serde_json::json!({ "action": "children.list" }))
            .await
            .expect("children.list dispatches"),
    );
    assert!(
        text.contains(&format!(
            "\n  resumability: not resumable (required cwd is missing: {})",
            descriptor_cwd.display()
        )),
        "a path that exists but is not a directory is `:69`'s own sentence: {text}"
    );

    // ---- Case C: the session file vanishes → the session rung answers first -----------------
    std::fs::remove_file(&session_file).expect("delete the session file");
    let text = tool_text(
        &dispatch_tool(&tool, serde_json::json!({ "action": "children.list" }))
            .await
            .expect("children.list dispatches"),
    );
    assert!(
        text.contains(&format!(
            "\n  resumability: not resumable (persisted session file is missing: {})",
            session_file.display()
        )),
        "{text}"
    );
    assert!(text.ends_with(NO_RESUMABLE_TRAILER), "{text}");
}

/// pi `:97` — the row's `state` is the CHILD's (`run` at `:85-104` is the child async run), so
/// a child that FAILED under a workflow that caught the failure and completed prints `failed`,
/// not the workflow's `complete`.
///
/// Gutted: read the workflow's `state` for the row → the row prints `complete`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn children_list_prints_the_childs_own_state_under_a_completed_workflow() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_failing_child_binary(dir.path());
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    executor.set_host_services(Arc::new(FixedSessionHost("session-children")));
    {
        let mut cfg = executor.config_cell().lock().await;
        cfg.spawn_command = Some(crate::spawn::SpawnCommand {
            binary: script,
            base_args: Vec::new(),
        });
    }
    let tool = SubagentTool::new(executor.clone(), dir.path().to_path_buf());

    // `runs.run` rejects on a failed child; the script catches it and completes.
    let result = dispatch_tool(
        &tool,
        serde_json::json!({
            "workflowScript":
                "try { await runs.run(\"a\", { agent: \"worker\", task: \"T\", model: \
                 \"sonnet\" }); } catch (e) {}\nreturn \"done\";"
        }),
    )
    .await
    .expect("a caught child failure does not fail the workflow");
    let workflow_run_id = result.details.as_ref().expect("details")["workflowRunId"]
        .as_str()
        .expect("the settlement stamps the run id")
        .to_string();
    let cfg = executor.config_snapshot().await;
    let status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            default_async_root_in(&cfg.roots, dir.path())
                .join(&workflow_run_id)
                .join("status.json"),
        )
        .expect("the workflow's status.json is on disk"),
    )
    .unwrap();
    assert_eq!(status["state"], "complete", "{status}");
    assert_eq!(status["steps"][0]["status"], "failed", "{status}");
    let child_run_id = status["steps"][0]["runId"]
        .as_str()
        .expect("the failed row carries the child's run id")
        .to_string();

    let text = tool_text(
        &dispatch_tool(&tool, serde_json::json!({ "action": "children.list" }))
            .await
            .expect("children.list dispatches"),
    );
    assert!(
        text.contains(&format!("- {child_run_id} | worker | failed | ")),
        "the row prints the child's own state: {text}"
    );
    assert!(
        !text.contains(&format!("- {child_run_id} | worker | complete | ")),
        "{text}"
    );
}

/// The advertise-vs-dispatch invariant, plus the child-safe gate: `children.list` sits at pi's
/// own index (`shared/types.ts:2801` @v0.68.0: `… "models", "children.list", "guide", …`), and a
/// tool built with `allow_mutating_management = false` does NOT refuse it — it is a read, absent
/// from pi's `MUTATING_MANAGEMENT_ACTIONS` (`subagent-executor.ts:213`).
///
/// Gutted: drop the `SUBAGENT_ACTIONS` entry → the position assertion fails; drop the
/// `route_action` arm → the child-safe dispatch lands on the unknown-action arm and errors.
#[tokio::test]
async fn children_list_is_advertised_at_pis_index_and_passes_the_child_safe_gate() {
    let actions = crate::extension::tool::text::subagent_actions();
    let at = actions
        .iter()
        .position(|a| *a == "children.list")
        .expect("`children.list` must be in SUBAGENT_ACTIONS");
    assert_eq!(actions.get(at - 1).copied(), Some("models"));
    assert_eq!(actions.get(at + 1).copied(), Some("guide"));

    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let child_safe = SubagentTool::new_child_safe(executor, dir.path().to_path_buf());
    let result = dispatch_tool(
        &child_safe,
        serde_json::json!({ "action": "children.list" }),
    )
    .await
    .expect("a read verb is not refused by the child-safe gate");
    assert_eq!(
        tool_text(&result),
        NO_RETAINED_CHILDREN,
        "with no workflow ever launched under this cwd the listing is pi's empty sentence"
    );
}

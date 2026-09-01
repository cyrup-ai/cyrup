//! Integration tests for Tier-1 (P1): the `subagent` tool's top-level PARALLEL (`tasks[]`) and
//! CHAIN (`chain[]`) dispatch arms, the per-task `count` fan-out multiplier, duplicate-output-path
//! rejection, and the inline `[k=v]` step-override application path (`/run [model=…]`, `/chain`
//! inline-group `[count=…]`, per-step `[model=…]`).
//!
//! No mocking anywhere (this crate's standing convention): every dispatch below drives the REAL
//! `cyrup_core::Tool::execute` / `NativeExtension::execute_command` path, which spawns REAL child OS
//! subprocesses — the scripted-NDJSON `cyrup-subagent-fixture` binary (arch-SA §11) — via
//! `CYRUP_SUBAGENT_BINARY` (R-SA-045 tier 1). The single observation channel is the per-attempt
//! raw-stdout tee `exec::run_sync` writes for every spawned child
//! (`<child_cwd>/.cyrup-subagent-scratch/attempt-0.jsonl`), which — with the fixture's `echo_argv`
//! — records exactly what argv (task text, `--model <id>`) each real child received.
//!
//! Gated on the `test-fixtures` Cargo feature, matching every other fixture-dependent integration
//! test in this crate; without it this file compiles to an empty, passing test list.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};



use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolResult};
use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;


fn scoped_config(root: &std::path::Path, script_path: &Path) -> SubagentExtensionConfig {
    SubagentExtensionConfig {
        missions: Some(cyrup_ext_subagents::missions::MissionStoreConfig {
            global_index_dir: Some(
                root.join("agent").join("missions").join("index").to_string_lossy().into_owned(),
            ),
            ..Default::default()
        }),
        // The fixture named for THIS extension rather than moved into the process environment
        // every concurrently-running test in this binary shares. Reaches chain/parallel steps
        // through `ExecSingleStepExecutor::foreground`, and the child's argv via `base_args`.
        spawn_command: Some(SpawnCommand {
            binary: fixture_binary_path(),
            base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
        }),
        // SUBA-083: this suite spawns real children and asserts their completed output, so the
        // config states its launch mode rather than inheriting it (an absent `asyncByDefault`
        // backgrounds — pi `config.ts:222-224`).
        async_by_default: false,
        ..Default::default()
    }
}

fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
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

/// Write a trivial agent persona `.md` to `<cwd>/.cyrup/agents/<name>.md` — the exact project-scope
/// discovery root `SubagentExecutor::discovery_config` scans, so the persona is genuinely
/// discovered through the real pipeline (and carries a real `model:` so a no-override child spawns).
fn write_fixture_persona(cwd: &Path, local_name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{local_name}.md")),
        format!(
            "---\nname: {local_name}\ndescription: a trivial fixture persona for tool \
             parallel/chain tests\nmodel: fixture/model\n---\n\nYou are a trivial test persona.\n"
        ),
    )
    .expect("write fixture persona");
}

/// The per-attempt raw-stdout tee for the (first, here only) spawn attempt of a child that ran in
/// `child_cwd` — holds every raw NDJSON line the real child emitted, incl. `echo_argv` lines.
fn read_attempt_tee(child_cwd: &Path) -> String {
    let path = child_cwd.join(".cyrup-subagent-scratch").join("attempt-0.jsonl");
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// Concatenate a [`ToolResult`]'s text blocks (the LLM-facing content the tool returns).
fn tool_result_text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_script(dir: &Path, script: &serde_json::Value) -> PathBuf {
    let path = dir.join("fixture-script.json");
    std::fs::write(&path, script.to_string()).expect("write fixture script");
    path
}

fn echo_argv_script(message: &str) -> serde_json::Value {
    serde_json::json!({
        "steps": [ { "kind": "emit", "line": message_end_line(message) } ],
        "echo_argv": true,
        "exit_code": 0
    })
}

fn command_ctx(cwd: &Path) -> HostCtx {
    HostCtx::command(ExtMode::Tui, false, cwd.to_path_buf())
}

async fn dispatch_tool(ext: &SubagentsExtension, params: serde_json::Value) -> ToolResult {
    ext.subagent_tool()
        .execute(
            ToolCallId::from("t"),
            params,
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect("tool execute must succeed")
}

// =============================================================================================
// (1) tasks[] PARALLEL: N real children, each receiving its OWN per-task input.
// =============================================================================================

/// A top-level `{ tasks: [...] }` call fans out over the REAL bounded worker pool: each task spawns
/// its own real child, in its own cwd, receiving its OWN distinct task text — proven by reading each
/// child's raw-stdout tee back and confirming task A reached child A and task B reached child B (not
/// a single shared child, and not cross-contaminated). The `N/M succeeded` summary is pi's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_parallel_tasks_run_n_real_children_with_per_task_outputs() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "worker");

    // Distinct cwd per task so each real child's tee is isolated (the tee is named per-attempt, not
    // per-child, so two children sharing a cwd would overwrite each other's tee).
    let cwd_a = work_dir.path().join("task-a");
    let cwd_b = work_dir.path().join("task-b");
    std::fs::create_dir_all(&cwd_a).expect("mkdir task-a");
    std::fs::create_dir_all(&cwd_b).expect("mkdir task-b");

    let script_path = write_script(work_dir.path(), &echo_argv_script("PARALLEL_OK"));

    let ext = SubagentsExtension::with_config_and_cwd(
        scoped_config(work_dir.path(), &script_path),
        work_dir.path().to_path_buf(),
    );

    let result = dispatch_tool(
        &ext,
        serde_json::json!({
            "tasks": [
                { "agent": "worker", "task": "TASK-ALPHA-INPUT", "cwd": cwd_a.to_string_lossy() },
                { "agent": "worker", "task": "TASK-BETA-INPUT", "cwd": cwd_b.to_string_lossy() },
            ]
        }),
    )
    .await;

    let text = tool_result_text(&result);
    assert!(
        text.contains("2/2 succeeded"),
        "the parallel summary must be pi's `N/M succeeded`: {text}"
    );

    let tee_a = read_attempt_tee(&cwd_a);
    let tee_b = read_attempt_tee(&cwd_b);
    assert!(
        !tee_a.is_empty() && !tee_b.is_empty(),
        "each task must have spawned its OWN real child (its own tee): a={tee_a:?} b={tee_b:?}"
    );
    assert!(
        tee_a.contains("TASK-ALPHA-INPUT") && !tee_a.contains("TASK-BETA-INPUT"),
        "child A must have received ONLY task A's input (per-task routing): {tee_a}"
    );
    assert!(
        tee_b.contains("TASK-BETA-INPUT") && !tee_b.contains("TASK-ALPHA-INPUT"),
        "child B must have received ONLY task B's input (per-task routing): {tee_b}"
    );
}

// =============================================================================================
// (2) tasks[] `count`: an inline count multiplies the fan-out width.
// =============================================================================================

/// `{ tasks: [{ agent, task, count: 3 }] }` fans a single declared task out into THREE real
/// children (pi `expandTopLevelTaskCounts`) — proven by the `3/3 succeeded` summary and three
/// distinct `=== Task N: worker ===` sections each carrying the child's real fixture output.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_parallel_count_multiplies_fan_out_into_that_many_real_children() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "worker");

    let script_path = write_script(work_dir.path(), &echo_argv_script("COUNT_FANOUT_CHILD"));

    let ext = SubagentsExtension::with_config_and_cwd(
        scoped_config(work_dir.path(), &script_path),
        work_dir.path().to_path_buf(),
    );

    let result = dispatch_tool(
        &ext,
        serde_json::json!({
            "tasks": [ { "agent": "worker", "task": "do the shared work", "count": 3 } ]
        }),
    )
    .await;

    let text = tool_result_text(&result);
    assert!(
        text.contains("3/3 succeeded"),
        "[count=3] must widen the fan-out to three real children all succeeding: {text}"
    );
    assert_eq!(
        text.matches("=== Task").count(),
        3,
        "the summary must carry three per-task sections (one per fanned-out child): {text}"
    );
    assert_eq!(
        text.matches("COUNT_FANOUT_CHILD").count(),
        3,
        "each of the three real children must contribute its own fixture output: {text}"
    );
    let details = result.details.expect("parallel details present");
    assert_eq!(details["total"], serde_json::json!(3));
    assert_eq!(details["succeeded"], serde_json::json!(3));
}

// =============================================================================================
// (3) duplicate-output-path rejection happens BEFORE any child spawns.
// =============================================================================================

/// Two parallel tasks resolving their `output` to the same path is rejected with pi's exact message
/// BEFORE any child is spawned (`findDuplicateParallelOutputPath`) — proven by the absence of any
/// tee (no `.cyrup-subagent-scratch` was ever created).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_parallel_rejects_duplicate_output_paths_before_any_spawn() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "worker");
    let script_path = write_script(work_dir.path(), &echo_argv_script("SHOULD_NOT_RUN"));

    let ext = SubagentsExtension::with_config_and_cwd(
        scoped_config(work_dir.path(), &script_path),
        work_dir.path().to_path_buf(),
    );

    let err = ext
        .subagent_tool()
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({
                "tasks": [
                    { "agent": "worker", "task": "write A", "output": "same.md" },
                    { "agent": "worker", "task": "write B", "output": "same.md" },
                ]
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("duplicate output paths must be rejected");

    assert!(
        err.to_string().contains("same path"),
        "the rejection must be pi's duplicate-output message: {err}"
    );
    assert!(
        !work_dir.path().join(".cyrup-subagent-scratch").exists(),
        "no child may have been spawned before the duplicate-path rejection"
    );
}

// =============================================================================================
// (4) chain[] via the tool: a static parallel group with per-task `count`.
// =============================================================================================

/// `{ chain: [{ parallel: [{ agent, count: 2 }] }] }` lowers to a parallel group step whose
/// per-task `count` is expanded (pi `expandChainParallelCounts`), spawning two real children — the
/// tool-driven CHAIN analogue of the slash inline-group `[count]` fan-out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_chain_static_parallel_group_count_expands_fan_out() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "worker");
    let script_path = write_script(work_dir.path(), &echo_argv_script("CHAIN_GROUP_CHILD"));

    let ext = SubagentsExtension::with_config_and_cwd(
        scoped_config(work_dir.path(), &script_path),
        work_dir.path().to_path_buf(),
    );

    let result = dispatch_tool(
        &ext,
        serde_json::json!({
            "chain": [ { "parallel": [ { "agent": "worker", "task": "fan", "count": 2 } ] } ]
        }),
    )
    .await;

    let text = tool_result_text(&result);
    assert!(
        text.contains("step 1: ok (parallel group)"),
        "the chain's single element must render as one collapsed parallel group: {text}"
    );
    assert_eq!(
        text.matches("CHAIN_GROUP_CHILD").count(),
        2,
        "[count=2] inside a chain parallel group must spawn two real children: {text}"
    );
    assert!(text.contains("child 1: ok") && text.contains("child 2: ok"), "got: {text}");
}

// =============================================================================================
// (5) /run [model=…]: the inline model override actually reaches the child as `--model <id>`.
// =============================================================================================

/// `/run worker[model=<id>] <task>` must spawn the child against the OVERRIDE model, not the
/// persona's own default — proven by reading the child's tee and confirming its `--model` argv is
/// the override and the persona default never appears (before the fix the override was filtered out
/// of the availability set and silently dropped, so the child ran the default model).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slash_run_inline_model_override_reaches_the_child() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "worker");
    let script_path = write_script(work_dir.path(), &echo_argv_script("RUN_OK"));

    let ext = SubagentsExtension::with_config_and_cwd(
        scoped_config(work_dir.path(), &script_path),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    ext.execute_command("run", "worker[model=override-model-zzz] do the work", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("run produces textual output");

    let tee = read_attempt_tee(work_dir.path());
    assert!(!tee.is_empty(), "the /run child must have spawned (a tee exists)");
    assert!(
        tee.contains("\"arg\":\"--model\"") && tee.contains("\"arg\":\"override-model-zzz\""),
        "the inline [model=…] override must reach the child as `--model override-model-zzz`: {tee}"
    );
    assert!(
        !tee.contains("\"arg\":\"fixture/model\""),
        "the persona's own default model must NOT be what the child ran — the override replaced \
         it, it did not merely append behind it: {tee}"
    );
}

// =============================================================================================
// (6) /chain inline group `[count=N]`: the slash inline-override count fan-out.
// =============================================================================================

/// `/chain worker "seed" -> (worker[count=2] "a" | worker "b")` widens the inline parallel group:
/// the `[count=2]` task expands to two children which, with the sibling `worker "b"`, makes THREE
/// real children in the group (the slash `[count=…]` inline-override path, previously
/// parsed-then-dropped). A parallel group needs ≥2 declared tasks (pi grammar), so `count` is
/// exercised alongside a sibling task rather than as a lone single-task group.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_chain_inline_group_count_multiplies_fan_out() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "worker");
    let script_path = write_script(work_dir.path(), &echo_argv_script("SLASH_COUNT_CHILD"));

    let ext = SubagentsExtension::with_config_and_cwd(
        scoped_config(work_dir.path(), &script_path),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command(
            "chain",
            "worker \"seed\" -> (worker[count=2] \"a\" | worker \"b\")",
            &ctx,
        )
        .await
        .expect("execute_command does not error")
        .expect("chain produces textual output");

    // step 1 is the seed single-step; step 2 is the group, widened to 3 children by [count=2] + 1.
    assert!(output.contains("step 2: ok (parallel group)"), "got: {output}");
    assert!(
        output.contains("child 3: ok"),
        "the [count=2] task + its sibling must fan out to three group children: {output}"
    );
    assert!(
        !output.contains("child 4:"),
        "exactly three children — [count=2] must expand to two, not more: {output}"
    );
}

// =============================================================================================
// (7) chain[] per-step `model`: the tool-driven chain step's model override reaches its child.
// =============================================================================================

/// A tool `{ chain: [{ agent, task, model }] }` sequential step's `model` override reaches that
/// step's spawned child as `--model <id>` (the tool-driven analogue of the slash per-step
/// `[model=…]`; previously the chain step's model was parsed-then-dropped). A single sequential
/// step runs in the tool's own cwd, so its tee is read directly there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_chain_step_model_override_reaches_the_child() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    write_fixture_persona(work_dir.path(), "worker");
    let script_path = write_script(work_dir.path(), &echo_argv_script("CHAIN_MODEL_OK"));

    let ext = SubagentsExtension::with_config_and_cwd(
        scoped_config(work_dir.path(), &script_path),
        work_dir.path().to_path_buf(),
    );

    dispatch_tool(
        &ext,
        serde_json::json!({
            "chain": [ { "agent": "worker", "task": "do it", "model": "chain-step-model-yyy" } ]
        }),
    )
    .await;

    let tee = read_attempt_tee(work_dir.path());
    assert!(
        tee.contains("\"arg\":\"chain-step-model-yyy\""),
        "the chain step's `model` override must reach its child as \
         `--model chain-step-model-yyy`: {tee}"
    );
}

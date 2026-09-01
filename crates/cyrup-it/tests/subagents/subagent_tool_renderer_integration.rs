//! Integration test: the `subagent` tool row is drawn by THIS EXTENSION (G128).
//!
//! `tui::events::render_inline_result` was written, unit-tested, and called by nothing but its own
//! `#[cfg(test)]` module: `SubagentsExtension` implemented neither `render_call` nor `render_result`
//! and never called `register_tool_renderer`, so the host's `has_tool_renderer("subagent")`
//! pre-check short-circuited on every event and the tool row always drew cyrup's built-in shell.
//!
//! Upstream declares both renderers on its `ToolDefinition` (`pi-subagents/src/extension/index.ts:
//! 465` `renderCall`, `:495` `renderResult` → `tui/render.ts:1678` `renderSubagentResult`,
//! @v0.34.0), which pi's interactive mode prefers over the built-in
//! (`pi/packages/coding-agent/src/modes/interactive/components/tool-execution.ts:81-112`).
//!
//! Every test here drives the REAL host seam — `ExtensionHost::has_tool_renderer` +
//! `render_tool_call` / `render_tool_result` — which is exactly what `cyrup-tui`'s
//! `extension_render` calls on `ToolExecutionStart`/`ToolExecutionEnd`
//! (`crates/cyrup-tui/src/app.rs:4276-4296`). Calling `render_inline_result` directly would prove
//! nothing that was not already true.
//!
//! The user action: the model issues a `subagent` tool call in an interactive session. The call row
//! and the result row are both drawn by this extension.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use cyrup_ext::{ExtMode, ExtensionHost, HostConfig};
use cyrup_ext_subagents::extension::{RegistrationMode, SubagentsExtension};
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use serde_json::{json, Value};

/// Missions are ON by default, and a task-bearing dispatch auto-creates one
/// (`missions/lifecycle.rs::prepare_mission_launch`). Its cross-project POINTER INDEX defaults to
/// `agent_dir()/missions/index` — the developer's real `~/.cyrup/agent/missions/index`, beside
/// `settings.json`, `models-store.json` and `sessions/` — so a tempdir cwd alone does not isolate
/// it. `config.missions.globalIndexDir` is the production lever that does, and it is the lever
/// upstream's own fixtures use (`pi-subagents` `test/unit/mission-lifecycle.test.ts:18`).
fn scoped_config(root: &std::path::Path) -> SubagentExtensionConfig {
    SubagentExtensionConfig {
        missions: Some(cyrup_ext_subagents::missions::MissionStoreConfig {
            global_index_dir: Some(
                root.join("agent").join("missions").join("index").to_string_lossy().into_owned(),
            ),
            ..Default::default()
        }),
        // SUBA-083: needed for the launch in `a_real_tool_result_renders_through_the_settled_branch`:
        // SETTLED is the foreground branch, and a backgrounded call would render the
        // async-start branch instead (pi `config.ts:222-224`). The `draw()` tests are
        // unaffected either way — rendering is a pure function of the payload.
        async_by_default: false,
        ..Default::default()
    }
}

async fn host_at(cwd: &Path) -> Arc<ExtensionHost> {
    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: cwd.to_path_buf(),
    }));
    host.load_native(Arc::new(SubagentsExtension::with_config_and_cwd(
        scoped_config(cwd),
        cwd.to_path_buf(),
    )))
    .await
    .expect("the subagents extension loads");
    host
}

/// `cyrup-tui`'s own flattener contract: a renderer's return is a widget tree, and an array of
/// strings flattens newline-joined (`crates/cyrup-tui/src/app.rs:4511-4512`).
fn flatten(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|i| i.as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        other => panic!("a renderer must return a string or an array of strings, got {other}"),
    }
}

/// Registration proof: the host's cheap SYNC pre-check — the one that decides whether a renderer is
/// consulted AT ALL — says yes for `subagent` and no for anything else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_host_knows_this_extension_renders_the_subagent_tool() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path()).await;
    assert!(
        host.has_tool_renderer("subagent"),
        "without this the host never calls render_call/render_result at all"
    );
    assert!(!host.has_tool_renderer("bash"), "no other tool is claimed");
}

/// A fanout child declares NEITHER renderer, matching upstream's restricted `ToolDefinition`
/// (`fanout-child.ts:156-168` has no `renderCall`/`renderResult`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_safe_registration_declares_no_renderer() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: false,
        cwd: dir.path().to_path_buf(),
    }));
    host.load_native(Arc::new(SubagentsExtension::with_mode(
        scoped_config(dir.path()),
        dir.path().to_path_buf(),
        RegistrationMode::ChildSafe,
    )))
    .await
    .unwrap();
    assert!(!host.has_tool_renderer("subagent"));
}

/// Every branch of pi's `renderCall` (`extension/index.ts:548-568` @v0.43.0), through the real host
/// call `cyrup-tui` makes on `ToolExecutionStart`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_call_row_renders_every_pi_branch() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path()).await;
    let draw = |args: Value| {
        let host = host.clone();
        async move { flatten(&host.render_tool_call("subagent", &args).await.expect("rendered")) }
    };

    // `:488-492` — a single run names its persona.
    assert_eq!(draw(json!({"agent": "researcher"})).await, "subagent researcher");
    // `:489` — `?` when no persona was given.
    assert_eq!(draw(json!({})).await, "subagent ?");
    // `:475` — the async badge.
    assert_eq!(
        draw(json!({"agent": "researcher", "async": true})).await,
        "subagent researcher [async]"
    );
    // `:475` — suppressed while clarifying.
    assert_eq!(
        draw(json!({"agent": "researcher", "async": true, "clarify": true})).await,
        "subagent researcher"
    );
    // `:476-481` — a chain names its LENGTH.
    assert_eq!(
        draw(json!({"chain": [{"agent": "a"}, {"agent": "b"}, {"agent": "c"}]})).await,
        "subagent chain (3)"
    );
    // `:482-487` + `effectiveParallelTaskCount` (`:447-453`) — per-task `count` is summed, and a
    // task without one counts as 1.
    assert_eq!(
        draw(json!({"tasks": [{"agent": "a", "count": 3}, {"agent": "b"}]})).await,
        "subagent parallel (4)"
    );
    // A non-integer / sub-1 `count` falls back to 1 (`:451`).
    assert_eq!(
        draw(json!({"tasks": [{"agent": "a", "count": 0}, {"agent": "b", "count": "x"}]})).await,
        "subagent parallel (2)"
    );
    // `:466-472` — a management action names its target, agent first then chainName.
    assert_eq!(draw(json!({"action": "list"})).await, "subagent list");
    assert_eq!(
        draw(json!({"action": "status", "agent": "worker"})).await,
        "subagent status worker"
    );
    assert_eq!(
        draw(json!({"action": "run-chain", "chainName": "review"})).await,
        "subagent run-chain review"
    );
    // `:466` — an action takes precedence over the chain/parallel shapes.
    assert_eq!(
        draw(json!({"action": "list", "chain": [{"agent": "a"}]})).await,
        "subagent list"
    );
}

/// The result row for a SETTLED single run draws through `render_inline_result` — the header line
/// (agent + `[fork]` badge) and its stats line — instead of the built-in shell. This is pi's
/// `d.mode === "single" && d.results.length === 1` compact branch (`tui/render.ts:1709-1712`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_settled_single_result_draws_the_compact_row() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path()).await;

    // The wire shape the host hands a renderer: pi's `AgentToolResult<Details>`, which cyrup emits
    // as `{content, details, terminate}` (`cyrup-agent/src/agent.rs:123-142`).
    let result = json!({
        "content": [{"type": "text", "text": "the delivered output"}],
        "details": {
            "mode": "single",
            "runId": "run-abc",
            "context": "fork",
            "results": [{
                "agent": "researcher",
                "task": "investigate",
                "exitCode": 0,
                "usage": {
                    "input": 100, "output": 28, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 128,
                    "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
                },
                "model": null,
                "attemptedModels": [],
                "modelAttempts": [],
                "finalOutput": "the delivered output",
                "structuredOutput": null,
                "acceptance": null,
                "detached": false,
                "interrupted": false,
                "timedOut": false,
                "error": null,
                "outputTruncated": false,
                "toolCalls": [
                    {"text": "read a", "expandedText": "read a"},
                    {"text": "read b", "expandedText": "read b"}
                ]
            }]
        },
        "terminate": false
    });

    let drawn = flatten(&host.render_tool_result("subagent", &result).await.expect("rendered"));
    assert!(drawn.contains("researcher"), "the header names the agent: {drawn}");
    assert!(drawn.contains("[fork]"), "the fork badge renders: {drawn}");
    assert!(drawn.contains("2 tools"), "tool count comes off the result: {drawn}");
    assert!(drawn.contains("128 tokens"), "tokens are input + output: {drawn}");
    assert!(
        !drawn.contains("the delivered output"),
        "the compact row is the derived summary, not an echo of the content block: {drawn}"
    );
}

/// pi's `!d || !d.results.length` branch (`tui/render.ts:1413-1423`): an ASYNC start (which carries
/// `results: []`) draws the content text, with the `[fork]` prefix when the details declared one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_async_start_draws_the_content_text_with_the_fork_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path()).await;

    let result = json!({
        "content": [{"type": "text", "text": "Async: researcher [run-1]"}],
        "details": {"mode": "single", "runId": "run-1", "context": "fork", "results": [], "asyncId": "run-1"},
        "terminate": false
    });
    let drawn = flatten(&host.render_tool_result("subagent", &result).await.expect("rendered"));
    assert_eq!(drawn, "[fork] Async: researcher [run-1]");

    // No fork → no prefix; no content at all → pi's literal `(no output)` (`:1415`).
    let plain = json!({
        "content": [{"type": "text", "text": "Async: researcher [run-2]"}],
        "details": {"mode": "single", "runId": "run-2", "results": []},
        "terminate": false
    });
    assert_eq!(
        flatten(&host.render_tool_result("subagent", &plain).await.unwrap()),
        "Async: researcher [run-2]"
    );
    let empty = json!({ "content": [], "details": Value::Null, "terminate": false });
    assert_eq!(
        flatten(&host.render_tool_result("subagent", &empty).await.unwrap()),
        "(no output)"
    );
}

/// A management action's result (`details: {"mode":"management"}`, no `results`) also takes the
/// plain-text branch rather than erroring or drawing an empty row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_management_result_draws_its_report_text() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path()).await;
    let result = json!({
        "content": [{"type": "text", "text": "line one\nline two"}],
        "details": {"mode": "management", "results": []},
        "terminate": false
    });
    let drawn = flatten(&host.render_tool_result("subagent", &result).await.expect("rendered"));
    assert_eq!(drawn, "line one\nline two");
}

// =================================================================================================
// End-to-end: a REAL tool result's `details` is a shape this renderer can read
// =================================================================================================

/// The loop-closing proof. The fixtures above pin the WIRE shape; this one runs the real
/// `subagent` tool against a real child subprocess, rebuilds the `{content, details, terminate}`
/// object exactly as `cyrup-agent` emits it for `ToolExecutionEnd`
/// (`crates/cyrup-agent/src/agent.rs:123-142,927`), and renders THAT.
///
/// It is the assertion that would have caught the port bug this item fixed: `runSinglePath`'s
/// `details` is `{ mode: "single", runId, results: [r], … }` (`subagent-executor.ts:3811-3823`
/// @v0.34.0), and cyrup used to emit the bare `SingleResult` at the details ROOT — no `mode`, no
/// `results` — so `renderSubagentResult`'s only settled branch could never fire.
// MIGRATION: the original `#[cfg(feature = "test-fixtures")]` here named
// cyrup-ext-subagents' own bin-gating feature. In cyrup-it that spelling names THIS crate's
// features, where no `test-fixtures` exists — so this item would have compiled OUT and the
// test would have passed vacuously. build.rs always builds the fixture binaries, so the gate
// is now a build-script postcondition. See this target's main.rs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_tool_result_renders_through_the_settled_branch() {

    use cyrup_core::{CancelToken, Tool, ToolCallId};

    let dir = tempfile::tempdir().unwrap();
    let agents = dir.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("renderer-worker.md"),
        "---\nname: renderer-worker\ndescription: fixture persona for the renderer proof\n\
         model: fixture/model\n---\n\nYou are a trivial test persona.\n",
    )
    .unwrap();

    let script = json!({
        "steps": [{ "kind": "emit", "line": json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "RENDERER_CHILD_OUTPUT"}],
                "usage": {
                    "input": 7, "output": 5, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 12,
                    "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
                },
                "stopReason": "stop"
            }
        }).to_string() }],
        "exit_code": 0
    });
    let script_path = dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).unwrap();
    // The fixture named for THIS extension rather than moved into the process environment every
    // concurrently-running test in this binary shares.
    let mut config = scoped_config(dir.path());
    config.spawn_command = Some(SpawnCommand {
        binary: crate::support::bins::subagent_fixture(),
        base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
    });

    let ext = SubagentsExtension::with_config_and_cwd(
        config,
        dir.path().to_path_buf(),
    );
    let tool_result = ext
        .subagent_tool()
        .execute(
            ToolCallId::from("t"),
            json!({ "agent": "renderer-worker", "task": "do the thing" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect("the tool call succeeds");

    let details = tool_result.details.clone().expect("a settled single run carries details");
    assert_eq!(
        details.get("mode").and_then(Value::as_str),
        Some("single"),
        "pi's Details carries `mode` at the ROOT: {details}"
    );
    assert_eq!(
        details.get("results").and_then(Value::as_array).map(Vec::len),
        Some(1),
        "the SingleResult must be WRAPPED under `results`, not spread at the root: {details}"
    );
    assert!(details.get("runId").is_some(), "pi carries runId too: {details}");

    // `cyrup-agent`'s exact `tool_execution_end.result` projection.
    let wire = json!({
        "content": tool_result.content,
        "details": details,
        "terminate": tool_result.terminate,
    });

    let host = host_at(dir.path()).await;
    let drawn = flatten(&host.render_tool_result("subagent", &wire).await.expect("rendered"));
    assert!(drawn.contains("renderer-worker"), "the compact row names the persona: {drawn}");
    // (The `[fork]` badge is covered by `a_settled_single_result_draws_the_compact_row`;
    // driving a real fork here would need a live session leaf to branch from.)
    assert!(drawn.contains("12 tokens"), "the child's real usage renders: {drawn}");
}

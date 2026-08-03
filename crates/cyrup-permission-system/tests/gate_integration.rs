//! FULLY-WIRED integration proof (R-PERM-010/012/050): on a REAL assembled `AgentSession` (built by
//! `cyrup-test-support`'s `Harness`), a scripted `bash` tool call is genuinely BLOCKED by a deny rule
//! and genuinely ALLOWED by an allow rule THROUGH the registered `before_tool_call` hook — not a unit
//! test of the engine in isolation. The ground truth is a recording `bash` tool that logs every
//! command it actually executes: a blocked call never reaches it; an allowed call does. A third test
//! proves the installed default-ASK posture fail-CLOSES to `Block` (never the fail-open
//! EpochTimeout-then-proceed).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use cyrup_core::{
    CancelToken, Content, Message, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_permission_system::PermissionSystemExtension;
use cyrup_test_support::{create_harness, FauxResponse, HarnessOptions, TestTempDir};

/// A fake `bash` tool that RECORDS every command it actually executes (overrides the built-in of the
/// same name, R-08-012). It runs nothing — recording is the observable proof the gate let the call
/// THROUGH the registered hook.
struct RecordingBash {
    executed: Arc<Mutex<Vec<String>>>,
    params: serde_json::Value,
}

impl RecordingBash {
    fn new(executed: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            executed,
            params: serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } }
            }),
        }
    }
}

#[async_trait::async_trait]
impl Tool for RecordingBash {
    fn name(&self) -> &str {
        "bash"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        "recording bash (test double)"
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let command =
            params.get("command").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        self.executed.lock().unwrap_or_else(|e| e.into_inner()).push(command);
        Ok(ToolResult { content: vec![Content::text("EXECUTED")], details: None, terminate: false, ..Default::default() })
    }
}

/// Build an agent dir carrying a permission policy file, then a `Harness` wiring the recording `bash`
/// tool + the permission extension, scripted with a single `bash` tool call then a stop.
async fn harness_for(
    policy: &str,
    command: serde_json::Value,
) -> (cyrup_test_support::Harness, Arc<Mutex<Vec<String>>>, TestTempDir) {
    let agent_dir = TestTempDir::new().unwrap();
    std::fs::write(agent_dir.path().join("cyrup-permissions.jsonc"), policy).unwrap();

    let executed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let bash = Arc::new(RecordingBash::new(executed.clone()));
    // Construct the extension directly (explicit opt-in), pointing it at our policy-bearing agent dir.
    let ext = Arc::new(PermissionSystemExtension::new(
        agent_dir.path().to_path_buf(),
        agent_dir.path().to_path_buf(),
    ));

    let options = HarnessOptions {
        responses: vec![
            FauxResponse::tool_call("bash", command),
            FauxResponse::text("done"),
        ],
        queue_responses: true,
        tools: vec![bash],
        native_extensions: vec![ext],
        ..Default::default()
    };
    let harness = create_harness(options).await.unwrap();
    (harness, executed, agent_dir)
}

fn has_error_tool_result_containing(msgs: &[Message], needle: &str) -> bool {
    msgs.iter().any(|m| match m {
        Message::ToolResult { is_error, content, .. } => {
            *is_error
                && content.iter().any(|c| match c {
                    Content::Text { text, .. } => text.contains(needle),
                    _ => false,
                })
        }
        _ => false,
    })
}

#[tokio::test]
async fn gate_blocks_a_deny_rule_through_before_tool_call() {
    let (harness, executed, _dir) = harness_for(
        r#"{ "bash": { "echo *": "allow", "curl *": "deny" } }"#,
        serde_json::json!({ "command": "curl http://evil.example" }),
    )
    .await;

    let run = harness.run("go").await.unwrap();

    // The denied command NEVER reached the tool (blocked by the gate through the registered hook).
    assert!(
        executed.lock().unwrap_or_else(|e| e.into_inner()).is_empty(),
        "denied bash command must not execute; run events: {run:?}"
    );
    // The block surfaced as an is_error tool result carrying the policy deny reason.
    let msgs = harness.session().messages().await;
    assert!(
        has_error_tool_result_containing(&msgs, "is not permitted to run 'bash'"),
        "a tool result carried the deny reason; messages: {msgs:?}"
    );
}

#[tokio::test]
async fn gate_allows_an_allow_rule_through_before_tool_call() {
    let (harness, executed, _dir) = harness_for(
        r#"{ "bash": { "echo *": "allow", "curl *": "deny" } }"#,
        serde_json::json!({ "command": "echo hello" }),
    )
    .await;

    harness.run("go").await.unwrap();

    // The allowed command PROCEEDED through the hook and executed exactly once.
    let executed = executed.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(executed, vec!["echo hello".to_string()], "allowed bash command must execute");
}

#[tokio::test]
async fn yolo_config_auto_approves_an_otherwise_ask_command() {
    // Empty policy → default ASK; but `config.json` enables yolo, which auto-approves an `ask`
    // WITHOUT a human. Proves the extension `config.json` (`yoloMode`) is wired onto the live gate.
    let agent_dir = TestTempDir::new().unwrap();
    std::fs::write(agent_dir.path().join("cyrup-permissions.jsonc"), "{}").unwrap();
    let cfg_dir = agent_dir.path().join("cyrup-permission-system");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("config.json"), r#"{ "yoloMode": true }"#).unwrap();

    let executed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let bash = Arc::new(RecordingBash::new(executed.clone()));
    let ext = Arc::new(PermissionSystemExtension::new(
        agent_dir.path().to_path_buf(),
        agent_dir.path().to_path_buf(),
    ));
    let options = HarnessOptions {
        responses: vec![
            FauxResponse::tool_call("bash", serde_json::json!({ "command": "ls -la" })),
            FauxResponse::text("done"),
        ],
        queue_responses: true,
        tools: vec![bash],
        native_extensions: vec![ext],
        ..Default::default()
    };
    let harness = create_harness(options).await.unwrap();

    harness.run("go").await.unwrap();
    assert_eq!(
        executed.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec!["ls -la".to_string()],
        "yolo auto-approves the otherwise-ask command → it executes through the hook"
    );
}

/// The active tool-name set the model actually SEES on turn 1, driving a real session under `policy`
/// (through the permission extension → `before_agent_start` `setActiveTools` shaping →
/// `AgentSession::assemble_run_messages` in-turn drain → agent → provider request). Read back from the
/// scripted provider's captured first request context.
async fn model_visible_tools_for(policy: &str) -> Vec<String> {
    let agent_dir = TestTempDir::new().unwrap();
    std::fs::write(agent_dir.path().join("cyrup-permissions.jsonc"), policy).unwrap();
    let ext = Arc::new(PermissionSystemExtension::new(
        agent_dir.path().to_path_buf(),
        agent_dir.path().to_path_buf(),
    ));
    let options = HarnessOptions {
        responses: vec![FauxResponse::text("done")],
        queue_responses: true,
        native_extensions: vec![ext],
        ..Default::default()
    };
    let harness = create_harness(options).await.unwrap();
    harness.run("go").await.unwrap();
    let faux = harness.faux();
    faux.contexts
        .first()
        .map(|c| c.tools.iter().map(|t| t.name.clone()).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn denied_tool_is_shaped_out_of_the_model_tool_set_through_the_session_seam() {
    // CONTROL — default-ask everything: `write` IS exposed to the model (nothing shaped out).
    let control = model_visible_tools_for("{}").await;
    assert!(
        control.contains(&"write".to_string()),
        "control: the `write` tool is exposed to the model under default-ask; got {control:?}"
    );

    // TREATMENT — deny the `write` TOOL: the `before_agent_start` shaping (`setActiveTools`) staged by
    // the companion is DRAINED + APPLIED in-turn by `assemble_run_messages`, so the very first provider
    // request the model sees EXCLUDES `write` (turn 1, not turn 2), while non-denied tools remain.
    let denied = model_visible_tools_for(r#"{ "tools": { "write": "deny" } }"#).await;
    assert!(
        !denied.contains(&"write".to_string()),
        "treatment: the denied `write` tool must be shaped out of the model's turn-1 tool set; got {denied:?}"
    );
    assert!(
        denied.contains(&"read".to_string()),
        "treatment: a non-denied tool (`read`) must remain exposed; got {denied:?}"
    );
}

#[tokio::test]
async fn installed_default_ask_fail_closes_to_block_not_open() {
    // Empty policy → installed default is ASK for every category. With no reachable human
    // (NoOpAskChannel), the gate must fail CLOSED to Block — never fail-open-and-proceed.
    let (harness, executed, _dir) =
        harness_for(r#"{}"#, serde_json::json!({ "command": "ls -la" })).await;

    let run = harness.run("go").await.unwrap();

    assert!(
        executed.lock().unwrap_or_else(|e| e.into_inner()).is_empty(),
        "an unconfigured (default-ask) command must be blocked when no human is reachable; run: {run:?}"
    );
    let msgs = harness.session().messages().await;
    assert!(
        has_error_tool_result_containing(&msgs, "no interactive UI is available"),
        "the fail-closed ask block reason surfaced; messages: {msgs:?}"
    );
}

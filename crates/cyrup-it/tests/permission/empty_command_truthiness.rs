//! FULLY-WIRED regression proof for pi's JS-truthiness guards over `PermissionCheckResult`'s
//! optional string fields (`gate::truthy`).
//!
//! pi (`pi-permission-system` v0.7.1 `src/index.ts:360-393`) guards the denial-reason parts with
//! bare truthiness — `if (result.command) { parts.push(`command '${result.command}'`); }` and
//! `result.toolName === "bash" && result.command ? … : `User denied tool '…'.`` — so an EMPTY
//! command string contributes nothing. Cyrup's `command` is an `Option<String>` and
//! `PermissionManager::check_permission`'s bash branch mirrors pi's
//! `typeof record.command === "string" ? record.command : ""` by always emitting
//! `command: Some(command)`. A bash tool call whose input carries no `command` key therefore
//! reaches the formatters as `Some("")`, which an `Option::is_some()` / `if let Some(_)` guard
//! happily renders as `command ''`.
//!
//! These tests drive a REAL assembled `AgentSession` (the same `cyrup-test-support::Harness` seam
//! as `gate_integration.rs`) so the assertion lands on the text the MODEL actually receives from
//! the production caller `extension.rs`'s `HookOutcome::Block { reason:
//! Some(gate::format_deny_reason(&check, agent_name)) }` — not on a unit call of the formatter.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use cyrup_core::{
    TerminateHint,
    CancelToken, Content, Message, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_permission_system::PermissionSystemExtension;
use cyrup_test_support::{create_harness, FauxResponse, HarnessOptions, TestTempDir};

/// A `bash` test double that records what it executed. Nothing should ever reach it here — every
/// scenario below is denied by policy — but recording is the ground truth that the gate really
/// blocked rather than the tool silently no-op'ing.
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
        Ok(ToolResult {
            content: vec![Content::text("EXECUTED")],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

/// Run one scripted `bash` tool call carrying `input` under `policy`, through a real session with
/// the permission extension registered. Returns every `is_error` tool-result text the model saw,
/// plus whatever the recording tool executed.
async fn denied_reasons_for(
    policy: &str,
    input: serde_json::Value,
) -> (Vec<String>, Vec<String>) {
    let agent_dir = TestTempDir::new().unwrap();
    std::fs::write(agent_dir.path().join("cyrup-permissions.jsonc"), policy).unwrap();

    let executed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let bash = Arc::new(RecordingBash::new(executed.clone()));
    let ext = Arc::new(PermissionSystemExtension::new(
        agent_dir.path().to_path_buf(),
        agent_dir.path().to_path_buf(),
    ));

    let options = HarnessOptions {
        responses: vec![FauxResponse::tool_call("bash", input), FauxResponse::text("done")],
        queue_responses: true,
        tools: vec![bash],
        native_extensions: vec![ext],
        ..Default::default()
    };
    let harness = create_harness(options).await.unwrap();
    harness.run("go").await.unwrap();

    let reasons = harness
        .session()
        .messages()
        .await
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult { is_error: true, content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .collect();
    let executed = executed.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // `agent_dir` is held until here so the policy file outlives the run.
    drop(agent_dir);
    (reasons, executed)
}

/// A bash call with NO `command` key that trips a deny rule must produce pi's text
/// (`… is not permitted to run 'bash' (matched '*'). Hard stop: …`) — never a dangling
/// `command ''`.
///
/// Reverting `gate::truthy` at `format_deny_reason`'s `if let Some(command) = &result.command`
/// fails this on the `command ''` assertion.
#[tokio::test]
async fn deny_reason_omits_an_empty_bash_command() {
    // CONTROL — a real command is still named, so the assertion below is about emptiness, not
    // about the `command '…'` part having been dropped wholesale.
    let (control, _) = denied_reasons_for(
        r#"{ "bash": { "*": "deny" } }"#,
        serde_json::json!({ "command": "curl http://evil.example" }),
    )
    .await;
    assert!(
        control.iter().any(|r| r.contains("command 'curl http://evil.example'")),
        "control: a non-empty bash command must still be named in the deny reason; got {control:?}"
    );

    // TREATMENT — the input carries no `command` key at all, so the manager's bash branch emits
    // `command: Some(\"\")` and pi's `if (result.command)` truthiness suppresses the clause.
    let (reasons, executed) =
        denied_reasons_for(r#"{ "bash": { "*": "deny" } }"#, serde_json::json!({})).await;

    assert!(executed.is_empty(), "the denied call must not execute; executed: {executed:?}");
    assert!(
        !reasons.is_empty(),
        "the deny must surface as an is_error tool result carrying a reason"
    );
    assert!(
        reasons.iter().any(|r| r.contains("is not permitted to run 'bash'")),
        "the deny reason must name the bash tool; got {reasons:?}"
    );
    for reason in &reasons {
        assert!(
            !reason.contains("command ''"),
            "an empty bash command must contribute NOTHING to the deny reason (pi \
             `if (result.command)`), but the model was told: {reason}"
        );
    }
}

/// The same truthiness rule at the approval-subject seam (pi `getPatternApprovalSubject`'s
/// `return result.command || result.toolName;`). Proven at the unit boundary because the
/// "Allow Always" persistence it feeds needs an interactive human the harness has no channel for;
/// the wired half above covers the model-facing text.
#[test]
fn approval_subject_falls_back_to_the_tool_name_for_an_empty_command() {
    use cyrup_permission_system::gate::get_pattern_approval_subject;
    use cyrup_permission_system::types::{
        CheckSource, PermissionCheckResult, PermissionState,
    };

    let result = PermissionCheckResult {
        tool_name: "bash".to_string(),
        state: PermissionState::Ask,
        matched_pattern: None,
        command: Some(String::new()),
        target: None,
        source: CheckSource::Bash,
    };
    // pi: `result.command || result.toolName` → "bash". An `unwrap_or_else` on the `Option` yields
    // "", which `extension.rs`'s `!subject.is_empty()` guard then drops, persisting nothing.
    assert_eq!(
        get_pattern_approval_subject(&result, &serde_json::json!({})),
        "bash",
        "an empty command must fall through to the tool name, not produce an empty subject"
    );
}

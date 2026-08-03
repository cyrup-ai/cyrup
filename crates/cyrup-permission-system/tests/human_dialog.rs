//! FULLY-WIRED PROOF for the LIVE in-session human permission dialog (P-1/P-3, R-PERM-022). On a REAL
//! assembled INTERACTIVE `AgentSession`, an ask-tier `bash` tool call blocks the `before_tool_call`
//! hook; a scripted UI sink — the SAME `LiveHostServices::set_ui_sink` seam the TUI/RPC renderer feeds
//! at runtime — answers the permission `select` dialog; and the resolved decision gates the call:
//!
//! - "Allow Once" → the tool PROCEEDS through the hook + the real `LocalAskChannel` dialog path.
//! - "Reject" → the tool is BLOCKED (never executes) and the denial surfaces as an is_error result.
//! - "Allow Always" → the tool proceeds AND the always-decision persists a session approval rule, so a
//!   later same-subject call auto-allows with NO second dialog (proves the always-* store write).
//!
//! This exercises the whole surface end-to-end: `set_host_services` (P-1 capture) → the registered
//! `before_tool_call` gate → `resolve_ask` under a `HostCtx::begin_human_wait` (P-3) guard →
//! `HostServices::select` → the scripted sink → the decision → the tool. Not a unit test of the dialog
//! in isolation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cyrup_config::AppMode;
use cyrup_core::{
    CancelToken, Content, Message, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_permission_system::PermissionSystemExtension;
use cyrup_session_svc::{UiKind, UiReply, UiRequest};
use cyrup_test_support::{create_harness, FauxResponse, HarnessOptions, TestTempDir};
use tokio::sync::mpsc::UnboundedReceiver;

/// A fake `bash` tool that RECORDS every command it actually executes (overrides the built-in of the
/// same name, R-08-012). A blocked call never reaches it; an allowed call does — the observable proof
/// the gate + dialog let the call THROUGH the registered hook.
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

/// Spawn a scripted UI responder: every `select` dialog is answered with `answer` (a dialog option
/// string); other kinds get a `None` text. Records how many `select` prompts it saw so a test can
/// assert the dedup / session-store paths render ZERO extra dialogs. Returns the shared counter.
fn spawn_select_responder(
    mut ui_rx: UnboundedReceiver<UiRequest>,
    answer: &'static str,
) -> Arc<AtomicUsize> {
    let selects_seen = Arc::new(AtomicUsize::new(0));
    let counter = selects_seen.clone();
    tokio::spawn(async move {
        while let Some(req) = ui_rx.recv().await {
            let reply = match req.kind {
                UiKind::Select => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    UiReply::Text(Some(answer.to_string()))
                }
                UiKind::Confirm => UiReply::Confirm(false),
                _ => UiReply::Text(None),
            };
            let _ = req.reply.send(reply);
        }
    });
    selects_seen
}

/// Build an interactive harness (has_ui=true) wiring the recording `bash` tool + the permission
/// extension over an empty policy (default ASK for bash), then install a scripted UI sink answering
/// `answer`. Returns the harness, the executed-command log, the select-prompt counter, and the RAII
/// agent dir. `commands` are the bash commands the scripted provider requests, in order (each drives a
/// tool call within the single run).
async fn interactive_harness(
    answer: &'static str,
    commands: &[&str],
) -> (cyrup_test_support::Harness, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>, TestTempDir) {
    let agent_dir = TestTempDir::new().unwrap();
    // Empty policy ⇒ default ASK for every category (pi `permission-manager.ts:44-50`).
    std::fs::write(agent_dir.path().join("cyrup-permissions.jsonc"), "{}").unwrap();

    let executed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let bash = Arc::new(RecordingBash::new(executed.clone()));
    let ext = Arc::new(PermissionSystemExtension::new(
        agent_dir.path().to_path_buf(),
        agent_dir.path().to_path_buf(),
    ));

    let mut responses: Vec<FauxResponse> = commands
        .iter()
        .map(|c| FauxResponse::tool_call("bash", serde_json::json!({ "command": c })))
        .collect();
    responses.push(FauxResponse::text("done"));

    let options = HarnessOptions {
        responses,
        queue_responses: true,
        // Interactive ⇒ has_ui=true, so the gate reaches the human via the live dialog (pi
        // `confirmPermission` `ctx.hasUI` branch, index.ts:1509-1511).
        app_mode: AppMode::Interactive,
        tools: vec![bash],
        native_extensions: vec![ext],
        ..Default::default()
    };
    let harness = create_harness(options).await.unwrap();

    // Install the scripted UI sink on the session's OWN `LiveHostServices` — the SAME `Arc` the
    // builder late-bound into the extension via `set_host_services` (P-1), so the extension's
    // `LocalAskChannel::select` round-trips to this sink.
    let (ui_tx, ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    harness.session().services().host_services.set_ui_sink(ui_tx);
    let selects = spawn_select_responder(ui_rx, answer);

    (harness, executed, selects, agent_dir)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_allow_once_lets_the_ask_tier_tool_proceed() {
    let (harness, executed, selects, _dir) = interactive_harness("Allow Once", &["ls -la"]).await;

    harness.run("go").await.unwrap();

    // The human approved via the real select dialog → the tool PROCEEDED through the hook.
    assert_eq!(
        executed.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec!["ls -la".to_string()],
        "an ask-tier command the human approved must execute through the registered hook"
    );
    assert_eq!(selects.load(Ordering::SeqCst), 1, "exactly one permission dialog was surfaced");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_reject_blocks_the_ask_tier_tool() {
    let (harness, executed, selects, _dir) = interactive_harness("Reject", &["rm -rf /"]).await;

    let run = harness.run("go").await.unwrap();

    // The human rejected → the tool NEVER executed (blocked at the hook).
    assert!(
        executed.lock().unwrap_or_else(|e| e.into_inner()).is_empty(),
        "a human-rejected command must not execute; run: {run:?}"
    );
    assert_eq!(selects.load(Ordering::SeqCst), 1, "the reject was surfaced through one dialog");
    // The denial surfaced as an is_error tool result (pi `formatUserDeniedReason`).
    let msgs = harness.session().messages().await;
    assert!(
        has_error_tool_result_containing(&msgs, "User denied"),
        "the user-denied reason surfaced as an is_error result; messages: {msgs:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_allow_always_persists_a_session_rule_so_the_next_call_needs_no_dialog() {
    // Two identical bash calls in one run. The FIRST asks and the human picks "Allow Always"; the
    // decision persists a session approval rule for (bash, "echo hi"), so the SECOND call (a different
    // tool_call_id, a dedup MISS) auto-ALLOWS via the store overlay WITHOUT a second dialog.
    let (harness, executed, selects, _dir) =
        interactive_harness("Allow Always", &["echo hi", "echo hi"]).await;

    harness.run("go").await.unwrap();

    assert_eq!(
        executed.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec!["echo hi".to_string(), "echo hi".to_string()],
        "both calls executed: the first after the human allowed-always, the second via the session rule"
    );
    assert_eq!(
        selects.load(Ordering::SeqCst),
        1,
        "only the FIRST call prompted; the always-decision persisted a session rule for the second"
    );
}

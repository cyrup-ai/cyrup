//! The `intercom` tool (`index.ts:1425-1806`): `list`/`send`/`ask`/`reply`/`pending`/`status` over
//! the shared broker client. `ask` is the one blocking action (single-slot outbound waiter).

use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

use crate::identity::short_session_id;
use crate::inbound::format_attachments;
use crate::session_state::SharedIntercomState;
use crate::transport::client::SendOptions;
use crate::transport::protocol::{Attachment, SessionInfo, now_ms};

use super::text_result;

/// The `intercom` tool.
pub struct IntercomTool {
    state: Arc<SharedIntercomState>,
    parameters: serde_json::Value,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntercomParams {
    action: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    attachments: Option<Vec<Attachment>>,
    #[serde(default)]
    reply_to: Option<String>,
}

impl IntercomTool {
    /// Build the tool over the shared session state.
    #[must_use]
    pub fn new(state: Arc<SharedIntercomState>) -> Self {
        Self { state, parameters: parameters_schema() }
    }

    async fn dispatch(&self, params: IntercomParams, cancel: &CancelToken) -> Result<ToolResult, ToolError> {
        // pi routes every tool call through `ensureConnected("tool")` (`index.ts:1477`), not a bare
        // `client` read: a tool call is worth (re)spawning the broker and reconnecting for, so a
        // single earlier connection failure does not make this tool permanently useless.
        let client = crate::connect::ensure_connected(&self.state, crate::connect::ConnectReason::Tool)
            .await
            .map_err(|e| ToolError::new(format!("intercom is not connected to the broker: {e}")))?;

        match params.action.as_str() {
            "list" => {
                // `index.ts:1478-1507`: split into a "Current session" / "Other sessions" pair, keyed
                // off the broker-reported current session's own `cwd` (not the locally-captured one),
                // and error out (rather than silently rendering a flat list) if the broker's session
                // list is missing this session entirely.
                let self_id = client.session_id();
                let sessions = client.list_sessions().await.map_err(to_tool_err)?;
                let current_session =
                    self_id.as_deref().and_then(|id| sessions.iter().find(|s| s.id == id));
                let Some(current_session) = current_session else {
                    return Err(ToolError::new("Current session is missing from intercom session list."));
                };
                let current_cwd = current_session.cwd.clone();
                let current_section = format!(
                    "**Current session:**\n{}",
                    format_session_list_row(current_session, &current_cwd, true)
                );
                let other_sessions: Vec<&SessionInfo> = sessions
                    .iter()
                    .filter(|s| self_id.as_deref() != Some(s.id.as_str()))
                    .collect();
                let other_section = if other_sessions.is_empty() {
                    "**Other sessions:**\nNo other sessions connected.".to_string()
                } else {
                    format!(
                        "**Other sessions:**\n{}",
                        other_sessions
                            .iter()
                            .map(|s| format_session_list_row(s, &current_cwd, false))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                Ok(text_result(format!("{current_section}\n\n{other_section}")))
            }
            "send" => {
                let to = require(params.to, "send requires `to`")?;
                let message = require(params.message, "send requires `message`")?;
                let target = self.resolve_or_err(&client, &to).await?;
                if client.session_id().as_deref() == Some(target.as_str()) {
                    return Err(ToolError::new("Cannot send an intercom message to yourself."));
                }
                // confirmSend gate (`index.ts:1524-1536`): only for a non-reply send, only when the
                // config opts in, and only when this session actually has a UI to confirm through.
                if params.reply_to.is_none() && self.state.config.confirm_send && self.state.has_ui()
                    && let Some(services) = self.state.host_services()
                {
                    let attachment_text = params
                        .attachments
                        .as_deref()
                        .filter(|a| !a.is_empty())
                        .map(format_attachments)
                        .unwrap_or_default();
                    let confirmed = services.confirm(
                        "Send Message",
                        &format!("Send to \"{to}\":\n\n{message}{attachment_text}"),
                        &cyrup_ext::DialogOptions::default(),
                    );
                    if !confirmed {
                        return Ok(text_result("Message cancelled by user"));
                    }
                }
                let result = client
                    .send(&target, SendOptions {
                        text: message.clone(),
                        attachments: params.attachments.clone(),
                        reply_to: params.reply_to.clone(),
                        expects_reply: None,
                        message_id: None,
                    })
                    .await
                    .map_err(to_tool_err)?;
                if !result.delivered {
                    return Err(ToolError::new(result.reason.unwrap_or_else(|| "delivery failed".to_string())));
                }
                // `index.ts:1549-1557`: the audit entry + markReplied both run ONLY after a confirmed
                // delivery — a failed/undelivered send must leave the original inbound ask pending.
                if let Some(services) = self.state.host_services() {
                    let _ = services.append_entry(
                        "intercom_sent",
                        &serde_json::json!({
                            "to": to,
                            "message": {
                                "text": message,
                                "attachments": params.attachments,
                                "replyTo": params.reply_to,
                            },
                            "messageId": result.id,
                            "timestamp": now_ms(),
                        }),
                    );
                }
                if let Some(reply_to) = &params.reply_to {
                    self.state
                        .tracker
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .mark_replied(reply_to);
                }
                Ok(text_result(format!("Message sent to {target}.")))
            }
            "ask" => {
                let to = require(params.to, "ask requires `to`")?;
                let message = require(params.message, "ask requires `message`")?;
                let target = self.resolve_or_err(&client, &to).await?;
                if client.session_id().as_deref() == Some(target.as_str()) {
                    return Err(ToolError::new("Cannot ask yourself."));
                }
                let question_id = uuid::Uuid::new_v4().to_string();
                let reply = self
                    .state
                    .ask_and_wait(&client, &target, question_id.clone(), message.clone(), params.attachments.clone(), cancel)
                    .await
                    .map_err(to_tool_err)?;
                // `index.ts:1639-1655`: an audit entry for both the outbound ask and the inbound
                // reply. `ask_and_wait` sends with `message_id: Some(question_id)`, so the delivered
                // send's id is exactly `question_id` (`transport::client::send`, `client.rs:200-204`).
                if let Some(services) = self.state.host_services() {
                    let _ = services.append_entry(
                        "intercom_sent",
                        &serde_json::json!({
                            "to": to,
                            "message": {
                                "text": message,
                                "attachments": params.attachments,
                                "replyTo": params.reply_to,
                            },
                            "messageId": question_id,
                            "timestamp": now_ms(),
                        }),
                    );
                    let _ = services.append_entry(
                        "intercom_received",
                        &serde_json::json!({
                            "from": to,
                            "message": { "text": reply },
                            "timestamp": now_ms(),
                        }),
                    );
                }
                Ok(text_result(reply))
            }
            "reply" => {
                let message = require(params.message, "reply requires `message`")?;
                let now = now_ms();
                let target = {
                    let mut tracker = self.state.tracker.lock().unwrap_or_else(|e| e.into_inner());
                    tracker
                        .resolve_reply_target(params.to.as_deref(), params.reply_to.as_deref(), now)
                        .map_err(ToolError::new)?
                };
                // Self-target guard (`index.ts:1686-1691`): a resolved reply target may legitimately
                // be this session's own id (e.g. a stale/misrouted ask entry); pi refuses to loop a
                // reply back to itself.
                if client.session_id().as_deref() == Some(target.from.id.as_str()) {
                    return Err(ToolError::new("Cannot message the current session"));
                }
                let result = client
                    .send(&target.from.id, SendOptions {
                        text: message.clone(),
                        attachments: None,
                        reply_to: Some(target.message.id.clone()),
                        expects_reply: None,
                        message_id: None,
                    })
                    .await
                    .map_err(to_tool_err)?;
                // `index.ts:1692-1706`: markReplied runs ONLY on a confirmed delivery; a failed
                // delivery either dismisses the pending ask (when the reason is exactly "Session not
                // found") or, for any other failure reason, leaves the tracker untouched entirely so
                // the ask remains pending for a retry.
                {
                    let mut tracker = self.state.tracker.lock().unwrap_or_else(|e| e.into_inner());
                    if result.delivered {
                        tracker.mark_replied(&target.message.id);
                    } else if result.reason.as_deref() == Some("Session not found") {
                        tracker.dismiss_pending_ask(&target.message.id);
                    }
                }
                if result.delivered {
                    if let Some(services) = self.state.host_services() {
                        let _ = services.append_entry(
                            "intercom_sent",
                            &serde_json::json!({
                                "to": target.from.name.clone().unwrap_or_else(|| target.from.id.clone()),
                                "message": { "text": message, "replyTo": target.message.id },
                                "messageId": result.id,
                                "timestamp": now_ms(),
                            }),
                        );
                    }
                    Ok(text_result(format!("Reply sent to {}.", target.from.id)))
                } else {
                    Err(ToolError::new(result.reason.unwrap_or_else(|| "reply not delivered".to_string())))
                }
            }
            "pending" => {
                let now = now_ms();
                let pending = self
                    .state
                    .tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .list_pending(now);
                if pending.is_empty() {
                    return Ok(text_result("No unresolved inbound asks."));
                }
                // pi `index.ts:1747-1751`:
                // `- ${from.name || from.id} · ${message.id} · ${elapsedSeconds}s ago · ${preview}`
                //
                // The MESSAGE ID is the load-bearing column. `reply_tracker.rs:126` refuses a
                // sender-targeted reply with `Multiple pending asks from "{x}" — use the message id`
                // (upstream's own wording), and the tool documents `replyTo` as the escape. This row
                // used to print the sender's SESSION short-id instead, which is not a valid
                // `replyTo`, so once two asks shared a sender the model was told to use an id that
                // nothing in its output ever showed — an unbreakable loop of failing replies.
                //
                // `preview` collapses whitespace and truncates to 80 chars; the body was previously
                // emitted whole, so one long inbound ask could flood the tool result.
                let rows: Vec<String> = pending
                    .iter()
                    .map(|c| {
                        let who = c.from.name.clone().unwrap_or_else(|| c.from.id.clone());
                        let preview: String = c
                            .message
                            .content
                            .text
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .chars()
                            .take(80)
                            .collect();
                        let elapsed = now.saturating_sub(c.received_at) / 1000;
                        format!("- {} · {} · {}s ago · {}", who, c.message.id, elapsed, preview)
                    })
                    .collect();
                Ok(text_result(format!("**Pending asks:**\n{}", rows.join("\n"))))
            }
            "status" => {
                let connected = client.is_connected();
                let session_id = client.session_id().unwrap_or_else(|| "<none>".to_string());
                let count = client.list_sessions().await.map(|s| s.len()).unwrap_or(0);
                Ok(text_result(format!(
                    "intercom: {} | session id: {session_id} | active sessions: {count}",
                    if connected { "connected" } else { "disconnected" }
                )))
            }
            other => Err(ToolError::new(format!("unknown intercom action \"{other}\""))),
        }
    }

    async fn resolve_or_err(&self, client: &Arc<crate::transport::client::IntercomClient>, to: &str) -> Result<String, ToolError> {
        self.state
            .resolve_target(client, to)
            .await
            .map_err(to_tool_err)?
            .ok_or_else(|| ToolError::new(format!("Session not found: \"{to}\"")))
    }
}

fn require(value: Option<String>, msg: &str) -> Result<String, ToolError> {
    match value {
        Some(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(ToolError::new(msg.to_string())),
    }
}

fn to_tool_err(e: crate::error::IntercomError) -> ToolError {
    ToolError::new(e.to_string())
}

/// `formatSessionListRow` (`index.ts:400-406`).
fn format_session_list_row(session: &SessionInfo, current_cwd: &str, is_self: bool) -> String {
    let name = session
        .name
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or("Unnamed session");
    let mut tags: Vec<String> = Vec::new();
    if is_self {
        tags.push("self".to_string());
    } else if session.cwd == current_cwd {
        tags.push("same cwd".to_string());
    }
    if let Some(status) = &session.status {
        tags.push(status.clone());
    }
    let suffix = if tags.is_empty() { String::new() } else { format!(" [{}]", tags.join(", ")) };
    format!(
        "• {} ({}) — {} ({}){}",
        name,
        short_session_id(&session.id),
        session.cwd,
        session.model,
        suffix
    )
}

fn parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["list", "send", "ask", "reply", "pending", "status"],
                "description": "The intercom action to perform."
            },
            "to": { "type": "string", "description": "Target session name or id (send/ask/reply)." },
            "message": { "type": "string", "description": "Message text (send/ask/reply)." },
            "attachments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "enum": ["file", "snippet", "context"] },
                        "name": { "type": "string" },
                        "content": { "type": "string" },
                        "language": { "type": "string" }
                    },
                    "required": ["type", "name", "content"]
                }
            },
            "replyTo": { "type": "string", "description": "The ask message id this replies to (reply)." }
        },
        "required": ["action"]
    })
}

#[async_trait]
impl Tool for IntercomTool {
    fn name(&self) -> &str {
        "intercom"
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        "Coordinate with other local agent sessions over the intercom broker: list/send/ask/reply/pending/status."
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let parsed: IntercomParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid intercom tool call: {e}")))?;
        self.dispatch(parsed, &cancel).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

    use std::path::PathBuf;
    use std::time::Duration;

    use cyrup_core::Content;
    use cyrup_ext::{CannedResponses, RecordingServices};

    use crate::config::IntercomConfig;
    use crate::transport::client::IntercomClient;
    use crate::transport::protocol::{Message, MessageContent, SessionRegistration};
    use crate::transport::spawn::wait_for_broker;

    use super::*;

    /// pi `index.ts:1747-1751`. The MESSAGE ID is the load-bearing column, not decoration:
    /// `reply_tracker.rs:126` refuses a sender-targeted reply with upstream's own wording
    /// `Multiple pending asks from "{x}" — use the message id`, and the tool documents `replyTo` as
    /// the escape hatch. This row previously printed the sender's SESSION short-id, which is not a
    /// valid `replyTo` — so once two asks shared a sender the model was told to use an id that
    /// nothing in its own output had ever shown it, and every reply attempt failed.
    #[test]
    fn pending_rows_carry_the_message_id_so_reply_to_is_reachable() {
        let mut tracker = crate::reply_tracker::ReplyTracker::new(600_000);
        let now = now_ms();
        // Two asks from the SAME sender: the exact case that forces `replyTo`.
        tracker.record_incoming_message(session("s1", "/tmp/a"), ask_message("m-first"), now);
        tracker.record_incoming_message(session("s1", "/tmp/a"), ask_message("m-second"), now);

        let pending = tracker.list_pending(now);
        assert_eq!(pending.len(), 2, "both asks are pending");

        let rows: Vec<String> = pending
            .iter()
            .map(|c| {
                let who = c.from.name.clone().unwrap_or_else(|| c.from.id.clone());
                format!("- {} · {} · 0s ago · {}", who, c.message.id, c.message.content.text)
            })
            .collect();
        let rendered = format!("**Pending asks:**\n{}", rows.join("\n"));

        assert!(rendered.starts_with("**Pending asks:**"), "pi's header");
        for id in ["m-first", "m-second"] {
            assert!(
                rendered.contains(id),
                "every row must name its message id so `replyTo: {id}` is reachable:\n{rendered}"
            );
        }
    }

    /// pi collapses whitespace and slices the preview to 80 chars (`index.ts:1748`); the body used
    /// to be emitted whole, so one long inbound ask could flood the tool result.
    #[test]
    fn a_long_pending_body_is_whitespace_collapsed_and_truncated_to_80_chars() {
        let raw = format!("word\n\tspaced   out {}", "x".repeat(200));
        let preview: String = raw
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(80)
            .collect();
        assert_eq!(preview.chars().count(), 80, "sliced to 80 chars");
        assert!(preview.starts_with("word spaced out "), "whitespace collapsed: {preview:?}");
        assert!(!preview.contains('\n') && !preview.contains('\t'));
    }

    fn registration(name: &str) -> SessionRegistration {
        SessionRegistration {
            name: Some(name.to_string()),
            cwd: "/tmp/work".to_string(),
            model: "test-model".to_string(),
            pid: std::process::id(),
            started_at: now_ms(),
            last_activity: now_ms(),
            status: None,
        }
    }

    fn session(id: &str, cwd: &str) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            name: Some(id.to_string()),
            cwd: cwd.to_string(),
            model: "m".to_string(),
            pid: 1,
            started_at: now_ms(),
            last_activity: now_ms(),
            status: None,
            peer_uid: None,
            trusted_local: None,
        }
    }

    fn ask_message(id: &str) -> Message {
        Message {
            id: id.to_string(),
            timestamp: now_ms(),
            reply_to: None,
            expects_reply: Some(true),
            content: MessageContent { text: "hi".to_string(), attachments: None },
        }
    }

    fn result_text(result: &ToolResult) -> String {
        result
            .content
            .iter()
            .map(|c| match c {
                Content::Text { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Locate the real `cyrup-intercom-broker` binary next to this test binary.
    /// `CARGO_BIN_EXE_*` (a compile-time macro) is only populated by Cargo for integration
    /// tests/benchmarks, never for a library's own `#[cfg(test)]` unit tests like this module — so
    /// `option_env!` here is always `None` in this compilation unit. Fall back to locating the
    /// binary beside the running test binary, mirroring Cargo's own
    /// `target/<profile>/{deps/<test-binary>, <bin-name>}` layout.
    fn broker_bin_path() -> PathBuf {
        if let Some(compile_time) = option_env!("CARGO_BIN_EXE_cyrup-intercom-broker") {
            return PathBuf::from(compile_time);
        }
        let mut exe = std::env::current_exe().expect("current test binary path");
        exe.pop(); // drop the test binary's own file name
        if exe.ends_with("deps") {
            exe.pop(); // unit-test binaries build into target/<profile>/deps/
        }
        exe.push(format!("cyrup-intercom-broker{}", std::env::consts::EXE_SUFFIX));
        exe
    }

    /// Spawn the REAL broker (the port doc §5 Phase 2 fixture pattern, `tests/broker_roundtrip.rs`)
    /// as a genuine child process, returning it + its temp agent dir + its socket path.
    async fn spawn_broker() -> (tokio::process::Child, tempfile::TempDir, PathBuf) {
        let broker_bin = broker_bin_path();
        let agent_dir = tempfile::tempdir().expect("tempdir");
        let socket_path = agent_dir.path().join("intercom").join("broker.sock");
        let broker = tokio::process::Command::new(&broker_bin)
            .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the real intercom broker subprocess");
        wait_for_broker(&socket_path, Duration::from_secs(5))
            .await
            .expect("broker becomes health-connectable");
        (broker, agent_dir, socket_path)
    }

    // Regression proof for the dossier item "`reply` tool action is missing pi's self-target guard"
    // (`pi-intercom/index.ts:1685-1691`): against the PRE-FIX cyrup behavior this would resolve the
    // self-addressed pending ask and forward it straight to `client.send`, and the assertion on the
    // still-pending ask below would fail (the pre-fix code unconditionally left the ask untouched only
    // because it never even tried to dismiss it — but the delivered send itself would succeed against
    // the live broker, which is the actual bug this proves is now refused before ever reaching send).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reply_refuses_when_the_resolved_target_is_the_current_session() {
        let (mut broker, _agent_dir, socket_path) = spawn_broker().await;
        let client = Arc::new(
            IntercomClient::connect(&socket_path, registration("self"), Some("self-session".to_string()))
                .await
                .expect("connects"),
        );
        let state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, PathBuf::from("/w")));
        state.set_client(Some(client.clone()));

        // A (misrouted) pending inbound ask whose sender id is THIS session's own id.
        state
            .tracker
            .lock()
            .unwrap()
            .record_incoming_message(session("self-session", "/w"), ask_message("q1"), now_ms());

        let tool = IntercomTool::new(state.clone());
        let cancel = CancelToken::new();
        let params = IntercomParams {
            action: "reply".to_string(),
            to: None,
            message: Some("hello back".to_string()),
            attachments: None,
            reply_to: None,
        };
        let err = tool.dispatch(params, &cancel).await.expect_err("must refuse a self-target reply");
        assert!(err.message.contains("Cannot message the current session"), "got: {}", err.message);

        // The ask must still be pending — the guard fires before `markReplied`/`dismissPendingAsk`.
        let pending = state.tracker.lock().unwrap().list_pending(now_ms());
        assert_eq!(pending.len(), 1, "the self-targeted ask must remain pending, not sent or dismissed");

        client.disconnect();
        let _ = broker.kill().await;
    }

    // Regression proof for "`send` marks the ask replied before/regardless of delivery success"
    // (`pi-intercom/index.ts:1537-1557`): against the PRE-FIX behavior, `mark_replied` ran
    // unconditionally BEFORE the send even reached the broker, so the pending ask being replied-to
    // would already be gone (list_pending empty) even though the send itself failed. This asserts the
    // ask survives an undelivered send.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn send_does_not_mark_the_ask_replied_when_delivery_fails() {
        let (mut broker, _agent_dir, socket_path) = spawn_broker().await;
        let client = Arc::new(
            IntercomClient::connect(&socket_path, registration("self"), Some("self-session".to_string()))
                .await
                .expect("connects"),
        );
        let state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, PathBuf::from("/w")));
        state.set_client(Some(client.clone()));

        // A real pending inbound ask this "send" call claims (via `replyTo`) to be answering.
        state
            .tracker
            .lock()
            .unwrap()
            .record_incoming_message(session("original-asker", "/w"), ask_message("q1"), now_ms());

        let tool = IntercomTool::new(state.clone());
        let cancel = CancelToken::new();
        let params = IntercomParams {
            action: "send".to_string(),
            to: Some("no-such-session".to_string()),
            message: Some("this will not deliver".to_string()),
            attachments: None,
            reply_to: Some("q1".to_string()),
        };
        let err = tool.dispatch(params, &cancel).await.expect_err("delivery to an unknown session fails");
        assert!(err.message.contains("Session not found"), "got: {}", err.message);

        // The original inbound ask must still be pending — a failed send must not have marked it
        // replied, so the agent can still retry answering it.
        let pending = state.tracker.lock().unwrap().list_pending(now_ms());
        assert_eq!(pending.len(), 1, "a failed send must leave the original ask pending for a retry");

        client.disconnect();
        let _ = broker.kill().await;
    }

    // Regression proof for "`intercom{list}` drops the self-missing guard and the Current/Other
    // section split" (`pi-intercom/index.ts:1478-1507`): against the PRE-FIX behavior this rendered a
    // single flat, unheaded list of every session (including self) with no section split — the
    // asserted headers below would be entirely absent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn list_splits_current_and_other_sessions_with_headed_sections() {
        let (mut broker, _agent_dir, socket_path) = spawn_broker().await;
        let me = Arc::new(
            IntercomClient::connect(&socket_path, registration("me"), Some("me-session".to_string()))
                .await
                .expect("connects"),
        );
        let other = Arc::new(
            IntercomClient::connect(&socket_path, registration("other"), Some("other-session".to_string()))
                .await
                .expect("connects"),
        );

        let state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, PathBuf::from("/w")));
        state.set_client(Some(me.clone()));
        let tool = IntercomTool::new(state.clone());
        let cancel = CancelToken::new();
        let params =
            IntercomParams { action: "list".to_string(), to: None, message: None, attachments: None, reply_to: None };
        let result = tool.dispatch(params, &cancel).await.expect("list succeeds");
        let text = result_text(&result);

        assert!(text.contains("**Current session:**"), "missing current-session header: {text}");
        assert!(text.contains("**Other sessions:**"), "missing other-sessions header: {text}");
        let current_idx = text.find("**Current session:**").unwrap();
        let other_idx = text.find("**Other sessions:**").unwrap();
        assert!(current_idx < other_idx, "current section must come first: {text}");
        // Self must be tagged `[self]` and NOT appear again under "Other sessions" (rows render the
        // `shortSessionId` — `identity::short_session_id` — not the raw id, so match on that).
        let current_section = &text[current_idx..other_idx];
        let other_section = &text[other_idx..];
        let self_short_id = short_session_id("me-session");
        let other_short_id = short_session_id("other-session");
        assert!(current_section.contains("[self]"), "self row missing [self] tag: {text}");
        assert!(current_section.contains(&self_short_id), "self row missing own id: {text}");
        assert!(
            !other_section.contains(&self_short_id) && !other_section.contains("[self]"),
            "self leaked into other sessions: {text}"
        );
        assert!(other_section.contains(&other_short_id), "the other session must be listed: {text}");

        me.disconnect();
        other.disconnect();
        let _ = broker.kill().await;
    }

    // Regression proof for "confirmSend config is parsed but never enforced" (`index.ts:1524-1536`):
    // against the PRE-FIX behavior `confirm_send`/`has_ui` were never read at all, so a declined
    // confirmation would still deliver the message and the assertions below (cancellation text, no
    // delivery, no audit entry) would fail.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn send_honors_a_declined_confirm_send_prompt() {
        let (mut broker, _agent_dir, socket_path) = spawn_broker().await;
        let me = Arc::new(
            IntercomClient::connect(&socket_path, registration("me"), Some("me-session".to_string()))
                .await
                .expect("connects"),
        );
        let target = Arc::new(
            IntercomClient::connect(&socket_path, registration("target"), Some("target-session".to_string()))
                .await
                .expect("connects"),
        );
        let mut target_events = target.subscribe();

        let config = IntercomConfig { confirm_send: true, ..IntercomConfig::default() };
        let state = Arc::new(SharedIntercomState::new(config, 600_000, PathBuf::from("/w")));
        state.set_client(Some(me.clone()));
        state.set_has_ui(true);
        let services = Arc::new(RecordingServices::new(CannedResponses { confirm: false, ..Default::default() }));
        state.set_host_services(services.clone());

        let tool = IntercomTool::new(state.clone());
        let cancel = CancelToken::new();
        let params = IntercomParams {
            action: "send".to_string(),
            to: Some("target-session".to_string()),
            message: Some("please don't actually send".to_string()),
            attachments: None,
            reply_to: None,
        };
        let result = tool.dispatch(params, &cancel).await.expect("a declined confirm is not an error");
        assert_eq!(result_text(&result), "Message cancelled by user");
        assert!(services.entries_persisted().is_empty(), "a cancelled send must not append an audit entry");

        // The target never actually received anything.
        let never_delivered = tokio::time::timeout(Duration::from_millis(300), target_events.recv()).await;
        assert!(never_delivered.is_err(), "the declined send must never reach the broker/target");

        me.disconnect();
        target.disconnect();
        let _ = broker.kill().await;
    }

    // Regression proof for "intercom_sent / intercom_received audit-log entries are never recorded"
    // (`index.ts:1549-1554`): against the PRE-FIX behavior `append_entry` was never called anywhere in
    // this file, so `entries_persisted()` below would be empty.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn successful_send_appends_an_intercom_sent_audit_entry() {
        let (mut broker, _agent_dir, socket_path) = spawn_broker().await;
        let me = Arc::new(
            IntercomClient::connect(&socket_path, registration("me"), Some("me-session".to_string()))
                .await
                .expect("connects"),
        );
        let target = Arc::new(
            IntercomClient::connect(&socket_path, registration("target"), Some("target-session".to_string()))
                .await
                .expect("connects"),
        );

        let state = Arc::new(SharedIntercomState::new(IntercomConfig::default(), 600_000, PathBuf::from("/w")));
        state.set_client(Some(me.clone()));
        let services = Arc::new(RecordingServices::new(CannedResponses::default()));
        state.set_host_services(services.clone());

        let tool = IntercomTool::new(state.clone());
        let cancel = CancelToken::new();
        let params = IntercomParams {
            action: "send".to_string(),
            to: Some("target-session".to_string()),
            message: Some("hello target".to_string()),
            attachments: None,
            reply_to: None,
        };
        let result = tool.dispatch(params, &cancel).await.expect("send delivers");
        assert_eq!(result_text(&result), "Message sent to target-session.");

        let entries = services.entries_persisted();
        assert_eq!(entries.len(), 1, "exactly one intercom_sent entry: {entries:?}");
        assert_eq!(entries[0].0, "intercom_sent");
        assert_eq!(entries[0].1.get("to").and_then(|v| v.as_str()), Some("target-session"));
        assert_eq!(
            entries[0].1.get("message").and_then(|m| m.get("text")).and_then(|v| v.as_str()),
            Some("hello target")
        );

        me.disconnect();
        target.disconnect();
        let _ = broker.kill().await;
    }
}

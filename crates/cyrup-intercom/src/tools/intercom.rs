//! The `intercom` tool (`index.ts:1425-1806`): `list`/`send`/`ask`/`reply`/`pending`/`status` over
//! the shared broker client. `ask` is the one blocking action (single-slot outbound waiter).

use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

use crate::identity::short_session_id;
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
        let client = self
            .state
            .client()
            .ok_or_else(|| ToolError::new("intercom is not connected to the broker"))?;

        match params.action.as_str() {
            "list" => {
                let sessions = client.list_sessions().await.map_err(to_tool_err)?;
                let self_id = client.session_id();
                let cwd = self.state.cwd.to_string_lossy().to_string();
                let mut rows = Vec::with_capacity(sessions.len());
                for s in &sessions {
                    let is_self = self_id.as_deref() == Some(s.id.as_str());
                    rows.push(format_session_list_row(s, &cwd, is_self));
                }
                let body = if rows.is_empty() {
                    "No intercom sessions connected.".to_string()
                } else {
                    rows.join("\n")
                };
                Ok(text_result(body))
            }
            "send" => {
                let to = require(params.to, "send requires `to`")?;
                let message = require(params.message, "send requires `message`")?;
                let target = self.resolve_or_err(&client, &to).await?;
                if client.session_id().as_deref() == Some(target.as_str()) {
                    return Err(ToolError::new("Cannot send an intercom message to yourself."));
                }
                if let Some(reply_to) = &params.reply_to {
                    self.state
                        .tracker
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .mark_replied(reply_to);
                }
                let result = client
                    .send(&target, SendOptions {
                        text: message,
                        attachments: params.attachments,
                        reply_to: params.reply_to,
                        expects_reply: None,
                        message_id: None,
                    })
                    .await
                    .map_err(to_tool_err)?;
                if result.delivered {
                    Ok(text_result(format!("Message sent to {target}.")))
                } else {
                    Err(ToolError::new(result.reason.unwrap_or_else(|| "delivery failed".to_string())))
                }
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
                    .ask_and_wait(&client, &target, question_id, message, params.attachments, cancel)
                    .await
                    .map_err(to_tool_err)?;
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
                let result = client
                    .send(&target.from.id, SendOptions {
                        text: message,
                        attachments: None,
                        reply_to: Some(target.message.id.clone()),
                        expects_reply: None,
                        message_id: None,
                    })
                    .await
                    .map_err(to_tool_err)?;
                // dismissPendingAsk on "Session not found" (index.ts:1698-1700), else markReplied.
                {
                    let mut tracker = self.state.tracker.lock().unwrap_or_else(|e| e.into_inner());
                    if !result.delivered
                        && result.reason.as_deref() == Some("Session not found")
                    {
                        tracker.dismiss_pending_ask(&target.message.id);
                    } else {
                        tracker.mark_replied(&target.message.id);
                    }
                }
                if result.delivered {
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
                    return Ok(text_result("No pending intercom asks."));
                }
                let rows: Vec<String> = pending
                    .iter()
                    .map(|c| {
                        let who = c.from.name.clone().unwrap_or_else(|| c.from.id.clone());
                        format!("• {} ({}): {}", who, short_session_id(&c.from.id), c.message.content.text)
                    })
                    .collect();
                Ok(text_result(rows.join("\n")))
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

//! `intercom{action:"ask"}` (`v0.10.1 index.ts:2063-2181`) — the one BLOCKING action (single-slot
//! outbound waiter), and the only arm that reads the [`CancelToken`].

use std::sync::Arc;

use cyrup_core::{CancelToken, ToolError, ToolResult};

use crate::inbound::format_attachments;
use crate::tools::text_result;
use crate::transport::client::IntercomClient;
use crate::transport::protocol::now_ms;

use super::{DeliveryTarget, IntercomParams, IntercomTool, resolve_cwd_delivery_target, to_tool_err};

impl IntercomTool {
    pub(super) async fn action_ask(
        &self,
        params: &IntercomParams,
        client: &Arc<IntercomClient>,
        cancel: &CancelToken,
    ) -> Result<ToolResult, ToolError> {
        // `v0.10.1 index.ts:2071-2076`: the same single `(!to && !cwd) || !message` guard
        // and the same message as `send`.
        let to = params.to.clone().filter(|v| !v.trim().is_empty());
        let cwd = params.cwd.clone().filter(|v| !v.trim().is_empty());
        let message = match params.message.clone().filter(|v| !v.trim().is_empty()) {
            Some(message) if to.is_some() || cwd.is_some() => message,
            _ => {
                return Err(ToolError::new(
                    "Missing 'to' or 'cwd', or missing 'message' parameter",
                ));
            }
        };
        // `v0.10.1 index.ts:2103-2114`. With a `cwd` the target is resolved inside that
        // directory, and `resolveCwdDeliveryTarget`'s own "no session there" error already
        // says what happened — the offline refusal below belongs to the `to`-only branch.
        let delivery = match cwd.as_deref() {
            Some(cwd) => resolve_cwd_delivery_target(client, to.as_deref(), cwd).await?,
            None => {
                let to_value = to.clone().unwrap_or_default();
                // `v0.10.1 index.ts:2107-2113` (v0.10.0): an ask whose target is offline is
                // refused UP FRONT with the actionable text, not with the bare
                // `Session not found: "x"` the shared resolver produces — a blocking ask is
                // not queued anywhere, so the model has to be told to use `send` or to retry
                // after the peer reconnects.
                let Some(resolved) =
                    self.state.resolve_target(client, &to_value).await.map_err(to_tool_err)?
                else {
                    return Err(ToolError::new(format!(
                        "Session \"{to_value}\" is not currently connected. Blocking asks are not queued; use send for a non-blocking mailbox delivery or retry after the session reconnects."
                    )));
                };
                DeliveryTarget { id: resolved, label: to_value }
            }
        };
        let DeliveryTarget { id: target, label } = delivery;
        // `const targetDisplay = target.projectPane ? target.label : to ?? target.label;`
        // (`v0.10.1 index.ts:2116`).
        let to = to.unwrap_or(label);
        // `v0.10.1 index.ts:2122-2127` — pi's single self-target string, shared with `send`
        // and `reply`.
        if client.session_id().as_deref() == Some(target.as_str()) {
            return Err(ToolError::new("Cannot message the current session"));
        }
        let question_id = uuid::Uuid::new_v4().to_string();
        // `v0.10.1 index.ts:2135-2143`: the ask's own send carries the caller's `replyTo`.
        // Dropping it made a counter-ask against a peer's pending ask fail against cyrup's
        // OWN broker with `Reply target does not match a pending ask`.
        let reply_message = self
            .state
            .ask_and_wait_with_reply_to(
                client,
                &target,
                question_id.clone(),
                message.clone(),
                params.attachments.clone(),
                params.reply_to.clone(),
                params.supersedes.clone(),
                params.retry_of.clone(),
                cancel,
            )
            .await
            .map_err(to_tool_err)?;
        let reply_text = reply_message.content.text.clone();
        let reply_attachments = reply_message
            .content
            .attachments
            .as_deref()
            .filter(|a| !a.is_empty())
            .map(format_attachments)
            .unwrap_or_default();
        // `v0.10.1 index.ts:2161-2176`: an audit entry for both the outbound ask and the
        // inbound reply. `ask_and_wait` sends with `message_id: Some(question_id)`, so the
        // delivered send's id is exactly `question_id` (`transport::client::send`,
        // `client.rs:200-204`).
        if let Some(services) = self.state.host_services() {
            if let Err(e) = services.append_entry(
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
            ) {
                tracing::warn!(error = %e, kind = "intercom_sent", "intercom: failed to append audit entry");
            }
            // `v0.10.1 index.ts:2171-2176`. Three things this entry MUST carry that it used
            // to drop: the reply's own `messageId`, its `attachments`, and the SENDER's
            // timestamp (not the local receipt time). The durable record of an exchange has
            // to match what was exchanged, or the loss is undiscoverable afterwards.
            if let Err(e) = services.append_entry(
                "intercom_received",
                &serde_json::json!({
                    "from": to,
                    "message": {
                        "text": reply_text,
                        "attachments": reply_message.content.attachments,
                    },
                    "messageId": reply_message.id,
                    "timestamp": reply_message.timestamp,
                }),
            ) {
                tracing::warn!(error = %e, kind = "intercom_received", "intercom: failed to append audit entry");
            }
        }
        // `v0.10.1 index.ts:2180`: `**Reply from ${targetDisplay}:**\n${replyText}${replyAttachments}`,
        // keyed off the caller-supplied `to`. Without the header a transcript that has asked
        // more than one peer cannot tell which of them answered.
        Ok(text_result(format!("**Reply from {to}:**\n{reply_text}{reply_attachments}")))
    }
}

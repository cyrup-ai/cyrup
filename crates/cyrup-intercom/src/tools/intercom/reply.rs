//! `intercom{action:"reply"}` (`v0.10.1 index.ts:2183-2235`) — answer a pending inbound ask.

use std::sync::Arc;

use cyrup_core::{ToolError, ToolResult};

use crate::tools::detailed_result;
use crate::transport::client::{IntercomClient, SendOptions};
use crate::transport::protocol::now_ms;

use super::{IntercomParams, IntercomTool, display_name, require, to_tool_err};

impl IntercomTool {
    pub(super) async fn action_reply(
        &self,
        params: &IntercomParams,
        client: &Arc<IntercomClient>,
    ) -> Result<ToolResult, ToolError> {
        let message = require(params.message.clone(), "reply requires `message`")?;
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
        // `v0.10.1 index.ts:2211-2215` (v0.10.1 `2ba9f53`, "fix: preserve reply attachments
        // (#100)"): `attachments` is threaded through, not dropped. Before this a reply
        // carrying a file/snippet sent the prose and silently lost the payload — and the
        // audit entry below recorded the same lie.
        let result = client
            .send(
                &target.from.id,
                SendOptions {
                    text: message.clone(),
                    attachments: params.attachments.clone(),
                    reply_to: Some(target.message.id.clone()),
                    expects_reply: None,
                    message_id: None,
                    supersedes: None,
                    retry_of: None,
                    provenance: None,
                },
            )
            .await
            .map_err(to_tool_err)?;
        // `index.ts:1692-1706`: the dismissal runs ONLY on a confirmed delivery (`:1718`); a
        // failed delivery still dismisses when the reason is exactly "Session not found"
        // (`:1711`, the peer is gone — the ask can never be answered) but for any other
        // failure reason leaves the state untouched entirely so the ask remains pending for
        // a retry. Both live branches are pi's `dismissIncomingAsk`, which ALSO splices the
        // message out of the pending-idle queue — without that, replying to a message that
        // arrived mid-run gets it re-injected by the flush when the run ends.
        if result.delivered || result.reason.as_deref() == Some("Session not found") {
            crate::inbound::dismiss_incoming_ask(&self.state, &target.message.id);
        }
        if result.delivered {
            if let Some(services) = self.state.host_services() {
                // The call stays a statement rather than a `&&` let-chain in the outer guard's
                // condition (clippy::collapsible_if): the audit append has a side effect.
                let appended = services.append_entry(
                    "intercom_sent",
                    &serde_json::json!({
                        "to": target.from.name.clone().unwrap_or_else(|| target.from.id.clone()),
                        "message": {
                            "text": message,
                            "attachments": params.attachments,
                            "replyTo": target.message.id,
                        },
                        "messageId": result.id,
                        "timestamp": now_ms(),
                    }),
                );
                if let Err(e) = appended {
                    tracing::warn!(error = %e, kind = "intercom_sent", "intercom: failed to append audit entry");
                }
            }
            // `v0.10.1 index.ts:2233`: `Reply sent to ${target.from.name || target.from.id}`
            // — name preferred over id (JS `||`, so a blank name falls through to the id),
            // and NO trailing period. `:2234` carries `{ messageId, delivered: true,
            // replyTo: target.message.id }` — here `replyTo` is unconditional, unlike the
            // `send` arm's spread, because a reply always has one.
            // ICOM-054 — `{ ...deliveryDetails(result), replyTo: target.message.id }`
            // (`v0.13.0 index.ts:2557`); `replyTo` stays unconditional, unlike the `send` arm's
            // spread, because a reply always has one.
            let mut details = crate::tools::delivery_details(&result);
            if let Some(map) = details.as_object_mut() {
                map.insert("replyTo".to_string(), serde_json::json!(target.message.id));
            }
            Ok(detailed_result(
                format!("Reply sent to {}", display_name(&target.from)),
                details,
            ))
        } else {
            // `v0.10.1 index.ts:2222-2225`: the failure names the peer and keeps pi's
            // fallback reason.
            let reason = result
                .reason
                .unwrap_or_else(|| "Session may not exist or has disconnected.".to_string());
            Err(ToolError::new(format!(
                "Reply to \"{}\" was not delivered: {reason}",
                display_name(&target.from)
            )))
        }
    }
}

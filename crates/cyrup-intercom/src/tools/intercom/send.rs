//! `intercom{action:"send"}` (`v0.10.1 index.ts:1971-2061`; `v0.16.0 index.ts:2608-2625`) — the
//! non-blocking mailbox delivery. The arm validates its parameters and hands the rest to
//! [`IntercomTool::deliver_message`], the delivery `handover` shares: the confirm gate, the
//! active-ask-turn misdirection guard (ICOM-060), the inferred-reply inference and the audit entry
//! all live there.

use std::sync::Arc;

use cyrup_core::{CancelToken, ToolError, ToolResult};

use crate::transport::client::IntercomClient;

use super::deliver::{DeliveryKind, DeliveryRequest};
use super::{IntercomParams, IntercomTool};

impl IntercomTool {
    pub(super) async fn action_send(
        &self,
        params: &IntercomParams,
        client: &Arc<IntercomClient>,
        cancel: &CancelToken,
    ) -> Result<ToolResult, ToolError> {
        // `v0.10.1 index.ts:1973-1978`: `if ((!to && !cwd) || !message)` — ONE guard and one
        // message covering all three params, because `cwd` is an alternative addressing mode
        // rather than an extra filter. A `to`-only requirement made cross-directory
        // coordination impossible without knowing the peer's name in advance.
        let to = params.to.as_deref().filter(|v| !v.trim().is_empty());
        let cwd = params.cwd.as_deref().filter(|v| !v.trim().is_empty());
        let message = match params.message.as_deref().filter(|v| !v.trim().is_empty()) {
            Some(message) if to.is_some() || cwd.is_some() => message,
            _ => {
                return Err(ToolError::new(
                    "Missing 'to' or 'cwd', or missing 'message' parameter",
                ));
            }
        };
        // `replyTo` is read by truthiness throughout `deliverMessage` (`replyTo ? null : …`,
        // `!replyTo && …`), so an empty string is no `replyTo` at all.
        let reply_to = params.reply_to.as_deref().filter(|v| !v.is_empty());
        self.deliver_message(
            client,
            cancel,
            DeliveryRequest {
                to,
                cwd,
                open_project_pane_if_missing: params.open_project_pane_if_missing.unwrap_or(false),
                focus: params.focus.unwrap_or(true),
                message,
                kind: DeliveryKind::Send {
                    attachments: params.attachments.as_deref(),
                    reply_to,
                    supersedes: params.supersedes.as_deref(),
                    retry_of: params.retry_of.as_deref(),
                },
            },
        )
        .await
    }
}

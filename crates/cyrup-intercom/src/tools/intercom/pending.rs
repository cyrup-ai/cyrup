//! `intercom{action:"pending"}` (`v0.10.1 index.ts:1740-1755`, `:2237-2258`) — the unresolved
//! inbound asks, one row each.

use std::sync::Arc;

use cyrup_core::{ToolError, ToolResult};

use crate::tools::text_result;
use crate::transport::client::IntercomClient;
use crate::transport::protocol::now_ms;

use super::{IntercomParams, IntercomTool};

impl IntercomTool {
    pub(super) async fn action_pending(
        &self,
        _params: &IntercomParams,
        _client: &Arc<IntercomClient>,
    ) -> Result<ToolResult, ToolError> {
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
                format!(
                    "- {} · {} · {}s ago · {}",
                    who, c.message.id, elapsed, preview
                )
            })
            .collect();
        Ok(text_result(format!(
            "**Pending asks:**\n{}",
            rows.join("\n")
        )))
    }
}

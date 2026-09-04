//! `intercom{action:"cancel"}` (`v0.10.1 index.ts:1943-1969`) — withdraw a message this session
//! already sent.

use std::sync::Arc;

use cyrup_core::{ToolError, ToolResult};

use crate::tools::detailed_result;
use crate::transport::client::IntercomClient;

use super::{IntercomParams, IntercomTool};

impl IntercomTool {
    // ICOM-017 — `case "cancel"` (`v0.10.1 index.ts:1943-1969`). Placed here, between
    // `list-cwd` and `send`, in upstream's own order.
    //
    // This is the action that makes the whole receipt/control half reachable from the model:
    // without it a stale ask sits in the peer's `pending` list until the ask timeout, and
    // the broker's `handle_cancel_message` (ported with ICOM-010) had no caller at all.
    pub(super) async fn action_cancel(
        &self,
        params: &IntercomParams,
        client: &Arc<IntercomClient>,
    ) -> Result<ToolResult, ToolError> {
        let Some(message_id) = params.message_id.clone().filter(|v| !v.trim().is_empty()) else {
            // Upstream answers `{ text: "Missing 'messageId' parameter", details: { error:
            // true } }` — a non-error RESULT. cyrup renders every such arm as a `ToolError`
            // (see the identical `Missing 'to' or 'cwd', or missing 'message' parameter`
            // guard in `send`, now `send.rs`); the text is upstream's, byte for byte.
            return Err(ToolError::new("Missing 'messageId' parameter"));
        };
        let result = client.cancel_message(&message_id).await.map_err(|e| {
            // `catch (error) { … \`Failed to cancel message: ${getErrorMessage(error)}\` }`
            // (`:1964-1968`).
            ToolError::new(format!("Failed to cancel message: {e}"))
        })?;
        if !result.delivered {
            // `result.reason ?? "Message may not exist or may belong to another sender."`
            // (`:1955`) — `??`, so an empty-string reason is KEPT, unlike the `||` fallbacks
            // elsewhere in this file.
            let error_text = result.reason.clone().unwrap_or_else(|| {
                "Message may not exist or may belong to another sender.".to_string()
            });
            return Ok(detailed_result(
                format!("Cancellation for {message_id} was not delivered: {error_text}"),
                serde_json::json!({
                    "messageId": message_id,
                    "delivered": false,
                    "reason": result.reason,
                }),
            ));
        }
        Ok(detailed_result(
            format!("Cancellation requested for {message_id}"),
            serde_json::json!({ "messageId": message_id, "delivered": true }),
        ))
    }
}

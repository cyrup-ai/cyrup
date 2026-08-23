//! `intercom{action:"status"}` (`v0.10.1 index.ts:1757-1770`, `:2260-2270`) — the four-line
//! connectivity block.

use std::sync::Arc;

use cyrup_core::{ToolError, ToolResult};

use crate::tools::text_result;
use crate::transport::client::IntercomClient;

use super::{IntercomParams, IntercomTool, to_tool_err};

impl IntercomTool {
    pub(super) async fn action_status(
        &self,
        _params: &IntercomParams,
        client: &Arc<IntercomClient>,
    ) -> Result<ToolResult, ToolError> {
        // `index.ts:1765`: a four-line markdown block, not a pipe-delimited one-liner.
        // `Connected: Yes` is a literal upstream — the branch only runs after
        // `ensureConnected` has already succeeded (here: `connect::ensure_connected` in `dispatch`),
        // so there is no "disconnected" rendering to reach. A failing `listSessions` is
        // upstream's `Failed to get status: …` error result, which this crate renders as a
        // `ToolError` throughout (cf. the `list` branch's `Failed to list sessions`).
        let session_id = client.session_id().unwrap_or_else(|| "<none>".to_string());
        let count = client.list_sessions().await.map_err(to_tool_err)?.len();
        Ok(text_result(format!(
            "**Intercom Status:**\nConnected: Yes\nSession ID: {session_id}\nActive sessions: {count}"
        )))
    }
}

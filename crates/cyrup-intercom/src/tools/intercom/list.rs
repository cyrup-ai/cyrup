//! `intercom{action:"list"}` (`v0.10.1 index.ts:1478-1507`, `:1859-1893`) — the whole roster,
//! split into a "Current session" / "Other sessions" pair.

use std::sync::Arc;

use cyrup_core::{ToolError, ToolResult};

use crate::identity::short_session_id;
use crate::tools::text_result;
use crate::transport::client::IntercomClient;
use crate::transport::protocol::SessionInfo;

use super::{IntercomParams, IntercomTool, format_session_list_row, to_tool_err};

impl IntercomTool {
    pub(super) async fn action_list(
        &self,
        _params: &IntercomParams,
        client: &Arc<IntercomClient>,
    ) -> Result<ToolResult, ToolError> {
        // `index.ts:1478-1507`: split into a "Current session" / "Other sessions" pair, keyed
        // off the broker-reported current session's own `cwd` (not the locally-captured one),
        // and error out (rather than silently rendering a flat list) if the broker's session
        // list is missing this session entirely.
        let self_id = client.session_id();
        let sessions = client.list_sessions().await.map_err(to_tool_err)?;
        let current_session = self_id
            .as_deref()
            .and_then(|id| sessions.iter().find(|s| s.id == id));
        let Some(current_session) = current_session else {
            return Err(ToolError::new(
                "Current session is missing from intercom session list.",
            ));
        };
        let current_cwd = current_session.cwd.clone();
        // `v0.10.1 index.ts:1872`: the addressable column is a DISTINGUISHING prefix
        // computed over the whole roster, not a fixed 8-char slice. UUIDv7 ids minted in the
        // same millisecond share far more than 8 characters, so the fixed slice printed the
        // same `(abcdef12)` for two peers — and that string was exactly what the model was
        // told to address them by.
        let prefixes = crate::identity::session_id_prefixes(sessions.iter().map(|s| s.id.as_str()));
        let id_prefix = |s: &SessionInfo| {
            prefixes
                .get(&s.id)
                .cloned()
                .unwrap_or_else(|| short_session_id(&s.id))
        };
        let current_section = format!(
            "**Current session:**\n{}",
            format_session_list_row(
                current_session,
                &current_cwd,
                true,
                &id_prefix(current_session)
            )
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
                    .map(|s| format_session_list_row(s, &current_cwd, false, &id_prefix(s)))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        Ok(text_result(format!("{current_section}\n\n{other_section}")))
    }
}

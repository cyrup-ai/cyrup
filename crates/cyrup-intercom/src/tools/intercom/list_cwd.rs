//! `intercom{action:"list-cwd"}` (`v0.10.1 index.ts:1895-1941`) — the roster filtered to one
//! working directory.

use std::sync::Arc;

use cyrup_core::{ToolError, ToolResult};

use crate::identity::short_session_id;
use crate::tools::text_result;
use crate::transport::client::IntercomClient;
use crate::transport::protocol::SessionInfo;

use super::{IntercomParams, IntercomTool, format_session_list_row, resolve_target_cwd, to_tool_err};

impl IntercomTool {
    // `v0.10.1 index.ts:1895-1941`: the same roster, filtered to one working directory —
    // the common supervisor query ("who else is in this repo?"), which was otherwise
    // unanswerable without knowing every peer's name in advance.
    pub(super) async fn action_list_cwd(
        &self,
        params: &IntercomParams,
        client: &Arc<IntercomClient>,
    ) -> Result<ToolResult, ToolError> {
        let self_id = client.session_id();
        let sessions = client.list_sessions().await.map_err(to_tool_err)?;
        let current_session =
            self_id.as_deref().and_then(|id| sessions.iter().find(|s| s.id == id));
        let Some(current_session) = current_session else {
            return Err(ToolError::new("Current session is missing from intercom session list."));
        };
        // `v0.10.1 index.ts:1903-1907`: default to the current session's cwd; an explicit
        // `cwd` overrides, with a relative path resolved AGAINST the current session's cwd
        // and `"."` meaning the current cwd.
        let filter_cwd = resolve_target_cwd(
            &current_session.cwd,
            params.cwd.as_deref().unwrap_or("."),
        );
        let other_sessions: Vec<&SessionInfo> = sessions
            .iter()
            .filter(|s| {
                self_id.as_deref() != Some(s.id.as_str())
                    && crate::cwd::same_cwd(&s.cwd, &filter_cwd)
            })
            .collect();
        // `v0.10.1 index.ts:1913-1924`, comment verbatim: "Fail loud: filtering by a
        // directory with no peers while the session's OWN cwd has some otherwise reads as a
        // misleading empty result (common when a caller passes a guessed parent cwd)."
        let mut empty_note = "No other sessions in this directory.".to_string();
        if other_sessions.is_empty()
            && !crate::cwd::same_cwd(&filter_cwd, &current_session.cwd)
        {
            let here = sessions
                .iter()
                .filter(|s| {
                    self_id.as_deref() != Some(s.id.as_str())
                        && crate::cwd::same_cwd(&s.cwd, &current_session.cwd)
                })
                .count();
            if here > 0 {
                let plural = if here == 1 { "" } else { "s" };
                empty_note.push_str(&format!(
                    " Your session's cwd is {} ({here} peer{plural} there) — call list-cwd without a cwd argument to list them.",
                    current_session.cwd
                ));
            }
        }
        let prefixes = crate::identity::session_id_prefixes(sessions.iter().map(|s| s.id.as_str()));
        let id_prefix = |s: &SessionInfo| {
            prefixes.get(&s.id).cloned().unwrap_or_else(|| short_session_id(&s.id))
        };
        let current_section = format!(
            "**Current session:**\n{}",
            format_session_list_row(current_session, &current_session.cwd, true, &id_prefix(current_session))
        );
        let other_section = if other_sessions.is_empty() {
            format!("**Other sessions (cwd: {filter_cwd}):**\n{empty_note}")
        } else {
            format!(
                "**Other sessions (cwd: {filter_cwd}):**\n{}",
                other_sessions
                    .iter()
                    .map(|s| format_session_list_row(s, &current_session.cwd, false, &id_prefix(s)))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        Ok(text_result(format!("{current_section}\n\n{other_section}")))
    }
}

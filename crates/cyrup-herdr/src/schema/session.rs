//! Mirrors `tmp/herdr/src/api/schema/session.rs` — the whole-session record.

use serde::Deserialize;

use super::agents::AgentInfo;
use super::panes::{PaneInfo, PaneLayoutSnapshot};
use super::tabs::TabInfo;
use super::workspaces::WorkspaceInfo;

/// `SessionSnapshot` (`tmp/herdr/src/api/schema/session.rs:8-23`) — the answer to
/// `session.snapshot`.
///
/// **Flat, not nested.** Workspaces do not contain their tabs and tabs do not contain their panes:
/// every record is in its own top-level `Vec` and the tree is rebuilt from
/// [`TabInfo::workspace_id`], [`PaneInfo::tab_id`] and [`AgentInfo::pane_id`]. A consumer that
/// wants a tree builds one; nothing here does it for them, because the flat form is what herdr
/// sends.
///
/// `version` and `protocol` repeat what `ping` answers, so a bootstrap that begins with a snapshot
/// need not also ping.
///
/// This is the one genuinely large reply on the socket, and the reason
/// [`crate::MAX_RESPONSE_BYTES`] is 4 MiB rather than herdr's own 1 MiB request bound.
///
/// **Ordering against `events.subscribe` is structural, not advisory.** herdr serves one request
/// per connection (`tmp/herdr/src/api/server.rs:156-317`), so a snapshot and a subscription cannot
/// share one; the subscription must be opened and acknowledged *first*, or events that fire while
/// the snapshot is being built are lost
/// (`tmp/herdr/docs/preview/website/src/content/docs/socket-api.mdx:118-130`). This crate's
/// stream half owns that ordering; a caller reaching for [`crate::HerdrClient::session_snapshot`]
/// alone is taking a point-in-time reading with no guarantee about what happens next.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SessionSnapshot {
    /// herdr's version string, as `ping` reports it.
    pub version: String,
    /// The binary client-shell protocol generation, as `ping` reports it.
    pub protocol: u32,
    /// The focused workspace, when there is one.
    #[serde(default)]
    pub focused_workspace_id: Option<String>,
    /// The focused tab, when there is one.
    #[serde(default)]
    pub focused_tab_id: Option<String>,
    /// The focused pane, when there is one.
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    /// Every workspace.
    pub workspaces: Vec<WorkspaceInfo>,
    /// Every tab, across every workspace.
    pub tabs: Vec<TabInfo>,
    /// Every pane, across every tab.
    pub panes: Vec<PaneInfo>,
    /// One layout per tab.
    pub layouts: Vec<PaneLayoutSnapshot>,
    /// Every agent herdr can see.
    pub agents: Vec<AgentInfo>,
}

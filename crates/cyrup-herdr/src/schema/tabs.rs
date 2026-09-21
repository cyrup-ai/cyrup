//! Mirrors `tmp/herdr/src/api/schema/tabs.rs`.

use serde::Deserialize;

use super::common::AgentStatus;

/// `TabRenameParams` (`tmp/herdr/src/api/schema/tabs.rs:27-31`).
///
/// Both fields are required — the published schema's `"required"` is `["tab_id","label"]`. There
/// is no "clear the label" shape here: `label` is a bare `String`, and herdr stores it as the
/// tab's custom name (`set_custom_name`, `tmp/herdr/src/app/api/tabs.rs:158`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TabRenameParams {
    /// The tab to rename, e.g. `"w1:t1"` — the value herdr injects as `HERDR_TAB_ID`.
    pub tab_id: String,
    /// The new label.
    pub label: String,
}

impl TabRenameParams {
    /// Rename `tab_id` to `label`.
    pub fn new(tab_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            tab_id: tab_id.into(),
            label: label.into(),
        }
    }
}

/// `TabInfo` (`tmp/herdr/src/api/schema/tabs.rs:39-48`) — herdr's tab record.
///
/// Answered by **both** `tab.get` and `tab.rename` (`tmp/herdr/src/app/api/tabs.rs:44,171`), so a
/// rename returns the tab as it now reads rather than a bare acknowledgement. That is why the
/// pane-owned-tab-label pattern — read the tab, decide, rename, keep the answer — is two calls and
/// not three.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TabInfo {
    /// The public tab id.
    pub tab_id: String,
    /// The owning workspace.
    pub workspace_id: String,
    /// The tab's 1-based position.
    pub number: usize,
    /// The displayed label — the custom name when one is set, herdr's own otherwise. Never
    /// `Option`: herdr always has something to show.
    pub label: String,
    /// Whether this is the workspace's active tab.
    pub focused: bool,
    /// How many panes it holds.
    pub pane_count: usize,
    /// The tab's rolled-up agent status.
    pub agent_status: AgentStatus,
}

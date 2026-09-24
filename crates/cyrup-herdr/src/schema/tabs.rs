//! Mirrors `tmp/herdr/src/api/schema/tabs.rs`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::common::AgentStatus;

/// `TabCreateParams` (`tmp/herdr/src/api/schema/tabs.rs:7-19`).
///
/// Answered by [`super::response::ResponseResult::TabCreated`] — the new tab and its root pane.
/// Saved-machine placement opens one per placed run inside an owned workspace
/// (`src/runs/shared/herdr-placed-run.ts:147` @v0.68.0).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TabCreateParams {
    /// The workspace to open the tab in; herdr's focused workspace when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// The root pane's working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Whether to focus it.
    pub focus: bool,
    /// The displayed label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Environment for the root pane's shell.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

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

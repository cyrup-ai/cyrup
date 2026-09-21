//! Mirrors `tmp/herdr/src/api/schema/workspaces.rs`.
//!
//! Only the records reached through [`super::session::SessionSnapshot`] live here. None of
//! `workspace.*`'s nine methods is ported — no cyrup consumer creates, closes, renames or reorders
//! the user's workspaces — but a snapshot carries `workspaces: Vec<WorkspaceInfo>`
//! (`tmp/herdr/src/api/schema/session.rs:18`), so the record is not optional even though its verbs
//! are.
//!
//! The file exists rather than these two types being folded into `session.rs` because herdr keeps
//! them in `schema/workspaces.rs`, and the mirroring rule is what makes a diff against a future
//! pin line-for-line.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::common::AgentStatus;

/// `WorkspaceInfo` (`tmp/herdr/src/api/schema/workspaces.rs:61-76`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkspaceInfo {
    /// The public workspace id, e.g. `"w1"` — the value herdr injects as `HERDR_WORKSPACE_ID`.
    pub workspace_id: String,
    /// The workspace's 1-based position.
    pub number: usize,
    /// The displayed label.
    pub label: String,
    /// Whether this workspace is the active one.
    pub focused: bool,
    /// How many panes it holds, across every tab.
    pub pane_count: usize,
    /// How many tabs it holds.
    pub tab_count: usize,
    /// Its active tab.
    pub active_tab_id: String,
    /// Its rolled-up agent status.
    pub agent_status: AgentStatus,
    /// Tokens reported through `workspace.report_metadata`.
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    /// Set when this workspace is a git worktree herdr opened.
    #[serde(default)]
    pub worktree: Option<WorkspaceWorktreeInfo>,
}

/// `WorkspaceWorktreeInfo` (`tmp/herdr/src/api/schema/workspaces.rs:78-85`).
///
/// Decoded, not acted on. cyrup owns its own worktrees through `gix`
/// (`crates/cyrup-ext-subagents/src/spawn/worktree.rs`); this record only says what herdr believes
/// about a workspace it opened, which is worth *reading* when reconciling a fleet view against the
/// user's terminal.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceWorktreeInfo {
    /// herdr's key for the origin repository.
    pub repo_key: String,
    /// The origin repository's name.
    pub repo_name: String,
    /// The origin repository's root.
    pub repo_root: String,
    /// This worktree's checkout.
    pub checkout_path: String,
    /// `false` when the "worktree" is the primary checkout itself.
    pub is_linked_worktree: bool,
}

//! Mirrors `tmp/herdr/src/api/schema/worktrees.rs`.
//!
//! Only the **record** lives here, for the same reason as [`super::workspaces`]: none of
//! `worktree.*`'s four methods is ported — herdr worktrees create herdr *workspaces*, and cyrup
//! owns its own worktrees through `gix` (`crates/cyrup-ext-subagents/src/spawn/worktree.rs`) — but
//! three lifecycle events carry a `WorktreeInfo` (`worktree.created`, `worktree.opened`,
//! `worktree.removed`, `tmp/herdr/src/api/schema/events.rs:455-471`), so a subscriber that asked
//! for them needs the record even though it drives none of the verbs.
//!
//! The file exists rather than these fields being folded into `events.rs` because herdr keeps them
//! in `schema/worktrees.rs`, and the mirroring rule is what makes a diff against a future pin
//! line-for-line.

use serde::Deserialize;

/// `WorktreeInfo` (`tmp/herdr/src/api/schema/worktrees.rs:70-82`).
///
/// Note that this is a **git** worktree as herdr sees it, which is not the same record as
/// [`super::workspaces::WorkspaceWorktreeInfo`]: that one is the provenance stamped on a
/// *workspace* herdr opened for a worktree, this one is the checkout itself.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorktreeInfo {
    /// The checkout's path.
    pub path: String,
    /// The checked-out branch, absent on a detached head.
    #[serde(default)]
    pub branch: Option<String>,
    /// Whether the repository is bare.
    pub is_bare: bool,
    /// Whether the head is detached.
    pub is_detached: bool,
    /// Whether git would prune this worktree.
    pub is_prunable: bool,
    /// `false` when this is the primary checkout rather than a linked worktree.
    pub is_linked_worktree: bool,
    /// The herdr workspace this checkout is open in, when it is open.
    #[serde(default)]
    pub open_workspace_id: Option<String>,
    /// The displayed label.
    pub label: String,
}

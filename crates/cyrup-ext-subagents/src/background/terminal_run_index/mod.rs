//! Session-scoped terminal-run index — "what finished recently, in my session" without walking
//! every run directory.
//!
//! Ports pi `runs/background/terminal-run-index.ts` (138 LOC) in full.
//!
//! # The problem this solves
//!
//! `<async_root>` is per-**cwd** ([`crate::background::run_artifact_roots`]), shared by every
//! concurrent cyrup instance in the directory. Answering "which of MY runs reached a terminal
//! state recently" from that root means `read_dir`-ing every run ever launched there and `stat`ing
//! each run's `status.json` — slow, and the exact accept-everything scan pattern
//! [`crate::background::result_index`] exists to close (`result_index/mod.rs`). This index makes
//! the answer a bounded read of one session-keyed directory.
//!
//! # Layout
//!
//! ```text
//! <async_root>/
//!   <runId>/…                                      the run directories (unchanged)
//!   .terminal-runs/                                THIS index — a dot-prefixed SIBLING of the runs
//!     <enc(sessionId)>/
//!       <0-padded endedAt>-<enc(runId)>.json       one advisory marker per terminal transition
//! ```
//!
//! `<enc(…)>` is [`crate::identity::IndexSegment`] — one write key, never the alias fan-out
//! (upstream's `sessionIndexDir` applies the segment encoder exactly once,
//! `terminal-run-index.ts:28-30`).
//! [`read::read_recent_terminal_run_index`]'s re-verification of every candidate against the run's
//! live `status.json` is what makes the single hashed key safe to use as an address.
//!
//! # Placement obliges every async-root scanner to skip it
//!
//! `.terminal-runs` lives INSIDE the async root, so every scanner that treats each subdirectory as
//! a run would otherwise see it: reconcile it, offer it as a prefix match, or resolve it into a
//! phantom run location. Upstream guards explicitly (`async-status.ts:235`, `:502`); cyrup's
//! scanners apply [`is_reserved_async_root_entry`].
//!
//! # Every marker is advisory
//!
//! A marker can be unlinked at any time without losing data — the run's own `status.json` and
//! terminal [`crate::background::ResultFile`] stay authoritative. That is why the reader deletes
//! invalid/foreign/disagreeing/duplicate markers on sight, why the writer is best-effort at every
//! call site, and why the one consumer ([`crate::tui::fleet`]'s history roster) falls back to its
//! full scan when the index is empty or unreadable.
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs      facade + layout diagram + the `.terminal-runs` reserved-name predicate; no logic
//! entry.rs    the record AND its address: TerminalRunIndexEntry, is_indexed_state, path builders
//! update.rs   the writer  — update_terminal_run_index
//! read.rs     the reader  — read_recent_terminal_run_index
//! ```
//!
//! `entry.rs` carries the path builders because "what an entry is" and "where it lives" are one
//! concern here (unlike [`crate::background::result_index`], which has four index families and
//! therefore earns a `paths.rs`).

mod entry;
mod read;
mod update;

pub use entry::{TERMINAL_RUN_INDEX_DIR, TerminalIndexVersion, TerminalRunIndexEntry};
pub use read::read_recent_terminal_run_index;
pub use update::update_terminal_run_index;

/// `true` for a reserved index directory that sits inside the async root but is not a run.
///
/// pi `entry !== ACTIVE_RUN_INDEX_DIR && entry !== TERMINAL_RUN_INDEX_DIR`
/// (`async-status.ts:235`, `:502`). `.active-runs` is named here even though
/// `active-run-index.ts` is unported, so the guard does not have to be revisited when it lands.
#[must_use]
pub fn is_reserved_async_root_entry(name: &str) -> bool {
    name == TERMINAL_RUN_INDEX_DIR || name == ".active-runs"
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    #[test]
    fn both_reserved_index_dirs_are_recognized_and_runs_are_not() {
        assert!(is_reserved_async_root_entry(".terminal-runs"));
        assert!(is_reserved_async_root_entry(".active-runs"));
        assert!(!is_reserved_async_root_entry("abc12345"));
        // A dot prefix alone is NOT reserved — upstream's guard is two literal names, not a glob.
        assert!(!is_reserved_async_root_entry(".hidden"));
        assert!(!is_reserved_async_root_entry(""));
    }
}

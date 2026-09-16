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

/// pi `RUN_TOMBSTONE_PREFIX` (`runs/background/async-retention.ts:23` @`v0.68.0`) — the name
/// prefix [`crate::background::async_retention`] renames a run tree onto before deleting it.
///
/// # Why the literal lives HERE and not in `async_retention`
///
/// A `.deleting-run-<uuid>` directory is created **inside the async root**, beside the run
/// directories, and a tombstone can outlive the pass that minted it (that is exactly what
/// [`crate::background::async_retention::ASYNC_RETENTION_TOMBSTONE_GRACE_MS`] exists for). The
/// reserved-name vocabulary belongs with the predicate that enforces it —
/// [`is_reserved_async_root_entry`] — for the same reason `.active-runs` was named here before
/// `active_run_index` existed: so the guard does not have to be revisited when the module that
/// writes the name lands.
pub const RUN_TOMBSTONE_PREFIX: &str = ".deleting-run-";

/// The per-`cwd` maintenance directory [`crate::background::async_retention`] keeps its
/// run-tombstone markers in (and that SCOPE_14's retention lock and cursor will join):
/// `<async_root>/.async-retention/`.
///
/// [CYRUP-DELTA] pi derives its `maintenanceRoot` as `path.dirname(asyncDirRoot)`
/// (`async-retention.ts:658`), which works because pi's `DIRS.async` is FLAT — one directory for
/// every project. cyrup's async root is already per-`cwd`
/// ([`crate::background::run_artifact_roots`]), so `dirname` would be
/// `<run_scratch>/async/`, shared by EVERY working directory on the machine: a lock there would
/// serialise retention across unrelated projects and a cursor there would be meaningless. Nesting
/// the maintenance root INSIDE the async root keys it by `cwd` for free, at the cost of one more
/// arm on [`is_reserved_async_root_entry`] — which this module was already growing for
/// [`RUN_TOMBSTONE_PREFIX`].
pub const ASYNC_RETENTION_MAINTENANCE_DIR: &str = ".async-retention";

/// `true` for a reserved entry that sits inside the async root but is not a run.
///
/// pi `entry !== ACTIVE_RUN_INDEX_DIR && entry !== TERMINAL_RUN_INDEX_DIR`
/// (`async-status.ts:235`, `:502`), plus the two names cyrup's async-root retention introduces.
/// `.active-runs` was named here before `active_run_index.rs` existed, so the guard did not have
/// to be revisited when it landed.
///
/// # The prefix arm is a correctness property, not a tidiness one
///
/// Six call sites, across five production scanners, treat every subdirectory of the async root as
/// a run and funnel through this predicate: [`crate::tui::fleet`]'s history roster,
/// `run_status::resolve_run_id`'s exact and prefix arms, `run_status::active_run_candidates`,
/// [`crate::background::resolve_async_run_id`] and the executor's status listing. Without the
/// [`RUN_TOMBSTONE_PREFIX`] arm a tombstone that outlives its pass appears in
/// `/subagents-fleet`, becomes an ambiguous prefix match in `resolve_run_id`, and mints a phantom
/// `AsyncRunLocation` (`run_id_resolver.rs`: *"`async_dir.exists()` below is TRUE for it and
/// would otherwise mint a phantom `AsyncRunLocation`"*).
///
/// This is deliberately still **not** a dot-glob: `.hidden` is not reserved, and the test below
/// pins that.
#[must_use]
pub fn is_reserved_async_root_entry(name: &str) -> bool {
    name == TERMINAL_RUN_INDEX_DIR
        || name == ".active-runs"
        || name == ASYNC_RETENTION_MAINTENANCE_DIR
        || name.starts_with(RUN_TOMBSTONE_PREFIX)
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

    /// The async-root retention names (SCOPE_13). A `.deleting-run-*` tombstone is a DIRECTORY
    /// inside the async root that can outlive the pass that minted it, so every async-root
    /// scanner must skip it — this is the arm that makes that true, and it is a PREFIX arm
    /// because the suffix is a fresh random id per tombstone.
    #[test]
    fn the_tombstone_prefix_is_a_reserved_async_root_entry() {
        assert!(is_reserved_async_root_entry(".deleting-run-abc"));
        assert!(is_reserved_async_root_entry(RUN_TOMBSTONE_PREFIX));
        assert!(is_reserved_async_root_entry(
            ".deleting-run-0123456789abcdef0123456789abcdef"
        ));
        assert!(is_reserved_async_root_entry(
            ASYNC_RETENTION_MAINTENANCE_DIR
        ));
        // Still not a dot-glob, and still not a prefix match against a bare run id.
        assert!(!is_reserved_async_root_entry(".hidden"));
        assert!(!is_reserved_async_root_entry("deleting-run-abc"));
        assert!(!is_reserved_async_root_entry(
            "0123456789abcdef0123456789abcdef"
        ));
    }
}

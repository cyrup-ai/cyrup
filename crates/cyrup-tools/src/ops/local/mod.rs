//! The default local backend over `tokio::fs` / `tokio::process` (arch-03 §3.3, §6.5).
//!
//! [`LocalFs`] (`fs`) is an indirection over the real filesystem; [`LocalProc`] (`proc`) runs
//! commands through the detected shell, streams combined stdout+stderr, and kills on
//! cancel/timeout. The process half is split by concern: `command` builds the OS command,
//! `signal` holds the raw `killpg`/`kill(2)` primitives, `tracking` the process-global
//! detached-child registry, and `guard` the RAII guard that binds a spawned group's lifetime to
//! the future that owns it. `proc`'s module doc carries the reason the two `ProcOps` methods
//! escalate DIFFERENTLY, 1:1 with their DIFFERENT real Pi consumers.
//!
//! The only `unsafe` in the crate lives in this module, isolated to the unix process-group calls
//! (`setsid`/`killpg`, `command::build_command` / `signal::send_sigkill_tree` /
//! [`kill_process_tree`] and `guard::KillTreeOnDrop`'s `Drop`, used ONLY by [`LocalProc::exec`]
//! and its shutdown drain), the single-pid `kill(2)` calls ([`terminate_pid`]/[`kill_pid`]), and
//! the `access(2)` probe in [`LocalFs`] — each with safety comments.
//!
//! [`LocalProc::exec`] additionally enrolls each spawned shell in the process-global
//! `TRACKED_DETACHED_CHILD_PIDS` registry, so a shutdown signal can `killpg` every detached bash
//! child still running BEFORE any teardown runs — Pi's `trackedDetachedChildPids` /
//! `killTrackedDetachedChildren` (`utils/shell.ts:180-195` @v0.83.0), drained as the first statement
//! of all three of its signal handlers. See [`kill_tracked_detached_children`] (SEAM-S03).
//!
//! [`LocalProc::exec`]: crate::ops::ProcOps::exec

pub(crate) mod command;
pub(crate) mod fs;
pub(crate) mod guard;
pub(crate) mod proc;
pub(crate) mod signal;
pub(crate) mod tracking;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU64, Ordering};

pub use fs::LocalFs;
pub use proc::LocalProc;
pub use signal::{kill_pid, kill_process_tree, terminate_pid};
pub use tracking::{
    kill_tracked_detached_children, track_detached_child_pid, untrack_detached_child_pid,
};

// The win32 arm of `FsOps::access`; its only in-crate consumer outside `fs` is the `#[cfg(test)]`
// `crate::tests::read_access_errno` module, so the re-export is unused in a non-test build.
#[allow(unused_imports)]
pub(crate) use fs::windows_access_result;

static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// Process-unique-ish suffix for temp files (no rng dependency).
pub(crate) fn unique_suffix() -> String {
    let pid = std::process::id();
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{pid:x}-{nanos:x}-{n:x}")
}

//! The process-global detached-child registry (SEAM-S03).
//!
//! A literal port of Pi's module-level `trackedDetachedChildPids` set and its
//! `killTrackedDetachedChildren` drain (`utils/shell.ts:180-195` @v0.83.0). Filled at
//! [`LocalProc::exec`]'s spawn and emptied by [`KillTreeOnDrop`]; drained by a shutdown signal
//! handler before any teardown runs.
//!
//! [`KillTreeOnDrop`]: crate::ops::local::guard::KillTreeOnDrop
//! [`LocalProc::exec`]: crate::ops::ProcOps::exec

use super::signal::kill_process_tree;

/// Process-global registry of the detached bash children that are currently running — a literal
/// port of Pi's `const trackedDetachedChildPids = new Set<number>()`
/// (`packages/coding-agent/src/utils/shell.ts:180` @v0.83.0), whose own comment states the purpose:
/// "Detached child processes must be tracked so they can be killed on parent shutdown signals
/// (SIGHUP/SIGTERM)."
///
/// Filled at [`LocalProc::exec`]'s spawn and emptied when that exec finishes, mirroring Pi's two
/// call sites — `if (child.pid) trackDetachedChildPid(child.pid);` right after the
/// `detached: process.platform !== "win32"` spawn (`core/tools/bash.ts:108`) and
/// `if (child.pid) untrackDetachedChildPid(child.pid);` as the FIRST statement of that spawn's
/// `finally` (`bash.ts:142`). [`LocalProc::exec_argv`] deliberately does NOT participate: its real
/// consumer `execCommand` (`exec.ts:41-45`) passes no `detached` and Pi never tracks it.
///
/// Process-global on purpose, exactly as Pi's module-level `Set` is: it must survive session
/// replacement (`/new`, `/fork`, `switchSession`), which the per-session `session_cancel` route
/// [`send_sigkill_tree`] hangs off does not — that scoping difference is half of what SEAM-S03
/// records.
///
/// [`LocalProc::exec`]: crate::ops::ProcOps::exec
/// [`LocalProc::exec_argv`]: crate::ops::ProcOps::exec_argv
/// [`send_sigkill_tree`]: crate::ops::local::signal::send_sigkill_tree
static TRACKED_DETACHED_CHILD_PIDS: std::sync::Mutex<std::collections::BTreeSet<u32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Lock the registry, ignoring poisoning.
///
/// A panic while the set is held cannot corrupt it (a `BTreeSet<u32>` has no invariant a partial
/// mutation can break) and this runs on the shutdown path, where refusing to kill orphans because
/// some unrelated task panicked is strictly worse than proceeding. Pi has no lock at all — JS is
/// single-threaded — so there is no upstream behaviour to mirror here, only a Rust obligation.
pub(super) fn tracked_detached_child_pids()
-> std::sync::MutexGuard<'static, std::collections::BTreeSet<u32>> {
    TRACKED_DETACHED_CHILD_PIDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Pi `trackDetachedChildPid` (`utils/shell.ts:182-184` @v0.83.0), called at the bash spawn
/// (`bash.ts:108`).
pub fn track_detached_child_pid(pid: u32) {
    tracked_detached_child_pids().insert(pid);
}

/// Pi `untrackDetachedChildPid` (`utils/shell.ts:186-188` @v0.83.0), called from the bash spawn's
/// `finally` (`bash.ts:142`).
pub fn untrack_detached_child_pid(pid: u32) {
    tracked_detached_child_pids().remove(&pid);
}

/// Kill every still-running detached bash child and empty the registry — Pi
/// `killTrackedDetachedChildren` (`utils/shell.ts:190-195` @v0.83.0).
///
/// This is the FIRST statement of all three of Pi's signal handlers (`modes/print-mode.ts:55`,
/// `modes/rpc/rpc-mode.ts:373`, `modes/interactive/interactive-mode.ts:3663`) and of interactive's
/// two emergency paths (`emergencyTerminalExit` at `:3605`, the `uncaughtException` handler at
/// `:3631`), all @v0.83.0. It is synchronous and total: by the time anything can re-enter the
/// handler, the groups are already signalled.
///
/// CYRUP-DELTA — order of drain vs. kill. Pi loops the live `Set` and clears it AFTERWARDS
/// (`for (const pid of trackedDetachedChildPids) killProcessTree(pid); trackedDetachedChildPids
/// .clear();`, `shell.ts:191-194`). This takes the set out of the lock FIRST and kills without
/// holding it. Two Rust-only obligations force that: another thread's [`KillTreeOnDrop`] may be
/// blocked on `untrack_detached_child_pid` while this runs, so holding the lock across a
/// syscall-per-pid loop makes that thread wait on a shutdown path; and a re-entrant call would
/// deadlock a `std::sync::Mutex` where Pi's re-entered handler is merely a nested call over a
/// single-threaded `Set`. The observable result is identical — every pid present at entry is
/// killed, and none of them is present at exit.
///
/// [`KillTreeOnDrop`]: crate::ops::local::guard::KillTreeOnDrop
pub fn kill_tracked_detached_children() {
    drain_and_kill(&TRACKED_DETACHED_CHILD_PIDS);
}

/// The body of [`kill_tracked_detached_children`], parameterised over the registry.
///
/// Split out ONLY so the drain can be tested against a registry the test owns. Calling the real
/// drain from a test would kill every detached child tracked by whatever else is running in the
/// same process — harmless under the nextest gate (one process per test), a cross-test SIGKILL
/// under a threaded `cargo test`, and this project's rule is not to introduce a flake.
pub(super) fn drain_and_kill(registry: &std::sync::Mutex<std::collections::BTreeSet<u32>>) {
    let mut guard = registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let pids = std::mem::take(&mut *guard);
    drop(guard);
    for pid in pids {
        kill_process_tree(pid);
    }
}

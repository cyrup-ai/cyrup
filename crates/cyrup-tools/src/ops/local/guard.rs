//! [`KillTreeOnDrop`] — the RAII guard that ties a spawned shell's process GROUP, and its
//! membership of the [`super::tracking`] registry, to the lifetime of the future that owns it.
//!
//! Exists because a Rust future can be dropped at any `.await`, where pi's `async` abort/timeout
//! handling always settles; see the type's own doc comment for the full argument.

use super::tracking::{track_detached_child_pid, untrack_detached_child_pid};

/// RAII group-kill for [`LocalProc::exec`], armed at spawn and disarmed only once the child has
/// been reaped on the normal path.
///
/// **This closes a JS→Rust mechanism gap, not a missing feature.** Pi's abort and timeout handlers
/// hang off an `async` function that ALWAYS settles: `bash.ts:111-121` registers `onAbort` →
/// `killProcessTree` (`shell.ts:200-225`, `process.kill(-pid, "SIGKILL")`) and the same handler runs
/// on the timeout leg, so upstream cannot reach a state where the shell's process GROUP outlives the
/// call. A Rust future has no such guarantee — it can be dropped at ANY `.await`: a cancelled
/// `tokio::spawn`, a `tokio::time::timeout`, an unwinding panic, or runtime teardown all abandon
/// [`LocalProc::exec`]'s `select!` loop (in [`super::proc`]) without running a single one of its
/// `send_sigkill_tree` arms.
///
/// `tokio::process`'s own `kill_on_drop(true)` is NOT a substitute and must not be mistaken for one:
/// it SIGKILLs the SINGLE direct child, so every grandchild the `setsid` group contains survives as
/// an orphan still holding this process's stdio pipes. That survival is already on the record as an
/// unfixed consequence in `docs/gap-analysis/12-upstream-drift-pi-core.md` (the `DRIFT-043`
/// rejection note: "grandchildren do survive — single-pid kill, not killpg"); this type is what
/// makes the drop path do what every non-drop path already does.
///
/// The pid cannot be recycled underneath the `killpg`: the guard is disarmed only AFTER the loop has
/// observed `child.wait()`, and until then [`LocalProc::exec`] still owns the un-reaped `Child`, so
/// the pid — and therefore the process-group id, which `setsid` made equal to it — remains ours.
/// Declared AFTER `child` in `exec` so Rust's reverse-declaration drop order runs this guard while
/// that ownership still holds.
/// It also owns the registry membership from [`TRACKED_DETACHED_CHILD_PIDS`], and that half is
/// deliberately NOT affected by [`Self::disarm`]. Pi untracks in a `finally` (`bash.ts:142`), which
/// runs on the normal return, the abort throw and the timeout throw alike; the Rust equivalent of
/// "runs no matter how we leave" is `Drop`, not a statement placed after that `select!` loop. Putting
/// the untrack on the success path instead would leak the pid PERMANENTLY whenever the future is
/// dropped mid-flight — the same class of gap this guard's `killpg` half already closes — and a
/// leaked pid is worse than a merely-forgotten one: the next
/// [`kill_tracked_detached_children`] would `killpg` a pid this process no longer owns and that the
/// kernel may since have recycled onto an unrelated process group.
///
/// [`LocalProc::exec`]: crate::ops::ProcOps::exec
/// [`TRACKED_DETACHED_CHILD_PIDS`]: crate::ops::local::tracking::TRACKED_DETACHED_CHILD_PIDS
/// [`kill_tracked_detached_children`]: crate::ops::local::tracking::kill_tracked_detached_children
#[cfg(unix)]
pub(super) struct KillTreeOnDrop {
    pgid: Option<u32>,
    tracked: Option<u32>,
}

#[cfg(unix)]
impl KillTreeOnDrop {
    pub(super) fn arm(pid: Option<u32>) -> Self {
        // Pi `bash.ts:108`: `if (child.pid) trackDetachedChildPid(child.pid);` — the `if` is why
        // this is keyed off the `Option` rather than a placeholder pid.
        if let Some(pid) = pid {
            track_detached_child_pid(pid);
        }
        Self {
            pgid: pid,
            tracked: pid,
        }
    }

    /// The child has been reaped by the normal path; the group must NOT be signalled on drop.
    ///
    /// Registry membership is untouched here on purpose — see the type's doc comment. `Drop` still
    /// runs immediately afterwards (the guard is a local in [`LocalProc::exec`]), so the untrack is
    /// not deferred by disarming, only made unconditional.
    ///
    /// [`LocalProc::exec`]: crate::ops::ProcOps::exec
    pub(super) fn disarm(&mut self) {
        self.pgid = None;
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
impl Drop for KillTreeOnDrop {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid {
            // SAFETY: identical to `send_sigkill_tree` — `killpg(2)` reads two integers and touches
            // no memory. `ESRCH` (group already gone) is the expected benign outcome and is ignored.
            unsafe {
                libc::killpg(pgid as libc::pid_t, libc::SIGKILL);
            }
        }
        // Pi's `finally` (`bash.ts:142`), unconditional on how this exec ended.
        if let Some(pid) = self.tracked {
            untrack_detached_child_pid(pid);
        }
    }
}

/// Windows has no process-group primitive that matches `setsid`; [`build_command`] installs none
/// there either, so the guard degrades to `kill_on_drop`'s single-pid behaviour — the same shape
/// the non-unix arm of [`send_sigkill_tree`] already documents.
///
/// The registry half is NOT degraded: Pi tracks on every platform (`bash.ts:108` is outside any
/// platform check, and its `killProcessTree` has a `taskkill /F /T` arm — `shell.ts:203-212` — that
/// kills a tree without needing a process group).
///
/// [`build_command`]: crate::ops::local::command::build_command
/// [`send_sigkill_tree`]: crate::ops::local::signal::send_sigkill_tree
#[cfg(not(unix))]
pub(super) struct KillTreeOnDrop {
    tracked: Option<u32>,
}

#[cfg(not(unix))]
impl KillTreeOnDrop {
    pub(super) fn arm(pid: Option<u32>) -> Self {
        if let Some(pid) = pid {
            track_detached_child_pid(pid);
        }
        Self { tracked: pid }
    }
    pub(super) fn disarm(&mut self) {}
}

#[cfg(not(unix))]
impl Drop for KillTreeOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.tracked {
            untrack_detached_child_pid(pid);
        }
    }
}

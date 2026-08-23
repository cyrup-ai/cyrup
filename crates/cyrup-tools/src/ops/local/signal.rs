//! The raw unix kill primitives, and the crate's `unsafe` leaf.
//!
//! Two shapes, deliberately kept apart: a process-GROUP `killpg` for the `setsid`'d shell
//! [`LocalProc::exec`] spawns ([`kill_process_tree`], [`send_sigkill_tree`]), and a SINGLE-pid
//! `kill(2)` for the plain child [`LocalProc::exec_argv`] spawns ([`terminate_pid`], [`kill_pid`]).
//! See [`super::proc`]'s module doc for why the two methods diverge.
//!
//! [`LocalProc::exec`]: crate::ops::ProcOps::exec
//! [`LocalProc::exec_argv`]: crate::ops::ProcOps::exec_argv

/// Pi `killProcessTree` (`utils/shell.ts:200-225` @v0.83.0) addressed by PID ALONE, for the
/// [`kill_tracked_detached_children`] drain — which, unlike [`send_sigkill_tree`], holds no
/// `tokio::process::Child` for the pids it is killing.
///
/// Ports Pi's fallback, which [`send_sigkill_tree`] expresses differently: upstream tries
/// `process.kill(-pid, "SIGKILL")` and, if that THROWS, falls back to `process.kill(pid,
/// "SIGKILL")` (`shell.ts:214-224`) — a group kill can fail with `ESRCH` when the pid is not a
/// group leader, i.e. when the `setsid` never took effect. `send_sigkill_tree` reaches the same
/// place via its unconditional `child.start_kill()`; here there is no `Child`, so the fallback is
/// spelled out.
///
/// [`kill_tracked_detached_children`]: crate::ops::local::tracking::kill_tracked_detached_children
#[allow(unsafe_code)]
pub fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: `killpg(2)` and `kill(2)` read two integer arguments and touch no memory. A
        // failure (`ESRCH` — group or process already gone) is the expected benign outcome.
        let killed_group = unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) } == 0;
        if !killed_group {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        // Pi's win32 arm is a fire-and-forget `spawn("taskkill", ["/F","/T","/PID", …], {stdio:
        // "ignore", detached: true, windowsHide: true})` (`shell.ts:203-212`) — NOT a blocking
        // wait, which matters because this runs inside a signal handler.
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Force-kill [`LocalProc::exec`]'s (the `bash`-tool/shell path's) child's whole process tree —
/// Pi's real `killProcessTree` (`shell.ts:200-225`: `process.kill(-pid, "SIGKILL")`), the ONLY step
/// of that escalation (no `SIGTERM`, no grace period, ever — see [`super::proc`]'s module doc
/// comment) — R-03-024/027. NOT used by [`LocalProc::exec_argv`], which is single-pid
/// ([`terminate_pid`]/[`kill_pid`]) and DOES have a graceful `SIGTERM`-then-grace leg first; see
/// [`super::proc`]'s module doc comment for why the two methods diverge.
///
/// [`LocalProc::exec`]: crate::ops::ProcOps::exec
/// [`LocalProc::exec_argv`]: crate::ops::ProcOps::exec_argv
#[allow(unsafe_code)]
pub(super) fn send_sigkill_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SAFETY: send SIGKILL to the child's process group (created via `setsid`). A negative
            // pid / killpg targets the group; harmless if the group is already gone (ESRCH).
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Some(pid) = child.id() {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
    }
    let _ = child.start_kill();
}

/// Send SIGTERM to a SINGLE process by pid — NOT a process group (contrast [`send_sigkill_tree`],
/// which targets the whole `setsid` group [`LocalProc::exec`]'s shell-spawned tree needs,
/// R-03-027). This is the graceful half of a two-step escalation for a caller that owns exactly one
/// non-group-leader child directly: TWO real consumers share this exact mechanism — cyrup-ext's
/// long-lived `proc` capability (arch-08 §5.2/pi-mcp-adapter-port.md §3.1, which spawns a plain —
/// not `setsid`'d — child, mirroring the real `StdioClientTransport`'s non-detached spawn 1:1), and
/// [`LocalProc::exec_argv`] (the WASM `exec` capability grant, whose real consumer `exec.ts:34-63`'s
/// `execCommand`/`killProcess` never sets `detached` and signals via a bare, un-negated
/// `proc.kill("SIGTERM")`). A best-effort no-op on non-unix (no portable single-pid graceful-signal
/// primitive there without holding the `Child` itself, which this pid-only API deliberately doesn't
/// require); [`kill_pid`] is the forceful escalation that DOES work everywhere.
///
/// Returns whether a REAL graceful signal was actually sent: `Ok(true)` on unix (the `kill(2)` call
/// succeeded); `Ok(false)` on non-unix, where nothing was sent at all. Callers MUST skip any
/// post-call grace-period wait when this returns `Ok(false)` — waiting for a reaction to a signal
/// that was never sent only pays a needless delay before the (always-effective) forceful escalation,
/// with zero chance of the child ever reacting.
///
/// [`LocalProc::exec`]: crate::ops::ProcOps::exec
/// [`LocalProc::exec_argv`]: crate::ops::ProcOps::exec_argv
#[allow(unsafe_code)]
pub fn terminate_pid(pid: u32) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        // SAFETY: `kill(2)` only reads its two integer args (pid, signal); it touches no memory. A
        // non-zero return is an `errno` (e.g. `ESRCH` if the pid is already gone), surfaced as an
        // `io::Error`, never a panic.
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if rc == 0 {
            Ok(true)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Ok(false)
    }
}

/// Force-kill a SINGLE process by pid (SIGKILL / non-unix `taskkill /F /PID`, no `/T` — this
/// targets exactly the one pid, never a subtree; contrast [`send_sigkill_tree`]). The escalation
/// half of [`terminate_pid`]; works everywhere (unlike the graceful half). Shared by cyrup-ext's
/// `proc` capability and [`LocalProc::exec_argv`] — see [`terminate_pid`]'s doc comment.
///
/// [`LocalProc::exec_argv`]: crate::ops::ProcOps::exec_argv
#[allow(unsafe_code)]
pub fn kill_pid(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: same as `terminate_pid` — `kill(2)` reads two integers, touches no memory.
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()?;
        Ok(())
    }
}

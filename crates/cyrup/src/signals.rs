//! Per-mode signal handling (arch-11 §5; R-11-010/018) — a literal port of pi's three
//! `registerSignalHandlers` sites.
//!
//! pi registers **`SIGTERM`, plus `SIGHUP` off Windows** in every host, and the handler body differs
//! only in what "shut down" means for that host:
//!
//! * `modes/print-mode.ts:48-64` — `killTrackedDetachedChildren()`, then
//!   `disposeRuntime().finally(() => process.exit(signal === "SIGHUP" ? 129 : 143))`. The FIRST
//!   delivery exits the process.
//! * `modes/rpc/rpc-mode.ts:365-379` → `shutdown(signal === "SIGHUP" ? 129 : 143, signal)`
//!   (`:723-740`): dispose the runtime host, detach stdin, and `process.exit(exitCode)`. Again the
//!   first delivery exits — and the re-entrancy guard at `:723-726`
//!   (`if (shuttingDown) process.exit(exitCode)`) is what a SECOND delivery hits while the first
//!   `await runtimeHost.dispose()` is still pending: a hard exit that skips teardown entirely.
//! * `modes/interactive/interactive-mode.ts:3648-3667` → `shutdown({ fromSignal: true })`
//!   (`:3559-3580`): dispose the runtime, drain input, stop the TUI, `process.exit(0)`. The exit is
//!   the *loop's* teardown, not the handler's — which is exactly cyrup's shape, where firing the
//!   [`CancelToken`] breaks `App::run`, `main` disposes the runtime and returns 0.
//!
//! So: interactive is driven through the cancel token (its teardown owns the terminal restore and
//! must not race a `process::exit` from this task), and every non-interactive host disposes and
//! exits from the handler itself with pi's code. SIGINT is cyrup-only — see [`ShutdownSignal`].
//!
//! # CYRUP-DELTA — `killTrackedDetachedChildren` (SEAM-S03, UNPORTED)
//!
//! All three pi handlers open with `killTrackedDetachedChildren()` as their FIRST statement, before
//! any dispose/shutdown: `modes/print-mode.ts:55`, `modes/rpc/rpc-mode.ts:373`,
//! `modes/interactive/interactive-mode.ts:3663` @v0.83.0 (interactive's two emergency paths call it
//! too — `emergencyTerminalExit` at `:3605`, the `uncaughtException` handler at `:3631`). It drains
//! a process-global registry — `const trackedDetachedChildPids = new Set<number>()`,
//! `utils/shell.ts:180`, with `trackDetachedChildPid`/`untrackDetachedChildPid`/
//! `killTrackedDetachedChildren` at `:182-195` — and `killProcessTree`s every pid in it
//! (`shell.ts:200-225`: on unix a `process.kill(-pid, "SIGKILL")`, i.e. the whole process GROUP,
//! falling back to the bare pid). The registry is filled by the bash tool at spawn
//! (`core/tools/bash.ts:108`, `if (child.pid) trackDetachedChildPid(child.pid);`, right beside its
//! `detached: process.platform !== "win32"`) and drained in that spawn's `finally`
//! (`bash.ts:142`) — so at signal time it holds exactly the bash children still running.
//!
//! **cyrup has no such registry, so the sequence below starts at pi's SECOND statement.** What
//! cyrup has instead is an indirect and LATE route to the same kill, and the difference is worth
//! stating exactly rather than as "orphans survive": `LocalProc::exec` `setsid`s its shell into its
//! own process group (`crates/cyrup-tools/src/ops/local.rs:267-279`) and `killpg(SIGKILL)`s that
//! group from a `cancel.cancelled()` select arm (`send_sigkill_tree`, `:327-337`, armed at
//! `:506-509`), where `cancel` is `self.session_cancel.child_token()`. `runtime.dispose()` below
//! ends in `session_cancel.cancel()` (`cyrup-session-svc/src/session.rs:2486`, inside
//! `dispose_with`). So on any path that actually reaches a dispose, an IN-FLIGHT detached bash group
//! is still killed — but *after* the whole teardown (`session_shutdown` fanout, extension dispatch,
//! the invalidate hook) rather than as its first synchronous act.
//!
//! Two things do not survive that translation, and they are what **SEAM-S03** ("No detached-child
//! registry: `setsid`-detached bash children survive teardown", medium, not-ported) still covers:
//! the SECOND-delivery hard exit below runs no dispose and no destructors, so a group live at that
//! moment is orphaned — pi's repeat path hard-exits too, but only *after* its first delivery already
//! SIGKILLed the group synchronously; and cyrup's route is scoped to the CURRENT session's
//! `session_cancel`, where pi's `Set<number>` is process-global and survives session replacement.
//!
//! It is not closed from this file, and that is a boundary fact rather than a preference: pi's two
//! tracking call sites are the spawn and its `finally` INSIDE the bash tool, which in cyrup is
//! `cyrup-tools` — a crate `cyrup` depends on, not the reverse — so a registry living in this bin
//! crate could never be written to. Closing SEAM-S03 means adding the pid set plus track/untrack to
//! `cyrup-tools` around `LocalProc::exec`'s spawn/completion, exposing a
//! `kill_tracked_detached_children()` drain over the existing `killpg` primitive, and calling it as
//! the first statement of [`spawn_abort_on_signal`]'s handler body — and, for pi's repeat-path
//! parity, of the repeat watcher's too.

use std::sync::Arc;

use cyrup_sdk::core::CancelToken;
use cyrup_session_svc::{AgentSessionRuntime, AppMode};

/// Which shutdown signal was delivered, so a REPEAT delivery can exit with the conventional code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShutdownSignal {
    Interrupt,
    Terminate,
    Hangup,
}

impl ShutdownSignal {
    /// pi's own exit codes: `process.exit(signal === "SIGHUP" ? 129 : 143)`
    /// (`print-mode.ts:52-62`, `rpc-mode.ts:374`), i.e. the shell's `128 + signum` convention.
    /// SIGINT is 130 by that same convention.
    const fn exit_code(self) -> i32 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
            Self::Hangup => 129,
        }
    }

    /// Whether pi's handler set covers this signal. pi registers `["SIGTERM", "SIGHUP"]` and NOTHING
    /// else (print-mode.ts:49-51, rpc-mode.ts:366-369, interactive-mode.ts:3652-3655).
    ///
    /// CYRUP-DELTA — SIGINT. pi installs no `SIGINT` listener in any host: its Ctrl-C is a TUI key
    /// event (`handleCtrlC`, interactive-mode.ts:3539-3546) because the terminal is in raw mode, and
    /// a literal `kill -INT` therefore takes Node's default (immediate death). cyrup's watcher is a
    /// tokio `ctrl_c()` future, which necessarily *intercepts* the signal — it cannot decline to
    /// handle it and leave the default in place — so the choice is between an immediate exit and the
    /// graceful abort this has always done. It stays graceful: `AgentSession::abort` + the cancel
    /// token, no process exit, so the mode's own teardown runs. A repeat still escalates below.
    const fn is_pi_shutdown_signal(self) -> bool {
        matches!(self, Self::Terminate | Self::Hangup)
    }
}

/// Await one shutdown signal (SIGINT/Ctrl-C, or on Unix SIGTERM/SIGHUP) and report which arrived.
///
/// A fresh set of streams is created per call so this can be awaited again for the SECOND delivery;
/// tokio's underlying handler is installed process-wide and stays installed either way.
async fn wait_for_signal() -> ShutdownSignal {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        // If a handler cannot be installed, fall back to Ctrl-C alone rather than failing startup.
        match (signal(SignalKind::terminate()), signal(SignalKind::hangup())) {
            (Ok(mut sigterm), Ok(mut sighup)) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => ShutdownSignal::Interrupt,
                    _ = sigterm.recv() => ShutdownSignal::Terminate,
                    _ = sighup.recv() => ShutdownSignal::Hangup,
                }
            }
            (Ok(mut sigterm), Err(_)) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => ShutdownSignal::Interrupt,
                    _ = sigterm.recv() => ShutdownSignal::Terminate,
                }
            }
            (Err(_), _) => {
                let _ = tokio::signal::ctrl_c().await;
                ShutdownSignal::Interrupt
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        ShutdownSignal::Interrupt
    }
}

/// Whether the FIRST delivery of `signal` exits the process from the handler, and with what code —
/// pi's per-host decision, isolated so it is unit-testable without delivering real signals.
///
/// `None` means "do not exit here": either the host is interactive (pi's `shutdown({fromSignal})`
/// ends in `process.exit(0)` *after* the TUI teardown, which in cyrup is the run loop's job — this
/// task only fires the cancel token that starts it, interactive-mode.ts:3559-3580), or the signal is
/// SIGINT, which pi does not handle at all (see [`ShutdownSignal::is_pi_shutdown_signal`]).
fn first_delivery_exit_code(host: AppMode, signal: ShutdownSignal) -> Option<i32> {
    if host == AppMode::Interactive || !signal.is_pi_shutdown_signal() {
        return None;
    }
    Some(signal.exit_code())
}

/// Spawn the shutdown watcher for `host`.
///
/// First delivery — pi's handler body, in pi's order: stop the in-flight run (the CURRENT session's
/// `AgentSession::abort` + `cancel`, cyrup's analog of pi aborting the agent through its
/// disposal), then for a non-interactive host `runtime.dispose()` (the `session_shutdown{quit}`
/// emission extensions rely on — `AgentSessionRuntime::dispose`, pi
/// `runtimeHost.dispose()` at print-mode.ts:57 / rpc-mode.ts:733) and `process::exit` with pi's
/// code. For an interactive host the cancel token is the whole handler: `App::run` breaks, restores
/// the terminal, and `main` disposes the runtime and returns 0, matching
/// interactive-mode.ts:3559-3580 — exiting from here instead would race the terminal restore and
/// leave the user's shell in raw mode.
///
/// Second delivery — pi's re-entrancy guard, `if (shuttingDown) process.exit(exitCode)`
/// (rpc-mode.ts:723-726; print-mode.ts reaches the same place through its `disposed` flag,
/// `:41-46`): a hard exit with the REPEAT signal's code, and it is armed *concurrently* with the
/// first delivery's dispose, exactly as pi's handler can re-enter while the first `await` is
/// pending. Without it a wedged teardown is unkillable by anything but SIGKILL.
///
/// CYRUP-DELTA — pi's interactive host swallows the repeat instead (`if (this.isShuttingDown)
/// return`, interactive-mode.ts:3560-3561). cyrup escalates there too: pi's first delivery has
/// already reached `process.exit` inside its own handler by then, whereas cyrup's interactive exit
/// is owned by the run loop, so leaving the repeat inert would make a stalled loop immune to
/// `kill` — the SEAM-047 symptom this function exists to remove.
///
/// Returns the task handle; dropping it does not stop the watcher (it lives for the run). The binary
/// keeps it alive for the duration of the active mode.
pub fn spawn_abort_on_signal(
    runtime: Arc<AgentSessionRuntime>,
    cancel: CancelToken,
    host: AppMode,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let first = wait_for_signal().await;

        // Arm the repeat watcher BEFORE the (awaiting) teardown below, so a second signal lands
        // while the first is still disposing — pi's handler is re-entrant for the same reason.
        let repeat = tokio::spawn(async move {
            let again = wait_for_signal().await;
            std::process::exit(again.exit_code());
        });

        // CYRUP-DELTA — pi's handler body opens with `killTrackedDetachedChildren()`
        // (print-mode.ts:55, rpc-mode.ts:373, interactive-mode.ts:3663 @v0.83.0) and cyrup has no
        // detached-child registry to drain, so this sequence starts at pi's SECOND statement. The
        // `runtime.dispose()` below reaches in-flight detached groups indirectly and late (its
        // `session_cancel.cancel()` fires the child token `LocalProc::exec` killpgs on); the
        // repeat watcher armed just above does not. UNPORTED, tracked as SEAM-S03 — the module doc
        // gives the exact residual and why the fix cannot land in this crate (the track/untrack
        // call sites are inside `cyrup-tools`' bash spawn, which `cyrup` depends on).
        //
        // SEAM-059 (which this function's rewrite was told to land with): dereference the CURRENT
        // session, never the startup `Arc`. pi's handlers reach the agent through the runtime host
        // (`runtimeHost.dispose()`, print-mode.ts:57 / rpc-mode.ts:733), so a `/new`, `/fork`,
        // `switchSession` or `reload` earlier in the run cannot leave the signal aborting a disposed
        // session while the live turn runs on to completion.
        runtime.session().await.abort();
        cancel.cancel();

        if let Some(code) = first_delivery_exit_code(host, first) {
            runtime.dispose().await;
            std::process::exit(code);
        }

        // Interactive / SIGINT: the run loop owns the exit. Keep the repeat watcher alive.
        let _ = repeat.await;
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// The shell's `128 + signum` convention, and pi's literal
    /// `process.exit(signal === "SIGHUP" ? 129 : 143)` (`print-mode.ts:52-62`).
    #[test]
    fn repeat_signal_exit_codes_match_pi() {
        assert_eq!(ShutdownSignal::Interrupt.exit_code(), 130);
        assert_eq!(ShutdownSignal::Terminate.exit_code(), 143);
        assert_eq!(ShutdownSignal::Hangup.exit_code(), 129);
    }

    /// SEAM-047: the FIRST SIGTERM/SIGHUP must terminate a non-interactive host with pi's code
    /// (print-mode.ts:52-62, rpc-mode.ts:374) — it used to be absorbed, leaving `--mode rpc` alive
    /// until SIGKILL. Interactive stays with the run loop (interactive-mode.ts:3559-3580), and
    /// SIGINT keeps cyrup's graceful abort (pi registers no SIGINT handler).
    #[test]
    fn first_sigterm_and_sighup_exit_non_interactive_hosts() {
        for host in [AppMode::Rpc, AppMode::Print, AppMode::Json] {
            assert_eq!(
                first_delivery_exit_code(host, ShutdownSignal::Terminate),
                Some(143),
                "{host:?} must exit 143 on the FIRST SIGTERM"
            );
            assert_eq!(
                first_delivery_exit_code(host, ShutdownSignal::Hangup),
                Some(129),
                "{host:?} must exit 129 on the FIRST SIGHUP"
            );
            assert_eq!(
                first_delivery_exit_code(host, ShutdownSignal::Interrupt),
                None,
                "{host:?} keeps the graceful SIGINT abort"
            );
        }
        for signal in [
            ShutdownSignal::Terminate,
            ShutdownSignal::Hangup,
            ShutdownSignal::Interrupt,
        ] {
            assert_eq!(
                first_delivery_exit_code(AppMode::Interactive, signal),
                None,
                "interactive teardown owns the exit ({signal:?})"
            );
        }
    }
}

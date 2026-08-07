//! Per-mode signal handling (arch-11 §5; R-11-010/018).
//!
//! A single background task waits for an interrupt (Ctrl-C/SIGINT) or SIGTERM and then triggers a
//! clean shutdown: it aborts the in-flight agent run ([`AgentSession::abort`], which propagates the
//! per-run `RunCancel`) and fires the supplied [`CancelToken`] so the interactive event loop breaks
//! out and restores the terminal. The agent loop still emits its terminal `agent_end` before exit,
//! so PRINT/JSON flush their output and the process exits cleanly.

use std::sync::Arc;

use cyrup_sdk::core::CancelToken;
use cyrup_session_svc::AgentSession;

/// Which shutdown signal was delivered, so a REPEAT delivery can exit with the conventional code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShutdownSignal {
    Interrupt,
    Terminate,
    Hangup,
}

impl ShutdownSignal {
    /// pi's own exit codes: `process.exit(signal === "SIGHUP" ? 129 : 143)`
    /// (`print-mode.ts:52-62`), i.e. the shell's `128 + signum` convention. SIGINT is 130 by that
    /// same convention.
    const fn exit_code(self) -> i32 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
            Self::Hangup => 129,
        }
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

/// Spawn the shutdown watcher. On the first signal it aborts `session` and fires `cancel`; on a
/// SECOND signal it force-exits.
///
/// The second half is the point. This task used to await exactly ONE signal and then end — but
/// tokio's signal handler is installed process-wide and stays installed, so every later SIGINT was
/// received and discarded with nothing listening. A run wedged in teardown (a child ignoring
/// SIGINT, a stalled flush) could not be interrupted again: Ctrl-C did nothing, `kill` did nothing,
/// and only `SIGKILL` from another terminal ended it.
///
/// pi force-exits on the repeat in every host — `rpc-mode.ts:722-724` opens `shutdown()` with
/// `if (shuttingDown) process.exit(exitCode)`, hard-exiting without waiting for
/// `runtimeHost.dispose()` or `flushRawStdout()`, and `print-mode.ts:52-62` re-enters to the same
/// `process.exit`. Deliberately NOT a graceful path: the user asking twice is the signal that
/// graceful is not working.
///
/// Returns the task handle; dropping it does not stop the watcher (it lives for the run). The binary
/// keeps it alive for the duration of the active mode.
pub fn spawn_abort_on_signal(
    session: Arc<AgentSession>,
    cancel: CancelToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        wait_for_signal().await;
        session.abort();
        cancel.cancel();

        // Second delivery: the graceful path did not finish, so stop waiting for it.
        let repeat = wait_for_signal().await;
        std::process::exit(repeat.exit_code());
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
}

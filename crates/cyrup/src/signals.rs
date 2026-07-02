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

/// Await the first shutdown signal (SIGINT/Ctrl-C or, on Unix, SIGTERM).
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        // If a handler cannot be installed, fall back to Ctrl-C alone rather than failing startup.
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sigterm.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Spawn the shutdown watcher. On the first signal it aborts `session` and fires `cancel`.
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
    })
}

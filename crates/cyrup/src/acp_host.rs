//! The binary half of `cyrup_acp::AcpHost`: building a session runtime for the ACP front-end.
//!
//! `ACP-003` / `ACP-023` / `ACP-Q30`. This is the inversion the library crate cannot do for itself:
//! [`crate::session_launch::build_factory`] and [`crate::signals::spawn_abort_on_signal`] live in
//! this crate, which **depends on** `cyrup-acp`, so `cyrup-acp` defines the trait and this module
//! implements it.
//!
//! Nothing here is a port — pi-acp's counterpart is `PiRpcProcess::spawn`, which is cut outright
//! (gap-analysis 15 §3: there is no child process). What survives from that cut is the *failure
//! class*: `session/new` loses pi-acp's three ENOENT/EACCES/other spawn-diagnostic strings and
//! gains `SessionFactory::build`'s typed errors instead, which is what
//! `cyrup_acp::AcpFailure::classify` is written against.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cyrup_acp::{AcpError, AcpHost, AgentSessionRuntime, BoxFuture, RuntimeRequest, SessionsRoot};
use cyrup_config::AppMode;
use cyrup_sdk::core::CancelToken;
use cyrup_session_svc::SessionFactory;

/// The `cyrup` binary's [`AcpHost`].
///
/// Holds only what building a runtime needs: the (already-constructed, cheap) factory, the sessions
/// root the CLI > env > settings ladder resolved, and the cancel token the signal watcher fires.
pub struct BinaryAcpHost {
    factory: Arc<SessionFactory>,
    sessions_root: SessionsRoot,
    cancel: CancelToken,
    /// What the ONE shutdown watcher tears down, re-pointed by each [`AcpHost::runtime_ready`].
    /// See [`crate::signals::RuntimeSlot`] for why this is a slot and not a captured `Arc`.
    watcher_target: crate::signals::RuntimeSlot,
    /// Whether the watcher has been armed. See [`claim_watcher`].
    watcher_armed: AtomicBool,
}

/// Claim the right to arm the shutdown watcher: `true` for exactly ONE caller, ever.
///
/// `AcpHost::runtime_ready` is called on every `session/new` and, through `build_and_install`, on
/// every `session/load` — both routine in Zed. Arming a watcher per call left N tasks each running
/// the whole of [`crate::signals::spawn_abort_on_signal_slot`]'s body on a single SIGTERM: N drains,
/// N repeat watchers, and N racing `std::process::exit(143)`. The stale watchers' runtimes are
/// already disposed (`cyrup_acp::SessionManager::install` disposes the one it replaces), so their
/// `dispose()` returns immediately and one of them can exit the process out from under the live
/// session's `session_shutdown` fanout and fsync drain.
///
/// A swap rather than a `OnceLock` because the answer is a bit, and rather than a `bool` behind the
/// existing lock because `runtime_ready` is `&self` on a `Sync` trait object and two `session/new`
/// requests can be dispatched concurrently — `swap` makes "who arms it" a single atomic decision.
fn claim_watcher(armed: &AtomicBool) -> bool {
    !armed.swap(true, Ordering::SeqCst)
}

impl BinaryAcpHost {
    /// Build the host. `factory` is constructed in `main` **without** creating a session — that is
    /// the whole point of `ACP-003`'s lazy build, and it is why `main`'s ACP arm does not call
    /// [`crate::session_launch::launch`].
    #[must_use]
    pub fn new(
        factory: Arc<SessionFactory>,
        sessions_root: SessionsRoot,
        cancel: CancelToken,
    ) -> Self {
        Self {
            factory,
            sessions_root,
            cancel,
            watcher_target: crate::signals::RuntimeSlot::new(),
            watcher_armed: AtomicBool::new(false),
        }
    }
}

impl AcpHost for BinaryAcpHost {
    fn build_runtime<'a>(
        &'a self,
        req: &'a RuntimeRequest,
    ) -> BoxFuture<'a, Result<Arc<AgentSessionRuntime>, AcpError>> {
        Box::pin(async move {
            // SEAM-033 / `ACP-003` — `create_unannounced` is pi's `createAgentSessionRuntime`, which
            // never emits `session_start`; the HOST announces. For ACP the announcement must follow
            // `initialize`, because `has_ui` and the client's advertised capabilities are what a
            // `session_start` handler should see — so the caller in `cyrup_acp::sessions` binds
            // extensions, not this function.
            //
            // `ACP-056` — the session is built AT the cwd the client named, not at the one the
            // editor happened to launch cyrup in. `create_unannounced` passes `None` and would root
            // every session in the process's own directory: the services cwd, the session directory
            // and therefore the transcript `session/list` looks for would all belong to the wrong
            // project, and a `session/list` scoped to the client's cwd would come back empty for a
            // session it had just created.
            //
            // # CYRUP-DELTA — the cwd reaches the session but not the native extensions
            //
            // **What differs.** `crate::session_launch::attach_native_extensions` bakes
            // `session_cwd` into each built-in at *factory construction* time, so a second
            // `session/new` with a different cwd gets a runtime whose services and session file are
            // correctly rebound but whose native extensions are still pointed at the first project.
            //
            // **What it costs.** A client that opens a **second** project on the same ACP
            // connection gets the first project's permission policy, MCP servers and flux state.
            // The first project is unaffected, which is the common case. This is a **pre-existing
            // defect independent of ACP** — `AgentSessionRuntime::switch_session` with a
            // `cwd_override` has the same hole today (gap-analysis 15 §1) — and closing it means a
            // new `SessionFactory` per cwd, which is a change in this crate, not in `cyrup-acp`.
            // Recorded here because this is the function that would carry the fix.
            AgentSessionRuntime::create_unannounced_at(
                Arc::clone(&self.factory),
                req.target.clone(),
                Some(req.cwd.as_path().to_path_buf()),
            )
            .await
            .map_err(AcpError::Session)
        })
    }

    fn runtime_ready(&self, runtime: &Arc<AgentSessionRuntime>) {
        // Point the watcher at this runtime FIRST, so the ordering holds for both callers: the
        // arming call below finds a non-empty slot, and every later call has re-pointed the
        // already-running watcher before it returns to `SessionManager::install`.
        self.watcher_target.set(Arc::clone(runtime));

        // `ACP-023` / `ACP-006` — arm the shutdown watcher on the first runtime, exactly as the
        // `Rpc` arm does in `main`, but at the point the runtime first exists rather than before it
        // (see `AcpHost::runtime_ready`'s doc for why the startup window that leaves is empty).
        //
        // ONE watcher for the connection, not one per runtime: `runtime_ready` fires on every
        // `session/new` and every `session/load`, and a second watcher would race the first for
        // `std::process::exit` on the next SIGTERM. See [`claim_watcher`].
        //
        // The handle is deliberately dropped: `spawn_abort_on_signal_slot`'s own doc states that
        // dropping it does not stop the watcher, which lives for the run.
        if claim_watcher(&self.watcher_armed) {
            let _watcher = crate::signals::spawn_abort_on_signal_slot(
                self.watcher_target.clone(),
                self.cancel.clone(),
                AppMode::Acp,
            );
        }
    }

    fn sessions_root(&self) -> SessionsRoot {
        self.sessions_root.clone()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::claim_watcher;

    /// One watcher per CONNECTION, not per runtime. `runtime_ready` fires on every `session/new`
    /// and every `session/load`; before this, each of those spawned a watcher of its own and one
    /// SIGTERM ran the whole teardown body N times, with N racing `std::process::exit(143)` — the
    /// stale ones returning instantly from an already-disposed `dispose()` and able to kill the
    /// process mid-`session_shutdown` fanout for the live session.
    #[test]
    fn only_the_first_runtime_arms_a_watcher() {
        let armed = AtomicBool::new(false);
        let claims = (0..8).filter(|_| claim_watcher(&armed)).count();
        assert_eq!(claims, 1, "exactly one runtime may arm the watcher");
    }

    /// Two `session/new` requests can be in flight at once — `AcpHost` is a `Sync` trait object and
    /// `cyrup_acp::connection` dispatches each off the loop — so the claim has to be atomic, not a
    /// read-then-write on a plain flag.
    #[test]
    fn concurrent_runtime_ready_calls_still_arm_exactly_one() {
        let armed = std::sync::Arc::new(AtomicBool::new(false));
        let winners: Vec<bool> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    let armed = std::sync::Arc::clone(&armed);
                    scope.spawn(move || claim_watcher(&armed))
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        assert_eq!(winners.len(), 16, "every thread must report");
        assert_eq!(
            winners.into_iter().filter(|won| *won).count(),
            1,
            "exactly one concurrent caller may arm the watcher"
        );
    }
}

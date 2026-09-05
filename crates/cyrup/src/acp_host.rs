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
        // `ACP-023` / `ACP-006` — arm the shutdown watcher on the first runtime, exactly as the
        // `Rpc` arm does in `main`, but at the point the runtime first exists rather than before it
        // (see `AcpHost::runtime_ready`'s doc for why the startup window that leaves is empty).
        //
        // The handle is deliberately dropped: `spawn_abort_on_signal`'s own doc states that
        // dropping it does not stop the watcher, which lives for the run.
        let _watcher = crate::signals::spawn_abort_on_signal(
            Arc::clone(runtime),
            self.cancel.clone(),
            AppMode::Acp,
        );
    }

    fn sessions_root(&self) -> SessionsRoot {
        self.sessions_root.clone()
    }
}

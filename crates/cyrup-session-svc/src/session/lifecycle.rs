//! Session lifecycle — dispose, extension binding and the one-shot `session_start`.
//!
//! Pi `agent-session.ts:2250,850-852` + `agent-session-runtime.ts:176-177`. The ordered teardown
//! (`session_shutdown` fan-out → `before_session_invalidate` → invalidate → cancel), the bind that
//! announces the session to extensions, and the manager hand-off used when a session is replaced.

use std::sync::atomic::Ordering;

use cyrup_ext::HostEvent;
use cyrup_session::flush_session_writes;
use cyrup_session::manager::SessionManager;

use crate::error::SessionServiceError;
use crate::event::AgentSessionEvent;

use super::AgentSession;
use super::types::BindOptions;

impl AgentSession {
    /// Dispose the session (Pi `AgentSession.dispose` via runtime `dispose`,
    /// agent-session-runtime.ts:390): abort any in-flight run **and wait for it to settle**, emit
    /// `session_shutdown`, and cancel the long-lived session token so the extension subscriber
    /// unwinds.
    ///
    /// SEAM-024 — the settle is not optional. Pi's `teardownCurrent` opens with
    /// `await this.session.abort()` and the comment "Settle any active response first so the
    /// aborted turn (including tool results) is persisted to the outgoing session before it is
    /// replaced" (agent-session-runtime.ts:167-169), and only then emits `session_shutdown` and
    /// disposes. cyrup collapses pi's `teardownCurrent` + `runtime.dispose` + `session.dispose`
    /// into this one method, so the await belongs here: it is on EVERY teardown path (`run.rs`,
    /// `main.rs`) and every replacement (`runtime.rs`). Previously the fire-and-forget `abort()`
    /// let `session_shutdown` be announced — and `session_cancel` fired — while the aborted turn
    /// was still writing its tool results.
    pub async fn dispose(&self, reason: &str) {
        self.dispose_with(reason, None, None).await;
    }

    /// [`Self::dispose`] with the host's `before_session_invalidate` hook (Pi
    /// `beforeSessionInvalidate`, agent-session-runtime.ts:76/:129-131).
    ///
    /// Pi's `teardownCurrent` is `await this.session.abort(); await emitSessionShutdownEvent(…);
    /// this.beforeSessionInvalidate?.(); this.session.dispose();` (agent-session-runtime.ts:167-177)
    /// and its `dispose()` repeats the last three lines (:398-404). So the hook's contract is
    /// positional: it runs **after** every `session_shutdown` handler has finished and **before**
    /// anything invalidates the session. Because cyrup collapses pi's `teardownCurrent` +
    /// `AgentSession.dispose` into this one method, the call site is here — after
    /// `dispatch_notify(SessionShutdown)` and before `session_cancel.cancel()`, which is cyrup's
    /// analog of pi's `_extensionRunner.invalidate("This extension ctx is stale after session
    /// replacement or reload…")` (agent-session.ts:848, inside `dispose()` at :837-853).
    ///
    /// The hook is deliberately **synchronous** (`Fn()`, pi's `() => void`): it exists so a host can
    /// detach extension-provided UI without yielding to the executor, i.e. without a window in which
    /// a component still rendered from the outgoing session's now-stale extension context can be
    /// driven. Awaiting inside it would reopen exactly that window, so there is nothing to await.
    ///
    /// [`Self::dispose`] passes `None`; the only producer of a `Some` is
    /// [`crate::runtime::AgentSessionRuntime`], which reads whatever the host registered via
    /// `set_before_session_invalidate`.
    /// `target_session_file` is pi's `teardownCurrent(reason, targetSessionFile?)` second argument
    /// (`agent-session-runtime.ts:165`), spread onto the event at `:171-174`: *"Destination session
    /// file when shutting down due to session replacement"*
    /// (`core/extensions/types.ts:616-621`). Every replacement path supplies it —
    /// `teardownCurrent("new", sessionManager.getSessionFile())` and its `resume`/`fork` siblings
    /// (`:239`, `:210`, `:300`/`:322`/`:341`) — and a plain quit passes `undefined`. Without it an
    /// extension observing a replacement could not tell WHICH session it was going to, so
    /// transcript-linking, audit trails and intercom identity handoff across a switch or fork were
    /// impossible. SEAM-025.
    pub async fn dispose_with(
        &self,
        reason: &str,
        before_invalidate: Option<crate::runtime::BeforeSessionInvalidate>,
        target_session_file: Option<String>,
    ) {
        self.abort_and_settle().await;
        self.fanout_emit(AgentSessionEvent::SessionShutdown { reason: reason.to_string() }).await;
        // Notify extensions, then release the long-lived token.
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(
                &HostEvent::SessionShutdown {
                    reason: reason.to_string(),
                    target_session_file,
                },
                &cancel,
            )
            .await;
        // Pi `this.beforeSessionInvalidate?.()` (agent-session-runtime.ts:176 and :403): the last
        // point at which this session — and the extension context bound to it — is still live.
        if let Some(hook) = before_invalidate {
            hook();
        }
        // pi `teardownCurrent` runs `this.beforeSessionInvalidate?.()` and then
        // `this.session.dispose()` (`core/agent-session-runtime.ts:176-177` @v0.83.0), and
        // `dispose()` ends in `this._extensionRunner.invalidate(<stale text>)`
        // (`core/agent-session.ts:851-853`) — the latch that makes a captured extension ctx refuse
        // to act, plus the event-bus unsubscribe sweep (`core/extensions/loader.ts:206-215`).
        //
        // cyrup reached exactly this position and then never invalidated: `invalidate_live` had
        // ONE caller in the workspace, `ExtensionHost::reload`. So `/new`, `/resume`, `/fork` and
        // `/switch` all left every live guest of the OUTGOING session un-stale and still
        // subscribed, and pi's `assertActive` refusal never fired for a call still in flight on
        // one of them: it could go on emitting and subscribing, and go on being reached by its own
        // host's dispatch, after the session it belongs to was torn down.
        //
        // What that does NOT mean — a claim that stood here and was wrong — is that those handlers
        // received events destined for the REPLACEMENT session's set. `SharedBus` is per host
        // (`ExtensionHost::new` builds its own, `cyrup-ext/src/facade.rs`) and the replacement gets
        // a fresh host from `SessionBuilder::build`, so the two sets never shared a bus and there
        // was no cross-talk between them. The damage is confined to the outgoing set acting on
        // after its session is gone, which is what `assertActive` exists to stop.
        //
        // `None` takes the ported default stale text (`GuestState::invalidate`); the call is a
        // no-op on the `--no-default-features` arm, which is why it needs no `#[cfg]` here. See
        // `impl Drop for AgentSession` for the backstop covering the paths that never get here.
        self.services.ext_host.invalidate_live(None);
        self.session_cancel.cancel();
        // PERF-004 §3.5: session appends put their bytes in the page cache synchronously and hand
        // only the `fdatasync` to a background worker, so nothing is lost if this never runs — but
        // this is the seam every host teardown funnels through, so it is where the outstanding
        // flushes get drained. On the blocking pool deliberately: the point of PERF-004 is to keep
        // file I/O off the runtime workers.
        let _ = tokio::task::spawn_blocking(flush_session_writes).await;
    }

    /// Move this session's manager out from behind its lock, leaving a fresh empty in-memory
    /// manager in its place, so `SessionFactory::build_from_manager` (which takes the manager by
    /// value) can adopt it — cyrup's stand-in for Pi passing `this.session.sessionManager` straight
    /// into `createRuntime` (agent-session-runtime.ts:341).
    ///
    /// **The caller must have settled this session's run first.** Anything the session writes after
    /// this call lands in the placeholder and is lost when the session is dropped. The sole caller
    /// (the runtime's non-persisted fork arm) awaits `abort_and_settle()` immediately before, then
    /// disposes and replaces the session.
    pub(crate) async fn take_manager(&self) -> Result<SessionManager, SessionServiceError> {
        let mut guard = self.manager.lock().await;
        let placeholder = SessionManager::in_memory(
            guard.cwd(),
            cyrup_session::manager::NewSessionOpts::default(),
        )?;
        Ok(std::mem::replace(&mut *guard, placeholder))
    }

    /// Invalidate every live subscription on replacement (R-11-021): emit the terminal
    /// `SessionReplaced{generation}` and drop all senders so consumers re-subscribe.
    pub async fn notify_replaced(&self, generation: u64) {
        self.fanout.invalidate(generation).await;
    }

    /// Bind this session to its extension host and announce it as a FRESH START (Pi
    /// `bindExtensions`, agent-session.ts:2229-2251, whose tail is
    /// `await this._extensionRunner.emit(this._sessionStartEvent)`; that event defaults to
    /// `{type:"session_start", reason:"startup"}` at agent-session.ts:389).
    ///
    /// This is the seam every host calls exactly once for the INITIAL session, before any prompt —
    /// pi does it from print-mode.ts:73, rpc-mode.ts:318 and interactive-mode.ts:1698. In cyrup the
    /// bindings themselves are installed at build time, so the remaining work is the announcement.
    ///
    /// Session REPLACEMENTS (`new`/`resume`/`fork`/`reload`) are announced by the runtime's install
    /// tail with their own reason, which is why this is idempotent per session: whichever tier
    /// announces first wins and a later bind is a no-op (pi likewise emits `_sessionStartEvent`
    /// exactly once per `AgentSession`).
    pub async fn bind_extensions(&self) {
        self.bind_extensions_with(BindOptions::default()).await;
    }

    /// [`Self::bind_extensions`] with pi's `bindExtensions({ … })` options — SEAM-006.
    ///
    /// pi's hosts pass THREE keys (`print-mode.ts:73-101` @v0.83.0, `:74-119` @v0.84.1):
    /// * `mode: mode === "json" ? "json" : "print"` (`:74`) — already live in cyrup, installed at
    ///   build time from `cfg.app_mode` (`builder.rs`'s `ext_mode` → `HostConfig { mode, has_ui }`),
    ///   because cyrup's builder knows the resolved mode where pi's `bindExtensions` is the first
    ///   place that does;
    /// * `commandContextActions` (`:75-97`) — already live as `RuntimeActions`, installed onto every
    ///   session by `AgentSessionRuntime::install_inner`;
    /// * `onError` (`:98-100`) — **this**, and it was the piece with no counterpart: the print and
    ///   json hosts registered no error sink at all, so an extension fault under `cyrup -p` or
    ///   `cyrup --mode json` was contained and never surfaced anywhere. See [`BindOptions::on_error`].
    pub async fn bind_extensions_with(&self, opts: BindOptions) {
        if let Some(listener) = opts.on_error {
            self.services.ext_host.add_error_listener(listener);
        }
        self.emit_session_start("startup", None).await;
    }

    /// Announce this (freshly-installed) session to its subscribers + extensions (Pi `session_start`,
    /// agent-session-runtime.ts:215). `reason` ∈ `startup`/`new`/`resume`/`fork`/`reload`.
    ///
    /// At most ONE announcement is emitted per session (pi emits `_sessionStartEvent` once, from
    /// `bindExtensions`); subsequent calls return without emitting.
    pub async fn emit_session_start(&self, reason: &str, previous_session_file: Option<String>) {
        if self.start_announced.swap(true, Ordering::SeqCst) {
            return;
        }
        self.fanout_emit(AgentSessionEvent::SessionStart {
            reason: reason.to_string(),
            previous_session_file: previous_session_file.clone(),
        })
        .await;
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(
                &HostEvent::SessionStart {
                    reason: reason.to_string(),
                    // SEAM-025 — pi's `SessionStartEvent.previousSessionFile?`, "Present for
                    // \"new\", \"resume\", and \"fork\"" (`core/extensions/types.ts:562-569`
                    // @v0.83.0), populated at `agent-session-runtime.ts:305`/`:328`/`:347`. The
                    // FACADE event already carried it; the EXTENSION event dropped it, so the one
                    // consumer that cannot see the facade — an extension — was the one that lost it.
                    previous_session_file: previous_session_file.clone(),
                },
                &cancel,
            )
            .await;
        // EXT-004: `session_start` is Pi's canonical place to register a tool dynamically
        // (`examples/extensions/dynamic-tools.ts`). Pi's `registerTool` refreshes the registry
        // inline; cyrup's crosses a SYNC wasm import, so the async push happens here — before any
        // prompt, so the very first turn already sees the tool.
        self.refresh_extension_tools().await;
    }
}

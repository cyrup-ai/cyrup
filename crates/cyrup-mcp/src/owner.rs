//! `McpRuntimeOwner` and the fenced services handle — `runtime-owner.ts` (MCP-004, MCP-005,
//! MCP-006).
//!
//! One generation of the MCP runtime owns exactly one [`McpRuntimeOwner`]. Everything that
//! generation starts — connect passes, reconnect timers, the health-check loop, OAuth callbacks,
//! in-flight `tools/call`s — either selects on its token or checks [`McpRuntimeOwner::is_active`]
//! before it writes anywhere. One call to [`McpRuntimeOwner::stop`] reliably ends all of it;
//! calling it twice is free; work started after the stop is refused rather than run.
//!
//! # The one forced mechanism difference, and why it costs a field
//!
//! Upstream is one `AbortController`. `stop(reason)` calls `controller.abort(new Error(reason))`,
//! which puts the reason **inside** the signal, and `signal.throwIfAborted()` rethrows that exact
//! object. A [`cyrup_core::CancelToken`] (`tokio_util::sync::CancellationToken`) carries no
//! payload. So the reason is stored in an [`arc_swap::ArcSwapOption`] written **immediately before**
//! `token.cancel()` — publish-then-cancel, so any task that observes the cancellation also observes
//! the reason. Without it, two things silently stop working: `isAbortError`'s literal
//! `error.message === "MCP extension runtime stopped"` arm (ported as
//! [`crate::abort::is_abort_error`]) and `abort.ts`'s reason-rethrow.
//!
//! # LIFO, and why the aggregate matters
//!
//! `stop()` does `cleanups.splice(0).reverse()` and settles all of them: cleanups run strictly
//! last-registered-first, **one failing cleanup never prevents the others from running**, and every
//! failure is reported together rather than the first masking the rest. The real LIFO order after
//! Cut 2 is `lifecycle.graceful_shutdown()` → `shutdown_oauth` →
//! `cleanup_materialized_binary_resources`. Reversing it — or bailing on the first error — leaves
//! an MCP child process alive after the session is gone.
//!
//! # Memoisation, and why `stop()` yields `Arc<McpError>`
//!
//! Upstream memoises `stopPromise` so the second `stop()` returns the first one's promise. The Rust
//! equivalent is a [`futures::future::Shared`], whose output must be `Clone` — and [`McpError`] is
//! not `Clone`, because it carries a `std::io::Error`. Hence [`StopResult`]. That is a consequence
//! of memoisation, not a design preference.

use std::future::Future;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use cyrup_core::CancelToken;
use futures::future::{BoxFuture, FutureExt, Shared};

use crate::errors::{CleanupErrors, McpError, McpResult};

/// `stop()`'s default `reason` parameter. **This exact string is load-bearing**: upstream's
/// `isAbortError` tests `error.message === "MCP extension runtime stopped"` literally, and a
/// custom reason deliberately does not match that arm (only the `signal.aborted` arm).
pub const DEFAULT_STOP_REASON: &str = "MCP extension runtime stopped";

/// One registered cleanup. `FnOnce` because upstream splices the array — a cleanup runs at most
/// once — and boxed-future-returning because the real cleanups (`gracefulShutdown`,
/// `shutdownOAuth`) are async.
pub type Cleanup = Box<dyn FnOnce() -> BoxFuture<'static, McpResult<()>> + Send + 'static>;

/// The memoised outcome of [`McpRuntimeOwner::stop`]. `Arc` because a memoised future's output must
/// be `Clone`; see the module docs.
pub type StopResult = Result<(), Arc<McpError>>;

/// The shared, memoised stop future.
type StopFuture = Shared<BoxFuture<'static, StopResult>>;

/// The ownership token for one generation of the MCP runtime — `runtime-owner.ts`'s
/// `{signal, isActive(), addCleanup(cb), stop(reason?), throwIfInactive()}`.
///
/// Held as an `Arc` by the extension, by [`crate::state::McpState`], and by every task the
/// generation spawns.
pub struct McpRuntimeOwner {
    /// `controller.signal`. Handed out by [`Self::token`] so a task can
    /// `tokio::select!` on it rather than polling [`Self::is_active`].
    token: CancelToken,
    /// The payload a `CancellationToken` cannot carry: `abort(new Error(reason))`'s reason.
    /// Written **before** `token.cancel()`, so observing the cancellation implies observing this.
    reason: ArcSwapOption<String>,
    /// The registration stack. Drained (not iterated) by [`Self::begin_stop`], which is what makes
    /// a second stop a no-op even if the memo slot were somehow bypassed.
    cleanups: Mutex<Vec<Cleanup>>,
    /// `stopPromise` — present from the moment [`Self::begin_stop`] first runs.
    stop: Mutex<Option<StopFuture>>,
}

impl Default for McpRuntimeOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRuntimeOwner {
    /// A fresh, active owner with its own root token.
    #[must_use]
    pub fn new() -> Self {
        Self::with_token(CancelToken::new())
    }

    /// An owner over a caller-supplied token — used where the generation's token must be a *child*
    /// of a longer-lived one (the session run token), so cancelling the parent cancels this
    /// generation without the owner having to observe it separately.
    #[must_use]
    pub fn with_token(token: CancelToken) -> Self {
        Self {
            token,
            reason: ArcSwapOption::empty(),
            cleanups: Mutex::new(Vec::new()),
            stop: Mutex::new(None),
        }
    }

    /// `owner.signal` — a clone of the generation's cancellation token, for `tokio::select!`.
    #[must_use]
    pub fn token(&self) -> CancelToken {
        self.token.clone()
    }

    /// `isActive()` — `!controller.signal.aborted`.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.token.is_cancelled()
    }

    /// The reason passed to the [`Self::stop`] that ended this generation, if it has ended.
    /// This is `signal.reason`'s message, and it is what [`Self::throw_if_inactive`] rethrows.
    #[must_use]
    pub fn stop_reason(&self) -> Option<Arc<String>> {
        self.reason.load_full()
    }

    /// `throwIfInactive()` — `Ok(())` while active, else [`McpError::Aborted`] carrying the stop
    /// reason verbatim. Called on **both** sides of an await wherever upstream guards one (see
    /// `state.openBrowser`, 13a §9 step 9).
    pub fn throw_if_inactive(&self) -> McpResult<()> {
        if self.is_active() {
            return Ok(());
        }
        Err(McpError::Aborted(
            self.stop_reason().map_or_else(|| DEFAULT_STOP_REASON.to_string(), |r| r.to_string()),
        ))
    }

    /// `addCleanup(cb)`.
    ///
    /// On an **already-stopped** owner the cleanup is not dropped and not deferred to a stop that
    /// will never come: upstream runs it on a microtask (`Promise.resolve().then(cleanup)`) and
    /// reports a failure through the same formatter with `late = true`. Here that is a
    /// `tokio::spawn`, guarded by `Handle::try_current()` so a caller outside a reactor gets a
    /// diagnostic instead of a panic (this crate denies `clippy::panic`, and `tokio::spawn` panics
    /// off-runtime).
    pub fn add_cleanup(&self, cleanup: Cleanup) {
        if self.is_active()
            && let Ok(mut stack) = self.cleanups.lock()
        {
            stack.push(cleanup);
            return;
        }
        // Late path — the owner already stopped (or the stack lock is poisoned, in which case the
        // stack can no longer be trusted to run anything and running now is strictly better).
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if let Err(err) = cleanup().await {
                        report_cleanup_failure(&err, true);
                    }
                });
            }
            Err(_) => {
                tracing::warn!(
                    "MCP: late runtime cleanup dropped — no tokio runtime to schedule it on"
                );
            }
        }
    }

    /// Cancel **now** and hand back the cleanup future, without awaiting it.
    ///
    /// This split is the whole point of MCP-008's `session_start` protocol: the previous
    /// generation's cancel must be observable *before* its cleanup completes, so a slow shutdown
    /// cannot delay the abort and the new generation can begin while the old one drains. Do not
    /// collapse this into a single `stop().await`.
    ///
    /// Idempotent: the second call returns a clone of the first call's memoised future and runs
    /// nothing again.
    pub fn begin_stop(&self, reason: Option<&str>) -> StopFuture {
        let Ok(mut slot) = self.stop.lock() else {
            // A poisoned memo slot means a previous `begin_stop` panicked mid-registration. The
            // token is what actually stops the work, so cancel and hand back a settled future
            // rather than deadlocking a shutdown on a poisoned lock.
            self.publish_reason_and_cancel(reason);
            return async { Ok(()) }.boxed().shared();
        };
        if let Some(existing) = slot.as_ref() {
            return existing.clone();
        }

        self.publish_reason_and_cancel(reason);

        // `cleanups.splice(0).reverse()` — drain, then LIFO. Invoking the closures here (rather
        // than inside the future) reproduces upstream's `Promise.resolve().then(cleanup)`, which
        // *calls* each cleanup in LIFO order and only then settles them together.
        let drained: Vec<Cleanup> = match self.cleanups.lock() {
            Ok(mut stack) => stack.drain(..).rev().collect(),
            Err(_) => Vec::new(),
        };
        let running: Vec<BoxFuture<'static, McpResult<()>>> =
            drained.into_iter().map(|cleanup| cleanup()).collect();

        let future = async move {
            // `Promise.allSettled` — every cleanup runs to completion even when an earlier one
            // rejected.
            let settled = futures::future::join_all(running).await;
            let failures: Vec<McpError> = settled.into_iter().filter_map(Result::err).collect();
            if failures.is_empty() {
                return Ok(());
            }
            let aggregate = McpError::RuntimeCleanupFailed(CleanupErrors::from(failures));
            report_cleanup_failure(&aggregate, false);
            Err(Arc::new(aggregate))
        }
        .boxed()
        .shared();

        *slot = Some(future.clone());
        future
    }

    /// `stop(reason = "MCP extension runtime stopped")` — cancel, run the LIFO cleanup stack, and
    /// return the aggregate. Memoised: the second call awaits the first call's future and returns
    /// the same result without re-running anything.
    ///
    /// Callers that need the cancel to be observable before the cleanup drains use
    /// [`Self::begin_stop`] instead.
    pub fn stop(&self, reason: Option<&str>) -> impl Future<Output = StopResult> + Send + 'static {
        self.begin_stop(reason)
    }

    /// Publish the reason, *then* cancel. The order is the contract.
    fn publish_reason_and_cancel(&self, reason: Option<&str>) {
        if self.is_active() {
            self.reason
                .store(Some(Arc::new(reason.unwrap_or(DEFAULT_STOP_REASON).to_string())));
        }
        self.token.cancel();
    }
}

/// `reportCleanupFailure(error, late)` — one formatter, one `late` flag, exactly as upstream.
/// The rendered text is `formatTerminalError`'s aggregate walk, which [`CleanupErrors`]'s `Display`
/// reproduces.
fn report_cleanup_failure(error: &McpError, late: bool) {
    let prefix = if late { "late " } else { "" };
    tracing::error!("MCP: {prefix}runtime cleanup failed: {error}");
}

// =================================================================================================
// MCP-006 — `createOwnedUi` as a fenced services handle
// =================================================================================================

/// `createOwnedUi(ui, owner)` — a [`cyrup_ext::HostServices`] handle that goes **inert** the moment
/// its owner stops.
///
/// The point, stated as upstream states it: after a session restart a stale reconnect callback that
/// calls `ui.notify(...)` does nothing at all — no crash, and no toast in the *new* session's TUI.
/// A stale handle becomes silently inert rather than throwing, so every `ui?.notify(...)` scattered
/// through the runtime needs no owner check of its own.
///
/// # Why this is an explicit delegating impl
///
/// Upstream is a recursive `Proxy` with a `WeakMap` identity cache. Rust has no `Proxy`, so the
/// fence is written out. The `WeakMap` has no analog and needs none: this wrapper is constructed
/// **once per generation**, not once per property read, so there is no identity to cache.
///
/// # The honest caveat about "a new trait method is a compile error"
///
/// It is not, and cannot be: every [`cyrup_ext::HostServices`] method has a default body, so a
/// method this impl does not list silently falls through to that default. The default is
/// "denied / empty" — the *same* value the fence returns — so an unlisted method behaves exactly
/// like a permanently-fenced one. That fails closed, which is the right direction, but it also
/// means an unlisted method never reaches the live backend even while the owner is active. The
/// `fenced!` macro below is therefore the place to add a method the moment a call site needs it,
/// and `MCP-006` completes the list against the full trait.
pub struct OwnedServices {
    inner: Arc<dyn cyrup_ext::HostServices>,
    owner: Arc<McpRuntimeOwner>,
}

impl OwnedServices {
    /// Fence `services` behind `owner`. Constructed once per generation, in
    /// [`crate::runtime::initialize_mcp`]'s `ui = rawUi ? createOwnedUi(rawUi, owner) : undefined`.
    #[must_use]
    pub fn new(services: Arc<dyn cyrup_ext::HostServices>, owner: Arc<McpRuntimeOwner>) -> Self {
        Self { inner: services, owner }
    }

    /// The owner this handle is fenced behind.
    #[must_use]
    pub fn owner(&self) -> &Arc<McpRuntimeOwner> {
        &self.owner
    }

    /// The live backend, **unfenced**. Only for a caller that has already proven the owner is
    /// active and needs a method this wrapper does not yet delegate.
    #[must_use]
    pub fn inner(&self) -> &Arc<dyn cyrup_ext::HostServices> {
        &self.inner
    }
}

impl std::fmt::Debug for OwnedServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedServices").field("active", &self.owner.is_active()).finish()
    }
}

/// Generate one owner-fenced delegation per [`cyrup_ext::HostServices`] method.
///
/// Each arm names the inert value returned once the owner has stopped — upstream's `undefined`,
/// expressed as the trait's own denied/empty default for that return type. `Result`-returning
/// methods report the stop rather than fabricating a success, because their callers branch on the
/// result and a silent `Ok` would look like the work happened.
macro_rules! fenced {
    ($( fn $name:ident ( & $me:ident $(, $arg:ident : $ty:ty )* ) $( -> $ret:ty )? => $inert:expr; )*) => {
        $(
            fn $name(& $me $(, $arg: $ty)*) $( -> $ret )? {
                // `$me` is the invocation site's own `self` token, captured as an `ident` so the
                // inert expression — written at that same site — can name it. Macro hygiene puts a
                // macro-internal `self` out of `$inert`'s reach, which is why this is threaded
                // through rather than hard-coded.
                if !$me.owner.is_active() {
                    return $inert;
                }
                $me.inner.$name($($arg),*)
            }
        )*
    };
}

// `clippy::unused_unit`: the `fenced!` arms below each state their inert value explicitly, and for
// a unit-returning method that value IS `()`. Writing it out is the point — a reader checking that a
// fenced method degrades the way upstream's `undefined` does should not have to infer the arm from
// its absence.
#[allow(clippy::unused_unit)]
impl cyrup_ext::HostServices for OwnedServices {
    fenced! {
        // --- ui: the surfaces a stale generation must never paint into -----------------------
        fn notify(&self, message: &str, kind: cyrup_ext::NotifyKind) => ();
        fn set_status(&self, key: &str, text: Option<&str>) => ();
        fn set_widget(
            &self,
            key: &str,
            lines: Option<&[String]>,
            placement: cyrup_ext::host::WidgetPlacement
        ) => ();
        fn set_working_message(&self, message: Option<&str>) => ();
        fn confirm(
            &self,
            prompt: &str,
            message: &str,
            opts: &cyrup_ext::DialogOptions
        ) -> bool => false;
        fn input(
            &self,
            prompt: &str,
            placeholder: Option<&str>,
            opts: &cyrup_ext::DialogOptions
        ) -> Option<String> => None;
        fn select(
            &self,
            prompt: &str,
            options: &serde_json::Value,
            opts: &cyrup_ext::DialogOptions
        ) -> Option<String> => None;
        fn oauth_prompt(
            &self,
            message: &str,
            placeholder: Option<&str>,
            allow_empty: bool
        ) -> Result<String, String> => Err(Self::inert_reason(&self.owner));
        fn oauth_select(
            &self,
            message: &str,
            options: &serde_json::Value
        ) -> Option<String> => None;
        fn editor(&self, title: &str, initial: &str) -> Option<String> => None;
        fn custom(&self, spec: &serde_json::Value) -> Option<String> => None;
        fn open_overlay(
            &self,
            overlay: Box<dyn cyrup_ext::InteractiveOverlay>
        ) -> bool => false;
        fn theme(&self) -> Option<String> => None;

        // --- session state a reconnect callback reads ------------------------------------------
        fn session_id(&self) -> Option<String> => None;
        fn session_file(&self) -> Option<std::path::PathBuf> => None;
        fn current_model(&self) -> Option<String> => None;
        fn models(&self) -> serde_json::Value => serde_json::json!([]);
        fn context_usage(&self) -> serde_json::Value => serde_json::json!({});
        fn is_idle(&self) -> bool => false;
        fn is_project_trusted(&self) -> bool => false;
        fn human_interaction_lock(
            &self
        ) -> Option<std::sync::Arc<cyrup_ext::HumanInteractionLock>> => None;

        // --- the caller-side half of the abort pair (13a "What does not fit cleanly" item 3):
        // a stopped generation reports "cancelled", which is exactly what a stale caller should
        // conclude.
        fn is_run_cancelled(&self) -> bool => true;

        // --- the tool surface: `deactivateTools`'s `setActiveTools(active \ removed)` fallback ---
        fn active_tools(&self) -> Option<Vec<String>> => None;
        fn all_tool_names(&self) -> Option<Vec<String>> => None;
        fn set_active_tools(&self, names: &[String]) => ();
        fn all_tools(&self) -> Option<Vec<serde_json::Value>> => None;
        fn commands(&self) -> Option<Vec<serde_json::Value>> => None;

        // --- control and injection --------------------------------------------------------------
        fn control(
            &self,
            op: cyrup_ext::ControlOp
        ) -> Result<(), String> => Err(Self::inert_reason(&self.owner));
        fn inject_message(
            &self,
            content: &str,
            custom_type: Option<&str>,
            display: bool,
            trigger_turn: bool
        ) -> Result<(), String> => Err(Self::inert_reason(&self.owner));
        fn emit_event(&self, topic: &str, payload: &serde_json::Value) => ();
        fn exec(
            &self,
            cmd: &str,
            args: &[String],
            opts: &serde_json::Value,
            cancel: cyrup_core::CancelToken
        ) -> Result<cyrup_ext::ExecOutput, String> => Err(Self::inert_reason(&self.owner));
    }
}

impl OwnedServices {
    /// The text a fenced `Result`-returning method reports. Carries the stop reason so a log line
    /// says *why* the handle is dead rather than merely that it is.
    fn inert_reason(owner: &McpRuntimeOwner) -> String {
        owner.stop_reason().map_or_else(|| DEFAULT_STOP_REASON.to_string(), |r| r.to_string())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn recording_cleanup(
        log: Arc<Mutex<Vec<&'static str>>>,
        name: &'static str,
        fail: bool
    ) -> Cleanup {
        Box::new(move || {
            async move {
                if let Ok(mut l) = log.lock() {
                    l.push(name);
                }
                if fail { Err(McpError::other(name)) } else { Ok(()) }
            }
            .boxed()
        })
    }

    #[tokio::test]
    async fn cleanups_run_lifo_and_all_of_them_run_even_when_one_fails() {
        let owner = McpRuntimeOwner::new();
        let log = Arc::new(Mutex::new(Vec::new()));
        owner.add_cleanup(recording_cleanup(log.clone(), "one", false));
        owner.add_cleanup(recording_cleanup(log.clone(), "two", true));
        owner.add_cleanup(recording_cleanup(log.clone(), "three", false));

        let err = owner.stop(None).await.expect_err("the middle cleanup failed");
        assert_eq!(*log.lock().unwrap(), vec!["three", "two", "one"], "strict LIFO");
        assert!(err.to_string().contains("two"), "the aggregate names the failure: {err}");
        assert!(err.is_cleanup_failure());
    }

    #[tokio::test]
    async fn stop_is_idempotent_and_runs_cleanups_once() {
        let owner = Arc::new(McpRuntimeOwner::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        owner.add_cleanup(Box::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }.boxed()
        }));

        assert!(owner.stop(Some("first")).await.is_ok());
        assert!(owner.stop(Some("second")).await.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            owner.stop_reason().map(|r| r.to_string()).as_deref(),
            Some("first"),
            "the reason is the FIRST stop's; a later stop cannot rewrite it"
        );
    }

    #[tokio::test]
    async fn begin_stop_cancels_before_the_cleanup_drains() {
        let owner = Arc::new(McpRuntimeOwner::new());
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let mut rx = Some(rx);
        owner.add_cleanup(Box::new(move || {
            async move {
                if let Some(rx) = rx.take() {
                    let _ = rx.await;
                }
                Ok(())
            }
            .boxed()
        }));

        let draining = owner.begin_stop(Some("restart"));
        // The cancel is observable immediately, while the cleanup is still blocked.
        assert!(!owner.is_active());
        assert!(owner.token().is_cancelled());
        let _ = tx.send(());
        assert!(draining.await.is_ok());
    }

    #[tokio::test]
    async fn late_cleanup_runs_immediately() {
        let owner = McpRuntimeOwner::new();
        assert!(owner.stop(None).await.is_ok());
        let ran = Arc::new(AtomicUsize::new(0));
        let seen = ran.clone();
        owner.add_cleanup(Box::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }.boxed()
        }));
        tokio::task::yield_now().await;
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn throw_if_inactive_reports_the_stop_reason() {
        let owner = McpRuntimeOwner::new();
        assert!(owner.throw_if_inactive().is_ok());
        let _ = owner.stop(Some("MCP extension session restarted")).await;
        let err = owner.throw_if_inactive().expect_err("stopped");
        assert_eq!(err.to_string(), "MCP extension session restarted");
        assert!(matches!(err, McpError::Aborted(_)));
    }

    #[tokio::test]
    async fn default_stop_reason_is_the_literal_upstream_string() {
        let owner = McpRuntimeOwner::new();
        let _ = owner.stop(None).await;
        assert_eq!(owner.throw_if_inactive().unwrap_err().to_string(), DEFAULT_STOP_REASON);
    }
}

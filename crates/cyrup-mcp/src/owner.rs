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
//!
//! # The two sections below `createOwnedUi`, and why they are here
//!
//! [`OwnedServices`] is this crate's **only** route to the human, so the two things that route has
//! to do before it paints anything live beside it:
//!
//! * [`McpDialog`] (MCP-471) — the one place a `confirm`/`select` is taken under the session's
//!   [`cyrup_ext::HumanInteractionLock`] and a [`cyrup_ext::HostCtx::begin_human_wait`] guard. A
//!   cyrup-only mechanism with no upstream line to port, which is exactly why it must be written
//!   down once rather than re-derived at each dialog.
//! * [`confirm_sampling`] and its two formatters (MCP-455) — `sampling-handler.ts`'s consent gate,
//!   which is four literal strings plus one [`McpDialog`] call.
//!
//! *Provenance: `sampling-handler.ts` is byte-identical at v2.25.0 and v2.26.1
//! (`git diff v2.25.0 v2.26.1 -- sampling-handler.ts` is empty), so the bare `file:line` citations
//! below resolve correctly under either reading of this crate's citation convention.*
//!
//! **Placement delta, stated plainly.** The plan's module map (`13-cyrup-mcp.md`, the file table)
//! gives `sampling.rs` the whole of `sampling-handler.ts`, and MCP-455 belongs there. That module
//! does not exist yet — MCP-450 creates it — and creating it means declaring it in `lib.rs`, which
//! is not this change's to touch. So the gate lands beside the machinery it shares with MCP-232's
//! tool-approval gate (the point of landing them together), and moves wholesale when `sampling.rs`
//! arrives: a `pub use crate::owner::{confirm_sampling, format_request_approval, …}` there keeps
//! every path already written against it valid, the same technique [`crate::state`] uses for its
//! forward declarations. Nothing here reads an [`McpRuntimeOwner`] field, so the move is a cut and
//! paste.

use std::future::Future;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use cyrup_core::{CancelToken, Content, Message};
use futures::future::{BoxFuture, FutureExt, Shared};
// SEP-2577 deprecates `sampling/createMessage` protocol-wide and rmcp marks the types accordingly;
// `pi-mcp-adapter` ships a sampling handler and 1:1 parity is a hard rule, so the deprecation is
// acknowledged and suppressed rather than obeyed — the identical suppression, for the identical
// reason, as `crate::runtime`'s own rmcp import block.
#[allow(deprecated)]
use rmcp::model::{CreateMessageResult, SamplingMessageContentBlock};

use crate::errors::{CleanupErrors, McpError, McpResult};
use crate::registration::truncate_at_word;

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
            details: Option<&serde_json::Value>,
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

        // --- editor / theme / layout: pure paint -------------------------------------------------
        // The `serde_json::Value` arms below return each method's OWN trait default rather than a
        // blanket `Value::Null`, so the invariant this type's doc states — "an unlisted method
        // behaves exactly like a permanently-fenced one" — holds for them the way it already does
        // for `models` (`json!([])`) and `context_usage` (`json!({})`) above. A caller that reads
        // `entries()` with `as_array()` therefore sees an empty transcript, not "unavailable".
        fn editor_text(&self) -> String => String::new();
        fn set_editor_text(&self, text: &str, is_paste: bool) => ();
        fn theme_list(&self) -> serde_json::Value => serde_json::json!([]);
        fn theme_by_name(&self, name: &str) -> Option<serde_json::Value> => None;
        fn set_theme(&self, name: &str) -> Result<(), String> => Err(Self::inert_reason(&self.owner));
        fn tools_expanded(&self) -> bool => false;
        fn set_tools_expanded(&self, expanded: bool) => ();
        fn set_working_visible(&self, visible: bool) => ();
        fn set_working_indicator(&self, opts: Option<&serde_json::Value>) => ();
        fn set_hidden_thinking_label(&self, label: Option<&str>) => ();
        fn set_header(&self, content: &str) => ();
        fn set_footer(&self, content: &str) => ();
        fn set_title(&self, title: &str) => ();
        fn thinking_level(&self) -> Option<String> => None;

        // --- session / transcript -----------------------------------------------------------------
        fn entries(&self) -> serde_json::Value => serde_json::json!([]);
        fn branch(&self) -> serde_json::Value => serde_json::json!([]);
        fn tree(&self) -> serde_json::Value => serde_json::Value::Null;
        fn session_name(&self) -> Option<String> => None;
        fn set_session_name(&self, name: &str) => ();
        fn set_label(&self, entry_id: &str, label: Option<&str>) => ();
        fn append_entry(
            &self,
            custom_type: &str,
            data: &serde_json::Value
        ) -> Result<String, String> => Err(Self::inert_reason(&self.owner));
        fn has_pending_messages(&self) -> bool => false;
        fn system_prompt(&self) -> Option<String> => None;
        fn system_prompt_options(&self) -> Option<serde_json::Value> => None;
        fn scoped_models(&self) -> serde_json::Value => serde_json::json!([]);

        // --- http: a stale generation must not issue a request -------------------------------------
        fn http_request(
            &self,
            req: &cyrup_ext::host::HttpRequest
        ) -> Result<cyrup_ext::host::HttpResponse, String> => Err(Self::inert_reason(&self.owner));
        fn http_request_stream(
            &self,
            req: &cyrup_ext::host::HttpRequest
        ) -> Result<cyrup_ext::host::HttpStreamResponse, String> => Err(Self::inert_reason(&self.owner));
        fn http_poll_stream_chunk(
            &self,
            handle: u32
        ) -> Result<Option<Vec<u8>>, String> => Err(Self::inert_reason(&self.owner));
        fn http_close_stream(&self, handle: u32) => ();

        // --- proc: a stale generation spawning a child is the exact leak the owner prevents ---------
        fn proc_spawn(
            &self,
            spec: &cyrup_ext::host::ProcSpawnSpec
        ) -> Result<u32, String> => Err(Self::inert_reason(&self.owner));
        fn proc_write_stdin(
            &self,
            handle: u32,
            data: &[u8]
        ) -> Result<u32, String> => Err(Self::inert_reason(&self.owner));
        fn proc_read_stdout(
            &self,
            handle: u32,
            max_bytes: u32
        ) -> Result<Vec<u8>, String> => Err(Self::inert_reason(&self.owner));
        fn proc_read_stderr(
            &self,
            handle: u32,
            max_bytes: u32
        ) -> Result<Vec<u8>, String> => Err(Self::inert_reason(&self.owner));
        fn proc_poll_exit(&self, handle: u32) -> Option<i32> => None;
        fn proc_kill(&self, handle: u32) -> Result<(), String> => Err(Self::inert_reason(&self.owner));
    }
}

impl OwnedServices {
    /// The text a fenced `Result`-returning method reports. Carries the stop reason so a log line
    /// says *why* the handle is dead rather than merely that it is.
    fn inert_reason(owner: &McpRuntimeOwner) -> String {
        owner.stop_reason().map_or_else(|| DEFAULT_STOP_REASON.to_string(), |r| r.to_string())
    }
}

// =================================================================================================
// MCP-471 — one serialized route to the human
// =================================================================================================

/// One dialog to the human, held under **both** of cyrup's serialization primitives.
///
/// # There is no upstream line to port here, and that is the whole point
///
/// Upstream calls `state.ui.select(...)` / `options.ui.confirm(...)` directly. It can: pi has no
/// invocation-budget watchdog on an extension handler, and no second companion extension that can
/// prompt the same human. cyrup has both, so MCP-471 makes every `confirm`/`select`/`input` this
/// crate performs take two things first. Because there is nothing upstream to copy, a porter
/// reading `sampling-handler.ts` or `tool-approval.ts` will not discover this — it has to be one
/// type that both gates go through, or the second gate will be written without it.
///
/// * [`cyrup_ext::HostServices::human_interaction_lock`] — the **one** session-scoped
///   [`cyrup_ext::HumanInteractionLock`]. `cyrup-permission-system`'s `ask` dialog
///   (`ask.rs`'s `LocalAskChannel`) and `cyrup-intercom`'s clarify prompt already acquire it. Both
///   of this crate's consent gates can genuinely race one: an MCP `sampling/createMessage` arrives
///   on a background rmcp task with no relation to the turn the permission gate is prompting about.
///   Without the lock the two dialogs paint over each other and the human answers whichever won.
/// * [`cyrup_ext::HostCtx::begin_human_wait`] — the P-3 budget-forgiveness guard, `#[must_use]`
///   precisely because dropping it immediately does nothing.
///
/// # The honest limit of the P-3 guard at today's two call sites
///
/// The dispatcher's budget wraps `Extension::invoke_event` only
/// (`cyrup_ext::dispatch::Dispatcher::invoke_contained`). Neither consent gate runs there today:
/// MCP-232's dialog runs inside `Tool::execute` and MCP-455's inside
/// `ClientHandler::create_message`, and the agent loop puts no budget on either. So the guard is
/// *defensive* at these two sites rather than load-bearing — what it actually buys today is that
/// an MCP `on_event` handler dispatched **while a dialog is open** (`HostEvent::Input`'s pre-turn
/// convergence is the live example) is forgiven instead of budget-killed, because
/// [`cyrup_ext::HostCtx::human_wait_gate`] is one shared `Arc` per native handle. The lock is the
/// half that is load-bearing now. Both are taken because the alternative — deciding per call site
/// which primitive "matters" — is how a gate ends up holding neither.
///
/// # Why the dialog is called on this thread rather than through `spawn_blocking`
///
/// `HostServices::{confirm,select}` are the SYNC blocking bridges: the live backend's
/// `LiveHostServices::ui_roundtrip` already does `block_in_place` + `block_on` internally, so the
/// worker thread is released for the human's latency without the caller arranging anything. This
/// is the same call shape `cyrup-permission-system`'s `LocalAskChannel` uses, and matching it
/// matters more than the alternative would gain: the two must serialize, and they now do it the
/// same way.
pub struct McpDialog {
    /// The services handle the dialog is drawn through — normally the **fenced**
    /// [`OwnedServices`] from [`crate::state::McpState::ui`], so a stale generation's dialog is
    /// inert (`select` → `None` → the caller's fail-closed default) rather than painting into the
    /// session that replaced it.
    services: Arc<dyn cyrup_ext::HostServices>,
    /// The dispatch context whose [`cyrup_ext::HumanWaitGate`] the budget watchdog polls. `None`
    /// where no handler ctx has been observed yet — see [`crate::state::McpState::human_wait_ctx`]
    /// for who records it and when.
    human_wait: Option<cyrup_ext::HostCtx>,
}

impl McpDialog {
    /// A dialog over an arbitrary services backend.
    #[must_use]
    pub fn new(services: Arc<dyn cyrup_ext::HostServices>) -> Self {
        Self { services, human_wait: None }
    }

    /// A dialog over the generation's **fenced** handle — `state.ui`, i.e. `createOwnedUi`'s
    /// result. This is the constructor production code should reach for; [`Self::new`] exists for
    /// the callers (tests, a headless embedding) that hold a raw backend.
    #[must_use]
    pub fn fenced(ui: &Arc<OwnedServices>) -> Self {
        Self::new(Arc::clone(ui) as Arc<dyn cyrup_ext::HostServices>)
    }

    /// Attach the handler context whose P-3 gate the dispatcher polls. `None` is a legal, silent
    /// degradation: the interaction lock is still taken, only the budget forgiveness is not.
    #[must_use]
    pub fn with_human_wait(mut self, ctx: Option<cyrup_ext::HostCtx>) -> Self {
        self.human_wait = ctx;
        self
    }

    /// Take both guards. Returned as a tuple the caller binds to a **named** `_guards` local:
    /// `let _ = self.enter().await` would drop them on the spot and hold nothing, which is the one
    /// way to write this that compiles and does nothing.
    async fn enter(
        &self,
    ) -> (Option<cyrup_ext::HumanInteractionGuard>, Option<cyrup_ext::HumanWaitGuard>) {
        // `acquire()` WAITS (it never rejects): an MCP approval queues behind an in-flight
        // permission prompt rather than being dropped. A backend with no lock — the default host,
        // a headless embedding — has no other human to collide with, so `None` is not a failure.
        let human = match self.services.human_interaction_lock() {
            Some(lock) => Some(lock.acquire().await),
            None => None,
        };
        let wait = self.human_wait.as_ref().map(cyrup_ext::HostCtx::begin_human_wait);
        (human, wait)
    }

    /// `ui.confirm(title, message)` — upstream passes no options bag, so neither does this.
    pub async fn confirm(&self, prompt: &str, message: &str) -> bool {
        let _guards = self.enter().await;
        self.services.confirm(prompt, message, &cyrup_ext::DialogOptions::default())
    }

    /// `ui.select(prompt, options)` — the chosen label, or `None` for a dismissal, a timeout, or
    /// no interactive surface. Every caller must treat `None` as a refusal.
    pub async fn select(&self, prompt: &str, options: &[&str]) -> Option<String> {
        let _guards = self.enter().await;
        let rendered = serde_json::Value::Array(
            options.iter().map(|option| serde_json::Value::String((*option).to_string())).collect(),
        );
        self.services.select(prompt, &rendered, &cyrup_ext::DialogOptions::default())
    }

    /// `ui.input(title, placeholder)` — the typed value, or `None` for a dismissal.
    ///
    /// `placeholder` is upstream's seed: `current === undefined ? undefined : String(current)`, so a
    /// re-prompt after a validation failure does not lose what the user typed.
    pub async fn input(&self, prompt: &str, placeholder: Option<&str>) -> Option<String> {
        let _guards = self.enter().await;
        self.services
            .input(prompt, placeholder, &cyrup_ext::DialogOptions::default())
    }

    /// `ui.notify(message, kind)` — fire-and-forget, and deliberately NOT under [`Self::enter`].
    ///
    /// A toast asks the human nothing, so taking the interaction lock for it would make a validation
    /// message queue behind the very dialog it is about to be shown beside. Upstream's `notify` is
    /// likewise outside every `await ui.select(...)`.
    pub fn notify(&self, message: &str, kind: cyrup_ext::NotifyKind) {
        self.services.notify(message, kind);
    }
}

impl std::fmt::Debug for McpDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpDialog")
            .field("human_wait", &self.human_wait.is_some())
            .finish_non_exhaustive()
    }
}

// =================================================================================================
// MCP-455 — `confirmSampling` and the two approval dialogs (`sampling-handler.ts:185-194`)
// =================================================================================================

/// `sampling-handler.ts:64` — the title of the **request** dialog.
///
/// Handed to [`confirm_sampling`] by [`crate::sampling::handle_sampling_request`], the
/// [`crate::runtime::SamplingHook`] that
/// [`crate::server_manager::McpServerManager::set_sampling_config`] carries from
/// [`crate::runtime::initialize_mcp`]'s step 5.
pub const SAMPLING_REQUEST_APPROVAL_TITLE: &str = "Approve MCP sampling request";

/// `sampling-handler.ts:89` — the title of the **response** dialog.
///
/// Read by [`crate::sampling::handle_sampling_request`]'s second gate — the one that shows the
/// human what the server is about to receive, after the model has answered.
pub const SAMPLING_RESPONSE_APPROVAL_TITLE: &str = "Return MCP sampling response";

/// `sampling-handler.ts:188` — thrown when approval is required and there is no UI to ask through.
///
/// The sentence names the setting that makes headless sampling legal, which is the only reason it
/// is a distinct message from [`SAMPLING_REQUEST_DECLINED`]: one is "I cannot ask you", the other
/// is "I asked and you said no", and a user who cannot tell them apart cannot fix the first.
pub const SAMPLING_REQUIRES_INTERACTIVE_APPROVAL: &str =
    "MCP sampling requires interactive approval. Set settings.samplingAutoApprove to true to allow it without UI.";

/// `sampling-handler.ts:192` — thrown when the human answered the dialog with "no".
pub const SAMPLING_REQUEST_DECLINED: &str = "MCP sampling request was declined";

/// The three fields `confirmSampling` reads off `SamplingHandlerOptions`
/// (`sampling-handler.ts:18-25`), and nothing else.
///
/// Upstream's bag also carries `serverName`, `modelRegistry`, `getCurrentModel` and `getSignal`;
/// those belong to MCP-450/452/458 and are deliberately absent here so the gate stays testable
/// without a model registry. `SamplingUIContext = Pick<ExtensionUIContext, "confirm">` — sampling
/// may call **`confirm` only**, which is why this carries an [`McpDialog`] rather than a services
/// handle.
///
/// Constructed by [`crate::sampling::SamplingOptions::approval`], once per dialog, so a generation
/// that stopped between the request gate and the response gate is inert at the second one. This
/// unit (MCP-455) is the *gate*; [`crate::sampling::handle_sampling_request`] is its caller.
#[derive(Debug)]
pub struct SamplingApproval {
    /// `options.autoApprove` — `settings.samplingAutoApprove === true`
    /// ([`crate::config::McpSettings::sampling_auto_approve`]). Short-circuits **both** dialogs.
    ///
    /// Read only by [`confirm_sampling`], and populated only by the unported MCP-118 handler, so
    /// `settings.samplingAutoApprove` has no production effect yet — a user who sets it changes
    /// nothing until section 05 lands.
    pub auto_approve: bool,
    /// `ctx.hasUI`, carried explicitly.
    ///
    /// **This is the field the plan singles out (MCP-455), and it cannot be inferred.** Upstream
    /// distinguishes "no UI" from "declined" by testing `options.ui` for presence *before*
    /// awaiting `confirm`. `HostServices::confirm` returns a bare `bool`, so a `false` from a
    /// backend with no interactive surface is indistinguishable from a human pressing "No" — and
    /// collapsing the two would replace an actionable message ("set samplingAutoApprove") with a
    /// misleading one ("you declined"). The bool comes from `HostConfig.has_ui`, the same value
    /// `crate::runtime::ContextSnapshot::has_ui` already snapshots.
    pub has_ui: bool,
    /// `options.ui` — `None` in a headless build.
    pub dialog: Option<McpDialog>,
}

/// `sampling-handler.ts:185-194` `confirmSampling(options, title, message)` — the consent gate for
/// spending the user's model credentials on server-directed input.
///
/// Three branches, in upstream's order:
///
/// 1. `if (options.autoApprove) return;` — the user pre-authorised it.
/// 2. `if (!options.ui) throw …` — [`SAMPLING_REQUIRES_INTERACTIVE_APPROVAL`].
/// 3. `if (!approved) throw …` — [`SAMPLING_REQUEST_DECLINED`].
///
/// **Fail-closed polarity.** Upstream's `await options.ui.confirm(...)` on an owner-fenced UI
/// resolves `undefined` after a stop, which is falsy, which throws. Here the fence returns `false`
/// from [`OwnedServices`]'s `confirm` arm, which is the same answer through the same door. There is
/// no arm on which an unanswered, dismissed, fenced or backend-less dialog yields `Ok(())`.
///
/// `has_ui && dialog.is_none()` is a wiring bug rather than a reachable state, and it resolves to
/// branch 2: a gate with nothing to ask through has no UI, whatever a flag says.
///
/// **No production caller.** Its caller is section 05's `registerSamplingHandler` body (MCP-118),
/// which is unported; until it lands the only exercise this gate gets is this module's tests. That
/// is safe in the direction that matters — with no handler installed, no server can ask for
/// sampling at all — so the missing caller costs the *capability*, not the *consent*.
pub async fn confirm_sampling(
    approval: &SamplingApproval,
    title: &str,
    message: &str,
) -> McpResult<()> {
    if approval.auto_approve {
        return Ok(());
    }
    let Some(dialog) = approval.dialog.as_ref().filter(|_| approval.has_ui) else {
        return Err(McpError::other(SAMPLING_REQUIRES_INTERACTIVE_APPROVAL));
    };
    if dialog.confirm(title, message).await {
        Ok(())
    } else {
        Err(McpError::other(SAMPLING_REQUEST_DECLINED))
    }
}

/// `sampling-handler.ts:95-109` `formatRequestApproval(serverName, modelName, systemPrompt,
/// messages)` — the body of the **request** dialog.
///
/// One paragraph per line, joined with `"\n\n"`:
///
/// * the header, with `message${n === 1 ? "" : "s"}` — the pluralisation is on **1**, so a
///   zero-message request reads "0 messages";
/// * `System: <400>` — emitted only when `systemPrompt` is *truthy*, so an empty string is skipped
///   exactly as an absent one is (hence the `.filter(|p| !p.is_empty())`, not a bare `if let`);
/// * one **1-indexed** row per message, `"{i}. {role}: {text}"`.
///
/// Both truncations are `truncateAtWord(..., 400)` — the word-boundary cut with the ASCII `...`
/// tail ([`truncate_at_word`], MCP-459). The dialog is meant to be *inspectable*: this is the only
/// place the user sees what a server is about to spend their credentials on.
///
/// **No production caller.** It renders the body [`confirm_sampling`] is given under
/// [`SAMPLING_REQUEST_APPROVAL_TITLE`], and both are called from section 05's
/// `registerSamplingHandler` body (MCP-118), which is unported.
#[must_use]
pub fn format_request_approval(
    server_name: &str,
    model_name: &str,
    system_prompt: Option<&str>,
    messages: &[Message],
) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(messages.len() + 2);
    let count = messages.len();
    let plural = if count == 1 { "" } else { "s" };
    lines.push(format!("{server_name} wants to sample {count} message{plural} with {model_name}."));
    if let Some(prompt) = system_prompt.filter(|prompt| !prompt.is_empty()) {
        lines.push(format!("System: {}", truncate_at_word(prompt, 400)));
    }
    for (index, message) in messages.iter().enumerate() {
        lines.push(format!(
            "{}. {}: {}",
            index + 1,
            message_role(message),
            truncate_at_word(&message_text(message), 400)
        ));
    }
    lines.join("\n\n")
}

/// `sampling-handler.ts:111-114` `formatResponseApproval(serverName, response)` — the body of the
/// **response** dialog, at a 1000-character budget rather than 400.
///
/// The second gate exists because the first one cannot cover it: the user approved a *request*, and
/// what comes back is model output the server has not seen yet. `text` when the block is text,
/// `` `[${block.type} content]` `` otherwise.
///
/// **The block tag is read off the serialised form, not off a `match`.** Upstream interpolates
/// `response.content.type` — the wire discriminant — and rmcp's `SamplingMessageContentBlock` is
/// `#[non_exhaustive]`, so a hand-written match would need a catch-all arm that has to *invent* a
/// name for a variant it has never seen. Serialising and reading `"type"` yields the same string
/// upstream prints, for every variant rmcp has and every one it adds.
///
/// Upstream's `response.content` is a single block; rmcp models SEP-1577's single-or-array form as
/// [`rmcp::model::SamplingContent`], so the first block is read. `convert_assistant_result`
/// (MCP-456) emits exactly one text block, so the array arm and the empty arm are unreachable from
/// this crate's own producer; the empty arm renders what JS would (`.type` of an array is
/// `undefined`) rather than inventing a friendlier string for a state that cannot occur.
///
/// **No production caller**, for the same reason as [`format_request_approval`]: the second dialog
/// is opened by section 05's `registerSamplingHandler` body (MCP-118), which is unported.
#[must_use]
#[allow(deprecated)]
pub fn format_response_approval(server_name: &str, response: &CreateMessageResult) -> String {
    let text = match response.message.content.first() {
        Some(SamplingMessageContentBlock::Text(block)) => block.text.clone(),
        Some(other) => format!("[{} content]", sampling_block_type(other)),
        None => "[undefined content]".to_string(),
    };
    format!(
        "{server_name} will receive this response from {}:\n\n{}",
        response.model,
        truncate_at_word(&text, 1000)
    )
}

/// `block.type` — the wire discriminant, read back out of the serialised block. `"unknown"` is
/// unreachable: every `SamplingMessageContentBlock` is an internally-tagged struct variant, so its
/// serialisation always carries a string `"type"`.
#[allow(deprecated)]
pub(crate) fn sampling_block_type(block: &SamplingMessageContentBlock) -> String {
    serde_json::to_value(block)
        .ok()
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

/// `message.role` — the wire spelling, which is what upstream interpolates.
///
/// `toolResult` cannot be produced by `convertSamplingMessage` (MCP-456 builds only `user` and
/// `assistant`); it is spelled here because [`Message`] admits it and a silent `_ => "user"` would
/// mislabel one if it ever arrived.
fn message_role(message: &Message) -> &'static str {
    match message {
        Message::User { .. } => "user",
        Message::Assistant(_) => "assistant",
        Message::ToolResult { .. } => "toolResult",
    }
}

/// `sampling-handler.ts:116-125` `messageText(message)` — the blocks flattened with `"\n"`.
///
/// # Two of upstream's spellings have no reachable producer here, and both are recorded
///
/// * `typeof message.content === "string"` — [`Message`]'s content is always `Vec<Content>`; the
///   bare-string form is tolerated on *read* by `cyrup_core`'s deserializer and promoted to a
///   single text block, so it never reaches this function.
/// * the `return "[content]"` fallthrough — [`Content`] is a closed four-variant enum, so a
///   wildcard arm after the four below is an `unreachable_patterns` error, not a defensive
///   default. The string survives as [`MESSAGE_TEXT_UNKNOWN_BLOCK`] so that the day `Content`
///   gains a fifth variant the arm is a compile error with the right text already written down —
///   the same "record the string, name why it has no throw site" treatment MCP-451 gives the
///   `task` guard.
fn message_text(message: &Message) -> String {
    let blocks: &[Content] = match message {
        Message::User { content, .. } => content,
        Message::Assistant(assistant) => &assistant.content,
        Message::ToolResult { content, .. } => content,
    };
    blocks
        .iter()
        .map(|block| match block {
            Content::Text { text, .. } => text.clone(),
            Content::Image { mime_type, .. } => format!("[image: {mime_type}]"),
            Content::Thinking { .. } => "[thinking]".to_string(),
            Content::ToolCall(call) => format!("[tool call: {}]", call.name),
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// `sampling-handler.ts:123` — `messageText`'s fifth spelling, the fallthrough for a block kind the
/// switch does not name. It has **no producer** over `cyrup_core`'s closed [`Content`]; see
/// [`message_text`] for why it is written down anyway.
///
/// **No reader either, and deliberately so: this is a recorded string, not a code path.** It is not
/// staged for a unit and nothing should wire it — the day [`Content`] gains a fifth variant,
/// [`message_text`]'s `match` becomes a compile error with the right text already sitting here.
pub const MESSAGE_TEXT_UNKNOWN_BLOCK: &str = "[content]";

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

    // ==============================================================================================
    // MCP-471 — the dialog holds the interaction lock and the P-3 guard
    // ==============================================================================================

    /// A [`cyrup_ext::HostServices`] that reports, from *inside* the blocking dialog call, what the
    /// two MCP-471 primitives looked like at that moment — which is the only instant at which the
    /// assertion means anything.
    struct ProbeUi {
        lock: Arc<cyrup_ext::HumanInteractionLock>,
        gate: Arc<cyrup_ext::HumanWaitGate>,
        /// `is_waiting()` observed while the dialog was open.
        waiting_during_dialog: Mutex<Vec<bool>>,
        /// How many dialogs were open at once — 1 iff the interaction lock actually serializes.
        depth: Arc<AtomicUsize>,
        max_depth: Arc<AtomicUsize>,
        /// How long each dialog occupies its thread, so two concurrent ones really do overlap on a
        /// multi-threaded runtime when nothing serializes them.
        hold: std::time::Duration,
        answer: bool,
    }

    impl ProbeUi {
        fn new(gate: Arc<cyrup_ext::HumanWaitGate>, hold_ms: u64, answer: bool) -> Arc<Self> {
            Arc::new(Self {
                lock: Arc::new(cyrup_ext::HumanInteractionLock::new()),
                gate,
                waiting_during_dialog: Mutex::new(Vec::new()),
                depth: Arc::new(AtomicUsize::new(0)),
                max_depth: Arc::new(AtomicUsize::new(0)),
                hold: std::time::Duration::from_millis(hold_ms),
                answer,
            })
        }

        fn observe(&self) {
            self.waiting_during_dialog.lock().unwrap().push(self.gate.is_waiting());
            let depth = self.depth.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_depth.fetch_max(depth, Ordering::SeqCst);
            std::thread::sleep(self.hold);
            self.depth.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl cyrup_ext::HostServices for ProbeUi {
        fn human_interaction_lock(&self) -> Option<Arc<cyrup_ext::HumanInteractionLock>> {
            Some(Arc::clone(&self.lock))
        }
        fn confirm(&self, _prompt: &str, _message: &str, _opts: &cyrup_ext::DialogOptions) -> bool {
            self.observe();
            self.answer
        }
        fn select(
            &self,
            _prompt: &str,
            _options: &serde_json::Value,
            _opts: &cyrup_ext::DialogOptions,
        ) -> Option<String> {
            self.observe();
            self.answer.then(|| "Allow once".to_string())
        }
    }

    fn probe_ctx() -> cyrup_ext::HostCtx {
        cyrup_ext::HostCtx::event(
            cyrup_ext::ExtMode::Tui,
            true,
            std::path::PathBuf::from("/workspace"),
        )
    }

    /// Both dialog verbs hold a [`cyrup_ext::HumanWaitGuard`] for the whole of the blocking call,
    /// and release it the instant the call returns. Observed from inside the dialog, because a
    /// guard taken and dropped around nothing is exactly the mistake `#[must_use]` exists to catch.
    #[tokio::test]
    async fn both_dialog_verbs_hold_the_p3_guard_for_the_duration_of_the_call() {
        let ctx = probe_ctx();
        let gate = ctx.human_wait_gate();
        let ui = ProbeUi::new(Arc::clone(&gate), 0, true);
        let dialog = McpDialog::new(Arc::clone(&ui) as Arc<dyn cyrup_ext::HostServices>)
            .with_human_wait(Some(ctx));

        assert!(!gate.is_waiting(), "idle before the first dialog");
        assert!(dialog.confirm("t", "m").await);
        assert!(!gate.is_waiting(), "released as soon as `confirm` returned");
        assert_eq!(dialog.select("p", &["Allow once"]).await.as_deref(), Some("Allow once"));
        assert!(!gate.is_waiting(), "released as soon as `select` returned");

        assert_eq!(
            *ui.waiting_during_dialog.lock().unwrap(),
            vec![true, true],
            "the dispatcher's budget must be suspended while the human is being asked"
        );
    }

    /// With no ctx to take a guard from, the dialog still runs — the interaction lock is
    /// independent of the P-3 guard, and losing budget forgiveness must never lose the dialog.
    #[tokio::test]
    async fn a_dialog_without_a_ctx_still_opens() {
        let ctx = probe_ctx();
        let ui = ProbeUi::new(ctx.human_wait_gate(), 0, true);
        let dialog = McpDialog::new(Arc::clone(&ui) as Arc<dyn cyrup_ext::HostServices>);
        assert!(dialog.confirm("t", "m").await);
        assert_eq!(*ui.waiting_during_dialog.lock().unwrap(), vec![false]);
    }

    /// The session-scoped [`cyrup_ext::HumanInteractionLock`] genuinely serializes: two dialogs
    /// racing on a multi-threaded runtime are never open at the same time.
    ///
    /// The `hold` is a **blocking** sleep on the dialog's own thread, which is what a real
    /// `HostServices::confirm` does; with four workers and no lock both would be inside `observe`
    /// at once and `max_depth` would reach 2.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_interaction_lock_serializes_two_concurrent_dialogs() {
        let ui = ProbeUi::new(probe_ctx().human_wait_gate(), 60, true);
        let first = McpDialog::new(Arc::clone(&ui) as Arc<dyn cyrup_ext::HostServices>);
        let second = McpDialog::new(Arc::clone(&ui) as Arc<dyn cyrup_ext::HostServices>);

        let a = tokio::spawn(async move { first.confirm("first", "m").await });
        let b = tokio::spawn(async move { second.confirm("second", "m").await });
        assert_eq!((a.await.ok(), b.await.ok()), (Some(true), Some(true)));

        assert_eq!(
            ui.max_depth.load(Ordering::SeqCst),
            1,
            "an MCP dialog and any other companion's prompt must never be on screen together"
        );
    }

    // ==============================================================================================
    // MCP-455 — `confirmSampling` and the two formatters
    // ==============================================================================================

    /// A [`cyrup_ext::HostServices`] whose `confirm` answers a fixed verdict and counts its calls.
    #[derive(Default)]
    struct SamplingUi {
        approved: bool,
        calls: Mutex<Vec<(String, String)>>,
    }

    impl SamplingUi {
        fn answering(approved: bool) -> Arc<Self> {
            Arc::new(Self { approved, calls: Mutex::new(Vec::new()) })
        }
    }

    impl cyrup_ext::HostServices for SamplingUi {
        fn confirm(&self, prompt: &str, message: &str, _opts: &cyrup_ext::DialogOptions) -> bool {
            self.calls.lock().unwrap().push((prompt.to_string(), message.to_string()));
            self.approved
        }
    }

    fn approval_with(ui: Option<Arc<SamplingUi>>, auto_approve: bool) -> SamplingApproval {
        SamplingApproval {
            auto_approve,
            has_ui: ui.is_some(),
            dialog: ui
                .map(|ui| McpDialog::new(ui as Arc<dyn cyrup_ext::HostServices>)),
        }
    }

    /// `sampling-handler.ts:64` and `:89`. Wire contract: these strings are what the human reads to
    /// tell the two dialogs apart.
    #[test]
    fn the_two_sampling_dialog_titles_are_the_upstream_literals() {
        assert_eq!(SAMPLING_REQUEST_APPROVAL_TITLE, "Approve MCP sampling request");
        assert_eq!(SAMPLING_RESPONSE_APPROVAL_TITLE, "Return MCP sampling response");
    }

    /// Branch 1 — `if (options.autoApprove) return;`, evaluated **before** the UI is looked at, so
    /// a headless auto-approving session samples without a dialog and without the no-UI error.
    #[tokio::test]
    async fn auto_approve_short_circuits_before_the_ui_is_consulted() {
        let headless = SamplingApproval { auto_approve: true, has_ui: false, dialog: None };
        assert!(confirm_sampling(&headless, SAMPLING_REQUEST_APPROVAL_TITLE, "body").await.is_ok());

        let ui = SamplingUi::answering(false);
        let would_decline = SamplingApproval {
            auto_approve: true,
            has_ui: true,
            dialog: Some(McpDialog::new(Arc::clone(&ui) as Arc<dyn cyrup_ext::HostServices>)),
        };
        assert!(confirm_sampling(&would_decline, SAMPLING_RESPONSE_APPROVAL_TITLE, "b").await.is_ok());
        assert!(ui.calls.lock().unwrap().is_empty(), "no dialog is opened at all");
    }

    /// Branches 2 and 3 — the two messages, byte for byte, and the fact that they are **different
    /// messages**. That difference is the unit: it is why `has_ui` is carried explicitly instead of
    /// being inferred from a `false` out of `confirm`.
    #[tokio::test]
    async fn no_ui_and_a_declined_dialog_are_different_errors() {
        let headless = approval_with(None, false);
        let no_ui = confirm_sampling(&headless, SAMPLING_REQUEST_APPROVAL_TITLE, "body")
            .await
            .expect_err("a gate with no UI must not approve");
        assert_eq!(
            no_ui.to_string(),
            "MCP sampling requires interactive approval. Set settings.samplingAutoApprove to true to allow it without UI."
        );

        let declined = approval_with(Some(SamplingUi::answering(false)), false);
        let refused = confirm_sampling(&declined, SAMPLING_REQUEST_APPROVAL_TITLE, "body")
            .await
            .expect_err("a declined dialog must not approve");
        assert_eq!(refused.to_string(), "MCP sampling request was declined");

        assert_ne!(no_ui.to_string(), refused.to_string());
    }

    /// The one arm that returns `Ok`, and the two strings the human actually saw.
    #[tokio::test]
    async fn an_approved_dialog_is_the_only_ok_arm() {
        let ui = SamplingUi::answering(true);
        let approval = approval_with(Some(Arc::clone(&ui)), false);
        assert!(
            confirm_sampling(&approval, SAMPLING_REQUEST_APPROVAL_TITLE, "the body").await.is_ok()
        );
        assert_eq!(
            *ui.calls.lock().unwrap(),
            vec![("Approve MCP sampling request".to_string(), "the body".to_string())]
        );
    }

    /// `has_ui` true with no dialog handle is a wiring bug, and it fails **closed** — as the no-UI
    /// case, because a gate with nothing to ask through has no UI whatever a flag says.
    #[tokio::test]
    async fn a_ui_flag_without_a_dialog_handle_fails_closed() {
        let broken = SamplingApproval { auto_approve: false, has_ui: true, dialog: None };
        assert_eq!(
            confirm_sampling(&broken, SAMPLING_REQUEST_APPROVAL_TITLE, "body")
                .await
                .expect_err("no handle, no approval")
                .to_string(),
            SAMPLING_REQUIRES_INTERACTIVE_APPROVAL
        );
    }

    // ---- the formatters --------------------------------------------------------------------------

    fn user_text(text: &str) -> Message {
        Message::User {
            content: vec![Content::Text { text: text.to_string(), text_signature: None }],
            timestamp: 0,
        }
    }

    fn assistant_blocks(content: Vec<Content>) -> Message {
        let mut assistant = cyrup_core::AssistantMessage::errored(
            cyrup_core::ProviderId::from("mcp"),
            "sampling-request",
            None,
            cyrup_core::StopReason::Stop,
            "",
        );
        assistant.content = content;
        assistant.error_message = None;
        Message::Assistant(assistant)
    }

    /// `formatRequestApproval` in full: the header's pluralisation, the optional `System:` line,
    /// the 1-indexed rows, and the `"\n\n"` join.
    #[test]
    fn the_request_dialog_body_is_the_upstream_paragraph_layout() {
        let body = format_request_approval(
            "docs",
            "anthropic/claude-opus-4",
            Some("be terse"),
            &[user_text("hello"), assistant_blocks(vec![Content::Text {
                text: "hi".to_string(),
                text_signature: None,
            }])],
        );
        assert_eq!(
            body,
            "docs wants to sample 2 messages with anthropic/claude-opus-4.\n\n\
             System: be terse\n\n\
             1. user: hello\n\n\
             2. assistant: hi"
        );
    }

    /// The pluralisation is on **1**, not on "more than one" — so a zero-message request reads
    /// `0 messages`, and the `System:` line is skipped for an *empty* prompt exactly as for an
    /// absent one (`if (systemPrompt)` is a truthiness test).
    #[test]
    fn the_header_pluralises_on_one_and_an_empty_system_prompt_is_skipped() {
        assert_eq!(
            format_request_approval("s", "m", None, &[user_text("only")]),
            "s wants to sample 1 message with m.\n\n1. user: only"
        );
        assert_eq!(
            format_request_approval("s", "m", Some(""), &[]),
            "s wants to sample 0 messages with m."
        );
        assert!(
            format_request_approval("s", "m", Some("   "), &[]).contains("System:    "),
            "a blank-but-non-empty prompt is truthy in JS and is kept"
        );
    }

    /// Both truncations are `truncateAtWord(..., 400)`, which cuts at a word boundary and appends
    /// three ASCII periods.
    #[test]
    fn the_request_dialog_truncates_the_system_prompt_and_every_row_at_400() {
        let long = "word ".repeat(200);
        let body = format_request_approval("s", "m", Some(&long), &[user_text(&long)]);
        assert_eq!(body.matches("...").count(), 2);
        for line in body.split("\n\n").skip(1) {
            assert!(line.ends_with("..."), "{line:?}");
            assert!(line.encode_utf16().count() <= 400 + "1. user: ".len() + 3);
        }
    }

    /// `messageText`'s four reachable block spellings, joined with a single `"\n"` — and the fifth
    /// one, recorded because [`Content`] cannot produce it.
    #[test]
    fn message_text_renders_every_reachable_block_spelling() {
        let message = assistant_blocks(vec![
            Content::Text { text: "plain".to_string(), text_signature: None },
            Content::Image { data: "AAAA".to_string(), mime_type: "image/png".to_string() },
            Content::Thinking {
                thinking: "hidden".to_string(),
                thinking_signature: None,
                redacted: false,
            },
            Content::ToolCall(cyrup_core::ToolCall {
                id: cyrup_core::ToolCallId::from("call-1"),
                name: "search".to_string(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
            }),
        ]);
        let body = format_request_approval("s", "m", None, &[message]);
        assert_eq!(
            body,
            "s wants to sample 1 message with m.\n\n\
             1. assistant: plain\n[image: image/png]\n[thinking]\n[tool call: search]"
        );
        assert_eq!(MESSAGE_TEXT_UNKNOWN_BLOCK, "[content]");
    }

    /// The role strings are the wire spellings, including the one
    /// `convertSamplingMessage` cannot produce.
    #[test]
    fn the_rows_carry_the_wire_role_spellings() {
        let tool_result = Message::ToolResult {
            tool_call_id: cyrup_core::ToolCallId::from("call-1"),
            tool_name: "search".to_string(),
            content: vec![Content::Text { text: "rows".to_string(), text_signature: None }],
            is_error: false,
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            timestamp: 0,
        };
        let body = format_request_approval("s", "m", None, &[user_text("u"), tool_result]);
        assert!(body.contains("\n\n1. user: u"), "{body:?}");
        assert!(body.contains("\n\n2. toolResult: rows"), "{body:?}");
    }

    /// `formatResponseApproval` — the text arm, the non-text arm's `[<type> content]`, and the
    /// 1000-character budget.
    #[test]
    #[allow(deprecated)]
    fn the_response_dialog_body_is_the_upstream_sentence() {
        let text = CreateMessageResult::new(
            rmcp::model::SamplingMessage::assistant_text("the answer"),
            "anthropic/claude-opus-4".to_string(),
        );
        assert_eq!(
            format_response_approval("docs", &text),
            "docs will receive this response from anthropic/claude-opus-4:\n\nthe answer"
        );

        let tool_use = CreateMessageResult::new(
            rmcp::model::SamplingMessage::assistant_tool_use(
                "call-1",
                "search",
                serde_json::Map::new(),
            ),
            "m".to_string(),
        );
        assert_eq!(
            format_response_approval("docs", &tool_use),
            "docs will receive this response from m:\n\n[tool_use content]"
        );

        let long = CreateMessageResult::new(
            rmcp::model::SamplingMessage::assistant_text("word ".repeat(400)),
            "m".to_string(),
        );
        let rendered = format_response_approval("docs", &long);
        assert!(rendered.ends_with("..."));
        assert!(rendered.contains("will receive this response from m:\n\n"));
    }

}

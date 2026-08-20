//! The reconnect / idle-shutdown health-check state machine, and the two teardown orderings the
//! session handlers hang off — `lifecycle.ts` in full, plus `index.ts`'s `shutdownState`
//! (13a §4, §5, §15, §16; MCP-008, MCP-009, MCP-010, MCP-027, MCP-034, MCP-035).
//!
//! One 30-second loop with two **sequential** passes: converge every keep-alive server (reconnect
//! what died, refresh what drifted), then close anything that has been idle past its timeout. A
//! `keep-alive` server that dies is silently restarted within 30 seconds; an unused lazy server's
//! process is reaped after its idle timeout; the checks never overlap; a session abort stops them
//! immediately.
//!
//! # What the plan's §16 summary leaves out, and why this file is bigger than it
//!
//! 13a §16 was written against an earlier `lifecycle.ts` whose reconnect pass was a bare
//! `connect`-on-failure loop. The shipped upstream file (`lifecycle.ts:1-416`) is a convergence
//! engine, and four of its mechanisms are load-bearing enough that dropping them would be a visible
//! behaviour change rather than a simplification:
//!
//! 1. **Exponential retry backoff** (`retryStates`, `lifecycle.ts:336-375`). A server that cannot
//!    start is retried at 30 s, 60 s, 120 s … capped at 5 minutes, and the retry state is
//!    *invalidated* the moment the connection object or its status changes underneath it. Without
//!    it a permanently broken `keep-alive` server respawns a doomed child every 30 seconds for the
//!    life of the session.
//! 2. **Definition identity fencing** (`lifecycle.ts:169`, `:265`, `:279`, `:302`, `:322`, `:355`).
//!    Every arm that mutates state re-checks `keepAliveServers.get(name) === definition` — an
//!    **object identity** test, not a name test, because a same-name re-registration must reject
//!    the in-flight pass that the *previous* definition started. Ported as `Arc::ptr_eq` over
//!    `Arc<ServerEntry>`: [`McpLifecycleManager::register_server`] allocates one `Arc` per
//!    registration and [`McpLifecycleManager::mark_keep_alive`] shares *that* `Arc`, so the two
//!    maps hold the same pointer exactly as the two JS `Map`s hold the same object.
//! 3. **`refreshTools` as a liveness probe for HTTP servers** (`lifecycle.ts:213-268`). A connected
//!    server with a `url` is polled every cycle; a `superseded` result or a terminated-session
//!    error drives a reconnect rather than a failure. stdio servers are skipped (`if
//!    (!definition.url) return`) because a dead child is already visible as a missing connection.
//! 4. **`ensureConverged`** (`lifecycle.ts:121-132`), the *externally callable* single-flight
//!    convergence pass. `index.ts`'s `input` handler and `init.ts`'s `sendMessage` both await it so
//!    a turn does not start against a keep-alive server that died since the last tick. It shares
//!    one in-flight pass across all callers, and only the caller that *started* the pass clears the
//!    slot.
//!
//! # Three details that are not decoration
//!
//! **Single-flight.** The guard is `stopped || signal?.aborted || activeHealthCheck` — a check
//! already running suppresses the next tick outright rather than queueing it. In Rust this is still
//! required, because [`tokio::time::interval`] fires on schedule regardless of how long the body
//! took; `MissedTickBehavior::Delay` is the closest match to `setInterval`'s behaviour when a tick
//! is skipped by the guard.
//!
//! **`gracefulShutdown` waits for the in-flight check.** It memoises `shutdownOnce`, sets
//! `stopped`, clears the interval, **awaits `activeHealthCheck` and `activeConvergence`**, nulls
//! them and the five callbacks, clears the retry state, then calls `manager.closeAll()`. Dropping
//! that join is the classic way to leak an MCP child process on quit: a `closeAll` racing a
//! just-opened connection leaves an orphan. Here the join is
//! `health_task.stop.cancel()` (the `clearInterval` analogue — *synchronous*, so no further tick
//! can start) followed by `handle.await` (the `await activeHealthCheck` analogue). The in-flight
//! body is **not** aborted: it re-checks [`McpLifecycleManager::is_stopped`] at every step and
//! unwinds itself, which is exactly what upstream's `stopped` flag buys.
//!
//! **Idle timeout is per-server minutes × 60 000, and `0` disables the close.** `eager` and
//! `lazy-keep-alive` servers with no explicit `idleTimeout` are given `0` by
//! [`crate::config::ServerLifecycle::persists_after_first_spawn`] (MCP-020) — that is how "connect
//! it early and keep it" is expressed, rather than with a separate flag.
//!
//! # The manager seam
//!
//! `lifecycle.ts` drives `McpServerManager` through seven methods and nothing else. Those seven are
//! [`ConnectionSupervisor`], and [`ManagerSupervisor`] is the single adapter that binds them to the
//! real manager. The indirection exists for two reasons, in this order: the whole state machine is
//! then unit-testable against a scripted fake (which is what MCP-034's and MCP-035's `verify`
//! paragraphs demand — "a check taking 45 s", "a connect taking 200 ms"), and `server_manager.rs`
//! (13c, MCP-100…MCP-140) can land independently of this file. **`ManagerSupervisor`'s bodies are
//! the one place integration happens**; see its doc comment for the exact upstream call each is.

use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::CancelToken;
use futures::future::{BoxFuture, FutureExt, Shared};
use futures::StreamExt;
use indexmap::IndexMap;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::config::{ServerEntry, ServerLifecycle};
use crate::errors::{McpError, McpResult};
use crate::owner::McpRuntimeOwner;
use crate::state::{McpServerManager, McpState, McpStatusSnapshot, OAuthRuntime};

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Constants — `lifecycle.ts:14-16`, `:29`, `:92`
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The health-check cadence — `setInterval(..., 30_000).unref()` (`lifecycle.ts:92`, `:117`).
pub const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// `settings.idleTimeout`'s default, in **minutes** — `globalIdleTimeout = 10 * 60 * 1000`
/// (`lifecycle.ts:29`). Applied as `typeof settings.idleTimeout === "number" ? … : 10`, so an
/// explicit `0` is honoured and means "never idle out".
pub const DEFAULT_IDLE_TIMEOUT_MINUTES: f64 = 10.0;

/// `KEEP_ALIVE_RETRY_BASE_MS = 30_000` (`lifecycle.ts:14`) — the first retry delay after a
/// keep-alive server fails to come up.
pub const KEEP_ALIVE_RETRY_BASE: Duration = Duration::from_secs(30);

/// `KEEP_ALIVE_RETRY_MAX_MS = 5 * 60_000` (`lifecycle.ts:15`) — the backoff ceiling.
pub const KEEP_ALIVE_RETRY_MAX: Duration = Duration::from_secs(300);

/// `Math.min(attempts - 1, 10)` (`lifecycle.ts:358`) — the doubling count is capped before the
/// multiply, so the shift can never overflow regardless of how long a server has been failing.
const KEEP_ALIVE_RETRY_MAX_DOUBLINGS: u32 = 10;

/// `KEEP_ALIVE_CHECK_CONCURRENCY = 10` (`lifecycle.ts:16`) — how many keep-alive servers one
/// convergence pass probes at once.
pub const KEEP_ALIVE_CHECK_CONCURRENCY: usize = 10;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Session teardown reasons — `index.ts:98`, `:464`, `:467`, `:522`, `:528`
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `previousOwner?.stop("MCP extension session restarted")` (`index.ts:464`).
pub const SESSION_RESTART_STOP_REASON: &str = "MCP extension session restarted";

/// `owner?.stop("MCP extension session shutdown")` (`index.ts:522`).
pub const SESSION_SHUTDOWN_STOP_REASON: &str = "MCP extension session shutdown";

/// `shutdownState(previousState, "session_restart")` (`index.ts:467`).
pub const SESSION_RESTART_STATE_REASON: &str = "session_restart";

/// `shutdownState(currentState, "session_shutdown")` (`index.ts:528`).
pub const SESSION_SHUTDOWN_STATE_REASON: &str = "session_shutdown";

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The manager seam
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `ServerConnection["status"]` — `server-manager.ts`'s three-valued connection state.
///
/// `needs-auth` is not a failure: it means the server answered and demanded OAuth, so the
/// convergence pass hands it to the auth-required callback instead of recording a connect failure
/// (`lifecycle.ts:171-174`, `:190-193`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Handshake completed; the peer is usable.
    Connected,
    /// The transport is gone. Either it never came up or it was closed.
    Closed,
    /// The server requires an OAuth authorization the user has not completed.
    NeedsAuth,
}

/// The two properties `lifecycle.ts` reads off a `ServerConnection`, and nothing else.
///
/// Kept deliberately narrow: `server_manager.rs`'s real `ServerConnection` (13c §3.1) carries the
/// client, transport, inventory and accounting, none of which this state machine touches. The
/// **identity** of the handle is what most of the fencing turns on, so the trait is always held
/// behind an `Arc` and compared with [`Arc::ptr_eq`] — the port of upstream's `current !==
/// connection` object comparison.
pub trait ServerConnectionRef: Send + Sync + std::fmt::Debug {
    /// `connection.status`.
    fn status(&self) -> ConnectionStatus;

    /// `(connection.transport as {sessionId?: string})?.sessionId != null` (`lifecycle.ts:214`) —
    /// the `hadSessionId` gate `isTerminatedSession` needs. Structurally `false` for stdio.
    fn has_session_id(&self) -> bool;
}

/// One connection, by identity. `Arc::ptr_eq` on this is upstream's `===`.
pub type ConnectionHandle = Arc<dyn ServerConnectionRef>;

/// `ToolRefreshResult` (`server-manager.ts:147`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRefreshResult {
    /// The tool list changed and the metadata listener was notified.
    Updated,
    /// The server is alive and its tool list is byte-identical.
    Unchanged,
    /// The connection this refresh was issued against is no longer the manager's current one.
    Superseded,
}

/// The seven `McpServerManager` methods `lifecycle.ts` calls, plus the one `session-recovery.ts`
/// predicate it consults.
///
/// See the module doc's *"The manager seam"*. Implemented for real by [`ManagerSupervisor`]; a
/// scripted fake stands in for it in this module's tests.
pub trait ConnectionSupervisor: Send + Sync + 'static {
    /// `manager.getConnection(name)` (`server-manager.ts:1195`).
    fn get_connection(&self, name: &str) -> Option<ConnectionHandle>;

    /// `manager.connect(name, definition, signal)` (`server-manager.ts:258`).
    fn connect<'a>(
        &'a self,
        name: &'a str,
        definition: &'a ServerEntry,
        token: CancelToken,
    ) -> BoxFuture<'a, McpResult<ConnectionHandle>>;

    /// `manager.reconnect(name, definition, staleConnection, signal)` (`server-manager.ts:314`).
    fn reconnect<'a>(
        &'a self,
        name: &'a str,
        definition: &'a ServerEntry,
        stale: &'a ConnectionHandle,
        token: CancelToken,
    ) -> BoxFuture<'a, McpResult<ConnectionHandle>>;

    /// `manager.refreshTools(name, expectedConnection, signal)` (`server-manager.ts:344`).
    fn refresh_tools<'a>(
        &'a self,
        name: &'a str,
        connection: &'a ConnectionHandle,
        token: CancelToken,
    ) -> BoxFuture<'a, McpResult<ToolRefreshResult>>;

    /// `manager.close(name)` (`server-manager.ts:1101`).
    fn close<'a>(&'a self, name: &'a str) -> BoxFuture<'a, McpResult<()>>;

    /// `manager.closeAll()` (`server-manager.ts:1146`).
    fn close_all(&self) -> BoxFuture<'_, McpResult<()>>;

    /// `manager.isIdle(name, timeoutMs)` (`server-manager.ts:1224`): connected, zero in-flight, and
    /// `now - lastUsedAt > timeoutMs` — a **strict** comparison.
    fn is_idle(&self, name: &str, timeout: Duration) -> bool;

    /// `shouldReconnectAfterRefresh(error, hadSessionId)` (`lifecycle.ts:412-416`):
    /// `isTerminatedSession(error, hadSessionId) || (SdkError && code ∈ {NotConnected,
    /// ConnectionClosed})`. Lives on the supervisor because both halves are the manager's error
    /// vocabulary — MCP-134 owns the predicate itself.
    fn should_reconnect_after_refresh(&self, error: &McpError, had_session_id: bool) -> bool;
}

/// The adapter from [`ConnectionSupervisor`] onto the real [`McpServerManager`].
///
/// **This is the whole integration surface between this file and 13c.** Every body below is a
/// one-line delegation once `server_manager.rs` exists; until then each returns the closed-fail
/// answer for its operation — `None` / `false` / an error naming the missing unit — so a
/// misconfigured build is loud rather than quietly inert.
///
/// TODO(MCP-100, MCP-125, MCP-126, MCP-127, MCP-134): bind these to `McpServerManager` and add
/// `impl ServerConnectionRef for ServerConnection` here. Nothing else in this module changes.
#[derive(Debug)]
pub struct ManagerSupervisor {
    manager: Arc<McpServerManager>,
}

impl ManagerSupervisor {
    /// Wrap the manager this lifecycle drives.
    #[must_use]
    pub fn new(manager: Arc<McpServerManager>) -> Self {
        Self { manager }
    }

    /// The manager behind the adapter.
    #[must_use]
    pub fn manager(&self) -> &Arc<McpServerManager> {
        &self.manager
    }

    /// The error every unbound body returns. Named so a grep for `MCP-100` finds every call site
    /// that still needs binding.
    fn unbound(operation: &str, name: &str) -> McpError {
        McpError::Server {
            server: name.to_string(),
            message: format!(
                "MCP server manager is not wired yet: `{operation}` is pending MCP-100 \
                 (crates/cyrup-mcp/src/lifecycle.rs, ManagerSupervisor)"
            ),
        }
    }
}

impl ConnectionSupervisor for ManagerSupervisor {
    fn get_connection(&self, _name: &str) -> Option<ConnectionHandle> {
        // TODO(MCP-100): `self.manager.get_connection(name)`.
        None
    }

    fn connect<'a>(
        &'a self,
        name: &'a str,
        _definition: &'a ServerEntry,
        _token: CancelToken,
    ) -> BoxFuture<'a, McpResult<ConnectionHandle>> {
        // TODO(MCP-100): `self.manager.connect(name, definition, Some(&token)).await`.
        Box::pin(async move { Err(Self::unbound("connect", name)) })
    }

    fn reconnect<'a>(
        &'a self,
        name: &'a str,
        _definition: &'a ServerEntry,
        _stale: &'a ConnectionHandle,
        _token: CancelToken,
    ) -> BoxFuture<'a, McpResult<ConnectionHandle>> {
        // TODO(MCP-125): `self.manager.reconnect(name, definition, stale, Some(&token)).await`.
        Box::pin(async move { Err(Self::unbound("reconnect", name)) })
    }

    fn refresh_tools<'a>(
        &'a self,
        name: &'a str,
        _connection: &'a ConnectionHandle,
        _token: CancelToken,
    ) -> BoxFuture<'a, McpResult<ToolRefreshResult>> {
        // TODO(MCP-100): `self.manager.refresh_tools(name, connection, Some(&token)).await`.
        Box::pin(async move { Err(Self::unbound("refresh_tools", name)) })
    }

    fn close<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, McpResult<()>> {
        // TODO(MCP-126): `self.manager.close(name).await`.
        Box::pin(async move { Ok(()) })
    }

    fn close_all(&self) -> BoxFuture<'_, McpResult<()>> {
        // TODO(MCP-126): `self.manager.close_all().await`. Until this is bound,
        // `graceful_shutdown` joins the health check and then closes nothing — the one place in
        // this file where an unbound body is a *silent* no-op rather than a loud error, because
        // upstream's own `if (typeof this.manager.closeAll === "function")` guard makes a missing
        // `closeAll` legal (`lifecycle.ts:406-408`).
        Box::pin(async move { Ok(()) })
    }

    fn is_idle(&self, _name: &str, _timeout: Duration) -> bool {
        // TODO(MCP-127): `self.manager.is_idle(name, timeout)`.
        false
    }

    fn should_reconnect_after_refresh(&self, _error: &McpError, _had_session_id: bool) -> bool {
        // TODO(MCP-134): `session_recovery::is_terminated_session(error, had_session_id)
        //   || matches!(error, McpError::NotConnected | McpError::ConnectionClosed)`.
        false
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Callbacks — `lifecycle.ts:9-12`, `:35`; installed by `init.ts:418-452` (MCP-027)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `hasPendingAuth(serverName, undefined, oauthRuntime)` — the predicate the manager is constructed
/// with, so a server mid-OAuth is never reconnected underneath its own authorization flow
/// (`lifecycle.ts:46`, `:177`, `:229`).
pub type PendingAuthCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// `ReconnectCallback` (`lifecycle.ts:9`). Fallible on purpose: `publishConnectedMetadata` catches
/// a throwing callback and turns it into a `"publish"` connection failure (`lifecycle.ts:311-314`),
/// which is what keeps a broken metadata write from being mistaken for a healthy reconnect.
pub type ReconnectCallback =
    Arc<dyn Fn(String) -> BoxFuture<'static, McpResult<()>> + Send + Sync>;

/// `ReconnectFailureCallback` (`lifecycle.ts:10`) — synchronous and infallible upstream
/// (`recordFailure` + `updateStatusBar`).
pub type ReconnectFailureCallback = Arc<dyn Fn(&str, &McpError) + Send + Sync>;

/// `HealthRestoredCallback` (`lifecycle.ts:11`) — fired the first pass after a failing server comes
/// back, so the footer clears its error without user action.
pub type HealthRestoredCallback =
    Arc<dyn Fn(String) -> BoxFuture<'static, McpResult<()>> + Send + Sync>;

/// `AuthRequiredCallback` (`lifecycle.ts:12`). Its failures are logged and swallowed
/// (`lifecycle.ts:330-333`), never escalated.
pub type AuthRequiredCallback =
    Arc<dyn Fn(String) -> BoxFuture<'static, McpResult<()>> + Send + Sync>;

/// `onIdleShutdown` (`lifecycle.ts:35`) — synchronous; `init.ts:446-452` logs
/// `` `${serverName} shut down (idle ${idleMinutes}m)` `` and refreshes the footer.
pub type IdleShutdownCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// `flushMetadataCache(state)` (`init.ts`; MCP-031), as [`shutdown_state`] needs it.
///
/// Passed in rather than called directly because the metadata cache is 13c/MCP-139's module and
/// this one must not grow a dependency on it just to sequence a shutdown. It is **synchronous**
/// upstream and stays synchronous here — `shutdownState` runs it before its first `await`, and a
/// fire-and-forget write would race process exit (MCP-031).
pub type MetadataFlush = Arc<dyn Fn(&Arc<McpState>) -> McpResult<()> + Send + Sync>;

/// A [`MetadataFlush`] that flushes nothing, for call sites that genuinely have no cache — and,
/// until MCP-031 lands, for the session handlers. It **warns**, because reaching process exit
/// without writing `mcp-cache.json` means the next launch registers an empty tool surface.
#[must_use]
pub fn no_metadata_flush() -> MetadataFlush {
    Arc::new(|_state| {
        tracing::warn!(
            "MCP: metadata cache not flushed on shutdown — `flush_metadata_cache` is pending MCP-031"
        );
        Ok(())
    })
}

/// The five callbacks, nulled together by `shutdownOnce` (`lifecycle.ts:399-403`) so a late timer
/// cannot re-enter a dead generation.
#[derive(Default)]
struct Callbacks {
    on_reconnect: Option<ReconnectCallback>,
    on_reconnect_failure: Option<ReconnectFailureCallback>,
    on_health_restored: Option<HealthRestoredCallback>,
    on_auth_required: Option<AuthRequiredCallback>,
    on_idle_shutdown: Option<IdleShutdownCallback>,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Registration state
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `lifecycle.registerServer(name, definition, {idleTimeout})`'s optional third argument
/// (`lifecycle.ts:70`; `init.ts:247-251`).
#[derive(Debug, Clone, Copy, Default)]
pub struct LifecycleOverrides {
    /// Idle timeout in **minutes**, overriding both the per-server and the global value. `Some(0)`
    /// disables the idle close, which is what an `eager` / `lazy-keep-alive` server gets when it
    /// declares no timeout of its own (MCP-020).
    pub idle_timeout: Option<f64>,
}

/// `RetryState` (`lifecycle.ts:18-23`).
///
/// `nextAttemptAt` is `Date.now() + delay` upstream; here it is a [`tokio::time::Instant`], for two
/// reasons: a wall-clock adjustment must not be able to shorten or extend a backoff, and tokio's
/// clock is the one `tokio::time::pause` moves — which is what makes a 30-second backoff assertable
/// without a 30-second test.
#[derive(Debug, Clone)]
struct RetryState {
    attempts: u32,
    next_attempt_at: Instant,
    connection: Option<ConnectionHandle>,
    status: Option<ConnectionStatus>,
}

/// The three registration maps plus the two per-server sets, behind one lock.
///
/// Upstream these are five separate `Map`/`Set` fields. One `Mutex` here rather than five is not a
/// simplification of the data model — the maps are still separate and are still keyed and deleted
/// independently — it is a lock-ordering guarantee: no arm of this state machine can hold two of
/// them, and none is ever held across an `await`.
#[derive(Default)]
struct Registry {
    /// `allServers` (`lifecycle.ts:27`) — every enabled registered server, in registration order.
    all_servers: IndexMap<String, Arc<ServerEntry>>,
    /// `keepAliveServers` (`lifecycle.ts:26`) — the subset the convergence pass owns. Shares the
    /// **same `Arc`** as `all_servers`, which is what makes the identity fencing work.
    keep_alive_servers: IndexMap<String, Arc<ServerEntry>>,
    /// `serverSettings` (`lifecycle.ts:28`) — recorded only when an override was supplied.
    server_settings: IndexMap<String, LifecycleOverrides>,
    /// `retryStates` (`lifecycle.ts:39`).
    retry_states: IndexMap<String, RetryState>,
    /// `pendingMetadataPublications` (`lifecycle.ts:40`) — a server whose reconnect callback has
    /// not yet succeeded, so the next pass retries the publish instead of the connect.
    pending_metadata_publications: HashSet<String>,
}

/// The spawned interval task and the token that plays `clearInterval`.
struct HealthTask {
    handle: JoinHandle<()>,
    stop: CancelToken,
}

/// `checkKeepAliveConnections`' memoised in-flight pass. `Arc<McpError>` because a `Shared`
/// future's output must be `Clone` and [`McpError`] carries an [`std::io::Error`].
type ConvergenceFuture = Shared<BoxFuture<'static, Result<(), Arc<McpError>>>>;

/// Which verb `reportConnectionFailure` names in its message (`lifecycle.ts:350`, `:369-373`).
#[derive(Debug, Clone, Copy)]
enum FailureAction {
    Refresh,
    Reconnect,
    Publish,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// McpLifecycleManager
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `McpLifecycleManager` (`lifecycle.ts:25-410`) — the convergence/idle state machine and the
/// graceful shutdown that waits for it.
pub struct McpLifecycleManager {
    manager: Arc<McpServerManager>,
    supervisor: Arc<dyn ConnectionSupervisor>,
    has_pending_auth: PendingAuthCheck,
    registry: Arc<Mutex<Registry>>,
    callbacks: Arc<Mutex<Callbacks>>,
    /// `settings.idleTimeout` in minutes; stored as minutes and converted at read, matching
    /// `getIdleTimeout`'s `minutes * 60_000`.
    global_idle_minutes: Mutex<f64>,
    /// `stopped` (`lifecycle.ts:41`) — set by [`Self::graceful_shutdown`] before anything else, so
    /// a tick already scheduled does no work and an in-flight pass unwinds at its next guard.
    stopped: AtomicBool,
    /// `activeHealthCheck` (`lifecycle.ts:36`) as a *guard*. The interval body is awaited inline in
    /// the ticker task, so a second concurrent check is structurally impossible today; the flag is
    /// kept because it is the invariant the whole loop is written against, and because a future
    /// caller that spawns a check must not be able to double it.
    active_health_check: AtomicBool,
    /// `activeConvergence` (`lifecycle.ts:37`) — shared by every [`Self::ensure_converged`] caller.
    convergence: Arc<Mutex<Option<ConvergenceFuture>>>,
    /// The interval task and its `clearInterval` token.
    health_task: Mutex<Option<HealthTask>>,
    /// `shutdownPromise` (`lifecycle.ts:38`) — memoised, so N concurrent shutdowns join one
    /// teardown.
    shutdown: Mutex<Option<Shared<BoxFuture<'static, ()>>>>,
}

impl McpLifecycleManager {
    /// `new McpLifecycleManager(manager, serverName => hasPendingAuth(...))` — 13a §8 step 7.
    #[must_use]
    pub fn new(manager: Arc<McpServerManager>, has_pending_auth: PendingAuthCheck) -> Self {
        let supervisor = Arc::new(ManagerSupervisor::new(Arc::clone(&manager)));
        Self::with_supervisor(manager, has_pending_auth, supervisor)
    }

    /// As [`Self::new`], with the manager seam supplied explicitly.
    ///
    /// The production caller is [`Self::new`]; this constructor exists so the state machine can be
    /// driven against a scripted [`ConnectionSupervisor`] in tests, and so an integrator can bind
    /// `server_manager.rs` without editing [`ManagerSupervisor`].
    #[must_use]
    pub fn with_supervisor(
        manager: Arc<McpServerManager>,
        has_pending_auth: PendingAuthCheck,
        supervisor: Arc<dyn ConnectionSupervisor>,
    ) -> Self {
        Self {
            manager,
            supervisor,
            has_pending_auth,
            registry: Arc::new(Mutex::new(Registry::default())),
            callbacks: Arc::new(Mutex::new(Callbacks::default())),
            global_idle_minutes: Mutex::new(DEFAULT_IDLE_TIMEOUT_MINUTES),
            stopped: AtomicBool::new(false),
            active_health_check: AtomicBool::new(false),
            convergence: Arc::new(Mutex::new(None)),
            health_task: Mutex::new(None),
            shutdown: Mutex::new(None),
        }
    }

    // ── registration ────────────────────────────────────────────────────────────────────────

    /// `setGlobalIdleTimeout(minutes)` (`lifecycle.ts:84`). Stored in minutes; [`Self::idle_timeout`]
    /// does the × 60 000.
    pub fn set_global_idle_timeout(&self, minutes: f64) {
        if let Ok(mut slot) = self.global_idle_minutes.lock() {
            *slot = minutes;
        }
    }

    /// `registerServer(name, definition, settings?)` (`lifecycle.ts:70-74`; 13a §10 step 4).
    ///
    /// **Early-returns on a disabled definition**, exactly as upstream does — a disabled server is
    /// never in `allServers`, so the idle sweep never sees it and
    /// [`Self::mark_keep_alive_after_connect`] can never resurrect it.
    pub fn register_server(
        &self,
        name: &str,
        definition: ServerEntry,
        overrides: Option<LifecycleOverrides>,
    ) {
        if definition.is_disabled() {
            return;
        }
        let Ok(mut registry) = self.registry.lock() else {
            return;
        };
        registry
            .all_servers
            .insert(name.to_string(), Arc::new(definition));
        // `if (settings?.idleTimeout !== undefined) this.serverSettings.set(name, settings)` — an
        // override with no `idleTimeout` records nothing, so it cannot mask the global.
        match overrides {
            Some(overrides) if overrides.idle_timeout.is_some() => {
                registry.server_settings.insert(name.to_string(), overrides);
            }
            _ => {}
        }
    }

    /// `unregisterServer(name)` (`lifecycle.ts:76-82`) — called from `index.ts:338` when a server
    /// disappears from the live config. Drops the definition from **all five** maps; the identity
    /// fence then rejects any convergence pass still running against it.
    pub fn unregister_server(&self, name: &str) {
        let Ok(mut registry) = self.registry.lock() else {
            return;
        };
        registry.all_servers.shift_remove(name);
        registry.keep_alive_servers.shift_remove(name);
        registry.server_settings.shift_remove(name);
        registry.retry_states.shift_remove(name);
        registry.pending_metadata_publications.remove(name);
    }

    /// `markKeepAlive(name, definition)` (`lifecycle.ts:65-68`) — called at registration for
    /// `keep-alive` **only**; `lazy-keep-alive` is marked later, by
    /// [`Self::mark_keep_alive_after_connect`].
    ///
    /// Upstream takes the definition as an argument; here it is looked up from `allServers`, which
    /// is what makes both maps hold the *same* `Arc` and therefore what makes `Arc::ptr_eq` a
    /// faithful port of upstream's `===`.
    pub fn mark_keep_alive(&self, name: &str) {
        let Ok(mut registry) = self.registry.lock() else {
            return;
        };
        let Some(definition) = registry.all_servers.get(name).map(Arc::clone) else {
            return;
        };
        if definition.is_disabled() {
            return;
        }
        registry.keep_alive_servers.insert(name.to_string(), definition);
    }

    /// `markKeepAliveAfterConnect(state, serverName)` (`init.ts:463-469`) — the `lazy-keep-alive`
    /// path, called after that server's **first successful connect**.
    ///
    /// Three guards, all of them upstream's: the definition must exist, must not be disabled, and
    /// its mode must be exactly `lazy-keep-alive`. Dropping the third would promote every `lazy`
    /// server to keep-alive on first use and never let it idle out again.
    pub fn mark_keep_alive_after_connect(&self, name: &str) {
        let Ok(mut registry) = self.registry.lock() else {
            return;
        };
        let Some(definition) = registry.all_servers.get(name).map(Arc::clone) else {
            return;
        };
        if definition.is_disabled()
            || definition.lifecycle_mode() != ServerLifecycle::LazyKeepAlive
        {
            return;
        }
        registry.keep_alive_servers.insert(name.to_string(), definition);
    }

    /// `getIdleTimeout(name)` (`lifecycle.ts:377-381`) — the registered override, else the
    /// definition's own `idleTimeout`, else the global; in every case **minutes × 60 000**, and `0`
    /// means "no idle close".
    ///
    /// The middle term is not upstream's: upstream reaches only `serverSettings`, because
    /// `init.ts:246` always folds `definition.idleTimeout` into the override it passes. Keeping the
    /// fallback makes a direct `register_server(name, definition, None)` behave the way the
    /// definition asks instead of silently taking the global.
    #[must_use]
    pub fn idle_timeout(&self, name: &str) -> Option<Duration> {
        let registry = self.registry.lock().ok()?;
        let definition = registry.all_servers.get(name)?;
        let minutes = registry
            .server_settings
            .get(name)
            .and_then(|s| s.idle_timeout)
            .or(definition.idle_timeout)
            .unwrap_or_else(|| self.global_idle_minutes());
        Self::minutes_to_timeout(minutes)
    }

    /// `getEffectiveIdleTimeoutMinutes(state, serverName)` (`init.ts:664-672`) — used **only** for
    /// the idle-shutdown debug message, which is why it answers in minutes and why it re-derives
    /// the `eager`/`lazy-keep-alive` zero from the mode rather than reading the stored override.
    #[must_use]
    pub fn effective_idle_timeout_minutes(&self, name: &str) -> f64 {
        let global = self.global_idle_minutes();
        let Ok(registry) = self.registry.lock() else {
            return global;
        };
        let Some(definition) = registry.all_servers.get(name) else {
            return global;
        };
        if let Some(explicit) = definition.idle_timeout {
            return explicit;
        }
        if definition.lifecycle_mode().persists_after_first_spawn() {
            return 0.0;
        }
        global
    }

    fn global_idle_minutes(&self) -> f64 {
        self.global_idle_minutes
            .lock()
            .map_or(DEFAULT_IDLE_TIMEOUT_MINUTES, |g| *g)
    }

    /// `timeout > 0` (`lifecycle.ts:142`) with the non-finite cases folded in: a `NaN` or negative
    /// `idleTimeout` from a hand-edited `mcp.json` must disable the close, never panic
    /// [`Duration::from_secs_f64`].
    fn minutes_to_timeout(minutes: f64) -> Option<Duration> {
        if !minutes.is_finite() || minutes <= 0.0 {
            return None;
        }
        Some(Duration::from_secs_f64(minutes * 60.0))
    }

    // ── callbacks (MCP-027) ─────────────────────────────────────────────────────────────────

    /// `setReconnectCallback` (`lifecycle.ts:49`). `init.ts:418-425` installs the owner-guarded
    /// `updateServerMetadata` → `updateMetadataCache` → `notifyToolMetadataUpdated(…,
    /// "lifecycle-reconnect")` → `clearFailure` → `updateStatusBar` chain.
    pub fn set_reconnect_callback(&self, callback: ReconnectCallback) {
        if let Ok(mut slot) = self.callbacks.lock() {
            slot.on_reconnect = Some(callback);
        }
    }

    /// `setReconnectFailureCallback` (`lifecycle.ts:53`) — `recordFailure` + `updateStatusBar`.
    pub fn set_reconnect_failure_callback(&self, callback: ReconnectFailureCallback) {
        if let Ok(mut slot) = self.callbacks.lock() {
            slot.on_reconnect_failure = Some(callback);
        }
    }

    /// `setHealthRestoredCallback` (`lifecycle.ts:57`) — `clearFailure` + `updateStatusBar`, fired
    /// the first pass a previously failing server answers again.
    pub fn set_health_restored_callback(&self, callback: HealthRestoredCallback) {
        if let Ok(mut slot) = self.callbacks.lock() {
            slot.on_health_restored = Some(callback);
        }
    }

    /// `setAuthRequiredCallback` (`lifecycle.ts:61`) — `clearFailure` + `updateStatusBar`; a
    /// `needs-auth` server is *not* a failure.
    pub fn set_auth_required_callback(&self, callback: AuthRequiredCallback) {
        if let Ok(mut slot) = self.callbacks.lock() {
            slot.on_auth_required = Some(callback);
        }
    }

    /// `setIdleShutdownCallback` (`lifecycle.ts:88`) — logs
    /// `` `${serverName} shut down (idle ${idleMinutes}m)` `` with
    /// [`Self::effective_idle_timeout_minutes`], then refreshes the footer.
    pub fn set_idle_shutdown_callback(&self, callback: IdleShutdownCallback) {
        if let Ok(mut slot) = self.callbacks.lock() {
            slot.on_idle_shutdown = Some(callback);
        }
    }

    // ── accessors ───────────────────────────────────────────────────────────────────────────

    /// Whether a shutdown has begun. The single-flight guard's first term.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    /// The connection manager this lifecycle drives.
    #[must_use]
    pub fn manager(&self) -> &Arc<McpServerManager> {
        &self.manager
    }

    /// The manager seam actually in use — [`ManagerSupervisor`] unless one was injected.
    #[must_use]
    pub fn supervisor(&self) -> &Arc<dyn ConnectionSupervisor> {
        &self.supervisor
    }

    /// Whether `name` is waiting on an OAuth flow — such a server is never reconnected underneath
    /// its own authorization (`lifecycle.ts:177`, `:229`).
    #[must_use]
    pub fn has_pending_auth(&self, name: &str) -> bool {
        (self.has_pending_auth)(name)
    }

    /// Whether `name` is currently in the keep-alive set. Diagnostic; the state machine itself
    /// always uses the identity-checked `is_current_keep_alive`.
    #[must_use]
    pub fn is_keep_alive(&self, name: &str) -> bool {
        self.registry
            .lock()
            .is_ok_and(|registry| registry.keep_alive_servers.contains_key(name))
    }

    // ── the interval loop (MCP-034) ─────────────────────────────────────────────────────────

    /// `startHealthChecks(signal)` (`lifecycle.ts:92-119`), owned by `owner`.
    ///
    /// Reproduces, in order: `stopped = false`; the `signal?.aborted` early return that re-sets
    /// `stopped = true` **without** starting; the interval; and the single-flight guard. `unref()`
    /// needs no analogue — a tokio task is not a process-liveness reference (MCP-024).
    ///
    /// Two deltas, both named: the first tick of a tokio [`interval`](tokio::time::interval) fires
    /// immediately and is consumed here because `setInterval` does not fire at *t*=0; and a second
    /// `start` cancels the previous loop instead of orphaning it, where upstream overwrites
    /// `healthCheckInterval` and leaks the old timer.
    pub fn start(self: &Arc<Self>, owner: &Arc<McpRuntimeOwner>) {
        let token = owner.token();
        self.stopped.store(false, Ordering::Release);
        // `if (signal?.aborted) { this.stopped = true; return; }`
        if token.is_cancelled() {
            self.stopped.store(true, Ordering::Release);
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("MCP: no tokio runtime for the lifecycle health check");
            self.stopped.store(true, Ordering::Release);
            return;
        };

        let stop = CancelToken::new();
        let this = Arc::clone(self);
        let loop_stop = stop.clone();
        let join = handle.spawn(async move {
            let mut ticker = tokio::time::interval(HEALTH_CHECK_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                tokio::select! {
                    biased;
                    () = token.cancelled() => return,
                    () = loop_stop.cancelled() => return,
                    _ = ticker.tick() => {}
                }
                // `if (this.stopped || signal?.aborted || this.activeHealthCheck) return;` — the
                // tick is dropped, not queued.
                if this.is_stopped() || token.is_cancelled() {
                    return;
                }
                if this.active_health_check.swap(true, Ordering::AcqRel) {
                    continue;
                }
                let result = this.check_connections(&token).await;
                this.active_health_check.store(false, Ordering::Release);
                if let Err(error) = result {
                    // `.catch(error => console.error(...))` — a failing check never stops the loop.
                    tracing::error!("MCP: Health check failed: {error}");
                }
            }
        });

        if let Ok(mut slot) = self.health_task.lock()
            && let Some(previous) = slot.replace(HealthTask { handle: join, stop })
        {
            previous.stop.cancel();
        }
    }

    /// `checkConnections(signal)` (`lifecycle.ts:134-148`) — the two sequential passes.
    async fn check_connections(&self, token: &CancelToken) -> McpResult<()> {
        if self.is_stopped() || token.is_cancelled() {
            return Ok(());
        }
        self.check_keep_alive_connections(token).await?;
        if self.is_stopped() || token.is_cancelled() {
            return Ok(());
        }

        // Pass 2 — the idle sweep, over `allServers` minus `keepAliveServers`. The names are
        // snapshotted because the lock cannot be held across `close`; membership of the keep-alive
        // set is therefore re-checked per name, which is what upstream's live `Map` iteration gets
        // for free.
        let names: Vec<String> = {
            let Ok(registry) = self.registry.lock() else {
                return Ok(());
            };
            registry.all_servers.keys().cloned().collect()
        };
        for name in names {
            if self.is_keep_alive(&name) {
                continue;
            }
            let Some(timeout) = self.idle_timeout(&name) else {
                continue;
            };
            if !self.supervisor.is_idle(&name, timeout) {
                continue;
            }
            self.supervisor.close(&name).await?;
            if self.is_stopped() || token.is_cancelled() {
                return Ok(());
            }
            if let Some(callback) = self.callback(|c| c.on_idle_shutdown.clone()) {
                callback(&name);
            }
        }
        Ok(())
    }

    /// `ensureConverged(signal)` (`lifecycle.ts:121-132`) — the externally callable, single-flight
    /// convergence pass.
    ///
    /// `index.ts:504` awaits it on every `input` event and `init.ts:190` awaits it before a
    /// turn-triggering `sendMessage`, so a keep-alive server that died since the last tick is back
    /// before the model is asked to use it. Concurrent callers share one pass; only the caller that
    /// *started* the pass clears the slot afterwards, which is upstream's `finally` with its
    /// identity check (`lifecycle.ts:129-131`).
    pub fn ensure_converged(
        self: &Arc<Self>,
        token: CancelToken,
    ) -> BoxFuture<'static, Result<(), Arc<McpError>>> {
        if self.is_stopped() || token.is_cancelled() {
            return Box::pin(std::future::ready(Ok(())));
        }
        let Ok(mut slot) = self.convergence.lock() else {
            return Box::pin(std::future::ready(Ok(())));
        };
        if let Some(existing) = slot.as_ref() {
            // `if (this.activeConvergence) return this.activeConvergence;` — a joiner does not run
            // the `finally`.
            return Box::pin(existing.clone());
        }
        let runner = Arc::clone(self);
        let check: ConvergenceFuture = async move {
            runner
                .check_keep_alive_connections(&token)
                .await
                .map_err(Arc::new)
        }
        .boxed()
        .shared();
        *slot = Some(check.clone());
        drop(slot);

        let owner = Arc::clone(self);
        Box::pin(async move {
            let result = check.clone().await;
            if let Ok(mut slot) = owner.convergence.lock()
                && slot.as_ref().is_some_and(|held| Shared::ptr_eq(held, &check))
            {
                *slot = None;
            }
            result
        })
    }

    /// `checkKeepAliveConnections(signal)` (`lifecycle.ts:150-157`) — `parallelLimit` at
    /// [`KEEP_ALIVE_CHECK_CONCURRENCY`].
    ///
    /// **Named divergence.** `parallelLimit`'s workers stop pulling new items once one rejects,
    /// while `Promise.all` rejects immediately and leaves the siblings running detached. Here every
    /// server is probed to completion and the **first** failure is returned. The difference is only
    /// observable when a callback throws, and converging the remaining servers is strictly better
    /// than abandoning them.
    async fn check_keep_alive_connections(&self, token: &CancelToken) -> McpResult<()> {
        if self.is_stopped() || token.is_cancelled() {
            return Ok(());
        }
        let entries: Vec<(String, Arc<ServerEntry>)> = {
            let Ok(registry) = self.registry.lock() else {
                return Ok(());
            };
            registry
                .keep_alive_servers
                .iter()
                .map(|(name, definition)| (name.clone(), Arc::clone(definition)))
                .collect()
        };
        let first_error: Mutex<Option<McpError>> = Mutex::new(None);
        futures::stream::iter(entries)
            .for_each_concurrent(KEEP_ALIVE_CHECK_CONCURRENCY, |(name, definition)| {
                let first_error = &first_error;
                async move {
                    if let Err(error) = self
                        .check_keep_alive_connection(&name, definition, token, true)
                        .await
                        && let Ok(mut slot) = first_error.lock()
                        && slot.is_none()
                    {
                        *slot = Some(error);
                    }
                }
            })
            .await;
        match first_error.into_inner() {
            Ok(Some(error)) => Err(error),
            _ => Ok(()),
        }
    }

    /// `checkKeepAliveConnection(name, definition, signal, retrySuperseded)`
    /// (`lifecycle.ts:159-269`) — one server's convergence.
    ///
    /// Boxed because of the one-level `retrySuperseded` recursion at `lifecycle.ts:293`.
    fn check_keep_alive_connection<'a>(
        &'a self,
        name: &'a str,
        definition: Arc<ServerEntry>,
        token: &'a CancelToken,
        retry_superseded: bool,
    ) -> BoxFuture<'a, McpResult<()>> {
        Box::pin(async move {
            if definition.is_disabled() || self.is_stopped() || token.is_cancelled() {
                return Ok(());
            }
            // Fence against `unregisterServer` racing an in-flight pass. Identity also rejects a
            // stale pass after a same-name replacement registration, which a name check would
            // wrongly accept (`lifecycle.ts:166-169`).
            if !self.is_current_keep_alive(name, &definition) {
                return Ok(());
            }

            let current = self.supervisor.get_connection(name);
            if current
                .as_ref()
                .is_some_and(|c| c.status() == ConnectionStatus::NeedsAuth)
            {
                self.forget_pending_publication(name);
                return Ok(());
            }
            if !self.should_attempt_connection(name, current.as_ref()) {
                return Ok(());
            }

            let connected = current
                .filter(|c| c.status() == ConnectionStatus::Connected);
            let Some(connection) = connected else {
                // ── missing or not connected: a full connect (`lifecycle.ts:176-207`) ──
                if self.has_pending_auth(name) {
                    tracing::debug!(
                        "Skipping reconnect for {name} while OAuth authorization is pending"
                    );
                    return Ok(());
                }
                let fresh = match self
                    .supervisor
                    .connect(name, &definition, token.clone())
                    .await
                {
                    Ok(fresh) => fresh,
                    Err(error) => {
                        if self.is_stopped() || token.is_cancelled() {
                            return Ok(());
                        }
                        // The connection re-read is upstream's: `connect` may have replaced the
                        // map entry before failing, and the retry state must fence against
                        // whatever is there *now*.
                        let observed = self.supervisor.get_connection(name);
                        self.report_connection_failure(
                            name,
                            &definition,
                            &error,
                            FailureAction::Reconnect,
                            observed.as_ref(),
                        );
                        return Ok(());
                    }
                };
                if self.is_stopped() || token.is_cancelled() {
                    return Ok(());
                }
                match fresh.status() {
                    ConnectionStatus::NeedsAuth => {
                        return self.notify_auth_required(name, &definition, &fresh).await;
                    }
                    ConnectionStatus::Closed => {
                        let error = McpError::other(format!(
                            "MCP server {name} did not return a connected session"
                        ));
                        self.report_connection_failure(
                            name,
                            &definition,
                            &error,
                            FailureAction::Reconnect,
                            Some(&fresh),
                        );
                        return Ok(());
                    }
                    ConnectionStatus::Connected => {}
                }
                tracing::debug!("Reconnected to {name}");
                return self
                    .publish_connected_metadata(name, &definition, &fresh)
                    .await;
            };

            // ── connected: retry a failed publish, else probe (`lifecycle.ts:209-268`) ──
            if self.has_pending_publication(name) {
                return self
                    .publish_connected_metadata(name, &definition, &connection)
                    .await;
            }
            // stdio servers are not probed: a dead child is already a missing connection, and a
            // `tools/list` round trip per 30 s per server would be pure cost.
            if definition.url.is_none() {
                return Ok(());
            }

            let had_session_id = connection.has_session_id();
            let outcome = match self
                .supervisor
                .refresh_tools(name, &connection, token.clone())
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    if self.is_stopped() || token.is_cancelled() {
                        return Ok(());
                    }
                    let still_current = self
                        .supervisor
                        .get_connection(name)
                        .as_ref()
                        .is_some_and(|c| Arc::ptr_eq(c, &connection));
                    if !still_current || connection.status() != ConnectionStatus::Connected {
                        return self
                            .handle_superseded_connection(
                                name,
                                &definition,
                                &connection,
                                token,
                                retry_superseded,
                            )
                            .await;
                    }
                    if !self
                        .supervisor
                        .should_reconnect_after_refresh(&error, had_session_id)
                    {
                        self.report_connection_failure(
                            name,
                            &definition,
                            &error,
                            FailureAction::Refresh,
                            Some(&connection),
                        );
                        return Ok(());
                    }
                    if self.has_pending_auth(name) {
                        tracing::debug!(
                            "Skipping reconnect for {name} while OAuth authorization is pending"
                        );
                        return Ok(());
                    }
                    let fresh = match self
                        .supervisor
                        .reconnect(name, &definition, &connection, token.clone())
                        .await
                    {
                        Ok(fresh) => fresh,
                        Err(reconnect_error) => {
                            if self.is_stopped() || token.is_cancelled() {
                                return Ok(());
                            }
                            let observed = self.supervisor.get_connection(name);
                            self.report_connection_failure(
                                name,
                                &definition,
                                &reconnect_error,
                                FailureAction::Reconnect,
                                observed.as_ref(),
                            );
                            return Ok(());
                        }
                    };
                    if self.is_stopped() || token.is_cancelled() {
                        return Ok(());
                    }
                    match fresh.status() {
                        ConnectionStatus::NeedsAuth => {
                            return self.notify_auth_required(name, &definition, &fresh).await;
                        }
                        ConnectionStatus::Closed => {
                            let error = McpError::other(format!(
                                "MCP server {name} did not return a connected session"
                            ));
                            self.report_connection_failure(
                                name,
                                &definition,
                                &error,
                                FailureAction::Reconnect,
                                Some(&fresh),
                            );
                            return Ok(());
                        }
                        ConnectionStatus::Connected => {}
                    }
                    tracing::debug!("Reconnected stale MCP session for {name}");
                    return self
                        .publish_connected_metadata(name, &definition, &fresh)
                        .await;
                }
            };

            if outcome == ToolRefreshResult::Superseded {
                return self
                    .handle_superseded_connection(
                        name,
                        &definition,
                        &connection,
                        token,
                        retry_superseded,
                    )
                    .await;
            }
            if !self.is_current_keep_alive(name, &definition) {
                return Ok(());
            }
            // `if (this.retryStates.delete(name)) await this.onHealthRestored?.(name)` — the health
            // notification fires exactly once, on the pass that clears the retry state.
            if self.forget_retry_state(name) {
                self.fire_health_restored(name).await?;
            }
            Ok(())
        })
    }

    /// `handleSupersededConnection` (`lifecycle.ts:271-295`).
    fn handle_superseded_connection<'a>(
        &'a self,
        name: &'a str,
        definition: &'a Arc<ServerEntry>,
        stale: &'a ConnectionHandle,
        token: &'a CancelToken,
        retry_superseded: bool,
    ) -> BoxFuture<'a, McpResult<()>> {
        Box::pin(async move {
            let current = self.supervisor.get_connection(name);
            if !self.is_current_keep_alive(name, definition) {
                return Ok(());
            }
            let same = current.as_ref().is_some_and(|c| Arc::ptr_eq(c, stale));
            if same && stale.status() == ConnectionStatus::Connected {
                if self.forget_retry_state(name) {
                    self.fire_health_restored(name).await?;
                }
                return Ok(());
            }
            if let Some(current) = current.as_ref() {
                match current.status() {
                    ConnectionStatus::Connected => {
                        return self
                            .publish_connected_metadata(name, definition, current)
                            .await;
                    }
                    ConnectionStatus::NeedsAuth => {
                        return self.notify_auth_required(name, definition, current).await;
                    }
                    ConnectionStatus::Closed => {}
                }
            }
            if retry_superseded {
                // One retry, and only one: `retrySuperseded = false` on the recursive call is what
                // bounds it (`lifecycle.ts:293`).
                self.check_keep_alive_connection(name, Arc::clone(definition), token, false)
                    .await?;
            }
            Ok(())
        })
    }

    /// `publishConnectedMetadata` (`lifecycle.ts:297-315`).
    ///
    /// The fence arm closes *this pass's own* connection only while it is still the manager's
    /// current entry, so a replacement's connection is never torn down by a stale pass.
    async fn publish_connected_metadata(
        &self,
        name: &str,
        definition: &Arc<ServerEntry>,
        connection: &ConnectionHandle,
    ) -> McpResult<()> {
        if !self.is_current_keep_alive(name, definition) {
            if self
                .supervisor
                .get_connection(name)
                .as_ref()
                .is_some_and(|c| Arc::ptr_eq(c, connection))
            {
                self.supervisor.close(name).await?;
            }
            return Ok(());
        }
        self.add_pending_publication(name);
        let callback = self.callback(|c| c.on_reconnect.clone());
        let result = match callback {
            Some(callback) => callback(name.to_string()).await,
            None => Ok(()),
        };
        match result {
            Ok(()) => {
                self.forget_pending_publication(name);
                self.forget_retry_state(name);
            }
            Err(error) => {
                // `if (this.stopped) return;` — no signal check here upstream, deliberately: a
                // publish that failed during shutdown is not a server fault.
                if self.is_stopped() {
                    return Ok(());
                }
                self.report_connection_failure(
                    name,
                    definition,
                    &error,
                    FailureAction::Publish,
                    Some(connection),
                );
            }
        }
        Ok(())
    }

    /// `notifyAuthRequired` (`lifecycle.ts:317-334`). A `needs-auth` result clears the retry state:
    /// the server answered, so the backoff must not keep counting it as a failure.
    async fn notify_auth_required(
        &self,
        name: &str,
        definition: &Arc<ServerEntry>,
        connection: &ConnectionHandle,
    ) -> McpResult<()> {
        if !self.is_current_keep_alive(name, definition) {
            if self
                .supervisor
                .get_connection(name)
                .as_ref()
                .is_some_and(|c| Arc::ptr_eq(c, connection))
            {
                self.supervisor.close(name).await?;
            }
            return Ok(());
        }
        self.forget_pending_publication(name);
        self.forget_retry_state(name);
        if let Some(callback) = self.callback(|c| c.on_auth_required.clone())
            && let Err(error) = callback(name.to_string()).await
        {
            // Logged and swallowed — a broken footer must not stop an auth flow.
            // TODO(MCP-235): route `error` through `sanitize_terminal_text` once it lands.
            tracing::debug!("MCP: auth-required callback failed for {name}: {error}");
        }
        Ok(())
    }

    async fn fire_health_restored(&self, name: &str) -> McpResult<()> {
        match self.callback(|c| c.on_health_restored.clone()) {
            Some(callback) => callback(name.to_string()).await,
            None => Ok(()),
        }
    }

    /// `shouldAttemptConnection` (`lifecycle.ts:336-344`).
    ///
    /// The retry state is invalidated — not merely ignored — when either the connection object or
    /// its status has moved since the failure was recorded. That is what lets a server that came
    /// back on its own be probed immediately instead of waiting out a stale backoff.
    fn should_attempt_connection(&self, name: &str, connection: Option<&ConnectionHandle>) -> bool {
        let Ok(mut registry) = self.registry.lock() else {
            return true;
        };
        let Some(retry) = registry.retry_states.get(name) else {
            return true;
        };
        let same_connection = match (&retry.connection, connection) {
            (None, None) => true,
            (Some(recorded), Some(current)) => Arc::ptr_eq(recorded, current),
            _ => false,
        };
        let same_status = retry.status == connection.map(|c| c.status());
        if !same_connection || !same_status {
            registry.retry_states.shift_remove(name);
            return true;
        }
        Instant::now() >= retry.next_attempt_at
    }

    /// `reportConnectionFailure` (`lifecycle.ts:346-375`).
    ///
    /// `min(30_000 * 2 ** min(attempts - 1, 10), 300_000)`. The doubling count is capped *before*
    /// the multiply, so the shift is bounded by construction.
    fn report_connection_failure(
        &self,
        name: &str,
        definition: &Arc<ServerEntry>,
        error: &McpError,
        action: FailureAction,
        connection: Option<&ConnectionHandle>,
    ) {
        // Do not recreate retry/failure state from a stale pass after disposal or a same-name
        // replacement registration.
        if !self.is_current_keep_alive(name, definition) {
            return;
        }
        if let Ok(mut registry) = self.registry.lock() {
            let attempts = registry
                .retry_states
                .get(name)
                .map_or(0, |retry| retry.attempts)
                .saturating_add(1);
            let doublings = attempts.saturating_sub(1).min(KEEP_ALIVE_RETRY_MAX_DOUBLINGS);
            let delay = KEEP_ALIVE_RETRY_BASE
                .checked_mul(1u32 << doublings)
                .unwrap_or(KEEP_ALIVE_RETRY_MAX)
                .min(KEEP_ALIVE_RETRY_MAX);
            let now = Instant::now();
            registry.retry_states.insert(
                name.to_string(),
                RetryState {
                    attempts,
                    next_attempt_at: now.checked_add(delay).unwrap_or(now),
                    connection: connection.map(Arc::clone),
                    status: connection.map(|c| c.status()),
                },
            );
        }
        if let Some(callback) = self.callback(|c| c.on_reconnect_failure.clone()) {
            callback(name, error);
        }
        let target = match action {
            FailureAction::Reconnect => format!("reconnect to {name}"),
            FailureAction::Publish => format!("publish metadata for {name}"),
            FailureAction::Refresh => format!("refresh {name}"),
        };
        // TODO(MCP-235): route `error` through `sanitize_terminal_text` once it lands.
        tracing::error!("MCP: Failed to {target}: {error}");
    }

    // ── registry helpers ────────────────────────────────────────────────────────────────────

    /// `this.keepAliveServers.get(name) === definition` — the identity fence.
    fn is_current_keep_alive(&self, name: &str, definition: &Arc<ServerEntry>) -> bool {
        self.registry.lock().is_ok_and(|registry| {
            registry
                .keep_alive_servers
                .get(name)
                .is_some_and(|held| Arc::ptr_eq(held, definition))
        })
    }

    fn has_pending_publication(&self, name: &str) -> bool {
        self.registry
            .lock()
            .is_ok_and(|registry| registry.pending_metadata_publications.contains(name))
    }

    fn add_pending_publication(&self, name: &str) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.pending_metadata_publications.insert(name.to_string());
        }
    }

    fn forget_pending_publication(&self, name: &str) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.pending_metadata_publications.remove(name);
        }
    }

    /// `this.retryStates.delete(name)` — returns whether anything was there, which is the signal
    /// `onHealthRestored` fires on.
    fn forget_retry_state(&self, name: &str) -> bool {
        self.registry
            .lock()
            .is_ok_and(|mut registry| registry.retry_states.shift_remove(name).is_some())
    }

    fn callback<T>(&self, pick: impl FnOnce(&Callbacks) -> Option<T>) -> Option<T> {
        self.callbacks.lock().ok().and_then(|slot| pick(&slot))
    }

    // ── graceful shutdown (MCP-035) ─────────────────────────────────────────────────────────

    /// `gracefulShutdown()` (`lifecycle.ts:383-409`) — memoised. Sets `stopped`, stops the loop,
    /// **waits for the in-flight check and convergence**, drops the callbacks and the retry state,
    /// then closes every connection.
    ///
    /// The synchronous half runs at call time, exactly as `shutdownOnce`'s does before its first
    /// `await`: `stopped = true`, `clearInterval` (the task's stop token is cancelled) and the
    /// handles are taken out of their slots. Only the joins and `closeAll` are in the returned
    /// future. Calling this and *not* awaiting it therefore still stops the timer — which is what
    /// upstream's `clearInterval` guarantees and what a lazy `async fn` would have silently lost.
    ///
    /// The in-flight check is joined, **not aborted**: it re-reads [`Self::is_stopped`] at every
    /// step and unwinds itself. Aborting it mid-`connect` is precisely the orphaned-child bug this
    /// join exists to prevent.
    pub fn graceful_shutdown(&self) -> impl Future<Output = ()> + Send + 'static {
        self.stopped.store(true, Ordering::Release);
        let Ok(mut slot) = self.shutdown.lock() else {
            return async {}.boxed().shared();
        };
        if let Some(existing) = slot.as_ref() {
            return existing.clone();
        }

        let task = self.health_task.lock().ok().and_then(|mut s| s.take());
        if let Some(task) = task.as_ref() {
            // `clearInterval(this.healthCheckInterval)` — synchronous, so no further tick starts.
            task.stop.cancel();
        }
        let convergence = self.convergence.lock().ok().and_then(|mut s| s.take());
        let supervisor = Arc::clone(&self.supervisor);
        let callbacks = Arc::clone(&self.callbacks);
        let registry = Arc::clone(&self.registry);

        let future = async move {
            if let Some(task) = task {
                // `await this.activeHealthCheck`.
                let _ = task.handle.await;
            }
            if let Some(convergence) = convergence {
                // `await this.activeConvergence`.
                let _ = convergence.await;
            }
            // The five callbacks are nulled *after* the joins, so a check still unwinding can
            // still reach them, and *before* `closeAll`, so nothing re-enters a dead generation.
            if let Ok(mut slot) = callbacks.lock() {
                *slot = Callbacks::default();
            }
            if let Ok(mut registry) = registry.lock() {
                registry.retry_states.clear();
                registry.pending_metadata_publications.clear();
            }
            if let Err(error) = supervisor.close_all().await {
                tracing::error!("MCP: closing MCP servers during shutdown failed: {error}");
            }
        }
        .boxed()
        .shared();
        *slot = Some(future.clone());
        future
    }
}

impl std::fmt::Debug for McpLifecycleManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (all, keep_alive) = self
            .registry
            .lock()
            .map_or((0, 0), |r| (r.all_servers.len(), r.keep_alive_servers.len()));
        f.debug_struct("McpLifecycleManager")
            .field("stopped", &self.is_stopped())
            .field("servers", &all)
            .field("keep_alive", &keep_alive)
            .finish_non_exhaustive()
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// shutdownState and the generation teardown (MCP-008, MCP-009, MCP-010)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `shutdownState(currentState, reason)` (`index.ts:86-122`; 13a §5, MCP-010).
///
/// The invariant this function exists for: **a metadata-flush failure is never masked by a shutdown
/// failure.** Losing the tool cache silently is worse than a noisy shutdown, because the next launch
/// would register an empty tool surface.
///
/// Steps 1–4 run **synchronously, at call time** — publishing the shutdown snapshot and flushing the
/// cache are both synchronous upstream and both sit before `shutdownState`'s first `await`, so a JS
/// caller that builds the promise without awaiting it has already had them happen. Returning a
/// future from a plain `fn` reproduces that; an `async fn` would not.
///
/// Two deltas, both forced and both named:
///
/// * **The null-state arm publishes nothing.** Upstream sends the empty snapshot to `pi.events`, a
///   process-global bus. cyrup's snapshot lives on [`McpState::status_events`], so with no state
///   there is no channel to publish into and no subscriber that could be listening.
/// * **Step 3 (`uiServer.close(reason)`) is cut** — Cut 2, MCP Apps.
///
/// Step 5's `else` arm (`currentState.lifecycle.gracefulShutdown()` when there is no owner) is
/// structurally unreachable here: [`McpState::owner`] is non-optional. The graceful shutdown is
/// still reached, through the owner's LIFO cleanup stack — `initializeMcp` registers
/// `lifecycle.graceful_shutdown()` as a cleanup (`init.ts:207`; 13a §8 step 12).
pub fn shutdown_state(
    state: Option<Arc<McpState>>,
    reason: &'static str,
    flush: MetadataFlush,
) -> impl Future<Output = McpResult<()>> + Send + 'static {
    let Some(state) = state else {
        tracing::debug!("MCP: shutdown ({reason}) with no live state");
        return futures::future::Either::Left(std::future::ready(Ok(())));
    };

    // Step 2 — the empty snapshot, before anything can fail.
    state.publish_status(McpStatusSnapshot::default());

    // Step 4 — captured, *not* rethrown yet.
    let flush_error = flush(&state).err();
    if let Some(error) = flush_error.as_ref() {
        tracing::debug!("MCP: metadata flush failed during shutdown ({reason}): {error}");
    }

    // `begin_stop`, not `stop`: the cancel must be observable *now*, and it also hands back a
    // fully-owned `Shared` future rather than an `impl Future` that (under Rust 2024's capture
    // rules) would borrow `state` for `'static`.
    let stop = state.owner.begin_stop(Some(reason));
    futures::future::Either::Right(async move {
        // `currentState` is live for the whole of `shutdownState` upstream. Carrying the `Arc` into
        // the future keeps that true here, so a cleanup reaching back through the state cannot
        // observe it freed mid-drain.
        let _state = state;
        // Step 5.
        if let Err(error) = stop.await {
            match flush_error {
                Some(flush_error) => {
                    tracing::error!(
                        "MCP: graceful shutdown failed after metadata flush error: {error}"
                    );
                    return Err(flush_error);
                }
                None => return Err(unwrap_stop_error(error)),
            }
        }
        // Step 6.
        match flush_error {
            Some(flush_error) => Err(flush_error),
            None => Ok(()),
        }
    })
}

/// Recover a stop failure from behind the memoised `Shared`'s `Arc`.
///
/// `McpRuntimeOwner::stop` must hand back a `Clone` output, so it yields `Arc<McpError>`. When this
/// caller holds the only reference the error moves out intact; when the memo still holds one the
/// message is preserved and the class is not. That loss is why the message text of
/// `RuntimeCleanupFailed` is exact (see [`McpError::RuntimeCleanupFailed`]).
fn unwrap_stop_error(error: Arc<McpError>) -> McpError {
    match Arc::try_unwrap(error) {
        Ok(error) => error,
        Err(shared) => McpError::other(shared.to_string()),
    }
}

/// The previous generation's three teardown handles, as `session_start` and `session_shutdown`
/// snapshot them (`index.ts:451-453`, `:514-516`).
#[derive(Default)]
pub struct PreviousGeneration {
    /// `previousState` / `currentState`.
    pub state: Option<Arc<McpState>>,
    /// `previousOwner` / `owner`.
    pub owner: Option<Arc<McpRuntimeOwner>>,
    /// `previousOAuthRuntime` / `oauthRuntime`.
    pub oauth: Option<Arc<OAuthRuntime>>,
}

/// The teardown half of both session handlers — `index.ts:462-472` (MCP-008) and `:520-532`
/// (MCP-009).
///
/// **The cancel is synchronous.** `previousOwner?.stop(reason)` is evaluated *before* the
/// `Promise.all`, and upstream says why in a comment at `index.ts:462-463`: *"Abort synchronously
/// before awaiting cleanup so old callbacks and startup work cannot resume into a stale
/// ExtensionContext."* This is a plain `fn`, not an `async fn`, for exactly that reason:
/// [`McpRuntimeOwner::begin_stop`] cancels the token and returns the drain, and it runs at **call**
/// time, not at first poll. Collapsing this into an `async fn` — or into a single `stop().await` —
/// would make the cancel observable only once someone awaits, which is the whole bug the ordering
/// prevents.
///
/// The three futures are then joined, and a failure is **logged, never propagated**: both handlers
/// wrap the join in `try/catch` and continue, because a session must start (or end) even if the
/// previous one could not be torn down cleanly.
///
/// `Promise.all` rejects on the first *settling* rejection while the siblings keep running detached;
/// [`futures::future::join3`] runs all three to completion and this reports the first failure in
/// argument order. Nothing observable turns on which of three logged messages wins.
pub fn shutdown_previous_generation(
    previous: PreviousGeneration,
    stop_reason: &'static str,
    state_reason: &'static str,
    failure_log: &'static str,
    flush: MetadataFlush,
) -> impl Future<Output = ()> + Send + 'static {
    // 1 — the synchronous abort.
    let stop_previous = previous.owner.map(|owner| owner.begin_stop(Some(stop_reason)));
    // 2 — `shutdownState`'s own synchronous prefix (snapshot + flush) also happens now.
    let shutdown = shutdown_state(previous.state, state_reason, flush);
    // 3 — the OAuth runtime's teardown.
    let oauth = shutdown_oauth(previous.oauth);

    async move {
        let stop = async move {
            match stop_previous {
                Some(stop) => stop.await.map_err(unwrap_stop_error),
                None => Ok(()),
            }
        };
        let (stop, shutdown, oauth) = futures::future::join3(stop, shutdown, oauth).await;
        for outcome in [stop, shutdown, oauth] {
            if let Err(error) = outcome {
                tracing::error!("{failure_log}: {error}");
                return;
            }
        }
    }
}

/// `shutdownOAuth(runtime)` (`mcp-auth-flow.ts`; 13g OA-5) — the flow registry's teardown.
///
/// TODO(MCP-280…MCP-330): call the real `shutdown_oauth` once `OAuthRuntime` is more than
/// `state.rs`'s forward declaration. Until then the `None` arm is exact and the `Some` arm is the
/// one hole: an OAuth listener registered by a previous generation is not torn down.
async fn shutdown_oauth(runtime: Option<Arc<OAuthRuntime>>) -> McpResult<()> {
    if runtime.is_some() {
        tracing::debug!("MCP: OAuth runtime shutdown is pending 13g (MCP-280…MCP-330)");
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::config::McpConfig;
    use crate::state::{AuthStorageOptions, McpStateParts};

    /// A `ServerConnection` reduced to the two properties `lifecycle.ts` reads.
    #[derive(Debug)]
    struct FakeConnection {
        status: ConnectionStatus,
        session_id: bool,
    }

    impl FakeConnection {
        fn handle(status: ConnectionStatus, session_id: bool) -> ConnectionHandle {
            Arc::new(Self { status, session_id })
        }
    }

    impl ServerConnectionRef for FakeConnection {
        fn status(&self) -> ConnectionStatus {
            self.status
        }
        fn has_session_id(&self) -> bool {
            self.session_id
        }
    }

    /// The scripted manager the whole state machine is driven against.
    #[derive(Default)]
    struct FakeSupervisor {
        connections: Mutex<IndexMap<String, ConnectionHandle>>,
        idle: Mutex<HashSet<String>>,
        /// Every observable side effect, in order — the assertion surface for the ordering tests.
        log: Mutex<Vec<String>>,
        connect_fails: AtomicBool,
        /// How long `connect` takes, in virtual time.
        connect_delay: Mutex<Duration>,
        close_all_calls: AtomicUsize,
    }

    impl FakeSupervisor {
        fn arc() -> Arc<Self> {
            Arc::new(Self::default())
        }
        fn record(&self, entry: impl Into<String>) {
            if let Ok(mut log) = self.log.lock() {
                log.push(entry.into());
            }
        }
        fn log(&self) -> Vec<String> {
            self.log.lock().map(|l| l.clone()).unwrap_or_default()
        }
        fn set_connection(&self, name: &str, connection: ConnectionHandle) {
            if let Ok(mut connections) = self.connections.lock() {
                connections.insert(name.to_string(), connection);
            }
        }
        fn mark_idle(&self, name: &str) {
            if let Ok(mut idle) = self.idle.lock() {
                idle.insert(name.to_string());
            }
        }
    }

    impl ConnectionSupervisor for FakeSupervisor {
        fn get_connection(&self, name: &str) -> Option<ConnectionHandle> {
            self.connections
                .lock()
                .ok()
                .and_then(|c| c.get(name).map(Arc::clone))
        }

        fn connect<'a>(
            &'a self,
            name: &'a str,
            _definition: &'a ServerEntry,
            _token: CancelToken,
        ) -> BoxFuture<'a, McpResult<ConnectionHandle>> {
            Box::pin(async move {
                self.record(format!("connect:{name}"));
                let delay = self.connect_delay.lock().map_or(Duration::ZERO, |d| *d);
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                self.record(format!("connected:{name}"));
                if self.connect_fails.load(Ordering::Acquire) {
                    return Err(McpError::other(format!("{name} refused")));
                }
                let handle = FakeConnection::handle(ConnectionStatus::Connected, false);
                self.set_connection(name, Arc::clone(&handle));
                Ok(handle)
            })
        }

        fn reconnect<'a>(
            &'a self,
            name: &'a str,
            definition: &'a ServerEntry,
            _stale: &'a ConnectionHandle,
            token: CancelToken,
        ) -> BoxFuture<'a, McpResult<ConnectionHandle>> {
            self.record(format!("reconnect:{name}"));
            self.connect(name, definition, token)
        }

        fn refresh_tools<'a>(
            &'a self,
            name: &'a str,
            _connection: &'a ConnectionHandle,
            _token: CancelToken,
        ) -> BoxFuture<'a, McpResult<ToolRefreshResult>> {
            Box::pin(async move {
                self.record(format!("refresh:{name}"));
                Ok(ToolRefreshResult::Unchanged)
            })
        }

        fn close<'a>(&'a self, name: &'a str) -> BoxFuture<'a, McpResult<()>> {
            Box::pin(async move {
                self.record(format!("close:{name}"));
                if let Ok(mut connections) = self.connections.lock() {
                    connections.shift_remove(name);
                }
                Ok(())
            })
        }

        fn close_all(&self) -> BoxFuture<'_, McpResult<()>> {
            Box::pin(async move {
                self.close_all_calls.fetch_add(1, Ordering::AcqRel);
                self.record("close_all");
                Ok(())
            })
        }

        fn is_idle(&self, name: &str, _timeout: Duration) -> bool {
            self.record(format!("is_idle:{name}"));
            self.idle.lock().is_ok_and(|idle| idle.contains(name))
        }

        fn should_reconnect_after_refresh(&self, _error: &McpError, _had: bool) -> bool {
            true
        }
    }

    /// Let every ready task run. `tokio::time::advance` moves the clock but does not guarantee the
    /// spawned health-check task has been polled, and the whole loop lives in that task.
    async fn settle() {
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }

    fn lifecycle_with(supervisor: Arc<FakeSupervisor>) -> Arc<McpLifecycleManager> {
        Arc::new(McpLifecycleManager::with_supervisor(
            Arc::new(McpServerManager::default()),
            Arc::new(|_| false),
            supervisor,
        ))
    }

    fn lifecycle() -> Arc<McpLifecycleManager> {
        lifecycle_with(FakeSupervisor::arc())
    }

    fn keep_alive_entry() -> ServerEntry {
        ServerEntry { lifecycle: Some(ServerLifecycle::KeepAlive), ..Default::default() }
    }

    // ── registration and idle accounting ────────────────────────────────────────────────────

    #[test]
    fn idle_timeout_prefers_override_then_entry_then_global() {
        let lc = lifecycle();
        lc.set_global_idle_timeout(10.0);

        lc.register_server("global", ServerEntry::default(), None);
        assert_eq!(lc.idle_timeout("global"), Some(Duration::from_secs(600)));

        let entry = ServerEntry { idle_timeout: Some(2.0), ..Default::default() };
        lc.register_server("entry", entry.clone(), None);
        assert_eq!(lc.idle_timeout("entry"), Some(Duration::from_secs(120)));

        lc.register_server("over", entry, Some(LifecycleOverrides { idle_timeout: Some(1.0) }));
        assert_eq!(lc.idle_timeout("over"), Some(Duration::from_secs(60)));
    }

    #[test]
    fn zero_disables_the_idle_close() {
        let lc = lifecycle();
        lc.register_server(
            "eager",
            ServerEntry::default(),
            Some(LifecycleOverrides { idle_timeout: Some(0.0) }),
        );
        assert_eq!(lc.idle_timeout("eager"), None, "0 means never idle out, not 'immediately'");
        assert_eq!(lc.idle_timeout("unregistered"), None);
    }

    #[test]
    fn a_non_finite_idle_timeout_disables_the_close_rather_than_panicking() {
        let lc = lifecycle();
        lc.register_server(
            "nan",
            ServerEntry { idle_timeout: Some(f64::NAN), ..Default::default() },
            None,
        );
        lc.register_server(
            "neg",
            ServerEntry { idle_timeout: Some(-5.0), ..Default::default() },
            None,
        );
        assert_eq!(lc.idle_timeout("nan"), None);
        assert_eq!(lc.idle_timeout("neg"), None);
    }

    #[test]
    fn register_server_and_mark_keep_alive_skip_a_disabled_definition() {
        let lc = lifecycle();
        lc.register_server(
            "off",
            ServerEntry { disabled: Some(true), ..Default::default() },
            None,
        );
        lc.mark_keep_alive("off");
        assert_eq!(lc.idle_timeout("off"), None, "a disabled server is never registered at all");
        assert!(!lc.is_keep_alive("off"));
    }

    #[test]
    fn keep_alive_after_connect_requires_the_lazy_keep_alive_mode() {
        let lc = lifecycle();
        lc.register_server("plain-lazy", ServerEntry::default(), None);
        lc.register_server(
            "lazy-keep",
            ServerEntry { lifecycle: Some(ServerLifecycle::LazyKeepAlive), ..Default::default() },
            None,
        );
        lc.mark_keep_alive_after_connect("plain-lazy");
        lc.mark_keep_alive_after_connect("lazy-keep");
        assert!(
            !lc.is_keep_alive("plain-lazy"),
            "`markKeepAliveAfterConnect` promotes only `lazy-keep-alive` (init.ts:463-469)"
        );
        assert!(lc.is_keep_alive("lazy-keep"));
    }

    #[test]
    fn effective_idle_minutes_reproduces_the_debug_messages_ladder() {
        let lc = lifecycle();
        lc.set_global_idle_timeout(7.0);
        lc.register_server("plain", ServerEntry::default(), None);
        lc.register_server(
            "explicit",
            ServerEntry { idle_timeout: Some(3.0), ..Default::default() },
            None,
        );
        lc.register_server(
            "eager",
            ServerEntry { lifecycle: Some(ServerLifecycle::Eager), ..Default::default() },
            None,
        );
        assert_eq!(lc.effective_idle_timeout_minutes("plain"), 7.0);
        assert_eq!(lc.effective_idle_timeout_minutes("explicit"), 3.0);
        assert_eq!(lc.effective_idle_timeout_minutes("eager"), 0.0);
        assert_eq!(lc.effective_idle_timeout_minutes("missing"), 7.0);
    }

    #[test]
    fn unregister_clears_every_map() {
        let lc = lifecycle();
        lc.register_server("a", keep_alive_entry(), Some(LifecycleOverrides { idle_timeout: Some(4.0) }));
        lc.mark_keep_alive("a");
        assert!(lc.is_keep_alive("a"));
        lc.unregister_server("a");
        assert!(!lc.is_keep_alive("a"));
        assert_eq!(lc.idle_timeout("a"), None);
    }

    // ── the convergence pass (MCP-034) ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_dead_keep_alive_server_is_reconnected_and_publishes_metadata() {
        let supervisor = FakeSupervisor::arc();
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");

        let published = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&published);
        lc.set_reconnect_callback(Arc::new(move |name| {
            let sink = Arc::clone(&sink);
            async move {
                if let Ok(mut sink) = sink.lock() {
                    sink.push(name);
                }
                Ok(())
            }
            .boxed()
        }));

        lc.check_connections(&CancelToken::new()).await.unwrap();
        assert_eq!(supervisor.log(), vec!["connect:a", "connected:a"]);
        assert_eq!(published.lock().unwrap().clone(), vec!["a".to_string()]);
        assert!(!lc.has_pending_publication("a"), "a successful publish clears the retry marker");
    }

    #[tokio::test]
    async fn a_connected_stdio_server_is_never_probed() {
        let supervisor = FakeSupervisor::arc();
        supervisor.set_connection("a", FakeConnection::handle(ConnectionStatus::Connected, false));
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");

        lc.check_connections(&CancelToken::new()).await.unwrap();
        assert!(
            !supervisor.log().iter().any(|e| e.starts_with("refresh:")),
            "`if (!definition.url) return` — only HTTP servers get a refresh probe"
        );
        assert!(!supervisor.log().iter().any(|e| e.starts_with("connect:")));
    }

    #[tokio::test]
    async fn a_connected_http_server_is_probed_every_pass() {
        let supervisor = FakeSupervisor::arc();
        supervisor.set_connection("a", FakeConnection::handle(ConnectionStatus::Connected, true));
        let lc = lifecycle_with(Arc::clone(&supervisor));
        let entry = ServerEntry {
            url: Some("https://example.test/mcp".to_string()),
            lifecycle: Some(ServerLifecycle::KeepAlive),
            ..Default::default()
        };
        lc.register_server("a", entry, None);
        lc.mark_keep_alive("a");

        lc.check_connections(&CancelToken::new()).await.unwrap();
        assert_eq!(supervisor.log(), vec!["refresh:a"], "keep-alive servers skip the idle sweep");
    }

    #[tokio::test]
    async fn a_needs_auth_connection_is_never_treated_as_a_failure() {
        let supervisor = FakeSupervisor::arc();
        supervisor.set_connection("a", FakeConnection::handle(ConnectionStatus::NeedsAuth, false));
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");

        let failures = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&failures);
        lc.set_reconnect_failure_callback(Arc::new(move |_, _| {
            counter.fetch_add(1, Ordering::AcqRel);
        }));

        lc.check_connections(&CancelToken::new()).await.unwrap();
        assert!(
            !supervisor.log().iter().any(|e| e.starts_with("connect:")),
            "no connect is attempted"
        );
        assert_eq!(failures.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn a_pending_oauth_flow_suppresses_the_reconnect() {
        let supervisor = FakeSupervisor::arc();
        let lc = Arc::new(McpLifecycleManager::with_supervisor(
            Arc::new(McpServerManager::default()),
            Arc::new(|_| true),
            Arc::clone(&supervisor) as Arc<dyn ConnectionSupervisor>,
        ));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");

        lc.check_connections(&CancelToken::new()).await.unwrap();
        assert!(!supervisor.log().iter().any(|e| e.starts_with("connect:")));
    }

    #[tokio::test]
    async fn a_failed_server_is_not_retried_before_its_backoff_expires() {
        let supervisor = FakeSupervisor::arc();
        supervisor.connect_fails.store(true, Ordering::Release);
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");

        let failures = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&failures);
        lc.set_reconnect_failure_callback(Arc::new(move |_, _| {
            counter.fetch_add(1, Ordering::AcqRel);
        }));

        let token = CancelToken::new();
        lc.check_connections(&token).await.unwrap();
        lc.check_connections(&token).await.unwrap();
        lc.check_connections(&token).await.unwrap();

        let connects = supervisor.log().iter().filter(|e| *e == "connect:a").count();
        assert_eq!(connects, 1, "the 30 s backoff suppresses passes two and three entirely");
        assert_eq!(failures.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn a_changed_connection_invalidates_the_backoff_immediately() {
        let supervisor = FakeSupervisor::arc();
        supervisor.connect_fails.store(true, Ordering::Release);
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");

        let token = CancelToken::new();
        lc.check_connections(&token).await.unwrap();
        // Something else replaced the connection: `retry.connection !== connection` must clear the
        // retry state instead of making the server wait out a backoff it no longer describes.
        supervisor.set_connection("a", FakeConnection::handle(ConnectionStatus::Closed, false));
        lc.check_connections(&token).await.unwrap();

        let connects = supervisor.log().iter().filter(|e| *e == "connect:a").count();
        assert_eq!(connects, 2, "a changed connection clears the retry state on sight");
    }

    #[tokio::test(start_paused = true)]
    async fn health_restored_fires_once_when_a_failing_server_answers_again() {
        let supervisor = FakeSupervisor::arc();
        supervisor.set_connection("a", FakeConnection::handle(ConnectionStatus::Connected, true));
        let lc = lifecycle_with(Arc::clone(&supervisor));
        let entry = ServerEntry {
            url: Some("https://example.test/mcp".to_string()),
            lifecycle: Some(ServerLifecycle::KeepAlive),
            ..Default::default()
        };
        lc.register_server("a", entry, None);
        lc.mark_keep_alive("a");

        let restored = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&restored);
        lc.set_health_restored_callback(Arc::new(move |_| {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
            .boxed()
        }));

        // Seed a retry state the way a refresh failure would — against the *current* connection,
        // so `shouldAttemptConnection`'s identity check does not simply discard it.
        let definition = lc
            .registry
            .lock()
            .unwrap()
            .keep_alive_servers
            .get("a")
            .map(Arc::clone)
            .unwrap();
        let connection = supervisor.get_connection("a").unwrap();
        lc.report_connection_failure(
            "a",
            &definition,
            &McpError::other("boom"),
            FailureAction::Refresh,
            Some(&connection),
        );

        let token = CancelToken::new();
        lc.check_connections(&token).await.unwrap();
        assert_eq!(
            restored.load(Ordering::Acquire),
            0,
            "inside the 30 s backoff the server is not even probed"
        );

        tokio::time::advance(KEEP_ALIVE_RETRY_BASE + Duration::from_secs(1)).await;
        lc.check_connections(&token).await.unwrap();
        assert_eq!(restored.load(Ordering::Acquire), 1, "the backoff expired and the probe passed");

        lc.check_connections(&token).await.unwrap();
        assert_eq!(restored.load(Ordering::Acquire), 1, "the notification is edge-triggered");
    }

    #[tokio::test]
    async fn the_identity_fence_rejects_a_pass_started_by_a_replaced_definition() {
        let supervisor = FakeSupervisor::arc();
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");
        let stale = lc
            .registry
            .lock()
            .unwrap()
            .keep_alive_servers
            .get("a")
            .map(Arc::clone)
            .unwrap();

        // A same-name re-registration allocates a *new* `Arc`; the old pass must be rejected.
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");

        lc.check_keep_alive_connection("a", stale, &CancelToken::new(), true)
            .await
            .unwrap();
        assert!(supervisor.log().is_empty(), "a stale pass touches nothing at all");
    }

    // ── the idle sweep (MCP-034, pass 2) ────────────────────────────────────────────────────

    #[tokio::test]
    async fn the_idle_sweep_closes_only_non_keep_alive_servers_past_their_timeout() {
        let supervisor = FakeSupervisor::arc();
        supervisor.set_connection("lazy", FakeConnection::handle(ConnectionStatus::Connected, false));
        supervisor.set_connection("kept", FakeConnection::handle(ConnectionStatus::Connected, false));
        supervisor.set_connection("busy", FakeConnection::handle(ConnectionStatus::Connected, false));
        supervisor.mark_idle("lazy");
        supervisor.mark_idle("kept");

        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("lazy", ServerEntry::default(), None);
        lc.register_server("kept", keep_alive_entry(), None);
        lc.mark_keep_alive("kept");
        lc.register_server("busy", ServerEntry::default(), None);

        let closed = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&closed);
        lc.set_idle_shutdown_callback(Arc::new(move |name| {
            if let Ok(mut sink) = sink.lock() {
                sink.push(name.to_string());
            }
        }));

        lc.check_connections(&CancelToken::new()).await.unwrap();
        assert!(supervisor.log().contains(&"close:lazy".to_string()));
        assert!(!supervisor.log().contains(&"close:kept".to_string()));
        assert!(!supervisor.log().contains(&"close:busy".to_string()));
        assert_eq!(closed.lock().unwrap().clone(), vec!["lazy".to_string()]);
    }

    #[tokio::test]
    async fn an_idle_timeout_of_zero_exempts_a_server_from_the_sweep() {
        let supervisor = FakeSupervisor::arc();
        supervisor.set_connection("a", FakeConnection::handle(ConnectionStatus::Connected, false));
        supervisor.mark_idle("a");
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server(
            "a",
            ServerEntry::default(),
            Some(LifecycleOverrides { idle_timeout: Some(0.0) }),
        );

        lc.check_connections(&CancelToken::new()).await.unwrap();
        assert!(
            !supervisor.log().contains(&"close:a".to_string()),
            "`timeout > 0` is what disables the sweep, and 0 is a legal configured value"
        );
        assert!(
            !supervisor.log().contains(&"is_idle:a".to_string()),
            "a server with no timeout is not even asked whether it is idle"
        );
    }

    // ── ensureConverged ─────────────────────────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn ensure_converged_shares_one_pass_across_concurrent_callers() {
        let supervisor = FakeSupervisor::arc();
        if let Ok(mut delay) = supervisor.connect_delay.lock() {
            *delay = Duration::from_millis(50);
        }
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");

        let token = CancelToken::new();
        let first = lc.ensure_converged(token.clone());
        let second = lc.ensure_converged(token.clone());
        let (a, b) = futures::future::join(first, second).await;
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(
            supervisor.log(),
            vec!["connect:a", "connected:a"],
            "two callers join one pass"
        );

        // The slot is cleared by the caller that started the pass, so a later call runs a new one.
        lc.ensure_converged(token).await.unwrap();
        assert_eq!(
            supervisor.log().len(),
            2,
            "the second pass runs and finds `a` already connected"
        );
    }

    // ── the interval loop and graceful shutdown (MCP-034, MCP-035) ──────────────────────────

    #[tokio::test(start_paused = true)]
    async fn the_loop_does_not_fire_at_zero_and_ticks_on_the_interval() {
        let supervisor = FakeSupervisor::arc();
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");
        let owner = Arc::new(McpRuntimeOwner::new());
        lc.start(&owner);
        settle().await;

        tokio::time::advance(HEALTH_CHECK_INTERVAL - Duration::from_secs(1)).await;
        settle().await;
        assert!(supervisor.log().is_empty(), "`setInterval` does not fire at t=0");

        tokio::time::advance(Duration::from_secs(2)).await;
        settle().await;
        assert_eq!(supervisor.log(), vec!["connect:a", "connected:a"]);

        lc.graceful_shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_long_check_skips_the_tick_it_overran_rather_than_queueing_it() {
        let supervisor = FakeSupervisor::arc();
        if let Ok(mut delay) = supervisor.connect_delay.lock() {
            // 45 s — longer than one 30 s period, shorter than two.
            *delay = Duration::from_secs(45);
        }
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");
        // A non-keep-alive server the idle sweep always asks about, so every completed pass leaves
        // exactly one `is_idle:` mark in the log. That is how the passes are counted.
        supervisor.set_connection("idler", FakeConnection::handle(ConnectionStatus::Connected, false));
        lc.register_server("idler", ServerEntry::default(), None);

        let owner = Arc::new(McpRuntimeOwner::new());
        lc.start(&owner);
        settle().await;

        // t=30 first tick; the connect blocks until t=75. The t=60 tick is missed. `Delay` fires
        // that missed tick once, when it is next polled, and re-bases the schedule from there — so
        // the following tick is at t≈105, not t=90. `Burst` would have fired at t=90 and produced a
        // third pass inside the window below.
        for _ in 0..12 {
            tokio::time::advance(Duration::from_secs(8)).await;
            settle().await;
        }

        let passes = supervisor.log().iter().filter(|e| e.as_str() == "is_idle:idler").count();
        assert_eq!(passes, 2, "the overrun tick fired once and re-based; nothing was queued");

        lc.graceful_shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn an_aborted_owner_stops_the_loop_without_starting_it() {
        let supervisor = FakeSupervisor::arc();
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");
        let owner = Arc::new(McpRuntimeOwner::new());
        owner.token().cancel();

        lc.start(&owner);
        assert!(lc.is_stopped(), "an aborted signal sets `stopped` and starts nothing");
        tokio::time::advance(HEALTH_CHECK_INTERVAL * 3).await;
        settle().await;
        assert!(supervisor.log().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn graceful_shutdown_waits_for_the_in_flight_check_before_close_all() {
        let supervisor = FakeSupervisor::arc();
        if let Ok(mut delay) = supervisor.connect_delay.lock() {
            *delay = Duration::from_millis(200);
        }
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.register_server("a", keep_alive_entry(), None);
        lc.mark_keep_alive("a");
        let owner = Arc::new(McpRuntimeOwner::new());
        lc.start(&owner);
        settle().await;

        // Let the tick fire and the connect start, then shut down while it is in flight.
        tokio::time::advance(HEALTH_CHECK_INTERVAL + Duration::from_millis(50)).await;
        settle().await;
        assert_eq!(supervisor.log(), vec!["connect:a"], "the connect is mid-flight");

        lc.graceful_shutdown().await;
        assert_eq!(
            supervisor.log(),
            vec!["connect:a", "connected:a", "close_all"],
            "closeAll must not race a just-opened connection"
        );
    }

    #[tokio::test]
    async fn graceful_shutdown_is_memoised_sets_stopped_and_closes_once() {
        let supervisor = FakeSupervisor::arc();
        let lc = lifecycle_with(Arc::clone(&supervisor));
        assert!(!lc.is_stopped());

        let first = lc.graceful_shutdown();
        assert!(lc.is_stopped(), "`stopped` is set before anything is awaited");
        let second = lc.graceful_shutdown();
        first.await;
        second.await;
        assert_eq!(supervisor.close_all_calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn graceful_shutdown_drops_the_callbacks() {
        let supervisor = FakeSupervisor::arc();
        let lc = lifecycle_with(Arc::clone(&supervisor));
        lc.set_idle_shutdown_callback(Arc::new(|_| {}));
        lc.graceful_shutdown().await;
        assert!(
            lc.callback(|c| c.on_idle_shutdown.clone()).is_none(),
            "a late timer must not be able to re-enter a dead generation"
        );
    }

    // ── shutdownState (MCP-010) ─────────────────────────────────────────────────────────────

    fn state_with(owner: Arc<McpRuntimeOwner>) -> Arc<McpState> {
        let manager = Arc::new(McpServerManager::default());
        let lifecycle = Arc::new(McpLifecycleManager::new(
            Arc::clone(&manager),
            Arc::new(|_| false),
        ));
        Arc::new(McpState::new(McpStateParts {
            owner,
            manager,
            lifecycle,
            config: McpConfig::default(),
            programmatic_config: None,
            oauth_runtime: Arc::new(OAuthRuntime::default()),
            auth_storage_options: AuthStorageOptions::default(),
            ui: None,
            open_browser: Arc::new(|_| async { Ok(()) }.boxed()),
            send_message: Arc::new(|_| {}),
        }))
    }

    #[tokio::test]
    async fn shutdown_state_publishes_the_empty_snapshot_and_flushes_synchronously() {
        let owner = Arc::new(McpRuntimeOwner::new());
        let state = state_with(Arc::clone(&owner));
        state.publish_status(McpStatusSnapshot {
            connected: vec!["a".to_string()],
            ..Default::default()
        });
        let mut status = state.subscribe_status();

        let flushed = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&flushed);
        let flush: MetadataFlush = Arc::new(move |_| {
            counter.fetch_add(1, Ordering::AcqRel);
            Ok(())
        });

        let pending = shutdown_state(Some(state), SESSION_SHUTDOWN_STATE_REASON, flush);
        assert_eq!(
            flushed.load(Ordering::Acquire),
            1,
            "the flush runs at call time, before the first await"
        );
        assert_eq!(status.borrow_and_update().connected.len(), 0);
        assert!(!owner.is_active(), "the owner is cancelled at call time too");
        pending.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_state_returns_the_flush_error_even_when_the_shutdown_also_fails() {
        let owner = Arc::new(McpRuntimeOwner::new());
        owner.add_cleanup(Box::new(|| {
            async { Err(McpError::other("cleanup exploded")) }.boxed()
        }));
        let state = state_with(Arc::clone(&owner));

        let flush: MetadataFlush =
            Arc::new(|_| Err(McpError::other("mcp-cache.json is read-only")));
        let error = shutdown_state(Some(state), SESSION_SHUTDOWN_STATE_REASON, flush)
            .await
            .expect_err("both halves failed");
        assert!(
            error.to_string().contains("mcp-cache.json is read-only"),
            "the flush error must win: {error}"
        );
    }

    #[tokio::test]
    async fn shutdown_state_surfaces_a_shutdown_failure_when_the_flush_succeeded() {
        let owner = Arc::new(McpRuntimeOwner::new());
        owner.add_cleanup(Box::new(|| {
            async { Err(McpError::other("cleanup exploded")) }.boxed()
        }));
        let state = state_with(Arc::clone(&owner));

        let error = shutdown_state(Some(state), SESSION_SHUTDOWN_STATE_REASON, no_metadata_flush())
            .await
            .expect_err("the cleanup failed");
        assert!(error.to_string().contains("cleanup exploded"), "{error}");
    }

    #[tokio::test]
    async fn shutdown_state_with_no_state_is_a_no_op() {
        shutdown_state(None, SESSION_RESTART_STATE_REASON, no_metadata_flush())
            .await
            .unwrap();
    }

    // ── the generation teardown (MCP-008, MCP-009) ──────────────────────────────────────────

    #[tokio::test]
    async fn the_previous_generation_is_cancelled_before_anything_is_awaited() {
        let owner = Arc::new(McpRuntimeOwner::new());
        let released = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&released);
        owner.add_cleanup(Box::new(move || {
            async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                flag.store(true, Ordering::Release);
                Ok(())
            }
            .boxed()
        }));

        let previous = PreviousGeneration {
            state: None,
            owner: Some(Arc::clone(&owner)),
            oauth: None,
        };
        let teardown = shutdown_previous_generation(
            previous,
            SESSION_RESTART_STOP_REASON,
            SESSION_RESTART_STATE_REASON,
            "MCP: failed to shut down previous session state",
            no_metadata_flush(),
        );

        // This is the whole point of MCP-008: the abort is observable *before* the drain finishes.
        assert!(!owner.is_active(), "the cancel happens at call time, not at first poll");
        assert!(!released.load(Ordering::Acquire), "the cleanup has not finished yet");
        assert_eq!(
            owner.stop_reason().as_deref().map(String::as_str),
            Some(SESSION_RESTART_STOP_REASON)
        );

        teardown.await;
        assert!(released.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn a_failing_teardown_is_logged_not_propagated() {
        let owner = Arc::new(McpRuntimeOwner::new());
        owner.add_cleanup(Box::new(|| {
            async { Err(McpError::other("cleanup exploded")) }.boxed()
        }));
        let state = state_with(Arc::clone(&owner));

        // Returns `()`: a session must start (or end) even when the previous one could not be torn
        // down cleanly (`index.ts:471`, `:530`).
        shutdown_previous_generation(
            PreviousGeneration {
                state: Some(state),
                owner: Some(owner),
                oauth: Some(Arc::new(OAuthRuntime::default())),
            },
            SESSION_SHUTDOWN_STOP_REASON,
            SESSION_SHUTDOWN_STATE_REASON,
            "MCP: session shutdown cleanup failed",
            no_metadata_flush(),
        )
        .await;
    }
}

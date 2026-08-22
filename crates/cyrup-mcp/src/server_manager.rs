//! `McpServerManager` — the connection registry, the five race guards and the teardown that leaves
//! no child behind (`server-manager.ts`; 13c §3.1, §3.11, §3.12; MCP-100, MCP-116, MCP-125,
//! MCP-126, MCP-131), plus `session-recovery.ts`'s `isTerminatedSession` (MCP-134).
//!
//! A session restart, a `/mcp reconnect`, the idle sweep and a model tool call can all touch the
//! same server at the same time. None of them may tear down a connection another owns, none may
//! resurrect a connection whose generation has advanced, and every transport must be closed exactly
//! once. Upstream defends that with five mechanisms; all five are ported here, and each is named at
//! its definition:
//!
//! 1. **`closeGenerations`** — a per-server monotonic counter bumped by every `close`/`closeAll` and
//!    re-checked *after* the connect attempt resolves ([`Tables::close_generations`]).
//! 2. **`connectAttempts`** — a per-attempt `AbortController` that `close` fires so an in-flight
//!    connect tears down its own half-built transport ([`AbortHandle`]).
//! 3. **Object-identity guards** — upstream's `current !== connection`. Ported as
//!    [`Arc::ptr_eq`]/[`std::ptr::eq`]; see [`McpServerManager::do_reconnect`].
//! 4. **The `abortCleanupPromises` `WeakMap`** — so the exact `transport.close()` an abort started
//!    is the one awaited. See [`ConnectionResource`]'s doc: in this port the resource owns its own
//!    once-only close, so the map has no counterpart and the guarantee is structural.
//! 5. **`connectPromises` / `reconnectPromises` / `closePromises`** — single-flight maps that each
//!    delete themselves **only on identity match** ([`futures::future::Shared::ptr_eq`]).
//!
//! # The sixth mechanism, which upstream gets for free: *who drives the work*
//!
//! A JS promise runs to completion whether or not anyone awaits it. A Rust future that nobody polls
//! simply stops — so in this port "who is awaiting" and "whether the work happens" are the same
//! question, and every place upstream relies on the first being irrelevant needs an answer here.
//! Three do, and each runs on a detached `tokio` task with the caller awaiting only the *result*:
//! `connect`'s post-attempt body (the generation fence and `connections.set`), `reconnect`'s
//! `doReconnect`, and `close`'s teardown. Cancelling a waiter cancels the wait, never the work.
//!
//! The same asymmetry is why every once-only flag in this file records that its work **completed**
//! and never that it started: a flag set before an await is a net that disarms itself the moment the
//! future is dropped. See [`ServerConnection::dispose`] and [`DisposeGuard`].
//!
//! And the two check-and-insert single-flight maps are each **one critical section**, not a read
//! and a later write. Upstream's `connectPromises.has` / `.set` pair is separated by pure
//! synchronous code on a single-threaded event loop; here two OS threads can both win a read with no
//! `.await` between it and the insert, because preemption needs no yield point.
//!
//! # Provenance of this file's behavioural claims
//!
//! Every ordering and single-flight claim below was **measured** by driving the real upstream
//! `McpServerManager` (pi-mcp-adapter v2.26.1 = `fafae21`) on node 22 with `createConnection` and
//! `disposeConnection` stubbed, not inferred from reading. The measurements that changed what this
//! file does are called out inline as `MEASURED:`. Two are worth reading before the code:
//!
//! * A `close` racing an in-flight `connect` surfaces the **abort reason**
//!   `MCP connection <name> was closed`, *not* `MCP connection for <name> was closed while
//!   connecting` — because `close` bumps the generation and aborts the attempt in the same
//!   synchronous step, and `throwIfAborted(attemptSignal)` runs before the generation message is
//!   built. (13c's MCP-100 *verify* bullet asserts the second string for this scenario; that is
//!   wrong. The second string is reachable only when the generation advanced without the attempt
//!   being aborted, which was also measured — see [`CONNECTION_CLOSED_WHILE_CONNECTING`].)
//! * `connect` **does not dispose** an existing `closed`/`needs-auth` connection before replacing it
//!   in the map. Measured: `disposedOld=0`. Ported as-is; see [`McpServerManager::connect`].
//!
//! # What this file does *not* build, and why
//!
//! `createConnection`'s body is five other port units — MCP-101 (stdio env/args/npx), MCP-103 (npx
//! pre-resolution), MCP-114/MCP-115 (the HTTP pre-flight and the OAuth attempt ladder) and MCP-119
//! (tool/resource/prompt discovery). None of them is this unit's work, so the manager takes a
//! [`ConnectionFactory`] seam instead of pretending to own them, and the default factory
//! ([`UnbuiltConnectionFactory`]) fails loudly naming the missing unit — the same discipline
//! `lifecycle.rs`'s `ManagerSupervisor::unbound` already uses. What *is* built here is the piece
//! MCP-131 owns and nothing else can supply: [`StdioChildConnection`], a child-process resource that
//! drains stderr, closes exactly once and leaves no surviving process.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwapOption;
use cyrup_core::CancelToken;
use futures::future::{BoxFuture, FutureExt, Shared};
use indexmap::IndexMap;
use rmcp::model::{Prompt, Resource, Tool};
use rmcp::service::PeerRequestOptions;
use rmcp::transport::TokioChildProcess;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStderr;
use tokio::task::JoinHandle;

use crate::abort::{abortable, throw_if_aborted};
use crate::config::ServerEntry;
use crate::errors::{McpError, McpResult};
use crate::lifecycle::{ConnectionHandle, ConnectionStatus, ServerConnectionRef};
use crate::runtime::{append_stderr_tail, build_request_options, normalize_request_timeout_ms,
    stderr_tail_detail};

// =================================================================================================
// Byte-exact strings (`server-manager.ts`)
// =================================================================================================

/// `` throw new Error(`MCP server "${name}" is disabled`) `` — `server-manager.ts:258`, `:319`.
/// MEASURED: both `connect` and `reconnect` raise it, and `reconnect` raises it **before** any
/// teardown (the stale connection is still in the map afterwards).
#[must_use]
pub fn server_disabled_message(name: &str) -> String {
    format!("MCP server \"{name}\" is disabled")
}

/// `throw new Error("MCP server manager is closed")` — `server-manager.ts:259`, `:320`.
pub const MANAGER_CLOSED: &str = "MCP server manager is closed";

/// `` connectAttempts.get(name)?.abort(new Error(`MCP connection ${name} was closed`)) `` —
/// `server-manager.ts:1097`, `:1152`. MEASURED: this is the message a caller of `connect` actually
/// receives when a `close` races the attempt, because `throwIfAborted(attemptSignal)` rethrows
/// `signal.reason` before the generation branch below can build its own message.
#[must_use]
pub fn connection_closed_reason(name: &str) -> String {
    format!("MCP connection {name} was closed")
}

/// `` throw new Error(`MCP connection for ${name} was closed while connecting`) `` —
/// `server-manager.ts:294`.
///
/// MEASURED reachable only when `closeGenerations[name]` advanced while the attempt's own signal was
/// **not** aborted; a plain `close()` does both in one synchronous step and therefore never gets
/// here. Ported anyway, because the arm exists and a future caller that bumps a generation without
/// aborting (a config-driven eviction, say) lands on it.
#[must_use]
pub fn connection_closed_while_connecting(name: &str) -> String {
    format!("MCP connection for {name} was closed while connecting")
}

/// `` throw new Error(`Server "${name}" is not connected`) `` — `server-manager.ts:1063`, `:1082`.
#[must_use]
pub fn server_not_connected_message(name: &str) -> String {
    format!("Server \"{name}\" is not connected")
}

/// The five `server-manager.ts` aggregate heads.
///
/// They used to be five `pub const` definitions in this file, byte-identical to `errors.rs`'s own —
/// a duplication that was harmless only while nothing compared the two. [`From<&ManagerError>`]
/// now dispatches on the head to pick an [`McpError`] aggregate variant, so a drift between the two
/// copies would silently route a real teardown failure to [`McpError::Other`] instead. Re-exported
/// rather than redefined so that cannot happen: there is one definition, in `errors.rs`, and
/// `crate::server_manager::CONNECTION_CLEANUP_FAILED` still resolves for every existing caller.
///
/// MEASURED against the real `disposeConnection`: a `client.close()` that throws yields exactly the
/// [`CONNECTION_CLEANUP_FAILED`] head with the thrown message as its single child, and
/// `containsCleanupFailure` returns `true` for it and `false` for a plain `Error` carrying the same
/// text.
pub use crate::errors::{CONNECTION_ABORT_CLEANUP_FAILED, CONNECTION_CLEANUP_FAILED,
    CONNECTION_SETUP_FAILED, HTTP_CONNECTION_CLEANUP_FAILED, MANAGER_CLEANUP_FAILED};

/// `MAX_CAPTURED_STDERR_BYTES` / `MAX_CAPTURED_STDERR_LINES` live in [`crate::runtime`]; re-exported
/// so a reader of this file finds the tail policy the stderr pump enforces.
pub use crate::runtime::{MAX_CAPTURED_STDERR_BYTES, MAX_CAPTURED_STDERR_LINES};

// =================================================================================================
// MCP-124 seam — the aggregate error shape, held locally until `errors.rs` grows its variants
// =================================================================================================

/// The manager's internal error, and the **only** place the cleanup-versus-connect distinction is
/// structural.
///
/// # Why this is not an `McpError` variant
///
/// `close`'s no-connection arm re-throws a pending connect's failure *only* when it is a teardown
/// failure (`containsCleanupFailure`), and `closeAll` filters its aggregate the same way. Upstream
/// decides that with `/cleanup failed|setup failed/` over `AggregateError.message`;
/// [`McpError::is_cleanup_failure`] is the structural replacement.
///
/// The type stays local anyway, for a reason that has nothing to do with the taxonomy: the
/// single-flight maps hand **one** failure to every waiter, so the internal error has to be
/// `Arc`-shared, and [`McpError`] is deliberately not `Clone` (it carries an [`std::io::Error`]).
///
/// **The blocker this note used to record is closed.** MCP-124 landed the five variants, and
/// [`From<&ManagerError>`] now maps an [`ManagerError::Aggregate`] onto the matching
/// [`McpError`] aggregate by head instead of flattening it to [`McpError::Other`] — so
/// `McpError::is_cleanup_failure` sees a teardown failure raised here, which is the entire point of
/// the unit. `Display` routes through [`crate::errors::render_aggregate_texts`] for the same
/// reason: the rendering a user sees must be `formatTerminalError`'s, and this type is the one that
/// actually reaches them through `closeAll`.
#[derive(Debug)]
pub enum ManagerError {
    /// An ordinary failure: anything [`McpError`] already models.
    Mcp(McpError),
    /// `new AggregateError(children, head)`. `head` is one of the byte-exact constants above.
    Aggregate {
        /// The aggregate's own message.
        head: &'static str,
        /// Its children, in the order upstream collects them.
        children: Vec<Arc<ManagerError>>,
    },
}

impl ManagerError {
    /// Wrap an ordinary failure.
    #[must_use]
    pub fn mcp(error: McpError) -> Arc<Self> {
        Arc::new(ManagerError::Mcp(error))
    }

    /// `throw new Error(message)`.
    #[must_use]
    pub fn other(message: impl Into<String>) -> Arc<Self> {
        Arc::new(ManagerError::Mcp(McpError::Other(message.into())))
    }

    /// `new AggregateError(children, head)`.
    #[must_use]
    pub fn aggregate(head: &'static str, children: Vec<Arc<ManagerError>>) -> Arc<Self> {
        Arc::new(ManagerError::Aggregate { head, children })
    }

    /// `containsCleanupFailure(error)` (`server-manager.ts:1177-1191`).
    ///
    /// Upstream walks an `Error` graph with an explicit stack and a `seen` set, testing
    /// `/cleanup failed|setup failed/` against every `AggregateError` message and pushing `.errors`
    /// and `.cause`. Here the graph is a tree the manager built itself, so the walk is a structural
    /// match on [`ManagerError::Aggregate`] — which cannot be spoofed by a *server-supplied* message
    /// that happens to contain "cleanup failed", the false positive upstream's regex admits. Recorded
    /// as an intentional divergence (the same one `errors.rs` records for
    /// [`McpError::is_cleanup_failure`]).
    ///
    /// Depth-capped for the same reason `errors.rs` caps its `source()` walk: a pathological tree
    /// must not be able to spin a shutdown.
    #[must_use]
    pub fn is_cleanup_failure(&self) -> bool {
        let mut pending: Vec<&ManagerError> = vec![self];
        let mut budget = 1024_u32;
        while let Some(current) = pending.pop() {
            budget = match budget.checked_sub(1) {
                Some(remaining) => remaining,
                None => return false,
            };
            match current {
                ManagerError::Aggregate { head, children } => {
                    // Upstream's regex, as the structural test it stands in for: every head this
                    // module raises is a cleanup-or-setup aggregate.
                    if head.contains("cleanup failed") || head.contains("setup failed") {
                        return true;
                    }
                    pending.extend(children.iter().map(Arc::as_ref));
                }
                ManagerError::Mcp(error) => {
                    if error.is_cleanup_failure() {
                        return true;
                    }
                }
            }
        }
        false
    }
}

impl std::fmt::Display for ManagerError {
    /// `formatTerminalError`'s aggregate arm, shared verbatim with [`McpError`]'s seven aggregates
    /// through [`crate::errors::render_aggregate_texts`].
    ///
    /// This used to be `write!("{head}")` followed by `": {child}"` per child — head-prefixed. That
    /// is the rendering MCP-124 measured as wrong and removed from `McpError`, and leaving it here
    /// meant `closeAll` printed `"MCP manager cleanup failed: client close failed"` where upstream
    /// prints `"client close failed"`: the head is dropped as soon as one child contributes text,
    /// and children are de-duplicated. MEASURED on node 22 —
    /// `AggregateError([Error("connect ECONNREFUSED"), Error("keychain unavailable")], "MCP
    /// connection setup failed")` renders `connect ECONNREFUSED: keychain unavailable`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManagerError::Mcp(error) => write!(f, "{error}"),
            ManagerError::Aggregate { head, children } => f.write_str(
                &crate::errors::render_aggregate_texts(
                    head,
                    children.iter().map(std::string::ToString::to_string),
                ),
            ),
        }
    }
}

impl std::error::Error for ManagerError {}

/// Rebuild an [`McpError`] from the manager's internal error.
///
/// [`McpError`] is deliberately not `Clone` (it carries an [`std::io::Error`]), and the single-flight
/// maps must hand the same failure to every waiter, so the internal type is `Arc`-shared and this is
/// the one-way door back.
///
/// **[`McpError::Aborted`] is preserved exactly**, and that is the point of writing this out rather
/// than collapsing everything to `Other(to_string())`: [`crate::abort::is_abort_error`] is what stops
/// a user cancellation from being recorded as a connect failure and poisoning the next 60 seconds of
/// that server's availability (MCP-024). `Config` and `Server` round-trip too because `/mcp` renders
/// them; everything else keeps its rendered text and loses only its class.
///
/// # The aggregate class is preserved too — MCP-124's actual point
///
/// An aggregate that loses its class here is an aggregate `McpError::is_cleanup_failure` cannot
/// see, and that predicate is the *whole* reason the five variants exist: `close`'s no-connection
/// arm re-throws a pending connect's failure only when it is a teardown failure, and `closeAll`
/// filters its children the same way. Both aggregate shapes are therefore rebuilt rather than
/// rendered:
///
/// * [`ManagerError::Aggregate`] — dispatched on its head onto the matching [`McpError`] variant.
///   The heads are the re-exported `errors.rs` constants, so this match is on the same items the
///   producers pass, not on two copies of a string literal.
/// * `ManagerError::Mcp(<an aggregate>)` — the shape a factory error takes
///   (`factory.create(..).map_err(ManagerError::mcp)`), which is how `createConnection`'s
///   [`McpError::SetupFailed`] arrives. Rebuilt through [`McpError::aggregate_head`] /
///   [`McpError::aggregate_children`] because [`McpError`] is not [`Clone`].
///
/// The walk is depth-capped for the same reason [`McpError::is_cleanup_failure`] is: these trees
/// are built by this crate, but a bounded walk is the cheap way to keep a pathological one from
/// spinning a shutdown. Past the cap the error degrades to its rendered text, never to a hang.
impl From<&ManagerError> for McpError {
    fn from(error: &ManagerError) -> Self {
        rebuild_manager_error(error, AGGREGATE_REBUILD_DEPTH)
    }
}

/// Depth cap for [`From<&ManagerError>`]'s aggregate rebuild. Matches `errors.rs`'s own budget.
const AGGREGATE_REBUILD_DEPTH: u32 = 1024;

fn rebuild_manager_error(error: &ManagerError, depth: u32) -> McpError {
    let Some(remaining) = depth.checked_sub(1) else {
        return McpError::Other(error.to_string());
    };
    match error {
        ManagerError::Mcp(McpError::Aborted(reason)) => McpError::Aborted(reason.clone()),
        ManagerError::Mcp(McpError::Config(message)) => McpError::Config(message.clone()),
        ManagerError::Mcp(McpError::Server { server, message }) => McpError::Server {
            server: server.clone(),
            message: message.clone(),
        },
        ManagerError::Mcp(inner) => match (inner.aggregate_head(), inner.aggregate_children()) {
            (Some(head), Some(children)) => {
                let rebuilt = children
                    .iter()
                    .map(|child| rebuild_mcp_error(child, remaining))
                    .collect::<Vec<_>>();
                aggregate_with_head(head, crate::errors::CleanupErrors::from(rebuilt), inner)
            }
            _ => McpError::Other(inner.to_string()),
        },
        ManagerError::Aggregate { head, children } => {
            let rebuilt = children
                .iter()
                .map(|child| rebuild_manager_error(child, remaining))
                .collect::<Vec<_>>();
            aggregate_with_head(head, crate::errors::CleanupErrors::from(rebuilt), error)
        }
    }
}

fn rebuild_mcp_error(error: &McpError, depth: u32) -> McpError {
    let Some(remaining) = depth.checked_sub(1) else {
        return McpError::Other(error.to_string());
    };
    match error {
        McpError::Aborted(reason) => McpError::Aborted(reason.clone()),
        McpError::Config(message) => McpError::Config(message.clone()),
        McpError::Server { server, message } => McpError::Server {
            server: server.clone(),
            message: message.clone(),
        },
        other => match (other.aggregate_head(), other.aggregate_children()) {
            (Some(head), Some(children)) => {
                let rebuilt = children
                    .iter()
                    .map(|child| rebuild_mcp_error(child, remaining))
                    .collect::<Vec<_>>();
                aggregate_with_head(head, crate::errors::CleanupErrors::from(rebuilt), other)
            }
            _ => McpError::Other(other.to_string()),
        },
    }
}

/// The head → variant dispatch. `fallback` is rendered only for a head this crate does not raise,
/// which today is unreachable — every producer passes one of the six constants.
fn aggregate_with_head(
    head: &str,
    children: crate::errors::CleanupErrors,
    fallback: &dyn std::fmt::Display,
) -> McpError {
    match head {
        CONNECTION_ABORT_CLEANUP_FAILED => McpError::AbortCleanupFailed(children),
        CONNECTION_SETUP_FAILED => McpError::SetupFailed(children),
        HTTP_CONNECTION_CLEANUP_FAILED => McpError::HttpCleanupFailed(children),
        CONNECTION_CLEANUP_FAILED => McpError::ConnectionCleanupFailed(children),
        MANAGER_CLEANUP_FAILED => McpError::ManagerCleanupFailed(children),
        crate::errors::RUNTIME_CLEANUP_FAILED => McpError::RuntimeCleanupFailed(children),
        // The OAuth flow's three phases are the seventh aggregate. They cannot be raised by this
        // module, but they can be *carried* by it — a credential-store failure during a connect
        // arrives as an `McpError::OAuthAggregate` inside `ManagerError::Mcp`, and that class is
        // load-bearing all the way up (section 07 rethrows a store failure and swallows everything
        // else, so a downgrade here is an infinite silent re-auth loop).
        crate::oauth::PHASE_STARTUP_CLEANUP => McpError::OAuthAggregate {
            phase: crate::oauth::PHASE_STARTUP_CLEANUP,
            errors: children,
        },
        crate::oauth::PHASE_COMPLETION_CLEANUP => McpError::OAuthAggregate {
            phase: crate::oauth::PHASE_COMPLETION_CLEANUP,
            errors: children,
        },
        crate::oauth::PHASE_CANCELLATION_CLEANUP => McpError::OAuthAggregate {
            phase: crate::oauth::PHASE_CANCELLATION_CLEANUP,
            errors: children,
        },
        _ => McpError::Other(fallback.to_string()),
    }
}

/// The manager's internal result. Public methods return [`McpResult`]; see [`ManagerError`] for what
/// the conversion keeps.
pub type ManagerResult<T> = Result<T, Arc<ManagerError>>;

fn to_mcp<T>(result: ManagerResult<T>) -> McpResult<T> {
    result.map_err(|error| McpError::from(error.as_ref()))
}

// =================================================================================================
// The per-attempt abort handle — `connectAttempts` (MCP-100)
// =================================================================================================

/// One entry of `connectAttempts`: upstream's `new AbortController()` plus the payload a
/// [`CancelToken`] cannot carry.
///
/// `AbortController.abort(new Error(reason))` stores the reason on `signal.reason`, and
/// `throwIfAborted` rethrows it verbatim — which is why a caller of `connect` sees
/// `MCP connection <name> was closed` and not a generic cancellation. `tokio_util`'s token has no
/// payload, so the reason rides alongside in an [`ArcSwapOption`] written **before** `cancel()`, the
/// same construction [`crate::owner::McpRuntimeOwner`] uses for `stop(reason)`.
#[derive(Debug)]
pub struct AbortHandle {
    token: CancelToken,
    reason: ArcSwapOption<String>,
}

impl AbortHandle {
    /// A fresh, un-fired handle.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            token: CancelToken::new(),
            reason: ArcSwapOption::empty(),
        })
    }

    /// `controller.abort(new Error(reason))`. The reason is stored **first**, so any task woken by
    /// the cancellation already sees it.
    pub fn abort(&self, reason: impl Into<String>) {
        self.reason.store(Some(Arc::new(reason.into())));
        self.token.cancel();
    }

    /// The attempt's own signal.
    #[must_use]
    pub fn token(&self) -> &CancelToken {
        &self.token
    }

    /// `signal.reason`, if this handle was aborted with one.
    #[must_use]
    pub fn reason(&self) -> Option<Arc<String>> {
        self.reason.load_full()
    }

    /// `controller.signal.aborted`.
    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Reap the [`crate::abort::combine`] joiner task this handle's token was combined into, once
    /// the attempt it belongs to is over.
    ///
    /// `combine`'s third `select!` arm exists so a child cancel ends the joiner without disturbing
    /// either parent — but the token to cancel must be **this handle's**, never the combined
    /// result. `combine` degenerates to one of its parents whenever the other is absent or already
    /// cancelled (`abort.rs:64-72`), and one of those parents is the runtime signal, so cancelling
    /// the combined token could stop the whole session. This token is private to one attempt and
    /// can reach nothing above it.
    ///
    /// It also marks the attempt aborted, which is why this is only ever called from
    /// [`AttemptSlot`]'s `Drop` — after every fence that reads [`Self::is_aborted`].
    pub fn reap(&self) {
        self.token.cancel();
    }
}

impl Default for AbortHandle {
    fn default() -> Self {
        Self {
            token: CancelToken::new(),
            reason: ArcSwapOption::empty(),
        }
    }
}

// =================================================================================================
// MCP-131 — the live half of a connection, and the child process it owns
// =================================================================================================

/// What `client.close()` closes: upstream's `{ client, transport }` pair, as one object that can be
/// disposed **exactly once**.
///
/// # Why the `abortCleanupPromises` `WeakMap` has no counterpart
///
/// Upstream needs it because `connectClientWithAbort` may call `transport.close()` from an `abort`
/// listener while `createConnection`'s `catch` is about to call `client.close()` on the same
/// transport; the `WeakMap` lets the catch await *that exact* close instead of starting a second
/// one. Here the once-only-ness is a property of the resource itself — [`Self::close`] is required
/// to be idempotent and to hand every caller the same outcome — so there is nothing to look up. The
/// guarantee is the same one, moved from a side table into the type.
pub trait ConnectionResource: Send + Sync + std::fmt::Debug {
    /// `client.close()`. Must be idempotent: `close`, `closeAll`'s late sweep and
    /// [`ServerConnection`]'s drop-net can all reach it, and a transport must be closed once.
    fn close(&self) -> BoxFuture<'_, McpResult<()>>;

    /// `(transport as {sessionId?: string})?.sessionId != null` (`session-recovery.ts:60-65`).
    ///
    /// Only the streamable-HTTP transport has one. **Recorded divergence** (MCP-134): JS duck-types
    /// the property and reads a missing transport as absent, while here stdio is structurally
    /// session-less — stricter, not observably different.
    fn has_session_id(&self) -> bool {
        false
    }

    /// The child pid, when this resource owns one. Exists so a test can assert on the process table
    /// rather than on a log line; production reads it only for diagnostics.
    fn child_pid(&self) -> Option<u32> {
        None
    }

    /// The bounded stderr tail this resource captured, if any (`stderrTail`, §3.3 step 8).
    fn stderr_detail(&self) -> Option<String> {
        None
    }
}

/// A stdio child process and the two things that keep it from becoming an orphan.
///
/// **MCP-131 is this type.** Two hazards, both of which this repository has been bitten by:
///
/// 1. **The unread stderr pipe.** `TokioChildProcessBuilder::spawn` hands back a `ChildStderr` when
///    stderr is piped (`definition.debug !== true`, §3.3 step 7). Upstream attaches a `"data"`
///    listener that drains it continuously into a bounded tail. A port that merely *holds* the
///    handle without reading it deadlocks the child as soon as it writes 64 KiB of diagnostics — the
///    child blocks in `write`, ignores its stdin closing, and only the 3-second hard kill ends it.
///    [`Self::spawn`] therefore starts a drain task, and [`Self::close`] aborts it.
/// 2. **Closing exactly once, and actually reaping.** `close` is `graceful_shutdown()`: close the
///    transport (which drops the child's stdin), then `select!` the child's `wait()` against
///    `MAX_WAIT_ON_DROP_SECS = 3`, killing on timeout. The slot guard is held **across** that await
///    and the process surrendered only once it returns, so a second `close` is a no-op rather than a
///    second kill *and* a cancelled close leaves the process where the next caller can find it.
///    `ChildWithCleanup::drop` is **not** a net for a future dropped mid-shutdown: rmcp moves the
///    child out of it before its own first await, so nothing is left holding a kill-on-drop. The
///    guarantee therefore has to come from never dropping the teardown — see
///    [`McpServerManager::close_inner`]'s detached driver.
///
/// **Named delta against the TS SDK** (13c §3.12): the SDK escalates close-stdin → 2 s → `SIGTERM`
/// → 2 s → `SIGKILL`; rmcp uses one 3-second window and then a hard kill, with **no `SIGTERM` leg**.
/// A server that ignores stdin closure but would have honoured `SIGTERM` is hard-killed.
///
/// **Named residual — the grandchild.** Both signal a single pid, not a process group. The plan
/// argues that is sufficient *because* npx pre-resolution (MCP-103) removes the `npm` launcher that
/// would otherwise be the grandparent. MCP-103 is **not ported** (see [`UnbuiltConnectionFactory`]),
/// and independently of it a server that forks its own worker leaves that worker behind. The
/// measurement is in this module's tests (`a_forking_child_leaves_its_grandchild_behind`), which
/// asserts the *current* behaviour rather than the desired one so the residual cannot be mistaken
/// for closed. Fixing it means `process_wrap::tokio::ProcessGroup::leader()` on the `CommandWrap`
/// rmcp spawns from, which needs a `process-wrap` entry in `crates/cyrup-mcp/Cargo.toml` — outside
/// this unit's files, so it is named here rather than smuggled in.
///
/// `Debug` is hand-written because `TokioChildProcess` is not `Debug` and
/// [`ConnectionResource`] requires it — the pid and whether the process is still held is the whole
/// of what a diagnostic needs anyway.
pub struct StdioChildConnection {
    /// `None` once shutdown has started, which is what makes [`Self::close`] once-only.
    process: tokio::sync::Mutex<Option<TokioChildProcess>>,
    pid: Option<u32>,
    /// The stderr drain task. Aborted on close; it also ends by itself at EOF.
    pump: Mutex<Option<JoinHandle<()>>>,
    /// `stderrTail` — bounded by [`MAX_CAPTURED_STDERR_BYTES`] / [`MAX_CAPTURED_STDERR_LINES`].
    tail: Arc<Mutex<VecDeque<u8>>>,
}

impl std::fmt::Debug for StdioChildConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioChildConnection")
            .field("pid", &self.pid)
            .field(
                "live",
                &self.process.try_lock().map_or(true, |slot| slot.is_some()),
            )
            .finish()
    }
}

impl StdioChildConnection {
    /// Adopt an already-spawned child (the pair [`crate::runtime::spawn_stdio_transport`] returns).
    ///
    /// `stderr` is `Some` exactly when the definition asked for `debug: false`; when it is `Some` a
    /// drain task starts immediately, because between here and the first read the child can already
    /// have filled the pipe.
    #[must_use]
    pub fn adopt(process: TokioChildProcess, stderr: Option<ChildStderr>) -> Arc<Self> {
        let pid = process.id();
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let pump = stderr.and_then(|stderr| {
            let sink = Arc::clone(&tail);
            // `tokio::spawn` panics off-runtime and this crate denies `clippy::panic`; no runtime
            // means no drain, which degrades the tail rather than the connection.
            tokio::runtime::Handle::try_current().ok().map(|handle| {
                handle.spawn(async move { drain_stderr(stderr, sink).await })
            })
        });
        Arc::new(Self {
            process: tokio::sync::Mutex::new(Some(process)),
            pid,
            pump: Mutex::new(pump),
            tail,
        })
    }
}

/// The `"data"` listener of §3.3 step 8, as a task.
///
/// Reads until EOF into the bounded tail. The buffer is 8 KiB — one page of a chatty server's
/// startup banner — and [`append_stderr_tail`] does the bounding, so an infinitely noisy child costs
/// a constant amount of memory and one blocked-on-read task, never a blocked child.
async fn drain_stderr(mut stderr: ChildStderr, tail: Arc<Mutex<VecDeque<u8>>>) {
    let mut buffer = [0_u8; 8192];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                let Some(chunk) = buffer.get(..read) else { return };
                let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
                append_stderr_tail(&mut tail, chunk);
            }
        }
    }
}

impl ConnectionResource for StdioChildConnection {
    fn close(&self) -> BoxFuture<'_, McpResult<()>> {
        Box::pin(async move {
            // The drain task is aborted whether or not there is still a process, and *before* the
            // process lock is taken so the two locks are never nested: after the child is gone the
            // read resolves at EOF anyway, and aborting is what keeps a `close` on a never-started
            // child from leaving a task parked on a pipe. `take()` makes a second close a no-op.
            if let Some(pump) = self
                .pump
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            {
                pump.abort();
            }
            // The guard is held ACROSS `graceful_shutdown` and the process surrendered only once
            // that has actually returned. `take()`ing first hands ownership away *at* the
            // suspension point, which is the cancellation bug: a close future dropped mid-shutdown
            // left `None` behind, so the next caller — `close_all`, the drop-net, a second
            // `close` — returned `Ok(())` in microseconds having killed nothing. Holding it makes
            // a concurrent close serialise behind this one instead (the once-only-ness upstream
            // buys with its `abortCleanupPromises` WeakMap) and leaves a cancelled shutdown's
            // process in the slot for the next caller.
            //
            // Named residual, because holding the guard is *not* sufficient on its own: rmcp's
            // `graceful_shutdown` does `self.child.inner.take()` before **its** first await
            // (`rmcp-3.1.4/src/transport/child_process.rs:81`), so a future dropped inside it has
            // already moved the child out of `ChildWithCleanup`'s kill-on-drop net, and neither
            // `tokio::process::Child` nor `process-wrap` has one of its own. What actually keeps
            // that child from being orphaned is that the teardown is never dropped — see the
            // detached driver in [`McpServerManager::close_inner`].
            let mut slot = self.process.lock().await;
            let Some(process) = slot.as_mut() else {
                return Ok(());
            };
            let outcome = process.graceful_shutdown().await;
            // Only now, with the child reaped, is the slot emptied: a second close is then a
            // genuine no-op rather than a second signal down a pid that may since have been
            // recycled.
            drop(slot.take());
            outcome.map_err(|source| McpError::Io {
                path: std::path::PathBuf::from("<mcp stdio child>"),
                source,
            })
        })
    }

    fn child_pid(&self) -> Option<u32> {
        self.pid
    }

    fn stderr_detail(&self) -> Option<String> {
        stderr_tail_detail(&self.tail.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// A resource with nothing to close — the shape a `needs-auth` HTTP connection reaches
/// `createConnection`'s early return with once its transport has already been surrendered, and the
/// stand-in every state-machine test uses.
#[derive(Debug, Default)]
pub struct InertResource {
    closes: AtomicU32,
    session_id: bool,
}

impl InertResource {
    /// A resource that reports no session id.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A resource that reports a streamable-HTTP session id — the `hadSessionId` gate of MCP-134.
    #[must_use]
    pub fn with_session_id() -> Arc<Self> {
        Arc::new(Self {
            closes: AtomicU32::new(0),
            session_id: true,
        })
    }

    /// How many times [`ConnectionResource::close`] reached this resource. The assertion behind
    /// "a transport is closed exactly once".
    #[must_use]
    pub fn close_count(&self) -> u32 {
        self.closes.load(Ordering::SeqCst)
    }
}

impl ConnectionResource for InertResource {
    fn close(&self) -> BoxFuture<'_, McpResult<()>> {
        Box::pin(async move {
            self.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn has_session_id(&self) -> bool {
        self.session_id
    }
}

// =================================================================================================
// ServerConnection — §3.1's record (MCP-100, MCP-116)
// =================================================================================================

/// `Date.now()`. A saturating epoch-millisecond clock: a machine whose clock predates the epoch
/// yields `0` rather than panicking, which matters because this crate denies `clippy::panic` and
/// `lastUsedAt` is read on every request.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

const STATUS_CONNECTED: u8 = 0;
const STATUS_CLOSED: u8 = 1;
const STATUS_NEEDS_AUTH: u8 = 2;

const fn status_code(status: ConnectionStatus) -> u8 {
    match status {
        ConnectionStatus::Connected => STATUS_CONNECTED,
        ConnectionStatus::Closed => STATUS_CLOSED,
        ConnectionStatus::NeedsAuth => STATUS_NEEDS_AUTH,
    }
}

const fn status_from_code(code: u8) -> ConnectionStatus {
    match code {
        STATUS_CLOSED => ConnectionStatus::Closed,
        STATUS_NEEDS_AUTH => ConnectionStatus::NeedsAuth,
        // Any other bit pattern is unreachable — the field is only ever written by
        // `status_code` — and `Connected` is the safe read: it is the state every guard tests
        // *against*, so a corrupt value fails closed at the next identity check rather than
        // silently satisfying an idle sweep.
        _ => ConnectionStatus::Connected,
    }
}

/// `ServerConnection` (`server-manager.ts:123-145`).
///
/// Upstream is a mutable JS record shared by a dozen closures; here every field that mutates after
/// construction carries its own atomic or lock, and the record is always held behind an `Arc` whose
/// **pointer identity** is the port of upstream's `===`. Nothing in this type takes two locks, and
/// nothing holds one across an `await`.
#[derive(Debug)]
pub struct ServerConnection {
    /// `connection.definition` — the config snapshot this connection was built from. Deliberately a
    /// snapshot: `withSessionRecovery` re-reads the *live* config precisely because this one is
    /// stale by then (13c §3.15 step 5).
    definition: Arc<ServerEntry>,
    /// `{ client, transport }`. See [`ConnectionResource`].
    resource: Arc<dyn ConnectionResource>,
    /// `connection.status`.
    status: AtomicU8,
    /// `connection.credentialsInvalidated` (MCP-116) — true once *this* needs-auth episode discarded
    /// the cached credential. Fed back into the next `connect` so a retry loop cannot repeatedly
    /// discard a good credential.
    credentials_invalidated: AtomicBool,
    /// `connection.lastUsedAt`, epoch ms.
    last_used_at: AtomicU64,
    /// `connection.inFlight`.
    in_flight: AtomicU32,
    /// `connection.instructions` — present only when the server sent one.
    instructions: Option<String>,
    /// `connection.tools` — replaced wholesale by `tools/list_changed`.
    ///
    /// **Populated by MCP-119**, which is not this unit. The field is here because the record's
    /// shape is part of MCP-100's contract and `refreshTools`/`list_changed` need somewhere to
    /// write; it stays empty until that unit lands.
    tools: Mutex<Vec<Tool>>,
    /// `connection.resources`. See [`Self::tools`].
    resources: Mutex<Vec<Resource>>,
    /// `connection.prompts`. See [`Self::tools`].
    prompts: Mutex<Vec<Prompt>>,
    /// `connection.promptDiscoveryFailed` — the `prompts` capability was advertised but
    /// `prompts/list` threw. See [`Self::tools`].
    prompt_discovery_failed: AtomicBool,
    /// `disposeConnection` **completed** for this record — or is running and will complete; see
    /// [`Self::dispose`] and [`DisposeGuard`], which puts the flag back when it does not. Distinct
    /// from `status == closed`: `close` sets the status *before* awaiting cleanup so a replacement
    /// cannot be removed by an old close finishing later, and this flag is what stops the drop-net
    /// below from starting a second teardown behind a completed one.
    disposed: AtomicBool,
}

impl ServerConnection {
    /// Build the record around a freshly created live half.
    #[must_use]
    pub fn new(
        definition: Arc<ServerEntry>,
        resource: Arc<dyn ConnectionResource>,
        status: ConnectionStatus,
        credentials_invalidated: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            definition,
            resource,
            status: AtomicU8::new(status_code(status)),
            credentials_invalidated: AtomicBool::new(credentials_invalidated),
            last_used_at: AtomicU64::new(now_ms()),
            in_flight: AtomicU32::new(0),
            instructions: None,
            tools: Mutex::new(Vec::new()),
            resources: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
            prompt_discovery_failed: AtomicBool::new(false),
            disposed: AtomicBool::new(false),
        })
    }

    /// `connection.definition`.
    #[must_use]
    pub fn definition(&self) -> &Arc<ServerEntry> {
        &self.definition
    }

    /// The live half.
    #[must_use]
    pub fn resource(&self) -> &Arc<dyn ConnectionResource> {
        &self.resource
    }

    /// `connection.status`.
    #[must_use]
    pub fn status(&self) -> ConnectionStatus {
        status_from_code(self.status.load(Ordering::SeqCst))
    }

    /// `connection.status = …`.
    pub fn set_status(&self, status: ConnectionStatus) {
        self.status.store(status_code(status), Ordering::SeqCst);
    }

    /// `connection.credentialsInvalidated === true` (MCP-116).
    #[must_use]
    pub fn credentials_invalidated(&self) -> bool {
        self.credentials_invalidated.load(Ordering::SeqCst)
    }

    /// `connection.lastUsedAt`.
    #[must_use]
    pub fn last_used_at(&self) -> u64 {
        self.last_used_at.load(Ordering::SeqCst)
    }

    /// `connection.lastUsedAt = Date.now()`.
    pub fn touch(&self) {
        self.last_used_at.store(now_ms(), Ordering::SeqCst);
    }

    /// `connection.inFlight`.
    #[must_use]
    pub fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// `connection.inFlight = (connection.inFlight ?? 0) + 1`. Saturating: an in-flight counter that
    /// wrapped to zero would let the idle sweep close a server mid-call.
    pub fn increment_in_flight(&self) {
        let _ = self
            .in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                Some(count.saturating_add(1))
            });
    }

    /// `if (connection && connection.inFlight) connection.inFlight--` — never below zero
    /// (`server-manager.ts:1216-1221`). MEASURED: three decrements against one increment leave `0`.
    pub fn decrement_in_flight(&self) {
        let _ = self
            .in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                Some(count.saturating_sub(1))
            });
    }

    /// `fresh.inFlight = Math.max(fresh.inFlight, staleInFlight)` (`doReconnect`, MCP-125).
    pub fn raise_in_flight_to(&self, floor: u32) {
        let _ = self
            .in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                Some(count.max(floor))
            });
    }

    /// `connection.instructions`.
    #[must_use]
    pub fn instructions(&self) -> Option<&str> {
        self.instructions.as_deref()
    }

    /// `connection.tools`.
    #[must_use]
    pub fn tools(&self) -> Vec<Tool> {
        self.tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `connection.tools = tools` — MCP-119/MCP-120's writer.
    pub fn set_tools(&self, tools: Vec<Tool>) {
        *self.tools.lock().unwrap_or_else(PoisonError::into_inner) = tools;
    }

    /// `connection.resources`.
    #[must_use]
    pub fn resources(&self) -> Vec<Resource> {
        self.resources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `connection.resources = resources`.
    pub fn set_resources(&self, resources: Vec<Resource>) {
        *self.resources.lock().unwrap_or_else(PoisonError::into_inner) = resources;
    }

    /// `connection.prompts`.
    #[must_use]
    pub fn prompts(&self) -> Vec<Prompt> {
        self.prompts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `connection.prompts = prompts; connection.promptDiscoveryFailed = failed`.
    pub fn set_prompts(&self, prompts: Vec<Prompt>, failed: bool) {
        *self.prompts.lock().unwrap_or_else(PoisonError::into_inner) = prompts;
        self.prompt_discovery_failed.store(failed, Ordering::SeqCst);
    }

    /// `connection.promptDiscoveryFailed === true`.
    #[must_use]
    pub fn prompt_discovery_failed(&self) -> bool {
        self.prompt_discovery_failed.load(Ordering::SeqCst)
    }

    /// `hasSessionId(connection)` (`session-recovery.ts:60-65`) — must be captured **before** the
    /// call whose failure is being classified, never read at catch time.
    #[must_use]
    pub fn has_session_id(&self) -> bool {
        self.resource.has_session_id()
    }

    /// The one place `client.close()` is called, and it runs at most once per record.
    ///
    /// # The flag records that the close *completed*, never that it started
    ///
    /// A once-only flag set before the await is a net that disarms itself the instant the future is
    /// dropped. This one used to be `swap(true)` on entry, so a `close()` cancelled inside
    /// `graceful_shutdown` left `disposed == true` with a live child: [`Drop for
    /// ServerConnection`](ServerConnection#impl-Drop) then read that flag, concluded the record was
    /// already disposed and declined to fire for exactly the case it exists for, and the next
    /// explicit close returned `Ok(())` in microseconds having killed nothing.
    ///
    /// The claim is kept (the flag is still taken up-front, so two concurrent disposes still reach
    /// `resource.close()` once) and only the *completion* half is corrected: [`DisposeGuard`] puts
    /// the flag back if this future is dropped before `resource.close()` returns.
    async fn dispose(&self) -> McpResult<()> {
        if self.disposed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let guard = DisposeGuard {
            disposed: Some(&self.disposed),
        };
        let result = self.resource.close().await;
        guard.completed();
        result
    }
}

/// Rearms [`ServerConnection::disposed`] when the teardown it guards did not finish.
///
/// The rule this type enforces, stated once: **a once-only flag records that the work was done,
/// never that it was started.** Every flag set before an await is a net that disarms itself the
/// moment the future is dropped.
struct DisposeGuard<'a> {
    disposed: Option<&'a AtomicBool>,
}

impl DisposeGuard<'_> {
    /// The close returned — the flag is now telling the truth, so leave it set.
    fn completed(mut self) {
        self.disposed = None;
    }
}

impl Drop for DisposeGuard<'_> {
    fn drop(&mut self) {
        if let Some(disposed) = self.disposed.take() {
            disposed.store(false, Ordering::SeqCst);
        }
    }
}

/// The drop-net.
///
/// Upstream cannot lose a connection object: `createConnection`'s promise runs to completion whether
/// or not anyone is still awaiting it, so the `connect` body always reaches its generation check and
/// always disposes what it decided not to keep. A Rust future that nobody polls simply stops, so a
/// caller who drops a `connect` mid-flight could otherwise strand a *resolved* connection — and with
/// it a live child process — with no code left to close it.
///
/// This mirrors rmcp's own `ChildWithCleanup::drop`: if the record is dropped without having been
/// disposed, schedule the disposal. It is a net, not the mechanism — every ordinary path disposes
/// explicitly — and off a tokio runtime it degrades to a warning, because this crate denies
/// `clippy::panic` and `tokio::spawn` panics off-runtime.
///
/// The net only works because [`ServerConnection::dispose`] sets `disposed` on **completion**: a
/// flag set on entry is already `true` by the time a cancelled teardown drops the record, and this
/// `Drop` would read it and decline to fire for precisely the case it exists for.
impl Drop for ServerConnection {
    fn drop(&mut self) {
        if self.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        let resource = Arc::clone(&self.resource);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if let Err(error) = resource.close().await {
                        tracing::warn!("MCP: late connection cleanup failed: {error}");
                    }
                });
            }
            Err(_) => {
                tracing::warn!(
                    "MCP: connection dropped off a tokio runtime with no disposal; \
                     its transport is closed by rmcp's own drop handling"
                );
            }
        }
    }
}

/// `ServerConnectionRef` is `lifecycle.ts`'s narrow view of a connection; the real record satisfies
/// it, which is what lets [`crate::lifecycle::ManagerSupervisor`] hand `Arc<ServerConnection>`s
/// straight to the state machine and compare them with [`Arc::ptr_eq`].
impl ServerConnectionRef for ServerConnection {
    fn status(&self) -> ConnectionStatus {
        ServerConnection::status(self)
    }

    fn has_session_id(&self) -> bool {
        ServerConnection::has_session_id(self)
    }
}

// =================================================================================================
// The `createConnection` seam
// =================================================================================================

/// `createConnection(name, definition, signal, requestSignal, credentialsInvalidated)`'s arguments.
///
/// # Both tokens are scoped to this attempt
///
/// [`Self::attempt`] and [`Self::request`] are derived tokens, and each costs one parked
/// [`crate::abort::combine`] joiner task; the manager cancels them once the attempt settles so that
/// cost is bounded per attempt rather than per generation-lifetime (`abort.rs:21-26`). A factory may
/// therefore use them freely for the duration of `create`, and must not stash either one on the
/// connection it returns — cancellation of a *derived* token says the attempt is over, never that
/// the caller or the runtime stopped.
pub struct CreateConnection {
    /// The `mcpServers` key.
    pub name: String,
    /// The definition this attempt was started from.
    pub definition: Arc<ServerEntry>,
    /// `signal` — the **attempt** signal: `combineAbortSignals(ownedSignal, attemptController.signal)`.
    /// A `close` racing this attempt fires it, and the attempt is expected to tear down its own
    /// half-built transport.
    pub attempt: CancelToken,
    /// `requestSignal` — the caller-plus-runtime signal, *without* the attempt controller. Upstream
    /// threads this one into `buildRequestOptions` so a per-request timeout outlives the attempt
    /// controller's abort.
    pub request: CancelToken,
    /// `credentialsInvalidated` — carried forward from a previous `needs-auth` connection so the
    /// credential cache is discarded at most once per episode (MCP-116).
    pub credentials_invalidated: bool,
    /// `buildRequestOptions(definition, requestSignal)`'s timeout half. Computed **once**, before
    /// any transport is built, and reused for the connect and all three discovery list calls
    /// (§3.2).
    pub request_options: Option<PeerRequestOptions>,
}

/// What `createConnection` returns.
pub struct NewConnection {
    /// `{ client, transport }`.
    pub resource: Arc<dyn ConnectionResource>,
    /// `"connected"` or `"needs-auth"` — `createConnection` never returns `"closed"`.
    pub status: ConnectionStatus,
    /// `credentialsInvalidated`, possibly set by this attempt's own 401 handling (MCP-116).
    pub credentials_invalidated: bool,
}

/// `McpServerManager.createConnection` — the transport build, the handshake and discovery.
///
/// Everything behind this trait is a **different port unit**: MCP-101 (stdio env/args), MCP-103 (npx
/// pre-resolution), MCP-113 (transport selection — already in [`crate::runtime::select_transport`]),
/// MCP-109/MCP-114/MCP-115 (the HTTP transport, its pre-flight and its OAuth attempt ladder), MCP-117
/// (revision negotiation), MCP-119 (discovery) and T-10/MCP-477 (`mcp-trace.ts`; there is no
/// MCP-1xx unit for it — see `13-cyrup-mcp.md:688`). The manager owns the state machine around it
/// and nothing inside it. MCP-133 is **not** the trace unit: it is `enrichHttpConnectionError`
/// (`13c-mcp-servers.md:1608`), and its seam is named inside [`McpServerManager::connect_inner`].
pub trait ConnectionFactory: Send + Sync + 'static {
    /// Build one connection, or fail.
    fn create(&self, request: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>>;
}

/// The default factory: fail loudly, naming the units that would build a connection.
///
/// The same discipline `lifecycle.rs`'s `ManagerSupervisor::unbound` uses — a misconfigured build is
/// loud rather than quietly inert. A grep for `MCP-101` finds every site still waiting on the
/// connection builder.
#[derive(Debug, Default)]
pub struct UnbuiltConnectionFactory;

impl ConnectionFactory for UnbuiltConnectionFactory {
    fn create(&self, request: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
        let name = request.name;
        Box::pin(async move {
            Err(McpError::Server {
                server: name,
                message: "MCP connection builder is not wired yet: `createConnection` is pending \
                          MCP-101/MCP-103 (stdio) and MCP-114/MCP-115 (HTTP), with discovery \
                          pending MCP-119"
                    .to_string(),
            })
        })
    }
}

// =================================================================================================
// McpServerManager
// =================================================================================================

/// `connectPromises` / `reconnectPromises` — a connect attempt, shared by every racing caller.
type ConnectFuture = Shared<BoxFuture<'static, ManagerResult<Arc<ServerConnection>>>>;

/// `closePromises` — one teardown, shared by every racing caller.
type CloseFuture = Shared<BoxFuture<'static, ManagerResult<()>>>;

/// `metadataListChangedListener` (`server-manager.ts:181`).
pub type MetadataListChangedListener = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// One entry of `pendingMetadataPublications` (`server-manager.ts:157-160`).
#[derive(Clone)]
struct PendingPublication {
    connection: Arc<ServerConnection>,
    reason: String,
}

/// The seven manager-owned maps, behind one lock.
///
/// Upstream these are seven separate `Map`s mutated from a single-threaded event loop. One `Mutex`
/// here rather than seven is **not** a simplification of the data model — every map is still keyed
/// and deleted independently — it is a lock-ordering guarantee, the same one `lifecycle.rs`'s
/// `Registry` makes: no arm of this state machine can hold two of them, and **none is ever held
/// across an `await`**. A deadlock here hangs the session, so the invariant is worth the coarser
/// lock; nothing in this file contends on it for longer than a map lookup.
#[derive(Default)]
struct Tables {
    /// `connections`. `IndexMap`, because `new Map()` preserves insertion order and that order is
    /// the one `getAllConnections` and the status snapshot iterate in.
    connections: IndexMap<String, Arc<ServerConnection>>,
    /// `connectPromises`.
    connect_promises: HashMap<String, ConnectFuture>,
    /// `reconnectPromises`.
    reconnect_promises: HashMap<String, ConnectFuture>,
    /// `closePromises`. The `u64` is a per-teardown ticket: upstream's `.finally()` closes over
    /// the very promise it is attached to and deletes only on identity match, which Rust cannot
    /// express without the future referring to itself, so the ticket carries the identity
    /// instead. It is issued by [`McpServerManager::close_ticket`] and is monotonic, so no two
    /// teardowns of one server can be confused.
    close_promises: HashMap<String, (u64, CloseFuture)>,
    /// `closeGenerations` — guard 1.
    close_generations: HashMap<String, u64>,
    /// `connectAttempts` — guard 2.
    connect_attempts: HashMap<String, Arc<AbortHandle>>,
    /// `acceptedUrlElicitations`.
    accepted_url_elicitations: HashMap<String, HashSet<String>>,
    /// `pendingMetadataPublications`.
    pending_metadata_publications: HashMap<String, PendingPublication>,
    /// Teardowns that belong to no server *name*: the dispose a losing connect attempt performs on
    /// its own connection inside `connect_inner`'s tail.
    ///
    /// It cannot live in [`Self::close_promises`], which is keyed by name — a concurrent `close`
    /// for the same server is disposing the OLD connection while this disposes the NEW one, so
    /// one would evict the other from the drain and leak whichever lost. Keyed by ticket instead,
    /// which is unique per teardown by construction.
    ///
    /// Why it must be drained at all: measured, `close_all()` returned `Ok` while a child spawned
    /// by an in-flight connect was still alive, and that child then SURVIVED the runtime being
    /// dropped — a real orphan, and a direct contradiction of MCP-126's verify criterion ("zero
    /// surviving child processes by process-table check", which is a claim about when `closeAll`
    /// returns). Upstream has the same *shape* here — `closeAll` awaits `connectPromises`, which
    /// holds `createConnection`'s promise rather than the `connect` body that disposes its result
    /// — but not the same *outcome*: in node that body is a live promise that runs to completion,
    /// where a dropped Rust future runs nothing at all.
    tail_disposes: HashMap<u64, CloseFuture>,
}

/// `McpServerManager` (`server-manager.ts:150-1247`).
pub struct McpServerManager {
    /// `constructor(private readonly defaultCwd?: string)` — the base for `resolveConfigPath` and
    /// the fallback `cwd` of every stdio child.
    default_cwd: Option<std::path::PathBuf>,
    factory: Arc<dyn ConnectionFactory>,
    tables: Mutex<Tables>,
    /// `stopped` — set by `closeAll` and never cleared. MEASURED: after `closeAll`, both `connect`
    /// and `reconnect` raise [`MANAGER_CLOSED`].
    stopped: AtomicBool,
    /// `runtimeSignal` — `combineAbortSignals(owner.signal, initialSignal)`, set once per generation.
    runtime_signal: Mutex<Option<CancelToken>>,
    /// `defaultRequestTimeoutMs`, **normalised on the way in** (`setDefaultRequestTimeoutMs`).
    default_request_timeout_ms: Mutex<Option<f64>>,
    /// `metadataListChangedListener`.
    metadata_listener: Mutex<Option<MetadataListChangedListener>>,
    /// `samplingConfig` — nulled by `closeAll` so a late callback cannot re-enter a dead runtime.
    sampling: Mutex<Option<crate::runtime::SamplingHook>>,
    /// `elicitationConfig` — the mode plus its handler; nulled by `closeAll` for the same reason.
    elicitation: Mutex<Option<(crate::runtime::ElicitationMode, crate::runtime::ElicitationHook)>>,
    /// `authStorageOptions`.
    auth_storage_options: Mutex<crate::credentials::AuthStorageOptions>,
    /// `oauthRuntime`.
    oauth_runtime: Mutex<Option<Arc<crate::oauth::McpOAuthRuntime>>>,
}

impl std::fmt::Debug for McpServerManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (connections, connecting) = match self.tables.try_lock() {
            Ok(tables) => (tables.connections.len(), tables.connect_promises.len()),
            Err(_) => (usize::MAX, usize::MAX),
        };
        f.debug_struct("McpServerManager")
            .field("default_cwd", &self.default_cwd)
            .field("stopped", &self.stopped.load(Ordering::SeqCst))
            .field("connections", &connections)
            .field("connecting", &connecting)
            .finish_non_exhaustive()
    }
}

impl Default for McpServerManager {
    fn default() -> Self {
        Self::with_factory(None, Arc::new(UnbuiltConnectionFactory))
    }
}

impl McpServerManager {
    /// `new McpServerManager(cwd)`.
    #[must_use]
    pub fn new(default_cwd: impl Into<std::path::PathBuf>) -> Self {
        Self::with_factory(Some(default_cwd.into()), Arc::new(UnbuiltConnectionFactory))
    }

    /// As [`Self::new`], with `createConnection` supplied explicitly — the seam MCP-101/114/115/119
    /// bind to, and the one this module's tests script.
    #[must_use]
    pub fn with_factory(
        default_cwd: Option<std::path::PathBuf>,
        factory: Arc<dyn ConnectionFactory>,
    ) -> Self {
        Self {
            default_cwd,
            factory,
            tables: Mutex::new(Tables::default()),
            stopped: AtomicBool::new(false),
            runtime_signal: Mutex::new(None),
            default_request_timeout_ms: Mutex::new(None),
            metadata_listener: Mutex::new(None),
            sampling: Mutex::new(None),
            elicitation: Mutex::new(None),
            auth_storage_options: Mutex::new(crate::credentials::AuthStorageOptions::default()),
            oauth_runtime: Mutex::new(None),
        }
    }

    /// `this.defaultCwd`.
    #[must_use]
    pub fn default_cwd(&self) -> Option<&std::path::Path> {
        self.default_cwd.as_deref()
    }

    fn tables(&self) -> MutexGuard<'_, Tables> {
        // A poisoned table is still a correct table: every mutation here is a single map insert or
        // remove, so no unwind can leave one half-written. Recovering beats poisoning the whole
        // manager, and `clippy::unwrap_used` is denied crate-wide.
        self.tables.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ── the eight setters (§3.1) ────────────────────────────────────────────────────────────

    /// `setSamplingConfig(config)`.
    pub fn set_sampling_config(&self, sampling: Option<crate::runtime::SamplingHook>) {
        *self.sampling.lock().unwrap_or_else(PoisonError::into_inner) = sampling;
    }

    /// `setElicitationConfig(config)`.
    pub fn set_elicitation_config(
        &self,
        elicitation: Option<(crate::runtime::ElicitationMode, crate::runtime::ElicitationHook)>,
    ) {
        *self.elicitation.lock().unwrap_or_else(PoisonError::into_inner) = elicitation;
    }

    /// `setMetadataListChangedListener(listener)`.
    pub fn set_metadata_list_changed_listener(&self, listener: Option<MetadataListChangedListener>) {
        *self
            .metadata_listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = listener;
    }

    /// `setRuntimeSignal(signal)` — **`owner.signal`**, not the combined runtime signal.
    ///
    /// This doc used to cite `combineAbortSignals(owner.signal, initialSignal)` as the provenance.
    /// It is not: `init.ts:110,119` keeps the two apart — `const runtimeSignal =
    /// combineAbortSignals(owner.signal, initialSignal)` is threaded per call (`init.ts:286,388,454`)
    /// and never stored, while the setter is given `owner.signal` alone. `13a-mcp-activation.md:329`
    /// step 4 says the same.
    ///
    /// The difference is observable, because the stored token is read as a *value* and not only as
    /// a cancellation source: [`Self::remember_url_elicitation`]'s gate is
    /// `this.runtimeSignal?.aborted`, which upstream means "the owner stopped". Store the combined
    /// token instead and a fired per-call `ctx.signal` that left the owner running silently
    /// disables elicitation recording for the rest of the generation.
    ///
    /// **Not fixed here, and deliberately so:** the caller that passes the combined token is
    /// `runtime.rs:149-150,182`, which is not this unit's file. The correction there is to pass
    /// `owner.token()` and keep the combined `runtime_signal` as the per-call `cancel` argument.
    /// Filed against that file's owner; this doc now states the contract this setter expects.
    pub fn set_runtime_signal(&self, signal: Option<CancelToken>) {
        *self
            .runtime_signal
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = signal;
    }

    /// `setDefaultRequestTimeoutMs(timeoutMs)` — **normalises on the way in**
    /// (`server-manager.ts:212-214`).
    ///
    /// MEASURED: `0` and `-5` both store "no timeout"; `1234` stores `{timeout: 1234}`. Storing the
    /// raw value only when it normalises keeps [`crate::runtime::resolve_request_timeout`]'s
    /// per-server-wins rule intact without a lossy `Duration` round-trip.
    pub fn set_default_request_timeout_ms(&self, timeout_ms: Option<f64>) {
        *self
            .default_request_timeout_ms
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            normalize_request_timeout_ms(timeout_ms).and(timeout_ms);
    }

    /// `setAuthStorageOptions(options)`.
    pub fn set_auth_storage_options(&self, options: crate::credentials::AuthStorageOptions) {
        *self
            .auth_storage_options
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = options;
    }

    /// `getAuthStorageOptions()`'s stored value — the HTTP ladder (MCP-115) reads it.
    #[must_use]
    pub fn auth_storage_options(&self) -> crate::credentials::AuthStorageOptions {
        self.auth_storage_options
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `setOAuthRuntime(runtime)`.
    pub fn set_oauth_runtime(&self, runtime: Arc<crate::oauth::McpOAuthRuntime>) {
        *self
            .oauth_runtime
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(runtime);
    }

    /// The OAuth runtime the HTTP ladder authenticates through.
    #[must_use]
    pub fn oauth_runtime(&self) -> Option<Arc<crate::oauth::McpOAuthRuntime>> {
        self.oauth_runtime
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    // `setTraceConfig(settings)` is the eighth setter and is **not here**: `mcp-trace.ts` has no
    // Rust counterpart (T-10/MCP-477 — `mcp-trace.ts` has no MCP-1xx unit), so there is no
    // `McpTraceSettings` to accept and no
    // `McpTraceWriter` to memoise either. Its two behavioural consequences are named where they would
    // land — `dispose_connection` and `close_all` both flush the writer upstream, and both note the
    // gap. Adding a setter that stores a value nothing reads would be worse than the absence.

    /// `getRequestOptions(name, signal)` (`server-manager.ts:228-231`).
    ///
    /// Delegates to [`crate::runtime::build_request_options`], which owns the per-server-wins rule
    /// and its trap (an invalid per-server `requestTimeoutMs` yields *no* timeout rather than
    /// falling back to the global). MEASURED end to end against upstream: unknown server + global
    /// `1234` → `{timeout: 1234}`; per-server `0` + global `1234` → `undefined`.
    ///
    /// The `signal` half of upstream's `RequestOptions` has no representation — rmcp cancels a
    /// request by dropping its future — so it stays in the `abortable(..)` wrapper around each call.
    #[must_use]
    pub fn get_request_options(&self, name: &str) -> Option<PeerRequestOptions> {
        let definition = self.tables().connections.get(name).map(|connection| Arc::clone(connection.definition()));
        let global = *self
            .default_request_timeout_ms
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        build_request_options(definition.as_deref(), global)
    }

    // ── accessors (§3.1) ────────────────────────────────────────────────────────────────────

    /// `isConnecting(name)` = `connectPromises.has(name)`.
    #[must_use]
    pub fn is_connecting(&self, name: &str) -> bool {
        self.tables().connect_promises.contains_key(name)
    }

    /// `getConnection(name)`.
    #[must_use]
    pub fn get_connection(&self, name: &str) -> Option<Arc<ServerConnection>> {
        self.tables().connections.get(name).map(Arc::clone)
    }

    /// `getAllConnections()` — a **copy** (`new Map(this.connections)`), not the live map. MEASURED:
    /// a `close` after the snapshot leaves the snapshot at its old size.
    #[must_use]
    pub fn get_all_connections(&self) -> IndexMap<String, Arc<ServerConnection>> {
        self.tables().connections.clone()
    }

    /// `this.stopped`.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// `touch(name)`.
    pub fn touch(&self, name: &str) {
        if let Some(connection) = self.tables().connections.get(name) {
            connection.touch();
        }
    }

    /// `incrementInFlight(name)`.
    pub fn increment_in_flight(&self, name: &str) {
        if let Some(connection) = self.tables().connections.get(name) {
            connection.increment_in_flight();
        }
    }

    /// `decrementInFlight(name)`.
    pub fn decrement_in_flight(&self, name: &str) {
        if let Some(connection) = self.tables().connections.get(name) {
            connection.decrement_in_flight();
        }
    }

    /// `isIdle(name, timeoutMs)` (`server-manager.ts:1223-1228`): connected, zero in-flight, and
    /// `now - lastUsedAt > timeoutMs` — a **strict** comparison.
    ///
    /// MEASURED: a connection last used exactly 1000 ms ago is *not* idle at a 1000 ms timeout and
    /// *is* idle at 999 ms; a `closed` connection and an unknown name are both never idle.
    #[must_use]
    pub fn is_idle(&self, name: &str, timeout: Duration) -> bool {
        let Some(connection) = self.get_connection(name) else {
            return false;
        };
        if connection.status() != ConnectionStatus::Connected {
            return false;
        }
        if connection.in_flight() > 0 {
            return false;
        }
        let age = now_ms().saturating_sub(connection.last_used_at());
        u128::from(age) > timeout.as_millis()
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// connect / reconnect / close / closeAll — the state machine (MCP-100, MCP-116, MCP-125, MCP-126)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `connect`'s `finally` block (`server-manager.ts:298-301`), as a guard.
///
/// Both removals are **identity-matched**: a slot replaced by a newer attempt must not be deleted by
/// the older one finishing. Upstream compares object identity; here that is
/// [`futures::future::Shared::ptr_eq`] and [`Arc::ptr_eq`].
///
/// Running in `Drop` rather than at the end of the body is a deliberate strengthening: upstream's
/// `finally` always runs because a JS promise completes whether or not anyone awaits it, while a
/// dropped Rust future runs nothing. Without this, a caller that cancels mid-connect would leave
/// `connectPromises[name]` populated forever and `isConnecting(name)` permanently true.
///
/// The slot lives on the **detached tail** inside [`McpServerManager::connect_inner`], not in the
/// caller's frame, so it now fires when the attempt settles — which is when upstream's `finally`
/// fires — rather than when whichever caller happened to win the race unwinds.
struct AttemptSlot {
    manager: Arc<McpServerManager>,
    name: String,
    promise: ConnectFuture,
    attempt: Arc<AbortHandle>,
}

impl Drop for AttemptSlot {
    fn drop(&mut self) {
        {
            let mut tables = self.manager.tables();
            if tables
                .connect_promises
                .get(&self.name)
                .is_some_and(|current| Shared::ptr_eq(current, &self.promise))
            {
                tables.connect_promises.remove(&self.name);
            }
            if tables
                .connect_attempts
                .get(&self.name)
                .is_some_and(|current| Arc::ptr_eq(current, &self.attempt))
            {
                tables.connect_attempts.remove(&self.name);
            }
        }
        // Outside the guard, and last: `abort.rs:21-26`'s discipline is that a combination costs
        // one parked task per pair, so it must be bounded per *generation* — but `connect_inner`
        // combines once per attempt. Nothing ever cancels the attempt token on the success path, so
        // without this every connect left a joiner parked until session end. See
        // [`AbortHandle::reap`] for why it is this token and not the combined one.
        self.attempt.reap();
    }
}

/// The reap handle for one [`crate::abort::combine`] joiner task, held by **every** participant in
/// the call it belongs to.
///
/// [`McpServerManager::owned_signal`] combines the runtime signal with a *private child* of the
/// caller's token; cancelling that child ends the joiner and cannot reach the caller's token or the
/// runtime signal. The cancel must not happen while anyone is still racing against the combined
/// token, or that waiter reports a cancellation nobody asked for — so the handle is shared by
/// `Arc` between the caller's frame and the detached tail, and fires only when the last of them is
/// done.
#[derive(Debug, Default)]
struct SignalScope(Option<CancelToken>);

impl SignalScope {
    fn new(token: Option<CancelToken>) -> Arc<Self> {
        Arc::new(Self(token))
    }
}

impl Drop for SignalScope {
    fn drop(&mut self) {
        if let Some(token) = self.0.take() {
            token.cancel();
        }
    }
}

/// `reconnect`'s `.finally()` (`server-manager.ts:328-332`), same identity discipline.
struct ReconnectSlot {
    manager: Arc<McpServerManager>,
    name: String,
    promise: ConnectFuture,
}

impl Drop for ReconnectSlot {
    fn drop(&mut self) {
        let mut tables = self.manager.tables();
        if tables
            .reconnect_promises
            .get(&self.name)
            .is_some_and(|current| Shared::ptr_eq(current, &self.promise))
        {
            tables.reconnect_promises.remove(&self.name);
        }
    }
}

impl McpServerManager {
    /// `combineAbortSignals(this.runtimeSignal, signal)`, plus the handle that reaps the forwarder
    /// task the combination costs.
    ///
    /// Upstream returns `undefined` when both are absent and the *same* signal (identity preserved)
    /// when only one is; [`crate::abort::combine`] reproduces the second, and the first becomes a
    /// token that never fires, which is what an absent `AbortSignal` is.
    ///
    /// The only case that allocates a joiner task is "runtime **and** caller", and that is the case
    /// this feeds a **private child** of the caller's token rather than the caller's token itself.
    /// The child fires the joiner's second `select!` arm exactly the same way (it is cancelled with
    /// its parent), and cancelling it to reap the task can reach neither the caller's token nor the
    /// runtime signal — which is what makes the reap safe at all. Cancelling `combine`'s *result*
    /// would not be: it degenerates to one of its parents whenever the other is absent or already
    /// cancelled, so that cancel could stop the whole session. See [`SignalScope`].
    fn owned_signal(&self, cancel: Option<&CancelToken>) -> (CancelToken, Arc<SignalScope>) {
        let runtime = self
            .runtime_signal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match (runtime, cancel) {
            (Some(runtime), Some(cancel)) => {
                let scope = cancel.child_token();
                let combined = crate::abort::combine(&runtime, Some(&scope));
                (combined, SignalScope::new(Some(scope)))
            }
            (Some(runtime), None) => (crate::abort::combine(&runtime, None), SignalScope::new(None)),
            (None, Some(cancel)) => (cancel.clone(), SignalScope::new(None)),
            (None, None) => (CancelToken::new(), SignalScope::new(None)),
        }
    }

    /// `this.closeGenerations.get(name) ?? 0`.
    fn generation(&self, name: &str) -> u64 {
        self.tables()
            .close_generations
            .get(name)
            .copied()
            .unwrap_or(0)
    }

    /// `abortable(promise, ownedSignal)` over a shared attempt.
    async fn race<T: Clone>(
        future: impl std::future::Future<Output = ManagerResult<T>>,
        token: &CancelToken,
    ) -> ManagerResult<T> {
        match abortable(future, token).await {
            Ok(inner) => inner,
            Err(aborted) => Err(ManagerError::mcp(aborted)),
        }
    }

    /// `connect(name, definition, signal)` (`server-manager.ts:257-303`).
    ///
    /// The order of the eight steps is the specification and every one of them is load-bearing:
    /// disabled → stopped → owned signal → **await a pending close** → dedupe → reuse a connected
    /// record → carry `credentialsInvalidated` forward → capture the generation, register the
    /// attempt, and re-check both fences after the attempt resolves.
    ///
    /// # Errors
    ///
    /// [`server_disabled_message`], [`MANAGER_CLOSED`], whatever the [`ConnectionFactory`] raises,
    /// the attempt's abort reason ([`connection_closed_reason`]) when a `close` won the race, or
    /// [`connection_closed_while_connecting`] when only the generation moved.
    pub async fn connect(
        self: &Arc<Self>,
        name: &str,
        definition: &ServerEntry,
        cancel: Option<&CancelToken>,
    ) -> McpResult<Arc<ServerConnection>> {
        to_mcp(self.connect_inner(name, definition, cancel).await)
    }

    async fn connect_inner(
        self: &Arc<Self>,
        name: &str,
        definition: &ServerEntry,
        cancel: Option<&CancelToken>,
    ) -> ManagerResult<Arc<ServerConnection>> {
        // MEASURED: both guards fire before anything else, including before the single-flight map is
        // consulted, and `connect` and `reconnect` carry the identical two strings.
        if definition.is_disabled() {
            return Err(ManagerError::other(server_disabled_message(name)));
        }
        if self.is_stopped() {
            return Err(ManagerError::other(MANAGER_CLOSED));
        }

        let (owned, owned_scope) = self.owned_signal(cancel);
        throw_if_aborted(&owned, None).map_err(ManagerError::mcp)?;

        // `const closing = this.closePromises.get(name); if (closing) await abortable(closing, …)`.
        // MEASURED: a connect issued while a close is disposing does not create anything until the
        // dispose resolves — without this a reconnect could hand back a child the close is killing.
        let closing = self
            .tables()
            .close_promises
            .get(name)
            .map(|(_, future)| future.clone());
        if let Some(closing) = closing {
            Self::race(closing, &owned).await?;
        }
        throw_if_aborted(&owned, None).map_err(ManagerError::mcp)?;

        // ── hoisted ABOVE the critical section: everything that locks, spawns or awaits ────────
        //
        // The single-flight check and its matching insert have to be ONE critical section. They
        // were not: the read dropped its `MutexGuard` at the end of its own statement and the
        // insert was sixty lines later, with no lock held across the gap — and no `.await` in it
        // either, which is exactly why it read as safe. OS preemption needs no yield point.
        // MEASURED before this change, 20 rounds × 64 racers on a multi-thread runtime: 2
        // `createConnection` calls in 12 rounds and 3 in one; upstream gives 1, and gets it for
        // free because `connectPromises.has` and `connectPromises.set` are separated by pure
        // synchronous code on a single-threaded event loop.
        //
        // Two things in that window must NOT move inside the guard: `abort::combine` spawns a
        // joiner task (`abort.rs:81`), and `default_request_timeout_ms` is a second mutex, which
        // nested inside the table lock would invent a lock-ordering edge this file's `Tables` doc
        // promises never exists. So both happen here, unconditionally, before the guard is taken.
        // `credentialsInvalidated` is the one input that genuinely depends on the table read, and
        // it is only ever used to build a future — pure construction, safe under the guard.
        let attempt = AbortHandle::new();
        // `combineAbortSignals(ownedSignal, attemptController.signal)`.
        let attempt_signal = crate::abort::combine(&owned, Some(attempt.token()));
        let definition = Arc::new(definition.clone());
        let global_timeout = *self
            .default_request_timeout_ms
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // §3.2: computed **once**, before any transport is built, and reused for the connect and
        // all three discovery list calls.
        let request_options = build_request_options(Some(&definition), global_timeout);
        //
        // MCP-133 SEAM — `enrichHttpConnectionError` (13c:1608, with MCP-132's `probeMcpEndpoint`).
        // Upstream wraps this attempt for URL servers only: `definition.url ? attempt.catch(err =>
        // enrichHttpConnectionError(...)) : attempt`, producing `<original> — probe:
        // <classification>` with the original as `cause`, and swallowing **any** probe failure. The
        // wrapper belongs exactly here, around the future built below, and is not ported: it needs
        // `mcp-probe.ts`'s classifier (MCP-132), which no unit in this crate supplies yet. Named
        // rather than silently dropped — the plan calls the eight steps of this function the
        // specification, and this is a ninth element of step 9 that is absent.

        /// What the one critical section below decided.
        enum Step {
            /// `connectPromises.has(name)` — dedupe onto the in-flight attempt.
            Dedupe(ConnectFuture),
            /// A `connected` record is already in the map.
            Reuse(Arc<ServerConnection>),
            /// This caller won: it owns the attempt that was just registered.
            Start {
                promise: ConnectFuture,
                generation: u64,
            },
        }

        // ── the critical section ──────────────────────────────────────────────────────────────
        //
        // Closed, and here is why it is closed: it reads `connect_promises`, reads `connections`,
        // snapshots `close_generations` and performs BOTH inserts under one guard, and takes no
        // other lock and reaches no `.await` while it holds it. `self.generation(name)` is
        // deliberately not called — it re-enters `self.tables()` and would deadlock on the guard
        // already held — so the generation is read straight off this guard instead. Nothing here
        // can run a `Drop` that re-enters either: every `Arc<ServerConnection>` cloned out of
        // `connections` leaves the map's own strong reference behind, so no refcount reaches zero.
        let step = {
            let mut tables = self.tables();
            if let Some(pending) = tables.connect_promises.get(name).cloned() {
                Step::Dedupe(pending)
            } else if let Some(existing) = tables
                .connections
                .get(name)
                .filter(|existing| existing.status() == ConnectionStatus::Connected)
                .map(Arc::clone)
            {
                Step::Reuse(existing)
            } else {
                // MCP-116 step 7: `existing?.status === "needs-auth" && existing.credentialsInvalidated`.
                // MEASURED: three consecutive connects against a permanent-401 fixture call
                // `createConnection` with `false, true, true` — the flag rides on the connection
                // record and is fed back in, so the credential cache is discarded at most once per
                // episode.
                let credentials_invalidated = tables.connections.get(name).is_some_and(|existing| {
                    existing.status() == ConnectionStatus::NeedsAuth
                        && existing.credentials_invalidated()
                });
                let generation = tables.close_generations.get(name).copied().unwrap_or(0);

                let promise: ConnectFuture = {
                    let factory = Arc::clone(&self.factory);
                    let request = CreateConnection {
                        name: name.to_string(),
                        definition: Arc::clone(&definition),
                        attempt: attempt_signal.clone(),
                        request: owned.clone(),
                        credentials_invalidated,
                        request_options,
                    };
                    let definition_for_record = Arc::clone(&definition);
                    async move {
                        let created = factory.create(request).await.map_err(ManagerError::mcp)?;
                        Ok(ServerConnection::new(
                            definition_for_record,
                            created.resource,
                            created.status,
                            created.credentials_invalidated,
                        ))
                    }
                    .boxed()
                    .shared()
                };

                tables
                    .connect_promises
                    .insert(name.to_string(), promise.clone());
                tables
                    .connect_attempts
                    .insert(name.to_string(), Arc::clone(&attempt));
                Step::Start {
                    promise,
                    generation,
                }
            }
        };

        let (promise, generation) = match step {
            // `if (this.connectPromises.has(name)) return abortable(this.connectPromises.get(name)!, …)`.
            //
            // Note what a deduped caller receives: the **raw attempt**, not the winner's body's
            // result. It therefore skips the generation fence and the `connections.set` below —
            // upstream's shape, reproduced deliberately. What makes that safe is that the winner's
            // body always runs; see the detached tail below.
            Step::Dedupe(pending) => {
                // This caller's attempt was never registered, so nothing will ever fire its
                // handle — reap `attempt_signal`'s joiner here or it parks until session end.
                attempt.reap();
                return Self::race(pending, &owned).await;
            }
            Step::Reuse(existing) => {
                attempt.reap();
                existing.touch();
                return Ok(existing);
            }
            Step::Start {
                promise,
                generation,
            } => (promise, generation),
        };

        // ── the winner's body, detached ───────────────────────────────────────────────────────
        //
        // Upstream's `connect` body always reaches its generation fence and its
        // `this.connections.set(name, connection)`, because a JS promise completes whether or not
        // anyone is still awaiting it. A dropped Rust future runs nothing, and the consequence was
        // MEASURED: abort the winner's task while it is parked on `promise.await` with one deduped
        // caller live, and the deduped caller receives a working connection while
        // `get_connection(name)` is `None` and `close_all()` returns `Ok` having disposed nothing —
        // with a real stdio server, a child outliving the session.
        //
        // So the tail runs on its own task and the caller awaits its `JoinHandle`. Cancelling the
        // *waiter* now cancels only the wait. `AttemptSlot` moves in with it, which also makes the
        // `finally` fire when the attempt settles rather than when this frame unwinds — upstream's
        // ordering exactly. `owned_scope` moves in too, so `abort::combine`'s joiner is reaped only
        // once the last participant is finished.
        let tail = {
            let manager = Arc::clone(self);
            let name = name.to_string();
            let attempt = Arc::clone(&attempt);
            let promise = promise.clone();
            let owned_scope = Arc::clone(&owned_scope);
            async move {
                let _owned_scope = owned_scope;
                // The `finally`. Held for the rest of the tail; see `AttemptSlot`.
                let _slot = AttemptSlot {
                    manager: Arc::clone(&manager),
                    name: name.clone(),
                    promise: promise.clone(),
                    attempt: Arc::clone(&attempt),
                };

                // Upstream awaits the raw promise here, NOT `abortable(...)`: the winner of the
                // race must reach its own generation check even if its caller's signal fired,
                // otherwise the connection it created is never disposed.
                let connection = promise.await?;

                if attempt.is_aborted() || manager.generation(&name) != generation {
                    // `await this.disposeConnection(connection)` — and its rejection propagates,
                    // which is what makes a failed teardown a cleanup failure rather than a silent
                    // leak.
                    manager.dispose_registered(&connection).await?;
                    // `throwIfAborted(attemptSignal)` — MEASURED to run *before* the generation
                    // message, so a plain `close` racing a connect surfaces
                    // `MCP connection <name> was closed`.
                    if attempt.is_aborted() {
                        return Err(ManagerError::mcp(McpError::Aborted(
                            attempt.reason().map_or_else(
                                || connection_closed_reason(&name),
                                |reason| reason.to_string(),
                            ),
                        )));
                    }
                    if attempt_signal.is_cancelled() {
                        return Err(ManagerError::mcp(McpError::Aborted(
                            crate::abort::ABORTED_FALLBACK_REASON.to_string(),
                        )));
                    }
                    return Err(ManagerError::other(connection_closed_while_connecting(&name)));
                }

                // MEASURED divergence, deliberate: upstream overwrites a `closed`/`needs-auth`
                // predecessor here **without disposing it** (`disposedOld=0`). For a `closed`
                // record that is harmless — it was disposed by the `close` that closed it — but a
                // `needs-auth` record still owns the client and transport its 401 came back on, and
                // upstream simply drops the reference. Here `ServerConnection`'s drop-net closes it
                // instead, which is the whole point of MCP-131.
                manager
                    .tables()
                    .connections
                    .insert(name.clone(), Arc::clone(&connection));
                Ok(connection)
            }
        };

        // Off a tokio runtime there is nothing to detach onto (`tokio::spawn` panics there and this
        // crate denies `clippy::panic`); running the tail inline is the pre-existing behaviour and
        // is strictly better than refusing to connect.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return tail.await;
        };
        match handle.spawn(tail).await {
            Ok(outcome) => outcome,
            Err(join) => Err(ManagerError::other(format!(
                "MCP connect for {name} did not finish: {join}"
            ))),
        }
    }
}

impl McpServerManager {
    /// `reconnect(name, definition, staleConnection, signal)` (`server-manager.ts:313-341`) —
    /// **MCP-125**.
    ///
    /// MEASURED, all four:
    ///
    /// * three concurrent callers racing the same stale handle produce exactly **one** close and
    ///   **one** create, and all three receive the same fresh connection;
    /// * the fresh connection's `inFlight` is raised to the stale one's (`4` in, `4` out), which is
    ///   what stops the idle sweep from closing a server whose callers are still waiting;
    /// * a stale handle the map no longer holds is **not** torn down — the current connection is
    ///   returned untouched, and with an empty map a plain `connect` runs instead;
    /// * a disabled definition raises before any teardown: the stale connection is still in the map
    ///   afterwards.
    ///
    /// # Errors
    ///
    /// [`server_disabled_message`], [`MANAGER_CLOSED`], or whatever the close/connect pair raises.
    pub async fn reconnect(
        self: &Arc<Self>,
        name: &str,
        definition: &ServerEntry,
        stale: &ConnectionHandle,
        cancel: Option<&CancelToken>,
    ) -> McpResult<Arc<ServerConnection>> {
        to_mcp(self.reconnect_inner(name, definition, stale, cancel).await)
    }

    async fn reconnect_inner(
        self: &Arc<Self>,
        name: &str,
        definition: &ServerEntry,
        stale: &ConnectionHandle,
        cancel: Option<&CancelToken>,
    ) -> ManagerResult<Arc<ServerConnection>> {
        // Both guards are load-bearing and are *not* inherited from `connect`: a reconnect on a
        // just-disabled server must fail **before any teardown happens** (§3.11).
        if definition.is_disabled() {
            return Err(ManagerError::other(server_disabled_message(name)));
        }
        if self.is_stopped() {
            return Err(ManagerError::other(MANAGER_CLOSED));
        }
        let (owned, owned_scope) = self.owned_signal(cancel);
        throw_if_aborted(&owned, None).map_err(ManagerError::mcp)?;

        // Hoisted above the critical section for the same reason as in `connect_inner`: the clones
        // the shared future captures are allocations, but nothing here may lock or await.
        let definition_owned = definition.clone();
        let stale_owned = Arc::clone(stale);

        /// What the one critical section below decided.
        enum Step {
            /// `reconnectPromises.get(name)` — dedupe onto the in-flight reconnect.
            Dedupe(ConnectFuture),
            /// This caller won: it owns the reconnect that was just registered.
            Start(ConnectFuture),
        }

        // ── the critical section ──────────────────────────────────────────────────────────────
        //
        // Same defect, same shape, three statements narrower: the read at the top and the insert at
        // the bottom used to sit in two separate guards, so two racing reconnects could both build
        // one and the second overwrite the first. Closed because the read, the future's
        // construction (pure — `async move {…}.boxed().shared()` never awaits) and the insert are
        // one guard, and nothing inside takes a second lock.
        let step = {
            let mut tables = self.tables();
            if let Some(existing) = tables.reconnect_promises.get(name).cloned() {
                Step::Dedupe(existing)
            } else {
                let promise: ConnectFuture = {
                    let manager = Arc::clone(self);
                    let name = name.to_string();
                    let owned = owned.clone();
                    async move {
                        manager
                            .do_reconnect(&name, &definition_owned, &stale_owned, &owned)
                            .await
                    }
                    .boxed()
                    .shared()
                };
                tables
                    .reconnect_promises
                    .insert(name.to_string(), promise.clone());
                Step::Start(promise)
            }
        };

        // `const inFlight = this.reconnectPromises.get(name); if (inFlight) return abortable(...)`.
        let promise = match step {
            Step::Dedupe(existing) => return Self::race(existing, &owned).await,
            Step::Start(promise) => promise,
        };

        // ── the reconnect runs detached; `race` only decides what THIS caller observes ─────────
        //
        // `Self::race` is `abortable(fut, token)`, whose cancel arm **drops** `fut`. Every clone of
        // the shared future died with this frame, so an owner stop during a `/mcp reconnect` left
        // `do_reconnect` dropped at whatever await it had reached — after `close_inner` had already
        // done `connections.shift_remove(name)`. MEASURED against upstream: the caller sees the
        // rejection either way, but upstream ends up **connected** (`doReconnect` is invoked at
        // `server-manager.ts:328`, so it is a live promise and `abortable` only decides what the
        // caller sees) while this port ended up closed and not reconnected.
        //
        // The driver is what restores that. `ReconnectSlot` moves into it, so the identity-matched
        // `finally` fires when the reconnect settles rather than when this frame unwinds; and the
        // `owned_scope` handle is shared with it, so `abort::combine`'s joiner is reaped only after
        // the last participant is done — reaping it earlier would cancel `owned` and make the
        // `race` below report a cancellation nobody asked for.
        let driver = {
            let manager = Arc::clone(self);
            let name = name.to_string();
            let promise = promise.clone();
            let owned_scope = Arc::clone(&owned_scope);
            async move {
                let _owned_scope = owned_scope;
                let _slot = ReconnectSlot {
                    manager,
                    name,
                    promise: promise.clone(),
                };
                let _ = promise.await;
            }
        };
        let spawned = tokio::runtime::Handle::try_current()
            .ok()
            .map(|handle| handle.spawn(driver));
        // Off a runtime there is no detached driver, so this frame keeps the `finally` — the
        // pre-existing behaviour, degraded exactly where `tokio::spawn` is unavailable.
        let _fallback_slot = spawned.is_none().then(|| ReconnectSlot {
            manager: Arc::clone(self),
            name: name.to_string(),
            promise: promise.clone(),
        });

        // Dropping the `JoinHandle` detaches the task rather than cancelling it, which is the
        // whole point: the reconnect finishes whatever this caller does.
        Self::race(promise, &owned).await
    }

    /// `doReconnect` (`server-manager.ts:419-440`).
    ///
    /// The identity test is the whole function: *"never tear down a connection we didn't prove
    /// stale"*. Upstream compares objects with `!==`; here the caller's handle is an
    /// `Arc<dyn ServerConnectionRef>` and the map holds `Arc<ServerConnection>`, so the comparison
    /// is [`Arc::ptr_eq`] against the *upcast* of the current record. That is sound because
    /// `Arc::ptr_eq` compares data addresses and ignores `dyn` metadata, so the upcast preserves
    /// identity exactly — and a scripted fake handle simply never matches a real connection, which
    /// is the correct answer for it.
    async fn do_reconnect(
        self: &Arc<Self>,
        name: &str,
        definition: &ServerEntry,
        stale: &ConnectionHandle,
        owned: &CancelToken,
    ) -> ManagerResult<Arc<ServerConnection>> {
        throw_if_aborted(owned, None).map_err(ManagerError::mcp)?;
        let current = self.tables().connections.get(name).map(Arc::clone);

        let Some(current) = current else {
            // `return current ?? this.connect(name, definition, signal)` with `current` absent.
            // MEASURED: an empty map runs a plain connect rather than failing.
            return self.connect_inner(name, definition, Some(owned)).await;
        };
        let current_handle: ConnectionHandle = Arc::clone(&current) as ConnectionHandle;
        if !Arc::ptr_eq(&current_handle, stale) {
            // Someone else already reconnected (or connected) first. Return theirs, untouched.
            return Ok(current);
        }

        let stale_in_flight = current.in_flight();
        self.close_inner(name).await?;
        let fresh = self.connect_inner(name, definition, Some(owned)).await?;
        fresh.raise_in_flight_to(stale_in_flight);
        Ok(fresh)
    }

    /// `close(name)` (`server-manager.ts:1096-1131`) — **MCP-126**.
    ///
    /// # Errors
    ///
    /// A teardown failure ([`CONNECTION_CLEANUP_FAILED`]), or — on the no-connection path — a
    /// pending connect's failure re-raised **only** when it is itself a cleanup failure. MEASURED:
    /// an ordinary connect failure during a close is swallowed; an
    /// `AggregateError(_, "MCP connection cleanup failed")` is re-thrown.
    pub async fn close(self: &Arc<Self>, name: &str) -> McpResult<()> {
        to_mcp(self.close_inner(name).await)
    }

    /// A monotonic ticket identifying one teardown. See [`Tables::close_promises`].
    fn close_ticket(&self) -> u64 {
        static TICKETS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        TICKETS.fetch_add(1, Ordering::SeqCst)
    }

    /// `disposeConnection`, registered in [`Tables::tail_disposes`] for the duration so
    /// [`Self::close_all_inner`]'s drain can see it.
    ///
    /// Used by `connect_inner`'s tail, which disposes a connection belonging to a generation that
    /// has already been closed. Registering is the whole point: an unregistered dispose is
    /// invisible to `close_all`, which then returns `Ok` over a live child.
    async fn dispose_registered(self: &Arc<Self>, connection: &Arc<ServerConnection>) -> ManagerResult<()> {
        let ticket = self.close_ticket();
        let disposing: CloseFuture = {
            let manager = Arc::clone(self);
            let connection = Arc::clone(connection);
            async move {
                let result = manager.dispose_connection(&connection).await;
                // Deregister from inside the future, so it happens whether the future was awaited
                // to completion or driven by the drain.
                manager.tables().tail_disposes.remove(&ticket);
                result
            }
            .boxed()
            .shared()
        };
        self.tables().tail_disposes.insert(ticket, disposing.clone());
        disposing.await
    }

    async fn close_inner(self: &Arc<Self>, name: &str) -> ManagerResult<()> {
        // Guard 1 and guard 2, in one critical section and before anything can await: the
        // generation bump and the attempt abort must not be separable, or a connect that resolves
        // between them slips through both fences.
        let attempt = {
            let mut tables = self.tables();
            let generation = tables.close_generations.entry(name.to_string()).or_insert(0);
            *generation = generation.saturating_add(1);
            tables.pending_metadata_publications.remove(name);
            tables.connect_attempts.get(name).map(Arc::clone)
        };
        if let Some(attempt) = attempt {
            attempt.abort(connection_closed_reason(name));
        }

        // Hoisted above the critical section: a monotonic counter, but the discipline is the same —
        // nothing but table reads and writes happens under the guard below.
        let ticket = self.close_ticket();

        /// What the one critical section below decided.
        enum Step {
            /// A live record was taken; this caller owns its teardown.
            Dispose(CloseFuture),
            /// `closePromises.get(name)` — someone else is already tearing this one down.
            AwaitPendingClose(CloseFuture),
            /// No record and no teardown, but a connect is in flight.
            AwaitPendingConnect(ConnectFuture),
            /// Nothing to do.
            Nothing,
        }

        // ── the critical section ──────────────────────────────────────────────────────────────
        //
        // "Delete before awaiting SDK cleanup so a replacement cannot be removed by an old close
        // operation finishing later" — and the read, the status flip, both removals and the
        // `close_promises` insert have to be ONE guard to mean it. They were not: the read dropped
        // its guard at the end of its statement and the removal was in a separate one, so two
        // closers both saw the same live connection. MEASURED: 5 of 20 rounds started a
        // `createConnection` mid-teardown, because the second closer's `dispose` short-circuited on
        // the already-taken flag, completed instantly, matched its own ticket and deleted
        // `close_promises[name]` while the first was still inside the transport teardown — at which
        // point a `connect` finds nothing to wait for. Upstream, 32 concurrent closes gave 0: its
        // second `close` falls into the `pendingClose` branch and waits, which is what makes
        // `dispose`'s once-only flag a backstop rather than the primary mechanism.
        //
        // Closed because nothing inside takes a second lock (the ticket is minted above, the
        // teardown future is pure construction, and the `tokio::spawn` that drives it happens after
        // the guard is released) and nothing inside awaits. `connections.shift_remove` cannot run a
        // `ServerConnection::drop` under the guard either: the clone taken one line earlier holds
        // the record alive.
        let step = {
            let mut tables = self.tables();
            match tables.connections.get(name).map(Arc::clone) {
                None => {
                    // `const pendingClose = this.closePromises.get(name); if (pendingClose) { await …; return }`
                    // — note the absence of a `catch`: a rejected pending close propagates.
                    // MEASURED: two concurrent `close` calls on one live connection dispose exactly
                    // once.
                    if let Some((_, pending)) = tables.close_promises.get(name) {
                        Step::AwaitPendingClose(pending.clone())
                    // The pending-connect rethrow. Its guard is `error.is_cleanup_failure()` on an
                    // error produced by the external [`ConnectionFactory`], which returns
                    // [`McpError`] and reaches here as `ManagerError::Mcp(..)`, so the test is
                    // `McpError::is_cleanup_failure` — which since MCP-124 matches all seven
                    // aggregates including the [`CONNECTION_SETUP_FAILED`] one `createConnection`
                    // raises. The MECHANISM is therefore live, and it is proved end to end through
                    // the real manager by `a_pending_connect_that_failed_cleanup_is_rethrown_by_
                    // close`. What is still missing is a *production* producer of `SetupFailed`
                    // against a real server: `ConnectionBuilder::post_handshake` has one narrow one
                    // (an abort racing a successful handshake whose own teardown then fails), and
                    // discovery — upstream's producer, MCP-119 — has not landed.
                    } else if let Some(pending) = tables.connect_promises.get(name).cloned() {
                        Step::AwaitPendingConnect(pending)
                    } else {
                        Step::Nothing
                    }
                }
                Some(connection) => {
                    connection.set_status(ConnectionStatus::Closed);
                    tables.connections.shift_remove(name);
                    tables.accepted_url_elicitations.remove(name);

                    let closing: CloseFuture = {
                        let manager = Arc::clone(self);
                        let name = name.to_string();
                        async move {
                            let result = manager.dispose_connection(&connection).await;
                            // `.finally(() => { if (this.closePromises.get(name) === closing) delete })`,
                            // by ticket rather than by promise identity. Inside the future, and the
                            // future is driven by a detached task, so it runs even when every
                            // waiter has gone.
                            let mut tables = manager.tables();
                            if tables
                                .close_promises
                                .get(&name)
                                .is_some_and(|(current, _)| *current == ticket)
                            {
                                tables.close_promises.remove(&name);
                            }
                            result
                        }
                        .boxed()
                        .shared()
                    };
                    tables
                        .close_promises
                        .insert(name.to_string(), (ticket, closing.clone()));
                    Step::Dispose(closing)
                }
            }
        };

        let closing = match step {
            Step::Nothing => return Ok(()),
            Step::AwaitPendingClose(pending) => return pending.await,
            Step::AwaitPendingConnect(pending) => {
                if let Err(error) = pending.await
                    && error.is_cleanup_failure()
                {
                    return Err(error);
                }
                return Ok(());
            }
            Step::Dispose(closing) => closing,
        };

        // ── the teardown runs detached; the caller only waits for it ──────────────────────────
        //
        // `futures::future::Shared` has no executor of its own. With the awaiting clone gone and
        // only the map's inert clone left, a teardown simply stops making progress — and the
        // consequence was MEASURED with a real child: `close("s")` in a task, aborted at 200 ms,
        // left the pid alive at +5 s (well past rmcp's 3 s hard-kill window) and `close_all()`
        // then returned `Ok` with the child still running, because `close_all` iterates
        // `connections ∪ connect_promises` and this name had already been removed from both.
        //
        // A driver task fixes it at the root: the kill is no longer something a *waiter* can
        // cancel. Upstream gets this free — a JS promise runs to completion whether or not anyone
        // awaits it — and this is the file's own header claim ("a Rust future that nobody polls
        // simply stops") applied to the teardown instead of only to the map bookkeeping.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let driving = closing.clone();
            handle.spawn(async move {
                let _ = driving.await;
            });
        }
        closing.await
    }

    /// `disposeConnection(connection)` (`server-manager.ts:1133-1140`).
    ///
    /// `Promise.allSettled([client.close(), traceWriter?.flush()])` — only `client.close()` is
    /// called, because the client owns its transport. The trace-writer flush has no counterpart:
    /// `mcp-trace.ts` is **T-10/MCP-477**, unported, so there is nothing to flush. Named rather
    /// than silently dropped, because its absence changes shutdown ordering once it lands.
    async fn dispose_connection(&self, connection: &Arc<ServerConnection>) -> ManagerResult<()> {
        let mut failures = Vec::new();
        if let Err(error) = connection.dispose().await {
            failures.push(ManagerError::mcp(error));
        }
        if failures.is_empty() {
            return Ok(());
        }
        Err(ManagerError::aggregate(CONNECTION_CLEANUP_FAILED, failures))
    }

    /// `closeAll()` (`server-manager.ts:1142-1176`) — **MCP-126**.
    ///
    /// The late sweep is the subtle half: a connect that resolved *during* the first sweep inserted
    /// itself into `connections` after the snapshot was taken, so the map is re-read and closed
    /// again. MEASURED end to end — an in-flight connect plus one live connection leaves the map
    /// empty and disposes both.
    ///
    /// # Errors
    ///
    /// [`MANAGER_CLEANUP_FAILED`] carrying only genuine teardown failures; ordinary connect failures
    /// are expected during shutdown and are filtered out.
    pub async fn close_all(self: &Arc<Self>) -> McpResult<()> {
        to_mcp(self.close_all_inner().await)
    }

    async fn close_all_inner(self: &Arc<Self>) -> ManagerResult<()> {
        self.stopped.store(true, Ordering::SeqCst);

        // `new Set([...connections.keys(), ...connectPromises.keys()])`, then bump + abort each.
        let (attempts, pending_connects, current_names) = {
            let tables = self.tables();
            let mut names: Vec<String> = tables.connections.keys().cloned().collect();
            for name in tables.connect_promises.keys() {
                if !names.iter().any(|existing| existing == name) {
                    names.push(name.clone());
                }
            }
            let attempts: Vec<(String, Option<Arc<AbortHandle>>)> = names
                .iter()
                .map(|name| (name.clone(), tables.connect_attempts.get(name).map(Arc::clone)))
                .collect();
            let pending: Vec<ConnectFuture> = tables.connect_promises.values().cloned().collect();
            let current: Vec<String> = tables.connections.keys().cloned().collect();
            drop(tables);
            (attempts, pending, current)
        };
        {
            let mut tables = self.tables();
            for (name, _) in &attempts {
                let generation = tables.close_generations.entry(name.clone()).or_insert(0);
                *generation = generation.saturating_add(1);
            }
        }
        for (name, attempt) in &attempts {
            if let Some(attempt) = attempt {
                attempt.abort(connection_closed_reason(name));
            }
        }

        let mut failures: Vec<Arc<ManagerError>> = Vec::new();
        // `await Promise.allSettled(pendingConnects)` — never `try_join`, which would abandon the
        // siblings on the first failure and leave their children running.
        for result in futures::future::join_all(pending_connects).await {
            if let Err(error) = result
                && error.is_cleanup_failure()
            {
                failures.push(error);
            }
        }
        for result in
            futures::future::join_all(current_names.iter().map(|name| self.close_inner(name))).await
        {
            if let Err(error) = result
                && error.is_cleanup_failure()
            {
                failures.push(error);
            }
        }

        // The late sweep.
        let late_names: Vec<String> = self.tables().connections.keys().cloned().collect();
        for result in
            futures::future::join_all(late_names.iter().map(|name| self.close_inner(name))).await
        {
            if let Err(error) = result
                && error.is_cleanup_failure()
            {
                failures.push(error);
            }
        }

        // And a drain of `close_promises`. **A deliberate strengthening over upstream**, which
        // never reads that map in `closeAll` and does not need to: a JS promise runs to completion
        // whether or not anyone awaits it, so a teardown another caller started is already
        // guaranteed to finish. Here `close_inner` removes the record from `connections` before it
        // awaits, so a teardown in flight when shutdown began is invisible to both sweeps above —
        // and MCP-126's *verify* is "zero surviving child processes by process-table check", which
        // is a claim about when `closeAll` **returns**. Bounded: each entry is one
        // `graceful_shutdown`, itself capped by rmcp's 3-second window.
        //
        // And the tail disposes. A connect attempt that resolves *after* its generation was bumped
        // disposes its own connection inside `connect_inner`'s tail; that dispose is keyed by
        // ticket rather than by name, so it needs draining separately.
        //
        // This is NOT the residual it was once documented as. That note argued the omission was
        // upstream-faithful because `closeAll` awaits `connectPromises` — `createConnection`'s
        // promise — rather than the `connect` body that disposes its result. The shape matches; the
        // outcome does not. In node that body is a live promise which runs to completion whether or
        // not anyone holds it, so the child dies; a dropped Rust future runs nothing. MEASURED
        // before this drain existed: `close_all()` returned `Ok` with the child still alive, and the
        // child then survived the runtime being dropped.
        let draining: Vec<CloseFuture> = {
            let tables = self.tables();
            tables
                .close_promises
                .values()
                .map(|(_, future)| future.clone())
                .chain(tables.tail_disposes.values().cloned())
                .collect()
        };
        for result in futures::future::join_all(draining).await {
            if let Err(error) = result
                && error.is_cleanup_failure()
            {
                failures.push(error);
            }
        }

        {
            let mut tables = self.tables();
            tables.accepted_url_elicitations.clear();
            tables.pending_metadata_publications.clear();
        }
        // "so a late callback cannot re-enter a dead runtime."
        *self.sampling.lock().unwrap_or_else(PoisonError::into_inner) = None;
        *self
            .elicitation
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        // `await this.traceWriter?.flush()` last — T-10/MCP-477, unported. See `dispose_connection`.

        if failures.is_empty() {
            return Ok(());
        }
        Err(ManagerError::aggregate(MANAGER_CLEANUP_FAILED, failures))
    }
}

impl McpServerManager {
    /// `publishMetadataChanged(name, expectedConnection, reason)` (`server-manager.ts:185-203`).
    ///
    /// Identity-guarded: a late publication from a replaced connection is dropped. A listener that
    /// **throws** does not fail the caller — the publication is queued in
    /// `pendingMetadataPublications` and retried by the next successful refresh, which is what keeps
    /// a broken metadata write from being mistaken for a healthy connection.
    ///
    /// Rust has no `throw` here: the listener is `Fn(&str, &str)` and a panic would abort under this
    /// crate's lint policy, so the queue-on-failure arm is reachable only through
    /// [`Self::queue_metadata_publication`]. The identity guard, the delete-on-success and the
    /// `false` return are the parts that are observable, and they are exact.
    pub fn publish_metadata_changed(
        &self,
        name: &str,
        expected: &Arc<ServerConnection>,
        reason: &str,
    ) -> bool {
        let listener = {
            let tables = self.tables();
            let Some(current) = tables.connections.get(name) else {
                return false;
            };
            if !Arc::ptr_eq(current, expected) || current.status() != ConnectionStatus::Connected {
                return false;
            }
            drop(tables);
            self.metadata_listener
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        };
        // Invoked outside every lock: a listener that re-enters the manager must not deadlock.
        if let Some(listener) = listener {
            listener(name, reason);
        }
        self.tables().pending_metadata_publications.remove(name);
        true
    }

    /// The catch arm of `publishMetadataChanged`: record that this connection still owes a
    /// publication.
    pub fn queue_metadata_publication(
        &self,
        name: &str,
        connection: &Arc<ServerConnection>,
        reason: &str,
    ) {
        self.tables().pending_metadata_publications.insert(
            name.to_string(),
            PendingPublication {
                connection: Arc::clone(connection),
                reason: reason.to_string(),
            },
        );
    }

    /// `retryPendingMetadataPublication(name, connection)` (`server-manager.ts:410-417`) —
    /// identity-guarded on the *queued* connection, and the delete is identity-guarded again in case
    /// the listener re-queued.
    pub fn retry_pending_metadata_publication(&self, name: &str, connection: &Arc<ServerConnection>) {
        let (listener, reason) = {
            let tables = self.tables();
            let Some(pending) = tables.pending_metadata_publications.get(name) else {
                return;
            };
            if !Arc::ptr_eq(&pending.connection, connection) {
                return;
            }
            let reason = pending.reason.clone();
            drop(tables);
            (
                self.metadata_listener
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone(),
                reason,
            )
        };
        if let Some(listener) = listener {
            listener(name, &reason);
        }
        let mut tables = self.tables();
        if tables
            .pending_metadata_publications
            .get(name)
            .is_some_and(|pending| Arc::ptr_eq(&pending.connection, connection))
        {
            tables.pending_metadata_publications.remove(name);
        }
    }

    /// `rememberUrlElicitation(serverName, elicitationId)` (`server-manager.ts:816-824`) — a **no-op
    /// once the runtime signal has fired**, so a stale generation cannot accumulate state.
    pub fn remember_url_elicitation(&self, name: &str, elicitation_id: &str) {
        let aborted = self
            .runtime_signal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(CancelToken::is_cancelled);
        if aborted {
            return;
        }
        self.tables()
            .accepted_url_elicitations
            .entry(name.to_string())
            .or_default()
            .insert(elicitation_id.to_string());
    }

    /// `accepted?.delete(notification.params.elicitationId)` — the user is told the interaction
    /// completed **only if this returns `true`** (§3.10).
    pub fn forget_url_elicitation(&self, name: &str, elicitation_id: &str) -> bool {
        self.tables()
            .accepted_url_elicitations
            .get_mut(name)
            .is_some_and(|accepted| accepted.remove(elicitation_id))
    }

    /// Whether an elicitation id is still recorded as accepted for this server.
    #[must_use]
    pub fn has_accepted_url_elicitation(&self, name: &str, elicitation_id: &str) -> bool {
        self.tables()
            .accepted_url_elicitations
            .get(name)
            .is_some_and(|accepted| accepted.contains(elicitation_id))
    }

    /// The two preconditions and the four accounting calls that wrap every `getPrompt` /
    /// `readResource` / `tools/call` (`server-manager.ts:1057-1094`; §3.13, MCP-127).
    ///
    /// Upstream writes `touch; incrementInFlight; try { … } finally { decrementInFlight; touch }`
    /// four times over. Here the `finally` is [`InFlightGuard`]'s `Drop`, so an early `?` cannot
    /// leak a slot — the failure mode the plan's MCP-127 calls out by name, and the one that makes
    /// the idle sweep stop reaping a server forever.
    ///
    /// The call itself is **not** here: it needs the connection's `Peer`, which the
    /// [`ConnectionFactory`] does not yet produce (MCP-119's plumbing). This is the half MCP-100
    /// owns; MCP-121 supplies the two one-line bodies on top of it.
    ///
    /// # Errors
    ///
    /// [`server_disabled_message`] when the *connection's* definition is disabled — upstream reads
    /// `this.connections.get(name)?.definition`, i.e. the snapshot, not live config — and
    /// [`server_not_connected_message`] when there is no connected record.
    pub fn begin_request(
        self: &Arc<Self>,
        name: &str,
    ) -> McpResult<(Arc<ServerConnection>, InFlightGuard)> {
        let connection = self.get_connection(name);
        if connection
            .as_ref()
            .is_some_and(|connection| connection.definition().is_disabled())
        {
            return Err(McpError::Other(server_disabled_message(name)));
        }
        let Some(connection) = connection else {
            return Err(McpError::Other(server_not_connected_message(name)));
        };
        if connection.status() != ConnectionStatus::Connected {
            return Err(McpError::Other(server_not_connected_message(name)));
        }
        connection.touch();
        connection.increment_in_flight();
        Ok((
            connection,
            InFlightGuard {
                manager: Some(Arc::clone(self)),
                name: name.to_string(),
            },
        ))
    }
}

/// The `finally { this.decrementInFlight(name); this.touch(name); }` of every request path.
///
/// # By name, and why that is not the weaker choice it looks like
///
/// This held the connection by `Arc` and decremented *that record*, on the reasoning that upstream's
/// `decrementInFlight(name)` (`server-manager.ts:1213-1218`) looks the name up again and does
/// nothing when the connection is gone. That reasoning inverts the actual contract. The by-name
/// lookup is **paired** with `doReconnect`'s `fresh.inFlight = Math.max(fresh.inFlight,
/// staleInFlight)` carry-forward (MCP-125, [`ServerConnection::raise_in_flight_to`]): a request that
/// straddles a reconnect has its count carried onto the *fresh* record, and the outstanding
/// decrement is meant to land there and cancel it.
///
/// MEASURED upstream (node 22, `fafae21`, real `McpServerManager` with `createConnection` /
/// `disposeConnection` stubbed), incrementing, reconnecting, then running the `finally`:
///
/// ```text
/// after increment: a.inFlight = 1   current.inFlight = 1
/// reconnect:       same object? false | fresh.inFlight = 1 | stale a.inFlight = 1
/// after finally:   fresh.inFlight = 0 | stale a.inFlight = 1
/// ```
///
/// Decrementing the captured `Arc` instead lands on the discarded record and leaves the fresh one
/// pinned at the floor `raise_in_flight_to` set, so [`McpServerManager::is_idle`] returns `false`
/// forever and the idle sweep never reaps that server — the exact failure the carry-forward was
/// ported to avoid.
#[derive(Debug)]
pub struct InFlightGuard {
    manager: Option<Arc<McpServerManager>>,
    name: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.take() {
            manager.decrement_in_flight(&self.name);
            manager.touch(&self.name);
        }
    }
}

// =================================================================================================
// MCP-134 — `isTerminatedSession` (`session-recovery.ts:47-58`)
// =================================================================================================

/// `CONNECTION_CLOSED_PROTOCOL_CODE = -32000` (`session-recovery.ts:41`).
pub const CONNECTION_CLOSED_PROTOCOL_CODE: i32 = -32000;

/// `SERVER_NOT_INITIALIZED_MCP_MESSAGES` (`session-recovery.ts:42-45`) — **two** members, and the
/// 400 regex below matches only the longer one. MEASURED: a 400 whose body carries the short message
/// is *not* a terminated session, while a `ProtocolError` carrying it *is*.
pub const SERVER_NOT_INITIALIZED_MCP_MESSAGES: [&str; 2] =
    ["Server not initialized", "Bad Request: Server not initialized"];

/// `/"code"\s*:\s*-32000/` and `/"message"\s*:\s*"Bad Request: Server not initialized"/`.
static TERMINATED_400_MARKERS: std::sync::LazyLock<Option<(regex::Regex, regex::Regex)>> =
    std::sync::LazyLock::new(|| {
        // Both patterns are literals with one bounded `\s*`; they cannot fail to compile. `Option`
        // rather than `unwrap` because this crate denies `clippy::unwrap_used`, and a `None` fails
        // the predicate closed — a session that is not proven terminated is not retried, which is
        // the safe direction (a retry that should not have happened can double-execute a request).
        let code = regex::Regex::new(r#""code"\s*:\s*-32000"#).ok()?;
        let message =
            regex::Regex::new(r#""message"\s*:\s*"Bad Request: Server not initialized""#).ok()?;
        Some((code, message))
    });

/// The transport-level facts `isTerminatedSession` classifies, extracted from whatever error the
/// caller is holding.
///
/// Upstream branches on `instanceof SdkHttpError` / `instanceof ProtocolError`. rmcp's equivalents
/// are `StreamableHttpError` (whose `SessionExpired` variant already *is* the 404-with-session arm,
/// gate included) and `ErrorData`'s numeric code, and neither is reachable from this module without
/// the HTTP transport MCP-114/MCP-115 build. This struct is the seam: the caller supplies the two
/// facts, the predicate owns the policy, and the policy is testable today.
#[derive(Debug, Clone, Copy, Default)]
pub struct TerminatedSessionEvidence<'a> {
    /// `err instanceof SdkHttpError ? err.status : undefined`. rmcp raises
    /// `StreamableHttpError::SessionExpired` for `status == 404 && session_was_attached`, so a
    /// caller holding that variant may pass `Some(404)` directly.
    pub http_status: Option<u16>,
    /// `err instanceof ProtocolError ? err.code : undefined`.
    pub protocol_code: Option<i32>,
    /// `err.message` — the *serialised* body for the HTTP arm, the protocol error's message for the
    /// other.
    pub message: &'a str,
}

/// `isTerminatedSession(err, hadSessionId)` (`session-recovery.ts:47-58`) — **MCP-134**.
///
/// MEASURED against upstream on node 22, all fifteen cases. The four positives: 404 with a session;
/// a 400 carrying **both** markers (whitespace around the colons is flexible); a `ProtocolError`
/// `-32000` with either of the two exact messages. The negatives that matter, and which a
/// plausible-looking port gets wrong:
///
/// * `hadSessionId == false` — an absolute gate, checked first, ahead of everything.
/// * a 400 with only the `code` marker, or only the `message` marker → **false**.
/// * a 400 carrying the *short* message `"Server not initialized"` → **false** (the regex names the
///   long one only).
/// * a **500** carrying both markers → false: the status must be exactly 400.
/// * a `ProtocolError` `-32000` with any other message, or the right message under any other code →
///   false.
/// * a plain `Error`, and an `AbortError`, → false. Cancellation is never a session failure.
///
/// The two arms are disjoint: MEASURED, `SdkHttpError` is **not** an `instanceof ProtocolError` in
/// the SDK, so the protocol arm cannot catch an HTTP error that fell through the first.
#[must_use]
pub fn is_terminated_session(evidence: &TerminatedSessionEvidence<'_>, had_session_id: bool) -> bool {
    if !had_session_id {
        return false;
    }
    if let Some(status) = evidence.http_status {
        if status == 404 {
            return true;
        }
        if status != 400 {
            return false;
        }
        let Some((code, message)) = TERMINATED_400_MARKERS.as_ref() else {
            return false;
        };
        return code.is_match(evidence.message) && message.is_match(evidence.message);
    }
    evidence.protocol_code == Some(CONNECTION_CLOSED_PROTOCOL_CODE)
        && SERVER_NOT_INITIALIZED_MCP_MESSAGES.contains(&evidence.message)
}

/// `shouldReconnectAfterRefresh(error, hadSessionId)` (`lifecycle.ts:412-416`):
/// `isTerminatedSession(error, hadSessionId) || (SdkError && code ∈ {NotConnected, ConnectionClosed})`.
///
/// The second disjunct needs rmcp's `ServiceError` discriminants, which only reach this crate once a
/// live peer exists (MCP-119/MCP-120). Until then the predicate is its first disjunct alone; a
/// missed reconnect costs one health-check cycle, whereas a false positive would tear down a healthy
/// connection, so failing closed is the right direction.
#[must_use]
pub fn should_reconnect_after_refresh(
    evidence: &TerminatedSessionEvidence<'_>,
    had_session_id: bool,
) -> bool {
    is_terminated_session(evidence, had_session_id)
}

// =================================================================================================
// Tests
//
// Every behavioural assertion below was first MEASURED against upstream `McpServerManager`
// (pi-mcp-adapter v2.26.1 = `fafae21`) on node 22, with `createConnection`/`disposeConnection`
// stubbed the way `ScriptedFactory` stubs them here. Where a Rust result deliberately differs from
// the measurement, the test says so in its own name or body.
// =================================================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::atomic::AtomicI64;

    use super::*;

    // ── scripted `createConnection` ─────────────────────────────────────────────────────────

    /// A gate a test opens when it wants the in-flight attempt to resolve.
    #[derive(Clone, Debug)]
    struct Gate(tokio::sync::watch::Sender<bool>);

    impl Gate {
        fn shut() -> Self {
            Self(tokio::sync::watch::channel(false).0)
        }
        fn open(&self) {
            // `send_replace`, never `send`: `watch::Sender::send` is a **no-op** when no receiver
            // exists yet, so a gate opened before the attempt subscribes would never be seen and the
            // test would hang until nextest's 180 s tripwire. (It did, once.)
            self.0.send_replace(true);
        }
        async fn wait(&self) {
            let mut rx = self.0.subscribe();
            while !*rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    return;
                }
            }
        }
    }

    /// What one scripted attempt should do.
    #[derive(Clone, Copy)]
    enum Script {
        /// Resolve to a live connection.
        Connect,
        /// Resolve to a `needs-auth` record that has already discarded its cached credential.
        NeedsAuthInvalidated,
        /// Fail the way an ordinary connect failure fails.
        FailOrdinary,
    }

    struct ScriptedFactory {
        gate: Mutex<Option<Gate>>,
        script: Script,
        /// Every `credentialsInvalidated` value `createConnection` was called with, in order.
        calls: Mutex<Vec<bool>>,
        /// Each attempt's resource, so a test can count closes.
        resources: Mutex<Vec<Arc<InertResource>>>,
    }

    impl ScriptedFactory {
        fn new(script: Script, gate: Option<Gate>) -> Arc<Self> {
            Arc::new(Self {
                gate: Mutex::new(gate),
                script,
                calls: Mutex::new(Vec::new()),
                resources: Mutex::new(Vec::new()),
            })
        }
        /// Re-gate later attempts — used to stall a *second* server while a first is already live.
        fn set_gate(&self, gate: Gate) {
            *self.gate.lock().unwrap() = Some(gate);
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
        fn credential_flags(&self) -> Vec<bool> {
            self.calls.lock().unwrap().clone()
        }
        fn total_closes(&self) -> u32 {
            self.resources
                .lock()
                .unwrap()
                .iter()
                .map(|resource| resource.close_count())
                .sum()
        }
    }

    impl ConnectionFactory for ScriptedFactory {
        fn create(&self, request: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
            self.calls
                .lock()
                .unwrap()
                .push(request.credentials_invalidated);
            let resource = InertResource::new();
            self.resources.lock().unwrap().push(Arc::clone(&resource));
            let gate = self.gate.lock().unwrap().clone();
            let script = self.script;
            Box::pin(async move {
                if let Some(gate) = gate {
                    gate.wait().await;
                }
                match script {
                    Script::Connect => Ok(NewConnection {
                        resource,
                        status: ConnectionStatus::Connected,
                        credentials_invalidated: false,
                    }),
                    Script::NeedsAuthInvalidated => Ok(NewConnection {
                        resource,
                        status: ConnectionStatus::NeedsAuth,
                        credentials_invalidated: true,
                    }),
                    Script::FailOrdinary => Err(McpError::other("plain connect failure")),
                }
            })
        }
    }

    /// A factory whose `client.close()` always fails — the shape that must reach
    /// [`CONNECTION_CLEANUP_FAILED`].
    #[derive(Debug, Default)]
    struct FailingResource;

    impl ConnectionResource for FailingResource {
        fn close(&self) -> BoxFuture<'_, McpResult<()>> {
            Box::pin(async move { Err(McpError::other("client close failed")) })
        }
    }

    struct FailingCloseFactory;

    impl ConnectionFactory for FailingCloseFactory {
        fn create(&self, _request: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
            Box::pin(async move {
                Ok(NewConnection {
                    resource: Arc::new(FailingResource),
                    status: ConnectionStatus::Connected,
                    credentials_invalidated: false,
                })
            })
        }
    }

    fn manager(factory: Arc<dyn ConnectionFactory>) -> Arc<McpServerManager> {
        Arc::new(McpServerManager::with_factory(
            Some(std::path::PathBuf::from("/tmp")),
            factory,
        ))
    }

    fn entry() -> ServerEntry {
        ServerEntry {
            command: Some("true".to_string()),
            ..ServerEntry::default()
        }
    }

    fn disabled_entry() -> ServerEntry {
        ServerEntry {
            disabled: Some(true),
            ..entry()
        }
    }

    /// Let a spawned task reach its first suspension point.
    ///
    /// `yield_now` alone is enough on the current-thread runtime but NOT on a multi-threaded one,
    /// where a freshly spawned task may not have been picked up by any worker yet — so this also
    /// parks briefly on the timer. Tests that need a specific state rather than "some progress"
    /// poll for it instead; see `the_reconnect_single_flight_slot_is_held_then_cleared`.
    async fn settle() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
    }

    // ── MCP-100: the single-flight and generation fences ────────────────────────────────────

    /// MEASURED upstream: `creates=1`, and both callers receive the *same* object.
    #[tokio::test]
    async fn two_concurrent_connects_share_one_attempt() {
        let gate = Gate::shut();
        let factory = ScriptedFactory::new(Script::Connect, Some(gate.clone()));
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);

        let a = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.connect("s", &entry(), None).await }
        });
        settle().await;
        assert!(manager.is_connecting("s"), "`isConnecting` is `connectPromises.has(name)`");
        let b = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.connect("s", &entry(), None).await }
        });
        settle().await;
        gate.open();

        let (a, b) = (a.await.unwrap().unwrap(), b.await.unwrap().unwrap());
        assert_eq!(factory.call_count(), 1, "the second caller must be deduped, not raced");
        assert!(Arc::ptr_eq(&a, &b), "both callers share one connection object");
        assert!(!manager.is_connecting("s"), "the `finally` clears the slot");
    }

    /// **BLOCKER regression: `connect`'s single-flight must be one critical section.**
    ///
    /// The dedupe read and the matching insert used to live in two different `MutexGuard`s sixty
    /// lines apart, with nothing held across the gap — and with no `.await` in it either, which is
    /// exactly why it read as safe: OS preemption needs no yield point. MEASURED before the fix at
    /// 20 rounds × 64 racers: two `createConnection` calls in 12 rounds and three in one. Upstream
    /// gives one.
    ///
    /// The flavour is the test, and it is why the existing suite could not see this: every other
    /// state-machine test here is a bare `#[tokio::test]`, i.e. the current-thread runtime, where
    /// the region genuinely *is* atomic. Ablating the dedupe read is discriminated there; making
    /// the dedupe racy cannot be. The barrier is what makes the 64 racers arrive together instead
    /// of trickling in behind each other.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn sixty_four_racing_connects_create_exactly_one_connection() {
        const ROUNDS: usize = 20;
        const RACERS: usize = 64;

        for round in 0..ROUNDS {
            let factory = ScriptedFactory::new(Script::Connect, None);
            let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);
            let line = Arc::new(tokio::sync::Barrier::new(RACERS));

            let racers: Vec<_> = (0..RACERS)
                .map(|_| {
                    let manager = Arc::clone(&manager);
                    let line = Arc::clone(&line);
                    tokio::spawn(async move {
                        line.wait().await;
                        manager.connect("s", &entry(), None).await
                    })
                })
                .collect();

            let mut connections = Vec::with_capacity(RACERS);
            for racer in racers {
                connections.push(racer.await.unwrap().unwrap());
            }
            assert_eq!(
                factory.call_count(),
                1,
                "round {round}: {RACERS} racers must produce ONE `createConnection`, not a child \
                 process per worker thread that won the read"
            );
            let first = connections.first().unwrap();
            assert!(
                connections.iter().all(|connection| Arc::ptr_eq(connection, first)),
                "round {round}: and every racer receives the same connection object"
            );
            assert_eq!(factory.total_closes(), 0, "round {round}: nothing was built to throw away");
        }
    }

    /// **A cancelled winner must not strand its deduped caller's connection outside the map.**
    ///
    /// The generation fence and the `connections.set` live in the winner's body. Upstream's always
    /// runs (a JS promise completes whether or not anyone awaits it); a dropped Rust future runs
    /// nothing, so MEASURED before the fix: abort the winner while it is parked on the attempt with
    /// one deduped caller live, and the deduped caller receives a working connection while
    /// `get_connection("s")` is `None` and `close_all()` returns `Ok` having disposed nothing. With
    /// a real stdio server that is a child process outliving the session.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_cancelled_winner_still_registers_what_its_deduped_caller_received() {
        let gate = Gate::shut();
        let factory = ScriptedFactory::new(Script::Connect, Some(gate.clone()));
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);

        let winner = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.connect("s", &entry(), None).await }
        });
        for _ in 0..200 {
            if manager.is_connecting("s") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(manager.is_connecting("s"), "the winner registered its attempt");

        let deduped = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.connect("s", &entry(), None).await }
        });
        settle().await;
        assert_eq!(factory.call_count(), 1, "the second caller deduped onto the first attempt");

        // Exactly what dropping a `connect` future does.
        winner.abort();
        let _ = winner.await;
        gate.open();
        let connection = deduped.await.unwrap().unwrap();

        for _ in 0..200 {
            if manager.get_connection("s").is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let registered = manager
            .get_connection("s")
            .expect("the winner's body ran and registered the connection");
        assert!(
            Arc::ptr_eq(&registered, &connection),
            "and it is the very connection the surviving caller is holding"
        );

        manager.close_all().await.unwrap();
        assert_eq!(
            factory.total_closes(),
            1,
            "so shutdown can actually dispose it, rather than returning `Ok` having disposed nothing"
        );
    }

    /// **An in-flight request that straddles a reconnect must decrement the *fresh* record.**
    ///
    /// `InFlightGuard` used to hold the connection by `Arc` and decrement that object. That breaks
    /// the pairing with `doReconnect`'s `fresh.inFlight = Math.max(fresh.inFlight, staleInFlight)`
    /// carry-forward: the decrement lands on the record that was thrown away, the fresh one stays
    /// pinned at the carried floor, and `is_idle` is false forever — the idle sweep never reaps that
    /// server, which is the exact failure `raise_in_flight_to` was ported to avoid.
    #[tokio::test]
    async fn an_in_flight_request_that_straddles_a_reconnect_decrements_the_fresh_record() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        let stale = manager.connect("s", &entry(), None).await.unwrap();

        let (_connection, guard) = manager.begin_request("s").unwrap();
        assert_eq!(stale.in_flight(), 1);

        let handle: ConnectionHandle = Arc::clone(&stale) as ConnectionHandle;
        let fresh = manager.reconnect("s", &entry(), &handle, None).await.unwrap();
        assert!(!Arc::ptr_eq(&fresh, &stale), "a reconnect replaces the record");
        assert_eq!(fresh.in_flight(), 1, "`raise_in_flight_to` carried the count forward");

        drop(guard);
        assert_eq!(
            fresh.in_flight(),
            0,
            "the outstanding decrement must cancel the carried-forward count, not land on the \
             discarded record"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(
            manager.is_idle("s", Duration::ZERO),
            "and the idle sweep can reap the server again"
        );
    }

    /// **A settled attempt reaps the joiner tasks its combined tokens cost.**
    ///
    /// `abort.rs:21-26` states the discipline: composing two independent `CancellationToken`s costs
    /// one parked task per pair, so it has to be bounded per *generation*. `connect_inner` combines
    /// twice per attempt (`owned_signal`, then the attempt signal) and nothing ever cancelled either
    /// derived token on the success path, so every connect left one to two tasks parked until
    /// session end. The reap is observable as the derived tokens firing — and the assertion that
    /// matters just as much is that neither **parent** is disturbed by it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_settled_attempt_reaps_the_tokens_it_combined() {
        struct RecordingFactory {
            tokens: Mutex<Vec<(CancelToken, CancelToken)>>,
        }

        impl ConnectionFactory for RecordingFactory {
            fn create(&self, request: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
                self.tokens
                    .lock()
                    .unwrap()
                    .push((request.attempt.clone(), request.request.clone()));
                let resource = InertResource::new();
                Box::pin(async move {
                    Ok(NewConnection {
                        resource,
                        status: ConnectionStatus::Connected,
                        credentials_invalidated: false,
                    })
                })
            }
        }

        let factory = Arc::new(RecordingFactory {
            tokens: Mutex::new(Vec::new()),
        });
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);
        // A runtime signal *and* a caller token is the only pairing that allocates a joiner at all;
        // it is also what `ManagerSupervisor::connect` always passes (`lifecycle.rs:303`).
        let runtime = CancelToken::new();
        manager.set_runtime_signal(Some(runtime.clone()));
        let caller = CancelToken::new();
        manager.connect("s", &entry(), Some(&caller)).await.unwrap();

        let (attempt_signal, request_signal) = factory.tokens.lock().unwrap().first().cloned().unwrap();
        for _ in 0..200 {
            if attempt_signal.is_cancelled() && request_signal.is_cancelled() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(attempt_signal.is_cancelled(), "the attempt joiner was reaped");
        assert!(request_signal.is_cancelled(), "and so was `owned_signal`'s");
        assert!(!runtime.is_cancelled(), "without cancelling the runtime signal");
        assert!(!caller.is_cancelled(), "or the caller's own token");
        assert!(
            manager.get_connection("s").is_some(),
            "and the connection the reap belongs to is still registered"
        );
    }

    /// MEASURED upstream: the caller receives the *abort reason* `MCP connection s was closed`, not
    /// the "while connecting" message, and the resolved connection is disposed.
    #[tokio::test]
    async fn a_close_racing_a_connect_disposes_it_and_reports_the_abort_reason() {
        let gate = Gate::shut();
        let factory = ScriptedFactory::new(Script::Connect, Some(gate.clone()));
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);

        let connecting = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.connect("s", &entry(), None).await }
        });
        settle().await;
        let closing = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close("s").await }
        });
        settle().await;
        gate.open();

        let error = connecting.await.unwrap().expect_err("the close won");
        assert_eq!(error.to_string(), "MCP connection s was closed");
        assert!(matches!(error, McpError::Aborted(_)), "it is an abort, not a connect failure");
        closing.await.unwrap().expect("an ordinary connect failure is swallowed by close");
        assert_eq!(factory.total_closes(), 1, "the fenced connection is disposed exactly once");
        assert!(manager.get_connection("s").is_none());
    }

    /// MEASURED upstream: with the generation advanced but the attempt *not* aborted, the connection
    /// is disposed and the error is `MCP connection for s was closed while connecting`.
    #[tokio::test]
    async fn an_advanced_generation_alone_reports_closed_while_connecting() {
        let gate = Gate::shut();
        let factory = ScriptedFactory::new(Script::Connect, Some(gate.clone()));
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);

        let connecting = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.connect("s", &entry(), None).await }
        });
        settle().await;
        // Bump the generation the way `close` does, but leave the attempt controller alone.
        manager
            .tables()
            .close_generations
            .insert("s".to_string(), 99);
        gate.open();

        let error = connecting.await.unwrap().expect_err("the generation moved");
        assert_eq!(error.to_string(), "MCP connection for s was closed while connecting");
        assert_eq!(factory.total_closes(), 1);
    }

    /// `getAllConnections()` returns `new Map(this.connections)`. MEASURED: a `close` after the
    /// snapshot leaves the snapshot at its old size.
    #[tokio::test]
    async fn get_all_connections_is_a_snapshot() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        manager.connect("s", &entry(), None).await.unwrap();

        let snapshot = manager.get_all_connections();
        manager.close("s").await.unwrap();
        assert_eq!(snapshot.len(), 1, "the snapshot is a copy");
        assert_eq!(manager.get_all_connections().len(), 0);
    }

    /// MEASURED: exact-timeout is *not* idle (`>` is strict); in-flight work, a closed record and an
    /// unknown name are never idle.
    #[tokio::test]
    async fn is_idle_is_strict_and_respects_in_flight() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        let connection = manager.connect("s", &entry(), None).await.unwrap();

        assert!(!manager.is_idle("s", Duration::from_secs(60)));
        assert!(manager.is_idle("s", Duration::ZERO) || connection.last_used_at() == now_ms());
        manager.increment_in_flight("s");
        assert!(!manager.is_idle("s", Duration::ZERO), "in-flight work is never idle");
        manager.decrement_in_flight("s");
        connection.set_status(ConnectionStatus::Closed);
        assert!(!manager.is_idle("s", Duration::ZERO), "a closed record is never idle");
        assert!(!manager.is_idle("nope", Duration::ZERO), "an unknown name is never idle");
    }

    /// MEASURED: three decrements against one increment leave `0`, never a wrapped counter.
    #[tokio::test]
    async fn in_flight_never_goes_below_zero_and_the_guard_restores_it() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        let connection = manager.connect("s", &entry(), None).await.unwrap();

        manager.decrement_in_flight("s");
        manager.decrement_in_flight("s");
        manager.increment_in_flight("s");
        manager.decrement_in_flight("s");
        manager.decrement_in_flight("s");
        assert_eq!(connection.in_flight(), 0);

        let before = connection.last_used_at();
        {
            let (_connection, _guard) = manager.begin_request("s").unwrap();
            assert_eq!(connection.in_flight(), 1, "the guard increments on construction");
        }
        assert_eq!(connection.in_flight(), 0, "and decrements on drop, even through an early `?`");
        assert!(connection.last_used_at() >= before, "the drop also touches");
    }

    /// The two `begin_request` preconditions, both byte-exact.
    #[tokio::test]
    async fn begin_request_refuses_a_disabled_or_unconnected_server() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        assert_eq!(
            manager.begin_request("s").unwrap_err().to_string(),
            "Server \"s\" is not connected"
        );
        manager.connect("s", &entry(), None).await.unwrap();
        manager.get_connection("s").unwrap().set_status(ConnectionStatus::Closed);
        assert_eq!(
            manager.begin_request("s").unwrap_err().to_string(),
            "Server \"s\" is not connected"
        );
    }

    // ── MCP-125: reconnect ──────────────────────────────────────────────────────────────────

    /// MEASURED upstream: `creates=1, disposes=1, allSame=true, freshInFlight=4`.
    #[tokio::test]
    async fn three_concurrent_reconnects_produce_one_close_and_one_connect() {
        let gate = Gate::shut();
        let factory = ScriptedFactory::new(Script::Connect, Some(gate.clone()));
        gate.open();
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);

        let stale = manager.connect("s", &entry(), None).await.unwrap();
        for _ in 0..4 {
            stale.increment_in_flight();
        }
        let stale_handle: ConnectionHandle = Arc::clone(&stale) as ConnectionHandle;

        let mut joins = Vec::new();
        for _ in 0..3 {
            joins.push(tokio::spawn({
                let manager = Arc::clone(&manager);
                let stale = Arc::clone(&stale_handle);
                async move { manager.reconnect("s", &entry(), &stale, None).await }
            }));
        }
        let mut fresh = Vec::new();
        for join in joins {
            fresh.push(join.await.unwrap().unwrap());
        }

        assert_eq!(factory.call_count(), 2, "one initial connect plus exactly one reconnect");
        assert_eq!(factory.total_closes(), 1, "the stale connection is disposed once");
        let [first, second, third] = <[Arc<ServerConnection>; 3]>::try_from(fresh)
            .unwrap_or_else(|_| panic!("three reconnects, three results"));
        assert!(Arc::ptr_eq(&first, &second) && Arc::ptr_eq(&second, &third));
        assert!(!Arc::ptr_eq(&first, &stale));
        assert_eq!(
            first.in_flight(),
            4,
            "`fresh.inFlight = max(fresh.inFlight, staleInFlight)` — without it the idle sweep \
             closes a server whose callers are still waiting"
        );
    }

    /// MEASURED upstream: `returnedFresh=true`, and the log is empty — nothing is closed, nothing is
    /// created.
    #[tokio::test]
    async fn a_reconnect_whose_stale_handle_was_replaced_returns_the_replacement_untouched() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);

        let stale = manager.connect("s", &entry(), None).await.unwrap();
        let stale_handle: ConnectionHandle = Arc::clone(&stale) as ConnectionHandle;
        // Something else replaced it first.
        manager.close("s").await.unwrap();
        let winner = manager.connect("s", &entry(), None).await.unwrap();
        let creates_before = factory.call_count();
        let closes_before = factory.total_closes();

        let result = manager
            .reconnect("s", &entry(), &stale_handle, None)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&result, &winner), "the current connection is returned, not rebuilt");
        assert_eq!(factory.call_count(), creates_before, "nothing was created");
        assert_eq!(factory.total_closes(), closes_before, "and nothing was torn down");
    }

    /// MEASURED upstream: `current ?? this.connect(...)` — an empty map runs a plain connect.
    #[tokio::test]
    async fn a_reconnect_with_an_empty_map_falls_through_to_connect() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);
        let orphan: ConnectionHandle = ServerConnection::new(
            Arc::new(entry()),
            InertResource::new(),
            ConnectionStatus::Connected,
            false,
        ) as ConnectionHandle;

        let fresh = manager.reconnect("s", &entry(), &orphan, None).await.unwrap();
        assert_eq!(factory.call_count(), 1);
        assert_eq!(fresh.status(), ConnectionStatus::Connected);
    }

    /// MEASURED upstream: both guards fire *before* any teardown, and the stale connection is still
    /// in the map afterwards.
    #[tokio::test]
    async fn reconnect_guards_fire_before_any_teardown() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);
        let stale = manager.connect("s", &entry(), None).await.unwrap();
        let handle: ConnectionHandle = Arc::clone(&stale) as ConnectionHandle;

        let error = manager
            .reconnect("s", &disabled_entry(), &handle, None)
            .await
            .expect_err("disabled");
        assert_eq!(error.to_string(), "MCP server \"s\" is disabled");
        assert_eq!(factory.total_closes(), 0, "nothing was torn down");
        assert!(
            manager
                .get_connection("s")
                .is_some_and(|current| Arc::ptr_eq(&current, &stale)),
            "the stale connection is untouched"
        );

        assert_eq!(
            manager.connect("s2", &disabled_entry(), None).await.unwrap_err().to_string(),
            "MCP server \"s2\" is disabled",
            "`connect` carries the identical string"
        );
    }

    // ── MCP-126: close / closeAll ───────────────────────────────────────────────────────────

    /// MEASURED upstream: two concurrent closes dispose exactly once.
    #[tokio::test]
    async fn a_transport_is_closed_exactly_once() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);
        manager.connect("s", &entry(), None).await.unwrap();

        let a = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close("s").await }
        });
        let b = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close("s").await }
        });
        a.await.unwrap().unwrap();
        b.await.unwrap().unwrap();
        assert_eq!(factory.total_closes(), 1);
    }

    /// **The concurrent-close regression, and the reason `dispose`'s flag is only a backstop.**
    ///
    /// `close`'s "delete before awaiting SDK cleanup" was a check-then-act as well: two closers both
    /// read the same live connection, both removed it, and the second — whose `dispose`
    /// short-circuited on the already-taken flag — completed instantly, matched its own ticket and
    /// deleted `close_promises[name]` while the first was still inside the transport teardown. A
    /// `connect` arriving then finds nothing to wait for. MEASURED before the fix: 5 of 20 rounds
    /// started a `createConnection` mid-teardown; upstream, 32 concurrent closes gave 0.
    ///
    /// `a_transport_is_closed_exactly_once` cannot see this — it is current-thread, and two closers.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn racing_closes_never_let_a_connect_start_mid_teardown() {
        /// A resource that reports, for the duration of its teardown, that it is being torn down.
        #[derive(Debug)]
        struct TeardownProbe {
            gate: Gate,
            in_teardown: Arc<AtomicBool>,
            closes: Arc<AtomicI64>,
        }

        impl ConnectionResource for TeardownProbe {
            fn close(&self) -> BoxFuture<'_, McpResult<()>> {
                Box::pin(async move {
                    self.closes.fetch_add(1, Ordering::SeqCst);
                    self.in_teardown.store(true, Ordering::SeqCst);
                    self.gate.wait().await;
                    self.in_teardown.store(false, Ordering::SeqCst);
                    Ok(())
                })
            }
        }

        struct ProbeFactory {
            gate: Gate,
            in_teardown: Arc<AtomicBool>,
            closes: Arc<AtomicI64>,
            creates: Arc<AtomicI64>,
            creates_mid_teardown: Arc<AtomicI64>,
        }

        impl ConnectionFactory for ProbeFactory {
            fn create(&self, _r: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
                self.creates.fetch_add(1, Ordering::SeqCst);
                if self.in_teardown.load(Ordering::SeqCst) {
                    self.creates_mid_teardown.fetch_add(1, Ordering::SeqCst);
                }
                let resource = Arc::new(TeardownProbe {
                    gate: self.gate.clone(),
                    in_teardown: Arc::clone(&self.in_teardown),
                    closes: Arc::clone(&self.closes),
                });
                Box::pin(async move {
                    Ok(NewConnection {
                        resource,
                        status: ConnectionStatus::Connected,
                        credentials_invalidated: false,
                    })
                })
            }
        }

        for round in 0..20 {
            let gate = Gate::shut();
            let in_teardown = Arc::new(AtomicBool::new(false));
            let closes = Arc::new(AtomicI64::new(0));
            let creates = Arc::new(AtomicI64::new(0));
            let creates_mid_teardown = Arc::new(AtomicI64::new(0));
            let manager = manager(Arc::new(ProbeFactory {
                gate: gate.clone(),
                in_teardown: Arc::clone(&in_teardown),
                closes: Arc::clone(&closes),
                creates: Arc::clone(&creates),
                creates_mid_teardown: Arc::clone(&creates_mid_teardown),
            }) as Arc<dyn ConnectionFactory>);
            manager.connect("s", &entry(), None).await.unwrap();

            let line = Arc::new(tokio::sync::Barrier::new(32));
            let closers: Vec<_> = (0..32)
                .map(|_| {
                    let manager = Arc::clone(&manager);
                    let line = Arc::clone(&line);
                    tokio::spawn(async move {
                        line.wait().await;
                        manager.close("s").await
                    })
                })
                .collect();
            tokio::time::sleep(Duration::from_millis(40)).await;
            let connecting = tokio::spawn({
                let manager = Arc::clone(&manager);
                async move { manager.connect("s", &entry(), None).await }
            });
            tokio::time::sleep(Duration::from_millis(40)).await;

            assert_eq!(
                creates_mid_teardown.load(Ordering::SeqCst),
                0,
                "round {round}: a `connect` began building a second child while the first \
                 transport was still being torn down"
            );
            gate.open();
            for closer in closers {
                closer.await.unwrap().unwrap();
            }
            connecting.await.unwrap().unwrap();
            assert_eq!(
                closes.load(Ordering::SeqCst),
                1,
                "round {round}: 32 closers must dispose the transport exactly once"
            );
            assert_eq!(creates.load(Ordering::SeqCst), 2, "round {round}: the setup connect, then one after");
        }
    }

    /// MEASURED upstream: a `connect` issued while a `close` is still disposing does not create
    /// anything until the dispose resolves (`connectDoneBeforeDisposeResolves=false`). Without this
    /// wait a reconnect can hand back a child the close is in the middle of killing.
    #[tokio::test]
    async fn connect_waits_for_a_pending_close() {
        /// A resource whose teardown blocks until the test lets it finish.
        #[derive(Debug)]
        struct SlowClose(Gate);

        impl ConnectionResource for SlowClose {
            fn close(&self) -> BoxFuture<'_, McpResult<()>> {
                Box::pin(async move {
                    self.0.wait().await;
                    Ok(())
                })
            }
        }

        struct SlowFactory {
            gate: Gate,
            created: Arc<AtomicI64>,
        }

        impl ConnectionFactory for SlowFactory {
            fn create(&self, _r: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
                self.created.fetch_add(1, Ordering::SeqCst);
                let resource = Arc::new(SlowClose(self.gate.clone()));
                Box::pin(async move {
                    Ok(NewConnection {
                        resource,
                        status: ConnectionStatus::Connected,
                        credentials_invalidated: false,
                    })
                })
            }
        }

        let gate = Gate::shut();
        let created = Arc::new(AtomicI64::new(0));
        let manager = manager(Arc::new(SlowFactory {
            gate: gate.clone(),
            created: Arc::clone(&created),
        }) as Arc<dyn ConnectionFactory>);

        manager.connect("s", &entry(), None).await.unwrap();
        assert_eq!(created.load(Ordering::SeqCst), 1);

        let closing = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close("s").await }
        });
        settle().await;
        let connecting = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.connect("s", &entry(), None).await }
        });
        settle().await;
        assert_eq!(
            created.load(Ordering::SeqCst),
            1,
            "the connect must be parked on `closePromises`, not building a second child"
        );

        gate.open();
        closing.await.unwrap().unwrap();
        connecting.await.unwrap().unwrap();
        assert_eq!(created.load(Ordering::SeqCst), 2, "and it proceeds once the close resolves");
    }

    /// MEASURED upstream: an ordinary connect failure is swallowed by a racing `close`; an
    /// `AggregateError(_, "MCP connection cleanup failed")` is re-thrown.
    #[tokio::test]
    async fn close_rethrows_only_cleanup_failures_from_a_pending_connect() {
        // Ordinary failure → swallowed.
        let gate = Gate::shut();
        let factory = ScriptedFactory::new(Script::FailOrdinary, Some(gate.clone()));
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        let connecting = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.connect("s", &entry(), None).await }
        });
        settle().await;
        let closing = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close("s").await }
        });
        settle().await;
        gate.open();
        assert!(connecting.await.unwrap().is_err());
        closing.await.unwrap().expect("an ordinary connect failure never fails a close");

        // A cleanup failure → re-raised, keeping its CLASS across the public boundary and
        // rendering the way `formatTerminalError` renders. Both halves are the MCP-124 fix:
        //
        // * `to_string()` is the child alone. The head is dropped as soon as one child has text, so
        //   `starts_with(CONNECTION_CLEANUP_FAILED)` — what this test asserted while
        //   `ManagerError::Display` was head-prefixed — was pinning the wrong rendering.
        // * the variant is `ConnectionCleanupFailed`, not `Other`. Before the mapping in
        //   `From<&ManagerError>` this arrived as `McpError::Other` and `is_cleanup_failure()` was
        //   `false` for a genuine teardown failure — the blocker MCP-124 was filed to remove.
        let manager = manager_with_failing_close();
        manager.connect("s", &entry(), None).await.unwrap();
        let error = manager.close("s").await.expect_err("the client close failed");
        assert_eq!(
            error.to_string(),
            "client close failed",
            "`formatTerminalError` drops the head once a child contributes text"
        );
        assert_eq!(error.aggregate_head(), Some(CONNECTION_CLEANUP_FAILED));
        assert!(
            matches!(error, McpError::ConnectionCleanupFailed(_)),
            "the aggregate must keep its class at the public boundary, got {error:?}"
        );
        assert!(error.is_cleanup_failure());
    }

    fn manager_with_failing_close() -> Arc<McpServerManager> {
        manager(Arc::new(FailingCloseFactory) as Arc<dyn ConnectionFactory>)
    }

    /// MEASURED upstream, exactly this shape: one live connection plus one in-flight connect leaves
    /// the map empty and disposes **both** — the in-flight one by its own generation fence, which is
    /// why the log upstream reads `dispose(pending) … dispose(live)` in that order.
    #[tokio::test]
    async fn close_all_disposes_a_live_connection_and_one_that_is_mid_connect() {
        let gate = Gate::shut();
        let factory = ScriptedFactory::new(Script::Connect, Some(gate.clone()));
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);

        gate.open();
        manager.connect("live", &entry(), None).await.unwrap();

        let stalled = Gate::shut();
        factory.set_gate(stalled.clone());
        let pending = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.connect("pending", &entry(), None).await }
        });
        settle().await;
        assert!(manager.is_connecting("pending"));

        let closing = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close_all().await }
        });
        settle().await;
        stalled.open();

        closing.await.unwrap().unwrap();
        assert!(pending.await.unwrap().is_err(), "the fenced connect fails rather than surviving");
        assert_eq!(manager.get_all_connections().len(), 0, "nothing survives `closeAll`");
        assert_eq!(factory.total_closes(), 2, "both transports are closed exactly once");
        assert!(manager.is_stopped());
    }

    /// The **late sweep** itself: `closeAll` re-reads `connections` after its first pass and closes
    /// whatever appeared during it.
    ///
    /// Constructed rather than measured, and deliberately so: every *natural* route to a late
    /// insertion is already blocked by an earlier fence (the generation bump catches a pending
    /// connect; `stopped` catches a new one), so upstream's own late sweep found nothing in the
    /// scenario measured above. The arm still exists and is still specified, so it is exercised
    /// directly — a teardown that inserts a connection while the first pass is running.
    #[tokio::test]
    async fn close_all_late_sweep_closes_a_connection_that_appeared_during_the_first_sweep() {
        /// A resource whose `close()` inserts one connection into the manager, once — standing in
        /// for a connect that resolved and registered itself between the two sweeps.
        #[derive(Debug)]
        struct Interloper {
            manager: Mutex<Option<Arc<McpServerManager>>>,
            late: Arc<InertResource>,
        }

        impl ConnectionResource for Interloper {
            fn close(&self) -> BoxFuture<'_, McpResult<()>> {
                Box::pin(async move {
                    let manager = self.manager.lock().unwrap().take();
                    if let Some(manager) = manager {
                        manager.tables().connections.insert(
                            "late".to_string(),
                            ServerConnection::new(
                                Arc::new(entry()),
                                Arc::clone(&self.late) as Arc<dyn ConnectionResource>,
                                ConnectionStatus::Connected,
                                false,
                            ),
                        );
                    }
                    Ok(())
                })
            }
        }

        let late = InertResource::new();
        let interloper = Arc::new(Interloper {
            manager: Mutex::new(None),
            late: Arc::clone(&late),
        });
        let manager = manager(Arc::new(UnbuiltConnectionFactory) as Arc<dyn ConnectionFactory>);
        *interloper.manager.lock().unwrap() = Some(Arc::clone(&manager));
        manager.tables().connections.insert(
            "first".to_string(),
            ServerConnection::new(
                Arc::new(entry()),
                Arc::clone(&interloper) as Arc<dyn ConnectionResource>,
                ConnectionStatus::Connected,
                false,
            ),
        );

        manager.close_all().await.unwrap();
        assert_eq!(late.close_count(), 1, "the late arrival is closed by the second sweep");
        assert_eq!(manager.get_all_connections().len(), 0);
    }

    /// MEASURED upstream: after `closeAll`, `connect` and `reconnect` both raise
    /// `MCP server manager is closed`.
    #[tokio::test]
    async fn close_all_stops_the_manager_for_both_entry_points() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        manager.close_all().await.unwrap();

        assert_eq!(
            manager.connect("s", &entry(), None).await.unwrap_err().to_string(),
            MANAGER_CLOSED
        );
        let orphan: ConnectionHandle = ServerConnection::new(
            Arc::new(entry()),
            InertResource::new(),
            ConnectionStatus::Connected,
            false,
        ) as ConnectionHandle;
        assert_eq!(
            manager.reconnect("s", &entry(), &orphan, None).await.unwrap_err().to_string(),
            MANAGER_CLOSED
        );
    }

    /// `closeAll`'s aggregate carries only genuine teardown failures, under the byte-exact head.
    #[tokio::test]
    async fn close_all_aggregates_only_cleanup_failures() {
        let failing = manager_with_failing_close();
        failing.connect("a", &entry(), None).await.unwrap();
        let error = failing.close_all().await.expect_err("the client close failed");
        assert_eq!(
            error.to_string(),
            "client close failed",
            "the head surfaces only when no child contributes text"
        );
        assert_eq!(error.aggregate_head(), Some(MANAGER_CLEANUP_FAILED));
        assert!(
            matches!(error, McpError::ManagerCleanupFailed(_)),
            "got {error:?}"
        );
        assert!(error.is_cleanup_failure());
        // The nesting survives too: `closeAll`'s children are `disposeConnection`'s aggregates.
        assert!(
            error
                .aggregate_children()
                .is_some_and(|children| children
                    .iter()
                    .any(|child| matches!(child, McpError::ConnectionCleanupFailed(_)))),
            "expected a nested `MCP connection cleanup failed`, got {error:?}"
        );

        // An ordinary connect failure during shutdown is expected and must not surface.
        let factory = ScriptedFactory::new(Script::FailOrdinary, None);
        let quiet = manager(factory as Arc<dyn ConnectionFactory>);
        assert!(quiet.connect("b", &entry(), None).await.is_err());
        quiet.close_all().await.expect("an ordinary connect failure is not a cleanup failure");
    }

    /// `close(name)` clears **only** that server's accepted-elicitation set; `closeAll` clears all.
    /// MEASURED upstream.
    #[tokio::test]
    async fn the_url_elicitation_registry_is_cleared_per_server_then_wholesale() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        manager.connect("s", &entry(), None).await.unwrap();
        manager.connect("t", &entry(), None).await.unwrap();
        manager.remember_url_elicitation("s", "e1");
        manager.remember_url_elicitation("t", "e2");

        manager.close("s").await.unwrap();
        assert!(!manager.has_accepted_url_elicitation("s", "e1"));
        assert!(manager.has_accepted_url_elicitation("t", "e2"));
        assert!(manager.forget_url_elicitation("t", "e2"), "the delete reports it removed one");
        assert!(!manager.forget_url_elicitation("t", "e2"), "and reports false the second time");

        manager.remember_url_elicitation("t", "e3");
        manager.close_all().await.unwrap();
        assert!(!manager.has_accepted_url_elicitation("t", "e3"));
    }

    /// `rememberUrlElicitation` is a no-op once the runtime signal has fired.
    #[tokio::test]
    async fn a_stopped_runtime_records_no_elicitations() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        let runtime = CancelToken::new();
        manager.set_runtime_signal(Some(runtime.clone()));
        runtime.cancel();
        manager.remember_url_elicitation("s", "e1");
        assert!(!manager.has_accepted_url_elicitation("s", "e1"));
    }

    // ── MCP-116: needs-auth and one-shot credential invalidation ────────────────────────────

    /// MEASURED upstream: three consecutive connects against a permanent-401 fixture call
    /// `createConnection` with `[false, true, true]`. The flag rides on the connection record, so a
    /// retry loop cannot repeatedly discard a good cached credential.
    #[tokio::test]
    async fn credentials_are_invalidated_at_most_once_per_needs_auth_episode() {
        let factory = ScriptedFactory::new(Script::NeedsAuthInvalidated, None);
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);

        for _ in 0..3 {
            let connection = manager.connect("s", &entry(), None).await.unwrap();
            assert_eq!(connection.status(), ConnectionStatus::NeedsAuth);
            assert!(connection.credentials_invalidated());
        }
        assert_eq!(factory.credential_flags(), vec![false, true, true]);
    }

    /// A `needs-auth` record is **not** a connected one: `connect` builds a new attempt rather than
    /// handing the old record back. MEASURED upstream (`existing-needs-auth: created=1`).
    #[tokio::test]
    async fn a_needs_auth_record_does_not_satisfy_a_later_connect() {
        let factory = ScriptedFactory::new(Script::NeedsAuthInvalidated, None);
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);
        let first = manager.connect("s", &entry(), None).await.unwrap();
        let second = manager.connect("s", &entry(), None).await.unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(factory.call_count(), 2);
    }

    /// A **connected** record short-circuits the attempt and is touched. MEASURED upstream
    /// (`same=true, lastUsedAtBumped=true, creates=0`).
    #[tokio::test]
    async fn a_connected_record_short_circuits_the_attempt() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);
        let first = manager.connect("s", &entry(), None).await.unwrap();
        first.last_used_at.store(0, Ordering::SeqCst);

        let second = manager.connect("s", &entry(), None).await.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(factory.call_count(), 1);
        assert!(second.last_used_at() > 0, "`existing.lastUsedAt = Date.now()`");
    }

    // ── request options (§3.13) ─────────────────────────────────────────────────────────────

    /// MEASURED upstream: `0` and `-5` normalise away; `1234` survives; a per-server `0` beats a
    /// valid global and yields **no** timeout.
    #[tokio::test]
    async fn the_default_request_timeout_normalises_on_the_way_in() {
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);

        manager.set_default_request_timeout_ms(Some(0.0));
        assert!(manager.get_request_options("nope").is_none());
        manager.set_default_request_timeout_ms(Some(-5.0));
        assert!(manager.get_request_options("nope").is_none());
        manager.set_default_request_timeout_ms(Some(1234.0));
        assert_eq!(
            manager.get_request_options("nope").and_then(|options| options.timeout),
            Some(Duration::from_millis(1234))
        );

        manager
            .connect(
                "s",
                &ServerEntry {
                    request_timeout_ms: Some(0.0),
                    ..entry()
                },
                None,
            )
            .await
            .unwrap();
        assert!(
            manager.get_request_options("s").is_none(),
            "an invalid per-server value yields no timeout; it does NOT fall back to the global"
        );
    }

    // ── metadata publication ────────────────────────────────────────────────────────────────

    /// The identity guard: a publication for a replaced connection is dropped.
    #[tokio::test]
    async fn metadata_publication_is_identity_guarded() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        manager.set_metadata_list_changed_listener(Some({
            let seen = Arc::clone(&seen);
            Arc::new(move |name: &str, reason: &str| {
                seen.lock().unwrap().push(format!("{name}/{reason}"));
            })
        }));

        let first = manager.connect("s", &entry(), None).await.unwrap();
        assert!(manager.publish_metadata_changed("s", &first, "tools-list-changed"));
        manager.close("s").await.unwrap();
        let second = manager.connect("s", &entry(), None).await.unwrap();
        assert!(
            !manager.publish_metadata_changed("s", &first, "tools-list-changed"),
            "a stale connection cannot publish"
        );
        assert!(manager.publish_metadata_changed("s", &second, "prompts-list-changed"));
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["s/tools-list-changed", "s/prompts-list-changed"]
        );
    }

    /// A queued publication is retried once and then cleared — and only for the connection that
    /// queued it.
    #[tokio::test]
    async fn a_queued_publication_is_retried_for_its_own_connection_only() {
        let calls = Arc::new(AtomicI64::new(0));
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        manager.set_metadata_list_changed_listener(Some({
            let calls = Arc::clone(&calls);
            Arc::new(move |_: &str, _: &str| {
                calls.fetch_add(1, Ordering::SeqCst);
            })
        }));
        let connection = manager.connect("s", &entry(), None).await.unwrap();
        let other = ServerConnection::new(
            Arc::new(entry()),
            InertResource::new(),
            ConnectionStatus::Connected,
            false,
        );

        manager.queue_metadata_publication("s", &connection, "session-reconnect");
        manager.retry_pending_metadata_publication("s", &other);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "a different connection does not drain it");
        manager.retry_pending_metadata_publication("s", &connection);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        manager.retry_pending_metadata_publication("s", &connection);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "and it is cleared after the retry");
    }

    /// `close` drops any queued publication for that server (`server-manager.ts:1099`).
    #[tokio::test]
    async fn close_drops_a_queued_publication() {
        let calls = Arc::new(AtomicI64::new(0));
        let factory = ScriptedFactory::new(Script::Connect, None);
        let manager = manager(factory as Arc<dyn ConnectionFactory>);
        manager.set_metadata_list_changed_listener(Some({
            let calls = Arc::clone(&calls);
            Arc::new(move |_: &str, _: &str| {
                calls.fetch_add(1, Ordering::SeqCst);
            })
        }));
        let connection = manager.connect("s", &entry(), None).await.unwrap();
        manager.queue_metadata_publication("s", &connection, "session-reconnect");
        manager.close("s").await.unwrap();
        manager.retry_pending_metadata_publication("s", &connection);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// **The drop-net, discriminated.** A `needs-auth` record replaced by a later `connect` is
    /// dropped with its transport still open — MEASURED upstream as `disposedOld=0`, i.e. upstream
    /// simply leaks it. `ServerConnection::drop` closes it instead.
    ///
    /// `InertResource` has no teardown of its own, unlike `TokioChildProcess`, so this test fails
    /// the moment the net is removed; the stdio one does not.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_record_closes_a_resource_rmcp_does_not_own() {
        let resource = InertResource::new();
        let connection = ServerConnection::new(
            Arc::new(entry()),
            Arc::clone(&resource) as Arc<dyn ConnectionResource>,
            ConnectionStatus::NeedsAuth,
            true,
        );
        assert_eq!(resource.close_count(), 0);
        drop(connection);

        for _ in 0..200 {
            if resource.close_count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            resource.close_count(),
            1,
            "a dropped, never-disposed record must close its transport exactly once"
        );
    }

    /// **`dispose()`'s once-only flag, discriminated.** The explicit teardown and the drop-net are
    /// two routes to the same resource; the flag is what stops them from both firing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_explicit_dispose_and_the_drop_net_do_not_both_fire() {
        let resource = InertResource::new();
        let connection = ServerConnection::new(
            Arc::new(entry()),
            Arc::clone(&resource) as Arc<dyn ConnectionResource>,
            ConnectionStatus::Connected,
            false,
        );
        connection.dispose().await.unwrap();
        connection.dispose().await.unwrap();
        assert_eq!(resource.close_count(), 1, "`dispose` is once-only on its own");
        drop(connection);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(resource.close_count(), 1, "and the drop-net does not re-run it");
    }

    /// **BLOCKER regression: the once-only flag records that the close COMPLETED, not that it
    /// started.**
    ///
    /// `dispose` used to `swap(true)` on entry. A close future dropped inside `resource.close()`
    /// therefore left `disposed == true` with the transport still open, and `Drop for
    /// ServerConnection` — the net whose entire purpose is that case — read the flag and declined to
    /// fire. Any flag set before an await is a net that disarms itself the moment the future is
    /// dropped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dispose_dropped_mid_close_leaves_the_drop_net_armed() {
        /// A transport whose teardown parks until the test lets it finish, counting both halves.
        #[derive(Debug)]
        struct GatedResource {
            gate: Gate,
            started: AtomicU32,
            finished: AtomicU32,
        }

        impl ConnectionResource for GatedResource {
            fn close(&self) -> BoxFuture<'_, McpResult<()>> {
                Box::pin(async move {
                    self.started.fetch_add(1, Ordering::SeqCst);
                    self.gate.wait().await;
                    self.finished.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }
        }

        let gate = Gate::shut();
        let resource = Arc::new(GatedResource {
            gate: gate.clone(),
            started: AtomicU32::new(0),
            finished: AtomicU32::new(0),
        });
        let connection = ServerConnection::new(
            Arc::new(entry()),
            Arc::clone(&resource) as Arc<dyn ConnectionResource>,
            ConnectionStatus::Connected,
            false,
        );

        let disposing = tokio::spawn({
            let connection = Arc::clone(&connection);
            async move { connection.dispose().await }
        });
        for _ in 0..200 {
            if resource.started.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(resource.started.load(Ordering::SeqCst), 1, "the teardown is parked");

        // Exactly what `Self::race(..)` / `abort::abortable` do to a close future.
        disposing.abort();
        let _ = disposing.await;
        assert_eq!(resource.finished.load(Ordering::SeqCst), 0, "and it never finished");

        // The last handle goes: the drop-net is the only thing left that can close this transport.
        drop(connection);
        gate.open();
        for _ in 0..200 {
            if resource.finished.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            resource.started.load(Ordering::SeqCst),
            2,
            "the drop-net must fire for a dispose that did not finish"
        );
        assert_eq!(resource.finished.load(Ordering::SeqCst), 1, "and this time it completes");
    }

    /// **A reconnect whose caller goes away must still finish reconnecting.**
    ///
    /// `Self::race` is `abortable(fut, token)`, whose cancel arm **drops** `fut` — and every clone
    /// of the shared reconnect died with `reconnect_inner`'s frame, so an in-flight `do_reconnect`
    /// was dropped at whatever await it had reached. This test parks it in the one place that is
    /// unrecoverable: inside `close_inner(name).await`, *before* `connect_inner` has been called at
    /// all. The teardown itself survives (it has its own driver), but nothing is left to run the
    /// second half, so the server ends up **closed and not reconnected**.
    ///
    /// MEASURED upstream: `doReconnect` is *invoked* at `server-manager.ts:328`, so it is a live
    /// promise and `abortable` only decides what the caller sees — `connections` was `[]` when the
    /// caller gave up and `['s']` once the orphaned reconnect finished.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_reconnect_whose_caller_goes_away_still_finishes() {
        /// A transport whose teardown parks until the test lets it finish.
        #[derive(Debug)]
        struct GatedClose(Gate);

        impl ConnectionResource for GatedClose {
            fn close(&self) -> BoxFuture<'_, McpResult<()>> {
                Box::pin(async move {
                    self.0.wait().await;
                    Ok(())
                })
            }
        }

        /// The first connection's teardown is gated; every later one is inert.
        struct GatedFirstFactory {
            gate: Gate,
            creates: Arc<AtomicI64>,
        }

        impl ConnectionFactory for GatedFirstFactory {
            fn create(&self, _r: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
                let first = self.creates.fetch_add(1, Ordering::SeqCst) == 0;
                let resource: Arc<dyn ConnectionResource> = if first {
                    Arc::new(GatedClose(self.gate.clone()))
                } else {
                    InertResource::new()
                };
                Box::pin(async move {
                    Ok(NewConnection {
                        resource,
                        status: ConnectionStatus::Connected,
                        credentials_invalidated: false,
                    })
                })
            }
        }

        let gate = Gate::shut();
        let creates = Arc::new(AtomicI64::new(0));
        let manager = manager(Arc::new(GatedFirstFactory {
            gate: gate.clone(),
            creates: Arc::clone(&creates),
        }) as Arc<dyn ConnectionFactory>);
        let stale = manager.connect("s", &entry(), None).await.unwrap();
        let handle: ConnectionHandle = Arc::clone(&stale) as ConnectionHandle;

        let running = tokio::spawn({
            let manager = Arc::clone(&manager);
            let handle = Arc::clone(&handle);
            async move { manager.reconnect("s", &entry(), &handle, None).await }
        });

        // The unrecoverable middle: `close_inner` has removed the record and is parked in the
        // transport teardown, and `connect_inner` has not been reached.
        for _ in 0..400 {
            if manager.get_connection("s").is_none() && manager.tables().close_promises.contains_key("s") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(manager.get_connection("s").is_none(), "the stale record is gone");
        assert_eq!(creates.load(Ordering::SeqCst), 1, "and the replacement has not been started");

        running.abort();
        let _ = running.await;
        gate.open();

        for _ in 0..400 {
            if manager.get_connection("s").is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let fresh = manager
            .get_connection("s")
            .expect("the reconnect must finish even though its caller went away");
        assert!(!Arc::ptr_eq(&fresh, &stale), "and it is the replacement, not the stale record");
        assert_eq!(creates.load(Ordering::SeqCst), 2, "exactly one replacement was built");
        assert!(
            manager.tables().reconnect_promises.is_empty(),
            "the identity-matched `finally` still clears the slot"
        );
    }

    /// **The reconnect single-flight slot, asserted directly.** Three callers racing one stale
    /// handle share one `reconnectPromises` entry, and only the caller that created it clears it.
    ///
    /// This is the mechanism assertion, not a behavioural one: see the ablation note in this
    /// module's report — with the identity guard, the connect dedupe and `close`'s
    /// delete-before-await all in place, no *observable* behaviour distinguishes the reconnect map
    /// from its absence in this port, so the slot itself is what is checked.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_reconnect_single_flight_slot_is_held_then_cleared() {
        let gate = Gate::shut();
        let factory = ScriptedFactory::new(Script::Connect, Some(gate.clone()));
        gate.open();
        let manager = manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);
        let stale = manager.connect("s", &entry(), None).await.unwrap();
        let handle: ConnectionHandle = Arc::clone(&stale) as ConnectionHandle;

        let stalled = Gate::shut();
        factory.set_gate(stalled.clone());
        let running = tokio::spawn({
            let manager = Arc::clone(&manager);
            let handle = Arc::clone(&handle);
            async move { manager.reconnect("s", &entry(), &handle, None).await }
        });
        let mut occupied = false;
        for _ in 0..200 {
            if manager.tables().reconnect_promises.contains_key("s") {
                occupied = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(occupied, "the in-flight reconnect occupies its slot");
        stalled.open();
        running.await.unwrap().unwrap();
        assert!(
            manager.tables().reconnect_promises.is_empty(),
            "and the identity-matched `finally` clears it"
        );
    }

    // ── MCP-134: isTerminatedSession ────────────────────────────────────────────────────────

    fn http(status: u16, message: &str) -> TerminatedSessionEvidence<'_> {
        TerminatedSessionEvidence {
            http_status: Some(status),
            protocol_code: None,
            message,
        }
    }

    // ── MCP-131: real child processes ───────────────────────────────────────────────────────
    //
    // These are the only tests in this module that spawn anything. They exist because MCP-131 is a
    // claim about the process table, and a claim about the process table has to be checked against
    // the process table. `/proc` makes that exact on Linux; a zombie is NOT alive, so the state
    // field is read rather than just testing for the directory.

    #[cfg(unix)]
    mod child_process {
        use super::*;
        use crate::runtime::{spawn_stdio_transport, StdioTransportSpec};

        /// Alive means "exists and is not a zombie". A killed-but-unreaped child still has a
        /// `/proc/<pid>` entry, so a test that only checked for the directory would pass on a leak.
        fn process_alive(pid: u32) -> bool {
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                return false;
            };
            // `pid (comm) state …`, and `comm` may itself contain spaces and parentheses, so the
            // state is the first field after the LAST `)`.
            let Some((_, rest)) = stat.rsplit_once(')') else {
                return false;
            };
            rest.trim_start().chars().next().is_some_and(|state| state != 'Z')
        }

        async fn wait_until_gone(pid: u32, budget: Duration) -> bool {
            let deadline = tokio::time::Instant::now() + budget;
            while tokio::time::Instant::now() < deadline {
                if !process_alive(pid) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            !process_alive(pid)
        }

        fn spec(script: &str) -> StdioTransportSpec {
            StdioTransportSpec {
                command: "sh".to_string(),
                args: vec!["-c".to_string(), script.to_string()],
                // A deliberately minimal child environment: this is the "replace, don't merge"
                // semantics `StdioClientTransport`'s `env` option has, and it keeps the test
                // hermetic.
                env: [("PATH".to_string(), std::env::var("PATH").unwrap_or_default())]
                    .into_iter()
                    .collect(),
                cwd: None,
                plugin_data_dir: None,
                // `debug: false` ⇒ stderr is PIPED, which is the arm that needs the drain task.
                debug: false,
            }
        }

        fn adopt(script: &str) -> (Arc<StdioChildConnection>, u32) {
            let (process, stderr) = spawn_stdio_transport(&spec(script)).expect("spawn");
            let connection = StdioChildConnection::adopt(process, stderr);
            let pid = connection.child_pid().expect("a spawned child has a pid");
            (connection, pid)
        }

        /// The graceful arm: closing the transport drops the child's stdin, and a server that
        /// honours EOF exits well inside the 3-second window.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_child_that_honours_stdin_closure_exits_promptly_on_close() {
            let (connection, pid) = adopt("cat > /dev/null");
            assert!(process_alive(pid));

            let started = std::time::Instant::now();
            connection.close().await.expect("graceful shutdown");
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "an EOF-honouring child must not need the hard-kill timer"
            );
            assert!(wait_until_gone(pid, Duration::from_secs(2)).await, "pid {pid} survived close");
        }

        /// The hard-kill arm, and the **named delta** against the TS SDK: rmcp uses one 3-second
        /// window and then `SIGKILL`, with no `SIGTERM` leg. `exec` so `sh` replaces itself and the
        /// tracked pid is the process that ignores EOF.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_child_that_ignores_stdin_closure_is_killed_within_the_window() {
            let (connection, pid) = adopt("exec sleep 300");
            assert!(process_alive(pid));

            let started = std::time::Instant::now();
            connection.close().await.expect("graceful shutdown escalates to a kill");
            let elapsed = started.elapsed();
            assert!(
                elapsed >= Duration::from_secs(2) && elapsed < Duration::from_secs(8),
                "expected the ~3 s MAX_WAIT_ON_DROP_SECS window, took {elapsed:?}"
            );
            assert!(wait_until_gone(pid, Duration::from_secs(2)).await, "pid {pid} survived a kill");
        }

        /// `close()` is idempotent — `close`, `closeAll`'s late sweep and the drop-net can all reach
        /// it, and a second call must not signal a pid that may since have been recycled.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn closing_a_child_twice_is_a_no_op_the_second_time() {
            let (connection, pid) = adopt("cat > /dev/null");
            connection.close().await.expect("first");
            let started = std::time::Instant::now();
            connection.close().await.expect("second");
            assert!(
                started.elapsed() < Duration::from_millis(200),
                "the second close must return immediately, not re-run graceful_shutdown"
            );
            assert!(!process_alive(pid));
        }

        /// **BLOCKER regression: the process slot is surrendered only once the child is reaped.**
        ///
        /// `close` used to `take()` the process out under the lock and *then* await the shutdown, so
        /// a second close arriving mid-kill found `None` and reported success in microseconds while
        /// the child was still alive — the fast return
        /// `closing_a_child_twice_is_a_no_op_the_second_time` asserts as correct, which it only is
        /// once the first close has completed. Holding the guard across `graceful_shutdown` makes
        /// the second caller serialise behind the first instead: the once-only-ness upstream buys
        /// with its `abortCleanupPromises` WeakMap.
        ///
        /// `exec sleep 300` ignores stdin EOF, so the first close is guaranteed to be inside the
        /// full 3-second window while the second one arrives.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_second_close_waits_for_the_kill_instead_of_reporting_success_early() {
            let (connection, pid) = adopt("exec sleep 300");
            assert!(process_alive(pid));

            let first = tokio::spawn({
                let connection = Arc::clone(&connection);
                async move { connection.close().await }
            });
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(process_alive(pid), "the first close is still inside the 3 s window");

            let started = std::time::Instant::now();
            connection.close().await.expect("second close");
            assert!(
                started.elapsed() >= Duration::from_secs(2),
                "the second close returned in {:?} while the first was still killing the child",
                started.elapsed()
            );
            assert!(!process_alive(pid), "and by the time it returns the child is gone");
            first.await.expect("join").expect("first close");
        }

        /// **The stderr-pump test, and the reason `StdioChildConnection` spawns a drain task.**
        ///
        /// The child writes far more than one pipe buffer (64 KiB on Linux) to stderr *before* it
        /// ever reads stdin. With the pump, it drains and the child reaches its `cat` and exits at
        /// EOF. Without the pump the child blocks in `write(2)` forever, never sees stdin close, and
        /// only the 3-second hard kill ends it — the deadlock this repository already knows by name
        /// from `.config/nextest.toml`.
        ///
        /// The assertion is therefore on the *elapsed time* of the close: fast means drained.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_noisy_child_does_not_block_on_an_unread_stderr_pipe() {
            // 300 lines x ~350 bytes ≈ 105 KiB, comfortably past a 64 KiB pipe buffer.
            let (connection, pid) = adopt(
                "pad=$(printf '%0.sx' $(seq 1 320)); i=0; \
                 while [ $i -lt 300 ]; do echo \"line $i $pad\" >&2; i=$((i+1)); done; \
                 cat > /dev/null",
            );
            // Give the child time to fill and overflow the pipe.
            tokio::time::sleep(Duration::from_millis(400)).await;

            let started = std::time::Instant::now();
            connection.close().await.expect("graceful shutdown");
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the child was still blocked writing stderr — the drain task is not running"
            );
            assert!(wait_until_gone(pid, Duration::from_secs(2)).await);

            // And the tail is bounded: at most MAX_CAPTURED_STDERR_LINES joined with ` — `.
            let detail = connection.stderr_detail().expect("some stderr was captured");
            assert!(
                detail.split(" — ").count() <= MAX_CAPTURED_STDERR_LINES,
                "the tail keeps at most {MAX_CAPTURED_STDERR_LINES} lines, got {detail:?}"
            );
            assert!(
                detail.len() <= MAX_CAPTURED_STDERR_BYTES,
                "and at most {MAX_CAPTURED_STDERR_BYTES} bytes"
            );
        }

        /// **A named residual, asserted as it actually behaves — not as it should behave.**
        ///
        /// rmcp signals a single pid, not a process group, and 13c §3.12 argues that is sufficient
        /// *because* npx pre-resolution (MCP-103) removes the `npm` launcher that would be the
        /// grandparent. MCP-103 is not ported, and a server that forks its own worker is a second
        /// route to the same shape. This test measures the consequence so it cannot be mistaken for
        /// closed: the direct child dies, the grandchild does not.
        ///
        /// The grandchild's stdio is redirected to `/dev/null` at spawn — it does not need the
        /// harness's pipes, and leaving it holding one is precisely how a leaked grandchild hangs a
        /// `wait_with_output()` (`.config/nextest.toml`'s header). The test reaps it explicitly, so
        /// nothing survives this process either way.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_forking_child_leaves_its_grandchild_behind() {
            // Plain digits only: this path is interpolated into a `sh` script, and a `ThreadId(3)`
            // style name puts unquoted parentheses in it.
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let pidfile = std::env::temp_dir()
                .join(format!("cyrup-mcp-grandchild-{}-{unique}.pid", std::process::id()));
            let script = format!(
                "sleep 300 > /dev/null 2>&1 < /dev/null & echo $! > {}; exec cat > /dev/null",
                pidfile.display()
            );
            let (connection, child_pid) = adopt(&script);

            let mut grandchild = 0_u32;
            for _ in 0..100 {
                if let Ok(text) = std::fs::read_to_string(&pidfile)
                    && let Ok(pid) = text.trim().parse::<u32>()
                {
                    grandchild = pid;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_ne!(grandchild, 0, "the fixture never reported its grandchild");

            connection.close().await.expect("graceful shutdown");
            assert!(wait_until_gone(child_pid, Duration::from_secs(2)).await, "the direct child dies");

            // The residual, stated as an assertion so it fails loudly the day it is fixed and this
            // test has to be inverted.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let orphaned = process_alive(grandchild);
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(grandchild.to_string())
                .status();
            let _ = std::fs::remove_file(&pidfile);
            assert!(
                orphaned,
                "a single-pid kill was expected to leave the grandchild running; if this now fails, \
                 the process-group fix landed and MCP-131's residual should be closed"
            );
            assert!(wait_until_gone(grandchild, Duration::from_secs(2)).await, "test cleanup");
        }

        /// A factory that spawns a real child per connect — the end-to-end shape MCP-126's
        /// *verify* asks for ("assert zero surviving child processes by process-table check").
        struct SpawningFactory {
            pids: Mutex<Vec<u32>>,
            /// The child's `sh` script. `cat > /dev/null` honours stdin EOF and exits at once;
            /// `exec sleep 300` ignores it, which parks `graceful_shutdown` in its full 3-second
            /// `select!` and is what a cancellation test needs to land *inside*.
            script: &'static str,
        }

        impl SpawningFactory {
            fn honouring_eof() -> Self {
                Self {
                    pids: Mutex::new(Vec::new()),
                    script: "cat > /dev/null",
                }
            }

            fn ignoring_eof() -> Self {
                Self {
                    pids: Mutex::new(Vec::new()),
                    script: "exec sleep 300",
                }
            }
        }

        impl ConnectionFactory for SpawningFactory {
            fn create(
                &self,
                _request: CreateConnection,
            ) -> BoxFuture<'static, McpResult<NewConnection>> {
                let spawned = spawn_stdio_transport(&spec(self.script))
                    .map(|(process, stderr)| StdioChildConnection::adopt(process, stderr));
                let recorded = spawned.as_ref().ok().and_then(|c| c.child_pid());
                if let Some(pid) = recorded {
                    self.pids.lock().unwrap().push(pid);
                }
                Box::pin(async move {
                    Ok(NewConnection {
                        resource: spawned? as Arc<dyn ConnectionResource>,
                        status: ConnectionStatus::Connected,
                        credentials_invalidated: false,
                    })
                })
            }
        }

        /// **BLOCKER regression: a `close()` whose caller is cancelled must still reap the child.**
        ///
        /// MEASURED before the fix, with exactly this fixture: `manager.close("s")` in a task,
        /// aborted at 200 ms, left the pid alive at +5 s — well past rmcp's 3-second hard-kill
        /// window — and `close_all()` then returned `Ok(())` with the child still running.
        ///
        /// Three defects lined up to produce that, and all three are what this test holds down.
        /// `ServerConnection::dispose` set its once-only flag on **entry**, so the cancelled
        /// teardown left `disposed == true` and the drop-net declined to fire for precisely the case
        /// it exists for. `StdioChildConnection::close` `take()`d the process out of its slot before
        /// the await, so the next caller found `None` and returned `Ok(())` in microseconds having
        /// killed nothing. And `close_inner`'s teardown was a `futures::future::Shared` with no
        /// executor of its own: with the awaiting clone gone and only the map's inert clone left, it
        /// stopped making progress mid-`graceful_shutdown` — invisible to `close_all`, which reads
        /// `connections ∪ connect_promises` and never `close_promises`, and this name had already
        /// been removed from `connections`.
        ///
        /// The child ignores stdin EOF, so the abort is *guaranteed* to land inside
        /// `graceful_shutdown`'s 3-second window rather than after a lucky-fast exit.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_close_whose_caller_is_cancelled_still_reaps_the_child() {
            let factory = Arc::new(SpawningFactory::ignoring_eof());
            let manager = super::manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);
            manager.connect("s", &super::entry(), None).await.expect("connect");
            let pid = *factory.pids.lock().unwrap().first().expect("a child was spawned");
            assert!(process_alive(pid), "the fixture child is up");

            let closing = tokio::spawn({
                let manager = Arc::clone(&manager);
                async move { manager.close("s").await }
            });
            // Inside the 3 s window: long enough that the teardown is definitely parked in
            // `graceful_shutdown`, short enough that it cannot have finished.
            tokio::time::sleep(Duration::from_millis(400)).await;
            closing.abort();
            let _ = closing.await;
            assert!(process_alive(pid), "the child cannot have exited on its own — it ignores EOF");

            assert!(
                wait_until_gone(pid, Duration::from_secs(10)).await,
                "pid {pid} outlived the `close` whose caller was dropped: cancelling a *waiter* \
                 must not cancel the *kill*"
            );
            manager.close_all().await.expect("closeAll");
            assert!(!process_alive(pid), "and shutdown agrees the child is gone");
        }

        /// `closeAll` over five live stdio servers leaves **zero** surviving processes.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn close_all_leaves_no_surviving_child_process() {
            let factory = Arc::new(SpawningFactory::honouring_eof());
            let manager = super::manager(Arc::clone(&factory) as Arc<dyn ConnectionFactory>);
            for index in 0..5 {
                manager
                    .connect(&format!("s{index}"), &super::entry(), None)
                    .await
                    .expect("connect");
            }
            let pids = factory.pids.lock().unwrap().clone();
            assert_eq!(pids.len(), 5);
            assert!(pids.iter().all(|pid| process_alive(*pid)), "all five children are up");

            manager.close_all().await.expect("closeAll");
            for pid in pids {
                assert!(
                    wait_until_gone(pid, Duration::from_secs(5)).await,
                    "pid {pid} survived closeAll"
                );
            }
        }

        /// A connection record dropped **without** being disposed still leaves no child behind.
        ///
        /// MEASURED, and the measurement is worth recording precisely because it says this crate is
        /// not what saves the child here: ablating `ServerConnection::drop`'s net leaves this test
        /// GREEN, because rmcp's own `ChildWithCleanup::drop` spawns the `kill()`. The net is what
        /// covers resources rmcp does *not* own — see
        /// `a_dropped_record_closes_a_resource_rmcp_does_not_own`, which is the test that actually
        /// discriminates it.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_dropped_connection_record_still_reaps_its_child() {
            let (process, stderr) = spawn_stdio_transport(&spec("cat > /dev/null")).expect("spawn");
            let child = StdioChildConnection::adopt(process, stderr);
            let pid = child.child_pid().expect("pid");
            let connection = ServerConnection::new(
                Arc::new(super::entry()),
                child as Arc<dyn ConnectionResource>,
                ConnectionStatus::Connected,
                false,
            );

            assert!(process_alive(pid));
            drop(connection);
            assert!(
                wait_until_gone(pid, Duration::from_secs(5)).await,
                "pid {pid} survived the drop-net"
            );
        }
    }

    fn protocol(code: i32, message: &str) -> TerminatedSessionEvidence<'_> {
        TerminatedSessionEvidence {
            http_status: None,
            protocol_code: Some(code),
            message,
        }
    }

    /// All fifteen cases, MEASURED against upstream `isTerminatedSession` on node 22. The
    /// `hadSessionId` gate is absolute and is checked first.
    #[test]
    fn is_terminated_session_matches_upstream_on_every_measured_case() {
        let both = r#"HTTP 400: {"jsonrpc":"2.0","error":{"code":-32000,"message":"Bad Request: Server not initialized"}}"#;
        let spaced = r#"x "code" :  -32000 y "message" : "Bad Request: Server not initialized""#;

        // Positives.
        assert!(is_terminated_session(&http(404, "Not Found"), true));
        assert!(is_terminated_session(&http(400, both), true));
        assert!(is_terminated_session(&http(400, spaced), true), "whitespace around the colons");
        assert!(is_terminated_session(&protocol(-32000, "Server not initialized"), true));
        assert!(is_terminated_session(&protocol(-32000, "Bad Request: Server not initialized"), true));

        // The absolute gate.
        assert!(!is_terminated_session(&http(404, "Not Found"), false));
        assert!(!is_terminated_session(&http(400, both), false));
        assert!(!is_terminated_session(&protocol(-32000, "Server not initialized"), false));

        // Negatives that a plausible-looking port gets wrong.
        assert!(!is_terminated_session(&http(400, r#"{"code":-32000}"#), true), "code marker alone");
        assert!(
            !is_terminated_session(&http(400, r#"{"message":"Bad Request: Server not initialized"}"#), true),
            "message marker alone"
        );
        assert!(
            !is_terminated_session(&http(400, r#"{"code":-32000,"message":"Server not initialized"}"#), true),
            "the 400 regex names the LONG message only, while the ProtocolError set has both"
        );
        assert!(!is_terminated_session(&http(400, "Bad Request"), true), "a generic 400");
        assert!(!is_terminated_session(&http(500, both), true), "the status must be exactly 400");
        assert!(!is_terminated_session(&protocol(-32000, "Connection closed"), true));
        assert!(!is_terminated_session(&protocol(-32001, "Server not initialized"), true));
        assert!(
            !is_terminated_session(
                &TerminatedSessionEvidence { http_status: None, protocol_code: None, message: "aborted" },
                true
            ),
            "cancellation is never a session failure"
        );
    }
}

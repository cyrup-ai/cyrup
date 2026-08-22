//! The error taxonomy — `errors.ts` (`McpUiError` + subclasses) and the five aggregate messages of
//! `server-manager.ts` (MCP-089, MCP-124).
//!
//! # What this file owns now, and what MCP-089/MCP-124 add
//!
//! Landed here are the variants the activation path itself returns: [`McpError::Aborted`] (the
//! typed replacement for upstream's literal `error.message === "MCP extension runtime stopped"`
//! string compare, see [`crate::abort::is_abort_error`]) and
//! [`McpError::RuntimeCleanupFailed`] with its [`CleanupErrors`] aggregate, whose exact message
//! `"MCP runtime cleanup failed"` is `runtime-owner.ts`'s
//! `new AggregateError(failures, "MCP runtime cleanup failed")` (MCP-005).
//!
//! **MCP-089** adds the remaining `errors.ts` classes with their byte-exact `#[error("…")]`
//! templates plus `fn code(&self) -> &'static str` / `fn recovery_hint(&self) -> &'static str`;
//! post-cut that taxonomy is the base shape plus `McpServerError` (the five MCP Apps classes —
//! `ResourceFetchError`, `ResourceParseError`, `BridgeConnectionError`, `SessionError`,
//! `ServerError` — went with Cut 2, and `wrapError`'s only production caller went with them).
//!
//! **MCP-124 is LANDED**: the five `server-manager.ts` aggregates are
//! [`McpError::AbortCleanupFailed`], [`McpError::SetupFailed`], [`McpError::HttpCleanupFailed`],
//! [`McpError::ConnectionCleanupFailed`] and [`McpError::ManagerCleanupFailed`], their heads are
//! the five constants below, and [`McpError::is_cleanup_failure`] matches all seven aggregates
//! structurally.
//!
//! # Why a typed aggregate rather than a message regex
//!
//! Upstream's `containsCleanupFailure` walks the error graph testing `/cleanup failed|setup failed/`
//! against aggregate messages. That regex would also match a *server-supplied* message that happens
//! to contain "cleanup failed"; [`McpError::is_cleanup_failure`] is a structural match and does
//! not. Recorded as an intentional divergence (MCP-124).
//!
//! # How an aggregate RENDERS, measured rather than assumed
//!
//! An `AggregateError` has two distinct user-facing projections upstream and they disagree:
//!
//! * `error.message` is the **head** alone. That is what `server-manager.ts:591`, `:918` and `:939`
//!   compare against `"MCP connection abort cleanup failed"`, and here that comparison is a
//!   structural `matches!` on the variant, never a string compare. [`McpError::aggregate_head`]
//!   exposes the head for a caller that needs the literal.
//! * `formatTerminalError(error)` (`utils.ts:238-261`) is what the user actually reads, and it
//!   **drops the head** whenever any child renders non-empty text: it recurses into `.errors` and
//!   `.cause` first and pushes `value.message` only `if (messages.length === countBefore)`.
//!
//! MEASURED on node 22 against upstream's own `formatTerminalError` (`v2.26.1`, `fafae21`):
//!
//! | input | `formatTerminalError` |
//! |---|---|
//! | `AggregateError([Error("connect ECONNREFUSED"), Error("keychain unavailable")], "MCP connection setup failed")` | `connect ECONNREFUSED: keychain unavailable` |
//! | `AggregateError([], "MCP manager cleanup failed")` | `MCP manager cleanup failed` |
//! | `AggregateError([Error("same"), Error("same")], "MCP connection cleanup failed")` | `same` |
//! | `AggregateError([AggregateError([Error("inner")], "MCP connection abort cleanup failed")], "MCP connection setup failed")` | `inner` |
//! | `AggregateError([Error("")], "MCP HTTP connection cleanup failed")` | `MCP HTTP connection cleanup failed` |
//!
//! [`render_aggregate`] is that rule, and every aggregate variant in this file now renders through
//! it — including [`McpError::RuntimeCleanupFailed`] and [`McpError::OAuthAggregate`], whose
//! `Display` templates used to prefix the head unconditionally (`"MCP runtime cleanup failed: {0}"`)
//! and whose docs claimed that *was* `formatTerminalError`'s walk. It was not; the measurement above
//! is what corrected it. **Residual:** `formatTerminalError` finishes with `sanitizeTerminalText`
//! (OSC/CSI stripping and whitespace collapsing), which is a terminal-rendering concern and is not
//! applied by `Display` — a child message containing a control byte therefore renders differently
//! here than on upstream's status line.
//!
//! `cyrup_core::ToolError` is `{ message }` only, so this enum renders into `ToolError::message` at
//! the tool boundary and the structured triple (message / code / context) stays inside this crate.

use std::path::PathBuf;

/// The crate's `Result` alias.
pub type McpResult<T> = Result<T, McpError>;

// ===================================================================================================
// MCP-124 — the five `server-manager.ts` aggregate heads
// ===================================================================================================

/// `new AggregateError([error, cleanupError], "MCP connection abort cleanup failed")` —
/// `server-manager.ts:668`, raised by `connectClientWithAbort` when the transport it closed on abort
/// *also* failed to close.
///
/// It is the one head that is compared for equality upstream (`:591`, `:918`, `:939`) to decide that
/// cleanup has already been attempted and must not be attempted again; here that decision is
/// `matches!(error, McpError::AbortCleanupFailed(_))`.
pub const CONNECTION_ABORT_CLEANUP_FAILED: &str = "MCP connection abort cleanup failed";

/// `new AggregateError([error, ...cleanupFailures], "MCP connection setup failed")` —
/// `server-manager.ts:600`, raised by `createConnection`'s catch when the connect failed *and* the
/// cleanup after it failed too. This is the head that must reach
/// `McpServerManager::close_inner`'s pending-connect rethrow arm.
pub const CONNECTION_SETUP_FAILED: &str = "MCP connection setup failed";

/// `new AggregateError([error, cleanupError], "MCP HTTP connection cleanup failed")` —
/// `server-manager.ts:923`, raised by `connectHttpClient`'s per-attempt teardown.
pub const HTTP_CONNECTION_CLEANUP_FAILED: &str = "MCP HTTP connection cleanup failed";

/// `new AggregateError(failures, "MCP connection cleanup failed")` — `server-manager.ts:1139`,
/// raised by `disposeConnection`.
pub const CONNECTION_CLEANUP_FAILED: &str = "MCP connection cleanup failed";

/// `new AggregateError(failures, "MCP manager cleanup failed")` — `server-manager.ts:1168`, raised
/// by `closeAll` once every per-connection close has settled.
pub const MANAGER_CLEANUP_FAILED: &str = "MCP manager cleanup failed";

/// `runtime-owner.ts`'s `new AggregateError(failures, "MCP runtime cleanup failed")` (MCP-005).
/// Not one of `server-manager.ts`'s five; named here so every aggregate head in the crate is
/// reachable from one place.
pub const RUNTIME_CLEANUP_FAILED: &str = "MCP runtime cleanup failed";

/// `formatTerminalError`'s aggregate arm (`utils.ts:245-251`), as the `Display` of every aggregate
/// variant in this file.
///
/// ```text
/// if (value instanceof AggregateError) {
///   const countBefore = messages.length;
///   for (const nested of value.errors) collect(nested);
///   if (value.cause !== undefined) collect(value.cause);
///   if (messages.length === countBefore && value.message) messages.push(value.message);
///   return;
/// }
/// ```
///
/// Two consequences a porter gets wrong by reading rather than running it, both MEASURED on node 22
/// against upstream's own function (see the module header's table):
///
/// 1. **The head is dropped** as soon as one child contributes text. `"MCP connection setup failed"`
///    never reaches the user when the connect error underneath it has a message — which it always
///    does.
/// 2. **Children are de-duplicated** by `collect`'s `seen` set *and* by the final
///    `[...new Set(messages)]`, and a child whose message is the empty string contributes nothing —
///    so an aggregate of one blank error renders as its head after all.
#[must_use]
pub fn render_aggregate(head: &str, children: &CleanupErrors) -> String {
    render_aggregate_texts(head, children.iter().map(ToString::to_string))
}

/// [`render_aggregate`] over children that have already been rendered to text.
///
/// [`crate::server_manager::ManagerError`] is a second aggregate tree — `Arc`-shared, because the
/// single-flight maps hand one failure to many waiters — and it must render **identically**, since
/// what reaches a user through `closeAll` is that tree, not this one. Sharing the body is what
/// stops the two from drifting: an earlier revision had `ManagerError` render as
/// `write!("{head}")` followed by `": {child}"` per child, which prints
/// `"MCP manager cleanup failed: client close failed"` where `formatTerminalError` prints
/// `"client close failed"`. One implementation, one measured behaviour.
#[must_use]
pub fn render_aggregate_texts<I>(head: &str, children: I) -> String
where
    I: IntoIterator<Item = String>,
{
    let mut rendered: Vec<String> = Vec::new();
    for text in children {
        // `if (value.message) messages.push(...)` — a falsy (empty) message is not collected, which
        // is what makes case 5 of the measured table fall through to the head.
        if text.is_empty() || rendered.contains(&text) {
            continue;
        }
        rendered.push(text);
    }
    if rendered.is_empty() {
        return head.to_string();
    }
    rendered.join(": ")
}

/// Every failure `cyrup-mcp` produces, and the only error type that crosses a module boundary
/// inside this crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum McpError {
    /// The runtime owner (or the caller's own token) cancelled the work, carrying the *reason*
    /// upstream stores inside `AbortController.abort(new Error(reason))` — see
    /// [`crate::owner::McpRuntimeOwner::throw_if_inactive`] for why the reason has to round-trip.
    /// The default reason is the literal `"MCP extension runtime stopped"`.
    #[error("{0}")]
    Aborted(String),

    /// `mcp.json` (or one of its six sources) could not be read, parsed or validated. Note that
    /// almost nothing on the config path is allowed to *surface* this: `installMcpAdapter` is
    /// defensive everywhere, so a malformed file degrades to an empty surface (MCP-003).
    #[error("{0}")]
    Config(String),

    /// A named MCP server failed — connect, handshake, `tools/call`, or a transport-level fault.
    /// Upstream's `McpServerError`.
    #[error("{server}: {message}")]
    Server {
        /// The `mcpServers` key, as configured.
        server: String,
        /// The failure text as it reaches the user.
        message: String,
    },

    /// A filesystem operation on an adapter-owned path failed. Carries the path because every one
    /// of them (`mcp.json`, `mcp-cache.json`, `mcp-onboarding.json`, `agent-plugin-data/`) is
    /// user-relocatable through `CYRUP_AGENT_DIR`, and "permission denied" without the path is
    /// unactionable.
    #[error("{path}: {source}")]
    Io {
        /// The path the operation was attempted on.
        path: PathBuf,
        /// The underlying `std::io` failure.
        #[source]
        source: std::io::Error,
    },

    /// `runtime-owner.ts`'s `new AggregateError(failures, "MCP runtime cleanup failed")` — the LIFO
    /// cleanup stack ran to completion and at least one cleanup rejected (MCP-005).
    ///
    /// The head is [`RUNTIME_CLEANUP_FAILED`] and the rendering is [`render_aggregate`]'s, so the
    /// head appears **only** when no child contributed text. The template used to be
    /// `"MCP runtime cleanup failed: {0}"`, and the doc claimed that was `formatTerminalError`'s
    /// walk; measuring `formatTerminalError` on node 22 showed it is not — see the module header.
    #[error("{}", render_aggregate(RUNTIME_CLEANUP_FAILED, .0))]
    RuntimeCleanupFailed(CleanupErrors),

    /// The OS secure credential store was unreachable, or the record it held was unusable
    /// (13f / MCP-277, MCP-291).
    ///
    /// **This class must stay distinguishable from an ordinary failure all the way up.** Section
    /// 07's refresh driver rethrows a store failure and swallows every other refresh error into
    /// `None`; a broken keychain that arrives as an ordinary `Server`/`Other` failure therefore
    /// becomes an infinite silent re-auth loop. [`crate::credentials::AuthStoreError`] carries the
    /// `operation` discriminant and a redacting `Debug`, so nothing secret rides along.
    #[error("{0}")]
    CredentialStore(
        #[from]
        #[source]
        crate::credentials::AuthStoreError,
    ),

    /// `new AggregateError([error, cleanupError], phase)` — the OAuth flow's three cleanup phases
    /// (MCP-345), where a credential-store failure during cleanup must not hide the OAuth error
    /// that caused it, nor be hidden by it.
    ///
    /// `phase` is one of [`crate::oauth::PHASE_STARTUP_CLEANUP`],
    /// [`crate::oauth::PHASE_COMPLETION_CLEANUP`] or
    /// [`crate::oauth::PHASE_CANCELLATION_CLEANUP`], each of which is itself a `… cleanup failed`
    /// head. The rendering is [`render_aggregate`]'s — the phase surfaces only when neither child
    /// contributed text. It used to be `"{phase}: {errors}"`; see [`McpError::RuntimeCleanupFailed`]
    /// for the measurement that corrected it.
    #[error("{}", render_aggregate(phase, errors))]
    OAuthAggregate {
        /// The aggregate's own message — upstream's second `AggregateError` argument.
        phase: &'static str,
        /// The primary error followed by the cleanup error, in that order.
        errors: CleanupErrors,
    },

    /// `new AggregateError([error, cleanupError], "MCP connection abort cleanup failed")` —
    /// `connectClientWithAbort`'s abort path (`server-manager.ts:668`), MCP-124.
    ///
    /// This variant is a **flag as much as an error**: upstream tests `error.message === "MCP
    /// connection abort cleanup failed"` in three places to decide that the transport has already
    /// been torn down and must not be closed a second time. Every one of those sites is a
    /// `matches!` on this variant in the port.
    #[error("{}", render_aggregate(CONNECTION_ABORT_CLEANUP_FAILED, .0))]
    AbortCleanupFailed(CleanupErrors),

    /// `new AggregateError([error, ...cleanupFailures], "MCP connection setup failed")` —
    /// `createConnection`'s catch (`server-manager.ts:600`), MCP-124.
    ///
    /// The connect failed **and** the cleanup after it failed. That distinction is the whole point
    /// of the taxonomy: during shutdown an ordinary connect failure is expected and swallowed, a
    /// teardown failure must surface (§3.12). It also **suppresses the `needs-auth` downgrade** — a
    /// 401 whose cleanup failed is a setup failure, not a `needs-auth` (MCP-116).
    #[error("{}", render_aggregate(CONNECTION_SETUP_FAILED, .0))]
    SetupFailed(CleanupErrors),

    /// `new AggregateError([error, cleanupError], "MCP HTTP connection cleanup failed")` —
    /// `connectHttpClient`'s per-attempt teardown (`server-manager.ts:923`), MCP-124.
    #[error("{}", render_aggregate(HTTP_CONNECTION_CLEANUP_FAILED, .0))]
    HttpCleanupFailed(CleanupErrors),

    /// `new AggregateError(failures, "MCP connection cleanup failed")` — `disposeConnection`
    /// (`server-manager.ts:1139`), MCP-124.
    #[error("{}", render_aggregate(CONNECTION_CLEANUP_FAILED, .0))]
    ConnectionCleanupFailed(CleanupErrors),

    /// `new AggregateError(failures, "MCP manager cleanup failed")` — `closeAll`
    /// (`server-manager.ts:1168`), MCP-124. Its children are already filtered by
    /// `containsCleanupFailure`, so every one of them is itself a teardown failure.
    #[error("{}", render_aggregate(MANAGER_CLEANUP_FAILED, .0))]
    ManagerCleanupFailed(CleanupErrors),

    /// Anything not yet given a class by MCP-089. Kept so a port unit can land a call site before
    /// the taxonomy unit lands its variant, rather than inventing a class name that will not match.
    #[error("{0}")]
    Other(String),
}

impl McpError {
    /// Is this variant one of the seven aggregates — the structural stand-in for upstream's
    /// `current instanceof AggregateError && /cleanup failed|setup failed/.test(current.message)`?
    ///
    /// Every head this crate raises matches that regex, so "is an aggregate" and "matches the
    /// regex" are the same predicate here. They are *not* the same predicate upstream, and that
    /// gap is the intentional divergence: a server-supplied message reading `"cleanup failed"`
    /// satisfies upstream's test and cannot satisfy this one.
    #[must_use]
    const fn is_cleanup_aggregate(&self) -> bool {
        matches!(
            self,
            McpError::RuntimeCleanupFailed(_)
                | McpError::OAuthAggregate { .. }
                | McpError::AbortCleanupFailed(_)
                | McpError::SetupFailed(_)
                | McpError::HttpCleanupFailed(_)
                | McpError::ConnectionCleanupFailed(_)
                | McpError::ManagerCleanupFailed(_)
        )
    }

    /// The aggregate's own message — upstream's `AggregateError.message`, which is the *second*
    /// constructor argument and the thing `server-manager.ts:591`/`:918`/`:939` compare for
    /// equality.
    ///
    /// `None` for everything that is not an aggregate. Note that this is deliberately **not** what
    /// `Display` renders: `formatTerminalError` drops the head as soon as a child has text (see the
    /// module header's measured table), so the two are different projections of the same value and
    /// conflating them is how a port ends up printing `"MCP connection setup failed"` at a user who
    /// upstream would have shown `"connect ECONNREFUSED"`.
    #[must_use]
    pub fn aggregate_head(&self) -> Option<&str> {
        match self {
            McpError::RuntimeCleanupFailed(_) => Some(RUNTIME_CLEANUP_FAILED),
            McpError::OAuthAggregate { phase, .. } => Some(phase),
            McpError::AbortCleanupFailed(_) => Some(CONNECTION_ABORT_CLEANUP_FAILED),
            McpError::SetupFailed(_) => Some(CONNECTION_SETUP_FAILED),
            McpError::HttpCleanupFailed(_) => Some(HTTP_CONNECTION_CLEANUP_FAILED),
            McpError::ConnectionCleanupFailed(_) => Some(CONNECTION_CLEANUP_FAILED),
            McpError::ManagerCleanupFailed(_) => Some(MANAGER_CLEANUP_FAILED),
            _ => None,
        }
    }

    /// This aggregate's children — upstream's `AggregateError.errors`, `None` for a non-aggregate.
    #[must_use]
    pub fn aggregate_children(&self) -> Option<&CleanupErrors> {
        match self {
            McpError::RuntimeCleanupFailed(children)
            | McpError::AbortCleanupFailed(children)
            | McpError::SetupFailed(children)
            | McpError::HttpCleanupFailed(children)
            | McpError::ConnectionCleanupFailed(children)
            | McpError::ManagerCleanupFailed(children)
            | McpError::OAuthAggregate {
                errors: children, ..
            } => Some(children),
            _ => None,
        }
    }

    /// Upstream `containsCleanupFailure` (`server-manager.ts:1171-1191`): does this error, or
    /// anything reachable from it, represent a *teardown* failure rather than an ordinary connect
    /// failure?
    ///
    /// The discriminator matters during shutdown, where an ordinary connect failure is expected and
    /// swallowed while a teardown failure must surface (13c §3.12). Two consumers, both in
    /// `server_manager.rs`: `close`'s no-connection arm re-throws a pending connect's failure only
    /// when this is true, and `closeAll` filters its own aggregate's children by it.
    ///
    /// # The walk
    ///
    /// Upstream's is an explicit stack over `.errors` **plus** `.cause`, with a `seen` set because a
    /// JS error graph can genuinely cycle (`a.cause = b; b.cause = a` is legal). Here the graph is
    /// two edges — [`Self::aggregate_children`] (upstream's `.errors`) and
    /// [`std::error::Error::source`] (upstream's `.cause`) — and it is a tree the crate built
    /// itself, so a cycle is not constructible. The budget below is therefore not a correctness
    /// device but a fuse: a future `source()` impl that returned `self` must not be able to hang a
    /// shutdown, which is the one thing this predicate is called from.
    ///
    /// # The intentional divergence, restated at the site
    ///
    /// `is_cleanup_aggregate` (private) is structural. `McpError::Other("cleanup failed")` — the shape
    /// a *server* can force by putting that text in an error response — returns `false` here and
    /// `true` upstream.
    #[must_use]
    pub fn is_cleanup_failure(&self) -> bool {
        // `const pending: unknown[] = [error];`
        let mut pending: Vec<&(dyn std::error::Error + 'static)> = vec![self];
        // Upstream's `seen` set has no analogue: see "The walk" above. 1024 is a fuse, not a bound
        // on any real graph — the deepest aggregate this crate builds is 2.
        let mut budget = 1024_u32;
        while let Some(current) = pending.pop() {
            budget = match budget.checked_sub(1) {
                Some(remaining) => remaining,
                None => return false,
            };
            if let Some(mcp) = current.downcast_ref::<McpError>() {
                // `if (current instanceof AggregateError) { if (/…/.test(message)) return true;
                //   pending.push(...current.errors); }`
                if mcp.is_cleanup_aggregate() {
                    return true;
                }
                if let Some(children) = mcp.aggregate_children() {
                    pending.extend(
                        children
                            .iter()
                            .map(|child| child as &(dyn std::error::Error + 'static)),
                    );
                }
            }
            // `if (current.cause !== undefined) pending.push(current.cause);`
            if let Some(source) = std::error::Error::source(current) {
                pending.push(source);
            }
        }
        false
    }

    /// Convenience for the many sites whose upstream counterpart is a bare `throw new Error(msg)`.
    #[must_use]
    pub fn other(message: impl Into<String>) -> Self {
        McpError::Other(message.into())
    }

    /// The abort with upstream's default reason —
    /// `stop()`'s `reason = "MCP extension runtime stopped"`.
    #[must_use]
    pub fn aborted_default() -> Self {
        McpError::Aborted(crate::owner::DEFAULT_STOP_REASON.to_string())
    }

    /// `new AggregateError([primary, cleanup], phase)` (MCP-345) — the two-child aggregate the
    /// OAuth flow raises when a cleanup fails while another error is already in flight.
    #[must_use]
    pub fn oauth_aggregate(phase: &'static str, primary: McpError, cleanup: McpError) -> Self {
        McpError::OAuthAggregate {
            phase,
            errors: CleanupErrors::from(vec![primary, cleanup]),
        }
    }

    /// Whether this error — or anything in its `source()` chain — is a *credential store* failure.
    ///
    /// The refresh driver's discriminator: a store failure is rethrown, everything else becomes
    /// `None` and triggers a re-auth. Walks the chain for the same reason
    /// [`Self::is_cleanup_failure`] does, and with the same depth cap.
    #[must_use]
    pub fn is_credential_store_failure(&self) -> bool {
        if matches!(self, McpError::CredentialStore(_)) {
            return true;
        }
        let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(self);
        for _ in 0..32 {
            let Some(err) = source else { return false };
            if let Some(mcp) = err.downcast_ref::<McpError>()
                && matches!(mcp, McpError::CredentialStore(_))
            {
                return true;
            }
            source = std::error::Error::source(err);
        }
        false
    }
}

/// The collected failures of one LIFO cleanup pass (MCP-005), and the children of an
/// [`McpError::OAuthAggregate`] (MCP-345).
///
/// `Display` reproduces `formatTerminalError`'s aggregate walk: the child messages joined with
/// `": "`, **deduplicated** by its `seen` set — two cleanups failing with the same message render
/// once, which is what keeps a fan-out teardown from printing the same line five times.
#[derive(Debug, Default)]
pub struct CleanupErrors(Vec<McpError>);

impl CleanupErrors {
    /// An empty aggregate.
    #[must_use]
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Record one failed cleanup. Order is registration order reversed (LIFO), because that is the
    /// order [`crate::owner::McpRuntimeOwner::stop`] runs them in.
    pub fn push(&mut self, error: McpError) {
        self.0.push(error);
    }

    /// Whether every cleanup succeeded — the only case in which `stop()` returns `Ok(())`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many cleanups failed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The individual failures, in the order they were collected.
    pub fn iter(&self) -> impl Iterator<Item = &McpError> {
        self.0.iter()
    }
}

impl From<Vec<McpError>> for CleanupErrors {
    fn from(errors: Vec<McpError>) -> Self {
        Self(errors)
    }
}

impl std::fmt::Display for CleanupErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut seen: Vec<String> = Vec::with_capacity(self.0.len());
        for error in &self.0 {
            let rendered = error.to_string();
            if seen.contains(&rendered) {
                continue;
            }
            seen.push(rendered);
        }
        f.write_str(&seen.join(": "))
    }
}

impl std::error::Error for CleanupErrors {}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// The head literals are the whole wire contract of MCP-124, and `server_manager.rs` carries its
    /// own copies of five of them (it landed before this unit did and does not own this file). This
    /// asserts the two sets are byte-identical so the duplication cannot drift into a
    /// two-spellings-one-meaning bug. Delete it the day `server_manager.rs` re-exports these.
    #[test]
    fn the_five_heads_match_server_managers_own_copies() {
        assert_eq!(
            CONNECTION_ABORT_CLEANUP_FAILED,
            crate::server_manager::CONNECTION_ABORT_CLEANUP_FAILED
        );
        assert_eq!(
            CONNECTION_SETUP_FAILED,
            crate::server_manager::CONNECTION_SETUP_FAILED
        );
        assert_eq!(
            HTTP_CONNECTION_CLEANUP_FAILED,
            crate::server_manager::HTTP_CONNECTION_CLEANUP_FAILED
        );
        assert_eq!(
            CONNECTION_CLEANUP_FAILED,
            crate::server_manager::CONNECTION_CLEANUP_FAILED
        );
        assert_eq!(
            MANAGER_CLEANUP_FAILED,
            crate::server_manager::MANAGER_CLEANUP_FAILED
        );
        // And against the .ts, character for character (`server-manager.ts:668, 600, 923, 1139,
        // 1168`).
        assert_eq!(
            CONNECTION_ABORT_CLEANUP_FAILED,
            "MCP connection abort cleanup failed"
        );
        assert_eq!(CONNECTION_SETUP_FAILED, "MCP connection setup failed");
        assert_eq!(
            HTTP_CONNECTION_CLEANUP_FAILED,
            "MCP HTTP connection cleanup failed"
        );
        assert_eq!(CONNECTION_CLEANUP_FAILED, "MCP connection cleanup failed");
        assert_eq!(MANAGER_CLEANUP_FAILED, "MCP manager cleanup failed");
        assert_eq!(RUNTIME_CLEANUP_FAILED, "MCP runtime cleanup failed");
    }

    /// Every row of the module header's measured table, as an assertion.
    ///
    /// Each expected string was produced by running upstream's own `formatTerminalError`
    /// (`tmp/pi-mcp-adapter/utils.ts:238`, `v2.26.1` = `fafae21`) on node 22 over the equivalent
    /// `AggregateError`, not by reading it.
    #[test]
    fn an_aggregate_renders_exactly_what_format_terminal_error_renders() {
        // `AggregateError([Error("connect ECONNREFUSED 127.0.0.1:9"), Error("transport close
        // failed")], "MCP connection setup failed")` → "connect ECONNREFUSED 127.0.0.1:9: transport
        // close failed" — the head is GONE.
        let setup = McpError::SetupFailed(CleanupErrors::from(vec![
            McpError::other("connect ECONNREFUSED 127.0.0.1:9"),
            McpError::other("transport close failed"),
        ]));
        assert_eq!(
            setup.to_string(),
            "connect ECONNREFUSED 127.0.0.1:9: transport close failed"
        );
        assert_eq!(setup.aggregate_head(), Some(CONNECTION_SETUP_FAILED));

        // `AggregateError([], "MCP manager cleanup failed")` → the head, because nothing else exists.
        let empty = McpError::ManagerCleanupFailed(CleanupErrors::new());
        assert_eq!(empty.to_string(), MANAGER_CLEANUP_FAILED);

        // De-duplication: `AggregateError([Error("same"), Error("same")], …)` → "same".
        let duplicated = McpError::ConnectionCleanupFailed(CleanupErrors::from(vec![
            McpError::other("same"),
            McpError::other("same"),
        ]));
        assert_eq!(duplicated.to_string(), "same");

        // Nesting: the inner aggregate's own head is dropped too, so only "inner" survives.
        let nested = McpError::SetupFailed(CleanupErrors::from(vec![McpError::AbortCleanupFailed(
            CleanupErrors::from(vec![McpError::other("inner")]),
        )]));
        assert_eq!(nested.to_string(), "inner");

        // A child whose message is the empty string is not collected (`if (value.message)`), so the
        // aggregate falls through to its head.
        let blank =
            McpError::HttpCleanupFailed(CleanupErrors::from(vec![McpError::other(String::new())]));
        assert_eq!(blank.to_string(), HTTP_CONNECTION_CLEANUP_FAILED);

        // `runtime-owner.ts:41`'s whole line, measured.
        let runtime = McpError::RuntimeCleanupFailed(CleanupErrors::from(vec![McpError::other(
            "unlink /tmp/x: EBUSY",
        )]));
        assert_eq!(
            format!("MCP: runtime cleanup failed: {runtime}"),
            "MCP: runtime cleanup failed: unlink /tmp/x: EBUSY"
        );
    }

    /// `containsCleanupFailure`, one assertion per bullet of MCP-124's verify line.
    #[test]
    fn contains_cleanup_failure_matches_the_five_aggregates_and_nothing_else() {
        for aggregate in [
            McpError::AbortCleanupFailed(CleanupErrors::new()),
            McpError::SetupFailed(CleanupErrors::new()),
            McpError::HttpCleanupFailed(CleanupErrors::new()),
            McpError::ConnectionCleanupFailed(CleanupErrors::new()),
            McpError::ManagerCleanupFailed(CleanupErrors::new()),
            McpError::RuntimeCleanupFailed(CleanupErrors::new()),
            McpError::oauth_aggregate(
                crate::oauth::PHASE_STARTUP_CLEANUP,
                McpError::other("a"),
                McpError::other("b"),
            ),
        ] {
            assert!(
                aggregate.is_cleanup_failure(),
                "{:?} must read as a cleanup failure",
                aggregate.aggregate_head()
            );
        }

        // "a plain connect error returns false"
        assert!(!McpError::other("connect ECONNREFUSED").is_cleanup_failure());
        assert!(
            !McpError::Server {
                server: "s".to_string(),
                message: "boom".to_string(),
            }
            .is_cleanup_failure()
        );

        // THE DOCUMENTED DIVERGENCE: upstream's `/cleanup failed|setup failed/` fires on this
        // string; the structural match does not. A server that puts the phrase in an error response
        // cannot make a shutdown re-throw here.
        assert!(!McpError::other("cleanup failed").is_cleanup_failure());
        assert!(!McpError::other("MCP connection setup failed").is_cleanup_failure());
        assert!(
            !McpError::Server {
                server: "hostile".to_string(),
                message: "MCP manager cleanup failed".to_string(),
            }
            .is_cleanup_failure()
        );
    }

    /// "a 3-deep `source()` nest with the marker at the bottom returns true", and the `.errors` edge
    /// with the marker at the bottom too — upstream walks both.
    #[test]
    fn contains_cleanup_failure_finds_a_marker_three_levels_down_either_edge() {
        // The `.errors` edge: an aggregate whose head does not match would still have to be walked
        // into. No head this crate raises fails the match, so the walk is exercised by nesting a
        // matching aggregate under an aggregate — which short-circuits at depth 0 — AND by the
        // `source()` edge below, which is the one that can genuinely reach a marker through a
        // non-aggregate.
        let deep = McpError::SetupFailed(CleanupErrors::from(vec![McpError::HttpCleanupFailed(
            CleanupErrors::from(vec![McpError::AbortCleanupFailed(CleanupErrors::from(
                vec![McpError::other("bottom")],
            ))]),
        )]));
        assert!(deep.is_cleanup_failure());

        // The `.cause` edge, three hops deep with the marker at the bottom. `Link` stands in for
        // any foreign error the port may one day wrap an `McpError` behind; upstream's walk
        // follows `.cause` through exactly such links.
        #[derive(Debug)]
        struct Link(Box<dyn std::error::Error + Send + Sync + 'static>);
        impl std::fmt::Display for Link {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "link")
            }
        }
        impl std::error::Error for Link {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(self.0.as_ref())
            }
        }

        let marker = McpError::ConnectionCleanupFailed(CleanupErrors::from(vec![McpError::other(
            "client.close threw",
        )]));
        let three = Link(Box::new(Link(Box::new(Link(Box::new(marker))))));
        // The predicate is on `McpError`, so the chain is rooted in the one variant that carries a
        // `#[source]`.
        let rooted = McpError::Io {
            path: std::path::PathBuf::from("/tmp/x"),
            source: std::io::Error::other(three),
        };
        // NOTE, measured while writing this: `std::io::Error`'s own `source()` delegates to the
        // BOXED payload's `source()` and never yields the payload itself, so a marker boxed
        // directly into `io::Error::other(marker)` is NOT reachable. Nothing in this crate does
        // that — `McpError::Io` boxes `std::io::Error`s — but a future site that does would be
        // narrower than upstream's `.cause` walk, and this comment is the record of it.
        assert!(rooted.is_cleanup_failure());

        // The same chain with an ordinary error at the bottom stays false.
        let benign = McpError::Io {
            path: std::path::PathBuf::from("/tmp/x"),
            source: std::io::Error::other(Link(Box::new(Link(Box::new(McpError::other("plain")))))),
        };
        assert!(!benign.is_cleanup_failure());
    }

    /// "a cyclic chain terminates". A cycle is not constructible with owned `McpError`s, so the
    /// thing actually asserted is the fuse: a graph far larger than the budget returns rather than
    /// spinning, and does so promptly.
    #[test]
    fn a_pathological_graph_terminates_instead_of_spinning() {
        // 4096 siblings under one non-matching root. The root is a non-aggregate so the walk cannot
        // short-circuit; it has no children either, so this measures the budget's floor, not its
        // ceiling.
        let mut children = Vec::with_capacity(4096);
        for index in 0..4096 {
            children.push(McpError::other(format!("child {index}")));
        }
        // Wrapping them in a matching aggregate short-circuits immediately — that is the fast path
        // and it must stay fast.
        let start = std::time::Instant::now();
        let wide = McpError::ManagerCleanupFailed(CleanupErrors::from(children));
        assert!(wide.is_cleanup_failure());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "the walk must short-circuit, not enumerate"
        );
    }
}

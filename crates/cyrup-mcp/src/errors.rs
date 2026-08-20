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
//! **MCP-124** adds the other four aggregates (`AbortCleanupFailed`, `SetupFailed`,
//! `HttpCleanupFailed`, `ConnectionCleanupFailed`, `ManagerCleanupFailed`) and completes
//! [`McpError::is_cleanup_failure`]. Both extend this enum; neither replaces it.
//!
//! # Why a typed aggregate rather than a message regex
//!
//! Upstream's `containsCleanupFailure` walks the error graph testing `/cleanup failed|setup failed/`
//! against aggregate messages. That regex would also match a *server-supplied* message that happens
//! to contain "cleanup failed"; [`McpError::is_cleanup_failure`] is a structural match and does
//! not. Recorded as an intentional divergence (MCP-124).
//!
//! `cyrup_core::ToolError` is `{ message }` only, so this enum renders into `ToolError::message` at
//! the tool boundary and the structured triple (message / code / context) stays inside this crate.

use std::path::PathBuf;

/// The crate's `Result` alias.
pub type McpResult<T> = Result<T, McpError>;

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
    /// cleanup stack ran to completion and at least one cleanup rejected. The head text is exact:
    /// it reaches the user through `formatTerminalError`, which renders an aggregate as its own
    /// message followed by its children (MCP-005).
    #[error("MCP runtime cleanup failed: {0}")]
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
    /// [`crate::oauth::PHASE_CANCELLATION_CLEANUP`]. The rendering is the phase followed by every
    /// child message, joined exactly as [`CleanupErrors`] joins its own — which is what
    /// `formatTerminalError` does to an `AggregateError`.
    #[error("{phase}: {errors}")]
    OAuthAggregate {
        /// The aggregate's own message — upstream's second `AggregateError` argument.
        phase: &'static str,
        /// The primary error followed by the cleanup error, in that order.
        errors: CleanupErrors,
    },

    /// Anything not yet given a class by MCP-089. Kept so a port unit can land a call site before
    /// the taxonomy unit lands its variant, rather than inventing a class name that will not match.
    #[error("{0}")]
    Other(String),
}

impl McpError {
    /// Upstream `containsCleanupFailure`: does this error, or anything in its `source()` chain,
    /// represent a *teardown* failure rather than an ordinary connect failure?
    ///
    /// The discriminator matters during shutdown, where an ordinary connect failure is expected and
    /// swallowed while a teardown failure must surface (13c §3.12). The walk is bounded — a
    /// `source()` chain cannot cycle the way upstream's `.errors` + `.cause` graph can, so no
    /// `seen` set is needed — but it is depth-capped anyway so a pathological chain cannot spin.
    ///
    /// MCP-124 extends the match arm as it lands the other four aggregates.
    #[must_use]
    pub fn is_cleanup_failure(&self) -> bool {
        if matches!(
            self,
            McpError::RuntimeCleanupFailed(_) | McpError::OAuthAggregate { .. }
        ) {
            return true;
        }
        let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(self);
        // 32 is far past any real chain; it exists only so a future `source()` impl that loops
        // cannot hang a shutdown.
        for _ in 0..32 {
            let Some(err) = source else { return false };
            if let Some(mcp) = err.downcast_ref::<McpError>()
                && matches!(
                    mcp,
                    McpError::RuntimeCleanupFailed(_) | McpError::OAuthAggregate { .. }
                )
            {
                return true;
            }
            source = std::error::Error::source(err);
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

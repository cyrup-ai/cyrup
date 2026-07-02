//! `SubagentError` — the crate-wide error taxonomy (arch-SA §8, thiserror-derived).
//!
//! No-panic guarantees (matching arch-00 §8 / arch-08 §8's workspace-wide convention): every
//! fallible path in this crate returns one of these variants (or wraps one via `?`/`#[from]`)
//! rather than panicking. In particular:
//! - NDJSON parse tolerance (R-SA-026): a malformed line from a child's stdout is a per-line
//!   `Err` swallowed by the consumer, never propagated as a `SubagentError`, never a panic.
//! - Subprocess faults surface as `SubagentError::Spawn`/timeout classification, never a panic.
//! - Reconciliation degrades to `Liveness::Unknown` on ambiguous signal-probe failures
//!   (R-SA-089), never conflated with `Dead`.
//! - Malformed on-disk state degrades to a synthesized failure result (R-SA-092), never a panic.
//! - Command-handler panics are caught at the `cyrup-ext` dispatch layer
//!   (`NativeHandle::invoke_event`'s `catch_unwind`), not re-caught inside this crate.

/// The crate-wide error taxonomy for `cyrup-ext-subagents` (arch-SA §8).
#[derive(thiserror::Error, Debug)]
pub enum SubagentError {
    /// The run (or the caller's wait on it) was cancelled.
    #[error("cancelled")]
    Cancelled,

    /// Fork-context requested but the parent session has no resolvable leaf (R-SA-137).
    #[error("fork-context requires a resolvable session leaf")]
    ForkRequiresLeaf,

    /// Fork-context requested but the parent session is not yet persisted (R-SA-137).
    #[error("fork-context requires a persisted parent session")]
    ForkRequiresPersistedParent,

    /// Fork-context branch creation failed for any other reason (R-SA-137).
    #[error("fork-context branch creation failed")]
    ForkFailed,

    /// The effective recursion-depth ceiling was reached before spawning a nested subagent
    /// (R-SA-055/056).
    #[error("depth limit exceeded: {current}/{max}")]
    DepthExceeded {
        /// The current recursion depth at the point the limit was checked.
        current: u32,
        /// The effective (tightened-only) maximum depth ceiling.
        max: u32,
    },

    /// A `worktree: true` parallel/dynamic group aborted setup before any child was spawned
    /// (R-SA-060/063).
    #[error("worktree group aborted: {0}")]
    WorktreeSetup(String),

    /// No agent definition matched the requested fully-qualified name (R-SA-008).
    #[error("agent not found: {0}")]
    AgentNotFound(String),

    /// A management (create/update/delete/rename) operation targeted a Builtin- or
    /// Package-sourced agent, which is read-only (R-SA-014).
    #[error("agent source is read-only: {0}")]
    ReadOnlySource(String),

    /// A malformed `subagents.*` settings field aborted discovery (R-SA-009).
    #[error("malformed subagents settings: {0}")]
    MalformedSettings(String),

    /// Acceptance-gate evaluation rejected an otherwise-clean run (R-SA-011/033).
    #[error("acceptance rejected: {0}")]
    AcceptanceRejected(String),

    /// `output_mode == "file-only"` was requested without an `output_path` (R-SA-025).
    #[error("output-file mode requires an output path")]
    OutputPathRequired,

    /// The child's structured output was absent or failed schema validation (R-SA-030).
    #[error("structured output missing or invalid: {0}")]
    StructuredOutputInvalid(String),

    /// A run-id selector matched more than one candidate across namespaces (R-SA-080).
    #[error("run id ambiguous: {0}")]
    AmbiguousRunId(String),

    /// A profile/provider path token failed the safe-token allowlist before any filesystem
    /// access (R-SA-087/142).
    #[error("unsafe path token: {0}")]
    UnsafePathToken(String),

    /// A resume was requested against a run with no persisted transcript to resume from
    /// (R-SA-085).
    #[error("resume target has no persisted transcript")]
    ResumeNoTranscript,

    /// Subprocess spawn or I/O failure.
    #[error("spawn failed: {0}")]
    Spawn(#[from] std::io::Error),

    /// Propagated from `cyrup-session` (fork-context branching, session opening).
    #[error(transparent)]
    Session(#[from] cyrup_session::SessionError),

    /// Propagated from `cyrup-core` (id/model/provider resolution, cancellation primitives).
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),

    /// Propagated from `cyrup-ext` (extension-host registration/dispatch).
    #[error(transparent)]
    Ext(#[from] cyrup_ext::ExtError),
}

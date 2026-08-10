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

    /// No saved chain definition matched the requested name (`/run-chain`, R-SA-129), applying
    /// the identical exact-string-equality convention R-SA-008 mandates for agent names.
    #[error("chain not found: {0}")]
    ChainNotFound(String),

    /// A management (create/update/delete/rename) operation targeted a Builtin- or
    /// Package-sourced agent, which is read-only (R-SA-014).
    #[error("agent source is read-only: {0}")]
    ReadOnlySource(String),

    /// A malformed `subagents.*` settings field aborted discovery (R-SA-009).
    #[error("malformed subagents settings: {0}")]
    MalformedSettings(String),

    /// A management/control surface reported a user-facing failure whose text is ALREADY the exact
    /// upstream message (pi's `isError: true` results carry rendered prose, not an error code) —
    /// e.g. `view: "fleet"`'s child-safe refusal or `Unknown status view: …`. The `Display` impl is
    /// therefore the bare message with no added prefix: prefixing it would corrupt a string this
    /// crate's parity tests pin against pi verbatim.
    #[error("{0}")]
    Management(String),

    /// An EXPLICITLY requested model (tool-call `model`, `/run [model=…]`, a chain step's `model`)
    /// fell outside the configured `subagents.modelScope` allow list, so the run was REFUSED
    /// before any child process was spawned (SUBA-003; pi `resolveSubagentModelOverride`'s
    /// `throw new Error(violation.message)`, `runs/shared/model-fallback.ts:207`).
    ///
    /// Carries pi's verbatim violation text (`Model '…' is outside the configured subagent model
    /// scope. Allowed patterns: … .`) as the whole message, so the refusal — and the reason for it
    /// — reaches the caller unaltered. Enforcement is deliberately fail-CLOSED: there is no
    /// substitute-an-allowed-model path, because a silent downgrade would run a different model
    /// than requested while reporting success.
    #[error("{0}")]
    ModelOutOfScope(String),

    /// The per-SESSION subagent spawn budget (`subagents.maxSubagentSpawnsPerSession`) would be
    /// exceeded by this dispatch, so the WHOLE call was refused before any child was planned
    /// (SUBA-002; pi `reserveSubagentSpawns`, `runs/foreground/subagent-executor.ts:266-282`).
    ///
    /// Raised by the SLASH surfaces (`/run`, `/chain`, `/parallel`, `/run-chain`), which reach
    /// execution through [`crate::extension::SubagentsExtension::dispatch_slash`] rather than the
    /// `subagent` tool's own `execute`. Upstream needs no dedicated error for this because every
    /// slash handler funnels back into the SAME `executor.execute` the tool uses
    /// (`slash/slash-commands.ts` `runSlashSubagent` -> `requestSlashRun` -> the bridge wired at
    /// `extension/index.ts:512-517` -> `executeSubagentCollapsed` -> `executor.execute`), so its one
    /// reserve covers both surfaces; in this crate the slash surface is a separate entry point and
    /// charges the budget itself.
    ///
    /// Carries pi's verbatim over-limit notice (`Subagent spawn limit reached for this session
    /// (N/M used, K requested). Complete the work directly or start a new session.`) as the whole
    /// message — byte-identical to the text the tool path returns as its `ToolError` — so the
    /// refusal reads the same on either surface.
    #[error("{0}")]
    SpawnLimitExceeded(String),

    /// Acceptance-gate evaluation rejected an otherwise-clean run (R-SA-011/033).
    #[error("acceptance rejected: {0}")]
    AcceptanceRejected(String),

    /// `output_mode == "file-only"` was requested without an `output_path` (R-SA-025).
    #[error("output-file mode requires an output path")]
    OutputPathRequired,

    /// The child's structured output was absent or failed schema validation (R-SA-030).
    #[error("structured output missing or invalid: {0}")]
    StructuredOutputInvalid(String),

    /// A `{outputs.name}` chain-output template reference was malformed, or named an output no
    /// strictly-earlier step produced (R-SA-053; pi `ChainOutputValidationError`,
    /// `chain-outputs.ts:85-93`). Carries pi's exact user/LLM-facing message verbatim so the
    /// observable diagnostic matches (`Unknown chain output reference '{outputs.x}'.` /
    /// `Invalid chain output reference '…'. Use {outputs.name} with /^[A-Za-z_][A-Za-z0-9_]*$/ names.`).
    #[error("{0}")]
    ChainOutputInvalid(String),

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

    /// G77 — a resume was requested against a run whose terminal state is
    /// [`crate::background::RunState::Stopped`]. pi throws this rather than reviving
    /// (`runs/background/async-resume.ts:406` @v0.43.0: `if (state === "stopped") throw new
    /// Error(\`Async run '${runId}' was stopped and cannot be resumed. Start a new run
    /// instead.\`);`), and the message is reproduced verbatim here because it is what the model
    /// reads back from a refused `action: "resume"`.
    ///
    /// Deliberately NOT folded into [`Self::ResumeNoTranscript`]: a stopped run's children very
    /// often DO have persisted transcripts, so the no-transcript variant would never fire for
    /// them and the run would be silently revived — the exact behaviour upstream added this throw
    /// to prevent.
    #[error("Async run '{0}' was stopped and cannot be resumed. Start a new run instead.")]
    ResumeStopped(String),

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

    /// Propagated from `cyrup-config` (settings-store read/write, e.g. the targeted `subagents`
    /// key merge a named-profile load performs, R-SA-141).
    #[error(transparent)]
    Config(#[from] cyrup_config::ConfigError),
}

//! `SessionServiceError` — the aggregate error surface of the facade (arch-11 §8).
//!
//! Wraps every subsystem error the seam composes (agent, session/compaction, config, resources,
//! ext, core) plus facade-local failures, so a single front-end-facing `Result` type is returned
//! from every `AgentSession` method.

use cyrup_session::compaction::CompactionError;

/// The aggregate error of the `AgentSession` facade (arch-11 §8). `thiserror` per arch-00 §8; the
/// only `anyhow` boundary is the `cyrup` binary, never this crate.
#[derive(Debug, thiserror::Error)]
pub enum SessionServiceError {
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),

    #[error("agent: {0}")]
    Agent(#[from] cyrup_agent::AgentError),

    #[error("session: {0}")]
    Session(#[from] cyrup_session::SessionError),

    #[error("compaction: {0}")]
    Compaction(#[from] CompactionError),

    #[error("config: {0}")]
    Config(#[from] cyrup_config::ConfigError),

    #[error("resources: {0}")]
    Resources(#[from] cyrup_resources::ResourceError),

    #[error("extension host: {0}")]
    Extension(#[from] cyrup_ext::ExtError),

    #[error("context load: {0}")]
    Context(#[from] cyrup_session::prompt::ContextError),

    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("no configured auth for model: {0}")]
    NoConfiguredAuth(String),

    /// A prompt / manual compaction was attempted on a session that has NO model — pi
    /// `if (!this.model) { throw new Error(formatNoModelSelectedMessage()); }`
    /// (agent-session.ts:1178-1180 for `prompt`, :1790-1792 for `compact`).
    ///
    /// This is the state a credential-less first run legitimately launches in
    /// (`main.ts:852-855` excludes `interactive` from the modelless hard stop on purpose), so the
    /// message is the `/login` → `/model` instruction, not a fatal diagnostic. The string is
    /// verbatim pi (`formatNoModelSelectedMessage`, auth-guidance.ts:18-20) because it is what an
    /// RPC client, the SDK and the TUI error line all surface.
    #[error("{}", crate::auth_guidance::format_no_model_selected_message())]
    NoModelSelected,

    /// A branch navigation asked for a summary on a modelless session — pi
    /// `if (options.summarize && !this.model) { throw new Error("No model available for
    /// summarization"); }` (agent-session.ts:2910-2912). Distinct string from
    /// [`Self::NoModelSelected`], matching pi.
    #[error("No model available for summarization")]
    NoModelForSummarization,

    #[error("agent is streaming; specify steer or follow_up")]
    StreamingNeedsBehavior,

    /// A `/command` that maps to a registered extension command was passed to `steer`/`follow_up`
    /// (Pi `_throwIfExtensionCommand`, agent-session.ts:1312-1321). Extension commands cannot be
    /// queued; the message carries the command name 1:1 with Pi's thrown `Error`.
    #[error(
        "Extension command \"/{0}\" cannot be queued. Use prompt() or execute the command when not streaming."
    )]
    ExtensionCommandNotQueueable(String),

    #[error("the session has no active run to operate on")]
    NoActiveRun,

    /// `prepareCompaction` produced nothing and the branch does NOT already end in a compaction —
    /// the transcript is too small to summarize (Pi `throw new Error("Nothing to compact (session
    /// too small)")`, agent-session.ts:1806). The message is verbatim Pi: it is what an RPC client
    /// and the SDK surface as the failure reason.
    #[error("Nothing to compact (session too small)")]
    NothingToCompact,

    /// `prepareCompaction` produced nothing because the last entry on the branch is already a
    /// `compaction` (Pi `throw new Error("Already compacted")`, agent-session.ts:1804).
    #[error("Already compacted")]
    AlreadyCompacted,

    /// A `session_before_compact` handler vetoed, or the compaction was aborted before the entry was
    /// appended (Pi `throw new Error("Compaction cancelled")`, agent-session.ts:1824 and :1869). The
    /// exact string is load-bearing upstream — Pi's own catch classifies an abort by comparing
    /// `message === "Compaction cancelled"` (agent-session.ts:1911).
    #[error("Compaction cancelled")]
    CompactionCancelled,

    #[error("invalid entry id for forking: {0}")]
    InvalidForkEntry(String),

    /// A loaded extension asked for a RUNTIME-tier control op (`newSession`/`switchSession`/`fork`/
    /// `reload`) on a session that no host installed an [`crate::RuntimeActions`] sink onto — a bare
    /// [`crate::AgentSession`] built straight from [`crate::SessionBuilder`] rather than through an
    /// [`crate::AgentSessionRuntime`]. Pi's equivalent is the pre-`bindCommandContext` action stub,
    /// which throws `"Extension runtime not initialized…"` (extensions/loader.ts:173-176
    /// `notInitialized`) rather than silently doing nothing. The op name is carried so the
    /// diagnostic names what was refused.
    #[error("control op `{0}` requires a session runtime host; none is installed on this session")]
    NoRuntimeHost(&'static str),

    #[error("import file not found: {0}")]
    ImportFileNotFound(String),

    #[error("the resumed session's cwd no longer exists: {0}")]
    MissingSessionCwd(String),

    #[error("session io: {0}")]
    Io(String),

    /// `/fork` or `/clone` on a persisted session whose file has not been written yet. The `Display`
    /// is pi's sentence **verbatim** — `throw new Error("This session has not been saved yet. Wait
    /// for the first assistant response before cloning or forking it.")`
    /// (`core/agent-session-runtime.ts:312-316` @v0.83.0, identical at v0.84.1) — because it is
    /// user-facing text, relayed straight through the RPC `fork`/`clone` `error` field
    /// (`cyrup-modes/src/rpc.rs`) and into whatever a client renders. SEAM-056.
    #[error(
        "This session has not been saved yet. Wait for the first assistant response before cloning \
         or forking it."
    )]
    SessionNotSaved,

    /// A genuine immediate-bash backend failure (spawn error, missing cwd, …) — Pi's
    /// `executeBashWithOperations` only catches the abort case in its `catch` block; every other
    /// error hits `throw err` (`bash-executor.ts:154`), which propagates out of
    /// `AgentSession.executeBash` uncaught (`agent-session.ts:2628-2643`: `recordBashResult` is only
    /// reached on the success path inside `try`) straight to the RPC dispatcher's `catch`
    /// (`rpc-mode.ts:756-772`), which converts it into an `error(...)` response with NO history
    /// entry ever recorded. Mirror that: never fabricate a "successful" [`crate::BashResult`] out of
    /// a real backend error.
    #[error("bash: {0}")]
    Bash(#[from] cyrup_core::ToolError),
}

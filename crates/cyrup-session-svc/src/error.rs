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

    #[error("no model available in provider catalog '{0}'")]
    NoModels(String),

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

    #[error("invalid entry id for forking: {0}")]
    InvalidForkEntry(String),

    #[error("import file not found: {0}")]
    ImportFileNotFound(String),

    #[error("the resumed session's cwd no longer exists: {0}")]
    MissingSessionCwd(String),

    #[error("session io: {0}")]
    Io(String),

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

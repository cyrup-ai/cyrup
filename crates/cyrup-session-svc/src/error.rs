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

    #[error("no model available in provider catalog '{0}'")]
    NoModels(String),

    #[error("agent is streaming; specify steer or follow_up")]
    StreamingNeedsBehavior,

    #[error("the session has no active run to operate on")]
    NoActiveRun,
}

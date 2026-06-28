//! Agent runtime error vocabulary (arch-02 §8). `thiserror` only; never `anyhow`.

/// Hook failure (arch-02 §8). A hook signals failure by returning `Err`; the loop degrades per
/// the failure-mode map (func-02 R-02-050) rather than panicking.
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

impl HookError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// Top-level agent error (arch-02 §8). Returned by the run entry points and lifecycle methods.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// At most one run may be active (func-02 R-02-006).
    #[error("a run is already active")]
    RunActive,
    /// `continue` requires a non-empty transcript (func-02 R-02-003).
    #[error("cannot continue: no messages")]
    NoMessages,
    /// `continue` cannot resume from an assistant message with empty queues (func-02 R-02-003/005).
    #[error("cannot continue from an assistant message")]
    ContinueFromAssistant,
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Hook(#[from] HookError),
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

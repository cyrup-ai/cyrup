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

/// Which entry point refused the call because a run was already in flight.
///
/// AGENT-034 — pi throws **four different messages** for this one condition, one per entry point,
/// and the strings are asserted by pi's own suite
/// (`packages/agent/test/agent.test.ts:508-547`, `:548-583` @v0.83.0). Collapsing them into a
/// single Rust string loses text that reaches the user: `AgentError` is wrapped verbatim by
/// `SessionServiceError::Agent` (`crates/cyrup-session-svc/src/error.rs:16-17`, `"agent: {0}"`).
/// The discriminant exists so each site keeps pi's literal text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyEntry {
    /// [`crate::Agent::prompt`] / `prompt_with_images` (Pi `prompt`,
    /// `packages/agent/src/agent.ts:340-344` @v0.83.0, `:351-355` @v0.84.1).
    Prompt,
    /// [`crate::Agent::continue_run`] (Pi `continue`, `agent.ts:351-353` @v0.83.0, `:362-364`).
    Continue,
    /// [`crate::Agent::reset`] (Pi `reset`, `agent.ts:334-336` @v0.84.1 — v0.83.0's `reset()` has
    /// no guard at all, which is the drift AGENT-023 ported).
    Reset,
    /// The run latch itself (Pi `runWithLifecycle`, `agent.ts:472-474` @v0.83.0, `:487-489`).
    /// In pi this is unreachable from `prompt`/`continue` because their own guards fire first on a
    /// single thread; in cyrup it is the residual check-then-claim race those guards cannot close.
    Run,
}

impl std::fmt::Display for BusyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            // agent.ts:341-343 @v0.83.0 — byte-identical at v0.84.1 (:352-354).
            Self::Prompt => {
                "Agent is already processing a prompt. Use steer() or followUp() to queue \
                 messages, or wait for completion."
            }
            // agent.ts:352 @v0.83.0 / :363 @v0.84.1.
            Self::Continue => "Agent is already processing. Wait for completion before continuing.",
            // agent.ts:335 @v0.84.1 (no v0.83.0 counterpart).
            Self::Reset => "Agent is already processing. Wait for completion before resetting.",
            // agent.ts:473 @v0.83.0 / :488 @v0.84.1.
            Self::Run => "Agent is already processing.",
        })
    }
}

/// Which `continue` surface refused an empty transcript.
///
/// AGENT-034 — pi's high-level [`crate::Agent::continue_run`] and the low-level
/// `agentLoopContinue`/`runAgentLoopContinue` free functions use **different** strings for the same
/// condition; `agentLoopContinue`'s is asserted verbatim by
/// `packages/agent/test/agent-loop.test.ts:1368-1385` @v0.83.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinueSurface {
    /// [`crate::Agent::continue_run`] (Pi `Agent.continue`, `agent.ts:357` @v0.83.0, `:368`).
    Agent,
    /// [`crate::agent_loop_continue`] / [`crate::run_agent_loop_continue`] (Pi `agentLoopContinue`
    /// `agent-loop.ts:71`, `runAgentLoopContinue` `:128` — identical offsets at both tags).
    Loop,
}

impl std::fmt::Display for ContinueSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Agent => "No messages to continue from",
            Self::Loop => "Cannot continue: no messages in context",
        })
    }
}

/// Top-level agent error (arch-02 §8). Returned by the run entry points and lifecycle methods.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// At most one run may be active (func-02 R-02-006). The payload selects pi's per-entry-point
    /// message (AGENT-034); see [`BusyEntry`].
    #[error("{0}")]
    RunActive(BusyEntry),
    /// `continue` requires a non-empty transcript (func-02 R-02-003). The payload selects pi's
    /// per-surface message (AGENT-034); see [`ContinueSurface`].
    #[error("{0}")]
    NoMessages(ContinueSurface),
    /// `continue` cannot resume from an assistant message with empty queues (func-02 R-02-003/005).
    /// One string on all three of pi's sites (`agent.ts:373` @v0.83.0, `agent-loop.ts:75`, `:132`).
    #[error("Cannot continue from message role: assistant")]
    ContinueFromAssistant,
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Hook(#[from] HookError),
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

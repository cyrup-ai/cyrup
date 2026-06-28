//! cyrup-agent — the turn-based agent loop (arch-02; conformance: func-02).
//!
//! Ordered event stream, parallel/sequential tool execution, the `Hooks` mutating seam +
//! notify-only `EventSubscriber`, steering/follow-up queues, abort/idle, agent state.
//!
//! Scaffold stub.

/// Agent runtime error (arch-02 §8). Scaffold placeholder.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}

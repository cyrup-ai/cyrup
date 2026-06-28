//! cyrup-session-svc — the `AgentSession` facade (arch-11; the single integration seam).
//!
//! Wires provider + agent + tools + session + config + resources + ext into the one surface every
//! front-end (`cyrup-tui`/`cyrup-modes`/`cyrup-sdk`) and embedder consumes (func-11 R-11-023).
//!
//! Scaffold stub.

/// AgentSession facade error (arch-11 §8). Scaffold placeholder.
#[derive(Debug, thiserror::Error)]
pub enum SessionServiceError {
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}

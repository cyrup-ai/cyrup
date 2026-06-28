//! cyrup-modes — print / json / rpc adapters (arch-11; conformance: func-11).
//!
//! Thin adapters over the `AgentSession` seam: print/JSON one-shot output and the RPC
//! strict-LF-JSONL protocol (framing + command set + extension-UI requests).
//!
//! Scaffold stub.

/// Runtime-mode error (arch-11 §8). Scaffold placeholder.
#[derive(Debug, thiserror::Error)]
pub enum ModeError {
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}

//! cyrup-session — sessions, compaction, context (arch-04/05/06; conformance: func-04/05/06).
//!
//! Append-only JSONL session tree (entries/leaf, fork/clone/resume), compaction + branch
//! summaries, and system-prompt/context assembly (AGENTS.md discovery, skills injection).
//!
//! Scaffold stub.

/// Session/compaction/context error (arch-04/05/06 §8). Scaffold placeholder.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}

//! Compaction error vocabulary (arch-05 §8). Library policy: `thiserror`, no panics on any path
//! reachable from model output, session files, or extension I/O.

use crate::error::SessionError;

#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    /// Cancel token fired or the summary terminal was `aborted`.
    #[error("compaction cancelled")]
    Aborted,
    /// The summarization model returned `stop_reason == error`.
    #[error("summarization failed: {0}")]
    Summarization(String),
    /// A required `firstKeptEntryId` could not be resolved (session may need migration).
    #[error("session needs migration: entry has no id")]
    MissingEntryId,
    /// Hook dispatch faulted (bridge/guest fault surfaced).
    #[error("hook dispatch failed: {0}")]
    Hook(String),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

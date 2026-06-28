//! Session error vocabulary (arch-04 §8). Library policy: `thiserror`, every fallible op returns
//! `Result<_, SessionError>`; malformed/truncated reads degrade rather than error (R-04-034).

use std::path::PathBuf;

use cyrup_core::EntryId;

/// Errors surfaced by the session store. Tolerant-read paths never produce an error for a
/// malformed trailing line (the valid prefix is kept and a warning is recorded instead).
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize entry: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("not a valid cyrup/pi session: {path}")]
    NotASession { path: PathBuf },
    #[error("session not found: {what}")]
    NotFound { what: String },
    #[error("ambiguous selector '{prefix}' matched {n} sessions")]
    AmbiguousSelector { prefix: String, n: usize },
    #[error("entry not found: {0}")]
    EntryNotFound(EntryId),
    #[error("cannot fork: source empty/invalid: {0}")]
    EmptyFork(PathBuf),
    #[error("session file already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("operation requires a persisted session")]
    NotPersisted,
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

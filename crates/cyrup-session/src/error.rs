//! Session error vocabulary (arch-04 §8). Library policy: `thiserror`, every fallible op returns
//! `Result<_, SessionError>`; malformed/truncated reads degrade rather than error (R-04-034).

use std::path::PathBuf;

use cyrup_core::EntryId;

/// Errors surfaced by the session store. Tolerant-read paths never produce an error for a
/// malformed trailing line (the valid prefix is kept and a warning is recorded instead).
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// A filesystem operation failed. `op` is a short verb phrase naming the step (`"open"`,
    /// `"append to"`, `"create directory"`, …) and `path` is the file or directory it was applied
    /// to, so the rendered message always identifies WHICH file failed — the bare
    /// `std::io::Error` text ("No such file or directory (os error 2)") never does.
    ///
    /// There is deliberately no blanket `#[from] std::io::Error`: every filesystem call site must
    /// go through [`SessionError::io`] and name its path, and the compiler enforces that for new
    /// ones.
    #[error("{op} {path}: {source}")]
    Io {
        /// Verb phrase for the failed step, e.g. `"open"` or `"rename temp file onto"`.
        op: &'static str,
        /// The file or directory the operation was applied to.
        path: PathBuf,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },
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
    #[error("{0}")]
    InvalidSessionId(String),
    #[error("cannot fork: source empty/invalid: {0}")]
    EmptyFork(PathBuf),
    #[error("session file already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("operation requires a persisted session")]
    NotPersisted,
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

impl SessionError {
    /// Build an [`SessionError::Io`] that names the file the failed operation was applied to.
    ///
    /// Written to be used point-free in a `map_err`, which is how every filesystem call site in
    /// this crate attaches its path:
    ///
    /// ```
    /// # use cyrup_session::SessionError;
    /// # use std::path::Path;
    /// # fn demo(path: &Path) -> Result<String, SessionError> {
    /// std::fs::read_to_string(path).map_err(|e| SessionError::io("read", path, e))
    /// # }
    /// ```
    pub fn io(op: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { op, path: path.into(), source }
    }
}

//! Cross-cutting error vocabulary (arch-00 §8). Other crates wrap these variants so the
//! abort/serde/io taxonomy stays consistent.

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Abort/cancellation (the `CancelToken` fired). Maps to a terminal `aborted`.
    #[error("cancelled")]
    Cancelled,
    /// Auth/credential-store failure (func-01 R-01-017 `auth`/`oauth`). Other crates wrap their
    /// typed auth errors into this for the shared vocabulary.
    #[error("auth: {0}")]
    Auth(String),
    /// Provider-resolution failure (unknown provider / missing catalog). func-01 R-01-017 `provider`.
    #[error("provider: {0}")]
    Provider(String),
    /// Stream/transport/decode failure surfaced as data (func-01 R-01-017 `stream`).
    #[error("stream: {0}")]
    Stream(String),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

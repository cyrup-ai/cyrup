//! [`IntercomError`] — the crate's error type (thiserror). Faithful to pi-intercom's error
//! surfaces, which are plain `Error` messages closing a connection or failing a spawn/probe.

/// Errors surfaced by the intercom transport, broker, and tools.
#[derive(Debug, thiserror::Error)]
pub enum IntercomError {
    /// A frame exceeded [`crate::transport::framing::MAX_FRAME_BYTES`] or the stream produced a
    /// malformed length prefix — fatal for the connection (pi `framing.ts:63-66`).
    #[error("intercom frame error: {0}")]
    Framing(String),

    /// A JSON (de)serialization failure on a wire frame — treated as fatal for the connection
    /// (pi `framing.ts:33-36`, `broker.ts:231-233`, `client.ts:242-251`).
    #[error("intercom protocol error: {0}")]
    Protocol(String),

    /// An underlying I/O failure on the socket / runtime files.
    #[error("intercom io error: {0}")]
    Io(String),

    /// The broker could not be spawned or did not become healthy within the timeout
    /// (pi `spawn.ts:213-236,386`).
    #[error("intercom broker error: {0}")]
    Broker(String),

    /// A client operation failed because the client is not connected / is disconnecting
    /// (pi `client.ts:147-162`).
    #[error("intercom client error: {0}")]
    Client(String),
}

impl From<std::io::Error> for IntercomError {
    fn from(e: std::io::Error) -> Self {
        IntercomError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for IntercomError {
    fn from(e: serde_json::Error) -> Self {
        IntercomError::Protocol(e.to_string())
    }
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, IntercomError>;

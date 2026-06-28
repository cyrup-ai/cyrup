//! [`ModesError`] — the front-end-facing error of the non-interactive adapters (arch-11 §8).
//!
//! Aggregates the seam error ([`cyrup_session_svc::SessionServiceError`]) plus the I/O and
//! (de)serialization failures the adapters add. `thiserror` per arch-00 §8; the only `anyhow`
//! boundary is the `cyrup` binary, never this crate.

use cyrup_session_svc::SessionServiceError;

/// Error surface of the print / json / rpc adapters (arch-11 §8).
#[derive(Debug, thiserror::Error)]
pub enum ModesError {
    /// A failure inside the `AgentSession` seam (prompt rejected, model not found, …).
    #[error(transparent)]
    Session(#[from] SessionServiceError),

    /// Writing to (or reading from) the adapter's I/O sink failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Serializing an event/response or deserializing a command failed.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

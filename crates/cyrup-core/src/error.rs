//! Cross-cutting error vocabulary (arch-00 §8). Other crates wrap these variants so the
//! abort/serde/io taxonomy stays consistent.

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("cancelled")]
    Cancelled,
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

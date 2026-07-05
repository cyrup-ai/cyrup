//! `PermissionError` — this crate's error vocabulary (thiserror only; libs never use `anyhow`,
//! arch-00 §8). Kept small: the policy engine degrades gracefully (a malformed policy file → the
//! `ask` fallback, never an error), so the two genuine errors are I/O at an explicit store/spool
//! write and an unsafe forwarding path token. Most engine paths return values, not `Result`.

/// A permission-system error (arch-00 §8).
#[derive(Debug, thiserror::Error)]
pub enum PermissionError {
    /// A filesystem operation on a store/config/spool file failed.
    #[error("io: {0}")]
    Io(String),
    /// A forwarding-spool path token (an encoded session id) failed the safe-token / contains-root
    /// guard before any `Path::join` (P-4/§7.4, reusing `cyrup_ext_subagents::validate_safe_token` /
    /// `validate_contains_root` — R-PERM-040). Never joined; the forward fail-closes to deny.
    #[error("unsafe forwarding path token: {0}")]
    UnsafeToken(String),
}

impl From<std::io::Error> for PermissionError {
    fn from(e: std::io::Error) -> Self {
        PermissionError::Io(e.to_string())
    }
}

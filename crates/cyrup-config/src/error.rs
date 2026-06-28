//! Typed error vocabulary for `cyrup-config` (arch-07 §8).

use std::path::PathBuf;

use crate::settings::SettingsScope;

/// Configuration / trust error surface (arch-07 §8).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("settings parse error in {scope:?}: {source}")]
    SettingsParse {
        scope: SettingsScope,
        #[source]
        source: serde_json::Error,
    },
    #[error("trust store: {0}")]
    Trust(String),
    #[error("config dir resolution: {0}")]
    Dir(String),
    #[error("project is not trusted; refusing to write project settings")]
    Untrusted,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("lock contention on {path}")]
    Lock { path: PathBuf },
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

/// Credential-store error surface (arch-07 §8). OAuth refresh failure preserves the stored
/// credential and never falls back to the environment (R-07-017 / A-07-5).
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("auth store io: {0}")]
    Io(#[from] std::io::Error),
    #[error("auth file parse: {0}")]
    Parse(#[source] serde_json::Error),
    #[error("oauth refresh failed (credential preserved): {0}")]
    Oauth(String),
    #[error("lock: {0}")]
    Lock(String),
    #[error("cancelled")]
    Cancelled,
}

/// A non-fatal, scope-tagged load error surfaced to the UI instead of panicking (R-00-009).
#[derive(Debug, Clone)]
pub struct ScopedError {
    pub scope: SettingsScope,
    pub message: String,
}

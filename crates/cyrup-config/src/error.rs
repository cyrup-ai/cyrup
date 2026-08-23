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
    /// A settings write was REFUSED because that scope's file could not be parsed (CFG-001).
    ///
    /// Ports Pi's writer guards: `settings-manager.ts` `save()` (≈:614-628) opens with
    /// `if (this.globalSettingsLoadError) { return; }` and `saveProjectSettings()` (≈:633-646) has
    /// the mirror `if (this.projectSettingsLoadError) return;`. Rewriting the document from the
    /// degraded in-memory view would drop every key the user actually has on disk, so the write is
    /// abandoned and the file is left byte-for-byte intact for the user to repair. Unlike Pi — which
    /// returns silently — cyrup surfaces this to the caller so a `/config` toggle can say why it
    /// did not stick.
    #[error(
        "refusing to write {scope:?} settings: the file could not be parsed ({message}); \
         fix or remove it, then retry — the existing file was left unchanged"
    )]
    SettingsWriteRefused {
        scope: SettingsScope,
        message: String,
    },
    /// A settings VALUE failed validation (Pi `parseTimeoutSetting` throws).
    #[error("Invalid {key} setting: {value}")]
    InvalidSetting { key: String, value: String },
    /// An in-memory settings lock was poisoned by a panic in another thread.
    #[error("settings lock poisoned")]
    LockPoisoned,
    #[error("io on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("lock contention on {path}")]
    Lock { path: PathBuf },
    /// A `spawn_blocking` job inside [`crate::lock::FileLock::acquire`] never produced a result:
    /// the task panicked (unwinding builds only — the release profile is `panic = "abort"`), or
    /// the runtime dropped it while shutting down.
    ///
    /// Deliberately NOT [`Self::Lock`]: nothing was contended, and "lock contention on …" sends an
    /// operator looking for a competing process that does not exist. Deliberately not
    /// [`Self::Cancelled`] either — that one means the caller's own `CancelToken` fired, which is a
    /// user-initiated abort rather than a failure, and `models_store::store_err` turns it into
    /// `ProviderError::Aborted`. `message` is the `JoinError`'s own `Display`, which carries the
    /// panic payload when there is one.
    #[error("lock acquisition for {path} failed to run to completion: {message}")]
    LockTaskFailed { path: PathBuf, message: String },
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

/// Credential-store error surface (arch-07 §8). OAuth refresh failure preserves the stored
/// credential and never falls back to the environment (R-07-017 / A-07-5).
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("auth store io on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("auth file parse: {0}")]
    Parse(#[source] serde_json::Error),
    #[error("oauth refresh failed (credential preserved): {0}")]
    Oauth(String),
    #[error("lock: {0}")]
    Lock(String),
    /// Credential-file I/O that went through the crate's locked / atomic write path
    /// ([`crate::lock::write_atomic`]), which returns [`ConfigError`], not `io::Error`.
    #[error("auth store: {0}")]
    Config(#[from] ConfigError),
    #[error("cancelled")]
    Cancelled,
}

/// A non-fatal, scope-tagged load error surfaced to the UI instead of panicking (R-00-009).
#[derive(Debug, Clone)]
pub struct ScopedError {
    pub scope: SettingsScope,
    pub message: String,
}

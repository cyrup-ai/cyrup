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
    ///
    /// `cause` is the typed load failure the latch holds — never recovered by matching on its text
    /// (CFG-088: that text is `JSON.parse`'s own message now, with no marker of its own).
    #[error(
        "refusing to write {scope:?} settings: the file could not be {verb} ({cause}); \
         fix or remove it, then retry — the existing file was left unchanged",
        verb = cause.verb()
    )]
    SettingsWriteRefused {
        scope: SettingsScope,
        cause: SettingsLoadError,
    },
    /// A settings VALUE failed validation (Pi `parseTimeoutSetting` throws).
    #[error("Invalid {key} setting: {value}")]
    InvalidSetting { key: String, value: String },
    /// A `compaction.reserveTokens`/`keepRecentTokens` value, ordinary or under
    /// `compaction.modelOverrides`, is not a non-negative safe integer, or an override entry is
    /// not an object. The payload is pi's `Error` message verbatim (`getCompactionTokenSetting`,
    /// `settings-manager.ts:859-885` @v0.87.1).
    #[error("{0}")]
    InvalidCompactionSetting(String),
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
    /// [`crate::lock::BlockingFileLock::acquire`]'s bound ran out with a peer still holding the
    /// lock. pi-subagents' sentence verbatim (`src/shared/file-lease.ts:69` @ad11b7ab):
    /// `` throw new Error(`Timed out waiting for another process to finish updating ${absolute}.`) ``
    /// — `path` is that `absolute`.
    #[error("Timed out waiting for another process to finish updating {}.", path.display())]
    LockTimeout { path: PathBuf },
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

/// Why a settings scope failed to load — the `Error` pi's `tryLoadFromStorage` catches
/// (`settings-manager.ts:428-438` @v0.87.1) and latches into `globalSettingsLoadError` /
/// `projectSettingsLoadError`. Typed, so the write-refusal latch (CFG-001) and its message can tell
/// a file that would not parse from one that could not be read without inspecting the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsLoadError {
    /// The file is not valid JSON. Carries the message `JSON.parse` throws for it
    /// ([`crate::js_json::json_parse_error_message`]) — pi's `error.message` — or, for a document
    /// V8 accepts but cyrup cannot load, serde's own message.
    Parse(String),
    /// The store could not read the file.
    Read(String),
}

impl SettingsLoadError {
    /// The failure for `text`, which `Settings::parse` rejected with `err`: `JSON.parse`'s message
    /// when V8 would reject the (BOM-stripped, as pi's `stripBom`) text too, else serde's.
    pub fn parse(text: &str, err: &serde_json::Error) -> Self {
        SettingsLoadError::Parse(
            crate::js_json::json_parse_error_message(crate::strip_bom(text))
                .unwrap_or_else(|| err.to_string()),
        )
    }

    /// The failure's message — pi's `error.message`.
    pub fn message(&self) -> &str {
        match self {
            SettingsLoadError::Parse(m) | SettingsLoadError::Read(m) => m,
        }
    }

    /// What could not be done to the file, for [`ConfigError::SettingsWriteRefused`].
    fn verb(&self) -> &'static str {
        match self {
            SettingsLoadError::Parse(_) => "parsed",
            SettingsLoadError::Read(_) => "read",
        }
    }
}

impl std::fmt::Display for SettingsLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// A non-fatal, scope-tagged load error surfaced to the UI instead of panicking (R-00-009) — Pi
/// `SettingsError { scope, path?, error }` (`core/settings-manager.ts:214-218` @v0.87.1).
#[derive(Debug, Clone)]
pub struct ScopedError {
    pub scope: SettingsScope,
    /// The file the scope was read from, when the store is file-backed (pi's `settingsPaths`).
    pub path: Option<std::path::PathBuf>,
    pub error: SettingsLoadError,
}

impl ScopedError {
    /// The warning text pi's `collectSettingsDiagnostics` renders for this error
    /// (`core/settings-diagnostics.ts:4-9` @v0.87.1, v0.84.3): `Invalid settings file <path>: <msg>`,
    /// or `Invalid <scope> settings: <msg>` when there is no path. CFG-088.
    pub fn diagnostic_message(&self) -> String {
        match &self.path {
            Some(path) => format!("Invalid settings file {}: {}", path.display(), self.error),
            None => {
                let scope = match self.scope {
                    SettingsScope::Global => "global",
                    SettingsScope::Project => "project",
                };
                format!("Invalid {scope} settings: {}", self.error)
            }
        }
    }
}

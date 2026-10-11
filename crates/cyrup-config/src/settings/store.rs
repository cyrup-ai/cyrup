//! Serialized read-modify-write of one scope's raw JSON text (arch-07 §3.3), plus the file-backed
//! and in-memory implementations.

use std::path::{Path, PathBuf};

use super::types::SettingsScope;
use crate::error::ConfigError;

/// Serialized read-modify-write of one scope's raw JSON text (arch-07 §3.3).
///
/// `#[async_trait]` rather than a native `async fn` in trait: this is consumed as
/// `Arc<dyn SettingsStore>` (`manager.rs:26`), and a native async fn is not dyn-compatible.
///
/// **[CYRUP-DELTA]** The trait is HALF async on purpose. `with_lock` is `async` only because
/// [`crate::lock::FileLock::acquire`] is — that lock awaits an in-process keyed mutex and then a
/// non-blocking `flock` retry loop. `read` takes no lock in either impl: `FileSettingsStore::read`
/// is a bare `std::fs::read_to_string`, and [`InMemorySettingsStore`]'s is a `std::sync::Mutex`
/// lock plus a clone. It therefore has no suspension point at all, and `async` there would box a
/// future that never yields. It would also cascade: `read`'s only caller is
/// `SettingsManager::load_scope`, under `reload_internal`, under the sync `SettingsManager::load`,
/// `reload` and `set_project_trusted`. Upstream is sync on BOTH halves — `lockfile.lockSync`
/// behind a busy-wait retry loop annotated "Sleep synchronously to avoid changing callers to
/// async" (settings-manager.ts:206/218 @v0.83.0) and `readFileSync` (`:237`) — so the `async` is
/// confined to the half that takes a lock. Do not "finish the job".
#[async_trait::async_trait]
pub trait SettingsStore: Send + Sync {
    /// Read the current raw text for a scope (`None` if absent).
    fn read(&self, scope: SettingsScope) -> Result<Option<String>, ConfigError>;

    /// The file a scope is stored in, when there is one (Pi's `settingsPaths`, carried onto each
    /// `SettingsError` so the diagnostic can name the file). `None` for a store with no file.
    fn location(&self, _scope: SettingsScope) -> Option<PathBuf> {
        None
    }

    /// Serialized read-modify-write. `f` receives the current text (None if absent) and returns
    /// `Some(new)` to write or `None` to leave untouched.
    ///
    /// `for<'s>` is spelled out because `#[async_trait]` renames every elided `&` in the signature
    /// into a method-level named lifetime, and `CollectLifetimes::visit_type_reference_mut`
    /// recurses into the `Fn(..)` sugar as well: a plain `FnMut(Option<&str>)` here loses the
    /// implicit `for<'a>` that Fn-sugar elision would give it and becomes early-bound, chosen by
    /// the caller. Both impls hand `f` a borrow of a local (`current`, `guard`), which an
    /// early-bound lifetime cannot accept. `+ Send` is load-bearing too: the body is boxed as
    /// `Pin<Box<dyn Future + Send>>`, and `&mut T: Send` only when `T: Send`.
    async fn with_lock(
        &self,
        scope: SettingsScope,
        f: &mut (dyn for<'s> FnMut(Option<&'s str>) -> Option<String> + Send),
    ) -> Result<(), ConfigError>;
}

/// File-backed store with a cross-process advisory lock (arch-07 §5).
pub struct FileSettingsStore {
    global_path: PathBuf,
    project_path: PathBuf,
}

impl FileSettingsStore {
    pub fn new(global_path: PathBuf, project_path: PathBuf) -> Self {
        Self {
            global_path,
            project_path,
        }
    }

    fn path(&self, scope: SettingsScope) -> &Path {
        match scope {
            SettingsScope::Global => &self.global_path,
            SettingsScope::Project => &self.project_path,
        }
    }
}

#[async_trait::async_trait]
impl SettingsStore for FileSettingsStore {
    fn location(&self, scope: SettingsScope) -> Option<PathBuf> {
        Some(self.path(scope).to_path_buf())
    }

    fn read(&self, scope: SettingsScope) -> Result<Option<String>, ConfigError> {
        let path = self.path(scope);
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source: e,
            }),
        }
    }

    async fn with_lock(
        &self,
        scope: SettingsScope,
        f: &mut (dyn for<'s> FnMut(Option<&'s str>) -> Option<String> + Send),
    ) -> Result<(), ConfigError> {
        let path = self.path(scope);
        // CFG-112: lock and write the PHYSICAL file, not the spelling this store was built with.
        // `FileLock` keys on `<given path>.lock`, so locking a symlinked `settings.json` by its
        // link name took a different sidecar from the `cyrup-ext-subagents` settings writers,
        // which lock `<physical target>.lock` (SUBA-029/171/184, pi `withSettingsFileLease` →
        // `resolveSettingsWriteTarget`), and the two did not exclude each other; and
        // `write_atomic` renamed its temp over the link, replacing it with a plain file. The
        // resolution needs an existing physical parent, so the parent is created first — the
        // `ensure_dir` `FileLock::acquire` already performed on it before CFG-112.
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            crate::lock::ensure_dir(parent)?;
        }
        let target =
            super::resolve_settings_write_target(path).map_err(|source| ConfigError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        let _guard = crate::lock::FileLock::acquire(&target, None).await?;
        let current = match std::fs::read_to_string(&target) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(ConfigError::Io {
                    path: target.clone(),
                    source: e,
                });
            }
        };
        if let Some(new_text) = f(current.as_deref()) {
            // Atomic onto the target, keeping the link, the existing mode, and pi's refusal of a
            // read-only file (`writeFileSync` → `EACCES`).
            super::write_settings_target_atomic(&target, new_text.as_bytes())?;
        }
        Ok(())
    }
}

/// In-memory store for tests / non-persistent runs.
#[derive(Default)]
pub struct InMemorySettingsStore {
    global: std::sync::Mutex<Option<String>>,
    project: std::sync::Mutex<Option<String>>,
}

impl InMemorySettingsStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, scope: SettingsScope) -> &std::sync::Mutex<Option<String>> {
        match scope {
            SettingsScope::Global => &self.global,
            SettingsScope::Project => &self.project,
        }
    }

    pub fn seed(&self, scope: SettingsScope, text: &str) {
        if let Ok(mut g) = self.slot(scope).lock() {
            *g = Some(text.to_string());
        }
    }
}

#[async_trait::async_trait]
impl SettingsStore for InMemorySettingsStore {
    fn read(&self, scope: SettingsScope) -> Result<Option<String>, ConfigError> {
        Ok(self.slot(scope).lock().ok().and_then(|g| g.clone()))
    }

    async fn with_lock(
        &self,
        scope: SettingsScope,
        f: &mut (dyn for<'s> FnMut(Option<&'s str>) -> Option<String> + Send),
    ) -> Result<(), ConfigError> {
        let mut guard = self
            .slot(scope)
            .lock()
            .map_err(|_| ConfigError::LockPoisoned)?;
        if let Some(new) = f(guard.as_deref()) {
            *guard = Some(new);
        }
        Ok(())
    }
}

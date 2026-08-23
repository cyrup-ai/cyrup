//! Serialized read-modify-write of one scope's raw JSON text (arch-07 §3.3), plus the file-backed
//! and in-memory implementations.

use std::path::{Path, PathBuf};

use super::types::SettingsScope;
use crate::error::ConfigError;

/// Serialized read-modify-write of one scope's raw JSON text (arch-07 §3.3).
///
/// `#[async_trait]` rather than a native `async fn` in trait: this is consumed as
/// `Arc<dyn SettingsStore>` (`manager.rs:26`), and a native async fn is not dyn-compatible.
#[async_trait::async_trait]
pub trait SettingsStore: Send + Sync {
    /// Read the current raw text for a scope (`None` if absent).
    fn read(&self, scope: SettingsScope) -> Result<Option<String>, ConfigError>;

    /// Serialized read-modify-write. `f` receives the current text (None if absent) and returns
    /// `Some(new)` to write or `None` to leave untouched.
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
        let path = self.path(scope).to_path_buf();
        let _guard = crate::lock::FileLock::acquire(&path, None).await?;
        let current = match std::fs::read_to_string(&path) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(ConfigError::Io {
                    path: path.clone(),
                    source: e,
                });
            }
        };
        // Bound before the write: inside an `async` body the `if let` scrutinee's borrow of
        // `current` would otherwise still be live across the call below.
        let next = f(current.as_deref());
        if let Some(new_text) = next {
            crate::lock::write_atomic(&path, new_text.as_bytes(), false)?;
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
        // Same reason as above: bind first so the immutable borrow of `guard` ends before the
        // assignment takes it mutably.
        let next = f(guard.as_deref());
        if let Some(new) = next {
            *guard = Some(new);
        }
        Ok(())
    }
}

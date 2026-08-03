//! `<agent_dir>/models-store.json` — the on-disk cache behind the remote model-catalog overlay
//! (1:1 port of Pi `packages/coding-agent/src/core/models-store.ts` `FileModelsStore`; DRIFT-007).
//!
//! Upstream splits this exactly the way cyrup does: the `ModelsStore` interface and the in-memory
//! implementation live in the vendor-neutral AI layer (`cyrup-provider::models_store`), while the
//! locked, atomically-written file backend lives in the agent layer — here, because this crate owns
//! [`crate::lock::FileLock`] and [`crate::lock::write_atomic`] and `cyrup-provider` must not depend
//! on it (dependencies point downward: `cyrup-config` → `cyrup-provider`, never the reverse).
//!
//! Pi persists through the SAME machinery as `auth.json` (`FileAuthStorageBackend`,
//! `auth-storage.ts:45+`) — a cross-process lock plus an atomic replace at 0600. So does this, via
//! the sidecar-lock + temp-and-rename pair `crate::auth` already uses.
//!
//! # Failure posture
//!
//! Every method returns `Ok` on any recoverable problem. A missing file, a truncated file, a file
//! full of JSON that is not a store, an unwritable directory — all of them mean "no cached overlay",
//! which degrades cleanly to the compiled-in catalogs. The cache is an accelerator; it is never
//! allowed to be the reason a user sees fewer models or a failed start.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cyrup_provider::error::ProviderError;
use cyrup_provider::models_store::{ModelsStore, ModelsStoreEntry};

/// The file name Pi uses, resolved beside `models.json` (`model-runtime.ts:141-144`,
/// `join(dirname(modelsPath), "models-store.json")`).
pub const MODELS_STORE_FILE_NAME: &str = "models-store.json";

/// `<agent_dir>/models-store.json` for a resolved [`crate::env::ConfigDirs`].
pub fn models_store_path(dirs: &crate::env::ConfigDirs) -> PathBuf {
    // `models_path()` is `<agent_dir>/models.json`; Pi anchors the store to the same directory so a
    // relocated `--agent-dir` moves both together.
    dirs.models_path()
        .parent()
        .map_or_else(|| PathBuf::from(MODELS_STORE_FILE_NAME), |p| p.join(MODELS_STORE_FILE_NAME))
}

/// Locked, atomically-written JSON storage for dynamically refreshed provider catalogs.
///
/// The on-disk shape is Pi's exactly: one JSON object mapping provider id → `ModelsStoreEntry`,
/// pretty-printed (`JSON.stringify(current, null, 2)`).
pub struct FileModelsStore {
    path: PathBuf,
}

impl FileModelsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The store that sits beside `<agent_dir>/models.json`.
    pub fn for_dirs(dirs: &crate::env::ConfigDirs) -> Self {
        Self::new(models_store_path(dirs))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the whole file as provider id → raw JSON value.
    ///
    /// Entries are kept as `serde_json::Value` rather than typed up front so ONE malformed provider
    /// entry cannot take the rest of the cache down with it — the typed conversion happens per
    /// provider in [`FileModelsStore::read`], where a failure is simply "no overlay for that one".
    fn read_all(&self) -> BTreeMap<String, serde_json::Value> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return BTreeMap::new();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    fn write_all(&self, entries: &BTreeMap<String, serde_json::Value>) {
        // `JSON.stringify(current, null, 2)` + the trailing newline every other cyrup config file
        // gets. A serialization failure here is unreachable (the map came from `Value`s), and is
        // swallowed rather than propagated for the same reason the rest of this file is infallible.
        if let Ok(mut text) = serde_json::to_string_pretty(entries) {
            text.push('\n');
            // 0600 + 0700 parent, matching Pi's `FileAuthStorageBackend`. The catalog itself is not
            // secret, but the file lives in the agent dir beside credentials and inherits its posture.
            let _ = crate::lock::write_atomic(&self.path, text.as_bytes(), true);
        }
    }
}

#[async_trait::async_trait]
impl ModelsStore for FileModelsStore {
    async fn read(&self, provider_id: &str) -> Result<Option<ModelsStoreEntry>, ProviderError> {
        // The lock is advisory and cross-process: a concurrent `cyrup update --models` in another
        // terminal must not be observed mid-rename.
        let _guard = crate::lock::FileLock::acquire(&self.path).ok();
        Ok(self
            .read_all()
            .get(provider_id)
            .and_then(|v| serde_json::from_value::<ModelsStoreEntry>(v.clone()).ok()))
    }

    async fn write(&self, provider_id: &str, entry: ModelsStoreEntry) -> Result<(), ProviderError> {
        let _guard = crate::lock::FileLock::acquire(&self.path).ok();
        let mut all = self.read_all();
        if let Ok(value) = serde_json::to_value(&entry) {
            all.insert(provider_id.to_string(), value);
            self.write_all(&all);
        }
        Ok(())
    }

    async fn delete(&self, provider_id: &str) -> Result<(), ProviderError> {
        let _guard = crate::lock::FileLock::acquire(&self.path).ok();
        let mut all = self.read_all();
        if all.remove(provider_id).is_some() {
            self.write_all(&all);
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn entry(etag: &str) -> ModelsStoreEntry {
        ModelsStoreEntry {
            models: Vec::new(),
            last_modified: Some(5),
            checked_at: Some(6),
            etag: Some(etag.to_string()),
        }
    }

    #[tokio::test]
    async fn round_trips_through_the_file_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MODELS_STORE_FILE_NAME);
        {
            let store = FileModelsStore::new(&path);
            store.write("groq", entry("\"v1\"")).await.unwrap();
            store.write("xai", entry("\"v2\"")).await.unwrap();
        }
        // A brand-new instance (i.e. a fresh process) sees the persisted entries — this is what makes
        // a restart NOT refetch.
        let reopened = FileModelsStore::new(&path);
        assert_eq!(
            reopened.read("groq").await.unwrap().unwrap().etag.as_deref(),
            Some("\"v1\"")
        );
        assert_eq!(
            reopened.read("xai").await.unwrap().unwrap().etag.as_deref(),
            Some("\"v2\"")
        );

        // The on-disk shape is Pi's: one object keyed by provider id.
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(raw.get("groq").is_some() && raw.get("xai").is_some());

        // Deleting one leaves the other.
        reopened.delete("groq").await.unwrap();
        assert!(reopened.read("groq").await.unwrap().is_none());
        assert!(reopened.read("xai").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_missing_or_corrupt_file_reads_as_no_overlay_and_never_errors() {
        let dir = tempfile::tempdir().unwrap();

        // Missing.
        let missing = FileModelsStore::new(dir.path().join("nope.json"));
        assert!(missing.read("groq").await.unwrap().is_none());

        // Corrupt whole-file.
        let path = dir.path().join(MODELS_STORE_FILE_NAME);
        std::fs::write(&path, "{ not json").unwrap();
        let corrupt = FileModelsStore::new(&path);
        assert!(corrupt.read("groq").await.unwrap().is_none());
        // ...and a write still repairs it rather than failing.
        corrupt.write("groq", entry("\"v1\"")).await.unwrap();
        assert!(corrupt.read("groq").await.unwrap().is_some());

        // ONE malformed provider entry must not take the others down.
        std::fs::write(
            &path,
            r#"{"groq": "not an entry", "xai": {"models": [], "etag": "\"ok\""}}"#,
        )
        .unwrap();
        let partial = FileModelsStore::new(&path);
        assert!(partial.read("groq").await.unwrap().is_none());
        assert_eq!(
            partial.read("xai").await.unwrap().unwrap().etag.as_deref(),
            Some("\"ok\"")
        );
    }

    #[tokio::test]
    async fn an_unwritable_path_is_not_an_error() {
        // A directory where a file should be: every write fails, every read is empty, nothing panics
        // and nothing propagates — the run degrades to the embedded catalogs.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("as-a-dir");
        std::fs::create_dir(&path).unwrap();
        let store: Arc<dyn ModelsStore> = Arc::new(FileModelsStore::new(&path));
        store.write("groq", entry("\"v1\"")).await.unwrap();
        assert!(store.read("groq").await.unwrap().is_none());
        store.delete("groq").await.unwrap();
    }

    #[test]
    fn the_store_sits_beside_models_json() {
        let dirs = crate::env::ConfigDirs {
            agent_dir: PathBuf::from("/tmp/agent"),
            session_dir: PathBuf::from("/tmp/agent/sessions"),
            session_dir_explicit: false,
            package_dir: PathBuf::from("/tmp/agent/packages"),
            cwd: PathBuf::from("/tmp"),
            home: PathBuf::from("/tmp/home"),
        };
        // Pi anchors the store to `dirname(modelsPath)`, so a relocated `--agent-dir` moves both.
        assert_eq!(
            models_store_path(&dirs),
            PathBuf::from("/tmp/agent/models-store.json")
        );
        assert_eq!(
            dirs.models_path().parent(),
            models_store_path(&dirs).parent()
        );
        assert_eq!(FileModelsStore::for_dirs(&dirs).path(), models_store_path(&dirs));
    }
}

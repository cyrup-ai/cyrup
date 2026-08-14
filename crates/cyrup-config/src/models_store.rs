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
    dirs.models_path().parent().map_or_else(
        || PathBuf::from(MODELS_STORE_FILE_NAME),
        |p| p.join(MODELS_STORE_FILE_NAME),
    )
}

/// Locked, atomically-written JSON storage for dynamically refreshed provider catalogs.
///
/// The on-disk shape is Pi's exactly: one JSON object mapping provider id → `ModelsStoreEntry`,
/// pretty-printed (`JSON.stringify(current, null, 2)`).
pub struct FileModelsStore {
    path: PathBuf,
    /// Revision-checked snapshot of the whole file (Pi `ModelsFileReadState`,
    /// models-store.ts:15-19 @v0.84.1). `readLatest` short-circuits on
    /// `getFileRevision(this.path) === readState.revision` (`:86-87`) instead of re-reading and
    /// re-parsing under the cross-process lock on every catalog-overlay lookup. CFG-042.
    read_state: std::sync::RwLock<ModelsFileReadState>,
}

/// Pi `ModelsFileReadState` (models-store.ts:15-19 @v0.84.1), minus the in-flight-reload coalescer:
/// cyrup's reader is synchronous file I/O under a `FileLock`, so there is no promise to share.
#[derive(Default)]
struct ModelsFileReadState {
    data: BTreeMap<String, serde_json::Value>,
    revision: Option<String>,
}

/// Pi `getFileRevision` (utils/paths.ts:36-43 @v0.84.1) — `${dev}:${ino}:${size}:${mtimeNs}:
/// ${ctimeNs}`, `undefined` when the file cannot be stat'd. Reproduced field for field on the
/// metadata `std::fs` exposes.
fn file_revision(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(format!(
            "{}:{}:{}:{}:{}",
            meta.dev(),
            meta.ino(),
            meta.size(),
            meta.mtime_nsec(),
            meta.ctime_nsec()
        ))
    }
    #[cfg(not(unix))]
    {
        let stamp = |t: std::io::Result<std::time::SystemTime>| {
            t.ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0u128, |d| d.as_nanos())
        };
        Some(format!(
            "0:0:{}:{}:{}",
            meta.len(),
            stamp(meta.modified()),
            stamp(meta.created())
        ))
    }
}

impl FileModelsStore {
    /// `this.path = normalizePath(path)` (models-store.ts:53 @v0.84.1) — so
    /// `FileModelsStore::new("~/alt/models-store.json")` targets the home dir rather than a literal
    /// `~` directory. CFG-042.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let path = crate::paths::normalize_path_buf(&path.to_string_lossy());
        Self {
            path,
            read_state: std::sync::RwLock::new(ModelsFileReadState::default()),
        }
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

    /// `readLatest` (models-store.ts:81-108 @v0.84.1): answer from the snapshot when the file's
    /// revision is unchanged, otherwise reload under the cross-process lock and re-stamp it.
    fn read_latest(&self) -> BTreeMap<String, serde_json::Value> {
        let revision = file_revision(&self.path);
        if revision.is_some()
            && let Ok(state) = self.read_state.read()
            && revision == state.revision
        {
            return state.data.clone();
        }
        // The lock is advisory and cross-process: a concurrent `cyrup update --models` in another
        // terminal must not be observed mid-rename.
        let _guard = crate::lock::FileLock::acquire(&self.path).ok();
        let data = self.read_all();
        self.update_read_state(&data, file_revision(&self.path));
        data
    }

    /// `updateReadState` (models-store.ts:65-68 @v0.84.1). A `write`/`delete` passes `None` for the
    /// revision, exactly as upstream's two-argument call does (`:134`, `:145`), so the next read
    /// re-stats rather than trusting a revision captured before the rename.
    fn update_read_state(
        &self,
        data: &BTreeMap<String, serde_json::Value>,
        revision: Option<String>,
    ) {
        if let Ok(mut state) = self.read_state.write() {
            state.data.clone_from(data);
            state.revision = revision;
        }
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
        Ok(self
            .read_latest()
            .get(provider_id)
            .and_then(|v| serde_json::from_value::<ModelsStoreEntry>(v.clone()).ok()))
    }

    async fn write(&self, provider_id: &str, entry: ModelsStoreEntry) -> Result<(), ProviderError> {
        let _guard = crate::lock::FileLock::acquire(&self.path).ok();
        let mut all = self.read_all();
        if let Ok(value) = serde_json::to_value(&entry) {
            all.insert(provider_id.to_string(), value);
            self.write_all(&all);
            // `if (latest) this.updateReadState(this.readState, latest)` (models-store.ts:134).
            self.update_read_state(&all, None);
        }
        Ok(())
    }

    async fn delete(&self, provider_id: &str) -> Result<(), ProviderError> {
        let _guard = crate::lock::FileLock::acquire(&self.path).ok();
        let mut all = self.read_all();
        if all.remove(provider_id).is_some() {
            self.write_all(&all);
            self.update_read_state(&all, None);
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
            reopened
                .read("groq")
                .await
                .unwrap()
                .unwrap()
                .etag
                .as_deref(),
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

    /// CFG-042: `read` answers from the revision-checked snapshot (Pi `readLatest`,
    /// models-store.ts:79-86 @v0.84.1) instead of re-reading and re-parsing the file under the
    /// cross-process lock on every lookup.
    ///
    /// Red at HEAD before the fix: `read` was `FileLock::acquire` + `read_all()` every call.
    ///
    /// The short-circuit is observed by planting a sentinel in the snapshot that does NOT exist on
    /// disk and leaving the stamped revision alone — a `read` that returns the sentinel provably
    /// never opened the file. (The revision is `dev:ino:size:mtimeNs:ctimeNs`, so there is no way
    /// to diverge the bytes from the outside without also moving the revision: `chmod` bumps
    /// `ctime`, a rewrite bumps `mtime`/`size`, an atomic replace bumps `ino`.)
    #[tokio::test]
    async fn read_answers_from_the_snapshot_until_the_file_revision_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MODELS_STORE_FILE_NAME);
        let store = FileModelsStore::new(&path);
        store.write("groq", entry("\"v1\"")).await.unwrap();
        // `write` clears the revision (`updateReadState(this.readState, latest)`,
        // models-store.ts:127 — no third argument), so THIS read is the one that stamps it.
        assert_eq!(
            store.read("groq").await.unwrap().unwrap().etag.as_deref(),
            Some("\"v1\"")
        );
        let stamped = file_revision(&path);
        assert_eq!(
            store.read_state.read().unwrap().revision,
            stamped,
            "a completed read must stamp the file's revision (models-store.ts:75)"
        );

        // Plant a value the file does not contain, keeping the revision untouched.
        store.read_state.write().unwrap().data.insert(
            "groq".to_string(),
            serde_json::to_value(entry("\"from-snapshot\"")).unwrap(),
        );
        assert!(
            std::fs::read_to_string(&path).unwrap().contains("\\\"v1\\\""),
            "the file still says v1 — only the snapshot was changed"
        );
        assert_eq!(
            store.read("groq").await.unwrap().unwrap().etag.as_deref(),
            Some("\"from-snapshot\""),
            "an unchanged revision must be answered from `readState.data`, not re-read"
        );

        // A file rewritten out from under us has a new revision and IS observed, discarding the
        // planted snapshot.
        std::fs::write(
            &path,
            serde_json::to_string(&BTreeMap::from([(
                "groq".to_string(),
                serde_json::to_value(entry("\"v2\"")).unwrap(),
            )]))
            .unwrap(),
        )
        .unwrap();
        assert_ne!(file_revision(&path), stamped);
        assert_eq!(
            store.read("groq").await.unwrap().unwrap().etag.as_deref(),
            Some("\"v2\"")
        );
    }

    /// The OTHER half of `readLatest`'s guard: `if (revision !== undefined && revision ===
    /// readState.revision) return readState.data` (models-store.ts:83). A deleted file has NO
    /// revision, so the short-circuit is skipped and the reload wins — pi reports the entry gone,
    /// it does not keep serving the stale snapshot. `getFileRevision` returns `undefined` on a
    /// failed `statSync` (utils/paths.ts:36-43).
    #[tokio::test]
    async fn a_deleted_file_reloads_rather_than_serving_the_stale_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MODELS_STORE_FILE_NAME);
        let store = FileModelsStore::new(&path);
        store.write("groq", entry("\"v1\"")).await.unwrap();
        assert_eq!(
            store.read("groq").await.unwrap().unwrap().etag.as_deref(),
            Some("\"v1\"")
        );

        std::fs::remove_file(&path).unwrap();
        assert!(file_revision(&path).is_none());
        assert!(
            store.read("groq").await.unwrap().is_none(),
            "no revision means no short-circuit: pi reloads and finds nothing"
        );
    }

    /// CFG-042: `this.path = normalizePath(path)` (models-store.ts:53 @v0.84.1). Red at HEAD — the
    /// path was stored raw, so a `~` became a literal directory component.
    #[test]
    fn new_normalizes_a_tilde_path() {
        let home = crate::paths::normalize_path("~");
        let store = FileModelsStore::new("~/alt/models-store.json");
        assert_eq!(
            store.path(),
            std::path::Path::new(&home)
                .join("alt")
                .join("models-store.json")
        );
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
        assert_eq!(
            FileModelsStore::for_dirs(&dirs).path(),
            models_store_path(&dirs)
        );
    }
}

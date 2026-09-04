//! Temp-dir fixture management (Pi `createTempDir`/`getLatestTempDir` + `afterEach` cleanup,
//! session-test-utils.ts:35-55; `createTempDir` in test-harness.ts:358). cyrup uses RAII
//! ([`TestTempDir`] removes the directory on drop) instead of a global `afterEach` hook — idiomatic
//! and leak-free even on panic.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// An RAII temporary directory rooted under the system temp dir with a `cyrup-harness-` prefix
/// (Pi `pi-harness-<ts>-<rand>`, test-harness.ts:359). Removed on drop (Pi's `afterEach`/`cleanup`).
pub struct TestTempDir {
    inner: Option<TempDir>,
    path: PathBuf,
}

impl TestTempDir {
    /// Create a fresh temp dir (Pi `createTempDir`).
    pub fn new() -> std::io::Result<Self> {
        Self::with_prefix("cyrup-harness-")
    }

    /// Create a fresh temp dir with a custom prefix.
    pub fn with_prefix(prefix: &str) -> std::io::Result<Self> {
        let inner = tempfile::Builder::new().prefix(prefix).tempdir()?;
        let path = inner.path().to_path_buf();
        Ok(Self {
            inner: Some(inner),
            path,
        })
    }

    /// The directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Detach: keep the directory on disk (caller owns cleanup). Mirrors Pi's `getLatestTempDir`
    /// case where the dir must outlive the helper.
    pub fn keep(mut self) -> PathBuf {
        if let Some(dir) = self.inner.take() {
            let _ = dir.keep();
        }
        self.path.clone()
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        // `TempDir`'s own drop removes the directory; nothing else to do.
    }
}

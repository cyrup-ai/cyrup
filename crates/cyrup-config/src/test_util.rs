//! Test-only helpers (unique temp dirs whose cleanup is tied to the guard's lifetime).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

/// A temporary directory that removes itself when the value is dropped.
///
/// Wraps [`tempfile::TempDir`] and derefs to [`Path`] so call sites keep using the value
/// directly as a path (`dir.join("x")`, `&dir` where `&Path` is wanted).
///
/// IMPORTANT: the guard must stay BOUND for as long as the directory is needed.
/// `let _ = temp_dir();` drops it immediately and deletes the directory; bind it
/// (`let _dir = temp_dir();`) instead.
pub struct TempDir(tempfile::TempDir);

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.0.path()
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}

impl std::fmt::Debug for TempDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.0.path(), f)
    }
}

/// Create and return a fresh, unique temporary directory for a test.
///
/// The directory is deleted when the returned guard is dropped, so a test run leaves
/// nothing behind under `std::env::temp_dir()`.
pub fn temp_dir() -> TempDir {
    TempDir(
        tempfile::Builder::new()
            .prefix("cyrup-config-test-")
            .tempdir()
            .expect("create temp dir"),
    )
}

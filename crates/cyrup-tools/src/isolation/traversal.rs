//! Traversal-root [`FsOps`] decorator (R-03-006, arch-12 isolation).
//!
//! [`TraversalFs`] confines every filesystem operation to a configured root directory, rejecting
//! escapes: a path that lexically resolves outside the root (`../` above it, an absolute path
//! elsewhere) is denied, and — best-effort — a path that resolves outside the root via a symlink is
//! denied by comparing the *canonicalized* target against the canonicalized root. Confinement is the
//! traversal-sandboxing that `path.rs` deliberately does **not** do inline (Pi parity, DI-5); it is
//! offered here as an opt-in decorator so the default (un-confined) stance is unchanged unless a
//! caller wraps the backend.
//!
//! Cross-platform: confinement is purely path-component arithmetic; the symlink check uses
//! `std::fs::canonicalize` and is skipped gracefully when the target does not yet exist (e.g. a
//! `write` creating a new file), in which case the lexical check still applies.

use super::FsOps;
use crate::error;
use cyrup_core::{EventStream, ToolError};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Lexically normalize a path by resolving `.` and `..` **without** touching the filesystem. The
/// result keeps any root/prefix component so `starts_with` comparisons are meaningful.
pub(crate) fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// An [`FsOps`] decorator confining all operations to `root` (R-03-006).
pub struct TraversalFs {
    inner: Arc<dyn FsOps>,
    root: PathBuf,
    canonical_root: Option<PathBuf>,
}

impl TraversalFs {
    /// Wrap `inner`, confining operations to `root`. `root` is normalized once; its canonical form
    /// (if it exists) is used for the best-effort symlink-escape check.
    pub fn new(inner: Arc<dyn FsOps>, root: impl Into<PathBuf>) -> Self {
        let root = normalize_lexical(&root.into());
        let canonical_root = std::fs::canonicalize(&root).ok();
        Self { inner, root, canonical_root }
    }

    /// Confine `path` to the root, returning the normalized in-root path or a denial error.
    fn confine(&self, path: &Path) -> Result<PathBuf, ToolError> {
        let abs = if path.is_absolute() { path.to_path_buf() } else { self.root.join(path) };
        let norm = normalize_lexical(&abs);
        if !norm.starts_with(&self.root) {
            return Err(error::denied(format!(
                "path escapes traversal root: {}",
                error::show(path)
            )));
        }
        // Best-effort symlink-escape guard: if the target resolves on disk, its canonical form must
        // remain under the canonical root. A non-existent target (e.g. a fresh write) falls back to
        // the lexical check above.
        if let Some(canon_root) = &self.canonical_root
            && let Ok(canon) = std::fs::canonicalize(&norm)
                && !canon.starts_with(canon_root) {
                    return Err(error::denied(format!(
                        "path escapes traversal root via symlink: {}",
                        error::show(path)
                    )));
                }
        Ok(norm)
    }
}

#[async_trait::async_trait]
impl FsOps for TraversalFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        let p = self.confine(path)?;
        self.inner.read(&p).await
    }

    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        let p = self.confine(path)?;
        self.inner.write_in_place(&p, bytes).await
    }

    async fn access(&self, path: &Path, mode: super::Access) -> Result<(), ToolError> {
        let p = self.confine(path)?;
        self.inner.access(&p, mode).await
    }

    async fn metadata(&self, path: &Path) -> Result<super::Meta, ToolError> {
        let p = self.confine(path)?;
        self.inner.metadata(&p).await
    }

    async fn read_dir(&self, path: &Path) -> Result<Vec<super::DirEntry>, ToolError> {
        let p = self.confine(path)?;
        self.inner.read_dir(&p).await
    }

    fn detect_image_mime(&self, path: &Path) -> Option<super::ImageMime> {
        self.inner.detect_image_mime(path)
    }

    fn walk(
        &self,
        root: &Path,
        opts: super::WalkOpts,
    ) -> EventStream<Result<super::WalkItem, ToolError>> {
        match self.confine(root) {
            Ok(p) => self.inner.walk(&p, opts),
            Err(e) => Box::pin(tokio_stream::once(Err(e))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_dot_and_parent() {
        assert_eq!(normalize_lexical(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
        assert_eq!(normalize_lexical(Path::new("/a/../../etc")), PathBuf::from("/etc"));
    }

    #[test]
    fn confine_accepts_inside_rejects_outside() {
        let fs: Arc<dyn FsOps> = Arc::new(crate::ops::local::LocalFs);
        let t = TraversalFs::new(fs, "/work/project");
        assert!(t.confine(Path::new("/work/project/src/main.rs")).is_ok());
        assert!(t.confine(Path::new("src/main.rs")).is_ok()); // relative joins root
        // Escapes:
        assert!(t.confine(Path::new("/work/project/../secret")).is_err());
        assert!(t.confine(Path::new("/etc/passwd")).is_err());
        assert!(t.confine(Path::new("../../etc/passwd")).is_err());
    }
}

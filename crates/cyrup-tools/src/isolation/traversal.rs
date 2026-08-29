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
        Self {
            inner,
            root,
            canonical_root,
        }
    }

    /// Confine `path` to the root, returning the normalized in-root path or a denial error.
    fn confine(&self, path: &Path) -> Result<PathBuf, ToolError> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
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
            && !canon.starts_with(canon_root)
        {
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

    /// Forwarded EXPLICITLY — and here the omission was worse than a lost optimization.
    ///
    /// `FsOps::read_stream`'s default is `Cursor::new(self.read(path).await?)`
    /// (`ops/mod.rs:329-334`). Routing through `self.read` does re-apply [`Self::confine`], so the
    /// containment was never bypassed; what was lost is `LocalFs`'s real-`File` override, so
    /// `confineToCwd` silently reverted grep to the whole-file materialization TOOL-034 removed —
    /// undetectable by any test, because the default returns exactly the bytes a real stream would.
    ///
    /// The confinement is applied here and NOT delegated to `read`, so the guarded path is what the
    /// inner seam opens, exactly as in every other method on this decorator.
    async fn read_stream(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>, ToolError> {
        let p = self.confine(path)?;
        self.inner.read_stream(&p).await
    }

    /// The batch form of the method above, and it carries the same obligation: EVERY path is
    /// confined here, before anything is opened. A path that fails confinement takes its own slot
    /// in the result and does not stop the rest — the caller already treats a per-path `Err` as
    /// "skip this file".
    ///
    /// Overriding is not optional for correctness (the trait default routes through
    /// `self.read_stream`, so it would confine anyway) — it is what keeps the batch to one hop.
    async fn read_streams(
        &self,
        paths: &[std::path::PathBuf],
    ) -> Vec<Result<Box<dyn std::io::Read + Send>, ToolError>> {
        let mut confined = Vec::with_capacity(paths.len());
        let mut verdicts = Vec::with_capacity(paths.len());
        for path in paths {
            match self.confine(path) {
                Ok(p) => {
                    verdicts.push(Ok(confined.len()));
                    confined.push(p);
                }
                Err(e) => verdicts.push(Err(e)),
            }
        }
        let mut opened = self.inner.read_streams(&confined).await.into_iter();
        // `opened` is indexed by position among the CONFINED paths, so it is walked in step with
        // the verdicts rather than by the caller's index.
        verdicts
            .into_iter()
            .map(|v| match v {
                Ok(_) => opened
                    .next()
                    .unwrap_or_else(|| Err(crate::error::invalid("read_streams: short result"))),
                Err(e) => Err(e),
            })
            .collect()
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
        assert_eq!(
            normalize_lexical(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(
            normalize_lexical(Path::new("/a/../../etc")),
            PathBuf::from("/etc")
        );
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

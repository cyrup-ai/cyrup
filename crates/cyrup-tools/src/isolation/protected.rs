//! Protected-paths [`FsOps`] decorator (R-12-006).
//!
//! [`ProtectedFs`] wraps an `Arc<dyn FsOps>` and **blocks mutating operations** (`write_in_place`,
//! and writable `access`) targeting configured protected paths (e.g. `.env`, `.git/`,
//! `node_modules/`), while **passing reads through** unchanged. A blocked write returns an `Err`
//! (via [`crate::error::denied`]) which the runtime maps to an `isError:true` tool result
//! (R-03-038) — never a panic. Because the built-in `edit` tool mutates through `write_in_place`,
//! wrapping the backend covers both `write` and `edit` (arch-12 §6.2). This is the *operations-seam*
//! sibling of the [`crate::isolation::policy`] gate rule: the gate blocks before `Tool::execute`,
//! this decorator blocks at the backend so any tool (built-in or custom) sharing the backend is
//! covered.

use super::FsOps;
use crate::error;
use cyrup_core::{EventStream, ToolError};
use std::path::{Component, Path};
use std::sync::Arc;

/// A set of protected path *component names*. A path is protected when **any** of its components
/// equals one of these names — so `.env` matches the file `.env`, and `.git` / `node_modules` match
/// anything inside `.git/` or `node_modules/`. Component-equality (not prefix) avoids false hits on
/// e.g. `.environment`. Cross-platform (operates on `Path` components, no separator assumptions).
#[derive(Clone, Debug)]
pub struct ProtectedPaths {
    names: Vec<String>,
}

impl ProtectedPaths {
    /// The conventional defaults from func-12 §4 / R-12-006: `.env`, `.git/`, `node_modules/`.
    pub fn defaults() -> Self {
        Self {
            names: vec![".env".into(), ".git".into(), "node_modules".into()],
        }
    }

    /// A custom protected set of component names (e.g. `[".env", ".git"]`).
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Add another protected component name (builder style).
    #[must_use]
    pub fn with(mut self, name: impl Into<String>) -> Self {
        self.names.push(name.into());
        self
    }

    /// True when `path` is protected: any of its components equals a protected name.
    pub fn is_protected(&self, path: &Path) -> bool {
        path.components().any(|c| match c {
            Component::Normal(os) => os
                .to_str()
                .is_some_and(|s| self.names.iter().any(|n| n == s)),
            _ => false,
        })
    }
}

impl Default for ProtectedPaths {
    fn default() -> Self {
        Self::defaults()
    }
}

/// An [`FsOps`] decorator that denies writes/edits to [`ProtectedPaths`] while passing reads
/// through (R-12-006).
pub struct ProtectedFs {
    inner: Arc<dyn FsOps>,
    protected: ProtectedPaths,
}

impl ProtectedFs {
    /// Wrap `inner`, blocking mutations to `protected`.
    pub fn new(inner: Arc<dyn FsOps>, protected: ProtectedPaths) -> Self {
        Self { inner, protected }
    }

    /// Convenience: wrap `inner` with the conventional default protected set.
    pub fn with_defaults(inner: Arc<dyn FsOps>) -> Self {
        Self::new(inner, ProtectedPaths::defaults())
    }

    fn deny_if_protected(&self, path: &Path) -> Result<(), ToolError> {
        if self.protected.is_protected(path) {
            return Err(error::denied(format!(
                "write to protected path denied: {}",
                error::show(path)
            )));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl FsOps for ProtectedFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.inner.read(path).await
    }

    /// Forwarded EXPLICITLY, not left to the trait default.
    ///
    /// `FsOps::read_stream`'s default is `Cursor::new(self.read(path).await?)` (`ops/mod.rs:329-334`)
    /// — a whole-file materialization. That default is *semantically* indistinguishable from a real
    /// stream, so a decorator that omits this method still returns the right bytes and no test can
    /// see the difference; what it silently discards is `LocalFs`'s real-`File` override, i.e. the
    /// entire point of TOOL-034 (grep must not hold every candidate file in memory). Enabling
    /// `protectPaths` would then have re-opened the exact defect TOOL-034 closed, invisibly.
    ///
    /// This is the JS-object-spread-vs-Rust-trait-delegation hazard: pi's decorators are
    /// `{ ...ops, writeFile }` object literals, so a method added to the seam later is forwarded BY
    /// CONSTRUCTION; a Rust decorator must name every one, and the failure mode of forgetting is
    /// silent because the trait default and a dropped delegation return the same thing.
    ///
    /// No guard is applied, matching [`Self::read`]: `ProtectedFs` restricts MUTATION only
    /// (`write_in_place`, and `access` under `ReadWrite`).
    async fn read_stream(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>, ToolError> {
        self.inner.read_stream(path).await
    }

    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        self.deny_if_protected(path)?;
        self.inner.write_in_place(path, bytes).await
    }

    async fn access(&self, path: &Path, mode: super::Access) -> Result<(), ToolError> {
        if mode == super::Access::ReadWrite {
            self.deny_if_protected(path)?;
        }
        self.inner.access(path, mode).await
    }

    async fn metadata(&self, path: &Path) -> Result<super::Meta, ToolError> {
        self.inner.metadata(path).await
    }

    async fn read_dir(&self, path: &Path) -> Result<Vec<super::DirEntry>, ToolError> {
        self.inner.read_dir(path).await
    }

    fn detect_image_mime(&self, path: &Path) -> Option<super::ImageMime> {
        self.inner.detect_image_mime(path)
    }

    fn walk(
        &self,
        root: &Path,
        opts: super::WalkOpts,
    ) -> EventStream<Result<super::WalkItem, ToolError>> {
        self.inner.walk(root, opts)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_env_git_node_modules() {
        let p = ProtectedPaths::defaults();
        assert!(p.is_protected(Path::new("/work/.env")));
        assert!(p.is_protected(Path::new("/work/.git/config")));
        assert!(p.is_protected(Path::new("/work/node_modules/foo/index.js")));
        // Not protected: a normal file and a near-miss name.
        assert!(!p.is_protected(Path::new("/work/src/main.rs")));
        assert!(!p.is_protected(Path::new("/work/.environment")));
    }

    #[test]
    fn custom_set_and_builder() {
        let p = ProtectedPaths::new([".secret"]).with("vendor");
        assert!(p.is_protected(Path::new("a/.secret")));
        assert!(p.is_protected(Path::new("a/vendor/x")));
        assert!(!p.is_protected(Path::new("a/.env")));
    }
}

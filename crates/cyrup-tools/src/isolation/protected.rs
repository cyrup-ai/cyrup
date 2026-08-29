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
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// A set of protected path *component names*. A path is protected when **any** of its components
/// equals one of these names, or begins with one followed by `.` — so `.env` matches the file
/// `.env` and the dotenv family (`.env.local`, `.env.production`), and `.git` / `node_modules`
/// match anything inside `.git/` or `node_modules/`. The name-plus-dot rule (not a bare substring)
/// avoids false hits on e.g. `.environment`. Cross-platform (operates on `Path` components, no
/// separator assumptions).
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

    /// True when `path` is protected: any of its components equals a protected name, or begins
    /// with that name followed by `.` — so `.env` covers the dotenv family (`.env.local`,
    /// `.env.production`, `.env.development.local`) exactly as pi's example extension does, without
    /// inheriting its substring false positives (`.environment`, `config.env`, `.envrc`).
    ///
    /// pi's own list is inconsistent — `.env` has no trailing slash and so matches as a bare
    /// substring, while `.git/` and `node_modules/` match only as directory prefixes
    /// (`examples/extensions/protected-paths.ts` @e8682309). cyrup applies ONE rule to every
    /// configured name instead: a custom `ProtectedPaths::new([".secret"])` then also covers
    /// `.secret.local`, which is the same intent. The cost is that `.git.bak` is protected here and
    /// not under pi — over-protection, never a hole.
    ///
    /// `.gitignore` stays writable: it is not `.git` and does not begin with `.git.`. `.envrc` is
    /// out of scope on purpose — direnv, not dotenv; pi only catches it via the same bare-substring
    /// behaviour that catches `.environment`. A component that is exactly `<name>.` with nothing
    /// after the dot is NOT protected — the dot must introduce a suffix — which is why the length
    /// test is `>` and not `>=`.
    pub fn is_protected(&self, path: &Path) -> bool {
        path.components().any(|c| match c {
            Component::Normal(os) => os.to_str().is_some_and(|s| {
                self.names.iter().any(|n| {
                    s == n.as_str()
                        || (s.len() > n.len() + 1
                            && s.starts_with(n.as_str())
                            && s.as_bytes().get(n.len()) == Some(&b'.'))
                })
            }),
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
    /// When set, paths are matched RELATIVE to this root — see [`ProtectedFs::rooted`].
    root: Option<PathBuf>,
}

impl ProtectedFs {
    /// Wrap `inner`, blocking mutations to `protected`, matching against the WHOLE path.
    pub fn new(inner: Arc<dyn FsOps>, protected: ProtectedPaths) -> Self {
        Self {
            inner,
            protected,
            root: None,
        }
    }

    /// Convenience: wrap `inner` with the conventional default protected set.
    pub fn with_defaults(inner: Arc<dyn FsOps>) -> Self {
        Self::new(inner, ProtectedPaths::defaults())
    }

    /// Wrap `inner`, matching `protected` against the path RELATIVE to `root`.
    ///
    /// The tools hand this decorator an ABSOLUTE path — `write` resolves the caller's argument with
    /// `path::resolve_to_cwd` before it reaches the backend (`tools/write.rs:106`) — so an
    /// unrooted matcher tests the session cwd's own components too. A session rooted at
    /// `…/node_modules/mypkg` then has EVERY write refused, which makes the flag unusable in a
    /// legitimate cwd.
    ///
    /// pi is not immune to this: its example extension tests `event.input.path`, and pi's `write`
    /// schema is "Path to the file to write (relative or absolute)" (`write.ts:16`), so an absolute
    /// path under such a cwd is blocked there too. The difference is certainty, not existence —
    /// cyrup absolutizes unconditionally. Rooting is better than both, and is what the session
    /// builder uses.
    ///
    /// A path that is not under `root` fails `strip_prefix` and falls back to the whole path, so
    /// writes escaping the session are still checked in full (`protect_paths` can be on with
    /// `confine_to_cwd` off).
    pub fn rooted(inner: Arc<dyn FsOps>, root: PathBuf, protected: ProtectedPaths) -> Self {
        Self {
            inner,
            protected,
            root: Some(root),
        }
    }

    fn deny_if_protected(&self, path: &Path) -> Result<(), ToolError> {
        let candidate = match &self.root {
            Some(root) => path.strip_prefix(root).unwrap_or(path),
            None => path,
        };
        if self.protected.is_protected(candidate) {
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
    /// `FsOps::read_stream`'s default is `Cursor::new(self.read(path).await?)` (`ops/mod.rs:456`)
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

    /// The defaults, over the whole matrix pi's example extension defines.
    ///
    /// The dotenv rows are the reason this rule is name-plus-dot rather than component-equality:
    /// `.env.local` and `.env.production` are where secrets actually live in JS projects, pi's
    /// extension blocks them (`protectedPaths` contains a bare `".env"`, matched with
    /// `String.includes`), and cyrup used to write them. The negative rows are the other half —
    /// cyrup must not acquire pi's bare-substring false positives while closing that hole.
    #[test]
    fn defaults_match_env_git_node_modules() {
        let p = ProtectedPaths::defaults();
        assert!(p.is_protected(Path::new("/work/.env")));
        assert!(p.is_protected(Path::new("/work/.env.local")));
        assert!(p.is_protected(Path::new("/work/.env.production")));
        assert!(p.is_protected(Path::new("/work/.env.development.local")));
        assert!(p.is_protected(Path::new("/work/.git")));
        assert!(p.is_protected(Path::new("/work/.git/config")));
        assert!(p.is_protected(Path::new("/work/node_modules/foo/index.js")));
        // Not protected: a normal file, and the near-miss names pi's `String.includes` DOES catch
        // — `.environment` and `config.env` contain `.env`, `.envrc` is direnv rather than dotenv,
        // and `.gitignore` is not `.git` and does not begin with `.git.`.
        assert!(!p.is_protected(Path::new("/work/src/main.rs")));
        assert!(!p.is_protected(Path::new("/work/.environment")));
        assert!(!p.is_protected(Path::new("/work/config.env")));
        assert!(!p.is_protected(Path::new("/work/.gitignore")));
        assert!(!p.is_protected(Path::new("/work/.envrc")));
    }

    /// A session rooted UNDER a protected name must not have every write refused.
    ///
    /// `write` hands the backend an absolutized path (`tools/write.rs:106`), so an unrooted
    /// matcher tests the session cwd's own components. Before [`ProtectedFs::rooted`] a session at
    /// `…/node_modules/mypkg` refused every write in its own tree — the flag was unusable in a
    /// legitimate cwd. Rooting strips the cwd before matching, while a path OUTSIDE the root falls
    /// back to whole-path matching so escapes are still checked.
    #[tokio::test]
    async fn rooted_matcher_ignores_the_cwd_and_still_guards_inside_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj/node_modules/mypkg");
        std::fs::create_dir_all(root.join("src")).unwrap();

        let base: Arc<dyn FsOps> = Arc::new(crate::ops::local::LocalFs);
        let fs = ProtectedFs::rooted(base.clone(), root.clone(), ProtectedPaths::defaults());

        // An ordinary file inside that root: allowed, despite `node_modules` in the cwd.
        fs.write_in_place(&root.join("src/lib.rs"), b"x")
            .await
            .unwrap();
        // Its own dotenv file: still refused.
        let err = fs
            .write_in_place(&root.join(".env.local"), b"x")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("protected"), "got: {err}");

        // Outside the root, the whole path is matched — an escape is not laundered by rooting.
        let outside = dir.path().join("other/.env");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        assert!(fs.write_in_place(&outside, b"x").await.is_err());

        // Unrooted keeps today's whole-path semantics for embedders calling `with_defaults`.
        let unrooted = ProtectedFs::with_defaults(base);
        assert!(
            unrooted
                .write_in_place(&root.join("src/lib.rs"), b"x")
                .await
                .is_err()
        );
    }

    #[test]
    fn custom_set_and_builder() {
        let p = ProtectedPaths::new([".secret"]).with("vendor");
        assert!(p.is_protected(Path::new("a/.secret")));
        assert!(p.is_protected(Path::new("a/vendor/x")));
        assert!(!p.is_protected(Path::new("a/.env")));
    }
}

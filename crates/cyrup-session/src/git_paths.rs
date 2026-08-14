//! Git metadata discovery — Pi `findGitPaths` (`coding-agent/src/core/footer-data-provider.ts:16-48`
//! @v0.83.0).
//!
//! Upstream this lives beside the footer data provider and is IMPORTED by the resource loader
//! (`resource-loader.ts:19` `import { findGitPaths } from "./footer-data-provider.ts";`) so
//! `findShadowedContextFile` and the footer share one definition. cyrup's crate graph puts the
//! footer in `cyrup-tui`, which sits ABOVE `cyrup-session`, so the shared definition has to live
//! down here for the context-file loader to reach it.
//!
//! [CYRUP-DELTA] `cyrup-tui/src/footer_data.rs` still carries its own copy
//! (`find_git_paths` / `GitPaths`); collapsing it onto this one is a cross-crate edit. Two copies
//! of one predicate is the exact shape of the `encode_cwd` defect (SESS-044), so the collapse
//! should not be left indefinitely.

use std::path::{Path, PathBuf};

/// Git metadata locations for one working tree — Pi's `GitPaths`
/// (`footer-data-provider.ts:33`, `:38`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitPaths {
    /// The directory that contains `.git` (Pi's `repoDir`).
    pub repo_dir: PathBuf,
    /// The *shared* git dir (Pi's `commonGitDir`): the `.git` dir itself for a normal checkout, or
    /// the `commondir` target for a linked worktree.
    pub common_git_dir: PathBuf,
    /// The HEAD file to read (Pi's `headPath`) — per-worktree, NOT under `common_git_dir`.
    pub head_path: PathBuf,
}

/// Resolve `relative` against `base` the way Node's `path.resolve(base, rel)` does: an absolute
/// `relative` wins outright, otherwise it is joined onto `base`
/// (`footer-data-provider.ts:26`, `:31`).
fn resolve_against(base: &Path, relative: &str) -> PathBuf {
    let p = Path::new(relative);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// Pi's `findGitPaths` (`footer-data-provider.ts:16-48`): walk up from `cwd` to the filesystem root
/// looking for `.git`, returning `None` when there is none (or when the one found has no `HEAD`).
///
/// Both layouts are handled, exactly as upstream:
/// * `.git` is a **directory** — a normal clone; `HEAD` and the object store are both inside it.
/// * `.git` is a **file** holding `gitdir: <path>` — a linked worktree or a submodule; `HEAD` lives
///   in that git dir, while the shared dir comes from its `commondir` file when present.
///
/// A `.git` file that does NOT start with `gitdir: ` is ignored and the walk continues upward,
/// which is what upstream's fall-through does.
pub fn find_git_paths(cwd: &Path) -> Option<GitPaths> {
    let mut dir = cwd.to_path_buf();
    loop {
        let git_path = dir.join(".git");
        if let Ok(meta) = std::fs::metadata(&git_path) {
            if meta.is_file() {
                if let Ok(content) = std::fs::read_to_string(&git_path)
                    && let Some(rest) = content.trim().strip_prefix("gitdir: ")
                {
                    let git_dir = resolve_against(&dir, rest.trim());
                    let head_path = git_dir.join("HEAD");
                    if !head_path.exists() {
                        return None;
                    }
                    let common_dir_path = git_dir.join("commondir");
                    let common_git_dir = match std::fs::read_to_string(&common_dir_path) {
                        Ok(c) => resolve_against(&git_dir, c.trim()),
                        Err(_) => git_dir,
                    };
                    return Some(GitPaths { repo_dir: dir, common_git_dir, head_path });
                }
            } else if meta.is_dir() {
                let head_path = git_path.join("HEAD");
                if !head_path.exists() {
                    return None;
                }
                return Some(GitPaths { repo_dir: dir, common_git_dir: git_path, head_path });
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Pi `canonicalizePath` (`utils/paths.ts:26-32`): `realpathSync` with the raw path as fallback so
/// a missing entry never throws. Used ONLY where a realpath comparison is required — Pi keeps it
/// distinct from `resolvePath` and uses it in exactly one place in the resource loader, the
/// worktree comparison (`resource-loader.ts:103-104`, `:113`).
pub fn canonicalize_path(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Pi `resolvePath` (`utils/paths.ts:81-85`): absolutize against the process cwd, then apply
/// Node `path.resolve`'s **lexical** normalization. Deliberately NOT `std::fs::canonicalize` —
/// `path.resolve` does not follow symlinks, so a cwd supplied as `<link>/proj` keeps the link's
/// ancestors and the ancestor walk picks up `<link>/AGENTS.md`, which is what the user can see.
pub fn resolve_path(p: &Path) -> PathBuf {
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    lexically_normalize(&absolute)
}

/// Collapse `.` and `..` textually, the way `path.resolve` does. `..` at the root is dropped
/// (POSIX `/..` is `/`), matching Node.
fn lexically_normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop a real directory name; never pop past the prefix/root.
                if out.components().next_back().is_some_and(|c| matches!(c, Component::Normal(_))) {
                    out.pop();
                } else if out.as_os_str().is_empty() {
                    out.push(c.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

//! The footer's git-branch source — the port of Pi's `FooterDataProvider` git half
//! (`pi/packages/coding-agent/src/core/footer-data-provider.ts`).
//!
//! # The gap this closes
//!
//! [`crate::StatusLine`] has always had a `branch` field and rendered `~/path (branch)` for it
//! (`footer.ts:116-120`: `const branch = this.footerData.getGitBranch(); if (branch) pwd = …`), but
//! nothing in cyrup ever resolved a branch, so `set_branch` had only test callers and the segment
//! could never appear in a real session. This module is the missing producer.
//!
//! # Shape of the port
//!
//! Pi resolves the branch by READING `.git/HEAD` itself, not by shelling out
//! (`resolveGitBranchSync`, `footer-data-provider.ts`): `ref: refs/heads/<name>` yields `<name>`,
//! anything else is a detached HEAD, and any I/O error means "not a repo" (`null`). `git` is spawned
//! for exactly one case — the reftable backend writes the literal `ref: refs/heads/.invalid` into
//! HEAD, and only `git symbolic-ref` can say what the real branch is.
//!
//! [`find_git_paths`] is Pi's `findGitPaths`: walk up from the cwd looking for `.git`, handling BOTH
//! a plain directory and a **worktree/submodule** `.git` *file* whose `gitdir: <path>` line points
//! elsewhere (and whose `commondir` names the shared object store the reftable lives under).
//!
//! [`FooterGitBranch`] adds the caching + change detection Pi gets from `fs.watch`: cyrup polls a
//! cheap `stat` fingerprint of `HEAD` (plus `reftable/tables.list`, which is what moves in a
//! reftable repo when HEAD does not) from the run loop instead of holding an inotify watch, so a
//! `git checkout` in another terminal still repaints the footer. Pi debounces its watch by 500 ms
//! ([`FooterDataProvider.WATCH_DEBOUNCE_MS`]); [`POLL_INTERVAL`] is the same figure, and the poll
//! costs one `stat` — strictly less than the 80 ms spinner tick the loop already pays while working.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Pi's `WATCH_DEBOUNCE_MS` (`footer-data-provider.ts`), reused as cyrup's poll period: the longest
/// a branch change may go unshown. Only ever costs a `stat`, and only inside a git repo.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The `ref: ` prefix Pi slices 16 bytes off (`content.slice(16)`).
const HEAD_REF_PREFIX: &str = "ref: refs/heads/";

/// The reftable backend's placeholder HEAD: the branch name is not in the file, so only `git` knows.
const REFTABLE_PLACEHOLDER: &str = ".invalid";

/// What Pi reports for a HEAD that names a commit rather than a branch.
const DETACHED: &str = "detached";

/// Git metadata locations for one working tree — Pi's `GitPaths`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitPaths {
    /// The directory that contains `.git` (Pi's `repoDir`) — the cwd `git` is invoked from.
    pub repo_dir: PathBuf,
    /// The *shared* git dir (Pi's `commonGitDir`): the `.git` dir itself for a normal checkout, or
    /// the `commondir` target for a linked worktree. `reftable/` lives here.
    pub common_git_dir: PathBuf,
    /// The HEAD file to read (Pi's `headPath`) — per-worktree, NOT under `common_git_dir`.
    pub head_path: PathBuf,
}

/// Resolve `relative` against `base` the way Node's `path.resolve(base, rel)` does: an absolute
/// `relative` wins outright, otherwise it is joined onto `base`.
fn resolve_against(base: &Path, relative: &str) -> PathBuf {
    let p = Path::new(relative);
    if p.is_absolute() { p.to_path_buf() } else { base.join(p) }
}

/// Pi's `findGitPaths` (`footer-data-provider.ts`): walk up from `cwd` to the filesystem root
/// looking for `.git`, returning `None` when there is none (or when the one found has no `HEAD`).
///
/// Both layouts are handled, exactly as upstream:
/// * `.git` is a **directory** — a normal clone; `HEAD` and the object store are both inside it.
/// * `.git` is a **file** holding `gitdir: <path>` — a linked worktree or a submodule; `HEAD` lives
///   in that git dir, while the shared dir comes from its `commondir` file when present.
///
/// A `.git` file that does NOT start with `gitdir: ` is ignored and the walk continues upward, which
/// is what upstream's fall-through does.
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
                return Some(GitPaths {
                    repo_dir: dir,
                    common_git_dir: git_path,
                    head_path,
                });
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Pi's `resolveGitBranchSync`: the branch named by `HEAD`, `"detached"` when HEAD holds a raw
/// commit, and `None` when HEAD cannot be read at all.
///
/// `None` is also returned for a HEAD that names an EMPTY ref — upstream produces `""` there, which
/// its `if (branch)` guard drops, and the footer must not render a bare `()`.
pub fn resolve_branch(paths: &GitPaths) -> Option<String> {
    let content = std::fs::read_to_string(&paths.head_path).ok()?;
    let content = content.trim();
    let Some(name) = content.strip_prefix(HEAD_REF_PREFIX) else {
        return Some(DETACHED.to_string());
    };
    if name == REFTABLE_PLACEHOLDER {
        // Pi: `resolveBranchWithGitSync(repoDir) ?? "detached"`.
        return Some(resolve_branch_with_git(&paths.repo_dir).unwrap_or_else(|| DETACHED.to_string()));
    }
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

/// Pi's `resolveBranchWithGitSync`: `git --no-optional-locks symbolic-ref --quiet --short HEAD`,
/// stdin and stderr closed. `None` on a detached HEAD, a non-zero exit, or no `git` on PATH.
///
/// Reached ONLY for a reftable repo (HEAD is the `.invalid` placeholder), so a normal session never
/// spawns a process for the footer.
fn resolve_branch_with_git(repo_dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["--no-optional-locks", "symbolic-ref", "--quiet", "--short", "HEAD"])
        .current_dir(repo_dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() { None } else { Some(branch) }
}

/// A cheap "did the refs move?" fingerprint: `(len, mtime)` of the watched files. Pi gets this
/// signal from `fs.watch`/`watchFile` on the same two paths.
type Fingerprint = Vec<(u64, Option<SystemTime>)>;

/// The footer's cached git branch plus the change detection that keeps it live.
///
/// Pi's `FooterDataProvider` in the shape cyrup needs: constructed once from the session cwd
/// ([`FooterGitBranch::discover`]), read by the footer ([`FooterGitBranch::branch`]), and re-checked
/// from the run loop ([`FooterGitBranch::poll`]).
#[derive(Debug, Default)]
pub struct FooterGitBranch {
    paths: Option<GitPaths>,
    branch: Option<String>,
    fingerprint: Fingerprint,
}

impl FooterGitBranch {
    /// Not in a repo and nothing to watch — the state a backend-only [`crate::AppState`] boots with
    /// until the binary points it at the session cwd.
    pub fn none() -> Self {
        Self::default()
    }

    /// Pi's constructor (`this.gitPaths = findGitPaths(cwd)`) plus its lazy first
    /// `getGitBranch()`: locate the repo above `cwd` and resolve the branch once.
    pub fn discover(cwd: &Path) -> Self {
        let paths = find_git_paths(cwd);
        let branch = paths.as_ref().and_then(resolve_branch);
        let fingerprint = paths.as_ref().map(fingerprint_of).unwrap_or_default();
        Self { paths, branch, fingerprint }
    }

    /// The cached branch — Pi's `getGitBranch()`. `None` outside a repo (and for an empty ref).
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// Whether there is anything to poll: `false` outside a git repo, which is what gates the run
    /// loop's tick so a non-repo cwd costs nothing at all.
    pub fn in_repo(&self) -> bool {
        self.paths.is_some()
    }

    /// Re-check the refs and return `true` when the branch string CHANGED (Pi's
    /// `refreshGitBranchAsync` → `notifyBranchChange`). A `stat` that shows nothing moved returns
    /// `false` without re-reading HEAD.
    pub fn poll(&mut self) -> bool {
        let Some(paths) = self.paths.as_ref() else { return false };
        let next = fingerprint_of(paths);
        if next == self.fingerprint {
            return false;
        }
        self.fingerprint = next;
        let branch = resolve_branch(paths);
        if branch == self.branch {
            return false;
        }
        self.branch = branch;
        true
    }
}

/// `(len, mtime)` for `HEAD` and — when the repo uses the reftable backend — `reftable/tables.list`,
/// which is the file that moves on a branch switch there while HEAD stays put (Pi watches both).
fn fingerprint_of(paths: &GitPaths) -> Fingerprint {
    let mut out = Vec::with_capacity(2);
    for p in [
        paths.head_path.clone(),
        paths.common_git_dir.join("reftable").join("tables.list"),
    ] {
        if let Ok(m) = std::fs::metadata(&p) {
            out.push((m.len(), m.modified().ok()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cyrup-footer-git-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a plain `.git` directory holding `head` as its HEAD content.
    fn plain_repo(root: &Path, head: &str) {
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git").join("HEAD"), head).unwrap();
    }

    #[test]
    fn plain_repo_head_names_the_branch() {
        let root = tmp("plain");
        plain_repo(&root, "ref: refs/heads/david/cyrup\n");
        let paths = find_git_paths(&root).unwrap();
        assert_eq!(paths.repo_dir, root);
        assert_eq!(paths.common_git_dir, root.join(".git"));
        assert_eq!(resolve_branch(&paths).as_deref(), Some("david/cyrup"));
    }

    #[test]
    fn resolution_walks_up_from_a_nested_cwd() {
        let root = tmp("nested");
        plain_repo(&root, "ref: refs/heads/main\n");
        let deep = root.join("crates").join("cyrup-tui").join("src");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(FooterGitBranch::discover(&deep).branch(), Some("main"));
    }

    #[test]
    fn a_raw_commit_head_is_detached() {
        let root = tmp("detached");
        plain_repo(&root, "9f8c1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b\n");
        assert_eq!(FooterGitBranch::discover(&root).branch(), Some(DETACHED));
    }

    #[test]
    fn no_repo_yields_no_branch_and_nothing_to_poll() {
        let root = tmp("norepo");
        let mut b = FooterGitBranch::discover(&root);
        assert_eq!(b.branch(), None);
        assert!(!b.in_repo());
        assert!(!b.poll());
    }

    #[test]
    fn an_empty_ref_renders_no_segment() {
        let root = tmp("emptyref");
        plain_repo(&root, "ref: refs/heads/\n");
        assert_eq!(FooterGitBranch::discover(&root).branch(), None);
    }

    #[test]
    fn a_worktree_git_file_follows_gitdir_and_commondir() {
        let root = tmp("worktree");
        // The shared clone…
        let main = root.join("main");
        plain_repo(&main, "ref: refs/heads/main\n");
        // …and a linked worktree whose `.git` is a FILE.
        let wt_git = main.join(".git").join("worktrees").join("feature");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt_git.join("HEAD"), "ref: refs/heads/feature\n").unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let wt = root.join("feature");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();

        let paths = find_git_paths(&wt).unwrap();
        assert_eq!(paths.head_path, wt_git.join("HEAD"));
        // `commondir` = `../..` from `<main>/.git/worktrees/feature` ⇒ `<main>/.git`.
        assert_eq!(paths.common_git_dir, wt_git.join("..").join(".."));
        assert_eq!(resolve_branch(&paths).as_deref(), Some("feature"));
    }

    #[test]
    fn a_git_file_without_a_gitdir_line_is_ignored_and_the_walk_continues() {
        let root = tmp("bogus");
        plain_repo(&root, "ref: refs/heads/outer\n");
        let inner = root.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join(".git"), "not a gitdir line\n").unwrap();
        assert_eq!(FooterGitBranch::discover(&inner).branch(), Some("outer"));
    }

    #[test]
    fn poll_reports_only_real_branch_changes() {
        let root = tmp("poll");
        plain_repo(&root, "ref: refs/heads/main\n");
        let mut b = FooterGitBranch::discover(&root);
        assert_eq!(b.branch(), Some("main"));
        assert!(b.in_repo());
        // Nothing moved ⇒ no change.
        assert!(!b.poll());
        // A checkout rewrites HEAD. The fingerprint carries the file LENGTH as well as the mtime,
        // so this is detected even when the clock has not ticked.
        std::fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert!(b.poll());
        assert_eq!(b.branch(), Some("feature/x"));
        assert!(!b.poll());
    }
}

//! Context-file discovery (`AGENTS.override.md`/`AGENTS.md`/`CLAUDE.md`) — arch-06 §6.2,
//! R-06-006/007/008/009.
//!
//! Discovery order: the global agent dir, then each ancestor of `cwd` walking up to root, then
//! `cwd` itself. At each level only the **first** found candidate is used (prefer
//! `AGENTS.override.md` over `AGENTS.md` over `CLAUDE.md`); the discovered files are concatenated
//! global → parents(top→down) → cwd.
//!
//! The loader is **owned + blocking** (`std::fs`); callers MUST invoke [`ContextFileLoader::load`]
//! via `tokio::task::spawn_blocking` (arch-00 §5) so the bounded ancestor walk never stalls the
//! reactor. [`crate::prompt::ContextStore::reload`] does exactly this.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Provenance of a discovered context file. `Global` loads **pre-trust** (R-06-009).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextScope {
    Global,
    Ancestor,
    Cwd,
}

/// One discovered context file, already read.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: Arc<str>,
    pub scope: ContextScope,
}

/// Non-fatal per-file issues collected during discovery — never thrown (arch-06 §8).
#[derive(Clone, Debug)]
pub enum ContextDiagnostic {
    Unreadable { path: PathBuf, reason: String },
    SkippedUntrusted { dir: PathBuf },
}

/// Trust seam (R-06-009). The caller resolves this to a `bool` (`project_trusted`) BEFORE
/// constructing the owned [`ContextFileLoader`], so the loader is `'static` and movable into
/// `spawn_blocking`. Kept as a trait for testability (stub in tests).
pub trait TrustQuery: Send + Sync {
    fn is_project_trusted(&self) -> bool;
}

/// Owned, blocking discoverer. Construct from already-resolved values, then move into
/// `spawn_blocking` and call [`ContextFileLoader::load`].
#[derive(Clone, Debug)]
pub struct ContextFileLoader {
    /// Current working directory; the ancestor walk starts here.
    pub cwd: PathBuf,
    /// Global agent dir (`~/.cyrup/agent`); its `AGENTS.md`/`CLAUDE.md` loads pre-trust.
    pub global_agent_dir: PathBuf,
    /// Resolved from a [`TrustQuery`] by the caller (R-06-009).
    pub project_trusted: bool,
    /// `--no-context-files` / `-nc` short-circuit (R-06-008).
    pub disabled: bool,
}

/// First-found candidate order per directory (R-06-007).
///
/// Exactly Pi's `loadContextFileFromDir` candidate array, in order
/// (`v0.84.1 coding-agent/src/core/resource-loader.ts:71`):
/// `["AGENTS.override.md", "AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"]`.
///
/// `AGENTS.override.md` was added upstream in `8ecf8a988`
/// ("feat(coding-agent): support AGENTS.override.md (#7681)", 2026-08-05), i.e. AFTER the ported
/// v0.83.0 baseline — where the array was the 4-entry
/// `["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"]`
/// (`v0.83.0 coding-agent/src/core/resource-loader.ts:71`). This is therefore VERSION LAG, not a
/// port bug: the relative order of the four pre-existing entries is unchanged upstream and here.
///
/// Because the loop returns on the FIRST hit, listing the override first is what makes it *win*
/// over a sibling `AGENTS.md` in the same directory — it is a per-directory override, not an
/// additional file (Pi never loads two candidates from one dir).
const CANDIDATES: [&str; 5] =
    ["AGENTS.override.md", "AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

impl ContextFileLoader {
    pub fn new(
        cwd: PathBuf,
        global_agent_dir: PathBuf,
        project_trusted: bool,
        disabled: bool,
    ) -> Self {
        Self { cwd, global_agent_dir, project_trusted, disabled }
    }

    /// Construct resolving project trust from a [`TrustQuery`] seam.
    pub fn from_trust(
        cwd: PathBuf,
        global_agent_dir: PathBuf,
        trust: &dyn TrustQuery,
        disabled: bool,
    ) -> Self {
        Self::new(cwd, global_agent_dir, trust.is_project_trusted(), disabled)
    }

    /// Discover + read context files (R-06-006/007/008/009).
    ///
    /// BLOCKING (`std::fs`): invoke via `tokio::task::spawn_blocking`. Unreadable files are skipped
    /// and recorded as diagnostics; nothing here panics.
    pub fn load(self) -> (Vec<ContextFile>, Vec<ContextDiagnostic>) {
        let mut files = Vec::new();
        let mut diags = Vec::new();
        if self.disabled {
            return (files, diags); // R-06-008
        }
        let mut seen: HashSet<PathBuf> = HashSet::new();

        // (a) GLOBAL — pre-trust (R-06-009).
        if let Some(cf) =
            first_context_file(&self.global_agent_dir, ContextScope::Global, &mut diags)
            && seen.insert(cf.path.clone()) {
                files.push(cf);
            }

        // (b) PROJECT (ancestors + cwd) — trust-gated (R-06-009).
        if self.project_trusted {
            // Pi `const resolvedCwd = resolvePath(options.cwd);` (`resource-loader.ts:122`) then
            // `let currentDir = resolvedCwd;` (`:137`) — LEXICAL resolution, not `realpath`. pi
            // keeps `canonicalizePath` separate and uses it in exactly one place in this file, the
            // worktree comparison below (`:102-103`, `:113`), precisely because that comparison
            // needs realpaths and the walk does not. Canonicalizing here made cyrup walk the
            // TARGET's ancestors where pi walks the LINK's, so an `--cwd <symlink>/proj` picked up
            // a different AGENTS.md than the user can see — and put the realpath into the
            // `<project_instructions path="…">` attribute the model is shown.
            let start = crate::git_paths::resolve_path(&self.cwd);
            let cwd_resolved = start.clone();
            // Pi `findShadowedContextFile(resolvedCwd)` (`resource-loader.ts:136`), hoisted out of
            // the loop exactly as upstream.
            let shadowed = find_shadowed_context_file(&start);
            // Walk cwd → root; prepend so final order is top→down (R-06-007).
            let mut ancestors: Vec<ContextFile> = Vec::new();
            let mut dir = start;
            loop {
                let scope = if dir == cwd_resolved {
                    ContextScope::Cwd
                } else {
                    ContextScope::Ancestor
                };
                if let Some(cf) = first_context_file(&dir, scope, &mut diags)
                    // Pi `const isShadowed = shadowedContextFile !== undefined &&
                    // canonicalizePath(contextFile?.path ?? "") === shadowedContextFile;`
                    // (`resource-loader.ts:140-142`), tested BEFORE the `seenPaths` insert.
                    && !shadowed.as_ref().is_some_and(|s| {
                        crate::git_paths::canonicalize_path(&cf.path) == *s
                    })
                    && seen.insert(cf.path.clone()) {
                        ancestors.insert(0, cf);
                    }
                match dir.parent() {
                    Some(p) if p != dir => dir = p.to_path_buf(),
                    _ => break,
                }
            }
            files.extend(ancestors); // global first, then parents top→down, then cwd
        } else {
            diags.push(ContextDiagnostic::SkippedUntrusted { dir: self.cwd.clone() });
        }

        (files, diags)
    }
}

/// The MAIN repo's context file that a nested linked worktree's own copy shadows — Pi
/// `findShadowedContextFile` (`resource-loader.ts:100-116`), ported guard-for-guard.
///
/// Both are the same tracked `AGENTS.md`/`CLAUDE.md`, so loading both injects the identical content
/// twice: wasted context, and duplicated instructions the model may weight more heavily. `None`
/// means nothing is shadowed, which leaves ordinary ancestor inheritance alone.
///
/// Returned CANONICALIZED, because `git worktree add` writes the `.git` file's `gitdir:` target in
/// realpath form while the cwd may still be symlinked (macOS `/tmp` → `/private/tmp`) — Pi's own
/// comment at `:96-98`.
fn find_shadowed_context_file(cwd: &Path) -> Option<PathBuf> {
    use crate::git_paths::{canonicalize_path, find_git_paths};

    let git = find_git_paths(cwd)?;
    let common_git_dir = canonicalize_path(&git.common_git_dir);
    let worktree_root = canonicalize_path(&git.repo_dir);
    let main_repo_root = common_git_dir.parent()?.to_path_buf();
    // Pi `if (!worktreeRoot.startsWith(`${mainRepoRoot}${sep}`)) return undefined;` (`:108`) —
    // false for an ordinary repo, where the two are the same dir, and for a SIBLING worktree
    // (`git worktree add ../feat`), whose main repo is not an ancestor. `Path::starts_with` is
    // component-wise, so the equality case has to be excluded explicitly — that is what pi's
    // trailing separator does.
    if worktree_root == main_repo_root || !worktree_root.starts_with(&main_repo_root) {
        return None;
    }
    // Pi `if (canonicalizePath(join(mainRepoRoot, ".git")) !== commonGitDir) return undefined;`
    // (`:113`). `dirname(commonGitDir)` is the main worktree root only when that dir is itself
    // checked out from the same repo: in a bare layout (`proj/.bare` + `proj/main`) it is just the
    // directory holding `.bare`, which tracks nothing, and a submodule's gitdir has no `commondir`
    // so it lands under `.git/modules`.
    if canonicalize_path(&main_repo_root.join(".git")) != common_git_dir {
        return None;
    }
    let worktree_context = first_context_file(&worktree_root, ContextScope::Cwd, &mut Vec::new())?;
    Some(main_repo_root.join(worktree_context.path.file_name()?))
}

/// First-found context file in `dir` among [`CANDIDATES`] (R-06-007).
fn first_context_file(
    dir: &Path,
    scope: ContextScope,
    diags: &mut Vec<ContextDiagnostic>,
) -> Option<ContextFile> {
    for name in CANDIDATES {
        let p = dir.join(name);
        match std::fs::read_to_string(&p) {
            Ok(content) => {
                return Some(ContextFile { path: p, content: content.into(), scope });
            }
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                diags.push(ContextDiagnostic::Unreadable { path: p, reason: e.to_string() });
                continue;
            }
        }
    }
    None
}

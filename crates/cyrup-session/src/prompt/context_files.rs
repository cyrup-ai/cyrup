//! Context-file discovery (`AGENTS.md`/`CLAUDE.md`) — arch-06 §6.2, R-06-006/007/008/009.
//!
//! Discovery order: the global agent dir, then each ancestor of `cwd` walking up to root, then
//! `cwd` itself. At each level only the **first** found candidate is used (prefer `AGENTS.md` over
//! `CLAUDE.md`); the discovered files are concatenated global → parents(top→down) → cwd.
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
const CANDIDATES: [&str; 4] = ["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

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
        {
            if seen.insert(cf.path.clone()) {
                files.push(cf);
            }
        }

        // (b) PROJECT (ancestors + cwd) — trust-gated (R-06-009).
        if self.project_trusted {
            let start = std::fs::canonicalize(&self.cwd).unwrap_or_else(|_| self.cwd.clone());
            let cwd_canon = start.clone();
            // Walk cwd → root; prepend so final order is top→down (R-06-007).
            let mut ancestors: Vec<ContextFile> = Vec::new();
            let mut dir = start;
            loop {
                let scope = if dir == cwd_canon {
                    ContextScope::Cwd
                } else {
                    ContextScope::Ancestor
                };
                if let Some(cf) = first_context_file(&dir, scope, &mut diags) {
                    if seen.insert(cf.path.clone()) {
                        ancestors.insert(0, cf);
                    }
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

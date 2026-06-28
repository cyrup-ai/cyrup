//! Path handling (R-03-005/006, arch-03 §6.2).
//!
//! `resolve_to_cwd` accepts relative or absolute paths (relatives resolve against the cwd) after
//! trimming, stripping a leading `@`, and expanding a leading `~`. `resolve_read_path` adds Pi's
//! macOS filename variants as fallbacks. Traversal sandboxing is deliberately NOT enforced here
//! (Pi parity, DI-5); it is provided as an `FsOps` policy decorator by arch-12. [CYRUP-DELTA]:
//! NFD/narrow-no-break-space variants are deferred (no unicode-normalization dep ratified);
//! the curly-quote variant is implemented.

use std::path::{Path, PathBuf};

/// Expand a leading `~` to the user's home directory.
fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    } else if path == "~"
        && let Some(home) = home_dir() {
            return home;
        }
    PathBuf::from(path)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Resolve a user-supplied path against `cwd` (R-03-005). Relatives join `cwd`; absolutes pass
/// through. Trims surrounding whitespace and a leading `@` (drag-and-drop / mention sugar).
pub fn resolve_to_cwd(path: &str, cwd: &Path) -> PathBuf {
    let trimmed = path.trim();
    let trimmed = trimmed.strip_prefix('@').unwrap_or(trimmed);
    let expanded = expand_home(trimmed);
    if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    }
}

/// Candidate read paths in priority order (R-03-006): the resolved path first, then macOS curly
/// quote (`'` ⇄ U+2019) variants. The `read` tool tries each via the `FsOps` seam and uses the
/// first that exists.
pub fn resolve_read_path(path: &str, cwd: &Path) -> Vec<PathBuf> {
    let primary = resolve_to_cwd(path, cwd);
    let mut out = vec![primary.clone()];

    let s = primary.to_string_lossy();
    if s.contains('\'') {
        out.push(PathBuf::from(s.replace('\'', "\u{2019}")));
    }
    if s.contains('\u{2019}') {
        out.push(PathBuf::from(s.replace('\u{2019}', "'")));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn relative_joins_cwd() {
        let cwd = Path::new("/work");
        assert_eq!(resolve_to_cwd("a/b.txt", cwd), PathBuf::from("/work/a/b.txt"));
    }

    #[test]
    fn absolute_passthrough() {
        let cwd = Path::new("/work");
        assert_eq!(resolve_to_cwd("/etc/hosts", cwd), PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn strips_at_and_trims() {
        let cwd = Path::new("/work");
        assert_eq!(resolve_to_cwd("  @rel.txt ", cwd), PathBuf::from("/work/rel.txt"));
    }

    #[test]
    fn read_path_adds_curly_variant() {
        let cwd = Path::new("/work");
        let v = resolve_read_path("it's.txt", cwd);
        assert_eq!(v[0], PathBuf::from("/work/it's.txt"));
        assert!(v.iter().any(|p| p.to_string_lossy().contains('\u{2019}')));
    }
}

//! `normalizeCwd` / `sameCwd` — a 1:1 port of `pi-intercom/cwd.ts` (31 lines, `v0.10.1`).
//!
//! Upstream's own rationale, verbatim from `cwd.ts:4-10`:
//!
//! ```text
//! // Normalize a cwd for same-directory comparison. A raw string match ("a === b")
//! // hides genuine same-directory peers when two cwd strings differ only by a
//! // trailing slash, a "."/".." segment, or a symlink (e.g. macOS /tmp <->
//! // /private/tmp). resolve() collapses the lexical variants; realpathSync()
//! // collapses symlinks (best-effort: falls back to the resolved path if the
//! // directory no longer exists). Memoized — the set of distinct cwd strings is
//! // small and stable, so repeat comparisons are free after warmup.
//! ```

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// `const normalizeCache = new Map<string, string>()` (`v0.10.1 cwd.ts:11`).
///
/// Process-global and unbounded upstream too; the population is the set of distinct peer cwd
/// strings the broker has ever reported, which is small and stable.
fn normalize_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `normalizeCwd(cwd)` (`v0.10.1 cwd.ts:13-27`): `resolve(cwd)` then a best-effort
/// `realpathSync`, memoized on the RAW input string.
///
/// Mechanism notes:
/// - `resolve()` is lexical AND absolutizes against the process cwd; [`resolve_lexical`] reproduces
///   both halves, including that `..` is collapsed lexically rather than by touching the
///   filesystem, which is what makes it work for a directory that no longer exists.
/// - `realpathSync` throwing (a deleted or unreadable directory) falls back to the resolved path
///   rather than propagating, so a stale peer cwd never breaks a `list`.
#[must_use]
pub fn normalize_cwd(cwd: &str) -> String {
    if let Some(hit) = normalize_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(cwd)
    {
        return hit.clone();
    }
    let resolved = resolve_lexical(Path::new(cwd));
    let normalized = std::fs::canonicalize(&resolved).map_or_else(
        |_| resolved.to_string_lossy().to_string(),
        |p| p.to_string_lossy().to_string(),
    );
    normalize_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cwd.to_string(), normalized.clone());
    normalized
}

/// `sameCwd(a, b)` (`v0.10.1 cwd.ts:29-31`): `normalizeCwd(a) === normalizeCwd(b)`.
#[must_use]
pub fn same_cwd(a: &str, b: &str) -> bool {
    normalize_cwd(a) == normalize_cwd(b)
}

/// `path.resolve(cwd)` (node) / `path.resolve(base, segment)` — absolutize against `base` (or the
/// process cwd) and collapse `.`/`..` and duplicate separators LEXICALLY, without touching the
/// filesystem.
///
/// `std::fs::canonicalize` alone is not a substitute: it fails outright on a path that does not
/// exist, whereas node's `resolve` always answers and only the `realpathSync` step is fallible.
#[must_use]
pub fn resolve_path(base: &Path, segment: &str) -> PathBuf {
    let candidate = Path::new(segment);
    if candidate.is_absolute() {
        resolve_lexical(candidate)
    } else {
        resolve_lexical(&base.join(candidate))
    }
}

fn resolve_lexical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            // `..` at the root is the root itself, exactly as node's `resolve` treats it.
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// `v0.10.1 cwd.ts:29-31`. The three shapes upstream's comment names — a trailing slash, a
    /// `.`/`..` segment, and a symlink — must all compare EQUAL. A raw byte compare (which is what
    /// `format_session_list_row`'s "same cwd" tag used to do) reports them as different projects.
    #[test]
    fn same_cwd_collapses_trailing_slashes_and_dot_segments() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_string_lossy().to_string();
        assert!(same_cwd(&base, &format!("{base}/")));
        assert!(same_cwd(&base, &format!("{base}/.")));
        assert!(same_cwd(&base, &format!("{base}/child/..")));
        assert!(!same_cwd(&base, &format!("{base}/child")));
    }

    /// The `realpathSync` half: macOS `/tmp` is a symlink to `/private/tmp`, so a session started
    /// through the link and one started at the target are the SAME project.
    #[cfg(unix)]
    #[test]
    fn same_cwd_collapses_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(same_cwd(&target.to_string_lossy(), &link.to_string_lossy()));
    }

    /// `realpathSync` throwing falls back to the RESOLVED path (`v0.10.1 cwd.ts:22-24`) rather than
    /// propagating — a peer whose cwd was deleted must still be comparable, not an error.
    #[test]
    fn a_missing_directory_falls_back_to_the_resolved_path() {
        assert_eq!(
            normalize_cwd("/definitely/not/here/../here/"),
            "/definitely/not/here"
        );
        assert!(same_cwd("/definitely/not/here", "/definitely/not/here/"));
    }

    /// `resolvePath(currentSession.cwd, cwd)` (`v0.10.1 index.ts:1905`): a relative `cwd` argument
    /// resolves against the CURRENT session's cwd, not the process cwd.
    #[test]
    fn resolve_path_absolutizes_relative_segments_against_the_base() {
        assert_eq!(
            resolve_path(Path::new("/w/project"), "sub"),
            PathBuf::from("/w/project/sub")
        );
        assert_eq!(
            resolve_path(Path::new("/w/project"), "../other"),
            PathBuf::from("/w/other")
        );
        assert_eq!(
            resolve_path(Path::new("/w/project"), "/abs"),
            PathBuf::from("/abs")
        );
    }
}

//! Path handling (R-03-005/006, arch-03 §6.2).
//!
//! `resolve_to_cwd` accepts relative or absolute paths (relatives resolve against the cwd) after
//! trimming, normalizing Unicode spaces, stripping a leading `@`, and expanding a leading `~`.
//! `resolve_read_path` adds Pi's macOS filename variants as fallbacks. Traversal sandboxing is
//! deliberately NOT enforced here (Pi parity, DI-5); it is provided as an `FsOps` policy decorator
//! by arch-12. The full Pi fallback set — AM/PM-NBSP, NFD, curly-quote, and NFD+curly
//! (path-utils.ts:53-83) — is implemented; NFD uses the workspace-declared `unicode-normalization`
//! crate.

use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

/// Pi's `UNICODE_SPACES` set (paths.ts:6): NBSP, U+2000-U+200A, narrow NBSP, medium-math, ideographic.
fn normalize_unicode_spaces(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{00A0}'
            | '\u{2000}'..='\u{200A}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

const NARROW_NO_BREAK_SPACE: char = '\u{202F}';

/// macOS screenshot variant: a regular space before `AM`/`PM` (case-insensitive) followed by a `.`
/// becomes a narrow no-break space (path-utils.ts:7-9, regex `/ (AM|PM)\./gi`). `None` if no change.
fn try_am_pm(path: &str) -> Option<String> {
    let chars: Vec<char> = path.chars().collect();
    let mut out = String::with_capacity(path.len());
    let mut changed = false;
    let mut i = 0usize;
    while i < chars.len() {
        let c0 = chars.get(i).copied();
        let c1 = chars.get(i + 1).copied();
        let c2 = chars.get(i + 2).copied();
        let c3 = chars.get(i + 3).copied();
        if c0 == Some(' ')
            && matches!(c1, Some('a' | 'A' | 'p' | 'P'))
            && matches!(c2, Some('m' | 'M'))
            && c3 == Some('.')
        {
            out.push(NARROW_NO_BREAK_SPACE);
            if let Some(a) = c1 {
                out.push(a);
            }
            if let Some(b) = c2 {
                out.push(b);
            }
            out.push('.');
            i += 4;
            changed = true;
        } else if let Some(c) = c0 {
            out.push(c);
            i += 1;
        } else {
            break;
        }
    }
    if changed {
        Some(out)
    } else {
        None
    }
}

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
    // Pi's `resolvePath(..., { normalizeUnicodeSpaces: true, stripAtPrefix: true })`
    // (path-utils.ts:48-50) normalizes Unicode spaces before resolving.
    let normalized = normalize_unicode_spaces(path);
    let trimmed = normalized.trim();
    let trimmed = trimmed.strip_prefix('@').unwrap_or(trimmed);
    let expanded = expand_home(trimmed);
    if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    }
}

/// Candidate read paths in priority order (R-03-006, path-utils.ts:53-83): the resolved path first,
/// then the macOS AM/PM narrow-NBSP screenshot variant, then the NFD (decomposed) variant, then the
/// curly-quote (`'` → U+2019) variant, then the combined NFD+curly variant. Each non-primary variant
/// is emitted only when it differs from the resolved path, matching Pi's `!== resolved` guard. The
/// `read` tool tries each via the `FsOps` seam and uses the first that exists. A reverse curly→`'`
/// variant is appended as an additive cyrup-only fallback (never resolves a path Pi's set already
/// covers).
pub fn resolve_read_path(path: &str, cwd: &Path) -> Vec<PathBuf> {
    let primary = resolve_to_cwd(path, cwd);
    let s = primary.to_string_lossy().into_owned();
    let mut out = vec![primary];

    // macOS screenshot AM/PM variant: narrow no-break space before AM/PM (path-utils.ts:7-9,57-61).
    if let Some(v) = try_am_pm(&s) {
        out.push(PathBuf::from(v));
    }
    // NFD variant: macOS stores filenames in decomposed form (path-utils.ts:11-14,63-67).
    let nfd: String = s.nfd().collect();
    if nfd != s {
        out.push(PathBuf::from(&nfd));
    }
    // Curly-quote variant: straight `'` → U+2019 (path-utils.ts:16-20,69-73).
    if s.contains('\'') {
        out.push(PathBuf::from(s.replace('\'', "\u{2019}")));
    }
    // Combined NFD + curly-quote variant for French macOS screenshots (path-utils.ts:75-79).
    let nfd_curly = nfd.replace('\'', "\u{2019}");
    if nfd_curly != s {
        out.push(PathBuf::from(nfd_curly));
    }
    // [CYRUP extra] reverse curly U+2019 → straight `'` (additive fallback beyond Pi's set).
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

    #[test]
    fn normalizes_unicode_spaces() {
        let cwd = Path::new("/work");
        // NBSP between words is folded to a regular space before resolving.
        assert_eq!(
            resolve_to_cwd("a\u{00A0}b.txt", cwd),
            PathBuf::from("/work/a b.txt")
        );
    }

    #[test]
    fn read_path_adds_nfd_variant() {
        let cwd = Path::new("/work");
        // Composed "é" (U+00E9). macOS stores it decomposed as "e" + U+0301 (combining acute).
        let v = resolve_read_path("caf\u{00E9}.txt", cwd);
        assert_eq!(v[0], PathBuf::from("/work/caf\u{00E9}.txt"));
        assert!(
            v.iter().any(|p| p.to_string_lossy().contains("cafe\u{0301}.txt")),
            "variants: {v:?}"
        );
    }

    #[test]
    fn read_path_adds_nfd_curly_variant() {
        let cwd = Path::new("/work");
        // French screenshot "Capture d'écran": straight apostrophe + composed "é". The combined
        // NFD+curly fallback decomposes "é" AND swaps the apostrophe to U+2019 (path-utils.ts:75-79).
        let v = resolve_read_path("d'\u{00E9}cran.txt", cwd);
        assert!(
            v.iter().any(|p| {
                let s = p.to_string_lossy();
                s.contains('\u{2019}') && s.contains("e\u{0301}")
            }),
            "variants: {v:?}"
        );
    }

    #[test]
    fn read_path_adds_am_pm_variant() {
        let cwd = Path::new("/work");
        // A screenshot like "Screenshot 3.04.10 PM.png" -> narrow NBSP before PM.
        let v = resolve_read_path("shot 3.04.10 PM.png", cwd);
        assert!(
            v.iter().any(|p| p.to_string_lossy().contains('\u{202F}')),
            "variants: {v:?}"
        );
    }
}

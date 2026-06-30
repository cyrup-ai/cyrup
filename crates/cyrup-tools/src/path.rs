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

/// Expand a leading `~` to the user's home directory. Mirrors Pi `normalizePath`'s tilde branch
/// (paths.ts:66-72): `~` → home, `~/rest` → home/rest, and on Windows `~\rest` → home/rest.
fn expand_home(path: &str) -> String {
    if path == "~" {
        if let Some(home) = home_dir() {
            return home.to_string_lossy().into_owned();
        }
        return path.to_string();
    }
    let win_tilde = cfg!(windows) && path.starts_with("~\\");
    if let Some(rest) = path
        .strip_prefix("~/")
        .or_else(|| win_tilde.then(|| path.get(2..)).flatten())
        && let Some(home) = home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    path.to_string()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Minimal `file://` URL → path, mirroring Node `fileURLToPath` for the cases the tools see
/// (paths.ts:74-76): `file:///abs` and `file://localhost/abs` both yield `/abs`, with percent-escapes
/// decoded. Returns `None` if `s` is not a `file://` URL.
fn file_url_to_path(s: &str) -> Option<String> {
    let rest = s.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    Some(percent_decode(rest))
}

/// Decode `%XX` escapes (UTF-8) as Node's `fileURLToPath` does. Invalid escapes pass through.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b'%') => {
                let hi = bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16));
                let lo = bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16));
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            Some(&b) => {
                out.push(b);
                i += 1;
            }
            None => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Lexically collapse `.`/`..` segments and drop trailing separators, mirroring Node `path.resolve`
/// (paths.ts:84). Purely lexical — symlinks are NOT resolved. `..` at the root is dropped.
fn lexical_resolve(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut stack: Vec<Component> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => stack.push(comp),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(stack.last(), Some(Component::Normal(_))) {
                    stack.pop();
                }
            }
            Component::Normal(_) => stack.push(comp),
        }
    }
    let mut out = PathBuf::new();
    for c in stack {
        out.push(c.as_os_str());
    }
    out
}

/// Resolve a user-supplied path against `cwd` (R-03-005), faithful to Pi's `resolveToCwd` →
/// `resolvePath(input, cwd, { normalizeUnicodeSpaces: true, stripAtPrefix: true })`
/// (path-utils.ts:48-50, paths.ts:57-85). The order is exactly Pi's `normalizePath`: NO trim,
/// fold Unicode spaces, strip a leading `@`, expand `~`, decode `file://`, then `path.resolve`
/// (lexical `.`/`..` collapse + trailing-slash removal). Relatives resolve against `cwd`.
pub fn resolve_to_cwd(path: &str, cwd: &Path) -> PathBuf {
    // Pi does NOT pass `trim`, so leading/trailing whitespace is preserved (a leading space even
    // suppresses `@` stripping — `"  @x"` stays `"  @x"`).
    let mut normalized = normalize_unicode_spaces(path);
    if let Some(rest) = normalized.strip_prefix('@') {
        normalized = rest.to_string();
    }
    let normalized = expand_home(&normalized);
    if let Some(p) = file_url_to_path(&normalized) {
        return lexical_resolve(Path::new(&p));
    }
    let candidate = Path::new(&normalized);
    if candidate.is_absolute() {
        lexical_resolve(candidate)
    } else {
        lexical_resolve(&cwd.join(candidate))
    }
}

/// Candidate read paths in priority order (R-03-006, `resolveReadPathAsync`, path-utils.ts:86-116):
/// EXACTLY Pi's five candidates, in Pi's order — the resolved path first, then the macOS AM/PM
/// narrow-NBSP screenshot variant, then the NFD (decomposed) variant, then the curly-quote
/// (`'` → U+2019) variant, then the combined NFD+curly variant. Each non-primary variant is emitted
/// only when it differs from the resolved path, matching Pi's `!== resolved` guard. The `read` tool
/// tries each via the `FsOps` seam and uses the first that exists. Pi appends NO further variants
/// (there is no reverse U+2019→`'` candidate), so cyrup must not either.
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
    fn strips_leading_at_when_first_char() {
        // Pi `stripAtPrefix` only fires when the (space-normalized) string STARTS with `@`
        // (paths.ts:62). Captured Pi: resolveToCwd("@rel.txt","/work") => "/work/rel.txt".
        let cwd = Path::new("/work");
        assert_eq!(resolve_to_cwd("@rel.txt", cwd), PathBuf::from("/work/rel.txt"));
    }

    #[test]
    fn does_not_trim_whitespace() {
        // Pi passes no `trim` option, so surrounding spaces are PRESERVED and a leading space
        // suppresses `@` stripping. Captured Pi: resolveToCwd("  @rel.txt ","/work")
        // => "/work/  @rel.txt " (UM-2, paths.ts:57-58). Old cyrup trimmed → "/work/rel.txt".
        let cwd = Path::new("/work");
        assert_eq!(
            resolve_to_cwd("  @rel.txt ", cwd),
            PathBuf::from("/work/  @rel.txt ")
        );
    }

    #[test]
    fn collapses_dot_and_dotdot_like_node_resolve() {
        // UM-3: Pi uses Node `path.resolve`, which lexically collapses `.`/`..` and drops trailing
        // separators. Captured Pi ground truth (node path.resolve):
        let cwd = Path::new("/work");
        assert_eq!(resolve_to_cwd("a/../b.txt", cwd), PathBuf::from("/work/b.txt"));
        assert_eq!(resolve_to_cwd("./x/./y", cwd), PathBuf::from("/work/x/y"));
        assert_eq!(resolve_to_cwd("a/b/../../c", cwd), PathBuf::from("/work/c"));
        assert_eq!(resolve_to_cwd("sub/", cwd), PathBuf::from("/work/sub"));
        assert_eq!(
            resolve_to_cwd("../sibling", Path::new("/work/sub")),
            PathBuf::from("/work/sibling")
        );
    }

    #[test]
    fn decodes_file_url() {
        // UM-3: Pi resolves `file://` URLs via Node `fileURLToPath` (paths.ts:74-76).
        // Captured Pi: resolveToCwd("file:///etc/hosts","/work") => "/etc/hosts".
        let cwd = Path::new("/work");
        assert_eq!(
            resolve_to_cwd("file:///etc/hosts", cwd),
            PathBuf::from("/etc/hosts")
        );
        // Percent-escapes are decoded.
        assert_eq!(
            resolve_to_cwd("file:///tmp/a%20b.txt", cwd),
            PathBuf::from("/tmp/a b.txt")
        );
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

    /// Pi `resolveReadPathAsync` (path-utils.ts:86-116) emits NO reverse U+2019→`'` candidate. For a
    /// path that already contains a curly apostrophe and no straight one, Pi's five guards all reduce
    /// to the resolved path (amPm: none; NFD: identical; curly `s.contains('\'')`: false; NFD+curly:
    /// equals resolved), so the candidate set is exactly `[resolved]`. The removed cyrup-only 6th
    /// variant would have injected `/work/it's.txt`; assert it does not.
    #[test]
    fn read_path_has_no_reverse_curly_variant() {
        let cwd = Path::new("/work");
        let v = resolve_read_path("it\u{2019}s.txt", cwd);
        assert_eq!(v, vec![PathBuf::from("/work/it\u{2019}s.txt")], "variants: {v:?}");
        assert!(
            !v.iter().any(|p| p.to_string_lossy().contains("it's.txt")),
            "Pi emits no reverse U+2019→' candidate: {v:?}"
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

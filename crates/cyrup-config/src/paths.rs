//! Path normalization shared by every tier that accepts a user-written path.
//!
//! 1:1 with Pi `normalizePath` (`coding-agent/src/utils/paths.ts:57-78` @v0.83.0), which is the
//! single function pi applies to CLI path flags (`main.ts:625-628`), the `PI_*_DIR` env vars
//! (`config.ts:515-521`, `:367-372`), settings-declared paths, local package sources
//! (`package-manager.ts:2069-2071`) and the models-store path (`models-store.ts:53` @v0.84.1).
//!
//! cyrup previously had one private `expand_tilde` in `settings.rs` wired to two getters, so every
//! higher-precedence tier took its path raw (CFG-025, CFG-036).

use std::path::PathBuf;

/// The process home directory, as `os.homedir()` sees it.
fn ambient_home() -> Option<PathBuf> {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
}

/// Pi `normalizePath(input)` with default options (`paths.ts:57-78` @v0.83.0): expand `~` / `~/`
/// (and `~\` on win32 only, `:70`), then convert a `file://` URL to a path (`:73-76`). Anything
/// else is returned unchanged — this is NOT `resolve`, it does not make a relative path absolute.
pub fn normalize_path(input: &str) -> String {
    normalize_path_with_home(input, ambient_home().as_deref())
}

/// [`normalize_path`] with an explicit home directory, mirroring pi's `options.homeDir`
/// (`paths.ts:66`). `home = None` reproduces the "no home resolvable" case by leaving a `~` form
/// untouched.
pub fn normalize_path_with_home(input: &str, home: Option<&std::path::Path>) -> String {
    if let Some(home) = home {
        if input == "~" {
            return home.to_string_lossy().into_owned();
        }
        // `normalized.startsWith("~/") || (process.platform === "win32" &&
        //  normalized.startsWith("~\\"))` (paths.ts:70) — the backslash form is win32-only
        // upstream, so it is `cfg!(windows)`-gated here rather than always-on.
        let rest = input.strip_prefix("~/").or_else(|| {
            if cfg!(windows) {
                input.strip_prefix("~\\")
            } else {
                None
            }
        });
        if let Some(rest) = rest {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    if let Some(path) = file_url_to_path(input) {
        return path;
    }
    input.to_string()
}

/// [`normalize_path`] returning a [`PathBuf`].
pub fn normalize_path_buf(input: &str) -> PathBuf {
    PathBuf::from(normalize_path(input))
}

/// Node's `fileURLToPath` restricted to what `normalizePath` can hand it — a string matching
/// `/^file:\/\//` (`paths.ts:73`). Returns `None` when `input` is not a `file://` URL.
///
/// Node rejects a URL with a non-empty, non-`localhost` host; upstream's call is unguarded and
/// would throw. Here an unusable URL falls through as `None`, i.e. the string is left alone, which
/// is the same observable outcome as cyrup's pre-CFG-025 behaviour for that input and avoids
/// turning a settings typo into a startup failure.
fn file_url_to_path(input: &str) -> Option<String> {
    let rest = input.strip_prefix("file://")?;
    // `file://localhost/x` and `file:///x` both mean `/x`; any other authority is not a local path.
    let path = rest
        .strip_prefix("localhost/")
        .map(|p| format!("/{p}"))
        .or_else(|| {
            if rest.starts_with('/') {
                Some(rest.to_string())
            } else {
                None
            }
        })?;
    let decoded = percent_decode(&path);
    // `file:///C:/x` on win32 is `C:\x`; on any other platform a drive-letter URL is not a local
    // path Node would accept either, so it is left as-is.
    if cfg!(windows)
        && let Some(drive) = decoded.strip_prefix('/')
        && drive.len() >= 2
        && drive
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
        && drive.as_bytes().get(1) == Some(&b':')
    {
        return Some(drive.replace('/', "\\"));
    }
    Some(decoded)
}

/// Percent-decode a URL path component (`%20` → space). Invalid escapes are left verbatim, as a
/// lenient decoder must, so a literal `%` in a filename survives.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (bytes.get(i + 1), bytes.get(i + 2))
            && let (Some(h), Some(l)) = ((*h as char).to_digit(16), (*l as char).to_digit(16))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn expands_bare_tilde_and_tilde_slash_against_the_supplied_home() {
        let home = Path::new("/tmp/home-fixture");
        assert_eq!(
            normalize_path_with_home("~", Some(home)),
            "/tmp/home-fixture"
        );
        assert_eq!(
            normalize_path_with_home("~/team-skills", Some(home)),
            "/tmp/home-fixture/team-skills"
        );
    }

    #[test]
    fn leaves_a_bare_tilde_prefix_that_is_not_a_path_segment_alone() {
        // Pi tests `startsWith("~/")`, not `startsWith("~")` (paths.ts:70), so `~alice` is a
        // literal relative path, not a home-of-another-user expansion.
        let home = Path::new("/tmp/home-fixture");
        assert_eq!(normalize_path_with_home("~alice", Some(home)), "~alice");
    }

    #[test]
    fn converts_a_file_url_to_a_path_and_decodes_escapes() {
        assert_eq!(
            normalize_path_with_home("file:///abs/pack", None),
            "/abs/pack"
        );
        assert_eq!(
            normalize_path_with_home("file:///abs/my%20pack", None),
            "/abs/my pack"
        );
        assert_eq!(
            normalize_path_with_home("file://localhost/abs/pack", None),
            "/abs/pack"
        );
    }

    #[test]
    fn leaves_a_plain_relative_path_untouched() {
        let home = Path::new("/tmp/home-fixture");
        assert_eq!(normalize_path_with_home("extra", Some(home)), "extra");
        assert_eq!(normalize_path_with_home("/abs/x", Some(home)), "/abs/x");
    }
}

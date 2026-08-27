//! Path normalization shared by every tier that accepts a user-written path.
//!
//! 1:1 with Pi `normalizePath` (`coding-agent/src/utils/paths.ts:57-78` @v0.83.0), which is the
//! single function pi applies to CLI path flags (`main.ts:625-628`), the `PI_*_DIR` env vars
//! (`config.ts:515-521`, `:367-372`), settings-declared paths, local package sources
//! (`package-manager.ts:2069-2071`) and the models-store path (`models-store.ts:53` @v0.84.1).
//!
//! cyrup previously had one private `expand_tilde` in `settings.rs` wired to two getters, so every
//! higher-precedence tier took its path raw (CFG-025, CFG-036).
//!
//! The body also carries `normalizeWindowsShellPath` (`paths.ts:66-73`, `:83-85` @v0.84.1), which is
//! a v0.84.1 addition rather than part of the v0.83.0 baseline — see
//! [`normalize_windows_shell_path`] and DRIFT-046.

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
    // `if (process.platform === "win32") { normalized = normalizeWindowsShellPath(normalized); }`
    // (paths.ts:83-85 @v0.84.1) — BEFORE the tilde expansion, and inside the shared normalizer
    // rather than at any call site. DRIFT-046.
    let owned;
    let input = if cfg!(windows) {
        owned = normalize_windows_shell_path(input);
        owned.as_str()
    } else {
        input
    };
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

/// Convert Git Bash, MSYS, Cygwin and WSL drive paths to a form native Windows APIs accept.
///
/// Port of `normalizeWindowsShellPath` (`pi/packages/coding-agent/src/utils/paths.ts:66-73`
/// @v0.84.1, added by `9524d3a58` — absent at v0.83.0). Upstream is a regex,
/// `/^\/(?:mnt\/|cygdrive\/)?([a-z])(?:\/(.*))?$/i`, behind three guard clauses; this is the same
/// grammar hand-parsed, because `cyrup-config` carries no regex dependency.
///
/// The hand parse does not backtrack where the regex would, which is unobservable: the only inputs
/// that would need a backtrack are ones where `mnt/`/`cygdrive/` is followed by something that is
/// not a single drive letter (`/mnt/2`, `/mnt/mnt/e`), and the regex rejects those on the retry too.
///
/// Compiled on every platform so the upstream unit tests can drive it directly, exactly as
/// `test/paths.test.ts:133-150` does; only its APPLICATION in [`normalize_path_with_home`] is
/// `cfg!(windows)`-gated, mirroring `paths.ts:83-85`.
#[must_use]
pub fn normalize_windows_shell_path(file_path: &str) -> String {
    // `if (!filePath.startsWith("/") || filePath.startsWith("//") || filePath.includes("\\"))`
    // (paths.ts:68) — a relative path, a UNC path, and anything already in Windows form pass through.
    if !file_path.starts_with('/') || file_path.starts_with("//") || file_path.contains('\\') {
        return file_path.to_string();
    }
    let rest = &file_path[1..];
    let rest = rest
        .strip_prefix("mnt/")
        .or_else(|| rest.strip_prefix("cygdrive/"))
        .unwrap_or(rest);
    let mut chars = rest.chars();
    let Some(drive) = chars.next().filter(char::is_ascii_alphabetic) else {
        return file_path.to_string();
    };
    let after = &rest[drive.len_utf8()..];
    // `(?:\/(.*))?$` — either the drive letter ends the string, or a `/` and the rest.
    let suffix = if after.is_empty() {
        String::new()
    } else if let Some(tail) = after.strip_prefix('/') {
        // `match[2]?.replaceAll("/", "\\")` (paths.ts:71).
        tail.replace('/', "\\")
    } else {
        return file_path.to_string();
    };
    // `${match[1].toUpperCase()}:\\${suffix ?? ""}` (paths.ts:72) — a bare `/c` becomes `C:\`.
    format!("{}:\\{suffix}", drive.to_ascii_uppercase())
}

/// [`normalize_path`] returning a [`PathBuf`].
pub fn normalize_path_buf(input: &str) -> PathBuf {
    PathBuf::from(normalize_path(input))
}

/// Pi `resolvePath(input, baseDir, { homeDir, trim: true })` (`paths.ts:81-85` @v0.83.0), reached
/// through `PackageManager.resolvePathFromBase` (`package-manager.ts:2086-2088`).
///
/// [`normalize_path`] is the FIRST half only — it expands `~` and `file://` and stops. This is the
/// second half: normalize the input (with `trim: true`, which every package-manager call site
/// passes) and the base (with pi's DEFAULT options, i.e. no trim, `paths.ts:83`), join when the
/// input is relative, and finish with node's `path.resolve` — which normalizes `.` / `..`
/// LEXICALLY, never touching the filesystem and never following a symlink.
pub fn resolve_path_from_base(input: &str, base_dir: &std::path::Path) -> PathBuf {
    resolve_path_from_base_with_home(input, base_dir, ambient_home().as_deref())
}

/// [`resolve_path_from_base`] with an explicit home directory (pi's `options.homeDir`), so a test
/// can pin `~` expansion without depending on the process environment.
pub fn resolve_path_from_base_with_home(
    input: &str,
    base_dir: &std::path::Path,
    home: Option<&std::path::Path>,
) -> PathBuf {
    let normalized = normalize_path_with_home(input.trim(), home);
    let normalized = std::path::Path::new(&normalized);
    let normalized_base = normalize_path_with_home(&base_dir.to_string_lossy(), home);
    let joined = if normalized.is_absolute() {
        normalized.to_path_buf()
    } else {
        std::path::Path::new(&normalized_base).join(normalized)
    };
    // `nodeResolvePath` prepends `process.cwd()` when the accumulated path is still relative
    // (node `path.resolve` contract); every in-tree base dir is absolute, so this is the guard for a
    // caller that hands over a relative one rather than a path anyone takes.
    let joined = if joined.is_absolute() {
        joined
    } else {
        std::env::current_dir().unwrap_or_default().join(joined)
    };
    lexically_normalize(&joined)
}

/// The directory holding the assets shipped **with the agent itself** — `README.md`, `docs/`,
/// `examples/`. Pi `getPackageDir()` (`config.ts:385-397`), whose only consumers are the
/// shipped-asset paths at `config.ts:436-448`.
///
/// Resolution order, mirroring upstream's three tiers:
/// 1. `$CYRUP_ASSET_DIR`, run through [`normalize_path_buf`] — pi's `PI_PACKAGE_DIR` escape hatch
///    for Nix/Guix store paths (`config.ts:387-390`). It is deliberately NOT spelled
///    `CYRUP_PACKAGE_DIR`: that name is already bound to [`crate::ConfigDirs::package_dir`], the
///    installed-package STORE (`env.rs:99`), which is a different directory.
/// 2. The directory containing the running executable, when it directly holds a `README.md` —
///    the single-file-binary layout, pi's `dirname(process.execPath)` arm (`config.ts:392-394`).
/// 3. The nearest ancestor of that directory holding a `Cargo.toml` — the source-checkout arm,
///    pi's `findNodePackageDir` (`config.ts:368-383`) with `Cargo.toml` for `package.json`. A
///    `cargo run` binary lives at `<root>/target/<profile>/cyrup`, and `target/` carries no
///    manifest, so the walk lands on the workspace root without needing upstream's `dist/`
///    special case.
///
/// `None` means no asset root is discoverable; every caller must then behave as if the tree is
/// absent rather than substituting the cwd.
///
/// Resolved ONCE per process. The result is immutable for the process lifetime (unlike a session
/// cwd), and the render path calls it per paint, so the `existsSync`-equivalent walk must not run
/// per frame.
pub fn asset_dir() -> Option<&'static std::path::Path> {
    static ASSET_DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    ASSET_DIR.get_or_init(resolve_asset_dir).as_deref()
}

fn resolve_asset_dir() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("CYRUP_ASSET_DIR")
        && !raw.is_empty()
    {
        return Some(lexically_normalize(&normalize_path_buf(&raw.to_string_lossy())));
    }
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    if exe_dir.join("README.md").is_file() {
        return Some(lexically_normalize(exe_dir));
    }
    let mut dir = exe_dir;
    loop {
        if dir.join("Cargo.toml").is_file() {
            return Some(lexically_normalize(dir));
        }
        dir = dir.parent()?;
    }
}

/// The `.` / `..` collapse node's `path.resolve` performs after joining — purely lexical.
///
/// `..` at the root is dropped rather than escaping it (`path.resolve("/a/../..") === "/"`); on a
/// relative remainder it is kept, because there is nothing to cancel it against.
///
/// `pub(crate)` for [`crate::lock::FileLock`]'s layer-1 key, which needs this collapse WITHOUT the
/// tilde / `file://` tier above it and without a `String` round trip: a lock key must survive a
/// non-UTF-8 path byte-for-byte, or two distinct files can hash to one entry.
pub(crate) fn lexically_normalize(path: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component.as_os_str()),
            // `Components` already elides `.` and repeated separators, so this arm is defensive.
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
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
    // `.get(i)` rather than `bytes[i]`: the loop bound already made both reads infallible, but the
    // workspace denies `clippy::indexing_slicing`, and because cyrup-config sits at the bottom of
    // the dependency graph those two denials aborted `cargo clippy` for EVERY crate downstream of
    // it before their own code was ever linted. An unindexed read is the same instruction.
    while let Some(&byte) = bytes.get(i) {
        if byte == b'%'
            && let (Some(h), Some(l)) = (bytes.get(i + 1), bytes.get(i + 2))
            && let (Some(h), Some(l)) = ((*h as char).to_digit(16), (*l as char).to_digit(16))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(byte);
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

    /// `pi/packages/coding-agent/test/paths.test.ts:134-139` @v0.84.1, verbatim. DRIFT-046.
    #[test]
    fn converts_git_bash_msys_cygwin_and_wsl_drive_paths() {
        assert_eq!(
            normalize_windows_shell_path("/c/Users/example/project"),
            "C:\\Users\\example\\project"
        );
        assert_eq!(normalize_windows_shell_path("/cygdrive/d/work"), "D:\\work");
        assert_eq!(normalize_windows_shell_path("/mnt/e/source"), "E:\\source");
        assert_eq!(normalize_windows_shell_path("/c"), "C:\\");
    }

    /// `test/paths.test.ts:141-150` @v0.84.1 — the pass-through list, which is what stops the rule
    /// from mangling legitimate POSIX paths.
    #[test]
    fn leaves_other_path_forms_unchanged() {
        for path in [
            "C:/Users/example",
            "C:\\Users\\example",
            "//server/share/file",
            "/c/Users\\example",
            "relative/file",
            "/tmp/file",
        ] {
            assert_eq!(normalize_windows_shell_path(path), path, "{path}");
        }
        // Cases the upstream list does not name but the grammar decides: a drive-letter prefix that
        // is not a whole segment, and a `mnt`/`cygdrive` prefix not followed by a drive letter.
        for path in ["/mnt", "/mnt/", "/mnt/2", "/mnt/mnt/e", "/", "/1/x"] {
            assert_eq!(normalize_windows_shell_path(path), path, "{path}");
        }
    }

    /// `resolvePath` (paths.ts:81-85) is `normalizePath` PLUS the base-relative join, which is what
    /// `getPackageIdentity`'s `local:` arm depends on (package-manager.ts:1686-1687).
    #[test]
    fn resolves_a_relative_source_against_the_scope_base() {
        let home = Path::new("/tmp/home-fixture");
        assert_eq!(
            resolve_path_from_base_with_home("./pack", Path::new("/proj/.cyrup"), Some(home)),
            PathBuf::from("/proj/.cyrup/pack")
        );
        assert_eq!(
            resolve_path_from_base_with_home("pack", Path::new("/home/u/.cyrup/agent"), Some(home)),
            PathBuf::from("/home/u/.cyrup/agent/pack")
        );
    }

    #[test]
    fn an_absolute_tilde_or_file_url_source_ignores_the_base() {
        let home = Path::new("/tmp/home-fixture");
        let base = Path::new("/proj/.cyrup");
        assert_eq!(
            resolve_path_from_base_with_home("/abs/pack", base, Some(home)),
            PathBuf::from("/abs/pack")
        );
        assert_eq!(
            resolve_path_from_base_with_home("~/pack", base, Some(home)),
            PathBuf::from("/tmp/home-fixture/pack")
        );
        assert_eq!(
            resolve_path_from_base_with_home("file:///abs/pack", base, Some(home)),
            PathBuf::from("/abs/pack")
        );
    }

    /// node's `path.resolve` collapses `.`/`..` lexically and clamps at the root; it never stats the
    /// filesystem, so a component that does not exist resolves exactly like one that does.
    #[test]
    fn collapses_dot_and_dotdot_lexically_and_clamps_at_the_root() {
        let base = Path::new("/proj/.cyrup");
        for (input, expected) in [
            ("../pack", "/proj/pack"),
            ("./a/./b/../c", "/proj/.cyrup/a/c"),
            ("../../../../pack", "/pack"),
            ("/a/../..", "/"),
            ("  ./spaced  ", "/proj/.cyrup/spaced"),
        ] {
            assert_eq!(
                resolve_path_from_base_with_home(input, base, None),
                PathBuf::from(expected),
                "{input}"
            );
        }
    }

    #[test]
    fn leaves_a_plain_relative_path_untouched() {
        let home = Path::new("/tmp/home-fixture");
        assert_eq!(normalize_path_with_home("extra", Some(home)), "extra");
        assert_eq!(normalize_path_with_home("/abs/x", Some(home)), "/abs/x");
    }
}

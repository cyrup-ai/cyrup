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
            '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
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
    if changed { Some(out) } else { None }
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

/// Mirror of Node's `os.homedir()`, which is what Pi's `normalizePath` calls (`paths.ts:67`
/// @v0.83.0, `:88` @v0.84.1 — `const home = options.homeDir ?? homedir();`). `homedir()` resolves
/// `USERPROFILE` on **Windows**, not `HOME`, which is normally unset there — so a `HOME`-only
/// lookup left `~`/`~\rest` unexpanded and `resolve_to_cwd` joined the literal `~` onto the cwd
/// (TOOL-036).
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(home) = windows_home_from(
            std::env::var_os("USERPROFILE"),
            std::env::var_os("HOMEDRIVE"),
            std::env::var_os("HOMEPATH"),
        ) {
            return Some(home);
        }
    }
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The Windows arm of [`home_dir`], as a pure function of the three variables it reads, so the
/// precedence is testable on every platform without mutating a live process environment (env
/// mutation is `unsafe` under edition 2024 and races every other test in the binary). CFG-072.
///
/// **[CYRUP-DELTA — the `HOMEDRIVE`/`HOMEPATH` fallback WIDENS home resolution past upstream]**
/// pi resolves a home two ways and neither reads this pair: `normalizePath` calls Node's
/// `homedir()` (`utils/paths.ts:67` @v0.83.0, `:88` @v0.84.1 — `const home = options.homeDir ??
/// homedir();`), and the display paths read `process.env.HOME || process.env.USERPROFILE`
/// (`modes/interactive/components/footer.ts:114`, `components/tree-selector.ts:940` @v0.83.0).
/// `git -C pi grep -c HOMEDRIVE v0.83.0 -- packages/` → 0; same for `HOMEPATH`. Node's `homedir()`
/// is libuv's `uv_os_homedir`, which checks `USERPROFILE` and then falls back to a *syscall*
/// (`GetUserProfileDirectoryW`), not to this pair — so `HOMEDRIVE`+`HOMEPATH` is the
/// environment-visible spelling of the same directory rather than a literal port of that fallback.
///
/// **Kept, not dropped**, and stated here because the divergence runs in the direction nobody
/// looks: on a Windows configuration with `USERPROFILE` and `HOME` both unset but the pair set,
/// cyrup expands `~` and pi leaves it literal. That makes a "same input, different output" report
/// unreproducible upstream unless the extra branch is findable, and `CYRUP-DELTA` is the grep this
/// project's parity sweeps run. Ordering is upstream's: `USERPROFILE` first, always.
#[cfg(any(windows, test))]
fn windows_home_from(
    userprofile: Option<std::ffi::OsString>,
    homedrive: Option<std::ffi::OsString>,
    homepath: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(profile) = userprofile {
        return Some(PathBuf::from(profile));
    }
    if let (Some(drive), Some(path)) = (homedrive, homepath) {
        let mut joined = drive;
        joined.push(path);
        return Some(PathBuf::from(joined));
    }
    None
}

/// fd's global ignore file, or `None` when there is no resolvable config dir or the file does not
/// exist.
///
/// fd joins `fd/ignore` onto `etcetera::choose_base_strategy().config_dir()` and registers it only
/// when `is_file()` holds (fd 10.5.0 `src/walk.rs:371-375`). pi passes no
/// `--no-global-ignore-file` (find.ts:235-267), so `read_global_ignore` is true on every call.
pub(crate) fn fd_global_ignore_file() -> Option<PathBuf> {
    let file = fd_config_dir()?.join("fd").join("ignore");
    file.is_file().then_some(file)
}

/// `etcetera::choose_base_strategy().config_dir()`, reproduced.
///
/// `choose_base_strategy` selects the `Windows` strategy on Windows and the **`Xdg`** strategy on
/// every other target INCLUDING macOS (etcetera `src/base_strategy.rs:53-63`; the macro's second
/// argument is the base strategy) — so a macOS user's fd ignore file is `~/.config/fd/ignore`, not
/// `~/Library/Application Support/fd/ignore`.
///
/// * Xdg: `$XDG_CONFIG_HOME` when set AND ABSOLUTE, else `$HOME/.config`
///   (`base_strategy/xdg.rs`, `env_var_or_none` + `env_var_or_default`).
/// * Windows: `%APPDATA%` when set and non-empty, else `{home}\AppData\Roaming`
///   (`base_strategy/windows.rs:123-127, :190-196`).
///
/// **[CYRUP-DELTA — the Windows arm omits etcetera's `SHGetKnownFolderPath` fallback]** etcetera's
/// `dir_inner` falls back to a win32 known-folder lookup between the `%APPDATA%` read and the
/// home-relative default. A Windows session with `%APPDATA%` unset but a redirected roaming folder
/// would therefore have fd read a file cyrup does not. Stated rather than papered over, because
/// the direction of the divergence is "cyrup excludes fewer paths", which is invisible in output.
fn fd_config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").filter(|s| !s.is_empty()) {
            return Some(PathBuf::from(appdata));
        }
        return home_dir().map(|h| h.join("AppData").join("Roaming"));
    }
    #[cfg(not(windows))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
            && xdg.is_absolute()
        {
            return Some(xdg);
        }
        home_dir().map(|h| h.join(".config"))
    }
}

/// `normalizeWindowsShellPath` (v0.84.1 `paths.ts:67-73`): convert Git Bash / MSYS / Cygwin / WSL
/// drive paths (`/c/…`, `/cygdrive/c/…`, `/mnt/c/…`) into a form the native Windows APIs accept
/// (`C:\…`). Bails out unchanged on anything that is not a single-slash-rooted, backslash-free
/// path, exactly like the guard at `:68`.
///
/// **v0.84.1 upstream-drift** (DRIFT-046): the function does not exist at v0.83.0 — `git show
/// v0.83.0:packages/coding-agent/src/utils/paths.ts | grep -n normalizeWindowsShellPath` is empty
/// — and it is ported here because ADR-0007 puts Windows in scope and this is the shape a Windows
/// user's own `bash` tool output has.
fn normalize_windows_shell_path(file_path: &str) -> String {
    // `if (!filePath.startsWith("/") || filePath.startsWith("//") || filePath.includes("\\"))`
    if !file_path.starts_with('/') || file_path.starts_with("//") || file_path.contains('\\') {
        return file_path.to_string();
    }
    // `/^\/(?:mnt\/|cygdrive\/)?([a-z])(?:\/(.*))?$/i` — the `i` flag covers the literal prefixes
    // as well as the drive letter.
    let after_root = file_path.get(1..).unwrap_or("");
    let lowered = after_root.to_ascii_lowercase();
    let body = if let Some(rest) = lowered.strip_prefix("mnt/") {
        after_root
            .get(after_root.len() - rest.len()..)
            .unwrap_or("")
    } else if let Some(rest) = lowered.strip_prefix("cygdrive/") {
        after_root
            .get(after_root.len() - rest.len()..)
            .unwrap_or("")
    } else {
        after_root
    };
    let Some(drive) = body.chars().next().filter(char::is_ascii_alphabetic) else {
        return file_path.to_string();
    };
    let tail = body.get(drive.len_utf8()..).unwrap_or("");
    // `(?:\/(.*))?$` — either the drive letter ends the string, or a `/` and the rest follow.
    let suffix = if tail.is_empty() {
        String::new()
    } else if let Some(rest) = tail.strip_prefix('/') {
        rest.replace('/', "\\")
    } else {
        return file_path.to_string();
    };
    format!("{}:\\{suffix}", drive.to_ascii_uppercase())
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

/// Node `pathToFileURL(p).href` (`render-utils.ts:22`) — the inverse of [`file_url_to_path`].
///
/// Percent-encodes every byte outside the WHATWG *path* safe set, so the C0 controls, space, `"`,
/// `#`, `<`, `>`, `?`, `` ` ``, `{`, `}`, `%` and all non-ASCII bytes (UTF-8, byte-wise) come back
/// escaped. `/` is a separator and is preserved. On Windows the leading component is prefixed with
/// `/` so `C:\x` becomes `file:///C:/x`, matching Node.
///
/// The set is a **superset** of Node's for ASCII (Node's own pre-pass escapes only `%`, `#`, `?`,
/// `\n`, `\r`, `\t` on top of the WHATWG path set), so an exotic path can produce an href that is
/// more escaped than Node's. Both decode to the same bytes, so nothing observable diverges.
pub fn path_to_file_url(path: &Path) -> String {
    const SAFE: &[u8] = b"-._~!$&'()*+,;=:@/";
    let raw = path.to_string_lossy();
    let raw = if cfg!(windows) { raw.replace('\\', "/") } else { raw.into_owned() };
    let mut out = String::from("file://");
    if !raw.starts_with('/') {
        out.push('/');
    }
    for &b in raw.as_bytes() {
        if b.is_ascii_alphanumeric() || SAFE.contains(&b) {
            out.push(b as char);
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            // `get`, not indexing: the workspace denies `clippy::indexing_slicing`. The nibbles are
            // 0..=15 by construction, so the fallback is unreachable.
            let hi = HEX.get(usize::from(b >> 4)).copied().unwrap_or(b'0');
            let lo = HEX.get(usize::from(b & 0x0f)).copied().unwrap_or(b'0');
            out.push('%');
            out.push(char::from(hi));
            out.push(char::from(lo));
        }
    }
    out
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
    // Pi's placement is exact and the order matters: the drive-path conversion runs BEFORE the
    // tilde expansion (v0.84.1 `paths.ts:83-85`, between the `@`-strip at `:80-82` and the
    // `expandTilde` block at `:87-93`).
    if cfg!(windows) {
        normalized = normalize_windows_shell_path(&normalized);
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn relative_joins_cwd() {
        let cwd = Path::new("/work");
        assert_eq!(
            resolve_to_cwd("a/b.txt", cwd),
            PathBuf::from("/work/a/b.txt")
        );
    }

    #[test]
    fn absolute_passthrough() {
        let cwd = Path::new("/work");
        assert_eq!(
            resolve_to_cwd("/etc/hosts", cwd),
            PathBuf::from("/etc/hosts")
        );
    }

    #[test]
    fn strips_leading_at_when_first_char() {
        // Pi `stripAtPrefix` only fires when the (space-normalized) string STARTS with `@`
        // (paths.ts:62). Captured Pi: resolveToCwd("@rel.txt","/work") => "/work/rel.txt".
        let cwd = Path::new("/work");
        assert_eq!(
            resolve_to_cwd("@rel.txt", cwd),
            PathBuf::from("/work/rel.txt")
        );
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
        assert_eq!(
            resolve_to_cwd("a/../b.txt", cwd),
            PathBuf::from("/work/b.txt")
        );
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
            v.iter()
                .any(|p| p.to_string_lossy().contains("cafe\u{0301}.txt")),
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
        assert_eq!(
            v,
            vec![PathBuf::from("/work/it\u{2019}s.txt")],
            "variants: {v:?}"
        );
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

    /// TOOL-036 / DRIFT-046 — `normalizeWindowsShellPath` (v0.84.1 `paths.ts:67-73`). Tested
    /// directly (not through `resolve_to_cwd`) so the port is pinned on every host, since the call
    /// site is `cfg!(windows)`-guarded exactly as Pi's `:83-85` is.
    ///
    /// RED before the fix (no such function existed anywhere in this file); GREEN after.
    #[test]
    fn normalize_windows_shell_path_ports_the_drive_forms() {
        // Git Bash / MSYS.
        assert_eq!(
            normalize_windows_shell_path("/c/Users/x/f.txt"),
            r"C:\Users\x\f.txt"
        );
        // Cygwin.
        assert_eq!(
            normalize_windows_shell_path("/cygdrive/c/Users/x/f.txt"),
            r"C:\Users\x\f.txt"
        );
        // WSL.
        assert_eq!(
            normalize_windows_shell_path("/mnt/c/Users/x/f.txt"),
            r"C:\Users\x\f.txt"
        );
        // The drive letter is upper-cased; the `i` flag also covers the literal prefixes.
        assert_eq!(normalize_windows_shell_path("/D/tmp"), r"D:\tmp");
        assert_eq!(normalize_windows_shell_path("/CYGDRIVE/e/tmp"), r"E:\tmp");
        // Bare drive root: `(?:\/(.*))?` is optional, `suffix ?? ""`.
        assert_eq!(normalize_windows_shell_path("/c"), r"C:\");
        assert_eq!(normalize_windows_shell_path("/c/"), r"C:\");

        // The `:68` guard: not `/`-rooted, UNC-ish `//`, or already containing a backslash.
        assert_eq!(normalize_windows_shell_path("relative/x"), "relative/x");
        assert_eq!(
            normalize_windows_shell_path("//server/share"),
            "//server/share"
        );
        assert_eq!(normalize_windows_shell_path(r"/c/Users\x"), r"/c/Users\x");
        // No regex match: multi-char first segment, or a non-letter.
        assert_eq!(
            normalize_windows_shell_path("/usr/local/bin"),
            "/usr/local/bin"
        );
        assert_eq!(normalize_windows_shell_path("/1/x"), "/1/x");
        assert_eq!(normalize_windows_shell_path("/"), "/");
    }

    /// The unix behaviour of `resolve_to_cwd` is untouched by the win32 leg — the conversion is
    /// behind `cfg!(windows)`, matching Pi's `process.platform === "win32"` guard at `:83-85`.
    #[test]
    fn unix_resolve_is_unaffected_by_the_win32_leg() {
        let cwd = Path::new("/work");
        assert_eq!(
            resolve_to_cwd("/c/Users/x/f.txt", cwd),
            PathBuf::from("/c/Users/x/f.txt")
        );
        assert_eq!(
            resolve_to_cwd("/mnt/c/f.txt", cwd),
            PathBuf::from("/mnt/c/f.txt")
        );
    }

    /// CFG-072 — the `HOMEDRIVE`/`HOMEPATH` fallback and its precedence, pinned as a pure function
    /// so the assertion runs on every platform rather than only on a Windows runner.
    ///
    /// pi reads NEITHER variable at v0.83.0 (`git -C pi grep -c HOMEDRIVE v0.83.0 -- packages/`
    /// → 0): `normalizePath` calls Node's `homedir()` (`utils/paths.ts:67`), and the display paths
    /// read `process.env.HOME || process.env.USERPROFILE` (`footer.ts:114`,
    /// `tree-selector.ts:940`). So the third case below is the DIVERGENCE: cyrup resolves a home
    /// where pi resolves none.
    ///
    /// RED before this pass on its first line, for a mechanical reason that is the point of the
    /// item: `windows_home_from` did not exist — the branch was three inlined statements inside
    /// `home_dir`'s `#[cfg(windows)]` block, unreachable from any test compiled for a unix host and
    /// carrying no `[CYRUP-DELTA]`. The extraction is what makes the widening assertable at all.
    #[test]
    fn cfg072_homedrive_homepath_is_the_documented_fallback_after_userprofile() {
        use std::ffi::OsString;
        let os = |s: &str| Some(OsString::from(s));

        // `USERPROFILE` wins outright — upstream's own order (`footer.ts:114`), never overtaken.
        assert_eq!(
            windows_home_from(os(r"C:\Users\up"), os("D:"), os(r"\Users\hd")),
            Some(PathBuf::from(r"C:\Users\up")),
        );
        // ... including when only it is set.
        assert_eq!(
            windows_home_from(os(r"C:\Users\up"), None, None),
            Some(PathBuf::from(r"C:\Users\up"))
        );

        // THE DIVERGENCE: no `USERPROFILE`, but the pair is set. pi resolves nothing here; cyrup
        // joins the pair verbatim, with no separator inserted (`HOMEPATH` carries its own leading
        // backslash).
        assert_eq!(
            windows_home_from(None, os("D:"), os(r"\Users\hd")),
            Some(PathBuf::from(r"D:\Users\hd")),
        );

        // BOTH halves are required: a half-set pair must not widen resolution further still, and
        // must fall through to the `HOME` lookup in `home_dir` rather than fabricating a path.
        assert_eq!(windows_home_from(None, os("D:"), None), None);
        assert_eq!(windows_home_from(None, None, os(r"\Users\hd")), None);
        assert_eq!(windows_home_from(None, None, None), None);
    }

    /// CFG-072's other half: the widening has to be FINDABLE by the grep the parity sweeps run.
    /// RED before this pass — the only comment on the branch was "libuv's `uv_os_homedir` falls
    /// back to the account's profile directory; the `HOMEDRIVE`+`HOMEPATH` pair is the
    /// environment-visible spelling of the same thing", which carries no `CYRUP-DELTA` marker and
    /// does not say that pi reads neither name.
    #[test]
    fn cfg072_the_widening_carries_a_delta_naming_what_it_extends() {
        let src = include_str!("path.rs");
        let at = src
            .find("fn windows_home_from")
            .expect("the extracted Windows arm");
        let doc = &src[..at];
        let doc = &doc[doc
            .rfind("[CYRUP-DELTA")
            .expect("a delta annotation on the widening")..];

        for needle in [
            "HOMEDRIVE",
            "HOMEPATH",
            "USERPROFILE",
            "homedir()",
            "v0.83.0",
        ] {
            assert!(
                doc.contains(needle),
                "the delta must name `{needle}` — what cyrup reads, and what pi reads instead; got: {doc}"
            );
        }
    }
}

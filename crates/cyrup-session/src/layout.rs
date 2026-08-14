//! Storage layout: sessions root resolution (overridable for tests, R-04-002) and reversible
//! per-cwd encoding (R-04-001), Pi-compatible (`--<encoded-cwd>--`).

use std::path::{Path, PathBuf};

/// Resolves the directory holding a given cwd's session files under a configurable root.
///
/// Two modes, mirroring Pi's `SessionManager` directory choice (session-manager.ts:1430,1447,1457):
/// * **encoded** ([`Self::new`]/[`Self::for_cwd`]) — the *default* case: `dir()` nests a per-cwd
///   subdirectory `<root>/--<encoded-cwd>--`, exactly Pi's `getDefaultSessionDir(cwd)`
///   (session-manager.ts:461-474).
/// * **literal** ([`Self::literal`]) — an already-resolved directory used verbatim: `dir()` returns
///   `root` unchanged, no further cwd encoding. This is Pi's explicit-`sessionDir` branch
///   (`sessionDir ? normalizePath(sessionDir) : getDefaultSessionDir(cwd)`), and also Pi's
///   `createBranchedSession` reuse of `this.sessionDir` (session-manager.ts:918-920,1343) — the field
///   fixed once at construction, never re-encoded on branch. Applying encoding to an
///   already-resolved dir would nest one level too deep (gap-analysis 05, Findings 1 & 3).
#[derive(Clone, Debug)]
pub struct SessionLayout {
    root: PathBuf,
    cwd: PathBuf,
    /// When `true`, `dir()` appends `--<encoded-cwd>--`; when `false`, `dir()` returns `root` as-is.
    encode: bool,
}

impl SessionLayout {
    /// Encoded layout for `cwd` under an explicit sessions `root` (overridable — R-04-002): `dir()`
    /// yields `<root>/--<encoded-cwd>--`. This is Pi's *default* directory (`getDefaultSessionDir`).
    pub fn new(root: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), cwd: cwd.into(), encode: true }
    }

    /// Literal layout: `dir()` returns `root` verbatim, with **no** further per-cwd encoding. Use for
    /// an already-resolved session directory — an explicit `--session-dir` (Pi's
    /// `sessionDir ? normalizePath(sessionDir) : …`) or the currently-open session's own directory on
    /// fork/clone (Pi `createBranchedSession`'s `this.getSessionDir()` reuse). `cwd` is retained for
    /// the cross-project `continue_recent_filtered` guard but never re-encoded into the path.
    pub fn literal(root: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), cwd: cwd.into(), encode: false }
    }

    /// Layout for `cwd` under the default `~/.cyrup/agent/sessions` root (encoded).
    pub fn for_cwd(cwd: impl Into<PathBuf>) -> Self {
        Self::new(default_root(), cwd)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The directory holding this session's files: `<root>/--<encoded-cwd>--` in encoded mode, or
    /// `root` verbatim in literal mode.
    pub fn dir(&self) -> PathBuf {
        if self.encode {
            self.root.join(encode_cwd(&self.cwd))
        } else {
            self.root.clone()
        }
    }

    /// A fresh `<timestamp>_<uuid>.jsonl` path in this cwd's directory.
    pub fn new_file_path(&self, timestamp: &str, uuid: &str) -> PathBuf {
        self.dir().join(format!("{}_{}.jsonl", sanitize_ts(timestamp), uuid))
    }
}

/// The top-level sessions root (for `list_all`).
#[derive(Clone, Debug)]
pub struct SessionsRoot(pub PathBuf);

impl Default for SessionsRoot {
    fn default() -> Self {
        SessionsRoot(default_root())
    }
}

impl SessionsRoot {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// `~/.cyrup/agent/sessions` (falls back to `./.cyrup/...` if `$HOME` is unset).
pub fn default_root() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cyrup").join("agent").join("sessions")
}

/// Pi-compatible encoding: strip EXACTLY ONE leading separator, map `/` `\\` `:` → `-`, wrap in
/// `--…--`. Reversible enough to group/list a directory's sessions (R-04-001).
///
/// Pi (`session-manager.ts:479` @v0.83.0, byte-identical to `migrations.ts:112`):
/// ``const safePath = `--${resolvedCwd.replace(/^[/\\]/, "").replace(/[/\\:]/g, "-")}--`;``
/// The first `replace` is anchored and carries **no `g` flag**, so exactly one separator is
/// removed; only the second is global. `str::trim_start_matches` removes ALL leading matches, so a
/// UNC (`\\srv\share`) or doubled-slash cwd encoded to a different directory name than pi's
/// (`--srv-share--` vs pi's `---srv-share--`), making pi- and cyrup-written session trees mutually
/// invisible for those cwds.
pub fn encode_cwd(cwd: &Path) -> String {
    let raw = cwd.to_string_lossy();
    let trimmed = raw.strip_prefix('/').or_else(|| raw.strip_prefix('\\')).unwrap_or(&raw);
    let mapped: String = trimmed
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c })
        .collect();
    format!("--{mapped}--")
}

/// Make an RFC3339 timestamp filename-safe (`:` and `.` → `-`).
fn sanitize_ts(ts: &str) -> String {
    ts.chars()
        .map(|c| if matches!(c, ':' | '.') { '-' } else { c })
        .collect()
}

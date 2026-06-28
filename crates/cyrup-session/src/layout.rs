//! Storage layout: sessions root resolution (overridable for tests, R-04-002) and reversible
//! per-cwd encoding (R-04-001), Pi-compatible (`--<encoded-cwd>--`).

use std::path::{Path, PathBuf};

/// Resolves the directory holding a given cwd's session files under a configurable root.
#[derive(Clone, Debug)]
pub struct SessionLayout {
    root: PathBuf,
    cwd: PathBuf,
}

impl SessionLayout {
    /// Layout for `cwd` under an explicit sessions `root` (overridable — R-04-002).
    pub fn new(root: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), cwd: cwd.into() }
    }

    /// Layout for `cwd` under the default `~/.cyrup/agent/sessions` root.
    pub fn for_cwd(cwd: impl Into<PathBuf>) -> Self {
        Self::new(default_root(), cwd)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The per-cwd directory: `<root>/--<encoded-cwd>--`.
    pub fn dir(&self) -> PathBuf {
        self.root.join(encode_cwd(&self.cwd))
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

/// Pi-compatible encoding: strip a leading separator, map `/` `\\` `:` → `-`, wrap in `--…--`.
/// Reversible enough to group/list a directory's sessions (R-04-001).
pub fn encode_cwd(cwd: &Path) -> String {
    let raw = cwd.to_string_lossy();
    let trimmed = raw.trim_start_matches(['/', '\\']);
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

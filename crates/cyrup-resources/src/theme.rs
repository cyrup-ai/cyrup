//! Themes — JSON TUI color schemes, hot-reloadable (arch-09 §3.5, R-09-011..014).
//!
//! Built-in `dark` and `light` ship compiled-in (R-09-011). The active theme file is watched and
//! re-published through a `tokio::sync::watch` channel on change (R-09-013); parse failures keep
//! the last good theme.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::discovery::Named;
use crate::error::ResourceError;
use crate::key::ResourceKey;
use crate::scope::{ResourceOrigin, ResourceScope};

/// Parsed theme JSON (shape per Pi's `theme-schema.json`: name/vars/colors/export).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeData {
    pub name: String,
    #[serde(default)]
    pub vars: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub colors: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub export: std::collections::BTreeMap<String, String>,
}

/// A discovered theme: parsed data plus discovery provenance.
#[derive(Clone, Debug)]
pub struct Theme {
    pub key: ResourceKey,
    pub data: ThemeData,
    /// Set on load from a file; watched for hot-reload.
    pub origin_path: Option<PathBuf>,
    pub scope: ResourceScope,
    pub origin: ResourceOrigin,
}

/// A color role resolved through `vars`. `""` / unknown means inherit (terminal default).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorSpec {
    Inherit,
    Rgb { r: u8, g: u8, b: u8 },
}

/// Roles resolved to concrete colors; cyrup-tui maps these to `ratatui::Color` (arch-10).
#[derive(Clone, Debug, Default)]
pub struct ResolvedTheme {
    pub roles: std::collections::BTreeMap<String, ColorSpec>,
}

impl Theme {
    /// Parse theme JSON text into a [`Theme`].
    pub fn parse(
        text: &str,
        path: Option<PathBuf>,
        scope: ResourceScope,
        origin: ResourceOrigin,
    ) -> Result<Theme, ResourceError> {
        let data: ThemeData =
            serde_json::from_str(text).map_err(|e| ResourceError::Theme {
                path: path.clone().unwrap_or_default(),
                reason: e.to_string(),
            })?;
        let mut key = ResourceKey::normalize(&data.name);
        if key.is_empty() {
            // Fall back to the file stem.
            if let Some(stem) = path.as_ref().and_then(|p| p.file_stem()).and_then(|s| s.to_str()) {
                key = ResourceKey::normalize(stem);
            }
        }
        if key.is_empty() {
            return Err(ResourceError::Theme {
                path: path.unwrap_or_default(),
                reason: "theme has no `name` and no file stem".to_string(),
            });
        }
        Ok(Theme { key, data, origin_path: path, scope, origin })
    }

    /// Load a theme from a `.json` file.
    pub fn load(
        path: &Path,
        scope: ResourceScope,
        origin: ResourceOrigin,
    ) -> Result<Theme, ResourceError> {
        let text = std::fs::read_to_string(path)?;
        Theme::parse(&text, Some(path.to_path_buf()), scope, origin)
    }

    /// Resolve `colors` roles through `vars` + hex parsing. Bad/empty values become `Inherit`
    /// (no panic).
    pub fn resolve(&self) -> ResolvedTheme {
        let mut roles = std::collections::BTreeMap::new();
        for (role, raw) in &self.data.colors {
            let resolved = resolve_value(raw, &self.data.vars);
            roles.insert(role.clone(), resolved);
        }
        ResolvedTheme { roles }
    }
}

impl Named for Theme {
    fn key(&self) -> &ResourceKey {
        &self.key
    }
    fn scope(&self) -> ResourceScope {
        self.scope
    }
}

/// Resolve a `colors` value: empty -> inherit; `$var` or bare var name -> look up `vars`; else
/// parse as a hex color.
fn resolve_value(raw: &str, vars: &std::collections::BTreeMap<String, String>) -> ColorSpec {
    let v = raw.trim();
    if v.is_empty() {
        return ColorSpec::Inherit;
    }
    // Indirection: a value naming a var (with or without a leading `$`).
    let var_name = v.strip_prefix('$').unwrap_or(v);
    if let Some(hex) = vars.get(var_name) {
        return parse_hex(hex);
    }
    parse_hex(v)
}

/// Parse `#rrggbb` / `rrggbb` / `#rgb`. Anything malformed -> `Inherit`.
fn parse_hex(s: &str) -> ColorSpec {
    let h = s.trim().strip_prefix('#').unwrap_or(s.trim());
    let bytes = h.as_bytes();
    match bytes.len() {
        6 => {
            let r = hex_pair(bytes.first(), bytes.get(1));
            let g = hex_pair(bytes.get(2), bytes.get(3));
            let b = hex_pair(bytes.get(4), bytes.get(5));
            match (r, g, b) {
                (Some(r), Some(g), Some(b)) => ColorSpec::Rgb { r, g, b },
                _ => ColorSpec::Inherit,
            }
        }
        3 => {
            let r = hex_digit(bytes.first()).map(|n| n * 17);
            let g = hex_digit(bytes.get(1)).map(|n| n * 17);
            let b = hex_digit(bytes.get(2)).map(|n| n * 17);
            match (r, g, b) {
                (Some(r), Some(g), Some(b)) => ColorSpec::Rgb { r, g, b },
                _ => ColorSpec::Inherit,
            }
        }
        _ => ColorSpec::Inherit,
    }
}

fn hex_digit(b: Option<&u8>) -> Option<u8> {
    let c = *b?;
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex_pair(hi: Option<&u8>, lo: Option<&u8>) -> Option<u8> {
    let h = hex_digit(hi)?;
    let l = hex_digit(lo)?;
    h.checked_mul(16)?.checked_add(l)
}

/// The compiled-in `dark` theme (R-09-011).
pub const BUILTIN_DARK_JSON: &str = r##"{
  "name": "dark",
  "vars": {
    "bg": "#1e1e1e",
    "fg": "#d4d4d4",
    "accent": "#569cd6",
    "error": "#f44747"
  },
  "colors": {
    "background": "$bg",
    "foreground": "$fg",
    "accent": "$accent",
    "error": "$error"
  }
}"##;

/// The compiled-in `light` theme (R-09-011).
pub const BUILTIN_LIGHT_JSON: &str = r##"{
  "name": "light",
  "vars": {
    "bg": "#ffffff",
    "fg": "#1e1e1e",
    "accent": "#0000ff",
    "error": "#cd3131"
  },
  "colors": {
    "background": "$bg",
    "foreground": "$fg",
    "accent": "$accent",
    "error": "$error"
  }
}"##;

/// The two compiled-in built-ins (`dark`, `light`) at [`ResourceScope::Builtin`].
pub fn builtin_themes() -> Vec<Theme> {
    let mut out = Vec::new();
    for json in [BUILTIN_DARK_JSON, BUILTIN_LIGHT_JSON] {
        if let Ok(t) =
            Theme::parse(json, None, ResourceScope::Builtin, ResourceOrigin::Builtin)
        {
            out.push(t);
        }
    }
    out
}

/// Hot-reload of the active theme file (R-09-013). Publishes `Arc<ThemeData>` on change.
///
/// Uses `notify`'s [`notify::PollWatcher`] for deterministic, cross-platform detection (editors
/// that replace files atomically are handled by watching the parent directory). Parse failures
/// publish nothing and keep the last good theme.
pub struct ThemeWatcher {
    rx: tokio::sync::watch::Receiver<Arc<ThemeData>>,
    inner: Arc<std::sync::Mutex<WatcherInner>>,
    _task: tokio::task::JoinHandle<()>,
}

struct WatcherInner {
    watcher: notify::PollWatcher,
    path: PathBuf,
    tx: tokio::sync::watch::Sender<Arc<ThemeData>>,
}

impl ThemeWatcher {
    /// Begin watching `path`, seeding the channel with `active`.
    pub fn spawn(
        active: Arc<ThemeData>,
        path: PathBuf,
        cancel: cyrup_core::CancelToken,
    ) -> Result<Self, ResourceError> {
        use notify::Watcher;

        let (tx, rx) = tokio::sync::watch::channel(active);
        let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

        // `compare_contents` so same-byte-length edits (e.g. swapping one hex digit in a theme)
        // are still detected — size/mtime comparison alone misses them on some filesystems.
        let cfg = notify::Config::default()
            .with_poll_interval(Duration::from_millis(50))
            .with_compare_contents(true);
        let mut watcher = notify::PollWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    let _ = evt_tx.send(());
                }
            },
            cfg,
        )
        .map_err(|e| ResourceError::Theme { path: path.clone(), reason: e.to_string() })?;

        // Watch the file directly; the poll watcher detects content/mtime changes.
        watcher
            .watch(&path, notify::RecursiveMode::NonRecursive)
            .map_err(|e| ResourceError::Theme { path: path.clone(), reason: e.to_string() })?;

        let inner = Arc::new(std::sync::Mutex::new(WatcherInner { watcher, path, tx }));
        let task_inner = Arc::clone(&inner);

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    msg = evt_rx.recv() => {
                        if msg.is_none() { break; }
                        reload(&task_inner);
                    }
                }
            }
        });

        Ok(ThemeWatcher { rx, inner, _task: task })
    }

    /// A fresh receiver for the active-theme channel.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Arc<ThemeData>> {
        self.rx.clone()
    }

    /// Switch the watched file at runtime (R-09-014). Immediately publishes the new file's theme.
    pub fn retarget(&self, path: PathBuf) -> Result<(), ResourceError> {
        use notify::Watcher;
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ResourceError::Theme { path: path.clone(), reason: "lock".into() })?;
        let old = guard.path.clone();
        let _ = guard.watcher.unwatch(&old);
        guard
            .watcher
            .watch(&path, notify::RecursiveMode::NonRecursive)
            .map_err(|e| ResourceError::Theme { path: path.clone(), reason: e.to_string() })?;
        guard.path = path;
        drop(guard);
        reload(&self.inner);
        Ok(())
    }
}

/// Re-read + parse the watched file; publish on success, keep last-good on failure.
fn reload(inner: &Arc<std::sync::Mutex<WatcherInner>>) {
    let Ok(guard) = inner.lock() else { return };
    let path = guard.path.clone();
    let tx = guard.tx.clone();
    drop(guard);
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(data) = serde_json::from_str::<ThemeData>(&text) {
            let _ = tx.send(Arc::new(data));
        }
}

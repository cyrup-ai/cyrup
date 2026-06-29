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
///
/// Color values may be hex strings, var references, **or 256-color integer indices** (0-255,
/// theme.ts:23-28). Integer values are mapped to their truecolor RGB via the standard xterm-256
/// palette at parse time and stored as `#rrggbb`, so downstream consumers see a uniform string map.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeData {
    pub name: String,
    #[serde(default, deserialize_with = "de_color_map")]
    pub vars: std::collections::BTreeMap<String, String>,
    #[serde(default, deserialize_with = "de_color_map")]
    pub colors: std::collections::BTreeMap<String, String>,
    #[serde(default, deserialize_with = "de_color_map")]
    pub export: std::collections::BTreeMap<String, String>,
}

/// A raw theme color value: a string (hex / var-ref / "") or a 256-color integer index.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ColorValueRaw {
    Int(i64),
    Str(String),
}

/// Deserialize a color map accepting string or integer values (theme.ts ColorValueSchema). Integer
/// indices 0-255 are converted to `#rrggbb` via the xterm-256 palette; out-of-range integers become
/// the empty (inherit) value.
fn de_color_map<'de, D>(
    de: D,
) -> Result<std::collections::BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: std::collections::BTreeMap<String, ColorValueRaw> =
        serde::Deserialize::deserialize(de)?;
    Ok(raw
        .into_iter()
        .map(|(k, v)| {
            let s = match v {
                ColorValueRaw::Str(s) => s,
                ColorValueRaw::Int(n) if (0..=255).contains(&n) => {
                    let (r, g, b) = index_to_rgb(n as u8);
                    format!("#{r:02x}{g:02x}{b:02x}")
                }
                ColorValueRaw::Int(_) => String::new(),
            };
            (k, s)
        })
        .collect())
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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColorSpec {
    #[default]
    Inherit,
    Rgb { r: u8, g: u8, b: u8 },
}

/// Roles resolved to concrete colors; cyrup-tui maps these to `ratatui::Color` (arch-10).
#[derive(Clone, Debug, Default)]
pub struct ResolvedTheme {
    pub roles: std::collections::BTreeMap<String, ColorSpec>,
}

/// The fixed set of required `colors` tokens every theme must define (theme.ts:34-93).
///
/// Pi compiles `colors` as a closed `Type.Object` of these ~51 keys; a theme that omits any of
/// them fails validation with a precise "Missing required color tokens" error (theme.ts:514-548).
/// Order here matches the schema declaration (Core UI → Backgrounds/Content → Markdown → Tool
/// Diffs → Syntax → Thinking borders → Bash mode).
pub const REQUIRED_COLOR_TOKENS: [&str; 51] = [
    // Core UI
    "accent",
    "border",
    "borderAccent",
    "borderMuted",
    "success",
    "error",
    "warning",
    "muted",
    "dim",
    "text",
    "thinkingText",
    // Backgrounds & Content Text
    "selectedBg",
    "userMessageBg",
    "userMessageText",
    "customMessageBg",
    "customMessageText",
    "customMessageLabel",
    "toolPendingBg",
    "toolSuccessBg",
    "toolErrorBg",
    "toolTitle",
    "toolOutput",
    // Markdown
    "mdHeading",
    "mdLink",
    "mdLinkUrl",
    "mdCode",
    "mdCodeBlock",
    "mdCodeBlockBorder",
    "mdQuote",
    "mdQuoteBorder",
    "mdHr",
    "mdListBullet",
    // Tool Diffs
    "toolDiffAdded",
    "toolDiffRemoved",
    "toolDiffContext",
    // Syntax Highlighting
    "syntaxComment",
    "syntaxKeyword",
    "syntaxFunction",
    "syntaxVariable",
    "syntaxString",
    "syntaxNumber",
    "syntaxType",
    "syntaxOperator",
    "syntaxPunctuation",
    // Thinking Level Borders
    "thinkingOff",
    "thinkingMinimal",
    "thinkingLow",
    "thinkingMedium",
    "thinkingHigh",
    "thinkingXhigh",
    // Bash Mode
    "bashMode",
];

/// The three typed export colors (HTML export, theme.ts:94-100), resolved through `vars`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportColors {
    pub page_bg: ColorSpec,
    pub card_bg: ColorSpec,
    pub info_bg: ColorSpec,
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
        // Validate the fixed `colors` schema: every required token must be present. Pi rejects a
        // theme that omits any token with a precise, sorted "Missing required color tokens" error
        // (theme.ts:514-548). Validation runs before the name check, matching Pi's order
        // (parseThemeJson → assertThemeNameIsValid).
        let missing: Vec<&str> = REQUIRED_COLOR_TOKENS
            .iter()
            .copied()
            .filter(|token| !data.colors.contains_key(*token))
            .collect();
        if !missing.is_empty() {
            let label = path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| data.name.clone());
            let mut sorted = missing;
            sorted.sort_unstable();
            let list = sorted
                .iter()
                .map(|color| format!("  - {color}"))
                .collect::<Vec<_>>()
                .join("\n");
            let reason = format!(
                "Invalid theme \"{label}\":\n\nMissing required color tokens:\n{list}\n\n\
                 Please add these colors to your theme's \"colors\" object.\n\
                 See the built-in themes (dark.json, light.json) for reference values."
            );
            return Err(ResourceError::Theme {
                path: path.unwrap_or_default(),
                reason,
            });
        }
        // Theme name must not contain `/` — reserved for the `light/dark` auto-theme setting
        // (theme.ts:506-512,551).
        if data.name.contains('/') {
            return Err(ResourceError::Theme {
                path: path.unwrap_or_default(),
                reason: format!("theme name must not contain '/': {}", data.name),
            });
        }
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

    /// Resolve the typed `export` section (`pageBg`/`cardBg`/`infoBg`) through `vars` for HTML
    /// export (theme.ts:94-100; arch-12). Absent keys degrade to `Inherit`.
    pub fn resolve_export(&self) -> ExportColors {
        let get = |k: &str| {
            self.data
                .export
                .get(k)
                .map(|raw| resolve_value(raw, &self.data.vars))
                .unwrap_or(ColorSpec::Inherit)
        };
        ExportColors {
            page_bg: get("pageBg"),
            card_bg: get("cardBg"),
            info_bg: get("infoBg"),
        }
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

/// Resolve a `colors` value **recursively** through `vars` (theme.ts:290-306).
///
/// Empty → inherit. A value starting with `#` is a terminal hex color. Otherwise it is treated as a
/// var reference (an optional leading `$` is accepted for cyrup compatibility; Pi uses the bare
/// name) and resolved recursively. Circular references and unknown vars degrade to `Inherit` (Pi
/// throws; cyrup's `resolve()` is total and never panics, R-00-009).
fn resolve_value(raw: &str, vars: &std::collections::BTreeMap<String, String>) -> ColorSpec {
    resolve_value_inner(raw, vars, &mut std::collections::BTreeSet::new())
}

fn resolve_value_inner(
    raw: &str,
    vars: &std::collections::BTreeMap<String, String>,
    seen: &mut std::collections::BTreeSet<String>,
) -> ColorSpec {
    let v = raw.trim();
    if v.is_empty() {
        return ColorSpec::Inherit;
    }
    if v.starts_with('#') {
        return parse_hex(v);
    }
    // Var reference (with or without a leading `$`).
    let var_name = v.strip_prefix('$').unwrap_or(v);
    if seen.contains(var_name) {
        return ColorSpec::Inherit; // circular reference
    }
    if let Some(next) = vars.get(var_name) {
        seen.insert(var_name.to_string());
        return resolve_value_inner(next, vars, seen);
    }
    // Not a known var: last-resort hex parse (e.g. `rrggbb` without `#`).
    parse_hex(v)
}

/// Map a 256-color palette index to truecolor RGB (standard xterm-256 palette).
fn index_to_rgb(idx: u8) -> (u8, u8, u8) {
    const SYSTEM: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match idx {
        0..=15 => SYSTEM.get(idx as usize).copied().unwrap_or((0, 0, 0)),
        16..=231 => {
            let i = idx - 16;
            let r = CUBE.get((i / 36) as usize).copied().unwrap_or(0);
            let g = CUBE.get(((i / 6) % 6) as usize).copied().unwrap_or(0);
            let b = CUBE.get((i % 6) as usize).copied().unwrap_or(0);
            (r, g, b)
        }
        232..=255 => {
            let gray = 8u8.saturating_add((idx - 232).saturating_mul(10));
            (gray, gray, gray)
        }
    }
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

/// The compiled-in `dark` theme (R-09-011), ported verbatim from Pi's `dark.json` (all 51 tokens).
pub const BUILTIN_DARK_JSON: &str = r##"{
  "name": "dark",
  "vars": {
    "cyan": "#00d7ff",
    "blue": "#5f87ff",
    "green": "#b5bd68",
    "red": "#cc6666",
    "yellow": "#ffff00",
    "text": "#d4d4d4",
    "gray": "#808080",
    "dimGray": "#666666",
    "darkGray": "#505050",
    "accent": "#8abeb7",
    "selectedBg": "#3a3a4a",
    "userMsgBg": "#343541",
    "toolPendingBg": "#282832",
    "toolSuccessBg": "#283228",
    "toolErrorBg": "#3c2828",
    "customMsgBg": "#2d2838"
  },
  "colors": {
    "accent": "accent",
    "border": "blue",
    "borderAccent": "cyan",
    "borderMuted": "darkGray",
    "success": "green",
    "error": "red",
    "warning": "yellow",
    "muted": "gray",
    "dim": "dimGray",
    "text": "text",
    "thinkingText": "gray",

    "selectedBg": "selectedBg",
    "userMessageBg": "userMsgBg",
    "userMessageText": "text",
    "customMessageBg": "customMsgBg",
    "customMessageText": "text",
    "customMessageLabel": "#9575cd",
    "toolPendingBg": "toolPendingBg",
    "toolSuccessBg": "toolSuccessBg",
    "toolErrorBg": "toolErrorBg",
    "toolTitle": "text",
    "toolOutput": "gray",

    "mdHeading": "#f0c674",
    "mdLink": "#81a2be",
    "mdLinkUrl": "dimGray",
    "mdCode": "accent",
    "mdCodeBlock": "green",
    "mdCodeBlockBorder": "gray",
    "mdQuote": "gray",
    "mdQuoteBorder": "gray",
    "mdHr": "gray",
    "mdListBullet": "accent",

    "toolDiffAdded": "green",
    "toolDiffRemoved": "red",
    "toolDiffContext": "gray",

    "syntaxComment": "#6A9955",
    "syntaxKeyword": "#569CD6",
    "syntaxFunction": "#DCDCAA",
    "syntaxVariable": "#9CDCFE",
    "syntaxString": "#CE9178",
    "syntaxNumber": "#B5CEA8",
    "syntaxType": "#4EC9B0",
    "syntaxOperator": "#D4D4D4",
    "syntaxPunctuation": "#D4D4D4",

    "thinkingOff": "darkGray",
    "thinkingMinimal": "#6e6e6e",
    "thinkingLow": "#5f87af",
    "thinkingMedium": "#81a2be",
    "thinkingHigh": "#b294bb",
    "thinkingXhigh": "#d183e8",

    "bashMode": "green"
  },
  "export": {
    "pageBg": "#18181e",
    "cardBg": "#1e1e24",
    "infoBg": "#3c3728"
  }
}"##;

/// The compiled-in `light` theme (R-09-011), ported verbatim from Pi's `light.json` (all 51 tokens).
pub const BUILTIN_LIGHT_JSON: &str = r##"{
  "name": "light",
  "vars": {
    "teal": "#5a8080",
    "blue": "#547da7",
    "green": "#588458",
    "red": "#aa5555",
    "yellow": "#9a7326",
    "text": "#1f2328",
    "mediumGray": "#6c6c6c",
    "dimGray": "#767676",
    "lightGray": "#b0b0b0",
    "selectedBg": "#d0d0e0",
    "userMsgBg": "#e8e8e8",
    "toolPendingBg": "#e8e8f0",
    "toolSuccessBg": "#e8f0e8",
    "toolErrorBg": "#f0e8e8",
    "customMsgBg": "#ede7f6"
  },
  "colors": {
    "accent": "teal",
    "border": "blue",
    "borderAccent": "teal",
    "borderMuted": "lightGray",
    "success": "green",
    "error": "red",
    "warning": "yellow",
    "muted": "mediumGray",
    "dim": "dimGray",
    "text": "text",
    "thinkingText": "mediumGray",

    "selectedBg": "selectedBg",
    "userMessageBg": "userMsgBg",
    "userMessageText": "text",
    "customMessageBg": "customMsgBg",
    "customMessageText": "text",
    "customMessageLabel": "#7e57c2",
    "toolPendingBg": "toolPendingBg",
    "toolSuccessBg": "toolSuccessBg",
    "toolErrorBg": "toolErrorBg",
    "toolTitle": "text",
    "toolOutput": "mediumGray",

    "mdHeading": "yellow",
    "mdLink": "blue",
    "mdLinkUrl": "dimGray",
    "mdCode": "teal",
    "mdCodeBlock": "green",
    "mdCodeBlockBorder": "mediumGray",
    "mdQuote": "mediumGray",
    "mdQuoteBorder": "mediumGray",
    "mdHr": "mediumGray",
    "mdListBullet": "green",

    "toolDiffAdded": "green",
    "toolDiffRemoved": "red",
    "toolDiffContext": "mediumGray",

    "syntaxComment": "#008000",
    "syntaxKeyword": "#0000FF",
    "syntaxFunction": "#795E26",
    "syntaxVariable": "#001080",
    "syntaxString": "#A31515",
    "syntaxNumber": "#098658",
    "syntaxType": "#267F99",
    "syntaxOperator": "#000000",
    "syntaxPunctuation": "#000000",

    "thinkingOff": "lightGray",
    "thinkingMinimal": "#767676",
    "thinkingLow": "blue",
    "thinkingMedium": "teal",
    "thinkingHigh": "#875f87",
    "thinkingXhigh": "#8b008b",

    "bashMode": "green"
  },
  "export": {
    "pageBg": "#f8f8f8",
    "cardBg": "#ffffff",
    "infoBg": "#fffae6"
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
    // Re-validate through `Theme::parse` so an incomplete/invalid edit (missing required tokens,
    // bad name) keeps the last good theme instead of publishing a broken one — matching Pi's
    // watch handler, which re-parses via `parseThemeJsonContent` and keeps the prior theme on
    // failure (theme.ts watch path; G1). Only `.data` is published, so scope/origin are nominal.
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(theme) = Theme::parse(
            &text,
            Some(path.clone()),
            ResourceScope::Cli,
            ResourceOrigin::Builtin,
        )
    {
        let _ = tx.send(Arc::new(theme.data));
    }
}

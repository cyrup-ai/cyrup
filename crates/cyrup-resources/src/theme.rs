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

/// Parsed theme JSON (shape per Pi's `theme-schema.json`: name/appearance/vars/colors/export).
///
/// Color values may be hex strings, `oklch(…)` / `okhsl(…)` functions, var references, the empty
/// string (terminal default), **or 256-color integer indices** (0-255, theme.ts:23-28). The two
/// kinds stay distinct in [`ColorValue`], as they do in pi's `ColorValue = string | number`: an
/// index is NOT converted to RGB, because indices 0-15 name the user's own terminal palette.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeData {
    pub name: String,
    /// The background the theme is designed for (`theme-schema.json:17-21`). `None` ⇒ detected from
    /// the theme's own colours ([`Theme::appearance`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<Appearance>,
    #[serde(default)]
    pub vars: std::collections::BTreeMap<String, ColorValue>,
    #[serde(default)]
    pub colors: std::collections::BTreeMap<String, ColorValue>,
    #[serde(default)]
    pub export: std::collections::BTreeMap<String, ColorValue>,
}

impl ThemeData {
    /// Resolve every `colors` role through `vars` and settle the theme's [`Appearance`] — the shared
    /// path for a discovered [`Theme`], a built-in, and a document the file watcher just re-read.
    /// A role that does not resolve degrades to `Inherit` rather than panicking (R-00-009); a
    /// document that came through [`Theme::parse`] has none.
    #[must_use]
    pub fn resolve(&self) -> ResolvedTheme {
        let mut roles = std::collections::BTreeMap::new();
        for (role, raw) in &self.colors {
            let resolved = resolve_color(raw, &self.vars).unwrap_or(ColorSpec::Inherit);
            roles.insert(role.clone(), resolved);
        }
        let appearance = self.appearance.or_else(|| detect_appearance(&roles));
        ResolvedTheme { roles, appearance }
    }
}

/// The background a theme is designed for — Pi `ThemeAppearance` (`theme.ts:186`), the schema's
/// `"enum": ["dark", "light"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Dark,
    Light,
}

/// A theme colour value as written — Pi `ColorValue` (`theme-json.ts`): a string (hex, `oklch(…)`,
/// `okhsl(…)`, variable name, or `""` for the terminal default) or a 256-colour palette index.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ColorValue {
    /// A palette index, 0-255.
    Index(u8),
    /// Any string form.
    Text(String),
}

impl From<&str> for ColorValue {
    fn from(s: &str) -> Self {
        ColorValue::Text(s.to_string())
    }
}

/// Why a [`ColorValue`] did not resolve — the three `throw`s on pi's colour path, carrying pi's own
/// message text (`resolveVarRefs` `theme.ts:143-147`, `parseColor` `colors.ts:147`). Pi's `setTheme`
/// catches the throw and the controller prints it as `Failed to load theme "<name>": <message>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorError {
    /// `Circular variable reference detected: <value>`.
    Circular(String),
    /// `Variable reference not found: <value>`.
    UnknownVariable(String),
    /// `Invalid color value: <value>`.
    Invalid(String),
}

impl std::fmt::Display for ColorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColorError::Circular(v) => write!(f, "Circular variable reference detected: {v}"),
            ColorError::UnknownVariable(v) => write!(f, "Variable reference not found: {v}"),
            ColorError::Invalid(v) => write!(f, "Invalid color value: {v}"),
        }
    }
}

impl std::error::Error for ColorError {}

/// Validate a single color value against Pi's `ColorValueSchema`
/// (`Type.Union([Type.String(), Type.Integer({minimum:0,maximum:255})])`, theme.ts:23-26).
///
/// A string is valid; a number is valid only if it is a non-negative integer `<= 255`; a float, a
/// negative, or an out-of-range integer fails the union, as does any non-string/non-number value
/// (bool/object/array/null). Returns the error message for the "Other errors" section when invalid.
///
/// Whether a string actually RESOLVES — a known variable, a parseable colour — is not a schema
/// question and is decided after validation by [`resolve_color`], exactly where pi decides it
/// (`createTheme`, not the typebox schema).
fn bad_color(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(_) => None,
        serde_json::Value::Number(n) if n.as_u64().is_some_and(|u| u <= 255) => None,
        _ => Some("Expected union value".to_string()),
    }
}

/// Validate every value in an optional color record (`vars`, `export`) and push any malformed value
/// to `other` as `  - {prefix}/{key}: {message}` (theme.ts:528-531).
fn validate_color_record(prefix: &str, value: &serde_json::Value, other: &mut Vec<String>) {
    if let Some(obj) = value.as_object() {
        for (k, val) in obj {
            if let Some(msg) = bad_color(val) {
                other.push(format!("  - {prefix}/{k}: {msg}"));
            }
        }
    }
}

/// Collect the schema violations Pi reports together (theme.ts:514-548): the set of missing
/// required `colors` tokens, and the "Other errors" list of malformed color values across
/// `vars`/`colors`/`export`. Iteration order mirrors the schema declaration order (`vars` →
/// `colors` (required tokens, then extras) → `export`) so the "Other errors" lines come out in a
/// stable, Pi-like order.
fn collect_theme_errors(
    value: &serde_json::Value,
    missing: &mut Vec<String>,
    other: &mut Vec<String>,
) {
    let Some(obj) = value.as_object() else { return };

    // `appearance: Type.Optional(Type.Union([Type.Literal("dark"), Type.Literal("light")]))`
    // (`theme-schema.json:17-21`).
    if let Some(appearance) = obj.get("appearance")
        && !matches!(appearance.as_str(), Some("dark" | "light"))
    {
        other.push("  - /appearance: Expected union value".to_string());
    }

    if let Some(vars) = obj.get("vars") {
        validate_color_record("/vars", vars, other);
    }

    match obj.get("colors").and_then(|c| c.as_object()) {
        Some(colors) => {
            for token in REQUIRED_COLOR_TOKENS {
                if !colors.contains_key(token) {
                    missing.push(token.to_string());
                }
            }
            // Required tokens first (schema order), then any extra keys.
            for token in REQUIRED_COLOR_TOKENS {
                if let Some(val) = colors.get(token)
                    && let Some(msg) = bad_color(val)
                {
                    other.push(format!("  - /colors/{token}: {msg}"));
                }
            }
            for (k, val) in colors {
                if !REQUIRED_COLOR_TOKENS.contains(&k.as_str())
                    && let Some(msg) = bad_color(val)
                {
                    other.push(format!("  - /colors/{k}: {msg}"));
                }
            }
        }
        None => {
            // `colors` absent or not an object → every required token is missing.
            for token in REQUIRED_COLOR_TOKENS {
                missing.push(token.to_string());
            }
        }
    }

    if let Some(export) = obj.get("export") {
        validate_color_record("/export", export, other);
    }
}

/// Assemble Pi's combined theme-validation error message (theme.ts:533-547): the
/// "Missing required color tokens" section (sorted) followed, when present, by the "Other errors"
/// section.
fn build_theme_error(label: &str, missing: &mut Vec<String>, other: &[String]) -> String {
    let mut msg = format!("Invalid theme \"{label}\":\n");
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        let list = missing
            .iter()
            .map(|color| format!("  - {color}"))
            .collect::<Vec<_>>()
            .join("\n");
        msg.push_str("\nMissing required color tokens:\n");
        msg.push_str(&list);
        msg.push_str("\n\nPlease add these colors to your theme's \"colors\" object.");
        msg.push_str("\nSee the built-in themes (dark.json, light.json) for reference values.");
    }
    if !other.is_empty() {
        msg.push_str("\n\nOther errors:\n");
        msg.push_str(&other.join("\n"));
    }
    msg
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

/// A color role resolved through `vars` — Pi's concrete `Color` (`colors.ts:24`) plus the empty
/// string. `""` means the terminal default (`Theme.addToken`'s `\x1b[39m` / `\x1b[49m`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColorSpec {
    /// `""` — the terminal's default foreground or background.
    #[default]
    Inherit,
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
    /// A 256-colour palette index, emitted as `38;5;N` / `48;5;N` so indices 0-15 follow the user's
    /// terminal palette (Pi `IndexedColor`, `colors.ts:4`).
    Indexed(u8),
}

impl ColorSpec {
    /// The sRGB triple this colour is documented to have (`colorToRgb`, `colors.ts:218`), or `None`
    /// for the terminal default, which has no colour of its own. An index maps through the standard
    /// xterm palette — the *nominal* value; the terminal may draw 0-15 differently.
    #[must_use]
    pub fn to_rgb(self) -> Option<(u8, u8, u8)> {
        match self {
            ColorSpec::Inherit => None,
            ColorSpec::Rgb { r, g, b } => Some((r, g, b)),
            ColorSpec::Indexed(i) => Some(index_to_rgb(i)),
        }
    }
}

/// Roles resolved to concrete colors; cyrup-tui maps these to `ratatui::Color` (arch-10).
#[derive(Clone, Debug, Default)]
pub struct ResolvedTheme {
    pub roles: std::collections::BTreeMap<String, ColorSpec>,
    /// The theme's [`Theme::appearance`]: declared, else detected from its colours. `None` for a
    /// theme with no concrete colour at all (Pi then asks the terminal).
    pub appearance: Option<Appearance>,
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
    // NOTE: `thinkingMax` is deliberately NOT here. Pi declares it `Type.Optional(...)`
    // (theme.ts:93) and falls back `thinkingMax ?? thinkingXhigh` (theme.ts:329,358) so themes
    // authored before the `max` rung keep validating. Both built-ins below do define it.
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
        // Parse into a generic value first so the full `colors` schema can be validated the way
        // Pi's typebox validator does (theme.ts:514-548): collect *both* the missing required
        // tokens and the "Other errors" (malformed color values) and report them in one combined
        // message, before deserializing into the typed `ThemeData`.
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| ResourceError::Theme {
                path: path.clone().unwrap_or_default(),
                reason: e.to_string(),
            })?;

        let mut missing: Vec<String> = Vec::new();
        let mut other: Vec<String> = Vec::new();
        collect_theme_errors(&value, &mut missing, &mut other);
        if !missing.is_empty() || !other.is_empty() {
            let label = path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| {
                    value
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_string()
                });
            let reason = build_theme_error(&label, &mut missing, &other);
            return Err(ResourceError::Theme {
                path: path.unwrap_or_default(),
                reason,
            });
        }

        let data: ThemeData = serde_json::from_value(value).map_err(|e| ResourceError::Theme {
            path: path.clone().unwrap_or_default(),
            reason: e.to_string(),
        })?;
        // Pi's `createTheme` resolves every `colors` entry — `resolveVarRefs`, then `parseColor` —
        // and THROWS on the first that does not (`theme.ts:135-151`, `colors.ts:147`), which
        // `setTheme` reports as `Failed to load theme …`. A value that cannot resolve is therefore
        // a load error here too, with pi's own message, not a role that silently repaints from a
        // compiled fallback. Only `colors` is judged: an unreferenced `vars` entry is never
        // resolved upstream, and a bad `export` colour is swallowed by `getThemeExportColors`.
        for raw in data.colors.values() {
            if let Err(error) = resolve_color(raw, &data.vars) {
                return Err(ResourceError::Theme {
                    path: path.unwrap_or_default(),
                    reason: error.to_string(),
                });
            }
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
            if let Some(stem) = path
                .as_ref()
                .and_then(|p| p.file_stem())
                .and_then(|s| s.to_str())
            {
                key = ResourceKey::normalize(stem);
            }
        }
        if key.is_empty() {
            return Err(ResourceError::Theme {
                path: path.unwrap_or_default(),
                reason: "theme has no `name` and no file stem".to_string(),
            });
        }
        Ok(Theme {
            key,
            data,
            origin_path: path,
            scope,
            origin,
        })
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

    /// Resolve `colors` roles through `vars` + colour parsing. A theme that came through
    /// [`Theme::parse`] cannot fail to resolve; a hand-built [`ThemeData`] that does fail degrades
    /// that role to `Inherit` rather than panicking (R-00-009).
    pub fn resolve(&self) -> ResolvedTheme {
        self.data.resolve()
    }

    /// The background this theme is designed for: the `appearance` its document declares, else
    /// detected from the lightness of its own colours (Pi `Theme.ownAppearance`, `theme.ts:306`).
    /// `None` when it has no concrete colour to judge by — Pi then asks the terminal.
    #[must_use]
    pub fn appearance(&self) -> Option<Appearance> {
        self.resolve().appearance
    }

    /// Resolve the typed `export` section (`pageBg`/`cardBg`/`infoBg`) through `vars` for HTML
    /// export (theme.ts:94-100; arch-12). Absent keys degrade to `Inherit`.
    ///
    /// The arch-12 HTML-export consumer is not yet in tree, so this method currently has exactly
    /// one caller: `src/tests/resources/themes.rs`. Its absence elsewhere is expected — the
    /// production consumer is pending, not missing.
    pub fn resolve_export(&self) -> ExportColors {
        let get = |k: &str| {
            self.data
                .export
                .get(k)
                .and_then(|raw| resolve_color(raw, &self.data.vars).ok())
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

/// Resolve a theme colour value **recursively** through `vars` and parse it — Pi `resolveVarRefs`
/// (`theme/theme.ts:135-151` @v1.0.0) followed by `parseColor` (`colors.ts:121-148`).
///
/// A number is a palette index. The empty string is the terminal default. A string starting with
/// `#`, or an `oklch(…)` / `okhsl(…)` function, is a VALUE and is parsed as written — upstream's
/// `resolveVarRefs` short-circuits on exactly `value.startsWith("#") || /^ok(lch|hsl)\(/i` and
/// leaves the rest to `parseColor`. Any other string is a variable name, looked up in `vars` and
/// resolved the same way (an optional leading `$` is accepted for cyrup-authored themes; pi uses the
/// bare name).
///
/// Every failure is an `Err` carrying pi's message; pi throws at each of these points. Nothing is
/// trimmed, case-folded or defaulted on the way: `" #abc"`, `"abcdef"` and an unknown name are all
/// errors, as they are upstream.
///
/// # Errors
///
/// [`ColorError::Circular`], [`ColorError::UnknownVariable`] or [`ColorError::Invalid`].
pub fn resolve_color<'a>(
    value: &'a ColorValue,
    vars: &'a std::collections::BTreeMap<String, ColorValue>,
) -> Result<ColorSpec, ColorError> {
    let mut visited = std::collections::BTreeSet::new();
    let mut current = value;
    loop {
        let text = match current {
            ColorValue::Index(i) => return Ok(ColorSpec::Indexed(*i)),
            ColorValue::Text(text) => text,
        };
        if text.is_empty() {
            return Ok(ColorSpec::Inherit);
        }
        if text.starts_with('#') || crate::color::is_color_function(text) {
            return crate::color::parse_color(text)
                .map(|(r, g, b)| ColorSpec::Rgb { r, g, b })
                .ok_or_else(|| ColorError::Invalid(text.clone()));
        }
        let name = text.strip_prefix('$').unwrap_or(text);
        if !visited.insert(name) {
            return Err(ColorError::Circular(text.clone()));
        }
        current = vars
            .get(name)
            .ok_or_else(|| ColorError::UnknownVariable(text.clone()))?;
    }
}

/// `averageLightness` (`theme.ts:221-226`): the mean OKLab lightness of the concrete colours,
/// ignoring palette indices 0-15 — those follow the user's terminal palette, so they say nothing
/// about the theme.
fn average_lightness(colors: &[ColorSpec]) -> Option<f64> {
    let lightness: Vec<f64> = colors
        .iter()
        .filter_map(|spec| match spec {
            ColorSpec::Inherit => None,
            ColorSpec::Indexed(i) if *i < 16 => None,
            other => other.to_rgb(),
        })
        .map(|rgb| crate::color::rgb_to_oklch(rgb).0)
        .collect();
    if lightness.is_empty() {
        return None;
    }
    Some(lightness.iter().sum::<f64>() / lightness.len() as f64)
}

/// The tokens pi draws as backgrounds (`BACKGROUND_TOKENS`, `theme.ts:573-581`); every other token
/// is a foreground.
const BACKGROUND_TOKENS: [&str; 7] = [
    "selectedBg",
    "searchMatchBg",
    "userMessageBg",
    "customMessageBg",
    "toolPendingBg",
    "toolSuccessBg",
    "toolErrorBg",
];

/// Pi's `withThemeColorFallbacks` (`theme.ts:164-179`): `(optional token, token it falls back to)`.
const OPTIONAL_TOKEN_FALLBACKS: [(&str, &str); 5] = [
    ("scrollbarTrack", "muted"),
    ("scrollbarThumb", "text"),
    ("thinkingMax", "thinkingXhigh"),
    ("searchMatchBg", "selectedBg"),
    ("searchMatchText", "text"),
];

/// Pi `detectAppearance(foregrounds, backgrounds)` (`theme.ts:228-236`) over a resolved role map:
/// background tokens against foreground tokens, with the optional tokens' fallbacks applied first
/// (they are part of the colour set pi averages, `theme.ts:164-179`, `:306`).
///
/// Both present: dark when the backgrounds are darker than the foregrounds. Backgrounds alone: dark
/// below 0.5. Foregrounds alone: dark above 0.5. Neither: undecided.
fn detect_appearance(roles: &std::collections::BTreeMap<String, ColorSpec>) -> Option<Appearance> {
    let mut all = roles.clone();
    for (optional, base) in OPTIONAL_TOKEN_FALLBACKS {
        if !all.contains_key(optional)
            && let Some(spec) = all.get(base).copied()
        {
            all.insert(optional.to_string(), spec);
        }
    }
    let (backgrounds, foregrounds): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|(token, _)| BACKGROUND_TOKENS.contains(&token.as_str()));
    let backgrounds: Vec<ColorSpec> = backgrounds.into_iter().map(|(_, spec)| spec).collect();
    let foregrounds: Vec<ColorSpec> = foregrounds.into_iter().map(|(_, spec)| spec).collect();
    match (
        average_lightness(&foregrounds),
        average_lightness(&backgrounds),
    ) {
        (Some(fg), Some(bg)) => Some(if bg < fg {
            Appearance::Dark
        } else {
            Appearance::Light
        }),
        (None, Some(bg)) => Some(if bg < 0.5 {
            Appearance::Dark
        } else {
            Appearance::Light
        }),
        (Some(fg), None) => Some(if fg > 0.5 {
            Appearance::Dark
        } else {
            Appearance::Light
        }),
        (None, None) => None,
    }
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

/// The compiled-in `dark` theme (R-09-011): pi's `theme/dark.json` @v1.0.0, byte for byte.
pub const BUILTIN_DARK_JSON: &str = include_str!("builtin_themes/dark.json");

/// The compiled-in `light` theme (R-09-011): pi's `theme/light.json` @v1.0.0, byte for byte.
pub const BUILTIN_LIGHT_JSON: &str = include_str!("builtin_themes/light.json");

/// The two compiled-in built-ins (`dark`, `light`) at [`ResourceScope::Builtin`].
pub fn builtin_themes() -> Vec<Theme> {
    let mut out = Vec::new();
    for json in [BUILTIN_DARK_JSON, BUILTIN_LIGHT_JSON] {
        if let Ok(t) = Theme::parse(json, None, ResourceScope::Builtin, ResourceOrigin::Builtin) {
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
        .map_err(|e| ResourceError::Theme {
            path: path.clone(),
            reason: e.to_string(),
        })?;

        // Watch the file directly; the poll watcher detects content/mtime changes.
        watcher
            .watch(&path, notify::RecursiveMode::NonRecursive)
            .map_err(|e| ResourceError::Theme {
                path: path.clone(),
                reason: e.to_string(),
            })?;

        let inner = Arc::new(std::sync::Mutex::new(WatcherInner { watcher, path, tx }));
        let task_inner = Arc::clone(&inner);

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    // `biased;` — the poll watcher fires every 50 ms, so at teardown the
                    // cancellation and a pending file event are routinely BOTH ready, and an
                    // unbiased `select!` picks between two ready arms at RANDOM. That let a
                    // cancelled watcher run one more `reload` and publish a theme onto `tx` after
                    // dispose — nondeterministically, which is why it never showed up as a failing
                    // test. There is no JS counterpart to the race: upstream's watcher callback is
                    // a plain listener removed by `close()`, and a listener cannot be invoked
                    // "concurrently with" its own removal on one event loop.
                    biased;
                    _ = cancel.cancelled() => break,
                    msg = evt_rx.recv() => {
                        if msg.is_none() { break; }
                        reload(&task_inner);
                    }
                }
            }
        });

        Ok(ThemeWatcher {
            rx,
            inner,
            _task: task,
        })
    }

    /// A fresh receiver for the active-theme channel.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Arc<ThemeData>> {
        self.rx.clone()
    }

    /// Switch the watched file at runtime (R-09-014). Immediately publishes the new file's theme.
    pub fn retarget(&self, path: PathBuf) -> Result<(), ResourceError> {
        use notify::Watcher;
        let mut guard = self.inner.lock().map_err(|_| ResourceError::Theme {
            path: path.clone(),
            reason: "lock".into(),
        })?;
        let old = guard.path.clone();
        let _ = guard.watcher.unwatch(&old);
        guard
            .watcher
            .watch(&path, notify::RecursiveMode::NonRecursive)
            .map_err(|e| ResourceError::Theme {
                path: path.clone(),
                reason: e.to_string(),
            })?;
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

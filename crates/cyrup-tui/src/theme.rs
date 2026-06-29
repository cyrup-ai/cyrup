//! Theme application (R-10-025/026/027; arch-10 §3.8).
//!
//! `cyrup-resources` owns themes on disk (parsing, `vars`, hot-reload via `ThemeWatcher`). This
//! module is the *render-facing* projection: it maps the resolved color roles
//! (`cyrup_resources::theme::ResolvedTheme` / `ColorSpec`) onto `ratatui::style::Color` and exposes
//! the per-component `Style`s the widgets read. A `generation` counter is bumped on every
//! hot-reload so render caches can be invalidated (R-10-026).

use cyrup_resources::theme::{builtin_themes, ColorSpec, ResolvedTheme, ThemeData};
use ratatui::style::{Color, Modifier, Style};

/// The render-facing theme. Cheap to clone (a handful of optional colors + a name).
#[derive(Clone, Debug)]
pub struct UiTheme {
    /// `"dark"` | `"light"` | a custom theme name (R-10-027).
    pub name: String,
    /// Bumped on hot-reload so caches can invalidate (R-10-026 / arch-10 §3.4).
    pub generation: u64,
    /// Pi `text` token — default foreground text (theme.ts:45). `None` ⇒ inherit terminal default.
    pub foreground: Option<Color>,
    /// Background. Pi has **no** global background token — backgrounds are per-component
    /// (`selectedBg` / `userMessageBg` / `toolPendingBg` / …, theme.ts:48-55), to be wired as the
    /// TUI grows. So this stays `None` (terminal default) for the built-ins.
    pub background: Option<Color>,
    /// Pi `accent` token — focus / assistant emphasis (theme.ts:36).
    pub accent: Option<Color>,
    /// Pi `error` token — errors and failed tool calls (theme.ts:41).
    pub error: Option<Color>,
}

impl Default for UiTheme {
    fn default() -> Self {
        UiTheme::dark()
    }
}

impl UiTheme {
    /// Project a `ResolvedTheme` (color roles already resolved through `vars`) into a `UiTheme`.
    pub fn from_resolved(name: impl Into<String>, resolved: &ResolvedTheme, generation: u64) -> Self {
        let role = |key: &str| resolved.roles.get(key).copied().and_then(color_of);
        UiTheme {
            name: name.into(),
            generation,
            foreground: role("text"),
            // Pi has no global background token; per-component backgrounds are wired separately.
            background: None,
            accent: role("accent"),
            error: role("error"),
        }
    }

    /// A compiled-in built-in (`"dark"` / `"light"`); falls back to the dark palette if the name is
    /// unknown so this is total and never panics (R-00-009, R-10-027).
    pub fn builtin(name: &str) -> Self {
        for theme in builtin_themes() {
            if theme.key.as_str() == name {
                let resolved = theme.resolve();
                return UiTheme::from_resolved(theme.data.name.clone(), &resolved, 0);
            }
        }
        UiTheme::dark()
    }

    /// The compiled-in `dark` theme (Pi `dark.json`: text `#d4d4d4`, accent `#8abeb7`, error `#cc6666`).
    pub fn dark() -> Self {
        UiTheme::builtin_or_static(
            "dark",
            Color::Rgb(0xd4, 0xd4, 0xd4),
            Color::Rgb(0x8a, 0xbe, 0xb7),
            Color::Rgb(0xcc, 0x66, 0x66),
        )
    }

    /// The compiled-in `light` theme (Pi `light.json`: text `#1f2328`, accent `#5a8080`, error `#aa5555`).
    pub fn light() -> Self {
        UiTheme::builtin_or_static(
            "light",
            Color::Rgb(0x1f, 0x23, 0x28),
            Color::Rgb(0x5a, 0x80, 0x80),
            Color::Rgb(0xaa, 0x55, 0x55),
        )
    }

    /// Look up a built-in by name, or synthesize a minimal palette from the given Pi `text`/`accent`/
    /// `error` colors if the resource layer somehow cannot supply it (keeps zero-disk-I/O
    /// availability, R-10-027). Background stays terminal-default (Pi has no global background token).
    fn builtin_or_static(name: &str, text: Color, accent: Color, error: Color) -> Self {
        for theme in builtin_themes() {
            if theme.key.as_str() == name {
                let resolved = theme.resolve();
                return UiTheme::from_resolved(theme.data.name.clone(), &resolved, 0);
            }
        }
        UiTheme {
            name: name.to_string(),
            generation: 0,
            foreground: Some(text),
            background: None,
            accent: Some(accent),
            error: Some(error),
        }
    }

    /// Project freshly-watched [`ThemeData`] (e.g. from `cyrup_resources::theme::ThemeWatcher`)
    /// into a `UiTheme` for hot-reload (R-10-026). Resolves the role through `vars` + hex parsing,
    /// mirroring `Theme::resolve`, so the watcher's `Arc<ThemeData>` can be applied without first
    /// reconstructing a `Theme`. Bad/empty values inherit the terminal default (no panic).
    pub fn from_theme_data(data: &ThemeData, generation: u64) -> Self {
        let role = |key: &str| data.colors.get(key).and_then(|raw| resolve_value(raw, &data.vars));
        UiTheme {
            name: data.name.clone(),
            generation,
            foreground: role("text"),
            // Pi has no global background token; per-component backgrounds are wired separately.
            background: None,
            accent: role("accent"),
            error: role("error"),
        }
    }

    /// Bump the generation (caches keyed by generation re-render). Used by the hot-reload hook.
    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    // --- component styles -------------------------------------------------------------------

    /// Base text style (foreground/background roles).
    pub fn base_style(&self) -> Style {
        let mut s = Style::default();
        if let Some(fg) = self.foreground {
            s = s.fg(fg);
        }
        if let Some(bg) = self.background {
            s = s.bg(bg);
        }
        s
    }

    /// Accent style (assistant text, focus, emphasis).
    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent.unwrap_or(Color::Cyan))
    }

    /// Error style (failed tools, error notifications).
    pub fn error_style(&self) -> Style {
        Style::default().fg(self.error.unwrap_or(Color::Red)).add_modifier(Modifier::BOLD)
    }

    /// Dimmed style for secondary/tool chrome.
    pub fn dim_style(&self) -> Style {
        let mut s = Style::default().add_modifier(Modifier::DIM);
        if let Some(fg) = self.foreground {
            s = s.fg(fg);
        }
        s
    }

    /// Style for the user's own messages (bold accent label).
    pub fn user_style(&self) -> Style {
        Style::default().fg(self.accent.unwrap_or(Color::Cyan)).add_modifier(Modifier::BOLD)
    }

    /// Style for assistant message text.
    pub fn assistant_style(&self) -> Style {
        self.base_style()
    }
}

/// Map a resolved color role onto a `ratatui::Color`. `Inherit` ⇒ `None` (terminal default).
pub fn color_of(spec: ColorSpec) -> Option<Color> {
    match spec {
        ColorSpec::Inherit => None,
        ColorSpec::Rgb { r, g, b } => Some(Color::Rgb(r, g, b)),
    }
}

/// Resolve a raw `colors` value through `vars` then hex-parse it (dependency-free mirror of
/// `cyrup_resources::theme`'s private resolver, for the [`UiTheme::from_theme_data`] hot-reload hook).
fn resolve_value(raw: &str, vars: &std::collections::BTreeMap<String, String>) -> Option<Color> {
    let v = raw.trim();
    if v.is_empty() {
        return None;
    }
    let var_name = v.strip_prefix('$').unwrap_or(v);
    let hex = vars.get(var_name).map(String::as_str).unwrap_or(v);
    parse_hex(hex)
}

/// Parse `#rrggbb` / `rrggbb` / `#rgb` into a `Color`; anything malformed ⇒ `None`.
fn parse_hex(s: &str) -> Option<Color> {
    let h = s.trim().strip_prefix('#').unwrap_or(s.trim());
    let bytes = h.as_bytes();
    match bytes.len() {
        6 => {
            let r = hex_pair(bytes.first(), bytes.get(1))?;
            let g = hex_pair(bytes.get(2), bytes.get(3))?;
            let b = hex_pair(bytes.get(4), bytes.get(5))?;
            Some(Color::Rgb(r, g, b))
        }
        3 => {
            let r = hex_digit(bytes.first()).map(|n| n * 17)?;
            let g = hex_digit(bytes.get(1)).map(|n| n * 17)?;
            let b = hex_digit(bytes.get(2)).map(|n| n * 17)?;
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

fn hex_digit(b: Option<&u8>) -> Option<u8> {
    match *b? {
        c @ b'0'..=b'9' => Some(c - b'0'),
        c @ b'a'..=b'f' => Some(c - b'a' + 10),
        c @ b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex_pair(hi: Option<&u8>, lo: Option<&u8>) -> Option<u8> {
    let h = hex_digit(hi)?;
    let l = hex_digit(lo)?;
    h.checked_mul(16)?.checked_add(l)
}

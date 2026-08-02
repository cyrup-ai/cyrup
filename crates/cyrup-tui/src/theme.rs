//! Theme application (R-10-025/026/027; arch-10 §3.8).
//!
//! `cyrup-resources` owns themes on disk (parsing, `vars`, hot-reload via `ThemeWatcher`). This
//! module is the *render-facing* projection: it maps the resolved color roles
//! (`cyrup_resources::theme::ResolvedTheme` / `ColorSpec`) onto `ratatui::style::Color` and exposes
//! the per-component `Style`s the widgets read. A `generation` counter is bumped on every
//! hot-reload so render caches can be invalidated (R-10-026).

use cyrup_resources::theme::{builtin_themes, ColorSpec, ResolvedTheme, ThemeData};
use ratatui::style::{Color, Modifier, Style};

/// The terminal color-depth the [`UiTheme`] projects its RGB roles into (Pi `ColorMode`,
/// `theme.ts:162` + the capability probe `theme.ts:588`). Pi carries only `truecolor`/`256color`;
/// cyrup extends the enum with `Ansi16` and `None` for depth-limited/monochrome terminals so the
/// projection is total. The mode is chosen once at boot from the terminal capabilities (`COLORTERM`)
/// and re-applied whenever the theme changes (`ThemeController`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// 24-bit direct color — RGB roles pass through as `Color::Rgb` (Pi `"truecolor"`).
    #[default]
    TrueColor,
    /// 256-color indexed — RGB roles are quantized to the xterm 6×6×6 cube + grayscale ramp via
    /// [`rgb_to_256`] and emitted as `Color::Indexed` (Pi `"256color"`, `hexTo256`/`fgAnsi`).
    Ansi256,
    /// 16-color — RGB roles collapse to the nearest of the 16 ANSI names (depth-limited terminals).
    Ansi16,
    /// Monochrome — every color role is dropped (`Color::Reset`, terminal default).
    None,
}

impl ColorMode {
    /// Pick the color mode from the environment the way Pi's capability probe does (`getCapabilities`,
    /// `theme.ts:588`): a `COLORTERM` of `truecolor`/`24bit` ⇒ [`ColorMode::TrueColor`]; a `TERM`
    /// mentioning `256color` ⇒ [`ColorMode::Ansi256`]; a dumb/no-color terminal ⇒ [`ColorMode::None`];
    /// otherwise the safe [`ColorMode::Ansi256`] default (matching Pi's `256color` fallback).
    pub fn detect() -> ColorMode {
        let colorterm = std::env::var("COLORTERM").unwrap_or_default().to_ascii_lowercase();
        if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            return ColorMode::TrueColor;
        }
        let term = std::env::var("TERM").unwrap_or_default().to_ascii_lowercase();
        if term == "dumb" || term.is_empty() {
            return ColorMode::None;
        }
        if term.contains("256color") {
            return ColorMode::Ansi256;
        }
        // Pi's `createTheme` falls back to `256color` when truecolor is unavailable (`theme.ts:588`).
        ColorMode::Ansi256
    }

    /// Project one `ratatui::Color` into this mode. Only `Color::Rgb` is transformed (named/indexed
    /// colors are already depth-safe); the transform is the single **style-projection boundary** the
    /// whole TUI passes its role colors through (mirrors Pi `fgAnsi`/`bgAnsi`, `theme.ts:260-288`).
    pub fn project(self, color: Color) -> Color {
        let Color::Rgb(r, g, b) = color else { return color };
        match self {
            ColorMode::TrueColor => color,
            ColorMode::Ansi256 => Color::Indexed(rgb_to_256(r, g, b)),
            ColorMode::Ansi16 => Color::Indexed(rgb_to_16(r, g, b)),
            ColorMode::None => Color::Reset,
        }
    }

    /// Project an optional role color (helper for the [`UiTheme`] fields).
    fn project_opt(self, color: Option<Color>) -> Option<Color> {
        match color {
            Some(c) => match self.project(c) {
                Color::Reset if self == ColorMode::None => None,
                projected => Some(projected),
            },
            None => None,
        }
    }
}

/// The render-facing theme. Cheap to clone (a handful of optional colors + a name).
#[derive(Clone, Debug)]
pub struct UiTheme {
    /// `"dark"` | `"light"` | a custom theme name (R-10-027).
    pub name: String,
    /// Bumped on hot-reload so caches can invalidate (R-10-026 / arch-10 §3.4).
    pub generation: u64,
    /// The terminal color depth this theme's roles were projected into (Pi `Theme.mode`). Set at boot
    /// from [`ColorMode::detect`] / the `ThemeController`, applied via [`UiTheme::with_color_mode`].
    pub color_mode: ColorMode,
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
    /// Pi `muted` token — secondary text (descriptions, scroll indicators, hints) (theme.ts:543).
    pub muted: Option<Color>,
    /// Pi `border` token — rule/border lines for the editor + selectors (theme.ts:537).
    pub border: Option<Color>,
    /// Pi `success` token — current/active markers (`✓`), succeeded states (theme.ts:540).
    pub success: Option<Color>,
    /// Pi `warning` token — context-% warning band, `(cancelled)`, experimental marker (theme.ts:542).
    pub warning: Option<Color>,
    /// Pi `bashMode` token — green editor border + `$ cmd` header while in bash mode (theme.ts).
    pub bash_mode: Option<Color>,
    /// Every resolved color role keyed by Pi token name (`syntaxComment`, `mdHeading`, `toolDiffAdded`,
    /// …). Populated from `ResolvedTheme`/`ThemeData` so the rich-rendering layer (markdown + syntax,
    /// spec/tui/06 §11) can resolve the full ~51-token role set (`REQUIRED_COLOR_TOKENS`) without a
    /// field per role. Empty for the synthetic static fallback (the role helpers then use defaults).
    pub roles: std::collections::BTreeMap<String, Color>,
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
        let roles = resolved
            .roles
            .iter()
            .filter_map(|(k, spec)| color_of(*spec).map(|c| (k.clone(), c)))
            .collect();
        UiTheme {
            name: name.into(),
            generation,
            color_mode: ColorMode::default(),
            foreground: role("text"),
            // Pi has no global background token; per-component backgrounds are wired separately.
            background: None,
            accent: role("accent"),
            error: role("error"),
            muted: role("muted"),
            border: role("border"),
            success: role("success"),
            warning: role("warning"),
            bash_mode: role("bashMode"),
            roles,
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
            color_mode: ColorMode::default(),
            foreground: Some(text),
            background: None,
            accent: Some(accent),
            error: Some(error),
            muted: None,
            border: None,
            success: None,
            warning: None,
            bash_mode: None,
            roles: std::collections::BTreeMap::new(),
        }
    }

    /// Project freshly-watched [`ThemeData`] (e.g. from `cyrup_resources::theme::ThemeWatcher`)
    /// into a `UiTheme` for hot-reload (R-10-026). Resolves the role through `vars` + hex parsing,
    /// mirroring `Theme::resolve`, so the watcher's `Arc<ThemeData>` can be applied without first
    /// reconstructing a `Theme`. Bad/empty values inherit the terminal default (no panic).
    pub fn from_theme_data(data: &ThemeData, generation: u64) -> Self {
        let role = |key: &str| data.colors.get(key).and_then(|raw| resolve_value(raw, &data.vars));
        let roles = data
            .colors
            .keys()
            .filter_map(|k| role(k).map(|c| (k.clone(), c)))
            .collect();
        UiTheme {
            name: data.name.clone(),
            generation,
            color_mode: ColorMode::default(),
            foreground: role("text"),
            // Pi has no global background token; per-component backgrounds are wired separately.
            background: None,
            accent: role("accent"),
            error: role("error"),
            muted: role("muted"),
            border: role("border"),
            success: role("success"),
            warning: role("warning"),
            bash_mode: role("bashMode"),
            roles,
        }
    }

    /// Bump the generation (caches keyed by generation re-render). Used by the hot-reload hook.
    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    /// Project every RGB role color into `mode` (the **style-projection boundary**, feature #3): on a
    /// 256-color terminal each `Color::Rgb` role is quantized to a `Color::Indexed` cube/grayscale
    /// index so the backend never emits a truecolor escape a 256-color terminal would mangle (Pi
    /// `createTheme` binds every color through `fgAnsi(value, mode)` at build time, `theme.ts:342-348`).
    /// The transform is idempotent for non-RGB colors, so re-applying a mode is safe. Every downstream
    /// `*_style` accessor reads the already-projected fields, so no per-widget change is needed.
    pub fn with_color_mode(mut self, mode: ColorMode) -> Self {
        self.color_mode = mode;
        self.foreground = mode.project_opt(self.foreground);
        self.background = mode.project_opt(self.background);
        self.accent = mode.project_opt(self.accent);
        self.error = mode.project_opt(self.error);
        self.muted = mode.project_opt(self.muted);
        self.border = mode.project_opt(self.border);
        self.success = mode.project_opt(self.success);
        self.warning = mode.project_opt(self.warning);
        self.bash_mode = mode.project_opt(self.bash_mode);
        // The full role map (syntax/markdown/thinking/bg tints) is projected too, so every role
        // resolved via `role_color`/`roles.get` is depth-safe.
        self.roles = self
            .roles
            .iter()
            .filter_map(|(k, &c)| mode.project_opt(Some(c)).map(|p| (k.clone(), p)))
            .collect();
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

    /// Muted style (descriptions, scroll indicators, hints, footer body) — Pi `muted` (theme.ts:543).
    /// Falls back to a dimmed foreground when the theme omits the role.
    pub fn muted_style(&self) -> Style {
        match self.muted {
            Some(c) => Style::default().fg(c),
            None => self.dim_style(),
        }
    }

    /// Border/rule style for the editor + selector `DynamicBorder` rules — Pi `border` (theme.ts:537).
    pub fn border_style(&self) -> Style {
        Style::default().fg(self.border.or(self.muted).unwrap_or(Color::DarkGray))
    }

    /// Success style (current/active `✓` markers, succeeded states) — Pi `success` (theme.ts:540).
    pub fn success_style(&self) -> Style {
        Style::default().fg(self.success.unwrap_or(Color::Green))
    }

    /// Warning style (context-% band, `(cancelled)`, experimental marker) — Pi `warning` (theme.ts:542).
    pub fn warning_style(&self) -> Style {
        Style::default().fg(self.warning.unwrap_or(Color::Yellow))
    }

    /// Bash-mode style (green editor border + `$ cmd` header) — Pi `bashMode` (theme.ts).
    pub fn bash_mode_style(&self) -> Style {
        Style::default().fg(self.bash_mode.or(self.success).unwrap_or(Color::Green))
    }

    /// The editor's top/bottom rule style for a reasoning `level` — Pi `thinking{Off..Xhigh}`
    /// (`interactive-mode.ts:3533-3541`, spec/tui/03 §3.3): an escalating per-level color that is the
    /// editor's primary always-visible mode signal. Falls back to the `border` role for unknown levels.
    pub fn thinking_border_style(&self, level: &str) -> Style {
        let thinking = self.thinking();
        let color = match level {
            "off" => thinking.off,
            "minimal" => thinking.minimal,
            "low" => thinking.low,
            "medium" => thinking.medium,
            "high" => thinking.high,
            "xhigh" => thinking.xhigh,
            // An unrecognized level keeps the neutral border color.
            _ => return self.border_style(),
        };
        Style::default().fg(color)
    }

    // --- structured sub-themes (feature #3) -----------------------------------------------------
    //
    // The audit's root cause of the remaining bg/thinking-border misses was a *flat* role map: every
    // background/thinking-border color was reachable only by an ad-hoc `roles.get("…")` string lookup,
    // so the fields were not addressable and easy to miss. These sub-theme structs project the flat map
    // into typed, exhaustive fields — every background role and every thinking-border level is now a
    // named field (`ThemeData` mirrors Pi's structured `Theme.colors`, theme.ts:34-93). The component
    // accessors below delegate to them, so there is one structured source of truth.

    /// The structured per-role **background** sub-theme (Pi's background tokens, theme.ts:48-55). Every
    /// message/tool/selected background is a named `Option<Color>` field (`None` ⇒ terminal default).
    pub fn backgrounds(&self) -> BackgroundTheme {
        let g = |k: &str| self.roles.get(k).copied();
        BackgroundTheme {
            selected: g("selectedBg"),
            user_message: g("userMessageBg"),
            custom_message: g("customMessageBg"),
            tool_pending: g("toolPendingBg"),
            tool_success: g("toolSuccessBg"),
            tool_error: g("toolErrorBg"),
        }
    }

    /// The structured **thinking-border** sub-theme (Pi `thinking{Off..Xhigh}`, interactive-mode.ts:
    /// 3533-3541): the escalating per-reasoning-level editor rule color, one typed field per level,
    /// each resolved from the live theme with the spec/tui/03 §3.3 dark-hex fallback so it is total.
    pub fn thinking(&self) -> ThinkingTheme {
        let level = |key: &str, default_hex: &str| self.role_color(key, default_hex);
        ThinkingTheme {
            off: level("thinkingOff", "#666666"),
            minimal: level("thinkingMinimal", "#6e6e6e"),
            low: level("thinkingLow", "#5f87af"),
            medium: level("thinkingMedium", "#81a2be"),
            high: level("thinkingHigh", "#b294bb"),
            xhigh: level("thinkingXhigh", "#d183e8"),
        }
    }

    // --- per-role background fills (spec/tui/02 §9.2; the affordance is the bg, not a box) ----------
    //
    // Pi has no global background token; message/tool/selected rows are tinted by per-role bg fills
    // (`selectedBg` / `userMessageBg` / `toolPendingBg|SuccessBg|ErrorBg` / `customMessageBg`,
    // theme.ts:48-55). These were dead (every `.bg()` hardwired to `None`, audit #6); projecting the
    // resolved roles restores the message-role + selected-row affordance.

    /// The resolved color for a background role key, if the live theme defines it (delegates to the
    /// structured [`BackgroundTheme`] so the flat lookup is not duplicated).
    fn bg_role(&self, key: &str) -> Option<Color> {
        let bg = self.backgrounds();
        match key {
            "selectedBg" => bg.selected,
            "userMessageBg" => bg.user_message,
            "customMessageBg" => bg.custom_message,
            "toolPendingBg" => bg.tool_pending,
            "toolSuccessBg" => bg.tool_success,
            "toolErrorBg" => bg.tool_error,
            _ => self.roles.get(key).copied(),
        }
    }

    /// Apply a background role onto `style` when the theme defines it (else leave `style` unchanged so
    /// the terminal default shows through — never a hardcoded fill).
    fn with_bg(&self, style: Style, key: &str) -> Style {
        match self.bg_role(key) {
            Some(bg) => style.bg(bg),
            None => style,
        }
    }

    /// Selected-row fill in selectors (`selectedBg`, select-list.ts:160-162).
    pub fn selected_bg_style(&self) -> Style {
        self.with_bg(self.accent_style(), "selectedBg")
    }

    /// User-message block fill (`userMessageBg`, user-message rendering).
    pub fn user_message_bg_style(&self) -> Style {
        self.with_bg(self.base_style(), "userMessageBg")
    }

    /// Custom/notice block fill (`customMessageBg`).
    pub fn custom_message_bg_style(&self) -> Style {
        self.with_bg(self.dim_style(), "customMessageBg")
    }

    /// Tool-call title (the `read`/`edit`/`$`/`grep …` headers) — Pi `toolTitle` (= `text`, the base
    /// foreground) rendered bold (`theme.fg("toolTitle", theme.bold(...))`, dark.json:44).
    pub fn tool_title_style(&self) -> Style {
        let mut s = Style::default().add_modifier(Modifier::BOLD);
        if let Some(fg) = self.foreground {
            s = s.fg(fg);
        }
        s
    }

    /// Tool output body — Pi `toolOutput` (= `gray`/`mediumGray`, dark.json:45). Prefers an explicit
    /// `toolOutput` role, else falls back to the muted (gray) role.
    pub fn tool_output_style(&self) -> Style {
        match self.roles.get("toolOutput").copied() {
            Some(c) => Style::default().fg(c),
            None => self.muted_style(),
        }
    }

    /// Tool-execution block fill keyed by state (`toolPendingBg`/`toolSuccessBg`/`toolErrorBg`,
    /// tool-execution.ts:253-258, spec/tui/06 §5.1). `base` carries the foreground role; the bg is the
    /// state tint when the theme defines it.
    pub fn tool_bg_style(&self, base: Style, done: bool, is_error: bool) -> Style {
        let key = if is_error {
            "toolErrorBg"
        } else if done {
            "toolSuccessBg"
        } else {
            "toolPendingBg"
        };
        self.with_bg(base, key)
    }

    // --- rich-rendering roles (spec/tui/06 §11) -------------------------------------------------

    /// Resolve a Pi color-token role by name (`syntaxKeyword`, `mdHeading`, …), falling back to the
    /// given hex default (the `dark.json` value from spec/tui/06 §3.2) when the live theme omits it —
    /// so the markdown/syntax layer is total even under the synthetic static fallback theme.
    pub fn role_color(&self, key: &str, default_hex: &str) -> Color {
        // Stored roles are already projected by `with_color_mode`; a hex fallback for a role the
        // theme omits is projected here so a 256-color terminal never gets a stray truecolor escape.
        match self.roles.get(key).copied() {
            Some(c) => c,
            None => self
                .color_mode
                .project_opt(parse_hex(default_hex))
                .unwrap_or(Color::Reset),
        }
    }

    /// `fg`-only style for a role with a hex default (spec/tui/06 §3.2 dark hexes).
    fn role_style(&self, key: &str, default_hex: &str) -> Style {
        Style::default().fg(self.role_color(key, default_hex))
    }

    /// Markdown heading — `mdHeading`, bold (`markdown.ts:336-362`).
    pub fn md_heading_style(&self) -> Style {
        self.role_style("mdHeading", "#f0c674").add_modifier(Modifier::BOLD)
    }
    /// Inline code span — `mdCode` (= accent), no backticks (`markdown.ts:512-516`).
    pub fn md_code_style(&self) -> Style {
        Style::default().fg(self.roles.get("mdCode").copied().or(self.accent).unwrap_or(Color::Cyan))
    }
    /// Flat (unknown-language) fenced-code body — `mdCodeBlock` (`markdown.ts:378-398`).
    pub fn md_code_block_style(&self) -> Style {
        self.role_style("mdCodeBlock", "#b5bd68")
    }
    /// Fence border lines (```` ``` ````) — `mdCodeBlockBorder` (`markdown.ts:380,393`).
    pub fn md_code_block_border_style(&self) -> Style {
        self.role_style("mdCodeBlockBorder", "#666666")
    }
    /// Assistant **reasoning** (thinking) body — `thinkingText`, italic (Pi
    /// `assistant-message.ts:145-165` renders each run of `thinking` blocks as one Markdown section
    /// with `{color: theme.fg("thinkingText", …), italic: true}`; the collapsed
    /// `hideThinkingBlock` label at `:139-143` uses the same role). `thinkingText` is `gray`
    /// (`#808080`) in Pi's `dark.json:33` and `mediumGray` in `light.json:32`; the hex default here
    /// is the dark one, used only when the live theme omits the role.
    ///
    /// NOTE this is a different thing from [`ThinkingTheme`], which is the per-reasoning-**level**
    /// editor-border palette (`thinkingOff`…`thinkingXhigh`).
    pub fn thinking_text_style(&self) -> Style {
        self.role_style("thinkingText", "#808080").add_modifier(Modifier::ITALIC)
    }
    /// Blockquote body — `mdQuote`, italic (`markdown.ts:414-461`).
    pub fn md_quote_style(&self) -> Style {
        self.role_style("mdQuote", "#969896").add_modifier(Modifier::ITALIC)
    }
    /// Blockquote `│ ` border — `mdQuoteBorder` (`markdown.ts:414-461`).
    pub fn md_quote_border_style(&self) -> Style {
        self.role_style("mdQuoteBorder", "#666666")
    }
    /// Horizontal rule — `mdHr` (`markdown.ts:463-468`).
    pub fn md_hr_style(&self) -> Style {
        self.role_style("mdHr", "#666666")
    }
    /// List bullet marker — `mdListBullet` (= accent) (`markdown.ts:604-654`).
    pub fn md_list_bullet_style(&self) -> Style {
        Style::default()
            .fg(self.roles.get("mdListBullet").copied().or(self.accent).unwrap_or(Color::Cyan))
    }
    /// Link text — `mdLink`, underlined (`markdown.ts:537-556`).
    pub fn md_link_style(&self) -> Style {
        self.role_style("mdLink", "#81a2be").add_modifier(Modifier::UNDERLINED)
    }
    /// Trailing `(url)` — `mdLinkUrl`, dim (`markdown.ts:548-556`).
    pub fn md_link_url_style(&self) -> Style {
        self.role_style("mdLinkUrl", "#5f819d").add_modifier(Modifier::DIM)
    }

    /// Diff added (`+`) line — `toolDiffAdded`, green (`diff.ts` `theme.fg("toolDiffAdded")`).
    pub fn tool_diff_added_style(&self) -> Style {
        Style::default().fg(self.role_color("toolDiffAdded", "#b5bd68"))
    }
    /// Diff removed (`-`) line — `toolDiffRemoved`, red.
    pub fn tool_diff_removed_style(&self) -> Style {
        Style::default().fg(self.role_color("toolDiffRemoved", "#cc6666"))
    }
    /// Diff context (unchanged) line — `toolDiffContext`, gray.
    pub fn tool_diff_context_style(&self) -> Style {
        Style::default().fg(self.role_color("toolDiffContext", "#808080"))
    }
    /// Intra-line changed-token emphasis — reversed video (`theme.inverse`, `diff.ts:renderIntraLineDiff`).
    pub fn inverse_style(&self) -> Style {
        Style::default().add_modifier(Modifier::REVERSED)
    }

    /// Resolve a `syntect` scope (top-of-stack) to a syntax-highlight style by the prefix table in
    /// spec/tui/06 §3.2 (`theme.ts:1083-1113`). Unknown scopes return `None` so the caller renders the
    /// run flat in `mdCodeBlock` (auto-detect-off parity, §3.1).
    pub fn syntax_style_for_scope(&self, scope: &str) -> Option<Style> {
        // Most-specific prefixes first; the first match wins.
        let (role, default_hex, modifier) = if scope.starts_with("comment") {
            ("syntaxComment", "#6A9955", None)
        } else if scope.starts_with("string") {
            ("syntaxString", "#CE9178", None)
        } else if scope.starts_with("constant.numeric") {
            ("syntaxNumber", "#B5CEA8", None)
        } else if scope.starts_with("entity.name.function") || scope.starts_with("support.function") {
            ("syntaxFunction", "#DCDCAA", None)
        } else if scope.starts_with("entity.name.type")
            || scope.starts_with("support.type")
            || scope.starts_with("support.class")
            || scope.starts_with("entity.name.class")
        {
            ("syntaxType", "#4EC9B0", None)
        } else if scope.starts_with("keyword.operator") {
            ("syntaxOperator", "#D4D4D4", None)
        } else if scope.starts_with("keyword") || scope.starts_with("storage") {
            ("syntaxKeyword", "#569CD6", None)
        } else if scope.starts_with("variable")
            || scope.starts_with("entity.other.attribute-name")
            || scope.starts_with("meta.attribute")
        {
            ("syntaxVariable", "#9CDCFE", None)
        } else if scope.starts_with("punctuation") {
            ("syntaxPunctuation", "#D4D4D4", None)
        } else if scope.starts_with("markup.inserted") {
            ("toolDiffAdded", "#b5bd68", None)
        } else if scope.starts_with("markup.deleted") {
            ("toolDiffRemoved", "#cc6666", None)
        } else if scope.starts_with("markup.italic") {
            ("text", "#d4d4d4", Some(Modifier::ITALIC))
        } else if scope.starts_with("markup.bold") {
            ("text", "#d4d4d4", Some(Modifier::BOLD))
        } else {
            return None;
        };
        let mut s = self.role_style(role, default_hex);
        if let Some(m) = modifier {
            s = s.add_modifier(m);
        }
        Some(s)
    }
}

/// The structured per-role **background** sub-theme (feature #3; Pi background tokens, theme.ts:48-55).
/// Every message/tool/selected background is a named field, so the whole background surface is
/// addressable at once instead of via ad-hoc flat-map string lookups. `None` ⇒ terminal default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackgroundTheme {
    /// Selected-row fill in selectors (`selectedBg`).
    pub selected: Option<Color>,
    /// User-message block fill (`userMessageBg`).
    pub user_message: Option<Color>,
    /// Custom/notice block fill (`customMessageBg`).
    pub custom_message: Option<Color>,
    /// Tool block fill while running (`toolPendingBg`).
    pub tool_pending: Option<Color>,
    /// Tool block fill on success (`toolSuccessBg`).
    pub tool_success: Option<Color>,
    /// Tool block fill on error (`toolErrorBg`).
    pub tool_error: Option<Color>,
}

/// The structured **thinking-border** sub-theme (feature #3; Pi `thinking{Off..Xhigh}`,
/// interactive-mode.ts:3533-3541): the editor's escalating per-reasoning-level rule color, one typed
/// field per level. Each field is always populated (the spec/tui/03 §3.3 dark-hex fallback fills a
/// level the live theme omits), so the border is total and never a stray flat-map miss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThinkingTheme {
    /// `off` — reasoning disabled.
    pub off: Color,
    /// `minimal`.
    pub minimal: Color,
    /// `low`.
    pub low: Color,
    /// `medium` (the default level).
    pub medium: Color,
    /// `high`.
    pub high: Color,
    /// `xhigh` — maximum reasoning.
    pub xhigh: Color,
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

/// The xterm 6×6×6 color-cube channel values (indices 0–5) — Pi `CUBE_VALUES` (`theme.ts:183`).
const CUBE_VALUES: [i32; 6] = [0, 95, 135, 175, 215, 255];

/// Quantize `(r,g,b)` to the nearest xterm-256 palette index (a **faithful port** of Pi's `rgbTo256`,
/// `theme.ts:222-253`): the closer of the nearest 6×6×6 cube cell (indices 16–231) and the nearest
/// 24-step grayscale ramp entry (indices 232–255) under a luma-weighted Euclidean distance, but the
/// grayscale ramp is only preferred for near-neutral colors (channel spread `< 10`) so tinted colors
/// keep their hue. This is the quantizer [`ColorMode::Ansi256`] applies at the projection boundary.
pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    // Nearest cube channel, returning both its palette index (0..5) and its value (no re-indexing, so
    // the whole function stays panic-free under `clippy::indexing_slicing`).
    let closest_cube = |v: i32| -> (usize, i32) {
        // Seed replaced on the first iteration (best_d starts at MAX); value is irrelevant.
        let mut best = (0usize, 0i32);
        let mut best_d = i32::MAX;
        for (i, &c) in CUBE_VALUES.iter().enumerate() {
            let d = (v - c).abs();
            if d < best_d {
                best_d = d;
                best = (i, c);
            }
        }
        best
    };
    // Human-eye-weighted squared distance (Pi `colorDistance`, coefficients ×1000 to stay integral).
    let dist = |r1: i32, g1: i32, b1: i32, r2: i32, g2: i32, b2: i32| -> i64 {
        let (dr, dg, db) = ((r1 - r2) as i64, (g1 - g2) as i64, (b1 - b2) as i64);
        dr * dr * 299 + dg * dg * 587 + db * db * 114
    };

    let ((ri, rv), (gi, gv), (bi, bv)) = (closest_cube(r), closest_cube(g), closest_cube(b));
    let cube_index = 16 + 36 * ri + 6 * gi + bi;
    let cube_dist = dist(r, g, b, rv, gv, bv);

    // Grayscale ramp: 24 grays from 8 to 238 (Pi `GRAY_VALUES`).
    let gray = ((299 * r + 587 * g + 114 * b) as f64 / 1000.0).round() as i32;
    let mut gray_idx = 0usize;
    let mut gray_best = i32::MAX;
    for i in 0..24i32 {
        let value = 8 + i * 10;
        let d = (gray - value).abs();
        if d < gray_best {
            gray_best = d;
            gray_idx = i as usize;
        }
    }
    let gray_value = 8 + gray_idx as i32 * 10;
    let gray_index = 232 + gray_idx;
    let gray_dist = dist(r, g, b, gray_value, gray_value, gray_value);

    let spread = r.max(g).max(b) - r.min(g).min(b);
    if spread < 10 && gray_dist < cube_dist {
        gray_index as u8
    } else {
        cube_index as u8
    }
}

/// The 16 ANSI base colors as RGB (the xterm defaults), for [`ColorMode::Ansi16`] quantization.
const ANSI16_RGB: [(u8, u8, u8); 16] = [
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

/// Quantize `(r,g,b)` to the nearest of the 16 ANSI base colors (depth-limited terminals). Returns the
/// palette index `0..=15` as a `Color::Indexed` value.
fn rgb_to_16(r: u8, g: u8, b: u8) -> u8 {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    let mut best = 0u8;
    let mut best_d = i64::MAX;
    for (i, &(cr, cg, cb)) in ANSI16_RGB.iter().enumerate() {
        let (dr, dg, db) = ((r - cr as i32) as i64, (g - cg as i32) as i64, (b - cb as i32) as i64);
        let d = dr * dr * 299 + dg * dg * 587 + db * db * 114;
        if d < best_d {
            best_d = d;
            best = i as u8;
        }
    }
    best
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

// ============================================================================
// ThemeController (Pi theme-controller.ts + theme.ts theme-resolution) — feature #4
// ============================================================================

/// The detected/assumed terminal background polarity (Pi `TerminalTheme`, `theme.ts`), used to resolve
/// an `auto` theme setting and as the fallback theme when `settings.theme` is unset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TerminalTheme {
    /// A light terminal background — the boot fallback resolves to the `light` theme.
    Light,
    /// A dark terminal background — the boot fallback resolves to the `dark` theme (Pi's default).
    #[default]
    Dark,
}

impl TerminalTheme {
    /// The built-in theme name for this polarity (Pi resolves an unset setting to the terminal theme
    /// name, `theme-controller.ts:53-55`).
    pub fn theme_name(self) -> &'static str {
        match self {
            TerminalTheme::Light => "light",
            TerminalTheme::Dark => "dark",
        }
    }

    /// Detect the terminal background polarity from the environment the way Pi does
    /// (`detectTerminalBackgroundFromEnv`, `theme.ts:724-743`): parse the last numeric field of
    /// `COLORFGBG` as the background palette index and classify by its luminance; on no hint, fall back
    /// to [`TerminalTheme::Dark`] (Pi's `"fallback"` / low-confidence default).
    pub fn detect() -> TerminalTheme {
        detect_terminal_background_from_env(&std::env::var("COLORFGBG").unwrap_or_default()).theme
    }
}

/// Where a [`TerminalThemeDetection`] came from (Pi `TerminalThemeDetection.source`,
/// `theme.ts:691-697`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalThemeSource {
    /// An OSC 11 reply from the terminal itself.
    TerminalBackground,
    /// The `COLORFGBG` environment hint.
    ColorFgBg,
    /// Nothing answered — Pi's low-confidence `dark` default.
    Fallback,
}

/// How much a detection can be trusted (Pi `confidence`). Pi persists `settings.theme` only on
/// `"high"` (`theme-controller.ts:57-61`); a `Fallback` guess must never be written to disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectionConfidence {
    High,
    Low,
}

/// The result of a background-polarity detection (Pi `TerminalThemeDetection`, `theme.ts:691-697`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalThemeDetection {
    pub theme: TerminalTheme,
    pub source: TerminalThemeSource,
    /// Human-readable provenance, shown by `/debug` and Pi's theme diagnostics.
    pub detail: String,
    pub confidence: DetectionConfidence,
}

/// Pi `getThemeForRgbColor` (`theme.ts:743-745`): an sRGB-linearized relative luminance at or above
/// `0.5` is a light background.
pub fn theme_for_rgb(r: u8, g: u8, b: u8) -> TerminalTheme {
    if relative_luminance(r, g, b) >= 0.5 {
        TerminalTheme::Light
    } else {
        TerminalTheme::Dark
    }
}

/// Pi `detectTerminalBackgroundFromEnv` (`theme.ts:747-765`) with `COLORFGBG` passed in rather than
/// read from the process (env reads are global mutable state; the caller owns the read).
pub fn detect_terminal_background_from_env(colorfgbg: &str) -> TerminalThemeDetection {
    if let Some(bg) = colorfgbg_background_index(colorfgbg) {
        let (r, g, b) = ansi256_to_rgb(bg);
        return TerminalThemeDetection {
            theme: theme_for_rgb(r, g, b),
            source: TerminalThemeSource::ColorFgBg,
            detail: format!("background color index {bg}"),
            confidence: DetectionConfidence::High,
        };
    }
    TerminalThemeDetection {
        theme: TerminalTheme::Dark,
        source: TerminalThemeSource::Fallback,
        detail: "no terminal background hint found".to_string(),
        confidence: DetectionConfidence::Low,
    }
}

/// Pi `detectTerminalBackgroundTheme` (`theme.ts:768-788`): **ask the terminal first** with OSC 11
/// and classify the reply, falling back to `COLORFGBG` when the query times out or is unparseable.
///
/// This is the half cyrup was missing (TUI-004): production only ever read `COLORFGBG`, which most
/// terminals — including iTerm2, Ghostty, Alacritty, WezTerm and Terminal.app — do not set, so a
/// light-background user was always handed the dark theme.
pub fn detect_terminal_background_theme(
    probe: &dyn crate::terminal_query::TerminalProbe,
    timeout: std::time::Duration,
    colorfgbg: &str,
) -> TerminalThemeDetection {
    if let Some((r, g, b)) = probe.query_background_color(timeout) {
        return TerminalThemeDetection {
            theme: theme_for_rgb(r, g, b),
            source: TerminalThemeSource::TerminalBackground,
            detail: format!("OSC 11 background rgb({r}, {g}, {b})"),
            confidence: DetectionConfidence::High,
        };
    }
    detect_terminal_background_from_env(colorfgbg)
}

/// Pi `detectTerminalThemeForAuto` (`theme.ts:790-801`): for an `auto` (`light/dark`) setting the
/// terminal's *declared* color scheme (DSR `?996` → `CSI ? 997 ; N n`) wins over inferring polarity
/// from the background color, because a terminal that implements the notification protocol knows its
/// own preference. Unsupported ⇒ fall through to [`detect_terminal_background_theme`].
pub fn detect_terminal_theme_for_auto(
    probe: &dyn crate::terminal_query::TerminalProbe,
    timeout: std::time::Duration,
    colorfgbg: &str,
) -> TerminalTheme {
    if let Some(scheme) = probe.query_color_scheme(timeout) {
        return scheme;
    }
    detect_terminal_background_theme(probe, timeout, colorfgbg).theme
}

/// Pi `parseAutoThemeSetting` (`theme.ts:638-653`): a `"<light>/<dark>"` setting with exactly one
/// slash parses into a `(light, dark)` pair; anything else is not an auto setting.
pub fn parse_auto_theme_setting(setting: Option<&str>) -> Option<(String, String)> {
    let s = setting?;
    let first = s.find('/')?;
    // Reject a second slash (Pi: `indexOf("/", slashIndex+1) !== -1`).
    if s[first + 1..].contains('/') {
        return None;
    }
    let light = s[..first].trim();
    let dark = s[first + 1..].trim();
    if light.is_empty() || dark.is_empty() {
        return None;
    }
    Some((light.to_string(), dark.to_string()))
}

/// Pi `resolveThemeSetting` (`theme.ts:655-666`): resolve the raw `settings.theme` value against the
/// detected `terminal` polarity into a concrete theme name. An `auto` (`light/dark`) setting picks the
/// arm matching `terminal`; a bare name passes through; any other slash-namespaced value ⇒ `None`
/// (unresolvable → caller falls back to the terminal theme).
pub fn resolve_theme_setting(setting: Option<&str>, terminal: TerminalTheme) -> Option<String> {
    if let Some((light, dark)) = parse_auto_theme_setting(setting) {
        return Some(match terminal {
            TerminalTheme::Light => light,
            TerminalTheme::Dark => dark,
        });
    }
    match setting {
        Some(s) if s.contains('/') => None,
        Some(s) => Some(s.to_string()),
        None => None,
    }
}

/// The boot + live-switch owner of the render theme (Pi `InteractiveThemeController`,
/// `theme-controller.ts`). It resolves the boot theme from `settings.theme` with a terminal-bg
/// fallback, carries the [`ColorMode`] so every projected [`UiTheme`] is depth-correct, and drives
/// `/theme` + hot-switch by name. This is the seam the audit calls for (#4): production booted
/// dark-only, ignoring `settings.theme` + terminal background; the controller fixes that.
#[derive(Clone, Debug)]
pub struct ThemeController {
    color_mode: ColorMode,
    terminal_theme: TerminalTheme,
    active_name: String,
    generation: u64,
    /// The raw `settings.theme` value the controller booted from, retained so
    /// [`Self::sync_with_terminal`] can re-run Pi's `applyFromSettings` once the terminal is in raw
    /// mode and can actually answer a query.
    theme_setting: Option<String>,
    /// Whether the resolved setting is an `auto` (`light/dark`) pair, i.e. whether Pi would have
    /// enabled color-scheme notifications (`setAutoSync(true)`, `theme-controller.ts:107-111`).
    auto_sync: bool,
    /// The theme name a HIGH-confidence detection wants written back to `settings.theme` (Pi
    /// `settingsManager.setTheme(detection.theme)` + `flush()`, `theme-controller.ts:57-61`). Only
    /// ever set when the user has no explicit setting.
    persist: Option<String>,
}

impl ThemeController {
    /// Boot the controller from the raw `settings.theme` value (Pi `getThemeSetting()`), the terminal
    /// [`ColorMode`], and the detected terminal background polarity (Pi
    /// `theme-controller.ts` constructor + `applyFromSettings`, lines 32-59). The active theme is
    /// `resolveThemeSetting(setting, terminal)` when it resolves, else the terminal theme name — never
    /// hardwired dark. An unknown name degrades to `dark` at projection time (`UiTheme::builtin`).
    pub fn boot(
        theme_setting: Option<&str>,
        color_mode: ColorMode,
        terminal_theme: TerminalTheme,
    ) -> Self {
        let active_name = resolve_theme_setting(theme_setting, terminal_theme)
            .unwrap_or_else(|| terminal_theme.theme_name().to_string());
        ThemeController {
            color_mode,
            terminal_theme,
            active_name,
            generation: 0,
            theme_setting: theme_setting.map(str::to_string),
            auto_sync: parse_auto_theme_setting(theme_setting).is_some(),
            persist: None,
        }
    }

    /// Boot with the color mode + terminal polarity detected from the environment (the binary path).
    ///
    /// This is only the FIRST half of Pi's boot: it uses `COLORFGBG` alone, because at this point the
    /// terminal is not yet in raw mode and cannot answer an escape query. Call
    /// [`Self::sync_with_terminal`] once raw mode is on to complete it.
    pub fn boot_from_env(theme_setting: Option<&str>) -> Self {
        ThemeController::boot(theme_setting, ColorMode::detect(), TerminalTheme::detect())
    }

    /// Re-run Pi's `applyFromSettings` (`theme-controller.ts:37-63`) now that the terminal can be
    /// **asked** rather than merely guessed at from `COLORFGBG`. Returns the freshly projected
    /// [`UiTheme`] when the active theme actually changed, so the caller repaints only then.
    ///
    /// The three branches are Pi's, in Pi's order:
    ///
    /// 1. an `auto` (`light/dark`) setting → [`detect_terminal_theme_for_auto`] (DSR `?996` first,
    ///    then OSC 11, then `COLORFGBG`), auto-sync on, and the matching arm applied;
    /// 2. an explicit `settings.theme` → auto-sync off, the name applied verbatim, no query at all
    ///    (Pi never probes when the user has chosen; `:46-49`);
    /// 3. no setting → [`detect_terminal_background_theme`], and on `confidence == High` the result
    ///    is offered back for persistence via [`Self::theme_to_persist`].
    ///
    /// `colorfgbg` is the raw env value; pass `""` when unset. Timing/safety of the query itself is
    /// the probe's contract — see [`crate::terminal_query`].
    pub fn sync_with_terminal(
        &mut self,
        probe: &dyn crate::terminal_query::TerminalProbe,
        timeout: std::time::Duration,
        colorfgbg: &str,
    ) -> Option<UiTheme> {
        let setting = self.theme_setting.clone();
        let resolved = if let Some((light, dark)) = parse_auto_theme_setting(setting.as_deref()) {
            self.terminal_theme = detect_terminal_theme_for_auto(probe, timeout, colorfgbg);
            self.auto_sync = true;
            match self.terminal_theme {
                TerminalTheme::Light => light,
                TerminalTheme::Dark => dark,
            }
        } else if let Some(name) = setting {
            self.auto_sync = false;
            name
        } else {
            self.auto_sync = false;
            let detection = detect_terminal_background_theme(probe, timeout, colorfgbg);
            self.terminal_theme = detection.theme;
            let name = detection.theme.theme_name().to_string();
            if detection.confidence == DetectionConfidence::High {
                self.persist = Some(name.clone());
            }
            name
        };
        (resolved != self.active_name).then(|| self.set_theme_name(resolved))
    }

    /// Whether the active setting is an `auto` pair, i.e. whether Pi would keep terminal
    /// color-scheme notifications (mode `2031`) enabled and re-theme on every change.
    ///
    /// cyrup reports this but deliberately does **not** enable mode `2031`, and the reason is a
    /// safety one rather than an oversight: crossterm surfaces no event for the unsolicited
    /// `CSI ? 997 ; N n` notification, so every push the terminal sent would reach `event::read()`
    /// and be mis-decoded as stray keystrokes into the user's prompt. Turning the notifications on
    /// without a consumer is strictly worse than leaving them off. Independently, committed
    /// transcript rows have already gone to `Terminal::insert_before` and live in the terminal's own
    /// scrollback, so a mid-session polarity flip could never recolor what is already on screen
    /// (ADR-0001). Detection therefore happens once, at boot.
    pub fn auto_sync(&self) -> bool {
        self.auto_sync
    }

    /// The theme name a high-confidence detection wants persisted to `settings.theme`, if any (Pi
    /// `theme-controller.ts:57-61`). Consumed once by the caller that owns the settings manager.
    pub fn theme_to_persist(&self) -> Option<&str> {
        self.persist.as_deref()
    }

    /// The projected render theme for the active name (built-in lookup, then depth projection). This is
    /// what the app boots its `UiTheme` from and re-reads on a live `/theme` switch.
    pub fn theme(&self) -> UiTheme {
        UiTheme::builtin(&self.active_name)
            .with_color_mode(self.color_mode)
            .with_generation(self.generation)
    }

    /// The active theme name (test/inspection).
    pub fn active_name(&self) -> &str {
        &self.active_name
    }

    /// The color mode the controller projects into (test/inspection).
    pub fn color_mode(&self) -> ColorMode {
        self.color_mode
    }

    /// The terminal background polarity the boot theme was resolved against (Pi
    /// `getTerminalTheme`, `theme-controller.ts:88-90`; drives auto-sync + the unset-setting fallback).
    pub fn terminal_theme(&self) -> TerminalTheme {
        self.terminal_theme
    }

    /// Switch the active theme by name (Pi `setThemeName`, `theme-controller.ts:62-65`), bumping the
    /// generation so render caches invalidate. Returns the freshly projected [`UiTheme`].
    pub fn set_theme_name(&mut self, name: impl Into<String>) -> UiTheme {
        self.active_name = name.into();
        self.generation = self.generation.saturating_add(1);
        self.theme()
    }
}

/// Pi `getColorFgBgBackgroundIndex` (`theme.ts:697-706`): the last valid `0..=255` field of a
/// semicolon-separated `COLORFGBG`.
fn colorfgbg_background_index(colorfgbg: &str) -> Option<u8> {
    colorfgbg
        .split(';')
        .rev()
        .filter_map(|p| p.trim().parse::<i32>().ok())
        .find(|&n| (0..=255).contains(&n))
        .map(|n| n as u8)
}

/// WCAG relative luminance of an sRGB color (Pi `getRgbColorLuminance`, `theme.ts:708-714`).
fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
    let lin = |c: u8| {
        let v = c as f64 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// Map an xterm-256 palette index to its RGB (Pi `ansi256ToHex`, `theme.ts:968`): the 16 base colors,
/// the 6×6×6 cube (16–231), and the 24-step grayscale ramp (232–255).
fn ansi256_to_rgb(index: u8) -> (u8, u8, u8) {
    // All indices are `.get`-guarded so the function stays panic-free (`clippy::indexing_slicing`).
    let cube = |i: i32| CUBE_VALUES.get(i as usize).copied().unwrap_or(0) as u8;
    match index {
        0..=15 => ANSI16_RGB.get(index as usize).copied().unwrap_or((0, 0, 0)),
        16..=231 => {
            let i = (index - 16) as i32;
            (cube(i / 36), cube((i / 6) % 6), cube(i % 6))
        }
        _ => {
            let v = (8 + (index as i32 - 232) * 10) as u8;
            (v, v, v)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn color_mode_projects_rgb_to_indexed_and_leaves_named_alone() {
        let rgb = Color::Rgb(0x8a, 0xbe, 0xb7);
        assert!(matches!(ColorMode::Ansi256.project(rgb), Color::Indexed(_)));
        assert_eq!(ColorMode::TrueColor.project(rgb), rgb);
        assert_eq!(ColorMode::None.project(rgb), Color::Reset);
        // Named/indexed colors are already depth-safe and pass through unchanged.
        assert_eq!(ColorMode::Ansi256.project(Color::Cyan), Color::Cyan);
        assert_eq!(ColorMode::Ansi256.project(Color::Indexed(42)), Color::Indexed(42));
    }

    #[test]
    fn with_color_mode_is_idempotent_and_projects_every_role() {
        let dark = UiTheme::dark().with_color_mode(ColorMode::Ansi256);
        // Foreground is now an indexed color, never RGB.
        assert!(matches!(dark.foreground, Some(Color::Indexed(_))));
        assert!(dark.roles.values().all(|c| !matches!(c, Color::Rgb(_, _, _))));
        // Re-applying the same mode changes nothing (idempotent for a projected theme).
        let again = dark.clone().with_color_mode(ColorMode::Ansi256);
        assert_eq!(again.foreground, dark.foreground);
    }

    #[test]
    fn parse_auto_theme_setting_matches_pi() {
        assert_eq!(
            parse_auto_theme_setting(Some("light/dark")),
            Some(("light".to_string(), "dark".to_string()))
        );
        // Exactly one slash required; a bare name or a two-slash value is not an auto setting.
        assert_eq!(parse_auto_theme_setting(Some("dark")), None);
        assert_eq!(parse_auto_theme_setting(Some("a/b/c")), None);
        assert_eq!(parse_auto_theme_setting(None), None);
    }

    #[test]
    fn resolve_theme_setting_matches_pi() {
        // Auto setting resolves against the terminal polarity.
        assert_eq!(
            resolve_theme_setting(Some("solarized-light/solarized-dark"), TerminalTheme::Dark),
            Some("solarized-dark".to_string())
        );
        // A bare name passes through; an unresolvable slash value ⇒ None (caller falls back).
        assert_eq!(resolve_theme_setting(Some("nord"), TerminalTheme::Light), Some("nord".to_string()));
        assert_eq!(resolve_theme_setting(Some("a/b/c"), TerminalTheme::Dark), None);
        assert_eq!(resolve_theme_setting(None, TerminalTheme::Light), None);
    }

    #[test]
    fn ansi256_to_rgb_known_points() {
        assert_eq!(ansi256_to_rgb(16), (0, 0, 0));
        assert_eq!(ansi256_to_rgb(231), (255, 255, 255));
        assert_eq!(ansi256_to_rgb(244), (128, 128, 128));
    }
}

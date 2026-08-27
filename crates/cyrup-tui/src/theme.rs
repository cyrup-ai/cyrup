//! Theme application (R-10-025/026/027; arch-10 §3.8).
//!
//! `cyrup-resources` owns themes on disk (parsing, `vars`, hot-reload via `ThemeWatcher`). This
//! module is the *render-facing* projection: it maps the resolved color roles
//! (`cyrup_resources::theme::ResolvedTheme` / `ColorSpec`) onto `ratatui::style::Color` and exposes
//! the per-component `Style`s the widgets read. A `generation` counter is bumped on every
//! hot-reload so render caches can be invalidated (R-10-026).

use cyrup_resources::theme::{builtin_themes, ColorSpec, ResolvedTheme, ThemeData};
use ratatui::style::{Color, Modifier, Style};

/// The terminal color-depth the [`UiTheme`] projects its RGB roles into (Pi `ColorMode`, v0.84.1
/// `coding-agent/src/modes/interactive/theme/theme.ts:167` + the capability gate at `:611`).
///
/// Pi carries only `truecolor`/`256color`. `Ansi16` and `None` are cyrup-only *explicit* modes,
/// reachable through [`UiTheme::with_color_mode`] for depth-limited/monochrome output so the
/// projection is total — but [`ColorMode::detect`] never selects them (T3, TUI-FIDELITY §2): a
/// detected terminal always lands on `TrueColor` or `Ansi256`, matching Pi. The mode is chosen once
/// at boot from the terminal capabilities and re-applied whenever the theme changes
/// (`ThemeController`).
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
    /// Pick the color mode from the environment exactly the way Pi does:
    /// `const colorMode = mode ?? (getCapabilities().trueColor ? "truecolor" : "256color")`
    /// (v0.84.1 `coding-agent/src/modes/interactive/theme/theme.ts:611`).
    ///
    /// T2/T3 (TUI-FIDELITY §2): this used to read `COLORTERM`/`TERM` directly and had two bugs.
    /// (a) `COLORTERM` is only Pi's *fallback* hint for an unidentified terminal
    /// (`tui/src/terminal-image.ts:73`); the gate is the terminal-program table at `:76-131`, so
    /// iTerm2 / Windows Terminal / VS Code / Alacritty / JetBrains — none of which set `COLORTERM`
    /// — were being quantised through [`rgb_to_256`], collapsing the three tool background tints
    /// into near-identical cube cells. (b) Pi has **no** monochrome mode at all —
    /// `type ColorMode = "truecolor" | "256color"` (`theme.ts:167`) — so `TERM=dumb` or an unset
    /// `TERM` must still get the full 256-colour UI, not [`ColorMode::None`].
    ///
    /// The terminal table is already ported once, in [`crate::image::detect_capabilities_from`];
    /// this delegates to it rather than growing a second copy. The tmux OSC-8 probe cannot change
    /// `true_color`, so `false` is passed for it and the `tmux display-message` subprocess is
    /// skipped.
    pub fn detect() -> ColorMode {
        ColorMode::detect_from(|k| std::env::var(k).ok())
    }

    /// The pure core of [`ColorMode::detect`], parameterised over an environment lookup so both
    /// arms are deterministically testable (same shape as `detect_capabilities_from`).
    pub fn detect_from(env: impl Fn(&str) -> Option<String>) -> ColorMode {
        if crate::image::detect_capabilities_from(env, false).true_color {
            ColorMode::TrueColor
        } else {
            // Pi `createTheme` falls back to `"256color"` when truecolor is unavailable
            // (v0.84.1 theme.ts:611). There is no lower rung upstream.
            ColorMode::Ansi256
        }
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

/// The three states Pi's `getEditHeaderBg` distinguishes for an `edit` block's fill
/// (`core/tools/edit.ts:239-253`) — the `EditCallRenderComponent.preview` union collapsed to what
/// the background actually keys on. See [`UiTheme::edit_bg_style`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditHeaderPreview {
    /// `computeEditsDiff` produced a diff (`preview` is `{diff, firstChangedLine}`) → `toolSuccessBg`.
    Computed,
    /// The preview failed (`preview` is `{error}`) → `toolErrorBg`.
    Failed,
    /// No preview yet (`preview === undefined`) → `settledError ? toolErrorBg : toolPendingBg`.
    Absent,
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
    /// The shared, process-wide `UiTheme::default()` — built **once**, then handed out by
    /// reference. For callers that need *a* theme only to reach a style-independent result and
    /// throw it away again; anything that actually paints must use the live theme instead.
    ///
    /// (D) The `desired_height(width)` measurement path is the motivating caller. `app/layout.rs`
    /// asks the focused selector for its height on **every frame** it owns the input slot, and the
    /// selectors answer by laying their body out against a scratch theme. Each `UiTheme::default()`
    /// goes to [`UiTheme::dark`] → [`UiTheme::builtin_or_static`] → `builtin_themes`
    /// (`cyrup-resources/src/theme.rs`), which re-parses BOTH `BUILTIN_DARK_JSON` and
    /// `BUILTIN_LIGHT_JSON` (~4.5 KB of JSON) with no cache and then resolves a ~51-entry
    /// `BTreeMap` of roles — all of it discarded as soon as the line count is known. The measured
    /// height depends on the text and the width, never on the colors, so sharing one instance
    /// changes nothing observable.
    ///
    /// This is deliberately **not** wired into `impl Default`, which keeps yielding an owned
    /// theme the caller may still project or re-stamp ([`UiTheme::with_color_mode`],
    /// [`UiTheme::with_generation`]) — unchanged.
    pub fn default_ref() -> &'static UiTheme {
        static DEFAULT: std::sync::LazyLock<UiTheme> = std::sync::LazyLock::new(UiTheme::default);
        &DEFAULT
    }

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
    #[must_use]
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
    #[must_use]
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

    /// Error style (failed tools, error notifications) — Pi `error` (dark.json:41), **colour only**.
    ///
    /// T4 (TUI-FIDELITY §2): this used to bake in `Modifier::BOLD`. Pi's `Theme.fg()`
    /// (v0.84.1 `coding-agent/src/modes/interactive/theme/theme.ts:372-376`) emits a bare SGR
    /// foreground and resets only `\x1b[39m`; `bold()` is a *separate* combinator (`:384-386`).
    /// `git grep -c 'bold(theme.fg("error"' v0.84.1 -- packages` matches nothing — no upstream
    /// error string is bold — so the modifier is dropped here rather than at each of the 13
    /// render sites.
    pub fn error_style(&self) -> Style {
        Style::default().fg(self.error.unwrap_or(Color::Red))
    }

    /// Secondary/hint chrome — Pi's **`dim` token**, colour only (`dark.json:31 "dim": "dimGray"`
    /// = `#666666`; `light.json:30` = `#767676`).
    ///
    /// T1 (TUI-FIDELITY §2): this used to resolve the `text` role and add `Modifier::DIM`, which is
    /// wrong twice over. Pi renders every hint through `theme.fg("dim", …)` (e.g.
    /// `theme.ts:1312`/`:1314` in `getSettingsListTheme`), and `fg()` (`theme.ts:372-376`) emits a
    /// plain foreground escape with **no SGR attribute** — so cyrup was painting body-bright text
    /// plus SGR 2, which terminals that ignore SGR 2 (Terminal.app, much of tmux, Windows consoles)
    /// render at full brightness, and which in the *light* theme came out near-black `#1f2328`
    /// where Pi draws grey.
    pub fn dim_style(&self) -> Style {
        self.role_style("dim", "#666666", "#767676")
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
    ///
    /// The `muted` token is `gray` `#808080` (`dark.json:30`+`:11`) / `mediumGray` `#6c6c6c`
    /// (`light.json:29`+`:11`); it is a *different* token from `dim`, so a theme that omits it falls
    /// back to its own palette's grey rather than to [`Self::dim_style`]'s `dimGray`.
    pub fn muted_style(&self) -> Style {
        match self.muted {
            Some(c) => Style::default().fg(c),
            None => self.role_style("muted", "#808080", "#6c6c6c"),
        }
    }

    /// Border/rule style for the editor + selector `DynamicBorder` rules — Pi `border` (theme.ts:537).
    pub fn border_style(&self) -> Style {
        Style::default().fg(self.border.or(self.muted).unwrap_or(Color::DarkGray))
    }

    /// The `borderAccent` role — Pi's "highlighted border" token (`docs/themes.md:158`).
    ///
    /// T9: `git grep borderAccent v0.84.1 -- packages/*/src` finds exactly **one** component render
    /// site outside the HTML-export stylesheet and the theme plumbing itself —
    /// `tree-selector.ts:824`, the `/tree` compaction row:
    ///
    /// ```text
    /// case "compaction": {
    ///     const tokens = Math.round(entry.tokensBefore / 1000);
    ///     result = theme.fg("borderAccent", `[compaction: ${tokens}k tokens]`);
    /// ```
    ///
    /// So this is not a border colour in practice; it is the colour of that one row, and until
    /// [`crate::TreeSelector`] coloured its rows per role (S24) the token had no read site at all
    /// even though a custom theme is *required* to define it (`theme-schema.json:41`).
    ///
    /// It is a distinct colour from `accent` in both built-ins — `cyan` `#00d7ff` vs `accent`
    /// `#8abeb7` (`dark.json:5,14,23,25`), `teal` vs `#5a8080` (`light.json:24`) — so the fallback
    /// chain goes to `border` before `accent` rather than collapsing onto the accent role.
    pub fn border_accent_style(&self) -> Style {
        let fg = self
            .roles
            .get("borderAccent")
            .copied()
            .or(self.border)
            .or(self.accent)
            .unwrap_or(Color::Cyan);
        Style::default().fg(fg)
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

    /// The editor's top/bottom rule style for a reasoning `level` — Pi `thinking{Off..Max}`
    /// (`Theme.getThinkingBorderColor`, v0.84.1
    /// `coding-agent/src/modes/interactive/theme/theme.ts:420-440`): an escalating per-level color
    /// that is the editor's primary always-visible mode signal.
    ///
    /// An unrecognized level resolves to `thinkingOff`, matching Pi's `default:` arm
    /// (`theme.ts:437-438` — `return (str) => this.fg("thinkingOff", str)`). This used to fall back
    /// to the `border` role, a token Pi never reaches from here.
    pub fn thinking_border_style(&self, level: &str) -> Style {
        let thinking = self.thinking();
        let color = match level {
            "minimal" => thinking.minimal,
            "low" => thinking.low,
            "medium" => thinking.medium,
            "high" => thinking.high,
            "xhigh" => thinking.xhigh,
            "max" => thinking.max,
            // `"off"` and Pi's `default:` arm share `thinkingOff` (theme.ts:423-424, :437-438).
            _ => thinking.off,
        };
        Style::default().fg(color)
    }

    /// The **editor's own** top/bottom rule when no reasoning level owns it — Pi `borderMuted`.
    ///
    /// T9 (TUI-FIDELITY §2): Pi's shared `Editor` initialises `this.borderColor` from
    /// `getEditorTheme().borderColor`, which is `(text) => theme.fg("borderMuted", text)` (v0.84.1
    /// `theme.ts:1301-1304`, consumed at `tui/src/components/editor.ts:348,494`). Only the *chat*
    /// editor is then reassigned per thinking level / bash mode
    /// (`interactive-mode.ts:3990-3993`); an `ExtensionEditorComponent`, built as
    /// `new Editor(tui, getEditorTheme(), options)` (`components/extension-editor.ts:70`), never is,
    /// so its rule stays `borderMuted` (`dark.json:26` = `darkGray`, `light.json:25` = `lightGray`).
    /// Falls back to the `border` role, then `muted`.
    pub fn border_muted_style(&self) -> Style {
        let fg = self
            .roles
            .get("borderMuted")
            .copied()
            .or(self.border)
            .or(self.muted)
            .unwrap_or(Color::DarkGray);
        Style::default().fg(fg)
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
        let selected = g("selectedBg");
        BackgroundTheme {
            selected,
            user_message: g("userMessageBg"),
            custom_message: g("customMessageBg"),
            tool_pending: g("toolPendingBg"),
            tool_success: g("toolSuccessBg"),
            tool_error: g("toolErrorBg"),
            // `scrollbarThumb: bgColors.scrollbarThumb ?? bgColors.selectedBg` (`theme.ts:365`,
            // and again at `:330`). The fallback is to this theme's OWN resolved `selectedBg`,
            // never a hardcoded colour — exactly as `thinkingMax` falls back to `thinkingXhigh`.
            scrollbar_thumb: g("scrollbarThumb").or(selected),
        }
    }

    /// The structured **thinking-border** sub-theme (Pi `thinking{Off..Xhigh}`, interactive-mode.ts:
    /// 3533-3541): the escalating per-reasoning-level editor rule color, one typed field per level,
    /// each resolved from the live theme with the spec/tui/03 §3.3 dark-hex fallback so it is total.
    pub fn thinking(&self) -> ThinkingTheme {
        // Fallback pairs, `dark.json:73-78` / `light.json:72-77`. `thinkingOff` used to default to
        // `#666666` (`dimGray`); the token is `darkGray` `#505050` dark / `lightGray` `#b0b0b0`
        // light — the same drifted-fallback defect as the markdown roles in [`Self::role_style`].
        let level = |key: &str, dark_hex: &str, light_hex: &str| {
            self.role_color_themed(key, dark_hex, light_hex)
        };
        let xhigh = level("thinkingXhigh", "#d183e8", "#8b008b");
        ThinkingTheme {
            off: level("thinkingOff", "#505050", "#b0b0b0"),
            minimal: level("thinkingMinimal", "#6e6e6e", "#767676"),
            low: level("thinkingLow", "#5f87af", "#547da7"),
            medium: level("thinkingMedium", "#81a2be", "#5a8080"),
            high: level("thinkingHigh", "#b294bb", "#875f87"),
            xhigh,
            // Pi made `thinkingMax` an OPTIONAL theme token with an explicit
            // `colors.thinkingMax ?? colors.thinkingXhigh` fallback (theme.ts:93,329,358) so
            // pre-`max` user themes keep loading. Ported verbatim: a theme that omits the token
            // reuses its OWN resolved `xhigh` color, never a hardcoded default.
            max: self.roles.get("thinkingMax").copied().unwrap_or(xhigh),
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
            // Routed through the struct so `theme.bg("scrollbarThumb", …)` (`interactive-mode.ts:
            // 874`) gets the `?? selectedBg` fallback rather than the raw map miss.
            "scrollbarThumb" => bg.scrollbar_thumb,
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

    /// Selected-row fill (`selectedBg`) laid over an arbitrary foreground style.
    ///
    /// SYS-4 (TUI-FIDELITY §5): upstream paints a selection background in exactly **two**
    /// components — `tree-selector.ts:750-753` (`gutter` and `body`) and `session-selector.ts:506-508`
    /// (the whole row) — and in neither case does it replace the row's foreground colours. It never
    /// fills in `SelectList` at all (`git grep selectedBg v0.84.1 -- packages/tui` is empty).
    /// Callers therefore build their spans first and lay the fill over each one, rather than
    /// swapping in a single style.
    /// S39: the old `selected_bg_style()` — `selectedBg` over the accent foreground, as a single
    /// ready-made style — is gone. After SYS-4 moved the fill out of `SelectList` it had **zero**
    /// callers under `src/`; being `pub`, nothing would have flagged it. Both remaining fill sites
    /// (`tree_selector.rs`, `session_selector.rs`) need the layering form above, because upstream
    /// wraps already-styled text (`theme.bg("selectedBg", body)`) rather than replacing its style.
    /// Callers that only want the colour ask for `selected_bg_over(Style::default()).bg`.
    pub fn selected_bg_over(&self, style: Style) -> Style {
        self.with_bg(style, "selectedBg")
    }

    /// User-message block: `userMessageBg` fill **and** `userMessageText` foreground.
    ///
    /// T8/T9 (TUI-FIDELITY §2): Pi's `UserMessage.rebuild()` wraps the markdown in
    /// `new Box(…, (content) => theme.bg("userMessageBg", content))` and passes
    /// `{ color: (content) => theme.fg("userMessageText", content) }` (v0.84.1
    /// `coding-agent/src/modes/interactive/components/user-message.ts:40-49`). This used to take its
    /// foreground from `base_style()` (the `text` role), so `userMessageText` — a token a custom
    /// theme is *required* to define — had no effect on screen. `text` is the fallback only when the
    /// theme omits the role.
    pub fn user_message_bg_style(&self) -> Style {
        let fg = self.roles.get("userMessageText").copied().or(self.foreground);
        let base = match fg {
            Some(c) => Style::default().fg(c),
            None => Style::default(),
        };
        self.with_bg(base, "userMessageBg")
    }

    /// Custom/notice block: `customMessageBg` fill **and** `customMessageText` foreground.
    ///
    /// T9: Pi renders the body as `new Markdown(text, …, { color: (text) => theme.fg(
    /// "customMessageText", text) })` (v0.84.1 `components/custom-message.ts:107-111`). This used to
    /// use [`Self::dim_style`], which is a different token entirely.
    pub fn custom_message_bg_style(&self) -> Style {
        self.with_bg(self.custom_message_text_style(), "customMessageBg")
    }

    /// `customMessageText` as a bare FOREGROUND, with no `customMessageBg` fill — Pi's
    /// `theme.fg("customMessageText", …)` on its own.
    ///
    /// X7 needs this: `formatCompactReadCall` paints the skill label
    /// `theme.fg("customMessageText", classification.label)` (`core/tools/read.ts:154`) inside the
    /// TOOL block, which already has its own `toolPendingBg`/`toolSuccessBg` tint.
    /// [`Self::custom_message_bg_style`] would patch the purple custom-message fill over that one
    /// row.
    pub fn custom_message_text_style(&self) -> Style {
        match self.roles.get("customMessageText").copied() {
            Some(c) => Style::default().fg(c),
            None => self.dim_style(),
        }
    }

    /// The `[customType]` label above a custom/notice block — `customMessageLabel`, bold.
    ///
    /// T9: Pi `theme.fg("customMessageLabel", "\x1b[1m[" + customType + "]\x1b[22m")` (v0.84.1
    /// `components/custom-message.ts:92`) — the `\x1b[1m…\x1b[22m` pair is SGR bold, applied inside
    /// the colour. Falls back to the accent role when the theme omits the token.
    pub fn custom_message_label_style(&self) -> Style {
        let fg = self
            .roles
            .get("customMessageLabel")
            .copied()
            .or(self.accent)
            .unwrap_or(Color::Cyan);
        Style::default().fg(fg).add_modifier(Modifier::BOLD)
    }

    /// Tool-call title (the `read`/`edit`/`$`/`grep …` headers) — Pi `toolTitle`, bold.
    ///
    /// T8: Pi is `theme.fg("toolTitle", theme.bold("read"))` (v0.84.1
    /// `coding-agent/src/core/tools/read.ts:81`, and identically `bash.ts:236`, `edit.ts:207`,
    /// `find.ts:80`, `grep.ts:84`, `ls.ts:60`, `write.ts:146`,
    /// `components/tool-execution.ts:136,366`). This used to read `self.foreground` — the `text`
    /// role — and never consult `roles["toolTitle"]`. The two built-ins alias them
    /// (`dark.json:45`/`light.json:44` both say `"toolTitle": "text"`) so nothing changes there, but
    /// a custom theme setting `toolTitle` was silently ignored on all ten tool headers.
    pub fn tool_title_style(&self) -> Style {
        let s = Style::default().add_modifier(Modifier::BOLD);
        match self.roles.get("toolTitle").copied().or(self.foreground) {
            Some(fg) => s.fg(fg),
            None => s,
        }
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

    /// X8 — the `edit` block's fill, which is **not** `done`/`is_error` keyed. Pi gives `edit` its own
    /// `getEditHeaderBg(preview, settledError)` (v0.84.1 `core/tools/edit.ts:239-253`), applied at
    /// `:262`:
    ///
    /// ```ts
    /// if (preview) {
    ///     if ("error" in preview) return (text) => theme.bg("toolErrorBg", text);
    ///     return (text) => theme.bg("toolSuccessBg", text);
    /// }
    /// if (settledError) return (text) => theme.bg("toolErrorBg", text);
    /// return (text) => theme.bg("toolPendingBg", text);
    /// ```
    ///
    /// The PREVIEW is tested first and `done` is never consulted: a diff that `computeEditsDiff`
    /// produced from the streamed arguments alone puts the block on the SUCCESS tint while the call
    /// is still pending (through the whole permission prompt), and a preview that failed puts it on
    /// the ERROR tint before anything is written. [`Self::tool_bg_style`] keys only on
    /// `done`/`is_error`, so both of those rendered neutral `toolPendingBg`.
    ///
    /// The settled case still lands on the right tint: `renderResult` re-runs `setEditPreview` from
    /// `details.diff` (`edit.ts:400-411`) before the component is rebuilt, so a successful write
    /// arrives here as [`EditHeaderPreview::Computed`].
    pub fn edit_bg_style(
        &self,
        base: Style,
        preview: EditHeaderPreview,
        settled_error: bool,
    ) -> Style {
        let key = match preview {
            EditHeaderPreview::Failed => "toolErrorBg",
            EditHeaderPreview::Computed => "toolSuccessBg",
            EditHeaderPreview::Absent if settled_error => "toolErrorBg",
            EditHeaderPreview::Absent => "toolPendingBg",
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

    /// Whether this palette is a **light** one, i.e. one that draws dark glyphs on a light ground.
    ///
    /// Only consulted to pick between the dark and light members of a hex fallback pair
    /// ([`Self::palette_hex`]); a theme that actually defines the role never reaches it. The name is
    /// authoritative for the two built-ins — `builtin_or_static` stamps `"dark"`/`"light"` and
    /// `from_resolved` copies `dark.json`/`light.json`'s own `"name"` field — and a custom theme
    /// falls back to the luma of its `text` role, because a light theme is exactly the one whose
    /// body text is dark. After [`Self::with_color_mode`] has quantized `foreground` to a
    /// `Color::Indexed` the luma test no longer applies and a non-built-in name resolves as dark;
    /// that only affects roles a custom theme left undefined, which Pi forbids outright
    /// (`REQUIRED_COLOR_TOKENS`).
    fn is_light_palette(&self) -> bool {
        if self.name.eq_ignore_ascii_case("light") {
            return true;
        }
        if self.name.eq_ignore_ascii_case("dark") {
            return false;
        }
        match self.foreground {
            // ITU-R BT.601 luma, in integer thousandths to keep the no-panic/no-float-cast profile.
            Some(Color::Rgb(r, g, b)) => {
                299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b) < 128_000
            }
            _ => false,
        }
    }

    /// Pick the member of a `(dark, light)` hex-fallback pair that matches this palette.
    fn palette_hex<'a>(&self, dark_hex: &'a str, light_hex: &'a str) -> &'a str {
        if self.is_light_palette() { light_hex } else { dark_hex }
    }

    /// [`Self::role_color`] with a **theme-aware** hex fallback pair.
    fn role_color_themed(&self, key: &str, dark_hex: &str, light_hex: &str) -> Color {
        self.role_color(key, self.palette_hex(dark_hex, light_hex))
    }

    /// `fg`-only style for a role with a `(dark, light)` hex fallback pair.
    ///
    /// The hexes are a last-resort value for the synthetic fallback theme (the one
    /// `builtin_or_static` synthesizes when the resource layer cannot supply a palette at all); any
    /// real theme resolves the role through [`Self::role_color`]. Five of these defaults had drifted
    /// away from BOTH Pi's `dark.json` and cyrup's own built-in dark palette — `mdCodeBlockBorder`,
    /// `mdQuote`, `mdQuoteBorder` and `mdHr` are `gray` `#808080` (`dark.json:53,54,55,56`;
    /// `cyrup-resources/src/theme.rs:568,569,570,571`) and `mdLinkUrl` is `dimGray` `#666666`
    /// (`dark.json:50`) — so the degraded theme drew a *different* palette from the normal one.
    /// Found while fixing T7; same accessor, same class of defect.
    ///
    /// Aligning them to `dark.json` alone then broke the *light* half: `builtin_or_static`
    /// synthesizes both [`Self::dark`] and [`Self::light`] with an empty `roles` map, so a
    /// resource-less light theme fell through to the same hexes and drew dark-theme greys. Every
    /// fallback is therefore a pair — `light.json`'s value is `mediumGray` `#6c6c6c` (`:11`, used by
    /// `mdCodeBlockBorder` `:52` / `mdQuote` `:53` / `mdQuoteBorder` `:54` / `mdHr` `:55`) and
    /// `dimGray` `#767676` (`:12`, used by `dim` `:30` / `mdLinkUrl` `:49`).
    fn role_style(&self, key: &str, dark_hex: &str, light_hex: &str) -> Style {
        Style::default().fg(self.role_color_themed(key, dark_hex, light_hex))
    }

    /// Markdown heading — `mdHeading`, bold (`markdown.ts:336-362`).
    pub fn md_heading_style(&self) -> Style {
        self.role_style("mdHeading", "#f0c674", "#9a7326").add_modifier(Modifier::BOLD)
    }
    /// Inline code span — `mdCode` (= accent), no backticks (`markdown.ts:512-516`).
    pub fn md_code_style(&self) -> Style {
        Style::default().fg(self.roles.get("mdCode").copied().or(self.accent).unwrap_or(Color::Cyan))
    }
    /// Flat (unknown-language) fenced-code body — `mdCodeBlock` (`markdown.ts:378-398`).
    pub fn md_code_block_style(&self) -> Style {
        self.role_style("mdCodeBlock", "#b5bd68", "#588458")
    }
    /// Fence border lines (```` ``` ````) — `mdCodeBlockBorder` (`markdown.ts:380,393`).
    pub fn md_code_block_border_style(&self) -> Style {
        self.role_style("mdCodeBlockBorder", "#808080", "#6c6c6c")
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
        self.role_style("thinkingText", "#808080", "#6c6c6c").add_modifier(Modifier::ITALIC)
    }
    /// Blockquote body — `mdQuote`, italic (`markdown.ts:414-461`).
    pub fn md_quote_style(&self) -> Style {
        self.role_style("mdQuote", "#808080", "#6c6c6c").add_modifier(Modifier::ITALIC)
    }
    /// Blockquote `│ ` border — `mdQuoteBorder` (`markdown.ts:414-461`).
    pub fn md_quote_border_style(&self) -> Style {
        self.role_style("mdQuoteBorder", "#808080", "#6c6c6c")
    }
    /// Horizontal rule — `mdHr` (`markdown.ts:463-468`).
    pub fn md_hr_style(&self) -> Style {
        self.role_style("mdHr", "#808080", "#6c6c6c")
    }
    /// List bullet marker — `mdListBullet` (`markdown.ts:604-654`).
    ///
    /// The `= accent` alias holds in the DARK palette only (`dark.json:57` `"mdListBullet":
    /// "accent"`); `light.json:56` maps it to `green` `#588458`, which is *not* the light accent
    /// (`teal` `#5a8080`). So the accent chain is the dark palette's rule and a resource-less light
    /// theme takes the light hex instead.
    pub fn md_list_bullet_style(&self) -> Style {
        match self.roles.get("mdListBullet").copied() {
            Some(c) => Style::default().fg(c),
            None if self.is_light_palette() => self.role_style("mdListBullet", "#588458", "#588458"),
            None => Style::default().fg(self.accent.unwrap_or(Color::Cyan)),
        }
    }
    /// Link text — `mdLink`, underlined (`markdown.ts:537-556`).
    pub fn md_link_style(&self) -> Style {
        self.role_style("mdLink", "#81a2be", "#547da7").add_modifier(Modifier::UNDERLINED)
    }
    /// Trailing ` (url)` after a markdown link — `mdLinkUrl`, **colour only**.
    ///
    /// T7 (TUI-FIDELITY §2): this used to add `Modifier::DIM`. Pi emits
    /// `styledLink + this.theme.linkUrl(" (" + token.href + ")")` (v0.84.1
    /// `tui/src/components/markdown.ts:705`) where `linkUrl` is
    /// `(text) => theme.fg("mdLinkUrl", text)` (`theme.ts:1256`) — `fg()` alone, no SGR attribute.
    /// The link *text* really is underlined (`markdown.ts:691`
    /// `this.theme.link(this.theme.underline(linkText))`), which is why
    /// [`Self::md_link_style`] keeps `UNDERLINED`; the URL suffix is not.
    pub fn md_link_url_style(&self) -> Style {
        self.role_style("mdLinkUrl", "#666666", "#767676")
    }

    /// Diff added (`+`) line — `toolDiffAdded`, green (`diff.ts` `theme.fg("toolDiffAdded")`).
    pub fn tool_diff_added_style(&self) -> Style {
        Style::default().fg(self.role_color_themed("toolDiffAdded", "#b5bd68", "#588458"))
    }
    /// Diff removed (`-`) line — `toolDiffRemoved`, red.
    pub fn tool_diff_removed_style(&self) -> Style {
        Style::default().fg(self.role_color_themed("toolDiffRemoved", "#cc6666", "#aa5555"))
    }
    /// Diff context (unchanged) line — `toolDiffContext`, gray.
    pub fn tool_diff_context_style(&self) -> Style {
        Style::default().fg(self.role_color_themed("toolDiffContext", "#808080", "#6c6c6c"))
    }
    /// Intra-line changed-token emphasis — reversed video (`theme.inverse`, `diff.ts:renderIntraLineDiff`).
    pub fn inverse_style(&self) -> Style {
        Style::default().add_modifier(Modifier::REVERSED)
    }

    /// The style for a scope that names a whole *annotation* / *preprocessor* construct, which Pi's
    /// highlighter emits as a single `meta` span (T6, TUI-FIDELITY §2).
    ///
    /// Pi maps cli-highlight's `meta` class to `muted` — `meta: (s) => t.fg("muted", s)`, v0.84.1
    /// `coding-agent/src/modes/interactive/theme/theme.ts:1128` — and highlight.js applies that one
    /// class to the *entire* Rust attribute / Python decorator / C preprocessor line.
    ///
    /// syntect's vocabulary is structural, not lexical: it nests finer scopes *inside* a
    /// `meta.annotation` / `meta.preprocessor` context (`#[derive(Debug)]` comes back as
    /// `meta.annotation.rust` wrapping `punctuation.definition.annotation.rust` and
    /// `variable.annotation.rust`), so the deepest-first walk in `markdown::scope_style` would only
    /// ever recolour the punctuation. This is checked *before* that walk so the container wins,
    /// reproducing Pi's one-span-one-colour result.
    ///
    /// It is deliberately NOT a blanket `meta` prefix. syntect also emits `meta.function.*`,
    /// `meta.block.*`, `meta.group.*` and `meta.qualified-name.*` around ordinary code — a bare
    /// `starts_with("meta")` would grey out most of a Rust or Python block, which is the opposite of
    /// Pi's output. The behaviour is ported; the scope string it keys off cannot be.
    ///
    /// The container does **not** swallow a nested string/comment literal — see
    /// [`Self::syntax_meta_nested_style`].
    pub fn syntax_meta_container_style(&self, scope: &str) -> Option<Style> {
        if scope.starts_with("meta.annotation") || scope.starts_with("meta.preprocessor") {
            Some(self.muted_style())
        } else {
            None
        }
    }

    /// The style for a scope nested *inside* a [`Self::syntax_meta_container_style`] construct that
    /// keeps its **own** colour instead of being greyed with the rest of the annotation.
    ///
    /// highlight.js does not emit one flat `meta` span: its `meta` modes declare sub-modes, and
    /// cli-highlight wraps each sub-mode in its own class, so the inner class is what
    /// `buildCliHighlightTheme` resolves. Both constructs named in the audit contain a *string*
    /// sub-mode — a Rust attribute's `#[cfg(feature = "wasm-host")]` literal and a C
    /// `#include <stdio.h>`'s bracketed header — so Pi paints those `syntaxString`
    /// (`theme.ts:1125`) while the surrounding `#[cfg(`…`)]` / `#include` stays `muted`
    /// (`theme.ts:1128`). Returning the container style for the whole construct over-greyed them.
    ///
    /// Restricted to `string` and `comment` on purpose. The wider "any nested scope with its own
    /// mapping escapes" rule would undo T6 entirely, because syntect scopes an annotation's own
    /// glyphs as `punctuation.definition.annotation.*` / `variable.annotation.*` /
    /// `keyword.control.import.*` — all three of which the prefix table maps — leaving nothing
    /// muted. Numbers are deliberately excluded because upstream is grammar-dependent there
    /// (highlight.js's Python decorator mode contains a number sub-mode, its C preprocessor mode
    /// does not, so `#define N 42`'s `42` is plain `meta`), and greying them matches the two
    /// constructs the audit names.
    pub fn syntax_meta_nested_style(&self, scope: &str) -> Option<Style> {
        if scope.starts_with("string") || scope.starts_with("comment") {
            self.syntax_style_for_scope(scope)
        } else {
            None
        }
    }

    /// Resolve a `syntect` scope to a syntax-highlight style, the prefix table mirroring Pi's
    /// `buildCliHighlightTheme` (v0.84.1 `theme.ts:1119-1145`). Unknown scopes return `None`, and the
    /// caller then emits the run **unstyled** — Pi pushes cli-highlight's output verbatim
    /// (`tui/src/components/markdown.ts:526` `lines.push(`${indent}${hlLine}`)`), so a token the
    /// highlighter did not classify carries no escape and sits at the terminal default.
    ///
    /// Three classes are **attribute-only, with no foreground at all**: `buildCliHighlightTheme`
    /// builds them from the bare `chalk` combinators — `emphasis: (s) => t.italic(s)` (`:1140`),
    /// `strong: (s) => t.bold(s)` (`:1141`), `link: (s) => t.underline(s)` (`:1142`), where
    /// `italic`/`bold`/`underline` are `chalk.italic`/`chalk.bold`/`chalk.underline`
    /// (`theme.ts:384-394`) and never call `fg()`. `markup.italic`/`markup.bold` used to return an
    /// explicit `text` foreground alongside the attribute, which *overrides* whatever colour the
    /// surrounding run had — the same defect class as unclassified spans defaulting to
    /// `mdCodeBlock`. `markup.underline` (syntect's scope for a markdown link target,
    /// `markup.underline.link.markdown`) was not mapped at all and is Pi's `link` class.
    pub fn syntax_style_for_scope(&self, scope: &str) -> Option<Style> {
        // Most-specific prefixes first; the first match wins. `role` is `None` for the classes Pi
        // styles with an SGR attribute and no colour.
        let (role, modifier) = if scope.starts_with("comment") {
            (Some(("syntaxComment", "#6A9955", "#008000")), None)
        } else if scope.starts_with("string") {
            (Some(("syntaxString", "#CE9178", "#A31515")), None)
        } else if scope.starts_with("constant.numeric") {
            (Some(("syntaxNumber", "#B5CEA8", "#098658")), None)
        } else if scope.starts_with("entity.name.function") || scope.starts_with("support.function") {
            (Some(("syntaxFunction", "#DCDCAA", "#795E26")), None)
        } else if scope.starts_with("entity.name.type")
            || scope.starts_with("support.type")
            || scope.starts_with("support.class")
            || scope.starts_with("entity.name.class")
        {
            (Some(("syntaxType", "#4EC9B0", "#267F99")), None)
        } else if scope.starts_with("keyword.operator") {
            (Some(("syntaxOperator", "#D4D4D4", "#000000")), None)
        } else if scope.starts_with("keyword") || scope.starts_with("storage") {
            (Some(("syntaxKeyword", "#569CD6", "#0000FF")), None)
        } else if scope.starts_with("variable") || scope.starts_with("entity.other.attribute-name")
        {
            (Some(("syntaxVariable", "#9CDCFE", "#001080")), None)
        } else if scope.starts_with("punctuation") {
            (Some(("syntaxPunctuation", "#D4D4D4", "#000000")), None)
        } else if scope.starts_with("markup.inserted") {
            (Some(("toolDiffAdded", "#b5bd68", "#588458")), None)
        } else if scope.starts_with("markup.deleted") {
            (Some(("toolDiffRemoved", "#cc6666", "#aa5555")), None)
        } else if scope.starts_with("markup.italic") {
            // `emphasis: (s) => t.italic(s)` — `theme.ts:1140`. Attribute only, no `fg()`.
            (None, Some(Modifier::ITALIC))
        } else if scope.starts_with("markup.bold") {
            // `strong: (s) => t.bold(s)` — `theme.ts:1141`.
            (None, Some(Modifier::BOLD))
        } else if scope.starts_with("markup.underline") {
            // `link: (s) => t.underline(s)` — `theme.ts:1142`.
            (None, Some(Modifier::UNDERLINED))
        } else {
            return None;
        };
        let mut s = match role {
            Some((key, dark_hex, light_hex)) => self.role_style(key, dark_hex, light_hex),
            None => Style::default(),
        };
        if let Some(m) = modifier {
            s = s.add_modifier(m);
        }
        Some(s)
    }
}

/// The structured per-role **background** sub-theme (feature #3; Pi background tokens, theme.ts:48-55).
/// Every message/tool/selected background is a named field, so the whole background surface is
/// addressable at once instead of via ad-hoc flat-map string lookups. `None` ⇒ terminal default.
///
// T9 — `scrollbarThumb` is Pi's SEVENTH background token (`theme/theme.ts:50`), and unlike the
// other six it is `Type.Optional`, with an explicit `?? selectedBg` fallback applied twice: in
// `withThemeColorFallbacks` (`:330`) and again in the `Theme` constructor (`:365`).
//
// It was long recorded here as blocked on an unported DRAW surface. That reasoning confused the
// token with its painter. Its only *painting* consumer is indeed
// `scrollbarStyle: (text) => theme.bg("scrollbarThumb", text)` (`interactive-mode.ts:874`) on the
// `ScrollView` that exists solely as `fullscreenLayoutRoot`'s first child — but the token has a
// second, independent consumer in the THEME LOADER itself, and upstream tests exactly that with no
// ScrollView, no alt-screen renderer and no settings in sight:
//
// ```ts
// // test/scrollbar-theme.test.ts:31-38
// delete themeJson.colors.scrollbarThumb;
// const loadedTheme = loadThemeFromPath(writeTheme(themeJson), "truecolor");
// expect(loadedTheme.getBgAnsi("scrollbarThumb")).toBe(loadedTheme.getBgAnsi("selectedBg"));
// ```
//
// That resolution behaviour is portable today and is ported below — it is the exact idiom
// [`UiTheme::thinking`] already uses for `thinkingMax ?? thinkingXhigh`, the OTHER optional token,
// declared on the adjacent line of the same upstream function (`theme.ts:329`/`:330`).
//
// What remains unported is the PAINTER, and only the painter: a fullscreen/alt-screen renderer with
// a `ScrollView` (`pi/packages/tui/src/components/scroll-view.ts`) and the `fullscreenScrollbar`
// setting that gates it (`settings-manager.ts:136,1138-1146`). cyrup's interactive layout is one
// ratatui `Viewport::Inline` committing history to native scrollback, so nothing draws a thumb yet.
// When that lands it reads this field; until then the field still has to RESOLVE correctly, because
// a user theme that sets `scrollbarThumb` and one that omits it must agree, and a theme that omits
// it must not resolve to "no colour".
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
    /// Fullscreen scrollbar thumb fill (`scrollbarThumb`, `theme.ts:50`). OPTIONAL upstream, with
    /// `scrollbarThumb ?? selectedBg` applied by the loader (`:330`) and again by the `Theme`
    /// constructor (`:365`), so a theme that omits it resolves to its own `selectedBg` — NEVER to
    /// `None`. Same shape as `thinkingMax ?? thinkingXhigh` in [`ThinkingTheme`].
    pub scrollbar_thumb: Option<Color>,
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
    /// `xhigh` — extra-high reasoning.
    pub xhigh: Color,
    /// `max` — maximum reasoning. Falls back to [`ThinkingTheme::xhigh`] when the theme omits the
    /// optional `thinkingMax` token (Pi theme.ts:329).
    pub max: Color,
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
    let (light, dark) = setting?.split_once('/')?;
    // Reject a second slash (Pi: `indexOf("/", slashIndex+1) !== -1`).
    if dark.contains('/') {
        return None;
    }
    let (light, dark) = (light.trim(), dark.trim());
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

/// Port of `getLanguageFromPath` (v0.84.1 `modes/interactive/theme/theme.ts:1184-1250`) — the
/// extension→language table the `read` and `write` tool bodies syntax-highlight through
/// (`core/tools/read.ts:184`, `write.ts:151`).
///
/// Verbatim, including the shape: the extension is the LAST `.`-separated segment lower-cased
/// (`filePath.split(".").pop()?.toLowerCase()`), so a path with no dot at all still yields the whole
/// basename as "the extension" and simply misses the table, and a dotfile like `.gitignore` looks up
/// `gitignore`. `undefined` (here `None`) is "no language" — the caller then renders the body flat
/// in `toolOutput` rather than falling back to any auto-detection, which Pi does not do.
///
/// The values are Pi's own language NAMES, fed to `highlightCode`; cyrup feeds them to syntect's
/// `find_syntax_by_token`, which resolves the same words (`rust`, `typescript`, `bash`, …) and
/// returns `None` for the handful syntect's default set does not ship (`fish`, `hcl`, `protobuf`),
/// leaving those flat — the same end state as Pi's `try`/`catch` fallback in `highlightCode`.
pub(crate) fn language_from_path(file_path: &str) -> Option<&'static str> {
    let ext = file_path.rsplit('.').next()?.to_ascii_lowercase();
    let lang = match ext.as_str() {
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "sh" | "bash" | "zsh" => "bash",
        "fish" => "fish",
        "ps1" => "powershell",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "sass" => "sass",
        "less" => "less",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" => "xml",
        "md" | "markdown" => "markdown",
        "dockerfile" => "dockerfile",
        "makefile" => "makefile",
        "cmake" => "cmake",
        "lua" => "lua",
        "perl" => "perl",
        "r" => "r",
        "scala" => "scala",
        "clj" => "clojure",
        "ex" | "exs" => "elixir",
        "erl" => "erlang",
        "hs" => "haskell",
        "ml" => "ocaml",
        "vim" => "vim",
        "graphql" => "graphql",
        "proto" => "protobuf",
        "tf" | "hcl" => "hcl",
        _ => return None,
    };
    Some(lang)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// PROV-002: the `max` rung needs its own editor border color. The built-ins define
    /// `thinkingMax` (Pi dark.json `#ff5fff` / light.json `#af005f`), so it must be distinct from
    /// `xhigh` and `thinking_border_style("max")` must resolve it rather than fall through to the
    /// neutral border.
    #[test]
    fn thinking_max_has_its_own_border_color_in_the_builtins() {
        for theme in [UiTheme::dark(), UiTheme::light()] {
            let t = theme.thinking();
            assert_ne!(
                t.max, t.xhigh,
                "`{}` must give `max` its own color",
                theme.name
            );
            assert_eq!(theme.thinking_border_style("max").fg, Some(t.max));
            assert_ne!(
                theme.thinking_border_style("max"),
                theme.border_style(),
                "`max` must not fall through to the neutral border"
            );
        }
    }

    /// Pi made `thinkingMax` an OPTIONAL theme token with a `?? thinkingXhigh` fallback
    /// (theme.ts:93,329,358) so themes authored before the rung existed keep working. Ported:
    /// a theme without the token reuses its OWN `xhigh` color. (Pi's own regression test is
    /// `coding-agent/test/max-thinking.test.ts`, "falls back to thinkingXhigh for legacy themes".)
    #[test]
    fn legacy_theme_without_thinking_max_falls_back_to_xhigh() {
        let mut legacy = UiTheme::dark();
        legacy.roles.remove("thinkingMax");
        let t = legacy.thinking();
        assert_eq!(t.max, t.xhigh, "legacy themes reuse their xhigh color");
        assert_eq!(
            legacy.thinking_border_style("max"),
            legacy.thinking_border_style("xhigh")
        );
        // A genuinely unknown level resolves to `thinkingOff`, which is Pi's `default:` arm in
        // `Theme.getThinkingBorderColor` (v0.84.1
        // `coding-agent/src/modes/interactive/theme/theme.ts:437-438` —
        // `default: return (str) => this.fg("thinkingOff", str)`). It used to fall through to the
        // `border` role, a token Pi never reaches from here.
        assert_eq!(
            legacy.thinking_border_style("ultra"),
            legacy.thinking_border_style("off")
        );
        assert_ne!(legacy.thinking_border_style("ultra"), legacy.border_style());
    }

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

//! Theme application (R-10-025/026/027; arch-10 §3.8).
//!
//! `cyrup-resources` owns themes on disk (parsing, `vars`, hot-reload via `ThemeWatcher`). This
//! module is the *render-facing* projection: it maps the resolved color roles
//! (`cyrup_resources::theme::ResolvedTheme` / `ColorSpec`) onto `ratatui::style::Color` and exposes
//! the per-component `Style`s the widgets read. A `generation` counter is bumped on every
//! hot-reload so render caches can be invalidated (R-10-026).

use std::collections::{BTreeMap, BTreeSet};

use cyrup_resources::ColorValue;
use cyrup_resources::theme::{Appearance, ColorSpec, ResolvedTheme, ThemeData, builtin_themes};
use ratatui::style::{Color, Modifier, Style};

use crate::system_theme::{
    SYSTEM_THEME_NAME, Saturation, SystemColor, SystemThemeInput, generate_system_theme_colors,
    terminal_appearance,
};
use crate::terminal_query::{COLOR_QUERY_TIMEOUT, LateColors, TerminalColors, TerminalProbe};

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
    /// `true_color`, so a `|| false` probe is passed for it and the `tmux display-message`
    /// subprocess is skipped.
    ///
    /// It goes through [`crate::image::detect_capabilities_with_overrides`] rather than the bare
    /// sniff so `CYRUP_TRUE_COLOR` reaches the colour depth: upstream reads this as
    /// `getCapabilities().trueColor`
    /// (`coding-agent/src/modes/interactive/theme/theme.ts:630`), and `getCapabilities` is the
    /// override-layered accessor (`tui/src/terminal-image.ts:164-172`).
    ///
    /// CFG-090 — a `terminal.trueColor` settings override
    /// ([`crate::image::set_capability_overrides`]) is spread over that, as pi's `getCapabilities()`
    /// spreads `capabilityOverrides` over `detectCapabilities()` (`terminal-image.ts:160-169`
    /// @v0.87.1).
    pub fn detect() -> ColorMode {
        match crate::image::capability_overrides().true_color {
            Some(true_color) => ColorMode::from_true_color(true_color),
            None => ColorMode::detect_from(|k| std::env::var(k).ok()),
        }
    }

    /// Pi `getCapabilities().trueColor ? "truecolor" : "256color"` (`theme.ts:529` @v0.87.1).
    pub fn from_true_color(true_color: bool) -> ColorMode {
        if true_color {
            ColorMode::TrueColor
        } else {
            ColorMode::Ansi256
        }
    }

    /// The pure core of [`ColorMode::detect`], parameterised over an environment lookup so both
    /// arms are deterministically testable (same shape as `detect_capabilities_from`).
    pub fn detect_from(env: impl Fn(&str) -> Option<String>) -> ColorMode {
        // Pi `createTheme` falls back to `"256color"` when truecolor is unavailable
        // (v0.84.1 theme.ts:611). There is no lower rung upstream.
        ColorMode::from_true_color(
            crate::image::detect_capabilities_with_overrides(env, || false).true_color,
        )
    }

    /// Project one `ratatui::Color` into this mode. Only `Color::Rgb` is transformed (named/indexed
    /// colors are already depth-safe); the transform is the single **style-projection boundary** the
    /// whole TUI passes its role colors through (mirrors Pi `fgAnsi`/`bgAnsi`, `theme.ts:260-288`).
    pub fn project(self, color: Color) -> Color {
        let Color::Rgb(r, g, b) = color else {
            return color;
        };
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
    /// The background the theme itself is designed for: declared in its JSON, detected from its
    /// colours, or — for the system theme — decided by the terminal's reported colours (Pi
    /// `Theme.ownAppearance`, `theme.ts:256`, `:306`). `None` for a theme with no concrete colour.
    own_appearance: Option<Appearance>,
    /// Pi `getTerminalTheme()` (`theme.ts:713`), stamped on by the [`ThemeController`]: what
    /// [`UiTheme::appearance`] answers for a theme that has no appearance of its own.
    terminal_appearance: Appearance,
    /// Foreground roles rendered faint (SGR 2) on top of their colour — the system theme's neutral
    /// tokens when the terminal reported nothing (Pi `Theme.dimTokens`, `theme.ts:255`, `:361`).
    dim_roles: BTreeSet<String>,
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

    /// Build a theme from already-resolved role colours — the one place the eight direct fields
    /// (`foreground`, `accent`, …) are read off the role map, so a built-in, a file theme and the
    /// generated system theme cannot disagree about which role feeds which.
    fn from_roles(
        name: String,
        roles: BTreeMap<String, Color>,
        own_appearance: Option<Appearance>,
        dim_roles: BTreeSet<String>,
        generation: u64,
    ) -> Self {
        let role = |key: &str| roles.get(key).copied();
        UiTheme {
            name,
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
            own_appearance,
            terminal_appearance: Appearance::Dark,
            dim_roles,
            roles,
        }
    }

    /// Project a `ResolvedTheme` (color roles already resolved through `vars`) into a `UiTheme`.
    ///
    /// A role the theme sets to `""` is present in the map as [`Color::Reset`] — the terminal's
    /// default foreground or background, which Pi draws with `\x1b[39m` / `\x1b[49m`
    /// (`Theme.addToken`, `theme.ts:290-294`) — and NOT absent: an absent role falls back to a
    /// compiled hex, which is a different colour.
    pub fn from_resolved(
        name: impl Into<String>,
        resolved: &ResolvedTheme,
        generation: u64,
    ) -> Self {
        let roles = resolved
            .roles
            .iter()
            .map(|(k, spec)| (k.clone(), color_of(*spec)))
            .collect();
        UiTheme::from_roles(
            name.into(),
            roles,
            resolved.appearance,
            BTreeSet::new(),
            generation,
        )
    }

    /// A compiled-in theme by name — `"system"`, `"dark"` or `"light"`. An unknown name is the
    /// system theme, as it is for pi's `applyThemeName` (`theme-controller.ts:180`) so this is total
    /// and never panics (R-00-009, R-10-027).
    ///
    /// Callers that must DISTINGUISH "the name is unknown" from "the name is `dark`" — the theme
    /// load path, which owes the user pi's `Failed to load theme …` sentence (TUI-096) — ask
    /// [`UiTheme::builtin_named`] instead; this stays the total, silent projection every render
    /// site uses.
    pub fn builtin(name: &str) -> Self {
        UiTheme::builtin_named(name)
            .unwrap_or_else(|| UiTheme::system(&SystemThemeInput::default()))
    }

    /// The compiled-in built-in with this exact name, or `None` when there is none.
    ///
    /// This is pi's `loadThemeJson` built-in lookup, `if (name in builtinThemes)`
    /// (`modes/interactive/theme/theme.ts:551-554` @v1.0.0), split out from [`UiTheme::builtin`]
    /// so a failed lookup is a value rather than a silent repaint: upstream's miss continues
    /// on to the registered themes and finally `throw new Error(\`Theme not found: ${name}\`)`
    /// (`:567`), which `setTheme` catches into the `{success: false, error}` the controller reports.
    ///
    /// `"system"` is a built-in too (`loadTheme`, `theme.ts:633`: "The system theme name is
    /// reserved: it takes precedence over custom themes of the same name"). It is generated from the
    /// terminal's colours, which this lookup does not know: the answer here is the theme for a
    /// terminal that reported nothing. A caller that holds the reported colours — the
    /// [`ThemeController`] — generates it with [`UiTheme::system`] instead.
    pub fn builtin_named(name: &str) -> Option<Self> {
        if name == SYSTEM_THEME_NAME {
            return Some(UiTheme::system(&SystemThemeInput::default()));
        }
        builtin_themes().into_iter().find_map(|theme| {
            (theme.key.as_str() == name).then(|| {
                let resolved = theme.resolve();
                UiTheme::from_resolved(theme.data.name.clone(), &resolved, 0)
            })
        })
    }

    /// The `system` theme generated from what the terminal reported — Pi `createSystemTheme`
    /// (`theme.ts:611-623`): every token's colour is derived from the terminal's own colours, in one
    /// of three tiers depending on what it reported (see [`crate::system_theme`]).
    pub fn system(input: &SystemThemeInput) -> Self {
        let generated = generate_system_theme_colors(input);
        let roles = generated
            .colors
            .iter()
            .map(|(token, color)| {
                let color = match color {
                    SystemColor::Rgb(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
                    SystemColor::Indexed(index) => Color::Indexed(*index),
                    SystemColor::Default => Color::Reset,
                };
                ((*token).to_string(), color)
            })
            .collect();
        let dim = generated.dim.iter().map(|t| (*t).to_string()).collect();
        UiTheme::from_roles(
            SYSTEM_THEME_NAME.to_string(),
            roles,
            generated.appearance,
            dim,
            0,
        )
    }

    /// The background this theme is designed for — Pi `Theme.appearance` (`theme.ts:313`): declared,
    /// detected from its colours, or, for a theme with no concrete colour, the terminal's.
    #[must_use]
    pub fn appearance(&self) -> Appearance {
        self.own_appearance.unwrap_or(self.terminal_appearance)
    }

    /// Record the terminal's appearance, the answer for a theme that has none of its own.
    #[must_use]
    pub fn with_terminal_appearance(mut self, terminal: Appearance) -> Self {
        self.terminal_appearance = terminal;
        self
    }

    /// The compiled-in `dark` theme (Pi `dark.json` @v1.0.0: text `okhsl(234 3% 89%)`, accent
    /// `okhsl(295 50% 67%)`, error `okhsl(20 72% 67%)`).
    pub fn dark() -> Self {
        UiTheme::builtin_or_static(
            "dark",
            Appearance::Dark,
            Color::Rgb(222, 224, 225),
            Color::Rgb(167, 152, 215),
            Color::Rgb(234, 127, 129),
        )
    }

    /// The compiled-in `light` theme (Pi `light.json` @v1.0.0: text `okhsl(225 5% 27%)`, accent
    /// `okhsl(295 60% 46%)`, error `okhsl(20 91% 47%)`).
    pub fn light() -> Self {
        UiTheme::builtin_or_static(
            "light",
            Appearance::Light,
            Color::Rgb(59, 63, 65),
            Color::Rgb(116, 89, 180),
            Color::Rgb(200, 37, 61),
        )
    }

    /// Look up a built-in by name, or synthesize a minimal palette from the given Pi `text`/`accent`/
    /// `error` colors if the resource layer somehow cannot supply it (keeps zero-disk-I/O
    /// availability, R-10-027). Background stays terminal-default (Pi has no global background token).
    fn builtin_or_static(
        name: &str,
        appearance: Appearance,
        text: Color,
        accent: Color,
        error: Color,
    ) -> Self {
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
            roles: BTreeMap::new(),
            own_appearance: Some(appearance),
            terminal_appearance: appearance,
            dim_roles: BTreeSet::new(),
        }
    }

    /// Project freshly-watched [`ThemeData`] (e.g. from `cyrup_resources::theme::ThemeWatcher`)
    /// into a `UiTheme` for hot-reload (R-10-026). Resolves exactly as [`Theme::resolve`] does —
    /// recursively through `vars`, with the same colour grammar — so the watcher's `Arc<ThemeData>`
    /// can be applied without first reconstructing a `Theme`.
    ///
    /// [`Theme::resolve`]: cyrup_resources::Theme::resolve
    pub fn from_theme_data(data: &ThemeData, generation: u64) -> Self {
        UiTheme::from_resolved(data.name.clone(), &data.resolve(), generation)
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

    /// Secondary/hint chrome — Pi's **`dim` token**, colour only (v1.0.0 `dark.json` `dim` =
    /// `okhsl(229 8% 56%)` = `#7e888e`; `light.json` `okhsl(229 7% 59%)` = `#879095`).
    ///
    /// T1 (TUI-FIDELITY §2): this used to resolve the `text` role and add `Modifier::DIM`, which is
    /// wrong twice over. Pi renders every hint through `theme.fg("dim", …)` (e.g.
    /// `theme.ts:1312`/`:1314` in `getSettingsListTheme`), and `fg()` (`theme.ts:372-376`) emits a
    /// plain foreground escape with **no SGR attribute** — so cyrup was painting body-bright text
    /// plus SGR 2, which terminals that ignore SGR 2 (Terminal.app, much of tmux, Windows consoles)
    /// render at full brightness, and which in the *light* theme came out near-black `#1f2328`
    /// where Pi draws grey.
    pub fn dim_style(&self) -> Style {
        self.role_style("dim", "#7e888e", "#879095")
    }

    /// Style for the user's own messages (bold accent label).
    pub fn user_style(&self) -> Style {
        Style::default()
            .fg(self.accent.unwrap_or(Color::Cyan))
            .add_modifier(Modifier::BOLD)
    }

    /// Style for assistant message text.
    pub fn assistant_style(&self) -> Style {
        self.base_style()
    }

    /// Muted style (descriptions, scroll indicators, hints, footer body) — Pi `muted` (theme.ts:543).
    ///
    /// The `muted` token is `#9da5a9` in v1.0.0's `dark.json` and `#677176` in `light.json`; it is a
    /// *different* token from `dim`, so a theme that omits it falls back to its own palette's value
    /// rather than to [`Self::dim_style`]'s.
    pub fn muted_style(&self) -> Style {
        match self.muted {
            Some(c) => Style::default()
                .fg(c)
                .add_modifier(self.faint_modifier("muted")),
            None => self.role_style("muted", "#9da5a9", "#677176"),
        }
    }

    /// `Modifier::DIM` (SGR 2) when the theme renders `key` faint, else nothing — Pi's `fg()` for a
    /// `dimTokens` member emits `\x1b[2m` after the colour (`theme.ts:361-366`). Only the system
    /// theme, generated for a terminal that reported no colours, has any.
    fn faint_modifier(&self, key: &str) -> Modifier {
        if self.dim_roles.contains(key) {
            Modifier::DIM
        } else {
            Modifier::empty()
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
    /// It is a distinct colour from `accent` in both built-ins (v1.0.0 `dark.json`: `borderAccent`
    /// `#a08ed5` vs `accent` `#a798d7`; `light.json`: `#8a72cb` vs `#7459b4`), so the fallback chain
    /// goes to `border` before `accent` rather than collapsing onto the accent role.
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
        let faint = match level {
            "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => Modifier::empty(),
            _ => self.faint_modifier("thinkingOff"),
        };
        Style::default().fg(color).add_modifier(faint)
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
        Style::default()
            .fg(fg)
            .add_modifier(self.faint_modifier("borderMuted"))
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
        }
    }

    /// The fullscreen scrollbar's TRACK foreground — pi's optional `scrollbarTrack` token, which
    /// the loader and the `Theme` constructor both default with `scrollbarTrack ?? muted`
    /// (`theme.ts:173`, `:281` @v1.0.0). The fallback is this theme's OWN resolved `muted`, never a
    /// hardcoded colour — exactly as `thinkingMax` falls back to `thinkingXhigh`. `None` is the
    /// terminal's default foreground. TUI-110.
    pub fn scrollbar_track(&self) -> Option<Color> {
        self.roles.get("scrollbarTrack").copied().or(self.muted)
    }

    /// The fullscreen scrollbar's THUMB foreground — `scrollbarThumb ?? text` (`theme.ts:174`,
    /// `:282` @v1.0.0). Both scrollbar tokens are FOREGROUND colours painted with
    /// `theme.fg(…)` (`interactive-mode.ts:966-967`), not backgrounds. TUI-110.
    pub fn scrollbar_thumb(&self) -> Option<Color> {
        self.roles
            .get("scrollbarThumb")
            .copied()
            .or(self.foreground)
    }

    /// The structured **thinking-border** sub-theme (Pi `thinking{Off..Xhigh}`, interactive-mode.ts:
    /// 3533-3541): the escalating per-reasoning-level editor rule color, one typed field per level,
    /// each resolved from the live theme with the spec/tui/03 §3.3 dark-hex fallback so it is total.
    pub fn thinking(&self) -> ThinkingTheme {
        // Fallback pairs: the v1.0.0 `dark.json` / `light.json` values of the six level tokens.
        let level = |key: &str, dark_hex: &str, light_hex: &str| {
            self.role_color_themed(key, dark_hex, light_hex)
        };
        let xhigh = level("thinkingXhigh", "#de54c1", "#e585cd");
        ThinkingTheme {
            off: level("thinkingOff", "#6c767b", "#c2c8ca"),
            minimal: level("thinkingMinimal", "#68808d", "#b5c4cb"),
            low: level("thinkingLow", "#5489a4", "#9fc2d5"),
            medium: level("thinkingMedium", "#6185cc", "#a2b7e0"),
            high: level("thinkingHigh", "#9776e5", "#b5a5e8"),
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
        let fg = self
            .roles
            .get("userMessageText")
            .copied()
            .or(self.foreground);
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
            Some(c) => Style::default()
                .fg(c)
                .add_modifier(self.faint_modifier("customMessageText")),
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
            Some(c) => Style::default()
                .fg(c)
                .add_modifier(self.faint_modifier("toolOutput")),
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
                .project_opt(parse_color(default_hex))
                .unwrap_or(Color::Reset),
        }
    }

    /// Whether this palette is a **light** one, i.e. one that draws dark glyphs on a light ground.
    ///
    /// Only consulted to pick between the dark and light members of a hex fallback pair
    /// ([`Self::palette_hex`]); a theme that actually defines the role never reaches it. The answer
    /// is the theme's [`appearance`](Self::appearance) — the `appearance` its JSON declares, else
    /// the lightness average of its own colours with palette indices 0-15 excluded (TUI-132). A
    /// declared `"appearance": "light"` therefore wins over colours that average dark, and
    /// quantizing a role to a `Color::Indexed` cannot change the answer.
    fn is_light_palette(&self) -> bool {
        self.appearance() == Appearance::Light
    }

    /// Pick the member of a `(dark, light)` hex-fallback pair that matches this palette.
    fn palette_hex<'a>(&self, dark_hex: &'a str, light_hex: &'a str) -> &'a str {
        if self.is_light_palette() {
            light_hex
        } else {
            dark_hex
        }
    }

    /// [`Self::role_color`] with a **theme-aware** hex fallback pair.
    fn role_color_themed(&self, key: &str, dark_hex: &str, light_hex: &str) -> Color {
        self.role_color(key, self.palette_hex(dark_hex, light_hex))
    }

    /// `fg`-only style for a role with a `(dark, light)` hex fallback pair.
    ///
    /// The hexes are a last-resort value for the synthetic fallback theme (the one
    /// `builtin_or_static` synthesizes when the resource layer cannot supply a palette at all); any
    /// real theme resolves the role through [`Self::role_color`]. Every fallback is a pair — the
    /// role's resolved value in v1.0.0's `dark.json` and in its `light.json` — so the degraded theme
    /// draws the same palette as the normal one, and a resource-less light theme does not fall
    /// through to dark greys. [`Self::palette_hex`] picks the member by the theme's appearance.
    fn role_style(&self, key: &str, dark_hex: &str, light_hex: &str) -> Style {
        Style::default()
            .fg(self.role_color_themed(key, dark_hex, light_hex))
            .add_modifier(self.faint_modifier(key))
    }

    /// Markdown heading — `mdHeading`, bold (`markdown.ts:336-362`).
    pub fn md_heading_style(&self) -> Style {
        self.role_style("mdHeading", "#cd9a22", "#8f6802")
            .add_modifier(Modifier::BOLD)
    }
    /// Inline code span — `mdCode` (= accent), no backticks (`markdown.ts:512-516`).
    pub fn md_code_style(&self) -> Style {
        Style::default().fg(self
            .roles
            .get("mdCode")
            .copied()
            .or(self.accent)
            .unwrap_or(Color::Cyan))
    }
    /// Flat (unknown-language) fenced-code body — `mdCodeBlock` (`markdown.ts:378-398`).
    pub fn md_code_block_style(&self) -> Style {
        self.role_style("mdCodeBlock", "#68b78d", "#337e58")
    }
    /// Fence border lines (```` ``` ````) — `mdCodeBlockBorder` (`markdown.ts:380,393`).
    pub fn md_code_block_border_style(&self) -> Style {
        self.role_style("mdCodeBlockBorder", "#9da5a9", "#677176")
    }
    /// Assistant **reasoning** (thinking) body — `thinkingText`, italic (Pi
    /// `assistant-message.ts:145-165` renders each run of `thinking` blocks as one Markdown section
    /// with `{color: theme.fg("thinkingText", …), italic: true}`; the collapsed
    /// `hideThinkingBlock` label at `:139-143` uses the same role). `thinkingText` is `gray`
    /// (`#96a0a4`) in Pi's v1.0.0 `dark.json` and `#7c868c` in `light.json`; the hex default here
    /// is used only when the live theme omits the role.
    ///
    /// NOTE this is a different thing from [`ThinkingTheme`], which is the per-reasoning-**level**
    /// editor-border palette (`thinkingOff`…`thinkingXhigh`).
    pub fn thinking_text_style(&self) -> Style {
        self.role_style("thinkingText", "#96a0a4", "#7c868c")
            .add_modifier(Modifier::ITALIC)
    }
    /// Blockquote body — `mdQuote`, italic (`markdown.ts:414-461`).
    pub fn md_quote_style(&self) -> Style {
        self.role_style("mdQuote", "#9da5a9", "#677176")
            .add_modifier(Modifier::ITALIC)
    }
    /// Blockquote `│ ` border — `mdQuoteBorder` (`markdown.ts:414-461`).
    pub fn md_quote_border_style(&self) -> Style {
        self.role_style("mdQuoteBorder", "#9da5a9", "#677176")
    }
    /// Horizontal rule — `mdHr` (`markdown.ts:463-468`).
    pub fn md_hr_style(&self) -> Style {
        self.role_style("mdHr", "#9da5a9", "#677176")
    }
    /// List bullet marker — `mdListBullet` (`markdown.ts:604-654`).
    ///
    /// `mdListBullet` is `accent` in both v1.0.0 palettes (`dark.json` / `light.json`
    /// `"mdListBullet": "accent"`), so a theme that omits it takes its own accent.
    pub fn md_list_bullet_style(&self) -> Style {
        match self.roles.get("mdListBullet").copied() {
            Some(c) => Style::default().fg(c),
            None => Style::default().fg(self.accent.unwrap_or(Color::Cyan)),
        }
    }
    /// Link text — `mdLink`, underlined (`markdown.ts:537-556`).
    pub fn md_link_style(&self) -> Style {
        self.role_style("mdLink", "#69add0", "#2f7899")
            .add_modifier(Modifier::UNDERLINED)
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
        self.role_style("mdLinkUrl", "#9da5a9", "#677176")
    }

    /// Diff added (`+`) line — `toolDiffAdded`, green (`diff.ts` `theme.fg("toolDiffAdded")`).
    pub fn tool_diff_added_style(&self) -> Style {
        self.role_style("toolDiffAdded", "#68b78d", "#337e58")
    }
    /// Diff removed (`-`) line — `toolDiffRemoved`, red.
    pub fn tool_diff_removed_style(&self) -> Style {
        self.role_style("toolDiffRemoved", "#ea7f81", "#c8253d")
    }
    /// Diff context (unchanged) line — `toolDiffContext`, gray.
    pub fn tool_diff_context_style(&self) -> Style {
        self.role_style("toolDiffContext", "#9da5a9", "#677176")
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
            (Some(("syntaxComment", "#9da5a9", "#677176")), None)
        } else if scope.starts_with("string") {
            (Some(("syntaxString", "#de8d5a", "#a45417")), None)
        } else if scope.starts_with("constant.numeric") {
            (Some(("syntaxNumber", "#68b78d", "#337e58")), None)
        } else if scope.starts_with("entity.name.function") || scope.starts_with("support.function")
        {
            (Some(("syntaxFunction", "#cd9a22", "#8f6802")), None)
        } else if scope.starts_with("entity.name.type")
            || scope.starts_with("support.type")
            || scope.starts_with("support.class")
            || scope.starts_with("entity.name.class")
        {
            (Some(("syntaxType", "#a798d7", "#7459b4")), None)
        } else if scope.starts_with("keyword.operator") {
            (Some(("syntaxOperator", "#9da5a9", "#677176")), None)
        } else if scope.starts_with("keyword") || scope.starts_with("storage") {
            (Some(("syntaxKeyword", "#69add0", "#2f7899")), None)
        } else if scope.starts_with("variable") || scope.starts_with("entity.other.attribute-name")
        {
            (Some(("syntaxVariable", "#5db3ba", "#287a81")), None)
        } else if scope.starts_with("punctuation") {
            (Some(("syntaxPunctuation", "#9da5a9", "#677176")), None)
        } else if scope.starts_with("markup.inserted") {
            (Some(("toolDiffAdded", "#68b78d", "#337e58")), None)
        } else if scope.starts_with("markup.deleted") {
            (Some(("toolDiffRemoved", "#ea7f81", "#c8253d")), None)
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
    /// `xhigh` — extra-high reasoning.
    pub xhigh: Color,
    /// `max` — maximum reasoning. Falls back to [`ThinkingTheme::xhigh`] when the theme omits the
    /// optional `thinkingMax` token (Pi theme.ts:329).
    pub max: Color,
}

/// Map a resolved color role onto a `ratatui::Color`.
///
/// `Inherit` — a theme's `""` — is [`Color::Reset`], the terminal's own default foreground or
/// background. A palette index stays [`Color::Indexed`], so indices 0-15 keep following the user's
/// terminal palette instead of becoming the standard xterm RGB for them.
pub fn color_of(spec: ColorSpec) -> Color {
    match spec {
        ColorSpec::Inherit => Color::Reset,
        ColorSpec::Rgb { r, g, b } => Color::Rgb(r, g, b),
        ColorSpec::Indexed(index) => Color::Indexed(index),
    }
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
        let (dr, dg, db) = (
            (r - cr as i32) as i64,
            (g - cg as i32) as i64,
            (b - cb as i32) as i64,
        );
        let d = dr * dr * 299 + dg * dg * 587 + db * db * 114;
        if d < best_d {
            best_d = d;
            best = i as u8;
        }
    }
    best
}

/// Parse a theme colour VALUE into a `ratatui::Color`; anything malformed ⇒ `None`.
///
/// TUI-131 — this was `parse_hex` and took `#rrggbb` / `rrggbb` / `#rgb` only. It now delegates to
/// [`cyrup_resources::color::parse_color`], the port of pi's `parseColor`
/// (`packages/tui/src/colors.ts:121` @v1.0.0), so `oklch(…)` and `okhsl(…)` resolve. pi 1.0's own
/// `dark.json` and `light.json` are written entirely in OKHSL, so before this every token of a
/// pi 1.0 theme fell back to a compiled hex with no error and no unresolved-role report.
///
/// One function for both resolvers: `cyrup-resources` owns the maths (it is the lower crate) and
/// this crate wraps it in `ratatui`'s colour type at the boundary.
fn parse_color(s: &str) -> Option<Color> {
    cyrup_resources::color::parse_color(s).map(|(r, g, b)| Color::Rgb(r, g, b))
}

// ============================================================================
// ThemeController (Pi theme-controller.ts + theme.ts theme-resolution)
// ============================================================================

/// Whether the terminal is dark or light (Pi `TerminalTheme`, `theme.ts:650`), used to resolve an
/// automatic `light/dark` theme setting and as the appearance of a theme that has none of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TerminalTheme {
    /// A light terminal background.
    Light,
    /// A dark terminal background — what pi assumes when nothing says otherwise.
    #[default]
    Dark,
}

impl TerminalTheme {
    /// The built-in theme named for this polarity.
    pub fn theme_name(self) -> &'static str {
        match self {
            TerminalTheme::Light => "light",
            TerminalTheme::Dark => "dark",
        }
    }

    /// The polarity the environment alone suggests: `COLORFGBG`, else dark (Pi's
    /// `detectColorFgBgTheme(env) ?? "dark"`, `theme.ts:709`). Before the terminal has answered its
    /// colour query this is all there is to go on.
    pub fn detect() -> TerminalTheme {
        detect_color_fg_bg_theme(&std::env::var("COLORFGBG").unwrap_or_default())
            .unwrap_or_default()
    }
}

impl From<TerminalTheme> for Appearance {
    fn from(theme: TerminalTheme) -> Self {
        match theme {
            TerminalTheme::Light => Appearance::Light,
            TerminalTheme::Dark => Appearance::Dark,
        }
    }
}

impl From<Appearance> for TerminalTheme {
    fn from(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => TerminalTheme::Light,
            Appearance::Dark => TerminalTheme::Dark,
        }
    }
}

/// Pi `detectColorFgBgTheme` (`theme.ts:689-695`): dark or light from the `COLORFGBG` environment
/// variable some terminals set, or `None` without a usable background index. The value is `fg;bg`
/// or `fg;xpm;bg` (rxvt), where a field is an ANSI colour index or `default` when the colour is not
/// in the palette. The index refers to the terminal's own palette, whose colours are unknown here,
/// so it is classified by index like Vim does: 0-6 and 8 (bright black, e.g. Solarized Dark's
/// background) are dark, 7 and 9-15 are light. The classification itself is
/// [`Appearance::from_colorfgbg`], which a headless HTML export shares.
pub fn detect_color_fg_bg_theme(colorfgbg: &str) -> Option<TerminalTheme> {
    Appearance::from_colorfgbg(colorfgbg).map(TerminalTheme::from)
}

/// Pi `detectTerminalTheme` (`theme.ts:702-710`): whether the terminal is dark or light. The
/// background it renders decides, classified the same way the system theme does
/// ([`terminal_appearance`]). Without a reported background: the terminal's light/dark report, then
/// `COLORFGBG`, then dark.
pub fn detect_terminal_theme(
    colors: &TerminalColors,
    reported_scheme: Option<TerminalTheme>,
    colorfgbg: Option<TerminalTheme>,
) -> TerminalTheme {
    match colors.background {
        Some(background) => terminal_appearance(background, colors.foreground).into(),
        None => reported_scheme.or(colorfgbg).unwrap_or_default(),
    }
}

/// Pi `parseAutoThemeSetting` (`theme.ts:652-667`): a `"<light>/<dark>"` setting with exactly one
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

/// Pi `resolveThemeSetting` (`theme.ts:669-680`): resolve the raw `settings.theme` value against the
/// detected `terminal` polarity into a concrete theme name. An `auto` (`light/dark`) setting picks the
/// arm matching `terminal`; a bare name passes through; any other slash-namespaced value ⇒ `None`
/// (unresolvable → the caller falls back to the system theme).
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

/// The outcome of one theme (re-)application — pi's `ThemeResult`
/// (`modes/interactive/theme/theme-controller.ts:18` @v1.0.0), widened by the one case upstream
/// cannot have: an app that was never handed a controller.
///
/// Returned by [`crate::App::reapply_theme_from_settings`] so the failure is a VALUE and not a
/// silent repaint (TUI-096). Upstream's `applyThemeName` reacts to the same result twice
/// (`:178-186`): it seats `"system"` as the active name, and, under `showError`, surfaces
/// ``Failed to load theme "<name>": <error>\nFell back to the system theme.``
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeApply {
    /// No controller was booted — a harness `App`, which has no `settings.theme` to answer from and
    /// keeps the theme it was constructed with. Upstream has no counterpart: its controller is a
    /// field of the interactive mode and always exists.
    NoController,
    /// The named theme resolved and is now painted; `active_name()` is that name
    /// (`this.activeThemeName = result.success ? themeName : SYSTEM_THEME_NAME`, `:180`).
    Loaded(String),
    /// The named theme did NOT resolve: the system theme is painted AND seated as the active name,
    /// and the notice was pushed to the transcript. `error` is pi's `result.error` — for a name that
    /// resolves to nothing, `loadThemeJson`'s `Theme not found: <name>`.
    FellBackToSystem {
        /// The name `settings.theme` asked for, as asked for.
        name: String,
        /// pi's `result.error` for that failure.
        error: String,
    },
}

/// The sentence a failed theme load ends with — pi's `applyThemeName` (`theme-controller.ts:183`).
pub const THEME_FALLBACK_SENTENCE: &str = "Fell back to the system theme.";

/// The theme state other threads read: an extension asking for the `system` theme, and an export
/// of the session. Computed from the controller when its [`ThemeController::generation`] moves and
/// published to [`crate::theme_access::TuiThemeAccess`], which is read off the run loop.
#[derive(Clone, Debug)]
pub(crate) struct ThemePublication {
    /// The controller generation this was computed for.
    pub(crate) generation: u64,
    /// The generated `system` theme as a theme document: hex colours, palette indices and `""`, the
    /// shape a file theme has.
    pub(crate) system_document: ThemeData,
    /// The `system` theme as an HTML export renders it: every token concrete.
    pub(crate) system_export: cyrup_session_svc::ExportTheme,
    /// The terminal's reported defaults, for exporting a theme that sets tokens to `""`.
    pub(crate) terminal_defaults: cyrup_session_svc::TerminalDefaults,
}

/// The boot + live-switch owner of the render theme (Pi `InteractiveThemeController`,
/// `theme-controller.ts` @v1.0.0). It resolves the theme from `settings.theme` — `system` when the
/// setting names nothing that resolves — carries the [`ColorMode`] so every projected [`UiTheme`] is
/// depth-correct, and holds everything the terminal reported about its own colours, which is what
/// the `system` theme is generated from.
///
/// The system theme renders in grayscale until the terminal's colours arrive (Pi
/// `markTerminalColorsPending`, `theme.ts:211`): the controller starts in that state and
/// [`Self::apply_terminal_colors`] ends it, whether the terminal answered or the query timed out.
///
/// [CYRUP-DELTA] vs `waitForTerminalColors` (`theme-controller.ts:116`, awaited at
/// `interactive-mode.ts:993` before the header and the startup notices are built, because they bake
/// theme colours into their text): cyrup's boot query is synchronous and bounded by the same 100 ms
/// ([`Self::request_terminal_colors`]), and runs before anything that bakes a colour is built, so
/// the gate is the order of the calls rather than a promise. A reply that misses the deadline
/// arrives through the late-reply callback and re-applies exactly as pi's `onLateReply` does.
#[derive(Clone, Debug)]
pub struct ThemeController {
    color_mode: ColorMode,
    /// What the environment says about the terminal (`COLORFGBG`), the polarity used until the
    /// terminal reports a background.
    env_theme: Option<TerminalTheme>,
    /// Pi's module-level `terminalColors` (`theme.ts:190`): replaced, never mutated.
    terminal_colors: TerminalColors,
    /// Pi's `terminalColorsPending` (`theme.ts:192`).
    colors_pending: bool,
    /// Pi's controller field `terminalColors` (`theme-controller.ts:65`): the last REPORTED colours,
    /// kept when a later query times out instead of being erased. `None` until the first apply.
    reported: Option<TerminalColors>,
    /// Pi's `terminalColorScheme` (`theme.ts:194`): the terminal's last light/dark report (mode
    /// `2031`), consulted only while it has not reported a background.
    reported_scheme: Option<TerminalTheme>,
    active_name: String,
    generation: u64,
    /// The raw `settings.theme` value, retained so [`Self::apply_from_settings`] can re-resolve it.
    theme_setting: Option<String>,
    /// Whether the terminal's appearance changes are followed (Pi `autoSyncEnabled`,
    /// `theme-controller.ts:67`): armed for an automatic `light/dark` pair and for the system
    /// theme.
    auto_sync: bool,
    /// Pi `currentThemeSetting` (`theme-controller.ts:63`): the in-memory theme setting that
    /// shadows `settings.theme`. Seeded from `--use-theme` (`initialThemeSetting`) and replaced by
    /// every user theme switch, so a one-run override survives `/reload` without ever being written
    /// to settings. SEAM-119.
    current_setting: Option<String>,
}

impl ThemeController {
    /// Boot the controller from the raw `settings.theme` value (Pi `getThemeSetting()`), the terminal
    /// [`ColorMode`], and the polarity the environment suggests (Pi's constructor +
    /// `applyFromSettings`, `theme-controller.ts:72-109`). The active theme is
    /// `resolveThemeSetting(setting, terminal)` when it resolves, else the `system` theme
    /// (`resolveThemeName`, `:174-176`) — never a hardwired `dark`.
    pub fn boot(
        theme_setting: Option<&str>,
        color_mode: ColorMode,
        terminal_theme: TerminalTheme,
    ) -> Self {
        let mut controller = ThemeController {
            color_mode,
            env_theme: Some(terminal_theme),
            terminal_colors: TerminalColors::default(),
            // `markTerminalColorsPending()` — "The system theme starts in grayscale; color follows
            // once the terminal reports its colors" (`theme-controller.ts:87-88`).
            colors_pending: true,
            reported: None,
            reported_scheme: None,
            active_name: String::new(),
            generation: 0,
            theme_setting: theme_setting.map(str::to_string),
            auto_sync: false,
            current_setting: None,
        };
        controller.active_name = controller.resolve_theme_name();
        controller.auto_sync = controller.wants_auto_sync();
        controller
    }

    /// Pi's constructor with an `initialThemeSetting` (`theme-controller.ts:72-90`): the boot theme
    /// resolves from `initialThemeSetting ?? settings.theme`, and the initial setting is kept as the
    /// in-memory shadow later re-resolutions prefer. `--use-theme` is the only source of one
    /// (`main.ts:944`). SEAM-119.
    pub fn boot_with_initial(
        initial_setting: Option<&str>,
        theme_setting: Option<&str>,
        color_mode: ColorMode,
        terminal_theme: TerminalTheme,
    ) -> Self {
        let mut controller = ThemeController::boot(
            initial_setting.or(theme_setting),
            color_mode,
            terminal_theme,
        );
        controller.current_setting = initial_setting.map(str::to_string);
        controller
    }

    /// Boot with the colour mode and polarity detected from the environment (the binary path).
    ///
    /// The terminal has not been asked anything yet — it is not in raw mode and cannot answer an
    /// escape query — so the `system` theme is grayscale and the polarity is `COLORFGBG`'s. Call
    /// [`Self::request_terminal_colors`] once raw mode is on to complete the boot.
    pub fn boot_from_env(theme_setting: Option<&str>) -> Self {
        ThemeController::boot(theme_setting, ColorMode::detect(), TerminalTheme::detect())
    }

    /// [`Self::boot_with_initial`] with the colour mode and polarity detected from the
    /// environment, as [`Self::boot_from_env`] detects them.
    pub fn boot_from_env_with_initial(
        initial_setting: Option<&str>,
        theme_setting: Option<&str>,
    ) -> Self {
        ThemeController::boot_with_initial(
            initial_setting,
            theme_setting,
            ColorMode::detect(),
            TerminalTheme::detect(),
        )
    }

    /// Pi `getThemeSetting()` (`theme-controller.ts:169-171`): the in-memory setting, else the one
    /// settings carried.
    fn effective_setting(&self) -> Option<&str> {
        self.current_setting
            .as_deref()
            .or(self.theme_setting.as_deref())
    }

    /// Pi `resolveThemeName` (`theme-controller.ts:174-176`): the theme for the current setting and
    /// terminal appearance. Without one that resolves, pi uses the system theme.
    fn resolve_theme_name(&self) -> String {
        resolve_theme_setting(self.effective_setting(), self.terminal_theme())
            .unwrap_or_else(|| SYSTEM_THEME_NAME.to_string())
    }

    /// `parseAutoThemeSetting(themeSetting) !== undefined || themeName === SYSTEM_THEME_NAME`
    /// (`theme-controller.ts:106`).
    fn wants_auto_sync(&self) -> bool {
        parse_auto_theme_setting(self.effective_setting()).is_some()
            || self.resolve_theme_name() == SYSTEM_THEME_NAME
    }

    /// Whether the terminal is dark or light, from everything it reported so far (Pi
    /// `getTerminalTheme`, `theme.ts:713-715`).
    pub fn terminal_theme(&self) -> TerminalTheme {
        detect_terminal_theme(&self.terminal_colors, self.reported_scheme, self.env_theme)
    }

    /// The colours the terminal last reported (Pi `terminalColors`, `theme.ts:190`).
    pub fn terminal_colors(&self) -> TerminalColors {
        self.terminal_colors
    }

    /// Whether the system theme is still waiting for the terminal's colours, and so grayscale
    /// (Pi `terminalColorsPending`, `theme.ts:192`).
    pub fn terminal_colors_pending(&self) -> bool {
        self.colors_pending
    }

    /// Ask the terminal for its colours and apply the answer — Pi `queryTerminalColors` +
    /// `requestTerminalColors` (`theme-controller.ts:31-40`, `:189-191`) with the 100 ms
    /// [`COLOR_QUERY_TIMEOUT`]. A terminal that does not answer reports nothing, which still ends
    /// the grayscale state. `on_late_reply` receives the finished colours if the terminal completes
    /// the query after the timeout; hand them to [`Self::apply_terminal_colors`] when they arrive.
    ///
    /// Returns the name of the theme to load when the colours changed anything (see
    /// [`Self::apply_terminal_colors`]).
    pub fn request_terminal_colors(
        &mut self,
        probe: &dyn TerminalProbe,
        on_late_reply: Option<LateColors>,
    ) -> Option<String> {
        let colors = probe.query_terminal_colors(COLOR_QUERY_TIMEOUT, on_late_reply);
        self.apply_terminal_colors(colors)
    }

    /// Record reported colours — Pi `applyTerminalColors` (`theme-controller.ts:197-211`): themes use
    /// the default colours for tokens set to `""`, the system theme is generated from all of them,
    /// and light/dark detection uses them. Ends the grayscale state.
    ///
    /// A colour the new report omits keeps its previous value, so a query that timed out does not
    /// erase what an earlier one reported. Nothing changing (including a repeat timeout) is a
    /// no-op: `None`. Otherwise the result is the name of the theme to (re)load — re-applying the
    /// setting regenerates the system theme, or switches the theme of an automatic pair
    /// (`reapplyForTerminal`, `:217-223`).
    pub fn apply_terminal_colors(&mut self, reported: TerminalColors) -> Option<String> {
        let previous = self.reported;
        let next = TerminalColors {
            foreground: reported.foreground.or(previous.and_then(|p| p.foreground)),
            background: reported.background.or(previous.and_then(|p| p.background)),
            palette: reported.palette.or(previous.and_then(|p| p.palette)),
        };
        // Re-rendering rebuilds every component, so skip it when nothing changed (including
        // timeouts).
        if previous == Some(next) {
            return None;
        }
        self.reported = Some(next);
        self.terminal_colors = next;
        self.colors_pending = false;
        let name = self.resolve_theme_name();
        self.set_theme_name(name.clone());
        self.auto_sync = self.wants_auto_sync();
        Some(name)
    }

    /// Re-resolve the active theme from a freshly re-read `settings.theme` — Pi `applyFromSettings`
    /// (`theme-controller.ts:103-109`) — and return the theme NAME the caller must now load.
    ///
    /// Pi runs it on every session replacement (the `setRebindSession` hook,
    /// `interactive-mode.ts:608`) and again explicitly from `handleReloadCommand` (`:6411`), so a
    /// `settings.theme` the user edited on disk, and a custom theme file whose CONTENT changed under
    /// an unchanged name, both take effect without a restart. An automatic `light/dark` pair
    /// resolves against the terminal's polarity; an explicit name is applied verbatim; with no
    /// setting that resolves the name is `system`. The system theme and a pair keep following the
    /// terminal ([`Self::auto_sync`]).
    ///
    /// Returns the resolved name rather than a theme, and deliberately: "the name did not change" is
    /// NOT "there is nothing to do" here, because the theme's own file may have been rewritten under
    /// the same name — which is precisely the `/reload` case. The caller re-loads the named theme
    /// from the swapped-in session's freshly discovered resources every time
    /// ([`crate::App::reapply_theme_from_settings`]), matching Pi's unconditional
    /// `applyThemeName` → `setTheme(name)` re-read.
    pub fn apply_from_settings(&mut self, setting: Option<&str>) -> String {
        // `const themeSetting = this.currentThemeSetting ?? settingsManager.getThemeSetting()`
        // (`theme-controller.ts:169-171`) — a `--use-theme` or an in-app switch shadows the
        // re-read settings value (SEAM-119).
        self.theme_setting = setting.map(str::to_string);
        let name = self.resolve_theme_name();
        self.auto_sync = self.wants_auto_sync();
        // `set_theme_name` is Pi's `applyThemeName`: it re-seats the active name and bumps the
        // generation so every render cache keyed by it re-materialises, which is what makes an
        // unchanged NAME with changed CONTENT repaint.
        self.set_theme_name(name.clone());
        name
    }

    /// Whether the active setting is an `auto` pair or the system theme, i.e. whether Pi would keep
    /// terminal colour-scheme notifications (mode `2031`) enabled and re-theme on every change
    /// (`setAutoSync`, `theme-controller.ts:225-229`).
    ///
    /// The app mirrors the flag onto the terminal ([`crate::color_scheme`]) whenever it can have
    /// changed, and a report that arrives while it is set re-themes through
    /// [`Self::apply_color_scheme`].
    pub fn auto_sync(&self) -> bool {
        self.auto_sync
    }

    /// Pi `applyTerminalColorSchemeChange` (`theme-controller.ts:240-248`): the terminal reported a
    /// light/dark switch. Ignored unless the theme follows the terminal ([`Self::auto_sync`]).
    /// Otherwise the report is recorded, and when it moved the terminal's appearance the setting is
    /// re-applied — the result is the name of the theme to (re)load, as for
    /// [`Self::apply_terminal_colors`]. The caller then queries the terminal's colours again, which
    /// decide the appearance when it reports a background; the scheme only matters for terminals
    /// that do not.
    pub fn apply_color_scheme(&mut self, scheme: TerminalTheme) -> Option<String> {
        if !self.auto_sync {
            return None;
        }
        let previous = self.terminal_theme();
        self.reported_scheme = Some(scheme);
        if self.terminal_theme() == previous {
            return None;
        }
        // `reapplyForTerminal` (`:217-223`): the system theme regenerates, a pair switches.
        let name = self.resolve_theme_name();
        if name != SYSTEM_THEME_NAME && name == self.active_name {
            return None;
        }
        self.set_theme_name(name.clone());
        Some(name)
    }

    /// The terminal's last light/dark report (test/inspection).
    pub fn reported_scheme(&self) -> Option<TerminalTheme> {
        self.reported_scheme
    }

    /// What the system theme is generated from: the terminal's reported colours, full colour once
    /// they have arrived (grayscale until then), and the appearance to assume when it reported no
    /// background (Pi `createSystemTheme`, `theme.ts:611-623`).
    fn system_input(&self) -> SystemThemeInput {
        let colors = self.terminal_colors;
        SystemThemeInput {
            foreground: colors.foreground,
            background: colors.background,
            palette: colors.palette,
            saturation: if self.colors_pending {
                Saturation::GRAYSCALE
            } else {
                Saturation::FULL
            },
            appearance_hint: Some(self.terminal_theme().into()),
        }
    }

    /// The `system` theme generated from what the terminal reported so far — grayscale while
    /// [`Self::terminal_colors_pending`] (Pi `createSystemTheme`, `theme.ts:611-623`).
    pub fn system_theme(&self) -> UiTheme {
        UiTheme::system(&self.system_input())
            .with_terminal_appearance(self.terminal_theme().into())
            .with_color_mode(self.color_mode)
            .with_generation(self.generation)
    }

    /// The generation the render caches key on; it moves whenever the active theme is re-applied or
    /// the terminal's colours change.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What an extension and an HTML export are told about the theme right now — see
    /// [`ThemePublication`]. Built from the same inputs as [`Self::system_theme`].
    pub(crate) fn publication(&self) -> ThemePublication {
        let generated = generate_system_theme_colors(&self.system_input());
        let terminal: Appearance = self.terminal_theme().into();
        let colors = self.terminal_colors;
        let css = |rgb: cyrup_resources::color::Rgb| {
            cyrup_session_svc::CssColor::from_rgb(rgb.r, rgb.g, rgb.b)
        };
        let document = ThemeData {
            name: SYSTEM_THEME_NAME.to_string(),
            appearance: generated.appearance,
            vars: BTreeMap::new(),
            colors: generated
                .colors
                .iter()
                .map(|(token, color)| {
                    let value = match color {
                        SystemColor::Rgb(rgb) => ColorValue::Text(rgb.hex()),
                        SystemColor::Indexed(index) => ColorValue::Index(*index),
                        SystemColor::Default => ColorValue::Text(String::new()),
                    };
                    ((*token).to_string(), value)
                })
                .collect(),
            export: BTreeMap::new(),
        };
        ThemePublication {
            generation: self.generation,
            system_document: document,
            system_export: cyrup_session_svc::ExportTheme::from_system(
                &generated,
                colors.foreground,
                colors.background,
                terminal,
            ),
            terminal_defaults: cyrup_session_svc::TerminalDefaults {
                foreground: colors.foreground.map(css),
                background: colors.background.map(css),
                appearance: terminal,
            },
        }
    }

    /// The projected render theme for the active name: `system` generated from the terminal's
    /// colours, or a compiled-in built-in, through the depth projection. This is what the app boots
    /// its `UiTheme` from and re-reads on a live switch. A name that is neither is the system theme,
    /// as it is for pi's failed `setTheme`.
    pub fn theme(&self) -> UiTheme {
        self.theme_named(&self.active_name)
    }

    /// The theme for `name` as this controller would paint it: `system` generated from the
    /// terminal's colours, a compiled-in built-in, else — an unknown name — the system theme.
    pub fn theme_named(&self, name: &str) -> UiTheme {
        if name == SYSTEM_THEME_NAME {
            return self.system_theme();
        }
        match UiTheme::builtin_named(name) {
            Some(theme) => theme
                .with_terminal_appearance(self.terminal_theme().into())
                .with_color_mode(self.color_mode)
                .with_generation(self.generation),
            None => self.system_theme(),
        }
    }

    /// Pi `getThemeSelection()` (`theme-controller.ts:117-119`): the setting in force — the in-memory
    /// one, else the one settings carried, else the theme that is painted. What `/settings` shows
    /// for the Theme row and opens the theme submenu on.
    pub fn theme_selection(&self) -> &str {
        self.effective_setting().unwrap_or(&self.active_name)
    }

    /// The active theme name (test/inspection).
    pub fn active_name(&self) -> &str {
        &self.active_name
    }

    /// Re-point the mode later theme applications project through, after a `terminal.trueColor`
    /// override changed the detected colour depth (CFG-090).
    pub(crate) fn set_color_mode(&mut self, color_mode: ColorMode) {
        self.color_mode = color_mode;
    }

    /// The colour mode the controller projects into (test/inspection).
    pub fn color_mode(&self) -> ColorMode {
        self.color_mode
    }

    /// pi's `applyThemeName` failure assignment — `this.activeThemeName = result.success ?
    /// themeName : SYSTEM_THEME_NAME` (`theme-controller.ts:180` @v1.0.0).
    ///
    /// Called by the loader ([`crate::App::reapply_theme_from_settings`]) when the name the
    /// controller just resolved could not be loaded, so `active_name()` and every reader of it
    /// (`/settings`, the theme-file watcher binding) name the theme that is actually painted rather
    /// than the one that failed. The generation bumps for the same reason every other apply does.
    pub fn fall_back_to_system(&mut self) -> UiTheme {
        self.set_theme_name(SYSTEM_THEME_NAME)
    }

    /// Record a user theme switch as the in-memory setting — pi's `this.currentThemeSetting =
    /// themeName` in `setThemeName` and `= themeSetting` in `setThemeSetting`
    /// (`theme-controller.ts:124-136`), which both of pi's switch paths (the `/settings` theme
    /// confirm and an extension's `ctx.ui.setTheme`) run. SEAM-119.
    pub fn set_current_setting(&mut self, setting: impl Into<String>) {
        self.current_setting = Some(setting.into());
        self.auto_sync = self.wants_auto_sync();
    }

    /// The in-memory setting that shadows `settings.theme`, if any (test/inspection).
    pub fn current_setting(&self) -> Option<&str> {
        self.current_setting.as_deref()
    }

    /// Switch the active theme by name (Pi `applyThemeName`), bumping the generation so render
    /// caches invalidate. Returns the freshly projected [`UiTheme`].
    pub fn set_theme_name(&mut self, name: impl Into<String>) -> UiTheme {
        self.active_name = name.into();
        self.generation = self.generation.saturating_add(1);
        self.theme()
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

/// A [`cyrup_ext::RenderTheme`] over the live [`UiTheme`], so a native renderer's component can
/// colour its own output in the user's palette (Pi hands its renderers the `Theme` directly).
///
/// The bridge is SGR text rather than a ratatui `Style`, because a component returns
/// `Vec<String>` — Pi's `Component.render(width): string[]`. [`crate::ansi::sgr_line`] converts each
/// row back into styled spans at draw time, so the round trip is lossless for the codes emitted here.
pub struct UiThemeRoles<'a> {
    theme: &'a UiTheme,
}

impl<'a> UiThemeRoles<'a> {
    /// Borrow a live theme as a renderer-facing palette.
    #[must_use]
    pub fn new(theme: &'a UiTheme) -> Self {
        Self { theme }
    }

    /// Wrap `text` in the truecolor SGR pair for `style`'s foreground, plus bold/dim when set.
    /// A style with no foreground and no modifier returns `text` untouched, so an unstyled role
    /// costs no escapes at all.
    fn paint(style: Style, text: &str) -> String {
        let mut open = String::new();
        if style.add_modifier.contains(Modifier::BOLD) {
            open.push_str("\u{1B}[1m");
        }
        if style.add_modifier.contains(Modifier::DIM) {
            open.push_str("\u{1B}[2m");
        }
        match style.fg {
            None => {}
            Some(Color::Rgb(r, g, b)) => open.push_str(&format!("\u{1B}[38;2;{r};{g};{b}m")),
            // A role the theme sets to `""` is the terminal's own default foreground, which pi
            // draws with `\x1b[39m` (`Theme.addToken`, `theme.ts:290-294` @v1.0.0). Resolving it to
            // a colour here painted it black.
            Some(Color::Reset) => open.push_str("\u{1B}[39m"),
            // A palette index stays an index, so 0-15 keep following the user's terminal palette
            // (`foregroundAnsi` for an indexed colour is `38;5;N`).
            Some(Color::Indexed(index)) => open.push_str(&format!("\u{1B}[38;5;{index}m")),
            // The sixteen ANSI names are resolved to RGB: `sgr_line` has no name form to read back.
            Some(named) => {
                let (r, g, b) = named_color_rgb(named);
                open.push_str(&format!("\u{1B}[38;2;{r};{g};{b}m"));
            }
        }
        if open.is_empty() {
            return text.to_string();
        }
        // One reset closes every attribute this call opened, matching Pi's `theme.fg` wrapping.
        format!("{open}{text}\u{1B}[0m")
    }
}

impl cyrup_ext::RenderTheme for UiThemeRoles<'_> {
    /// Pi `theme.fg(color, text)`. An unknown role returns `text` unchanged — upstream's `Theme.fg`
    /// never throws and never invents a colour.
    fn fg(&self, role: &str, text: &str) -> String {
        let style = match role {
            "muted" => self.theme.muted_style(),
            "toolTitle" => self.theme.tool_title_style(),
            "text" => self.theme.custom_message_text_style(),
            "dim" => self.theme.dim_style(),
            "accent" => self.theme.accent_style(),
            "error" => self.theme.error_style(),
            _ => return text.to_string(),
        };
        Self::paint(style, text)
    }

    /// Pi `theme.bold(text)`.
    fn bold(&self, text: &str) -> String {
        format!("\u{1B}[1m{text}\u{1B}[22m")
    }
}

/// Resolve a ratatui named/indexed colour to RGB so [`UiThemeRoles::paint`] only ever emits the
/// truecolor form. The 16 ANSI names use the widely-shared xterm defaults; an indexed colour walks
/// the standard 6×6×6 cube and greyscale ramp.
fn named_color_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 0, 0),
        Color::Green => (0, 205, 0),
        Color::Yellow => (205, 205, 0),
        Color::Blue => (0, 0, 238),
        Color::Magenta => (205, 0, 205),
        Color::Cyan => (0, 205, 205),
        Color::Gray => (229, 229, 229),
        Color::DarkGray => (127, 127, 127),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (92, 92, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::White => (255, 255, 255),
        Color::Indexed(i) => indexed_rgb(i),
        Color::Reset => (0, 0, 0),
    }
}

/// The xterm-256 palette: 0-15 map to the named colours, 16-231 are the 6×6×6 cube, 232-255 the
/// greyscale ramp.
fn indexed_rgb(i: u8) -> (u8, u8, u8) {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match i {
        0..=15 => named_color_rgb(match i {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Gray,
            8 => Color::DarkGray,
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            _ => Color::White,
        }),
        16..=231 => {
            let n = i - 16;
            // `n` is 0..=215, so every divisor below lands in `0..=5` and `LEVELS` (len 6) always
            // has the entry. Read through `get` rather than `[]` so the bound is carried in the
            // types instead of asserted in a comment (`clippy::indexing_slicing` is denied).
            let level = |k: u8| LEVELS.get(usize::from(k)).copied().unwrap_or(0);
            (level(n / 36), level((n % 36) / 6), level(n % 6))
        }
        232..=255 => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
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
        assert_eq!(
            ColorMode::Ansi256.project(Color::Indexed(42)),
            Color::Indexed(42)
        );
    }

    #[test]
    fn with_color_mode_is_idempotent_and_projects_every_role() {
        let dark = UiTheme::dark().with_color_mode(ColorMode::Ansi256);
        // Foreground is now an indexed color, never RGB.
        assert!(matches!(dark.foreground, Some(Color::Indexed(_))));
        assert!(
            dark.roles
                .values()
                .all(|c| !matches!(c, Color::Rgb(_, _, _)))
        );
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
        assert_eq!(
            resolve_theme_setting(Some("nord"), TerminalTheme::Light),
            Some("nord".to_string())
        );
        assert_eq!(
            resolve_theme_setting(Some("a/b/c"), TerminalTheme::Dark),
            None
        );
        assert_eq!(resolve_theme_setting(None, TerminalTheme::Light), None);
    }
}

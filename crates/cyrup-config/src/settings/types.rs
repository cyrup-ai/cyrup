//! Scope tag and the typed value objects settings getters return (Pi settings-manager.ts:10-85).

/// Which layer a settings document belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsScope {
    Global,
    Project,
}

/// `defaultProjectTrust` (global-only; §4.8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefaultProjectTrust {
    #[default]
    Ask,
    Always,
    Never,
}

/// How mermaid fences are rendered (Pi `MermaidRenderingMode`, settings-manager.ts:57 @v0.84.1 —
/// `"off" | "final" | "streaming"`; the key and the type are both v0.84.1 additions). CFG-040.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MermaidRenderingMode {
    Off,
    Final,
    /// Pi's documented default (`settings-manager.ts:61`, `// default: "streaming"`).
    #[default]
    Streaming,
}

impl MermaidRenderingMode {
    /// The settings-file spelling, i.e. the value `setMermaidRenderingMode` writes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Final => "final",
            Self::Streaming => "streaming",
        }
    }
}

/// `cacheWarming` — the prompt-cache warming profile (Pi `CacheWarmingMode`,
/// `settings-manager.ts:76-78` @v0.87.1: `CACHE_WARMING_MODES = ["off", "streaming", "idle"]`;
/// "idle" also warms between agent runs). New at v0.86.0. CFG-093.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheWarmingMode {
    Off,
    /// Pi's documented default (`settings-manager.ts:157`, `// default: "streaming"`).
    #[default]
    Streaming,
    Idle,
}

impl CacheWarmingMode {
    /// The settings-file spelling, i.e. the value `setCacheWarmingMode` writes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Streaming => "streaming",
            Self::Idle => "idle",
        }
    }
}

/// A forced `terminal.images` value (Pi `TerminalSettings.images`, `settings-manager.ts:58`
/// @v0.87.1: `"kitty" | "iterm2" | "auto" | false`). `"auto"` is the absence of an override, so it
/// has no variant. CFG-090.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalImagesOverride {
    Kitty,
    Iterm2,
    /// `false` — no inline-image protocol (pi's `{ images: null }`).
    Disabled,
}

/// The settings-level terminal capability overrides (Pi `getTerminalCapabilityOverrides()`,
/// `settings-manager.ts:1195-1203` @v0.87.1, a `Partial<TerminalCapabilities>`): each `None` field
/// is a key pi leaves out of the partial, so the detected value stands. CFG-090.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCapabilityOverrides {
    pub images: Option<TerminalImagesOverride>,
    pub true_color: Option<bool>,
    pub hyperlinks: Option<bool>,
}

/// `tuiMode` — which renderer the interactive TUI starts in (Pi `TuiMode`, settings-manager.ts:36,
/// itself a re-export of `pi-tui`'s `TuiMode` = `"regular" | "fullscreen"`; the settings key is
/// declared at `:184` @v1.0.0 with `// default: "fullscreen"`). ADR-0005 §Decision A-3.
///
/// The key is upstream drift relative to v0.83.0, the tag cyrup otherwise ports — and pairs with
/// the `--tui-mode` flag (`args.ts:326` @v1.0.0).
///
/// CFG-096 / TUI-135 — the default moved from `regular` to `fullscreen` at v1.0.0 (`88ff80b98`),
/// so [`Self::Fullscreen`] carries `#[default]` and is the value every unrecognized spelling
/// degrades to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TuiMode {
    /// The inline (main-screen) renderer, reached only by an explicit `"regular"` — see
    /// [`super::EffectiveSettings::tui_mode`].
    Regular,
    /// The alternate-screen renderer (`crates/cyrup-tui/src/altscreen/`). Pi's documented default
    /// since v1.0.0, and the value every unrecognized spelling degrades to.
    #[default]
    Fullscreen,
}

impl TuiMode {
    /// The settings-file spelling, i.e. the value `setTuiMode` writes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Fullscreen => "fullscreen",
        }
    }
}

/// `fullscreenScrollbar` — the alternate screen's scrollbar policy (Pi `ScrollViewScrollbar`,
/// `pi-tui` `scroll-view.ts:4`: `"hidden" | "auto" | "always"`; the settings key is declared at
/// settings-manager.ts:136 @v0.84.1 with `// default: "auto"; no effect in regular TUI mode`).
/// ADR-0005 §Decision A-3.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FullscreenScrollbar {
    Hidden,
    /// Pi's documented default, and the value every unrecognized spelling degrades to — see
    /// [`super::EffectiveSettings::fullscreen_scrollbar`].
    #[default]
    Auto,
    Always,
}

impl FullscreenScrollbar {
    /// The settings-file spelling, i.e. the value `setFullscreenScrollbar` writes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Auto => "auto",
            Self::Always => "always",
        }
    }
}

/// `fullscreenExitOutput` — what leaving the alternate screen puts on the main screen (Pi
/// `FullscreenExitOutput`, settings-manager.ts:38 @v0.84.4: `"transcript" | "resume-hint"`; the
/// settings key is declared at `:143` with `// default: "transcript"; no effect in regular TUI
/// mode`). CFG-078.
///
/// Both this type and the key are **v0.84.4 additions** — neither exists at v0.84.1, the tag
/// ADR-0005 §A-3 was measured against, which is why `crates/cyrup-tui/src/app/settings_rows.rs`
/// carried a note saying so until this landed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FullscreenExitOutput {
    /// Print the excursion's transcript into the main screen's scrollback on the way out. Pi's
    /// documented default, and the value every unrecognized spelling degrades to — see
    /// [`super::EffectiveSettings::fullscreen_exit_output`].
    #[default]
    Transcript,
    /// Restore the screen the terminal had before the alternate screen was entered and print only
    /// the session's resume hint (`docs/settings.md:70` @v0.84.4).
    ResumeHint,
}

impl FullscreenExitOutput {
    /// The settings-file spelling, i.e. the value `setFullscreenExitOutput` writes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transcript => "transcript",
            Self::ResumeHint => "resume-hint",
        }
    }
}

/// A fixed `fullscreenWheelScrollLines` count — always within `1..=100`.
///
/// Pi clamps this range independently on READ (`getFullscreenWheelScrollLines`,
/// `Math.max(1, Math.min(100, Math.floor(lines)))`, `settings-manager.ts:1389-1394` @v1.0.0) and on
/// WRITE (`setFullscreenWheelScrollLines`, `:1396-1400`), so a hand-edited `500` reads back as `100`
/// rather than being rejected. A newtype with one private field makes both clamps the same
/// constructor: an out-of-range value cannot be built, so neither side can forget to apply it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WheelLineCount(u8);

impl WheelLineCount {
    /// The smallest legal count (`Math.max(1, …)`).
    pub const MIN: u8 = 1;
    /// The largest legal count (`Math.min(100, …)`).
    pub const MAX: u8 = 100;

    /// Floor then clamp a number into `1..=100` — pi's `Math.max(1, Math.min(100,
    /// Math.floor(lines)))`. `None` for a non-finite input, which pi's reader maps to `"auto"`
    /// (`Number.isFinite`, `:1391`).
    pub fn from_number(n: f64) -> Option<Self> {
        if !n.is_finite() {
            return None;
        }
        let clamped = n.floor().clamp(f64::from(Self::MIN), f64::from(Self::MAX));
        // `clamped` is an integer-valued float in `1..=100`, so the conversion is exact.
        Some(Self(clamped as u8))
    }

    /// The count as a plain integer, `1..=100`.
    pub fn get(self) -> u8 {
        self.0
    }
}

/// `fullscreenWheelScrollLines` — lines per wheel event, or `"auto"` to accelerate fast spins (pi
/// `WheelScrollLines = number | "auto"`, `packages/tui/src/wheel-scroll.ts:2` @v1.0.0; the settings
/// key is declared at `settings-manager.ts:188` with `// default: "auto"; lines per wheel event,
/// 1-100`). TUI-136 / CFG-100.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WheelScrollLines {
    /// Pi's default, and what every non-finite or non-numeric stored value reads back as — see
    /// [`super::EffectiveSettings::fullscreen_wheel_scroll_lines`].
    #[default]
    Auto,
    /// A fixed count per event.
    Lines(WheelLineCount),
}

impl WheelScrollLines {
    /// The settings-file value `setFullscreenWheelScrollLines` writes: the string `"auto"`, or the
    /// clamped number (a JSON NUMBER — pi's reader answers `"auto"` for a numeric string).
    pub fn to_json(self) -> serde_json::Value {
        match self {
            Self::Auto => serde_json::Value::from("auto"),
            Self::Lines(n) => serde_json::Value::from(n.get()),
        }
    }

    /// Build from an arbitrary number, clamping exactly as pi's setter does; a non-finite number
    /// is `Auto`.
    pub fn from_number(n: f64) -> Self {
        WheelLineCount::from_number(n).map_or(Self::Auto, Self::Lines)
    }

    /// Build from a `/settings` row's cycle value (`newValue === "auto" ? "auto" :
    /// parseInt(newValue, 10)`, `settings-selector.ts:975`), clamped like the setter. Text that is
    /// neither `auto` nor a number is `Auto`.
    pub fn from_row_value(value: &str) -> Self {
        match value.trim().parse::<i64>() {
            // `i64 -> f64` only loses precision above 2^53, far outside the `1..=100` clamp.
            Ok(n) => Self::from_number(n as f64),
            Err(_) => Self::Auto,
        }
    }
}

impl std::fmt::Display for WheelScrollLines {
    /// `String(config.fullscreenWheelScrollLines)` (`settings-selector.ts:738`): `auto` or the
    /// decimal count.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Lines(n) => write!(f, "{}", n.get()),
        }
    }
}

/// `quietStartup` — how much of the startup output is silenced (pi `QuietStartup = boolean |
/// "header"`, `settings-manager.ts:112` @v1.0.0; the key is declared at `:150` with
/// `// default: false`).
///
/// Two independent decisions hang off the one value (`interactive-mode.ts:1409-1417`):
///
/// * the startup HEADER (logo, version, key hints) is hidden only by [`Self::On`]
///   (`shouldShowStartupHeader`: `verbose || getQuietStartup() !== true`);
/// * the startup DETAILS (the model-scope line and the loaded-resources listing) are hidden by
///   [`Self::On`] and [`Self::Header`] alike (`shouldShowStartupDetails`: `verbose ||
///   getQuietStartup() === false`).
///
/// `--verbose` overrides both, so the two decisions take it as an argument.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum QuietStartup {
    /// `false` — pi's default, and what every unrecognised stored value reads back as (see
    /// [`Self::from_value`]).
    #[default]
    Off,
    /// `true` — header and details are both hidden.
    On,
    /// `"header"` — the header stays, the details are hidden.
    Header,
}

impl QuietStartup {
    /// The three spellings the `/settings` row cycles through, in pi's order
    /// (`settings-selector.ts:559`: `["true", "header", "false"]`).
    pub const ROW_VALUES: [&'static str; 3] = ["true", "header", "false"];

    /// `getQuietStartup` (`settings-manager.ts:1089-1092`): `value === true || value === "header" ?
    /// value : false`. Only the boolean `true` and the exact string `"header"` are recognised;
    /// absent, `false`, `null`, `"true"`, `"HEADER"`, a number or any other type is [`Self::Off`].
    pub fn from_value(value: Option<&serde_json::Value>) -> Self {
        match value {
            Some(serde_json::Value::Bool(true)) => Self::On,
            Some(serde_json::Value::String(s)) if s == "header" => Self::Header,
            _ => Self::Off,
        }
    }

    /// What `setQuietStartup` stores: the boolean, or the string `"header"`.
    pub fn to_json(self) -> serde_json::Value {
        match self {
            Self::Off => serde_json::Value::Bool(false),
            Self::On => serde_json::Value::Bool(true),
            Self::Header => serde_json::Value::from("header"),
        }
    }

    /// Build from a `/settings` row's cycle value (`onQuietStartupChange(newValue === "header" ?
    /// "header" : newValue === "true")`, `settings-selector.ts:924`): `"header"` is the string,
    /// `"true"` the boolean and every other text `false`.
    pub fn from_row_value(value: &str) -> Self {
        match value {
            "header" => Self::Header,
            "true" => Self::On,
            _ => Self::Off,
        }
    }

    /// `shouldShowStartupHeader` (`interactive-mode.ts:1410-1412`): hidden only by [`Self::On`],
    /// and `--verbose` always shows it.
    pub const fn shows_header(self, verbose: bool) -> bool {
        verbose || !matches!(self, Self::On)
    }

    /// `shouldShowStartupDetails` (`interactive-mode.ts:1415-1417`): shown only by [`Self::Off`],
    /// and `--verbose` always shows them.
    pub const fn shows_details(self, verbose: bool) -> bool {
        verbose || matches!(self, Self::Off)
    }
}

impl std::fmt::Display for QuietStartup {
    /// `String(config.quietStartup)` (`settings-selector.ts:558`): `false`, `true` or `header`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Off => "false",
            Self::On => "true",
            Self::Header => "header",
        })
    }
}

/// Custom per-level thinking token budgets (Pi `ThinkingBudgetsSettings`, settings-manager.ts:46-51).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimal: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub medium: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<i64>,
}

/// How the `codemode` tool presents tools while it is active (pi `CodemodeMode`,
/// `settings-manager.ts:94-101` @v1.0.1).
///
/// * `on` — declared tools that scripts can call get a note on calling them from scripts appended
///   to their description; the codemode description lists only the tools without `direct` exposure.
/// * `only` — the codemode description lists every tool scripts can call, and active `direct`
///   tools are not declared to the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodemodeMode {
    /// Pi's documented default (`settings-manager.ts:105`, `/** Default: `on`. */`).
    #[default]
    On,
    Only,
}

impl CodemodeMode {
    /// The settings-file spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Only => "only",
        }
    }
}

/// Default for [`CodemodeSettings::inline_budget`]: the estimated tokens (characters / 4) the
/// codemode description may spend on tool declarations (pi `DEFAULT_CODEMODE_INLINE_BUDGET`,
/// `extensions/codemode/tool.ts:154` @v1.0.1; documented as "Default: 3000" at
/// `settings-manager.ts:107`).
pub const DEFAULT_CODEMODE_INLINE_BUDGET: f64 = 3000.0;

/// The `codemode` settings object (pi `CodemodeSettings`, `settings-manager.ts:103-108` @v1.0.1,
/// read into `Settings.codemode` at `:178`). Both fields are as written in the file; the
/// validated values the codemode extension acts on are
/// [`EffectiveSettings::codemode_mode`](crate::EffectiveSettings::codemode_mode) and
/// [`EffectiveSettings::codemode_inline_budget`](crate::EffectiveSettings::codemode_inline_budget).
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodemodeSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<CodemodeMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_budget: Option<f64>,
}

/// User-facing warning toggles (Pi `WarningSettings`, settings-manager.ts:57-59).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Warnings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_extra_usage: Option<bool>,
}

/// SDK/provider retry knobs (Pi `ProviderRetrySettings`, settings-manager.ts:21-25).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderRetrySettings {
    pub timeout_ms: Option<i64>,
    pub max_retries: Option<i64>,
    pub max_retry_delay_ms: i64,
}

/// Branch-summary knobs (Pi `BranchSummarySettings`, settings-manager.ts:16-19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BranchSummarySettings {
    pub reserve_tokens: i64,
    pub skip_prompt: bool,
}

/// Combined compaction knobs (Pi `CompactionSettings`, settings-manager.ts:10-14;
/// `getCompactionSettings`, :776-782).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: i64,
    pub keep_recent_tokens: i64,
}

/// Combined top-level retry knobs (Pi `RetrySettings` sans the nested `provider` object;
/// `settings-manager.ts:41-47`, `getRetrySettings`, `:932-939` @v0.87.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetrySettings {
    pub enabled: bool,
    pub max_retries: i64,
    pub base_delay_ms: i64,
    pub max_agent_delay_ms: i64,
}

/// A configured package source (Pi `PackageSource`, settings-manager.ts:74-85): either a bare
/// source string, or an object naming the `source` plus `autoload` and optional per-resource
/// filters. Pi documents the three forms at :70-73 — string = load everything, object = filter
/// which resources load, and `autoload=false` = "start empty and only apply explicit resource
/// patterns".
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum PackageSource {
    Name(String),
    Detailed {
        source: String,
        /// `autoload` (Pi settings-manager.ts:79). `Some(false)` turns every per-type list from an
        /// INCLUDE filter (start from everything, narrow) into a DELTA (start from nothing, add
        /// back only what is named) — see [`PackageSource::autoload`].
        #[serde(skip_serializing_if = "Option::is_none", default)]
        autoload: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        extensions: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        skills: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        prompts: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        themes: Option<Vec<String>>,
    },
}

impl PackageSource {
    /// The raw source string (Pi `getPackageSourceString`, package-manager.ts:1338-1340).
    pub fn source(&self) -> &str {
        match self {
            PackageSource::Name(s) => s,
            PackageSource::Detailed { source, .. } => source,
        }
    }

    /// The entry's `autoload` flag, `None` for a bare string entry (Pi reads it off the object form
    /// only, `filter.autoload === false`, package-manager.ts:2084).
    ///
    /// Only an explicit `false` changes anything: it selects `applyPackageDeltaFilter` (:2085) in
    /// place of `applyPackageFilter`/`collectDefaultResources`, which starts from an EMPTY resource
    /// set and adds back only what the per-type patterns name — so a bare
    /// `{"source": …, "autoload": false}` contributes NOTHING (:2180-2182). `true` and absent are
    /// identical and leave the ordinary include-filter path alone.
    pub fn autoload(&self) -> Option<bool> {
        match self {
            PackageSource::Name(_) => None,
            PackageSource::Detailed { autoload, .. } => *autoload,
        }
    }

    /// The per-resource filters, `None` for a bare string entry (Pi
    /// `const filter = typeof pkg === "object" ? pkg : undefined`, package-manager.ts:1231).
    /// Order: `extensions`, `skills`, `prompts`, `themes` — Pi's `RESOURCE_TYPES` (:194).
    /// Read alongside [`PackageSource::autoload`], which decides whether these are include filters
    /// or delta patterns.
    #[allow(clippy::type_complexity)]
    pub fn filters(
        &self,
    ) -> (
        Option<&[String]>,
        Option<&[String]>,
        Option<&[String]>,
        Option<&[String]>,
    ) {
        match self {
            PackageSource::Name(_) => (None, None, None, None),
            PackageSource::Detailed {
                extensions,
                skills,
                prompts,
                themes,
                ..
            } => (
                extensions.as_deref(),
                skills.as_deref(),
                prompts.as_deref(),
                themes.as_deref(),
            ),
        }
    }
}

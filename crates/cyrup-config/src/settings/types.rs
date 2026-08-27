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

/// `tuiMode` — which renderer the interactive TUI starts in (Pi `TuiMode`, settings-manager.ts:36
/// @v0.84.1, itself a re-export of `pi-tui`'s `TuiMode` = `"regular" | "fullscreen"`; the settings
/// key is declared at `:135` with `// default: "regular"`). ADR-0005 §Decision A-3.
///
/// The key exists at v0.84.1 only — it is upstream drift relative to v0.83.0, the tag cyrup
/// otherwise ports — and pairs with the `--tui-mode` flag (`args.ts:180-192`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TuiMode {
    /// The inline (main-screen) renderer. Pi's documented default, and the value every
    /// unrecognized spelling degrades to — see [`super::EffectiveSettings::tui_mode`].
    #[default]
    Regular,
    /// The alternate-screen renderer (`crates/cyrup-tui/src/altscreen/`).
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
/// settings-manager.ts:27-32, `getRetrySettings`, :808-814).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetrySettings {
    pub enabled: bool,
    pub max_retries: i64,
    pub base_delay_ms: i64,
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

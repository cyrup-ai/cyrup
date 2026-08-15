//! Layered settings: global ◁ project ◁ CLI deep-merge with unknown-key preservation
//! (arch-07 §3.2/§3.3/§4.3, R-07-001/004/005).
//!
//! Settings are represented structurally as a JSON object map. This makes unknown-key
//! preservation (R-07-004) and per-key nested deep-merge (R-07-001) trivially correct, while
//! typed getters apply documented defaults in one place (mirrors Pi's `getX()` accessors).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::ModelThinkingLevel;
use serde_json::{Map, Value};

use crate::error::{ConfigError, ScopedError};

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

/// Keys that are only honoured in the GLOBAL scope; stripped from project/CLI before merge.
///
/// Upstream expresses this by reading them off the raw global document rather than the merged view
/// — `getGlobalSettings()` (settings-manager.ts:442-444 @v0.83.0) returns `this.globalSettings`,
/// not `this.settings`, so a key read through it can never be supplied by a project. Exactly two
/// production keys are read that way at v0.83.0:
///
/// - `defaultProjectTrust` (§4.8).
/// - `httpProxy` — `applyHttpProxySettings(bootstrapSettingsManager.getGlobalSettings().httpProxy)`
///   at `main.ts:537` and `applyHttpProxySettings(settingsManager.getGlobalSettings().httpProxy)`
///   at `main.ts:801`, documented as such in `packages/coding-agent/docs/settings.md:87` —
///   "HTTP proxy URL applied as `HTTP_PROXY` and `HTTPS_PROXY`. Global setting only." CFG-057: this
///   was missing, so a project `.cyrup/settings.json` could rewrite the session's egress. Note the
///   neighbouring `httpIdleTimeoutMs` IS merged upstream (`getHttpIdleTimeoutMs` reads
///   `this.settings`) — this is a per-key upstream decision, not a category.
const GLOBAL_ONLY_KEYS: &[&str] = &["defaultProjectTrust", "httpProxy"];

/// A settings document (one scope). Wraps a JSON object so unknown / not-yet-modelled keys are
/// preserved across a load→save round-trip (R-07-004).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    obj: Map<String, Value>,
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse from raw JSON text. An empty / whitespace document is the empty object. Legacy shapes
    /// are migrated on parse (R-07; Pi `migrateSettings` runs on every load path).
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        // A non-object top level is an ERROR, not a silent empty document (CFG-030). Deserializing
        // straight into the map produces serde's own "invalid type" message, which then flows
        // through `record_load_error` → the scope write latch (`ensure_scope_writable`), so the
        // next `/config` write is REFUSED instead of rewriting the user's file from `{}`. pi has no
        // degraded path either: `JSON.parse` + `migrateSettings` (settings-manager.ts:389
        // @v0.83.0), and `persistScopedSettings` (`:585-593`) spreads whatever it parsed.
        let mut obj: Map<String, Value> = serde_json::from_str(text)?;
        migrate_settings(&mut obj);
        Ok(Self { obj })
    }

    pub fn from_map(obj: Map<String, Value>) -> Self {
        Self { obj }
    }

    pub fn as_map(&self) -> &Map<String, Value> {
        &self.obj
    }

    pub fn to_value(&self) -> Value {
        Value::Object(self.obj.clone())
    }

    /// Pretty JSON (2-space) with a trailing newline (Pi byte-interop).
    pub fn to_pretty(&self) -> String {
        let mut s = serde_json::to_string_pretty(&self.obj).unwrap_or_else(|_| "{}".to_string());
        s.push('\n');
        s
    }

    pub fn is_empty(&self) -> bool {
        self.obj.is_empty()
    }

    /// Set a single top-level field (typed).
    pub fn set_field<T: serde::Serialize>(
        &mut self,
        key: &str,
        value: T,
    ) -> Result<(), serde_json::Error> {
        self.obj
            .insert(key.to_string(), serde_json::to_value(value)?);
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.obj.get(key)
    }

    fn get_str(&self, key: &str) -> Option<String> {
        self.obj
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn get_bool(&self, key: &str) -> Option<bool> {
        self.obj.get(key).and_then(Value::as_bool)
    }

    /// Walk a nested `a.b.c` path through objects, returning the leaf [`Value`].
    fn get_path(&self, path: &[&str]) -> Option<&Value> {
        let (first, rest) = path.split_first()?;
        let mut cur = self.obj.get(*first)?;
        for key in rest {
            cur = cur.as_object()?.get(*key)?;
        }
        Some(cur)
    }

    fn get_nested_bool(&self, path: &[&str]) -> Option<bool> {
        self.get_path(path).and_then(Value::as_bool)
    }

    fn get_nested_i64(&self, path: &[&str]) -> Option<i64> {
        self.get_path(path).and_then(Value::as_i64)
    }

    fn get_nested_f64(&self, path: &[&str]) -> Option<f64> {
        self.get_path(path).and_then(Value::as_f64)
    }

    fn get_nested_str(&self, path: &[&str]) -> Option<String> {
        self.get_path(path)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Read a `Vec<String>` settings array from THIS layer alone (no merge), with Pi's empty-array
    /// default for an absent/non-array value. Mirrors [`EffectiveSettings::string_list`] but reads a
    /// single raw layer so a caller can split global- vs project-scope resource overrides (Pi
    /// `SettingsManager` exposes the per-layer `globalSettings`/`projectSettings` split,
    /// settings-manager.ts:455-470).
    fn layer_string_list(&self, key: &str) -> Vec<String> {
        self.obj
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `getSkillPaths` for THIS layer only (Pi `globalSettings.skills`/`projectSettings.skills`).
    pub fn skill_paths(&self) -> Vec<String> {
        self.layer_string_list("skills")
    }

    /// `getPromptTemplatePaths` for THIS layer only.
    pub fn prompt_template_paths(&self) -> Vec<String> {
        self.layer_string_list("prompts")
    }

    /// `getThemePaths` for THIS layer only.
    pub fn theme_paths(&self) -> Vec<String> {
        self.layer_string_list("themes")
    }

    /// `getExtensionPaths` for THIS layer only. Pi's `RESOURCE_TYPES` starts with `"extensions"`
    /// (package-manager.ts:194) and `resolve()` runs `resolveLocalEntries` over all four types per
    /// scope (:905-931), so these plain paths LOAD extension roots — they are not merely filters.
    pub fn extension_paths(&self) -> Vec<String> {
        self.layer_string_list("extensions")
    }

    /// `getPackages` for THIS layer only (Pi reads `projectSettings.packages` and
    /// `globalSettings.packages` separately so the project layer wins the dedupe and can be
    /// trust-gated independently, package-manager.ts:891-898).
    ///
    /// Entries are parsed INDIVIDUALLY: one malformed entry is reported and skipped rather than
    /// discarding the whole array (which is what a blanket `from_value::<Vec<_>>().ok()` does) and
    /// never affects the rest of the settings document. Returns `(parsed, errors)`.
    pub fn packages_with_errors(&self) -> (Vec<PackageSource>, Vec<String>) {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        let Some(arr) = self.obj.get("packages").and_then(Value::as_array) else {
            // A present-but-non-array `packages` is itself worth saying out loud.
            if self.obj.contains_key("packages") {
                errors.push("settings `packages` must be an array".to_string());
            }
            return (out, errors);
        };
        for (i, v) in arr.iter().enumerate() {
            match serde_json::from_value::<PackageSource>(v.clone()) {
                Ok(p) => out.push(p),
                Err(e) => errors.push(format!(
                    "settings `packages[{i}]` is not a package source: {e}"
                )),
            }
        }
        (out, errors)
    }

    /// [`Self::packages_with_errors`] without the error channel.
    pub fn packages(&self) -> Vec<PackageSource> {
        self.packages_with_errors().0
    }
}

/// Remove keys that are only honoured globally (see [`GLOBAL_ONLY_KEYS`]).
fn strip_global_only(settings: &mut Settings) {
    for k in GLOBAL_ONLY_KEYS {
        settings.obj.remove(*k);
    }
}

/// Migrate legacy settings shapes in place (Pi `migrateSettings`, settings-manager.ts:376-435):
/// 1. `queueMode` → `steeringMode`
/// 2. legacy `websockets` boolean → `transport` enum (`websocket`/`sse`)
/// 3. old `skills` object (`{enableSkillCommands, customDirectories}`) → array form
/// 4. `retry.maxDelayMs` → `retry.provider.maxRetryDelayMs`
pub fn migrate_settings(settings: &mut Map<String, Value>) {
    // 1. queueMode -> steeringMode (only when steeringMode is absent; otherwise leave as-is).
    if settings.contains_key("queueMode")
        && !settings.contains_key("steeringMode")
        && let Some(v) = settings.remove("queueMode")
    {
        settings.insert("steeringMode".to_string(), v);
    }

    // 2. websockets boolean -> transport enum
    if !settings.contains_key("transport")
        && let Some(Value::Bool(b)) = settings.get("websockets").cloned()
    {
        settings.insert(
            "transport".to_string(),
            Value::String(if b {
                "websocket".to_string()
            } else {
                "sse".to_string()
            }),
        );
        settings.remove("websockets");
    }

    // 3. skills object -> array
    if let Some(Value::Object(skills)) = settings.get("skills").cloned() {
        if let Some(enable) = skills.get("enableSkillCommands")
            && !settings.contains_key("enableSkillCommands")
        {
            settings.insert("enableSkillCommands".to_string(), enable.clone());
        }
        match skills.get("customDirectories") {
            Some(Value::Array(dirs)) if !dirs.is_empty() => {
                settings.insert("skills".to_string(), Value::Array(dirs.clone()));
            }
            _ => {
                settings.remove("skills");
            }
        }
    }

    // 4. retry.maxDelayMs -> retry.provider.maxRetryDelayMs
    if let Some(Value::Object(retry)) = settings.get_mut("retry") {
        if let Some(Value::Number(max_delay)) = retry.get("maxDelayMs").cloned() {
            let provider_has_max = retry
                .get("provider")
                .and_then(Value::as_object)
                .and_then(|p| p.get("maxRetryDelayMs"))
                .is_some_and(|v| !v.is_null());
            if !provider_has_max {
                let mut provider = retry
                    .get("provider")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                provider.insert("maxRetryDelayMs".to_string(), Value::Number(max_delay));
                retry.insert("provider".to_string(), Value::Object(provider));
            }
        }
        retry.remove("maxDelayMs");
    }
}

/// Generate a random v4 UUID string (Pi `randomUUID()` for `trackingId`). Dependency-free: derives
/// 16 entropy bytes from the OS-seeded `RandomState` hasher plus monotonic/PID inputs, then sets the
/// RFC 4122 version (4) and variant bits. Used only for a non-secret analytics tracking id.
fn random_uuid_v4() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let entropy = |salt: usize| -> u64 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(std::process::id() as u64);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        hasher.write_usize(salt);
        hasher.finish()
    };

    let mut bytes = [0u8; 16];
    for (i, b) in entropy(0)
        .to_le_bytes()
        .into_iter()
        .chain(entropy(1).to_le_bytes())
        .enumerate()
    {
        if let Some(slot) = bytes.get_mut(i) {
            *slot = b;
        }
    }
    // RFC 4122: version 4, variant 10xx.
    if let Some(b) = bytes.get_mut(6) {
        *b = (*b & 0x0f) | 0x40;
    }
    if let Some(b) = bytes.get_mut(8) {
        *b = (*b & 0x3f) | 0x80;
    }
    let mut s = String::with_capacity(36);
    for (i, b) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Deep-merge `over` onto `base` (R-07-001): objects merge recursively per-key; primitives and
/// arrays replace wholesale (matching Pi).
pub fn deep_merge(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            let mut out = b.clone();
            for (k, ov) in o {
                let merged = match out.get(k) {
                    Some(bv) => deep_merge(bv, ov),
                    None => ov.clone(),
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        // arrays + primitives: `over` wins
        (_, over) => over.clone(),
    }
}

/// The merged, read-only effective view (R-07-001). Getters apply documented defaults.
#[derive(Clone, Debug, Default)]
pub struct EffectiveSettings {
    merged: Settings,
}

impl EffectiveSettings {
    pub fn from_settings(merged: Settings) -> Self {
        Self { merged }
    }

    pub fn raw(&self) -> &Settings {
        &self.merged
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.merged.get(key)
    }

    pub fn default_provider(&self) -> Option<String> {
        self.merged.get_str("defaultProvider")
    }

    pub fn default_model(&self) -> Option<String> {
        self.merged.get_str("defaultModel")
    }

    /// `getDefaultThinkingLevel` reads the `defaultThinkingLevel` settings key (Pi
    /// settings-manager.ts:84, and the getter itself at `:740-742` @v0.83.0 —
    /// `getDefaultThinkingLevel(): ThinkingLevel | undefined { return
    /// this.settings.defaultThinkingLevel; }`).
    ///
    /// **Returns `None` when unset, exactly as upstream returns `undefined`.** CFG-056: this used
    /// to end `.unwrap_or_default()`, collapsing "unset" into `ModelThinkingLevel::default()` =
    /// `Off` at the settings layer — so every user who had never written the key started every
    /// session with reasoning DISABLED where Pi starts at
    /// [`crate::DEFAULT_THINKING_LEVEL`] (`medium`). Keeping the `Option` is the mechanism that
    /// keeps it correct: it forces each consumer to spell `?? DEFAULT_THINKING_LEVEL` in the
    /// source the way Pi's six call sites do, instead of hiding the choice inside a `Default` impl
    /// no reviewer of the consumer can see.
    pub fn default_thinking_level(&self) -> Option<ModelThinkingLevel> {
        self.merged
            .get("defaultThinkingLevel")
            .and_then(|v| serde_json::from_value::<ModelThinkingLevel>(v.clone()).ok())
    }

    pub fn hide_thinking_block(&self) -> bool {
        self.merged.get_bool("hideThinkingBlock").unwrap_or(false)
    }

    /// `getShowCacheMissNotices` — `showCacheMissNotices`, default `false` (Pi
    /// settings-manager.ts:96 declares the key, `:850-852` the getter, `:872-875` the setter, which
    /// is cyrup's generic [`SettingsManager::set`] on the GLOBAL scope; upstream's per-key setter
    /// writes `this.globalSettings` too). CFG-014.
    ///
    /// **Consumer half is NOT here and this accessor is not the item's closure.** Pi reads it at
    /// `modes/interactive/interactive-mode.ts:3354` @v0.83.0 to re-inject a per-message cache-miss
    /// notice into the transcript; the detection side already exists at
    /// `cyrup-provider/src/cache_stats.rs`, whose module doc names this key as the missing
    /// prerequisite for PROV-035's wiring half. Landing the accessor unblocks that work — an
    /// accessor with no consumer is a `/settings` row, not a feature.
    pub fn show_cache_miss_notices(&self) -> bool {
        self.merged
            .get_bool("showCacheMissNotices")
            .unwrap_or(false)
    }

    /// `getThemeSetting` — the raw `theme` string, or `None` when unset / not a string
    /// (Pi settings-manager.ts:718-721). No default is applied here.
    pub fn theme_setting(&self) -> Option<String> {
        self.merged.get_str("theme")
    }

    /// `getTheme` — the resolved theme name. A slash-namespaced value (e.g. `a/b`) resolves to
    /// `None` (Pi settings-manager.ts:723-726); the `"dark"` fallback belongs to the theme/TUI
    /// layer, not here.
    pub fn theme(&self) -> Option<String> {
        self.theme_setting().filter(|t| !t.contains('/'))
    }

    /// `defaultProjectTrust` is global-only; stripped from project/cli before merge.
    pub fn default_project_trust(&self) -> DefaultProjectTrust {
        self.merged
            .get("defaultProjectTrust")
            .and_then(|v| serde_json::from_value::<DefaultProjectTrust>(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn external_editor(&self, env: &crate::env::EnvVars) -> String {
        self.merged
            .get_str("externalEditor")
            // Pi only honors the configured editor when it is a non-empty (after-trim) string;
            // empty/whitespace falls through to VISUAL/EDITOR/default
            // (settings-manager.ts:846-848 `getExternalEditorCommand`). The original (untrimmed)
            // value is returned, matching Pi which returns `configuredEditor` verbatim.
            .filter(|s| !s.trim().is_empty())
            .or_else(|| env.visual.clone())
            .or_else(|| env.editor.clone())
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    "notepad".to_string()
                } else {
                    "nano".to_string()
                }
            })
    }

    pub fn enable_install_telemetry(&self) -> bool {
        self.merged
            .get_bool("enableInstallTelemetry")
            .unwrap_or(true)
    }

    pub fn enable_analytics(&self) -> bool {
        self.merged.get_bool("enableAnalytics").unwrap_or(false)
    }

    /// `getEnabledModels(): string[] | undefined` (Pi settings-manager.ts:1133-1135). Returns
    /// `None` when the key is unset — distinct from `Some(vec![])` (an explicit empty list) — so a
    /// consumer can tell "cycle ALL models" (unset) from "cycle NONE" (empty). Collapsing both to
    /// an empty `Vec` (the prior `unwrap_or_default`) lost that distinction.
    pub fn enabled_models(&self) -> Option<Vec<String>> {
        self.merged.get("enabledModels").map(|v| {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// `sessionDir`, tilde-expanded (Pi `getSessionDir` → `normalizePath`, settings-manager.ts:665).
    pub fn session_dir(&self) -> Option<String> {
        self.merged.get_str("sessionDir").map(|s| expand_tilde(&s))
    }

    // --- Typed accessors with Pi's exact defaults (settings-manager.ts:698-1207) ---

    /// `steeringMode` (default `one-at-a-time`; :698-700).
    pub fn steering_mode(&self) -> String {
        self.merged
            .get_str("steeringMode")
            .unwrap_or_else(|| "one-at-a-time".to_string())
    }

    /// `followUpMode` (default `one-at-a-time`; :708-710).
    pub fn follow_up_mode(&self) -> String {
        self.merged
            .get_str("followUpMode")
            .unwrap_or_else(|| "one-at-a-time".to_string())
    }

    /// `transport` (default `auto`; :745-747).
    pub fn transport(&self) -> String {
        self.merged
            .get_str("transport")
            .unwrap_or_else(|| "auto".to_string())
    }

    /// `compaction.enabled` (default true; :755-757).
    pub fn compaction_enabled(&self) -> bool {
        self.merged
            .get_nested_bool(&["compaction", "enabled"])
            .unwrap_or(true)
    }

    /// `compaction.reserveTokens` (default 16384; :768-770).
    pub fn compaction_reserve_tokens(&self) -> i64 {
        self.merged
            .get_nested_i64(&["compaction", "reserveTokens"])
            .unwrap_or(16384)
    }

    /// `compaction.keepRecentTokens` (default 20000; :772-774).
    pub fn compaction_keep_recent_tokens(&self) -> i64 {
        self.merged
            .get_nested_i64(&["compaction", "keepRecentTokens"])
            .unwrap_or(20000)
    }

    /// `branchSummary.reserveTokens` (default 16384; :784-789).
    pub fn branch_summary_reserve_tokens(&self) -> i64 {
        self.merged
            .get_nested_i64(&["branchSummary", "reserveTokens"])
            .unwrap_or(16384)
    }

    /// `branchSummary.skipPrompt` (default false; :791-793).
    pub fn branch_summary_skip_prompt(&self) -> bool {
        self.merged
            .get_nested_bool(&["branchSummary", "skipPrompt"])
            .unwrap_or(false)
    }

    /// `retry.enabled` (default true; :795-797).
    pub fn retry_enabled(&self) -> bool {
        self.merged
            .get_nested_bool(&["retry", "enabled"])
            .unwrap_or(true)
    }

    /// `retry.maxRetries` (default 3; :808-813).
    pub fn retry_max_retries(&self) -> i64 {
        self.merged
            .get_nested_i64(&["retry", "maxRetries"])
            .unwrap_or(3)
    }

    /// `retry.baseDelayMs` (default 2000; :808-813).
    pub fn retry_base_delay_ms(&self) -> i64 {
        self.merged
            .get_nested_i64(&["retry", "baseDelayMs"])
            .unwrap_or(2000)
    }

    /// `retry.provider.maxRetryDelayMs` (default 60000; :829-835).
    pub fn provider_max_retry_delay_ms(&self) -> i64 {
        self.merged
            .get_nested_i64(&["retry", "provider", "maxRetryDelayMs"])
            .unwrap_or(60000)
    }

    /// `httpIdleTimeoutMs`, validated; default 300000 (Pi `getHttpIdleTimeoutMs`, :816-818).
    /// Returns `Err` for a present-but-invalid value (Pi throws in `parseTimeoutSetting`).
    pub fn http_idle_timeout_ms(&self) -> Result<u64, ConfigError> {
        match self.merged.get("httpIdleTimeoutMs") {
            None => Ok(DEFAULT_HTTP_IDLE_TIMEOUT_MS),
            Some(v) => parse_http_idle_timeout_ms(v).ok_or_else(|| {
                ConfigError::Trust(format!("Invalid httpIdleTimeoutMs setting: {v}"))
            }),
        }
    }

    /// `websocketConnectTimeoutMs`, validated; `None` when unset (Pi `getWebSocketConnectTimeoutMs`,
    /// :837-839). Returns `Err` for a present-but-invalid value.
    pub fn websocket_connect_timeout_ms(&self) -> Result<Option<u64>, ConfigError> {
        match self.merged.get("websocketConnectTimeoutMs") {
            None => Ok(None),
            Some(v) => parse_http_idle_timeout_ms(v).map(Some).ok_or_else(|| {
                ConfigError::Trust(format!("Invalid websocketConnectTimeoutMs setting: {v}"))
            }),
        }
    }

    /// `httpProxy` — the SETTING alone, never the ambient environment (CFG-060).
    ///
    /// Pi's only read of this key is `applyHttpProxySettings(bootstrapSettingsManager
    /// .getGlobalSettings().httpProxy)` (`main.ts:537`, again at `:801` @v0.83.0), and
    /// `applyHttpProxySettings` is `const proxy = httpProxy?.trim(); if (!proxy) return;
    /// process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`
    /// (`core/http-dispatcher.ts:43-48` @v0.83.0) — hence the trim and the empty filter here. The
    /// key is declared at `settings-manager.ts:126` and is GLOBAL-only (CFG-057), which is why the
    /// merged view is the right document to read it from.
    ///
    /// **The setting-vs-ambient precedence deliberately does NOT live here.** `??=` lets an ambient
    /// `HTTP_PROXY` win, and cyrup expresses that in the resolver instead — `get_proxy_env`
    /// (`cyrup-provider/src/utils/node_http_proxy.rs`) consults `configured_http_proxy()` only
    /// after all four ambient lookups miss. An earlier version of this accessor took an `EnvVars`
    /// and fell back to `env.http_proxy`, which was dead (both callers passed
    /// `EnvVars::default()`) and wrong the moment it was not: pi writes the SETTING into
    /// `HTTPS_PROXY` even when `HTTP_PROXY` is ambient, so returning the ambient value here would
    /// have installed it as the configured proxy and lost the setting for https targets entirely.
    pub fn http_proxy(&self) -> Option<String> {
        self.merged
            .get_str("httpProxy")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// `shellPath`, tilde-expanded (Pi `getShellPath` → `normalizePath`, settings-manager.ts:883-886).
    ///
    /// The key's own declaration documents the contract — settings-manager.ts:101 `shellPath?:
    /// string; // Custom shell path (e.g., for Cygwin users on Windows); supports leading ~
    /// expansion`. `getSessionDir` (:676-679) and `getShellPath` are the only two getters Pi runs
    /// through `normalizePath`, so this uses the same [`expand_tilde`] as [`Self::session_dir`].
    /// Without it a configured `~/bin/bash` reaches `ShellConfig::resolve` verbatim, fails its
    /// existence check, and breaks every bash invocation.
    pub fn shell_path(&self) -> Option<String> {
        self.merged.get_str("shellPath").map(|s| expand_tilde(&s))
    }

    /// `shellCommandPrefix` (Pi `getShellCommandPrefix`, :894-896).
    pub fn shell_command_prefix(&self) -> Option<String> {
        self.merged.get_str("shellCommandPrefix")
    }

    /// `npmCommand` (Pi `getNpmCommand`, :904-906).
    pub fn npm_command(&self) -> Option<Vec<String>> {
        self.merged
            .get("npmCommand")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
    }

    /// `quietStartup` (default false; :873-875).
    pub fn quiet_startup(&self) -> bool {
        self.merged.get_bool("quietStartup").unwrap_or(false)
    }

    /// `enableSkillCommands` (default true; :1033-1035).
    pub fn enable_skill_commands(&self) -> bool {
        self.merged.get_bool("enableSkillCommands").unwrap_or(true)
    }

    /// `terminal.showImages` (default true; :1047-1049).
    pub fn show_images(&self) -> bool {
        self.merged
            .get_nested_bool(&["terminal", "showImages"])
            .unwrap_or(true)
    }

    /// `terminal.imageWidthCells` (default 60; clamped `max(1, floor)`; :1060-1066).
    pub fn image_width_cells(&self) -> i64 {
        match self.merged.get_nested_f64(&["terminal", "imageWidthCells"]) {
            Some(w) if w.is_finite() => (w.floor() as i64).max(1),
            _ => 60,
        }
    }

    /// `terminal.showTerminalProgress` (default false; :1094-1096).
    pub fn show_terminal_progress(&self) -> bool {
        self.merged
            .get_nested_bool(&["terminal", "showTerminalProgress"])
            .unwrap_or(false)
    }

    /// `images.autoResize` (default true; :1107-1109).
    pub fn image_auto_resize(&self) -> bool {
        self.merged
            .get_nested_bool(&["images", "autoResize"])
            .unwrap_or(true)
    }

    /// `images.blockImages` (default false; :1120-1122).
    pub fn block_images(&self) -> bool {
        self.merged
            .get_nested_bool(&["images", "blockImages"])
            .unwrap_or(false)
    }

    /// `doubleEscapeAction` (default `tree`; :1143-1145).
    pub fn double_escape_action(&self) -> String {
        self.merged
            .get_str("doubleEscapeAction")
            .unwrap_or_else(|| "tree".to_string())
    }

    /// `treeFilterMode` (validated; default `default`; :1153-1157).
    pub fn tree_filter_mode(&self) -> String {
        const VALID: &[&str] = &["default", "no-tools", "user-only", "labeled-only", "all"];
        match self.merged.get_str("treeFilterMode") {
            Some(m) if VALID.contains(&m.as_str()) => m,
            _ => "default".to_string(),
        }
    }

    /// `editorPaddingX` (default 0; :1175-1177).
    pub fn editor_padding_x(&self) -> i64 {
        self.merged.get_nested_i64(&["editorPaddingX"]).unwrap_or(0)
    }

    /// `outputPad` — horizontal padding for user/assistant/thinking chat output, `0` or `1`, default
    /// `1` (Pi `getOutputPad`, settings-manager.ts:1186-1188: `outputPad === 0 ? 0 : 1`). Any value
    /// other than an explicit `0` (unset, `1`, or a stray value) resolves to `1`.
    pub fn output_pad(&self) -> i64 {
        if self.merged.get_nested_i64(&["outputPad"]) == Some(0) {
            0
        } else {
            1
        }
    }

    /// `autocompleteMaxVisible` (default 5; :1185-1187).
    pub fn autocomplete_max_visible(&self) -> i64 {
        self.merged
            .get_nested_i64(&["autocompleteMaxVisible"])
            .unwrap_or(5)
    }

    /// `markdown.codeBlockIndent` (default two spaces; :1195-1197).
    pub fn code_block_indent(&self) -> String {
        self.merged
            .get_nested_str(&["markdown", "codeBlockIndent"])
            .unwrap_or_else(|| "  ".to_string())
    }

    /// `markdown.mermaid` — how mermaid fences are rendered (Pi `getMermaidRenderingMode`,
    /// settings-manager.ts:1251-1254 @v0.84.1). pi VALIDATES rather than parses: `mode === "off" ||
    /// mode === "final" ? mode : "streaming"`, so an unknown value and an absent key both fall to
    /// `Streaming`. CFG-040.
    pub fn mermaid_rendering_mode(&self) -> MermaidRenderingMode {
        match self
            .merged
            .get_nested_str(&["markdown", "mermaid"])
            .as_deref()
        {
            Some("off") => MermaidRenderingMode::Off,
            Some("final") => MermaidRenderingMode::Final,
            _ => MermaidRenderingMode::Streaming,
        }
    }

    /// `showHardwareCursor` — the setting takes precedence, then the
    /// `CYRUP_HARDWARE_CURSOR`/`PI_HARDWARE_CURSOR` env (true only when exactly `"1"`), else false
    /// (Pi settings-manager.ts:1165-1167).
    pub fn show_hardware_cursor(&self, env: &crate::env::EnvVars) -> bool {
        self.merged
            .get_bool("showHardwareCursor")
            .unwrap_or(env.hardware_cursor)
    }

    /// `clearOnShrink` — the `terminal.clearOnShrink` setting takes precedence, then the
    /// `CYRUP_CLEAR_ON_SHRINK`/`PI_CLEAR_ON_SHRINK` env (true only when exactly `"1"`), else false
    /// (Pi `getClearOnShrink`, settings-manager.ts:1077-1083).
    pub fn clear_on_shrink(&self, env: &crate::env::EnvVars) -> bool {
        self.merged
            .get_nested_bool(&["terminal", "clearOnShrink"])
            .unwrap_or(env.clear_on_shrink)
    }

    /// `thinkingBudgets` — custom per-level token budgets, or `None` when unset (Pi
    /// `getThinkingBudgets`, settings-manager.ts:1043-1045). Pi returns the raw object as-is
    /// (loose typing): a single malformed field does NOT discard the others. Parse field-wise so a
    /// bad value for one level leaves that field `None` while the other valid levels survive,
    /// instead of collapsing the entire object to `None` (whole-object `from_value`).
    pub fn thinking_budgets(&self) -> Option<ThinkingBudgets> {
        let obj = self.merged.get("thinkingBudgets")?.as_object()?;
        Some(ThinkingBudgets {
            minimal: obj.get("minimal").and_then(Value::as_i64),
            low: obj.get("low").and_then(Value::as_i64),
            medium: obj.get("medium").and_then(Value::as_i64),
            high: obj.get("high").and_then(Value::as_i64),
        })
    }

    /// `getWarnings` — the warnings object (a shallow copy; empty when unset) (Pi
    /// settings-manager.ts:1199-1201, `{ ...(this.settings.warnings ?? {}) }`). Like Pi's loose
    /// typing, parse field-wise: a malformed field falls back to its default without discarding the
    /// rest of the object (whole-object `from_value` would collapse it all to `Default`).
    pub fn warnings(&self) -> Warnings {
        match self.merged.get("warnings").and_then(Value::as_object) {
            Some(obj) => Warnings {
                anthropic_extra_usage: obj.get("anthropicExtraUsage").and_then(Value::as_bool),
            },
            None => Warnings::default(),
        }
    }

    /// `retry.provider.timeoutMs` — SDK/provider request timeout, or `None` when unset (Pi
    /// `getProviderRetrySettings.timeoutMs`, settings-manager.ts:830).
    pub fn provider_retry_timeout_ms(&self) -> Option<i64> {
        self.merged
            .get_nested_i64(&["retry", "provider", "timeoutMs"])
    }

    /// `retry.provider.maxRetries` — SDK/provider retry attempts, or `None` when unset (Pi
    /// `getProviderRetrySettings.maxRetries`, settings-manager.ts:831).
    pub fn provider_retry_max_retries(&self) -> Option<i64> {
        self.merged
            .get_nested_i64(&["retry", "provider", "maxRetries"])
    }

    /// `getProviderRetrySettings` combined (Pi settings-manager.ts:829-835).
    pub fn provider_retry_settings(&self) -> ProviderRetrySettings {
        ProviderRetrySettings {
            timeout_ms: self.provider_retry_timeout_ms(),
            max_retries: self.provider_retry_max_retries(),
            max_retry_delay_ms: self.provider_max_retry_delay_ms(),
        }
    }

    /// `getBranchSummarySettings` combined (Pi settings-manager.ts:784-789).
    pub fn branch_summary_settings(&self) -> BranchSummarySettings {
        BranchSummarySettings {
            reserve_tokens: self.branch_summary_reserve_tokens(),
            skip_prompt: self.branch_summary_skip_prompt(),
        }
    }

    /// `getCompactionSettings` combined (Pi settings-manager.ts:776-782).
    pub fn compaction_settings(&self) -> CompactionSettings {
        CompactionSettings {
            enabled: self.compaction_enabled(),
            reserve_tokens: self.compaction_reserve_tokens(),
            keep_recent_tokens: self.compaction_keep_recent_tokens(),
        }
    }

    /// `getRetrySettings` combined (Pi settings-manager.ts:808-814).
    pub fn retry_settings(&self) -> RetrySettings {
        RetrySettings {
            enabled: self.retry_enabled(),
            max_retries: self.retry_max_retries(),
            base_delay_ms: self.retry_base_delay_ms(),
        }
    }

    /// Read a `Vec<String>` settings array, with Pi's empty-array default for an absent/non-array
    /// value (`[...(this.settings.x ?? [])]`).
    fn string_list(&self, key: &str) -> Vec<String> {
        self.merged
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `getPackages` — configured npm/git package sources (empty default; Pi `getPackages`,
    /// settings-manager.ts:969-971 @v0.83.0, which is `[...(this.settings.packages ?? [])]`).
    ///
    /// CFG-061: upstream copies the array verbatim and never parses it, so a malformed entry is
    /// carried forward and rejected INDIVIDUALLY downstream. This delegates to
    /// [`Settings::packages_with_errors`], which is the per-entry port; the blanket
    /// `from_value::<Vec<_>>().ok().unwrap_or_default()` that used to sit here turned one typo in a
    /// ten-entry array into "no packages configured" for all ten.
    pub fn packages(&self) -> Vec<PackageSource> {
        self.merged.packages()
    }

    /// [`Self::packages`] with the per-entry diagnostics upstream's downstream rejection produces
    /// (CFG-061). The live discovery path composes the per-LAYER twin
    /// (`Settings::packages_with_errors`); this is the merged-view equivalent.
    pub fn packages_with_errors(&self) -> (Vec<PackageSource>, Vec<String>) {
        self.merged.packages_with_errors()
    }

    /// `getExtensionPaths` — local extension file/dir paths (empty default; Pi
    /// settings-manager.ts:969-971).
    pub fn extension_paths(&self) -> Vec<String> {
        self.string_list("extensions")
    }

    /// `getSkillPaths` — local skill file/dir paths (empty default; Pi settings-manager.ts:985-987).
    pub fn skill_paths(&self) -> Vec<String> {
        self.string_list("skills")
    }

    /// `getPromptTemplatePaths` — local prompt-template paths (empty default; Pi
    /// settings-manager.ts:1001-1003).
    pub fn prompt_template_paths(&self) -> Vec<String> {
        self.string_list("prompts")
    }

    /// `getThemePaths` — local theme file/dir paths (empty default; Pi
    /// settings-manager.ts:1017-1019).
    pub fn theme_paths(&self) -> Vec<String> {
        self.string_list("themes")
    }

    /// `trackingId` (Pi `getTrackingId`, :938-940).
    pub fn tracking_id(&self) -> Option<String> {
        self.merged.get_str("trackingId")
    }

    /// `lastChangelogVersion` (Pi `getLastChangelogVersion`, :655-657).
    pub fn last_changelog_version(&self) -> Option<String> {
        self.merged.get_str("lastChangelogVersion")
    }

    /// `collapseChangelog` (default false; :914-916).
    pub fn collapse_changelog(&self) -> bool {
        self.merged.get_bool("collapseChangelog").unwrap_or(false)
    }
}

/// Default HTTP idle timeout (Pi `DEFAULT_HTTP_IDLE_TIMEOUT_MS`, http-dispatcher.ts:3).
pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: u64 = 300_000;

/// Port of Pi `parseHttpIdleTimeoutMs` (http-dispatcher.ts:16-32): accepts a non-negative finite
/// number or a numeric/`"disabled"` string; floors to ms. `None` = unset/empty (caller defaults).
pub fn parse_http_idle_timeout_ms(value: &Value) -> Option<u64> {
    match value {
        Value::String(s) => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("disabled") {
                return Some(0);
            }
            if t.is_empty() {
                return None;
            }
            let n: f64 = t.parse().ok()?;
            if !n.is_finite() || n < 0.0 {
                return None;
            }
            Some(n.floor() as u64)
        }
        Value::Number(n) => {
            let f = n.as_f64()?;
            if !f.is_finite() || f < 0.0 {
                return None;
            }
            Some(f.floor() as u64)
        }
        _ => None,
    }
}

/// Tilde-expand a path string. Thin alias for the shared [`crate::paths::normalize_path`], which
/// is the whole of Pi `normalizePath` (paths.ts:57-78 @v0.83.0) — `~` / `~/` / win32 `~\\` AND
/// `file://` — rather than the tilde branch alone. Kept as a name because two getters below read
/// better with it.
fn expand_tilde(input: &str) -> String {
    crate::paths::normalize_path(input)
}

/// Serialized read-modify-write of one scope's raw JSON text (arch-07 §3.3).
pub trait SettingsStore: Send + Sync {
    /// Read the current raw text for a scope (`None` if absent).
    fn read(&self, scope: SettingsScope) -> Result<Option<String>, ConfigError>;

    /// Serialized read-modify-write. `f` receives the current text (None if absent) and returns
    /// `Some(new)` to write or `None` to leave untouched.
    fn with_lock(
        &self,
        scope: SettingsScope,
        f: &mut dyn FnMut(Option<&str>) -> Option<String>,
    ) -> Result<(), ConfigError>;
}

/// File-backed store with a cross-process advisory lock (arch-07 §5).
pub struct FileSettingsStore {
    global_path: PathBuf,
    project_path: PathBuf,
}

impl FileSettingsStore {
    pub fn new(global_path: PathBuf, project_path: PathBuf) -> Self {
        Self {
            global_path,
            project_path,
        }
    }

    fn path(&self, scope: SettingsScope) -> &Path {
        match scope {
            SettingsScope::Global => &self.global_path,
            SettingsScope::Project => &self.project_path,
        }
    }
}

impl SettingsStore for FileSettingsStore {
    fn read(&self, scope: SettingsScope) -> Result<Option<String>, ConfigError> {
        match std::fs::read_to_string(self.path(scope)) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ConfigError::Io(e)),
        }
    }

    fn with_lock(
        &self,
        scope: SettingsScope,
        f: &mut dyn FnMut(Option<&str>) -> Option<String>,
    ) -> Result<(), ConfigError> {
        let path = self.path(scope).to_path_buf();
        let _guard = crate::lock::FileLock::acquire(&path)?;
        let current = match std::fs::read_to_string(&path) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(ConfigError::Io(e)),
        };
        if let Some(new_text) = f(current.as_deref()) {
            crate::lock::write_atomic(&path, new_text.as_bytes(), false)?;
        }
        Ok(())
    }
}

/// In-memory store for tests / non-persistent runs.
#[derive(Default)]
pub struct InMemorySettingsStore {
    global: std::sync::Mutex<Option<String>>,
    project: std::sync::Mutex<Option<String>>,
}

impl InMemorySettingsStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, scope: SettingsScope) -> &std::sync::Mutex<Option<String>> {
        match scope {
            SettingsScope::Global => &self.global,
            SettingsScope::Project => &self.project,
        }
    }

    pub fn seed(&self, scope: SettingsScope, text: &str) {
        if let Ok(mut g) = self.slot(scope).lock() {
            *g = Some(text.to_string());
        }
    }
}

impl SettingsStore for InMemorySettingsStore {
    fn read(&self, scope: SettingsScope) -> Result<Option<String>, ConfigError> {
        Ok(self.slot(scope).lock().ok().and_then(|g| g.clone()))
    }

    fn with_lock(
        &self,
        scope: SettingsScope,
        f: &mut dyn FnMut(Option<&str>) -> Option<String>,
    ) -> Result<(), ConfigError> {
        let mut guard = self
            .slot(scope)
            .lock()
            .map_err(|_| ConfigError::Trust("poisoned lock".into()))?;
        if let Some(new) = f(guard.as_deref()) {
            *guard = Some(new);
        }
        Ok(())
    }
}

/// The layered settings facade (arch-07 §3.3). Holds the two layers + a memoized merge.
///
/// **Exactly two persistent layers, `global ◁ project`, matching pi's
/// `this.settings = deepMergeSettings(this.globalSettings, this.projectSettings)`
/// (settings-manager.ts:305, and again at `:466`, `:503` @v0.83.0).** There is no CLI tier
/// upstream: the ONLY way to push a value above the project layer is the transient
/// [`Self::apply_overrides`] (`applyOverrides`, settings-manager.ts:508-510), which merges onto the
/// already-merged view and is discarded by the next recompute. CFG-059 removed a third persistent
/// `cli` layer that sat above project, was stripped by `strip_global_only`, and survived every
/// `reload()` / `set_project_trusted()` — three properties upstream's override path does not have.
pub struct SettingsManager {
    store: Arc<dyn SettingsStore>,
    global: Settings,
    project: Settings,
    effective: EffectiveSettings,
    project_trusted: bool,
    load_errors: Vec<ScopedError>,
    /// The LAST load's failure for the global scope, if any (Pi `globalSettingsLoadError`,
    /// settings-manager.ts:289). Distinct from `load_errors`, which accumulates across reloads and
    /// is drained once for display: this is the live per-scope latch every writer consults so a
    /// document cyrup could not read is never rewritten from the degraded in-memory view (CFG-001).
    global_load_error: Option<String>,
    /// The LAST load's failure for the project scope (Pi `projectSettingsLoadError`,
    /// settings-manager.ts:290). Always `None` while the project is untrusted — that scope is not
    /// read at all, and its writes are already refused with [`ConfigError::Untrusted`].
    project_load_error: Option<String>,
}

impl SettingsManager {
    /// Load global unconditionally; load project ONLY if `project_trusted` (R-07-002). A parse
    /// error degrades that scope to empty and records a `ScopedError` (R-00-009) plus the
    /// per-scope write latch (CFG-001).
    pub fn load(store: Arc<dyn SettingsStore>, project_trusted: bool) -> Self {
        let mut mgr = Self {
            store,
            global: Settings::default(),
            project: Settings::default(),
            effective: EffectiveSettings::default(),
            project_trusted,
            load_errors: Vec::new(),
            global_load_error: None,
            project_load_error: None,
        };
        mgr.reload_internal();
        mgr
    }

    /// Record this load's failure both in the drainable log and in the per-scope write latch.
    ///
    /// Pi latches on ANY failure of `loadFromStorage`, not only a JSON syntax error:
    /// `tryLoadFromStorage` (settings-manager.ts:373-383) wraps the whole load in one try/catch and
    /// hands the caught error straight to `globalSettingsLoadError`/`projectSettingsLoadError`. So a
    /// store READ failure latches here too — an unreadable file is exactly as unsafe to overwrite
    /// as an unparseable one.
    fn record_load_error(&mut self, scope: SettingsScope, message: String) {
        match scope {
            SettingsScope::Global => self.global_load_error = Some(message.clone()),
            SettingsScope::Project => self.project_load_error = Some(message.clone()),
        }
        self.load_errors.push(ScopedError { scope, message });
    }

    fn load_scope(&mut self, scope: SettingsScope) -> Settings {
        match self.store.read(scope) {
            Ok(Some(text)) => match Settings::parse(&text) {
                Ok(s) => s,
                Err(e) => {
                    self.record_load_error(scope, format!("parse error: {e}"));
                    Settings::default()
                }
            },
            Ok(None) => Settings::default(),
            Err(e) => {
                self.record_load_error(scope, e.to_string());
                Settings::default()
            }
        }
    }

    fn reload_internal(&mut self) {
        // Clear the latches first: they describe the load that is about to happen, so a user who
        // repairs the file and reloads regains the ability to write (Pi sets both fields from the
        // fresh `tryLoadFromStorage` result on every reload, settings-manager.ts:477/489-491/503-505).
        self.global_load_error = None;
        self.project_load_error = None;
        self.global = self.load_scope(SettingsScope::Global);
        self.project = if self.project_trusted {
            self.load_scope(SettingsScope::Project)
        } else {
            Settings::default()
        };
        self.recompute();
    }

    fn recompute(&mut self) {
        // Strip global-only keys from project before merge.
        let mut project = self.project.clone();
        strip_global_only(&mut project);

        let merged = deep_merge(&self.global.to_value(), &project.to_value());
        let merged = match merged {
            Value::Object(obj) => Settings::from_map(obj),
            _ => Settings::default(),
        };
        self.effective = EffectiveSettings::from_settings(merged);
    }

    pub fn effective(&self) -> &EffectiveSettings {
        &self.effective
    }

    pub fn global(&self) -> &Settings {
        &self.global
    }

    pub fn project(&self) -> &Settings {
        &self.project
    }

    pub fn project_trusted(&self) -> bool {
        self.project_trusted
    }

    /// Re-read both scopes from disk and rebuild the effective view (R-07-005/029).
    pub fn reload(&mut self) -> Result<(), ConfigError> {
        self.reload_internal();
        Ok(())
    }

    /// `applyOverrides` (Pi settings-manager.ts:508-510): deep-merge additional overrides on top of
    /// the current effective settings at runtime. These overrides are NOT persisted and are
    /// transient — any subsequent `reload`/`set_project_trusted` recomputes the merge from the
    /// global/project layers and discards them (matching Pi, where `applyOverrides` mutates the
    /// in-memory `this.settings` that `reload()` later rebuilds at `:503`).
    ///
    /// This is the ONLY override path above the project layer, and the seam an embedder or a test
    /// harness uses — upstream's own callers are `examples/sdk/10-settings.ts:17`,
    /// `test/test-harness.ts:395` (`applyOverrides(options.settings)`) and
    /// `test/utilities.ts:258` (`options.settingsOverrides`); there are ZERO production callers at
    /// v0.83.0, which is why CFG-059 deleted cyrup's persistent `cli` tier rather than keeping it.
    ///
    /// Global-only keys are stripped from the overrides. Upstream expresses global-only-ness at the
    /// GETTER — `getGlobalSettings()` returns `this.globalSettings`, never `this.settings`
    /// (settings-manager.ts:442-444) — so `applyOverrides`, which only ever touches `this.settings`,
    /// cannot influence `defaultProjectTrust` or `httpProxy` upstream either. cyrup implements the
    /// same guarantee at the merge, so the strip has to happen on this path too or CFG-057's fix
    /// would be reachable through an override.
    pub fn apply_overrides(&mut self, overrides: &Settings) {
        let mut overrides = overrides.clone();
        strip_global_only(&mut overrides);
        let merged = deep_merge(&self.effective.raw().to_value(), &overrides.to_value());
        let merged = match merged {
            Value::Object(obj) => Settings::from_map(obj),
            _ => Settings::default(),
        };
        self.effective = EffectiveSettings::from_settings(merged);
    }

    /// Trust transition: false→true loads the project scope; true→false drops it (R-07-002/012).
    pub fn set_project_trusted(&mut self, trusted: bool) {
        if self.project_trusted != trusted {
            self.project_trusted = trusted;
            self.reload_internal();
        }
    }

    /// The load-error latch for `scope`, if the last load of that scope failed (CFG-001).
    ///
    /// A front-end can read this before offering an edit UI so it can explain the situation up
    /// front rather than after a refused write.
    pub fn load_error(&self, scope: SettingsScope) -> Option<&str> {
        match scope {
            SettingsScope::Global => self.global_load_error.as_deref(),
            SettingsScope::Project => self.project_load_error.as_deref(),
        }
    }

    /// Refuse a write into a scope whose last load failed (CFG-001).
    ///
    /// Every writer opens with this, mirroring Pi's `save()` / `saveProjectSettings()` guards
    /// (settings-manager.ts ≈:614-628 / ≈:633-646). Without it a single typo — a trailing comma, an
    /// unclosed brace — turns the very next `/config` toggle, `/theme`, analytics opt-in, or
    /// `set_editor_padding_x` into a total rewrite of the file as `{"<key>": <value>}`, silently
    /// destroying every other setting the user had.
    fn ensure_scope_writable(&self, scope: SettingsScope) -> Result<(), ConfigError> {
        match self.load_error(scope) {
            Some(message) => Err(ConfigError::SettingsWriteRefused {
                scope,
                message: message.to_string(),
            }),
            None => Ok(()),
        }
    }

    /// Persist a single field via scoped read-modify-write that re-reads the on-disk file and
    /// applies only the modified field (concurrent-edit safe; R-07-004). Project writes require
    /// trust.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::Untrusted`] for a project write in an untrusted folder.
    /// - [`ConfigError::SettingsWriteRefused`] if that scope's file failed to load, or if it became
    ///   unparseable between the load and this write — the file is left byte-for-byte unchanged
    ///   (CFG-001).
    pub fn set<T: serde::Serialize>(
        &mut self,
        scope: SettingsScope,
        key: &str,
        value: T,
    ) -> Result<(), ConfigError> {
        if scope == SettingsScope::Project && !self.project_trusted {
            return Err(ConfigError::Untrusted);
        }
        self.ensure_scope_writable(scope)?;
        let json = serde_json::to_value(value)?;
        let key_owned = key.to_string();
        let mut corrupt: Option<String> = None;
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                // Absent file: create it. This is the ONLY branch that may start from an empty doc.
                None => Settings::default(),
                // Corruption that appeared BETWEEN the load and this locked write. Returning `None`
                // leaves the file untouched; the message is surfaced below so the caller can tell
                // the write did not happen (CFG-001).
                Some(Err(e)) => {
                    corrupt = Some(format!("parse error: {e}"));
                    return None;
                }
            };
            // CFG-062 — "clear" means the key is GONE, not present-and-null. Pi's clearing setters
            // assign `undefined` (`setShellPath` settings-manager.ts:883-887, `setShellCommandPrefix`
            // :914-918, `setNpmCommand` :924-928 @v0.83.0) and `persistScopedSettings` serializes
            // through `JSON.stringify(mergedSettings, null, 2)` (:605), which OMITS
            // undefined-valued properties. `serde_json` has no `undefined`, so `None::<String>`
            // arrives here as `Value::Null` and used to persist as `"shellPath": null` — a value
            // upstream cannot write, and one that a lower layer's `deep_merge` treats as a real
            // override (both sides let a project `null` blank a global value, so the divergence is
            // the WRITE, not the merge).
            if json.is_null() {
                doc.obj.remove(&key_owned);
            } else {
                doc.obj.insert(key_owned.clone(), json.clone());
            }
            Some(doc.to_pretty())
        })?;
        if let Some(message) = corrupt {
            return Err(ConfigError::SettingsWriteRefused { scope, message });
        }
        self.reload_internal();
        Ok(())
    }

    /// Persist a nested field (e.g. `terminal.showImages`) via scoped read-modify-write, creating
    /// intermediate objects and PRESERVING sibling nested keys (Pi `persistScopedSettings` nested
    /// tracking, settings-manager.ts:573-602). Unlike [`Self::set`], this never clobbers the rest of
    /// the parent object. Project writes require trust.
    ///
    /// # Errors
    ///
    /// Same as [`Self::set`], including the [`ConfigError::SettingsWriteRefused`] refusal that keeps
    /// an unparseable file intact (CFG-001).
    pub fn set_nested(
        &mut self,
        scope: SettingsScope,
        path: &[&str],
        value: Value,
    ) -> Result<(), ConfigError> {
        if path.is_empty() {
            return Ok(());
        }
        if scope == SettingsScope::Project && !self.project_trusted {
            return Err(ConfigError::Untrusted);
        }
        self.ensure_scope_writable(scope)?;
        let path_owned: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        let mut corrupt: Option<String> = None;
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                None => Settings::default(),
                Some(Err(e)) => {
                    corrupt = Some(format!("parse error: {e}"));
                    return None;
                }
            };
            set_value_at_path(&mut doc.obj, &path_owned, value.clone());
            Some(doc.to_pretty())
        })?;
        if let Some(message) = corrupt {
            return Err(ConfigError::SettingsWriteRefused { scope, message });
        }
        self.reload_internal();
        Ok(())
    }

    /// Persist a nested field to the on-disk store via scoped read-modify-write **without** updating
    /// the in-memory merged view (an additive `&self` write seam: `set_nested` requires `&mut self`
    /// because it reloads, but a front-end holding the manager behind an `Arc` — the TUI `/config`
    /// selector — drives a `/reload` afterward, exactly as Pi's settings selector applies-then-reloads,
    /// settings-manager.ts:573). The change becomes visible in `effective()` after the next
    /// [`Self::reload`]. Project writes still require trust (R-07-004).
    ///
    /// # Errors
    ///
    /// Same as [`Self::set`], including the [`ConfigError::SettingsWriteRefused`] refusal that keeps
    /// an unparseable file intact (CFG-001). This is the seam the TUI `/config` selector and the
    /// `cyrup config` subcommand drive, and both already surface the returned error to the user.
    pub fn persist_nested(
        &self,
        scope: SettingsScope,
        path: &[&str],
        value: Value,
    ) -> Result<(), ConfigError> {
        if path.is_empty() {
            return Ok(());
        }
        if scope == SettingsScope::Project && !self.project_trusted {
            return Err(ConfigError::Untrusted);
        }
        self.ensure_scope_writable(scope)?;
        let path_owned: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        let mut corrupt: Option<String> = None;
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                None => Settings::default(),
                Some(Err(e)) => {
                    corrupt = Some(format!("parse error: {e}"));
                    return None;
                }
            };
            set_value_at_path(&mut doc.obj, &path_owned, value.clone());
            Some(doc.to_pretty())
        })?;
        if let Some(message) = corrupt {
            return Err(ConfigError::SettingsWriteRefused { scope, message });
        }
        Ok(())
    }

    /// `setMermaidRenderingMode` (Pi settings-manager.ts:1257-1262 @v0.84.1): writes the GLOBAL
    /// scope through `markdown.mermaid`, so a sibling `markdown.codeBlockIndent` survives — pi does
    /// `this.globalSettings.markdown ??= {}` and assigns one key, never replacing the block.
    /// CFG-040.
    pub fn set_mermaid_rendering_mode(
        &mut self,
        mode: MermaidRenderingMode,
    ) -> Result<(), ConfigError> {
        self.set_nested(
            SettingsScope::Global,
            &["markdown", "mermaid"],
            Value::String(mode.as_str().to_string()),
        )
    }

    /// `setEditorPaddingX`: clamp to 0..=3 (Pi settings-manager.ts:1179-1183).
    pub fn set_editor_padding_x(&mut self, padding: f64) -> Result<(), ConfigError> {
        let clamped = (padding.floor() as i64).clamp(0, 3);
        self.set(SettingsScope::Global, "editorPaddingX", clamped)
    }

    /// `setAutocompleteMaxVisible`: clamp to 3..=20 (Pi settings-manager.ts:1189-1193).
    pub fn set_autocomplete_max_visible(&mut self, max_visible: f64) -> Result<(), ConfigError> {
        let clamped = (max_visible.floor() as i64).clamp(3, 20);
        self.set(SettingsScope::Global, "autocompleteMaxVisible", clamped)
    }

    /// `setImageWidthCells`: floor and clamp to >=1 (Pi settings-manager.ts:1068-1075).
    pub fn set_image_width_cells(&mut self, width: f64) -> Result<(), ConfigError> {
        let clamped = (width.floor() as i64).max(1);
        self.set_nested(
            SettingsScope::Global,
            &["terminal", "imageWidthCells"],
            clamped.into(),
        )
    }

    /// `setHttpIdleTimeoutMs`: reject non-finite/negative; floor (Pi settings-manager.ts:820-827).
    pub fn set_http_idle_timeout_ms(&mut self, timeout_ms: f64) -> Result<(), ConfigError> {
        if !timeout_ms.is_finite() || timeout_ms < 0.0 {
            return Err(ConfigError::Trust(format!(
                "Invalid httpIdleTimeoutMs setting: {timeout_ms}"
            )));
        }
        self.set(
            SettingsScope::Global,
            "httpIdleTimeoutMs",
            timeout_ms.floor() as i64,
        )
    }

    /// `setShowImages` (nested `terminal.showImages`; Pi settings-manager.ts:1051-1058).
    pub fn set_show_images(&mut self, show: bool) -> Result<(), ConfigError> {
        self.set_nested(
            SettingsScope::Global,
            &["terminal", "showImages"],
            show.into(),
        )
    }

    /// `setEnableAnalytics`: set the opt-in flag and, on first opt-in, generate a `trackingId`
    /// (randomUUID) if absent (Pi settings-manager.ts:943-951). Both fields land in one write.
    ///
    /// Does not route through [`Self::set`] (it writes two keys under one lock), so it carries its
    /// own copy of the CFG-001 guard.
    pub fn set_enable_analytics(&mut self, enabled: bool) -> Result<(), ConfigError> {
        self.ensure_scope_writable(SettingsScope::Global)?;
        let mut corrupt: Option<String> = None;
        self.store
            .with_lock(SettingsScope::Global, &mut |current| {
                let mut doc = match current.map(Settings::parse) {
                    Some(Ok(s)) => s,
                    None => Settings::default(),
                    Some(Err(e)) => {
                        corrupt = Some(format!("parse error: {e}"));
                        return None;
                    }
                };
                doc.obj
                    .insert("enableAnalytics".to_string(), Value::Bool(enabled));
                let has_tracking = doc
                    .obj
                    .get("trackingId")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty());
                if enabled && !has_tracking {
                    doc.obj
                        .insert("trackingId".to_string(), Value::String(random_uuid_v4()));
                }
                Some(doc.to_pretty())
            })?;
        if let Some(message) = corrupt {
            return Err(ConfigError::SettingsWriteRefused {
                scope: SettingsScope::Global,
                message,
            });
        }
        self.reload_internal();
        Ok(())
    }

    pub fn drain_load_errors(&mut self) -> Vec<ScopedError> {
        std::mem::take(&mut self.load_errors)
    }
}

/// Set `value` at a nested object path, creating intermediate objects and replacing any
/// non-object on the way (Pi nested setters create `{}` then assign).
fn set_value_at_path(map: &mut Map<String, Value>, path: &[String], value: Value) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        // CFG-062, nested leg. `persistScopedSettings` writes the nested object through the same
        // `JSON.stringify(mergedSettings, null, 2)` (settings-manager.ts:605 @v0.83.0), so an
        // undefined nested field is omitted at depth exactly as a top-level one is. A `Null` leaf
        // therefore CLEARS the key rather than persisting `"terminal": { "showImages": null }`.
        if value.is_null() {
            map.remove(first);
        } else {
            map.insert(first.clone(), value);
        }
        return;
    }
    let entry = map
        .entry(first.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Value::Object(child) = entry {
        set_value_at_path(child, rest, value);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// `shellPath` supports a leading `~` (settings-manager.ts:101), which Pi honors by running the
    /// getter through `normalizePath` (`getShellPath`, settings-manager.ts:883-886) exactly as it
    /// does for `sessionDir`. Regression guard for CFG-031: the raw `~/bin/bash` reached
    /// `ShellConfig::resolve`, failed `Path::exists`, and broke every bash command.
    #[test]
    fn shell_path_is_tilde_expanded_like_session_dir() {
        let Some(home) = directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        else {
            return; // no home on this host; expansion is a no-op by contract
        };

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "shellPath": "~/bin/bash", "sessionDir": "~/sessions" }"#,
        );
        let mgr = SettingsManager::load(store, true);
        let effective = mgr.effective();

        let shell = effective.shell_path().expect("shellPath is configured");
        assert!(
            !shell.starts_with('~'),
            "shellPath must be tilde-expanded before it reaches the shell resolver, got {shell}"
        );
        assert_eq!(shell, home.join("bin/bash").to_string_lossy());
        // Same treatment as the sibling getter Pi normalizes.
        assert_eq!(
            effective.session_dir().as_deref(),
            Some(home.join("sessions").to_string_lossy().as_ref())
        );

        // A bare `~` expands to the home dir itself, and an absolute path is untouched.
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "shellPath": "~" }"#);
        let mgr = SettingsManager::load(store, true);
        assert_eq!(
            mgr.effective().shell_path().as_deref(),
            Some(home.to_string_lossy().as_ref())
        );

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "shellPath": "/bin/zsh" }"#);
        let mgr = SettingsManager::load(store, true);
        assert_eq!(mgr.effective().shell_path().as_deref(), Some("/bin/zsh"));
    }

    #[test]
    fn external_editor_blank_setting_falls_through() {
        // Pi `getExternalEditorCommand` (settings-manager.ts:846-848) only honors a configured
        // editor when it is a non-empty (after-trim) string; empty/whitespace falls through.
        let env = crate::env::EnvVars {
            visual: Some("vim".to_string()),
            ..Default::default()
        };
        let default_editor = if cfg!(windows) { "notepad" } else { "nano" };

        // whitespace-only configured editor is treated as unset -> VISUAL
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "externalEditor": "   " }"#);
        let mgr = SettingsManager::load(store, true);
        assert_eq!(mgr.effective().external_editor(&env), "vim");

        // empty-string configured editor is treated as unset -> VISUAL
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "externalEditor": "" }"#);
        let mgr = SettingsManager::load(store, true);
        assert_eq!(mgr.effective().external_editor(&env), "vim");

        // empty configured editor with no VISUAL/EDITOR -> platform default
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "externalEditor": "  " }"#);
        let mgr = SettingsManager::load(store, true);
        assert_eq!(
            mgr.effective()
                .external_editor(&crate::env::EnvVars::default()),
            default_editor
        );

        // a non-blank configured editor wins (returned verbatim, including surrounding content)
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "externalEditor": "code --wait" }"#,
        );
        let mgr = SettingsManager::load(store, true);
        assert_eq!(mgr.effective().external_editor(&env), "code --wait");
    }

    #[test]
    fn deep_merge_precedence_and_nested() {
        // A-07-1: nested objects merge per-key; arrays replace; CLI > project > global.
        let global = serde_json::json!({
            "defaultModel": "g-model",
            "retry": { "enabled": true, "maxRetries": 3 },
            "list": [1, 2, 3]
        });
        let project = serde_json::json!({
            "defaultModel": "p-model",
            "retry": { "maxRetries": 5 },
            "list": [9]
        });
        let cli = serde_json::json!({ "defaultModel": "c-model" });

        let merged = deep_merge(&global, &project);
        let merged = deep_merge(&merged, &cli);

        assert_eq!(merged["defaultModel"], "c-model");
        // nested per-key merge: enabled kept from global, maxRetries overridden by project
        assert_eq!(merged["retry"]["enabled"], true);
        assert_eq!(merged["retry"]["maxRetries"], 5);
        // arrays replace wholesale
        assert_eq!(merged["list"], serde_json::json!([9]));
    }

    #[test]
    fn unknown_keys_survive_roundtrip() {
        // A-07-8 / R-07-004
        let text = r#"{ "defaultModel": "x", "someFutureKey": { "a": 1 }, "topUnknown": 7 }"#;
        let s = Settings::parse(text).unwrap();
        let out = s.to_pretty();
        let reparsed = Settings::parse(&out).unwrap();
        assert_eq!(
            reparsed.get("someFutureKey"),
            Some(&serde_json::json!({"a": 1}))
        );
        assert_eq!(reparsed.get("topUnknown"), Some(&serde_json::json!(7)));
        assert_eq!(reparsed.get("defaultModel"), Some(&serde_json::json!("x")));
    }

    /// CFG-059 — the precedence MODEL is pi's: exactly two persistent layers, `global ◁ project`,
    /// and the only tier above project is the TRANSIENT `applyOverrides`.
    ///
    /// Presence before absence: the override is first shown to WIN over project (so this is a
    /// statement about precedence, not a dead call), and only then shown not to survive a
    /// recompute. The old shape of this test asserted a third `cli` layer that outranked project
    /// AND persisted; pi has no such tier — `applyOverrides` has exactly two v0.83.0 call sites
    /// (`examples/sdk/10-settings.ts:17`, `test/test-harness.ts:395`) and zero production callers.
    #[test]
    fn project_outranks_global_and_the_only_tier_above_project_is_transient() {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "defaultModel": "g", "theme": "light" }"#,
        );
        store.seed(SettingsScope::Project, r#"{ "defaultModel": "p" }"#);

        // Two layers: project wins for its own key, global still supplies the rest.
        let mut mgr = SettingsManager::load(store.clone(), true);
        assert_eq!(mgr.effective().default_model(), Some("p".to_string()));
        assert_eq!(mgr.effective().theme(), Some("light".to_string()));

        // PRESENCE: an override outranks the project layer while it is applied.
        let mut overrides = Settings::new();
        overrides.set_field("defaultModel", "c").unwrap();
        mgr.apply_overrides(&overrides);
        assert_eq!(mgr.effective().default_model(), Some("c".to_string()));
        assert_eq!(mgr.effective().theme(), Some("light".to_string()));

        // ABSENCE: it is not a layer — every recompute path drops it and project wins again.
        mgr.reload().unwrap();
        assert_eq!(mgr.effective().default_model(), Some("p".to_string()));
        mgr.apply_overrides(&overrides);
        mgr.set_project_trusted(false);
        assert_eq!(mgr.effective().default_model(), Some("g".to_string()));
        mgr.set_project_trusted(true);
        assert_eq!(mgr.effective().default_model(), Some("p".to_string()));
    }

    /// CFG-059 × CFG-057 — an override cannot supply a global-only key. Upstream expresses
    /// global-only-ness at the getter (`getGlobalSettings()` returns `this.globalSettings`,
    /// settings-manager.ts:442-444) and `applyOverrides` only ever touches `this.settings`, so
    /// upstream's override path cannot reach `httpProxy` / `defaultProjectTrust` either. cyrup
    /// implements the same guarantee at the merge, so the strip has to cover this path too.
    #[test]
    fn an_override_cannot_supply_a_global_only_key() {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "httpProxy": "http://global:8080", "defaultModel": "g" }"#,
        );
        let mut mgr = SettingsManager::load(store, false);

        let overrides = Settings::parse(
            r#"{ "httpProxy": "http://override:9", "defaultProjectTrust": "always", "defaultModel": "o" }"#,
        )
        .unwrap();
        mgr.apply_overrides(&overrides);

        // PRESENCE: a non-global-only key from the same override document DID land.
        assert_eq!(mgr.effective().default_model(), Some("o".to_string()));
        // ABSENCE: the two global-only keys did not.
        assert_eq!(
            mgr.effective().http_proxy(),
            Some("http://global:8080".to_string())
        );
        assert_eq!(
            mgr.effective().default_project_trust(),
            DefaultProjectTrust::Ask
        );
    }

    #[test]
    fn per_layer_resource_path_accessors_read_a_single_scope() {
        // gap-09 #26 cross-layer wiring: `global()`/`project()` expose the per-layer split so a
        // consumer (session-svc DiscoveryConfig) can gate global- vs project-scope resource
        // overrides independently — NOT from the merged `effective()` view (which would let a
        // project list silently widen the global scope, or vice-versa).
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "skills": ["g-skill"], "prompts": ["g-prompt"], "themes": ["g-theme"] }"#,
        );
        store.seed(
            SettingsScope::Project,
            r#"{ "skills": ["p-skill-a", "p-skill-b"], "prompts": ["p-prompt"] }"#,
        );
        let mgr = SettingsManager::load(store, true);

        // Each layer reports ONLY its own list (no merge).
        assert_eq!(mgr.global().skill_paths(), vec!["g-skill".to_string()]);
        assert_eq!(
            mgr.project().skill_paths(),
            vec!["p-skill-a".to_string(), "p-skill-b".to_string()]
        );
        assert_eq!(
            mgr.global().prompt_template_paths(),
            vec!["g-prompt".to_string()]
        );
        assert_eq!(
            mgr.project().prompt_template_paths(),
            vec!["p-prompt".to_string()]
        );
        // `themes` set only globally: project layer is empty (NOT inheriting the global value).
        assert_eq!(mgr.global().theme_paths(), vec!["g-theme".to_string()]);
        assert!(mgr.project().theme_paths().is_empty());
        // The merged effective view still unions them (sanity: per-layer != effective).
        assert!(mgr.effective().skill_paths().len() >= mgr.global().skill_paths().len());
    }

    #[test]
    fn project_not_loaded_until_trusted() {
        // R-07-002
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "defaultModel": "g" }"#);
        store.seed(SettingsScope::Project, r#"{ "defaultModel": "p" }"#);

        let mut mgr = SettingsManager::load(store, false);
        assert_eq!(mgr.effective().default_model(), Some("g".to_string()));
        mgr.set_project_trusted(true);
        assert_eq!(mgr.effective().default_model(), Some("p".to_string()));
        mgr.set_project_trusted(false);
        assert_eq!(mgr.effective().default_model(), Some("g".to_string()));
    }

    #[test]
    fn default_project_trust_is_global_only() {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "defaultProjectTrust": "always" }"#,
        );
        // project tries to set it but it must be stripped
        store.seed(
            SettingsScope::Project,
            r#"{ "defaultProjectTrust": "never" }"#,
        );
        let mgr = SettingsManager::load(store, true);
        assert_eq!(
            mgr.effective().default_project_trust(),
            DefaultProjectTrust::Always
        );
    }

    /// CFG-057 — RED before the fix. Pi reads `httpProxy` off the raw GLOBAL document
    /// (`main.ts:537` / `:801`, both `getGlobalSettings().httpProxy`) and documents it as
    /// "Global setting only." (`packages/coding-agent/docs/settings.md:87` @v0.83.0), so a
    /// project `.cyrup/settings.json` must not be able to rewrite the session's egress — not even
    /// a TRUSTED one, since approving a project is not approving a proxy.
    #[test]
    fn http_proxy_is_global_only() {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "httpProxy": "http://global:8080" }"#,
        );
        store.seed(
            SettingsScope::Project,
            r#"{ "httpProxy": "http://project:9090" }"#,
        );
        let mgr = SettingsManager::load(store, true);
        assert_eq!(
            mgr.effective().http_proxy(),
            Some("http://global:8080".to_string())
        );

        // And with no global value the project one supplies nothing at all.
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Project,
            r#"{ "httpProxy": "http://project:9090" }"#,
        );
        let mgr = SettingsManager::load(store, true);
        assert_eq!(mgr.effective().http_proxy(), None);
    }

    /// CFG-060 — **COVERAGE, not a red-before proof, and the distinction is the point.** The fix is
    /// the REMOVAL of the accessor's `&EnvVars` parameter, so this test cannot be written against
    /// the pre-fix API at all: it would not compile. What the pre-fix code did, stated so the
    /// change is auditable — `http_proxy(&EnvVars { http_proxy: Some("http://ambient:3128"), .. })`
    /// with NO `httpProxy` key in either document returned `Some("http://ambient:3128")`, because
    /// the body ended in `.or_else(|| env.http_proxy.clone())`.
    ///
    /// Why that was wrong rather than merely redundant. pi calls
    /// `applyHttpProxySettings(getGlobalSettings().httpProxy)` (`main.ts:537`, `:801` @v0.83.0),
    /// which is `process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`
    /// (`http-dispatcher.ts:43-48`) — the two names are filled INDEPENDENTLY. With an ambient
    /// `HTTP_PROXY=http://ambient:3128` and `"httpProxy": "http://setting:8080"`, upstream leaves
    /// `HTTP_PROXY` ambient and sets `HTTPS_PROXY` to the SETTING, so an https target proxies
    /// through `http://setting:8080`. Feeding the ambient value back through this accessor into
    /// `configure_http_proxy` would have made `http://ambient:3128` the configured proxy for both
    /// names and lost the setting for https targets entirely. The ambient-wins half of `??=` is
    /// already ported, once, in `node_http_proxy::get_proxy_env`.
    #[test]
    fn http_proxy_is_the_setting_alone_and_takes_no_environment() {
        // Unset on both layers: the accessor has nothing to fall back TO any more.
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "defaultModel": "m" }"#);
        let mgr = SettingsManager::load(store, true);
        assert_eq!(
            mgr.effective().http_proxy(),
            None,
            "no httpProxy key means no configured proxy, whatever the ambient environment holds"
        );

        // Set: trimmed, and an all-whitespace value is `!proxy` upstream (http-dispatcher.ts:44-45).
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "httpProxy": "  http://setting:8080  " }"#,
        );
        let mgr = SettingsManager::load(store, true);
        assert_eq!(
            mgr.effective().http_proxy(),
            Some("http://setting:8080".to_string())
        );

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "httpProxy": "   " }"#);
        let mgr = SettingsManager::load(store, true);
        assert_eq!(mgr.effective().http_proxy(), None);
    }

    /// CFG-061 — **RED before the fix**: `EffectiveSettings::packages()` was
    /// `from_value::<Vec<PackageSource>>(v.clone()).ok().unwrap_or_default()`, so the `Err` from
    /// entry 4 collapsed the whole array and this asserted 9 against 0. pi's `getPackages`
    /// (`settings-manager.ts:969-971` @v0.83.0) is `[...(this.settings.packages ?? [])]` — a
    /// verbatim copy with no parsing at all, so a malformed entry travels downstream and is
    /// rejected on its own.
    #[test]
    fn one_malformed_package_entry_does_not_discard_the_other_nine() {
        let s = EffectiveSettings::from_settings(
            Settings::parse(
                r#"{"packages": [
                     "a", "b", "c",
                     42,
                     "e", "f", "g", "h", "i", "j"
                   ]}"#,
            )
            .unwrap(),
        );
        let (pkgs, errors) = s.packages_with_errors();
        assert_eq!(
            pkgs.len(),
            9,
            "nine well-formed entries survive the tenth being a number"
        );
        assert_eq!(s.packages().len(), 9, "the error-free accessor agrees");
        assert_eq!(errors.len(), 1, "and the bad entry is reported, not silent");
        assert!(
            errors
                .first()
                .is_some_and(|e| e.starts_with("settings `packages[3]`")),
            "the diagnostic names the index: {errors:?}"
        );
        assert_eq!(
            pkgs.first(),
            Some(&PackageSource::Name("a".to_string())),
            "and the entries before the bad one are kept, not just the ones after"
        );
    }

    /// CFG-062 — **RED before the fix** on both halves: the written document contained
    /// `"shellPath": null` / `"terminal": {"showImages": null}` and both `contains` assertions
    /// failed. pi's clearing setters assign `undefined` (`setShellPath`
    /// settings-manager.ts:883-887, `setShellCommandPrefix` `:914-918`, `setNpmCommand`
    /// `:924-928` @v0.83.0) and `persistScopedSettings` writes through
    /// `JSON.stringify(mergedSettings, null, 2)` (`:605`), which omits undefined-valued properties
    /// at every depth — so upstream cannot produce a `null` in a settings document at all.
    #[test]
    fn clearing_a_key_removes_it_rather_than_writing_json_null() {
        let store = Arc::new(InMemorySettingsStore::new());
        let mut mgr = SettingsManager::load(store.clone(), true);

        mgr.set(SettingsScope::Global, "shellPath", Some("~/bin/bash"))
            .unwrap();
        mgr.set_nested(
            SettingsScope::Global,
            &["terminal", "showImages"],
            Value::Bool(true),
        )
        .unwrap();
        let written = store.read(SettingsScope::Global).unwrap().unwrap();
        assert!(written.contains("shellPath"), "precondition: {written}");
        assert!(written.contains("showImages"), "precondition: {written}");

        // Clear both. `None::<&str>` serializes to `Value::Null`, which is the only way a Rust
        // caller can express pi's `undefined`.
        mgr.set(SettingsScope::Global, "shellPath", None::<&str>)
            .unwrap();
        mgr.set_nested(
            SettingsScope::Global,
            &["terminal", "showImages"],
            Value::Null,
        )
        .unwrap();

        let written = store.read(SettingsScope::Global).unwrap().unwrap();
        assert!(
            !written.contains("shellPath"),
            "the key must be GONE, not present-and-null: {written}"
        );
        assert!(
            !written.contains("showImages"),
            "nested leaves clear the same way: {written}"
        );
        assert!(
            !written.contains("null"),
            "no null survives anywhere in the document: {written}"
        );
        assert!(
            written.contains("terminal"),
            "clearing a leaf must not delete its parent object: {written}"
        );
        assert_eq!(mgr.effective().shell_path(), None);
    }

    /// CFG-062, the merge half — **recorded as a REFUTATION, and it is not a bug.** The item's
    /// Impact claims cyrup's `deep_merge` lacks pi's undefined-skip and that "a project
    /// `npmCommand: null` blanks the global value where pi has no way to express that state at
    /// all". Both clauses are false. `serde_json` has no `undefined`, so a key absent from the
    /// project map is structurally skipped — the skip pi spells at `settings-manager.ts:139-141`
    /// @v0.83.0 (and at `:149-152` of the v0.84.1 `deepMergeObjects`) is unrepresentable here. And
    /// a hand-written `"npmCommand": null` in a project file IS expressible upstream: JSON.parse
    /// yields `null`, `overrideValue === undefined` is false, so pi's merge takes the null too and
    /// `getNpmCommand`'s `this.settings.npmCommand ? … : undefined` then reads it as unset —
    /// exactly what cyrup does. The write path was the only divergence.
    #[test]
    fn a_project_null_blanks_a_global_value_on_both_sides() {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "npmCommand": ["pnpm"], "defaultModel": "m" }"#,
        );
        store.seed(SettingsScope::Project, r#"{ "npmCommand": null }"#);
        let mgr = SettingsManager::load(store, true);
        assert_eq!(
            mgr.effective().npm_command(),
            None,
            "pi's deepMergeSettings skips undefined, not null — the null wins there too"
        );
        assert_eq!(
            mgr.effective().default_model(),
            Some("m".to_string()),
            "and it is scoped to the one key, not the document"
        );
    }

    #[test]
    fn set_field_preserves_unknown_keys() {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "futureKey": 42, "defaultModel": "old" }"#,
        );
        let mut mgr = SettingsManager::load(store.clone(), false);
        mgr.set(SettingsScope::Global, "defaultModel", "new")
            .unwrap();
        let raw = store.read(SettingsScope::Global).unwrap().unwrap();
        let s = Settings::parse(&raw).unwrap();
        assert_eq!(s.get("futureKey"), Some(&serde_json::json!(42)));
        assert_eq!(s.get("defaultModel"), Some(&serde_json::json!("new")));
    }

    #[test]
    fn project_write_requires_trust() {
        let store = Arc::new(InMemorySettingsStore::new());
        let mut mgr = SettingsManager::load(store, false);
        let err = mgr.set(SettingsScope::Project, "defaultModel", "x");
        assert!(matches!(err, Err(ConfigError::Untrusted)));
    }

    #[test]
    fn migrations_applied_on_parse() {
        // settings-manager.ts:376-435
        // queueMode -> steeringMode
        let s = Settings::parse(r#"{ "queueMode": "all" }"#).unwrap();
        assert_eq!(s.get("steeringMode"), Some(&serde_json::json!("all")));
        assert!(s.get("queueMode").is_none());
        // websockets bool -> transport
        let s = Settings::parse(r#"{ "websockets": true }"#).unwrap();
        assert_eq!(s.get("transport"), Some(&serde_json::json!("websocket")));
        let s = Settings::parse(r#"{ "websockets": false }"#).unwrap();
        assert_eq!(s.get("transport"), Some(&serde_json::json!("sse")));
        // skills object -> array
        let s = Settings::parse(
            r#"{ "skills": { "enableSkillCommands": false, "customDirectories": ["/a", "/b"] } }"#,
        )
        .unwrap();
        assert_eq!(s.get("skills"), Some(&serde_json::json!(["/a", "/b"])));
        assert_eq!(
            s.get("enableSkillCommands"),
            Some(&serde_json::json!(false))
        );
        // retry.maxDelayMs -> retry.provider.maxRetryDelayMs
        let s = Settings::parse(r#"{ "retry": { "maxDelayMs": 5000 } }"#).unwrap();
        assert_eq!(
            s.get("retry").unwrap()["provider"]["maxRetryDelayMs"],
            serde_json::json!(5000)
        );
        assert!(s.get("retry").unwrap().get("maxDelayMs").is_none());
    }

    #[test]
    fn typed_accessors_defaults() {
        let s = EffectiveSettings::from_settings(Settings::default());
        assert_eq!(s.steering_mode(), "one-at-a-time");
        assert_eq!(s.transport(), "auto");
        assert!(s.compaction_enabled());
        assert_eq!(s.compaction_reserve_tokens(), 16384);
        assert_eq!(s.retry_max_retries(), 3);
        assert_eq!(s.provider_max_retry_delay_ms(), 60000);
        assert_eq!(s.http_idle_timeout_ms().unwrap(), 300_000);
        assert!(s.show_images());
        assert_eq!(s.image_width_cells(), 60);
        assert_eq!(s.double_escape_action(), "tree");
        assert_eq!(s.tree_filter_mode(), "default");
        assert_eq!(s.autocomplete_max_visible(), 5);
        assert_eq!(s.code_block_indent(), "  ");
        // CFG-014 — `showCacheMissNotices` defaults to false (settings-manager.ts:96 @v0.83.0).
        assert!(!s.show_cache_miss_notices());
        // `outputPad` defaults to 1 (Pi `getOutputPad`: only an explicit 0 yields 0).
        assert_eq!(s.output_pad(), 1);
    }

    /// CFG-014 — the key round-trips through the merged view, so the TUI consumer (PROV-035's
    /// wiring half) has a real value to read rather than a hardcoded `false`.
    #[test]
    fn show_cache_miss_notices_reads_the_settings_key() {
        let s = EffectiveSettings::from_settings(
            Settings::parse(r#"{"showCacheMissNotices": true}"#).expect("valid settings"),
        );
        assert!(s.show_cache_miss_notices());
    }

    /// CFG-030: a top level that is valid JSON but not an object is an ERROR, so the load-error
    /// latch (`record_load_error` → `ensure_scope_writable`) engages and the next `/config` write is
    /// REFUSED instead of rewriting the user's file from an empty document.
    ///
    /// Red at HEAD: `Settings::parse` matched `Value::Object(..)` and returned `Ok(default)` for
    /// everything else, so `[1,2,3]` parsed clean, produced no diagnostic, and was silently emptied
    /// on the next write.
    #[test]
    fn a_non_object_top_level_settings_document_is_a_parse_error() {
        for text in ["[1,2,3]", "\"hello\"", "42", "null", "true"] {
            assert!(
                Settings::parse(text).is_err(),
                "non-object top level {text:?} must not parse as empty settings"
            );
        }
        // An object and an empty document are still fine.
        assert!(Settings::parse("{}").is_ok());
        assert!(Settings::parse("   ").is_ok());
    }

    /// CFG-040: `getMermaidRenderingMode` VALIDATES rather than parses —
    /// `mode === "off" || mode === "final" ? mode : "streaming"` (settings-manager.ts:1251-1254
    /// @v0.84.1) — so an unknown value and an absent key both yield `Streaming`.
    ///
    /// Red at HEAD: `grep -rni mermaid crates/cyrup-config/src` returned ZERO; there was no getter.
    #[test]
    fn mermaid_rendering_mode_defaults_to_streaming_and_accepts_only_pis_three_values() {
        let g = |json: &str| {
            EffectiveSettings::from_settings(Settings::parse(json).unwrap())
                .mermaid_rendering_mode()
        };
        assert_eq!(
            g(r#"{"markdown":{"mermaid":"off"}}"#),
            MermaidRenderingMode::Off
        );
        assert_eq!(
            g(r#"{"markdown":{"mermaid":"final"}}"#),
            MermaidRenderingMode::Final
        );
        assert_eq!(
            g(r#"{"markdown":{"mermaid":"streaming"}}"#),
            MermaidRenderingMode::Streaming
        );
        assert_eq!(
            g(r#"{"markdown":{"mermaid":"nonsense"}}"#),
            MermaidRenderingMode::Streaming
        );
        assert_eq!(g("{}"), MermaidRenderingMode::Streaming);
        // A sibling markdown key is untouched by the getter.
        let s =
            Settings::parse(r#"{"markdown":{"codeBlockIndent":"\t","mermaid":"off"}}"#).unwrap();
        let eff = EffectiveSettings::from_settings(s);
        assert_eq!(eff.mermaid_rendering_mode(), MermaidRenderingMode::Off);
        assert_eq!(eff.code_block_indent(), "\t");
    }

    #[test]
    fn output_pad_only_explicit_zero_disables() {
        // Pi `getOutputPad`: `outputPad === 0 ? 0 : 1` — only an explicit 0 turns padding off.
        let zero =
            EffectiveSettings::from_settings(Settings::parse(r#"{ "outputPad": 0 }"#).unwrap());
        assert_eq!(zero.output_pad(), 0);
        let one =
            EffectiveSettings::from_settings(Settings::parse(r#"{ "outputPad": 1 }"#).unwrap());
        assert_eq!(one.output_pad(), 1);
        // A stray/unexpected value (or unset) resolves to the default 1, not 0.
        let stray =
            EffectiveSettings::from_settings(Settings::parse(r#"{ "outputPad": 5 }"#).unwrap());
        assert_eq!(stray.output_pad(), 1);
    }

    #[test]
    fn http_idle_timeout_invalid_errors() {
        let s = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "httpIdleTimeoutMs": "garbage" }"#).unwrap(),
        );
        assert!(s.http_idle_timeout_ms().is_err());
        let s = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "httpIdleTimeoutMs": "disabled" }"#).unwrap(),
        );
        assert_eq!(s.http_idle_timeout_ms().unwrap(), 0);
    }

    #[test]
    fn nested_set_preserves_siblings() {
        // R-07-004: setting terminal.showImages must not clobber terminal.imageWidthCells.
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "terminal": { "imageWidthCells": 40 } }"#,
        );
        let mut mgr = SettingsManager::load(store.clone(), false);
        mgr.set_show_images(false).unwrap();
        let raw = store.read(SettingsScope::Global).unwrap().unwrap();
        let s = Settings::parse(&raw).unwrap();
        assert_eq!(
            s.get("terminal").unwrap()["showImages"],
            serde_json::json!(false)
        );
        assert_eq!(
            s.get("terminal").unwrap()["imageWidthCells"],
            serde_json::json!(40)
        );
    }

    #[test]
    fn setters_clamp() {
        let store = Arc::new(InMemorySettingsStore::new());
        let mut mgr = SettingsManager::load(store.clone(), false);
        mgr.set_editor_padding_x(9.0).unwrap();
        assert_eq!(mgr.effective().editor_padding_x(), 3);
        mgr.set_autocomplete_max_visible(1.0).unwrap();
        assert_eq!(mgr.effective().autocomplete_max_visible(), 3);
        mgr.set_image_width_cells(0.0).unwrap();
        assert_eq!(mgr.effective().image_width_cells(), 1);
        assert!(mgr.set_http_idle_timeout_ms(-5.0).is_err());
    }

    #[test]
    fn default_thinking_level_reads_correct_key() {
        // settings-manager.ts:84,735-737 — the key is `defaultThinkingLevel`.
        let s = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "defaultThinkingLevel": "high" }"#).unwrap(),
        );
        assert_eq!(s.default_thinking_level(), Some(ModelThinkingLevel::High));
        // The legacy/wrong key must NOT be honoured.
        let s = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "defaultModelThinkingLevel": "high" }"#).unwrap(),
        );
        assert_eq!(s.default_thinking_level(), None);
    }

    /// CFG-056 — RED before the fix, which returned `ModelThinkingLevel::default()` (= `Off`) for
    /// an unset key. Pi's getter returns `undefined` (settings-manager.ts:740-742 @v0.83.0) and
    /// every consumer falls back to `DEFAULT_THINKING_LEVEL` = `"medium"`
    /// (`core/defaults.ts:3`), so the unset case must NOT be `Off` and must NOT be decided here.
    #[test]
    fn unset_default_thinking_level_is_none_and_falls_back_to_medium() {
        let s = EffectiveSettings::from_settings(Settings::parse("{}").unwrap());
        assert_eq!(s.default_thinking_level(), None);
        assert_eq!(
            s.default_thinking_level()
                .unwrap_or(crate::DEFAULT_THINKING_LEVEL),
            ModelThinkingLevel::Medium,
        );
        assert_ne!(crate::DEFAULT_THINKING_LEVEL, ModelThinkingLevel::default());
    }

    /// PROV-002 / pi `test/max-thinking.test.ts` ("is accepted by CLI and settings"): a settings
    /// file declaring `"max"` must round-trip to the `Max` rung, not silently fall back to `off`.
    #[test]
    fn default_thinking_level_accepts_max() {
        let s = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "defaultThinkingLevel": "max" }"#).unwrap(),
        );
        assert_eq!(s.default_thinking_level(), Some(ModelThinkingLevel::Max));
        // A genuinely unknown level still degrades to "unset" rather than erroring, and the
        // consumer's `DEFAULT_THINKING_LEVEL` fallback then applies (CFG-056).
        let s = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "defaultThinkingLevel": "ultra" }"#).unwrap(),
        );
        assert_eq!(s.default_thinking_level(), None);
    }

    #[test]
    fn theme_split_get_theme_vs_get_theme_setting() {
        // settings-manager.ts:718-727
        let s =
            EffectiveSettings::from_settings(Settings::parse(r#"{ "theme": "light" }"#).unwrap());
        assert_eq!(s.theme_setting(), Some("light".to_string()));
        assert_eq!(s.theme(), Some("light".to_string()));
        // namespaced (a/b) themes resolve to None in getTheme but are kept in getThemeSetting.
        let s = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "theme": "pkg/dark" }"#).unwrap(),
        );
        assert_eq!(s.theme_setting(), Some("pkg/dark".to_string()));
        assert_eq!(s.theme(), None);
        // unset: both None (the "dark" default lives in the TUI layer).
        let s = EffectiveSettings::from_settings(Settings::default());
        assert_eq!(s.theme_setting(), None);
        assert_eq!(s.theme(), None);
    }

    #[test]
    fn hardware_cursor_and_clear_on_shrink_env_fallback() {
        // settings-manager.ts:1077-1083,1165-1167 — setting wins, then env (== "1"), else false.
        let mut env = crate::env::EnvVars::default();
        let empty = EffectiveSettings::from_settings(Settings::default());
        assert!(!empty.show_hardware_cursor(&env));
        assert!(!empty.clear_on_shrink(&env));
        env.hardware_cursor = true;
        env.clear_on_shrink = true;
        assert!(empty.show_hardware_cursor(&env));
        assert!(empty.clear_on_shrink(&env));
        // explicit setting (even false) overrides the env fallback.
        let s = EffectiveSettings::from_settings(
            Settings::parse(
                r#"{ "showHardwareCursor": false, "terminal": { "clearOnShrink": false } }"#,
            )
            .unwrap(),
        );
        assert!(!s.show_hardware_cursor(&env));
        assert!(!s.clear_on_shrink(&env));
    }

    #[test]
    fn thinking_budgets_warnings_and_combined_settings() {
        // settings-manager.ts:1043-1045 (thinkingBudgets), :1199-1201 (warnings),
        // :784-789 (branchSummary), :829-835 (providerRetry).
        let s = EffectiveSettings::from_settings(Settings::default());
        assert_eq!(s.thinking_budgets(), None);
        assert_eq!(s.warnings(), Warnings::default());
        assert_eq!(
            s.provider_retry_settings(),
            ProviderRetrySettings {
                timeout_ms: None,
                max_retries: None,
                max_retry_delay_ms: 60000
            }
        );
        assert_eq!(
            s.branch_summary_settings(),
            BranchSummarySettings {
                reserve_tokens: 16384,
                skip_prompt: false
            }
        );

        let s = EffectiveSettings::from_settings(
            Settings::parse(
                r#"{
                    "thinkingBudgets": { "low": 100, "high": 9000 },
                    "warnings": { "anthropicExtraUsage": false },
                    "branchSummary": { "reserveTokens": 2048, "skipPrompt": true },
                    "retry": { "provider": { "timeoutMs": 1234, "maxRetries": 7, "maxRetryDelayMs": 999 } }
                }"#,
            )
            .unwrap(),
        );
        assert_eq!(
            s.thinking_budgets(),
            Some(ThinkingBudgets {
                minimal: None,
                low: Some(100),
                medium: None,
                high: Some(9000)
            })
        );
        assert_eq!(
            s.warnings(),
            Warnings {
                anthropic_extra_usage: Some(false)
            }
        );
        assert_eq!(
            s.branch_summary_settings(),
            BranchSummarySettings {
                reserve_tokens: 2048,
                skip_prompt: true
            }
        );
        assert_eq!(
            s.provider_retry_settings(),
            ProviderRetrySettings {
                timeout_ms: Some(1234),
                max_retries: Some(7),
                max_retry_delay_ms: 999,
            }
        );
    }

    #[test]
    fn thinking_budgets_and_warnings_parse_field_wise() {
        // Pi returns these objects raw/loosely-typed (settings-manager.ts:1043-1045, 1199-1201):
        // one malformed field does NOT discard the rest. The prior whole-object `from_value`
        // collapsed the ENTIRE object to None/Default on a single bad field. Assert the surviving
        // valid fields are preserved (Pi behaviour), not lost.
        let s = EffectiveSettings::from_settings(
            Settings::parse(
                r#"{
                    "thinkingBudgets": { "minimal": 50, "low": "oops", "medium": 700 },
                    "warnings": { "anthropicExtraUsage": "nope" }
                }"#,
            )
            .unwrap(),
        );
        // `low` is a string (malformed) → that field is None, but `minimal` and `medium` survive.
        // The whole-object parse would have yielded `None` for the entire budgets object.
        assert_eq!(
            s.thinking_budgets(),
            Some(ThinkingBudgets {
                minimal: Some(50),
                low: None,
                medium: Some(700),
                high: None
            })
        );
        // `anthropicExtraUsage` is a string (malformed) → that field falls back to None; the object
        // itself is still returned (present key) rather than collapsing.
        assert_eq!(
            s.warnings(),
            Warnings {
                anthropic_extra_usage: None
            }
        );

        // An empty `thinkingBudgets` object is present → `Some(default)`, distinct from unset/None.
        let s2 = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "thinkingBudgets": {} }"#).unwrap(),
        );
        assert_eq!(s2.thinking_budgets(), Some(ThinkingBudgets::default()));
    }

    #[test]
    fn enabled_models_distinguishes_unset_from_empty() {
        // Pi `getEnabledModels(): string[] | undefined` (settings-manager.ts:1133-1135): unset is
        // `undefined` (cycle ALL), an explicit `[]` is empty (cycle NONE). The prior
        // `unwrap_or_default` collapsed both to an empty Vec.
        let unset = EffectiveSettings::from_settings(Settings::default());
        assert_eq!(unset.enabled_models(), None);

        let empty = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "enabledModels": [] }"#).unwrap(),
        );
        assert_eq!(empty.enabled_models(), Some(vec![]));

        let some = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "enabledModels": ["anthropic/claude-opus-4-8"] }"#).unwrap(),
        );
        assert_eq!(
            some.enabled_models(),
            Some(vec!["anthropic/claude-opus-4-8".to_string()])
        );
    }

    #[test]
    fn compaction_and_retry_combined_getters() {
        // settings-manager.ts:776-782 (getCompactionSettings), :808-814 (getRetrySettings).
        let s = EffectiveSettings::from_settings(Settings::default());
        assert_eq!(
            s.compaction_settings(),
            CompactionSettings {
                enabled: true,
                reserve_tokens: 16384,
                keep_recent_tokens: 20000
            }
        );
        assert_eq!(
            s.retry_settings(),
            RetrySettings {
                enabled: true,
                max_retries: 3,
                base_delay_ms: 2000
            }
        );

        let s = EffectiveSettings::from_settings(
            Settings::parse(
                r#"{
                    "compaction": { "enabled": false, "reserveTokens": 100, "keepRecentTokens": 200 },
                    "retry": { "enabled": false, "maxRetries": 9, "baseDelayMs": 500 }
                }"#,
            )
            .unwrap(),
        );
        assert_eq!(
            s.compaction_settings(),
            CompactionSettings {
                enabled: false,
                reserve_tokens: 100,
                keep_recent_tokens: 200
            }
        );
        assert_eq!(
            s.retry_settings(),
            RetrySettings {
                enabled: false,
                max_retries: 9,
                base_delay_ms: 500
            }
        );
    }

    #[test]
    fn typed_list_getters() {
        // settings-manager.ts:953-1031 — getPackages/getExtensionPaths/getSkillPaths/
        // getPromptTemplatePaths/getThemePaths, each with an empty-array default.
        let empty = EffectiveSettings::from_settings(Settings::default());
        assert!(empty.packages().is_empty());
        assert!(empty.extension_paths().is_empty());
        assert!(empty.skill_paths().is_empty());
        assert!(empty.prompt_template_paths().is_empty());
        assert!(empty.theme_paths().is_empty());

        let s = EffectiveSettings::from_settings(
            Settings::parse(
                r#"{
                    "packages": ["pkg-a", { "source": "pkg-b", "extensions": ["x.ts"], "themes": ["t"] }],
                    "extensions": ["/ext/a"],
                    "skills": ["/skill/a", "/skill/b"],
                    "prompts": ["/p"],
                    "themes": ["/theme/a"]
                }"#,
            )
            .unwrap(),
        );
        assert_eq!(
            s.packages(),
            vec![
                PackageSource::Name("pkg-a".to_string()),
                PackageSource::Detailed {
                    source: "pkg-b".to_string(),
                    autoload: None,
                    extensions: Some(vec!["x.ts".to_string()]),
                    skills: None,
                    prompts: None,
                    themes: Some(vec!["t".to_string()]),
                },
            ]
        );
        assert_eq!(s.extension_paths(), vec!["/ext/a".to_string()]);
        assert_eq!(
            s.skill_paths(),
            vec!["/skill/a".to_string(), "/skill/b".to_string()]
        );
        assert_eq!(s.prompt_template_paths(), vec!["/p".to_string()]);
        assert_eq!(s.theme_paths(), vec!["/theme/a".to_string()]);
    }

    /// CFG-010 — `autoload` is a real key on the object form (Pi `PackageSource`,
    /// settings-manager.ts:79). `PackageSource` is `#[serde(untagged)]` with no
    /// `deny_unknown_fields`, so before it was modelled the key deserialized into `Detailed` and was
    /// silently discarded — the user's opt-out simply evaporated between settings.json and
    /// discovery.
    #[test]
    fn a_package_entry_carries_its_autoload_flag() {
        let s = EffectiveSettings::from_settings(
            Settings::parse(
                r#"{"packages": [
                     "plain",
                     { "source": "opted-out", "autoload": false },
                     { "source": "delta", "autoload": false, "skills": ["skills/a/**"] },
                     { "source": "explicit-on", "autoload": true }
                   ]}"#,
            )
            .unwrap(),
        );
        let pkgs = s.packages();
        assert_eq!(
            pkgs.iter().map(PackageSource::autoload).collect::<Vec<_>>(),
            vec![None, Some(false), Some(false), Some(true)],
            "a bare string entry has no autoload; the object form round-trips the flag verbatim"
        );
        assert_eq!(
            pkgs.get(2).map(PackageSource::filters).and_then(|f| f.1),
            Some(["skills/a/**".to_string()].as_slice()),
            "the per-type patterns survive alongside it"
        );
        // Serializing back preserves the key (settings documents round-trip, R-07-004).
        let json = serde_json::to_string(&pkgs).unwrap();
        assert!(json.contains(r#""autoload":false"#), "{json}");
    }

    #[test]
    fn apply_overrides_deep_merges_onto_effective() {
        // settings-manager.ts:503-505 — runtime overrides deep-merge onto the effective view.
        let store = Arc::new(InMemorySettingsStore::new());
        let mut mgr = SettingsManager::load(store.clone(), false);
        assert!(mgr.effective().compaction_enabled());
        assert_eq!(mgr.effective().compaction_reserve_tokens(), 16384);

        let overrides =
            Settings::parse(r#"{ "compaction": { "reserveTokens": 4096 }, "quietStartup": true }"#)
                .unwrap();
        mgr.apply_overrides(&overrides);
        // nested merge preserves the sibling `enabled` default while overriding reserveTokens.
        assert!(mgr.effective().compaction_enabled());
        assert_eq!(mgr.effective().compaction_reserve_tokens(), 4096);
        assert!(mgr.effective().quiet_startup());

        // transient: a reload recomputes from the layers and drops the overrides.
        mgr.reload().unwrap();
        assert_eq!(mgr.effective().compaction_reserve_tokens(), 16384);
        assert!(!mgr.effective().quiet_startup());
    }

    #[test]
    fn enable_analytics_generates_tracking_id() {
        // settings-manager.ts:943-951
        let store = Arc::new(InMemorySettingsStore::new());
        let mut mgr = SettingsManager::load(store.clone(), false);
        assert!(mgr.effective().tracking_id().is_none());
        mgr.set_enable_analytics(true).unwrap();
        assert!(mgr.effective().enable_analytics());
        let id = mgr.effective().tracking_id().unwrap();
        assert_eq!(id.len(), 36); // canonical UUID form
        // opting out doesn't regenerate / clear the id; opting back in keeps the same id
        mgr.set_enable_analytics(false).unwrap();
        mgr.set_enable_analytics(true).unwrap();
        assert_eq!(mgr.effective().tracking_id().unwrap(), id);
    }

    // -----------------------------------------------------------------------
    // CFG-001 — a writer must REFUSE a scope whose file it could not parse, never rewrite it.
    //
    // Pi guards every writer: `save()` (settings-manager.ts ≈:614-628) opens with
    // `if (this.globalSettingsLoadError) { return; }` and `saveProjectSettings()` (≈:633-646) has
    // the mirror. Before this fix cyrup's `set`/`set_nested`/`persist_nested` all did
    // `match current.map(Settings::parse) { Some(Ok(s)) => s, _ => Settings::default() }`, so a
    // trailing comma in `~/.cyrup/settings.json` meant the next `/config` toggle rewrote the whole
    // file as `{"<key>": <value>}` — every other setting gone.
    //
    // The assertions are BYTE-level (`assert_eq!(after, MALFORMED)`), not "the key I wrote is
    // absent": the whole point is that the user's file is left exactly as they left it.
    // -----------------------------------------------------------------------

    /// A realistic corruption: a trailing comma before `}`, plus settings worth losing.
    const MALFORMED: &str = "{\n  \"defaultModel\": \"anthropic/claude-opus-4\",\n  \"theme\": \"dark\",\n  \"editorPaddingX\": 2,\n}\n";

    fn malformed_global() -> (Arc<InMemorySettingsStore>, SettingsManager) {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, MALFORMED);
        let mgr = SettingsManager::load(store.clone(), false);
        (store, mgr)
    }

    fn assert_refused(result: Result<(), ConfigError>, expected_scope: SettingsScope) {
        let described = format!("{result:?}");
        // The refusal must name the scope it protected AND carry the underlying cause, so a
        // `/config` toggle can tell the user which file to go fix.
        let matched = matches!(
            &result,
            Err(ConfigError::SettingsWriteRefused { scope, message })
                if *scope == expected_scope && message.contains("parse error")
        );
        assert!(
            matched,
            "expected SettingsWriteRefused{{{expected_scope:?}, ..parse error..}}, got {described}"
        );
    }

    #[test]
    fn cfg001_set_refuses_to_clobber_a_malformed_file() {
        let (store, mut mgr) = malformed_global();
        // The load recorded the failure (R-00-009) and latched the scope (Pi globalSettingsLoadError).
        assert!(
            mgr.load_error(SettingsScope::Global).is_some(),
            "the scope is latched"
        );

        assert_refused(
            mgr.set(SettingsScope::Global, "theme", "light"),
            SettingsScope::Global,
        );

        let after = store.read(SettingsScope::Global).unwrap().unwrap();
        assert_eq!(
            after, MALFORMED,
            "the malformed file is byte-for-byte unchanged"
        );
    }

    #[test]
    fn cfg001_set_nested_refuses_to_clobber_a_malformed_file() {
        let (store, mut mgr) = malformed_global();

        assert_refused(
            mgr.set_nested(
                SettingsScope::Global,
                &["terminal", "showImages"],
                false.into(),
            ),
            SettingsScope::Global,
        );

        let after = store.read(SettingsScope::Global).unwrap().unwrap();
        assert_eq!(
            after, MALFORMED,
            "the malformed file is byte-for-byte unchanged"
        );
    }

    #[test]
    fn cfg001_persist_nested_refuses_to_clobber_a_malformed_file() {
        let (store, mgr) = malformed_global();

        assert_refused(
            mgr.persist_nested(SettingsScope::Global, &["outputPad"], 0.into()),
            SettingsScope::Global,
        );

        let after = store.read(SettingsScope::Global).unwrap().unwrap();
        assert_eq!(
            after, MALFORMED,
            "the malformed file is byte-for-byte unchanged"
        );
    }

    #[test]
    fn cfg001_convenience_setters_refuse_too() {
        // Every `/config`-reachable convenience setter routes through one of the three writers, so
        // each inherits the guard — including `set_enable_analytics`, which owns its own `with_lock`.
        let (store, mut mgr) = malformed_global();

        assert_refused(mgr.set_editor_padding_x(3.0), SettingsScope::Global);
        assert_refused(mgr.set_show_images(false), SettingsScope::Global);
        assert_refused(mgr.set_image_width_cells(40.0), SettingsScope::Global);
        assert_refused(mgr.set_autocomplete_max_visible(9.0), SettingsScope::Global);
        assert_refused(mgr.set_http_idle_timeout_ms(1000.0), SettingsScope::Global);
        assert_refused(mgr.set_enable_analytics(true), SettingsScope::Global);

        let after = store.read(SettingsScope::Global).unwrap().unwrap();
        assert_eq!(
            after, MALFORMED,
            "six refused writes later, still untouched"
        );
    }

    #[test]
    fn cfg001_project_scope_is_latched_independently() {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "theme": "dark" }"#);
        store.seed(SettingsScope::Project, MALFORMED);
        let mut mgr = SettingsManager::load(store.clone(), true);

        assert!(mgr.load_error(SettingsScope::Project).is_some());
        assert!(
            mgr.load_error(SettingsScope::Global).is_none(),
            "a healthy scope is not latched"
        );

        assert_refused(
            mgr.set(SettingsScope::Project, "quietStartup", true),
            SettingsScope::Project,
        );
        assert_eq!(
            store.read(SettingsScope::Project).unwrap().unwrap(),
            MALFORMED
        );

        // The healthy GLOBAL scope still writes — the guard is per-scope, not a global kill switch.
        mgr.set(SettingsScope::Global, "quietStartup", true)
            .unwrap();
        assert!(mgr.effective().quiet_startup());
    }

    #[test]
    fn cfg001_corruption_between_load_and_write_is_also_refused() {
        // The second half of the fix: the file loaded FINE (no latch), then something corrupted it
        // before the locked read-modify-write. The in-closure `Some(Err(_))` arm must abandon the
        // write and surface the refusal rather than starting from an empty document.
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "theme": "dark" }"#);
        let mut mgr = SettingsManager::load(store.clone(), false);
        assert!(
            mgr.load_error(SettingsScope::Global).is_none(),
            "loaded clean"
        );

        store.seed(SettingsScope::Global, MALFORMED); // corrupted behind our back

        assert_refused(
            mgr.set(SettingsScope::Global, "theme", "light"),
            SettingsScope::Global,
        );
        assert_eq!(
            store.read(SettingsScope::Global).unwrap().unwrap(),
            MALFORMED
        );

        assert_refused(
            mgr.set_nested(
                SettingsScope::Global,
                &["terminal", "showImages"],
                true.into(),
            ),
            SettingsScope::Global,
        );
        assert_refused(
            mgr.persist_nested(SettingsScope::Global, &["outputPad"], 1.into()),
            SettingsScope::Global,
        );
        assert_refused(mgr.set_enable_analytics(true), SettingsScope::Global);
        assert_eq!(
            store.read(SettingsScope::Global).unwrap().unwrap(),
            MALFORMED
        );
    }

    #[test]
    fn cfg001_repairing_the_file_and_reloading_restores_writability() {
        let (store, mut mgr) = malformed_global();
        assert!(mgr.set(SettingsScope::Global, "theme", "light").is_err());

        // The user fixes the trailing comma and cyrup reloads: the latch clears and writes resume.
        store.seed(
            SettingsScope::Global,
            r#"{ "defaultModel": "anthropic/claude-opus-4" }"#,
        );
        mgr.reload().unwrap();
        assert!(
            mgr.load_error(SettingsScope::Global).is_none(),
            "latch cleared on a clean reload"
        );

        mgr.set(SettingsScope::Global, "theme", "light").unwrap();
        let after = Settings::parse(&store.read(SettingsScope::Global).unwrap().unwrap()).unwrap();
        assert_eq!(after.get("theme"), Some(&serde_json::json!("light")));
        assert_eq!(
            after.get("defaultModel"),
            Some(&serde_json::json!("anthropic/claude-opus-4")),
            "and the repaired file's other keys survive"
        );
    }

    #[test]
    fn cfg001_an_absent_file_is_still_created() {
        // The refusal must not break first-run: `None` (no file) is not a parse failure.
        let store = Arc::new(InMemorySettingsStore::new());
        let mut mgr = SettingsManager::load(store.clone(), false);
        assert!(mgr.load_error(SettingsScope::Global).is_none());

        mgr.set(SettingsScope::Global, "theme", "light").unwrap();
        mgr.set_nested(
            SettingsScope::Global,
            &["terminal", "showImages"],
            true.into(),
        )
        .unwrap();
        let after = Settings::parse(&store.read(SettingsScope::Global).unwrap().unwrap()).unwrap();
        assert_eq!(after.get("theme"), Some(&serde_json::json!("light")));
    }

    /// CFG-003: the PER-LAYER `packages()` accessor (Pi reads `projectSettings.packages` and
    /// `globalSettings.packages` separately, package-manager.ts:891-898) parses entry-by-entry, so
    /// one malformed entry costs only that entry — not the array, and not the settings document.
    #[test]
    fn per_layer_packages_reports_a_bad_entry_and_keeps_the_good_ones() {
        let s = Settings::parse(
            r#"{"defaultModel":"anthropic/x","packages":[17,"good-pkg",{"source":"filtered","skills":["a"]}]}"#,
        )
        .unwrap();
        let (pkgs, errors) = s.packages_with_errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("packages[0]"), "{errors:?}");
        assert_eq!(pkgs.len(), 2, "the two well-formed entries survive");
        assert_eq!(pkgs[0].source(), "good-pkg");
        assert_eq!(pkgs[1].source(), "filtered");
        assert_eq!(pkgs[1].filters().1, Some(&["a".to_string()][..]));
        // The rest of the document is untouched.
        assert_eq!(
            EffectiveSettings::from_settings(s)
                .default_model()
                .as_deref(),
            Some("anthropic/x")
        );
    }

    /// A non-array `packages` is itself reported rather than silently treated as absent.
    #[test]
    fn per_layer_packages_reports_a_non_array_value() {
        let s = Settings::parse(r#"{"packages":"oops"}"#).unwrap();
        let (pkgs, errors) = s.packages_with_errors();
        assert!(pkgs.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("must be an array"), "{errors:?}");
    }

    /// CFG-004: the per-layer `extension_paths()` accessor exists (the merged view cannot say which
    /// scope declared an entry, and project entries are trust-gated independently).
    #[test]
    fn per_layer_extension_paths() {
        let s = Settings::parse(r#"{"extensions":["a","!b/*"]}"#).unwrap();
        assert_eq!(
            s.extension_paths(),
            vec!["a".to_string(), "!b/*".to_string()]
        );
        assert!(Settings::default().extension_paths().is_empty());
    }
}

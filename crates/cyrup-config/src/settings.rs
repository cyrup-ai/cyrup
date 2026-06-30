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

/// A configured package source (Pi `PackageSource`, settings-manager.ts:70-78): either a bare
/// source string, or an object naming the `source` plus optional per-resource include filters.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum PackageSource {
    Name(String),
    Detailed {
        source: String,
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

/// A key that is only honoured in the GLOBAL scope; stripped from project/CLI before merge.
const GLOBAL_ONLY_KEYS: &[&str] = &["defaultProjectTrust"];

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
        let value: Value = serde_json::from_str(text)?;
        match value {
            Value::Object(mut obj) => {
                migrate_settings(&mut obj);
                Ok(Self { obj })
            }
            // A non-object top-level is treated as empty (degraded), never a panic.
            _ => Ok(Self::default()),
        }
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
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
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
}

/// Remove keys that are only honoured globally (§4.8: `defaultProjectTrust`).
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
    /// settings-manager.ts:84,735-737). Pi returns `undefined` when unset; we map that to the
    /// type's default so the consumer always has a concrete level.
    pub fn default_thinking_level(&self) -> ModelThinkingLevel {
        self.merged
            .get("defaultThinkingLevel")
            .and_then(|v| serde_json::from_value::<ModelThinkingLevel>(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn hide_thinking_block(&self) -> bool {
        self.merged.get_bool("hideThinkingBlock").unwrap_or(false)
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

    pub fn enabled_models(&self) -> Vec<String> {
        self.merged
            .get("enabledModels")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
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

    /// `httpProxy` setting (Pi reads the setting in addition to env; settings-manager.ts:121,
    /// http-dispatcher.ts:42-46). The actual `HTTP_PROXY`/`HTTPS_PROXY` apply is a bin concern.
    pub fn http_proxy(&self, env: &crate::env::EnvVars) -> Option<String> {
        self.merged
            .get_str("httpProxy")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| env.http_proxy.clone())
    }

    /// `shellPath` (Pi `getShellPath`, :863-865).
    pub fn shell_path(&self) -> Option<String> {
        self.merged.get_str("shellPath")
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
    /// `getThinkingBudgets`, settings-manager.ts:1043-1045).
    pub fn thinking_budgets(&self) -> Option<ThinkingBudgets> {
        self.merged
            .get("thinkingBudgets")
            .and_then(|v| serde_json::from_value::<ThinkingBudgets>(v.clone()).ok())
    }

    /// `getWarnings` — the warnings object (a shallow copy; empty when unset) (Pi
    /// settings-manager.ts:1199-1201).
    pub fn warnings(&self) -> Warnings {
        self.merged
            .get("warnings")
            .and_then(|v| serde_json::from_value::<Warnings>(v.clone()).ok())
            .unwrap_or_default()
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

    /// `getPackages` — configured npm/git package sources (empty default; Pi
    /// settings-manager.ts:953-955).
    pub fn packages(&self) -> Vec<PackageSource> {
        self.merged
            .get("packages")
            .and_then(|v| serde_json::from_value::<Vec<PackageSource>>(v.clone()).ok())
            .unwrap_or_default()
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

/// Tilde-expand a path string (Pi `normalizePath` expandTilde branch, paths.ts:66-72).
fn expand_tilde(input: &str) -> String {
    let home = directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
    let Some(home) = home else {
        return input.to_string();
    };
    if input == "~" {
        return home.to_string_lossy().into_owned();
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return home.join(rest).to_string_lossy().into_owned();
    }
    input.to_string()
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

/// The layered settings facade (arch-07 §3.3). Holds the three layers + a memoized merge.
pub struct SettingsManager {
    store: Arc<dyn SettingsStore>,
    global: Settings,
    project: Settings,
    cli: Settings,
    effective: EffectiveSettings,
    project_trusted: bool,
    load_errors: Vec<ScopedError>,
}

impl SettingsManager {
    /// Load global unconditionally; load project ONLY if `project_trusted` (R-07-002). A parse
    /// error degrades that scope to empty and records a `ScopedError` (R-00-009).
    pub fn load(store: Arc<dyn SettingsStore>, cli: Settings, project_trusted: bool) -> Self {
        let mut mgr = Self {
            store,
            global: Settings::default(),
            project: Settings::default(),
            cli,
            effective: EffectiveSettings::default(),
            project_trusted,
            load_errors: Vec::new(),
        };
        mgr.reload_internal();
        mgr
    }

    fn load_scope(&mut self, scope: SettingsScope) -> Settings {
        match self.store.read(scope) {
            Ok(Some(text)) => match Settings::parse(&text) {
                Ok(s) => s,
                Err(e) => {
                    self.load_errors.push(ScopedError {
                        scope,
                        message: format!("parse error: {e}"),
                    });
                    Settings::default()
                }
            },
            Ok(None) => Settings::default(),
            Err(e) => {
                self.load_errors.push(ScopedError {
                    scope,
                    message: e.to_string(),
                });
                Settings::default()
            }
        }
    }

    fn reload_internal(&mut self) {
        self.global = self.load_scope(SettingsScope::Global);
        self.project = if self.project_trusted {
            self.load_scope(SettingsScope::Project)
        } else {
            Settings::default()
        };
        self.recompute();
    }

    fn recompute(&mut self) {
        // Strip global-only keys from project + cli before merge.
        let mut project = self.project.clone();
        let mut cli = self.cli.clone();
        strip_global_only(&mut project);
        strip_global_only(&mut cli);

        let merged = deep_merge(&self.global.to_value(), &project.to_value());
        let merged = deep_merge(&merged, &cli.to_value());
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

    /// `applyOverrides` (Pi settings-manager.ts:503-505): deep-merge additional overrides on top of
    /// the current effective settings at runtime. These overrides are NOT persisted and are
    /// transient — any subsequent `reload`/`set_project_trusted` recomputes the merge from the
    /// global/project/cli layers and discards them (matching Pi, where `applyOverrides` mutates the
    /// in-memory `this.settings` that `loadSettings` later rebuilds).
    pub fn apply_overrides(&mut self, overrides: &Settings) {
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

    /// Persist a single field via scoped read-modify-write that re-reads the on-disk file and
    /// applies only the modified field (concurrent-edit safe; R-07-004). Project writes require
    /// trust.
    pub fn set<T: serde::Serialize>(
        &mut self,
        scope: SettingsScope,
        key: &str,
        value: T,
    ) -> Result<(), ConfigError> {
        if scope == SettingsScope::Project && !self.project_trusted {
            return Err(ConfigError::Untrusted);
        }
        let json = serde_json::to_value(value)?;
        let key_owned = key.to_string();
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                _ => Settings::default(),
            };
            doc.obj.insert(key_owned.clone(), json.clone());
            Some(doc.to_pretty())
        })?;
        self.reload_internal();
        Ok(())
    }

    /// Persist a nested field (e.g. `terminal.showImages`) via scoped read-modify-write, creating
    /// intermediate objects and PRESERVING sibling nested keys (Pi `persistScopedSettings` nested
    /// tracking, settings-manager.ts:573-602). Unlike [`Self::set`], this never clobbers the rest of
    /// the parent object. Project writes require trust.
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
        let path_owned: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        self.store.with_lock(scope, &mut |current| {
            let mut doc = match current.map(Settings::parse) {
                Some(Ok(s)) => s,
                _ => Settings::default(),
            };
            set_value_at_path(&mut doc.obj, &path_owned, value.clone());
            Some(doc.to_pretty())
        })?;
        self.reload_internal();
        Ok(())
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
    pub fn set_enable_analytics(&mut self, enabled: bool) -> Result<(), ConfigError> {
        self.store
            .with_lock(SettingsScope::Global, &mut |current| {
                let mut doc = match current.map(Settings::parse) {
                    Some(Ok(s)) => s,
                    _ => Settings::default(),
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
        map.insert(first.clone(), value);
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
        let mgr = SettingsManager::load(store, Settings::new(), true);
        assert_eq!(mgr.effective().external_editor(&env), "vim");

        // empty-string configured editor is treated as unset -> VISUAL
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "externalEditor": "" }"#);
        let mgr = SettingsManager::load(store, Settings::new(), true);
        assert_eq!(mgr.effective().external_editor(&env), "vim");

        // empty configured editor with no VISUAL/EDITOR -> platform default
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Global, r#"{ "externalEditor": "  " }"#);
        let mgr = SettingsManager::load(store, Settings::new(), true);
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
        let mgr = SettingsManager::load(store, Settings::new(), true);
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

    #[test]
    fn cli_then_project_then_global_resolution() {
        // A-07-1
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "defaultModel": "g", "theme": "light" }"#,
        );
        store.seed(SettingsScope::Project, r#"{ "defaultModel": "p" }"#);
        let mut cli = Settings::new();
        cli.set_field("defaultModel", "c").unwrap();

        let mgr = SettingsManager::load(store.clone(), cli, true);
        assert_eq!(mgr.effective().default_model(), Some("c".to_string()));
        assert_eq!(mgr.effective().theme(), Some("light".to_string()));

        // without CLI, project wins
        let mgr2 = SettingsManager::load(store, Settings::new(), true);
        assert_eq!(mgr2.effective().default_model(), Some("p".to_string()));
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
        let mgr = SettingsManager::load(store, Settings::new(), true);

        // Each layer reports ONLY its own list (no merge).
        assert_eq!(mgr.global().skill_paths(), vec!["g-skill".to_string()]);
        assert_eq!(
            mgr.project().skill_paths(),
            vec!["p-skill-a".to_string(), "p-skill-b".to_string()]
        );
        assert_eq!(mgr.global().prompt_template_paths(), vec!["g-prompt".to_string()]);
        assert_eq!(mgr.project().prompt_template_paths(), vec!["p-prompt".to_string()]);
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

        let mut mgr = SettingsManager::load(store, Settings::new(), false);
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
        let mgr = SettingsManager::load(store, Settings::new(), true);
        assert_eq!(
            mgr.effective().default_project_trust(),
            DefaultProjectTrust::Always
        );
    }

    #[test]
    fn set_field_preserves_unknown_keys() {
        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Global,
            r#"{ "futureKey": 42, "defaultModel": "old" }"#,
        );
        let mut mgr = SettingsManager::load(store.clone(), Settings::new(), false);
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
        let mut mgr = SettingsManager::load(store, Settings::new(), false);
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
        let mut mgr = SettingsManager::load(store.clone(), Settings::new(), false);
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
        let mut mgr = SettingsManager::load(store.clone(), Settings::new(), false);
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
        assert_eq!(s.default_thinking_level(), ModelThinkingLevel::High);
        // The legacy/wrong key must NOT be honoured.
        let s = EffectiveSettings::from_settings(
            Settings::parse(r#"{ "defaultModelThinkingLevel": "high" }"#).unwrap(),
        );
        assert_eq!(s.default_thinking_level(), ModelThinkingLevel::default());
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

    #[test]
    fn apply_overrides_deep_merges_onto_effective() {
        // settings-manager.ts:503-505 — runtime overrides deep-merge onto the effective view.
        let store = Arc::new(InMemorySettingsStore::new());
        let mut mgr = SettingsManager::load(store.clone(), Settings::new(), false);
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
        let mut mgr = SettingsManager::load(store.clone(), Settings::new(), false);
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
}

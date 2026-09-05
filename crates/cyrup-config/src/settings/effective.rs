//! The merged, read-only effective view (R-07-001) and its typed getters, which apply the
//! documented defaults in one place.

use std::collections::BTreeMap;

use cyrup_core::ModelThinkingLevel;
use serde_json::Value;

use super::layer::Settings;
use super::types::{
    BranchSummarySettings, CompactionSettings, DefaultProjectTrust, FullscreenExitOutput,
    FullscreenScrollbar, MermaidRenderingMode, PackageSource, ProviderRetrySettings, RetrySettings,
    ThinkingBudgets, TuiMode, Warnings,
};
use crate::error::ConfigError;

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

    /// The per-model reasoning override for `provider/id` (Pi `getModelThinkingLevel`,
    /// `settings-manager.ts:792-794`: `this.settings.modelThinkingLevels?.[`${provider}/${modelId}`]`).
    ///
    /// Takes priority over [`Self::default_thinking_level`] when switching TO that model
    /// (`agent-session.ts:1832-1838`), which is what makes "reason hard on the big model, cheaply
    /// on the small one" hold across a `Ctrl+P` cycle instead of resetting to one global level.
    pub fn model_thinking_level(&self, provider: &str, id: &str) -> Option<ModelThinkingLevel> {
        self.merged
            .get("modelThinkingLevels")?
            .as_object()?
            .get(&format!("{provider}/{id}"))
            .and_then(|v| serde_json::from_value::<ModelThinkingLevel>(v.clone()).ok())
    }

    /// Every per-model override, keyed `"provider/id"` (Pi `getAllModelThinkingLevels`,
    /// `settings-manager.ts:796-798`) — the summary source for the `/settings` row.
    pub fn all_model_thinking_levels(&self) -> BTreeMap<String, ModelThinkingLevel> {
        self.merged
            .get("modelThinkingLevels")
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| {
                serde_json::from_value::<ModelThinkingLevel>(v)
                    .ok()
                    .map(|l| (k, l))
            })
            .collect()
    }

    pub fn hide_thinking_block(&self) -> bool {
        self.merged.get_bool("hideThinkingBlock").unwrap_or(false)
    }

    /// `getShowCacheMissNotices` — `showCacheMissNotices`, default `false` (Pi
    /// settings-manager.ts:96 declares the key, `:850-852` the getter, `:872-875` the setter, which
    /// is cyrup's generic [`crate::SettingsManager::set`] on the GLOBAL scope; upstream's per-key setter
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

    /// `getDefaultTools(): string[] | undefined` (Pi v0.84.4 settings-manager.ts:1273-1276, key
    /// declared at `:128` as `defaultTools?: string[]; // Initial built-in tool selection`).
    ///
    /// A v0.84.4 addition (absent at v0.84.1): the INITIAL BUILT-IN tool selection a session starts
    /// with, in place of pi's `defaultActiveToolNames` (`read`/`bash`/`edit`/`write`, sdk.ts:256).
    /// Ported as a plain passthrough — upstream's getter only makes a defensive copy and validates
    /// nothing, so an unknown name is carried through and simply matches no tool.
    ///
    /// `None` (unset) and `Some(vec![])` (an explicit empty list) are DIFFERENT, exactly as for
    /// [`Self::enabled_models`]: unset means "pi's own four built-ins", empty means "no built-ins at
    /// all". Extension/SDK tools are unaffected either way — see the consumer in
    /// `cyrup-session-svc`'s `select_active_tools`.
    ///
    /// Tag-to-tag (ADR-0006): the key landed at `4d9aa837c` ("add configurable default tools") as an
    /// `allowedToolNames` allowlist, which also SUPPRESSED extension and SDK custom tools;
    /// `541045ae0` ("preserve extension tools with defaults") narrowed it to the initial built-in
    /// selection before v0.84.4 shipped. v0.84.4's behaviour — selection, not allowlist — is what is
    /// ported here.
    pub fn default_tools(&self) -> Option<Vec<String>> {
        self.merged.get("defaultTools").map(|v| {
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
            Some(v) => parse_http_idle_timeout_ms(v).ok_or_else(|| ConfigError::InvalidSetting {
                key: "httpIdleTimeoutMs".to_string(),
                value: v.to_string(),
            }),
        }
    }

    /// `websocketConnectTimeoutMs`, validated; `None` when unset (Pi `getWebSocketConnectTimeoutMs`,
    /// :837-839). Returns `Err` for a present-but-invalid value.
    pub fn websocket_connect_timeout_ms(&self) -> Result<Option<u64>, ConfigError> {
        match self.merged.get("websocketConnectTimeoutMs") {
            None => Ok(None),
            Some(v) => {
                parse_http_idle_timeout_ms(v)
                    .map(Some)
                    .ok_or_else(|| ConfigError::InvalidSetting {
                        key: "websocketConnectTimeoutMs".to_string(),
                        value: v.to_string(),
                    })
            }
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

    /// `tuiMode` — which renderer the interactive TUI starts in (Pi `getTuiMode`,
    /// settings-manager.ts:1128-1130 @v0.84.1). ADR-0005 §Decision A-3.
    ///
    /// Pi DEGRADES rather than validates: `this.settings.tuiMode === "fullscreen" ? "fullscreen" :
    /// "regular"` (`:1129`), so an unknown value and an absent key both answer
    /// [`TuiMode::Regular`] instead of erroring — the sole reason this reads the raw string rather
    /// than `serde_json::from_value::<TuiMode>`, which would reject `"Fullscreen"`, `true`, or a
    /// typo and force a `Result` on a getter upstream cannot fail.
    ///
    /// Degrading is a READ-side rule only. The unrecognized value stays in the document verbatim:
    /// [`Settings`] is a JSON map and every writer is a read-modify-write of the re-parsed file
    /// ([`super::SettingsManager::set`]), so a `settings.json` a newer pi wrote survives a cyrup
    /// edit of some other key byte-for-byte (R-07-004).
    pub fn tui_mode(&self) -> TuiMode {
        match self.merged.get_str("tuiMode").as_deref() {
            Some("fullscreen") => TuiMode::Fullscreen,
            _ => TuiMode::Regular,
        }
    }

    /// `fullscreenScrollbar` — the alternate screen's scrollbar policy, default `auto` (Pi
    /// `getFullscreenScrollbar`, settings-manager.ts:1138-1141 @v0.84.1: `mode === "always" || mode
    /// === "hidden" ? mode : "auto"`). ADR-0005 §Decision A-3.
    ///
    /// Same degrade-don't-reject contract as [`Self::tui_mode`], and the same preservation
    /// guarantee for the value on disk. Upstream documents the key as having "no effect in regular
    /// TUI mode" (`:136`); that conditionality belongs to the renderer, not to this accessor, which
    /// answers the configured policy in either mode.
    pub fn fullscreen_scrollbar(&self) -> FullscreenScrollbar {
        match self.merged.get_str("fullscreenScrollbar").as_deref() {
            Some("always") => FullscreenScrollbar::Always,
            Some("hidden") => FullscreenScrollbar::Hidden,
            _ => FullscreenScrollbar::Auto,
        }
    }

    /// `fullscreenExitOutput` — what leaving the alternate screen puts on the main screen, default
    /// `transcript` (Pi `getFullscreenExitOutput`, settings-manager.ts:1212-1214 @v0.84.4:
    /// `this.settings.fullscreenExitOutput === "resume-hint" ? "resume-hint" : "transcript"`).
    /// CFG-078.
    ///
    /// Same degrade-don't-reject contract as [`Self::tui_mode`] — upstream tests it directly, with
    /// `{"fullscreenExitOutput":"nothing"}` reading back as `"transcript"`
    /// (`test/settings-manager.test.ts:471-474` @v0.84.4) — and the same preservation guarantee for
    /// the value on disk. The key is documented "no effect in regular TUI mode" (`:143`); that
    /// conditionality belongs to the renderer, not to this accessor.
    pub fn fullscreen_exit_output(&self) -> FullscreenExitOutput {
        match self.merged.get_str("fullscreenExitOutput").as_deref() {
            Some("resume-hint") => FullscreenExitOutput::ResumeHint,
            _ => FullscreenExitOutput::Transcript,
        }
    }

    /// `fullscreenCopyOnSelect` — whether ending a fullscreen text selection copies it to the
    /// clipboard, default `true` (Pi `getFullscreenCopyOnSelect`, settings-manager.ts:1233-1235
    /// @v0.84.4: `this.settings.fullscreenCopyOnSelect ?? true`). CFG-078.
    ///
    /// `??`, not `||`, so only an ABSENT (or null) key defaults — an explicit `false` is honoured.
    /// The merged layer's `get_bool` answers `None` for a non-boolean, which is the
    /// nearest reading of a TypeScript field typed `boolean | undefined`: a `"false"` STRING is not
    /// a boolean, and pi would hand it to `if (this.copyOnSelect)` as a truthy value. That one
    /// mis-typed-value edge is the only place the two implementations differ here, and both end at
    /// "copy on select stays on".
    pub fn fullscreen_copy_on_select(&self) -> bool {
        self.merged
            .get_bool("fullscreenCopyOnSelect")
            .unwrap_or(true)
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
        self.merged.layer_string_list("extensions")
    }

    /// `getSkillPaths` — local skill file/dir paths (empty default; Pi settings-manager.ts:985-987).
    pub fn skill_paths(&self) -> Vec<String> {
        self.merged.layer_string_list("skills")
    }

    /// `getPromptTemplatePaths` — local prompt-template paths (empty default; Pi
    /// settings-manager.ts:1001-1003).
    pub fn prompt_template_paths(&self) -> Vec<String> {
        self.merged.layer_string_list("prompts")
    }

    /// `getThemePaths` — local theme file/dir paths (empty default; Pi
    /// settings-manager.ts:1017-1019).
    pub fn theme_paths(&self) -> Vec<String> {
        self.merged.layer_string_list("themes")
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

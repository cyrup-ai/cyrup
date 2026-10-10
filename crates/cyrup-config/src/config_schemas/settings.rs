//! `settings.schema.json` — port of pi's `SettingsSchema`
//! (`packages/coding-agent/src/core/settings-schema.ts` @f1b2e77f5, 474 lines), property for
//! property and in pi's declaration order, with pi's editor descriptions.
//!
//! Two deliberate differences from a verbatim copy:
//!
//! - **Defaults come from cyrup's getters**, not from a copy of pi's `SETTINGS_DEFAULTS`
//!   (`core/settings-defaults.ts`). [`runtime_defaults`] reads each one from
//!   [`EffectiveSettings`] over an empty document, so the schema advertises what cyrup actually
//!   applies. They coincide with pi's table today; reading them keeps it that way.
//! - **Product names**: a description that names pi as the actor ("Pi-managed HTTP clients") names
//!   cyrup instead. The rule text is otherwise pi's.
//!
//! Like pi, the document is open (`additionalProperties: true`, `settings-schema.ts:444`): an
//! extension's own key, or a key this table does not model, is never flagged — cyrup round-trips
//! unknown keys too (R-07-004).

use serde_json::{Value, json};

use super::typebox::{
    array, boolean, described, enumeration, integer, literal, number, object, opt, record, string,
    string_literals, union, with,
};
use crate::env::EnvVars;
use crate::settings::{EffectiveSettings, QuietStartup, WheelScrollLines};

/// `Number.MAX_SAFE_INTEGER` — the upper bound of pi's `nonNegativeSafeInteger`.
const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// The thinking levels `ModelThinkingLevelSchema` enumerates (`packages/ai/src/providers/
/// model-schema.ts:4-11` @f1b2e77f5: `MODEL_THINKING_LEVELS = ["off", ...THINKING_LEVELS]`).
pub(super) const MODEL_THINKING_LEVELS: [&str; 7] =
    ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// `ModelThinkingLevelSchema = Type.Enum(MODEL_THINKING_LEVELS)` (`model-schema.ts:10`).
pub(super) fn model_thinking_level() -> Value {
    enumeration(&MODEL_THINKING_LEVELS)
}

/// The settings document's `$defs` (`generate-schemas.ts:46-50`).
pub(super) fn definitions() -> Vec<(&'static str, Value)> {
    vec![("ModelThinkingLevel", model_thinking_level())]
}

/// `nonNegativeSafeInteger(options)` (`settings-schema.ts:5-7`).
fn non_negative_safe_integer() -> Value {
    with(
        integer(),
        json!({ "minimum": 0, "maximum": MAX_SAFE_INTEGER }),
    )
}

/// `timeoutSetting(options)` (`settings-schema.ts:9-11`): a non-negative number or `"disabled"`.
fn timeout_setting(description: &str) -> Value {
    described(
        union(vec![
            with(number(), json!({ "minimum": 0 })),
            literal("disabled"),
        ]),
        description,
    )
}

fn with_default(schema: Value, default: Value) -> Value {
    with(schema, json!({ "default": default }))
}

fn described_default(schema: Value, description: &str, default: Value) -> Value {
    with(
        schema,
        json!({ "description": description, "default": default }),
    )
}

/// Serialize a getter's enum result into the string it reads from (`rename_all` on each type).
fn ser(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Every `default` the schema states, read from the getter that applies it. A getter that
/// returns `Result` cannot fail on an empty document; `unwrap_or` keeps the generator panic-free
/// and, were it ever to fail, the committed-artifact test would show the changed value.
struct RuntimeDefaults {
    transport: Value,
    steering_mode: Value,
    follow_up_mode: Value,
    compaction_enabled: Value,
    compaction_reserve_tokens: Value,
    compaction_keep_recent_tokens: Value,
    branch_summary_reserve_tokens: Value,
    branch_summary_skip_prompt: Value,
    retry_enabled: Value,
    retry_max_retries: Value,
    retry_base_delay_ms: Value,
    retry_max_agent_delay_ms: Value,
    provider_max_retry_delay_ms: Value,
    hide_thinking_block: Value,
    show_cache_miss_notices: Value,
    quiet_startup: Value,
    default_project_trust: Value,
    collapse_changelog: Value,
    enable_install_telemetry: Value,
    enable_analytics: Value,
    enable_skill_commands: Value,
    show_images: Value,
    image_width_cells: Value,
    clear_on_shrink: Value,
    show_terminal_progress: Value,
    image_auto_resize: Value,
    block_images: Value,
    double_escape_action: Value,
    tree_filter_mode: Value,
    editor_padding_x: Value,
    output_pad: Value,
    autocomplete_max_visible: Value,
    code_block_indent: Value,
    mermaid: Value,
    anthropic_extra_usage: Value,
    codemode_mode: Value,
    codemode_inline_budget: Value,
    cache_warming: Value,
    tui_mode: Value,
    fullscreen_exit_output: Value,
    fullscreen_scrollbar: Value,
    fullscreen_copy_on_select: Value,
    fullscreen_wheel_scroll_lines: Value,
}

fn runtime_defaults() -> RuntimeDefaults {
    let s = EffectiveSettings::default();
    let env = EnvVars::default();
    RuntimeDefaults {
        transport: s.transport().into(),
        steering_mode: s.steering_mode().into(),
        follow_up_mode: s.follow_up_mode().into(),
        compaction_enabled: s.compaction_enabled().into(),
        compaction_reserve_tokens: s.compaction_reserve_tokens(None).unwrap_or_default().into(),
        compaction_keep_recent_tokens: s
            .compaction_keep_recent_tokens(None)
            .unwrap_or_default()
            .into(),
        branch_summary_reserve_tokens: s.branch_summary_reserve_tokens().into(),
        branch_summary_skip_prompt: s.branch_summary_skip_prompt().into(),
        retry_enabled: s.retry_enabled().into(),
        retry_max_retries: s.retry_max_retries().into(),
        retry_base_delay_ms: s.retry_base_delay_ms().into(),
        retry_max_agent_delay_ms: s.retry_max_agent_delay_ms().into(),
        provider_max_retry_delay_ms: s.provider_max_retry_delay_ms().into(),
        hide_thinking_block: s.hide_thinking_block().into(),
        show_cache_miss_notices: s.show_cache_miss_notices().into(),
        // `QuietStartup` has no serde form: it reads `true` / `"header"` / anything else, and its
        // default is the `false` arm.
        quiet_startup: match s.quiet_startup() {
            QuietStartup::Off => false.into(),
            QuietStartup::On => true.into(),
            QuietStartup::Header => "header".into(),
        },
        default_project_trust: ser(s.default_project_trust()),
        collapse_changelog: s.collapse_changelog().into(),
        enable_install_telemetry: s.enable_install_telemetry().into(),
        enable_analytics: s.enable_analytics().into(),
        enable_skill_commands: s.enable_skill_commands().into(),
        show_images: s.show_images().into(),
        image_width_cells: s.image_width_cells().into(),
        clear_on_shrink: s.clear_on_shrink(&env).into(),
        show_terminal_progress: s.show_terminal_progress().into(),
        image_auto_resize: s.image_auto_resize().into(),
        block_images: s.block_images().into(),
        double_escape_action: s.double_escape_action().into(),
        tree_filter_mode: s.tree_filter_mode().into(),
        editor_padding_x: s.editor_padding_x().into(),
        output_pad: s.output_pad().into(),
        autocomplete_max_visible: s.autocomplete_max_visible().into(),
        code_block_indent: s.code_block_indent().into(),
        mermaid: ser(s.mermaid_rendering_mode()),
        // `warnings()` keeps the raw optional; the `?? true` lives at its consumer, exactly as pi's
        // `getWarnings()` leaves it to `SETTINGS_DEFAULTS.warnings.anthropicExtraUsage`.
        anthropic_extra_usage: s.warnings().anthropic_extra_usage.unwrap_or(true).into(),
        codemode_mode: ser(s.codemode_mode()),
        codemode_inline_budget: inline_budget_value(s.codemode_inline_budget()),
        cache_warming: ser(s.cache_warming_mode()),
        tui_mode: ser(s.tui_mode()),
        fullscreen_exit_output: ser(s.fullscreen_exit_output()),
        fullscreen_scrollbar: ser(s.fullscreen_scrollbar()),
        fullscreen_copy_on_select: s.fullscreen_copy_on_select().into(),
        fullscreen_wheel_scroll_lines: match s.fullscreen_wheel_scroll_lines() {
            WheelScrollLines::Auto => "auto".into(),
            WheelScrollLines::Lines(n) => n.get().into(),
        },
    }
}

/// The inline budget is an `f64` at runtime; JSON shows an integral value without a fraction, as
/// pi's `3000` does.
fn inline_budget_value(budget: f64) -> Value {
    #[allow(clippy::cast_possible_truncation)]
    let integral = budget as i64;
    #[allow(clippy::cast_precision_loss)]
    if integral as f64 == budget {
        integral.into()
    } else {
        budget.into()
    }
}

/// `CompactionModelOverrideSchema` (`settings-schema.ts:13-16`).
fn compaction_model_override() -> Value {
    object(vec![
        opt("reserveTokens", non_negative_safe_integer()),
        opt("keepRecentTokens", non_negative_safe_integer()),
    ])
}

/// `CompactionSettingsSchema` (`settings-schema.ts:18-27`).
fn compaction(d: &RuntimeDefaults) -> Value {
    object(vec![
        opt(
            "enabled",
            with_default(boolean(), d.compaction_enabled.clone()),
        ),
        opt(
            "reserveTokens",
            with_default(
                non_negative_safe_integer(),
                d.compaction_reserve_tokens.clone(),
            ),
        ),
        opt(
            "keepRecentTokens",
            with_default(
                non_negative_safe_integer(),
                d.compaction_keep_recent_tokens.clone(),
            ),
        ),
        opt(
            "modelOverrides",
            described(
                record(compaction_model_override()),
                "Per-model overrides keyed by exact \"provider/modelId\" strings.",
            ),
        ),
    ])
}

/// `BranchSummarySettingsSchema` (`settings-schema.ts:29-42`).
fn branch_summary(d: &RuntimeDefaults) -> Value {
    object(vec![
        opt(
            "reserveTokens",
            described_default(
                number(),
                "Tokens reserved for the prompt and LLM response.",
                d.branch_summary_reserve_tokens.clone(),
            ),
        ),
        opt(
            "skipPrompt",
            described_default(
                boolean(),
                "When true, skips the \"Summarize branch?\" prompt and defaults to no summary.",
                d.branch_summary_skip_prompt.clone(),
            ),
        ),
    ])
}

/// `RetrySettingsSchema` with its nested `ProviderRetrySettingsSchema` (`settings-schema.ts:44-75`).
fn retry(d: &RuntimeDefaults) -> Value {
    let provider = object(vec![
        opt(
            "timeoutMs",
            described(number(), "SDK or provider request timeout in milliseconds."),
        ),
        opt(
            "maxRetries",
            described(number(), "SDK or provider retry attempts."),
        ),
        opt(
            "maxRetryDelayMs",
            described_default(
                number(),
                "Maximum server-requested delay before failing.",
                d.provider_max_retry_delay_ms.clone(),
            ),
        ),
    ]);
    object(vec![
        opt("enabled", with_default(boolean(), d.retry_enabled.clone())),
        opt(
            "maxRetries",
            with_default(number(), d.retry_max_retries.clone()),
        ),
        opt(
            "baseDelayMs",
            described_default(
                number(),
                "Exponential backoff base delay in milliseconds: 2s, 4s, 8s.",
                d.retry_base_delay_ms.clone(),
            ),
        ),
        opt(
            "maxAgentDelayMs",
            with_default(number(), d.retry_max_agent_delay_ms.clone()),
        ),
        opt("provider", provider),
        opt(
            "maxDelayMs",
            with(
                number(),
                json!({
                    "description": "Legacy retry delay setting. Use provider.maxRetryDelayMs instead.",
                    "deprecated": true,
                }),
            ),
        ),
    ])
}

/// `TerminalSettingsSchema` (`settings-schema.ts:77-107`).
fn terminal(d: &RuntimeDefaults) -> Value {
    object(vec![
        opt(
            "showImages",
            described_default(
                boolean(),
                "Show images when the terminal supports them.",
                d.show_images.clone(),
            ),
        ),
        opt(
            "imageWidthCells",
            described_default(
                number(),
                "Preferred inline image width in terminal cells.",
                d.image_width_cells.clone(),
            ),
        ),
        opt(
            "clearOnShrink",
            described_default(
                boolean(),
                "Clear empty rows when content shrinks.",
                d.clear_on_shrink.clone(),
            ),
        ),
        opt(
            "showTerminalProgress",
            described_default(
                boolean(),
                "Show OSC 9;4 terminal progress indicators.",
                d.show_terminal_progress.clone(),
            ),
        ),
        opt("hyperlinks", union(vec![boolean(), literal("auto")])),
        opt(
            "images",
            union(vec![
                literal("kitty"),
                literal("iterm2"),
                literal("auto"),
                literal(false),
            ]),
        ),
        opt("trueColor", union(vec![boolean(), literal("auto")])),
    ])
}

/// `ImageSettingsSchema` (`settings-schema.ts:109-122`).
fn images(d: &RuntimeDefaults) -> Value {
    object(vec![
        opt(
            "autoResize",
            described_default(
                boolean(),
                "Resize images to 2000x2000 maximum for better model compatibility.",
                d.image_auto_resize.clone(),
            ),
        ),
        opt(
            "blockImages",
            described_default(
                boolean(),
                "When true, prevents all images from being sent to LLM providers.",
                d.block_images.clone(),
            ),
        ),
    ])
}

/// `thinkingBudgetsSettings(options)` (`settings-schema.ts:124-134`).
fn thinking_budgets() -> Value {
    object(vec![
        opt("minimal", number()),
        opt("low", number()),
        opt("medium", number()),
        opt("high", number()),
    ])
}

/// `MarkdownSettingsSchema` (`settings-schema.ts:138-145`).
fn markdown(d: &RuntimeDefaults) -> Value {
    object(vec![
        opt(
            "codeBlockIndent",
            with_default(string(), d.code_block_indent.clone()),
        ),
        opt(
            "mermaid",
            with_default(
                string_literals(&["off", "final", "streaming"]),
                d.mermaid.clone(),
            ),
        ),
    ])
}

/// `WarningSettingsSchema` (`settings-schema.ts:147-149`).
fn warnings(d: &RuntimeDefaults) -> Value {
    object(vec![opt(
        "anthropicExtraUsage",
        with_default(boolean(), d.anthropic_extra_usage.clone()),
    )])
}

/// `CodemodeSettingsSchema` (`settings-schema.ts:151-174`).
fn codemode(d: &RuntimeDefaults) -> Value {
    object(vec![
        opt(
            "mode",
            described_default(
                string_literals(&["on", "only"]),
                "How codemode presents tools. \"on\" keeps direct tools declared and annotates \
                 tools callable from scripts; codemode lists only tools without direct exposure. \
                 \"only\" lists every script-callable tool in codemode and does not declare \
                 active direct tools.",
                d.codemode_mode.clone(),
            ),
        ),
        opt(
            "inlineBudget",
            with(
                number(),
                json!({
                    "minimum": 0,
                    "description": "Estimated tokens available for inline codemode tool declarations.",
                    "default": d.codemode_inline_budget,
                }),
            ),
        ),
    ])
}

fn string_array() -> Value {
    array(string())
}

/// `PackageSourceSchema` (`settings-schema.ts:176-195`).
fn package_source() -> Value {
    described(
        union(vec![
            described(string(), "Load all resources from the package."),
            object(vec![
                super::typebox::req("source", string()),
                opt(
                    "autoload",
                    described(
                        boolean(),
                        "When false, start empty and only apply explicit resource patterns.",
                    ),
                ),
                opt("extensions", string_array()),
                opt("skills", string_array()),
                opt("prompts", string_array()),
                opt("themes", string_array()),
            ]),
        ]),
        "Package source for npm or git packages. Use the string form to load all resources or \
         the object form to filter resources.",
    )
}

/// `SkillsInputSchema` (`settings-schema.ts:197-209`): the current array form, or the legacy
/// object form `migrate_settings` rewrites.
fn skills_input() -> Value {
    described(
        union(vec![
            string_array(),
            with(
                object(vec![
                    opt("enableSkillCommands", boolean()),
                    opt("customDirectories", string_array()),
                ]),
                json!({ "deprecated": true }),
            ),
        ]),
        "Local skill file paths or directories.",
    )
}

/// `SettingsSchema` (`settings-schema.ts:211-445`).
pub(super) fn settings_schema() -> Value {
    let d = runtime_defaults();
    let one_at_a_time = || string_literals(&["all", "one-at-a-time"]);
    let regular_mode_only = "No effect in regular TUI mode.";
    with(
        object(vec![
            opt(
                "$schema",
                super::schema_reference_property(Some("JSON Schema reference.")),
            ),
            opt("lastChangelogVersion", string()),
            opt("defaultProvider", string()),
            opt("defaultModel", string()),
            opt("defaultThinkingLevel", model_thinking_level()),
            opt(
                "modelThinkingLevels",
                described(
                    record(model_thinking_level()),
                    "Per-model default thinking level overrides keyed by \"provider/modelId\".",
                ),
            ),
            opt(
                "transport",
                with_default(
                    string_literals(&["auto", "sse", "websocket", "websocket-cached"]),
                    d.transport.clone(),
                ),
            ),
            opt(
                "steeringMode",
                with_default(one_at_a_time(), d.steering_mode.clone()),
            ),
            opt(
                "followUpMode",
                with_default(one_at_a_time(), d.follow_up_mode.clone()),
            ),
            opt("theme", string()),
            opt("compaction", compaction(&d)),
            opt("branchSummary", branch_summary(&d)),
            opt("retry", retry(&d)),
            opt(
                "hideThinkingBlock",
                with_default(boolean(), d.hide_thinking_block.clone()),
            ),
            opt(
                "showCacheMissNotices",
                described_default(
                    boolean(),
                    "Show cache cost and provider recovery notices.",
                    d.show_cache_miss_notices.clone(),
                ),
            ),
            opt(
                "externalEditor",
                described(
                    string(),
                    "Command for Ctrl+G external editor; takes precedence over VISUAL and EDITOR.",
                ),
            ),
            opt(
                "shellPath",
                described(
                    string(),
                    "Custom shell path, for example for Cygwin on Windows, with support for \
                     leading ~ expansion.",
                ),
            ),
            opt(
                "quietStartup",
                described_default(
                    union(vec![boolean(), literal("header")]),
                    "When true, hide all startup output. When \"header\", keep only the startup \
                     header.",
                    d.quiet_startup.clone(),
                ),
            ),
            opt(
                "defaultProjectTrust",
                described_default(
                    string_literals(&["ask", "always", "never"]),
                    "Global setting only.",
                    d.default_project_trust.clone(),
                ),
            ),
            opt(
                "shellCommandPrefix",
                described(
                    string(),
                    "Prefix prepended to every bash command, for example to enable shell aliases.",
                ),
            ),
            opt(
                "npmCommand",
                described(
                    string_array(),
                    "Command used for npm package lookup and installation, in argv form such as \
                     [\"mise\", \"exec\", \"node@20\", \"--\", \"npm\"].",
                ),
            ),
            opt(
                "collapseChangelog",
                described_default(
                    boolean(),
                    "Show the condensed changelog after update; use /changelog for the full \
                     changelog.",
                    d.collapse_changelog.clone(),
                ),
            ),
            opt(
                "enableInstallTelemetry",
                described_default(
                    boolean(),
                    "Send an anonymous version and update ping after changelog-detected updates.",
                    d.enable_install_telemetry.clone(),
                ),
            ),
            opt(
                "enableAnalytics",
                described_default(
                    boolean(),
                    "Opt in to analytics data sharing.",
                    d.enable_analytics.clone(),
                ),
            ),
            opt(
                "trackingId",
                described(
                    string(),
                    "Analytics tracking identifier, generated when analytics is enabled.",
                ),
            ),
            opt(
                "packages",
                described(
                    array(package_source()),
                    "npm or git package sources, as strings or objects with resource filtering.",
                ),
            ),
            opt(
                "extensions",
                described(string_array(), "Local extension file paths or directories."),
            ),
            opt(
                "deviceId",
                described(
                    string(),
                    "Stable installation UUID, generated when authentication first needs it. \
                     Global only.",
                ),
            ),
            opt("skills", skills_input()),
            opt(
                "prompts",
                described(
                    string_array(),
                    "Local prompt template file paths or directories.",
                ),
            ),
            opt(
                "themes",
                described(string_array(), "Local theme file paths or directories."),
            ),
            opt(
                "enableSkillCommands",
                described_default(
                    boolean(),
                    "Register skills as /skill:name commands.",
                    d.enable_skill_commands.clone(),
                ),
            ),
            opt("terminal", terminal(&d)),
            opt("images", images(&d)),
            opt(
                "enabledModels",
                described(
                    string_array(),
                    "Model patterns for cycling, in the same format as the --models CLI flag.",
                ),
            ),
            opt(
                "defaultTools",
                described(
                    string_array(),
                    "Initial tool selection. Plain names replace the inherited selection; +name \
                     and -name entries add or remove tools.",
                ),
            ),
            opt(
                "doubleEscapeAction",
                described_default(
                    string_literals(&["fork", "tree", "none"]),
                    "Action for double-escape with an empty editor.",
                    d.double_escape_action.clone(),
                ),
            ),
            opt(
                "treeFilterMode",
                described_default(
                    string_literals(&["default", "no-tools", "user-only", "labeled-only", "all"]),
                    "Default filter when opening /tree.",
                    d.tree_filter_mode.clone(),
                ),
            ),
            opt(
                "thinkingBudgets",
                described(
                    thinking_budgets(),
                    "Custom token budgets for thinking levels.",
                ),
            ),
            opt(
                "editorPaddingX",
                described_default(
                    number(),
                    "Horizontal padding for the input editor.",
                    d.editor_padding_x.clone(),
                ),
            ),
            opt(
                "outputPad",
                described_default(
                    union(vec![literal(0), literal(1)]),
                    "Horizontal padding for transcript content.",
                    d.output_pad.clone(),
                ),
            ),
            opt(
                "autocompleteMaxVisible",
                described_default(
                    number(),
                    "Maximum visible items in the autocomplete dropdown.",
                    d.autocomplete_max_visible.clone(),
                ),
            ),
            opt(
                "showHardwareCursor",
                described(
                    boolean(),
                    "Show the terminal cursor while still positioning it for IME.",
                ),
            ),
            opt("markdown", markdown(&d)),
            opt("warnings", warnings(&d)),
            opt("codemode", codemode(&d)),
            opt(
                "sessionDir",
                described(
                    string(),
                    "Custom session storage directory, in the same format as the --session-dir \
                     CLI flag.",
                ),
            ),
            opt(
                "httpProxy",
                described(
                    string(),
                    "Proxy URL applied as HTTP_PROXY and HTTPS_PROXY for cyrup-managed HTTP \
                     clients.",
                ),
            ),
            opt(
                "httpIdleTimeoutMs",
                timeout_setting(
                    "HTTP header or body idle timeout in milliseconds; 0 or \"disabled\" \
                     disables it.",
                ),
            ),
            opt(
                "cacheWarming",
                described_default(
                    string_literals(&["off", "streaming", "idle"]),
                    "Cache-warming profile. \"idle\" also warms between agent runs. Global only \
                     because each refresh costs money.",
                    d.cache_warming.clone(),
                ),
            ),
            opt(
                "websocketConnectTimeoutMs",
                timeout_setting(
                    "WebSocket connect or open handshake timeout in milliseconds; 0 or \
                     \"disabled\" disables it.",
                ),
            ),
            opt(
                "tuiMode",
                with_default(
                    string_literals(&["regular", "fullscreen"]),
                    d.tui_mode.clone(),
                ),
            ),
            opt(
                "fullscreenExitOutput",
                described_default(
                    string_literals(&["transcript", "resume-hint"]),
                    regular_mode_only,
                    d.fullscreen_exit_output.clone(),
                ),
            ),
            opt(
                "fullscreenScrollbar",
                described_default(
                    string_literals(&["auto", "always", "hidden"]),
                    regular_mode_only,
                    d.fullscreen_scrollbar.clone(),
                ),
            ),
            opt(
                "fullscreenCopyOnSelect",
                described_default(
                    boolean(),
                    regular_mode_only,
                    d.fullscreen_copy_on_select.clone(),
                ),
            ),
            opt(
                "fullscreenWheelScrollLines",
                described_default(
                    union(vec![number(), literal("auto")]),
                    "Lines scrolled per wheel event in fullscreen mode; numeric values are \
                     clamped from 1 to 100.",
                    d.fullscreen_wheel_scroll_lines.clone(),
                ),
            ),
            opt(
                "queueMode",
                with(
                    one_at_a_time(),
                    json!({
                        "description": "Legacy setting migrated to steeringMode.",
                        "deprecated": true,
                    }),
                ),
            ),
            opt(
                "websockets",
                with(
                    boolean(),
                    json!({
                        "description": "Legacy setting migrated to transport.",
                        "deprecated": true,
                    }),
                ),
            ),
        ]),
        json!({ "additionalProperties": true }),
    )
}

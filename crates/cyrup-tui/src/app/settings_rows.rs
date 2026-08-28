use super::*;

/// Build the `/settings` grid rows from the live effective settings (Pi `settings-selector.ts`
/// `SettingsConfig` → `SettingItem`s, :479-712). Each row's `id` is the dotted settings key persisted
/// on cycle; toggles cycle `true`/`false`, choices cycle their fixed sets. Read straight off
/// [`cyrup_session_svc::EffectiveSettings`] so the displayed value matches the merged config.
/// Pi `modelThinkingOverridesSummary` (`settings-selector.ts:184-188`): `"none"` when the map is
/// empty, else `"{count} configured"`. Verbatim, because it is the row's user-visible value.
fn model_thinking_summary(eff: &cyrup_session_svc::EffectiveSettings) -> String {
    let count = eff.all_model_thinking_levels().len();
    if count == 0 {
        "none".to_string()
    } else {
        format!("{count} configured")
    }
}

pub(crate) fn settings_rows(
    eff: &cyrup_session_svc::EffectiveSettings,
    current_theme: &str,
    keymap: &Keymap,
    thinking_level: &str,
    supports_images: bool,
    env: &cyrup_session_svc::EnvVars,
) -> Vec<SettingRow> {
    let choices = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // `const followUpKey = keyDisplayText("app.message.followUp")` (`settings-selector.ts:491`),
    // interpolated into the follow-up row's description at `:513`. `keyDisplayText` is `keyText`
    // with `{ capitalize: true }` (`keybinding-hints.ts:37-39`), i.e. `Alt+Enter`, and it reads the
    // LIVE table — a rebind changes the sentence.
    let follow_up_key = keymap
        .keys_label(Action::FollowUp)
        .map(|k| crate::chrome::format_key_text(&k, true))
        .unwrap_or_default();
    // `const cycleThinkingKey = keyDisplayText("app.thinking.cycle")` (`settings-selector.ts:448`),
    // resolved the same live way as `follow_up_key` above and interpolated at `:576`.
    let cycle_thinking_key = keymap
        .keys_label(Action::ThinkingCycle)
        .map(|k| crate::chrome::format_key_text(&k, true))
        .unwrap_or_default();
    // TUI-036 — `Show images` / `Image width` are offered ONLY on a terminal that has an image
    // protocol: `// Only show image toggle if terminal supports it` / `if (supportsImages) {
    // items.splice(1, 0, {id:"show-images", …}); items.splice(2, 0, {id:"image-width-cells", …}); }`
    // (`settings-selector.ts:654-671` @v0.83.0), where `supportsImages` comes from
    // `getCapabilities()`. The neighbouring `auto-resize-images` row is deliberately NOT gated — it
    // is spliced at `supportsImages ? 3 : 1` — which is exactly the distinction cyrup lost by
    // pushing all three unconditionally. On a plain xterm the two rows could not change anything,
    // and every row below them sat at a different index from pi's.
    let image_rows: Vec<SettingRow> = if supports_images {
        vec![
            SettingRow::toggle("terminal.showImages", "Show images", eff.show_images())
                .with_description("Render images inline in terminal"),
            SettingRow::choice(
                "terminal.imageWidthCells",
                "Image width",
                eff.image_width_cells().to_string(),
                choices(&["60", "80", "120"]),
            )
            .with_description("Preferred inline image width in terminal cells"),
        ]
    } else {
        Vec::new()
    };
    let mut rows = vec![
        // The "Theme" row opens the theme picker (Pi `SettingItem.submenu` → `ThemeSubmenu`,
        // settings-selector.ts:603-610) — the one in-app path Pi reaches theme switching through.
        SettingRow::submenu("theme", "Theme", current_theme.to_string(), "theme")
            .with_description("Color theme for the interface"),
        SettingRow::toggle("compaction.enabled", "Auto-compact", eff.compaction_enabled())
            .with_description("Automatically compact context when it gets too large"),
    ];
    rows.extend(image_rows);
    rows.extend([
        SettingRow::toggle("images.autoResize", "Auto-resize images", eff.image_auto_resize())
            .with_description("Resize large images to 2000x2000 max for better model compatibility"),
        SettingRow::toggle("images.blockImages", "Block images", eff.block_images())
            .with_description("Prevent images from being sent to LLM providers"),
        SettingRow::toggle("enableSkillCommands", "Skill commands", eff.enable_skill_commands())
            .with_description("Register skills as /skill:name commands"),
        // TUI-041 — these two getters resolve `setting → env → false`
        // (`cyrup-config/src/settings.rs`, `.unwrap_or(env.hardware_cursor)` /
        // `.unwrap_or(env.clear_on_shrink)`, sourced from `CYRUP_HARDWARE_CURSOR`/`PI_HARDWARE_CURSOR`
        // and `CYRUP_CLEAR_ON_SHRINK`/`PI_CLEAR_ON_SHRINK`). The grid used to build both rows against
        // a **default** `EnvVars`, i.e. an env-blind read, while the RUNTIME used
        // `EnvVars::from_process()` (`crates/cyrup/src/main.rs`) — so with either variable set and
        // nothing persisted, `/settings` reported `false` for behaviour that was on and toggling the
        // row looked like a no-op. Pi renders every row from the same resolved value the runtime
        // uses; it has no second, env-blind read path.
        SettingRow::toggle("showHardwareCursor", "Show hardware cursor", eff.show_hardware_cursor(env))
            .with_description("Show the terminal cursor while still positioning it for IME support"),
        SettingRow::toggle("terminal.clearOnShrink", "Clear on shrink", eff.clear_on_shrink(env))
            .with_description("Clear empty rows when content shrinks (may cause flicker)"),
        SettingRow::choice(
            "editorPaddingX",
            "Editor padding",
            eff.editor_padding_x().to_string(),
            choices(&["0", "1", "2", "3"]),
        )
        .with_description("Horizontal padding for input editor (0-3)"),
        // Inserted right after editor-padding, matching Pi (`settings-selector.ts:681-689` splices the
        // "Output padding" row after "editor-padding"). Cycles 0|1; honored live by the transcript.
        SettingRow::choice(
            "outputPad",
            "Output padding",
            eff.output_pad().to_string(),
            choices(&["0", "1"]),
        )
        .with_description(
            "Horizontal padding for user messages, assistant messages, and thinking",
        ),
        SettingRow::choice(
            "autocompleteMaxVisible",
            "Autocomplete max items",
            eff.autocomplete_max_visible().to_string(),
            choices(&["3", "5", "7", "10", "15", "20"]),
        )
        .with_description("Max visible items in autocomplete dropdown (3-20)"),
        // `httpIdleTimeoutMs` — cycle the raw millisecond presets (Pi shows human labels; the persisted
        // value is the same ms number). `disabled` = 0 (`HTTP_IDLE_TIMEOUT_CHOICES`, http-dispatcher.ts:5).
        SettingRow::choice(
            "httpIdleTimeoutMs",
            "HTTP idle timeout (ms)",
            eff.http_idle_timeout_ms().unwrap_or(300_000).to_string(),
            choices(&["30000", "60000", "120000", "300000", "0"]),
        )
        .with_description(
            "Maximum idle gap while waiting for HTTP headers or body chunks. Disable for local \
             models that pause longer than five minutes.",
        ),
        SettingRow::toggle("hideThinkingBlock", "Hide thinking", eff.hide_thinking_block())
            .with_description("Hide thinking blocks in assistant responses"),
        SettingRow::toggle("collapseChangelog", "Collapse changelog", eff.collapse_changelog())
            .with_description("Show condensed changelog after updates"),
        SettingRow::toggle("quietStartup", "Quiet startup", eff.quiet_startup())
            .with_description("Disable verbose printing at startup"),
        SettingRow::toggle(
            "enableInstallTelemetry",
            "Install telemetry",
            eff.enable_install_telemetry(),
        )
        .with_description(
            "Send an anonymous version/update ping after changelog-detected updates",
        ),
        SettingRow::toggle(
            "terminal.showTerminalProgress",
            "Terminal progress",
            eff.show_terminal_progress(),
        )
        .with_description("Show OSC 9;4 progress indicators in the terminal tab bar"),
        SettingRow::choice(
            "steeringMode",
            "Steering mode",
            eff.steering_mode(),
            choices(&["all", "one-at-a-time"]),
        )
        .with_description(
            "Enter while streaming queues steering messages. 'one-at-a-time': deliver one, wait \
             for response. 'all': deliver all at once.",
        ),
        SettingRow::choice(
            "followUpMode",
            "Follow-up mode",
            eff.follow_up_mode(),
            choices(&["all", "one-at-a-time"]),
        )
        .with_description(format!(
            "{follow_up_key} queues follow-up messages until agent stops. 'one-at-a-time': \
             deliver one, wait for response. 'all': deliver all at once."
        )),
        SettingRow::choice(
            "transport",
            "Transport",
            eff.transport(),
            // Pi's four `TransportSetting` values in Pi's own cycle order (`settings-selector.ts:
            // 505-510`: `["sse", "websocket", "websocket-cached", "auto"]`). `websocket-cached` was
            // missing here, so a value the settings parser and `parse_transport` both accept was
            // unreachable from `/settings` and cycling past `sse` could never select it.
            choices(&["sse", "websocket", "websocket-cached", "auto"]),
        )
        .with_description(
            "Preferred transport for providers that support multiple transports",
        ),
        SettingRow::choice(
            "doubleEscapeAction",
            "Double-escape action",
            eff.double_escape_action(),
            choices(&["fork", "tree", "none"]),
        )
        .with_description("Action when pressing Escape twice with empty editor"),
        SettingRow::choice(
            "treeFilterMode",
            "Tree filter mode",
            eff.tree_filter_mode(),
            choices(&["default", "no-tools", "user-only", "labeled-only", "all"]),
        )
        .with_description("Default filter when opening /tree"),
        SettingRow::choice(
            "defaultProjectTrust",
            "Default project trust",
            default_trust_label(eff.default_project_trust()),
            choices(&["ask", "always", "never"]),
        )
        .with_description(
            "Fallback behavior when no extension or saved trust decision decides project trust",
        ),
        // TUI-032 — the two submenu rows pi ships that cyrup had no counterpart for.
        //
        // `warnings` (`settings-selector.ts:578-590` @v0.83.0): `currentValue: "configure"`,
        // `submenu: … new WarningSettingsSubmenu(currentWarnings, …)` whose single item is
        // `anthropic-extra-usage` (`:130-136`). `warnings.anthropicExtraUsage` is fully parsed and
        // honoured by cyrup (`cyrup-config/src/settings.rs:922-926`) and had **no editor**, so the
        // only way to turn the Anthropic paid-extra-usage warning off was to hand-edit
        // `settings.json`.
        SettingRow::submenu("warnings", "Warnings", "configure", "warnings")
            .with_description("Enable or disable individual warnings"),
        // `thinking` (`:591-611`): `label: "Thinking level"`, a `SelectSubmenu` over
        // `config.availableThinkingLevels`. cyrup already had the picker built —
        // `SelectorKind::Thinking` with a live confirm arm — and no way in: `open_selector` had
        // exactly one call site and it only ever constructed `SelectorKind::Theme`. Shift+Tab
        // cycled blindly with no list of the levels.
        SettingRow::submenu("thinking", "Thinking level", thinking_level.to_string(), "thinking")
            .with_description("Reasoning depth for thinking-capable models"),
        // GAP 3 — `id: "model-thinking"` (`settings-selector.ts:574-577`). `currentValue` is
        // `modelThinkingOverridesSummary`: `"none"` at zero, else `"{n} configured"` (`:184-188`).
        // The description interpolates the live cycle key exactly as upstream does (`:576`).
        SettingRow::submenu(
            "model-thinking",
            "Default thinking level per model",
            model_thinking_summary(eff),
            "model-thinking",
        )
        .with_description(format!(
            "Override the default thinking level for specific models. {cycle_thinking_key} cycles in-session."
        )),
        // ADR-0005 §A-4 — the two alternate-screen rows, withheld until §A-3's settings keys and
        // §B-14's renderer existed. Pi: `{id:"tui-mode", label:"TUI mode", …, values:["regular",
        // "fullscreen"]}` (`components/settings-selector.ts:671-676`) and `{id:
        // "fullscreen-scrollbar", …, values:["auto","always","hidden"]}` (`:685-691`), dispatched
        // at `:904-905` / `:910-911`. `parse_setting_value` re-types neither — both cycle strings
        // and both settings keys hold strings — so the row `id` IS the settings key
        // `EffectiveSettings::tui_mode` / `::fullscreen_scrollbar` reads back.
        //
        // Pi's third row of that group, `fullscreen-exit-output` (`:678-684`), is deliberately
        // absent: the `fullscreenExitOutput` key does not exist at v0.84.1, the tag ADR-0005 §A-3
        // ports, so cyrup has no getter for it and offering the row would invent a config surface.
        //
        // PLACEMENT — upstream has this pair immediately BEFORE its `theme` row, i.e. last. cyrup
        // hoisted `theme` to row 0 long ago (`crates/cyrup-tui/src/tests/selector_wiring.rs:221`
        // pins it there), so "adjacent to theme" is not reachable without moving a row a test
        // fixes in place. Appending keeps pi's RELATIVE order for everything that remains —
        // `warnings`, `thinking`, `model-thinking`, then this pair — the closest faithful position left.
        SettingRow::choice(
            "tuiMode",
            "TUI mode",
            eff.tui_mode().as_str(),
            choices(&["regular", "fullscreen"]),
        )
        .with_description("Interface layout; fullscreen mode is experimental"),
        // The "no effect in regular mode" half of the description is upstream's own wording and is
        // load-bearing: the key is read by the alternate screen's scrollbar only, so in the default
        // renderer this row is a stored preference and nothing more (the same conditionality
        // `EffectiveSettings::fullscreen_scrollbar` documents on the getter — it answers the
        // configured policy in either mode, and the renderer decides whether that matters).
        SettingRow::choice(
            "fullscreenScrollbar",
            "Fullscreen scrollbar",
            eff.fullscreen_scrollbar().as_str(),
            choices(&["auto", "always", "hidden"]),
        )
        .with_description("Scrollbar behavior in fullscreen mode; has no effect in regular mode"),
    ]);
    rows
}

/// Build the `/settings` grid against default effective settings — the test seam for the two rows
/// TUI-032 adds and the two TUI-036 gates. Production always goes through the `C::OpenSelector`
/// arm, which sources the live session's settings.
#[cfg(test)]
pub(crate) fn settings_rows_for_test_with_images(supports_images: bool) -> Vec<SettingRow> {
    let eff = cyrup_session_svc::EffectiveSettings::default();
    settings_rows(
        &eff,
        "dark",
        &Keymap::default(),
        "medium",
        supports_images,
        &cyrup_session_svc::EnvVars::default(),
    )
}

/// [`settings_rows_for_test_with_images`] with an image-capable terminal.
#[cfg(test)]
pub(crate) fn settings_rows_for_test() -> Vec<SettingRow> {
    settings_rows_for_test_with_images(true)
}

/// The settings string for a [`cyrup_session_svc::DefaultProjectTrust`] (Pi serializes it as the
/// lowercase enum value `ask`/`always`/`never`).
pub(crate) fn default_trust_label(trust: cyrup_session_svc::DefaultProjectTrust) -> String {
    use cyrup_session_svc::DefaultProjectTrust as D;
    match trust {
        D::Ask => "ask",
        D::Always => "always",
        D::Never => "never",
    }
    .to_string()
}

/// Coerce a cycled `/settings` value string back into JSON for persistence: `true`/`false` → bool, an
/// integer → number, else a string (Pi's settings each have a typed `onChange`; the grid cycles the
/// display string, so we re-type it here).
pub(crate) fn parse_setting_value(value: &str) -> serde_json::Value {
    match value {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        other => match other.parse::<i64>() {
            Ok(n) => serde_json::Value::from(n),
            Err(_) => serde_json::Value::String(other.to_string()),
        },
    }
}

/// Format the saved trust-decision header line for the `/trust` selector (Pi `formatDecision`,
/// trust-selector.ts:23-31): `none`, or `trusted (path)` / `untrusted (path)`.
pub(crate) fn format_saved_trust(saved: &Option<cyrup_session_svc::TrustEntry>) -> String {
    match saved {
        None => "none".to_string(),
        Some(entry) => {
            let label = if entry.decision.is_trusted() { "trusted" } else { "untrusted" };
            format!("{label} ({})", entry.path.display())
        }
    }
}

/// The `/resume` row label for a session (Pi `session-selector.ts` row): its name (or first message),
/// trimmed to one line.
pub(crate) fn session_label(info: &cyrup_session_svc::SessionInfo) -> String {
    let raw = match &info.name {
        Some(n) if !n.trim().is_empty() => n.clone(),
        _ if !info.first_message.trim().is_empty() => info.first_message.clone(),
        _ => info.id.to_string(),
    };
    truncate_summary(&raw)
}

/// A monotonic recency key for a session's `modified` time (nanoseconds since the Unix epoch; `0`
/// before the epoch / on a clock fault). Drives the `Relevance` sort tie-break (newest first).
pub(crate) fn system_time_nanos(t: std::time::SystemTime) -> u128 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}


pub(crate) const PROJECT_UNTRUSTED_WARNING: &str = "This project is not trusted. Project .cyrup resources and packages are ignored. Use /trust to save a trust decision, then restart cyrup.";


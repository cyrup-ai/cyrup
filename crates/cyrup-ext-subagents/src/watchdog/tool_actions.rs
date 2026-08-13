//! The four `watchdog.*` tool actions — a 1:1 port of
//! `pi-subagents/src/watchdog/tool-actions.ts` (155 lines @v0.43.0).
//!
//! This is the AGENT-facing watchdog surface: the actions `subagent({ action: "watchdog.…" })`
//! dispatches to (`subagent-executor.ts:4432`), as opposed to the human-facing
//! `/subagents-watchdog` slash command in `register-main.ts`.
//!
//! Four actions ([`WATCHDOG_TOOL_ACTIONS`], `:154`): `watchdog.status`, `watchdog.check`,
//! `watchdog.configure`, `watchdog.recommend-model`.
//!
//! The one with teeth is `watchdog.configure`, and its `scope` default is a safety decision, not a
//! convenience one: it defaults to `session` (`:38-43`), so an agent that configures the watchdog
//! changes NOTHING on disk unless the caller explicitly asked for `user` or `project`. Upstream says
//! exactly that in the schema description (`schemas.ts:285`), and the session branch says it back to
//! the caller — `"No settings files were changed."` (`:135`).
//!
//! Two more rules that are easy to lose:
//!
//! * **Session scope supports `target: "main"` ONLY** (`:132`). A session override lives in the
//!   runtime, and the runtime has one main endpoint; a session-scoped `children` write would have
//!   nowhere to live and is refused rather than silently persisted.
//! * **Every failure is a tool ERROR RESULT, not an exception** (`:150-152`): the whole handler is
//!   wrapped so a bad model id comes back to the model as `Subagent watchdog action failed: …` and
//!   the turn continues.
//!
//! [CYRUP-DELTA] upstream returns an `AgentToolResult<Details>` with pi's `{ mode: "management",
//! results: [] }` details. Here the action returns [`WatchdogToolActionResult`] — text plus an error
//! flag — and this crate's `extension.rs` maps it onto a `cyrup_core::ToolResult`/`ToolError` at the
//! dispatch site, matching how every other management action in that file already reports (a pi
//! `isError: true` becomes a `ToolError`, R-02-024).

use super::model_selection::{
    WatchdogModelContext, WatchdogThinkingInput, parse_watchdog_thinking_input,
    recommend_strong_watchdog_model, resolve_watchdog_model_input,
};
use super::register_main::{WatchdogCommandContext, build_watchdog_status};
use super::runtime::MainWatchdogRuntime;
use super::settings::{
    WatchdogModelSettingsTarget, WatchdogModelSettingsWrite, WatchdogSettingsWriteScope,
    write_watchdog_model_settings,
};
use super::types::ThinkingSetting;

/// `WATCHDOG_TOOL_ACTIONS` (`tool-actions.ts:154`), in upstream order.
pub const WATCHDOG_TOOL_ACTIONS: [&str; 4] = [
    "watchdog.status",
    "watchdog.check",
    "watchdog.configure",
    "watchdog.recommend-model",
];

/// `WATCHDOG_THINKING_VALUES` (`tool-actions.ts:155`) — `inherit` plus every thinking level.
///
/// No caller, and deliberately so: upstream exports it at `:155` and nothing imports it, because
/// the `subagent` tool's `thinking` parameter is NOT declared as an enum. `extension/schemas.ts:288`
/// declares `anyOf: [{ type: "string" }, { type: "boolean", enum: [false] }]` with the level names
/// only in its `description` — so wiring this constant into the schema would DIVERGE from upstream
/// rather than complete it. It is the ported vocabulary, kept for a caller that wants to validate
/// against it.
pub const WATCHDOG_THINKING_VALUES: [&str; 8] = [
    "inherit", "off", "minimal", "low", "medium", "high", "xhigh", "max",
];

/// `WatchdogToolParams` (`tool-actions.ts:11-19`) — the subset of the `subagent` tool's parameter
/// bag these actions read.
#[derive(Debug, Clone, Default)]
pub struct WatchdogToolParams {
    /// `session` (default), `user` or `project`.
    pub scope: Option<String>,
    /// `main` (default), `children` or `child`.
    pub target: Option<String>,
    /// Required when `target == "child"`.
    pub agent: Option<String>,
    /// A model id, `recommended`, or `inherit`.
    pub model: Option<String>,
    /// A thinking level, `inherit`, or `false`.
    pub thinking: Option<String>,
}

/// The result of one action: the text, and whether it is an error result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogToolActionResult {
    /// The text content.
    pub text: String,
    /// Upstream's `isError: true`.
    pub is_error: bool,
}

impl WatchdogToolActionResult {
    /// `result(text)` (`tool-actions.ts:21-27`).
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    /// `result(text, true)`.
    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// The `ExtensionContext` -> model-registry view every model-facing helper here needs. Upstream
/// passes `ctx` itself, whose `modelRegistry`/`model` fields ARE that view.
fn model_context<'a>(ctx: &'a WatchdogCommandContext<'a>) -> WatchdogModelContext<'a> {
    WatchdogModelContext::new(ctx.registry).with_current_model(ctx.current_model.clone())
}

/// Upstream's `"session" | WatchdogSettingsWriteScope` union (`tool-actions.ts:33-38`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigureScope {
    Session,
    Write(WatchdogSettingsWriteScope),
}

/// `parseScope` (`tool-actions.ts:33-38`): absent defaults to `session`.
fn parse_scope(raw: Option<&str>) -> Result<ConfigureScope, String> {
    match raw {
        None | Some("session") => Ok(ConfigureScope::Session),
        Some("user") => Ok(ConfigureScope::Write(WatchdogSettingsWriteScope::User)),
        Some("project") => Ok(ConfigureScope::Write(WatchdogSettingsWriteScope::Project)),
        Some(_) => Err("watchdog.configure scope must be 'session', 'user', or 'project'.".to_string()),
    }
}

/// `parseTarget` (`tool-actions.ts:40-48`): absent defaults to `main`; `child` requires `agent`.
fn parse_target(params: &WatchdogToolParams) -> Result<WatchdogModelSettingsTarget, String> {
    match params.target.as_deref().unwrap_or("main") {
        "main" => Ok(WatchdogModelSettingsTarget::Main),
        "children" => Ok(WatchdogModelSettingsTarget::Children),
        "child" => {
            let agent = params.agent.as_deref().unwrap_or("").trim();
            if agent.is_empty() {
                return Err("watchdog.configure target='child' requires agent.".to_string());
            }
            Ok(WatchdogModelSettingsTarget::Child(agent.to_string()))
        }
        _ => Err("watchdog.configure target must be 'main', 'children', or 'child'.".to_string()),
    }
}

/// A configured `thinking` value with upstream's THREE states plus "not supplied":
/// `None` is `undefined`, `Some(None)` is `null` (= `inherit`, DELETE the key), and
/// `Some(Some(v))` sets it.
type ConfiguredThinking = Option<Option<ThinkingSetting>>;

/// `parseThinking` (`tool-actions.ts:50-54`): `inherit` becomes `null`, and — note — an input that
/// parses to `undefined` (only the empty string can) ALSO becomes `undefined` rather than `null`.
fn parse_thinking(raw: Option<&str>) -> Result<ConfiguredThinking, String> {
    match raw {
        None => Ok(None),
        Some("inherit") => Ok(Some(None)),
        Some(value) => Ok(
            match parse_watchdog_thinking_input(Some(value), "watchdog.configure thinking")? {
                None => None,
                Some(WatchdogThinkingInput::Off) => Some(Some(ThinkingSetting::Off)),
                Some(WatchdogThinkingInput::Level(level)) => {
                    Some(Some(ThinkingSetting::Level(level)))
                }
            },
        ),
    }
}

/// The value `resolveConfiguredValue` produces (`tool-actions.ts:56-76`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfiguredValue {
    /// `None` = leave alone, `Some(None)` = delete, `Some(Some(v))` = set.
    model: Option<Option<String>>,
    thinking: ConfiguredThinking,
    description: String,
}

/// `resolveConfiguredValue` (`tool-actions.ts:56-76`) — the four input shapes, in upstream order.
///
/// 1. no model at all: thinking alone, and it is REQUIRED (`:60`);
/// 2. `inherit`: clear both, `description` is `"inherit"` — note this sets thinking to `null` too
///    unless the caller pinned one (`thinking ?? null`, `:64`);
/// 3. `recommended`: the strong-complement recommendation, at its own `high` level;
/// 4. anything else: a fully-validated model, whose own `:suffix` level BEATS the `thinking`
///    parameter (`resolved.thinking ?? thinking`, `:73-74`).
fn resolve_configured_value(
    ctx: &WatchdogModelContext<'_>,
    params: &WatchdogToolParams,
) -> Result<ConfiguredValue, String> {
    let thinking = parse_thinking(params.thinking.as_deref())?;
    let raw_model = params.model.as_deref().map(str::trim).unwrap_or("");
    if raw_model.is_empty() {
        let Some(thinking_value) = thinking.clone() else {
            return Err("watchdog.configure requires model, thinking, or both.".to_string());
        };
        let label = match thinking_value.as_ref() {
            None => "inherit".to_string(),
            Some(ThinkingSetting::Off) => "off".to_string(),
            Some(ThinkingSetting::Level(level)) => level.clone(),
        };
        return Ok(ConfiguredValue {
            model: None,
            thinking,
            description: format!("thinking {label}"),
        });
    }
    if raw_model == "inherit" {
        return Ok(ConfiguredValue {
            model: Some(None),
            thinking: Some(thinking.flatten()),
            description: "inherit".to_string(),
        });
    }
    if raw_model == "recommended" {
        let recommendation = recommend_strong_watchdog_model(ctx)?;
        return Ok(ConfiguredValue {
            description: format!("{}:{}", recommendation.model, recommendation.thinking),
            model: Some(Some(recommendation.model)),
            thinking: Some(Some(ThinkingSetting::Level(recommendation.thinking))),
        });
    }
    let resolved = resolve_watchdog_model_input(ctx, raw_model)?;
    // `resolved.thinking ?? thinking` — the model's own suffix wins over the parameter.
    let effective: Option<ThinkingSetting> = match resolved.thinking {
        Some(level) => Some(ThinkingSetting::Level(level)),
        None => thinking.clone().flatten(),
    };
    let suffix = effective
        .as_ref()
        .map(|value| format!(":{}", value.label()))
        .unwrap_or_default();
    Ok(ConfiguredValue {
        description: format!("{}{suffix}", resolved.model),
        model: Some(Some(resolved.model)),
        thinking: Some(effective),
    })
}

/// `buildRecommendationText` (`tool-actions.ts:78-88`).
///
/// # Errors
///
/// Propagates [`recommend_strong_watchdog_model`].
pub fn build_recommendation_text(ctx: &WatchdogModelContext<'_>) -> Result<String, String> {
    let recommendation = recommend_strong_watchdog_model(ctx)?;
    Ok([
        "Subagent watchdog recommended model".to_string(),
        format!("Recommended: {}:{}", recommendation.model, recommendation.thinking),
        format!("Reason: {}", recommendation.reason),
        "Apply temporarily with subagent({ action: \"watchdog.configure\", scope: \"session\", \
         model: \"recommended\" })."
            .to_string(),
        "Persist with scope: \"project\" or scope: \"user\" only when the user asks for that scope."
            .to_string(),
    ]
    .join("\n"))
}

/// `buildCheckText` (`tool-actions.ts:90-107`).
///
/// Unlike the slash command's check (`register-main.ts:230-245`), this one prints the LSP line but
/// NOT the main-thinking line, and its unavailable-recommendation wording differs
/// (`Recommended strong watchdog unavailable: …` versus `… : unavailable (…)`). Both are reproduced
/// as-is: they are distinct surfaces with distinct text.
#[must_use]
pub fn build_check_text(
    runtime: Option<&MainWatchdogRuntime>,
    ctx: &WatchdogCommandContext<'_>,
) -> String {
    let Some(runtime) = runtime else {
        return "Subagent watchdog runtime is unavailable.".to_string();
    };
    let snapshot = runtime.get_snapshot(Some(&ctx.cwd));
    if !snapshot.config_ok {
        let mut lines = vec![
            "Subagent watchdog config check".to_string(),
            "Config errors:".to_string(),
        ];
        lines.extend(snapshot.errors.iter().map(|error| format!("- {}", error.message)));
        return lines.join("\n");
    }
    let mut lines = vec![
        "Subagent watchdog config check".to_string(),
        "Config: ok".to_string(),
    ];
    // `tool-actions.ts:99-104` builds these two lines ITSELF rather than reusing
    // `register-main.ts`'s `mainModelLine`/`lspLine` — the tool surface prints `auth ok` and a
    // two-field LSP line where the slash surface prints provenance and counts.
    match snapshot.config.main.model.as_deref() {
        Some(model) => match resolve_watchdog_model_input(&model_context(ctx), model) {
            Ok(resolved) => lines.push(format!("Main model: {} auth ok", resolved.model)),
            // Upstream lets this throw out of `buildCheckText` into
            // `handleWatchdogToolAction`'s catch (`:150`).
            Err(error) => return format!("Subagent watchdog action failed: {error}"),
        },
        None => lines.push("Main model: current session".to_string()),
    }
    lines.push(format!(
        "LSP diagnostics: {} · {}",
        if snapshot.lsp.enabled { "on" } else { "off" },
        snapshot.lsp.result.status.as_str()
    ));
    match recommend_strong_watchdog_model(&model_context(ctx)) {
        Ok(recommendation) => lines.push(format!(
            "Recommended strong watchdog: {}:{}",
            recommendation.model, recommendation.thinking
        )),
        Err(error) => lines.push(format!("Recommended strong watchdog unavailable: {error}")),
    }
    lines.join("\n")
}

/// `handleWatchdogToolAction` (`tool-actions.ts:109-152`).
///
/// Never returns an `Err`: every failure — including a thrown one from any helper — becomes an
/// error RESULT carrying `Subagent watchdog action failed: <message>`, so a mistyped model id does
/// not abort the caller's turn.
#[must_use]
pub fn handle_watchdog_tool_action(
    action: &str,
    params: &WatchdogToolParams,
    ctx: &WatchdogCommandContext<'_>,
    runtime: Option<&MainWatchdogRuntime>,
) -> WatchdogToolActionResult {
    match handle_inner(action, params, ctx, runtime) {
        Ok(result) => result,
        Err(error) => {
            WatchdogToolActionResult::error(format!("Subagent watchdog action failed: {error}"))
        }
    }
}

fn handle_inner(
    action: &str,
    params: &WatchdogToolParams,
    ctx: &WatchdogCommandContext<'_>,
    runtime: Option<&MainWatchdogRuntime>,
) -> Result<WatchdogToolActionResult, String> {
    if action == "watchdog.status" {
        let Some(runtime) = runtime else {
            return Ok(WatchdogToolActionResult::error(
                "Subagent watchdog runtime is unavailable.",
            ));
        };
        return Ok(WatchdogToolActionResult::ok(build_watchdog_status(
            &runtime.get_snapshot(Some(&ctx.cwd)),
            ctx,
        )));
    }
    if action == "watchdog.recommend-model" {
        return Ok(WatchdogToolActionResult::ok(build_recommendation_text(&model_context(ctx))?));
    }
    if action == "watchdog.check" {
        return Ok(WatchdogToolActionResult::ok(build_check_text(runtime, ctx)));
    }
    if action != "watchdog.configure" {
        return Ok(WatchdogToolActionResult::error(format!(
            "Unknown watchdog action: {action}"
        )));
    }

    let scope = parse_scope(params.scope.as_deref())?;
    let target = parse_target(params)?;
    let value = resolve_configured_value(&model_context(ctx), params)?;
    if scope == ConfigureScope::Session {
        let Some(runtime) = runtime else {
            return Ok(WatchdogToolActionResult::error(
                "Subagent watchdog runtime is unavailable.",
            ));
        };
        if target != WatchdogModelSettingsTarget::Main {
            return Ok(WatchdogToolActionResult::error(
                "Session-scoped watchdog.configure currently supports target='main' only.",
            ));
        }
        runtime.set_session_model(value.model.clone(), value.thinking.clone(), &ctx.cwd);
        return Ok(WatchdogToolActionResult::ok(
            [
                format!(
                    "Subagent watchdog session model configured: {}.",
                    value.description
                ),
                "No settings files were changed.".to_string(),
                String::new(),
                build_watchdog_status(&runtime.get_snapshot(Some(&ctx.cwd)), ctx),
            ]
            .join("\n"),
        ));
    }

    let ConfigureScope::Write(write_scope) = scope else {
        // Unreachable: `ConfigureScope` has exactly two variants and the session one returned above.
        return Err("watchdog.configure scope must be 'session', 'user', or 'project'.".to_string());
    };
    let settings_path = write_watchdog_model_settings(&WatchdogModelSettingsWrite {
        scope: write_scope,
        cwd: Some(ctx.cwd.clone()),
        target: target.clone(),
        model: value.model,
        thinking: value.thinking,
    })?;
    if let Some(runtime) = runtime {
        runtime.refresh_config(&ctx.cwd);
    }
    let target_label = match &target {
        WatchdogModelSettingsTarget::Main => "main".to_string(),
        WatchdogModelSettingsTarget::Children => "children".to_string(),
        WatchdogModelSettingsTarget::Child(agent) => format!("child {agent}"),
    };
    Ok(WatchdogToolActionResult::ok(
        [
            format!(
                "Subagent watchdog {target_label} model configured: {}.",
                value.description
            ),
            format!("Updated: {}", settings_path.display()),
        ]
        .join("\n"),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::watchdog::model_selection::{WatchdogModelInfo, WatchdogModelRegistry};
    use crate::watchdog::runtime::{MainWatchdogRuntime, MainWatchdogRuntimeOptions};
    use std::path::Path;
    use tempfile::TempDir;

    struct Registry(Vec<WatchdogModelInfo>);

    impl WatchdogModelRegistry for Registry {
        fn available(&self) -> Vec<WatchdogModelInfo> {
            self.0.clone()
        }
        fn find(&self, provider: &str, id: &str) -> Option<WatchdogModelInfo> {
            self.0.iter().find(|m| m.provider == provider && m.id == id).cloned()
        }
        fn has_configured_auth(&self, _model: &WatchdogModelInfo) -> bool {
            true
        }
    }

    fn registry() -> Registry {
        let mut opus = WatchdogModelInfo::new("anthropic", "claude-opus-4-8");
        opus.reasoning = Some(true);
        Registry(vec![opus])
    }

    fn cmd_ctx<'a>(registry: &'a Registry, cwd: &Path) -> WatchdogCommandContext<'a> {
        WatchdogCommandContext {
            cwd: cwd.to_path_buf(),
            registry,
            current_model: None,
            thinking_level: None,
        }
    }

    fn runtime(cwd: &Path) -> MainWatchdogRuntime {
        MainWatchdogRuntime::new(MainWatchdogRuntimeOptions {
            cwd: Some(cwd.to_path_buf()),
            ..Default::default()
        })
    }

    fn params(model: Option<&str>, thinking: Option<&str>) -> WatchdogToolParams {
        WatchdogToolParams {
            model: model.map(str::to_string),
            thinking: thinking.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn the_action_list_and_thinking_enum_match_upstream() {
        assert_eq!(
            WATCHDOG_TOOL_ACTIONS,
            [
                "watchdog.status",
                "watchdog.check",
                "watchdog.configure",
                "watchdog.recommend-model"
            ]
        );
        assert_eq!(WATCHDOG_THINKING_VALUES[0], "inherit");
        assert_eq!(WATCHDOG_THINKING_VALUES.len(), 8);
    }

    #[test]
    fn scope_defaults_to_session_and_rejects_anything_else() {
        assert_eq!(parse_scope(None).unwrap(), ConfigureScope::Session);
        assert_eq!(parse_scope(Some("session")).unwrap(), ConfigureScope::Session);
        assert_eq!(
            parse_scope(Some("user")).unwrap(),
            ConfigureScope::Write(WatchdogSettingsWriteScope::User)
        );
        assert_eq!(
            parse_scope(Some("global")).unwrap_err(),
            "watchdog.configure scope must be 'session', 'user', or 'project'."
        );
    }

    #[test]
    fn target_defaults_to_main_and_child_requires_an_agent() {
        assert_eq!(
            parse_target(&WatchdogToolParams::default()).unwrap(),
            WatchdogModelSettingsTarget::Main
        );
        let child = WatchdogToolParams {
            target: Some("child".into()),
            agent: Some("  ".into()),
            ..Default::default()
        };
        assert_eq!(
            parse_target(&child).unwrap_err(),
            "watchdog.configure target='child' requires agent."
        );
        let named = WatchdogToolParams {
            target: Some("child".into()),
            agent: Some(" reviewer ".into()),
            ..Default::default()
        };
        assert_eq!(
            parse_target(&named).unwrap(),
            WatchdogModelSettingsTarget::Child("reviewer".into())
        );
        let bad = WatchdogToolParams {
            target: Some("grandchild".into()),
            ..Default::default()
        };
        assert_eq!(
            parse_target(&bad).unwrap_err(),
            "watchdog.configure target must be 'main', 'children', or 'child'."
        );
    }

    #[test]
    fn configure_with_neither_model_nor_thinking_is_rejected() {
        let registry = registry();
        let ctx = WatchdogModelContext::new(&registry);
        assert_eq!(
            resolve_configured_value(&ctx, &WatchdogToolParams::default()).unwrap_err(),
            "watchdog.configure requires model, thinking, or both."
        );
    }

    #[test]
    fn a_models_own_suffix_beats_the_thinking_parameter() {
        let registry = registry();
        let ctx = WatchdogModelContext::new(&registry);
        let value =
            resolve_configured_value(&ctx, &params(Some("anthropic/claude-opus-4-8:xhigh"), Some("low")))
                .unwrap();
        assert_eq!(value.description, "anthropic/claude-opus-4-8:xhigh");
        assert_eq!(
            value.thinking,
            Some(Some(ThinkingSetting::Level("xhigh".into())))
        );
        // Without a suffix the parameter applies.
        let plain =
            resolve_configured_value(&ctx, &params(Some("anthropic/claude-opus-4-8"), Some("low")))
                .unwrap();
        assert_eq!(plain.description, "anthropic/claude-opus-4-8:low");
    }

    #[test]
    fn inherit_clears_both_the_model_and_the_thinking() {
        let registry = registry();
        let ctx = WatchdogModelContext::new(&registry);
        let value = resolve_configured_value(&ctx, &params(Some("inherit"), None)).unwrap();
        assert_eq!(value.description, "inherit");
        assert_eq!(value.model, Some(None), "Some(None) is upstream's null = delete");
        assert_eq!(value.thinking, Some(None));
    }

    #[test]
    fn thinking_alone_renders_its_own_description() {
        let registry = registry();
        let ctx = WatchdogModelContext::new(&registry);
        assert_eq!(
            resolve_configured_value(&ctx, &params(None, Some("high"))).unwrap().description,
            "thinking high"
        );
        assert_eq!(
            resolve_configured_value(&ctx, &params(None, Some("false"))).unwrap().description,
            "thinking off"
        );
        assert_eq!(
            resolve_configured_value(&ctx, &params(None, Some("inherit"))).unwrap().description,
            "thinking inherit"
        );
    }

    #[test]
    fn a_session_scoped_configure_writes_no_file_and_says_so() {
        let tmp = TempDir::new().unwrap();
        let registry = registry();
        let ctx = cmd_ctx(&registry, tmp.path());
        let runtime = runtime(tmp.path());
        let result = handle_watchdog_tool_action(
            "watchdog.configure",
            &params(Some("anthropic/claude-opus-4-8"), Some("high")),
            &ctx,
            Some(&runtime),
        );
        assert!(!result.is_error, "{}", result.text);
        assert!(result.text.starts_with(
            "Subagent watchdog session model configured: anthropic/claude-opus-4-8:high."
        ));
        assert!(result.text.contains("No settings files were changed."));
        assert!(result.text.contains("Subagent watchdog"));
        // The status it appends reflects the override that was just installed.
        assert!(result.text.contains("(session override)"));
    }

    #[test]
    fn a_session_scoped_configure_refuses_a_non_main_target() {
        let tmp = TempDir::new().unwrap();
        let registry = registry();
        let ctx = cmd_ctx(&registry, tmp.path());
        let runtime = runtime(tmp.path());
        let result = handle_watchdog_tool_action(
            "watchdog.configure",
            &WatchdogToolParams {
                target: Some("children".into()),
                model: Some("anthropic/claude-opus-4-8".into()),
                ..Default::default()
            },
            &ctx,
            Some(&runtime),
        );
        assert!(result.is_error);
        assert_eq!(
            result.text,
            "Session-scoped watchdog.configure currently supports target='main' only."
        );
    }

    #[test]
    fn a_project_scoped_configure_writes_the_settings_file() {
        let tmp = TempDir::new().unwrap();
        let registry = registry();
        let ctx = cmd_ctx(&registry, tmp.path());
        let runtime = runtime(tmp.path());
        let result = handle_watchdog_tool_action(
            "watchdog.configure",
            &WatchdogToolParams {
                scope: Some("project".into()),
                target: Some("children".into()),
                model: Some("anthropic/claude-opus-4-8".into()),
                ..Default::default()
            },
            &ctx,
            Some(&runtime),
        );
        assert!(!result.is_error, "{}", result.text);
        assert!(result.text.starts_with(
            "Subagent watchdog children model configured: anthropic/claude-opus-4-8."
        ));
        let path_line = result.text.lines().nth(1).unwrap();
        let path = path_line.trim_start_matches("Updated: ");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            written["subagents"]["watchdog"]["children"]["model"],
            serde_json::json!("anthropic/claude-opus-4-8")
        );
    }

    #[test]
    fn an_unknown_action_is_an_error_result_not_a_panic() {
        let tmp = TempDir::new().unwrap();
        let registry = registry();
        let ctx = cmd_ctx(&registry, tmp.path());
        let result = handle_watchdog_tool_action(
            "watchdog.nope",
            &WatchdogToolParams::default(),
            &ctx,
            None,
        );
        assert!(result.is_error);
        assert_eq!(result.text, "Unknown watchdog action: watchdog.nope");
    }

    #[test]
    fn a_bad_model_id_comes_back_as_an_action_failure() {
        let tmp = TempDir::new().unwrap();
        let registry = registry();
        let ctx = cmd_ctx(&registry, tmp.path());
        let runtime = runtime(tmp.path());
        let result = handle_watchdog_tool_action(
            "watchdog.configure",
            &params(Some("anthropic/does-not-exist"), None),
            &ctx,
            Some(&runtime),
        );
        assert!(result.is_error);
        assert!(result.text.starts_with("Subagent watchdog action failed: Watchdog model"));
    }

    #[test]
    fn status_and_check_report_an_absent_runtime_rather_than_failing() {
        let tmp = TempDir::new().unwrap();
        let registry = registry();
        let ctx = cmd_ctx(&registry, tmp.path());
        let status = handle_watchdog_tool_action(
            "watchdog.status",
            &WatchdogToolParams::default(),
            &ctx,
            None,
        );
        assert!(status.is_error);
        assert_eq!(status.text, "Subagent watchdog runtime is unavailable.");
        let check = handle_watchdog_tool_action(
            "watchdog.check",
            &WatchdogToolParams::default(),
            &ctx,
            None,
        );
        assert!(!check.is_error, "check reports the absence as content, not an error");
        assert_eq!(check.text, "Subagent watchdog runtime is unavailable.");
    }

    #[test]
    fn check_prints_the_config_verdict_the_model_line_and_the_lsp_line() {
        let tmp = TempDir::new().unwrap();
        let registry = registry();
        let ctx = cmd_ctx(&registry, tmp.path());
        let runtime = runtime(tmp.path());
        let text = build_check_text(Some(&runtime), &ctx);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Subagent watchdog config check");
        assert_eq!(lines[1], "Config: ok");
        assert_eq!(lines[2], "Main model: current session");
        assert!(lines[3].starts_with("LSP diagnostics: "));
        // This fixture's registry authenticates Opus 4.8, so the recommendation SUCCEEDS; the
        // `unavailable:` wording is the other branch, covered by
        // `recommend_model_reports_the_unavailable_case_as_an_action_failure`.
        assert_eq!(
            lines[4],
            "Recommended strong watchdog: anthropic/claude-opus-4-8:high"
        );
    }

    #[test]
    fn recommend_model_reports_the_unavailable_case_as_an_action_failure() {
        let tmp = TempDir::new().unwrap();
        let registry = Registry(Vec::new());
        let ctx = cmd_ctx(&registry, tmp.path());
        let result = handle_watchdog_tool_action(
            "watchdog.recommend-model",
            &WatchdogToolParams::default(),
            &ctx,
            None,
        );
        assert!(result.is_error);
        assert!(result.text.starts_with(
            "Subagent watchdog action failed: No authenticated strong complementary watchdog model"
        ));
    }
}

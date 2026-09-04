//! The ORCHESTRATOR-side watchdog registration — a 1:1 port of
//! `pi-subagents/src/watchdog/register-main.ts` (440 lines @v0.43.0).
//!
//! `registerMainWatchdog(pi)` (`:377-440`) does four things, and this module ports all four:
//!
//! 1. **Builds the runtime** with the main role's four wirings (`:382-390`): the real model review,
//!    `reviewChangesOnly: true`, a `displayWarning` that sends the custom warning message (as a
//!    STEER when the runtime marks it a mid-run correction), and a `sendUserMessage` for
//!    auto-follow.
//! 2. **Registers the message renderer** for `subagent_watchdog_warning` (`:392-401`).
//! 3. **Registers the `/subagents-watchdog` command** (`:403-409`) — status, on/off, session
//!    on/off, model/thinking writes, `check`, `recommend-model`, and the two `test` forms.
//! 4. **Subscribes the nine lifecycle events** (`:411-437`) the runtime's state machine needs.
//!
//! ## Where cyrup puts each of those
//!
//! cyrup's native-extension seam splits registration from dispatch, so the four land in three
//! places on [`crate::extension::SubagentsExtension`]: the runtime is built in its constructor
//! (there is no `pi` object to hang it off), `init` registers the renderer/command/subscriptions,
//! and `on_event`/`execute_command` dispatch. [`register_main_watchdog`] is the constructor step —
//! it is what builds the runtime with the main role's wirings, and it is called from
//! `SubagentsExtension::with_mode_and_channels`.
//!
//! ## Delivery, and the one mechanism difference
//!
//! `pi.sendMessage(msg, {deliverAs:"steer"})` becomes
//! [`cyrup_ext::host::HostServices::inject_message`] with `trigger_turn` set: a steer re-enters the
//! live turn loop, which is what upstream's `deliverAs: "steer"` does, and what this crate's own
//! steering inbox already uses (`prompt_runtime.rs:338`). `pi.sendUserMessage(message)` is the same
//! call with no custom type, which routes to `send_user_message` and always triggers a turn.
//!
//! That seam carries `content` but no `details` object, so the renderer cannot read
//! `message.details` the way upstream's does (`:393`). It is not lost: `content` IS the
//! `<subagent_watchdog …>` block [`super::warning_format`] wrote, every field of the details record
//! is in it, and [`parse_watchdog_warning_content`] reads it back — so the transcript draws the
//! same card upstream draws. The `details` path is still preferred when a payload carries one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::model_selection::{
    THINKING_LEVELS, WatchdogModelContext, WatchdogModelInfo, WatchdogModelRegistry,
    WatchdogThinkingInput, parse_watchdog_thinking_input, recommend_strong_watchdog_model,
    resolve_watchdog_model_input,
};
use super::render::render_watchdog_warning;
use super::runtime::{
    MainWatchdogRuntime, MainWatchdogRuntimeOptions, WatchdogDelivery, WatchdogReview,
    WatchdogRuntimeSnapshot,
};
use super::settings::{
    WatchdogModelSettingsTarget, WatchdogModelSettingsWrite, WatchdogSettingsWriteScope,
    get_watchdog_user_settings_path, write_user_watchdog_enabled, write_watchdog_model_settings,
};
use super::types::{
    SUBAGENT_WATCHDOG_WARNING_TYPE, ThinkingSetting, WatchdogCategory, WatchdogConfidence,
    WatchdogRuntimeStatus, WatchdogSettingsSource, WatchdogSeverity, WatchdogWarning,
    WatchdogWarningDetails, WatchdogWarningSource, WatchdogWarningState,
};
use super::warning_format::{
    WatchdogWarningDetailsPatch, create_watchdog_warning_message_from_details,
    normalize_watchdog_warning_details,
};

/// The slash command upstream registers (`register-main.ts:403-404`).
pub const WATCHDOG_COMMAND_NAME: &str = "subagents-watchdog";
/// Its description (`:404`).
pub const WATCHDOG_COMMAND_DESCRIPTION: &str = "Show or toggle the default-off subagent watchdog";

/// The capability slice `registerMainWatchdog` needs from `pi`: message injection (for the warning
/// message and the auto-follow prompt) resolved LATE, because a native extension is constructed
/// before its capability backend is bound.
pub type WatchdogServicesFn =
    Arc<dyn Fn() -> Option<Arc<dyn cyrup_ext::host::HostServices>> + Send + Sync>;

/// The config layout the watchdog's model registry reads `auth.json` from — the process's own,
/// resolved exactly as the binary resolves it (`cyrup_config::ConfigDirs::resolve`).
///
/// `None` when resolution genuinely fails (no home directory, an unreadable cwd). `ConfigDirs` has
/// no infallible constructor, and inventing one here would put a synthetic auth path in front of
/// the recommendation; the callers instead treat `None` as "no authenticated models", which is the
/// same answer the registry would give for an empty `auth.json` and keeps every other line of the
/// status report printing.
///
/// Passing `None` to [`super::model_selection::BuiltinWatchdogModelRegistry::new`] is NOT the same
/// thing and is never correct in production: it makes `hasConfiguredAuth` answer `false` for every
/// model in the catalog, which turns `resolveConfiguredModel`'s authentication check
/// (`review.ts:107-109`) into an unconditional failure.
#[must_use]
pub fn watchdog_config_dirs() -> Option<cyrup_config::ConfigDirs> {
    let env = cyrup_config::EnvVars::from_process();
    cyrup_config::ConfigDirs::resolve(&cyrup_config::CliConfigOverrides::default(), &env).ok()
}

/// `ctx.model` as `model-selection.ts` reads it: a `provider/id` string split back into the pair.
#[must_use]
pub fn watchdog_model_info(model: &str) -> Option<WatchdogModelInfo> {
    let (provider, id) = model.split_once('/')?;
    if provider.is_empty() || id.is_empty() {
        return None;
    }
    Some(WatchdogModelInfo::new(provider, id))
}

/// The live-session slice a review reads (`review.ts:250-253`): `ctx.model` — enriched from the
/// catalog when the registry knows it, so the review sees the model's `api`/`reasoning`/thinking
/// map rather than a bare `provider/id` pair — and `pi.getThinkingLevel()`.
///
/// `None` (no bound capability backend) is upstream's absent `ExtensionContext`.
fn session_context(
    services: &WatchdogServicesFn,
    registry: &Arc<dyn WatchdogModelRegistry>,
) -> Option<super::review::WatchdogSessionContext> {
    let services = services()?;
    let model = services.current_model().as_deref().and_then(|model| {
        let info = watchdog_model_info(model)?;
        Some(registry.find(&info.provider, &info.id).unwrap_or(info))
    });
    Some(super::review::WatchdogSessionContext {
        model,
        thinking_level: services.thinking_level(),
    })
}

/// `RegisterMainWatchdogOptions` (`register-main.ts:18-21`).
#[derive(Clone, Default)]
pub struct RegisterMainWatchdogOptions {
    /// Use an already-built runtime instead of constructing one (`:382`).
    pub runtime: Option<Arc<MainWatchdogRuntime>>,
    /// Override the review seam (`:383`).
    pub review: Option<Arc<dyn WatchdogReview>>,
}

/// `registerMainWatchdog(pi, options)` (`register-main.ts:377-440`) — build the orchestrator's
/// runtime with its four wirings.
///
/// The renderer/command/subscription registrations of `:392-437` are the host's, not the runtime's,
/// and live on [`crate::extension::SubagentsExtension`]'s `init`/`on_event`/`execute_command`; see
/// this module's doc for why.
#[must_use]
pub fn register_main_watchdog(
    services: WatchdogServicesFn,
    cwd: &Path,
    options: RegisterMainWatchdogOptions,
) -> Arc<MainWatchdogRuntime> {
    if let Some(runtime) = options.runtime {
        return runtime;
    }
    let injected_review = options.review.is_some();
    // `review: options.review ?? createMainWatchdogReview(…)` (`register-main.ts:383`): the caller's
    // seam when it supplied one, else the REAL review — which resolves and validates the review
    // model on every boundary (so a misconfigured `subagents.watchdog.main.model` fails loudly
    // rather than silently reviewing nothing) and runs its turn through whichever
    // [`super::review::WatchdogReviewAgent`] is bound.
    let review_services = Arc::clone(&services);
    let review: Arc<dyn WatchdogReview> = options.review.unwrap_or_else(move || {
        // The registry MUST see the process's real `auth.json` — see [`watchdog_config_dirs`] for
        // why `BuiltinWatchdogModelRegistry::new(None)` cannot resolve any configured model.
        let registry: Arc<dyn WatchdogModelRegistry> =
            Arc::new(super::model_selection::BuiltinWatchdogModelRegistry::new(
                watchdog_config_dirs().as_ref(),
            ));
        let session_registry = Arc::clone(&registry);
        Arc::new(
            super::review::MainWatchdogReview::new(
                registry,
                Arc::new(super::review::AmbientReviewAuth),
                Arc::new(super::review::NoTurnReviewAgent),
                cwd.to_path_buf(),
            )
            // `createMainWatchdogReview(() => currentContext, { getThinkingLevel: () =>
            // pi.getThinkingLevel() })` (`register-main.ts:383`): the live session model and
            // reasoning level, read at review time through the same late-bound capability slot the
            // warning sinks use.
            .with_session_context(Arc::new(move || {
                session_context(&review_services, &session_registry)
            })),
        )
    });
    let display_services = Arc::clone(&services);
    let user_message_services = Arc::clone(&services);
    Arc::new(MainWatchdogRuntime::new(MainWatchdogRuntimeOptions {
        cwd: Some(cwd.to_path_buf()),
        resolve_config: None,
        review: Some(review),
        // `:384` — upstream says "real model review" when it wired `createMainWatchdogReview`, and
        // "injected seam" when the caller supplied one. A runtime with NO review seam reports the
        // constructor's own "not wired", which is what `/subagents-watchdog status` then prints.
        review_description: Some(
            if injected_review {
                "injected seam"
            } else {
                "real model review"
            }
            .to_string(),
        ),
        display_warning: Some(Arc::new(move |details, delivery| {
            let Some(services) = display_services() else {
                return;
            };
            let message = create_watchdog_warning_message_from_details(details, true);
            // The structured record this message was rendered FROM, carried as pi's `details` so a
            // renderer reads structure instead of re-parsing `content`.
            let details_json = serde_json::to_value(&message.details).ok();
            // `:386-388` — `{deliverAs:"steer"}` re-enters the live turn loop.
            let _ = services.inject_message(
                &message.content,
                Some(SUBAGENT_WATCHDOG_WARNING_TYPE),
                message.display,
                details_json.as_ref(),
                delivery == Some(WatchdogDelivery::Steer),
            );
        })),
        send_user_message: Some(Arc::new(move |message: &str| {
            let services = user_message_services().ok_or_else(|| "no live session".to_string())?;
            // `:389` — `pi.sendUserMessage(message)`: a plain user message, which always triggers.
            // `sendUserMessage` takes a bare string in pi; there is no `details` to carry.
            services.inject_message(message, None, true, None, true)
        })),
        review_changes_only: true,
        // The two real collectors, replacing the runtime's `UnavailableLspDiagnostics` /
        // `NoRepoChangeSignatures` placeholders. Without the second one a `review_changes_only`
        // runtime never sees a signature and re-reviews an unchanged tree at every boundary; without
        // the first the review input carries no `LSP diagnostics:` block and the boundary raises no
        // LSP-sourced warning at all (`runtime.ts:178-179` binds exactly these two).
        lsp_diagnostics: Some(Arc::new(
            super::lsp_diagnostics::TypeScriptLspDiagnostics::new(),
        )),
        repo_change_signature: Some(Arc::new(super::change_signature::GitRepoChangeSource)),
    }))
}

// =================================================================================================
// Status rendering (`register-main.ts:31-153`)
// =================================================================================================

/// The `ExtensionContext` slice the status/model commands read (`ctx.model`, `ctx.thinkingLevel`,
/// `ctx.modelRegistry`, `ctx.cwd`).
pub struct WatchdogCommandContext<'a> {
    /// `ctx.cwd`.
    pub cwd: PathBuf,
    /// `ctx.modelRegistry`.
    pub registry: &'a dyn WatchdogModelRegistry,
    /// `ctx.model`.
    pub current_model: Option<WatchdogModelInfo>,
    /// `ctx.thinkingLevel`.
    pub thinking_level: Option<String>,
}

impl WatchdogCommandContext<'_> {
    fn model_context(&self) -> WatchdogModelContext<'_> {
        WatchdogModelContext::new(self.registry).with_current_model(self.current_model.clone())
    }
}

/// `boolLabel` (`register-main.ts:31-33`).
fn bool_label(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

/// `statusLabel` (`register-main.ts:35-37`).
fn status_label(status: WatchdogRuntimeStatus) -> String {
    status.as_str().replace('-', " ")
}

/// `sourceLine` (`register-main.ts:39-42`).
fn source_line(source: &WatchdogSettingsSource) -> String {
    let location = source
        .path
        .as_ref()
        .map_or_else(String::new, |path| format!(" {path}"));
    format!(
        "- {}{location}: {}",
        source.scope.as_str(),
        if source.exists { "found" } else { "not found" }
    )
}

/// `currentSessionModelLine` (`register-main.ts:44-48`).
fn current_session_model_line(ctx: &WatchdogCommandContext<'_>) -> String {
    match &ctx.current_model {
        Some(model) => format!("current session ({}/{})", model.provider, model.id),
        None => "current session (not configured)".to_string(),
    }
}

/// `splitKnownThinkingSuffix(model).baseModel` (`shared/model-info.ts:43-51`), reusing this crate's
/// existing port.
fn base_model(model: &str) -> String {
    crate::exec::split_known_thinking_suffix(model)
        .0
        .to_string()
}

/// `resolveEffectiveThinking(model, configThinking)` (`shared/model-info.ts:34-40`): a `:level`
/// suffix on the model wins; otherwise the configured level, but only when it is a recognized one.
fn resolve_effective_thinking(
    model: &str,
    config_thinking: Option<&ThinkingSetting>,
) -> Option<String> {
    if model.is_empty() {
        return None;
    }
    let (_, suffix) = crate::exec::split_known_thinking_suffix(model);
    if !suffix.is_empty() {
        return suffix.get(1..).map(str::to_string);
    }
    match config_thinking {
        Some(ThinkingSetting::Level(level)) if THINKING_LEVELS.contains(&level.as_str()) => {
            Some(level.clone())
        }
        _ => None,
    }
}

/// `mainThinkingLine` (`register-main.ts:50-62`).
fn main_thinking_line(
    snapshot: &WatchdogRuntimeSnapshot,
    ctx: &WatchdogCommandContext<'_>,
) -> String {
    let configured_model = snapshot.config.main.model.as_deref();
    let configured_thinking = snapshot.config.main.thinking.as_ref();
    if let Some(model) = configured_model {
        return match resolve_effective_thinking(model, configured_thinking) {
            Some(effective) => effective,
            None => "off (default for explicit watchdog model)".to_string(),
        };
    }
    match configured_thinking {
        Some(ThinkingSetting::Off) => "off".to_string(),
        Some(ThinkingSetting::Level(level)) => level.clone(),
        None => match &ctx.thinking_level {
            Some(level) => format!("current session ({level})"),
            None => "current session".to_string(),
        },
    }
}

/// `mainModelLine` (`register-main.ts:64-70`).
fn main_model_line(snapshot: &WatchdogRuntimeSnapshot, ctx: &WatchdogCommandContext<'_>) -> String {
    match &snapshot.config.main.model {
        Some(model) => {
            let source = if snapshot
                .session_model_override
                .as_ref()
                .is_some_and(|o| o.model.is_some())
            {
                "session override"
            } else {
                "configured"
            };
            format!("Main model: {} ({source})", base_model(model))
        }
        None => format!("Main model: {}", current_session_model_line(ctx)),
    }
}

/// `childrenLine` (`register-main.ts:72-87`).
fn children_line(snapshot: &WatchdogRuntimeSnapshot) -> String {
    let children = &snapshot.config.children;
    let model = children
        .model
        .as_ref()
        .map_or_else(|| "current child session".to_string(), |m| base_model(m));
    let thinking = match &children.thinking {
        None => "current child session".to_string(),
        Some(ThinkingSetting::Off) => "off".to_string(),
        Some(ThinkingSetting::Level(level)) => level.clone(),
    };
    let override_text = if children.overrides.is_empty() {
        String::new()
    } else {
        let rendered: Vec<String> = children
            .overrides
            .iter()
            .map(|(agent, entry)| {
                let mut bits = vec![agent.clone()];
                if let Some(enabled) = entry.enabled {
                    bits.push(bool_label(enabled).to_string());
                }
                if let Some(model) = &entry.model {
                    bits.push(base_model(model));
                }
                match &entry.thinking {
                    Some(ThinkingSetting::Off) => bits.push("thinking off".to_string()),
                    Some(ThinkingSetting::Level(level)) => bits.push(format!("thinking {level}")),
                    None => {}
                }
                bits.join(" ")
            })
            .collect();
        format!(" · overrides {}", rendered.join("; "))
    };
    format!(
        "Children: {} · model {model} · thinking {thinking}{override_text}",
        bool_label(snapshot.config.enabled && children.enabled)
    )
}

/// `recommendationLine` (`register-main.ts:89-96`).
fn recommendation_line(ctx: &WatchdogCommandContext<'_>) -> String {
    match recommend_strong_watchdog_model(&ctx.model_context()) {
        Ok(recommendation) => format!(
            "Recommended strong watchdog: {}:{} ({}, complementary reviewer)",
            recommendation.model, recommendation.thinking, recommendation.label
        ),
        Err(message) => format!("Recommended strong watchdog: unavailable ({message})"),
    }
}

/// `lspLine` (`register-main.ts:98-106`).
fn lsp_line(snapshot: &WatchdogRuntimeSnapshot) -> String {
    let lsp = &snapshot.lsp;
    let provider = lsp
        .result
        .provider
        .as_ref()
        .map_or_else(String::new, |p| format!(" · {p}"));
    let counts = if lsp.diagnostic_count > 0 || lsp.fresh_diagnostic_count > 0 {
        format!(
            " · {} new/{} total",
            lsp.fresh_diagnostic_count, lsp.diagnostic_count
        )
    } else {
        String::new()
    };
    let message = lsp
        .result
        .message
        .as_ref()
        .map_or_else(String::new, |m| format!(" · {m}"));
    format!(
        "LSP diagnostics: {} · {}{provider}{counts}{message}",
        if lsp.enabled { "on" } else { "off" },
        lsp.result.status.as_str()
    )
}

/// `buildWatchdogStatus(snapshot, ctx)` (`register-main.ts:108-153`).
#[must_use]
pub fn build_watchdog_status(
    snapshot: &WatchdogRuntimeSnapshot,
    ctx: &WatchdogCommandContext<'_>,
) -> String {
    let mut lines = vec![
        "Subagent watchdog".to_string(),
        format!(
            "Main: {}{}",
            bool_label(snapshot.enabled),
            if !snapshot.config.enabled && snapshot.session_override.is_none() {
                " (default off)"
            } else {
                ""
            }
        ),
        format!(
            "Runtime: {}{}",
            status_label(snapshot.status),
            if snapshot.buffered_deltas > 0 {
                format!(" · buffered deltas {}", snapshot.buffered_deltas)
            } else {
                String::new()
            }
        ),
        format!(
            "Review trigger: {}",
            match snapshot.review_trigger {
                super::runtime::WatchdogReviewTrigger::RepoEdits => "repo edits only",
                super::runtime::WatchdogReviewTrigger::TurnDelta => "every non-empty turn delta",
            }
        ),
        format!(
            "Scope context: {}",
            if snapshot.config.scope.enabled {
                "on"
            } else {
                "off"
            }
        ),
        format!(
            "Cadence: {}",
            match snapshot.config.cadence.every_n_tools {
                None => "boundary only".to_string(),
                Some(n) => format!("every {n} tools + boundary"),
            }
        ),
        lsp_line(snapshot),
        format!(
            "Session override: {}",
            match snapshot.session_override {
                None => "none".to_string(),
                Some(value) => bool_label(value).to_string(),
            }
        ),
        main_model_line(snapshot, ctx),
        format!("Main thinking: {}", main_thinking_line(snapshot, ctx)),
        children_line(snapshot),
        recommendation_line(ctx),
        format!(
            "Agent-end timeout: {}ms",
            snapshot.config.agent_end_timeout_ms
        ),
        format!(
            "Auto-follow: {} · attempts {}{}{}{}",
            if snapshot.enabled && snapshot.config.auto_follow.blockers {
                "on for blockers"
            } else {
                "off"
            },
            snapshot.auto_follow_attempts,
            match snapshot.config.auto_follow.max_attempts {
                None => String::new(),
                Some(max) => format!("/{max}"),
            },
            if snapshot.auto_follow_queued {
                " · queued"
            } else {
                ""
            },
            if snapshot.auto_follow_stalemate {
                " · stalemate"
            } else {
                ""
            }
        ),
        format!("Review model call: {}", snapshot.review_description),
    ];
    if snapshot.failed_reviews > 0 {
        lines.push(format!("Failed reviews: {}", snapshot.failed_reviews));
    }
    if snapshot.stale_reviews > 0 {
        lines.push(format!("Stale reviews: {}", snapshot.stale_reviews));
    }
    if let Some(paths) = snapshot.changed_paths.as_ref().filter(|p| !p.is_empty()) {
        let head: Vec<String> = paths.iter().take(8).cloned().collect();
        lines.push(format!(
            "Changed paths: {}{}",
            head.join(", "),
            if paths.len() > 8 {
                format!(", +{} more", paths.len() - 8)
            } else {
                String::new()
            }
        ));
    }
    if let Some(warning) = &snapshot.last_warning {
        lines.push(format!(
            "Last warning: {} · {} · {}",
            warning.severity.as_str(),
            warning
                .state
                .map_or("candidate", WatchdogWarningState::as_str),
            warning.summary
        ));
    }
    if let Some(error) = &snapshot.last_error {
        lines.push(format!("Last error: {error}"));
    }
    if snapshot.config_ok {
        lines.push(String::new());
        lines.push("Config: ok".to_string());
    } else {
        lines.push(String::new());
        lines.push("Config errors:".to_string());
        lines.extend(
            snapshot
                .errors
                .iter()
                .map(|error| format!("- {}", error.message)),
        );
        lines.push("Watchdog is disabled until the config is fixed.".to_string());
    }
    lines.push("Sources:".to_string());
    lines.extend(snapshot.sources.iter().map(source_line));
    lines.extend([
        String::new(),
        "Model commands:".to_string(),
        "- /subagents-watchdog recommend-model".to_string(),
        "- /subagents-watchdog model recommended".to_string(),
        "- /subagents-watchdog model <provider/model[:thinking]>".to_string(),
        "- /subagents-watchdog model inherit".to_string(),
        "- /subagents-watchdog session model recommended".to_string(),
        "Agent action: subagent({ action: \"watchdog.configure\", model: \"recommended\", scope: \"session\" })"
            .to_string(),
    ]);
    lines.join("\n")
}

// =================================================================================================
// Command handling (`register-main.ts:155-375`)
// =================================================================================================

/// `parseTestCommand(input)` (`register-main.ts:155-159`) — `test concern|blocker <text>`.
fn parse_test_command(input: &str) -> Option<(WatchdogSeverity, String)> {
    let rest = input.strip_prefix("test")?;
    let rest = rest.strip_prefix(|c: char| c.is_whitespace())?;
    let rest = rest.trim_start();
    for (token, severity) in [
        ("concern", WatchdogSeverity::Concern),
        ("blocker", WatchdogSeverity::Blocker),
    ] {
        if let Some(tail) = rest.strip_prefix(token)
            && let Some(text) = tail.strip_prefix(|c: char| c.is_whitespace())
        {
            return Some((severity, text.trim().to_string()));
        }
    }
    None
}

/// `formatThinking(value)` (`register-main.ts:161-164`).
fn format_thinking(value: Option<&ThinkingSetting>) -> String {
    match value {
        None => "inherit".to_string(),
        Some(ThinkingSetting::Off) => "off".to_string(),
        Some(ThinkingSetting::Level(level)) => level.clone(),
    }
}

/// `parseThinkingCommand(raw)` (`register-main.ts:166-170`) — `inherit` deletes the setting
/// (upstream's `null`); anything else is validated.
fn parse_thinking_command(raw: &str) -> Result<Option<ThinkingSetting>, String> {
    let value = raw.trim();
    if value == "inherit" {
        return Ok(None);
    }
    let parsed = parse_watchdog_thinking_input(Some(value), "/subagents-watchdog thinking")?;
    Ok(match parsed {
        None => None,
        Some(WatchdogThinkingInput::Off) => Some(ThinkingSetting::Off),
        Some(WatchdogThinkingInput::Level(level)) => Some(ThinkingSetting::Level(level)),
    })
}

/// The three-way answer of `resolveModelCommandValue` (`register-main.ts:172-190`).
struct ModelCommandValue {
    model: Option<String>,
    thinking: Option<ThinkingSetting>,
    description: String,
}

/// `resolveModelCommandValue(ctx, raw)` (`register-main.ts:172-190`).
fn resolve_model_command_value(
    ctx: &WatchdogCommandContext<'_>,
    raw: &str,
) -> Result<ModelCommandValue, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("Expected a model, 'recommended', or 'inherit'.".to_string());
    }
    if value == "inherit" {
        return Ok(ModelCommandValue {
            model: None,
            thinking: None,
            description: "current session model and thinking".to_string(),
        });
    }
    if value == "recommended" {
        let recommendation = recommend_strong_watchdog_model(&ctx.model_context())?;
        return Ok(ModelCommandValue {
            description: format!(
                "{}:{} ({})",
                recommendation.model, recommendation.thinking, recommendation.label
            ),
            model: Some(recommendation.model),
            thinking: Some(ThinkingSetting::Level(recommendation.thinking)),
        });
    }
    let resolved = resolve_watchdog_model_input(&ctx.model_context(), value)?;
    Ok(ModelCommandValue {
        description: format!(
            "{}{}",
            resolved.model,
            resolved
                .thinking
                .as_ref()
                .map_or_else(String::new, |t| format!(":{t}"))
        ),
        model: Some(resolved.model),
        thinking: resolved.thinking.map(ThinkingSetting::Level),
    })
}

/// `buildRecommendationText(ctx)` (`register-main.ts:192-206`).
fn build_recommendation_text(ctx: &WatchdogCommandContext<'_>) -> Result<String, String> {
    let recommendation = recommend_strong_watchdog_model(&ctx.model_context())?;
    Ok([
        "Subagent watchdog recommended model".to_string(),
        format!("Current session: {}", current_session_model_line(ctx)),
        format!(
            "Recommended: {}:{}",
            recommendation.model, recommendation.thinking
        ),
        format!("Reason: {}", recommendation.reason),
        String::new(),
        "Apply for this session:".to_string(),
        "/subagents-watchdog session model recommended".to_string(),
        String::new(),
        "Save as your user default:".to_string(),
        "/subagents-watchdog model recommended".to_string(),
    ]
    .join("\n"))
}

/// `buildCheckText(runtime, ctx)` (`register-main.ts:208-229`).
fn build_check_text(
    runtime: &MainWatchdogRuntime,
    ctx: &WatchdogCommandContext<'_>,
) -> Result<String, String> {
    let snapshot = runtime.get_snapshot(Some(&ctx.cwd));
    if !snapshot.config_ok {
        let mut lines = vec![
            "Subagent watchdog config check".to_string(),
            String::new(),
            "Config errors:".to_string(),
        ];
        lines.extend(
            snapshot
                .errors
                .iter()
                .map(|error| format!("- {}", error.message)),
        );
        return Ok(lines.join("\n"));
    }
    let mut lines = vec![
        "Subagent watchdog config check".to_string(),
        String::new(),
        "Config: ok".to_string(),
    ];
    match &snapshot.config.main.model {
        Some(model) => {
            let resolved = resolve_watchdog_model_input(&ctx.model_context(), model)?;
            lines.push(format!("Main model: {} auth ok", resolved.model));
        }
        None => lines.push(format!("Main model: {}", current_session_model_line(ctx))),
    }
    lines.push(format!(
        "Main thinking: {}",
        main_thinking_line(&snapshot, ctx)
    ));
    lines.push(lsp_line(&snapshot));
    match recommend_strong_watchdog_model(&ctx.model_context()) {
        Ok(recommendation) => lines.push(format!(
            "Recommended strong watchdog: {}:{}",
            recommendation.model, recommendation.thinking
        )),
        Err(message) => lines.push(format!(
            "Recommended strong watchdog: unavailable ({message})"
        )),
    }
    Ok(lines.join("\n"))
}

/// `createTestWarning(severity, text)` (`register-main.ts:231-244`).
fn create_test_warning(severity: WatchdogSeverity, text: &str) -> WatchdogWarning {
    WatchdogWarning {
        severity,
        summary: text.to_string(),
        evidence: format!(
            "Manual /subagents-watchdog test {} message from the main session.",
            severity.as_str()
        ),
        recommended_action: if severity == WatchdogSeverity::Blocker {
            "Verify the renderer, transcript delivery, and auto-follow policy.".to_string()
        } else {
            "Verify the renderer and transcript delivery; decide manually whether any action is needed."
                .to_string()
        },
        category: Some(WatchdogCategory::Other),
        confidence: Some(WatchdogConfidence::High),
        source: Some(WatchdogWarningSource::Main),
        agent: None,
        run_id: None,
        stale: None,
        auto_follow_attempt: None,
        state: Some(WatchdogWarningState::Displayed),
    }
}

/// What `handleWatchdogCommand` produced. Upstream has two channels — `sendSlashText` (a transcript
/// message) and `ctx.ui.notify(…, "error")` — and cyrup's command seam has the same two: the
/// returned `String`, and [`cyrup_ext::host::HostServices::notify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogCommandOutcome {
    /// `sendSlashText(pi, text)` — surfaced as the command's text output.
    Text(String),
    /// `ctx.ui.notify(text, "error")` — the two usage errors (`:366,374`).
    UsageError(String),
}

/// `handleWatchdogCommand(pi, runtime, args, ctx)` (`register-main.ts:246-375`).
///
/// Every arm's text is upstream's verbatim, including the failure texts: a model/thinking write that
/// throws reports the message AND "No settings files were changed.", because it does not reach
/// `writeSettingsFile`.
pub fn handle_watchdog_command(
    runtime: &MainWatchdogRuntime,
    args: &str,
    ctx: &WatchdogCommandContext<'_>,
) -> (WatchdogCommandOutcome, Option<WatchdogWarningDetails>) {
    let input = args.trim();
    let text = |value: String| (WatchdogCommandOutcome::Text(value), None);

    if input.is_empty() || input == "status" {
        return text(build_watchdog_status(
            &runtime.get_snapshot(Some(&ctx.cwd)),
            ctx,
        ));
    }
    if input == "recommend-model" {
        return text(match build_recommendation_text(ctx) {
            Ok(value) => value,
            Err(message) => format!("Subagent watchdog recommended model\n\n{message}"),
        });
    }
    if input == "check" {
        return text(match build_check_text(runtime, ctx) {
            Ok(value) => value,
            Err(message) => format!("Subagent watchdog config check\n\n{message}"),
        });
    }
    if input == "on" || input == "off" {
        let enabled = input == "on";
        return text(match write_user_watchdog_enabled(enabled) {
            Ok(settings_path) => {
                let snapshot = runtime.get_snapshot(Some(&ctx.cwd));
                [
                    format!(
                        "Subagent watchdog {} saved to user settings.",
                        bool_label(enabled)
                    ),
                    format!("Updated: {}", settings_path.display()),
                    format!(
                        "Main now: {}{}",
                        bool_label(snapshot.enabled),
                        match snapshot.session_override {
                            Some(value) => format!(" (session override {})", bool_label(value)),
                            None => String::new(),
                        }
                    ),
                ]
                .join("\n")
            }
            Err(message) => format!(
                "Subagent watchdog\n\nCould not update {}: {message}",
                get_watchdog_user_settings_path().display()
            ),
        });
    }
    if input == "session on" || input == "session off" {
        let enabled = input.ends_with("on");
        let snapshot = runtime.set_session_enabled(enabled, &ctx.cwd);
        return text(
            [
                format!(
                    "Subagent watchdog session override: {}.",
                    bool_label(enabled)
                ),
                "No settings files were changed.".to_string(),
                String::new(),
                build_watchdog_status(&snapshot, ctx),
            ]
            .join("\n"),
        );
    }
    if let Some(raw_model) = input.strip_prefix("session model ") {
        return text(match resolve_model_command_value(ctx, raw_model) {
            Ok(value) => {
                let snapshot = match &value.model {
                    None => runtime.clear_session_model(&ctx.cwd),
                    Some(model) => runtime.set_session_model(
                        Some(Some(model.clone())),
                        Some(value.thinking.clone()),
                        &ctx.cwd,
                    ),
                };
                [
                    format!("Subagent watchdog session model: {}.", value.description),
                    "No settings files were changed.".to_string(),
                    String::new(),
                    build_watchdog_status(&snapshot, ctx),
                ]
                .join("\n")
            }
            Err(message) => format!("Subagent watchdog session model\n\n{message}"),
        });
    }
    if let Some(raw_model) = input.strip_prefix("model ") {
        return text(
            match resolve_model_command_value(ctx, raw_model).and_then(|value| {
                let settings_path = write_watchdog_model_settings(&WatchdogModelSettingsWrite {
                    scope: WatchdogSettingsWriteScope::User,
                    cwd: None,
                    target: WatchdogModelSettingsTarget::Main,
                    model: Some(value.model.clone()),
                    thinking: Some(value.thinking.clone()),
                })?;
                Ok((value, settings_path))
            }) {
                Ok((value, settings_path)) => {
                    runtime.refresh_config(&ctx.cwd);
                    let snapshot = runtime.get_snapshot(Some(&ctx.cwd));
                    [
                        format!("Subagent watchdog model saved: {}.", value.description),
                        format!("Updated: {}", settings_path.display()),
                        format!("Main now: {}", bool_label(snapshot.enabled)),
                        if value.model.is_none() {
                            "The watchdog now inherits the current session model and thinking."
                                .to_string()
                        } else {
                            "Run /subagents-watchdog on if the watchdog is still off.".to_string()
                        },
                        String::new(),
                        build_watchdog_status(&snapshot, ctx),
                    ]
                    .join("\n")
                }
                Err(message) => {
                    format!("Subagent watchdog model\n\n{message}\nNo settings files were changed.")
                }
            },
        );
    }
    if let Some(raw_thinking) = input.strip_prefix("thinking ") {
        return text(
            match parse_thinking_command(raw_thinking).and_then(|thinking| {
                let settings_path = write_watchdog_model_settings(&WatchdogModelSettingsWrite {
                    scope: WatchdogSettingsWriteScope::User,
                    cwd: None,
                    target: WatchdogModelSettingsTarget::Main,
                    model: None,
                    thinking: Some(thinking.clone()),
                })?;
                Ok((thinking, settings_path))
            }) {
                Ok((thinking, settings_path)) => {
                    runtime.refresh_config(&ctx.cwd);
                    [
                        format!(
                            "Subagent watchdog thinking saved: {}.",
                            format_thinking(thinking.as_ref())
                        ),
                        format!("Updated: {}", settings_path.display()),
                        String::new(),
                        build_watchdog_status(&runtime.get_snapshot(Some(&ctx.cwd)), ctx),
                    ]
                    .join("\n")
                }
                Err(message) => format!(
                    "Subagent watchdog thinking\n\n{message}\nNo settings files were changed."
                ),
            },
        );
    }
    if let Some((severity, test_text)) = parse_test_command(input) {
        if test_text.is_empty() {
            return (
                WatchdogCommandOutcome::UsageError(
                    "Usage: /subagents-watchdog test concern|blocker <text>".to_string(),
                ),
                None,
            );
        }
        let warning = create_test_warning(severity, &test_text);
        let details = runtime.record_displayed_warning(&warning);
        // `:371` — the message is sent by the CALLER, which owns the delivery capability.
        return (WatchdogCommandOutcome::Text(String::new()), Some(details));
    }
    (
        WatchdogCommandOutcome::UsageError(format!(
            "Usage: /subagents-watchdog [status|on|off|session on|session off|recommend-model|model recommended|model <provider/model[:thinking]>|model inherit|thinking {}|thinking inherit|session model recommended|check|test concern <text>|test blocker <text>]",
            THINKING_LEVELS.join("|")
        )),
        None,
    )
}

// =================================================================================================
// The message renderer (`register-main.ts:392-401`)
// =================================================================================================

/// `pi.registerMessageRenderer(SUBAGENT_WATCHDOG_WARNING_TYPE, …)` (`register-main.ts:392-401`).
///
/// `message` is the serialized custom message. Upstream reads `message.details` and falls through
/// to the plain content when the three required fields are missing; this does the same, and adds
/// the one recovery the cyrup delivery seam needs — see [`parse_watchdog_warning_content`].
#[must_use]
pub fn render_watchdog_warning_message(message: &serde_json::Value) -> Option<serde_json::Value> {
    let details = message
        .get("details")
        .and_then(|d| serde_json::from_value::<WatchdogWarningDetails>(d.clone()).ok())
        .or_else(|| parse_watchdog_warning_content(&message_text(message)?));
    let Some(details) = details else {
        // `:394-399` — the fallback `new Text(content)`.
        return Some(serde_json::Value::String(message_text(message)?));
    };
    let lines = render_watchdog_warning(&details, false);
    Some(serde_json::Value::String(
        crate::tui::render::lines_to_plain_text(&lines).join("\n"),
    ))
}

/// The message body, from whichever field the host's serialization put it in: `content` (upstream's
/// own field) or `payload` (cyrup's `AgentMessage::Custom`, whose payload IS the content string).
fn message_text(message: &serde_json::Value) -> Option<String> {
    for key in ["content", "payload"] {
        match message.get(key) {
            Some(serde_json::Value::String(text)) => return Some(text.clone()),
            // Pi's `content` may be a block array; join the text blocks (`:396-397`).
            Some(serde_json::Value::Array(blocks)) => {
                let joined: Vec<&str> = blocks
                    .iter()
                    .filter(|block| {
                        block.get("type").and_then(serde_json::Value::as_str) == Some("text")
                    })
                    .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
                    .collect();
                return Some(joined.join("\n"));
            }
            _ => {}
        }
    }
    None
}

/// Read a `<subagent_watchdog …>` block ([`super::warning_format::format_watchdog_warning_content`])
/// back into the details record it was written from.
///
/// This exists because [`cyrup_ext::host::HostServices::inject_message`] carries `content` but no
/// `details` object, so the renderer has no structured payload to read on the cyrup path. The block
/// is this crate's own output and carries every field, so the recovery is lossless; `None` for
/// anything that is not one (a hand-written message, a truncated block), which falls the renderer
/// back to the plain text exactly as upstream's missing-details branch does.
#[must_use]
pub fn parse_watchdog_warning_content(content: &str) -> Option<WatchdogWarningDetails> {
    let trimmed = content.trim();
    if !trimmed.starts_with("<subagent_watchdog ") {
        return None;
    }
    let attr = |name: &str| {
        let needle = format!("{name}=\"");
        let start = trimmed.find(&needle)? + needle.len();
        let rest = trimmed.get(start..)?;
        let end = rest.find('"')?;
        Some(unescape_xml(rest.get(..end)?))
    };
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = trimmed.find(&open)? + open.len();
        let rest = trimmed.get(start..)?;
        let end = rest.find(&close)?;
        Some(unescape_xml(rest.get(..end)?))
    };
    let severity = WatchdogSeverity::parse(&attr("severity")?)?;
    let warning = WatchdogWarning {
        severity,
        summary: tag("summary")?,
        evidence: tag("evidence")?,
        recommended_action: tag("recommended_action")?,
        category: attr("category").and_then(|v| WatchdogCategory::parse(&v)),
        confidence: tag("confidence").and_then(|v| WatchdogConfidence::parse(&v)),
        source: attr("source").and_then(|v| WatchdogWarningSource::parse(&v)),
        agent: tag("agent"),
        run_id: tag("run_id"),
        stale: tag("stale").and_then(|v| v.parse::<bool>().ok()),
        auto_follow_attempt: tag("auto_follow_attempt").and_then(|v| v.parse::<u32>().ok()),
        state: tag("state").and_then(|v| WatchdogWarningState::parse(&v)),
    };
    Some(normalize_watchdog_warning_details(
        &warning,
        &WatchdogWarningDetailsPatch::default(),
    ))
}

/// The inverse of `escapeXmlText`/`escapeXmlAttribute` — `&amp;` LAST, mirroring the escape order.
fn unescape_xml(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::super::runtime::WatchdogRuntimeSnapshot;
    use super::super::settings::default_watchdog_config;
    use super::super::types::{
        WatchdogLspResult, WatchdogLspRuntimeSnapshot, WatchdogLspStatus, WatchdogSettingsScope,
    };
    use super::super::warning_format::format_watchdog_warning_content;
    use super::*;

    struct EmptyRegistry;

    impl WatchdogModelRegistry for EmptyRegistry {
        fn available(&self) -> Vec<WatchdogModelInfo> {
            Vec::new()
        }
        fn find(&self, _provider: &str, _id: &str) -> Option<WatchdogModelInfo> {
            None
        }
        fn has_configured_auth(&self, _model: &WatchdogModelInfo) -> bool {
            false
        }
    }

    fn ctx() -> WatchdogCommandContext<'static> {
        WatchdogCommandContext {
            cwd: PathBuf::from("/tmp"),
            registry: &EmptyRegistry,
            current_model: Some(WatchdogModelInfo::new("anthropic", "claude-sonnet-4")),
            thinking_level: Some("medium".to_string()),
        }
    }

    fn snapshot() -> WatchdogRuntimeSnapshot {
        WatchdogRuntimeSnapshot {
            status: WatchdogRuntimeStatus::Idle,
            enabled: false,
            config: default_watchdog_config(),
            config_ok: true,
            errors: Vec::new(),
            sources: vec![WatchdogSettingsSource {
                scope: WatchdogSettingsScope::User,
                path: Some("/home/u/.cyrup/agent/settings.json".to_string()),
                exists: false,
            }],
            buffered_deltas: 0,
            epoch: 0,
            active_review_id: None,
            session_override: None,
            session_model_override: None,
            last_warning: None,
            last_error: None,
            failed_reviews: 0,
            stale_reviews: 0,
            review_connected: false,
            review_description: "not wired".to_string(),
            auto_follow_queued: false,
            auto_follow_attempts: 0,
            auto_follow_stalemate: false,
            review_trigger: super::super::runtime::WatchdogReviewTrigger::RepoEdits,
            changed_paths: None,
            lsp: WatchdogLspRuntimeSnapshot {
                result: WatchdogLspResult {
                    status: WatchdogLspStatus::Skipped,
                    provider: None,
                    checked_paths: Vec::new(),
                    skipped_paths: Vec::new(),
                    diagnostics: Vec::new(),
                    message: None,
                },
                enabled: true,
                diagnostic_count: 0,
                fresh_diagnostic_count: 0,
                updated_at: None,
            },
        }
    }

    #[test]
    fn the_status_block_reports_the_default_off_state_verbatim() {
        let status = build_watchdog_status(&snapshot(), &ctx());
        assert!(
            status.starts_with("Subagent watchdog\nMain: off (default off)\n"),
            "{status}"
        );
        assert!(status.contains("Runtime: idle\n"));
        assert!(status.contains("Review trigger: repo edits only\n"));
        assert!(status.contains("Scope context: on\n"));
        assert!(status.contains("Cadence: boundary only\n"));
        assert!(status.contains("Session override: none\n"));
        assert!(status.contains("Main model: current session (anthropic/claude-sonnet-4)\n"));
        assert!(status.contains("Main thinking: current session (medium)\n"));
        assert!(status.contains(
            "Children: off · model current child session · thinking current child session\n"
        ));
        assert!(status.contains("Agent-end timeout: 30000ms\n"));
        assert!(status.contains("Auto-follow: off · attempts 0/3\n"));
        assert!(status.contains("Review model call: not wired\n"));
        assert!(status.contains("\nConfig: ok\n"));
        assert!(status.contains("- user /home/u/.cyrup/agent/settings.json: not found"));
        assert!(status.ends_with(
            "Agent action: subagent({ action: \"watchdog.configure\", model: \"recommended\", scope: \"session\" })"
        ));
    }

    #[test]
    fn a_waiting_status_label_loses_its_hyphens() {
        let mut snap = snapshot();
        snap.status = WatchdogRuntimeStatus::WaitingAtAgentEnd;
        snap.buffered_deltas = 2;
        let status = build_watchdog_status(&snap, &ctx());
        assert!(
            status.contains("Runtime: waiting at agent end · buffered deltas 2\n"),
            "{status}"
        );
    }

    #[test]
    fn a_broken_config_says_the_watchdog_is_disabled_until_it_is_fixed() {
        let mut snap = snapshot();
        snap.config_ok = false;
        snap.errors = vec![super::super::types::WatchdogSettingsError {
            scope: WatchdogSettingsScope::User,
            path: None,
            message: "bad field".to_string(),
        }];
        let status = build_watchdog_status(&snap, &ctx());
        assert!(
            status.contains(
                "\nConfig errors:\n- bad field\nWatchdog is disabled until the config is fixed."
            ),
            "{status}"
        );
    }

    #[test]
    fn an_unavailable_recommendation_is_reported_rather_than_thrown() {
        let status = build_watchdog_status(&snapshot(), &ctx());
        assert!(
            status.contains("Recommended strong watchdog: unavailable (No authenticated strong"),
            "{status}"
        );
    }

    #[test]
    fn the_changed_paths_line_caps_at_eight_and_counts_the_rest() {
        let mut snap = snapshot();
        snap.changed_paths = Some((0..11).map(|n| format!("f{n}")).collect());
        let status = build_watchdog_status(&snap, &ctx());
        assert!(
            status.contains("Changed paths: f0, f1, f2, f3, f4, f5, f6, f7, +3 more\n"),
            "{status}"
        );
    }

    #[test]
    fn the_test_command_grammar_matches_upstreams_regex() {
        assert_eq!(
            parse_test_command("test blocker something is wrong"),
            Some((WatchdogSeverity::Blocker, "something is wrong".to_string()))
        );
        assert_eq!(
            parse_test_command("test concern  a  b "),
            Some((WatchdogSeverity::Concern, "a  b".to_string()))
        );
        assert_eq!(parse_test_command("test blocker"), None);
        assert_eq!(parse_test_command("test warning x"), None);
        assert_eq!(parse_test_command("status"), None);
    }

    #[test]
    fn thinking_inherit_deletes_and_a_level_is_validated() {
        assert_eq!(parse_thinking_command("inherit").unwrap(), None);
        assert_eq!(
            parse_thinking_command(" high ").unwrap(),
            Some(ThinkingSetting::Level("high".to_string()))
        );
        assert_eq!(
            parse_thinking_command("false").unwrap(),
            Some(ThinkingSetting::Off)
        );
        let error = parse_thinking_command("nonsense").unwrap_err();
        assert!(
            error.starts_with("Unsupported watchdog thinking 'nonsense'"),
            "{error}"
        );
    }

    #[test]
    fn an_unknown_subcommand_returns_the_usage_error_naming_every_thinking_level() {
        let runtime = MainWatchdogRuntime::default();
        let (outcome, details) = handle_watchdog_command(&runtime, "wat", &ctx());
        assert!(details.is_none());
        match outcome {
            WatchdogCommandOutcome::UsageError(message) => {
                assert!(
                    message.starts_with("Usage: /subagents-watchdog [status|on|off|"),
                    "{message}"
                );
                assert!(
                    message.contains("thinking off|minimal|low|medium|high|xhigh|max|"),
                    "{message}"
                );
            }
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn a_test_command_records_a_displayed_warning_for_the_caller_to_send() {
        let runtime = MainWatchdogRuntime::default();
        let (outcome, details) =
            handle_watchdog_command(&runtime, "test blocker the renderer is broken", &ctx());
        assert_eq!(outcome, WatchdogCommandOutcome::Text(String::new()));
        let details = details.expect("a warning to send");
        assert_eq!(details.severity, WatchdogSeverity::Blocker);
        assert_eq!(details.summary, "the renderer is broken");
        assert_eq!(details.state, Some(WatchdogWarningState::Displayed));
        assert_eq!(details.category, WatchdogCategory::Other);
        assert_eq!(
            details.recommended_action,
            "Verify the renderer, transcript delivery, and auto-follow policy."
        );
        // It also became the runtime's last warning, so the status line reports it.
        assert_eq!(
            runtime.get_snapshot(None).last_warning.map(|w| w.summary),
            Some("the renderer is broken".to_string())
        );
    }

    #[test]
    fn a_test_command_with_no_text_falls_through_to_the_general_usage_error() {
        // `handleWatchdogCommand` trims its argument FIRST (`:252`), so by the time
        // `parseTestCommand`'s `\s+([\s\S]+)$` runs there is no trailing whitespace left for the
        // capture to match — `test blocker   ` simply does not parse as a test command, and
        // upstream falls through to the general usage error (`:374`) exactly as this does. The
        // `!test.text` branch at `:365-368` is defensive and unreachable through this entry point;
        // it is still ported, and [`parse_test_command`]'s own test pins the grammar directly.
        let runtime = MainWatchdogRuntime::default();
        let (outcome, warning) = handle_watchdog_command(&runtime, "test blocker   ", &ctx());
        assert!(warning.is_none(), "nothing was recorded");
        match outcome {
            WatchdogCommandOutcome::UsageError(message) => assert!(
                message.starts_with("Usage: /subagents-watchdog [status|on|off|"),
                "{message}"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_defensive_empty_text_branch_is_ported_but_unreachable_through_the_command() {
        // `parseTestCommand` DOES capture a whitespace-only tail (upstream's `([\s\S]+)` matches a
        // non-breaking space, and its `.trim()` then empties it) —
        assert_eq!(
            parse_test_command("test blocker \u{a0}"),
            Some((WatchdogSeverity::Blocker, String::new()))
        );
        // — but `handleWatchdogCommand` trims its whole argument first (`:252`), and both JS
        // `String.trim()` and Rust's `str::trim` treat U+00A0 as whitespace, so no input reaches
        // the parser with a tail that survives capture and then trims to empty. The `!test.text`
        // guard at `:365-368` is therefore ported for fidelity and is dead in both languages; the
        // reachable outcome is the general usage error, which
        // [`a_test_command_with_no_text_falls_through_to_the_general_usage_error`] pins.
        let runtime = MainWatchdogRuntime::default();
        let (outcome, warning) = handle_watchdog_command(&runtime, "test blocker \u{a0}", &ctx());
        assert!(warning.is_none());
        match outcome {
            WatchdogCommandOutcome::UsageError(message) => assert!(
                message.starts_with("Usage: /subagents-watchdog [status|on|off|"),
                "{message}"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn session_on_flips_the_override_and_reprints_the_status() {
        let runtime = MainWatchdogRuntime::default();
        let (outcome, _) = handle_watchdog_command(&runtime, "session on", &ctx());
        match outcome {
            WatchdogCommandOutcome::Text(text) => {
                assert!(text.starts_with("Subagent watchdog session override: on.\nNo settings files were changed.\n\nSubagent watchdog\n"), "{text}");
                assert!(text.contains("Session override: on\n"), "{text}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(runtime.get_snapshot(None).session_override, Some(true));
    }

    #[test]
    fn a_bad_session_model_reports_the_error_and_changes_nothing() {
        let runtime = MainWatchdogRuntime::default();
        let (outcome, _) = handle_watchdog_command(&runtime, "session model not-a-model", &ctx());
        match outcome {
            WatchdogCommandOutcome::Text(text) => assert!(
                text.starts_with("Subagent watchdog session model\n\nWatchdog model 'not-a-model' did not resolve to provider/model."),
                "{text}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(runtime.get_snapshot(None).session_model_override.is_none());
    }

    #[test]
    fn status_is_the_empty_argument_default() {
        let runtime = MainWatchdogRuntime::default();
        let (empty, _) = handle_watchdog_command(&runtime, "   ", &ctx());
        let (named, _) = handle_watchdog_command(&runtime, "status", &ctx());
        assert_eq!(empty, named);
    }

    // ---- the renderer ---------------------------------------------------------------------------

    fn details_fixture() -> WatchdogWarningDetails {
        WatchdogWarningDetails {
            severity: WatchdogSeverity::Blocker,
            summary: "the <migration> is not reversible".to_string(),
            evidence: "step 3 drops a column & has no down".to_string(),
            recommended_action: "add a down migration".to_string(),
            category: WatchdogCategory::UnsafeChange,
            source: WatchdogWarningSource::Child,
            confidence: Some(WatchdogConfidence::High),
            agent: Some("db-writer".to_string()),
            run_id: Some("run-7".to_string()),
            stale: Some(false),
            auto_follow_attempt: Some(2),
            state: Some(WatchdogWarningState::Displayed),
            identity: None,
            displayed_at: None,
            error: None,
            stalemate_repeats: None,
        }
    }

    #[test]
    fn the_warning_content_round_trips_back_into_details() {
        let original = details_fixture();
        let content = format_watchdog_warning_content(
            &super::super::warning_format::details_as_warning(&original),
        );
        let parsed = parse_watchdog_warning_content(&content).expect("parsed");
        assert_eq!(parsed.severity, original.severity);
        assert_eq!(
            parsed.summary, original.summary,
            "the escaped `<>` survived"
        );
        assert_eq!(
            parsed.evidence, original.evidence,
            "the escaped `&` survived"
        );
        assert_eq!(parsed.recommended_action, original.recommended_action);
        assert_eq!(parsed.category, original.category);
        assert_eq!(parsed.source, original.source);
        assert_eq!(parsed.confidence, original.confidence);
        assert_eq!(parsed.agent, original.agent);
        assert_eq!(parsed.run_id, original.run_id);
        assert_eq!(parsed.stale, original.stale);
        assert_eq!(parsed.auto_follow_attempt, original.auto_follow_attempt);
        assert_eq!(parsed.state, original.state);
    }

    #[test]
    fn a_non_watchdog_body_does_not_parse() {
        assert!(parse_watchdog_warning_content("just some text").is_none());
        assert!(
            parse_watchdog_warning_content("<subagent_watchdog severity=\"blocker\">").is_none()
        );
    }

    #[test]
    fn the_renderer_draws_the_card_from_a_cyrup_payload_message() {
        let content = format_watchdog_warning_content(
            &super::super::warning_format::details_as_warning(&details_fixture()),
        );
        let message = serde_json::json!({
            "role": "custom",
            "kind": SUBAGENT_WATCHDOG_WARNING_TYPE,
            "payload": content,
        });
        let rendered = render_watchdog_warning_message(&message).expect("rendered");
        let text = rendered.as_str().expect("text");
        assert!(text.starts_with("Subagent watchdog Blocker"), "{text}");
        assert!(text.contains("the <migration> is not reversible"), "{text}");
        assert!(
            !text.contains("<subagent_watchdog"),
            "the raw XML is not shown: {text}"
        );
    }

    #[test]
    fn the_renderer_prefers_an_explicit_details_object() {
        let message = serde_json::json!({
            "role": "custom",
            "kind": SUBAGENT_WATCHDOG_WARNING_TYPE,
            "payload": "not a watchdog block",
            "details": serde_json::to_value(details_fixture()).unwrap(),
        });
        let rendered = render_watchdog_warning_message(&message).expect("rendered");
        let text = rendered.as_str().expect("text");
        assert!(text.starts_with("Subagent watchdog Blocker"), "{text}");
    }

    #[test]
    fn an_unparseable_body_falls_back_to_the_plain_content() {
        let message = serde_json::json!({
            "role": "custom",
            "kind": SUBAGENT_WATCHDOG_WARNING_TYPE,
            "payload": "hand written",
        });
        let rendered = render_watchdog_warning_message(&message).expect("rendered");
        assert_eq!(rendered.as_str(), Some("hand written"));
    }

    #[test]
    fn a_message_with_no_body_at_all_renders_nothing() {
        assert!(
            render_watchdog_warning_message(&serde_json::json!({ "role": "custom" })).is_none()
        );
    }

    // ---- registration ---------------------------------------------------------------------------

    #[test]
    fn the_registered_runtime_reviews_on_repo_edits_with_the_real_review_wired() {
        let runtime = register_main_watchdog(
            Arc::new(|| None),
            Path::new("/tmp"),
            RegisterMainWatchdogOptions::default(),
        );
        let snapshot = runtime.get_snapshot(None);
        assert_eq!(
            snapshot.review_trigger,
            super::super::runtime::WatchdogReviewTrigger::RepoEdits,
            "the main role is `reviewChangesOnly: true`"
        );
        // `register-main.ts:383-384` ALWAYS wires a review (`options.review ??
        // createMainWatchdogReview(…)`) and labels it `"real model review"`; a runtime reporting
        // `not wired` would mean the orchestrator's boundary reviews nothing at all.
        assert!(snapshot.review_connected);
        assert_eq!(snapshot.review_description, "real model review");
    }

    #[test]
    fn an_injected_review_is_used_and_labelled_as_the_seam() {
        struct Seam;
        #[async_trait::async_trait]
        impl WatchdogReview for Seam {
            async fn review(
                &self,
                _request: super::super::runtime::WatchdogReviewRequest,
            ) -> Result<Option<super::super::runtime::WatchdogReviewResult>, String> {
                Ok(Some(super::super::runtime::WatchdogReviewResult::default()))
            }
        }
        let runtime = register_main_watchdog(
            Arc::new(|| None),
            Path::new("/tmp"),
            RegisterMainWatchdogOptions {
                runtime: None,
                review: Some(Arc::new(Seam)),
            },
        );
        let snapshot = runtime.get_snapshot(None);
        assert!(snapshot.review_connected);
        assert_eq!(snapshot.review_description, "injected seam");
    }

    #[test]
    fn an_injected_runtime_is_returned_as_is() {
        let existing = Arc::new(MainWatchdogRuntime::default());
        let returned = register_main_watchdog(
            Arc::new(|| None),
            Path::new("/tmp"),
            RegisterMainWatchdogOptions {
                runtime: Some(Arc::clone(&existing)),
                review: None,
            },
        );
        assert!(Arc::ptr_eq(&existing, &returned));
    }
}

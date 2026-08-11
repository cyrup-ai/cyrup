//! The main-session review — a 1:1 port of `pi-subagents/src/watchdog/review.ts` (302 lines
//! @v0.43.0).
//!
//! This is the model call the runtime's [`WatchdogReview`] seam expects: pick a review model,
//! build a read-only agent whose ONLY way to report anything is the `watchdog_warn` tool, hand it
//! the turn delta, and report the stop reason.
//!
//! The part that carries the safety property is the tool policy, and it is enforced twice
//! (`review.ts:271-276`): the tool LIST is filtered to [`WATCHDOG_ALLOWED_TOOL_NAMES`] before the
//! agent is built, AND `beforeToolCall` re-checks every call against the same set at execution time.
//! One filter would be enough for the tools the review was given; the second exists because a model
//! can name a tool the harness supplies from elsewhere. The reviewer must never edit, never run a
//! shell, never spawn an agent.
//!
//! The second load-bearing rule is that **freeform assistant text is ignored** (`:213`): a warning
//! exists only if `watchdog_warn` was called. That is what lets `finalStopReason` (`:236-246`) be
//! the sole quality signal — a review that rambled instead of calling the tool is a CLEAN review,
//! not a partially-parsed one.
//!
//! Model resolution ([`resolve_watchdog_review_model`], `:126-160`) has two arms with deliberately
//! different thinking rules. With an explicitly CONFIGURED model, context thinking is NOT consulted
//! (`allowContextThinking: false`, `:136`) — an explicit reviewer gets `off` unless its own config
//! or its `:suffix` says otherwise, because inheriting the session's reasoning budget would make the
//! reviewer's cost track the session's by accident. With the INHERITED session model, context
//! thinking IS consulted (`:153`), because there the reviewer genuinely is the session's model.
//!
//! [CYRUP-DELTA] upstream constructs a `pi-agent-core` `Agent` in-process and awaits
//! `agent.prompt(...)`. This crate's charter (`lib.rs`) is that a SUBAGENT run is always a real OS
//! subprocess — but a watchdog review is not a subagent run, it is a nested single-turn model call,
//! and cyrup has no in-crate agent-loop dependency to build one from (`cyrup-agent` is a dependency
//! for TYPES only, per `Cargo.toml`'s own comment). The turn itself is therefore expressed as the
//! [`WatchdogReviewAgent`] trait — one method, "run this system prompt + prompt with these tools and
//! tell me the stop reason" — and EVERYTHING else in `review.ts` (model resolution, thinking
//! resolution, both prompts, the tool schema, the warn-parameter validation, the allow-list, the
//! stop-reason fold) is ported here and runs identically whichever agent implementation is bound.

use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::CancelToken;
use serde_json::{Value, json};

use super::model_selection::{
    THINKING_LEVELS, WatchdogModelContext, WatchdogModelInfo, normalize_model_segment,
    resolve_model_candidate,
};
use super::runtime::{
    ReviewStopReason, WatchdogReview, WatchdogReviewRequest, WatchdogReviewResult,
    WatchdogWarningEmitter,
};
use super::types::{
    ResolvedWatchdogConfig, ThinkingSetting, WatchdogCategory, WatchdogConfidence, WatchdogSeverity,
    WatchdogWarning, WatchdogWarningSource,
};
use crate::exec::split_known_thinking_suffix;

/// `WATCHDOG_ALLOWED_TOOL_NAMES` (`review.ts:20`) — read, search, list, and the warn tool. Nothing
/// that writes, executes or delegates.
pub const WATCHDOG_ALLOWED_TOOL_NAMES: [&str; 5] = ["read", "grep", "find", "ls", "watchdog_warn"];

/// The name of the one tool a review may use to report anything (`review.ts:174`).
pub const WATCHDOG_WARN_TOOL_NAME: &str = "watchdog_warn";

/// `WatchdogReviewAuth` (`review.ts:41-45`) — the credential overlay a review's stream calls carry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchdogReviewAuth {
    /// A bearer/API key.
    pub api_key: Option<String>,
    /// Extra request headers.
    pub headers: Option<Vec<(String, String)>>,
    /// Extra process env for a CLI-backed provider.
    pub env: Option<Vec<(String, String)>>,
}

/// `WatchdogReviewModelSelection` (`review.ts:47-52`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogReviewModelSelection {
    /// The model to review with.
    pub model: WatchdogModelInfo,
    /// The resolved reasoning level (always a concrete level — `off` when nothing pins one).
    pub thinking_level: String,
    /// The credential overlay.
    pub auth: WatchdogReviewAuth,
    /// True when `subagents.watchdog.main.model` named it; false when it is the session model.
    pub explicit: bool,
}

/// `fullModelId` (`review.ts:58-60`).
fn full_model_id(model: &WatchdogModelInfo) -> String {
    format!("{}/{}", model.provider, model.id)
}

/// `splitProviderModel` (`review.ts:62-66`).
fn split_provider_model(value: &str) -> Option<(&str, &str)> {
    let slash = value.find('/')?;
    if slash == 0 || slash == value.len() - 1 {
        return None;
    }
    Some((value.get(..slash)?, value.get(slash + 1..)?))
}

/// `assertThinkingLevel` (`review.ts:68-71`) — note the message differs from
/// `model-selection.ts`'s (`... or false` rather than `..., false, or inherit`), because this
/// parser never sees the `inherit` spelling.
fn assert_thinking_level(value: &str, source: &str) -> Result<String, String> {
    if THINKING_LEVELS.contains(&value) {
        return Ok(value.to_string());
    }
    Err(format!(
        "Unsupported watchdog thinking level '{value}' from {source}; expected {} or false.",
        THINKING_LEVELS.join(", ")
    ))
}

/// `resolveEffectiveThinking` (`shared/model-info.ts:35-40`): a `:suffix` on the model string wins
/// outright; otherwise the config value, but ONLY if it is a recognized level.
#[must_use]
pub fn resolve_effective_thinking(
    model: Option<&str>,
    config_thinking: Option<&ThinkingSetting>,
) -> Option<String> {
    let model = model?;
    let (_, suffix) = split_known_thinking_suffix(model);
    if let Some(level) = suffix.strip_prefix(':') {
        return Some(level.to_string());
    }
    match config_thinking {
        Some(ThinkingSetting::Level(level)) if THINKING_LEVELS.contains(&level.as_str()) => {
            Some(level.clone())
        }
        _ => None,
    }
}

/// `resolveReviewThinking` (`review.ts:79-88`) — the five-step ladder, in order.
///
/// 1. the model string's `:suffix`, or a recognized config level;
/// 2. an explicit `thinking: false` -> `off`;
/// 3. any other explicit config value, validated;
/// 4. the session's own level, but ONLY when `allow_context_thinking` (see the module doc);
/// 5. `off`.
///
/// # Errors
///
/// Propagates [`assert_thinking_level`] for an unrecognized configured level.
pub fn resolve_review_thinking(
    model_string: &str,
    config_thinking: Option<&ThinkingSetting>,
    allow_context_thinking: bool,
    current_thinking_level: Option<&str>,
) -> Result<String, String> {
    if let Some(level) = resolve_effective_thinking(Some(model_string), config_thinking) {
        return assert_thinking_level(&level, "watchdog model/config");
    }
    match config_thinking {
        Some(ThinkingSetting::Off) => return Ok("off".to_string()),
        Some(ThinkingSetting::Level(level)) => {
            return assert_thinking_level(level, "watchdog config");
        }
        None => {}
    }
    if allow_context_thinking
        && let Some(level) = current_thinking_level.filter(|l| THINKING_LEVELS.contains(l))
    {
        return Ok(level.to_string());
    }
    Ok("off".to_string())
}

/// `resolveConfiguredModel` (`review.ts:90-110`).
///
/// Note the FIRST failure message is reused for two distinct causes (no candidate at all, and a
/// candidate that is not `provider/model`), verbatim as upstream does.
///
/// # Errors
///
/// One of upstream's three messages: unresolvable, not found, or unauthenticated.
pub fn resolve_configured_model(
    ctx: &WatchdogModelContext<'_>,
    raw_model: &str,
) -> Result<(WatchdogModelInfo, String), String> {
    let available = ctx.registry.available();
    let preferred = ctx.current_model.as_ref().map(|m| m.provider.as_str());
    let unresolved = || {
        format!(
            "Configured watchdog model '{raw_model}' did not match exactly one authenticated \
             available model. Use provider/model or configure credentials for the intended provider."
        )
    };
    let Some(resolved) = resolve_model_candidate(raw_model, &available, preferred) else {
        return Err(unresolved());
    };
    let (base_model, _) = split_known_thinking_suffix(&resolved);
    let Some((provider, id)) = split_provider_model(base_model) else {
        return Err(unresolved());
    };
    let Some(model) = ctx.registry.find(provider, id) else {
        return Err(format!(
            "Configured watchdog model '{raw_model}' was not found as '{base_model}'."
        ));
    };
    if !ctx.registry.has_configured_auth(&model) {
        return Err(format!(
            "Configured watchdog model '{base_model}' is not authenticated. Configure credentials \
             for provider '{provider}' or choose an authenticated model."
        ));
    }
    Ok((model, resolved))
}

/// `ctx.modelRegistry.getApiKeyAndHeaders(model)` (`review.ts:112-120`) as a seam. `Err` is
/// upstream's `auth.ok === false` throw.
pub trait WatchdogReviewAuthResolver: Send + Sync {
    /// Resolve the credential overlay for `model`.
    ///
    /// # Errors
    ///
    /// A provider-specific failure message, which the caller wraps with the model id.
    fn resolve(&self, model: &WatchdogModelInfo) -> Result<WatchdogReviewAuth, String>;
}

/// The overlay-free resolver: every provider authenticates from the ambient environment. Used when
/// no explicit resolver is bound; the auth CHECK still happens (in
/// [`resolve_configured_model`]), only the overlay is empty.
#[derive(Debug, Clone, Copy, Default)]
pub struct AmbientReviewAuth;

impl WatchdogReviewAuthResolver for AmbientReviewAuth {
    fn resolve(&self, _model: &WatchdogModelInfo) -> Result<WatchdogReviewAuth, String> {
        Ok(WatchdogReviewAuth::default())
    }
}

/// `resolveReviewAuth` (`review.ts:112-120`) — wraps a resolver failure with the model id.
fn resolve_review_auth(
    auth: &dyn WatchdogReviewAuthResolver,
    model: &WatchdogModelInfo,
) -> Result<WatchdogReviewAuth, String> {
    auth.resolve(model).map_err(|error| {
        format!("Watchdog model auth failed for {}: {error}", full_model_id(model))
    })
}

/// `resolveWatchdogReviewModel` (`review.ts:122-160`).
///
/// # Errors
///
/// Propagates [`resolve_configured_model`], or reports the no-session-model case verbatim.
pub fn resolve_watchdog_review_model(
    ctx: &WatchdogModelContext<'_>,
    config: &ResolvedWatchdogConfig,
    auth_resolver: &dyn WatchdogReviewAuthResolver,
    current_thinking_level: Option<&str>,
) -> Result<WatchdogReviewModelSelection, String> {
    if let Some(configured) = config.main.model.as_deref() {
        let (model, model_string) = resolve_configured_model(ctx, configured)?;
        let thinking_level = resolve_review_thinking(
            &model_string,
            config.main.thinking.as_ref(),
            false,
            current_thinking_level,
        )?;
        let auth = resolve_review_auth(auth_resolver, &model)?;
        return Ok(WatchdogReviewModelSelection {
            model,
            thinking_level,
            auth,
            explicit: true,
        });
    }
    let Some(current_model) = ctx.current_model.clone() else {
        return Err(
            "Main watchdog review cannot run because the current Pi session model is unavailable \
             and subagents.watchdog.main.model is not configured."
                .to_string(),
        );
    };
    let thinking_level = resolve_review_thinking(
        &full_model_id(&current_model),
        config.main.thinking.as_ref(),
        true,
        current_thinking_level,
    )?;
    let auth = resolve_review_auth(auth_resolver, &current_model)?;
    Ok(WatchdogReviewModelSelection {
        model: current_model,
        thinking_level,
        auth,
        explicit: false,
    })
}

/// `WatchdogWarnParams` (`review.ts:22-30`) — the tool's JSON Schema, with upstream's exact
/// descriptions (they are prompt text the model reads).
#[must_use]
pub fn watchdog_warn_parameters_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["severity", "summary", "evidence", "recommendedAction"],
        "properties": {
            "severity": {
                "type": "string",
                "enum": WatchdogSeverity::ALL.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                "description": "concern for actionable risk, blocker for a likely wrong or unsafe outcome",
            },
            "summary": { "type": "string", "description": "One concise sentence naming the issue." },
            "evidence": {
                "type": "string",
                "description": "Specific evidence from the turn delta or inspected files.",
            },
            "recommendedAction": {
                "type": "string",
                "description": "Specific action the parent should take before accepting or continuing.",
            },
            "category": {
                "type": "string",
                "enum": WatchdogCategory::ALL.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
            },
            "confidence": {
                "type": "string",
                "enum": WatchdogConfidence::ALL.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
            },
        },
    })
}

/// The `watchdog_warn` tool's own description (`review.ts:176-180`), joined with spaces.
#[must_use]
pub fn watchdog_warn_tool_description() -> String {
    [
        "Emit one actionable main-session watchdog warning.",
        "Use only for medium/high confidence concerns or blockers that the parent should consider \
         before accepting the work.",
        "Do not use for nits, praise, informational notes, or clean reviews.",
    ]
    .join(" ")
}

/// `nonEmptyString` (`review.ts:162-166`).
fn non_empty_string(value: Option<&str>, field: &str) -> Result<String, String> {
    let trimmed = value.unwrap_or("").trim();
    if trimmed.is_empty() {
        return Err(format!("watchdog_warn.{field} must be a non-empty string."));
    }
    Ok(trimmed.to_string())
}

/// `toWatchdogWarning` (`review.ts:168-178`) — validate a `watchdog_warn` call's arguments into a
/// warning. `category` defaults to `other`, `confidence` to `medium`, and `source` is always `main`.
///
/// # Errors
///
/// The per-field message for a missing/blank required string, or an unrecognized enum member.
pub fn to_watchdog_warning(params: &Value) -> Result<WatchdogWarning, String> {
    let object = params
        .as_object()
        .ok_or_else(|| "watchdog_warn parameters must be an object.".to_string())?;
    let severity = object
        .get("severity")
        .and_then(Value::as_str)
        .and_then(WatchdogSeverity::parse)
        .ok_or_else(|| "watchdog_warn.severity must be concern or blocker.".to_string())?;
    let category = match object.get("category").and_then(Value::as_str) {
        None => WatchdogCategory::Other,
        Some(value) => WatchdogCategory::parse(value)
            .ok_or_else(|| format!("watchdog_warn.category '{value}' is not a known category."))?,
    };
    let confidence = match object.get("confidence").and_then(Value::as_str) {
        None => WatchdogConfidence::Medium,
        Some(value) => WatchdogConfidence::parse(value).ok_or_else(|| {
            format!("watchdog_warn.confidence '{value}' is not a known confidence.")
        })?,
    };
    Ok(WatchdogWarning {
        severity,
        summary: non_empty_string(object.get("summary").and_then(Value::as_str), "summary")?,
        evidence: non_empty_string(object.get("evidence").and_then(Value::as_str), "evidence")?,
        recommended_action: non_empty_string(
            object.get("recommendedAction").and_then(Value::as_str),
            "recommendedAction",
        )?,
        category: Some(category),
        confidence: Some(confidence),
        source: Some(WatchdogWarningSource::Main),
        agent: None,
        run_id: None,
        stale: None,
        auto_follow_attempt: None,
        state: None,
    })
}

/// The tool-result text a `watchdog_warn` call gets back (`review.ts:186-192`) — which tells the
/// model whether the runtime actually took the warning, so a rejected one is not simply re-sent.
#[must_use]
pub fn watchdog_warn_result_text(accepted: bool) -> &'static str {
    if accepted {
        "Watchdog warning recorded."
    } else {
        "Watchdog warning was ignored by the runtime guard because it was stale, duplicate, or over \
         budget."
    }
}

/// `buildWatchdogSystemPrompt` (`review.ts:196-210`) — the reviewer's whole instruction set, lines
/// in upstream's order, with the scope line included only when the delta carries a scope block.
#[must_use]
pub fn build_watchdog_system_prompt(cwd: &str, has_scope: bool) -> String {
    let mut lines = vec![
        "You are the main-session subagent watchdog for Pi.".to_string(),
        format!("Working directory: {cwd}"),
        "Review only the supplied parent turn delta. Inspect repository files only when needed to \
         verify a concrete concern."
            .to_string(),
    ];
    if has_scope {
        lines.push(
            "When the review input includes a Current scope block, treat newer scope prompts as \
             superseding/mutating older prompts and use category='scope-drift' for work that serves \
             no current scope item."
                .to_string(),
        );
    }
    lines.extend([
        "You are read-only. You may use read, grep, find, and ls. Do not edit files, run shell \
         commands, spawn agents, or mutate state."
            .to_string(),
        "Emit warnings only by calling watchdog_warn. Freeform assistant text is ignored and must \
         not be used to report warnings."
            .to_string(),
        "Emit only medium/high confidence actionable concerns or blockers: missed user constraints, \
         correctness risks, test gaps that matter, unsafe changes, stale facts, loop risks, or scope \
         drift."
            .to_string(),
        "Do not emit nits, style preferences, low-confidence guesses, informational notes, praise, \
         or summaries."
            .to_string(),
        "If the turn is clean, call no tools and end normally.".to_string(),
        "Use severity='blocker' only when the issue should stop acceptance until addressed; \
         otherwise use severity='concern'."
            .to_string(),
    ]);
    lines.join("\n")
}

/// `buildReviewPrompt` (`review.ts:212-221`) — blocks joined by a BLANK line, the delta wrapped in
/// `<turn_delta>` so the model can tell instructions from reviewed content.
#[must_use]
pub fn build_review_prompt(
    request: &WatchdogReviewRequest,
    selection: &WatchdogReviewModelSelection,
) -> String {
    [
        "Review this parent-session turn delta for subagent-watchdog-worthy issues.".to_string(),
        format!(
            "Review id: {}; epoch: {}; review model: {}; thinking: {}.",
            request.review_id,
            request.epoch,
            full_model_id(&selection.model),
            selection.thinking_level
        ),
        "Call watchdog_warn for each qualifying concern or blocker. Call no tools when clean."
            .to_string(),
        "<turn_delta>".to_string(),
        request.delta.clone(),
        "</turn_delta>".to_string(),
    ]
    .join("\n\n")
}

/// `beforeToolCall` (`review.ts:281-283`) — the execution-time half of the two-layer tool policy.
#[must_use]
pub fn watchdog_tool_call_block_reason(tool_name: &str) -> Option<String> {
    if WATCHDOG_ALLOWED_TOOL_NAMES.contains(&tool_name) {
        return None;
    }
    Some(format!(
        "Watchdog reviews are read-only; tool '{tool_name}' is not allowed."
    ))
}

/// `finalStopReason` (`review.ts:223-233`): scan the message list BACKWARDS for the last assistant
/// message and report its stop reason, mapping anything that is not `error`/`aborted`/`length` — and
/// the no-assistant-message case — to `stop`.
#[must_use]
pub fn final_stop_reason(messages: &[Value]) -> ReviewStopReason {
    for message in messages.iter().rev() {
        let Some(object) = message.as_object() else {
            continue;
        };
        if object.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        return match object.get("stopReason").and_then(Value::as_str) {
            Some("error") => ReviewStopReason::Error,
            Some("aborted") => ReviewStopReason::Aborted,
            Some("length") => ReviewStopReason::Length,
            _ => ReviewStopReason::Stop,
        };
    }
    ReviewStopReason::Stop
}

/// The single turn `review.ts:270-295` runs: a read-only agent, the watchdog system prompt, the
/// review prompt, and the `watchdog_warn` tool wired to the runtime's emitter.
///
/// See the module doc for why this is a seam rather than an inline `Agent` construction. An
/// implementation MUST honour three things, all of which upstream's does: call `emit` for each
/// `watchdog_warn` call (feeding the tool result back from
/// [`watchdog_warn_result_text`]), refuse any tool
/// [`watchdog_tool_call_block_reason`] names, and abort when `cancel` fires.
#[async_trait]
pub trait WatchdogReviewAgent: Send + Sync {
    /// Run the review turn and report the message list it produced (for
    /// [`final_stop_reason`]).
    ///
    /// # Errors
    ///
    /// Any provider or transport failure, which the runtime records as a failed review.
    async fn run(&self, turn: WatchdogReviewTurn<'_>) -> Result<Vec<Value>, String>;
}

/// The whole input of one review turn.
pub struct WatchdogReviewTurn<'a> {
    /// The resolved review model and its credential overlay.
    pub selection: &'a WatchdogReviewModelSelection,
    /// `buildWatchdogSystemPrompt`'s output.
    pub system_prompt: String,
    /// `buildReviewPrompt`'s output.
    pub prompt: String,
    /// The tool names the agent may expose, already filtered.
    pub allowed_tools: &'a [&'a str],
    /// The `watchdog_warn` schema.
    pub warn_tool_schema: Value,
    /// Where an accepted `watchdog_warn` call goes.
    pub emit_warning: &'a WatchdogWarningEmitter,
    /// Cancellation.
    pub cancel: CancelToken,
}

/// `createMainWatchdogReview` (`review.ts:248-302`) — the [`WatchdogReview`] the main session binds.
pub struct MainWatchdogReview {
    registry: Arc<dyn super::model_selection::WatchdogModelRegistry>,
    auth_resolver: Arc<dyn WatchdogReviewAuthResolver>,
    agent: Arc<dyn WatchdogReviewAgent>,
    cwd: std::path::PathBuf,
    current_model: Option<WatchdogModelInfo>,
    current_thinking_level: Option<String>,
}

impl std::fmt::Debug for MainWatchdogReview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainWatchdogReview")
            .field("cwd", &self.cwd)
            .field("current_model", &self.current_model)
            .field("current_thinking_level", &self.current_thinking_level)
            .finish_non_exhaustive()
    }
}

impl MainWatchdogReview {
    /// Bind a review to a registry, an auth resolver and an agent implementation.
    #[must_use]
    pub fn new(
        registry: Arc<dyn super::model_selection::WatchdogModelRegistry>,
        auth_resolver: Arc<dyn WatchdogReviewAuthResolver>,
        agent: Arc<dyn WatchdogReviewAgent>,
        cwd: std::path::PathBuf,
    ) -> Self {
        Self {
            registry,
            auth_resolver,
            agent,
            cwd,
            current_model: None,
            current_thinking_level: None,
        }
    }

    /// `ctx.model` (`review.ts:250`).
    #[must_use]
    pub fn with_current_model(mut self, model: Option<WatchdogModelInfo>) -> Self {
        self.current_model = model;
        self
    }

    /// `options.getThinkingLevel?.()` (`review.ts:253`).
    #[must_use]
    pub fn with_current_thinking_level(mut self, level: Option<String>) -> Self {
        self.current_thinking_level = level;
        self
    }
}

#[async_trait]
impl WatchdogReview for MainWatchdogReview {
    async fn review(
        &self,
        request: WatchdogReviewRequest,
    ) -> Result<Option<WatchdogReviewResult>, String> {
        // `if (ctx.signal?.aborted || request.signal?.aborted) return { stopReason: "aborted" }`
        // (`:256`) — checked before the model is even resolved.
        if request.cancel.is_cancelled() {
            return Ok(Some(WatchdogReviewResult {
                warnings: Vec::new(),
                stop_reason: Some(ReviewStopReason::Aborted),
            }));
        }
        let ctx = WatchdogModelContext {
            registry: self.registry.as_ref(),
            current_model: self.current_model.clone(),
        };
        let selection = resolve_watchdog_review_model(
            &ctx,
            &request.config,
            self.auth_resolver.as_ref(),
            self.current_thinking_level.as_deref(),
        )?;
        // Re-checked AFTER resolution (`:259`): resolution can await auth, and the boundary may have
        // been superseded meanwhile.
        if request.cancel.is_cancelled() {
            return Ok(Some(WatchdogReviewResult {
                warnings: Vec::new(),
                stop_reason: Some(ReviewStopReason::Aborted),
            }));
        }
        let system_prompt = build_watchdog_system_prompt(
            &self.cwd.to_string_lossy(),
            request.has_scope,
        );
        let prompt = build_review_prompt(&request, &selection);
        let messages = self
            .agent
            .run(WatchdogReviewTurn {
                selection: &selection,
                system_prompt,
                prompt,
                allowed_tools: &WATCHDOG_ALLOWED_TOOL_NAMES,
                warn_tool_schema: watchdog_warn_parameters_schema(),
                emit_warning: &request.emit_warning,
                cancel: request.cancel.clone(),
            })
            .await?;
        Ok(Some(WatchdogReviewResult {
            warnings: Vec::new(),
            stop_reason: Some(final_stop_reason(&messages)),
        }))
    }
}

/// A review agent that runs no model at all and reports a clean turn — the honest stand-in for a
/// deployment with no provider bound, and the fixture every test in this module drives.
///
/// It is NOT `InertWatchdogReview`: that one bypasses model resolution entirely, where this still
/// resolves and validates the review model (so a misconfigured `subagents.watchdog.main.model`
/// still fails loudly) and simply performs no turn.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTurnReviewAgent;

#[async_trait]
impl WatchdogReviewAgent for NoTurnReviewAgent {
    async fn run(&self, _turn: WatchdogReviewTurn<'_>) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }
}

/// `currentProviderFamily`-style helper reused by callers that want to log which vendor reviewed.
#[must_use]
pub fn review_provider_family(selection: &WatchdogReviewModelSelection) -> String {
    normalize_model_segment(&selection.model.provider)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::watchdog::model_selection::WatchdogModelRegistry;
    use crate::watchdog::settings::default_watchdog_config;

    struct FakeRegistry(Vec<WatchdogModelInfo>);

    impl WatchdogModelRegistry for FakeRegistry {
        fn available(&self) -> Vec<WatchdogModelInfo> {
            self.0.clone()
        }
        fn find(&self, provider: &str, id: &str) -> Option<WatchdogModelInfo> {
            self.0.iter().find(|m| m.provider == provider && m.id == id).cloned()
        }
        fn has_configured_auth(&self, model: &WatchdogModelInfo) -> bool {
            model.provider != "unauthenticated"
        }
    }

    fn registry() -> FakeRegistry {
        FakeRegistry(vec![
            WatchdogModelInfo::new("anthropic", "claude-opus-4-8"),
            WatchdogModelInfo::new("unauthenticated", "secret-model"),
        ])
    }

    #[test]
    fn an_explicit_model_never_inherits_the_sessions_thinking_level() {
        let registry = registry();
        let ctx = WatchdogModelContext::new(&registry)
            .with_current_model(Some(WatchdogModelInfo::new("openai", "gpt-4o")));
        let mut config = default_watchdog_config();
        config.main.model = Some("anthropic/claude-opus-4-8".into());
        let selection =
            resolve_watchdog_review_model(&ctx, &config, &AmbientReviewAuth, Some("high")).unwrap();
        assert!(selection.explicit);
        assert_eq!(
            selection.thinking_level, "off",
            "an explicit reviewer must not inherit the session's reasoning budget"
        );
    }

    #[test]
    fn the_inherited_session_model_does_inherit_the_sessions_thinking_level() {
        let registry = registry();
        let ctx = WatchdogModelContext::new(&registry)
            .with_current_model(Some(WatchdogModelInfo::new("anthropic", "claude-opus-4-8")));
        let config = default_watchdog_config();
        let selection =
            resolve_watchdog_review_model(&ctx, &config, &AmbientReviewAuth, Some("medium"))
                .unwrap();
        assert!(!selection.explicit);
        assert_eq!(selection.thinking_level, "medium");
        // With no session thinking level at all it falls to `off`.
        let bare =
            resolve_watchdog_review_model(&ctx, &config, &AmbientReviewAuth, None).unwrap();
        assert_eq!(bare.thinking_level, "off");
    }

    #[test]
    fn a_model_suffix_beats_the_configured_level_which_beats_the_context() {
        assert_eq!(
            resolve_review_thinking("p/m:xhigh", Some(&ThinkingSetting::Level("low".into())), true, Some("high"))
                .unwrap(),
            "xhigh"
        );
        assert_eq!(
            resolve_review_thinking("p/m", Some(&ThinkingSetting::Level("low".into())), true, Some("high"))
                .unwrap(),
            "low"
        );
        assert_eq!(
            resolve_review_thinking("p/m", Some(&ThinkingSetting::Off), true, Some("high")).unwrap(),
            "off"
        );
        assert_eq!(
            resolve_review_thinking("p/m", None, true, Some("high")).unwrap(),
            "high"
        );
        assert_eq!(resolve_review_thinking("p/m", None, false, Some("high")).unwrap(), "off");
    }

    #[test]
    fn an_unrecognized_configured_level_is_reported_with_reviews_own_message() {
        let err =
            resolve_review_thinking("p/m", Some(&ThinkingSetting::Level("turbo".into())), false, None)
                .unwrap_err();
        assert_eq!(
            err,
            "Unsupported watchdog thinking level 'turbo' from watchdog config; expected off, \
             minimal, low, medium, high, xhigh, max or false."
        );
    }

    #[test]
    fn no_session_model_and_no_configured_model_is_a_hard_failure() {
        let registry = registry();
        let ctx = WatchdogModelContext::new(&registry);
        let err =
            resolve_watchdog_review_model(&ctx, &default_watchdog_config(), &AmbientReviewAuth, None)
                .unwrap_err();
        assert_eq!(
            err,
            "Main watchdog review cannot run because the current Pi session model is unavailable \
             and subagents.watchdog.main.model is not configured."
        );
    }

    #[test]
    fn an_unauthenticated_configured_model_is_rejected_before_any_turn() {
        let registry = registry();
        let ctx = WatchdogModelContext::new(&registry);
        let mut config = default_watchdog_config();
        config.main.model = Some("unauthenticated/secret-model".into());
        let err =
            resolve_watchdog_review_model(&ctx, &config, &AmbientReviewAuth, None).unwrap_err();
        assert!(err.contains("is not authenticated"));
    }

    #[test]
    fn the_tool_policy_allows_exactly_four_read_tools_plus_the_warn_tool() {
        for allowed in ["read", "grep", "find", "ls", "watchdog_warn"] {
            assert_eq!(watchdog_tool_call_block_reason(allowed), None, "{allowed}");
        }
        for denied in ["edit", "write", "bash", "subagent", "task"] {
            assert_eq!(
                watchdog_tool_call_block_reason(denied).unwrap(),
                format!("Watchdog reviews are read-only; tool '{denied}' is not allowed.")
            );
        }
    }

    #[test]
    fn warn_parameters_default_category_and_confidence_and_always_source_main() {
        let warning = to_watchdog_warning(&json!({
            "severity": "blocker",
            "summary": " s ",
            "evidence": " e ",
            "recommendedAction": " a ",
        }))
        .unwrap();
        assert_eq!(warning.severity, WatchdogSeverity::Blocker);
        assert_eq!(warning.category, Some(WatchdogCategory::Other));
        assert_eq!(warning.confidence, Some(WatchdogConfidence::Medium));
        assert_eq!(warning.source, Some(WatchdogWarningSource::Main));
        assert_eq!(warning.summary, "s", "fields are trimmed");
    }

    #[test]
    fn a_blank_required_field_is_rejected_per_field() {
        for (field, payload) in [
            ("summary", json!({ "severity": "concern", "summary": "  ", "evidence": "e", "recommendedAction": "a" })),
            ("evidence", json!({ "severity": "concern", "summary": "s", "evidence": "", "recommendedAction": "a" })),
            ("recommendedAction", json!({ "severity": "concern", "summary": "s", "evidence": "e", "recommendedAction": " " })),
        ] {
            assert_eq!(
                to_watchdog_warning(&payload).unwrap_err(),
                format!("watchdog_warn.{field} must be a non-empty string.")
            );
        }
        assert!(to_watchdog_warning(&json!({ "severity": "nope" })).is_err());
        assert!(to_watchdog_warning(&json!("string")).is_err());
    }

    #[test]
    fn the_system_prompt_includes_the_scope_line_only_when_there_is_scope() {
        let without = build_watchdog_system_prompt("/repo", false);
        assert!(!without.contains("Current scope block"));
        assert!(without.contains("Working directory: /repo"));
        assert_eq!(without.lines().count(), 9);
        let with = build_watchdog_system_prompt("/repo", true);
        assert!(with.contains("use category='scope-drift'"));
        assert_eq!(with.lines().count(), 10);
        // The scope line sits fourth, immediately after the review-scope instruction.
        assert!(with.lines().nth(3).unwrap().starts_with("When the review input includes"));
    }

    #[test]
    fn the_stop_reason_fold_reads_the_last_assistant_message_only() {
        assert_eq!(final_stop_reason(&[]), ReviewStopReason::Stop);
        let messages = vec![
            json!({ "role": "assistant", "stopReason": "error" }),
            json!({ "role": "toolResult" }),
            json!({ "role": "assistant", "stopReason": "length" }),
            json!({ "role": "user" }),
        ];
        assert_eq!(final_stop_reason(&messages), ReviewStopReason::Length);
        assert_eq!(
            final_stop_reason(&[json!({ "role": "assistant", "stopReason": "toolUse" })]),
            ReviewStopReason::Stop,
            "an unknown stop reason is a clean stop"
        );
        assert_eq!(
            final_stop_reason(&[json!({ "role": "user", "stopReason": "error" })]),
            ReviewStopReason::Stop,
            "only assistant messages count"
        );
    }

    #[test]
    fn the_review_prompt_wraps_the_delta_and_names_the_model_and_thinking() {
        let selection = WatchdogReviewModelSelection {
            model: WatchdogModelInfo::new("anthropic", "claude-opus-4-8"),
            thinking_level: "high".into(),
            auth: WatchdogReviewAuth::default(),
            explicit: true,
        };
        let request = WatchdogReviewRequest {
            delta: "the delta".into(),
            epoch: 3,
            has_scope: false,
            review_id: 7,
            config: default_watchdog_config(),
            emit_warning: WatchdogWarningEmitter::inert(),
            cancel: CancelToken::new(),
        };
        let prompt = build_review_prompt(&request, &selection);
        assert!(prompt.contains(
            "Review id: 7; epoch: 3; review model: anthropic/claude-opus-4-8; thinking: high."
        ));
        assert!(prompt.contains("<turn_delta>\n\nthe delta\n\n</turn_delta>"));
        assert_eq!(review_provider_family(&selection), "anthropic");
    }

    #[test]
    fn the_warn_result_text_tells_the_model_whether_it_landed() {
        assert_eq!(watchdog_warn_result_text(true), "Watchdog warning recorded.");
        assert!(watchdog_warn_result_text(false).contains("stale, duplicate, or over budget"));
        assert!(watchdog_warn_tool_description().starts_with("Emit one actionable"));
        let schema = watchdog_warn_parameters_schema();
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["required"].as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn a_cancelled_request_reports_aborted_without_resolving_a_model() {
        let review = MainWatchdogReview::new(
            Arc::new(registry()),
            Arc::new(AmbientReviewAuth),
            Arc::new(NoTurnReviewAgent),
            std::path::PathBuf::from("/repo"),
        );
        let cancel = CancelToken::new();
        cancel.cancel();
        let result = review
            .review(WatchdogReviewRequest {
                delta: "d".into(),
                epoch: 1,
                has_scope: false,
                review_id: 1,
                // A config with NO model and no session model would otherwise be a hard error.
                config: default_watchdog_config(),
                emit_warning: WatchdogWarningEmitter::inert(),
                cancel,
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.stop_reason, Some(ReviewStopReason::Aborted));
    }

    #[tokio::test]
    async fn a_live_request_resolves_the_model_and_reports_the_turns_stop_reason() {
        let review = MainWatchdogReview::new(
            Arc::new(registry()),
            Arc::new(AmbientReviewAuth),
            Arc::new(NoTurnReviewAgent),
            std::path::PathBuf::from("/repo"),
        )
        .with_current_model(Some(WatchdogModelInfo::new("anthropic", "claude-opus-4-8")));
        let result = review
            .review(WatchdogReviewRequest {
                delta: "d".into(),
                epoch: 1,
                has_scope: false,
                review_id: 1,
                config: default_watchdog_config(),
                emit_warning: WatchdogWarningEmitter::inert(),
                cancel: CancelToken::new(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.stop_reason, Some(ReviewStopReason::Stop));
        assert!(result.warnings.is_empty(), "warnings stream through the emitter");
    }
}

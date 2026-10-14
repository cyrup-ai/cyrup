//! The main-session review — a 1:1 port of `pi-subagents/src/watchdog/review.ts` (302 lines
//! @v0.43.0).
//!
//! This is the model call the runtime's [`WatchdogReview`] seam expects: pick a review model,
//! build a read-only agent whose ONLY way to report anything is the `watchdog_warn` tool, hand it
//! the turn delta, and report the stop reason.
//!
//! The part that carries the safety property is the tool policy, and it is enforced twice
//! (`review.ts:271-274` + `:285-287`): the tool LIST the agent is GIVEN is filtered to
//! [`WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES`] before the agent is built, AND `beforeToolCall`
//! re-checks every call against the wider [`WATCHDOG_ALLOWED_TOOL_NAMES`] permit set at execution
//! time.
//! One filter would be enough for the tools the review was given; the second exists because a model
//! can name a tool the harness supplies from elsewhere. The reviewer must never edit, never run a
//! shell, never spawn an agent.
//!
//! The second load-bearing rule is that **freeform assistant text is ignored** (`:214`): a warning
//! exists only if `watchdog_warn` was called. That is what lets `finalStopReason` (`:233-243`) be
//! the sole quality signal — a review that rambled instead of calling the tool is a CLEAN review,
//! not a partially-parsed one.
//!
//! Model resolution ([`resolve_watchdog_review_model`], `:123-160`) has two arms with deliberately
//! different thinking rules. With an explicitly CONFIGURED model, context thinking is NOT consulted
//! (`allowContextThinking: false`, `:136`) — an explicit reviewer gets `off` unless its own config
//! or its `:suffix` says otherwise, because inheriting the session's reasoning budget would make the
//! reviewer's cost track the session's by accident. With the INHERITED session model, context
//! thinking IS consulted (`allowContextThinking: true`, `:154`), because there the reviewer
//! genuinely is the session's model.
//!
//! [CYRUP-DELTA] upstream constructs a `pi-agent-core` `Agent` in-process and awaits
//! `agent.prompt(...)`. This crate's charter (`lib.rs`) is that a SUBAGENT run is always a real OS
//! subprocess — but a watchdog review is not a subagent run, it is a nested single-turn model call,
//! and upstream says so itself (`shared/opencode-session-headers.ts:17-18 @v0.68.0`: "subagent-
//! internal calls … stream through bare Agents"). The turn is therefore expressed as the
//! [`WatchdogReviewAgent`] trait — one method, "run this system prompt + prompt with these tools and
//! tell me the stop reason" — so EVERYTHING else in `review.ts` (model resolution, thinking
//! resolution, both prompts, the tool schema, the warn-parameter validation, the allow-list, the
//! stop-reason fold) is ported here and runs identically whichever agent implementation is bound.
//!
//! UW-4 filled that seam: [`ModelTurnReviewAgent`] runs the real nested `cyrup_agent::Agent` and is
//! what both production paths bind. The construction it shares with the permission arbiter lives in
//! [`super::agent_turn`], which is pinned to v0.68.0 while this file stays pinned to v0.43.0 — see
//! that module's doc for why the two tags are stated separately rather than mixed.

use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::CancelToken;
use serde_json::{Value, json};

use super::model_selection::{
    THINKING_LEVELS, WatchdogModelContext, WatchdogModelInfo, resolve_model_candidate,
};
use super::runtime::{
    ReviewStopReason, WatchdogClarification, WatchdogReview, WatchdogReviewRequest,
    WatchdogReviewResult, WatchdogWarningEmitter,
};
use super::types::{
    ResolvedWatchdogConfig, ThinkingSetting, WatchdogCategory, WatchdogImportance,
    WatchdogSeverity, WatchdogWarning, WatchdogWarningSource,
};
use crate::exec::split_known_thinking_suffix;

/// `WATCHDOG_ALLOWED_TOOL_NAMES` (`review.ts:20`; `:23` @ad11b7ab) — read, search, list, the warn
/// tool and `watchdog_diff` (SUBA-233). Nothing that writes, executes or delegates.
///
/// This is the one list that names `watchdog_diff`: like `watchdog_warn` it is never in the
/// read-only EXPOSE list ([`WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES`]), because it is built per review
/// over the session's baseline, and only when there is one ([`WatchdogReviewTurn::diff_baseline`]).
pub const WATCHDOG_ALLOWED_TOOL_NAMES: [&str; 6] = [
    "read",
    "grep",
    "find",
    "ls",
    "watchdog_warn",
    super::diff_tool::WATCHDOG_DIFF_TOOL_NAME,
];

/// The name of the one tool a review may use to report anything (`review.ts:182`).
pub const WATCHDOG_WARN_TOOL_NAME: &str = "watchdog_warn";

/// `label: "Watchdog warning"` (`review.ts:183`) — the human-facing name a harness shows for the
/// tool, distinct from the wire [`WATCHDOG_WARN_TOOL_NAME`] the model calls.
pub const WATCHDOG_WARN_TOOL_LABEL: &str = "Watchdog warning";

/// The tools the review agent is actually GIVEN (`review.ts:271-274`):
/// `createReadOnlyTools(ctx.cwd).filter(t => WATCHDOG_ALLOWED_TOOL_NAMES.has(t.name) && t.name !==
/// "watchdog_warn")`.
///
/// This is NOT [`WATCHDOG_ALLOWED_TOOL_NAMES`], and the difference is load-bearing. That constant is
/// the PERMIT list `beforeToolCall` re-checks at execution time (`:285`), and it contains
/// `watchdog_warn` because the review must be allowed to call it. This one is the EXPOSE list, and
/// upstream subtracts `watchdog_warn` from it precisely so a harness-supplied tool of that name
/// cannot shadow [`WatchdogWarnTool`] — the only `watchdog_warn` on the agent is the one built at
/// `:273`, whose `execute` reaches the runtime's emitter.
pub const WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES: [&str; 4] = ["read", "grep", "find", "ls"];

/// SUBA-234 — the reviewer's question tool (`name: "watchdog_ask"`, `review.ts:309` @ad11b7ab).
/// Given to a review only when its request allows a question
/// ([`WatchdogReviewRequest::allow_clarification`]), and permitted at execution time only then.
pub const WATCHDOG_ASK_TOOL_NAME: &str = "watchdog_ask";
/// Its `label` (`review.ts:310` @ad11b7ab).
pub const WATCHDOG_ASK_TOOL_LABEL: &str = "Watchdog clarification";
/// Its `description` (`review.ts:311` @ad11b7ab), verbatim — prompt text the model reads.
pub const WATCHDOG_ASK_TOOL_DESCRIPTION: &str = "Send one focused question when missing task \
     status, intent or other orchestrator context prevents a concrete review judgment. Yields and \
     ends this review so the orchestrator can handle the message and continue; no answer or \
     follow-up review is required. Never ask for approval or permission, or after recording a \
     warning. Question and evidence are capped at 1000 and 2000 characters.";
/// What a yielding call returns to the model (`review.ts:320` @ad11b7ab).
pub const WATCHDOG_ASK_RESULT_TEXT: &str = "Review yielded for clarification.";

/// `WatchdogAskParams` (`review.ts:28` @ad11b7ab): `{ question: string, evidence: string }`, no
/// other keys.
#[must_use]
pub fn watchdog_ask_parameters_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "question": { "type": "string" },
            "evidence": { "type": "string" },
        },
        "required": ["question", "evidence"],
        "additionalProperties": false,
    })
}

#[derive(Debug, Default)]
struct ClarificationLatchState {
    /// `let clarification` (`review.ts:294` @ad11b7ab).
    clarification: Option<WatchdogClarification>,
    /// `let warned` (`review.ts:295` @ad11b7ab) — a warning was ACCEPTED in this review.
    warned: bool,
}

/// SUBA-234 — one review's clarification state, shared by the three places that read or write it
/// upstream as closure captures (`review.ts:294-338` @ad11b7ab): the `watchdog_ask` tool (records
/// the question and yields), the wrapped `emitWarning` (refuses a warning after the yield, and
/// records that one was accepted, which closes the question), and `beforeToolCall` (refuses every
/// tool after the yield). The turn's own token is cancelled on the yield — upstream's
/// `agent.abort()` (`:319`) — so a batch the model sent alongside the question stops there.
#[derive(Debug, Clone)]
pub struct WatchdogClarificationLatch {
    state: Arc<std::sync::Mutex<ClarificationLatchState>>,
    /// The nested turn's token, cancelled on the yield.
    turn_cancel: CancelToken,
    /// The REVIEW's token — the runtime's abort — which refuses a question too (`:316`).
    review_cancel: CancelToken,
}

impl WatchdogClarificationLatch {
    /// A latch over one review: `review_cancel` is the request's token, `turn_cancel` the token
    /// the nested turn runs under (a child of it).
    #[must_use]
    pub fn new(review_cancel: CancelToken, turn_cancel: CancelToken) -> Self {
        Self {
            state: Arc::default(),
            turn_cancel,
            review_cancel,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ClarificationLatchState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether the reviewer has yielded with a question.
    #[must_use]
    pub fn yielded(&self) -> bool {
        self.lock().clarification.is_some()
    }

    /// The question, once yielded.
    #[must_use]
    pub fn clarification(&self) -> Option<WatchdogClarification> {
        self.lock().clarification.clone()
    }

    /// `warnRequest.emitWarning` (`review.ts:297-302` @ad11b7ab): after the yield every warning is
    /// refused; otherwise the runtime decides, and an accepted one closes the question.
    #[must_use]
    pub fn wrap_emitter(&self, inner: WatchdogWarningEmitter) -> WatchdogWarningEmitter {
        let latch = self.clone();
        WatchdogWarningEmitter::from_fn(Arc::new(move |warning| {
            if latch.yielded() {
                return false;
            }
            let accepted = inner.emit(warning);
            if accepted {
                latch.lock().warned = true;
            }
            accepted
        }))
    }

    /// `beforeToolCall` (`review.ts:338` @ad11b7ab): nothing after the yield; before it, the read-only
    /// permit list plus `watchdog_ask`.
    #[must_use]
    pub fn block_reason(&self, tool_name: &str) -> Option<String> {
        if !self.yielded()
            && (WATCHDOG_ALLOWED_TOOL_NAMES.contains(&tool_name)
                || tool_name == WATCHDOG_ASK_TOOL_NAME)
        {
            return None;
        }
        Some(format!(
            "Watchdog reviews are read-only; tool '{tool_name}' is not allowed."
        ))
    }

    /// `watchdog_ask`'s `execute` (`review.ts:314-321` @ad11b7ab): refuse after a warning, a
    /// previous yield or a cancellation; require both halves; record them bounded; and yield.
    ///
    /// # Errors
    ///
    /// Upstream's two thrown messages, which the harness reports to the model as the tool's error.
    pub fn ask(&self, params: &Value) -> Result<&'static str, String> {
        let text = |key: &str| {
            params
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string()
        };
        {
            let mut state = self.lock();
            if state.warned
                || state.clarification.is_some()
                || self.review_cancel.is_cancelled()
                || self.turn_cancel.is_cancelled()
            {
                return Err(
                    "Clarification unavailable after warning, yield, or cancellation.".to_string(),
                );
            }
            let (question, evidence) = (text("question"), text("evidence"));
            if question.is_empty() || evidence.is_empty() {
                return Err("A focused question and concrete evidence are required.".to_string());
            }
            state.clarification = Some(WatchdogClarification {
                question: super::runtime::bound_watchdog_review_text(
                    &question,
                    super::runtime::CLARIFICATION_QUESTION_MAX_CHARS,
                ),
                evidence: super::runtime::bound_watchdog_review_text(
                    &evidence,
                    super::runtime::CLARIFICATION_EVIDENCE_MAX_CHARS,
                ),
            });
        }
        // `agent.abort(); // Intentional yield also stops mixed tool batches` (`review.ts:319`).
        self.turn_cancel.cancel();
        Ok(WATCHDOG_ASK_RESULT_TEXT)
    }
}

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
        format!(
            "Watchdog model auth failed for {}: {error}",
            full_model_id(model)
        )
    })
}

/// `resolveWatchdogReviewModel` (`review.ts:123-160`).
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
        "required": ["severity", "importance", "summary", "evidence", "recommendedAction"],
        "properties": {
            "severity": {
                "type": "string",
                "enum": WatchdogSeverity::ALL.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                "description": "concern for actionable risk, blocker for a likely wrong or unsafe outcome",
            },
            // SUBA-120 — `review.ts:30` @v0.71.0. REQUIRED, no default: upstream declares it as a
            // plain (non-`Optional`) TypeBox property, so a call that omits it never reaches
            // `toWatchdogWarning`. The description is upstream's verbatim, because it is prompt text.
            "importance": {
                "type": "string",
                "enum": WatchdogImportance::ALL.iter().map(|i| i.as_str()).collect::<Vec<_>>(),
                "description": "low or medium for user-only visibility; high when the parent model must receive the finding",
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
        },
    })
}

/// The `watchdog_warn` tool's own description (`review.ts:184-188`), joined with spaces.
#[must_use]
pub fn watchdog_warn_tool_description() -> String {
    [
        "Emit one actionable main-session watchdog warning.",
        "Set importance explicitly: low or medium for user-only visibility, high only when the \
         parent model must receive the finding.",
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
/// warning. `category` defaults to `other` and `source` is always `main`.
///
/// SUBA-120: `importance` has NO default. Upstream's `WatchdogWarnParams` (`review.ts:30`) declares
/// it as a required TypeBox property, so a call omitting it is rejected by the schema before
/// `toWatchdogWarning` runs. This function is the only validation gate on this side, so it must
/// itself reject a missing `importance` rather than substitute one — the field it replaced
/// (`confidence`) did silently default to `medium` here, which would route every unlabelled finding
/// to the parent model.
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
    let importance = match object.get("importance").and_then(Value::as_str) {
        None => {
            return Err("watchdog_warn.importance must be low, medium, or high.".to_string());
        }
        Some(value) => WatchdogImportance::parse(value).ok_or_else(|| {
            format!("watchdog_warn.importance '{value}' is not a known importance.")
        })?,
    };
    let category = match object.get("category").and_then(Value::as_str) {
        None => WatchdogCategory::Other,
        Some(value) => WatchdogCategory::parse(value)
            .ok_or_else(|| format!("watchdog_warn.category '{value}' is not a known category."))?,
    };
    Ok(WatchdogWarning {
        severity,
        importance,
        summary: non_empty_string(object.get("summary").and_then(Value::as_str), "summary")?,
        evidence: non_empty_string(object.get("evidence").and_then(Value::as_str), "evidence")?,
        recommended_action: non_empty_string(
            object.get("recommendedAction").and_then(Value::as_str),
            "recommendedAction",
        )?,
        category: Some(category),
        source: Some(WatchdogWarningSource::Main),
        agent: None,
        run_id: None,
        stale: None,
        auto_follow_attempt: None,
        state: None,
    })
}

/// The tool-result text a `watchdog_warn` call gets back (`review.ts:197-199`) — which tells the
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

/// What one `watchdog_warn` call returns (`review.ts:194-202`): the text the model reads, and the
/// `details: { accepted }` the harness records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchdogWarnToolResult {
    /// `details: { accepted }` (`review.ts:201`) — whether the runtime's emission guard took it.
    pub accepted: bool,
    /// `content: [{ type: "text", text }]` (`review.ts:195-200`).
    pub text: &'static str,
}

/// `createWatchdogWarnTool(request)` (`review.ts:180-205`) — the complete descriptor of the ONE
/// tool a review may use to report anything, and the only path from a model call to the runtime's
/// emitter.
///
/// It exists as a value rather than as loose helpers because upstream builds the whole tool
/// eagerly at `:273` and hands it to the `Agent` alongside the read-only tools. Every field
/// upstream sets is here: [`Self::name`] (`:182`), [`Self::label`] (`:183`),
/// [`Self::description`] (`:184-188`), [`Self::parameters`] (`:189`), [`Self::SEQUENTIAL`]
/// (`executionMode`, `:190`) and [`Self::execute`] (`:191-203`). Before this type the seam carried
/// only the parameter schema, so a bound [`WatchdogReviewAgent`] had no way to learn the tool's
/// name, its label, or the description — which is PROMPT TEXT the model reads to decide when to
/// warn at all — and had to re-derive the `params -> warning -> emit -> result text` pipeline from
/// the trait's prose instead of calling it.
pub struct WatchdogWarnTool<'a> {
    emit_warning: &'a WatchdogWarningEmitter,
}

impl<'a> WatchdogWarnTool<'a> {
    /// `executionMode: "sequential"` (`review.ts:190`) — two `watchdog_warn` calls in one assistant
    /// message must reach the emission guard in order, because the guard's budget and dedup state
    /// is what decides the second one.
    pub const SEQUENTIAL: bool = true;

    /// Bind the tool to the runtime emitter `request.emitWarning` (`review.ts:193`).
    #[must_use]
    pub fn new(emit_warning: &'a WatchdogWarningEmitter) -> Self {
        Self { emit_warning }
    }

    /// `name: "watchdog_warn"` (`review.ts:182`).
    #[must_use]
    pub fn name(&self) -> &'static str {
        WATCHDOG_WARN_TOOL_NAME
    }

    /// `label: "Watchdog warning"` (`review.ts:183`).
    #[must_use]
    pub fn label(&self) -> &'static str {
        WATCHDOG_WARN_TOOL_LABEL
    }

    /// `description` (`review.ts:184-188`).
    #[must_use]
    pub fn description(&self) -> String {
        watchdog_warn_tool_description()
    }

    /// `parameters: WatchdogWarnParams` (`review.ts:189`).
    #[must_use]
    pub fn parameters(&self) -> Value {
        watchdog_warn_parameters_schema()
    }

    /// `execute(_toolCallId, params)` (`review.ts:191-203`) — validate, offer to the runtime, and
    /// report back whether it was taken.
    ///
    /// # Errors
    ///
    /// [`to_watchdog_warning`]'s per-field message. Upstream's `nonEmptyString` THROWS
    /// (`:163-164`), which the harness turns into a tool error the model sees; a bound agent must
    /// surface this `Err` the same way rather than swallowing it, or a malformed warning becomes
    /// silence instead of a correction the model can act on.
    pub fn execute(&self, params: &Value) -> Result<WatchdogWarnToolResult, String> {
        let warning = to_watchdog_warning(params)?;
        let accepted = self.emit_warning.emit(&warning);
        Ok(WatchdogWarnToolResult {
            accepted,
            text: watchdog_warn_result_text(accepted),
        })
    }
}

impl std::fmt::Debug for WatchdogWarnTool<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchdogWarnTool")
            .field("name", &self.name())
            .finish()
    }
}

/// `buildWatchdogSystemPrompt` (`review.ts:207-220`) — the reviewer's whole instruction set, lines
/// in upstream's order, with the scope line included only when the delta carries a scope block, and
/// `watchdog_diff` named only when the reviewer has it (`hasDiff`, `review.ts:231` @ad11b7ab).
///
/// SUBA-234 — `guidance` is [`super::guidance::load_watchdog_guidance`]'s text, appended after the
/// rules as `Standing instructions from WATCHDOG.md (project first, then user):` when non-empty
/// (`review.ts:226`,`:237` @ad11b7ab); blank guidance adds no block.
#[must_use]
pub fn build_watchdog_system_prompt(
    cwd: &str,
    has_scope: bool,
    has_diff: bool,
    guidance: &str,
) -> String {
    let mut lines = vec![
        "You are the main-session subagent watchdog for Pi.".to_string(),
        format!("Working directory: {cwd}"),
        "Review only the supplied parent turn delta. Inspect repository files only when needed to \
         verify a concrete concern."
            .to_string(),
    ];
    if has_scope {
        lines.push(
            // The pin's line (`review.ts:230` @ad11b7ab, `6c1af3a2`); see `scope.rs`'s render for
            // why `SUBA-198` needs it. The category instruction lives in the scope block itself.
            "Use the Current scope record alongside supplied activity evidence; an unrelated user \
             question does not cancel older authorized work."
                .to_string(),
        );
    }
    let tools = if has_diff {
        "read, grep, find, ls, and watchdog_diff (the full repo diff since the session baseline; \
         pass a path to narrow it)"
    } else {
        "read, grep, find, and ls"
    };
    lines.extend([
        format!(
            "You are read-only. You may use {tools}. Do not edit files, run shell commands, spawn \
             agents, or mutate state."
        ),
        "Emit warnings only by calling watchdog_warn. Freeform assistant text is ignored and must \
         not be used to report warnings."
            .to_string(),
        "Emit only actionable concerns or blockers: missed user constraints, correctness risks, \
         test gaps that matter, unsafe changes, stale facts, loop risks, or scope drift."
            .to_string(),
        "Do not emit nits, style preferences, unsupported guesses, informational notes, praise, or \
         summaries."
            .to_string(),
        "If the turn is clean, call no tools and end normally.".to_string(),
        "Use severity='blocker' only when the issue should stop acceptance until addressed; \
         otherwise use severity='concern'."
            .to_string(),
    ]);
    let guidance = guidance.trim();
    if !guidance.is_empty() {
        lines.push(format!(
            "\nStanding instructions from WATCHDOG.md (project first, then user):\n{guidance}"
        ));
    }
    lines.join("\n")
}

/// `buildReviewPrompt` (`review.ts:222-231`) — blocks joined by a BLANK line, the delta wrapped in
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

/// `beforeToolCall` (`review.ts:285-287`) — the execution-time half of the two-layer tool policy.
#[must_use]
pub fn watchdog_tool_call_block_reason(tool_name: &str) -> Option<String> {
    if WATCHDOG_ALLOWED_TOOL_NAMES.contains(&tool_name) {
        return None;
    }
    Some(format!(
        "Watchdog reviews are read-only; tool '{tool_name}' is not allowed."
    ))
}

/// `finalStopReason` (`review.ts:233-243`): scan the message list BACKWARDS for the last assistant
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

/// The single turn `review.ts:271-295` runs: a read-only agent, the watchdog system prompt, the
/// review prompt, and the `watchdog_warn` tool wired to the runtime's emitter.
///
/// See the module doc for why this is a seam rather than an inline `Agent` construction. An
/// implementation MUST honour three things, all of which upstream's does, and all three are now
/// CODE on [`WatchdogReviewTurn`] rather than prose to re-derive:
///
/// 1. expose [`WatchdogReviewTurn::read_only_tools`] plus [`WatchdogReviewTurn::warn_tool`] as the
///    agent's tool list (`:271-274`), and route every `watchdog_warn` call through
///    [`WatchdogWarnTool::execute`] (`:191-203`) — which emits AND produces the model-visible
///    result text;
/// 2. refuse any tool [`WatchdogReviewTurn::block_reason`] names, at execution time, even one the
///    harness supplied from outside the tool list (`beforeToolCall`, `:285-287`);
/// 3. abort when `cancel` fires (`:290-292`).
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
    /// `buildWatchdogSystemPrompt`'s output (`review.ts:207-220`).
    pub system_prompt: String,
    /// `buildReviewPrompt`'s output (`review.ts:222-231`).
    pub prompt: String,
    /// [`WATCHDOG_ALLOWED_TOOL_NAMES`] (`review.ts:20`) — the PERMIT list, which INCLUDES
    /// `watchdog_warn`. This is what `beforeToolCall` re-checks (`:285`), not what the agent is
    /// given; use [`Self::read_only_tools`] for that. See [`Self::block_reason`].
    pub allowed_tools: &'a [&'a str],
    /// [`WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES`] (`review.ts:271-274`) — the tools the agent is
    /// actually GIVEN: the permit list minus `watchdog_warn`, which arrives as [`Self::warn_tool`]
    /// instead so nothing can shadow it.
    pub read_only_tools: &'a [&'a str],
    /// `createWatchdogWarnTool(request)` (`review.ts:273`) — name, label, description, parameters
    /// and the executing body, already bound to this request's emitter.
    pub warn_tool: WatchdogWarnTool<'a>,
    /// The session's Git baseline, when there is one: the reviewer is then given `watchdog_diff`
    /// over it (`...(diffBaseline ? [createWatchdogDiffTool(diffBaseline)] : [])`, `review.ts:306`
    /// @ad11b7ab). `None` — no repository, or no `HEAD` yet — gives no diff tool, and the system
    /// prompt does not name one.
    pub diff_baseline: Option<super::diff_tool::WatchdogDiffBaseline>,
    /// `toolExecution: "sequential"` (`review.ts:288`) — the Agent-wide setting, distinct from the
    /// warn tool's own [`WatchdogWarnTool::SEQUENTIAL`] (`:190`). A review's read tools must not
    /// interleave, so a warning's evidence names a file state the reviewer actually observed.
    pub tool_execution_sequential: bool,
    /// Where an accepted `watchdog_warn` call goes. [`Self::warn_tool`] is already bound to it;
    /// this is the same emitter, exposed for an agent that tees or wraps.
    pub emit_warning: &'a WatchdogWarningEmitter,
    /// Cancellation.
    pub cancel: CancelToken,
    /// SUBA-234 — the review's clarification latch when this review may ask
    /// ([`WatchdogReviewRequest::allow_clarification`]); `None` gives the agent no `watchdog_ask`
    /// and leaves the tool policy the read-only one. When present, [`Self::emit_warning`] is
    /// already wrapped by it and [`Self::cancel`] is the token it cancels on the yield.
    pub clarification: Option<WatchdogClarificationLatch>,
}

impl WatchdogReviewTurn<'_> {
    /// `beforeToolCall` (`review.ts:285-287`) — the execution-time half of the two-layer tool
    /// policy. `None` permits the call; `Some(reason)` is upstream's
    /// `{ block: true, reason }`, which the harness reports to the model as the tool's result.
    ///
    /// This exists on the turn, not only as the free
    /// [`watchdog_tool_call_block_reason`], because it is the layer an implementation is most
    /// likely to skip: filtering the tool LIST looks sufficient right up until the harness supplies
    /// a tool of its own.
    #[must_use]
    pub fn block_reason(&self, tool_name: &str) -> Option<String> {
        match &self.clarification {
            Some(latch) => latch.block_reason(tool_name),
            None => watchdog_tool_call_block_reason(tool_name),
        }
    }
}

/// The slice of the LIVE session a review reads at call time: `ctx.model` and
/// `pi.getThinkingLevel()` (`review.ts:250-253`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchdogSessionContext {
    /// `ctx.model` — the session's current model, absent when none is bound.
    pub model: Option<WatchdogModelInfo>,
    /// `options.getThinkingLevel?.()` — the session's reasoning level.
    pub thinking_level: Option<String>,
}

/// `createMainWatchdogReview(provider, { getThinkingLevel })` (`review.ts:248-253`), where
/// `provider` is upstream's `() => currentContext` closure: BOTH roles pass a getter, never a
/// snapshot, because the extension context does not exist when the review is constructed and
/// changes across `session_start`/`session_before_switch`.
///
/// `None` is upstream's absent context, which throws at `:257`.
pub type WatchdogSessionContextFn = Arc<dyn Fn() -> Option<WatchdogSessionContext> + Send + Sync>;

/// `createMainWatchdogReview` (`review.ts:248-302`) — the [`WatchdogReview`] the main session binds.
pub struct MainWatchdogReview {
    registry: Arc<dyn super::model_selection::WatchdogModelRegistry>,
    auth_resolver: Arc<dyn WatchdogReviewAuthResolver>,
    agent: Arc<dyn WatchdogReviewAgent>,
    cwd: std::path::PathBuf,
    current_model: Option<WatchdogModelInfo>,
    current_thinking_level: Option<String>,
    session: Option<WatchdogSessionContextFn>,
}

impl std::fmt::Debug for MainWatchdogReview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainWatchdogReview")
            .field("cwd", &self.cwd)
            .field("current_model", &self.current_model)
            .field("current_thinking_level", &self.current_thinking_level)
            .field("session_bound", &self.session.is_some())
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
            session: None,
        }
    }

    /// `ctx.model` (`review.ts:250`), as a FIXED value — the test/embedder form. Production binds
    /// [`Self::with_session_context`] instead, because the live model can change under a review
    /// that outlives one session.
    #[must_use]
    pub fn with_current_model(mut self, model: Option<WatchdogModelInfo>) -> Self {
        self.current_model = model;
        self
    }

    /// `options.getThinkingLevel?.()` (`review.ts:255`), as a FIXED value. See
    /// [`Self::with_current_model`].
    ///
    /// No production caller, and none is coming: upstream reads the LIVE session
    /// (`options.getThinkingLevel?.()` calls `pi.getThinkingLevel()`), which is
    /// [`Self::with_session_context`], and both cyrup production paths bind that instead
    /// (`register_main::register_main_watchdog`'s review arm and
    /// `prompt_runtime::child_watchdog_review`). This and [`Self::with_current_model`]
    /// are the pair that fills the same two slots for an embedder with NO live session provider —
    /// `review()` reads them via `.or_else()` after the provider — so deleting one and keeping the
    /// other would leave a half-usable fallback.
    #[must_use]
    pub fn with_current_thinking_level(mut self, level: Option<String>) -> Self {
        self.current_thinking_level = level;
        self
    }

    /// Bind upstream's `() => currentContext` provider (`review.ts:250-253`) — the live session
    /// model and reasoning level, resolved at REVIEW time.
    ///
    /// Without it `config.main.model` is the only model a review can ever resolve, so the
    /// DEFAULT config (which sets no model, `settings.ts:72-96`) reaches
    /// [`resolve_watchdog_review_model`]'s no-session-model arm and every boundary fails with
    /// "the current Pi session model is unavailable". That is the whole inherited-model path, and
    /// it is the path every unconfigured session takes.
    #[must_use]
    pub fn with_session_context(mut self, session: WatchdogSessionContextFn) -> Self {
        self.session = Some(session);
        self
    }
}

#[async_trait]
impl WatchdogReview for MainWatchdogReview {
    async fn review(
        &self,
        request: WatchdogReviewRequest,
    ) -> Result<Option<WatchdogReviewResult>, String> {
        // `const ctx = resolveContext(provider); if (!ctx) throw …` (`:255`): the LIVE session,
        // resolved now rather than captured at construction. A bound provider that answers `None`
        // is upstream's missing extension context and fails the review loudly; an UNbound provider
        // is the embedder/test form, which falls back to whatever fixed values were set.
        let session = match &self.session {
            Some(provider) => match provider() {
                Some(session) => Some(session),
                None => {
                    return Err(
                        "Main watchdog review cannot run without an active Pi extension context."
                            .to_string(),
                    );
                }
            },
            None => None,
        };
        let current_model = session
            .as_ref()
            .and_then(|session| session.model.clone())
            .or_else(|| self.current_model.clone());
        let current_thinking_level = session
            .as_ref()
            .and_then(|session| session.thinking_level.clone())
            .or_else(|| self.current_thinking_level.clone());
        // `if (ctx.signal?.aborted || request.signal?.aborted) return { stopReason: "aborted" }`
        // (`:256`) — checked before the model is even resolved.
        if request.cancel.is_cancelled() {
            return Ok(Some(WatchdogReviewResult {
                warnings: Vec::new(),
                stop_reason: Some(ReviewStopReason::Aborted),
                clarification: None,
            }));
        }
        let ctx = WatchdogModelContext {
            registry: self.registry.as_ref(),
            current_model,
        };
        let selection = resolve_watchdog_review_model(
            &ctx,
            &request.config,
            self.auth_resolver.as_ref(),
            current_thinking_level.as_deref(),
        )?;
        // Re-checked AFTER resolution (`:259`): resolution can await auth, and the boundary may have
        // been superseded meanwhile.
        if request.cancel.is_cancelled() {
            return Ok(Some(WatchdogReviewResult {
                warnings: Vec::new(),
                stop_reason: Some(ReviewStopReason::Aborted),
                clarification: None,
            }));
        }
        // `const diffBaseline = await options.diffBaseline?.()` (`review.ts:293` @ad11b7ab) — after
        // the model is resolved, as upstream orders it.
        let diff_baseline = request.diff_baseline.get().await;
        // `guidance: loadWatchdogGuidance(ctx.cwd, request.config.guidance.watchdogMd)`
        // (`review.ts:325` @ad11b7ab) — read fresh for every review, so an edited `WATCHDOG.md`
        // reaches the next one without a restart.
        let guidance =
            super::guidance::load_watchdog_guidance(&self.cwd, request.config.guidance.watchdog_md);
        let system_prompt = build_watchdog_system_prompt(
            &self.cwd.to_string_lossy(),
            request.has_scope,
            diff_baseline.is_some(),
            &guidance,
        );
        let prompt = build_review_prompt(&request, &selection);
        // SUBA-234 — `request.allowClarification ? … : request` (`review.ts:294-322` @ad11b7ab):
        // a review that may ask runs its turn under a CHILD token the question cancels, with the
        // warn tool's emitter wrapped by the same latch. A review that may not is built exactly as
        // before — no `watchdog_ask`, the unwrapped emitter, the request's own token.
        let latch = request.allow_clarification.then(|| {
            WatchdogClarificationLatch::new(request.cancel.clone(), request.cancel.child_token())
        });
        let emit_warning = match &latch {
            Some(latch) => latch.wrap_emitter(request.emit_warning.clone()),
            None => request.emit_warning.clone(),
        };
        let turn_cancel = latch
            .as_ref()
            .map_or_else(|| request.cancel.clone(), |latch| latch.turn_cancel.clone());
        let messages = self
            .agent
            .run(WatchdogReviewTurn {
                selection: &selection,
                system_prompt,
                prompt,
                allowed_tools: &WATCHDOG_ALLOWED_TOOL_NAMES,
                read_only_tools: &WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES,
                // `createWatchdogWarnTool(warnRequest)` (`:306` @ad11b7ab) — built eagerly,
                // exactly where upstream builds it, so `name`/`label`/`description`/`parameters`
                // reach a bound agent instead of being re-derived by it.
                warn_tool: WatchdogWarnTool::new(&emit_warning),
                diff_baseline,
                tool_execution_sequential: true,
                emit_warning: &emit_warning,
                cancel: turn_cancel,
                clarification: latch.clone(),
            })
            .await?;
        // `if (ctx.signal?.aborted || request.signal?.aborted) return { stopReason: "aborted" }`
        // (`review.ts:354` @ad11b7ab) — the REVIEW's token, not the turn's: the yield cancels only
        // the turn, and a yielded review is not an aborted one.
        if request.cancel.is_cancelled() {
            return Ok(Some(WatchdogReviewResult {
                warnings: Vec::new(),
                stop_reason: Some(ReviewStopReason::Aborted),
                clarification: None,
            }));
        }
        // `clarification ? { clarification } : …` (`review.ts:361` @ad11b7ab) — a yielded review
        // reports its question and no stop reason; the turn's own abort is the yield, not a fault.
        if let Some(clarification) = latch
            .as_ref()
            .and_then(WatchdogClarificationLatch::clarification)
        {
            return Ok(Some(WatchdogReviewResult {
                warnings: Vec::new(),
                stop_reason: None,
                clarification: Some(clarification),
            }));
        }
        Ok(Some(WatchdogReviewResult {
            warnings: Vec::new(),
            stop_reason: Some(final_stop_reason(&messages)),
            clarification: None,
        }))
    }
}

/// `runWatchdogAttempt`'s turn (`review.ts:304-358 @v0.68.0`) — the REAL review agent: build the
/// read-only tool set over the review's cwd, add the bound `watchdog_warn` tool, run ONE nested
/// [`cyrup_agent::Agent`] turn under the two-layer tool policy, and hand back the message list the
/// stop-reason fold reads.
///
/// Bound in BOTH production paths (`register_main.rs`'s `options.review.unwrap_or_else` arm and
/// `prompt_runtime.rs`'s `child_watchdog_review`), which is what makes the runtime's
/// `review_description` — "real model review" — true.
///
/// [CYRUP-DELTA] this file is a v0.43.0 port; THIS type is pinned to **v0.68.0** and every citation
/// on it says so. The three features that live in the same upstream block are ported against
/// @ad11b7ab: `watchdog_diff` (SUBA-233, `review.ts:293`,`:306`: added when the session has a Git
/// baseline), and `watchdog_ask` with the clarification yield and `loadWatchdogGuidance`/
/// `WATCHDOG.md` (SUBA-234, `review.ts:294-338`,`:325`): the ask tool is added, and permitted, when
/// the turn carries a [`WatchdogClarificationLatch`]; the guidance is read by
/// [`MainWatchdogReview`] into the system prompt.
pub struct ModelTurnReviewAgent {
    cwd: std::path::PathBuf,
    turn: super::agent_turn::WatchdogAgentTurn,
}

impl std::fmt::Debug for ModelTurnReviewAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelTurnReviewAgent")
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

impl ModelTurnReviewAgent {
    /// Bind the turn to the review's working directory (`createReadOnlyTools(ctx.cwd)`,
    /// `review.ts:305 @v0.68.0`) and to the late-resolved capability backend it streams through.
    #[must_use]
    pub fn new(
        cwd: std::path::PathBuf,
        services: super::register_main::WatchdogServicesFn,
    ) -> Self {
        Self {
            cwd,
            turn: super::agent_turn::WatchdogAgentTurn::new(services),
        }
    }

    /// `beforeToolCall` (`review.ts:338-340 @v0.68.0`) — the EXECUTION-TIME half of the two-layer
    /// tool policy, as an `Arc` so the value production installs on the nested agent is a value a
    /// caller can hold and exercise.
    ///
    /// This accessor is the ONLY source of the policy [`Self::turn_request`] installs, so it is
    /// not a second copy that can drift from the one the agent runs under: gutting it guts the
    /// running turn. It is `pub` because the layer it guards is unreachable end to end — cyrup's
    /// nested agent gets its tool list from [`Self::turn_request`] and nowhere else, so a tool the
    /// permit list rejects is a tool the agent does not have and the loop answers "Tool `<name>`
    /// not found" before any hook runs (`cyrup-agent`'s `agent/run/tools/preflight.rs:17`, where
    /// the lookup precedes `before_tool_call`). An out-of-crate test therefore has to reach the
    /// policy here to assert the sentence a blocked call produces.
    #[must_use]
    #[allow(clippy::type_complexity)]
    pub fn tool_call_block_reason(&self) -> Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
        Arc::new(watchdog_tool_call_block_reason)
    }

    /// The exact [`super::agent_turn::WatchdogTurnRequest`] [`WatchdogReviewAgent::run`] hands the
    /// nested agent: the filtered read-only tool list plus the bound `watchdog_warn` tool
    /// (`review.ts:305-306 @v0.68.0`) and the execution-time policy (`:338-340`).
    ///
    /// Extracted from `run` so the construction is reachable without a provider, a model or a
    /// stream: `run` builds nothing of its own, so a test over this function is a test over what
    /// production installs rather than over a re-declared copy of it.
    pub(crate) fn turn_request<'a>(
        &self,
        turn: WatchdogReviewTurn<'a>,
    ) -> super::agent_turn::WatchdogTurnRequest<'a> {
        // `...(options.createReadOnlyTools ?? createReadOnlyTools)(ctx.cwd).filter((tool) =>
        // WATCHDOG_ALLOWED_TOOL_NAMES.has(tool.name) && tool.name !== "watchdog_warn")`
        // (`review.ts:305 @v0.68.0`). `cyrup_tools::read_only_tools` IS
        // `createReadOnlyToolDefinitions` — it filters the eight built-ins to exactly
        // `{read,grep,find,ls}` (`cyrup-tools/src/registry.rs:195-202`) — and the filter below is
        // upstream's, kept rather than assumed: it is what guarantees the EXPOSE list stays the
        // permit list minus `watchdog_warn`, so nothing the registry gains can shadow the bound
        // tool.
        let mut tools: Vec<Arc<dyn cyrup_core::Tool>> = cyrup_tools::read_only_tools(
            self.cwd.clone(),
            cyrup_tools::Backend::local(),
            cyrup_tools::ToolsOptions::default(),
        )
        .into_iter()
        .filter(|tool| {
            turn.allowed_tools.contains(&tool.name())
                && turn.read_only_tools.contains(&tool.name())
                && tool.name() != WATCHDOG_WARN_TOOL_NAME
        })
        .collect();
        // `createWatchdogWarnTool(warnRequest)` (`:306 @v0.68.0`) — the one tool that can report
        // anything, already bound to this request's emitter by the turn descriptor.
        tools.push(Arc::new(super::agent_turn::WatchdogWarnAgentTool::new(
            turn.emit_warning.clone(),
        )));
        // `...(diffBaseline ? [createWatchdogDiffTool(diffBaseline)] : [])` (`review.ts:306`
        // @ad11b7ab, SUBA-233) — the diff since the session's baseline, only when there is one.
        if let Some(baseline) = turn.diff_baseline {
            tools.push(Arc::new(super::diff_tool::WatchdogDiffAgentTool::new(
                Some(baseline),
                false,
            )));
        }
        // `if (request.allowClarification) tools.push({ name: "watchdog_ask", … })`
        // (`review.ts:308-322` @ad11b7ab, SUBA-234), and the execution-time policy that permits it
        // (`:338`) — both from the one latch, so the tool and its permission cannot disagree.
        let block_reason = match &turn.clarification {
            Some(latch) => {
                tools.push(Arc::new(super::agent_turn::WatchdogAskAgentTool::new(
                    latch.clone(),
                )));
                let latch = latch.clone();
                Arc::new(move |name: &str| latch.block_reason(name))
                    as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>
            }
            None => self.tool_call_block_reason(),
        };
        super::agent_turn::WatchdogTurnRequest {
            system_prompt: turn.system_prompt,
            prompt: turn.prompt,
            selection: turn.selection,
            tools,
            // `beforeToolCall` (`:338-340 @v0.68.0`) — the EXECUTION-TIME layer, checked against
            // the wider PERMIT list, which is what refuses a tool the harness supplied from
            // outside the list above.
            block_reason,
            cancel: turn.cancel.clone(),
        }
    }
}

#[async_trait]
impl WatchdogReviewAgent for ModelTurnReviewAgent {
    async fn run(&self, turn: WatchdogReviewTurn<'_>) -> Result<Vec<Value>, String> {
        let messages = self.turn.run(self.turn_request(turn)).await?;
        // `if (ctx.signal?.aborted || request.signal?.aborted) return { stopReason: "aborted" }`
        // (`:355 @v0.68.0`) — re-checked AFTER the prompt. An empty list folds to `Stop`, so an
        // aborted run has to say so through the terminal assistant message the loop emitted; the
        // fold reads it.
        Ok(messages)
    }
}

/// A review agent that runs no model at all and reports a clean turn.
///
/// **Test fixture only, as of UW-4.** Production binds [`ModelTurnReviewAgent`] on both paths; this
/// remains because it is the honest double for "the turn produced nothing", which every test in
/// this module drives and which the `Stop`-with-zero-warnings contract
/// ([`final_stop_reason`]'s empty-list arm) is stated in terms of. It is NOT
/// `#[allow(dead_code)]`-ed and it is NOT bound in production.
///
/// It is also NOT `InertWatchdogReview`: that one bypasses model resolution entirely, where a
/// review over THIS agent still resolves and validates the review model (so a misconfigured
/// `subagents.watchdog.main.model` still fails loudly) and simply performs no turn.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTurnReviewAgent;

#[cfg(test)]
#[async_trait]
impl WatchdogReviewAgent for NoTurnReviewAgent {
    async fn run(&self, _turn: WatchdogReviewTurn<'_>) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }
}

// A `review_provider_family(selection) -> String` helper lived here and was deleted. It had no
// caller, and its doc named it a "`currentProviderFamily`-style helper" — but `currentProviderFamily`
// is `model-selection.ts:109-114`, it takes the CONTEXT (`ctx.model`), not a resolved review
// selection, and it answers the three-valued `"openai" | "anthropic" | undefined` that
// `strongFamilyOrder` branches on at `:120-122`. cyrup already ports that function faithfully, as
// the private `current_provider_family` in `model_selection.rs:604`, called at `:632`. This one
// returned a bare normalized provider string, branched on nothing, and existed in no upstream file.

// ## Where the seam's contract runs (UW-4: it now does)
//
// [`WatchdogWarnTool::execute`], [`WatchdogReviewTurn::block_reason`] and
// [`WatchdogWarnTool::SEQUENTIAL`] are the SEAM's own contract, and they run when a bound
// [`WatchdogReviewAgent`] runs a model turn — upstream's `agent.prompt(...)`
// (`review.ts:350 @v0.68.0`). That implementation is [`ModelTurnReviewAgent`], bound in BOTH
// production paths: `register_main::register_main_watchdog`'s `options.review.unwrap_or_else` arm
// (reached from `extension/host/mod.rs`'s `SubagentsExtension::new`) and
// `prompt_runtime::child_watchdog_review` (reached from `crates/cyrup/src/session_launch.rs`).
// `WatchdogWarnTool::execute` is reached through `agent_turn::WatchdogWarnAgentTool`, and
// `block_reason` through the `Hooks::before_tool_call` the same module installs — the execution-time
// layer, which is the one a tool-LIST filter alone would leave unenforced.

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::watchdog::model_selection::{WatchdogModelRegistry, normalize_model_segment};
    use crate::watchdog::settings::default_watchdog_config;

    struct FakeRegistry(Vec<WatchdogModelInfo>);

    impl WatchdogModelRegistry for FakeRegistry {
        fn available(&self) -> Vec<WatchdogModelInfo> {
            self.0.clone()
        }
        fn find(&self, provider: &str, id: &str) -> Option<WatchdogModelInfo> {
            self.0
                .iter()
                .find(|m| m.provider == provider && m.id == id)
                .cloned()
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
        let bare = resolve_watchdog_review_model(&ctx, &config, &AmbientReviewAuth, None).unwrap();
        assert_eq!(bare.thinking_level, "off");
    }

    #[test]
    fn a_model_suffix_beats_the_configured_level_which_beats_the_context() {
        assert_eq!(
            resolve_review_thinking(
                "p/m:xhigh",
                Some(&ThinkingSetting::Level("low".into())),
                true,
                Some("high")
            )
            .unwrap(),
            "xhigh"
        );
        assert_eq!(
            resolve_review_thinking(
                "p/m",
                Some(&ThinkingSetting::Level("low".into())),
                true,
                Some("high")
            )
            .unwrap(),
            "low"
        );
        assert_eq!(
            resolve_review_thinking("p/m", Some(&ThinkingSetting::Off), true, Some("high"))
                .unwrap(),
            "off"
        );
        assert_eq!(
            resolve_review_thinking("p/m", None, true, Some("high")).unwrap(),
            "high"
        );
        assert_eq!(
            resolve_review_thinking("p/m", None, false, Some("high")).unwrap(),
            "off"
        );
    }

    #[test]
    fn an_unrecognized_configured_level_is_reported_with_reviews_own_message() {
        let err = resolve_review_thinking(
            "p/m",
            Some(&ThinkingSetting::Level("turbo".into())),
            false,
            None,
        )
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
        let err = resolve_watchdog_review_model(
            &ctx,
            &default_watchdog_config(),
            &AmbientReviewAuth,
            None,
        )
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
    fn warn_parameters_default_category_and_always_source_main() {
        let warning = to_watchdog_warning(&json!({
            "severity": "blocker",
            "importance": "high",
            "summary": " s ",
            "evidence": " e ",
            "recommendedAction": " a ",
        }))
        .unwrap();
        assert_eq!(warning.severity, WatchdogSeverity::Blocker);
        assert_eq!(warning.importance, WatchdogImportance::High);
        assert_eq!(warning.category, Some(WatchdogCategory::Other));
        assert_eq!(warning.source, Some(WatchdogWarningSource::Main));
        assert_eq!(warning.summary, "s", "fields are trimmed");
    }

    /// SUBA-120 test (b). `importance` is REQUIRED with NO default (`review.ts:30` declares it as a
    /// plain, non-`Optional` TypeBox property). A call that omits it is a tool-call ERROR.
    ///
    /// The field it replaced, `confidence`, silently defaulted to `medium` here. Carrying that
    /// default forward would have meant every unlabelled finding routes as non-`high`, i.e. the
    /// parent model silently stops receiving findings the review meant for it — a default is the one
    /// thing this field must not have.
    #[test]
    fn a_warn_call_omitting_importance_is_rejected_rather_than_defaulted() {
        let err = to_watchdog_warning(&json!({
            "severity": "blocker",
            "summary": "s",
            "evidence": "e",
            "recommendedAction": "a",
        }))
        .expect_err("a missing importance is a tool-call error");
        assert_eq!(
            err,
            "watchdog_warn.importance must be low, medium, or high."
        );
    }

    /// …and an importance outside the enum is likewise rejected, not coerced.
    #[test]
    fn a_warn_call_with_an_unknown_importance_is_rejected() {
        let err = to_watchdog_warning(&json!({
            "severity": "blocker",
            "importance": "critical",
            "summary": "s",
            "evidence": "e",
            "recommendedAction": "a",
        }))
        .expect_err("an unknown importance is a tool-call error");
        assert_eq!(
            err,
            "watchdog_warn.importance 'critical' is not a known importance."
        );
    }

    /// The schema advertises `importance` as required, with upstream's verbatim description — that
    /// text is prompt the review model reads, so a paraphrase is a behaviour change.
    #[test]
    fn the_warn_schema_requires_importance_and_carries_upstream_description() {
        let schema = watchdog_warn_parameters_schema();
        let required = schema["required"].as_array().expect("required array");
        assert!(required.iter().any(|v| v == "importance"), "{schema}");
        assert!(
            !required.iter().any(|v| v == "confidence"),
            "confidence is gone from the type entirely"
        );
        assert_eq!(
            schema["properties"]["importance"]["description"],
            json!(
                "low or medium for user-only visibility; high when the parent model must receive the finding"
            )
        );
        assert_eq!(
            schema["properties"]["importance"]["enum"],
            json!(["low", "medium", "high"])
        );
        assert!(schema["properties"].get("confidence").is_none(), "{schema}");
    }

    #[test]
    fn a_blank_required_field_is_rejected_per_field() {
        for (field, payload) in [
            (
                "summary",
                json!({ "severity": "concern", "importance": "high", "summary": "  ", "evidence": "e", "recommendedAction": "a" }),
            ),
            (
                "evidence",
                json!({ "severity": "concern", "importance": "high", "summary": "s", "evidence": "", "recommendedAction": "a" }),
            ),
            (
                "recommendedAction",
                json!({ "severity": "concern", "importance": "high", "summary": "s", "evidence": "e", "recommendedAction": " " }),
            ),
        ] {
            assert_eq!(
                to_watchdog_warning(&payload).unwrap_err(),
                format!("watchdog_warn.{field} must be a non-empty string.")
            );
        }
        assert!(to_watchdog_warning(&json!({ "severity": "nope" })).is_err());
        assert!(to_watchdog_warning(&json!("string")).is_err());
    }

    /// SUBA-233 — the reviewer is told about `watchdog_diff` only when it has it, in upstream's
    /// words (`review.ts:231` @ad11b7ab); otherwise the read-only line is unchanged.
    #[test]
    fn the_system_prompt_names_watchdog_diff_only_when_the_reviewer_has_it() {
        let with = build_watchdog_system_prompt("/repo", false, true, "");
        assert!(with.contains(
            "You are read-only. You may use read, grep, find, ls, and watchdog_diff (the full repo \
             diff since the session baseline; pass a path to narrow it). Do not edit files, run \
             shell commands, spawn agents, or mutate state."
        ));
        let without = build_watchdog_system_prompt("/repo", false, false, "");
        assert!(without.contains(
            "You are read-only. You may use read, grep, find, and ls. Do not edit files, run shell \
             commands, spawn agents, or mutate state."
        ));
        assert!(!without.contains("watchdog_diff"));
    }

    /// SUBA-233 — with a baseline the production turn offers `watchdog_diff`, described verbatim;
    /// without one it does not (the control is
    /// `the_production_review_turn_installs_the_expose_list_and_the_execution_time_policy`).
    #[test]
    fn with_a_baseline_the_production_turn_offers_watchdog_diff() {
        let emitter = WatchdogWarningEmitter::inert();
        let selection = WatchdogReviewModelSelection {
            model: WatchdogModelInfo::new("anthropic", "claude-opus-4-8"),
            thinking_level: "off".to_string(),
            auth: WatchdogReviewAuth::default(),
            explicit: true,
        };
        let agent = ModelTurnReviewAgent::new(std::env::temp_dir(), Arc::new(|| None));
        let request = agent.turn_request(WatchdogReviewTurn {
            selection: &selection,
            system_prompt: "system".to_string(),
            prompt: "prompt".to_string(),
            allowed_tools: &WATCHDOG_ALLOWED_TOOL_NAMES,
            read_only_tools: &WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES,
            warn_tool: WatchdogWarnTool::new(&emitter),
            diff_baseline: Some(super::super::diff_tool::WatchdogDiffBaseline {
                root: "/repo".to_string(),
                reference: "0123456789abcdef".to_string(),
            }),
            tool_execution_sequential: true,
            emit_warning: &emitter,
            cancel: CancelToken::new(),
            clarification: None,
        });
        let diff = request
            .tools
            .iter()
            .find(|tool| tool.name() == "watchdog_diff")
            .expect("the reviewer is given watchdog_diff");
        assert_eq!(
            diff.description(),
            "Show the repository diff since the review baseline, plus untracked file paths, \
             including edits by other sessions in this checkout. Optional path narrows it; \
             stat:true returns per-file counts only."
        );
        assert_eq!((request.block_reason)("watchdog_diff"), None);
    }

    #[test]
    fn the_system_prompt_includes_the_scope_line_only_when_there_is_scope() {
        let without = build_watchdog_system_prompt("/repo", false, false, "");
        assert!(!without.contains("Use the Current scope record"));
        assert!(without.contains("Working directory: /repo"));
        assert_eq!(without.lines().count(), 9);
        let with = build_watchdog_system_prompt("/repo", true, false, "");
        // The pin's line (`review.ts:230` @ad11b7ab), verbatim.
        assert!(with.contains(
            "Use the Current scope record alongside supplied activity evidence; an unrelated user \
             question does not cancel older authorized work."
        ));
        assert_eq!(with.lines().count(), 10);
        // The scope line sits fourth, immediately after the review-scope instruction.
        assert!(
            with.lines()
                .nth(3)
                .unwrap()
                .starts_with("Use the Current scope record")
        );
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
            diff_baseline: Default::default(),
            allow_clarification: false,
        };
        let prompt = build_review_prompt(&request, &selection);
        assert!(prompt.contains(
            "Review id: 7; epoch: 3; review model: anthropic/claude-opus-4-8; thinking: high."
        ));
        assert!(prompt.contains("<turn_delta>\n\nthe delta\n\n</turn_delta>"));
        assert_eq!(
            normalize_model_segment(&selection.model.provider),
            "anthropic"
        );
    }

    #[test]
    fn the_warn_result_text_tells_the_model_whether_it_landed() {
        assert_eq!(
            watchdog_warn_result_text(true),
            "Watchdog warning recorded."
        );
        assert!(watchdog_warn_result_text(false).contains("stale, duplicate, or over budget"));
        assert!(watchdog_warn_tool_description().starts_with("Emit one actionable"));
        let schema = watchdog_warn_parameters_schema();
        assert_eq!(schema["additionalProperties"], json!(false));
        // SUBA-120: five, not four — `importance` (`review.ts:30`) is required.
        assert_eq!(schema["required"].as_array().unwrap().len(), 5);
    }

    /// `:271-274` subtracts `watchdog_warn` from the tools the agent is GIVEN, while `:285` keeps
    /// it in the set `beforeToolCall` permits. Two different lists; asserting one against the other
    /// is what keeps them from drifting into one.
    #[test]
    fn the_expose_list_is_the_permit_list_minus_the_warn_tool() {
        // Minus `watchdog_diff` too: like the warn tool it is built per review, over the session's
        // baseline, and only when there is one (SUBA-233).
        let mut expected: Vec<&str> = WATCHDOG_ALLOWED_TOOL_NAMES
            .iter()
            .copied()
            .filter(|name| {
                *name != WATCHDOG_WARN_TOOL_NAME
                    && *name != super::super::diff_tool::WATCHDOG_DIFF_TOOL_NAME
            })
            .collect();
        expected.sort_unstable();
        let mut actual: Vec<&str> = WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES.to_vec();
        actual.sort_unstable();
        assert_eq!(actual, expected);
        // The permit list still contains it — the review must be allowed to CALL what it is not
        // handed a duplicate of.
        assert!(WATCHDOG_ALLOWED_TOOL_NAMES.contains(&WATCHDOG_WARN_TOOL_NAME));
        assert!(!WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES.contains(&WATCHDOG_WARN_TOOL_NAME));
        assert_eq!(
            watchdog_tool_call_block_reason(WATCHDOG_WARN_TOOL_NAME),
            None
        );
    }

    /// **The production construction, pinned.** [`WatchdogReviewAgent::run`] on
    /// [`ModelTurnReviewAgent`] builds nothing of its own — both halves of the two-layer tool
    /// policy come out of [`ModelTurnReviewAgent::turn_request`] — so this drives the EXACT
    /// [`super::agent_turn::WatchdogTurnRequest`] a real review turn is built with, rather than a
    /// tool list and a closure the test itself declared.
    ///
    /// Layer one (`review.ts:305-306 @v0.68.0`): the tools the agent is GIVEN are the read-only
    /// four plus the bound `watchdog_warn`, and nothing else.
    ///
    /// Layer two (`:338-340`): the execution-time refusal, checked against the wider PERMIT list.
    /// Replacing [`ModelTurnReviewAgent::tool_call_block_reason`]'s body with
    /// `Arc::new(|_| None)` — which is exactly what deleting the execution-time layer looks like —
    /// fails the three `block_reason` assertions below.
    #[test]
    fn the_production_review_turn_installs_the_expose_list_and_the_execution_time_policy() {
        let emitter = WatchdogWarningEmitter::inert();
        let selection = WatchdogReviewModelSelection {
            model: WatchdogModelInfo::new("anthropic", "claude-opus-4-8"),
            thinking_level: "off".to_string(),
            auth: WatchdogReviewAuth::default(),
            explicit: true,
        };
        let agent = ModelTurnReviewAgent::new(std::env::temp_dir(), Arc::new(|| None));
        let request = agent.turn_request(WatchdogReviewTurn {
            selection: &selection,
            system_prompt: "system".to_string(),
            prompt: "prompt".to_string(),
            allowed_tools: &WATCHDOG_ALLOWED_TOOL_NAMES,
            read_only_tools: &WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES,
            warn_tool: WatchdogWarnTool::new(&emitter),
            diff_baseline: None,
            tool_execution_sequential: true,
            emit_warning: &emitter,
            cancel: CancelToken::new(),
            clarification: None,
        });

        let mut offered: Vec<&str> = request.tools.iter().map(|tool| tool.name()).collect();
        offered.sort_unstable();
        assert_eq!(
            offered,
            vec!["find", "grep", "ls", "read", "watchdog_warn"],
            "the EXPOSE list is the read-only four plus the one bound warn tool"
        );

        // The execution-time layer, read off the value production installed on the turn.
        assert_eq!((request.block_reason)("read"), None);
        assert_eq!((request.block_reason)(WATCHDOG_WARN_TOOL_NAME), None);
        assert_eq!(
            (request.block_reason)("write").as_deref(),
            Some("Watchdog reviews are read-only; tool 'write' is not allowed."),
            "a reviewer that names `write` must be refused at execution time, not merely un-offered"
        );
        assert_eq!(
            (request.block_reason)("bash").as_deref(),
            Some("Watchdog reviews are read-only; tool 'bash' is not allowed.")
        );
        assert_eq!(
            (request.block_reason)("subagent").as_deref(),
            Some("Watchdog reviews are read-only; tool 'subagent' is not allowed.")
        );
    }

    /// Every field `createWatchdogWarnTool` sets (`:182-190`) must survive to a bound agent — the
    /// description in particular is prompt text the model reads to decide whether to warn at all.
    #[test]
    fn the_warn_tool_descriptor_carries_every_upstream_field() {
        let emitter = WatchdogWarningEmitter::inert();
        let tool = WatchdogWarnTool::new(&emitter);
        assert_eq!(tool.name(), "watchdog_warn");
        assert_eq!(tool.label(), "Watchdog warning");
        assert_eq!(tool.description(), watchdog_warn_tool_description());
        assert!(tool.description().contains("Do not use for nits"));
        assert_eq!(tool.parameters(), watchdog_warn_parameters_schema());
    }

    /// `execute` (`:191-203`) is the whole pipeline: validate, emit, and report which of the two
    /// result texts applies. A REJECTED warning must still be a successful tool call carrying the
    /// "was ignored" text — not an error — or the model re-sends it.
    #[test]
    fn the_warn_tool_executes_the_full_emit_pipeline() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::<WatchdogWarning>::new()));
        let sink = Arc::clone(&seen);
        let emitter = WatchdogWarningEmitter::from_fn(Arc::new(move |warning| {
            sink.lock().unwrap().push(warning.clone());
            warning.severity == WatchdogSeverity::Blocker
        }));
        let tool = WatchdogWarnTool::new(&emitter);

        let taken = tool
            .execute(&json!({
                "severity": "blocker",
                "importance": "high",
                "summary": "s",
                "evidence": "e",
                "recommendedAction": "r",
            }))
            .unwrap();
        assert!(taken.accepted);
        assert_eq!(taken.text, "Watchdog warning recorded.");

        let refused = tool
            .execute(&json!({
                "severity": "concern",
                "importance": "high",
                "summary": "s2",
                "evidence": "e2",
                "recommendedAction": "r2",
            }))
            .unwrap();
        assert!(!refused.accepted);
        assert!(refused.text.contains("stale, duplicate, or over budget"));

        // Both reached the emitter, with `toWatchdogWarning`'s defaults applied (`:170-172`).
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].source, Some(WatchdogWarningSource::Main));
        assert_eq!(seen[0].importance, WatchdogImportance::High);
        assert_eq!(seen[0].category, Some(WatchdogCategory::Other));

        // A malformed call is an Err and must NOT reach the emitter.
        assert!(
            tool.execute(&json!({ "severity": "blocker", "importance": "high", "summary": "  " }))
                .is_err()
        );
        assert_eq!(seen.len(), 2);
    }

    /// The turn a REAL review hands its agent must carry the descriptor and both lists — this is
    /// the production construction site (`MainWatchdogReview::review`), reached through the seam.
    #[tokio::test]
    async fn the_production_turn_carries_the_warn_tool_and_both_tool_lists() {
        /// Everything the turn hands a bound agent, in the shape a test can compare.
        struct SeenTurn {
            warn_tool_name: String,
            warn_tool_description: String,
            read_only_tools: Vec<String>,
            allowed_tools: Vec<String>,
            sequential: bool,
        }

        struct Capturing(Arc<std::sync::Mutex<Option<SeenTurn>>>);

        #[async_trait]
        impl WatchdogReviewAgent for Capturing {
            async fn run(&self, turn: WatchdogReviewTurn<'_>) -> Result<Vec<Value>, String> {
                // A bound agent can now build upstream's tool list from the turn alone.
                let result = turn
                    .warn_tool
                    .execute(&json!({
                        "severity": "concern",
                        "importance": "high",
                        "summary": "s",
                        "evidence": "e",
                        "recommendedAction": "r",
                    }))
                    .unwrap();
                assert!(
                    result.accepted,
                    "the emitter this turn was built with accepts"
                );
                *self.0.lock().unwrap() = Some(SeenTurn {
                    warn_tool_name: turn.warn_tool.name().to_string(),
                    warn_tool_description: turn.warn_tool.description(),
                    read_only_tools: turn
                        .read_only_tools
                        .iter()
                        .map(|t| (*t).to_string())
                        .collect(),
                    allowed_tools: turn
                        .allowed_tools
                        .iter()
                        .map(|t| (*t).to_string())
                        .collect(),
                    sequential: turn.tool_execution_sequential,
                });
                // `beforeToolCall` is reachable from the turn, not only as a free function.
                assert_eq!(turn.block_reason("read"), None);
                assert!(turn.block_reason("bash").unwrap().contains("read-only"));
                Ok(Vec::new())
            }
        }

        let captured = Arc::new(std::sync::Mutex::new(None));
        let review = MainWatchdogReview::new(
            Arc::new(registry()),
            Arc::new(AmbientReviewAuth),
            Arc::new(Capturing(Arc::clone(&captured))),
            std::path::PathBuf::from("/repo"),
        )
        .with_current_model(Some(WatchdogModelInfo::new("anthropic", "claude-opus-4-8")));

        let emitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&emitted);
        review
            .review(WatchdogReviewRequest {
                delta: "d".into(),
                config: default_watchdog_config(),
                review_id: 1,
                epoch: 1,
                has_scope: false,
                emit_warning: WatchdogWarningEmitter::from_fn(Arc::new(move |_| {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    true
                })),
                cancel: CancelToken::new(),
                diff_baseline: Default::default(),
                allow_clarification: false,
            })
            .await
            .unwrap();

        // The tool the agent ran was bound to THIS request's emitter (`:193`).
        assert_eq!(emitted.load(std::sync::atomic::Ordering::SeqCst), 1);
        let guard = captured.lock().unwrap();
        let seen = guard.as_ref().expect("the agent ran");
        assert_eq!(seen.warn_tool_name, "watchdog_warn");
        assert!(
            seen.warn_tool_description
                .starts_with("Emit one actionable")
        );
        assert_eq!(seen.read_only_tools, vec!["read", "grep", "find", "ls"]);
        assert_eq!(
            seen.allowed_tools,
            vec![
                "read",
                "grep",
                "find",
                "ls",
                "watchdog_warn",
                "watchdog_diff"
            ]
        );
        // `toolExecution: "sequential"` (`:288`), and the warn tool's own `executionMode`
        // (`:190`) — the turn must carry the SAME answer the descriptor's constant gives.
        assert_eq!(seen.sequential, WatchdogWarnTool::SEQUENTIAL);
        assert!(seen.sequential, "a review's tools must not interleave");
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
                diff_baseline: Default::default(),
                allow_clarification: false,
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
                diff_baseline: Default::default(),
                allow_clarification: false,
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.stop_reason, Some(ReviewStopReason::Stop));
        assert!(
            result.warnings.is_empty(),
            "warnings stream through the emitter"
        );
    }

    fn default_request() -> WatchdogReviewRequest {
        WatchdogReviewRequest {
            delta: "d".into(),
            epoch: 1,
            has_scope: false,
            review_id: 1,
            // `DEFAULT_WATCHDOG_CONFIG` sets no `main.model` (`settings.ts:72-96`), so this is the
            // config EVERY unconfigured session reviews under, and it can only resolve through the
            // live session model.
            config: default_watchdog_config(),
            emit_warning: WatchdogWarningEmitter::inert(),
            cancel: CancelToken::new(),
            diff_baseline: Default::default(),
            allow_clarification: false,
        }
    }

    /// The defect this seam closes: with no session provider AND no fixed model, the default
    /// config resolves nothing at all — which is what a production runtime built with
    /// `BuiltinWatchdogModelRegistry::new(None)` and no live model did at every boundary.
    #[tokio::test]
    async fn the_default_config_with_no_session_model_at_all_fails_the_review() {
        let review = MainWatchdogReview::new(
            Arc::new(registry()),
            Arc::new(AmbientReviewAuth),
            Arc::new(NoTurnReviewAgent),
            std::path::PathBuf::from("/repo"),
        );
        let error = review.review(default_request()).await.unwrap_err();
        assert!(
            error.contains("the current Pi session model is unavailable"),
            "{error}"
        );
    }

    /// The live provider is consulted at REVIEW time, not at construction: the same review object
    /// resolves whatever the session is currently on.
    #[tokio::test]
    async fn a_bound_session_provider_supplies_the_model_the_default_config_inherits() {
        let model: Arc<std::sync::Mutex<Option<WatchdogModelInfo>>> =
            Arc::new(std::sync::Mutex::new(None));
        let provider_model = Arc::clone(&model);
        let review = MainWatchdogReview::new(
            Arc::new(registry()),
            Arc::new(AmbientReviewAuth),
            Arc::new(NoTurnReviewAgent),
            std::path::PathBuf::from("/repo"),
        )
        .with_session_context(Arc::new(move || {
            Some(WatchdogSessionContext {
                model: provider_model.lock().unwrap().clone(),
                thinking_level: Some("high".to_string()),
            })
        }));

        // Session start has not bound a model yet: the review still fails loudly rather than
        // reviewing nothing.
        assert!(
            review
                .review(default_request())
                .await
                .unwrap_err()
                .contains("the current Pi session model is unavailable")
        );

        // The session picks a model; the SAME review object now resolves it.
        *model.lock().unwrap() = Some(WatchdogModelInfo::new("anthropic", "claude-opus-4-8"));
        let result = review.review(default_request()).await.unwrap().unwrap();
        assert_eq!(result.stop_reason, Some(ReviewStopReason::Stop));
    }

    /// The FIXED-value fallback pair ([`MainWatchdogReview::with_current_model`] /
    /// [`MainWatchdogReview::with_current_thinking_level`]) is what an embedder with no live
    /// session provider gets. Both production paths bind `with_session_context` instead, so this
    /// is the only place the `.or_else()` fallback in `review()` is exercised — and it must lose to
    /// a provider that answers, exactly as upstream's live `ctx` beats the fixed value.
    #[tokio::test]
    async fn the_fixed_fallback_pair_applies_only_when_no_session_provider_answers() {
        /// Records the selection the turn was actually built with.
        struct Capture(Arc<std::sync::Mutex<Option<WatchdogReviewModelSelection>>>);

        #[async_trait]
        impl WatchdogReviewAgent for Capture {
            async fn run(&self, turn: WatchdogReviewTurn<'_>) -> Result<Vec<Value>, String> {
                *self.0.lock().unwrap() = Some(turn.selection.clone());
                Ok(Vec::new())
            }
        }

        // No provider at all: the fixed values ARE the session, and the fixed thinking level
        // reaches the inherited review model (`allowContextThinking: true`, `:154`).
        let seen = Arc::new(std::sync::Mutex::new(None));
        let unbound = MainWatchdogReview::new(
            Arc::new(registry()),
            Arc::new(AmbientReviewAuth),
            Arc::new(Capture(Arc::clone(&seen))),
            std::path::PathBuf::from("/repo"),
        )
        .with_current_model(Some(WatchdogModelInfo::new("anthropic", "claude-opus-4-8")))
        .with_current_thinking_level(Some("high".to_string()));
        unbound.review(default_request()).await.unwrap();
        let selection = seen.lock().unwrap().clone().expect("the turn ran");
        assert_eq!(selection.model.id, "claude-opus-4-8");
        assert_eq!(
            selection.thinking_level, "high",
            "the fixed level was consulted"
        );

        // A provider that ANSWERS wins over both fixed values.
        let seen = Arc::new(std::sync::Mutex::new(None));
        let bound = MainWatchdogReview::new(
            Arc::new(registry()),
            Arc::new(AmbientReviewAuth),
            Arc::new(Capture(Arc::clone(&seen))),
            std::path::PathBuf::from("/repo"),
        )
        .with_current_model(Some(WatchdogModelInfo::new("anthropic", "claude-opus-4-8")))
        .with_current_thinking_level(Some("high".to_string()))
        .with_session_context(Arc::new(|| {
            Some(WatchdogSessionContext {
                model: Some(WatchdogModelInfo::new("anthropic", "claude-sonnet-4-5")),
                thinking_level: Some("low".to_string()),
            })
        }));
        bound.review(default_request()).await.unwrap();
        let selection = seen.lock().unwrap().clone().expect("the turn ran");
        assert_eq!(
            selection.model.id, "claude-sonnet-4-5",
            "the live session beat the fixed model"
        );
        assert_eq!(
            selection.thinking_level, "low",
            "and the fixed thinking level too"
        );
    }

    /// A bound provider that answers `None` is upstream's missing `ExtensionContext`
    /// (`review.ts:257`) — an error, not a silent clean review.
    #[tokio::test]
    async fn a_provider_with_no_live_context_reports_upstreams_error() {
        let review = MainWatchdogReview::new(
            Arc::new(registry()),
            Arc::new(AmbientReviewAuth),
            Arc::new(NoTurnReviewAgent),
            std::path::PathBuf::from("/repo"),
        )
        .with_current_model(Some(WatchdogModelInfo::new("anthropic", "claude-opus-4-8")))
        .with_session_context(Arc::new(|| None));
        assert_eq!(
            review.review(default_request()).await.unwrap_err(),
            "Main watchdog review cannot run without an active Pi extension context."
        );
    }

    /// The session's reasoning level travels with the model on the INHERITED arm
    /// (`review.ts:153` — `allowContextThinking: true`).
    #[tokio::test]
    async fn the_session_thinking_level_reaches_the_inherited_review_model() {
        let registry = registry();
        let ctx = WatchdogModelContext::new(&registry)
            .with_current_model(Some(WatchdogModelInfo::new("anthropic", "claude-opus-4-8")));
        let selection = resolve_watchdog_review_model(
            &ctx,
            &default_watchdog_config(),
            &AmbientReviewAuth,
            Some("xhigh"),
        )
        .unwrap();
        assert_eq!(selection.thinking_level, "xhigh");
    }

    // ---- SUBA-234: `watchdog_ask` and `WATCHDOG.md` -------------------------------------------

    fn ask_request(
        allow: bool,
        emit: WatchdogWarningEmitter,
        config: ResolvedWatchdogConfig,
    ) -> WatchdogReviewRequest {
        WatchdogReviewRequest {
            delta: "d".into(),
            epoch: 1,
            has_scope: false,
            review_id: 1,
            config,
            emit_warning: emit,
            cancel: CancelToken::new(),
            diff_baseline: Default::default(),
            allow_clarification: allow,
        }
    }

    fn live_review(
        agent: Arc<dyn WatchdogReviewAgent>,
        cwd: std::path::PathBuf,
    ) -> MainWatchdogReview {
        MainWatchdogReview::new(
            Arc::new(registry()),
            Arc::new(AmbientReviewAuth),
            agent,
            cwd,
        )
        .with_current_model(Some(WatchdogModelInfo::new("anthropic", "claude-opus-4-8")))
    }

    /// Plays the model's ONE mixed batch upstream's adapter test sends (`watchdog-review.test.ts`
    /// @ad11b7ab, "yields an ask from a mixed batch without another model call or warning"):
    /// `watchdog_ask`, then `watchdog_warn`, then `ls` — through the turn's own tools and policy.
    struct MixedBatch(Arc<std::sync::Mutex<Vec<String>>>);

    #[async_trait]
    impl WatchdogReviewAgent for MixedBatch {
        async fn run(&self, turn: WatchdogReviewTurn<'_>) -> Result<Vec<Value>, String> {
            let mut log = self.0.lock().unwrap();
            let latch = turn
                .clarification
                .clone()
                .expect("an allowed review carries the latch");
            log.push(format!(
                "ask permitted: {}",
                turn.block_reason(WATCHDOG_ASK_TOOL_NAME).is_none()
            ));
            log.push(
                latch
                    .ask(&json!({
                        "question": "  Which constraint applies?  ",
                        "evidence": "Two scope statements differ.",
                    }))?
                    .to_string(),
            );
            log.push(format!("turn cancelled: {}", turn.cancel.is_cancelled()));
            let warn = turn.warn_tool.execute(&json!({
                "severity": "concern", "importance": "high", "summary": "Must not emit",
                "evidence": "x", "recommendedAction": "x",
            }))?;
            log.push(format!("warning accepted: {}", warn.accepted));
            log.push(format!("ls blocked: {}", turn.block_reason("ls").is_some()));
            log.push(format!(
                "second ask: {}",
                latch
                    .ask(&json!({ "question": "again?", "evidence": "e" }))
                    .unwrap_err()
            ));
            // The yield aborted the turn: its terminal message says so, and that must not turn a
            // question into an aborted review.
            Ok(vec![
                json!({ "role": "assistant", "stopReason": "aborted" }),
            ])
        }
    }

    /// Clause 3, the reviewer half — the question YIELDS the review: it is reported as the result
    /// (no stop reason), the turn's token is cancelled (upstream's `agent.abort()`), a warning in
    /// the same batch is refused before it reaches the runtime, every later tool is blocked, and
    /// a second ask is refused.
    ///
    /// RED at HEAD: does not compile — no `watchdog_ask`, no latch, no `clarification` result.
    #[tokio::test]
    async fn watchdog_ask_yields_the_review_and_refuses_the_rest_of_the_batch() {
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let review = live_review(Arc::new(MixedBatch(Arc::clone(&log))), "/repo".into());
        let emitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&emitted);
        let request = ask_request(
            true,
            WatchdogWarningEmitter::from_fn(Arc::new(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                true
            })),
            default_watchdog_config(),
        );
        let review_cancel = request.cancel.clone();
        let result = review.review(request).await.unwrap().unwrap();
        assert_eq!(
            result.clarification,
            Some(WatchdogClarification {
                question: "Which constraint applies?".into(),
                evidence: "Two scope statements differ.".into(),
            })
        );
        assert_eq!(result.stop_reason, None, "a yield is not an abort");
        assert!(result.warnings.is_empty());
        assert_eq!(
            emitted.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no warning reached the runtime"
        );
        assert!(
            !review_cancel.is_cancelled(),
            "the yield cancels the turn, not the review"
        );
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "ask permitted: true".to_string(),
                WATCHDOG_ASK_RESULT_TEXT.to_string(),
                "turn cancelled: true".to_string(),
                "warning accepted: false".to_string(),
                "ls blocked: true".to_string(),
                "second ask: Clarification unavailable after warning, yield, or cancellation."
                    .to_string(),
            ]
        );
    }

    /// The other order: a warning ACCEPTED first closes the question (upstream's `warned`), and
    /// empty halves are refused. RED at HEAD: does not compile.
    #[test]
    fn an_accepted_warning_closes_the_question_and_both_halves_are_required() {
        let latch = WatchdogClarificationLatch::new(CancelToken::new(), CancelToken::new());
        assert_eq!(
            latch
                .ask(&json!({ "question": "  ", "evidence": "e" }))
                .unwrap_err(),
            "A focused question and concrete evidence are required."
        );
        let emit = latch.wrap_emitter(WatchdogWarningEmitter::from_fn(Arc::new(|_| true)));
        assert!(
            emit.emit(
                &to_watchdog_warning(&json!({
                    "severity": "concern", "importance": "high", "summary": "s", "evidence": "e",
                    "recommendedAction": "r"
                }))
                .unwrap()
            )
        );
        assert!(
            latch
                .ask(&json!({ "question": "q", "evidence": "e" }))
                .is_err()
        );
        assert!(!latch.yielded());
    }

    /// A review NOT allowed to ask is built exactly as before: no `watchdog_ask` on the agent, the
    /// policy refuses it by name, and the turn runs under the request's own token. With the latch,
    /// the tool is there and permitted. RED at HEAD: does not compile.
    #[test]
    fn the_ask_tool_exists_only_on_a_review_allowed_to_ask() {
        let selection = WatchdogReviewModelSelection {
            model: WatchdogModelInfo::new("anthropic", "claude-opus-4-8"),
            thinking_level: "off".to_string(),
            auth: WatchdogReviewAuth::default(),
            explicit: true,
        };
        let emitter = WatchdogWarningEmitter::inert();
        let agent = ModelTurnReviewAgent::new(std::env::temp_dir(), Arc::new(|| None));
        for allowed in [false, true] {
            let latch = allowed
                .then(|| WatchdogClarificationLatch::new(CancelToken::new(), CancelToken::new()));
            let request = agent.turn_request(WatchdogReviewTurn {
                selection: &selection,
                system_prompt: "system".to_string(),
                prompt: "prompt".to_string(),
                allowed_tools: &WATCHDOG_ALLOWED_TOOL_NAMES,
                read_only_tools: &WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES,
                warn_tool: WatchdogWarnTool::new(&emitter),
                diff_baseline: None,
                tool_execution_sequential: true,
                emit_warning: &emitter,
                cancel: CancelToken::new(),
                clarification: latch,
            });
            let has_ask = request
                .tools
                .iter()
                .any(|t| t.name() == WATCHDOG_ASK_TOOL_NAME);
            assert_eq!(has_ask, allowed);
            assert_eq!(
                (request.block_reason)(WATCHDOG_ASK_TOOL_NAME).is_none(),
                allowed
            );
            assert_eq!((request.block_reason)("read"), None);
            assert!((request.block_reason)("bash").is_some());
            if let Some(ask) = request
                .tools
                .iter()
                .find(|t| t.name() == WATCHDOG_ASK_TOOL_NAME)
            {
                assert_eq!(ask.description(), WATCHDOG_ASK_TOOL_DESCRIPTION);
                assert_eq!(
                    ask.parameters()["required"],
                    json!(["question", "evidence"])
                );
            }
        }
    }

    /// Records the system prompt the review handed its turn.
    struct PromptCapture(Arc<std::sync::Mutex<Option<String>>>);

    #[async_trait]
    impl WatchdogReviewAgent for PromptCapture {
        async fn run(&self, turn: WatchdogReviewTurn<'_>) -> Result<Vec<Value>, String> {
            *self.0.lock().unwrap() = Some(turn.system_prompt);
            Ok(Vec::new())
        }
    }

    /// Clause 5 — `WATCHDOG.md` reaches the reviewer's system prompt when present
    /// (`review.ts:237`,`:325` @ad11b7ab), is absent-safe, and `guidance.watchdogMd: false`
    /// turns it off. Drives the REAL review, so the read happens where production reads it.
    ///
    /// RED at HEAD: does not compile — `build_watchdog_system_prompt` takes no guidance and
    /// nothing reads `WATCHDOG.md`; with the read removed from `MainWatchdogReview::review` it
    /// compiles and the first assertion fails.
    #[tokio::test]
    async fn watchdog_md_reaches_the_reviewer_and_is_absent_safe() {
        let project = tempfile::tempdir().unwrap();
        let captured = Arc::new(std::sync::Mutex::new(None));
        let review = live_review(
            Arc::new(PromptCapture(Arc::clone(&captured))),
            project.path().to_path_buf(),
        );
        let prompt = |config| {
            let review = &review;
            let captured = Arc::clone(&captured);
            async move {
                review
                    .review(ask_request(false, WatchdogWarningEmitter::inert(), config))
                    .await
                    .unwrap();
                captured.lock().unwrap().clone().unwrap()
            }
        };
        let without = prompt(default_watchdog_config()).await;
        assert!(
            without.contains("If the turn is clean"),
            "absent-safe: the review still runs"
        );

        let dir = crate::paths::project_config_dir(project.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("WATCHDOG.md"),
            "Never flag generated files under out/.\n",
        )
        .unwrap();
        let with = prompt(default_watchdog_config()).await;
        assert!(with.contains(
            "Standing instructions from WATCHDOG.md (project first, then user):\nNever flag generated files under out/."
        ), "{with}");

        let mut off = default_watchdog_config();
        off.guidance.watchdog_md = false;
        assert!(!prompt(off).await.contains("Never flag generated files"));
    }
}

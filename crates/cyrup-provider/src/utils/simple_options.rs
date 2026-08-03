//! The unified "simple" option surface (1:1 with Pi `api/simple-options.ts`).
//!
//! `streamSimple`/`completeSimple` accept a [`SimpleStreamOptions`] (a [`StreamOptions`] plus a
//! unified `reasoning` level and optional per-level token budgets) and lower it to a concrete
//! [`StreamOptions`], clamping `max_tokens` to the remaining context window and (for token-budget
//! providers) splitting the budget between thinking and output. Faithful port of
//! `simple-options.ts:12-77`.

use crate::context::Context;
use crate::model::Model;
use crate::stream::StreamOptions;
use crate::utils::estimate::estimate_context_tokens;
use cyrup_core::ThinkingLevel;

/// Tokens reserved as headroom when clamping `max_tokens` to the context window (Pi
/// `CONTEXT_SAFETY_TOKENS`, simple-options.ts:12).
const CONTEXT_SAFETY_TOKENS: i64 = 4096;
/// Floor for a clamped `max_tokens` (Pi `MIN_MAX_TOKENS`, simple-options.ts:13).
const MIN_MAX_TOKENS: i64 = 1;
/// Minimum output tokens kept when a thinking budget would consume the whole window
/// (Pi `minOutputTokens`, simple-options.ts:66).
const MIN_OUTPUT_TOKENS: u64 = 1024;

/// Per-level thinking token budgets (Pi `ThinkingBudgets`, types.ts:88-94). A `None` field falls
/// back to the built-in default for that level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThinkingBudgets {
    pub minimal: Option<u64>,
    pub low: Option<u64>,
    pub medium: Option<u64>,
    pub high: Option<u64>,
}

/// The "simple" per-request options (Pi `SimpleStreamOptions extends StreamOptions`, types.ts:290).
/// The unified [`StreamOptions`] is carried as `base`; `reasoning` is the unified on-level effort
/// and `thinking_budgets` overrides the default token budgets for token-budget providers.
#[derive(Clone, Default)]
pub struct SimpleStreamOptions {
    pub base: StreamOptions,
    pub reasoning: Option<ThinkingLevel>,
    /// Custom token budgets for thinking levels (token-based providers only).
    pub thinking_budgets: Option<ThinkingBudgets>,
}

/// Clamp `max_tokens` so the prompt + completion fit the model's context window, keeping a safety
/// margin (Pi `clampMaxTokensToContext`, simple-options.ts:15-19). A model with an unknown
/// (`0`) context window just floors `max_tokens` at [`MIN_MAX_TOKENS`].
pub fn clamp_max_tokens_to_context(model: &Model, context: &Context, max_tokens: u64) -> u64 {
    if model.context_window == 0 {
        return (max_tokens as i64).max(MIN_MAX_TOKENS) as u64;
    }
    let used = estimate_context_tokens(context).tokens as i64;
    let available = model.context_window as i64 - used - CONTEXT_SAFETY_TOKENS;
    (max_tokens as i64).min(available.max(MIN_MAX_TOKENS)) as u64
}

/// Lower a [`SimpleStreamOptions`] to a concrete [`StreamOptions`] (Pi `buildBaseOptions`,
/// simple-options.ts:21-45): clamps `max_tokens` (defaulting to the model cap) and threads through
/// every transport-level field. `api_key` (when non-empty) wins over `options.base.api_key`,
/// matching Pi's `apiKey || options?.apiKey`.
pub fn build_base_options(
    model: &Model,
    context: &Context,
    options: &SimpleStreamOptions,
    api_key: Option<&str>,
) -> StreamOptions {
    let requested = options.base.max_tokens.unwrap_or(model.max_tokens);
    let api_key = match api_key {
        Some(k) if !k.is_empty() => Some(k.to_string()),
        _ => options.base.api_key.clone(),
    };
    StreamOptions {
        temperature: options.base.temperature,
        max_tokens: Some(clamp_max_tokens_to_context(model, context, requested)),
        cancel: options.base.cancel.clone(),
        api_key,
        transport: options.base.transport,
        cache_retention: options.base.cache_retention,
        session_id: options.base.session_id.clone(),
        headers: options.base.headers.clone(),
        on_payload: options.base.on_payload.clone(),
        on_response: options.base.on_response.clone(),
        timeout_ms: options.base.timeout_ms,
        websocket_connect_timeout_ms: options.base.websocket_connect_timeout_ms,
        max_retries: options.base.max_retries,
        max_retry_delay_ms: options.base.max_retry_delay_ms,
        metadata: options.base.metadata.clone(),
        env: options.base.env.clone(),
        // Carried through so a downstream collection can map the unified level to provider thinking.
        reasoning: options.base.reasoning,
        tool_choice: options.base.tool_choice.clone(),
        // Per-level custom thinking budgets ride through to budget-based providers (Pi
        // `streamSimple` forwards `options.thinkingBudgets`, anthropic-messages.ts:792-797).
        thinking_budgets: options.thinking_budgets,
        // Per-API typed options ride through unchanged (Pi `streamSimple` preserves `options`).
        api_options: options.base.api_options.clone(),
    }
}

/// Clamp a reasoning effort to the levels token-budget providers understand (Pi `clampReasoning`,
/// simple-options.ts:48-49): `xhigh` AND `max` both collapse to `high`; every other level is
/// unchanged. Matched exhaustively (no `_` arm) so a future rung cannot silently pass through.
pub fn clamp_reasoning(effort: ThinkingLevel) -> ThinkingLevel {
    match effort {
        ThinkingLevel::Xhigh | ThinkingLevel::Max => ThinkingLevel::High,
        ThinkingLevel::Minimal => ThinkingLevel::Minimal,
        ThinkingLevel::Low => ThinkingLevel::Low,
        ThinkingLevel::Medium => ThinkingLevel::Medium,
        ThinkingLevel::High => ThinkingLevel::High,
    }
}

/// Split a max-token budget between thinking and output for token-budget providers (Pi
/// `adjustMaxTokensForThinking`, simple-options.ts:51-77).
///
/// `base_max_tokens == None` means the caller set no explicit cap: use the model cap and fit
/// thinking inside it. Otherwise `max_tokens = min(base + budget, model cap)`. If the resulting
/// window cannot hold both, the thinking budget shrinks to leave [`MIN_OUTPUT_TOKENS`] of output.
pub fn adjust_max_tokens_for_thinking(
    base_max_tokens: Option<u64>,
    model_max_tokens: u64,
    reasoning_level: ThinkingLevel,
    custom_budgets: Option<&ThinkingBudgets>,
) -> (u64, u64) {
    let level = clamp_reasoning(reasoning_level);
    let mut thinking_budget = budget_for_level(level, custom_budgets);

    let max_tokens = match base_max_tokens {
        None => model_max_tokens,
        Some(base) => base.saturating_add(thinking_budget).min(model_max_tokens),
    };

    if max_tokens <= thinking_budget {
        thinking_budget = max_tokens.saturating_sub(MIN_OUTPUT_TOKENS);
    }

    (max_tokens, thinking_budget)
}

/// The token budget for a (already-clamped) on-level, applying any custom override over the Pi
/// default (`minimal:1024, low:2048, medium:8192, high:16384`, simple-options.ts:58-64).
fn budget_for_level(level: ThinkingLevel, custom: Option<&ThinkingBudgets>) -> u64 {
    let (default, override_val) = match level {
        ThinkingLevel::Minimal => (1024, custom.and_then(|c| c.minimal)),
        ThinkingLevel::Low => (2048, custom.and_then(|c| c.low)),
        ThinkingLevel::Medium => (8192, custom.and_then(|c| c.medium)),
        // `clamp_reasoning` collapses `xhigh` and `max` to `high`; all three land here.
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => {
            (16384, custom.and_then(|c| c.high))
        }
    };
    override_val.unwrap_or(default)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::model::{Modality, ModelCost};
    use cyrup_core::Message;

    fn model(context_window: u64, max_tokens: u64) -> Model {
        Model {
            id: "m".into(),
            name: "M".into(),
            api: "openai-completions".into(),
            provider: "openai".into(),
            base_url: String::new(),
            reasoning: true,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window,
            max_tokens,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn ctx(text: &str) -> Context {
        Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![cyrup_core::Content::Text {
                    text: text.to_string(),
                    text_signature: None,
                }],
                timestamp: 0,
            }],
            tools: Vec::new(),
        }
    }

    #[test]
    fn clamp_respects_available_window() {
        let m = model(10_000, 4_096);
        // small prompt → available ≈ 10000 - used - 4096; request 4096 fits.
        assert_eq!(clamp_max_tokens_to_context(&m, &ctx("hi"), 4_096), 4_096);
    }

    #[test]
    fn clamp_floors_at_one_when_window_full() {
        let m = model(100, 4_096);
        // window smaller than the safety margin → available negative → floor at 1.
        assert_eq!(
            clamp_max_tokens_to_context(&m, &ctx("hello world"), 4_096),
            1
        );
    }

    #[test]
    fn clamp_unknown_window_floors_at_min() {
        let m = model(0, 4_096);
        assert_eq!(clamp_max_tokens_to_context(&m, &ctx("x"), 4_096), 4_096);
        assert_eq!(clamp_max_tokens_to_context(&m, &ctx("x"), 0), 1);
    }

    #[test]
    fn build_base_options_clamps_and_threads_fields() {
        let m = model(10_000, 8_000);
        let mut opts = SimpleStreamOptions::default();
        opts.base.temperature = Some(0.5);
        opts.base.transport = Some(crate::stream::Transport::Sse);
        opts.base.timeout_ms = Some(60_000);
        let out = build_base_options(&m, &ctx("hi"), &opts, Some("sk-123"));
        assert_eq!(out.temperature, Some(0.5));
        assert_eq!(out.transport, Some(crate::stream::Transport::Sse));
        assert_eq!(out.timeout_ms, Some(60_000));
        assert_eq!(out.api_key.as_deref(), Some("sk-123"));
        // defaults max_tokens to model cap then clamps to fit the window.
        assert!(out.max_tokens.unwrap() <= 8_000);
    }

    #[test]
    fn api_key_param_wins_over_option_when_nonempty() {
        let m = model(10_000, 100);
        let mut opts = SimpleStreamOptions::default();
        opts.base.api_key = Some("from-options".into());
        assert_eq!(
            build_base_options(&m, &ctx("x"), &opts, Some("from-param"))
                .api_key
                .as_deref(),
            Some("from-param")
        );
        // empty param falls back to the option.
        assert_eq!(
            build_base_options(&m, &ctx("x"), &opts, Some(""))
                .api_key
                .as_deref(),
            Some("from-options")
        );
    }

    #[test]
    fn clamp_reasoning_collapses_xhigh() {
        assert_eq!(clamp_reasoning(ThinkingLevel::Xhigh), ThinkingLevel::High);
        assert_eq!(clamp_reasoning(ThinkingLevel::Low), ThinkingLevel::Low);
    }

    #[test]
    fn adjust_default_budgets() {
        // No explicit cap: use model cap; budget = default for the level.
        assert_eq!(
            adjust_max_tokens_for_thinking(None, 32_000, ThinkingLevel::Medium, None),
            (32_000, 8_192)
        );
        assert_eq!(
            adjust_max_tokens_for_thinking(None, 32_000, ThinkingLevel::High, None),
            (32_000, 16_384)
        );
        // xhigh collapses to high's budget.
        assert_eq!(
            adjust_max_tokens_for_thinking(None, 32_000, ThinkingLevel::Xhigh, None),
            (32_000, 16_384)
        );
    }

    #[test]
    fn adjust_with_explicit_cap_adds_budget_then_clamps() {
        // base + budget under model cap.
        assert_eq!(
            adjust_max_tokens_for_thinking(Some(4_000), 32_000, ThinkingLevel::Low, None),
            (6_048, 2_048)
        );
        // base + budget over model cap → clamped to cap.
        assert_eq!(
            adjust_max_tokens_for_thinking(Some(30_000), 32_000, ThinkingLevel::High, None),
            (32_000, 16_384)
        );
    }

    #[test]
    fn adjust_shrinks_budget_to_keep_output() {
        // window <= budget → budget shrinks to leave MIN_OUTPUT_TOKENS of output.
        let (max_tokens, budget) =
            adjust_max_tokens_for_thinking(Some(0), 16_384, ThinkingLevel::High, None);
        // base 0 + 16384 = 16384 == model cap; max_tokens == 16384 <= budget 16384 → shrink.
        assert_eq!(max_tokens, 16_384);
        assert_eq!(budget, 16_384 - MIN_OUTPUT_TOKENS);
    }

    #[test]
    fn adjust_custom_budget_overrides_default() {
        let custom = ThinkingBudgets {
            medium: Some(5_000),
            ..ThinkingBudgets::default()
        };
        assert_eq!(
            adjust_max_tokens_for_thinking(None, 32_000, ThinkingLevel::Medium, Some(&custom)),
            (32_000, 5_000)
        );
    }
}

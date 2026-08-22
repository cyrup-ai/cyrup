//! Request encoding (pi `commandInput`, `bedrock-converse-stream.ts:230-241`).

use super::capabilities::{
    is_anthropic_claude_model, is_gov_cloud_bedrock_target, map_thinking_level_to_effort,
    supports_adaptive_thinking,
};
use super::convert::{build_system_prompt, convert_messages, convert_tool_config};
use super::env::EnvSource;
use super::options::{BedrockOptions, BedrockThinkingDisplay};
use crate::context::Context;
use crate::model::Model;
use crate::stream::{CacheRetention, StreamOptions};
use crate::utils::simple_options::{adjust_max_tokens_for_thinking, clamp_max_tokens_to_context};
use cyrup_core::ThinkingLevel;
use serde_json::{Map, Value, json};

/// pi's interleaved-thinking beta token (`bedrock-converse-stream.ts:1080`).
pub(super) const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// Build the `ConverseStreamCommand` input (pi `commandInput`,
/// `bedrock-converse-stream.ts:230-241`), including the `modelId` URI label so `onPayload` sees the
/// same object upstream hands it. [`split_command_input`] lifts `modelId` back out afterwards.
///
/// Returns `Err(message)` for the one throwing path upstream has on this route:
/// `createImageBlock`'s `Unknown image type: <mimeType>` (`:1106`).
pub(super) fn build_params(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    bedrock: &BedrockOptions,
    cache_retention: CacheRetention,
    env: &EnvSource<'_>,
) -> Result<Value, String> {
    let claude = is_anthropic_claude_model(model);
    let adaptive = supports_adaptive_thinking(model);
    let reasoning_on = opts.reasoning.is_on();

    // pi `streamSimple` (`:403-449`): only budget-based Claude models re-split `maxTokens` between
    // thinking and output; adaptive Claude and every non-Claude model pass the base cap through.
    let mut effective_max_tokens = opts.max_tokens;
    let mut budget_override: Option<u64> = None;
    if reasoning_on && claude && !adaptive {
        let level = opts.reasoning.level().unwrap_or(ThinkingLevel::High);
        let (adjusted, budget) = adjust_max_tokens_for_thinking(
            opts.max_tokens,
            model.max_tokens,
            level,
            opts.thinking_budgets.as_ref(),
        );
        let max_tokens = clamp_max_tokens_to_context(model, ctx, adjusted);
        effective_max_tokens = Some(max_tokens);
        budget_override = Some(budget.min(max_tokens.saturating_sub(1024)));
    }

    // pi `:229`: `options.maxTokens ?? (isAnthropicClaudeModel(model) ? model.maxTokens : undefined)`.
    let inference_max_tokens = effective_max_tokens.or(if claude {
        Some(model.max_tokens)
    } else {
        None
    });

    let mut obj = Map::new();
    obj.insert("modelId".to_string(), json!(model.id.as_str()));
    obj.insert(
        "messages".to_string(),
        Value::Array(convert_messages(ctx, model, cache_retention, env)?),
    );
    if let Some(system) = build_system_prompt(ctx.system_prompt.as_deref(), model, cache_retention, env) {
        obj.insert("system".to_string(), Value::Array(system));
    }

    let mut inference = Map::new();
    if let Some(max_tokens) = inference_max_tokens {
        inference.insert("maxTokens".to_string(), json!(max_tokens));
    }
    if let Some(temperature) = opts.temperature {
        inference.insert("temperature".to_string(), json!(temperature));
    }
    obj.insert("inferenceConfig".to_string(), Value::Object(inference));

    // pi `:238` reads `model.compat?.supportsStrictMode ?? false` at the call site.
    let supports_strict_mode = model
        .compat
        .as_ref()
        .and_then(|c| c.supports_strict_mode)
        .unwrap_or(false);
    if let Some(tool_config) =
        convert_tool_config(&ctx.tools, bedrock.tool_choice.as_ref(), supports_strict_mode)
            .map_err(|e| e.0)?
    {
        obj.insert("toolConfig".to_string(), tool_config);
    }
    if let Some(extra) =
        build_additional_model_request_fields(model, opts, bedrock, env, budget_override)
    {
        obj.insert("additionalModelRequestFields".to_string(), extra);
    }
    if let Some(metadata) = &bedrock.request_metadata {
        let map: Map<String, Value> = metadata
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        obj.insert("requestMetadata".to_string(), Value::Object(map));
    }

    Ok(Value::Object(obj))
}

/// pi `resolveCacheRetention` (`bedrock-converse-stream.ts:640-648`): explicit wins, else
/// `PI_CACHE_RETENTION=long` promotes, else `"short"`.
pub(super) fn resolve_cache_retention(
    cache_retention: Option<CacheRetention>,
    env: &EnvSource<'_>,
) -> CacheRetention {
    if let Some(c) = cache_retention {
        return c;
    }
    if env.get("PI_CACHE_RETENTION").as_deref() == Some("long") {
        return CacheRetention::Long;
    }
    CacheRetention::Short
}

/// pi `buildAdditionalModelRequestFields` (`bedrock-converse-stream.ts:1039-1087`).
fn build_additional_model_request_fields(
    model: &Model,
    opts: &StreamOptions,
    bedrock: &BedrockOptions,
    env: &EnvSource<'_>,
    budget_override: Option<u64>,
) -> Option<Value> {
    if !opts.reasoning.is_on() || !model.reasoning {
        return None;
    }
    if !is_anthropic_claude_model(model) {
        return None;
    }
    let level = opts.reasoning.level().unwrap_or(ThinkingLevel::High);

    // pi `:1048-1050`: GovCloud's Converse schema rejects `thinking.display`.
    let display = if is_gov_cloud_bedrock_target(model, bedrock, env) {
        None
    } else {
        Some(
            bedrock
                .thinking_display
                .map(BedrockThinkingDisplay::as_wire)
                .unwrap_or("summarized"),
        )
    };

    let adaptive = supports_adaptive_thinking(model);
    let mut result = Map::new();
    if adaptive {
        let mut thinking = Map::new();
        thinking.insert("type".to_string(), json!("adaptive"));
        if let Some(display) = display {
            thinking.insert("display".to_string(), json!(display));
        }
        result.insert("thinking".to_string(), Value::Object(thinking));
        result.insert(
            "output_config".to_string(),
            json!({ "effort": map_thinking_level_to_effort(model, level) }),
        );
    } else {
        let budget = budget_override.unwrap_or_else(|| default_thinking_budget(level, opts));
        let mut thinking = Map::new();
        thinking.insert("type".to_string(), json!("enabled"));
        thinking.insert("budget_tokens".to_string(), json!(budget));
        if let Some(display) = display {
            thinking.insert("display".to_string(), json!(display));
        }
        result.insert("thinking".to_string(), Value::Object(thinking));
        // pi `:1079-1081`: the interleaved-thinking beta rides only the budget-based branch.
        if bedrock.interleaved_thinking.unwrap_or(true) {
            result.insert(
                "anthropic_beta".to_string(),
                json!([INTERLEAVED_THINKING_BETA]),
            );
        }
    }
    Some(Value::Object(result))
}

/// pi's inline `defaultBudgets` table plus the custom-budget lookup
/// (`bedrock-converse-stream.ts:1057-1068`).
///
/// The custom lookup uses the CLAMPED level (`xhigh`/`max` → `high`, because custom budgets only
/// cover the token-based rungs) while the default table is keyed by the ORIGINAL level — which is
/// why `xhigh` and `max` both default to 16384 rather than falling back to `high`'s entry.
fn default_thinking_budget(level: ThinkingLevel, opts: &StreamOptions) -> u64 {
    let budgets = opts.thinking_budgets.as_ref();
    let custom = match level {
        ThinkingLevel::Minimal => budgets.and_then(|b| b.minimal),
        ThinkingLevel::Low => budgets.and_then(|b| b.low),
        ThinkingLevel::Medium => budgets.and_then(|b| b.medium),
        // `xhigh`/`max` clamp to `high` for the custom lookup.
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => {
            budgets.and_then(|b| b.high)
        }
    };
    custom.unwrap_or(match level {
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 2048,
        ThinkingLevel::Medium => 8192,
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => 16384,
    })
}

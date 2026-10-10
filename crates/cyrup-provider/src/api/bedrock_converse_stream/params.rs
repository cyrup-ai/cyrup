//! Request encoding (pi `commandInput`, `bedrock-converse-stream.ts:230-241`).

use super::capabilities::{
    is_anthropic_claude_model, is_gov_cloud_bedrock_target, map_thinking_level_to_effort,
    model_match_candidates, supports_adaptive_thinking, supports_thinking_block_binding,
};
use super::convert::{build_system_prompt, convert_messages, convert_tool_config};
use super::options::{BedrockOptions, BedrockThinkingDisplay};
use crate::api::compat::mapped_effort_or;
use crate::context::Context;
use crate::model::Model;
use crate::stream::{CacheRetention, StreamOptions};
use crate::utils::provider_plumbing::{EnvSource, resolve_cache_retention_with};
use crate::utils::simple_options::{adjust_max_tokens_for_thinking, clamp_max_tokens_to_context};
use cyrup_core::ThinkingLevel;
use serde_json::{Map, Value, json};

/// pi's interleaved-thinking beta token (`bedrock-converse-stream.ts:1080`).
pub(super) const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// pi `THINKING_BINDING_CONTROLS_BETA` (`bedrock-converse-stream.ts:122` @v1.0.1) — rides the
/// adaptive branch beside `thinking.block_binding` (PROV-130).
pub(super) const THINKING_BINDING_CONTROLS_BETA: &str = "thinking-binding-controls-2026-08-01";

/// Build the `ConverseStreamCommand` input (pi `commandInput`,
/// `bedrock-converse-stream.ts:230-241`), including the `modelId` URI label so `onPayload` sees the
/// same object upstream hands it. [`super::driver::split_command_input`] lifts `modelId` back out afterwards.
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
    let inference_max_tokens =
        effective_max_tokens.or(if claude { Some(model.max_tokens) } else { None });

    let mut obj = Map::new();
    obj.insert("modelId".to_string(), json!(model.id.as_str()));
    obj.insert(
        "messages".to_string(),
        Value::Array(convert_messages(ctx, model, cache_retention, env)?),
    );
    if let Some(system) =
        build_system_prompt(ctx.system_prompt.as_deref(), model, cache_retention, env)
    {
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
    if let Some(tool_config) = convert_tool_config(
        &ctx.tools,
        bedrock.tool_choice.as_ref(),
        supports_strict_mode,
    )
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
/// `CYRUP_CACHE_RETENTION=long` promotes, else `"short"`.
///
/// The ladder itself is the shared one; only the lookup differs, because bedrock reads through
/// [`EnvSource`] rather than a bare overlay (its ambient map is a test seam).
pub(super) fn resolve_cache_retention(
    cache_retention: Option<CacheRetention>,
    env: &EnvSource<'_>,
) -> CacheRetention {
    resolve_cache_retention_with(cache_retention, |name| env.get(name))
}

/// pi `buildAdditionalModelRequestFields` (`bedrock-converse-stream.ts:1262-1333` @f1b2e77f5):
/// the Claude thinking payload, or, for OpenAI models, the reasoning effort (PROV-140).
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
    let level = opts.reasoning.level().unwrap_or(ThinkingLevel::High);
    if !is_anthropic_claude_model(model) {
        return build_openai_reasoning_fields(model, opts, level);
    }

    let is_gov_cloud = is_gov_cloud_bedrock_target(model, bedrock, env);
    // pi `:1048-1050`: GovCloud's Converse schema rejects `thinking.display`.
    let display = if is_gov_cloud {
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
    // pi `:1266` @v1.0.1: replayed signed thinking blocks are bound to the system prompt and tools
    // they were created with, and Bedrock 400s on replay once either changes unless the stale
    // blocks are dropped. Skipped on GovCloud like `display`, and only on models that accept the
    // field (PROV-130).
    let use_block_binding = !is_gov_cloud && supports_thinking_block_binding(model);
    let mut result = Map::new();
    if adaptive {
        let mut thinking = Map::new();
        thinking.insert("type".to_string(), json!("adaptive"));
        if let Some(display) = display {
            thinking.insert("display".to_string(), json!(display));
        }
        if use_block_binding {
            thinking.insert(
                "block_binding".to_string(),
                json!({ "prefix_mismatch_behavior": "drop_block" }),
            );
        }
        result.insert("thinking".to_string(), Value::Object(thinking));
        result.insert(
            "output_config".to_string(),
            json!({ "effort": map_thinking_level_to_effort(model, level) }),
        );
        // pi `:1279`: the binding beta rides the adaptive branch only, where the interleaved beta
        // never appears.
        if use_block_binding {
            result.insert(
                "anthropic_beta".to_string(),
                json!([THINKING_BINDING_CONTROLS_BETA]),
            );
        }
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

/// pi's non-Claude branches of `buildAdditionalModelRequestFields`
/// (`bedrock-converse-stream.ts:1318-1330` @f1b2e77f5, `2989eb581`, PROV-140).
///
/// `gpt-oss` is tested first because every `gpt-oss` id also contains `gpt-`. It takes a flat
/// `reasoning_effort` from its own table and ignores `thinkingLevelMap`. Other `gpt-` models
/// (GPT-5.x, GPT-6) take a nested `reasoning.effort`, where a string `thinkingLevelMap` entry
/// wins over the table. Every other non-Claude model gets no field.
fn build_openai_reasoning_fields(
    model: &Model,
    opts: &StreamOptions,
    level: ThinkingLevel,
) -> Option<Value> {
    let candidates = model_match_candidates(model);
    if candidates.iter().any(|s| s.contains("gpt-oss")) {
        return Some(json!({ "reasoning_effort": openai_gpt_oss_effort(level) }));
    }
    if candidates.iter().any(|s| s.contains("gpt-")) {
        // `typeof mapped === "string" ? mapped : OPENAI_GPT_EFFORT[level]`: an absent key and a
        // `null` entry both fall back to the table.
        let effort = mapped_effort_or(
            model.thinking_level_map.as_ref(),
            opts.reasoning,
            openai_gpt_effort(level),
        );
        return Some(json!({ "reasoning": { "effort": effort } }));
    }
    None
}

/// pi `OPENAI_GPT_EFFORT` (`bedrock-converse-stream.ts:1339-1346` @f1b2e77f5): GPT-5.x and GPT-6
/// reject `minimal`, so it is sent as `low`.
fn openai_gpt_effort(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

/// pi `OPENAI_GPT_OSS_EFFORT` (`bedrock-converse-stream.ts:1349-1356` @f1b2e77f5): gpt-oss only
/// accepts low, medium and high.
fn openai_gpt_oss_effort(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => "high",
    }
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

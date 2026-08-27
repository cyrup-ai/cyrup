//! Request encoding — the Messages request body (Pi `buildParams`).

use super::cache::get_cache_control;
use super::claude_code::to_claude_code_name;
use super::compat::{force_adaptive_thinking, get_anthropic_compat, off_is_not_null};
use super::messages::convert_messages;
use super::options::AnthropicThinkingDisplay;
use super::tools::convert_tools;
use crate::api::compat::sanitize_surrogates;
use crate::api::openai_completions::transform_messages_with;
use crate::auth::ProviderEnv;
use crate::context::Context;
use crate::model::Model;
use crate::stream::StreamOptions;
use crate::utils::constrained_sampling::ConstrainedSamplingError;
use crate::utils::deferred_tools::split_deferred_tools;
use crate::utils::simple_options::{adjust_max_tokens_for_thinking, clamp_max_tokens_to_context};
use cyrup_core::ThinkingLevel;
use serde_json::{Map, Value, json};
use std::collections::HashSet;

/// Map a unified [`ThinkingLevel`] to an Anthropic adaptive-thinking effort (Pi
/// `mapThinkingLevelToEffort`, anthropic-messages.ts:747-765). A `thinkingLevelMap` string overrides.
fn map_thinking_level_to_effort(model: &Model, level: ThinkingLevel) -> String {
    let key = match level {
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    };
    if let Some(Some(mapped)) = model.thinking_level_map.as_ref().and_then(|m| m.get(key)) {
        return mapped.clone();
    }
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => "low".to_string(),
        ThinkingLevel::Medium => "medium".to_string(),
        // Pi's switch has no `xhigh`/`max` case: both land on `default: "high"`
        // (anthropic-messages.ts:786-798). Only an explicit `thinkingLevelMap` entry (handled
        // above) promotes them to the native `xhigh`/`max` efforts.
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => "high".to_string(),
    }
}

/// Test-only convenience wrapper for [`build_params`] with no env overlay and API-key auth.
#[cfg(test)]
// Test-only fixture wrapper: the deny-list allowance the crate's `mod tests` blocks carry.
#[allow(clippy::expect_used)]
pub(crate) fn build_body(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    build_params(model, ctx, opts, None, false)
        .expect("fixture declares no unsatisfiable constrained sampling")
}

/// Build the Messages request JSON body (1:1 port of Pi `buildParams` + the `streamSimple` thinking
/// lowering, anthropic-messages.ts:767-1004). The unified `opts.reasoning` level drives the thinking
/// config and (for budget-based models) the `max_tokens` split.
/// `[CYRUP-DELTA]` — fallible where pi's `buildParams` throws: `convertTools` rejects a
/// `strict: "require"` tool on a model without `supportsStrictTools`
/// (`constrained-sampling.ts:91-95` @v0.83.0). Upstream that unwinds into `stream`'s catch and
/// becomes the turn's terminal error message; here the caller emits the identical event (PROV-011).
pub(crate) fn build_params(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    env: Option<&ProviderEnv>,
    is_oauth: bool,
) -> Result<Value, ConstrainedSamplingError> {
    let compat = get_anthropic_compat(model);
    let cache_control = get_cache_control(model, opts.cache_retention, env);

    // --- DRIFT-001 deferred-tool placement (Pi anthropic-messages.ts:947-960) ---
    //
    // The transform is HOISTED out of `convert_messages` because Pi splits over the TRANSFORMED
    // list (`{ ...context, messages: transformedMessages }`, :949-953) and then hands that same
    // list to `convertMessages` (:961). Splitting over the raw list would be a structural
    // divergence even though today's transform only rewrites tool-call ids.
    let transformed = transform_messages_with(&ctx.messages, model, normalize_tool_call_id);
    let normalize_tool_name: &dyn Fn(&str) -> String = if is_oauth {
        &|name: &str| to_claude_code_name(name)
    } else {
        &|name: &str| name.to_string()
    };
    let placement = split_deferred_tools(
        &transformed,
        &ctx.tools,
        compat.supports_tool_references,
        normalize_tool_name,
    );
    let mut deferred_tools = placement.deferred_tools();
    let mut immediate_tools = placement.immediate;
    // The SAFETY VALVE lives here and ONLY here (Pi :955-959). It is deliberately absent from
    // `split_deferred_tools` and from the openai-responses caller, which ships no `tools` key at
    // all when everything is deferred.
    if immediate_tools.is_empty() && !deferred_tools.is_empty() {
        immediate_tools = std::mem::take(&mut deferred_tools);
    }
    let deferred_tool_names: HashSet<String> = deferred_tools
        .iter()
        .map(|t| normalize_tool_name(&t.name))
        .collect();

    let reasoning_on = opts.reasoning.is_on();
    let thinking_enabled = model.reasoning && reasoning_on;
    let adaptive = force_adaptive_thinking(model);

    // max_tokens lowering (Pi `streamSimple`, anthropic-messages.ts:790-806). Budget-based models
    // split the cap between thinking and output; adaptive / non-thinking just clamp to the context.
    let mut budget_tokens: u64 = 1024;
    let max_tokens: u64 = if thinking_enabled && !adaptive {
        let level = opts.reasoning.level().unwrap_or(ThinkingLevel::High);
        let (adjusted, budget) = adjust_max_tokens_for_thinking(
            opts.max_tokens,
            model.max_tokens,
            level,
            opts.thinking_budgets.as_ref(),
        );
        let mt = clamp_max_tokens_to_context(model, ctx, adjusted);
        budget_tokens = budget.min(mt.saturating_sub(1024));
        mt
    } else {
        clamp_max_tokens_to_context(model, ctx, opts.max_tokens.unwrap_or(model.max_tokens))
    };

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert(
        "messages".to_string(),
        Value::Array(convert_messages(
            &transformed,
            is_oauth,
            cache_control.as_ref(),
            compat.allow_empty_signature,
            &deferred_tool_names,
            normalize_tool_name,
        )),
    );
    obj.insert("max_tokens".to_string(), json!(max_tokens));
    obj.insert("stream".to_string(), json!(true));

    // System prompt (+ OAuth Claude Code identity).
    let mut system: Vec<Value> = Vec::new();
    if is_oauth {
        system.push(system_text(
            "You are Claude Code, Anthropic's official CLI for Claude.",
            cache_control.as_ref(),
        ));
        if let Some(sp) = &ctx.system_prompt {
            system.push(system_text(
                &sanitize_surrogates(sp),
                cache_control.as_ref(),
            ));
        }
    } else if let Some(sp) = &ctx.system_prompt {
        system.push(system_text(
            &sanitize_surrogates(sp),
            cache_control.as_ref(),
        ));
    }
    if !system.is_empty() {
        obj.insert("system".to_string(), Value::Array(system));
    }

    // Temperature is incompatible with extended thinking and unsupported on Opus 4.7+.
    if let Some(temp) = opts.temperature
        && !thinking_enabled
        && compat.supports_temperature
    {
        obj.insert("temperature".to_string(), json!(temp));
    }

    // Tools: immediate prefix, then the deferred tail (Pi anthropic-messages.ts:1007-1021).
    // `cache_control` marks the last IMMEDIATE tool only — Pi passes `undefined` for the deferred
    // call, so the cache breakpoint never lands on a definition that is not part of the stable
    // prefix.
    if !immediate_tools.is_empty() || !deferred_tools.is_empty() {
        let tool_cc = if compat.supports_cache_control_on_tools {
            cache_control.as_ref()
        } else {
            None
        };
        let mut tools = convert_tools(
            &immediate_tools,
            is_oauth,
            compat.supports_eager_tool_input_streaming,
            compat.supports_strict_tools,
            tool_cc,
            false,
        )?;
        tools.extend(convert_tools(
            &deferred_tools,
            is_oauth,
            compat.supports_eager_tool_input_streaming,
            compat.supports_strict_tools,
            None,
            true,
        )?);
        obj.insert("tools".to_string(), Value::Array(tools));
    }

    // Thinking configuration (Pi anthropic-messages.ts:957-986).
    if model.reasoning {
        if thinking_enabled {
            // Pi `options.thinkingDisplay ?? "summarized"` (anthropic-messages.ts:962).
            let display = json!(
                opts.anthropic_options()
                    .and_then(|o| o.thinking_display)
                    .map(AnthropicThinkingDisplay::as_wire)
                    .unwrap_or("summarized")
            );
            if adaptive {
                let mut thinking = Map::new();
                thinking.insert("type".to_string(), json!("adaptive"));
                thinking.insert("display".to_string(), display);
                obj.insert("thinking".to_string(), Value::Object(thinking));
                if let Some(level) = opts.reasoning.level() {
                    let effort = map_thinking_level_to_effort(model, level);
                    obj.insert("output_config".to_string(), json!({ "effort": effort }));
                }
            } else {
                obj.insert(
                    "thinking".to_string(),
                    json!({
                        "type": "enabled",
                        "budget_tokens": budget_tokens.max(1),
                        "display": display,
                    }),
                );
            }
        } else if off_is_not_null(model) {
            obj.insert("thinking".to_string(), json!({ "type": "disabled" }));
        }
    }

    // metadata.user_id (Pi anthropic-messages.ts:988-993).
    if let Some(meta) = &opts.metadata
        && let Some(user_id) = meta.get("user_id").and_then(Value::as_str)
    {
        obj.insert("metadata".to_string(), json!({ "user_id": user_id }));
    }

    // tool_choice (Pi anthropic-messages.ts:995-1001). cyrup's unified ToolChoice maps onto
    // Anthropic's `{type:"auto"|"any"|"none"}` / `{type:"tool",name}`.
    if let Some(tc) = &opts.tool_choice {
        obj.insert("tool_choice".to_string(), tool_choice_wire(tc));
    }

    Ok(Value::Object(obj))
}

/// Map cyrup's unified [`crate::stream::ToolChoice`] onto Anthropic's tool-choice wire shape.
pub(super) fn tool_choice_wire(tc: &crate::stream::ToolChoice) -> Value {
    use crate::stream::ToolChoice;
    match tc {
        ToolChoice::Auto => json!({ "type": "auto" }),
        ToolChoice::None => json!({ "type": "none" }),
        // Anthropic spells "required" as "any".
        ToolChoice::Required => json!({ "type": "any" }),
        ToolChoice::Function { name } => json!({ "type": "tool", "name": name }),
    }
}

/// A `system` text block, optionally cached.
fn system_text(text: &str, cache_control: Option<&Value>) -> Value {
    let mut o = Map::new();
    o.insert("type".to_string(), json!("text"));
    o.insert("text".to_string(), json!(text));
    if let Some(cc) = cache_control {
        o.insert("cache_control".to_string(), cc.clone());
    }
    Value::Object(o)
}

/// Anthropic tool-call-id normalization (Pi `normalizeToolCallId`, anthropic-messages.ts:1006-1009):
/// non-`[a-zA-Z0-9_-]` → `_`, truncated to 64 chars.
pub(super) fn normalize_tool_call_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

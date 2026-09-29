//! Request encoding: the Chat Completions request body (Pi `buildParams`).

use super::cache::{apply_anthropic_cache_control, compat_cache_control};
use super::convert::convert_messages;
use super::reasoning::apply_reasoning;
use super::tools::{convert_tools, deferred_tool_names, message_has_tool_use};
use crate::api::compat::{
    DeferredToolsMode, MaxTokensField, ResolvedCompat, ThinkingTokenBudgetField,
    clamp_openai_prompt_cache_key, get_compat,
};
use crate::context::{Context, ToolDef};
use crate::model::Model;
use crate::stream::{CacheRetention, StreamOptions};
use crate::utils::constrained_sampling::ConstrainedSamplingError;
use crate::utils::provider_plumbing::{EnvSource, resolve_cache_retention};
use crate::utils::simple_options::{
    clamp_thinking_budget_to_answer_room, thinking_budget_for_level,
};
use cyrup_core::ModelThinkingLevel;
use serde_json::{Map, Value, json};

/// Map a unified [`ModelThinkingLevel`] to the OpenAI `reasoning_effort` value (None for `Off`).
pub(super) fn reasoning_effort(level: ModelThinkingLevel) -> Option<&'static str> {
    match level {
        ModelThinkingLevel::Off => None,
        ModelThinkingLevel::Minimal => Some("minimal"),
        ModelThinkingLevel::Low => Some("low"),
        ModelThinkingLevel::Medium => Some("medium"),
        ModelThinkingLevel::High => Some("high"),
        ModelThinkingLevel::Xhigh => Some("xhigh"),
        // Pi `reasoningEffort` is the level string verbatim (openai-completions.ts:621) and its
        // `OpenAICompletionsOptions.reasoningEffort` union includes `"max"` (:143).
        ModelThinkingLevel::Max => Some("max"),
    }
}

/// Build the Chat Completions request JSON body from the [`Context`] + [`StreamOptions`].
///
/// 1:1 port of Pi `buildParams` (openai-completions.ts L534-687): resolves the compatibility
/// matrix for the model, encodes prompt-cache options, the max-tokens field, `store`, tools +
/// `tool_choice`, the per-provider reasoning encoding, and routing preferences.
/// Test-only convenience wrapper for [`build_body_with_env`] with no env overlay (the request path
/// uses [`build_body_with_env`] directly so it can forward the provider-scoped env).
#[cfg(test)]
// Test-only fixture wrapper: the deny-list allowance the crate's `mod tests` blocks carry.
#[allow(clippy::expect_used)]
pub(crate) fn build_body(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    build_body_with_env(model, ctx, opts, EnvSource::default())
        .expect("fixture declares no unsatisfiable constrained sampling")
}

// `getProviderEnvValue` (provider-env.ts:45-52) and `resolveCacheRetention`
// (openai-completions.ts:141-149) live in `crate::utils::provider_plumbing`: this file carried
// byte-identical ports of both, shared with anthropic-messages and openai-responses.

/// Env-aware `build_body`: `env` carries the provider-scoped overlay (Pi `options.env`) consulted
/// by [`resolve_cache_retention`] for the `CYRUP_CACHE_RETENTION` fallback, plus (test-only) the
/// ambient override an [`EnvSource`] adds over a bare overlay (TEST_ENV_HERMETICITY).
/// `[CYRUP-DELTA]` — fallible where pi's `buildParams` throws. `convertTools` can throw for a
/// `strict: "require"` tool on a provider without strict mode (`constrained-sampling.ts:91-95`
/// @v0.83.0); upstream that unwinds into `stream`'s catch and becomes the turn's terminal error
/// message. cyrup returns the same message through `Result` and the caller emits the identical
/// terminal event (PROV-011).
pub(crate) fn build_body_with_env(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    env: EnvSource<'_>,
) -> Result<Value, ConstrainedSamplingError> {
    let compat = get_compat(model);
    let cache = resolve_cache_retention(opts.cache_retention, env);
    let mut messages = convert_messages(model, ctx, &compat)?;
    let cache_control = compat_cache_control(&compat, cache);
    let base_url = model.base_url.as_str();

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert("stream".to_string(), json!(true));

    // Prompt caching (OpenAI `prompt_cache_key` / `prompt_cache_retention`).
    let want_cache_key = (base_url.contains("api.openai.com") && cache != CacheRetention::None)
        || (cache == CacheRetention::Long && compat.supports_long_cache_retention);
    if want_cache_key && let Some(sid) = &opts.session_id {
        obj.insert(
            "prompt_cache_key".to_string(),
            json!(clamp_openai_prompt_cache_key(sid.as_str())),
        );
    }
    if cache == CacheRetention::Long && compat.supports_long_cache_retention {
        obj.insert("prompt_cache_retention".to_string(), json!("24h"));
    }

    if compat.supports_usage_in_streaming {
        obj.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );
    }
    if compat.supports_store {
        obj.insert("store".to_string(), json!(false));
    }

    // PROV-069 — an output ceiling is ALWAYS sent: the caller's when it supplied one, otherwise the
    // model's own `max_tokens` from the catalog.
    //
    // [CYRUP-DELTA] Upstream sends this key only when the caller supplies one
    // (`ai/src/api/openai-completions.ts:716`, `if (options?.maxTokens)`), and nothing in cyrup's
    // turn path ever does — `GenConfig::max_tokens` has no production writer, so the key was never
    // emitted and the server applied its OWN default ceiling. On Together that default truncates a
    // reply mid-sentence within a few hundred tokens, on every turn, with `finish_reason: length`,
    // while the session sits at ~3% of a 1M context window. The catalog's `max_tokens` — ported for
    // all 1087 rows and covered by tests — reached no request at all.
    //
    // The fallback is upstream's OWN rule, taken from the two APIs where it is explicit rather than
    // invented here: `anthropic-messages.ts:989` sends `options?.maxTokens ?? model.maxTokens`, and
    // `adjustMaxTokensForThinking` (`simple-options.ts:61-64`) documents the same intent in words —
    // "Undefined means no explicit caller cap. Use the model cap and fit thinking inside it."
    // Applying it here makes the three wire paths agree instead of leaving this one uncapped.
    //
    // A caller-supplied value still wins, so `maxTokens` in settings / `modelOverrides` keeps its
    // precedence. When neither exists (`max_tokens == 0`, the modelless fallback), nothing is sent
    // and upstream's behaviour is unchanged.
    let ceiling = opts.max_tokens.or(if model.max_tokens > 0 {
        Some(model.max_tokens)
    } else {
        None
    });
    if let Some(max) = ceiling {
        match compat.max_tokens_field {
            MaxTokensField::MaxTokens => {
                obj.insert("max_tokens".to_string(), json!(max));
            }
            MaxTokensField::MaxCompletionTokens => {
                obj.insert("max_completion_tokens".to_string(), json!(max));
            }
        }
    }
    if let Some(temp) = opts.temperature {
        obj.insert("temperature".to_string(), json!(temp));
    }

    // Tools (+ z.ai tool_stream) / empty-tools-for-tool-history.
    let has_tool_history = ctx.messages.iter().any(message_has_tool_use);
    let mut tools: Option<Vec<Value>> = None;
    // PROV-025 — `const deferredToolNames = compat.deferredToolsMode === "kimi" ?
    // getDeferredToolNames(context.messages) : new Set(); const activeTools =
    // context.tools?.filter((tool) => !deferredToolNames.has(tool.name));`
    // (`openai-completions.ts:719-721` @v0.83.0). A tool introduced mid-transcript is emitted ONCE
    // inline by `convert_messages` and must NOT be repeated in the top-level array — that
    // repetition is exactly the prompt-prefix churn the mode exists to avoid. Note the emptiness
    // test below is on the FILTERED list, so a transcript whose every tool is deferred falls
    // through to the `has_tool_history` arm, as upstream's `activeTools.length > 0` does.
    let active_tools: Vec<ToolDef> = if compat.deferred_tools_mode == Some(DeferredToolsMode::Kimi)
    {
        let deferred = deferred_tool_names(&ctx.messages);
        ctx.tools
            .iter()
            .filter(|t| !deferred.iter().any(|n| n == &t.name))
            .cloned()
            .collect()
    } else {
        ctx.tools.clone()
    };
    if !active_tools.is_empty() {
        tools = Some(convert_tools(&active_tools, &compat)?);
        if compat.zai_tool_stream {
            obj.insert("tool_stream".to_string(), json!(true));
        }
    } else if has_tool_history {
        // Some OpenAI-compatible proxies require `tools` to be present whenever the conversation
        // already contains tool calls / tool results.
        tools = Some(Vec::new());
    }

    // Anthropic-style cache_control markers (OpenRouter `anthropic/*`).
    if let Some(cc) = &cache_control {
        apply_anthropic_cache_control(&mut messages, tools.as_mut(), cc);
    }

    obj.insert("messages".to_string(), Value::Array(messages));
    if let Some(t) = tools {
        obj.insert("tools".to_string(), Value::Array(t));
    }

    // tool_choice — emitted ONLY when the caller specifies one (matches Pi; no auto-injection).
    if let Some(tc) = &opts.tool_choice {
        obj.insert("tool_choice".to_string(), tc.to_wire());
    }

    // PROV-100. `if (compat.vllmPriority !== undefined) params.priority = compat.vllmPriority`
    // (openai-completions.ts:866-868 @v0.87.1).
    if let Some(priority) = &compat.vllm_priority {
        obj.insert("priority".to_string(), Value::Number(priority.clone()));
    }

    // PROV-100. `resolveThinkingTokenBudgetField(compat)` / `resolveClampedThinkingBudget(model,
    // options, params)` (`:870-871`, `:1004-1024`). The ceiling is the output cap just written —
    // `params.max_tokens ?? params.max_completion_tokens ?? model.maxTokens`.
    let budget_field = thinking_token_budget_field(&compat);
    let thinking_budget = clamped_thinking_budget(model, opts, ceiling.unwrap_or(model.max_tokens));

    apply_reasoning(&mut obj, model, opts, &compat, thinking_budget);

    // PROV-100. Cap reasoning with a top-level budget field, independent of `thinkingFormat`: the
    // same server can serve zai, qwen or chat-template models, and reasoning shares `max_tokens`
    // with the answer, so an uncapped reasoning phase can leave no answer and no tool call
    // (`:971-978`).
    if let (Some(field), Some(budget)) = (budget_field, thinking_budget) {
        obj.insert(field.as_str().to_string(), json!(budget));
    }

    // OpenRouter / Vercel AI Gateway routing preferences (read from raw `model.compat`).
    if let Some(c) = &model.compat {
        // PROV-066: the typed `OpenRouterRouting` serializes back to the same JSON object the
        // `Value` form carried, so the wire payload is unchanged; `to_value` on a plain struct of
        // primitives cannot fail, and if it somehow did, omitting the key is the safe direction
        // (OpenRouter routes by its own defaults) rather than sending a partial object.
        if let Some(routing) = &c.open_router_routing
            && let Ok(value) = serde_json::to_value(routing)
        {
            obj.insert("provider".to_string(), value);
        }
        if let Some(vg) = &c.vercel_gateway_routing
            && (vg.only.is_some() || vg.order.is_some())
        {
            let mut gateway = Map::new();
            if let Some(only) = &vg.only {
                gateway.insert("only".to_string(), json!(only));
            }
            if let Some(order) = &vg.order {
                gateway.insert("order".to_string(), json!(order));
            }
            obj.insert(
                "providerOptions".to_string(),
                json!({ "gateway": Value::Object(gateway) }),
            );
        }
    }

    // Last so custom keys override the named request fields (Pi's own comment,
    // `openai-completions.ts:884-887` @v0.84.1: `if (options?.samplingParams)
    // Object.assign(params, options.samplingParams)`). AGENT-026. The merge with
    // `Model.sampling_params` already happened in `build_base_options`
    // (`simple-options.ts:27-33`), so what arrives here is the resolved map — and being LAST is the
    // whole point: an operator's `top_p` must beat the named `temperature`/`max_tokens` block above.
    apply_sampling_params(&mut obj, opts);

    Ok(Value::Object(obj))
}

/// Pi `resolveThinkingTokenBudgetField` (`openai-completions.ts:1004-1010` @v0.87.1): the explicit
/// field wins; `supportsThinkingTokenBudget` is the vLLM alias.
fn thinking_token_budget_field(compat: &ResolvedCompat) -> Option<ThinkingTokenBudgetField> {
    compat.thinking_token_budget_field.or(compat
        .supports_thinking_token_budget
        .then_some(ThinkingTokenBudgetField::ThinkingTokenBudget))
}

/// Pi `resolveClampedThinkingBudget` (`openai-completions.ts:1012-1024` @v0.87.1): the level's
/// budget (custom `thinkingBudgets` over the default table), capped so [`MIN_ANSWER_TOKENS`] of the
/// response ceiling stay free for the answer. `None` when reasoning is off, the model does not
/// reason, or no room is left.
///
/// [`MIN_ANSWER_TOKENS`]: crate::utils::simple_options::MIN_ANSWER_TOKENS
pub(super) fn clamped_thinking_budget(
    model: &Model,
    opts: &StreamOptions,
    ceiling: u64,
) -> Option<u64> {
    let level = opts.reasoning.level().filter(|_| model.reasoning)?;
    let budget = clamp_thinking_budget_to_answer_room(
        thinking_budget_for_level(level, opts.thinking_budgets.as_ref()),
        ceiling,
    );
    (budget > 0).then_some(budget)
}

/// `Object.assign(params, options.samplingParams)` — the identical three-line tail of all three
/// OpenAI-compatible `buildParams` (`openai-completions.ts:884-887`, `openai-responses.ts:330-333`,
/// `azure-openai-responses.ts:324-327` @v0.84.1). Shared here rather than triplicated so the three
/// cannot drift apart; the absent-map case is a no-op exactly as pi's `if` guard is. AGENT-026.
pub(crate) fn apply_sampling_params(obj: &mut Map<String, Value>, opts: &StreamOptions) {
    let Some(params) = &opts.sampling_params else {
        return;
    };
    for (k, v) in params {
        obj.insert(k.clone(), v.clone());
    }
}

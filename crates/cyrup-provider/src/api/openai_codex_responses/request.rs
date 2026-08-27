//! Request encoding.

use super::CODEX_TOOL_CALL_PROVIDERS;
use super::options::{CodexReasoningSummary, CodexToolChoice, OpenAiCodexResponsesOptions};
use crate::api::compat::{level_map_lookup, thinking_level_key};
use crate::api::openai_responses::{
    ConvertResponsesToolsOptions, convert_responses_messages, convert_responses_tools,
};
use crate::collection::clamp_thinking_level;
use crate::context::Context;
use crate::model::Model;
use crate::stream::StreamOptions;
use crate::utils::deferred_tools::split_deferred_tools;
use cyrup_core::ModelThinkingLevel;
use serde_json::{Map, Value, json};

/// 1:1 port of pi `buildRequestBody` (`openai-codex-responses.ts:529-596`).
///
/// Differences from `openai-responses`' `buildParams` that are easy to "fix" wrongly:
/// * the system prompt rides in `instructions`, NOT as a leading input item
///   (`includeSystemPrompt: false`, `:545`), defaulting to `"You are a helpful assistant."`;
/// * there is **no** `max_output_tokens` — Codex never sends one;
/// * `include: ["reasoning.encrypted_content"]` is unconditional, not reasoning-gated;
/// * `tool_choice` and `parallel_tool_calls` are always present;
/// * `reasoning` is emitted purely from the requested effort, with no `model.reasoning` gate and no
///   `off`-branch `{effort}`-only body.
///
/// `[CYRUP-DELTA]` — fallible where pi's `buildParams` throws: `convertResponsesTools` rejects a
/// `strict: "require"` tool on a route without strict mode (`constrained-sampling.ts:91-95`
/// @v0.83.0). PROV-011.
pub(super) fn build_request_body(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    codex: &OpenAiCodexResponsesOptions,
    cache_session_id: Option<&str>,
) -> Result<Value, crate::utils::constrained_sampling::ConstrainedSamplingError> {
    let supports_tool_search = model
        .compat
        .as_ref()
        .and_then(|c| c.supports_tool_search)
        .unwrap_or(false);
    let placement = split_deferred_tools(
        &ctx.messages,
        &ctx.tools,
        supports_tool_search,
        &|name: &str| name.to_string(),
    );

    // `includeSystemPrompt: false` (:545). cyrup's shared converter always prepends
    // `ctx.system_prompt` when present, so the prompt is withheld from the context it sees and
    // placed in `instructions` below — the same bytes, in the field Codex expects.
    let body_ctx = Context {
        system_prompt: None,
        messages: ctx.messages.clone(),
        tools: ctx.tools.clone(),
    };
    // Codex's own tool options (openai-codex-responses.ts:539-540, `:575-579` @v0.83.0):
    // `supportsStrictMode ?? true` (NOT openai-responses' `?? false`), and `strict: null` as the
    // default — a JSON `null` on the wire, not an absent key.
    let tool_options = ConvertResponsesToolsOptions {
        defer_loading: false,
        supports_strict_mode: model
            .compat
            .as_ref()
            .and_then(|c| c.supports_strict_mode)
            .unwrap_or(true),
        default_strict: None,
    };
    let messages = convert_responses_messages(
        model,
        &body_ctx,
        CODEX_TOOL_CALL_PROVIDERS,
        &placement.deferred,
        tool_options,
    );

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert("store".to_string(), json!(false));
    obj.insert("stream".to_string(), json!(true));
    // `context.systemPrompt || "You are a helpful assistant."` — the empty string is falsy.
    obj.insert(
        "instructions".to_string(),
        json!(
            ctx.system_prompt
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("You are a helpful assistant.")
        ),
    );
    obj.insert("input".to_string(), Value::Array(messages?));
    obj.insert(
        "text".to_string(),
        json!({ "verbosity": codex.text_verbosity.as_deref().filter(|s| !s.is_empty()).unwrap_or("low") }),
    );
    obj.insert(
        "include".to_string(),
        json!(["reasoning.encrypted_content"]),
    );
    // `prompt_cache_key: cacheSessionId` — `undefined` serializes to an absent key.
    if let Some(sid) = cache_session_id {
        obj.insert("prompt_cache_key".to_string(), json!(sid));
    }
    obj.insert(
        "tool_choice".to_string(),
        json!(codex.tool_choice.unwrap_or(CodexToolChoice::Auto).as_str()),
    );
    obj.insert("parallel_tool_calls".to_string(), json!(true));

    // PERM-012. `openai-codex-responses` is in upstream's `TEMPERATURE_UNSUPPORTED_APIS`
    // (`pi-permission-system/src/model-option-compatibility.ts:20-22` @v0.8.0), so the key is
    // NEVER written on this route regardless of the model — the first arm of
    // `getUnsupportedTemperatureReason` (`:65-67`). The `filter` is the whole api-level rule; it
    // is expressed through the shared predicate so the three Responses builders cannot drift.
    if let Some(temp) = opts
        .temperature
        .filter(|_| crate::api::compat::temperature_is_supported(model))
    {
        obj.insert("temperature".to_string(), json!(temp));
    }
    if let Some(tier) = codex.service_tier.as_deref() {
        obj.insert("service_tier".to_string(), json!(tier));
    }
    if !placement.immediate.is_empty() {
        obj.insert(
            "tools".to_string(),
            Value::Array(convert_responses_tools(&placement.immediate, tool_options)?),
        );
    }

    // `if (options?.reasoningEffort !== undefined)` (:582). cyrup's unified level is `off` exactly
    // where pi's `streamSimple` leaves `reasoningEffort` undefined (:516-517), so `off` emits
    // nothing at all — Codex has no `openai-responses`-style `{ effort }`-only off branch.
    let clamped = clamp_thinking_level(model, opts.reasoning);
    if clamped != ModelThinkingLevel::Off {
        let key = thinking_level_key(clamped);
        // `model.thinkingLevelMap?.[level] ?? level`, then `if (effort !== null)`: a level mapped
        // explicitly to `null` suppresses the whole `reasoning` object. Ported for fidelity even
        // though `clampThinkingLevel` treats a null-mapped rung as unsupported and re-targets the
        // request before it gets here — the guard becomes live only for a caller that supplies
        // `reasoningEffort` directly, which is upstream's non-`streamSimple` entry point.
        let effort = match level_map_lookup(model.thinking_level_map.as_ref(), key) {
            Some(None) => None,
            Some(Some(mapped)) => Some(mapped.clone()),
            None => Some(key.to_string()),
        };
        if let Some(effort) = effort {
            let summary = codex
                .reasoning_summary
                .unwrap_or(CodexReasoningSummary::Auto)
                .as_str();
            obj.insert(
                "reasoning".to_string(),
                json!({ "effort": effort, "summary": summary }),
            );
        }
    }

    Ok(Value::Object(obj))
}

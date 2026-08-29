//! Response decoding: the terminal message, usage and the stop-reason table.

use super::blocks::{Decoder, project_block};
use crate::model::Model;
use crate::usage::apply_cost;
use crate::utils::provider_plumbing::now_millis;
use cyrup_core::{ApiId, AssistantMessage, StopReason, Usage};
use serde_json::{Map, Value};

/// Build the terminal [`AssistantMessage`] from accumulated decoder state.
pub(super) fn build_final_message(dec: Decoder, model: &Model, api: &ApiId) -> AssistantMessage {
    let content: Vec<_> = dec.blocks.iter().map(project_block).collect();

    let mut usage = dec.usage.unwrap_or_default();
    apply_cost(&model.cost, &mut usage);

    // Pi's `output.stopReason` when no `finish_reason` ever arrived: still the `"pending"` seed
    // (openai-completions.ts:218). The sole caller hands this straight to
    // `StreamEvent::end_of_stream`, which rewrites it — but seeding `Stop` here would have made a
    // truncated message look complete to anyone who called this helper directly.
    let stop_reason = dec.stop_reason.unwrap_or(StopReason::Pending);

    AssistantMessage {
        content,
        provider: model.provider.clone(),
        model: model.id.as_str().to_string(),
        api: api.clone(),
        response_model: dec.response_model,
        response_id: dec.response_id,
        diagnostics: None,
        usage,
        stop_reason,
        deferred: None,
        error_message: dec.error_message,
        raw_stop_reason: dec.raw_stop_reason,
        timestamp: now_millis(),
    }
}

/// Parse usage from a chunk's `usage` object (cache-read/write split + reasoning), applying cost.
pub(super) fn parse_usage(raw: &Value, model: &Model) -> Usage {
    let u64_at = |v: &Value, key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);

    let prompt = u64_at(raw, "prompt_tokens");
    let completion = u64_at(raw, "completion_tokens");
    let details = raw.get("prompt_tokens_details");
    let cache_read = details
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| raw.get("prompt_cache_hit_tokens").and_then(Value::as_u64))
        .unwrap_or(0);
    let cache_write = details
        .and_then(|d| d.get("cache_write_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning = raw
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(Value::as_u64);

    let input = prompt
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);
    let mut usage = Usage {
        input,
        output: completion,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning,
        total_tokens: 0,
        cost: Default::default(),
    };
    apply_cost(&model.cost, &mut usage);
    usage
}

/// Map an OpenAI `finish_reason` to a [`StopReason`] (plus an optional error message).
pub(super) fn map_stop_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" | "end" => (StopReason::Stop, None),
        "length" => (StopReason::Length, None),
        "tool_calls" | "function_call" => (StopReason::ToolUse, None),
        other => (
            StopReason::Error,
            Some(format!("Provider finish_reason: {other}")),
        ),
    }
}

/// Best-effort parse of accumulated tool-call argument JSON. An empty/incomplete buffer yields an
/// empty object so a truncated stream still produces a valid (if empty) tool call.
/// Parse a (possibly partial / streaming) tool-call argument string into the JSON object Pi's
/// `ToolCall.arguments: Record<string, any>` requires (types.ts:348). Incomplete, invalid, or
/// non-object input yields an empty object `{}` rather than a scalar, so the decoder always produces
/// a well-typed object.
pub(super) fn parse_partial_json(s: &str) -> Map<String, Value> {
    // Best-effort recovery of truncated/streamed tool-call args (Pi `parseStreamingJson`,
    // utils/json-parse.ts): a strict parse first, then repair, then a tolerant partial parse that
    // preserves a truncated string/number/array instead of discarding the whole object (#28).
    crate::utils::json_parse::parse_streaming_json_object(Some(s))
}

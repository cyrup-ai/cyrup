//! Response decoding — usage folding, the prefix-cache accounting, the `finishReason` table
//! and the terminal error (Pi mistral-conversations.ts:92-101,333-345,662-677).

use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use cyrup_core::{ApiId, StopReason, Usage};
use serde_json::Value;
use super::decoder::Decoder;

/// Apply Mistral `usage` (Pi mistral-conversations.ts:333-345).
pub(super) fn apply_usage(usage: &mut Usage, raw: &Value) {
    let prompt = raw.get("promptTokens").and_then(Value::as_u64).unwrap_or(0);
    let cached = mistral_cached_prompt_tokens(raw, prompt);
    usage.input = prompt.saturating_sub(cached);
    usage.output = raw
        .get("completionTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.cache_read = cached;
    usage.cache_write = 0;
    usage.total_tokens = raw
        .get("totalTokens")
        .and_then(Value::as_u64)
        .filter(|t| *t > 0)
        .unwrap_or(usage.input + usage.output + usage.cache_read + usage.cache_write);
}

/// Extract the cached prompt tokens across Mistral's several spellings (Pi
/// `getMistralCachedPromptTokens`, mistral-conversations.ts:274-293), clamped to `[0, promptTokens]`.
fn mistral_cached_prompt_tokens(raw: &Value, prompt_tokens: u64) -> u64 {
    let candidates = [
        raw.get("promptTokensDetails")
            .and_then(|d| d.get("cachedTokens")),
        raw.get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens")),
        raw.get("promptTokenDetails")
            .and_then(|d| d.get("cachedTokens")),
        raw.get("prompt_token_details")
            .and_then(|d| d.get("cached_tokens")),
        raw.get("numCachedTokens"),
        raw.get("num_cached_tokens"),
    ];
    let cached = candidates
        .into_iter()
        .flatten()
        .find_map(Value::as_u64)
        .unwrap_or(0);
    cached.min(prompt_tokens)
}

/// Map a Mistral `finishReason` to a cyrup [`StopReason`] (Pi `mapChatStopReason`,
/// mistral-conversations.ts:662-677).
///
/// The `Option` mirrors Pi's `reason: string | null` signature; the `None` arm is unreachable from
/// the streaming decoder, because Pi guards the call with the truthiness test
/// `if (choice.finishReason)` (`:355`) and cyrup matches that — a null/empty `finishReason` leaves
/// the turn unsettled rather than mapping it to `Stop`.
///
/// Returns `(stop_reason, error_message)`, mirroring pi's `{ stopReason, errorMessage? }` tuple —
/// the same shape [`crate::api::anthropic_messages`]'s `map_stop_reason` already uses.
///
/// The unknown arm is the point. This previously returned a bare [`StopReason`] whose catch-all was
/// `Some(_) => StopReason::Stop`, so ANY finish reason outside the five known values — `content_filter`,
/// or anything Mistral adds later — was transcribed as a clean, successful turn: the agent loop saw
/// `Stop`, emitted `turn_end` with no tool calls, and ended the run with no error banner and no
/// retry, carrying only whatever partial text arrived before the cutoff. pi's `default` arm returns
/// `{ stopReason: "error", errorMessage: `Provider stopped with: ${reason}` }` (`:674-675`), and its
/// `"error"` arm likewise carries `"Provider stopped with: error"` (`:672-673`) rather than letting
/// the call site fall back to the generic `"An unknown error occurred"`.
pub(super) fn map_chat_stop_reason(reason: Option<&str>) -> (StopReason, Option<String>) {
    match reason {
        None | Some("stop") => (StopReason::Stop, None),
        Some("length") | Some("model_length") => (StopReason::Length, None),
        Some("tool_calls") => (StopReason::ToolUse, None),
        Some("error") => (
            StopReason::Error,
            Some("Provider stopped with: error".to_string()),
        ),
        Some(other) => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
    }
}

/// Emit a terminal error event carrying the partial snapshot (Pi catch block,
/// mistral-conversations.ts:92-101).
pub(super) async fn emit_error(dec: &Decoder, model: &Model, api: &ApiId, sink: &EventSink, message: String) {
    let mut msg = dec.snapshot(model, api);
    msg.stop_reason = StopReason::Error;
    msg.error_message = Some(message);
    sink.send(StreamEvent::terminal(msg)).await;
}

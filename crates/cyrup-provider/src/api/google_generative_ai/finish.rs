//! Response decoding — block close-out, `thoughtSignature` retention, usage folding and the
//! terminal error (Pi google-generative-ai.ts:106-122,218-273).

use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use cyrup_core::{ApiId, Content, StopReason, Usage};
use serde_json::Value;
use super::decoder::{CurrentKind, Decoder};

/// Emit the `*_end` for the in-progress text/thinking block, if any (Pi google-generative-ai.ts:106-122).
pub(super) async fn close_current(dec: &mut Decoder, model: &Model, api: &ApiId, sink: &EventSink) -> bool {
    let Some(kind) = dec.current.take() else {
        return true;
    };
    let idx = dec.block_index();
    let partial = dec.snapshot(model, api);
    let ev = match (kind, dec.blocks.get(idx)) {
        (CurrentKind::Text, Some(Content::Text { text, .. })) => StreamEvent::TextEnd {
            content_index: idx,
            content: text.to_string(),
            partial,
        },
        (CurrentKind::Thinking, Some(Content::Thinking { thinking, .. })) => {
            StreamEvent::ThinkingEnd {
                content_index: idx,
                content: thinking.to_string(),
                partial,
            }
        }
        _ => return true,
    };
    sink.send(ev).await
}

/// Retain the last non-empty signature for the current block (Pi `retainThoughtSignature`,
/// google-shared.ts:46-49). Returns the new value when it should replace the existing one.
pub(super) fn retain_signature(_existing: Option<&str>, incoming: Option<&str>) -> Option<String> {
    match incoming {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    }
}

/// Apply Gemini `usageMetadata` (Pi google-generative-ai.ts:216-235).
pub(super) fn apply_usage(usage: &mut Usage, meta: &Value) {
    let prompt = meta
        .get("promptTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached = meta
        .get("cachedContentTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let candidates = meta
        .get("candidatesTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let thoughts = meta
        .get("thoughtsTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = meta
        .get("totalTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    usage.input = prompt.saturating_sub(cached);
    usage.output = candidates + thoughts;
    usage.cache_read = cached;
    usage.cache_write = 0;
    usage.reasoning = Some(thoughts);
    usage.total_tokens = total;
}

/// Emit a terminal error event carrying the partial snapshot (Pi catch block,
/// google-generative-ai.ts:266-277).
pub(super) async fn emit_error(dec: &Decoder, model: &Model, api: &ApiId, sink: &EventSink, message: String) {
    let mut msg = dec.snapshot_owned(model, api);
    msg.stop_reason = StopReason::Error;
    msg.error_message = Some(message);
    sink.send(StreamEvent::terminal(msg)).await;
}

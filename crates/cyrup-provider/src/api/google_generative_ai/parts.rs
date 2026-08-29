//! Response decoding — one `GenerateContentResponse` chunk: the `candidate.content.parts`
//! walk over text / thinking parts and `functionCall` parts
//! (Pi google-generative-ai.ts:106-210).

use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use crate::utils::provider_plumbing::now_millis;
use cyrup_core::{ApiId, Content, StopReason, ToolCall, ToolCallId};
use serde_json::Value;
use std::sync::atomic::Ordering;
use super::TOOL_CALL_COUNTER;
use super::decoder::{CurrentKind, Decoder};
use super::finish::{apply_usage, close_current, retain_signature};
use super::stop_reason::map_stop_reason;

/// Process one decoded `GenerateContentResponse` chunk. Returns `false` if the consumer dropped.
pub(super) async fn process_chunk(
    chunk: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    if dec.response_id.is_none()
        && let Some(id) = chunk.get("responseId").and_then(Value::as_str)
        && !id.is_empty()
    {
        dec.response_id = Some(id.to_string());
    }

    let candidate = chunk.get("candidates").and_then(|c| c.get(0));
    if let Some(candidate) = candidate
        && let Some(parts) = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
    {
        for part in parts {
            if part.get("text").and_then(Value::as_str).is_some()
                && !process_text_part(part, dec, model, api, sink).await
            {
                return false;
            }
            if part.get("functionCall").is_some()
                && !process_function_call(part, dec, model, api, sink).await
            {
                return false;
            }
        }
    }

    if let Some(reason) = candidate
        .and_then(|c| c.get("finishReason"))
        .and_then(Value::as_str)
    {
        // pi records the raw reason first (`v0.84.1 ai/src/api/google-generative-ai.ts:216`) and
        // never unsets it — not even when the tool-call override below rewrites `stopReason`.
        dec.raw_stop_reason = Some(reason.to_string());
        let (stop, err) = map_stop_reason(reason);
        dec.stop_reason = Some(stop);
        if let Some(err) = err {
            dec.error_message = Some(err);
        }
        if dec.blocks.iter().any(|b| matches!(b, Content::ToolCall(_))) {
            // A tool call present alongside a non-STOP reason is still a tool-use turn; clear the
            // diagnostic with it so a successful turn never carries a stale error message.
            dec.stop_reason = Some(StopReason::ToolUse);
            dec.error_message = None;
        }
    }

    if let Some(meta) = chunk.get("usageMetadata") {
        apply_usage(&mut dec.usage, meta);
    }

    true
}

/// Handle a text (or thinking) part (Pi google-generative-ai.ts:99-158).
async fn process_text_part(
    part: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let text = part.get("text").and_then(Value::as_str).unwrap_or("");
    let signature = part.get("thoughtSignature").and_then(Value::as_str);
    let is_thinking = part.get("thought").and_then(Value::as_bool) == Some(true);
    let want = if is_thinking {
        CurrentKind::Thinking
    } else {
        CurrentKind::Text
    };

    // Transition: close the current block + open a new one when the kind changes.
    if dec.current != Some(want) {
        if !close_current(dec, model, api, sink).await {
            return false;
        }
        if is_thinking {
            dec.blocks.push(Content::thinking(""));
            dec.current = Some(CurrentKind::Thinking);
            let idx = dec.block_index();
            let partial = dec.snapshot(model, api);
            if !sink
                .send(StreamEvent::ThinkingStart {
                    content_index: idx,
                    partial,
                })
                .await
            {
                return false;
            }
        } else {
            dec.blocks.push(Content::text(""));
            dec.current = Some(CurrentKind::Text);
            let idx = dec.block_index();
            let partial = dec.snapshot(model, api);
            if !sink
                .send(StreamEvent::TextStart {
                    content_index: idx,
                    partial,
                })
                .await
            {
                return false;
            }
        }
    }

    let idx = dec.block_index();
    match dec.blocks.get_mut(idx) {
        Some(Content::Thinking {
            thinking,
            thinking_signature,
            ..
        }) => {
            thinking.push_str(text);
            if let Some(s) = retain_signature(thinking_signature.as_deref(), signature) {
                *thinking_signature = Some(s);
            }
            let partial = dec.snapshot(model, api);
            sink.send(StreamEvent::ThinkingDelta {
                content_index: idx,
                delta: text.to_string(),
                partial,
            })
            .await
        }
        Some(Content::Text {
            text: acc,
            text_signature,
        }) => {
            acc.push_str(text);
            if let Some(s) = retain_signature(text_signature.as_deref(), signature) {
                *text_signature = Some(s);
            }
            let partial = dec.snapshot(model, api);
            sink.send(StreamEvent::TextDelta {
                content_index: idx,
                delta: text.to_string(),
                partial,
            })
            .await
        }
        _ => true,
    }
}

/// Handle a function-call part (Pi google-generative-ai.ts:160-205).
async fn process_function_call(
    part: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    // Close any open text/thinking block first.
    if !close_current(dec, model, api, sink).await {
        return false;
    }

    let fc = match part.get("functionCall") {
        Some(fc) => fc,
        None => return true,
    };
    let name = fc
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let provided_id = fc
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());

    // Unique-id synthesis (Pi google-generative-ai.ts:181-186): mint a new id when absent or a dup.
    let dup = provided_id
        .map(|pid| {
            dec.blocks
                .iter()
                .any(|b| matches!(b, Content::ToolCall(tc) if tc.id.as_str() == pid))
        })
        .unwrap_or(false);
    let tool_call_id = match provided_id {
        Some(pid) if !dup => pid.to_string(),
        _ => {
            let n = TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
            format!("{name}_{}_{n}", now_millis())
        }
    };

    let arguments = fc
        .get("args")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let thought_signature = part
        .get("thoughtSignature")
        .and_then(Value::as_str)
        .map(str::to_string);

    let tool_call = ToolCall {
        id: ToolCallId::from(tool_call_id.as_str()),
        name,
        arguments: arguments.into(),
        thought_signature,
    };

    dec.blocks.push(Content::ToolCall(tool_call.clone()));
    let idx = dec.block_index();

    let partial = dec.snapshot(model, api);
    if !sink
        .send(StreamEvent::ToolCallStart {
            content_index: idx,
            partial,
        })
        .await
    {
        return false;
    }
    let delta = serde_json::to_string(&tool_call.arguments)
        .unwrap_or_else(|_| "{}".to_string());
    let partial = dec.snapshot(model, api);
    if !sink
        .send(StreamEvent::ToolCallDelta {
            content_index: idx,
            delta,
            partial,
        })
        .await
    {
        return false;
    }
    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::ToolCallEnd {
        content_index: idx,
        tool_call,
        partial,
    })
    .await
}

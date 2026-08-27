//! Response decoding — one `CompletionChunk`: the `delta.content` walk over string / `text` /
//! `thinking` chunks (Pi mistral-conversations.ts:325-416).

use crate::api::compat::sanitize_surrogates;
use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use cyrup_core::{ApiId, Content};
use serde_json::Value;
use super::blocks::{close_current, process_tool_call};
use super::decoder::{CurrentKind, Decoder};
use super::finish::{apply_usage, map_chat_stop_reason};

/// Process one decoded `CompletionChunk`. Returns `false` if the consumer dropped the stream.
pub(super) async fn process_chunk(
    chunk: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    if dec.response_id.is_none()
        && let Some(id) = chunk.get("id").and_then(Value::as_str)
        && !id.is_empty()
    {
        dec.response_id = Some(id.to_string());
    }

    if let Some(usage) = chunk.get("usage") {
        apply_usage(&mut dec.usage, usage);
    }

    let choice = match chunk.get("choices").and_then(|c| c.get(0)) {
        Some(c) => c,
        None => return true,
    };

    // Pi guards with `if (choice.finishReason)` (mistral-conversations.ts:355) — a JS TRUTHINESS
    // test, so `null`, `undefined` and `""` all leave `output.stopReason` at its `"pending"` seed
    // and end the stream as truncated. The previous `else if is_null → map(None)` branch settled
    // such a stream on a clean `Stop`, which is the PROV-010 defect in its second form: a Mistral
    // stream whose final chunk carries `"finishReason": null` was transcribed as a completed turn.
    if let Some(reason) = choice
        .get("finishReason")
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty())
    {
        // pi records the raw reason first (`v0.84.1 ai/src/api/mistral-conversations.ts:356`), so a
        // `content_filter` / future reason names itself on the turn even after the narrowing map.
        dec.raw_stop_reason = Some(reason.to_string());
        let (stop, err) = map_chat_stop_reason(Some(reason));
        dec.stop_reason = Some(stop);
        if let Some(err) = err {
            dec.error_message = Some(err);
        }
    }

    let delta = match choice.get("delta") {
        Some(d) => d,
        None => return true,
    };

    // Content (string OR an array of content chunks).
    if let Some(content) = delta.get("content").filter(|c| !c.is_null())
        && !process_content(content, dec, model, api, sink).await
    {
        return false;
    }

    // Tool calls.
    if let Some(tool_calls) = delta.get("toolCalls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            if !process_tool_call(tool_call, dec, model, api, sink).await {
                return false;
            }
        }
    }

    true
}

/// Handle a `delta.content` value (Pi mistral-conversations.ts:355-416).
async fn process_content(
    content: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    // `string` content collapses to a single text item.
    if let Some(s) = content.as_str() {
        return push_text(dec, model, api, sink, &sanitize_surrogates(s)).await;
    }
    let Some(items) = content.as_array() else {
        return true;
    };
    for item in items {
        if let Some(s) = item.as_str() {
            if !push_text(dec, model, api, sink, &sanitize_surrogates(s)).await {
                return false;
            }
            continue;
        }
        match item.get("type").and_then(Value::as_str) {
            Some("thinking") => {
                let text = item
                    .get("thinking")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p.get("text").and_then(Value::as_str))
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                let delta = sanitize_surrogates(&text);
                if delta.is_empty() {
                    continue;
                }
                if !push_thinking(dec, model, api, sink, &delta).await {
                    return false;
                }
            }
            Some("text") => {
                let text = item.get("text").and_then(Value::as_str).unwrap_or("");
                if !push_text(dec, model, api, sink, &sanitize_surrogates(text)).await {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// Append a text delta, opening/closing blocks as needed.
async fn push_text(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    delta: &str,
) -> bool {
    if dec.current != Some(CurrentKind::Text) {
        if !close_current(dec, model, api, sink).await {
            return false;
        }
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
    let idx = dec.block_index();
    if let Some(Content::Text { text, .. }) = dec.blocks.get_mut(idx) {
        text.push_str(delta);
    }
    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::TextDelta {
        content_index: idx,
        delta: delta.to_string(),
        partial,
    })
    .await
}

/// Append a thinking delta, opening/closing blocks as needed.
async fn push_thinking(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    delta: &str,
) -> bool {
    if dec.current != Some(CurrentKind::Thinking) {
        if !close_current(dec, model, api, sink).await {
            return false;
        }
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
    }
    let idx = dec.block_index();
    if let Some(Content::Thinking { thinking, .. }) = dec.blocks.get_mut(idx) {
        thinking.push_str(delta);
    }
    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::ThinkingDelta {
        content_index: idx,
        delta: delta.to_string(),
        partial,
    })
    .await
}

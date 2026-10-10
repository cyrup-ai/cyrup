//! Response decoding — one `CompletionChunk`: the `delta.content` walk over string / `text` /
//! `thinking` chunks, read in Mistral's wire keys (Pi `consumeChatStream`,
//! `mistral-conversations.ts:576-757` @f1b2e77f5; PROV-152).

use super::blocks::{close_current, process_tool_call};
use super::decoder::{CurrentKind, Decoder};
use super::finish::{apply_usage, map_chat_stop_reason};
use crate::api::EventSink;
use crate::api::compat::sanitize_surrogates;
use crate::model::Model;
use crate::stream::StreamEvent;
use cyrup_core::{ApiId, Content};
use serde_json::Value;

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

    // Pi guards with `if (choice.finish_reason)` (`mistral-conversations.ts:633` @f1b2e77f5) — a
    // JS TRUTHINESS test, so `null`, `undefined` and `""` all leave `output.stopReason` at its
    // `"pending"` seed and end the stream as truncated. The previous `else if is_null → map(None)`
    // branch settled such a stream on a clean `Stop`, which is the PROV-010 defect in its second
    // form: a Mistral stream whose final chunk carries `"finish_reason": null` was transcribed as a
    // completed turn.
    //
    // PROV-152: the key is Mistral's wire `finish_reason`. This read `finishReason` — the old SDK's
    // TypeScript name, which its zod schema remapped from the wire — so against the real API no
    // finish reason was ever seen and every turn ended "without a finish reason".
    if let Some(reason) = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty())
    {
        // pi records the raw reason first (`mistral-conversations.ts:634` @f1b2e77f5), so a
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

    // Tool calls — Pi `const toolCalls = delta.tool_calls || []` (`:710` @f1b2e77f5). PROV-152:
    // this read the SDK's `toolCalls`, so streamed tool calls were dropped against the real API.
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            if !process_tool_call(tool_call, dec, model, api, sink).await {
                return false;
            }
        }
    }

    true
}

/// Handle a `delta.content` value (Pi mistral-conversations.ts:629-693).
///
/// PROV-115 — every text site drops an EMPTY delta before it can open a block. Upstream guards both
/// of its text branches (`mistral-conversations.ts:636` and `:678` @v1.0.0, commit `8930b9ec0`
/// "ignore empty Mistral content deltas") with the reason in the comment: *"GLM models on Mistral
/// send empty content deltas around thinking and tool calls. Opening a block for them splits
/// thinking into multiple blocks, which Mistral rejects on replay."* cyrup ships `zai-glm-5-2` and
/// `zai-glm-5-3` in `providers/catalog/mistral.json`, so without the guard a turn on either model
/// lands `thinking / text:"" / thinking`; `messages.rs` then replays each thinking block as its own
/// assistant `"thinking"` entry and Mistral rejects the body, breaking every later turn while that
/// message stays in context. The guard is at the three CALL SITES, not inside [`push_text`]: a
/// caller that legitimately wants an empty text block must not be silenced invisibly.
async fn process_content(
    content: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    // `string` content collapses to a single text item. Upstream reaches this through
    // `typeof delta.content === "string" ? [delta.content] : delta.content`
    // (`mistral-conversations.ts:630`), so the string case runs the SAME guarded string-item branch
    // as an array element — hence the same emptiness test here (PROV-115).
    if let Some(s) = content.as_str() {
        let delta = sanitize_surrogates(s);
        if delta.is_empty() {
            return true;
        }
        return push_text(dec, model, api, sink, &delta).await;
    }
    let Some(items) = content.as_array() else {
        return true;
    };
    for item in items {
        if let Some(s) = item.as_str() {
            let delta = sanitize_surrogates(s);
            if delta.is_empty() {
                continue;
            }
            if !push_text(dec, model, api, sink, &delta).await {
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
                let delta = sanitize_surrogates(text);
                if delta.is_empty() {
                    continue;
                }
                if !push_text(dec, model, api, sink, &delta).await {
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
        dec.push_block(Content::text(""));
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
    if let Some(Content::Text { text, .. }) = dec.block_mut(idx) {
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
        dec.push_block(Content::thinking(""));
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
    if let Some(Content::Thinking { thinking, .. }) = dec.block_mut(idx) {
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

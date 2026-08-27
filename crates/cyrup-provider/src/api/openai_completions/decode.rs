//! Response decoding: the SSE frame loop and the per-chunk dispatch.

use super::blocks::{Block, Decoder};
use super::deltas::{
    encrypted_reasoning_detail_id, ensure_text_block, ensure_thinking_block, first_reasoning_delta,
    process_tool_call_delta,
};
use super::finalize::{build_final_message, map_stop_reason, parse_partial_json, parse_usage};
use crate::api::EventSink;
use crate::api::compat::get_compat;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::StreamEvent;
use crate::stream::sse::SseFrame;
use cyrup_core::{ApiId, Content, StopReason, ToolCall, ToolCallId};
use futures::{Stream, StreamExt};
use serde_json::Value;

/// Reasoning delta field names emitted by OpenAI-compatible endpoints (first non-empty wins).
pub(super) const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];

/// Drive the SSE frame stream into ordered [`StreamEvent`]s pushed to `sink`. Emits `Start` first,
/// then per-block `*Start/*Delta/*End`, then exactly one terminal (`Done`/`Error`).
pub(crate) async fn decode_stream<S>(mut frames: S, model: &Model, api: &ApiId, sink: &EventSink)
where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    let provider = model.provider.clone();
    let model_id = model.id.as_str().to_string();

    let mut dec = Decoder::default();

    if !sink
        .send(StreamEvent::Start {
            partial: dec.snapshot(model, api),
        })
        .await
    {
        return;
    }

    while let Some(frame) = frames.next().await {
        let frame = match frame {
            Ok(f) => f,
            Err(e) => {
                // transport/decode/abort mid-stream → terminal Error (R-01-018/044/045)
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };
        let data = frame.data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            break;
        }
        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            // Be robust to keep-alive / non-JSON comment frames.
            Err(_) => continue,
        };
        if !process_chunk(&chunk, &mut dec, model, api, sink).await {
            return; // consumer dropped
        }
    }

    // Finalize each open block in appearance order. The `partial` snapshot reflects all assembled
    // blocks (Pi `finishBlock` pushes `*_end` with `partial: output`, openai-completions.ts:214-246).
    let block_count = dec.blocks.len();
    for idx in 0..block_count {
        let partial = dec.snapshot(model, api);
        let ev = match dec.blocks.get(idx) {
            Some(Block::Text(text)) => StreamEvent::TextEnd {
                content_index: idx,
                content: text.clone(),
                partial,
            },
            Some(Block::Thinking { text, .. }) => StreamEvent::ThinkingEnd {
                content_index: idx,
                content: text.clone(),
                partial,
            },
            Some(Block::Tool {
                id,
                name,
                args,
                thought_signature,
            }) => StreamEvent::ToolCallEnd {
                content_index: idx,
                tool_call: ToolCall {
                    id: ToolCallId::from(id.as_str()),
                    name: name.clone(),
                    arguments: parse_partial_json(args),
                    thought_signature: thought_signature.clone(),
                },
                partial,
            },
            None => continue,
        };
        if !sink.send(ev).await {
            return;
        }
    }

    let saw_finish_reason = dec.saw_finish_reason;
    let settled = dec.stop_reason;
    let message = build_final_message(dec, model, api);

    // Which stop reason the provider actually DELIVERED — `None` is Pi's still-`"pending"` output.
    // Pi's end-of-stream ladder (v0.84.1 `ai/src/api/openai-completions.ts:571-586`):
    //   1. `aborted`/`error` already settled by an abort or an error chunk → throw with THAT
    //      message, so the reason and the recorded `error_message` are used verbatim;
    //   2. a `finish_reason` actually arrived → use it;
    //   3. `!hasFinishReason && !compat.supportsFinishReason` (`:578-580`) → the provider never
    //      reports one, so INFER: `toolUse` when the turn produced a tool call, else `stop`;
    //   4. otherwise `(supportsFinishReason && !hasFinishReason) || stopReason === "pending"`
    //      (`:584-586`) → throw "Stream ended without finish_reason".
    //
    // VERSION LAG (v0.83.0 → v0.84.1): at v0.83.0 (`openai-completions.ts:577`) step 4 was the
    // unconditional `if (!hasFinishReason || output.stopReason === "pending")` and there was no
    // `supportsFinishReason` compat key at all (absent from v0.83.0 `ai/src/types.ts`), so a
    // provider that never sends `finish_reason` always produced the truncated-stream error.
    //
    // Step 3 cannot mask a settled `error`: pi only assigns `stopReason = "error"` from
    // `mapStopReason` (`:465`), which also sets `hasFinishReason = true` (`:469`), so the inference
    // branch is unreachable whenever the reason is `error`.
    let delivered = match settled {
        Some(r @ (StopReason::Error | StopReason::Aborted)) => Some(r),
        other if saw_finish_reason => other,
        _ if !get_compat(model).supports_finish_reason => Some(
            if message
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall(_)))
            {
                StopReason::ToolUse
            } else {
                StopReason::Stop
            },
        ),
        _ => None,
    };

    sink.send(StreamEvent::end_of_stream(
        message,
        delivered,
        "Stream ended without finish_reason",
    ))
    .await;
}

/// Process one decoded chunk. Returns `false` if the consumer dropped the stream.
async fn process_chunk(
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
    if dec.response_model.is_none()
        && let Some(m) = chunk.get("model").and_then(Value::as_str)
        && !m.is_empty()
        && m != model.id.as_str()
    {
        dec.response_model = Some(m.to_string());
    }
    if let Some(usage) = chunk.get("usage").filter(|u| !u.is_null()) {
        dec.usage = Some(parse_usage(usage, model));
    }

    // Provider error chunk (e.g. OpenRouter streams `{"error": {...}}` instead of throwing). Pi
    // surfaces this as the OpenAI SDK throwing; the catch block sets `errorMessage` and, when the
    // error carries `error.metadata.raw`, appends it (openai-completions.ts:466-469).
    if let Some(err) = chunk.get("error").filter(|e| !e.is_null()) {
        let mut message = err
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "Provider returned an error".to_string());
        if let Some(raw) = err.get("metadata").and_then(|m| m.get("raw")) {
            let raw_str = raw
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| raw.to_string());
            if !raw_str.is_empty() {
                message.push('\n');
                message.push_str(&raw_str);
            }
        }
        dec.stop_reason = Some(StopReason::Error);
        dec.error_message = Some(message);
        return true;
    }

    let choice = match chunk
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
    {
        Some(c) => c,
        None => return true,
    };

    // Some providers (e.g. Moonshot) place usage on the choice instead of the chunk.
    if dec.usage.is_none()
        && let Some(usage) = choice.get("usage").filter(|u| !u.is_null())
    {
        dec.usage = Some(parse_usage(usage, model));
    }

    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        // pi records the raw reason first (`v0.84.1 ai/src/api/openai-completions.ts:463`), before
        // the narrowing map, so `content_filter` and every provider-specific reason survive on the
        // turn instead of collapsing into `StopReason::Error`.
        dec.raw_stop_reason = Some(reason.to_string());
        let (stop, err) = map_stop_reason(reason);
        dec.stop_reason = Some(stop);
        if let Some(err) = err {
            dec.error_message = Some(err);
        }
        dec.saw_finish_reason = true;
    }

    let delta = match choice.get("delta") {
        Some(d) if d.is_object() => d,
        _ => return true,
    };

    // 1. Text content.
    if let Some(text) = delta.get("content").and_then(Value::as_str)
        && !text.is_empty()
    {
        let idx = match ensure_text_block(dec, model, api, sink).await {
            Some(idx) => idx,
            None => return false,
        };
        if let Some(Block::Text(buf)) = dec.blocks.get_mut(idx) {
            buf.push_str(text);
        }
        let partial = dec.snapshot(model, api);
        if !sink
            .send(StreamEvent::TextDelta {
                content_index: idx,
                delta: text.to_string(),
                partial,
            })
            .await
        {
            return false;
        }
    }

    // 2. Reasoning / thinking content (first non-empty reasoning field).
    if let Some((field, reason_text)) = first_reasoning_delta(delta)
        && !reason_text.is_empty()
    {
        // The thinking signature records which field carried the reasoning, so a same-model replay
        // can echo it back under the same key (Pi `thinkingSignature` logic).
        let signature = if model.provider.as_str() == "opencode-go" && field == "reasoning" {
            "reasoning_content"
        } else {
            field
        };
        let idx = match ensure_thinking_block(dec, signature, model, api, sink).await {
            Some(idx) => idx,
            None => return false,
        };
        if let Some(Block::Thinking { text, .. }) = dec.blocks.get_mut(idx) {
            text.push_str(reason_text);
        }
        let partial = dec.snapshot(model, api);
        if !sink
            .send(StreamEvent::ThinkingDelta {
                content_index: idx,
                delta: reason_text.to_string(),
                partial,
            })
            .await
        {
            return false;
        }
    }

    // 3. Streamed tool calls.
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tool_calls {
            if !process_tool_call_delta(tc, dec, model, api, sink).await {
                return false;
            }
        }
    }

    // 4. Encrypted reasoning details — attach as the thought signature of the matching tool call,
    // or stash until that tool call appears (Pi `reasoning_details` handling, L422-435).
    if let Some(details) = delta.get("reasoning_details").and_then(Value::as_array) {
        for detail in details {
            if let Some(id) = encrypted_reasoning_detail_id(detail) {
                let serialized = detail.to_string();
                if let Some(&idx) = dec.tool_by_id.get(id) {
                    if let Some(Block::Tool {
                        thought_signature, ..
                    }) = dec.blocks.get_mut(idx)
                    {
                        *thought_signature = Some(serialized);
                    }
                } else {
                    dec.pending_reasoning_by_tool_id
                        .insert(id.to_string(), serialized);
                }
            }
        }
    }

    true
}

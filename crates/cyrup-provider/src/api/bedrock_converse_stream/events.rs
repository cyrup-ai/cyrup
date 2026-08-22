//! Response decoding (pi `handleContentBlock*`, `bedrock-converse-stream.ts:451-573`).

use super::blocks::{Block, Decoder};
use super::errors::{
    bedrock_error_prefix, data_retention_hint, format_bedrock_error, map_stop_reason, upper_first,
};
use super::framing::EventFrame;
use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use crate::utils::json_parse::parse_streaming_json_object;
use cyrup_core::{ApiId, ToolCall, ToolCallId};
use serde_json::Value;

/// Dispatch one decoded event-stream frame (pi's `for await (const item of response.stream!)` body,
/// `bedrock-converse-stream.ts:257-289`).
///
/// `Ok(false)` means the consumer dropped the stream; `Err(message)` is one of upstream's five
/// `throw item.<x>Exception` arms.
pub(super) async fn dispatch_frame(
    frame: &EventFrame,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> Result<bool, String> {
    // An `:message-type: exception` frame is upstream's `item.<x>Exception` throw.
    if frame.header(":message-type").as_deref() == Some("exception") {
        let name = frame
            .header(":exception-type")
            .map(|t| upper_first(&t))
            .unwrap_or_else(|| "BedrockRuntimeServiceException".to_string());
        let message = frame
            .json()
            .and_then(|v| {
                v.get("message")
                    .or_else(|| v.get("Message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let core = if message.is_empty() {
            name.clone()
        } else {
            message
        };
        let hint = data_retention_hint(&core);
        let prefix = bedrock_error_prefix(&name).unwrap_or(name.as_str());
        return Err(format!("{prefix}: {core}{hint}"));
    }

    let Some(event_type) = frame.header(":event-type") else {
        return Ok(true);
    };
    let payload = frame.json().unwrap_or(Value::Null);

    match event_type.as_str() {
        "messageStart" => {
            // pi `:258-262`: a non-assistant role is fatal.
            if payload.get("role").and_then(Value::as_str) != Some("assistant") {
                return Err(format_bedrock_error(
                    "Unexpected assistant message start but got user message start instead",
                ));
            }
            Ok(sink
                .send(StreamEvent::Start {
                    partial: dec.snapshot(model, api),
                })
                .await)
        }
        "contentBlockStart" => Ok(handle_content_block_start(&payload, dec, model, api, sink).await),
        "contentBlockDelta" => Ok(handle_content_block_delta(&payload, dec, model, api, sink).await),
        "contentBlockStop" => Ok(handle_content_block_stop(&payload, dec, model, api, sink).await),
        "messageStop" => {
            let raw = payload.get("stopReason").and_then(Value::as_str);
            // pi `output.rawStopReason = item.messageStop.stopReason` (`v0.84.1
            // ai/src/api/bedrock-converse-stream.ts:276`) — recorded before the narrowing map, so
            // `guardrail_intervened` and every future reason name themselves on the turn.
            dec.raw_stop_reason = raw.map(str::to_string);
            let (stop_reason, error_message) = map_stop_reason(raw);
            dec.stop_reason = Some(stop_reason);
            if let Some(message) = error_message {
                dec.error_message = Some(message);
            }
            Ok(true)
        }
        "metadata" => {
            handle_metadata(&payload, dec);
            Ok(true)
        }
        _ => Ok(true),
    }
}

/// pi `handleContentBlockStart` (`bedrock-converse-stream.ts:451-472`). Only `toolUse` starts a
/// block; text and reasoning blocks are created lazily by the first delta.
async fn handle_content_block_start(
    payload: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = payload
        .get("contentBlockIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let Some(tool_use) = payload.get("start").and_then(|s| s.get("toolUse")) else {
        return true;
    };
    dec.blocks.push(Block::Tool {
        index,
        id: tool_use
            .get("toolUseId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        name: tool_use
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        partial_json: String::new(),
    });
    let content_index = dec.blocks.len().saturating_sub(1);
    sink.send(StreamEvent::ToolCallStart {
        content_index,
        partial: dec.snapshot(model, api),
    })
    .await
}

/// pi `handleContentBlockDelta` (`bedrock-converse-stream.ts:474-530`).
async fn handle_content_block_delta(
    payload: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = payload
        .get("contentBlockIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let delta = payload.get("delta");
    let position = dec.position_of(index);

    if let Some(text) = delta.and_then(|d| d.get("text")).and_then(Value::as_str) {
        let position = match position {
            Some(p) => p,
            None => {
                // pi `:486-493`: no `contentBlockStart` is sent for text blocks.
                dec.blocks.push(Block::Text {
                    index,
                    text: String::new(),
                });
                let content_index = dec.blocks.len().saturating_sub(1);
                if !sink
                    .send(StreamEvent::TextStart {
                        content_index,
                        partial: dec.snapshot(model, api),
                    })
                    .await
                {
                    return false;
                }
                content_index
            }
        };
        if let Some(Block::Text { text: buf, .. }) = dec.blocks.get_mut(position) {
            buf.push_str(text);
        } else {
            return true;
        }
        return sink
            .send(StreamEvent::TextDelta {
                content_index: position,
                delta: text.to_string(),
                partial: dec.snapshot(model, api),
            })
            .await;
    }

    if let Some(tool_use) = delta.and_then(|d| d.get("toolUse")) {
        let Some(position) = position else {
            return true;
        };
        let input = tool_use
            .get("input")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match dec.blocks.get_mut(position) {
            Some(Block::Tool { partial_json, .. }) => partial_json.push_str(&input),
            // pi guards on `block?.type === "toolCall"`; any other block type is ignored.
            _ => return true,
        }
        return sink
            .send(StreamEvent::ToolCallDelta {
                content_index: position,
                delta: input,
                partial: dec.snapshot(model, api),
            })
            .await;
    }

    if let Some(reasoning) = delta.and_then(|d| d.get("reasoningContent")) {
        let position = match position {
            Some(p) => p,
            None => {
                dec.blocks.push(Block::Thinking {
                    index,
                    thinking: String::new(),
                    signature: String::new(),
                });
                let content_index = dec.blocks.len().saturating_sub(1);
                if !sink
                    .send(StreamEvent::ThinkingStart {
                        content_index,
                        partial: dec.snapshot(model, api),
                    })
                    .await
                {
                    return false;
                }
                content_index
            }
        };
        // pi `:514`: everything below is guarded on the block actually being a thinking block.
        if !matches!(dec.blocks.get(position), Some(Block::Thinking { .. })) {
            return true;
        }
        if let Some(text) = reasoning.get("text").and_then(Value::as_str)
            && !text.is_empty()
        {
            if let Some(Block::Thinking { thinking, .. }) = dec.blocks.get_mut(position) {
                thinking.push_str(text);
            }
            if !sink
                .send(StreamEvent::ThinkingDelta {
                    content_index: position,
                    delta: text.to_string(),
                    partial: dec.snapshot(model, api),
                })
                .await
            {
                return false;
            }
        }
        // pi `:524-527`: the signature accumulates silently — no event is emitted for it.
        if let Some(sig) = reasoning.get("signature").and_then(Value::as_str)
            && !sig.is_empty()
            && let Some(Block::Thinking { signature, .. }) = dec.blocks.get_mut(position)
        {
            signature.push_str(sig);
        }
    }

    true
}

/// pi `handleContentBlockStop` (`bedrock-converse-stream.ts:547-573`).
async fn handle_content_block_stop(
    payload: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = payload
        .get("contentBlockIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    // pi `:555`: an unknown index is a no-op, not an error.
    let Some(position) = dec.position_of(index) else {
        return true;
    };
    let event = match dec.blocks.get(position) {
        Some(Block::Text { text, .. }) => StreamEvent::TextEnd {
            content_index: position,
            content: text.clone(),
            partial: dec.snapshot(model, api),
        },
        Some(Block::Thinking { thinking, .. }) => StreamEvent::ThinkingEnd {
            content_index: position,
            content: thinking.clone(),
            partial: dec.snapshot(model, api),
        },
        Some(Block::Tool {
            id,
            name,
            partial_json,
            ..
        }) => StreamEvent::ToolCallEnd {
            content_index: position,
            tool_call: ToolCall {
                id: ToolCallId::from(id.as_str()),
                name: name.clone(),
                arguments: parse_streaming_json_object(Some(partial_json)),
                thought_signature: None,
            },
            partial: dec.snapshot(model, api),
        },
        None => return true,
    };
    sink.send(event).await
}

/// pi `handleMetadata` (`bedrock-converse-stream.ts:532-545`).
fn handle_metadata(payload: &Value, dec: &mut Decoder) {
    let Some(usage) = payload.get("usage") else {
        return;
    };
    let n = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    dec.usage.input = n("inputTokens");
    dec.usage.output = n("outputTokens");
    dec.usage.cache_read = n("cacheReadInputTokens");
    dec.usage.cache_write = n("cacheWriteInputTokens");
    let total = n("totalTokens");
    dec.usage.total_tokens = if total == 0 {
        dec.usage.input.saturating_add(dec.usage.output)
    } else {
        total
    };
}

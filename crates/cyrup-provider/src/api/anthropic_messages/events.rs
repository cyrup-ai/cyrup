//! Response decoding — per-event block handling and event emission.

use super::blocks::{Block, Decoder};
use super::claude_code::remap_decoded_tool_name;
use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use crate::utils::json_parse::parse_streaming_json_object;
use cyrup_core::{ApiId, AssistantMessage, SharedStr, StopReason, ToolCall, ToolCallId};
use serde_json::Value;
use std::sync::Arc;

pub(super) async fn process_block_start(
    event: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
    let cb = match event.get("content_block") {
        Some(c) => c,
        None => return true,
    };
    match cb.get("type").and_then(Value::as_str) {
        Some("text") => {
            // Seed from the payload Anthropic ships on the open event (Pi
            // `text: event.content_block.text ?? ""`, anthropic-messages.ts:591). Dropping it loses
            // the first chunk of the block whenever the server front-loads text here.
            dec.push_block(Block::Text {
                index,
                text: cb.get("text").and_then(Value::as_str).unwrap_or("").into(),
            });
            send_with_pos(dec, model, api, sink, |pos, partial| {
                StreamEvent::TextStart {
                    content_index: pos,
                    partial,
                }
            })
            .await
        }
        Some("thinking") => {
            // Same seeding for thinking (Pi `thinking: event.content_block.thinking ?? ""`,
            // `thinkingSignature: event.content_block.signature ?? ""`, anthropic-messages.ts:
            // 599-600). The signature especially: a thinking block replayed back to Anthropic
            // without its signature is rejected, so a server that delivers the signature on the
            // open event (and never as a `signature_delta`) must not have it discarded.
            dec.push_block(Block::Thinking {
                index,
                thinking: cb
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                signature: cb
                    .get("signature")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                redacted: false,
            });
            send_with_pos(dec, model, api, sink, |pos, partial| {
                StreamEvent::ThinkingStart {
                    content_index: pos,
                    partial,
                }
            })
            .await
        }
        Some("redacted_thinking") => {
            let data = cb
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            dec.push_block(Block::Thinking {
                index,
                thinking: "[Reasoning redacted]".into(),
                signature: data,
                redacted: true,
            });
            send_with_pos(dec, model, api, sink, |pos, partial| {
                StreamEvent::ThinkingStart {
                    content_index: pos,
                    partial,
                }
            })
            .await
        }
        Some("tool_use") => {
            let id = cb
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let raw_name = cb.get("name").and_then(Value::as_str).unwrap_or("");
            // OAuth: map the Claude-Code tool name back to the caller's declared name (Pi decode,
            // anthropic-messages.ts:592-594).
            let name = if dec.is_oauth {
                remap_decoded_tool_name(&dec.tool_names, raw_name)
            } else {
                raw_name.to_string()
            };
            dec.push_block(Block::Tool {
                index,
                id,
                name,
                partial_json: SharedStr::new(),
            });
            send_with_pos(dec, model, api, sink, |pos, partial| {
                StreamEvent::ToolCallStart {
                    content_index: pos,
                    partial,
                }
            })
            .await
        }
        _ => true,
    }
}

pub(super) async fn process_block_delta(
    event: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
    let delta = match event.get("delta") {
        Some(d) => d,
        None => return true,
    };
    let pos = match dec.position_of(index) {
        Some(p) => p,
        None => return true,
    };
    match delta.get("type").and_then(Value::as_str) {
        Some("text_delta") => {
            let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
            if let Some(Block::Text { text: acc, .. }) = dec.block_mut(pos) {
                acc.push_str(text);
            }
            let partial = dec.snapshot(model, api);
            sink.send(StreamEvent::TextDelta {
                content_index: pos,
                delta: text.to_string(),
                partial,
            })
            .await
        }
        Some("thinking_delta") => {
            let text = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
            if let Some(Block::Thinking { thinking, .. }) = dec.block_mut(pos) {
                thinking.push_str(text);
            }
            let partial = dec.snapshot(model, api);
            sink.send(StreamEvent::ThinkingDelta {
                content_index: pos,
                delta: text.to_string(),
                partial,
            })
            .await
        }
        Some("input_json_delta") => {
            let text = delta
                .get("partial_json")
                .and_then(Value::as_str)
                .unwrap_or("");
            if let Some(Block::Tool { partial_json, .. }) = dec.block_mut(pos) {
                // O(delta): the append is amortised and no parse happens here at all — see
                // [`SharedStr`] and [`LazyArgs`](cyrup_core::LazyArgs) (PERF-001).
                partial_json.push_str(text);
            }
            let partial = dec.snapshot(model, api);
            sink.send(StreamEvent::ToolCallDelta {
                content_index: pos,
                delta: text.to_string(),
                partial,
            })
            .await
        }
        Some("signature_delta") => {
            let sig = delta.get("signature").and_then(Value::as_str).unwrap_or("");
            if let Some(Block::Thinking { signature, .. }) = dec.block_mut(pos) {
                signature.push_str(sig);
            }
            true // signature deltas do not emit a stream event (Pi anthropic-messages.ts:640-647)
        }
        _ => true,
    }
}

pub(super) async fn process_block_stop(
    event: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
    let pos = match dec.position_of(index) {
        Some(p) => p,
        None => return true,
    };
    let partial = dec.snapshot(model, api);
    let ev = match dec.blocks.get(pos) {
        Some(Block::Text { text, .. }) => StreamEvent::TextEnd {
            content_index: pos,
            content: text.to_string(),
            partial,
        },
        Some(Block::Thinking { thinking, .. }) => StreamEvent::ThinkingEnd {
            content_index: pos,
            content: thinking.to_string(),
            partial,
        },
        Some(Block::Tool {
            id,
            name,
            partial_json,
            ..
        }) => StreamEvent::ToolCallEnd {
            content_index: pos,
            tool_call: ToolCall {
                id: ToolCallId::from(id.as_str()),
                name: name.clone(),
                arguments: parse_streaming_json_object(Some(partial_json)).into(),
                thought_signature: None,
            },
            partial,
        },
        None => return true,
    };
    sink.send(ev).await
}

/// Push a `*_start` event for the just-pushed block (its position is `len-1`).
async fn send_with_pos<F>(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    make: F,
) -> bool
where
    F: FnOnce(usize, Arc<AssistantMessage>) -> StreamEvent,
{
    let pos = dec.blocks.len().saturating_sub(1);
    let partial = dec.snapshot(model, api);
    sink.send(make(pos, partial)).await
}

/// Emit any block `*_end` events the stream did not already close (no-op when all closed cleanly,
/// which is the normal path — Anthropic always sends `content_block_stop`).
pub(super) async fn finish_blocks(_dec: &Decoder, _model: &Model, _api: &ApiId, _sink: &EventSink) {
    // Anthropic always emits a `content_block_stop` per block, so the `*_end` events are already
    // sent by `process_block_stop`. This hook exists for symmetry with the openai-completions
    // decoder and is intentionally a no-op.
}

/// Emit a terminal error event carrying the partial snapshot's content (Pi's catch block,
/// anthropic-messages.ts:727-736).
pub(super) async fn emit_error(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    message: String,
) {
    let mut msg = dec.snapshot_owned(model, api);
    msg.stop_reason = StopReason::Error;
    msg.error_message = Some(message);
    sink.send(StreamEvent::terminal(msg)).await;
}

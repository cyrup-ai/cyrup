//! Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531):
//! per-event dispatch.

use super::blocks::{RBlock, blocks_to_content};
use super::decoder::RDecoder;
use super::finalize::finalize_response;
use super::slots::{SlotKind, create_slot, get_or_create_slot};
use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use cyrup_core::{ApiId, Content, TextPhase, TextSignatureV1, ToolCall, ToolCallId};
use serde_json::{Map, Value};

pub(super) enum ProcessResult {
    Continue,
    Dropped,
    Error(String),
}

pub(super) async fn process_event(
    event: &Value,
    dec: &mut RDecoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> ProcessResult {
    let etype = event.get("type").and_then(Value::as_str).unwrap_or("");
    let oi = event
        .get("output_index")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    macro_rules! emit {
        ($ev:expr) => {
            if !sink.send($ev).await {
                return ProcessResult::Dropped;
            }
        };
    }

    match etype {
        "response.created" => {
            if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                dec.response_id = Some(id.to_string());
            }
        }
        "response.output_item.added" => {
            if let Some(item) = event.get("item")
                && let Some((ci, kind)) = create_slot(dec, oi, item)
            {
                let ev = match kind {
                    SlotKind::Thinking => StreamEvent::ThinkingStart {
                        content_index: ci,
                        partial: dec.snapshot(model, api),
                    },
                    SlotKind::Text => StreamEvent::TextStart {
                        content_index: ci,
                        partial: dec.snapshot(model, api),
                    },
                    SlotKind::Tool => StreamEvent::ToolCallStart {
                        content_index: ci,
                        partial: dec.snapshot(model, api),
                    },
                };
                emit!(ev);
            }
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
            if let Some(ci) = dec.slot(oi, SlotKind::Thinking) {
                if let Some(RBlock::Thinking { thinking, .. }) = dec.blocks.get_mut(ci) {
                    thinking.push_str(delta);
                }
                emit!(StreamEvent::ThinkingDelta {
                    content_index: ci,
                    delta: delta.to_string(),
                    partial: dec.snapshot(model, api),
                });
            }
        }
        "response.reasoning_summary_part.done" => {
            if let Some(ci) = dec.slot(oi, SlotKind::Thinking) {
                if let Some(RBlock::Thinking { thinking, .. }) = dec.blocks.get_mut(ci) {
                    thinking.push_str("\n\n");
                }
                emit!(StreamEvent::ThinkingDelta {
                    content_index: ci,
                    delta: "\n\n".to_string(),
                    partial: dec.snapshot(model, api),
                });
            }
        }
        "response.output_text.delta" | "response.refusal.delta" => {
            let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
            if let Some(ci) = dec.slot(oi, SlotKind::Text) {
                if let Some(RBlock::Text { text, .. }) = dec.blocks.get_mut(ci) {
                    text.push_str(delta);
                }
                emit!(StreamEvent::TextDelta {
                    content_index: ci,
                    delta: delta.to_string(),
                    partial: dec.snapshot(model, api),
                });
            }
        }
        "response.function_call_arguments.delta" => {
            let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
            if let Some(ci) = dec.slot(oi, SlotKind::Tool) {
                if let Some(RBlock::Tool { partial_json, .. }) = dec.blocks.get_mut(ci) {
                    partial_json.push_str(delta);
                }
                emit!(StreamEvent::ToolCallDelta {
                    content_index: ci,
                    delta: delta.to_string(),
                    partial: dec.snapshot(model, api),
                });
            }
        }
        "response.function_call_arguments.done" => {
            let arguments = event.get("arguments").and_then(Value::as_str).unwrap_or("");
            if let Some(ci) = dec.slot(oi, SlotKind::Tool) {
                let mut maybe_delta: Option<String> = None;
                if let Some(RBlock::Tool { partial_json, .. }) = dec.blocks.get_mut(ci) {
                    let previous = partial_json.clone();
                    *partial_json = arguments.to_string();
                    if let Some(rest) = arguments
                        .strip_prefix(previous.as_str())
                        .filter(|r| !r.is_empty())
                    {
                        maybe_delta = Some(rest.to_string());
                    }
                }
                if let Some(delta) = maybe_delta {
                    emit!(StreamEvent::ToolCallDelta {
                        content_index: ci,
                        delta,
                        partial: dec.snapshot(model, api),
                    });
                }
            }
        }
        "response.output_item.done" => {
            let Some(item) = event.get("item") else {
                return ProcessResult::Continue;
            };
            let Some((ci, kind)) = get_or_create_slot(dec, oi, item, model, api, sink).await else {
                return ProcessResult::Continue;
            };
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            match (item_type, kind) {
                ("reasoning", SlotKind::Thinking) => {
                    let summary = join_text_array(item.get("summary"));
                    let content = join_text_array(item.get("content"));
                    let final_text = if !summary.is_empty() {
                        summary
                    } else if !content.is_empty() {
                        content
                    } else if let Some(RBlock::Thinking { thinking, .. }) = dec.blocks.get(ci) {
                        thinking.clone()
                    } else {
                        String::new()
                    };
                    let sig = serde_json::to_string(item).ok();
                    if let Some(RBlock::Thinking {
                        thinking,
                        signature,
                    }) = dec.blocks.get_mut(ci)
                    {
                        *thinking = final_text.clone();
                        *signature = sig;
                    }
                    dec.slots.remove(&oi);
                    emit!(StreamEvent::ThinkingEnd {
                        content_index: ci,
                        content: final_text,
                        partial: dec.snapshot(model, api),
                    });
                }
                ("message", SlotKind::Text) => {
                    let final_text = join_message_content(item.get("content"));
                    let id = item.get("id").and_then(Value::as_str).unwrap_or("");
                    let phase = item
                        .get("phase")
                        .and_then(Value::as_str)
                        .and_then(parse_phase);
                    let sig = TextSignatureV1::new(id, phase).encode();
                    if let Some(RBlock::Text { text, signature }) = dec.blocks.get_mut(ci) {
                        *text = final_text.clone();
                        *signature = Some(sig);
                    }
                    dec.slots.remove(&oi);
                    emit!(StreamEvent::TextEnd {
                        content_index: ci,
                        content: final_text,
                        partial: dec.snapshot(model, api),
                    });
                }
                ("function_call", SlotKind::Tool) => {
                    let raw = item.get("arguments").and_then(Value::as_str).unwrap_or("");
                    if let Some(RBlock::Tool { partial_json, .. }) = dec.blocks.get_mut(ci) {
                        if !raw.is_empty() {
                            *partial_json = raw.to_string();
                        } else if partial_json.is_empty() {
                            *partial_json = "{}".to_string();
                        }
                    }
                    let tool_call = match blocks_to_content(&dec.blocks).get(ci) {
                        Some(Content::ToolCall(tc)) => tc.clone(),
                        _ => ToolCall {
                            id: ToolCallId::from(""),
                            name: String::new(),
                            arguments: Map::new(),
                            thought_signature: None,
                        },
                    };
                    dec.slots.remove(&oi);
                    emit!(StreamEvent::ToolCallEnd {
                        content_index: ci,
                        tool_call,
                        partial: dec.snapshot(model, api),
                    });
                }
                _ => {}
            }
        }
        "response.completed" | "response.incomplete" => {
            finalize_response(event.get("response"), dec, model);
        }
        "error" => {
            let code = event.get("code").and_then(Value::as_str).unwrap_or("");
            let message = event
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Unknown error");
            return ProcessResult::Error(format!("Error Code {code}: {message}"));
        }
        "response.failed" => {
            dec.saw_terminal = true;
            let response = event.get("response");
            // `output.rawStopReason = event.response?.status` (v0.84.1
            // `openai-responses-shared.ts:726`; already present at `v0.83.0:721`).
            dec.raw_stop_reason = response
                .and_then(|r| r.get("status"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let error = response.and_then(|r| r.get("error"));
            let msg = if let Some(error) = error.filter(|e| !e.is_null()) {
                let code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("no message");
                format!("{code}: {message}")
            } else if let Some(reason) = response
                .and_then(|r| r.pointer("/incomplete_details/reason"))
                .and_then(Value::as_str)
            {
                format!("incomplete: {reason}")
            } else {
                "Unknown error (no error details in response)".to_string()
            };
            return ProcessResult::Error(msg);
        }
        _ => {}
    }
    ProcessResult::Continue
}

fn parse_phase(s: &str) -> Option<TextPhase> {
    match s {
        "commentary" => Some(TextPhase::Commentary),
        "final_answer" => Some(TextPhase::FinalAnswer),
        _ => None,
    }
}

/// Join a reasoning item's `summary`/`content` array of `{text}` parts with `"\n\n"`.
fn join_text_array(value: Option<&Value>) -> String {
    let Some(Value::Array(arr)) = value else {
        return String::new();
    };
    arr.iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Join a message item's `content` parts: `output_text.text` or `refusal` (Pi `item.content?.map`).
fn join_message_content(value: Option<&Value>) -> String {
    let Some(Value::Array(arr)) = value else {
        return String::new();
    };
    arr.iter()
        .map(|c| {
            if c.get("type").and_then(Value::as_str) == Some("output_text") {
                c.get("text").and_then(Value::as_str).unwrap_or("")
            } else {
                c.get("refusal").and_then(Value::as_str).unwrap_or("")
            }
        })
        .collect::<String>()
}

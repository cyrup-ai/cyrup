//! Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531):
//! output-item slot allocation (Pi `createSlot` / `getOrCreateSlot`).

use super::blocks::RBlock;
use super::decoder::RDecoder;
use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use cyrup_core::{ApiId, SharedStr};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SlotKind {
    Thinking,
    Text,
    Tool,
}

/// Create a content slot for a streamed output item (Pi `createSlot`). Returns the new block's
/// content index + kind, or `None` for an unrecognized item type.
pub(super) fn create_slot(
    dec: &mut RDecoder,
    output_index: i64,
    item: &Value,
) -> Option<(usize, SlotKind)> {
    let item_type = item.get("type").and_then(Value::as_str)?;
    let (block, kind) = match item_type {
        "reasoning" => (
            RBlock::Thinking {
                thinking: SharedStr::new(),
                signature: None,
            },
            SlotKind::Thinking,
        ),
        "message" => (
            RBlock::Text {
                text: SharedStr::new(),
                signature: None,
            },
            SlotKind::Text,
        ),
        "function_call" => {
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let item_id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            // A slot can open with arguments already present; the buffer carries them, and there
            // is no derived scanner to seed alongside it (PERF-001).
            let partial_json: SharedStr = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into();
            (
                RBlock::Tool {
                    call_id,
                    item_id,
                    name,
                    partial_json,
                },
                SlotKind::Tool,
            )
        }
        _ => return None,
    };
    dec.push_block(block);
    let ci = dec.blocks.len() - 1;
    dec.slots.insert(output_index, (ci, kind));
    Some((ci, kind))
}

/// Pi `getOrCreateSlot`: the existing slot, else create one (emitting its `*_start`). Returns the
/// content index + kind.
pub(super) async fn get_or_create_slot(
    dec: &mut RDecoder,
    output_index: i64,
    item: &Value,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> Option<(usize, SlotKind)> {
    if let Some((ci, kind)) = dec.slots.get(&output_index).copied() {
        return Some((ci, kind));
    }
    let (ci, kind) = create_slot(dec, output_index, item)?;
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
    sink.send(ev).await;
    Some((ci, kind))
}

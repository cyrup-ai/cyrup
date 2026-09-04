//! Response decoding — streamed tool-call deltas and block finalization (Pi
//! mistral-conversations.ts:418-482).

use super::decoder::{CurrentKind, Decoder};
use super::tool_call_id::derive_mistral_tool_call_id;
use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use crate::utils::json_parse::parse_streaming_json_object;
use cyrup_core::{ApiId, Content, ToolCall, ToolCallId};
use serde_json::{Map, Value};

/// Handle one streamed tool-call delta (Pi mistral-conversations.ts:418-464).
pub(super) async fn process_tool_call(
    tool_call: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    // A tool call closes any open text/thinking block.
    if dec.current.is_some() && !close_current(dec, model, api, sink).await {
        return false;
    }

    let index = tool_call.get("index").and_then(Value::as_i64).unwrap_or(0);
    let provided_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && *id != "null");
    let call_id = match provided_id {
        Some(id) => id.to_string(),
        None => derive_mistral_tool_call_id(&format!("toolcall:{index}"), 0),
    };
    let key = format!("{call_id}:{index}");

    // Open a new tool block on first sight of this key (Pi mistral-conversations.ts:439-450).
    if !dec.tool_blocks_by_key.contains_key(&key) {
        let name = tool_call
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        dec.push_block(Content::ToolCall(ToolCall {
            id: ToolCallId::from(call_id.as_str()),
            name,
            arguments: Map::new().into(),
            thought_signature: None,
        }));
        let block_idx = dec.block_index();
        dec.tool_blocks_by_key.insert(key.clone(), block_idx);
        dec.open_tool_args(block_idx);
        let partial = dec.snapshot(model, api);
        if !sink
            .send(StreamEvent::ToolCallStart {
                content_index: block_idx,
                partial,
            })
            .await
        {
            return false;
        }
    }

    let block_idx = match dec.tool_blocks_by_key.get(&key) {
        Some(i) => *i,
        None => return true,
    };

    let args_delta = match tool_call.get("function").and_then(|f| f.get("arguments")) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
        None => String::new(),
    };
    // Routed through the decoder so the scratch write also invalidates that block's memo
    // (PERF-001): this decoder's projection depends on the scratch, not only on `blocks`.
    dec.push_tool_args(block_idx, &args_delta);

    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::ToolCallDelta {
        content_index: block_idx,
        delta: args_delta,
        partial,
    })
    .await
}

/// Emit the `*_end` for the in-progress text/thinking block, if any.
pub(super) async fn close_current(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
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

/// Finalize every tool block: parse its scratch buffer and emit `toolcall_end` (Pi
/// mistral-conversations.ts:468-482). Emitted in ascending content-block order.
pub(super) async fn finalize_tool_blocks(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let mut indices: Vec<usize> = dec.tool_blocks_by_key.values().copied().collect();
    indices.sort_unstable();
    for idx in indices {
        let (id, name) = match dec.blocks.get(idx) {
            Some(Content::ToolCall(tc)) => (tc.id.clone(), tc.name.clone()),
            _ => continue,
        };
        let args = dec
            .tool_partial_args
            .get(&idx)
            .map(|p| parse_streaming_json_object(Some(p)))
            .unwrap_or_default();
        let tool_call = ToolCall {
            id,
            name,
            arguments: args.clone().into(),
            thought_signature: None,
        };
        if let Some(Content::ToolCall(tc)) = dec.block_mut(idx) {
            tc.arguments = args.into();
        }
        let partial = dec.snapshot(model, api);
        if !sink
            .send(StreamEvent::ToolCallEnd {
                content_index: idx,
                tool_call,
                partial,
            })
            .await
        {
            return false;
        }
    }
    true
}

//! Response decoding: per-delta block assembly.

use super::blocks::{Block, Decoder};
use super::decode::REASONING_FIELDS;
use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use cyrup_core::{ApiId, SharedStr};
use serde_json::Value;

/// The id of a `reasoning.encrypted` detail (Pi `isEncryptedReasoningDetail`): requires
/// `type == "reasoning.encrypted"` plus non-empty `id` and `data` strings.
pub(super) fn encrypted_reasoning_detail_id(detail: &Value) -> Option<&str> {
    if detail.get("type").and_then(Value::as_str) != Some("reasoning.encrypted") {
        return None;
    }
    let id = detail
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let _data = detail
        .get("data")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    Some(id)
}

/// Ensure a text block exists, emitting `TextStart` on first appearance. Returns its index, or
/// `None` if the consumer dropped the stream.
pub(super) async fn ensure_text_block(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> Option<usize> {
    if let Some(idx) = dec.text_idx {
        return Some(idx);
    }
    let idx = dec.blocks.len();
    dec.push_block(Block::Text(SharedStr::new()));
    dec.text_idx = Some(idx);
    let partial = dec.snapshot(model, api);
    if !sink
        .send(StreamEvent::TextStart {
            content_index: idx,
            partial,
        })
        .await
    {
        return None;
    }
    Some(idx)
}

/// Ensure a thinking block exists, emitting `ThinkingStart` on first appearance. The `signature`
/// (the reasoning field name) is recorded on first creation only (matching Pi).
pub(super) async fn ensure_thinking_block(
    dec: &mut Decoder,
    signature: &str,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> Option<usize> {
    if let Some(idx) = dec.thinking_idx {
        return Some(idx);
    }
    let idx = dec.blocks.len();
    dec.push_block(Block::Thinking {
        text: SharedStr::new(),
        signature: Some(signature.to_string()),
    });
    dec.thinking_idx = Some(idx);
    let partial = dec.snapshot(model, api);
    if !sink
        .send(StreamEvent::ThinkingStart {
            content_index: idx,
            partial,
        })
        .await
    {
        return None;
    }
    Some(idx)
}

/// Apply one `tool_calls[]` delta fragment, assembling id/name/arguments across chunks.
pub(super) async fn process_tool_call_delta(
    tc: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let stream_index = tc.get("index").and_then(Value::as_i64);
    let id = tc
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let name = tc
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let args_fragment = tc
        .get("function")
        .and_then(|f| f.get("arguments"))
        .and_then(Value::as_str)
        .unwrap_or("");

    // Locate the block: by stream index first, then by id.
    let existing = stream_index
        .and_then(|si| dec.tool_by_stream.get(&si).copied())
        .or_else(|| id.and_then(|i| dec.tool_by_id.get(i).copied()));

    let idx = match existing {
        Some(idx) => idx,
        None => {
            let idx = dec.blocks.len();
            dec.push_block(Block::Tool {
                id: id.unwrap_or("").to_string(),
                name: name.unwrap_or("").to_string(),
                args: SharedStr::new(),
                thought_signature: None,
            });
            if let Some(si) = stream_index {
                dec.tool_by_stream.insert(si, idx);
            }
            if let Some(i) = id {
                dec.tool_by_id.insert(i.to_string(), idx);
            }
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
            idx
        }
    };

    // Attach any reasoning detail that arrived before this tool call (Pi
    // `applyPendingReasoningDetail`).
    let pending = id.and_then(|i| dec.pending_reasoning_by_tool_id.remove(i));

    if let Some(Block::Tool {
        id: bid,
        name: bname,
        args,
        thought_signature,
    }) = dec.block_mut(idx)
    {
        if let Some(i) = id
            && bid.is_empty()
        {
            *bid = i.to_string();
        }
        if let Some(n) = name
            && bname.is_empty()
        {
            *bname = n.to_string();
        }
        if !args_fragment.is_empty() {
            // O(delta): the append is amortised and no parse happens here at all — see
            // [`SharedStr`] and [`LazyArgs`](cyrup_core::LazyArgs) (PERF-001).
            args.push_str(args_fragment);
        }
        if let Some(sig) = pending {
            *thought_signature = Some(sig);
        }
    }
    // Maintain the id index if the id only arrived now.
    if let Some(i) = id {
        dec.tool_by_id.entry(i.to_string()).or_insert(idx);
    }

    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::ToolCallDelta {
        content_index: idx,
        delta: args_fragment.to_string(),
        partial,
    })
    .await
}

/// First non-empty reasoning delta across the known field names, returned as `(field, value)`.
pub(super) fn first_reasoning_delta(delta: &Value) -> Option<(&'static str, &str)> {
    for field in REASONING_FIELDS {
        if let Some(s) = delta.get(field).and_then(Value::as_str)
            && !s.is_empty()
        {
            return Some((field, s));
        }
    }
    None
}

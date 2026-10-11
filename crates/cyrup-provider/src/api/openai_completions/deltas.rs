//! Response decoding: per-delta block assembly.

use super::blocks::{Block, Decoder};
use super::decode::REASONING_FIELDS;
use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use crate::utils::constrained_sampling::CustomToolInput;
use cyrup_core::{ApiId, SharedStr, StopReason};
use serde_json::Value;

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
///
/// A fragment with a `custom` member and no `function` member is a grammar-constrained call
/// (PROV-101, pi `ensureToolCallBlock` / the `tool_calls` loop, `openai-completions.ts:493-540`,
/// `:644-668` @v1.0.4): its `custom.input` is raw text, stored under the tool's grammar property
/// (pi's `"input"` fallback for a tool the request never declared) and streamed as append-only
/// JSON deltas.
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
    let function = tc.get("function").filter(|f| !f.is_null());
    let custom = tc.get("custom").filter(|c| !c.is_null());
    let name = function
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .or_else(|| custom.and_then(|c| c.get("name")).and_then(Value::as_str))
        .filter(|s| !s.is_empty());
    let args_fragment = function
        .and_then(|f| f.get("arguments"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let custom_fragment = custom
        .and_then(|c| c.get("input"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let is_custom = custom.is_some() && function.is_none();

    // Locate the block: by stream index first, then by id.
    let existing = stream_index
        .and_then(|si| dec.tool_by_stream.get(&si).copied())
        .or_else(|| id.and_then(|i| dec.tool_by_id.get(i).copied()));

    let idx = match existing {
        Some(idx) => idx,
        None => {
            let idx = dec.blocks.len();
            let custom_input = is_custom.then(|| {
                let property = name
                    .and_then(|n| dec.grammar_inputs.get(n))
                    .map_or("input", String::as_str);
                CustomToolInput::new(property, "")
            });
            dec.push_block(Block::Tool {
                id: id.unwrap_or("").to_string(),
                name: name.unwrap_or("").to_string(),
                args: custom_input
                    .as_ref()
                    .map_or_else(SharedStr::new, |c| c.open_json().as_str().into()),
                thought_signature: None,
                custom: custom_input,
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

    // A block that opened as a plain function call and now receives a `custom` fragment becomes a
    // custom call (pi `:539-548`): its name is known by now, which is what picks the property.
    let upgraded = match dec.blocks.get(idx) {
        Some(Block::Tool {
            name: bname,
            custom: None,
            ..
        }) if is_custom => {
            let known = if bname.is_empty() {
                name.unwrap_or("")
            } else {
                bname.as_str()
            };
            let property = dec
                .grammar_inputs
                .get(known)
                .map_or("input", String::as_str);
            Some(CustomToolInput::new(property, ""))
        }
        _ => None,
    };
    if let Some(Block::Tool {
        id: bid,
        name: bname,
        args,
        custom: block_custom,
        ..
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
        if let Some(upgraded) = upgraded {
            *args = upgraded.open_json().as_str().into();
            *block_custom = Some(upgraded);
        }
        if block_custom.is_none() && !args_fragment.is_empty() {
            // O(delta): the append is amortised and no parse happens here at all — see
            // [`SharedStr`] and [`LazyArgs`](cyrup_core::LazyArgs) (PERF-001).
            args.push_str(args_fragment);
        }
    }
    // Maintain the id index if the id only arrived now.
    if let Some(i) = id {
        dec.tool_by_id.entry(i.to_string()).or_insert(idx);
    }

    let delta = if dec.custom_input(idx).is_some() {
        // `else if (toolCall.custom?.input)`: nothing to append for an empty fragment, but the
        // event is still pushed (pi `:661-667`), with an empty delta.
        if custom_fragment.is_empty() {
            String::new()
        } else {
            let next = format!(
                "{}{custom_fragment}",
                dec.custom_input(idx).unwrap_or_default()
            );
            match dec.append_custom_input(idx, &next, false) {
                Ok(delta) => delta.unwrap_or_default(),
                Err(e) => {
                    dec.stop_reason = Some(StopReason::Error);
                    dec.error_message = Some(e.0);
                    String::new()
                }
            }
        }
    } else {
        args_fragment.to_string()
    };

    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::ToolCallDelta {
        content_index: idx,
        delta,
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

//! Request encoding — the `messages` array (Pi `convertMessages`).

use super::claude_code::to_claude_code_name;
use super::convert::{ToolAnchors, convert_tool_result};
use crate::api::compat::sanitize_surrogates;
use cyrup_core::{AssistantMessage, Content, Message};
use serde_json::{Value, json};
use std::collections::HashSet;

/// Map cyrup [`Message`]s to Anthropic `messages` (1:1 port of Pi `convertMessages`,
/// anthropic-messages.ts:1011-1182).
///
/// Takes messages that have ALREADY been through `transform_messages_with` — [`build_params`](super::params::build_params)
/// hoists that call so the deferred-tool split sees the same list this does (Pi :947-961).
pub(crate) fn convert_messages(
    transformed: &[Message],
    is_oauth: bool,
    cache_control: Option<&Value>,
    allow_empty_signature: bool,
    deferred_tool_names: &HashSet<String>,
    normalize_tool_name: &dyn Fn(&str) -> String,
) -> Vec<Value> {
    let mut params: Vec<Value> = Vec::new();
    // Declared once per request so a deferred tool is referenced exactly once (Pi :1125).
    let mut anchors = ToolAnchors {
        deferred_tool_names,
        loaded_tool_names: HashSet::new(),
        normalize_tool_name,
    };

    let mut i = 0;
    while let Some(msg) = transformed.get(i) {
        match msg {
            Message::User { content, .. } => {
                if let Some(value) = build_user(content) {
                    params.push(value);
                }
            }
            Message::Assistant(am) => {
                if let Some(value) = build_assistant(am, is_oauth, allow_empty_signature) {
                    params.push(value);
                }
            }
            Message::ToolResult { .. } => {
                // Collect consecutive tool results into one `user` message of `tool_result` blocks.
                let mut tool_results: Vec<Value> = Vec::new();
                // Displaced content is accumulated across the WHOLE consecutive run and flushed
                // once, AFTER every `tool_result` block of the batch (Pi :1226-1252) — not
                // interleaved per block.
                let mut sibling_content: Vec<Value> = Vec::new();
                let mut j = i;
                while let Some(Message::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                    added_tool_names,
                    ..
                }) = transformed.get(j)
                {
                    let (tr, siblings) = convert_tool_result(
                        tool_call_id.as_str(),
                        content,
                        *is_error,
                        added_tool_names,
                        is_oauth,
                        &mut anchors,
                    );
                    tool_results.push(tr);
                    sibling_content.extend(siblings);
                    j += 1;
                }
                i = j;
                tool_results.extend(sibling_content);
                params.push(json!({ "role": "user", "content": Value::Array(tool_results) }));
                continue;
            }
        }
        i += 1;
    }

    // cache_control on the last user message's last block (Pi anthropic-messages.ts:1157-1179).
    if let Some(cc) = cache_control {
        apply_last_user_cache_control(&mut params, cc);
    }

    params
}

/// Build a `user` message; `None` when it has no non-empty content (Pi anthropic-messages.ts:1026-1063).
fn build_user(content: &[Content]) -> Option<Value> {
    let only_text = content.iter().all(|c| matches!(c, Content::Text { .. }));
    if only_text {
        let joined = content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if joined.trim().is_empty() {
            return None;
        }
        return Some(json!({ "role": "user", "content": sanitize_surrogates(&joined) }));
    }

    let mut blocks: Vec<Value> = Vec::new();
    for block in content {
        match block {
            Content::Text { text, .. } => {
                if text.trim().is_empty() {
                    continue;
                }
                blocks.push(json!({ "type": "text", "text": sanitize_surrogates(text) }));
            }
            Content::Image { data, mime_type } => blocks.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": mime_type, "data": data },
            })),
            _ => {}
        }
    }
    if blocks.is_empty() {
        return None;
    }
    Some(json!({ "role": "user", "content": Value::Array(blocks) }))
}

/// Build an `assistant` message; `None` when it has no content blocks (Pi
/// anthropic-messages.ts:1064-1120).
pub(super) fn build_assistant(
    am: &AssistantMessage,
    is_oauth: bool,
    allow_empty_signature: bool,
) -> Option<Value> {
    let mut blocks: Vec<Value> = Vec::new();
    for block in &am.content {
        match block {
            Content::Text { text, .. } => {
                if text.trim().is_empty() {
                    continue;
                }
                blocks.push(json!({ "type": "text", "text": sanitize_surrogates(text) }));
            }
            Content::Thinking {
                thinking,
                thinking_signature,
                redacted,
            } => {
                if *redacted {
                    blocks.push(json!({
                        "type": "redacted_thinking",
                        "data": thinking_signature.clone().unwrap_or_default(),
                    }));
                    continue;
                }
                if thinking.trim().is_empty() {
                    continue;
                }
                let sig_empty = thinking_signature
                    .as_ref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true);
                if sig_empty {
                    if allow_empty_signature {
                        blocks.push(json!({
                            "type": "thinking",
                            "thinking": sanitize_surrogates(thinking),
                            "signature": "",
                        }));
                    } else {
                        blocks
                            .push(json!({ "type": "text", "text": sanitize_surrogates(thinking) }));
                    }
                } else {
                    blocks.push(json!({
                        "type": "thinking",
                        "thinking": sanitize_surrogates(thinking),
                        "signature": thinking_signature.clone().unwrap_or_default(),
                    }));
                }
            }
            Content::ToolCall(tc) => {
                let name = if is_oauth {
                    to_claude_code_name(&tc.name)
                } else {
                    tc.name.clone()
                };
                blocks.push(json!({
                    "type": "tool_use",
                    "id": tc.id.as_str(),
                    "name": name,
                    "input": Value::Object(tc.arguments.clone()),
                }));
            }
            _ => {}
        }
    }
    if blocks.is_empty() {
        return None;
    }
    Some(json!({ "role": "assistant", "content": Value::Array(blocks) }))
}

/// Add `cache_control` to the last user message's last cache-eligible block (Pi
/// anthropic-messages.ts:1157-1179).
fn apply_last_user_cache_control(params: &mut [Value], cc: &Value) {
    let Some(last) = params.last_mut() else {
        return;
    };
    if last.get("role").and_then(Value::as_str) != Some("user") {
        return;
    }
    match last.get_mut("content") {
        Some(Value::Array(arr)) => {
            if let Some(block) = arr.last_mut()
                && let Some(o) = block.as_object_mut()
            {
                let kind = o.get("type").and_then(Value::as_str);
                if matches!(kind, Some("text") | Some("image") | Some("tool_result")) {
                    o.insert("cache_control".to_string(), cc.clone());
                }
            }
        }
        Some(Value::String(_)) => {
            if let Some(Value::String(s)) = last.get("content") {
                let text = s.clone();
                if let Some(o) = last.as_object_mut() {
                    o.insert(
                        "content".to_string(),
                        json!([{ "type": "text", "text": text, "cache_control": cc.clone() }]),
                    );
                }
            }
        }
        _ => {}
    }
}

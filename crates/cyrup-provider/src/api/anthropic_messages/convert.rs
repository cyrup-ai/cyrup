//! Request encoding — content-block and tool-result conversion.

use super::claude_code::to_claude_code_name;
use crate::api::compat::sanitize_surrogates;
use cyrup_core::Content;
use serde_json::{Map, Value, json};
use std::collections::HashSet;

/// Convert tool-result / user content blocks to Anthropic format (Pi `convertContentBlocks`,
/// anthropic-messages.ts:114-161). Text-only collapses to a string; mixed content becomes a block
/// array (a leading `(see attached image)` text block is added when only images are present).
fn convert_content_blocks(content: &[Content]) -> Value {
    let has_images = content.iter().any(|c| matches!(c, Content::Image { .. }));
    if !has_images {
        let joined = content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        return json!(sanitize_surrogates(&joined));
    }

    let mut blocks: Vec<Value> = Vec::new();
    let mut has_text = false;
    for block in content {
        match block {
            Content::Text { text, .. } => {
                has_text = true;
                blocks.push(json!({ "type": "text", "text": sanitize_surrogates(text) }));
            }
            Content::Image { data, mime_type } => blocks.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": mime_type, "data": data },
            })),
            _ => {}
        }
    }
    if !has_text {
        blocks.insert(0, json!({ "type": "text", "text": "(see attached image)" }));
    }
    Value::Array(blocks)
}

/// The per-request deferred-tool anchoring state threaded through [`convert_messages`](super::messages::convert_messages) (Pi keeps
/// these as three separate parameters of `convertToolResult`, anthropic-messages.ts:1081-1086).
pub(super) struct ToolAnchors<'a> {
    /// Normalized names that were split out of the request prefix and must be anchored.
    pub(super) deferred_tool_names: &'a HashSet<String>,
    /// Names already referenced in THIS request — declared once per `convertMessages` call
    /// (Pi :1125) so a tool is loaded exactly once even if several results mark it.
    pub(super) loaded_tool_names: HashSet<String>,
    /// `toClaudeCodeName` under OAuth, identity otherwise (Pi :948).
    pub(super) normalize_tool_name: &'a dyn Fn(&str) -> String,
}

/// Convert ONE tool-result message into its `tool_result` block plus any content that had to be
/// DISPLACED out of it (1:1 port of Pi `convertToolResult`, anthropic-messages.ts:1081-1112).
///
/// Anthropic **rejects** a `tool_result` whose `content` mixes `tool_reference` blocks with
/// ordinary blocks, so when this result anchors a deferred tool the reference list REPLACES the
/// content and the real content is returned separately, to be re-appended as a sibling of the
/// `tool_result` in the same `user` message. Nothing is dropped — it is relocated.
///
/// A name is referenced at most once per request: `loaded_tool_names` is declared once per
/// [`convert_messages`](super::messages::convert_messages) call (Pi :1125) and is shared across every tool result in the transcript.
pub(super) fn convert_tool_result(
    tool_call_id: &str,
    content: &[Content],
    is_error: bool,
    added_tool_names: &[String],
    is_oauth: bool,
    anchors: &mut ToolAnchors<'_>,
) -> (Value, Vec<Value>) {
    let mut references: Vec<Value> = Vec::new();
    for name in added_tool_names {
        let normalized = (anchors.normalize_tool_name)(name);
        if !anchors.deferred_tool_names.contains(&normalized)
            || anchors.loaded_tool_names.contains(&normalized)
        {
            continue;
        }
        anchors.loaded_tool_names.insert(normalized);
        let wire_name = if is_oauth {
            to_claude_code_name(name)
        } else {
            name.clone()
        };
        references.push(json!({ "type": "tool_reference", "tool_name": wire_name }));
    }

    let converted = convert_content_blocks(content);
    let has_refs = !references.is_empty();

    let mut tr = Map::new();
    tr.insert("type".to_string(), json!("tool_result"));
    tr.insert("tool_use_id".to_string(), json!(tool_call_id));
    tr.insert(
        "content".to_string(),
        if has_refs {
            Value::Array(references)
        } else {
            converted.clone()
        },
    );
    // `is_error` rides on the `tool_result` regardless of whether it carries references.
    tr.insert("is_error".to_string(), json!(is_error));

    // Pi `typeof convertedContent === "string" ? [{type:"text",text:…}] : convertedContent`. Pi
    // has NO empty-string guard, so an empty tool result with a reference emits `text: ""`.
    let siblings: Vec<Value> = if !has_refs {
        Vec::new()
    } else {
        match converted {
            Value::String(s) => vec![json!({ "type": "text", "text": s })],
            Value::Array(a) => a,
            other => vec![other],
        }
    };
    (Value::Object(tr), siblings)
}

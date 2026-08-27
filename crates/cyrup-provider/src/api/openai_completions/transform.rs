//! Message transformation (Pi `transform-messages.ts`).

use crate::model::Model;
use crate::utils::hash::short_hash;
use crate::utils::provider_plumbing::now_millis;
use cyrup_core::{AssistantMessage, Content, Message, StopReason, ToolCall, ToolCallId};
use std::collections::{HashMap, HashSet};

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

/// `true` if `am` was produced by exactly this model (Pi `isSameModel`).
fn is_same_model(am: &AssistantMessage, model: &Model) -> bool {
    am.provider == model.provider && am.api == model.api && am.model == model.id.as_str()
}

/// Sanitize an id fragment to the `[a-zA-Z0-9_-]` alphabet OpenAI accepts for tool-call ids
/// (Pi `replace(/[^a-zA-Z0-9_-]/g, "_")`). Output is always ASCII.
fn sanitize_tool_call_id_part(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Normalize a tool-call id for cross-provider replay (Pi `convertMessages.normalizeToolCallId`,
/// openai-completions.ts:1006-1030).
///
/// Responses-API ids arrive as `{call_id}|{item_id}`. Several tool calls in one turn can share a
/// `call_id` and differ only by `item_id`, so keeping just the `call_id` half collapses them into
/// duplicate `tool_call_id`s — which Chat Completions rejects with a 400 when the conversation is
/// replayed onto an openai-completions model. Pi therefore keeps BOTH halves (`{call}_{item}`) and
/// falls back to `{call-prefix}_{8-char shortHash of the whole id}` when the join exceeds the
/// 40-char limit.
pub(super) fn normalize_tool_call_id(model: &Model, id: &str) -> String {
    if let Some(separator_index) = id.find('|') {
        let call_id = sanitize_tool_call_id_part(id.get(..separator_index).unwrap_or(""));
        let item_id =
            sanitize_tool_call_id_part(id.get(separator_index.saturating_add(1)..).unwrap_or(""));
        let combined_id = if item_id.is_empty() {
            call_id.clone()
        } else {
            format!("{call_id}_{item_id}")
        };
        if combined_id.len() <= 40 {
            return combined_id;
        }
        let hash: String = short_hash(id).chars().take(8).collect();
        // `Math.max(1, 40 - hash.length - 1)` — never truncate the call id to nothing.
        let prefix_len = 40usize.saturating_sub(hash.len().saturating_add(1)).max(1);
        let prefix: String = call_id.chars().take(prefix_len).collect();
        return format!("{prefix}_{hash}");
    }
    if model.provider.as_str() == "openai" {
        return if id.chars().count() > 40 {
            id.chars().take(40).collect()
        } else {
            id.to_string()
        };
    }
    id.to_string()
}

/// Replace image blocks with a text placeholder, de-duplicating consecutive placeholders
/// (Pi `replaceImagesWithPlaceholder`).
fn replace_images_with_placeholder(content: &[Content], placeholder: &str) -> Vec<Content> {
    let mut result = Vec::new();
    let mut previous_was_placeholder = false;
    for block in content {
        match block {
            Content::Image { .. } => {
                if !previous_was_placeholder {
                    result.push(Content::text(placeholder));
                }
                previous_was_placeholder = true;
            }
            Content::Text { text, .. } => {
                let is_placeholder = text == placeholder;
                result.push(block.clone());
                previous_was_placeholder = is_placeholder;
            }
            other => {
                result.push(other.clone());
                previous_was_placeholder = false;
            }
        }
    }
    result
}

/// Downgrade unsupported images to placeholders for non-vision models (Pi
/// `downgradeUnsupportedImages`).
fn downgrade_unsupported_images(messages: &[Message], model: &Model) -> Vec<Message> {
    if model.supports_image_input() {
        return messages.to_vec();
    }
    messages
        .iter()
        .map(|m| match m {
            Message::User { content, timestamp } => Message::User {
                content: replace_images_with_placeholder(
                    content,
                    NON_VISION_USER_IMAGE_PLACEHOLDER,
                ),
                timestamp: *timestamp,
            },
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                details,
                usage,
                added_tool_names,
                timestamp,
            } => Message::ToolResult {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                content: replace_images_with_placeholder(
                    content,
                    NON_VISION_TOOL_IMAGE_PLACEHOLDER,
                ),
                is_error: *is_error,
                details: details.clone(),
                // This transform only swaps image blocks for a placeholder; every other field must
                // survive it. `added_tool_names` in particular is the deferred-tool anchor, and a
                // request-path transform that silently dropped it would move the tool definition
                // back to the prefix and wipe the prompt cache.
                usage: usage.clone(),
                added_tool_names: added_tool_names.clone(),
                timestamp: *timestamp,
            },
            other => other.clone(),
        })
        .collect()
}

/// Insert synthetic empty tool results for orphaned tool calls (Pi
/// `transformMessages.insertSyntheticToolResults`).
fn insert_synthetic_tool_results(
    result: &mut Vec<Message>,
    pending: &mut Vec<ToolCall>,
    existing: &mut HashSet<String>,
) {
    if pending.is_empty() {
        return;
    }
    for tc in pending.iter() {
        if !existing.contains(tc.id.as_str()) {
            result.push(Message::ToolResult {
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                content: vec![Content::text("No result provided")],
                is_error: true,
                details: None,
                timestamp: now_millis(),
                usage: None,
                added_tool_names: Vec::new(),
            });
        }
    }
    pending.clear();
    existing.clear();
}

/// 1:1 port of Pi `transformMessages` (transform-messages.ts): downgrade images, drop/convert
/// cross-model thinking, normalize tool-call ids, skip errored/aborted assistant turns, and
/// synthesize results for orphaned tool calls.
pub(crate) fn transform_messages(messages: &[Message], model: &Model) -> Vec<Message> {
    transform_messages_with(messages, model, |id| normalize_tool_call_id(model, id))
}

/// [`transform_messages`] parameterized by the per-api tool-call-id normalizer (Pi
/// `transformMessages(messages, model, normalizeToolCallId)`, transform-messages.ts:64-67). The
/// `openai-completions` caller passes [`normalize_tool_call_id`]; the `anthropic-messages` caller
/// passes its own 64-char/`^[a-zA-Z0-9_-]+$` normalizer.
pub(crate) fn transform_messages_with(
    messages: &[Message],
    model: &Model,
    normalize: impl Fn(&str) -> String,
) -> Vec<Message> {
    transform_messages_with_source(messages, model, |id, _src| normalize(id))
}

/// [`transform_messages_with`] whose normalizer also receives the source [`AssistantMessage`] (Pi
/// `normalizeToolCallId(id, model, source)`, transform-messages.ts:67/134). The `openai-responses`
/// caller needs `source` to decide whether a tool call is *foreign* (a different provider/api)
/// when rewriting its `call_id|item_id` (openai-responses-shared.ts:109-121).
pub(crate) fn transform_messages_with_source(
    messages: &[Message],
    model: &Model,
    normalize: impl Fn(&str, &AssistantMessage) -> String,
) -> Vec<Message> {
    let mut tool_call_id_map: HashMap<String, String> = HashMap::new();
    let image_aware = downgrade_unsupported_images(messages, model);

    // First pass: per-message transform.
    let transformed: Vec<Message> = image_aware
        .iter()
        .map(|msg| match msg {
            Message::User { .. } => msg.clone(),
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                details,
                usage,
                added_tool_names,
                timestamp,
            } => {
                if let Some(norm) = tool_call_id_map.get(tool_call_id.as_str()).cloned()
                    && norm != tool_call_id.as_str()
                {
                    return Message::ToolResult {
                        tool_call_id: ToolCallId::from(norm.as_str()),
                        tool_name: tool_name.clone(),
                        content: content.clone(),
                        is_error: *is_error,
                        details: details.clone(),
                        // Only the tool-call id is being rewritten; carry the rest through
                        // untouched (see `downgrade_unsupported_images`).
                        usage: usage.clone(),
                        added_tool_names: added_tool_names.clone(),
                        timestamp: *timestamp,
                    };
                }
                msg.clone()
            }
            Message::Assistant(am) => {
                let same = is_same_model(am, model);
                let mut new_content: Vec<Content> = Vec::new();
                for block in &am.content {
                    match block {
                        Content::Thinking {
                            thinking,
                            thinking_signature,
                            redacted,
                        } => {
                            if *redacted {
                                if same {
                                    new_content.push(block.clone());
                                }
                                continue;
                            }
                            if same && thinking_signature.is_some() {
                                new_content.push(block.clone());
                                continue;
                            }
                            if thinking.trim().is_empty() {
                                continue;
                            }
                            if same {
                                new_content.push(block.clone());
                            } else {
                                new_content.push(Content::text(thinking.clone()));
                            }
                        }
                        Content::Text { text, .. } => {
                            if same {
                                new_content.push(block.clone());
                            } else {
                                new_content.push(Content::text(text.clone()));
                            }
                        }
                        Content::ToolCall(tc) => {
                            let mut ntc = tc.clone();
                            if !same && ntc.thought_signature.is_some() {
                                ntc.thought_signature = None;
                            }
                            if !same {
                                let norm = normalize(tc.id.as_str(), am);
                                if norm != tc.id.as_str() {
                                    tool_call_id_map
                                        .insert(tc.id.as_str().to_string(), norm.clone());
                                    ntc.id = ToolCallId::from(norm.as_str());
                                }
                            }
                            new_content.push(Content::ToolCall(ntc));
                        }
                        other => new_content.push(other.clone()),
                    }
                }
                let mut nam = am.clone();
                nam.content = new_content;
                Message::Assistant(nam)
            }
        })
        .collect();

    // Second pass: skip errored/aborted assistants; synthesize orphaned tool results.
    let mut result: Vec<Message> = Vec::new();
    let mut pending: Vec<ToolCall> = Vec::new();
    let mut existing: HashSet<String> = HashSet::new();

    for msg in transformed {
        match &msg {
            Message::Assistant(am) => {
                insert_synthetic_tool_results(&mut result, &mut pending, &mut existing);
                if matches!(am.stop_reason, StopReason::Error | StopReason::Aborted) {
                    continue;
                }
                let tool_calls: Vec<ToolCall> = am
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::ToolCall(tc) => Some(tc.clone()),
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    pending = tool_calls;
                    existing.clear();
                }
                result.push(msg);
            }
            Message::ToolResult { tool_call_id, .. } => {
                existing.insert(tool_call_id.as_str().to_string());
                result.push(msg);
            }
            Message::User { .. } => {
                insert_synthetic_tool_results(&mut result, &mut pending, &mut existing);
                result.push(msg);
            }
        }
    }
    insert_synthetic_tool_results(&mut result, &mut pending, &mut existing);
    result
}

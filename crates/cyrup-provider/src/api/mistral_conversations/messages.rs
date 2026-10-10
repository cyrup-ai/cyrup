//! Request encoding — the chat-message encoder (Pi `toChatMessages`,
//! mistral-conversations.ts:513-598).

use super::tools::build_tool_result_text;
use crate::api::compat::sanitize_surrogates;
use cyrup_core::{Content, Message};
use serde_json::{Map, Value, json};

/// Convert cyrup [`Message`]s to Mistral chat messages (Pi `toChatMessages`,
/// mistral-conversations.ts:513-598).
pub(super) fn to_chat_messages(messages: &[Message], supports_images: bool) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        match msg {
            // PROV-083a — a `Message::System` carries the transcript's own prompt and tool
            // state, not a turn to convert. Upstream every adapter reaches its message loop through
            // `collapseSystemMessages` / `withoutInitialSystemMessage`
            // (`utils/transcript.ts:114`/`:57`), so the leading system message is consumed by the
            // params builder and no later one survives; skipping is exactly what that collapsed
            // transcript yields. PROV-083b wires each adapter's own `resolve_transcript` and, where
            // the transport can express it, emits later system messages in place.
            Message::System(_) => {}
            Message::User { content, .. } => {
                let had_images = content.iter().any(|c| matches!(c, Content::Image { .. }));
                let parts: Vec<Value> = content
                    .iter()
                    .filter(|c| matches!(c, Content::Text { .. }) || supports_images)
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => {
                            Some(json!({ "type": "text", "text": sanitize_surrogates(text) }))
                        }
                        Content::Image { data, mime_type } => Some(json!({
                            "type": "image_url",
                            "imageUrl": format!("data:{mime_type};base64,{data}"),
                        })),
                        _ => None,
                    })
                    .collect();
                if !parts.is_empty() {
                    result.push(json!({ "role": "user", "content": parts }));
                } else if had_images && !supports_images {
                    result.push(json!({
                        "role": "user",
                        "content": "(image omitted: model does not support images)",
                    }));
                }
            }
            Message::Assistant(am) => {
                let mut content_parts: Vec<Value> = Vec::new();
                let mut tool_calls: Vec<Value> = Vec::new();
                for block in &am.content {
                    match block {
                        Content::Text { text, .. } => {
                            if !text.trim().is_empty() {
                                content_parts.push(
                                    json!({ "type": "text", "text": sanitize_surrogates(text) }),
                                );
                            }
                        }
                        Content::Thinking { thinking, .. } => {
                            if !thinking.trim().is_empty() {
                                content_parts.push(json!({
                                    "type": "thinking",
                                    "thinking": [{ "type": "text", "text": sanitize_surrogates(thinking) }],
                                }));
                            }
                        }
                        Content::ToolCall(tc) => {
                            let args = serde_json::to_string(&tc.arguments)
                                .unwrap_or_else(|_| "{}".to_string());
                            // `index: 0` is pi's (`mistral-conversations.ts:860-865` @f1b2e77f5),
                            // added with the native transport (`9dd90a497`): Mistral's `ToolCall`
                            // schema defaults it to 0, and the SDK used to fill it in.
                            tool_calls.push(json!({
                                "id": tc.id.as_str(),
                                "type": "function",
                                "function": { "name": tc.name, "arguments": args },
                                "index": 0,
                            }));
                        }
                        _ => {}
                    }
                }
                if !content_parts.is_empty() || !tool_calls.is_empty() {
                    // Pi `{ role: "assistant", prefix: false }` (`:868` @f1b2e77f5), the SDK
                    // default the native transport now writes itself.
                    let mut o = Map::new();
                    o.insert("role".to_string(), json!("assistant"));
                    o.insert("prefix".to_string(), json!(false));
                    if !content_parts.is_empty() {
                        o.insert("content".to_string(), Value::Array(content_parts));
                    }
                    // SDK-style `toolCalls`; `wire::to_mistral_wire_payload` sends it as
                    // `tool_calls` (PROV-152).
                    if !tool_calls.is_empty() {
                        o.insert("toolCalls".to_string(), Value::Array(tool_calls));
                    }
                    result.push(Value::Object(o));
                }
            }
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                ..
            } => {
                let text_result = content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(sanitize_surrogates(text)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let has_images = content.iter().any(|c| matches!(c, Content::Image { .. }));
                let tool_text =
                    build_tool_result_text(&text_result, has_images, supports_images, *is_error);
                let mut tool_content = vec![json!({ "type": "text", "text": tool_text })];
                if supports_images {
                    for part in content {
                        if let Content::Image { data, mime_type } = part {
                            tool_content.push(json!({
                                "type": "image_url",
                                "imageUrl": format!("data:{mime_type};base64,{data}"),
                            }));
                        }
                    }
                }
                result.push(json!({
                    "role": "tool",
                    "toolCallId": tool_call_id.as_str(),
                    "name": tool_name,
                    "content": tool_content,
                }));
            }
        }
    }

    result
}

//! Request encoding — the `Content[]` encoder: tool-call-id normalization and the user /
//! assistant part builders (Pi `convertMessages`, google-shared.ts:100-270).

use crate::api::compat::sanitize_surrogates;
use crate::api::openai_completions::transform_messages_with;
use crate::context::Context;
use crate::model::{Modality, Model};
use cyrup_core::{AssistantMessage, Content, Message};
use serde_json::{Map, Value, json};
use super::capabilities::{requires_tool_call_id, supports_multimodal_function_response};
use super::signatures::resolve_thought_signature;

/// The Gemini tool-call-id normalizer (Pi `convertMessages` `normalizeToolCallId`,
/// v0.84.1 `ai/src/api/google-shared.ts:100-103`; v0.83.0 `:94-97` — same body, shifted).
fn normalize_tool_call_id(model_id: &str, id: &str) -> String {
    if !requires_tool_call_id(model_id) {
        return id.to_string();
    }
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// Convert cyrup [`Message`]s to Gemini `Content[]` (1:1 port of Pi `convertMessages`,
/// google-shared.ts:91-235).
pub(super) fn convert_messages(model: &Model, ctx: &Context) -> Vec<Value> {
    let model_id = model.id.as_str().to_string();
    let transformed = transform_messages_with(&ctx.messages, model, |id| {
        normalize_tool_call_id(&model_id, id)
    });

    let supports_image = model.input.contains(&Modality::Image);
    let multimodal_fr = supports_multimodal_function_response(&model_id);
    let include_id = requires_tool_call_id(&model_id);

    let mut contents: Vec<Value> = Vec::new();

    for msg in &transformed {
        match msg {
            Message::User { content, .. } => {
                let parts = user_parts(content);
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({ "role": "user", "parts": parts }));
            }
            Message::Assistant(am) => {
                let same = am.provider == model.provider && am.model == model_id;
                let parts = assistant_parts(am, same, include_id);
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            Message::ToolResult {
                tool_name,
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                let text_result = content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let image_parts: Vec<Value> = if supports_image {
                    content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Image { data, mime_type } => Some(json!({
                                "inlineData": { "mimeType": mime_type, "data": data }
                            })),
                            _ => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                let has_text = !text_result.is_empty();
                let has_images = !image_parts.is_empty();
                let response_value = if has_text {
                    sanitize_surrogates(&text_result)
                } else if has_images {
                    "(see attached image)".to_string()
                } else {
                    String::new()
                };

                let mut response = Map::new();
                if *is_error {
                    response.insert("error".to_string(), json!(response_value));
                } else {
                    response.insert("output".to_string(), json!(response_value));
                }

                let mut fr = Map::new();
                fr.insert("name".to_string(), json!(tool_name));
                fr.insert("response".to_string(), Value::Object(response));
                if has_images && multimodal_fr {
                    fr.insert("parts".to_string(), Value::Array(image_parts.clone()));
                }
                if include_id {
                    fr.insert("id".to_string(), json!(tool_call_id.as_str()));
                }
                let function_response_part = json!({ "functionResponse": Value::Object(fr) });

                // Merge consecutive function responses into one user turn (Pi google-shared.ts:214-222).
                if let Some(last) = contents.last_mut()
                    && last.get("role").and_then(Value::as_str) == Some("user")
                    && last
                        .get("parts")
                        .and_then(Value::as_array)
                        .map(|p| p.iter().any(|x| x.get("functionResponse").is_some()))
                        .unwrap_or(false)
                    && let Some(Value::Array(parts)) = last.get_mut("parts")
                {
                    parts.push(function_response_part);
                } else {
                    contents.push(json!({ "role": "user", "parts": [function_response_part] }));
                }

                // Gemini < 3: images go in a separate user turn (Pi google-shared.ts:225-230).
                if has_images && !multimodal_fr {
                    let mut parts = vec![json!({ "text": "Tool result image:" })];
                    parts.extend(image_parts);
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            }
        }
    }

    contents
}

/// Build the `parts` for a user turn (Pi google-shared.ts:101-125).
fn user_parts(content: &[Content]) -> Vec<Value> {
    content
        .iter()
        .filter_map(|item| match item {
            Content::Text { text, .. } => Some(json!({ "text": sanitize_surrogates(text) })),
            Content::Image { data, mime_type } => {
                Some(json!({ "inlineData": { "mimeType": mime_type, "data": data } }))
            }
            _ => None,
        })
        .collect()
}

/// Build the `parts` for an assistant (`model`) turn (Pi google-shared.ts:127-182).
///
/// Empty text/thinking blocks are dropped only when they carry no usable thought signature
/// (Pi 6138f5a0, google-shared.ts:134-151); the cross-provider `else` branch keeps the old
/// unconditional skip because the signature is unusable there (google-shared.ts:157-162).
///
/// `include_id` is [`requires_tool_call_id`] of the **target** model — the model this request is
/// being built for — NOT of `am.model`, the model that happened to produce the historical turn.
/// DRIFT-048: this argument used to be re-derived inside the `toolCall` arm from `am.model`, so a
/// mid-session model switch emitted a `functionCall` whose `id` presence disagreed with the
/// matching `functionResponse` (which `convert_messages` already keys on the target). pi is
/// unambiguous: `requiresToolCallId(model.id)` at `google-shared.ts:177`, inside the same
/// `convertMessages` whose `model` parameter is the request's target.
pub(super) fn assistant_parts(am: &AssistantMessage, same: bool, include_id: bool) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    for block in &am.content {
        match block {
            Content::Text {
                text,
                text_signature,
            } => {
                let sig = resolve_thought_signature(same, text_signature.as_deref());
                // Skip empty text blocks — unless they carry a thought signature. Gemini can
                // attach the signature to a part whose visible text is empty and requires it
                // echoed back; dropping it breaks the reasoning chain and the model
                // intermittently ends mid-task turns with a thought-only STOP (empty
                // completion, no tool call). (Pi google-shared.ts:134-139.)
                if text.trim().is_empty() && sig.is_none() {
                    continue;
                }
                let mut o = Map::new();
                o.insert("text".to_string(), json!(sanitize_surrogates(text)));
                if let Some(s) = sig {
                    o.insert("thoughtSignature".to_string(), json!(s));
                }
                parts.push(Value::Object(o));
            }
            Content::Thinking {
                thinking,
                thinking_signature,
                ..
            } => {
                // Only keep as thinking block if same provider AND same model; otherwise
                // convert to plain text (no tags to avoid model mimicking them).
                if same {
                    let sig = resolve_thought_signature(same, thinking_signature.as_deref());
                    // Same rule as text blocks: an empty thinking block is dropped only when it
                    // carries no signature (Pi google-shared.ts:148-151).
                    if thinking.trim().is_empty() && sig.is_none() {
                        continue;
                    }
                    let mut o = Map::new();
                    o.insert("thought".to_string(), json!(true));
                    o.insert("text".to_string(), json!(sanitize_surrogates(thinking)));
                    if let Some(s) = sig {
                        o.insert("thoughtSignature".to_string(), json!(s));
                    }
                    parts.push(Value::Object(o));
                } else {
                    // Cross-provider/model: the signature is unusable, empty blocks stay
                    // dropped unconditionally (Pi google-shared.ts:157-162).
                    if thinking.trim().is_empty() {
                        continue;
                    }
                    // Convert to plain text (no tags) for a different provider/model.
                    parts.push(json!({ "text": sanitize_surrogates(thinking) }));
                }
            }
            Content::ToolCall(tc) => {
                let sig = resolve_thought_signature(same, tc.thought_signature.as_deref());
                let mut fc = Map::new();
                fc.insert("name".to_string(), json!(tc.name));
                fc.insert("args".to_string(), Value::Object(tc.arguments.clone()));
                // `...(requiresToolCallId(model.id) ? { id: block.id } : {})`
                // (google-shared.ts:177) — the TARGET model, threaded in as `include_id`.
                if include_id {
                    fc.insert("id".to_string(), json!(tc.id.as_str()));
                }
                let mut o = Map::new();
                o.insert("functionCall".to_string(), Value::Object(fc));
                if let Some(s) = sig {
                    o.insert("thoughtSignature".to_string(), json!(s));
                }
                parts.push(Value::Object(o));
            }
            _ => {}
        }
    }
    parts
}

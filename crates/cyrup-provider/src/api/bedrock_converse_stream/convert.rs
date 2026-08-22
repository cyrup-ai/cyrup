//! Message, content and tool conversion (pi `buildSystemPrompt` / `convertMessages` /
//! `convertToolConfig`).

use super::capabilities::{supports_prompt_caching, supports_thinking_signature};
use super::env::EnvSource;
use super::options::BedrockToolChoice;
use crate::api::compat::sanitize_surrogates;
use crate::api::openai_completions::transform_messages_with;
use crate::context::{Context, ToolDef};
use crate::model::Model;
use crate::stream::CacheRetention;
use crate::utils::constrained_sampling::{
    ConstrainedSamplingError, resolve_json_schema_strict_sampling,
};
use base64::Engine as _;
use cyrup_core::{Content, Message};
use serde_json::{Map, Value, json};

/// pi `EMPTY_TEXT_PLACEHOLDER` (`bedrock-converse-stream.ts:104`).
pub(super) const EMPTY_TEXT_PLACEHOLDER: &str = "<empty>";

/// The `cachePoint` block for the resolved retention (pi `:724-726` / `:912-919`).
///
/// The `ttl` value is the SDK's `CacheTTL.ONE_HOUR`, whose wire form is Bedrock's `"1h"` — the same
/// spelling Anthropic's own `cache_control.ttl` uses and which cyrup's `anthropic-messages` port
/// already emits.
fn cache_point(cache_retention: CacheRetention) -> Value {
    let mut point = Map::new();
    point.insert("type".to_string(), json!("default"));
    if cache_retention == CacheRetention::Long {
        point.insert("ttl".to_string(), json!("1h"));
    }
    json!({ "cachePoint": Value::Object(point) })
}

/// pi `buildSystemPrompt` (`bedrock-converse-stream.ts:712-730`).
pub(super) fn build_system_prompt(
    system_prompt: Option<&str>,
    model: &Model,
    cache_retention: CacheRetention,
    env: &EnvSource<'_>,
) -> Option<Vec<Value>> {
    let system_prompt = system_prompt.filter(|s| !s.is_empty())?;
    let mut blocks = vec![json!({ "text": sanitize_surrogates(system_prompt) })];
    if cache_retention != CacheRetention::None && supports_prompt_caching(model, env) {
        blocks.push(cache_point(cache_retention));
    }
    Some(blocks)
}

/// pi `normalizeToolCallId` (`bedrock-converse-stream.ts:732-735`): every character outside
/// `[a-zA-Z0-9_-]` becomes `_`, then the id is capped at 64 characters.
pub(super) fn normalize_tool_call_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.chars().count() > 64 {
        sanitized.chars().take(64).collect()
    } else {
        sanitized
    }
}

/// pi `createNonBlankTextBlock` (`bedrock-converse-stream.ts:737-740`).
fn non_blank_text_block(text: &str) -> Option<Value> {
    let sanitized = sanitize_surrogates(text);
    if sanitized.trim().is_empty() {
        None
    } else {
        Some(json!({ "text": sanitized }))
    }
}

/// pi `createRequiredTextBlock` (`bedrock-converse-stream.ts:742-744`).
fn required_text_block(text: &str) -> Value {
    non_blank_text_block(text).unwrap_or_else(|| json!({ "text": EMPTY_TEXT_PLACEHOLDER }))
}

/// pi `createImageBlock` (`bedrock-converse-stream.ts:1089-1116`).
///
/// Upstream decodes the base64 with `atob` and hands the SDK a `Uint8Array`; the REST binding then
/// re-encodes it as base64, so the bytes on the wire are the same. The decode is still performed
/// here because it is the check that makes upstream's `atob` throw on a malformed payload, and the
/// canonical re-encode normalises whitespace/padding the same way the SDK's serializer does.
fn create_image_block(mime_type: &str, data: &str) -> Result<Value, String> {
    let format = match mime_type {
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        other => return Err(format!("Unknown image type: {other}")),
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|_| "The string to be decoded contains invalid characters.".to_string())?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(json!({ "image": { "format": format, "source": { "bytes": encoded } } }))
}

/// pi `convertToolResultContent` (`bedrock-converse-stream.ts:746-758`).
fn convert_tool_result_content(content: &[Content]) -> Result<Vec<Value>, String> {
    let mut result = Vec::new();
    for c in content {
        match c {
            Content::Image { data, mime_type } => {
                result.push(create_image_block(mime_type, data)?);
            }
            Content::Text { text, .. } => {
                if let Some(block) = non_blank_text_block(text) {
                    result.push(block);
                }
            }
            // `ToolResultMessage.content` is `(TextContent | ImageContent)[]` upstream, and cyrup's
            // deserializer enforces the same, so the remaining variants are unreachable — upstream's
            // `else` branch treats anything non-image as text, which cannot fire here.
            _ => {}
        }
    }
    if result.is_empty() {
        result.push(json!({ "text": EMPTY_TEXT_PLACEHOLDER }));
    }
    Ok(result)
}

/// pi `convertMessages` (`bedrock-converse-stream.ts:760-923`).
pub(super) fn convert_messages(
    ctx: &Context,
    model: &Model,
    cache_retention: CacheRetention,
    env: &EnvSource<'_>,
) -> Result<Vec<Value>, String> {
    let transformed = transform_messages_with(&ctx.messages, model, normalize_tool_call_id);
    let mut result: Vec<Value> = Vec::new();

    let mut i = 0usize;
    while i < transformed.len() {
        let Some(m) = transformed.get(i) else { break };
        match m {
            Message::User { content, .. } => {
                let mut blocks: Vec<Value> = Vec::new();
                for c in content {
                    match c {
                        Content::Text { text, .. } => {
                            if let Some(block) = non_blank_text_block(text) {
                                blocks.push(block);
                            }
                        }
                        Content::Image { data, mime_type } => {
                            blocks.push(create_image_block(mime_type, data)?);
                        }
                        // pi's `default: continue` — an unknown block is skipped, never fatal.
                        _ => {}
                    }
                }
                if blocks.is_empty() {
                    blocks.push(required_text_block(""));
                }
                result.push(json!({ "role": "user", "content": blocks }));
                i += 1;
            }
            Message::Assistant(assistant) => {
                // pi `:803-805`: Bedrock rejects an empty assistant content array.
                if assistant.content.is_empty() {
                    i += 1;
                    continue;
                }
                let mut blocks: Vec<Value> = Vec::new();
                for c in &assistant.content {
                    match c {
                        Content::Text { text, .. } => {
                            if let Some(block) = non_blank_text_block(text) {
                                blocks.push(block);
                            }
                        }
                        Content::ToolCall(tc) => {
                            blocks.push(json!({
                                "toolUse": {
                                    "toolUseId": tc.id.as_str(),
                                    "name": tc.name,
                                    "input": Value::Object(tc.arguments.clone()),
                                }
                            }));
                        }
                        Content::Thinking {
                            thinking,
                            thinking_signature,
                            ..
                        } => {
                            let thinking = sanitize_surrogates(thinking);
                            if thinking.trim().is_empty() {
                                continue;
                            }
                            if supports_thinking_signature(model) {
                                // pi `:830-843`: a replayed reasoning block without a signature is
                                // rejected by Bedrock, so it degrades to plain text.
                                match thinking_signature.as_deref().filter(|s| !s.trim().is_empty())
                                {
                                    Some(sig) => blocks.push(json!({
                                        "reasoningContent": {
                                            "reasoningText": { "text": thinking, "signature": sig }
                                        }
                                    })),
                                    None => blocks.push(json!({ "text": thinking })),
                                }
                            } else {
                                blocks.push(json!({
                                    "reasoningContent": { "reasoningText": { "text": thinking } }
                                }));
                            }
                        }
                        _ => {}
                    }
                }
                if blocks.is_empty() {
                    i += 1;
                    continue;
                }
                result.push(json!({ "role": "assistant", "content": blocks }));
                i += 1;
            }
            Message::ToolResult { .. } => {
                // pi `:867-903`: every RUN of consecutive tool results collapses into ONE user
                // message, because Bedrock requires all results for a turn in a single message.
                let mut tool_results: Vec<Value> = Vec::new();
                while let Some(Message::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                    ..
                }) = transformed.get(i)
                {
                    tool_results.push(json!({
                        "toolResult": {
                            "toolUseId": tool_call_id.as_str(),
                            "content": convert_tool_result_content(content)?,
                            "status": if *is_error { "error" } else { "success" },
                        }
                    }));
                    i += 1;
                }
                result.push(json!({ "role": "user", "content": tool_results }));
            }
        }
    }

    // pi `:909-920`: the cache point goes on the LAST message, and only when it is a user message.
    if cache_retention != CacheRetention::None
        && supports_prompt_caching(model, env)
        && let Some(last) = result.last_mut()
        && last.get("role").and_then(Value::as_str) == Some("user")
        && let Some(Value::Array(content)) = last.get_mut("content")
    {
        content.push(cache_point(cache_retention));
    }

    Ok(result)
}

/// pi `convertToolConfig` (`bedrock-converse-stream.ts:925-960` @**v0.83.0**).
///
/// PROV-011 — `strict: true` is emitted only when `resolveJsonSchemaStrictSampling` resolves it
/// (`:934`, `:940`), against `model.compat?.supportsStrictMode ?? false` read at `:238`.
pub(super) fn convert_tool_config(
    tools: &[ToolDef],
    tool_choice: Option<&BedrockToolChoice>,
    supports_strict_mode: bool,
) -> Result<Option<Value>, ConstrainedSamplingError> {
    if tools.is_empty() {
        return Ok(None);
    }
    if matches!(tool_choice, Some(BedrockToolChoice::None)) {
        return Ok(None);
    }
    let bedrock_tools: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode)?;
            let mut spec = Map::new();
            spec.insert("name".to_string(), json!(tool.name));
            spec.insert("description".to_string(), json!(tool.description));
            spec.insert(
                "inputSchema".to_string(),
                json!({ "json": tool.parameters }),
            );
            if strict == Some(true) {
                spec.insert("strict".to_string(), json!(true));
            }
            Ok(json!({ "toolSpec": Value::Object(spec) }))
        })
        .collect::<Result<Vec<Value>, ConstrainedSamplingError>>()?;

    let mut config = Map::new();
    config.insert("tools".to_string(), Value::Array(bedrock_tools));
    if let Some(choice) = tool_choice.and_then(BedrockToolChoice::to_wire) {
        config.insert("toolChoice".to_string(), choice);
    }
    Ok(Some(Value::Object(config)))
}

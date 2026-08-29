//! Message conversion (Pi `convertMessages`).

use super::tools::{convert_tools, tools_by_name};
use super::transform::transform_messages;
use crate::api::compat::{DeferredToolsMode, ResolvedCompat, sanitize_surrogates};
use crate::context::Context;
use crate::model::Model;
use crate::utils::constrained_sampling::ConstrainedSamplingError;
use cyrup_core::{AssistantMessage, Content, Message, SharedStr, ToolCall};
use serde_json::{Map, Value, json};

/// Map cyrup [`Message`]s to OpenAI chat messages (Pi `convertMessages`, applying the compat flags).
pub(crate) fn convert_messages(
    model: &Model,
    ctx: &Context,
    compat: &ResolvedCompat,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    let transformed = transform_messages(&ctx.messages, model);
    let mut params: Vec<Value> = Vec::new();

    if let Some(system) = &ctx.system_prompt {
        let role = if model.reasoning && compat.supports_developer_role {
            "developer"
        } else {
            "system"
        };
        params.push(json!({ "role": role, "content": sanitize_surrogates(system) }));
    }

    let mut last_role: Option<&'static str> = None;
    let mut i = 0;
    while let Some(msg) = transformed.get(i) {
        // Bridge a synthetic assistant message between tool results and a following user message.
        if compat.requires_assistant_after_tool_result
            && last_role == Some("toolResult")
            && matches!(msg, Message::User { .. })
        {
            params.push(json!({
                "role": "assistant",
                "content": "I have processed the tool results.",
            }));
        }

        match msg {
            Message::User { content, .. } => {
                if content.is_empty() {
                    i += 1;
                    continue;
                }
                let uc = user_content(content, model.supports_image_input());
                if matches!(&uc, Value::Array(a) if a.is_empty()) {
                    i += 1;
                    continue;
                }
                params.push(json!({ "role": "user", "content": uc }));
                last_role = Some("user");
            }
            Message::Assistant(am) => match build_assistant(am, model, compat) {
                Some(value) => {
                    params.push(value);
                    last_role = Some("assistant");
                }
                None => {
                    i += 1;
                    continue;
                }
            },
            Message::ToolResult { .. } => {
                let mut image_blocks: Vec<Value> = Vec::new();
                // PROV-025 — pi's per-RUN `const deferredToolNames = new Set<string>()`
                // (`openai-completions.ts:1194` @v0.83.0), declared inside the tool-result branch
                // so each run emits its OWN inline tool block. A `Vec` rather than a set because
                // upstream's `Array.from(names)` walks a JS `Set` in INSERTION order and
                // `getToolsByName` preserves it (`:104-110`); a `HashSet` would randomize the
                // emitted tool order and a `BTreeSet` would sort it, and neither is what the wire
                // sees upstream.
                let mut deferred_tool_names: Vec<String> = Vec::new();
                let mut j = i;
                while let Some(Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    added_tool_names,
                    ..
                }) = transformed.get(j)
                {
                    let text_result = content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let has_images = content.iter().any(|c| matches!(c, Content::Image { .. }));
                    let has_text = !text_result.is_empty();

                    let mut tr = Map::new();
                    tr.insert("role".to_string(), json!("tool"));
                    tr.insert(
                        "content".to_string(),
                        json!(sanitize_surrogates(if has_text {
                            &text_result
                        } else {
                            "(see attached image)"
                        })),
                    );
                    tr.insert("tool_call_id".to_string(), json!(tool_call_id.as_str()));
                    if compat.requires_tool_result_name && !tool_name.is_empty() {
                        tr.insert("name".to_string(), json!(tool_name));
                    }
                    params.push(Value::Object(tr));

                    // `if (compat.deferredToolsMode === "kimi") { for (const name of
                    // toolMsg.addedToolNames ?? []) deferredToolNames.add(name); }`
                    // (`openai-completions.ts:1221-1226` @v0.83.0) — immediately after the tool
                    // message is pushed, before the image handling.
                    if compat.deferred_tools_mode == Some(DeferredToolsMode::Kimi) {
                        for name in added_tool_names {
                            if !deferred_tool_names.iter().any(|n| n == name) {
                                deferred_tool_names.push(name.clone());
                            }
                        }
                    }

                    if has_images && model.supports_image_input() {
                        for c in content {
                            if let Content::Image { data, mime_type } = c {
                                image_blocks.push(json!({
                                    "type": "image_url",
                                    "image_url": { "url": format!("data:{mime_type};base64,{data}") },
                                }));
                            }
                        }
                    }
                    j += 1;
                }
                i = j;

                if image_blocks.is_empty() {
                    last_role = Some("toolResult");
                } else {
                    if compat.requires_assistant_after_tool_result {
                        params.push(json!({
                            "role": "assistant",
                            "content": "I have processed the tool results.",
                        }));
                    }
                    let mut arr = vec![
                        json!({ "type": "text", "text": "Attached image(s) from tool result:" }),
                    ];
                    arr.extend(image_blocks);
                    params.push(json!({ "role": "user", "content": Value::Array(arr) }));
                    last_role = Some("user");
                }

                // `if (deferredToolNames.size > 0) { … params.push(kimiToolMessage) }`
                // (`openai-completions.ts:1266-1276` @v0.83.0), positioned exactly here: AFTER the
                // image/`lastRole` handling and immediately before the `continue`. Kimi accepts a
                // system message carrying `tools` and omitting the standard `content` field, so
                // the object has exactly the two keys upstream emits.
                if !deferred_tool_names.is_empty() {
                    let deferred_tools = tools_by_name(&ctx.tools, &deferred_tool_names);
                    if !deferred_tools.is_empty() {
                        params.push(json!({
                            "role": "system",
                            "tools": convert_tools(&deferred_tools, compat)?,
                        }));
                    }
                }
                continue;
            }
        }
        i += 1;
    }

    Ok(params)
}

/// Build an assistant chat message (Pi `convertMessages` assistant branch, L913-1013); `None` when
/// it has neither content nor tool calls.
fn build_assistant(am: &AssistantMessage, model: &Model, compat: &ResolvedCompat) -> Option<Value> {
    let mut obj = Map::new();
    obj.insert("role".to_string(), json!("assistant"));

    // Default content: "" when an assistant message is required after tool results, else null.
    let mut content_val: Value = if compat.requires_assistant_after_tool_result {
        json!("")
    } else {
        Value::Null
    };

    let text_parts: Vec<String> = am
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } if !text.trim().is_empty() => {
                Some(sanitize_surrogates(text))
            }
            _ => None,
        })
        .collect();
    let assistant_text: String = text_parts.concat();

    let thinking_blocks: Vec<(&SharedStr, &Option<String>)> = am
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Thinking {
                thinking,
                thinking_signature,
                ..
            } if !thinking.trim().is_empty() => Some((thinking, thinking_signature)),
            _ => None,
        })
        .collect();

    if let Some(first) = thinking_blocks.first() {
        let first_thinking_sig = first.1;
        if compat.requires_thinking_as_text {
            let thinking_text = thinking_blocks
                .iter()
                .map(|(t, _)| sanitize_surrogates(t))
                .collect::<Vec<_>>()
                .join("\n\n");
            let mut arr = vec![json!({ "type": "text", "text": thinking_text })];
            for tp in &text_parts {
                arr.push(json!({ "type": "text", "text": tp }));
            }
            content_val = Value::Array(arr);
        } else {
            if !assistant_text.is_empty() {
                content_val = json!(assistant_text);
            }
            // Replay reasoning under the original field name (llama.cpp server + gpt-oss).
            let mut signature = first_thinking_sig.clone();
            if model.provider.as_str() == "opencode-go" && signature.as_deref() == Some("reasoning")
            {
                signature = Some("reasoning_content".to_string());
            }
            if let Some(sig) = signature
                && !sig.is_empty()
            {
                let joined = thinking_blocks
                    .iter()
                    .map(|(t, _)| (*t).clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                obj.insert(sig, json!(joined));
            }
        }
    } else if !assistant_text.is_empty() {
        content_val = json!(assistant_text);
    }

    let tool_calls: Vec<&ToolCall> = am
        .content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .collect();
    let has_tool_calls = !tool_calls.is_empty();
    if has_tool_calls {
        let mut tc_values: Vec<Value> = Vec::new();
        let mut reasoning_details: Vec<Value> = Vec::new();
        for tc in &tool_calls {
            tc_values.push(json!({
                "id": tc.id.as_str(),
                "type": "function",
                "function": {
                    "name": tc.name,
                    "arguments": serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".to_string()),
                },
            }));
            if let Some(sig) = &tc.thought_signature
                && let Ok(parsed) = serde_json::from_str::<Value>(sig)
                && !parsed.is_null()
            {
                reasoning_details.push(parsed);
            }
        }
        obj.insert("tool_calls".to_string(), Value::Array(tc_values));
        if !reasoning_details.is_empty() {
            obj.insert(
                "reasoning_details".to_string(),
                Value::Array(reasoning_details),
            );
        }
    }

    if compat.requires_reasoning_content_on_assistant_messages
        && model.reasoning
        && !obj.contains_key("reasoning_content")
    {
        obj.insert("reasoning_content".to_string(), json!(""));
    }

    let has_content = match &content_val {
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => true,
    };
    if !has_content && !has_tool_calls {
        return None;
    }

    obj.insert("content".to_string(), content_val);
    Some(Value::Object(obj))
}

/// User content: a plain string when text-only, else an array of `text`/`image_url` parts.
fn user_content(content: &[Content], supports_image: bool) -> Value {
    let only_text = content.iter().all(|c| matches!(c, Content::Text { .. }));
    if only_text {
        return Value::String(sanitize_surrogates(&join_text(content)));
    }

    let mut parts: Vec<Value> = Vec::new();
    for block in content {
        match block {
            Content::Text { text, .. } => {
                parts.push(json!({ "type": "text", "text": sanitize_surrogates(text) }))
            }
            Content::Image { data, mime_type } if supports_image => parts.push(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{mime_type};base64,{data}") },
            })),
            _ => {}
        }
    }
    Value::Array(parts)
}

/// Concatenate the text blocks of a content vector.
fn join_text(content: &[Content]) -> String {
    let mut s = String::new();
    for block in content {
        if let Content::Text { text, .. } = block {
            s.push_str(text);
        }
    }
    s
}

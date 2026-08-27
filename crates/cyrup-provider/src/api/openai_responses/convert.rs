//! Message + tool conversion (Pi openai-responses-shared.ts): the `input` message array.

use super::ids::{build_foreign_responses_item_id, normalize_id_part};
use super::tools::{ConvertResponsesToolsOptions, convert_responses_tools};
use crate::api::compat::{get_responses_compat, sanitize_surrogates};
use crate::api::openai_completions::transform_messages_with_source;
use crate::context::{Context, ToolDef};
use crate::model::Model;
use crate::utils::constrained_sampling::ConstrainedSamplingError;
use crate::utils::hash::short_hash;
use cyrup_core::{AssistantMessage, Content, Message, TextPhase, TextSignatureV1};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

/// 1:1 port of Pi `convertResponsesMessages` (openai-responses-shared.ts:90-267).
///
/// `deferred_tools` is Pi's `options.deferredTools` map (`ConvertResponsesMessagesOptions`,
/// openai-responses-shared.ts:118) in insertion order: the tools that [`try_build_params`](super::params::try_build_params) withheld
/// from `body.tools` and that must instead be anchored at their `addedToolNames` marker as a
/// synthetic client `tool_search_call`/`tool_search_output` pair. Pass an empty slice to disable
/// the rendering entirely — that is what `azure-openai-responses` does (Pi
/// `azure-openai-responses.ts:280` passes options WITHOUT `deferredTools` and never imports
/// `splitDeferredTools`).
pub(crate) fn convert_responses_messages(
    model: &Model,
    ctx: &Context,
    allowed_tool_call_providers: &[&str],
    deferred_tools: &[(String, ToolDef)],
    tool_options: ConvertResponsesToolsOptions,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    let provider = model.provider.as_str().to_string();
    let api = model.api.clone();
    let model_id = model.id.as_str().to_string();
    let allow = allowed_tool_call_providers.contains(&provider.as_str());

    let normalize = |id: &str, source: &AssistantMessage| -> String {
        if !allow {
            return normalize_id_part(id);
        }
        if !id.contains('|') {
            return normalize_id_part(id);
        }
        let parts: Vec<&str> = id.split('|').collect();
        let call_id = parts.first().copied().unwrap_or("");
        let item_id = parts.get(1).copied().unwrap_or("");
        let normalized_call_id = normalize_id_part(call_id);
        let is_foreign = source.provider.as_str() != provider || source.api != api;
        let mut normalized_item_id = if is_foreign {
            build_foreign_responses_item_id(item_id)
        } else {
            normalize_id_part(item_id)
        };
        if !normalized_item_id.starts_with("fc_") {
            normalized_item_id = normalize_id_part(&format!("fc_{normalized_item_id}"));
        }
        format!("{normalized_call_id}|{normalized_item_id}")
    };

    let transformed = transform_messages_with_source(&ctx.messages, model, normalize);

    let mut messages: Vec<Value> = Vec::new();
    // Declared once per conversion so a deferred tool is loaded EXACTLY ONCE per request even when
    // several tool results name it (Pi `const loadedToolNames = new Set<string>()`,
    // openai-responses-shared.ts:143). Keys are RAW names — unlike the Anthropic path there is no
    // normalizer on this side.
    let mut loaded_tool_names: HashSet<String> = HashSet::new();

    if let Some(system) = &ctx.system_prompt {
        let compat = get_responses_compat(model);
        let role = if model.reasoning && compat.supports_developer_role {
            "developer"
        } else {
            "system"
        };
        messages.push(json!({ "role": role, "content": sanitize_surrogates(system) }));
    }

    let mut msg_index: i64 = 0;
    for msg in &transformed {
        match msg {
            Message::User { content, .. } => {
                let parts: Vec<Value> = content
                    .iter()
                    .filter_map(|item| match item {
                        Content::Text { text, .. } => Some(json!({
                            "type": "input_text",
                            "text": sanitize_surrogates(text),
                        })),
                        Content::Image { data, mime_type } => Some(json!({
                            "type": "input_image",
                            "detail": "auto",
                            "image_url": format!("data:{mime_type};base64,{data}"),
                        })),
                        _ => None,
                    })
                    .collect();
                if parts.is_empty() {
                    continue;
                }
                messages.push(json!({ "role": "user", "content": parts }));
            }
            Message::Assistant(am) => {
                let is_different_model =
                    am.model != model_id && am.provider.as_str() == provider && am.api == api;
                let mut output: Vec<Value> = Vec::new();
                let mut text_block_index: i64 = 0;
                for block in &am.content {
                    match block {
                        Content::Thinking {
                            thinking_signature, ..
                        } => {
                            if let Some(sig) = thinking_signature {
                                // The signature is the JSON-encoded reasoning item; replay verbatim.
                                if let Ok(item) = serde_json::from_str::<Value>(sig) {
                                    output.push(item);
                                }
                            }
                        }
                        Content::Text {
                            text,
                            text_signature,
                        } => {
                            let parsed = text_signature.as_deref().and_then(parse_text_signature);
                            let fallback = if text_block_index == 0 {
                                format!("msg_pi_{msg_index}")
                            } else {
                                format!("msg_pi_{msg_index}_{text_block_index}")
                            };
                            text_block_index += 1;
                            let mut msg_id = parsed.as_ref().map(|p| p.id.clone());
                            match &msg_id {
                                None => msg_id = Some(fallback),
                                Some(id) if id.chars().count() > 64 => {
                                    msg_id = Some(format!("msg_{}", short_hash(id)));
                                }
                                Some(_) => {}
                            }
                            let mut item = Map::new();
                            item.insert("type".to_string(), json!("message"));
                            item.insert("role".to_string(), json!("assistant"));
                            item.insert(
                                "content".to_string(),
                                json!([{
                                    "type": "output_text",
                                    "text": sanitize_surrogates(text),
                                    "annotations": [],
                                }]),
                            );
                            item.insert("status".to_string(), json!("completed"));
                            item.insert("id".to_string(), json!(msg_id));
                            if let Some(phase) = parsed.and_then(|p| p.phase) {
                                item.insert("phase".to_string(), json!(phase_wire(phase)));
                            }
                            output.push(Value::Object(item));
                        }
                        Content::ToolCall(tc) => {
                            let id = tc.id.as_str();
                            let parts: Vec<&str> = id.split('|').collect();
                            let call_id = parts.first().copied().unwrap_or("");
                            let mut item_id = parts.get(1).copied().map(|s| s.to_string());
                            // Drop a different-model `fc_*` item id to avoid pairing validation.
                            if is_different_model
                                && item_id
                                    .as_deref()
                                    .map(|s| s.starts_with("fc_"))
                                    .unwrap_or(false)
                            {
                                item_id = None;
                            }
                            let mut item = Map::new();
                            item.insert("type".to_string(), json!("function_call"));
                            if let Some(iid) = item_id {
                                item.insert("id".to_string(), json!(iid));
                            }
                            item.insert("call_id".to_string(), json!(call_id));
                            item.insert("name".to_string(), json!(tc.name));
                            item.insert(
                                "arguments".to_string(),
                                json!(serde_json::to_string(&tc.arguments).unwrap_or_default()),
                            );
                            output.push(Value::Object(item));
                        }
                        Content::Image { .. } => {}
                    }
                }
                if output.is_empty() {
                    continue;
                }
                messages.extend(output);
            }
            Message::ToolResult {
                tool_call_id,
                content,
                added_tool_names,
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
                let has_images = content.iter().any(|c| matches!(c, Content::Image { .. }));
                let has_text = !text_result.is_empty();
                let call_id = tool_call_id
                    .as_str()
                    .split('|')
                    .next()
                    .unwrap_or(tool_call_id.as_str());

                let output: Value = if has_images && model.supports_image_input() {
                    let mut parts: Vec<Value> = Vec::new();
                    if has_text {
                        parts.push(json!({
                            "type": "input_text",
                            "text": sanitize_surrogates(&text_result),
                        }));
                    }
                    for block in content {
                        if let Content::Image { data, mime_type } = block {
                            parts.push(json!({
                                "type": "input_image",
                                "detail": "auto",
                                "image_url": format!("data:{mime_type};base64,{data}"),
                            }));
                        }
                    }
                    Value::Array(parts)
                } else {
                    Value::String(sanitize_surrogates(if has_text {
                        &text_result
                    } else {
                        "(see attached image)"
                    }))
                };

                messages.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));

                // --- DRIFT-001 anchor: the Responses rendering (Pi
                // openai-responses-shared.ts:304-332) ---
                //
                // Injected IMMEDIATELY AFTER the `function_call_output` at this transcript index,
                // so the definitions land at the point in the conversation where the tools became
                // available. A marked name that is absent from `deferred_tools` produces nothing:
                // either the model can already see it (it was left immediate) or it is not in
                // `Context.tools` at all.
                let mut loaded: Vec<&ToolDef> = Vec::new();
                for name in added_tool_names {
                    if loaded_tool_names.contains(name) {
                        continue;
                    }
                    // Pi `options?.deferredTools?.get(name)` — the map is keyed by the (identity-)
                    // normalized name, so this is a raw-name lookup.
                    let Some((_, tool)) = deferred_tools.iter().find(|(key, _)| key == name) else {
                        continue;
                    };
                    loaded_tool_names.insert(name.clone());
                    loaded.push(tool);
                }
                if !loaded.is_empty() {
                    let names: Vec<&str> = loaded.iter().map(|t| t.name.as_str()).collect();
                    // The hash input uses the FULL `tool_call_id`, INCLUDING any `|item_id`
                    // suffix — NOT the `call_id` split off above for `function_call_output`
                    // (Pi `${msg.toolCallId}:${names.join(",")}`, :306). Comma-joined for the
                    // hash, SPACE-joined for the query.
                    //
                    // The `pi_tool_load_` prefix is kept VERBATIM despite cyrup's `pi` → `cyrup`
                    // rebrand: this string is on the wire and is what the differential/golden
                    // parity harnesses diff. Renaming it would be a silent wire divergence, so it
                    // is deliberately NOT a `[CYRUP-DELTA]`.
                    let search_call_id = format!(
                        "pi_tool_load_{}",
                        short_hash(&format!("{}:{}", tool_call_id.as_str(), names.join(",")))
                    );
                    messages.push(json!({
                        "type": "tool_search_call",
                        "call_id": search_call_id,
                        "execution": "client",
                        "status": "completed",
                        "arguments": {
                            "query": names.join(" "),
                            "limit": names.len(),
                        },
                    }));
                    let defs: Vec<ToolDef> = loaded.into_iter().cloned().collect();
                    messages.push(json!({
                        "type": "tool_search_output",
                        "call_id": search_call_id,
                        "execution": "client",
                        "status": "completed",
                        // `defer_loading: true` on every definition in the output (Pi
                        // `{ ...options?.toolOptions, deferLoading: true }`, :330).
                        "tools": Value::Array(convert_responses_tools(
                            &defs,
                            ConvertResponsesToolsOptions {
                                defer_loading: true,
                                ..tool_options
                            },
                        )?),
                    }));
                }
            }
        }
        msg_index += 1;
    }

    Ok(messages)
}

/// Pi `parseTextSignature` (openai-responses-shared.ts:46-64): structured V1 JSON or a legacy
/// plain-string id.
fn parse_text_signature(signature: &str) -> Option<TextSignatureV1> {
    if signature.starts_with('{')
        && let Some(v1) = TextSignatureV1::parse(signature)
    {
        return Some(v1);
    }
    Some(TextSignatureV1 {
        v: 1,
        id: signature.to_string(),
        phase: None,
    })
}

/// The wire string for a [`TextPhase`] (Pi `commentary` / `final_answer`).
fn phase_wire(phase: TextPhase) -> &'static str {
    match phase {
        TextPhase::Commentary => "commentary",
        TextPhase::FinalAnswer => "final_answer",
    }
}

//! Request encoding: the Anthropic-style `cache_control` breakpoints.

use crate::api::compat::{CacheControlFormat, ResolvedCompat};
use crate::stream::CacheRetention;
use serde_json::{Map, Value, json};

/// Anthropic-style ephemeral cache-control marker (Pi `getCompatCacheControl`).
pub(super) fn compat_cache_control(compat: &ResolvedCompat, cache: CacheRetention) -> Option<Value> {
    if compat.cache_control_format != Some(CacheControlFormat::Anthropic)
        || cache == CacheRetention::None
    {
        return None;
    }
    let mut cc = Map::new();
    cc.insert("type".to_string(), json!("ephemeral"));
    if cache == CacheRetention::Long && compat.supports_long_cache_retention {
        cc.insert("ttl".to_string(), json!("1h"));
    }
    Some(Value::Object(cc))
}

/// Apply Anthropic `cache_control` to the system prompt, last tool, and last conversation message
/// (Pi `applyAnthropicCacheControl`).
pub(super) fn apply_anthropic_cache_control(
    messages: &mut [Value],
    tools: Option<&mut Vec<Value>>,
    cc: &Value,
) {
    add_cache_control_to_system_prompt(messages, cc);
    if let Some(tools) = tools
        && let Some(last) = tools.last_mut()
        && let Some(o) = last.as_object_mut()
    {
        o.insert("cache_control".to_string(), cc.clone());
    }
    add_cache_control_to_last_conversation_message(messages, cc);
}

fn add_cache_control_to_system_prompt(messages: &mut [Value], cc: &Value) {
    for msg in messages.iter_mut() {
        let role = msg.get("role").and_then(Value::as_str);
        if role == Some("system") || role == Some("developer") {
            if let Some(o) = msg.as_object_mut() {
                add_cache_control_to_text_content(o, cc);
            }
            return;
        }
    }
}

/// Pi `addCacheControlToLastConversationMessage` — `openai-completions.ts:913-925` @**v0.83.0**
/// (byte-identical at v0.84.1, `:964-976`), with pi's `addCacheControlToMessage` (`:946-954`
/// @v0.83.0) inlined because its role test is the same three-way test.
///
/// DRIFT-028: the `"tool"` arm was dropped in the port, so in an agent loop — where the last
/// message is almost always a tool result — the cache breakpoint landed one message too early on
/// every turn. Filed `upstream-drift`; it is **not-ported**: `git show
/// v0.83.0:packages/ai/src/api/openai-completions.ts` already has `message.role === "tool"` at
/// `:918` and `:947`, inside the ported baseline, so no rebase would have swept it up.
pub(super) fn add_cache_control_to_last_conversation_message(messages: &mut [Value], cc: &Value) {
    for msg in messages.iter_mut().rev() {
        let role = msg.get("role").and_then(Value::as_str);
        if (role == Some("user") || role == Some("assistant") || role == Some("tool"))
            && let Some(o) = msg.as_object_mut()
            && add_cache_control_to_text_content(o, cc)
        {
            return;
        }
    }
}

/// Add `cache_control` to a message's last text content (Pi `addCacheControlToTextContent`).
fn add_cache_control_to_text_content(message: &mut Map<String, Value>, cc: &Value) -> bool {
    match message.get("content") {
        Some(Value::String(s)) => {
            if s.is_empty() {
                return false;
            }
            let text = s.clone();
            let mut part = Map::new();
            part.insert("type".to_string(), json!("text"));
            part.insert("text".to_string(), json!(text));
            part.insert("cache_control".to_string(), cc.clone());
            message.insert(
                "content".to_string(),
                Value::Array(vec![Value::Object(part)]),
            );
            true
        }
        Some(Value::Array(_)) => {
            if let Some(Value::Array(arr)) = message.get_mut("content") {
                for part in arr.iter_mut().rev() {
                    if part.get("type").and_then(Value::as_str) == Some("text")
                        && let Some(o) = part.as_object_mut()
                    {
                        o.insert("cache_control".to_string(), cc.clone());
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

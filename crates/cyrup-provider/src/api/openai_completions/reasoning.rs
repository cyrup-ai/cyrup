//! Request encoding: the per-provider reasoning encoding and `chat_template_kwargs`.

use super::params::reasoning_effort;
use crate::api::compat::{
    ResolvedCompat, ThinkingFormat, level_map_lookup, mapped_effort_or, off_is_not_null,
    off_value_or, thinking_level_key,
};
use crate::model::Model;
use crate::stream::StreamOptions;
use serde_json::{Map, Value, json};

/// Apply the per-provider reasoning encoding (Pi `buildParams` reasoning chain, L594-668). Each
/// branch is gated on `model.reasoning` and the resolved `thinking_format`.
pub(super) fn apply_reasoning(
    obj: &mut Map<String, Value>,
    model: &Model,
    opts: &StreamOptions,
    compat: &ResolvedCompat,
) {
    if !model.reasoning {
        return;
    }
    let map = model.thinking_level_map.as_ref();
    let level = opts.reasoning;
    let key = thinking_level_key(level);
    // `options.reasoningEffort`: `Some(effort)` when reasoning is on, `None` when off.
    let eff: Option<&'static str> = reasoning_effort(level);
    let sre = compat.supports_reasoning_effort;

    match compat.thinking_format {
        ThinkingFormat::Zai => {
            obj.insert(
                "thinking".to_string(),
                json!({ "type": if eff.is_some() { "enabled" } else { "disabled" } }),
            );
            if let Some(e) = eff
                && sre
            {
                // mappedEffort === undefined ? reasoningEffort : mappedEffort; emit only if string.
                let effort = match level_map_lookup(map, key) {
                    None => Some(e.to_string()),
                    Some(None) => None,
                    Some(Some(s)) => Some(s.clone()),
                };
                if let Some(s) = effort {
                    obj.insert("reasoning_effort".to_string(), json!(s));
                }
            }
        }
        ThinkingFormat::Qwen => {
            obj.insert("enable_thinking".to_string(), json!(eff.is_some()));
        }
        ThinkingFormat::QwenChatTemplate => {
            obj.insert(
                "chat_template_kwargs".to_string(),
                json!({ "enable_thinking": eff.is_some(), "preserve_thinking": true }),
            );
        }
        ThinkingFormat::ChatTemplate => {
            if let Some(kwargs) =
                build_chat_template_values(model, opts, &compat.chat_template_kwargs)
            {
                obj.insert("chat_template_kwargs".to_string(), Value::Object(kwargs));
            }
        }
        // DRIFT-009 — `api/openai-completions.ts:888-904` @v0.84.4. Two independent halves, and
        // neither is gated on the other: `chat_template_args` is emitted whenever the resolved
        // args map produces at least one value (`:893-896`), and `reasoning_effort` whenever the
        // model `supportsReasoningEffort` (`:897-903`) — note there is NO `options.reasoningEffort`
        // guard on the second half, unlike every sibling branch: with thinking OFF it falls back
        // to `thinkingLevelMap.off` (`:899`), so Baseten is told "off" explicitly rather than being
        // left to its own default.
        ThinkingFormat::Baseten => {
            if let Some(args) = build_chat_template_values(model, opts, &compat.chat_template_args)
            {
                obj.insert("chat_template_args".to_string(), Value::Object(args));
            }
            if sre {
                // `mappedEffort = requestedEffort ? map[requestedEffort] : map.off`, then
                // `effort = mappedEffort === undefined ? requestedEffort : mappedEffort`, emitted
                // only when it is a string (`:899-903`).
                let mapped = if eff.is_some() {
                    level_map_lookup(map, key)
                } else {
                    level_map_lookup(map, "off")
                };
                let effort = match mapped {
                    None => eff.map(str::to_string),
                    Some(Some(s)) => Some(s.clone()),
                    Some(None) => None,
                };
                if let Some(s) = effort {
                    obj.insert("reasoning_effort".to_string(), json!(s));
                }
            }
        }
        ThinkingFormat::Deepseek => {
            if eff.is_some() {
                obj.insert("thinking".to_string(), json!({ "type": "enabled" }));
            } else if off_is_not_null(map) {
                obj.insert("thinking".to_string(), json!({ "type": "disabled" }));
            }
            if let Some(e) = eff
                && sre
            {
                obj.insert(
                    "reasoning_effort".to_string(),
                    json!(mapped_effort_or(map, level, e)),
                );
            }
        }
        ThinkingFormat::Openrouter => {
            if let Some(e) = eff {
                obj.insert(
                    "reasoning".to_string(),
                    json!({ "effort": mapped_effort_or(map, level, e) }),
                );
            } else if off_is_not_null(map) {
                obj.insert(
                    "reasoning".to_string(),
                    json!({ "effort": off_value_or(map, "none") }),
                );
            }
        }
        ThinkingFormat::AntLing => {
            if eff.is_some()
                && let Some(Some(s)) = level_map_lookup(map, key)
            {
                obj.insert("reasoning".to_string(), json!({ "effort": s }));
            }
        }
        ThinkingFormat::Together => {
            obj.insert("reasoning".to_string(), json!({ "enabled": eff.is_some() }));
            if let Some(e) = eff
                && sre
            {
                obj.insert(
                    "reasoning_effort".to_string(),
                    json!(mapped_effort_or(map, level, e)),
                );
            }
        }
        ThinkingFormat::StringThinking => {
            if let Some(e) = eff {
                obj.insert(
                    "thinking".to_string(),
                    json!(mapped_effort_or(map, level, e)),
                );
            } else if off_is_not_null(map) {
                obj.insert("thinking".to_string(), json!(off_value_or(map, "none")));
            }
        }
        ThinkingFormat::Openai => {
            // OpenAI-style `reasoning_effort` (Pi's two fallthrough branches).
            if let Some(e) = eff {
                if sre {
                    obj.insert(
                        "reasoning_effort".to_string(),
                        json!(mapped_effort_or(map, level, e)),
                    );
                }
            } else if sre && let Some(Some(s)) = level_map_lookup(map, "off") {
                obj.insert("reasoning_effort".to_string(), json!(s));
            }
        }
    }
}

/// Resolve one of the two `ChatTemplateKwargValue` maps (Pi `buildChatTemplateValues`,
/// `openai-completions.ts:1010-1026` @v0.84.4).
///
/// DRIFT-009 took the `compat`-shaped parameter off this function: upstream passes the MAP
/// (`compat.chatTemplateKwargs` at `:884`, `compat.chatTemplateArgs` at `:893`), because the same
/// resolution feeds two different request fields.
fn build_chat_template_values(
    model: &Model,
    opts: &StreamOptions,
    values: &Map<String, Value>,
) -> Option<Map<String, Value>> {
    let mut kwargs = Map::new();
    for (key, value) in values {
        if let Some(resolved) = resolve_chat_template_kwarg_value(model, opts, value) {
            kwargs.insert(key.clone(), resolved);
        }
    }
    if kwargs.is_empty() {
        None
    } else {
        Some(kwargs)
    }
}

/// Resolve one `ChatTemplateKwargValue` (Pi `resolveChatTemplateKwargValue`).
fn resolve_chat_template_kwarg_value(
    model: &Model,
    opts: &StreamOptions,
    value: &Value,
) -> Option<Value> {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Some(value.clone()),
    };
    let map = model.thinking_level_map.as_ref();
    let level = opts.reasoning;
    let eff = reasoning_effort(level);

    if eff.is_none() && obj.get("omitWhenOff").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    if obj.get("$var").and_then(Value::as_str) == Some("thinking.enabled") {
        return Some(json!(eff.is_some()));
    }

    let mapped = if eff.is_some() {
        level_map_lookup(map, thinking_level_key(level))
    } else {
        level_map_lookup(map, "off")
    };
    match mapped {
        None => eff.map(|e| json!(e)),
        Some(Some(s)) => Some(json!(s)),
        Some(None) => None,
    }
}

//! Request builder: the request body, max-tokens field and `reasoning_effort`.

use super::*;

/// PROV-069 — the production path: NO caller cap, so the MODEL's `max_tokens` must reach the
/// wire. Reported from live use — every reply truncated mid-sentence with `finish_reason:
/// length` at ~3% of a 1M context window.
///
/// RED before the fix, and this is the test the suite was missing rather than getting wrong:
/// `GenConfig::max_tokens` has no production writer (`grep -rn '\.max_tokens(' crates/ | grep -v
/// tests` is empty), so `opts.max_tokens` is always `None` in the product, the key was never
/// emitted, and the server applied its own small default. Every OTHER wire test here supplies
/// `max_tokens: Some(...)` by hand — which proves serialisation and hides the one path that
/// actually ships.
#[test]
fn with_no_caller_cap_the_models_own_max_tokens_reaches_the_body() {
    let ctx = Context {
        system_prompt: None,
        messages: vec![],
        tools: vec![],
    };

    // Exactly what the turn path passes today: nothing.
    let body = build_body(&model(), &ctx, &StreamOptions::default());
    assert_eq!(
        body["max_tokens"], 131_072,
        "the catalog's max_tokens must reach the request, not sit decorative: {body}"
    );

    // A caller cap still wins, so a `maxTokens` setting / modelOverrides keeps precedence.
    let capped = build_body(
        &model(),
        &ctx,
        &StreamOptions {
            max_tokens: Some(256),
            ..Default::default()
        },
    );
    assert_eq!(
        capped["max_tokens"], 256,
        "an explicit caller cap beats the model's"
    );

    // Modelless fallback (`max_tokens: 0`) sends nothing, leaving upstream behaviour unchanged.
    let mut modelless = model();
    modelless.max_tokens = 0;
    let none = build_body(&modelless, &ctx, &StreamOptions::default());
    assert!(
        none.get("max_tokens").is_none(),
        "a zero model ceiling means unknown — send no key: {none}"
    );
}

#[test]
fn request_body_matches_openai_shape() {
    let ctx = Context {
        system_prompt: Some("be terse".to_string()),
        messages: vec![
            Message::User {
                content: vec![Content::text("hi")],
                timestamp: 0,
            },
            Message::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCall {
                    id: ToolCallId::from("call_1"),
                    name: "get_weather".into(),
                    arguments: json!({ "city": "Paris" })
                        .as_object()
                        .cloned()
                        .expect("object")
                        .into(),
                    thought_signature: None,
                })],
                provider: "together".into(),
                model: "m".into(),
                api: "openai-completions".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                timestamp: 0,
            }),
            Message::ToolResult {
                tool_call_id: ToolCallId::from("call_1"),
                tool_name: "get_weather".into(),
                content: vec![Content::text("sunny")],
                is_error: false,
                details: None,
                timestamp: 0,
                usage: None,
                added_tool_names: Vec::new(),
            },
        ],
        tools: vec![ToolDef {
            name: "get_weather".into(),
            description: "Get weather".into(),
            parameters: json!({
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"],
            }),
            constrained_sampling: None,
        }],
    };

    let opts = StreamOptions {
        max_tokens: Some(256),
        temperature: Some(0.5),
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };

    let body = build_body(&model(), &ctx, &opts);

    assert_eq!(body["model"], "openai/gpt-oss-120b");
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
    // Together uses `max_tokens` (not `max_completion_tokens`) and omits `store`
    // (supportsStore=false), unlike standard OpenAI which sends `store: false`.
    assert_eq!(body["max_tokens"], 256);
    assert!(body.get("max_completion_tokens").is_none());
    assert!(body.get("store").is_none());
    assert_eq!(body["temperature"], 0.5);
    // Together encodes reasoning as `reasoning: { enabled }` and NEVER `reasoning_effort`.
    assert_eq!(body["reasoning"], json!({ "enabled": true }));
    assert!(body.get("reasoning_effort").is_none());

    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(
        messages[0],
        json!({ "role": "system", "content": "be terse" })
    );
    assert_eq!(messages[1], json!({ "role": "user", "content": "hi" }));
    // assistant tool call — content is `null` (Pi sends null unless an assistant message is
    // required after tool results; Together does not require that).
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[2]["content"], Value::Null);
    let tcs = messages[2]["tool_calls"].as_array().unwrap();
    assert_eq!(tcs[0]["id"], "call_1");
    assert_eq!(tcs[0]["type"], "function");
    assert_eq!(tcs[0]["function"]["name"], "get_weather");
    assert_eq!(tcs[0]["function"]["arguments"], "{\"city\":\"Paris\"}");
    // tool result
    assert_eq!(
        messages[3],
        json!({ "role": "tool", "tool_call_id": "call_1", "content": "sunny" })
    );

    // tools
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "get_weather");
    assert_eq!(tools[0]["function"]["description"], "Get weather");
    assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
    // Together does not support `strict` on tools.
    assert!(tools[0]["function"].get("strict").is_none());
    // No caller `tool_choice` => the field is omitted (Pi never auto-injects "auto").
    assert!(body.get("tool_choice").is_none());
}

#[test]
fn reasoning_effort_omitted_for_non_reasoning_model() {
    let mut m = model();
    m.reasoning = false;
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = build_body(&m, &Context::default(), &opts);
    assert!(body.get("reasoning_effort").is_none());
    assert!(body.get("reasoning").is_none());
}

#[test]
fn openai_uses_max_completion_tokens_store_and_reasoning_effort() {
    let m = openai_model();
    let opts = StreamOptions {
        max_tokens: Some(100),
        reasoning: ModelThinkingLevel::Medium,
        ..Default::default()
    };
    let body = build_body(&m, &Context::default(), &opts);
    // OpenAI uses `max_completion_tokens`, `store: false`, and `reasoning_effort`.
    assert_eq!(body["max_completion_tokens"], 100);
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["store"], false);
    assert_eq!(body["reasoning_effort"], "medium");
    assert!(body.get("reasoning").is_none());
}

#[test]
fn openai_reasoning_effort_uses_thinking_level_map() {
    let mut m = openai_model();
    // Map "high" -> "xhigh" wire value (Pi `thinkingLevelMap`).
    m.thinking_level_map = Some(crate::model::ThinkingLevelMap::from([(
        "high".to_string(),
        Some("xhigh".to_string()),
    )]));
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = build_body(&m, &Context::default(), &opts);
    assert_eq!(body["reasoning_effort"], "xhigh");
}

/// PROV-002: `max` is a first-class `reasoning_effort` value. Pi passes the level string
/// verbatim (`reasoningEffort = clampedReasoning`, openai-completions.ts:621) and its option
/// union lists `"max"` (:143).
#[test]
fn openai_reasoning_effort_encodes_max() {
    let m = openai_model();
    let body = build_body(
        &m,
        &Context::default(),
        &StreamOptions {
            reasoning: ModelThinkingLevel::Max,
            ..Default::default()
        },
    );
    assert_eq!(body["reasoning_effort"], "max");
}

/// The real corrected catalog: `deepseek-v4-pro` maps `max -> "max"` (pi deepseek.models.ts
/// @91585d9a) and must send it, proving the DRIFT-008 catalog values reach the wire.
#[test]
fn deepseek_catalog_sends_max_effort() {
    use crate::collection::get_supported_thinking_levels;
    let m = crate::providers::fleet::DEEPSEEK
        .models()
        .iter()
        .find(|m| m.id.as_str() == "deepseek-v4-pro")
        .expect("deepseek-v4-pro")
        .clone();
    assert!(get_supported_thinking_levels(&m).contains(&ModelThinkingLevel::Max));
    let body = build_body(
        &m,
        &Context::default(),
        &StreamOptions {
            reasoning: ModelThinkingLevel::Max,
            max_tokens: Some(64),
            ..Default::default()
        },
    );
    assert_eq!(body["reasoning_effort"], "max", "body={body}");
}

// ------------------------------------------------------- DRIFT-009: the `baseten` thinkingFormat --

/// The exact `zai-org/GLM-5.2` row pi's own test pins (`packages/ai/test/baseten-models.test.ts:19-54`
/// @v0.84.4), in the JSON shape a catalog / pi.dev-overlay row arrives in.
///
/// It is written as JSON rather than as a `Model` literal on purpose: Baseten is one of the four
/// providers whose rows are in git at no upstream revision (DRIFT-009), so **every** row it will
/// ever have reaches cyrup by deserialization at runtime. A `ThinkingFormat` that cannot parse
/// `"baseten"` fails the whole row, and a provider whose rows all fail to parse is a provider that
/// silently offers nothing — which is why this fixture goes through `serde_json`.
fn baseten_glm_52_row() -> &'static str {
    r#"{
        "id": "zai-org/GLM-5.2",
        "name": "GLM-5.2",
        "api": "openai-completions",
        "provider": "baseten",
        "baseUrl": "https://inference.baseten.co/v1",
        "reasoning": true,
        "thinkingLevelMap": {
            "off": "none", "minimal": null, "low": null, "medium": null,
            "high": "high", "xhigh": null, "max": "max"
        },
        "input": ["text", "image"],
        "contextWindow": 1048576,
        "maxTokens": 262144,
        "cost": { "input": 1.4, "output": 4.4, "cacheRead": 0.3, "cacheWrite": 0 },
        "compat": {
            "supportsStore": false,
            "supportsDeveloperRole": false,
            "supportsReasoningEffort": true,
            "supportsUsageInStreaming": true,
            "maxTokensField": "max_tokens",
            "supportsStrictMode": true,
            "supportsLongCacheRetention": false,
            "thinkingFormat": "baseten",
            "chatTemplateArgs": { "enable_thinking": { "$var": "thinking.enabled" } }
        }
    }"#
}

/// DRIFT-009 — `api/openai-completions.ts:888-904` @v0.84.4, against the row pi's
/// `baseten-models.test.ts` pins.
///
/// Two independent halves. `chat_template_args` carries the resolved `chatTemplateArgs` map
/// (`:893-896`) — `{"$var": "thinking.enabled"}` becomes the boolean. `reasoning_effort` is the
/// thinking-level map's value for the requested level (`:897-903`), and — unlike every sibling
/// branch — it is emitted **with thinking off too**, from `thinkingLevelMap.off` (`:899`).
#[test]
fn baseten_sends_chat_template_args_and_a_mapped_reasoning_effort() {
    let model: Model = serde_json::from_str(baseten_glm_52_row()).expect("catalog row parses");
    let ctx = Context {
        system_prompt: None,
        messages: vec![],
        tools: vec![],
    };

    let on = build_body(
        &model,
        &ctx,
        &StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        },
    );
    assert_eq!(
        on["chat_template_args"],
        json!({ "enable_thinking": true }),
        "`{{$var: thinking.enabled}}` resolves to the boolean and rides `chat_template_args`, not \
         `chat_template_kwargs`: {on}"
    );
    assert!(
        on.get("chat_template_kwargs").is_none(),
        "the two maps are separate request fields (`:884` vs `:893`): {on}"
    );
    assert_eq!(on["reasoning_effort"], "high", "map[high] = \"high\": {on}");

    // `max` maps to "max" on this row, which is the rung `thinkingLevelMap` exists to carry.
    let maxed = build_body(
        &model,
        &ctx,
        &StreamOptions {
            reasoning: ModelThinkingLevel::Max,
            ..Default::default()
        },
    );
    assert_eq!(maxed["reasoning_effort"], "max");

    // Thinking OFF: `mappedEffort = map.off` (`:899`) — Baseten is told "none" explicitly.
    let off = build_body(&model, &ctx, &StreamOptions::default());
    assert_eq!(
        off["chat_template_args"],
        json!({ "enable_thinking": false }),
        "{off}"
    );
    assert_eq!(
        off["reasoning_effort"], "none",
        "there is no `options.reasoningEffort` guard on the effort half (`:897`): {off}"
    );
}

/// The rung the map nulls out sends no `reasoning_effort` at all — `mappedEffort` is `null`, not
/// `undefined`, so the `typeof effort === "string"` guard at `:901` rejects it. And a row that
/// does not `supportsReasoningEffort` never reaches the guard (`:897`).
#[test]
fn baseten_omits_reasoning_effort_for_a_nulled_rung_and_without_effort_support() {
    let model: Model = serde_json::from_str(baseten_glm_52_row()).expect("catalog row parses");
    let ctx = Context {
        system_prompt: None,
        messages: vec![],
        tools: vec![],
    };

    let low = build_body(
        &model,
        &ctx,
        &StreamOptions {
            reasoning: ModelThinkingLevel::Low,
            ..Default::default()
        },
    );
    assert!(
        low.get("reasoning_effort").is_none(),
        "map[low] is null — send nothing: {low}"
    );
    assert_eq!(
        low["chat_template_args"],
        json!({ "enable_thinking": true }),
        "the args half is independent of the effort half: {low}"
    );

    // pi's `toggleReasoningCompat` (`ai/scripts/generate-models.ts:1274-1278`): the same format
    // with `supportsReasoningEffort: false`, which is what a Baseten row without an `effort`
    // reasoning option generates.
    let mut toggle_only = model.clone();
    toggle_only
        .compat
        .as_mut()
        .expect("compat")
        .supports_reasoning_effort = Some(false);
    let body = build_body(
        &toggle_only,
        &ctx,
        &StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        },
    );
    assert!(body.get("reasoning_effort").is_none(), "{body}");
    assert_eq!(
        body["chat_template_args"],
        json!({ "enable_thinking": true })
    );
}

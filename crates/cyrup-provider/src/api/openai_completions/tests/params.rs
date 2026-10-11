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
                    namespace: None,
                })],
                provider: "together".into(),
                model: "m".into(),
                api: "openai-completions".into(),
                response_model: None,
                response_id: None,
                provider_thinking_level: None,
                thinking_level: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 0,
                duration_ms: None,
            }),
            Message::ToolResult {
                duration_ms: None,
                tool_call_id: ToolCallId::from("call_1"),
                tool_name: "get_weather".into(),
                content: vec![Content::text("sunny")],
                is_error: false,
                details: None,
                timestamp: 0,
                usage: None,
                added_tool_names: Vec::new(),
                nested_calls: None,
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

/// PROV-079 — pi #9797 (`openai-completions.ts:1261` @v0.87.1): an attachment sent with no prompt
/// text carries only the image part, not a leading `{"type":"text","text":""}` that some
/// OpenAI-compatible providers reject. Non-empty text parts are kept, in order.
#[test]
fn empty_text_parts_are_dropped_from_a_user_content_array() {
    let mut m = model();
    m.input = vec![Modality::Text, Modality::Image];
    let image = Content::Image {
        data: "aGk=".to_string(),
        mime_type: "image/png".to_string(),
    };
    let ctx = Context {
        system_prompt: None,
        messages: vec![Message::User {
            content: vec![Content::text(""), image.clone()],
            timestamp: 0,
        }],
        tools: vec![],
    };
    let body = build_body(&m, &ctx, &StreamOptions::default());
    assert_eq!(
        body["messages"][0]["content"],
        json!([{ "type": "image_url", "image_url": { "url": "data:image/png;base64,aGk=" } }]),
        "{body}"
    );

    let ctx = Context {
        system_prompt: None,
        messages: vec![Message::User {
            content: vec![Content::text("look"), Content::text(""), image],
            timestamp: 0,
        }],
        tools: vec![],
    };
    let body = build_body(&m, &ctx, &StreamOptions::default());
    let parts = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(parts.len(), 2, "{body}");
    assert_eq!(parts[0], json!({ "type": "text", "text": "look" }));
    assert_eq!(parts[1]["type"], "image_url");
}

/// PROV-100 — `test/openai-completions-thinking-token-budget.test.ts` @v0.87.1, translated. A local
/// vLLM GLM row (`maxTokens: 16384`) whose compat is `{thinkingFormat:"zai",
/// supportsThinkingTokenBudget:true}` unless a case overrides it. `max_tokens` is what pi's
/// `streamSimple` resolves (`options.maxTokens ?? model.maxTokens`, clamped to a context this
/// window never reaches).
mod prov100_thinking_token_budget {
    use super::*;
    use crate::api::compat::{ThinkingFormat, ThinkingTokenBudgetField};
    use crate::utils::simple_options::ThinkingBudgets;

    fn vllm_model(compat: Option<ModelCompat>) -> Model {
        let mut m = model();
        m.id = "zai-org/glm-5.2".into();
        m.provider = "local-vllm".into();
        m.base_url = "http://localhost:8000/v1".to_string();
        m.context_window = 262_144;
        m.max_tokens = 16_384;
        m.compat = Some(compat.unwrap_or(ModelCompat {
            thinking_format: Some(ThinkingFormat::Zai),
            supports_thinking_token_budget: Some(true),
            ..Default::default()
        }));
        m
    }

    fn capture(
        m: &Model,
        reasoning: ModelThinkingLevel,
        budgets: Option<ThinkingBudgets>,
        max_tokens: Option<u64>,
    ) -> Value {
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text("Hi")],
                timestamp: 0,
            }],
            tools: vec![],
        };
        build_body(
            m,
            &ctx,
            &StreamOptions {
                reasoning,
                thinking_budgets: budgets,
                max_tokens: Some(max_tokens.unwrap_or(m.max_tokens)),
                ..Default::default()
            },
        )
    }

    fn medium(n: u64) -> Option<ThinkingBudgets> {
        Some(ThinkingBudgets {
            medium: Some(n),
            ..Default::default()
        })
    }

    fn high(n: u64) -> Option<ThinkingBudgets> {
        Some(ThinkingBudgets {
            high: Some(n),
            ..Default::default()
        })
    }

    #[test]
    fn sends_the_configured_budget_for_the_requested_level() {
        let p = capture(
            &vllm_model(None),
            ModelThinkingLevel::Medium,
            medium(4096),
            None,
        );
        assert_eq!(p["thinking_token_budget"], 4096, "{p}");
    }

    #[test]
    fn omits_the_budget_when_neither_the_field_nor_the_alias_is_set() {
        let m = vllm_model(Some(ModelCompat {
            thinking_format: Some(ThinkingFormat::Zai),
            ..Default::default()
        }));
        let p = capture(&m, ModelThinkingLevel::Medium, medium(4096), None);
        for field in [
            "thinking_token_budget",
            "thinking_budget",
            "thinking_budget_tokens",
        ] {
            assert!(p.get(field).is_none(), "{field}: {p}");
        }
    }

    #[test]
    fn omits_the_budget_when_thinking_is_off() {
        let p = capture(&vllm_model(None), ModelThinkingLevel::Off, high(8192), None);
        assert!(p.get("thinking_token_budget").is_none(), "{p}");
    }

    #[test]
    fn clamps_xhigh_and_max_to_the_high_budget() {
        for level in [ModelThinkingLevel::Xhigh, ModelThinkingLevel::Max] {
            let p = capture(&vllm_model(None), level, high(8192), None);
            assert_eq!(p["thinking_token_budget"], 8192, "{level:?}: {p}");
        }
    }

    #[test]
    fn leaves_room_for_the_answer_when_the_budget_meets_the_response_ceiling() {
        let p = capture(&vllm_model(None), ModelThinkingLevel::High, None, None);
        assert_eq!(p["thinking_token_budget"], 16_384 - 1024, "{p}");
    }

    #[test]
    fn uses_the_caller_max_tokens_as_the_ceiling_when_lower_than_the_model_cap() {
        let p = capture(
            &vllm_model(None),
            ModelThinkingLevel::High,
            high(8192),
            Some(4096),
        );
        assert_eq!(p["thinking_token_budget"], 4096 - 1024, "{p}");
    }

    #[test]
    fn sends_the_named_field_when_thinking_token_budget_field_is_set() {
        for (field, name) in [
            (ThinkingTokenBudgetField::ThinkingBudget, "thinking_budget"),
            (
                ThinkingTokenBudgetField::ThinkingBudgetTokens,
                "thinking_budget_tokens",
            ),
        ] {
            let m = vllm_model(Some(ModelCompat {
                thinking_format: Some(ThinkingFormat::Qwen),
                thinking_token_budget_field: Some(field),
                ..Default::default()
            }));
            let p = capture(&m, ModelThinkingLevel::Medium, medium(4096), None);
            assert_eq!(p[name], 4096, "{p}");
            assert!(p.get("thinking_token_budget").is_none(), "{p}");
        }
    }

    /// The two keys in their `models.json` spelling (pi's camelCase key, snake_case field values).
    #[test]
    fn both_keys_parse_from_their_models_json_form() {
        for (compat, name) in [
            (
                json!({ "thinkingFormat": "qwen", "thinkingTokenBudgetField": "thinking_budget_tokens" }),
                "thinking_budget_tokens",
            ),
            (
                json!({ "thinkingFormat": "zai", "supportsThinkingTokenBudget": true }),
                "thinking_token_budget",
            ),
        ] {
            let m = vllm_model(Some(serde_json::from_value(compat).unwrap()));
            let p = capture(&m, ModelThinkingLevel::Medium, medium(4096), None);
            assert_eq!(p[name], 4096, "{p}");
        }
    }

    #[test]
    fn thinking_token_budget_field_wins_over_the_boolean_alias() {
        let m = vllm_model(Some(ModelCompat {
            thinking_format: Some(ThinkingFormat::Zai),
            supports_thinking_token_budget: Some(true),
            thinking_token_budget_field: Some(ThinkingTokenBudgetField::ThinkingBudget),
            ..Default::default()
        }));
        let p = capture(&m, ModelThinkingLevel::Medium, medium(4096), None);
        assert_eq!(p["thinking_budget"], 4096, "{p}");
        assert!(p.get("thinking_token_budget").is_none(), "{p}");
    }

    /// The `$var` half: before PROV-100 cyrup resolved `thinking.budget` through the effort map
    /// and sent `"high"` where the template expects a token count.
    #[test]
    fn puts_the_clamped_budget_in_chat_template_kwargs_for_thinking_budget() {
        let template = || {
            Some(ModelCompat {
                thinking_format: Some(ThinkingFormat::ChatTemplate),
                chat_template_kwargs: Some(
                    json!({
                        "enable_thinking": { "$var": "thinking.enabled" },
                        "thinking_budget": { "$var": "thinking.budget" },
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
                ..Default::default()
            })
        };
        let p = capture(
            &vllm_model(template()),
            ModelThinkingLevel::High,
            None,
            None,
        );
        assert_eq!(
            p["chat_template_kwargs"],
            json!({ "enable_thinking": true, "thinking_budget": 16_384 - 1024 })
        );
        assert!(p.get("thinking_token_budget").is_none(), "{p}");

        // `"omits thinking.budget from chat_template_kwargs when thinking is off"`.
        let p = capture(&vllm_model(template()), ModelThinkingLevel::Off, None, None);
        assert_eq!(
            p["chat_template_kwargs"],
            json!({ "enable_thinking": false })
        );
    }

    /// The same resolver feeds Baseten's `chat_template_args` (`openai-completions.ts:893`).
    #[test]
    fn baseten_chat_template_args_resolve_thinking_budget_too() {
        let m = vllm_model(Some(ModelCompat {
            thinking_format: Some(ThinkingFormat::Baseten),
            chat_template_args: Some(
                json!({ "budget": { "$var": "thinking.budget" } })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            ..Default::default()
        }));
        let p = capture(&m, ModelThinkingLevel::Medium, medium(4096), None);
        assert_eq!(p["chat_template_args"], json!({ "budget": 4096 }), "{p}");
    }
}

/// PROV-100 — `test/openai-completions-vllm-priority.test.ts` @v0.87.1, translated:
/// `compat.vllmPriority` is sent as the top-level `priority` request field
/// (`openai-completions.ts:866-868`), and omitted when unset. Parsed from `models.json` form so the
/// camelCase key is pinned too.
#[test]
fn vllm_priority_is_the_top_level_priority_field() {
    let ctx = Context {
        system_prompt: Some("sys".to_string()),
        messages: vec![Message::User {
            content: vec![Content::text("hi")],
            timestamp: 0,
        }],
        tools: vec![],
    };
    let mut m = model();
    m.compat = Some(serde_json::from_value(json!({ "vllmPriority": 10 })).unwrap());
    assert_eq!(
        build_body(&m, &ctx, &StreamOptions::default())["priority"],
        10
    );
    assert!(
        build_body(&model(), &ctx, &StreamOptions::default())
            .get("priority")
            .is_none()
    );
}

// -------------------------------------------------------- DRIFT-009c: the qwen thinking format --

/// `qwen-token-plan`'s `deepseek-v4-flash`, verbatim from
/// `https://pi.dev/api/models/providers/qwen-token-plan` — the same live artifact
/// `xtask gen-catalogs` now writes the embedded floor from (DRIFT-009) and the runtime overlay
/// reads. All 49 rows across the three `qwen-token-plan*` providers carry
/// `compat.thinkingFormat: "qwen"`; 28 of them pair it with `supportsReasoningEffort: true` and a
/// `thinkingLevelMap`, and this is one of those 28.
///
/// Note the ladder: only `high` and `max` carry strings, every other rung is an explicit `null`.
/// That is what makes this row able to tell qwen's `??` apart from zai's and Baseten's
/// `=== undefined` ternary.
fn qwen_token_plan_deepseek_v4_flash_row() -> &'static str {
    r#"{
        "id": "deepseek-v4-flash",
        "name": "DeepSeek V4 Flash",
        "api": "openai-completions",
        "provider": "qwen-token-plan",
        "baseUrl": "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
        "compat": {
            "supportsStrictMode": true,
            "thinkingFormat": "qwen",
            "supportsDeveloperRole": false,
            "supportsStore": false,
            "supportsReasoningEffort": true
        },
        "thinkingLevelMap": {
            "off": null, "minimal": null, "low": null, "medium": null,
            "high": "high", "xhigh": null, "max": "max"
        },
        "reasoning": true,
        "input": ["text"],
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 1000000,
        "maxTokens": 384000
    }"#
}

/// DRIFT-009c — `api/openai-completions.ts:886-893` @v0.87.1. The qwen arm has TWO halves and cyrup
/// only ever emitted the first: `enable_thinking` went out, `reasoning_effort` did not.
///
/// This was a PORT OMISSION, not upstream drift — the arm reads identically at the ported baseline
/// `v0.83.0`, at `v0.84.4` and at `v0.87.1`. Both halves are asserted together because a passing
/// `enable_thinking` assertion is precisely what made the missing half invisible.
#[test]
fn qwen_sends_enable_thinking_and_a_mapped_reasoning_effort() {
    let model: Model =
        serde_json::from_str(qwen_token_plan_deepseek_v4_flash_row()).expect("catalog row parses");
    let ctx = Context {
        system_prompt: None,
        messages: vec![],
        tools: vec![],
    };

    let high = build_body(
        &model,
        &ctx,
        &StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        },
    );
    assert_eq!(high["enable_thinking"], json!(true), "{high}");
    assert_eq!(
        high["reasoning_effort"], "high",
        "map[high] = \"high\" (`:889`): {high}"
    );

    let maxed = build_body(
        &model,
        &ctx,
        &StreamOptions {
            reasoning: ModelThinkingLevel::Max,
            ..Default::default()
        },
    );
    assert_eq!(maxed["reasoning_effort"], "max", "{maxed}");

    // The `??` distinction. `map[low]` is an explicit `null`, so upstream's
    // `map?.[effort] ?? options.reasoningEffort` (`:889`) falls back to the REQUESTED level — where
    // zai's and Baseten's `mappedEffort === undefined ? … : mappedEffort` would keep the `null` and
    // let the `typeof effort === "string"` guard drop the key entirely.
    let low = build_body(
        &model,
        &ctx,
        &StreamOptions {
            reasoning: ModelThinkingLevel::Low,
            ..Default::default()
        },
    );
    assert_eq!(
        low["reasoning_effort"], "low",
        "a nulled rung falls back to the requested level under `??`, it does not suppress the key: \
         {low}"
    );
    assert_eq!(low["enable_thinking"], json!(true), "{low}");
}

/// DRIFT-009c — the two guards the qwen arm does have, and the one it does not.
///
/// `reasoning_effort` is gated on BOTH `options.reasoningEffort` and `compat.supportsReasoningEffort`
/// (`:888`). The first gate is the difference from the Baseten arm at `:897`, whose effort half is
/// ungated and therefore sends `thinkingLevelMap.off` with thinking off — copying Baseten's shape
/// here would send an effort qwen never asked for. `enable_thinking` itself is unconditional.
///
/// 21 of the 49 live `qwen-token-plan*` rows declare `supportsReasoningEffort: false` — including
/// `qwen3.7-max`, which `cyrup-config`'s `default_model_per_provider` names as the default for two of
/// the three providers — so the second case is the common one, not an edge.
#[test]
fn qwen_omits_reasoning_effort_with_thinking_off_and_without_effort_support() {
    let model: Model =
        serde_json::from_str(qwen_token_plan_deepseek_v4_flash_row()).expect("catalog row parses");
    let ctx = Context {
        system_prompt: None,
        messages: vec![],
        tools: vec![],
    };

    // Thinking OFF: `!!options?.reasoningEffort` is false and the effort half is skipped.
    let off = build_body(&model, &ctx, &StreamOptions::default());
    assert_eq!(
        off["enable_thinking"],
        json!(false),
        "`enable_thinking` is unconditional (`:887`): {off}"
    );
    assert!(
        off.get("reasoning_effort").is_none(),
        "unlike Baseten (`:897`), the qwen effort half IS gated on `options.reasoningEffort` \
         (`:888`) — nothing may be sent, least of all `thinkingLevelMap.off`: {off}"
    );

    // `supportsReasoningEffort: false` — the shape 21 of the 49 live rows ship.
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
    assert_eq!(
        body["enable_thinking"],
        json!(true),
        "the toggle survives without effort support: {body}"
    );
    assert!(body.get("reasoning_effort").is_none(), "{body}");
}

/// PROMPT-001 — the prompt a transcript replays to reaches the chat-completions body: the agent
/// builds its request from system rows alone, so the adapter must render what the context carries.
#[test]
fn the_transcripts_prompt_reaches_the_request_body() {
    use crate::api::prompt_fixture::{agent_context, assert_wire_carries_prompt};
    let body = build_body(
        &model(),
        &agent_context(Vec::new()),
        &StreamOptions::default(),
    );
    assert_wire_carries_prompt(&body.to_string());
    assert_eq!(body["messages"][0]["role"], "system", "{body}");
}

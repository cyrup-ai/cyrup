//! Request body.

use super::*;

#[test]
fn body_matches_upstream_shape() {
    let model = codex_model("gpt-5.5-codex");
    let ctx = Context::default();
    let so = StreamOptions {
        // Codex sends NO max_output_tokens even when the caller sets a cap.
        max_tokens: Some(4096),
        ..Default::default()
    };
    let body = build_request_body(&model, &ctx, &so, &opts(), None).unwrap();

    assert_eq!(body["model"], json!("gpt-5.5-codex"));
    assert_eq!(body["store"], json!(false));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["instructions"], json!("You are a helpful assistant."));
    assert_eq!(body["text"], json!({ "verbosity": "low" }));
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["tool_choice"], json!("auto"));
    assert_eq!(body["parallel_tool_calls"], json!(true));
    assert!(
        body.get("max_output_tokens").is_none(),
        "Codex never sends max_output_tokens (buildRequestBody :553-564)"
    );
    assert!(body.get("prompt_cache_key").is_none());
    assert!(body.get("temperature").is_none());
    assert!(body.get("service_tier").is_none());
    assert!(body.get("tools").is_none());
    // No requested effort ⇒ pi's `reasoningEffort` is undefined (`:514-516`) and the
    // `else if (model.reasoning && model.thinkingLevelMap?.off !== null)` arm
    // (openai-codex-responses.ts:596-597 @v0.87.1) writes `{ effort: "none" }` — `{ effort }`
    // alone, with no `summary`.
    assert_eq!(body["reasoning"], json!({ "effort": "none" }));
}

#[test]
fn system_prompt_rides_in_instructions_not_input() {
    // pi passes `includeSystemPrompt: false` (:545) and puts the prompt in `instructions`
    // (:557) — the opposite of `openai-responses`, which prepends a system/developer item.
    let model = codex_model("gpt-5.5-codex");
    let ctx = Context {
        system_prompt: Some("BE TERSE".to_string()),
        messages: vec![cyrup_core::Message::User {
            content: vec![cyrup_core::Content::text("hi")],
            timestamp: 0,
        }],
        tools: Vec::new(),
    };
    let body = build_request_body(&model, &ctx, &StreamOptions::default(), &opts(), None).unwrap();
    assert_eq!(body["instructions"], json!("BE TERSE"));
    let raw = serde_json::to_string(&body["input"]).unwrap();
    assert!(
        !raw.contains("BE TERSE"),
        "system prompt leaked into input: {raw}"
    );
    // MIRROR: the user turn IS in `input`, so the assertion above is not vacuously green on an
    // empty array.
    assert!(raw.contains("hi"), "user message missing from input: {raw}");
}

#[test]
fn empty_system_prompt_falls_back_to_the_default_instructions() {
    // `context.systemPrompt || "You are a helpful assistant."` — "" is falsy.
    let model = codex_model("gpt-5.5-codex");
    let ctx = Context {
        system_prompt: Some(String::new()),
        ..Default::default()
    };
    let body = build_request_body(&model, &ctx, &StreamOptions::default(), &opts(), None).unwrap();
    assert_eq!(body["instructions"], json!("You are a helpful assistant."));
}

#[test]
fn optional_fields_appear_only_when_set() {
    let model = codex_model("gpt-5.5-codex");
    let ctx = Context {
        tools: vec![crate::context::ToolDef {
            name: "bash".into(),
            description: "run".into(),
            parameters: json!({ "type": "object" }),
            constrained_sampling: None,
        }],
        ..Default::default()
    };
    let so = StreamOptions {
        temperature: Some(0.25),
        tool_choice: Some(ToolChoice::Required),
        ..Default::default()
    };
    let codex = OpenAiCodexResponsesOptions {
        service_tier: Some("priority".to_string()),
        text_verbosity: Some("high".to_string()),
        ..OpenAiCodexResponsesOptions::from_stream_options(&so)
    };
    let body = build_request_body(&model, &ctx, &so, &codex, Some("sess-9")).unwrap();

    // PERM-012: `temperature` is the ONE optional field that never appears on this api, set or
    // not — `openai-codex-responses` is in upstream's `TEMPERATURE_UNSUPPORTED_APIS`
    // (`model-option-compatibility.ts:20-22` @v0.8.0). This assertion was
    // `assert_eq!(body["temperature"], json!(0.25))` before the guard landed, i.e. it pinned
    // the unguarded behaviour; it is the RED half of the fix.
    assert!(
        body.get("temperature").is_none(),
        "codex-responses must never carry temperature, even when the caller sets one: {body}"
    );
    assert_eq!(body["service_tier"], json!("priority"));
    assert_eq!(body["text"], json!({ "verbosity": "high" }));
    assert_eq!(body["prompt_cache_key"], json!("sess-9"));
    assert_eq!(body["tool_choice"], json!("required"));
    assert_eq!(body["tools"][0]["name"], json!("bash"));
}

#[test]
fn named_function_tool_choice_falls_back_to_auto() {
    // `OpenAICodexResponsesOptions["toolChoice"]` is `"auto" | "none" | "required"` (:91) —
    // the named-function form has no Codex spelling.
    let so = StreamOptions {
        tool_choice: Some(ToolChoice::Function {
            name: "bash".into(),
        }),
        ..Default::default()
    };
    let codex = OpenAiCodexResponsesOptions::from_stream_options(&so);
    assert_eq!(codex.tool_choice, None);
    let body = build_request_body(
        &codex_model("gpt-5.5-codex"),
        &Context::default(),
        &so,
        &codex,
        None,
    )
    .unwrap();
    assert_eq!(body["tool_choice"], json!("auto"));
}

#[test]
fn session_id_is_dropped_when_cache_retention_is_none() {
    // pi `options?.cacheRetention === "none" ? undefined : options?.sessionId` (:281).
    let none = StreamOptions {
        session_id: Some(SessionId::from("sess-1")),
        cache_retention: Some(CacheRetention::None),
        ..Default::default()
    };
    assert_eq!(codex_session_id(&none), None);
    // MIRROR: any other retention keeps it (clamped).
    let short = StreamOptions {
        session_id: Some(SessionId::from("sess-1")),
        cache_retention: Some(CacheRetention::Short),
        ..Default::default()
    };
    assert_eq!(codex_session_id(&short).as_deref(), Some("sess-1"));
}

#[test]
fn reasoning_effort_maps_and_null_suppresses() {
    let mut model = codex_model("gpt-5.5-codex");
    model.thinking_level_map = Some(
        [
            ("high".to_string(), Some("xhigh".to_string())),
            ("medium".to_string(), None),
        ]
        .into_iter()
        .collect(),
    );
    // Mapped level: `model.thinkingLevelMap?.[level] ?? level` (:586).
    let so = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = build_request_body(&model, &Context::default(), &so, &opts(), None).unwrap();
    assert_eq!(
        body["reasoning"],
        json!({ "effort": "xhigh", "summary": "auto" })
    );

    // A level mapped to `null` is *unsupported*, so `clampThinkingLevel` (:516) moves the
    // request to the nearest supported rung before `buildRequestBody` ever sees it — which is
    // why the `if (effort !== null)` guard at :587 cannot fire from this path. `medium` → the
    // next supported rung, `high`, whose mapped effort is `xhigh`.
    let so = StreamOptions {
        reasoning: ModelThinkingLevel::Medium,
        ..Default::default()
    };
    let body = build_request_body(&model, &Context::default(), &so, &opts(), None).unwrap();
    assert_eq!(body["reasoning"]["effort"], json!("xhigh"));

    // `off` leaves `reasoningEffort` undefined (`:514-516`), which lands in the `else if
    // (model.reasoning && model.thinkingLevelMap?.off !== null)` arm at
    // openai-codex-responses.ts:596-597 @v0.87.1. This map has no `off` key, so
    // `model.thinkingLevelMap?.off ?? "none"` yields `"none"` — and that arm writes `{ effort }`
    // ALONE, with no `summary` (unlike the `if` branch at `:588-591`).
    let so = StreamOptions {
        reasoning: ModelThinkingLevel::Off,
        ..Default::default()
    };
    let body = build_request_body(&model, &Context::default(), &so, &opts(), None).unwrap();
    assert_eq!(body["reasoning"], json!({ "effort": "none" }));
    assert!(body["reasoning"].get("summary").is_none());
}

/// The `off` arm's two guards (openai-codex-responses.ts:596-597 @v0.87.1): a mapped `off` supplies
/// the effort, an `off: null` suppresses the object, and a non-reasoning model never reaches it.
#[test]
fn off_honours_a_mapped_off_and_a_null_off_suppresses() {
    let off = StreamOptions {
        reasoning: ModelThinkingLevel::Off,
        ..Default::default()
    };

    // `off: "minimal"` ⇒ `{ effort: "minimal" }`. `off` is a SUPPORTED rung here
    // (`Some(Some(_))` in `get_supported_thinking_levels`, collection.rs:823), so
    // `clamp_thinking_level` leaves the request on `off` and the arm is reached.
    let mut mapped = codex_model("gpt-5.5-codex");
    mapped.thinking_level_map = Some(
        [("off".to_string(), Some("minimal".to_string()))]
            .into_iter()
            .collect(),
    );
    let body = build_request_body(&mapped, &Context::default(), &off, &opts(), None).unwrap();
    assert_eq!(body["reasoning"], json!({ "effort": "minimal" }));

    // `off: null` ⇒ nothing at all. Reaching this guard needs care: `off: null` marks `off`
    // UNSUPPORTED (collection.rs:822), so `clamp_thinking_level` re-targets the request to the
    // nearest supported rung and `build_request_body` would take the `if` branch instead. Nulling
    // EVERY rung leaves no supported level, so the clamp falls back to `off`
    // (collection.rs:856-859) and the arm is reached with `off` present-and-null.
    let mut nulled = codex_model("gpt-5.5-codex");
    nulled.thinking_level_map = Some(
        ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
            .into_iter()
            .map(|k| (k.to_string(), None))
            .collect(),
    );
    let body = build_request_body(&nulled, &Context::default(), &off, &opts(), None).unwrap();
    assert!(body.get("reasoning").is_none());

    // `model.reasoning == false` gates the arm off entirely.
    let mut plain = codex_model("gpt-5.5-codex");
    plain.reasoning = false;
    let body = build_request_body(&plain, &Context::default(), &off, &opts(), None).unwrap();
    assert!(body.get("reasoning").is_none());
}

#[test]
fn reasoning_summary_option_overrides_auto() {
    let model = codex_model("gpt-5.5-codex");
    let codex = OpenAiCodexResponsesOptions {
        reasoning_summary: Some(CodexReasoningSummary::Detailed),
        ..Default::default()
    };
    let so = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = build_request_body(&model, &Context::default(), &so, &codex, None).unwrap();
    assert_eq!(body["reasoning"]["summary"], json!("detailed"));
}

/// PROMPT-001 — the prompt a transcript replays to reaches the Codex `instructions`.
#[test]
fn the_transcripts_prompt_reaches_the_request_body() {
    use crate::api::prompt_fixture::{agent_context, assert_wire_carries_prompt};
    let model = codex_model("gpt-5.5-codex");
    let body = build_request_body(
        &model,
        &agent_context(Vec::new()),
        &StreamOptions::default(),
        &opts(),
        None,
    )
    .unwrap();
    assert_wire_carries_prompt(&body["instructions"].to_string());
}

/// PROV-101: Codex sends a grammar tool as a `custom` tool with `strict` left out, only when the
/// model opts in (`compat.supportsOpenAIGrammarTools`), and replays the call and its result as
/// `custom_tool_call` / `custom_tool_call_output`.
#[test]
fn grammar_tools_are_custom_tools_on_an_opted_in_model_only() {
    use crate::api::compat::ModelCompat;
    use crate::context::{ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants};
    use cyrup_core::{Content, Message, ToolCall, ToolCallId, Usage};

    let tool = crate::context::ToolDef {
        name: "sample_tool".into(),
        description: "d".into(),
        parameters: json!({
            "type": "object",
            "properties": { "payload": { "type": "string" } },
            "required": ["payload"],
        }),
        constrained_sampling: Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::Grammar {
                variants: GrammarVariants {
                    openai_lark: Some("start: /[a-z]+/".to_string()),
                    openai_regex: None,
                },
            },
        )),
    };
    let mut args = serde_json::Map::new();
    args.insert("payload".to_string(), json!("abc"));
    let mut model = codex_model("gpt-5.1-codex");
    let ctx = Context {
        system_prompt: None,
        messages: vec![
            Message::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCall {
                    id: ToolCallId::from("call_1|ctc_1"),
                    name: "sample_tool".to_string(),
                    arguments: args.into(),
                    thought_signature: None,
                    namespace: None,
                })],
                provider: "openai-codex".into(),
                model: "gpt-5.1-codex".to_string(),
                api: API_ID.into(),
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
                timestamp: 1,
                duration_ms: None,
            }),
            Message::ToolResult {
                duration_ms: None,
                tool_call_id: ToolCallId::from("call_1|ctc_1"),
                tool_name: "sample_tool".to_string(),
                content: vec![Content::text("done")],
                is_error: false,
                details: None,
                usage: None,
                added_tool_names: Vec::new(),
                timestamp: 2,
                nested_calls: None,
            },
        ],
        tools: vec![tool],
    };

    let body = build_request_body(&model, &ctx, &StreamOptions::default(), &opts(), None).unwrap();
    assert_eq!(body["tools"][0]["type"], "function");
    let input = body["input"].as_array().unwrap();
    assert!(input.iter().any(|i| i["type"] == "function_call"));
    assert!(input.iter().all(|i| i["type"] != "custom_tool_call"));

    model.compat = Some(ModelCompat {
        supports_openai_grammar_tools: Some(true),
        ..Default::default()
    });
    let body = build_request_body(&model, &ctx, &StreamOptions::default(), &opts(), None).unwrap();
    assert_eq!(
        body["tools"][0],
        json!({
            "type": "custom",
            "name": "sample_tool",
            "description": "d",
            "format": { "type": "grammar", "syntax": "lark", "definition": "start: /[a-z]+/" },
        })
    );
    let input = body["input"].as_array().unwrap();
    assert!(input.contains(&json!({
        "type": "custom_tool_call",
        "id": "ctc_1",
        "call_id": "call_1",
        "name": "sample_tool",
        "input": "abc",
    })));
    assert!(input.contains(&json!({
        "type": "custom_tool_call_output",
        "call_id": "call_1",
        "output": "done",
    })));
}

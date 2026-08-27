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
    assert!(body.get("reasoning").is_none());
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
    ).unwrap();
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

    // `off` leaves `reasoningEffort` undefined (:516-517) — no reasoning key, and NO
    // `openai-responses`-style `{ effort: "none" }` off-branch.
    let so = StreamOptions {
        reasoning: ModelThinkingLevel::Off,
        ..Default::default()
    };
    let body = build_request_body(&model, &Context::default(), &so, &opts(), None).unwrap();
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


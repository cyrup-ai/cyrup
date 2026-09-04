//! Thinking payload, cache points and `inferenceConfig`.

use super::*;

#[test]
fn adaptive_models_send_adaptive_thinking_and_an_effort() {
    let opts = opts_with_reasoning(ModelThinkingLevel::High);
    for (id, name) in [
        (
            "global.anthropic.claude-opus-4-8-v1",
            "Claude Opus 4.8 (Global)",
        ),
        ("global.anthropic.claude-fable-5", "Claude Fable 5"),
        ("global.anthropic.claude-sonnet-5", "Claude Sonnet 5"),
        ("global.anthropic.claude-opus-5", "Claude Opus 5"),
    ] {
        let model = model_with(id, name);
        let body = payload(
            &model,
            &user_ctx("Hello"),
            &opts,
            &BedrockOptions::default(),
        );
        let fields = &body["additionalModelRequestFields"];
        assert_eq!(
            fields["thinking"],
            json!({ "type": "adaptive", "display": "summarized" }),
            "{id}"
        );
        assert_eq!(fields["output_config"], json!({ "effort": "high" }), "{id}");
        assert!(fields.get("anthropic_beta").is_none(), "{id}");
    }
}

#[test]
fn xhigh_reaches_the_native_effort_on_models_that_support_it() {
    let opts = opts_with_reasoning(ModelThinkingLevel::Xhigh);
    let model = opus_48();
    let body = payload(
        &model,
        &user_ctx("Hello"),
        &opts,
        &BedrockOptions::default(),
    );
    assert_eq!(
        body["additionalModelRequestFields"]["output_config"],
        json!({ "effort": "xhigh" })
    );

    // MIRROR: an adaptive model WITHOUT native xhigh support still clamps to "high" — proving
    // the branch keys off `supportsNativeXhighEffort`, not off the level alone.
    let sonnet_46 = model_with("global.anthropic.claude-sonnet-4-6", "Claude Sonnet 4.6");
    let body = payload(
        &sonnet_46,
        &user_ctx("Hello"),
        &opts,
        &BedrockOptions::default(),
    );
    assert_eq!(
        body["additionalModelRequestFields"]["output_config"],
        json!({ "effort": "high" })
    );
}

/// pi: "omits display for GovCloud model ids on non-adaptive Claude thinking".
#[test]
fn govcloud_omits_the_thinking_display_field() {
    let opts = opts_with_reasoning(ModelThinkingLevel::High);

    let model = model_with(
        "us-gov.anthropic.claude-sonnet-4-5-20250929-v1:0",
        "Claude Sonnet 4.5 (GovCloud)",
    );
    let body = payload(
        &model,
        &user_ctx("Hello"),
        &opts,
        &BedrockOptions::default(),
    );
    assert_eq!(
        body["additionalModelRequestFields"]["thinking"],
        json!({ "type": "enabled", "budget_tokens": 16384 })
    );
    assert_eq!(
        body["additionalModelRequestFields"]["anthropic_beta"],
        json!([INTERLEAVED_THINKING_BETA])
    );

    // A GovCloud REGION does the same to an adaptive model.
    let bedrock = BedrockOptions {
        region: Some("us-gov-west-1".to_string()),
        ..Default::default()
    };
    let body = payload(&opus_48(), &user_ctx("Hello"), &opts, &bedrock);
    assert_eq!(
        body["additionalModelRequestFields"]["thinking"],
        json!({ "type": "adaptive" })
    );

    // MIRROR: the same adaptive model outside GovCloud keeps `display`.
    let body = payload(
        &opus_48(),
        &user_ctx("Hello"),
        &opts,
        &BedrockOptions::default(),
    );
    assert_eq!(
        body["additionalModelRequestFields"]["thinking"],
        json!({ "type": "adaptive", "display": "summarized" })
    );
}

/// pi `:1079-1081` — the beta rides only the budget-based branch, and `interleavedThinking`
/// defaults to `true`.
#[test]
fn interleaved_thinking_defaults_on_and_can_be_suppressed() {
    let opts = opts_with_reasoning(ModelThinkingLevel::High);
    let model = sonnet_45();

    let body = payload(
        &model,
        &user_ctx("Hello"),
        &opts,
        &BedrockOptions::default(),
    );
    assert_eq!(
        body["additionalModelRequestFields"]["anthropic_beta"],
        json!([INTERLEAVED_THINKING_BETA])
    );

    let bedrock = BedrockOptions {
        interleaved_thinking: Some(false),
        ..Default::default()
    };
    let body = payload(&model, &user_ctx("Hello"), &opts, &bedrock);
    assert!(
        body["additionalModelRequestFields"]
            .get("anthropic_beta")
            .is_none()
    );
}

#[test]
fn thinking_display_omitted_reaches_the_wire() {
    let opts = opts_with_reasoning(ModelThinkingLevel::High);
    let bedrock = BedrockOptions {
        thinking_display: Some(BedrockThinkingDisplay::Omitted),
        ..Default::default()
    };
    let body = payload(&opus_48(), &user_ctx("Hello"), &opts, &bedrock);
    assert_eq!(
        body["additionalModelRequestFields"]["thinking"]["display"],
        json!("omitted")
    );
}

/// The typed-options plumbing: every field a caller can only reach through
/// `ApiStreamOptions::Bedrock` must survive `from_stream_options`.
#[test]
fn typed_options_are_reachable_through_api_options() {
    let typed = BedrockOptions {
        region: Some("ap-southeast-2".to_string()),
        profile: Some("p".to_string()),
        tool_choice: Some(BedrockToolChoice::Any),
        interleaved_thinking: Some(false),
        thinking_display: Some(BedrockThinkingDisplay::Omitted),
        request_metadata: Some(
            [("team".to_string(), "core".to_string())]
                .into_iter()
                .collect(),
        ),
        bearer_token: Some("bt".to_string()),
    };
    let opts = StreamOptions {
        api_options: Some(crate::stream::ApiStreamOptions::Bedrock(typed.clone())),
        ..Default::default()
    };
    let resolved = BedrockOptions::from_stream_options(&opts);
    assert_eq!(resolved, typed);

    // The unified tool choice wins over the typed one when both are set.
    let opts = StreamOptions {
        api_options: Some(crate::stream::ApiStreamOptions::Bedrock(typed)),
        tool_choice: Some(crate::stream::ToolChoice::Required),
        ..Default::default()
    };
    assert_eq!(
        BedrockOptions::from_stream_options(&opts).tool_choice,
        Some(BedrockToolChoice::Any)
    );
}

#[test]
fn request_metadata_reaches_the_payload() {
    let bedrock = BedrockOptions {
        request_metadata: Some(
            [("team".to_string(), "core".to_string())]
                .into_iter()
                .collect(),
        ),
        ..Default::default()
    };
    let body = payload(
        &sonnet_45(),
        &user_ctx("Hello"),
        &StreamOptions::default(),
        &bedrock,
    );
    assert_eq!(body["requestMetadata"], json!({ "team": "core" }));

    let body = payload(
        &sonnet_45(),
        &user_ctx("Hello"),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert!(body.get("requestMetadata").is_none());
}

#[test]
fn cache_points_land_on_the_system_prompt_and_the_last_user_message() {
    let ambient = ProviderEnv::new();
    let ctx = Context {
        system_prompt: Some("You are helpful.".to_string()),
        messages: vec![Message::User {
            content: vec![Content::text("Hello")],
            timestamp: 0,
        }],
        tools: Vec::new(),
    };
    let body = build_params(
        &sonnet_45(),
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
        CacheRetention::Short,
        &env_source(None, &ambient),
    )
    .unwrap();
    let system = body["system"].as_array().unwrap();
    assert_eq!(system.len(), 2);
    assert_eq!(system[1], json!({ "cachePoint": { "type": "default" } }));
    let content = messages_of(&body)[0]["content"].as_array().unwrap();
    assert_eq!(
        content.last().unwrap(),
        &json!({ "cachePoint": { "type": "default" } })
    );

    // Long retention adds the ttl.
    let body = build_params(
        &sonnet_45(),
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
        CacheRetention::Long,
        &env_source(None, &ambient),
    )
    .unwrap();
    assert_eq!(
        body["system"][1],
        json!({ "cachePoint": { "type": "default", "ttl": "1h" } })
    );

    // MIRROR: a model with no Claude reference gets no cache points at all, unless
    // AWS_BEDROCK_FORCE_CACHE=1 says otherwise.
    let nova = model_with("amazon.nova-pro-v1:0", "Nova Pro");
    let body = build_params(
        &nova,
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
        CacheRetention::Short,
        &env_source(None, &ambient),
    )
    .unwrap();
    assert_eq!(body["system"].as_array().unwrap().len(), 1);

    let forced = env_map(&[("AWS_BEDROCK_FORCE_CACHE", "1")]);
    let body = build_params(
        &nova,
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
        CacheRetention::Short,
        &env_source(None, &forced),
    )
    .unwrap();
    assert_eq!(body["system"].as_array().unwrap().len(), 2);
}

/// pi's "injects cache points when model.name identifies a supported Claude model" — the ARN
/// carries no model name, so the decision has to come from `model.name`.
#[test]
fn an_application_inference_profile_caches_via_the_model_name() {
    let ambient = ProviderEnv::new();
    let mut model = sonnet_45();
    model.id =
        "arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile".into();
    model.name = "Claude Sonnet 4.6".to_string();
    let ctx = Context {
        system_prompt: Some("You are helpful.".to_string()),
        messages: vec![Message::User {
            content: vec![Content::text("Hello")],
            timestamp: 0,
        }],
        tools: Vec::new(),
    };
    let body = build_params(
        &model,
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
        CacheRetention::Short,
        &env_source(None, &ambient),
    )
    .unwrap();
    assert_eq!(body["system"].as_array().unwrap().len(), 2);

    // The same ARN with a name that identifies no Claude model gets nothing.
    model.name = "My Profile".to_string();
    let body = build_params(
        &model,
        &ctx,
        &StreamOptions::default(),
        &BedrockOptions::default(),
        CacheRetention::Short,
        &env_source(None, &ambient),
    )
    .unwrap();
    assert_eq!(body["system"].as_array().unwrap().len(), 1);
}

#[test]
fn claude_defaults_max_tokens_to_the_model_cap_and_non_claude_omits_it() {
    let body = payload(
        &sonnet_45(),
        &user_ctx("hi"),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert_eq!(body["inferenceConfig"]["maxTokens"], json!(64_000));

    let nova = model_with("amazon.nova-pro-v1:0", "Nova Pro");
    let body = payload(
        &nova,
        &user_ctx("hi"),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert!(body["inferenceConfig"].get("maxTokens").is_none());

    // An explicit cap always wins.
    let opts = StreamOptions {
        max_tokens: Some(1234),
        temperature: Some(0.5),
        ..Default::default()
    };
    let body = payload(&nova, &user_ctx("hi"), &opts, &BedrockOptions::default());
    assert_eq!(body["inferenceConfig"]["maxTokens"], json!(1234));
    assert_eq!(body["inferenceConfig"]["temperature"], json!(0.5));
}

/// pi `streamSimple` (`:424-441`): a budget-based Claude model re-splits the cap, and the
/// resulting budget is `min(adjusted, maxTokens - 1024)`.
#[test]
fn budget_based_claude_resplits_max_tokens_between_thinking_and_output() {
    let mut model = sonnet_45();
    model.max_tokens = 8_000;
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        max_tokens: Some(2_000),
        ..Default::default()
    };
    let body = payload(&model, &user_ctx("hi"), &opts, &BedrockOptions::default());
    // adjust: min(2000 + 16384, 8000) = 8000 ⇒ budget 16384 > 8000 ⇒ budget = 8000-1024 = 6976.
    assert_eq!(body["inferenceConfig"]["maxTokens"], json!(8_000));
    assert_eq!(
        body["additionalModelRequestFields"]["thinking"]["budget_tokens"],
        json!(6_976)
    );

    // MIRROR: an ADAPTIVE model does not re-split — it keeps the caller's cap and emits no
    // budget at all.
    let mut adaptive = opus_48();
    adaptive.max_tokens = 8_000;
    let body = payload(
        &adaptive,
        &user_ctx("hi"),
        &opts,
        &BedrockOptions::default(),
    );
    assert_eq!(body["inferenceConfig"]["maxTokens"], json!(2_000));
    assert!(
        body["additionalModelRequestFields"]["thinking"]
            .get("budget_tokens")
            .is_none()
    );
}

#[test]
fn a_non_reasoning_model_sends_no_additional_fields() {
    let mut model = sonnet_45();
    model.reasoning = false;
    let opts = opts_with_reasoning(ModelThinkingLevel::High);
    let body = payload(&model, &user_ctx("hi"), &opts, &BedrockOptions::default());
    assert!(body.get("additionalModelRequestFields").is_none());

    // …and neither does a reasoning model with reasoning off.
    let body = payload(
        &sonnet_45(),
        &user_ctx("hi"),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert!(body.get("additionalModelRequestFields").is_none());
}

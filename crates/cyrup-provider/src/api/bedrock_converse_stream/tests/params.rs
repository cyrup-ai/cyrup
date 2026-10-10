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
            json!({
                "type": "adaptive",
                "display": "summarized",
                "block_binding": { "prefix_mismatch_behavior": "drop_block" },
            }),
            "{id}"
        );
        assert_eq!(fields["output_config"], json!({ "effort": "high" }), "{id}");
        assert_eq!(
            fields["anthropic_beta"],
            json!([THINKING_BINDING_CONTROLS_BETA]),
            "{id}"
        );
        assert!(
            !fields["anthropic_beta"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b == INTERLEAVED_THINKING_BETA),
            "the interleaved beta must stay off the adaptive branch: {id}"
        );
    }
}

/// PROV-130 — pi `supportsThinkingBlockBinding` (`bedrock-converse-stream.ts:792-806` @v1.0.1)
/// and `useBlockBinding` (`:1266`). The GATE matters as much as the field: Opus 4.6 and Sonnet 4.6
/// support adaptive thinking but reject `thinking.block_binding` outright, and GovCloud is skipped
/// like `display`.
#[test]
fn thinking_block_binding_rides_only_the_adaptive_models_that_accept_it() {
    let opts = opts_with_reasoning(ModelThinkingLevel::High);
    let binding = json!({ "prefix_mismatch_behavior": "drop_block" });

    // Accepts the field: the five needles upstream matches.
    for (id, name) in [
        ("global.anthropic.claude-opus-4-7-v1", "Claude Opus 4.7"),
        ("global.anthropic.claude-opus-4-8-v1", "Claude Opus 4.8"),
        ("global.anthropic.claude-opus-5", "Claude Opus 5"),
        ("global.anthropic.claude-sonnet-5", "Claude Sonnet 5"),
        ("global.anthropic.claude-fable-5", "Claude Fable 5"),
    ] {
        let fields = payload(
            &model_with(id, name),
            &user_ctx("Hello"),
            &opts,
            &BedrockOptions::default(),
        )["additionalModelRequestFields"]
            .clone();
        assert_eq!(fields["thinking"]["block_binding"], binding, "{id}");
        assert_eq!(
            fields["anthropic_beta"],
            json!([THINKING_BINDING_CONTROLS_BETA]),
            "{id}"
        );
    }

    // MIRROR: adaptive but REJECTS the field — Opus 4.6 and Sonnet 4.6 400 on it.
    for (id, name) in [
        ("global.anthropic.claude-opus-4-6-v1", "Claude Opus 4.6"),
        ("global.anthropic.claude-sonnet-4-6-v1", "Claude Sonnet 4.6"),
    ] {
        let fields = payload(
            &model_with(id, name),
            &user_ctx("Hello"),
            &opts,
            &BedrockOptions::default(),
        )["additionalModelRequestFields"]
            .clone();
        assert_eq!(
            fields["thinking"],
            json!({ "type": "adaptive", "display": "summarized" }),
            "{id} must carry no block_binding"
        );
        assert!(
            fields.get("anthropic_beta").is_none(),
            "{id} must carry no binding beta"
        );
    }

    // MIRROR: GovCloud skips the field even on a model that accepts it — by region...
    let gov_region = BedrockOptions {
        region: Some("us-gov-west-1".to_string()),
        ..Default::default()
    };
    let fields =
        payload(&opus_48(), &user_ctx("Hello"), &opts, &gov_region)["additionalModelRequestFields"]
            .clone();
    assert_eq!(fields["thinking"], json!({ "type": "adaptive" }));
    assert!(fields.get("anthropic_beta").is_none());

    // ...and by `us-gov.` model-id prefix.
    let fields = payload(
        &model_with("us-gov.anthropic.claude-opus-4-8-v1", "Claude Opus 4.8"),
        &user_ctx("Hello"),
        &opts,
        &BedrockOptions::default(),
    )["additionalModelRequestFields"]
        .clone();
    assert_eq!(fields["thinking"], json!({ "type": "adaptive" }));
    assert!(fields.get("anthropic_beta").is_none());
}

/// PROV-130 mirror: the budget-based branch never gets `block_binding`, even on a model whose id
/// matches the needle list — upstream adds it to the adaptive branch only.
#[test]
fn the_budget_based_branch_never_carries_block_binding() {
    let opts = opts_with_reasoning(ModelThinkingLevel::High);
    // Sonnet 4.5 is budget-based: not in `supports_adaptive_thinking`'s needle list.
    let fields = payload(
        &sonnet_45(),
        &user_ctx("Hello"),
        &opts,
        &BedrockOptions::default(),
    )["additionalModelRequestFields"]
        .clone();
    assert!(
        fields["thinking"].get("block_binding").is_none(),
        "budget branch must not bind: {}",
        fields["thinking"]
    );
    assert_eq!(
        fields["anthropic_beta"],
        json!([INTERLEAVED_THINKING_BETA]),
        "the budget branch keeps the interleaved beta alone"
    );
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

    // MIRROR: the same adaptive model outside GovCloud keeps `display` (and, per PROV-130,
    // `block_binding`, which GovCloud drops alongside it).
    let body = payload(
        &opus_48(),
        &user_ctx("Hello"),
        &opts,
        &BedrockOptions::default(),
    );
    assert_eq!(
        body["additionalModelRequestFields"]["thinking"],
        json!({
            "type": "adaptive",
            "display": "summarized",
            "block_binding": { "prefix_mismatch_behavior": "drop_block" },
        })
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

/// PROV-139 — pi `f76c1db66` (v1.1.0) added the `haiku-5` needle to all four Bedrock predicates
/// (`bedrock-converse-stream.ts:766-779`, `:781-792`, `:794-809`, `:876-900` @f1b2e77f5). The ids
/// are the six Haiku 5.5 rows pi.dev's `amazon-bedrock` catalog serves. Each model is built with
/// `model_with`, which carries no `thinkingLevelMap`, so the `xhigh` assertion can only be met by
/// `supportsNativeXhighEffort` and not by the catalog row's own map.
#[test]
fn haiku_55_on_bedrock_caches_thinks_adaptively_and_binds_blocks_while_haiku_45_keeps_budget() {
    let ambient = ProviderEnv::new();
    let ctx = Context {
        system_prompt: Some("You are helpful.".to_string()),
        messages: vec![Message::User {
            content: vec![Content::text("Hello")],
            timestamp: 0,
        }],
        tools: Vec::new(),
    };
    for (id, name) in [
        ("anthropic.claude-haiku-5-5", "Claude Haiku 5.5"),
        (
            "global.anthropic.claude-haiku-5-5",
            "Claude Haiku 5.5 (Global)",
        ),
        ("us.anthropic.claude-haiku-5-5", "Claude Haiku 5.5 (US)"),
        ("eu.anthropic.claude-haiku-5-5", "Claude Haiku 5.5 (EU)"),
        ("jp.anthropic.claude-haiku-5-5", "Claude Haiku 5.5 (JP)"),
        ("au.anthropic.claude-haiku-5-5", "Claude Haiku 5.5 (AU)"),
        // The id alone must decide: no name to fall back on.
        ("global.anthropic.claude-haiku-5-5", ""),
    ] {
        let model = model_with(id, name);
        assert!(model.thinking_level_map.is_none(), "{id}");

        // Prompt caching: a cache point after the system prompt.
        assert!(
            super::capabilities::supports_prompt_caching(&model, &env_source(None, &ambient)),
            "{id} must support prompt caching"
        );
        let body = build_params(
            &model,
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
            CacheRetention::Short,
            &env_source(None, &ambient),
        )
        .unwrap();
        assert_eq!(
            body["system"][1],
            json!({ "cachePoint": { "type": "default" } }),
            "{id} must emit a cachePoint"
        );

        // Adaptive thinking with block binding and the binding beta, not the interleaved one.
        let fields = payload(
            &model,
            &user_ctx("Hello"),
            &opts_with_reasoning(ModelThinkingLevel::High),
            &BedrockOptions::default(),
        )["additionalModelRequestFields"]
            .clone();
        assert_eq!(
            fields["thinking"],
            json!({
                "type": "adaptive",
                "display": "summarized",
                "block_binding": { "prefix_mismatch_behavior": "drop_block" },
            }),
            "{id}"
        );
        assert_eq!(fields["output_config"], json!({ "effort": "high" }), "{id}");
        assert_eq!(
            fields["anthropic_beta"],
            json!([THINKING_BINDING_CONTROLS_BETA]),
            "{id}"
        );

        // Native xhigh, both at the mapper and on the wire.
        assert_eq!(
            super::capabilities::map_thinking_level_to_effort(
                &model,
                cyrup_core::ThinkingLevel::Xhigh
            ),
            "xhigh",
            "{id}"
        );
        let fields = payload(
            &model,
            &user_ctx("Hello"),
            &opts_with_reasoning(ModelThinkingLevel::Xhigh),
            &BedrockOptions::default(),
        )["additionalModelRequestFields"]
            .clone();
        assert_eq!(
            fields["output_config"],
            json!({ "effort": "xhigh" }),
            "{id}"
        );
    }

    // MIRROR: Haiku 4.5 is not a `haiku-5` match (`haiku-4-5` does not contain it), so it keeps
    // its budget-based shape: `enabled` thinking with a budget, the interleaved beta alone, no
    // `block_binding`, `xhigh` clamped to `high`. It still caches, through the Claude 4.x `-4-`
    // arm. This half catches a needle widened past upstream's (`haiku`, `haiku-`).
    for (id, name) in [
        (
            "anthropic.claude-haiku-4-5-20251001-v1:0",
            "Claude Haiku 4.5",
        ),
        (
            "global.anthropic.claude-haiku-4-5-20251001-v1:0",
            "Claude Haiku 4.5 (Global)",
        ),
    ] {
        let model = model_with(id, name);
        assert!(
            super::capabilities::supports_prompt_caching(&model, &env_source(None, &ambient)),
            "{id}"
        );
        assert_eq!(
            super::capabilities::map_thinking_level_to_effort(
                &model,
                cyrup_core::ThinkingLevel::Xhigh
            ),
            "high",
            "{id}"
        );
        let fields = payload(
            &model,
            &user_ctx("Hello"),
            &opts_with_reasoning(ModelThinkingLevel::Xhigh),
            &BedrockOptions::default(),
        )["additionalModelRequestFields"]
            .clone();
        assert_eq!(fields["thinking"]["type"], json!("enabled"), "{id}");
        assert!(fields["thinking"]["budget_tokens"].is_u64(), "{id}");
        assert!(fields["thinking"].get("block_binding").is_none(), "{id}");
        assert!(fields.get("output_config").is_none(), "{id}");
        assert_eq!(
            fields["anthropic_beta"],
            json!([INTERLEAVED_THINKING_BETA]),
            "{id}"
        );
    }
}

// ------------------------------------------------------------------ PROV-140: OpenAI on Bedrock

/// A row from the embedded Bedrock catalog, so these tests drive the id, name, `reasoning` flag
/// and `thinkingLevelMap` the request actually resolves.
fn catalog_model(id: &str) -> Model {
    crate::providers::amazon_bedrock::amazon_bedrock_models()
        .into_iter()
        .find(|m| m.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} is in the embedded Bedrock catalog"))
}

fn openai_fields(model: &Model, level: ModelThinkingLevel) -> Option<Value> {
    payload(
        model,
        &user_ctx("Hello"),
        &opts_with_reasoning(level),
        &BedrockOptions::default(),
    )
    .get("additionalModelRequestFields")
    .cloned()
}

/// PROV-140 — pi `test/bedrock-thinking-payload.test.ts` "sends reasoning=%s as
/// reasoning.effort=%s for GPT-6 and GPT-5.6" (`2989eb581`): the nested form, with `minimal`
/// sent as `low`.
#[test]
fn gpt_models_send_a_nested_reasoning_effort() {
    for (level, effort) in [
        (ModelThinkingLevel::Minimal, "low"),
        (ModelThinkingLevel::Low, "low"),
        (ModelThinkingLevel::Medium, "medium"),
        (ModelThinkingLevel::High, "high"),
        (ModelThinkingLevel::Xhigh, "xhigh"),
        (ModelThinkingLevel::Max, "max"),
    ] {
        for id in [
            "global.openai.gpt-6-sol",
            "us.openai.gpt-6-luna",
            "global.openai.gpt-5.6-sol",
        ] {
            assert_eq!(
                openai_fields(&catalog_model(id), level),
                Some(json!({ "reasoning": { "effort": effort } })),
                "{id} at {level:?}"
            );
        }
    }
}

/// PROV-140 — pi "sends reasoning.effort when only model.name identifies a GPT model": an
/// application-inference-profile ARN matches through the model name.
#[test]
fn a_gpt_model_named_only_by_model_name_still_gets_the_effort() {
    let mut model = catalog_model("global.openai.gpt-6-sol");
    model.id =
        "arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile".into();
    model.name = "GPT-6 Sol".to_string();
    assert_eq!(
        openai_fields(&model, ModelThinkingLevel::Medium),
        Some(json!({ "reasoning": { "effort": "medium" } }))
    );
}

/// PROV-140 — pi "sends flat reasoning_effort for gpt-oss, clamped to high": the flat form,
/// clamped to low/medium/high.
#[test]
fn gpt_oss_sends_a_flat_reasoning_effort_clamped_to_high() {
    let model = catalog_model("openai.gpt-oss-120b-1:0");
    for (level, effort) in [
        (ModelThinkingLevel::Minimal, "low"),
        (ModelThinkingLevel::Medium, "medium"),
        (ModelThinkingLevel::Xhigh, "high"),
        (ModelThinkingLevel::Max, "high"),
    ] {
        assert_eq!(
            openai_fields(&model, level),
            Some(json!({ "reasoning_effort": effort })),
            "{level:?}"
        );
    }
}

/// PROV-140 — only the nested `gpt-` branch consults `thinkingLevelMap` (pi `:1326`); a string
/// entry wins over the table, and a `null` entry falls back to it.
#[test]
fn a_mapped_level_wins_for_gpt_but_gpt_oss_ignores_the_map() {
    let mut gpt = catalog_model("global.openai.gpt-6-sol");
    gpt.thinking_level_map = Some(
        [
            ("minimal".to_string(), Some("medium".to_string())),
            ("high".to_string(), None),
        ]
        .into_iter()
        .collect(),
    );
    assert_eq!(
        openai_fields(&gpt, ModelThinkingLevel::Minimal),
        Some(json!({ "reasoning": { "effort": "medium" } })),
        "a string entry beats OPENAI_GPT_EFFORT's `low`"
    );
    assert_eq!(
        openai_fields(&gpt, ModelThinkingLevel::High),
        Some(json!({ "reasoning": { "effort": "high" } })),
        "a null entry falls back to the table"
    );

    let mut oss = catalog_model("openai.gpt-oss-120b-1:0");
    oss.thinking_level_map = Some(
        [("xhigh".to_string(), Some("xhigh".to_string()))]
            .into_iter()
            .collect(),
    );
    assert_eq!(
        openai_fields(&oss, ModelThinkingLevel::Xhigh),
        Some(json!({ "reasoning_effort": "high" })),
        "gpt-oss uses its own table unconditionally"
    );
}

/// PROV-140 — the new arms sit after the Claude branch: at the same level a Claude model keeps
/// its thinking payload with no OpenAI field, a GPT model gets only the effort, and a model that
/// is neither still sends nothing.
#[test]
fn claude_is_unchanged_while_gpt_gets_its_effort() {
    let fields = openai_fields(&opus_48(), ModelThinkingLevel::Minimal).unwrap();
    assert!(fields.get("reasoning").is_none());
    assert!(fields.get("reasoning_effort").is_none());
    assert_eq!(fields["thinking"]["type"], json!("adaptive"));
    assert_eq!(fields["output_config"], json!({ "effort": "low" }));

    let fields = openai_fields(&sonnet_45(), ModelThinkingLevel::Minimal).unwrap();
    assert_eq!(
        fields["thinking"],
        json!({ "type": "enabled", "budget_tokens": 1024, "display": "summarized" })
    );
    assert!(fields.get("reasoning").is_none());

    assert_eq!(
        openai_fields(
            &catalog_model("global.openai.gpt-6-sol"),
            ModelThinkingLevel::Minimal
        ),
        Some(json!({ "reasoning": { "effort": "low" } }))
    );

    let nova = model_with("amazon.nova-pro-v1:0", "Nova Pro");
    assert_eq!(openai_fields(&nova, ModelThinkingLevel::High), None);
}

/// PROV-140 — pi "sends no reasoning fields when reasoning is off". The MIRROR at `High` proves
/// the same model does carry the field once reasoning is on.
#[test]
fn reasoning_off_sends_no_openai_reasoning_field() {
    for id in ["global.openai.gpt-6-sol", "openai.gpt-oss-120b-1:0"] {
        let model = catalog_model(id);
        assert_eq!(openai_fields(&model, ModelThinkingLevel::Off), None, "{id}");
        assert!(
            openai_fields(&model, ModelThinkingLevel::High).is_some(),
            "{id}"
        );
    }
}

/// PROMPT-001 — the prompt a transcript replays to reaches the Converse `system` blocks.
#[test]
fn the_transcripts_prompt_reaches_the_request_body() {
    use crate::api::prompt_fixture::{agent_context, assert_wire_carries_prompt};
    let model = model_with("global.anthropic.claude-sonnet-5", "Claude Sonnet 5");
    let body = payload(
        &model,
        &agent_context(Vec::new()),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert_wire_carries_prompt(&body["system"].to_string());
}

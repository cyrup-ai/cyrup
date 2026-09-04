//! Request body: url, reasoning, service tier, cache retention and clamping.

use super::*;

#[test]
fn url_appends_responses() {
    let m = model();
    assert_eq!(
        resolve_url(&m, &auth()).as_deref(),
        Some("https://api.openai.com/v1/responses")
    );
}

#[test]
fn build_params_basic_shape() {
    let m = model();
    let opts = StreamOptions {
        max_tokens: Some(100),
        ..Default::default()
    };
    let body = build_params(&m, &user_ctx("hi"), &opts, None);
    assert_eq!(body["model"], "gpt-5");
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert_eq!(body["max_output_tokens"], 100);
    // input is the Responses message array with one user input_text.
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(body["input"][0]["content"][0]["text"], "hi");
}

/// PERM-012 — pi's `getUnsupportedTemperatureReason` strips `temperature` for a REASONING
/// model on this api (`pi-permission-system/src/model-option-compatibility.ts:78-80`
/// @v0.8.0). Before the guard landed this body carried `"temperature": 0.7`, so this test is
/// red at HEAD~: `model()` is `reasoning: true`.
#[test]
fn a_reasoning_model_never_carries_temperature() {
    let m = model();
    assert!(
        m.reasoning,
        "fixture must be a reasoning model for this to bite"
    );
    let opts = StreamOptions {
        temperature: Some(0.7),
        ..Default::default()
    };
    let body = build_params(&m, &user_ctx("hi"), &opts, None);
    assert!(
        body.get("temperature").is_none(),
        "a reasoning model accepts only the provider default temperature: {body}"
    );
}

/// The control for the test above: the SAME api and the same request, with the one field the
/// rule reads flipped. Without this, the assertion above would also pass if the builder had
/// simply stopped emitting `temperature` at all.
#[test]
fn a_non_reasoning_model_still_carries_temperature() {
    let m = Model {
        reasoning: false,
        ..model()
    };
    let opts = StreamOptions {
        temperature: Some(0.7),
        ..Default::default()
    };
    let body = build_params(&m, &user_ctx("hi"), &opts, None);
    // `StreamOptions::temperature` is `Option<f32>` (`stream.rs:165`), so the serialized number
    // is the f32 widened to f64 — `0.699999988079071`, not the f64 literal `0.7`. Compare
    // against the same widening rather than the literal; asserting `json!(0.7)` here fails on a
    // value that is byte-for-byte what the wire carries.
    assert_eq!(
        body.get("temperature").and_then(Value::as_f64),
        Some(f64::from(0.7_f32))
    );
}

/// The id-token arm (`model-option-compatibility.ts:74-76` @v0.8.0): a `codex` TOKEN in the
/// model id strips temperature even off a non-reasoning model on this api.
#[test]
fn a_codex_token_in_the_model_id_strips_temperature() {
    let m = Model {
        id: "gpt-5.5-codex".into(),
        reasoning: false,
        ..model()
    };
    let opts = StreamOptions {
        temperature: Some(0.7),
        ..Default::default()
    };
    let body = build_params(&m, &user_ctx("hi"), &opts, None);
    assert!(
        body.get("temperature").is_none(),
        "`gpt-5.5-codex` splits to a `codex` token: {body}"
    );
}

#[test]
fn reasoning_effort_encodes_with_encrypted_content() {
    let m = model();
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = build_params(&m, &user_ctx("hi"), &opts, None);
    assert_eq!(body["reasoning"]["effort"], "high");
    assert_eq!(body["reasoning"]["summary"], "auto");
    assert_eq!(body["include"][0], "reasoning.encrypted_content");
}

#[test]
fn reasoning_summary_per_api_option_overrides_auto() {
    // Pi `summary: options?.reasoningSummary || "auto"` (openai-responses.ts:257).
    let m = model();
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        api_options: Some(crate::stream::ApiStreamOptions::OpenAiResponses(
            OpenAiResponsesOptions {
                reasoning_summary: Some(ReasoningSummary::Detailed),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    let body = build_params(&m, &user_ctx("hi"), &opts, None);
    assert_eq!(body["reasoning"]["summary"], "detailed");

    // Explicit `null` falls back to "auto" (Pi `|| "auto"`).
    let opts_null = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        api_options: Some(crate::stream::ApiStreamOptions::OpenAiResponses(
            OpenAiResponsesOptions {
                reasoning_summary: Some(ReasoningSummary::Null),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    let body = build_params(&m, &user_ctx("hi"), &opts_null, None);
    assert_eq!(body["reasoning"]["summary"], "auto");
}

#[test]
fn service_tier_per_api_option_threads_to_request() {
    // Pi sets `params.service_tier` only when `serviceTier` is defined (openai-responses.ts:242).
    let m = model();
    // Omitted by default.
    let body = build_params(&m, &user_ctx("hi"), &StreamOptions::default(), None);
    assert!(body.get("service_tier").is_none());

    let opts = StreamOptions {
        api_options: Some(crate::stream::ApiStreamOptions::OpenAiResponses(
            OpenAiResponsesOptions {
                service_tier: Some("priority".to_string()),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    let body = build_params(&m, &user_ctx("hi"), &opts, None);
    assert_eq!(body["service_tier"], "priority");
}

#[test]
fn reasoning_off_uses_off_map_value_when_supported() {
    let m = model();
    // With no thinkingLevelMap, `off` is a supported level, so a default (Off) request clamps to
    // `off` and takes the no-summary branch: reasoning.effort = off-map value ?? "none".
    let opts = StreamOptions::default(); // reasoning defaults to Off
    let body = build_params(&m, &user_ctx("hi"), &opts, None);
    assert_eq!(body["reasoning"]["effort"], "none");
    assert!(body["reasoning"].get("summary").is_none());
    // No `include` on the no-summary branch (only the effort-bearing branch sets it).
    assert!(body.get("include").is_none());
}

#[test]
fn reasoning_off_null_clamps_up_to_lowest_supported() {
    let mut m = model();
    // thinkingLevelMap.off = null marks `off` unsupported, so a default (Off) request clamps UP
    // to the lowest supported level (minimal) and takes the effort branch — matching Pi's
    // streamSimple clamp + buildParams (the `off !== null` guard is never reached here).
    let mut map = crate::model::ThinkingLevelMap::new();
    map.insert("off".to_string(), None);
    m.thinking_level_map = Some(map);
    let body = build_params(&m, &user_ctx("hi"), &StreamOptions::default(), None);
    assert_eq!(body["reasoning"]["effort"], "minimal");
    assert_eq!(body["reasoning"]["summary"], "auto");
    assert_eq!(body["include"][0], "reasoning.encrypted_content");
}

#[test]
fn long_cache_retention_emits_24h() {
    let m = model();
    let opts = StreamOptions {
        cache_retention: Some(CacheRetention::Long),
        session_id: Some("sess-1".into()),
        ..Default::default()
    };
    let body = build_params(&m, &user_ctx("hi"), &opts, None);
    assert_eq!(body["prompt_cache_retention"], "24h");
    assert_eq!(body["prompt_cache_key"], "sess-1");
}

/// PROV-019. `Math.max(options.maxTokens, OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)` with pi's JS
/// truthiness gate (openai-responses.ts:289-290 @v0.83.0). Red before the fix on the `Some(4)`
/// and `Some(0)` rows: cyrup inserted the raw value, so the endpoint answered HTTP 400.
#[test]
fn max_output_tokens_is_clamped_to_sixteen() {
    let m = model();
    let ctx = user_ctx("hi");
    let body = |max: Option<u64>| {
        build_params(
            &m,
            &ctx,
            &StreamOptions {
                max_tokens: max,
                ..Default::default()
            },
            None,
        )
    };
    assert_eq!(body(Some(100))["max_output_tokens"], 100);
    assert_eq!(body(Some(4))["max_output_tokens"], 16);
    assert!(body(Some(0)).get("max_output_tokens").is_none());
    assert!(body(None).get("max_output_tokens").is_none());
}

/// PROV-023. `prompt_cache_options: disableImplicitPromptCache ? {mode:"explicit"} : undefined`
/// (openai-responses.ts:285 @v0.83.0), where `disableImplicitPromptCache = cacheRetention ===
/// "none" && compat.supportsExplicitPromptCacheMode` (`:278`). The flag MUST stay default-false:
/// older OpenAI models reject the parameter.
#[test]
fn explicit_prompt_cache_mode_only_on_opt_in_and_none_retention() {
    let mut m = model();
    let ctx = user_ctx("hi");
    let opts = |r: CacheRetention| StreamOptions {
        cache_retention: Some(r),
        session_id: Some("s1".into()),
        ..Default::default()
    };
    // Flag absent ⇒ never, regardless of retention (the older-model regression guard).
    for r in [
        CacheRetention::None,
        CacheRetention::Short,
        CacheRetention::Long,
    ] {
        assert!(
            build_params(&m, &ctx, &opts(r), None)
                .get("prompt_cache_options")
                .is_none()
        );
    }
    m.compat = Some(ModelCompat {
        supports_explicit_prompt_cache_mode: Some(true),
        ..Default::default()
    });
    let body = build_params(&m, &ctx, &opts(CacheRetention::None), None);
    assert_eq!(body["prompt_cache_options"], json!({"mode": "explicit"}));
    assert!(
        body.get("prompt_cache_key").is_none(),
        "retention none never writes a cache key"
    );
    for r in [CacheRetention::Short, CacheRetention::Long] {
        assert!(
            build_params(&m, &ctx, &opts(r), None)
                .get("prompt_cache_options")
                .is_none()
        );
    }
}

/// PROV-045. Pi's first reasoning arm fires on `reasoningEffort || reasoningSummary`
/// (openai-responses.ts:313 @v0.83.0) and the xAI `include` sits OUTSIDE the if/else (`:327`).
#[test]
fn reasoning_summary_alone_and_the_xai_include() {
    let m = model();
    let ctx = user_ctx("hi");
    // (a) summary only, reasoning level off ⇒ effort "medium" + summary + include.
    let opts = StreamOptions {
        api_options: Some(ApiStreamOptions::OpenAiResponses(OpenAiResponsesOptions {
            reasoning_summary: Some(ReasoningSummary::Detailed),
            ..Default::default()
        })),
        ..Default::default()
    };
    let body = build_params(&m, &ctx, &opts, None);
    assert_eq!(body["reasoning"]["effort"], "medium");
    assert_eq!(body["reasoning"]["summary"], "detailed");
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));

    // (b) an xai-provider reasoning model with reasoning OFF still emits `include`.
    let mut xai = model();
    xai.provider = "xai".into();
    let body = build_params(&xai, &ctx, &StreamOptions::default(), None);
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));

    // (c) a non-xai model with reasoning off emits neither.
    let body = build_params(&m, &ctx, &StreamOptions::default(), None);
    assert!(body.get("include").is_none());
}

//! Request body: thinking, sampling and `max_tokens`.

use super::*;

#[test]
fn build_params_basic_shape() {
    let m = model();
    let opts = StreamOptions {
        max_tokens: Some(1000),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("hello"), &opts);
    assert_eq!(body["model"], "claude-opus-4-5");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 1000);
    // system prompt is an array of text blocks.
    assert_eq!(body["system"][0]["type"], "text");
    assert_eq!(body["system"][0]["text"], "be brief");
    // user message text-only; default (short) retention applies cache_control to the last user
    // message, so the string is promoted to a single cached text block (Pi convertMessages).
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
    // A reasoning model with reasoning Off (and `off` not null-marked) emits thinking:disabled
    // (Pi buildParams: `thinkingEnabled === false && thinkingLevelMap?.off !== null`).
    assert_eq!(body["thinking"]["type"], "disabled");
}

#[test]
fn budget_thinking_encodes_enabled_with_budget() {
    let m = model(); // not adaptive
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        max_tokens: Some(4000),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("think"), &opts);
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["display"], "summarized");
    assert!(body["thinking"]["budget_tokens"].as_u64().unwrap() > 0);
    // temperature omitted while thinking is enabled.
    assert!(body.get("temperature").is_none());
}

#[test]
fn thinking_display_per_api_option_overrides_default() {
    // Pi `options.thinkingDisplay ?? "summarized"` (anthropic-messages.ts:962). The typed
    // per-API option flips the emitted `thinking.display` for both budget and adaptive thinking.
    let m = model(); // not adaptive
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        max_tokens: Some(4000),
        api_options: Some(crate::stream::ApiStreamOptions::Anthropic(
            AnthropicOptions {
                thinking_display: Some(AnthropicThinkingDisplay::Omitted),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("think"), &opts);
    assert_eq!(body["thinking"]["display"], "omitted");

    // Adaptive model carries the same display through to the adaptive thinking block.
    let mut adaptive = model();
    adaptive.compat = Some(ModelCompat {
        force_adaptive_thinking: Some(true),
        ..Default::default()
    });
    let body = build_body(&adaptive, &user_ctx("deep"), &opts);
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["thinking"]["display"], "omitted");
}

#[test]
fn custom_thinking_budgets_override_default_budget_tokens() {
    // Pi `streamSimple` forwards `options.thinkingBudgets` into `adjustMaxTokensForThinking`
    // (anthropic-messages.ts:792-797). A custom `high` budget must override the built-in default
    // (16_384) in the emitted `thinking.budget_tokens`.
    let m = model(); // not adaptive, max_tokens 64_000, window 200_000
    let custom = crate::utils::simple_options::ThinkingBudgets {
        high: Some(30_000),
        ..Default::default()
    };
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        thinking_budgets: Some(custom),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("think"), &opts);
    assert_eq!(body["thinking"]["budget_tokens"].as_u64().unwrap(), 30_000);
    // Sanity: without the override the default (16_384) is used, proving the field threads.
    let default_opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let default_body = build_body(&m, &user_ctx("think"), &default_opts);
    assert_eq!(
        default_body["thinking"]["budget_tokens"].as_u64().unwrap(),
        16_384
    );
}

#[test]
fn adaptive_thinking_encodes_effort() {
    let mut m = model();
    m.compat = Some(ModelCompat {
        force_adaptive_thinking: Some(true),
        ..Default::default()
    });
    m.thinking_level_map = Some(
        [("xhigh".to_string(), Some("xhigh".to_string()))]
            .into_iter()
            .collect(),
    );
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::Xhigh,
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("deep"), &opts);
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "xhigh");
}

/// PROV-002: the `max` rung must reach `output_config.effort` as `"max"` (Pi
/// `mapThinkingLevelToEffort`, anthropic-messages.ts:781-799 — the map lookup wins).
#[test]
fn adaptive_thinking_encodes_max_effort() {
    let mut m = model();
    m.compat = Some(ModelCompat {
        force_adaptive_thinking: Some(true),
        ..Default::default()
    });
    m.thinking_level_map = Some(
        [("max".to_string(), Some("max".to_string()))]
            .into_iter()
            .collect(),
    );
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::Max,
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("deepest"), &opts);
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "max");
}

/// With no `thinkingLevelMap` entry, `max` falls to Pi's `default: "high"` arm — it must NOT
/// leak a bare `"max"` to a model that never advertised the effort.
#[test]
fn unmapped_max_falls_back_to_high_effort() {
    let mut m = model();
    m.compat = Some(ModelCompat {
        force_adaptive_thinking: Some(true),
        ..Default::default()
    });
    m.thinking_level_map = None;
    let body = build_body(
        &m,
        &user_ctx("x"),
        &StreamOptions {
            reasoning: ModelThinkingLevel::Max,
            ..Default::default()
        },
    );
    assert_eq!(body["output_config"]["effort"], "high");
}

/// The real-catalog end of PROV-002: on `claude-opus-4-6` the level the UI DISPLAYS and the
/// effort that goes on the wire must be the same string. Before the fix the catalog carried
/// `{"xhigh":"max"}`, so the footer said `xhigh` while Anthropic received `max`.
#[test]
fn max_label_matches_the_wire_effort_on_opus_4_6() {
    use crate::collection::{clamp_thinking_level, get_supported_thinking_levels};
    let m = crate::providers::anthropic_models()
        .iter()
        .find(|m| m.id.as_str() == "claude-opus-4-6")
        .expect("opus-4-6")
        .clone();

    // The only top rung this model offers is `max`; a request for `xhigh` promotes onto it.
    let levels = get_supported_thinking_levels(&m);
    assert!(levels.contains(&ModelThinkingLevel::Max), "{levels:?}");
    assert!(!levels.contains(&ModelThinkingLevel::Xhigh), "{levels:?}");
    let selected = clamp_thinking_level(&m, ModelThinkingLevel::Xhigh);
    assert_eq!(selected, ModelThinkingLevel::Max);

    let body = build_body(
        &m,
        &user_ctx("x"),
        &StreamOptions {
            reasoning: selected,
            ..Default::default()
        },
    );
    let wire = body["output_config"]["effort"].as_str().expect("effort");
    assert_eq!(wire, "max");
    assert_eq!(
        crate::api::compat::thinking_level_key(selected),
        wire,
        "displayed level and wire effort must agree"
    );
}

/// `claude-sonnet-5` advertises BOTH top rungs and they must stay distinct on the wire.
#[test]
fn sonnet_5_sends_xhigh_and_max_distinctly() {
    let m = crate::providers::anthropic_models()
        .iter()
        .find(|m| m.id.as_str() == "claude-sonnet-5")
        .expect("sonnet-5")
        .clone();
    let effort = |level| {
        build_body(
            &m,
            &user_ctx("x"),
            &StreamOptions {
                reasoning: level,
                ..Default::default()
            },
        )["output_config"]["effort"]
            .as_str()
            .map(str::to_string)
    };
    assert_eq!(effort(ModelThinkingLevel::Xhigh).as_deref(), Some("xhigh"));
    assert_eq!(effort(ModelThinkingLevel::Max).as_deref(), Some("max"));
}

#[test]
fn disabled_thinking_when_off_map_not_null() {
    let mut m = model();
    // off not present => off !== null => disabled emitted.
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::Off,
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert_eq!(body["thinking"]["type"], "disabled");
    // when off is null-marked, no thinking key.
    m.thinking_level_map = Some([("off".to_string(), None)].into_iter().collect());
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert!(body.get("thinking").is_none());
}

#[test]
fn temperature_only_without_thinking_and_when_supported() {
    let mut m = model();
    m.reasoning = false;
    let opts = StreamOptions {
        temperature: Some(0.7),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 1e-6);
    // supportsTemperature=false suppresses it (Opus 4.7+).
    m.compat = Some(ModelCompat {
        supports_temperature: Some(false),
        ..Default::default()
    });
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert!(body.get("temperature").is_none());
}

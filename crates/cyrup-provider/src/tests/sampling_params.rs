//! AGENT-026 — `samplingParams`: a 1:1 port of pi's own `packages/ai/test/sampling-options.test.ts`
//! @v0.84.1, case for case, plus the two adapters that file does not exercise.
//!
//! Upstream shape: `Model.samplingParams` (`types.ts:801-802`) and
//! `StreamOptions.samplingParams` (`types.ts:183-189`) are merged per key by `buildBaseOptions`
//! (`{ ...model.samplingParams, ...options?.samplingParams }`, `simple-options.ts:27-33`), and the
//! three OpenAI-compatible adapters `Object.assign` the result onto the request body **last**, so
//! custom keys override the named request fields (`openai-completions.ts:884-887`,
//! `openai-responses.ts:330-333`, `azure-openai-responses.ts:324-327`). Every other api ignores it.
//!
//! CFG-104 (pi @f1b2e77f5) adds `Model.samplingParamsByThinkingLevel` between the two layers and
//! moves the merge into `resolveSamplingParams`, which the adapters call too — see the `cfg104_`
//! cases at the bottom.
//!
//! pi captures the body with `onPayload` and throws out of the callback; cyrup's `build_body` /
//! `build_params` are the same function pi's payload comes from, called directly — no live socket,
//! same assertion.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::{Map, Value, json};

use crate::context::Context;
use crate::model::{Modality, Model, ModelCost};
use crate::stream::StreamOptions;
use crate::utils::simple_options::{SimpleStreamOptions, build_base_options};

fn sampling(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

/// pi `makeContext()` — one user message.
fn make_context() -> Context {
    Context {
        messages: vec![cyrup_core::Message::User {
            content: vec![cyrup_core::Content::text("Hello")],
            timestamp: 0,
        }],
        ..Default::default()
    }
}

/// pi `makeCompletionsModel()` — a custom OpenAI-compatible endpoint.
fn completions_model(sampling_params: Option<Map<String, Value>>) -> Model {
    Model {
        id: "custom-model".into(),
        name: "Custom Model".to_string(),
        api: "openai-completions".into(),
        provider: "custom-provider".into(),
        base_url: "http://127.0.0.1:9/v1".to_string(),
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        input_limits: None,
        prompt_cache: None,
        context_window: 128_000,
        max_tokens: 16_384,
        sampling_params: sampling_params.map(crate::model::ModelSamplingParams::flat),
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

/// pi `makeAnthropicModel()` — the negative case: a non-OpenAI-compatible api.
fn anthropic_model() -> Model {
    Model {
        id: "vendor--claude".into(),
        name: "Vendor Proxy Claude".to_string(),
        api: "anthropic-messages".into(),
        provider: "vendor-proxy".into(),
        base_url: "http://127.0.0.1:9".to_string(),
        reasoning: true,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        input_limits: None,
        prompt_cache: None,
        context_window: 200_000,
        max_tokens: 32_000,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

/// pi `capturePayload(model, options)` — lower the simple options exactly as `streamSimple` does,
/// then build the body the request would have carried. "Exactly" includes the clamp: the default
/// `Provider::stream_simple` sets `lowered.reasoning = clamp_thinking_level(model, level)` after
/// `build_base_options`, so the adapter's own `resolve_sampling_params` pass runs at the clamped
/// level here too, not at `Off`.
fn capture_completions_payload(model: &Model, options: SimpleStreamOptions) -> Value {
    let ctx = make_context();
    let mut lowered = build_base_options(model, &ctx, &options, Some("fake-key"));
    if let Some(level) = options.reasoning {
        lowered.reasoning = crate::collection::clamp_thinking_level(model, level.into());
    }
    crate::api::openai_completions::build_body(model, &ctx, &lowered)
}

fn simple_with(
    sampling_params: Option<Map<String, Value>>,
    temperature: Option<f32>,
) -> SimpleStreamOptions {
    SimpleStreamOptions {
        base: StreamOptions {
            sampling_params,
            temperature,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// pi: "merges stream-option sampling params into the request body".
///
/// `top_k: 0` / `min_p: 0` are in pi's fixture on purpose — a zero must survive, so the port cannot
/// be written with a truthiness filter on the VALUES (only the map itself is `if`-gated upstream).
#[test]
fn agent026_merges_stream_option_sampling_params_into_the_request_body() {
    let model = completions_model(None);
    let body = capture_completions_payload(
        &model,
        simple_with(
            Some(sampling(&[
                ("top_p", json!(0.95)),
                ("top_k", json!(0)),
                ("min_p", json!(0)),
            ])),
            None,
        ),
    );
    assert_eq!(body.get("top_p"), Some(&json!(0.95)));
    assert_eq!(
        body.get("top_k"),
        Some(&json!(0)),
        "a zero-valued key must still be sent"
    );
    assert_eq!(body.get("min_p"), Some(&json!(0)));
}

/// pi: "omits sampling params when neither options nor model set them".
#[test]
fn agent026_omits_sampling_params_when_neither_side_sets_them() {
    let model = completions_model(None);
    let body = capture_completions_payload(&model, SimpleStreamOptions::default());
    assert_eq!(body.get("temperature"), None);
    assert_eq!(body.get("top_p"), None);
}

/// pi: "applies model-level sampling params" — the half that needs `Model.sampling_params` to exist
/// at all, and the reason the merge lives in `build_base_options` rather than in each adapter.
#[test]
fn agent026_applies_model_level_sampling_params() {
    let model = completions_model(Some(sampling(&[
        ("temperature", json!(1)),
        ("top_p", json!(0.95)),
    ])));
    let body = capture_completions_payload(&model, SimpleStreamOptions::default());
    assert_eq!(body.get("temperature"), Some(&json!(1)));
    assert_eq!(body.get("top_p"), Some(&json!(0.95)));
}

/// pi: "merges stream-option keys over model-level keys" — PER KEY. A whole-map replacement passes
/// the `top_p` assertion and fails `min_p`, which is exactly why pi asserts both.
#[test]
fn agent026_stream_option_keys_override_model_level_keys_per_key() {
    let model = completions_model(Some(sampling(&[
        ("top_p", json!(0.95)),
        ("min_p", json!(0.05)),
    ])));
    let body = capture_completions_payload(
        &model,
        simple_with(Some(sampling(&[("top_p", json!(0.5))])), None),
    );
    assert_eq!(
        body.get("top_p"),
        Some(&json!(0.5)),
        "the per-request key wins"
    );
    assert_eq!(
        body.get("min_p"),
        Some(&json!(0.05)),
        "a model-level key the request does not mention must survive the merge"
    );
}

/// pi: "overrides named request fields" — the assign is LAST, after `temperature` is written from
/// the named option. An adapter that applied it earlier would report `0` here.
#[test]
fn agent026_sampling_params_override_the_named_request_fields() {
    let model = completions_model(None);
    let body = capture_completions_payload(
        &model,
        simple_with(Some(sampling(&[("temperature", json!(1))])), Some(0.0)),
    );
    assert_eq!(body.get("temperature"), Some(&json!(1)));
}

/// pi: "is ignored by non-OpenAI-compatible APIs". Assert PRESENCE first — the same options DO
/// reach an OpenAI-compatible body — so the absence below cannot be satisfied by a merge that
/// silently does nothing anywhere.
#[test]
fn agent026_sampling_params_are_ignored_by_non_openai_compatible_apis() {
    let params = sampling(&[("top_p", json!(0.9)), ("top_k", json!(40))]);
    let ctx = make_context();

    let openai = completions_model(None);
    let present = capture_completions_payload(&openai, simple_with(Some(params.clone()), None));
    assert_eq!(
        present.get("top_p"),
        Some(&json!(0.9)),
        "control: the openai route DOES send it"
    );

    let model = anthropic_model();
    let lowered = build_base_options(
        &model,
        &ctx,
        &simple_with(Some(params), None),
        Some("fake-key"),
    );
    assert!(
        lowered.sampling_params.is_some(),
        "the lowering is api-blind — `buildBaseOptions` resolves the map for EVERY api; only the \
         adapter decides (simple-options.ts:27-33)"
    );
    let body = crate::api::anthropic_messages::build_body(&model, &ctx, &lowered);
    assert_eq!(body.get("top_p"), None);
    assert_eq!(body.get("top_k"), None);
}

/// The two OpenAI-compatible adapters pi's test file does not cover, asserted through the same
/// override-the-named-field property that proves position, not merely presence.
#[test]
fn agent026_both_responses_adapters_assign_sampling_params_last() {
    let ctx = make_context();
    let params = sampling(&[("top_p", json!(0.25)), ("max_output_tokens", json!(4096))]);

    let mut responses = completions_model(None);
    responses.api = "openai-responses".into();
    responses.base_url = "https://api.openai.com/v1".to_string();
    let lowered = build_base_options(
        &responses,
        &ctx,
        &simple_with(Some(params.clone()), None),
        Some("fake-key"),
    );
    let body = crate::api::openai_responses::build_params(&responses, &ctx, &lowered, None);
    assert_eq!(body.get("top_p"), Some(&json!(0.25)));
    assert_eq!(
        body.get("max_output_tokens"),
        Some(&json!(4096)),
        "assigned last, so it beats the named max-tokens field the adapter wrote"
    );

    let mut azure = completions_model(None);
    azure.api = "azure-openai-responses".into();
    azure.base_url = "https://example.openai.azure.com".to_string();
    let lowered = build_base_options(&azure, &ctx, &simple_with(Some(params), None), Some("k"));
    let body = crate::api::azure_openai_responses::build_params(&azure, &ctx, &lowered, "dep")
        .expect("fixture declares no unsatisfiable constrained sampling");
    assert_eq!(body.get("top_p"), Some(&json!(0.25)));
    assert_eq!(body.get("max_output_tokens"), Some(&json!(4096)));
}

/// The merge's `undefined` case, which the adapters' `if (options?.samplingParams)` guard depends
/// on: neither side set ⇒ `None`, NOT `Some({})`. An empty-but-present map would make every adapter
/// take a branch pi does not take.
#[test]
fn agent026_the_merge_yields_none_when_neither_side_sets_anything() {
    let ctx = make_context();
    let model = completions_model(None);
    let lowered = build_base_options(&model, &ctx, &SimpleStreamOptions::default(), None);
    assert!(lowered.sampling_params.is_none());

    let model = completions_model(Some(Map::new()));
    let lowered = build_base_options(&model, &ctx, &SimpleStreamOptions::default(), None);
    assert!(
        lowered.sampling_params.is_some(),
        "a present-but-empty map is truthy in JS and spreads to `{{}}`; pi keeps it"
    );
}

// ---------------------------------------------------------------------------------------------
// CFG-104 / PROV-146 — `samplingParamsByThinkingLevel`, ported from pi's
// `packages/ai/test/sampling-options.test.ts:133-238` @f1b2e77f5. pi resolves
// `{ ...model.samplingParams, ...model.samplingParamsByThinkingLevel[clamp(level)], ...request }`
// (`api/simple-options.ts:24-34`) in `buildBaseOptions` AND in each OpenAI-compatible
// `buildParams`, so the direct (non-simple) cases below call the adapters with NO
// `build_base_options` in front — the path the interactive agent takes.
// ---------------------------------------------------------------------------------------------

/// pi `makeModel(api, samplingParams, { reasoning, thinkingLevelMap, samplingParamsByThinkingLevel })`.
fn leveled_model(
    api: &str,
    flat: Option<Map<String, Value>>,
    by_level: crate::model::SamplingParamsByThinkingLevel,
) -> Model {
    let mut model = completions_model(None);
    model.api = api.into();
    model.reasoning = true;
    model.sampling_params = crate::model::ModelSamplingParams::new(flat, Some(by_level));
    model
}

/// Build the body each OpenAI-compatible adapter would send for `opts` — pi's `capturePayload`
/// via `stream()` with `onPayload`, i.e. NO `buildBaseOptions` in front.
fn direct_payload(model: &Model, opts: &StreamOptions) -> Value {
    let ctx = make_context();
    match model.api.as_str() {
        "openai-completions" => crate::api::openai_completions::build_body(model, &ctx, opts),
        "openai-responses" => crate::api::openai_responses::build_params(model, &ctx, opts, None),
        "azure-openai-responses" => {
            crate::api::azure_openai_responses::build_params(model, &ctx, opts, "dep")
                .expect("fixture declares no unsatisfiable constrained sampling")
        }
        other => panic!("not an OpenAI-compatible api: {other}"),
    }
}

/// pi: "applies sampling params for the effective thinking level over model defaults". `low` and
/// `medium` are mapped to `null` (unsupported), so a `low` request clamps UP to `high` and picks
/// `high`'s entry, while the flat `top_p` survives underneath it.
#[test]
fn cfg104_applies_the_effective_levels_params_over_model_defaults() {
    let model = {
        let mut m = leveled_model(
            "openai-completions",
            Some(sampling(&[
                ("temperature", json!(1)),
                ("top_p", json!(0.95)),
            ])),
            crate::model::SamplingParamsByThinkingLevel {
                high: Some(sampling(&[
                    ("temperature", json!(0.8)),
                    ("top_k", json!(64)),
                ])),
                ..Default::default()
            },
        );
        m.thinking_level_map = Some(
            [("low".to_string(), None), ("medium".to_string(), None)]
                .into_iter()
                .collect(),
        );
        m
    };
    let payload = capture_completions_payload(
        &model,
        SimpleStreamOptions {
            reasoning: Some(cyrup_core::ThinkingLevel::Low),
            ..Default::default()
        },
    );
    assert_eq!(payload.get("temperature"), Some(&json!(0.8)));
    assert_eq!(payload.get("top_p"), Some(&json!(0.95)));
    assert_eq!(payload.get("top_k"), Some(&json!(64)));
}

/// pi: "applies off sampling params when reasoning is disabled".
#[test]
fn cfg104_applies_the_off_entry_when_reasoning_is_disabled() {
    let mut model = leveled_model(
        "openai-completions",
        None,
        crate::model::SamplingParamsByThinkingLevel {
            off: Some(sampling(&[("temperature", json!(0.7))])),
            ..Default::default()
        },
    );
    model.reasoning = false;
    let payload = capture_completions_payload(&model, SimpleStreamOptions::default());
    assert_eq!(payload.get("temperature"), Some(&json!(0.7)));
}

/// pi: "merges stream-option keys over thinking-level keys".
#[test]
fn cfg104_request_keys_win_over_thinking_level_keys() {
    let model = leveled_model(
        "openai-completions",
        None,
        crate::model::SamplingParamsByThinkingLevel {
            low: Some(sampling(&[
                ("temperature", json!(0.6)),
                ("top_p", json!(0.95)),
            ])),
            ..Default::default()
        },
    );
    let mut options = simple_with(Some(sampling(&[("top_p", json!(0.5))])), None);
    options.reasoning = Some(cyrup_core::ThinkingLevel::Low);
    let payload = capture_completions_payload(&model, options);
    assert_eq!(payload.get("temperature"), Some(&json!(0.6)));
    assert_eq!(payload.get("top_p"), Some(&json!(0.5)));
}

/// pi `it.each([...three apis])`: "applies thinking-level params between model and request params"
/// — through the adapter ALONE, with `reasoningEffort: "low"`. This is the path where nothing
/// upstream of the adapter merged the model's defaults in (PROV-123's shape): the flat
/// `temperature: 1` loses to the level's `0.6`, the level's `top_k` arrives, and the request's
/// `top_p` beats the flat one.
#[test]
fn cfg104_each_openai_compatible_adapter_applies_level_params_without_build_base_options() {
    for api in [
        "openai-completions",
        "openai-responses",
        "azure-openai-responses",
    ] {
        let model = leveled_model(
            api,
            Some(sampling(&[
                ("temperature", json!(1)),
                ("top_p", json!(0.95)),
            ])),
            crate::model::SamplingParamsByThinkingLevel {
                low: Some(sampling(&[
                    ("temperature", json!(0.6)),
                    ("top_k", json!(64)),
                ])),
                ..Default::default()
            },
        );
        let opts = StreamOptions {
            reasoning: cyrup_core::ModelThinkingLevel::Low,
            sampling_params: Some(sampling(&[("top_p", json!(0.5))])),
            ..Default::default()
        };
        let payload = direct_payload(&model, &opts);
        assert_eq!(payload.get("temperature"), Some(&json!(0.6)), "{api}");
        assert_eq!(payload.get("top_p"), Some(&json!(0.5)), "{api}");
        assert_eq!(payload.get("top_k"), Some(&json!(64)), "{api}");
    }
}

/// pi: "uses medium sampling params for summary-only openai-responses requests" — the level is
/// `reasoningEffort ?? (reasoningSummary ? "medium" : undefined)` (`openai-responses.ts:363`).
/// The azure half of pi's `it.each` is not portable: cyrup's azure options carry no
/// `reasoningSummary` (see `azure_openai_responses::build_params`).
#[test]
fn cfg104_summary_only_responses_request_uses_the_medium_entry() {
    let model = leveled_model(
        "openai-responses",
        None,
        crate::model::SamplingParamsByThinkingLevel {
            off: Some(sampling(&[("temperature", json!(0.7))])),
            medium: Some(sampling(&[("temperature", json!(0.8))])),
            ..Default::default()
        },
    );
    let opts = StreamOptions {
        api_options: Some(crate::stream::ApiStreamOptions::OpenAiResponses(
            crate::api::openai_responses::OpenAiResponsesOptions {
                reasoning_summary: Some(crate::api::openai_responses::ReasoningSummary::Auto),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    let payload = direct_payload(&model, &opts);
    assert_eq!(payload["reasoning"]["effort"], json!("medium"));
    assert_eq!(payload.get("temperature"), Some(&json!(0.8)));
}

/// pi: "is ignored by non-OpenAI-compatible APIs", for the per-level map too.
///
/// A regression GUARD, not a red proof of CFG-104: `anthropic_messages` never read sampling params
/// before or after the change, so this passes at the base too. It pins that the new per-level map
/// does not leak into a non-OpenAI-compatible body.
#[test]
fn cfg104_level_params_are_ignored_by_anthropic_messages() {
    let mut model = anthropic_model();
    model.sampling_params = crate::model::ModelSamplingParams::new(
        None,
        Some(crate::model::SamplingParamsByThinkingLevel {
            high: Some(sampling(&[("top_k", json!(40))])),
            off: Some(sampling(&[("top_k", json!(40))])),
            ..Default::default()
        }),
    );
    let opts = StreamOptions {
        reasoning: cyrup_core::ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = crate::api::anthropic_messages::build_body(&model, &make_context(), &opts);
    assert_eq!(body.get("top_k"), None);
}

/// The JSON shape stays pi's: `samplingParams` and `samplingParamsByThinkingLevel` are sibling
/// keys on the model (`types.ts:851-854` @f1b2e77f5), whatever the Rust field layout is.
#[test]
fn cfg104_model_json_keeps_pis_two_sibling_keys_and_round_trips() {
    let model = leveled_model(
        "openai-completions",
        Some(sampling(&[("top_p", json!(0.9))])),
        crate::model::SamplingParamsByThinkingLevel {
            high: Some(sampling(&[("temperature", json!(0.6))])),
            ..Default::default()
        },
    );
    let value = serde_json::to_value(&model).unwrap();
    assert_eq!(value["samplingParams"], json!({ "top_p": 0.9 }));
    assert_eq!(
        value["samplingParamsByThinkingLevel"],
        json!({ "high": { "temperature": 0.6 } })
    );
    let back: Model = serde_json::from_value(value).unwrap();
    assert_eq!(back, model);

    let bare = serde_json::to_value(completions_model(None)).unwrap();
    assert!(bare.get("samplingParams").is_none());
    assert!(bare.get("samplingParamsByThinkingLevel").is_none());
    let back: Model = serde_json::from_value(bare).unwrap();
    assert_eq!(back.sampling_params, None, "absent keys are the outer None");
}

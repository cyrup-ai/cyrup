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
//! pi captures the body with `onPayload` and throws out of the callback; cyrup's `build_body` /
//! `build_params` are the same function pi's payload comes from, called directly — no live socket,
//! same assertion.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use serde_json::{json, Map, Value};

use crate::context::Context;
use crate::model::{Modality, Model, ModelCost};
use crate::stream::StreamOptions;
use crate::utils::simple_options::{build_base_options, SimpleStreamOptions};

fn sampling(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect()
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
        context_window: 128_000,
        max_tokens: 16_384,
        sampling_params,
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
        context_window: 200_000,
        max_tokens: 32_000,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

/// pi `capturePayload(model, options)` — lower the simple options exactly as `streamSimple` does,
/// then build the body the request would have carried.
fn capture_completions_payload(model: &Model, options: SimpleStreamOptions) -> Value {
    let ctx = make_context();
    let lowered = build_base_options(model, &ctx, &options, Some("fake-key"));
    crate::api::openai_completions::build_body(model, &ctx, &lowered)
}

fn simple_with(sampling_params: Option<Map<String, Value>>, temperature: Option<f32>) -> SimpleStreamOptions {
    SimpleStreamOptions {
        base: StreamOptions { sampling_params, temperature, ..Default::default() },
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
            Some(sampling(&[("top_p", json!(0.95)), ("top_k", json!(0)), ("min_p", json!(0))])),
            None,
        ),
    );
    assert_eq!(body.get("top_p"), Some(&json!(0.95)));
    assert_eq!(body.get("top_k"), Some(&json!(0)), "a zero-valued key must still be sent");
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
    let model =
        completions_model(Some(sampling(&[("temperature", json!(1)), ("top_p", json!(0.95))])));
    let body = capture_completions_payload(&model, SimpleStreamOptions::default());
    assert_eq!(body.get("temperature"), Some(&json!(1)));
    assert_eq!(body.get("top_p"), Some(&json!(0.95)));
}

/// pi: "merges stream-option keys over model-level keys" — PER KEY. A whole-map replacement passes
/// the `top_p` assertion and fails `min_p`, which is exactly why pi asserts both.
#[test]
fn agent026_stream_option_keys_override_model_level_keys_per_key() {
    let model =
        completions_model(Some(sampling(&[("top_p", json!(0.95)), ("min_p", json!(0.05))])));
    let body = capture_completions_payload(
        &model,
        simple_with(Some(sampling(&[("top_p", json!(0.5))])), None),
    );
    assert_eq!(body.get("top_p"), Some(&json!(0.5)), "the per-request key wins");
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
    let present = capture_completions_payload(
        &openai,
        simple_with(Some(params.clone()), None),
    );
    assert_eq!(present.get("top_p"), Some(&json!(0.9)), "control: the openai route DOES send it");

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

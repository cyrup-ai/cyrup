//! PROV-147: the `openai-decisions` classifier api (pi `packages/ai/src/api/openai-decisions.ts`
//! @f1b2e77f5), driven over the workspace's LOOPBACK fake answering `POST /v1/decisions` through an
//! intercept, case for case with pi's `test/openai-decisions.test.ts` (each test names the case it
//! ports), plus the cyrup-side cases those cannot express: the JS text of the state, a non-image
//! `images` entry, and the order of the answers' parse errors.
//!
//! pi drives `classify` through a fake `fetch`; cyrup's `ClassifierOptions` carries no transport
//! hook (`llama_cpp_classify`'s module doc), so the model's `baseUrl` points at the fake instead and
//! every request is read back from it.
//!
//! All of these were red at the base: there was no `openai-decisions` implementation, so
//! `KnownClassifierApi::OpenAiDecisions` did not exist and the module did not compile. Each
//! behaviour was also red-proved by a targeted mutation, listed on the PROV-147 ledger row.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use serde_json::{Value, json};

use super::llama_cpp_classify_fake_server::{
    Behavior, FakeServer, Recorded, Reply, boolean, choice, options, score,
};
use crate::api::openai_decisions::{MAX_IMAGES, openai_decisions_api};
use crate::classifier::{
    ClassifierAnswer, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult,
    ClassifierStopReason, OrderedMap,
};
use crate::model::{ModelCost, ModelCostTier};
use crate::Modality;
use cyrup_core::Content;

/// pi's test model (test:5-21): `gpt-6-luna` with the long-context tier, on the fake.
fn model(base_url: &str) -> ClassifierModel {
    ClassifierModel {
        id: "gpt-6-luna".into(),
        name: "GPT-6 Luna".into(),
        api: "openai-decisions".into(),
        provider: "openai".into(),
        base_url: base_url.to_string(),
        input: vec![Modality::Text, Modality::Image],
        input_limits: None,
        cost: ModelCost {
            input: 0.1,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            tiers: Some(vec![ModelCostTier {
                input_tokens_above: 272_000,
                input: 0.2,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            }]),
        },
        headers: None,
        context_window: 922_000,
    }
}

/// pi's test context (test:23-42). Note the EMPTY `failure` description, which the wire omits.
fn context() -> ClassifierContext {
    let mut state = serde_json::Map::new();
    state.insert(
        "text".to_string(),
        json!("The deployment succeeded, thank you."),
    );
    ClassifierContext {
        state,
        images: None,
        questions: vec![
            (
                "category",
                choice(
                    "Classify the message",
                    &[("success", "Successful"), ("failure", "")],
                ),
            ),
            (
                "satisfaction",
                score("Score satisfaction", &["low", "neutral", "high"]),
            ),
            (
                "approved",
                boolean("Does the user approve?", "Approval", "No approval"),
            ),
        ]
        .into_iter()
        .collect(),
    }
}

/// "Response shape from the API reference and live `gpt-6-luna` requests." (test:44-68).
fn wire_answers() -> Vec<Value> {
    vec![
        json!({
            "type": "choice",
            "name": "category",
            "choice": "success",
            "probabilities": [
                { "value": "success", "probability": 0.9 },
                { "value": "failure", "probability": 0.1 },
            ],
            "confidence": 0.8,
        }),
        json!({
            "type": "score",
            "name": "satisfaction",
            "score": 1.8,
            "probabilities": [
                { "value": 0, "label": "low", "probability": 0.05 },
                { "value": 1, "label": "neutral", "probability": 0.1 },
                { "value": 2, "label": "high", "probability": 0.85 },
            ],
            "confidence": 0.7,
        }),
        json!({ "type": "predicate", "name": "approved", "probability": 0.95 }),
    ]
}

/// test:70-76.
fn wire_usage() -> Value {
    json!({
        "input_tokens": 164,
        "input_tokens_details": { "cached_tokens": 0, "cache_write_tokens": 0 },
        "output_tokens": 0,
        "output_tokens_details": { "reasoning_tokens": 0 },
        "total_tokens": 164,
    })
}

/// test:78.
fn image(mime_type: &str) -> Content {
    Content::Image {
        data: "aW1hZ2U=".to_string(),
        mime_type: mime_type.to_string(),
    }
}

fn keyed() -> ClassifierOptions {
    let mut opts = options();
    opts.api_key = Some("secret".to_string());
    opts
}

/// A fake that answers `/v1/decisions` with `reply(nth)`.
async fn decisions(
    reply: impl Fn(&Recorded, usize) -> Reply + Send + Sync + 'static,
) -> FakeServer {
    FakeServer::with(Behavior::default().intercept(move |request, nth| {
        (request.path == "/v1/decisions").then(|| reply(request, nth))
    }))
    .await
}

async fn classify(
    server: &FakeServer,
    context: &ClassifierContext,
    opts: &ClassifierOptions,
) -> ClassifierResult {
    openai_decisions_api()
        .classify(&model(&server.base_url), context, opts)
        .await
}

fn expected_answers() -> OrderedMap<ClassifierAnswer> {
    let mut answers = OrderedMap::new();
    answers.insert(
        "category",
        ClassifierAnswer::Choice {
            choice: "success".to_string(),
            probabilities: [("success", 0.9), ("failure", 0.1)].into_iter().collect(),
            confidence: 0.8,
        },
    );
    answers.insert(
        "satisfaction",
        ClassifierAnswer::Score {
            score: 1.8,
            confidence: 0.7,
        },
    );
    answers.insert("approved", ClassifierAnswer::Bool { probability: 0.95 });
    answers
}

/// pi test:81-130, "maps questions to Decisions types and answers back by name". The answers come
/// back REVERSED and are matched by name; `temperature` is not sent.
#[tokio::test]
async fn maps_questions_to_decisions_types_and_answers_back_by_name() {
    let server = decisions(|_, _| {
        let mut answers = wire_answers();
        answers.reverse();
        Reply::Json(json!({ "model": "gpt-6-luna", "answers": answers, "usage": wire_usage() }))
    })
    .await;
    let mut opts = keyed();
    opts.temperature = 1.5;

    let result = classify(&server, &context(), &opts).await;

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "one request: {requests:?}");
    let request = &requests[0];
    assert_eq!(request.path, "/v1/decisions");
    assert_eq!(request.header("authorization"), Some("Bearer secret"));
    assert_eq!(
        request.body,
        json!({
            "model": "gpt-6-luna",
            "input": r#"{"text":"The deployment succeeded, thank you."}"#,
            "questions": [
                {
                    "type": "choice",
                    "name": "category",
                    "instructions": "Classify the message",
                    "choices": [{ "value": "success", "description": "Successful" }, { "value": "failure" }],
                },
                {
                    "type": "score",
                    "name": "satisfaction",
                    "instructions": "Score satisfaction",
                    "levels": [{ "label": "low" }, { "label": "neutral" }, { "label": "high" }],
                },
                {
                    "type": "predicate",
                    "name": "approved",
                    "instructions": "Does the user approve?\n\nTrue means: Approval\nFalse means: No approval",
                },
            ],
        })
    );
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop, "{result:?}");
    assert_eq!(result.answers, expected_answers());
    let usage = result.usage.expect("usage");
    assert_eq!((usage.input, usage.output, usage.cache_read), (164, 0, 0));
    assert_eq!(usage.total_tokens, 164);
    assert!((usage.cost.total - 0.000_016_4).abs() < 1e-12, "{}", usage.cost.total);
}

/// pi test:132-139, "prices long-context requests at the long-context input rate".
#[tokio::test]
async fn prices_long_context_requests_at_the_long_context_input_rate() {
    let server = decisions(|_, _| {
        Reply::Json(json!({
            "answers": wire_answers(),
            "usage": { "input_tokens": 300_000, "output_tokens": 0 },
        }))
    })
    .await;
    let result = classify(&server, &context(), &keyed()).await;
    let total = result.usage.expect("usage").cost.total;
    assert!((total - 0.06).abs() < 1e-12, "{total}");
}

/// pi test:141-166, "sends images after the state in one user message".
#[tokio::test]
async fn sends_images_after_the_state_in_one_user_message() {
    let server = decisions(|_, _| Reply::Json(json!({ "answers": wire_answers() }))).await;
    let mut ctx = context();
    ctx.images = Some(vec![image("image/png"), image("image/jpeg")]);

    let result = classify(&server, &ctx, &keyed()).await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Stop, "{result:?}");
    assert_eq!(
        server.requests()[0].body["input"],
        json!([{
            "role": "user",
            "content": [
                { "type": "input_text", "text": r#"{"text":"The deployment succeeded, thank you."}"# },
                { "type": "input_image", "image_url": "data:image/png;base64,aW1hZ2U=" },
                { "type": "input_image", "image_url": "data:image/jpeg;base64,aW1hZ2U=" },
            ],
        }])
    );
}

/// `images: []` is the no-image form (pi `context.images ?? []`, then `images.length === 0`).
#[tokio::test]
async fn an_empty_image_list_sends_the_state_string() {
    let server = decisions(|_, _| Reply::Json(json!({ "answers": wire_answers() }))).await;
    let mut ctx = context();
    ctx.images = Some(Vec::new());
    classify(&server, &ctx, &keyed()).await;
    assert_eq!(
        server.requests()[0].body["input"],
        json!(r#"{"text":"The deployment succeeded, thank you."}"#)
    );
}

/// pi test:168-183, "rejects more than 128 images before sending". 128 is accepted.
#[tokio::test]
async fn rejects_more_than_128_images_before_sending() {
    let server = decisions(|_, _| Reply::Json(json!({ "answers": wire_answers() }))).await;
    let mut ctx = context();
    ctx.images = Some(vec![image("image/png"); MAX_IMAGES + 1]);

    let result = classify(&server, &ctx, &keyed()).await;

    assert!(server.requests().is_empty());
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("OpenAI Decisions accepts at most 128 images, got 129")
    );

    ctx.images = Some(vec![image("image/png"); MAX_IMAGES]);
    let result = classify(&server, &ctx, &keyed()).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop, "{result:?}");
    assert_eq!(server.requests().len(), 1);
}

/// The image cap is checked while the body is built, BEFORE `postClassifierRequest`'s api-key
/// check (pi evaluates `wireInput(context)` as an argument of the post): with no key AND too many
/// images, the image error wins.
#[tokio::test]
async fn the_image_cap_fails_before_the_missing_key() {
    let server = decisions(|_, _| Reply::Json(json!({ "answers": wire_answers() }))).await;
    let mut ctx = context();
    ctx.images = Some(vec![image("image/png"); MAX_IMAGES + 1]);
    let result = classify(&server, &ctx, &options()).await;
    assert_eq!(
        result.error_message.as_deref(),
        Some("OpenAI Decisions accepts at most 128 images, got 129")
    );
}

/// `[CYRUP-DELTA, type]`: pi's `images` is `ImageContent[]`, so a text block there is unspellable;
/// cyrup's carries `Content`, and a non-image entry is refused before sending rather than sent as
/// `data:undefined;base64,undefined`.
#[tokio::test]
async fn a_non_image_entry_in_images_is_refused_before_sending() {
    let server = decisions(|_, _| Reply::Json(json!({ "answers": wire_answers() }))).await;
    let mut ctx = context();
    ctx.images = Some(vec![image("image/png"), Content::text("not an image")]);
    let result = classify(&server, &ctx, &keyed()).await;
    assert!(server.requests().is_empty());
    assert_eq!(
        result.error_message.as_deref(),
        Some("OpenAI Decisions accepts only image content in images")
    );
}

/// pi test:185-199, "fails the result when a question is refused and keeps the billed usage".
#[tokio::test]
async fn fails_the_result_when_a_question_is_refused_and_keeps_the_billed_usage() {
    let server = decisions(|_, _| {
        let answers = wire_answers();
        Reply::Json(json!({
            "answers": [answers[0], answers[1], { "type": "refusal", "name": "approved" }],
            "usage": wire_usage(),
        }))
    })
    .await;
    let result = classify(&server, &context(), &keyed()).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert!(result.answers.is_empty());
    assert_eq!(
        result.error_message.as_deref(),
        Some("OpenAI Decisions refused to answer approved")
    );
    assert_eq!(result.usage.expect("billed usage kept").input, 164);
}

/// pi test:201-216, "returns missing and mistyped answers as classifier errors".
#[tokio::test]
async fn returns_missing_and_mistyped_answers_as_classifier_errors() {
    let missing = decisions(|_, _| {
        let answers = wire_answers();
        Reply::Json(json!({ "answers": [answers[0], answers[1]] }))
    })
    .await;
    let mistyped = decisions(|_, _| {
        let answers = wire_answers();
        Reply::Json(json!({
            "answers": [answers[0], answers[1], { "type": "score", "name": "approved" }],
        }))
    })
    .await;

    let missing = classify(&missing, &context(), &keyed()).await;
    let mistyped = classify(&mistyped, &context(), &keyed()).await;

    assert_eq!(missing.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        missing.error_message.as_deref(),
        Some("OpenAI Decisions did not return an answer for approved")
    );
    assert_eq!(mistyped.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        mistyped.error_message.as_deref(),
        Some("OpenAI Decisions did not return a predicate answer for approved")
    );
}

/// pi test:218-233, "preserves prototype-sensitive question IDs in answers". `__proto__` is an
/// ordinary key in Rust, so this pins the id round trip the JS test exists for.
#[tokio::test]
async fn preserves_prototype_sensitive_question_ids_in_answers() {
    let server = decisions(|_, _| {
        Reply::Json(json!({
            "answers": [{ "type": "predicate", "name": "__proto__", "probability": 0.75 }],
        }))
    })
    .await;
    let ctx: ClassifierContext = serde_json::from_str(
        r#"{"state":{},"questions":{"__proto__":{"type":"bool","instructions":"Is this true?","criteria":{"true":"Yes","false":"No"}}}}"#,
    )
    .unwrap();
    let result = classify(&server, &ctx, &keyed()).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop, "{result:?}");
    assert_eq!(
        result.answers.get("__proto__"),
        Some(&ClassifierAnswer::Bool { probability: 0.75 })
    );
}

/// pi test:235-250, "does not retry gateway timeouts and explains them instead of returning the
/// HTML page". `retry-after-ms: 0` would make a retry immediate, and the default budget is 2.
#[tokio::test]
async fn does_not_retry_gateway_timeouts_and_explains_them() {
    let server = decisions(|_, _| {
        Reply::status_with(
            504,
            &[("retry-after-ms", "0")],
            "<!DOCTYPE html><html>Gateway time-out</html>",
        )
    })
    .await;
    let result = classify(&server, &context(), &keyed()).await;
    assert_eq!(server.requests().len(), 1, "a 504 is not retried");
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some(
            "OpenAI Decisions error (504): the request timed out at the gateway. Very large inputs \
             (above roughly 600K tokens) currently exceed its time limit."
        )
    );
}

/// pi `provider-retry.ts:121` checks the no-retry list AFTER `isRetryableProviderError`, so it wins
/// over an explicit `x-should-retry: true` too.
#[tokio::test]
async fn a_504_is_not_retried_even_when_the_server_asks_for_a_retry() {
    let server = decisions(|_, _| {
        Reply::status_with(
            504,
            &[("retry-after-ms", "0"), ("x-should-retry", "true")],
            "gateway",
        )
    })
    .await;
    classify(&server, &context(), &keyed()).await;
    assert_eq!(server.requests().len(), 1);
}

/// pi test:252-264, "still retries other server errors".
#[tokio::test]
async fn still_retries_other_server_errors() {
    let server = decisions(|_, nth| {
        if nth == 0 {
            Reply::status_with(503, &[("retry-after-ms", "0")], "busy")
        } else {
            Reply::Json(json!({ "answers": wire_answers() }))
        }
    })
    .await;
    let result = classify(&server, &context(), &keyed()).await;
    assert_eq!(server.requests().len(), 2);
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop, "{result:?}");
}

/// pi test:266-280, "includes the API error body for other HTTP failures".
#[tokio::test]
async fn includes_the_api_error_body_for_other_http_failures() {
    let body = json!({
        "error": {
            "message": "Decision input exceeds the token limit.",
            "type": "invalid_request_error",
        },
    })
    .to_string();
    let server = decisions(move |_, _| Reply::status(400, &body)).await;
    let mut opts = keyed();
    opts.max_retries = 0;
    let result = classify(&server, &context(), &opts).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    let message = result.error_message.unwrap();
    assert!(message.contains("OpenAI Decisions error (400)"), "{message}");
    assert!(
        message.contains("Decision input exceeds the token limit."),
        "{message}"
    );
}

/// pi test:282-290, "rejects models for other classifier APIs and missing API keys".
#[tokio::test]
async fn rejects_models_for_other_classifier_apis_and_missing_api_keys() {
    let server = decisions(|_, _| Reply::Json(json!({ "answers": wire_answers() }))).await;
    let mut other = model(&server.base_url);
    other.api = "typesafe-system-one".into();
    let other_api = openai_decisions_api()
        .classify(&other, &context(), &keyed())
        .await;
    let no_key = classify(&server, &context(), &options()).await;

    assert!(server.requests().is_empty());
    assert_eq!(
        other_api.error_message.as_deref(),
        Some("Unsupported classifier API: typesafe-system-one")
    );
    assert_eq!(
        no_key.error_message.as_deref(),
        Some("No API key for provider: openai")
    );
}

/// PROV-110 corners (1) and (2) on this wire: the `input` is `JSON.stringify(context.state)` as JS
/// prints it, because it is the text the model reads — integer-like keys hoisted ahead of the
/// others in ascending order, at every depth; a float with no fraction printed without one; an
/// integer beyond 2^53 printed as the nearest double; exponents in JS form. Red-proved by sending
/// `serde_json::to_string(state)` instead (every one of these assertions fails).
#[tokio::test]
async fn the_state_text_is_what_json_stringify_prints() {
    let server = decisions(|_, _| Reply::Json(json!({ "answers": wire_answers() }))).await;
    let mut ctx = context();
    ctx.state = serde_json::from_str(
        r#"{"b":1.0,"10":"ten","2":{"z":true,"1":[{"y":0,"0":1}]},"big":9007199254740993,"e":1e21}"#,
    )
    .unwrap();
    classify(&server, &ctx, &keyed()).await;
    assert_eq!(
        server.requests()[0].body["input"],
        json!(
            r#"{"2":{"1":[{"0":1,"y":0}],"z":true},"10":"ten","b":1,"big":9007199254740992,"e":1e+21}"#
        )
    );
}

/// pi's `body.answers` must be an ARRAY; anything else, and a body that is not an object, is
/// "returned an unexpected response". Usage on such a body is still kept when the body is an
/// object (pi parses usage first).
#[tokio::test]
async fn an_answers_object_is_an_unexpected_response_with_usage_kept() {
    let server = decisions(|_, _| {
        Reply::Json(json!({ "answers": { "approved": 0.5 }, "usage": wire_usage() }))
    })
    .await;
    let result = classify(&server, &context(), &keyed()).await;
    assert_eq!(
        result.error_message.as_deref(),
        Some("OpenAI Decisions returned an unexpected response")
    );
    assert_eq!(result.usage.expect("usage kept").input, 164);

    let array = decisions(|_, _| Reply::Json(json!([1, 2]))).await;
    let result = classify(&array, &context(), &keyed()).await;
    assert_eq!(
        result.error_message.as_deref(),
        Some("OpenAI Decisions returned an unexpected response")
    );
    assert!(result.usage.is_none());
}

/// `choiceProbabilities`: entries are `{value, probability}` objects; a non-array, a non-object
/// entry or a non-string value is "invalid probabilities", a non-finite probability names its
/// field, and a later entry for the same value takes its probability while keeping its place.
#[tokio::test]
async fn choice_probabilities_are_validated_and_read_in_entry_order() {
    let with_probabilities = |probabilities: Value| {
        let mut answers = wire_answers();
        answers[0]["probabilities"] = probabilities;
        Reply::Json(json!({ "answers": answers }))
    };
    for (probabilities, message) in [
        (
            json!({ "success": 0.9 }),
            "OpenAI Decisions returned invalid probabilities for category",
        ),
        (
            json!([0.9]),
            "OpenAI Decisions returned invalid probabilities for category",
        ),
        (
            json!([{ "value": 1, "probability": 0.9 }]),
            "OpenAI Decisions returned invalid probabilities for category",
        ),
        (
            json!([{ "value": "success", "probability": "0.9" }]),
            "OpenAI Decisions returned an invalid probability for category.success",
        ),
    ] {
        let reply = with_probabilities(probabilities);
        let server = decisions(move |_, _| reply.clone()).await;
        let result = classify(&server, &context(), &keyed()).await;
        assert_eq!(result.error_message.as_deref(), Some(message));
    }

    let reply = with_probabilities(json!([
        { "value": "success", "probability": 0.5 },
        { "value": "failure", "probability": 0.1 },
        { "value": "success", "probability": 0.9 },
    ]));
    let server = decisions(move |_, _| reply.clone()).await;
    let result = classify(&server, &context(), &keyed()).await;
    match result.answers.get("category") {
        Some(ClassifierAnswer::Choice { probabilities, .. }) => assert_eq!(
            probabilities
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect::<Vec<_>>(),
            [("success".to_string(), 0.9), ("failure".to_string(), 0.1)]
        ),
        other => panic!("{other:?}"),
    }
}

/// A bool with no meanings sends its instructions alone; one with only a `false` meaning sends
/// just that line (pi `predicateInstructions`, `:43-49`).
#[tokio::test]
async fn predicate_instructions_carry_only_the_meanings_given() {
    let server = decisions(|_, _| {
        Reply::Json(json!({
            "answers": [
                { "type": "predicate", "name": "plain", "probability": 0.1 },
                { "type": "predicate", "name": "no_only", "probability": 0.2 },
            ],
        }))
    })
    .await;
    let ctx = ClassifierContext {
        state: serde_json::Map::new(),
        images: None,
        questions: vec![
            ("plain", boolean("Is it?", "", "")),
            ("no_only", boolean("Is it?", "", "Nope")),
        ]
        .into_iter()
        .collect(),
    };
    let result = classify(&server, &ctx, &keyed()).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop, "{result:?}");
    let questions = &server.requests()[0].body["questions"];
    assert_eq!(questions[0]["instructions"], json!("Is it?"));
    assert_eq!(
        questions[1]["instructions"],
        json!("Is it?\n\nFalse means: Nope")
    );
    assert_eq!(server.requests()[0].body["input"], json!("{}"));
}

// ------------------------------------------------------------------ the built-in openai provider

/// An env with the given vars, and proxy resolution pinned off for the loopback request.
struct Env(Vec<(&'static str, &'static str)>);

#[async_trait::async_trait]
impl crate::auth::AuthContext for Env {
    async fn env(&self, name: &str) -> Option<String> {
        if name.eq_ignore_ascii_case("no_proxy") {
            return Some("*".to_string());
        }
        self.0
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_string())
    }
    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

fn builtin_models(
    credentials: Option<std::sync::Arc<dyn crate::auth::CredentialStore>>,
    overlay: Option<std::sync::Arc<crate::remote_catalog::CatalogOverlay>>,
) -> crate::Models {
    crate::providers::all::default_models(crate::CreateModelsOptions {
        credentials,
        auth_context: Some(std::sync::Arc::new(Env(vec![("OPENAI_API_KEY", "secret")]))),
        catalog_overlay: overlay,
    })
}

/// pi `classifier-models.test.ts:200-224`, "routes OpenAI GPT-6 Luna through the Decisions API with
/// images": the built-in `openai` provider lists `gpt-6-luna` as a CLASSIFIER on
/// `openai-decisions` beside the chat row of the same id on `openai-responses`, and
/// `Models::classify` dispatches it — images included — to `<base>/decisions`. Red at the base,
/// where `openai` had no classifier rows and no `classifiers` map (`Models::classify` answered
/// `Provider openai does not support classification`).
#[tokio::test]
async fn the_builtin_openai_provider_routes_gpt_6_luna_through_the_decisions_api() {
    let models = builtin_models(None, None);
    let luna = models
        .get_classifier_model("openai", "gpt-6-luna")
        .expect("the openai classifier catalog carries gpt-6-luna");
    assert_eq!(luna.api.as_str(), "openai-decisions");
    assert_eq!(luna.input, [Modality::Text, Modality::Image]);
    assert_eq!(luna.context_window, 922_000);
    assert_eq!(luna.base_url, "https://api.openai.com/v1");
    // `withOpenAiLongContextPricing` (generate-models.ts:2705): one 272k tier at 2x input.
    assert_eq!(luna.cost.input, 0.1);
    assert_eq!(
        luna.cost.tiers.as_deref().map(|tiers| tiers
            .iter()
            .map(|t| (t.input_tokens_above, t.input))
            .collect::<Vec<_>>()),
        Some(vec![(272_000, 0.2)])
    );
    // The chat entry with the same id stays separate.
    assert_eq!(
        models
            .get_model("openai", "gpt-6-luna")
            .map(|m| m.api.as_str().to_string())
            .as_deref(),
        Some("openai-responses")
    );
    assert!(models.get_provider("openai").unwrap().supports_classification());

    let server = decisions(|_, _| {
        Reply::Json(json!({
            "answers": [{ "type": "predicate", "name": "approved", "probability": 0.8 }],
        }))
    })
    .await;
    let mut luna = luna;
    luna.base_url = server.base_url.clone();
    let ctx = ClassifierContext {
        state: context().state,
        images: Some(vec![image("image/png")]),
        questions: vec![("approved", boolean("Approved?", "", ""))]
            .into_iter()
            .collect(),
    };
    let result = models.classify(&luna, &ctx, &options()).await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Stop, "{result:?}");
    assert_eq!(
        result.answers.get("approved"),
        Some(&ClassifierAnswer::Bool { probability: 0.8 })
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/decisions");
    // The provider's `OPENAI_API_KEY` strategy supplied the bearer.
    assert_eq!(requests[0].header("authorization"), Some("Bearer secret"));
}

/// pi `classifier-models.test.ts:226-246`, "lists OpenAI Decisions models only for API key
/// credentials": the openai provider's `filterAllModels` (`providers/openai.ts:27-29`) hides the
/// classifier rows from a Sign in with ChatGPT credential, which the Decisions API rejects, and
/// keeps every chat row — including the chat `gpt-6-luna`. Red at the base (no classifier rows and
/// no `filter_all_models` seam on `WireProvider`).
#[tokio::test]
async fn openai_decisions_models_are_listed_only_for_api_key_credentials() {
    for overlay in [None, Some(openai_overlay())] {
        let label = if overlay.is_some() { "overlaid" } else { "embedded" };
        let with_api_key = builtin_models(
            Some(store(crate::Credential::api_key("secret"))),
            overlay.clone(),
        );
        let with_oauth = builtin_models(
            Some(store(crate::auth::oauth::oauth_credential(
                "access",
                "refresh",
                crate::auth::oauth::now_ms() + 3_600_000,
            ))),
            overlay,
        );
        let classifier_ids = |models: Vec<crate::AnyModel>| -> Vec<String> {
            models.iter().map(|m| m.id().to_string()).collect()
        };
        assert_eq!(
            classifier_ids(
                with_api_key
                    .get_available_of_type(crate::ModelType::Classifier, Some("openai"))
                    .await
                    .unwrap()
            ),
            ["gpt-6-luna"],
            "{label}"
        );
        assert!(
            with_oauth
                .get_available_of_type(crate::ModelType::Classifier, Some("openai"))
                .await
                .unwrap()
                .is_empty(),
            "{label}: an OAuth credential must not list the Decisions model"
        );
        // Chat models stay available with ChatGPT OAuth.
        assert!(
            with_oauth
                .get_available(Some("openai"))
                .await
                .unwrap()
                .iter()
                .any(|m| m.id.as_str() == "gpt-6-luna"),
            "{label}: the chat gpt-6-luna stays available"
        );
    }
}

/// A credential store holding one `openai` credential.
fn store(credential: crate::Credential) -> std::sync::Arc<dyn crate::auth::CredentialStore> {
    std::sync::Arc::new(
        crate::auth::InMemoryCredentialStore::new().with_credential("openai".into(), credential),
    )
}

/// A non-empty pi.dev overlay for `openai`, so `default_models` wraps the built-in in
/// `RemoteCatalogProvider` — every built-in is wrapped once a catalog is cached. The decorator must
/// carry `filterAllModels` through (`remote-catalog-provider.ts` spreads `...provider`); the trait
/// default is the ABSENT member, which would list the Decisions row to an OAuth credential.
fn openai_overlay() -> std::sync::Arc<crate::remote_catalog::CatalogOverlay> {
    let row = crate::providers::openai::openai_models()
        .into_iter()
        .next()
        .expect("an openai chat row");
    std::sync::Arc::new(crate::remote_catalog::CatalogOverlay::from_entries([(
        "openai".to_string(),
        vec![row],
    )]))
}

//! PROV-104: the two System One classifier apis, `typesafe-system-one` and
//! `cloudflare-workers-ai-system-one` (pi `packages/ai/src/api/typesafe-system-one.ts`,
//! `cloudflare-workers-ai-system-one.ts`, `system-one-shared.ts`, `classifier-shared.ts`
//! @f1b2e77f5), each driven over a LOOPBACK fake answering one choice, one bool and one score
//! question, plus the cases of pi's `test/typesafe-system-one.test.ts`.
//!
//! The `typesafe-system-one` fake is the workspace's `llama-server` fake
//! ([`super::llama_cpp_classify_fake_server`]) answering `POST /v1/systemone` from
//! `cyrup_llama_cpp_wire::systemone`, whose definitions reproduce a LIVE b11436 answer byte for
//! byte. The Cloudflare fake is the same server answering `POST /run` in Cloudflare's envelope
//! through an intercept.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::llama_cpp_classify_fake_server::{
    Behavior, FakeServer, Reply, SYSTEM_ONE_INPUT_TOKENS, boolean, choice, options, score,
    system_one_output,
};
use crate::api::cloudflare_workers_ai_system_one::cloudflare_workers_ai_system_one_api;
use crate::api::typesafe_system_one::typesafe_system_one_api;
use crate::classifier::{
    ClassifierAnswer, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult,
    ClassifierStopReason, KnownClassifierApi, OrderedMap,
};
use crate::provider::Provider;
use crate::{Modality, ModelCost};

const TYPESAFE: &str = "typesafe-system-one";
const CLOUDFLARE: &str = "cloudflare-workers-ai-system-one";

/// A System One classifier model on `base_url` (pi's test model, `jev-latest`, test:5-15).
fn model(api: &str, base_url: &str) -> ClassifierModel {
    ClassifierModel {
        id: "jev-latest".into(),
        name: "Jev".into(),
        api: api.into(),
        provider: "typesafe".into(),
        base_url: base_url.to_string(),
        input: vec![Modality::Text],
        input_limits: None,
        cost: ModelCost::default(),
        headers: None,
        context_window: 64000,
    }
}

/// pi's test context (test:17-36): one choice, one score and one bool question.
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
                    &[("success", "Successful"), ("failure", "Failed")],
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

/// Options carrying an api key (System One refuses a request without one).
fn keyed() -> ClassifierOptions {
    let mut opts = options();
    opts.api_key = Some("secret".to_string());
    opts
}

/// The answers the fake gives [`context`] by default: the first choice option at 0.75, the
/// expected level of weights 1:2:3 (= 8/6), and a noul of 0.95.
fn expected_answers() -> OrderedMap<ClassifierAnswer> {
    let mut answers = OrderedMap::new();
    answers.insert(
        "category",
        ClassifierAnswer::Choice {
            choice: "success".to_string(),
            probabilities: [("success", 0.75), ("failure", 0.25)].into_iter().collect(),
            confidence: 0.5,
        },
    );
    answers.insert(
        "satisfaction",
        ClassifierAnswer::Score {
            score: 2.0 / 6.0 + 1.0,
            confidence: 0.25,
        },
    );
    answers.insert("approved", ClassifierAnswer::Bool { probability: 0.95 });
    answers
}

async fn typesafe(model: &ClassifierModel, opts: &ClassifierOptions) -> ClassifierResult {
    typesafe_system_one_api()
        .classify(model, &context(), opts)
        .await
}

async fn cloudflare(model: &ClassifierModel, opts: &ClassifierOptions) -> ClassifierResult {
    cloudflare_workers_ai_system_one_api()
        .classify(model, &context(), opts)
        .await
}

// --------------------------------------------------------------------------- typesafe-system-one --

/// THE VERIFY: a loopback `llama-server` answers one choice, one bool and one score question
/// through `typesafe-system-one`; pi's main case (test:53-81): the bool goes out as `noul`, no
/// temperature is sent, the key is a bearer token, the URL is `<base>/systemone`, and the `noul`
/// answer comes back as the public `bool`.
#[tokio::test]
async fn typesafe_answers_one_choice_one_bool_and_one_score_question_over_loopback() {
    let server = FakeServer::start().await;
    let mut opts = keyed();
    opts.temperature = 1.5;

    let result = typesafe(&model(TYPESAFE, &server.base_url), &opts).await;

    assert_eq!(result.error_message, None);
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
    assert_eq!(result.answers, expected_answers());
    assert_eq!(result.api.as_str(), TYPESAFE);
    assert_eq!(result.model, "jev-latest");

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "one request answers every question");
    let request = &requests[0];
    assert_eq!(request.path, "/v1/systemone", "llama.cpp's own route");
    assert_eq!(request.header("authorization"), Some("Bearer secret"));
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(
        request.body,
        json!({
            "model": "jev-latest",
            "state": { "text": "The deployment succeeded, thank you." },
            "questions": {
                "category": {
                    "type": "choice",
                    "instructions": "Classify the message",
                    "criteria": { "success": "Successful", "failure": "Failed" },
                },
                "satisfaction": {
                    "type": "score",
                    "instructions": "Score satisfaction",
                    "criteria": ["low", "neutral", "high"],
                },
                "approved": {
                    "type": "noul",
                    "instructions": "Does the user approve?",
                    "criteria": { "true": "Approval", "false": "No approval" },
                },
            },
        }),
        "System One has no temperature field; a bool question is a noul on the wire"
    );
    let keys: Vec<&str> = request
        .body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["model", "state", "questions"],
        "`{{ model, ...request }}`"
    );
    // llama.cpp reports the prompt tokens and no output; the model's catalog price is zero.
    let usage = result.usage.expect("usage");
    assert_eq!(
        (usage.input, usage.output, usage.total_tokens),
        (SYSTEM_ONE_INPUT_TOKENS, 0, SYSTEM_ONE_INPUT_TOKENS)
    );
    assert_eq!(usage.cost.total, 0.0);
}

/// test:72-81: usage is priced from the model's catalog like chat usage
/// (308 * 0.042 / 1e6 = 0.000012936), and a reply without usage leaves none.
#[tokio::test]
async fn typesafe_prices_usage_from_the_catalog() {
    let server = FakeServer::with(Behavior::default().system_one(|_, _| None, 308)).await;
    let mut priced = model(TYPESAFE, &server.base_url);
    priced.cost.input = 0.042;
    let result = typesafe(&priced, &keyed()).await;
    let usage = result.usage.expect("usage");
    assert_eq!(
        (usage.input, usage.output, usage.total_tokens),
        (308, 0, 308)
    );
    assert!(
        (usage.cost.total - 0.000012936).abs() < 1e-12,
        "{}",
        usage.cost.total
    );

    let bare = FakeServer::with(Behavior::default().intercept(|request, _| {
        let mut body = system_one_output(&Behavior::default(), &request.body).unwrap();
        body.as_object_mut().unwrap().remove("usage");
        Some(Reply::Json(body))
    }))
    .await;
    let result = typesafe(&model(TYPESAFE, &bare.base_url), &keyed()).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
    assert!(result.usage.is_none());
}

/// test:84-95 shape: OpenRouter's reply carries `id` and `provider` beside `answers`; the base URL
/// with no trailing slash still resolves to `<base>/systemone`.
#[tokio::test]
async fn typesafe_reads_answers_beside_other_members() {
    let server = FakeServer::with(Behavior::default().intercept(|request, _| {
        let mut body = system_one_output(&Behavior::default(), &request.body).unwrap();
        let object = body.as_object_mut().unwrap();
        object.insert("id".to_string(), json!("gen-dec-1"));
        object.insert("provider".to_string(), json!("TypeSafe"));
        Some(Reply::Json(body))
    }))
    .await;
    let base = format!("{}///", server.base_url);
    let result = typesafe(&model(TYPESAFE, &base), &keyed()).await;
    assert_eq!(result.error_message, None);
    assert_eq!(result.answers, expected_answers());
    assert_eq!(server.requests()[0].path, "/v1/systemone");
}

/// test:112-124: a model of another classifier api is refused before any request.
#[tokio::test]
async fn typesafe_rejects_models_for_other_classifier_apis() {
    let server = FakeServer::start().await;
    let result = typesafe(&model(CLOUDFLARE, &server.base_url), &keyed()).await;
    assert!(server.requests().is_empty());
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Unsupported classifier API: cloudflare-workers-ai-system-one")
    );
    assert!(result.answers.is_empty());
}

/// `postClassifierRequest`: no api key fails before any request.
#[tokio::test]
async fn typesafe_without_an_api_key_sends_nothing() {
    let server = FakeServer::start().await;
    let result = typesafe(&model(TYPESAFE, &server.base_url), &options()).await;
    assert!(server.requests().is_empty());
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("No API key for provider: typesafe")
    );
}

/// test:139-163: headers merge case-insensitively, the request's over the model's over the
/// defaults, and a `None` request header suppresses one.
#[tokio::test]
async fn typesafe_merges_headers_case_insensitively_and_supports_suppression() {
    let server = FakeServer::start().await;
    let mut with_headers = model(TYPESAFE, &server.base_url);
    let mut model_headers = crate::HeaderMap::new();
    model_headers.insert(
        "authorization".to_string(),
        Some("Bearer model".to_string()),
    );
    model_headers.insert("X-Source".to_string(), Some("model".to_string()));
    with_headers.headers = Some(model_headers);

    let mut overlay = keyed();
    let mut request_headers = crate::HeaderMap::new();
    request_headers.insert(
        "Authorization".to_string(),
        Some("Bearer request".to_string()),
    );
    request_headers.insert("x-source".to_string(), Some("request".to_string()));
    overlay.headers = Some(request_headers);
    typesafe(&with_headers, &overlay).await;

    let mut suppress = keyed();
    let mut suppressed = crate::HeaderMap::new();
    suppressed.insert("Authorization".to_string(), None);
    suppress.headers = Some(suppressed);
    typesafe(&with_headers, &suppress).await;

    let requests = server.requests();
    assert_eq!(requests[0].header("authorization"), Some("Bearer request"));
    assert_eq!(requests[0].header("x-source"), Some("request"));
    assert_eq!(
        requests[0]
            .headers
            .iter()
            .filter(|(name, _)| name == "authorization")
            .count(),
        1
    );
    assert_eq!(requests[1].header("authorization"), None);
}

/// test:206-225: a retryable failure is retried with a fresh attempt.
#[tokio::test]
async fn typesafe_retries_a_retryable_failure() {
    let server = FakeServer::with(Behavior::default().intercept(|_, nth| {
        (nth == 0).then(|| Reply::status_with(500, &[("retry-after-ms", "0")], "retry"))
    }))
    .await;
    let mut opts = keyed();
    opts.max_retries = 1;
    let result = typesafe(&model(TYPESAFE, &server.base_url), &opts).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
    assert_eq!(server.requests_to("/v1/systemone").len(), 2);
}

/// test:227-239: malformed answers are an error that keeps the billed usage.
#[tokio::test]
async fn typesafe_malformed_answers_are_errors_that_keep_usage() {
    let server = FakeServer::with(Behavior::default().intercept(|_, _| {
        Some(Reply::Json(
            json!({ "answers": {}, "usage": { "input_tokens": 10, "output_tokens": 2 } }),
        ))
    }))
    .await;
    let result = typesafe(&model(TYPESAFE, &server.base_url), &keyed()).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert!(result.answers.is_empty());
    assert_eq!(
        result.error_message.as_deref(),
        Some("System One API did not return an answer for category")
    );
    let usage = result
        .usage
        .expect("the request was billed, so its usage is kept");
    assert_eq!((usage.input, usage.output), (10, 2));
}

/// An answer of the wrong type for its question is refused, per type.
#[tokio::test]
async fn typesafe_refuses_an_answer_of_the_wrong_type() {
    for (id, wrong, message) in [
        (
            "category",
            json!({ "type": "noul", "noul": 0.5 }),
            "did not return a choice answer for category",
        ),
        (
            "satisfaction",
            json!({ "type": "choice", "choice": "x" }),
            "did not return a score answer for satisfaction",
        ),
        (
            "approved",
            json!({ "type": "bool", "probability": 0.5 }),
            "did not return a bool answer for approved",
        ),
        (
            "approved",
            json!({ "type": "noul", "noul": "high" }),
            "returned an invalid probability for approved",
        ),
    ] {
        let server = FakeServer::with(Behavior::default().intercept(move |request, _| {
            let mut body = system_one_output(&Behavior::default(), &request.body).unwrap();
            body["answers"][id] = wrong.clone();
            Some(Reply::Json(body))
        }))
        .await;
        let result = typesafe(&model(TYPESAFE, &server.base_url), &keyed()).await;
        assert_eq!(result.stop_reason, ClassifierStopReason::Error);
        assert_eq!(
            result.error_message.as_deref(),
            Some(format!("System One API {message}").as_str())
        );
    }
}

/// test:241-255: malformed usage is ignored rather than failing the result.
#[tokio::test]
async fn typesafe_ignores_malformed_usage() {
    for (usage, expected) in [
        (
            json!({ "input_tokens": "many", "output_tokens": 3 }),
            Some((0, 3, 3)),
        ),
        (json!({ "cost": 0.1 }), None),
    ] {
        let server = FakeServer::with(Behavior::default().intercept(move |request, _| {
            let mut body = system_one_output(&Behavior::default(), &request.body).unwrap();
            body["usage"] = usage.clone();
            Some(Reply::Json(body))
        }))
        .await;
        let result = typesafe(&model(TYPESAFE, &server.base_url), &keyed()).await;
        assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
        assert_eq!(
            result
                .usage
                .map(|usage| (usage.input, usage.output, usage.total_tokens)),
            expected
        );
    }
}

/// A llama.cpp model that is not a decision model answers `/v1/systemone` with llama.cpp's 501
/// (captured from a live b11436 router for `stories260K`), rendered as pi renders an HTTP error.
#[tokio::test]
async fn typesafe_renders_llama_cpp_refusing_a_text_model() {
    let server = FakeServer::with(Behavior::default().intercept(|_, _| {
        let (status, body) = cyrup_llama_cpp_wire::systemone::not_a_decision_model();
        Some(Reply::status(status, &body.to_string()))
    }))
    .await;
    let result = typesafe(&model(TYPESAFE, &server.base_url), &keyed()).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some(
            r#"System One API error (501): {"error":{"code":501,"message":"This model is not a decision model","type":"not_supported_error"}}"#
        )
    );
}

// --------------------------------------------------------------- cloudflare-workers-ai-system-one --

/// Builds a Cloudflare envelope around a System One output.
type Wrap = fn(Value) -> Value;

/// The Cloudflare fake: `POST <base>/run` answered in the envelope `wrap` builds around the System
/// One output the llama.cpp fake would give for the request's `input`.
async fn cloudflare_server(wrap: Wrap) -> FakeServer {
    FakeServer::with(Behavior::default().intercept(move |request, _| {
        (request.path == "/v1/run").then(|| {
            let output = system_one_output(&Behavior::default(), &request.body["input"]).unwrap();
            Reply::Json(wrap(output))
        })
    }))
    .await
}

/// THE VERIFY for the second api: a loopback Workers AI fake answers one choice, one bool and one
/// score question; the request is `{model, input: {state, questions}}` at `<base>/run`, and the run
/// record (`typesafe/jev`'s form) is unwrapped.
#[tokio::test]
async fn cloudflare_answers_one_choice_one_bool_and_one_score_question_over_loopback() {
    let server = cloudflare_server(
        |output| json!({ "success": true, "result": { "state": "Completed", "result": output } }),
    )
    .await;
    let result = cloudflare(&model(CLOUDFLARE, &server.base_url), &keyed()).await;

    assert_eq!(result.error_message, None);
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
    assert_eq!(result.answers, expected_answers());
    assert_eq!(result.usage.expect("usage").input, SYSTEM_ONE_INPUT_TOKENS);
    let request = &server.requests()[0];
    assert_eq!(request.path, "/v1/run");
    assert_eq!(request.header("authorization"), Some("Bearer secret"));
    let keys: Vec<&str> = request
        .body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["model", "input"]);
    assert_eq!(request.body["model"], json!("jev-latest"));
    assert_eq!(
        request.body["input"]["questions"]["approved"]["type"],
        json!("noul")
    );
    assert_eq!(
        request.body["input"]["state"],
        json!({ "text": "The deployment succeeded, thank you." })
    );
}

/// A Cloudflare-hosted model (`@cf/cloudflare/clef`) returns the output directly in `result`.
#[tokio::test]
async fn cloudflare_reads_a_direct_result() {
    let server = cloudflare_server(|output| json!({ "success": true, "result": output })).await;
    let result = cloudflare(&model(CLOUDFLARE, &server.base_url), &keyed()).await;
    assert_eq!(result.error_message, None);
    assert_eq!(result.answers, expected_answers());
}

/// `success: false` fails with the envelope's messages; a run that has not completed fails with
/// its state; a missing `result` is an unexpected response.
#[tokio::test]
async fn cloudflare_reports_envelope_failures() {
    let cases: [(Wrap, &str); 4] = [
        (
            |_| json!({ "success": false, "errors": [{ "message": "a" }, { "code": 1 }, { "message": "b" }] }),
            "Cloudflare Workers AI error: a; b",
        ),
        (
            |_| json!({ "success": false, "errors": [] }),
            "Cloudflare Workers AI request failed",
        ),
        (
            |_| json!({ "success": true, "result": { "state": "Running" } }),
            "Cloudflare Workers AI run did not complete (state: Running)",
        ),
        (
            |_| json!({ "success": true }),
            "Cloudflare Workers AI returned an unexpected response",
        ),
    ];
    for (wrap, message) in cases {
        let server = cloudflare_server(wrap).await;
        let result = cloudflare(&model(CLOUDFLARE, &server.base_url), &keyed()).await;
        assert_eq!(result.stop_reason, ClassifierStopReason::Error);
        assert_eq!(result.error_message.as_deref(), Some(message));
    }
}

/// Each api refuses the other's models before any request.
#[tokio::test]
async fn cloudflare_rejects_models_for_other_classifier_apis() {
    let server = FakeServer::start().await;
    let result = cloudflare(&model(TYPESAFE, &server.base_url), &keyed()).await;
    assert!(server.requests().is_empty());
    assert_eq!(
        result.error_message.as_deref(),
        Some("Unsupported classifier API: typesafe-system-one")
    );
}

/// pi `system-one-shared.ts:116`: a context carrying images is refused before any request, by
/// both transports, with the transport's own label. `ClassifierContext.images` arrived with
/// PROV-148 after PROV-104 ported these apis, so this guard is what keeps an image-judging classify
/// from silently dropping its images here.
#[tokio::test]
async fn both_system_one_transports_refuse_image_input_before_sending() {
    let server = FakeServer::start().await;
    let with_images = ClassifierContext {
        images: Some(vec![cyrup_core::Content::Image {
            data: "iVBORw0KGgo=".to_string(),
            mime_type: "image/png".to_string(),
        }]),
        ..context()
    };
    for (expected, result) in [
        (
            "System One API does not support image input",
            typesafe_system_one_api()
                .classify(&model(TYPESAFE, &server.base_url), &with_images, &keyed())
                .await,
        ),
        (
            "Cloudflare Workers AI does not support image input",
            cloudflare_workers_ai_system_one_api()
                .classify(&model(CLOUDFLARE, &server.base_url), &with_images, &keyed())
                .await,
        ),
    ] {
        assert_eq!(
            result.stop_reason,
            ClassifierStopReason::Error,
            "{expected}"
        );
        assert_eq!(result.error_message.as_deref(), Some(expected));
        assert!(result.answers.is_empty(), "{expected}");
    }
    assert!(server.requests().is_empty(), "nothing may reach the server");
}

// ------------------------------------------------------------------------------ provider entries --

/// The built-in `openrouter` provider registers `typesafe-system-one` (pi
/// `providers/openrouter.ts:34`), so its classifier rows classify; before PROV-104 the registry
/// had no entry and this answered `Provider openrouter does not support classification`.
#[tokio::test]
async fn the_openrouter_provider_classifies_through_typesafe_system_one() {
    let provider = crate::providers::fleet::OPENROUTER.provider();
    assert!(provider.supports_classification());
    let server = FakeServer::start().await;
    let mut row = model(TYPESAFE, &server.base_url);
    row.provider = "openrouter".into();
    let result = provider.classify(&row, &context(), &keyed()).await;
    assert_eq!(result.error_message, None);
    assert_eq!(result.answers, expected_answers());
    assert_eq!(server.requests()[0].path, "/v1/systemone");
}

/// The Workers AI provider registers `cloudflare-workers-ai-system-one`
/// (`providers/cloudflare-workers-ai.ts:21-23`), and `Models::classify` resolves the catalog's
/// `{CLOUDFLARE_ACCOUNT_ID}` placeholder through the provider's auth before dispatch — the job
/// pi's `cloudflareClassifier` wrapper does.
#[tokio::test]
async fn the_workers_ai_provider_classifies_with_the_account_id_resolved() {
    use crate::auth::AuthContext;
    use crate::collection::{CreateModelsOptions, create_models};

    struct CloudflareEnv;
    #[async_trait::async_trait]
    impl AuthContext for CloudflareEnv {
        async fn env(&self, name: &str) -> Option<String> {
            match name {
                "CLOUDFLARE_API_KEY" => Some("cf-key".to_string()),
                "CLOUDFLARE_ACCOUNT_ID" => Some("acct".to_string()),
                _ => None,
            }
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    let server = FakeServer::with(Behavior::default().intercept(|request, _| {
        (request.path == "/acct/ai/run").then(|| {
            let output = system_one_output(&Behavior::default(), &request.body["input"]).unwrap();
            Reply::Json(json!({ "success": true, "result": output }))
        })
    }))
    .await;
    let root = server.base_url.trim_end_matches("/v1");
    let provider = crate::providers::cloudflare::cloudflare_workers_ai_provider();
    assert!(provider.supports_classification());
    let mut models = create_models(CreateModelsOptions {
        credentials: None,
        auth_context: Some(Arc::new(CloudflareEnv)),
        catalog_overlay: None,
    });
    models.set_provider(Arc::new(provider));
    let mut clef = model(CLOUDFLARE, &format!("{root}/{{CLOUDFLARE_ACCOUNT_ID}}/ai"));
    clef.id = "@cf/cloudflare/clef".into();
    clef.provider = "cloudflare-workers-ai".into();

    let result = models.classify(&clef, &context(), &options()).await;

    assert_eq!(result.error_message, None);
    assert_eq!(result.answers, expected_answers());
    let request = &server.requests()[0];
    assert_eq!(request.path, "/acct/ai/run");
    assert_eq!(request.header("authorization"), Some("Bearer cf-key"));
}

/// PROV-153's Verify: the BUILT-IN `typesafe` provider (pi `typesafeProvider()`,
/// `providers/typesafe.ts` @f1b2e77f5) lists `jev-latest` as its only model — a classifier, never a
/// chat model (pi `classifier-models.test.ts:115-129`) — classifies, and `Models::classify` through
/// it reaches TypeSafe's own System One endpoint with `Bearer <TYPESAFE_API_KEY>`. Without the key
/// the provider is not configured. Red at the base: there was no `typesafe` provider.
#[tokio::test]
async fn the_builtin_typesafe_provider_classifies_against_its_own_system_one_endpoint() {
    use crate::auth::AuthContext;
    use crate::classifier::AnyModel;
    use crate::collection::CreateModelsOptions;

    struct TypesafeEnv(Option<&'static str>);
    #[async_trait::async_trait]
    impl AuthContext for TypesafeEnv {
        async fn env(&self, name: &str) -> Option<String> {
            match name {
                "TYPESAFE_API_KEY" => self.0.map(str::to_string),
                // Pin proxy resolution off for the loopback request.
                "no_proxy" | "NO_PROXY" => Some("*".to_string()),
                _ => None,
            }
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }
    let models_with = |key: Option<&'static str>| {
        crate::providers::all::default_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(TypesafeEnv(key))),
            catalog_overlay: None,
        })
    };

    let models = models_with(Some("ts-key"));
    let all = models.get_all_models(Some("typesafe"));
    assert_eq!(all.len(), 1, "{all:?}");
    let Some(AnyModel::Classifier(jev)) = all.first().cloned() else {
        panic!("typesafe's one model is a classifier: {all:?}");
    };
    assert_eq!(jev.id.as_str(), "jev-latest");
    assert_eq!(jev.api.as_str(), TYPESAFE);
    assert_eq!(jev.base_url, "https://api.typesafe.ai/v1/");
    assert_eq!(jev.context_window, 64000);
    assert!(models.get_model("typesafe", "jev-latest").is_none());
    assert!(models.get_models(Some("typesafe")).is_empty());
    assert_eq!(
        models.get_classifier_model("typesafe", "jev-latest"),
        Some(jev.clone())
    );
    assert!(
        models
            .get_provider("typesafe")
            .expect("registered")
            .supports_classification()
    );

    let server = FakeServer::start().await;
    let mut jev = jev;
    // The catalog's trailing slash is kept: `endpoint_url` collapses it as pi's `new URL` does.
    jev.base_url = format!("{}/", server.base_url);
    let result = models.classify(&jev, &context(), &options()).await;
    assert_eq!(result.error_message, None);
    assert_eq!(result.answers, expected_answers());
    let request = &server.requests()[0];
    assert_eq!(request.path, "/v1/systemone");
    assert_eq!(request.header("authorization"), Some("Bearer ts-key"));
    assert_eq!(request.body["model"], json!("jev-latest"));
    // "price them at zero" (generate-models.ts:2605-2606): counted, not charged.
    let usage = result.usage.expect("usage");
    assert_eq!(usage.input, SYSTEM_ONE_INPUT_TOKENS);
    assert_eq!(usage.cost.total, 0.0);

    let unconfigured = models_with(None)
        .classify(&jev, &context(), &options())
        .await;
    assert_eq!(
        unconfigured.error_message.as_deref(),
        Some("Provider is not configured: typesafe")
    );
}

/// pi's `typesafeProvider()` passes no `api`, so `createProvider` answers any chat stream with
/// `Provider typesafe has no API implementation for "<api>"` (`models.ts:1076-1080`). The built-in
/// carries an EMPTY api registry, so a chat model handed to it fails rather than being streamed
/// through the shared registry under TypeSafe's key. The assertion is cyrup's wording
/// (`ProviderError::NoApiImpl`), which differs from pi's and carries no provider id: PROV-157.
#[tokio::test]
async fn the_typesafe_provider_streams_nothing() {
    use crate::context::Context;
    use crate::stream::{StreamOptions, collect_message};

    let provider = crate::providers::typesafe::typesafe_provider();
    let chat = crate::providers::openai::openai_models()
        .into_iter()
        .next()
        .expect("an openai chat row");
    let mut chat = chat;
    chat.provider = "typesafe".into();
    let message = collect_message(provider.stream(
        &chat,
        &Context::default(),
        &StreamOptions {
            api_key: Some("ts-key".to_string()),
            ..StreamOptions::default()
        },
    ))
    .await;
    assert_eq!(message.stop_reason, cyrup_core::StopReason::Error);
    assert!(
        message
            .error_message
            .as_deref()
            .is_some_and(|m| m.contains("no API implementation for openai-responses")),
        "{message:?}"
    );
}

/// PROV-110 corner (1) on the System One wire. pi posts `JSON.stringify({model, state, questions})`
/// built from JS objects, so the state's integer-like keys reach the server hoisted ahead of the
/// others, ascending, at every depth (`OrdinaryOwnPropertyKeys`); a decision model is shown the
/// state in that order. The fake records the body as parsed with `preserve_order`, so the order
/// asserted here is the wire's. Red at the base, where the body was the state's insertion order.
#[tokio::test]
async fn the_state_reaches_the_wire_in_js_key_order() {
    let server = FakeServer::start().await;
    let mut ctx = context();
    ctx.state = serde_json::from_str(r#"{"b":1,"10":2,"2":{"y":0,"1":0},"a":3}"#).unwrap();
    let result = typesafe_system_one_api()
        .classify(&model(TYPESAFE, &server.base_url), &ctx, &keyed())
        .await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop, "{result:?}");
    let body = &server.requests_to("/v1/systemone")[0].body;
    let keys =
        |value: &Value| -> Vec<String> { value.as_object().unwrap().keys().cloned().collect() };
    assert_eq!(keys(&body["state"]), ["2", "10", "b", "a"]);
    assert_eq!(keys(&body["state"]["2"]), ["1", "y"]);
}

/// PROV-110 corner (1), the hook's view: pi's payload is built from the caller's JS object, so the
/// `onPayload` hook already sees `state` in own-key order (`system-one-shared.ts:88`,
/// `classifier-shared.ts:71`). Hoisting only while serializing the body would hand the hook the
/// insertion order instead. Red when the state is hoisted at serialization alone.
#[tokio::test]
async fn the_on_payload_hook_sees_the_state_in_js_key_order() {
    let server = FakeServer::start().await;
    let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let mut opts = keyed();
    let sink = Arc::clone(&seen);
    opts.on_payload = Some(Arc::new(move |payload: Value, _model| {
        let keys: Vec<String> = payload["state"]
            .as_object()
            .map(|state| state.keys().cloned().collect())
            .unwrap_or_default();
        *sink.lock().unwrap() = keys;
        Box::pin(async { None })
    }));
    let mut ctx = context();
    ctx.state = serde_json::from_str(r#"{"b":1,"10":2,"2":3}"#).unwrap();
    typesafe_system_one_api()
        .classify(&model(TYPESAFE, &server.base_url), &ctx, &opts)
        .await;
    assert_eq!(*seen.lock().unwrap(), ["2", "10", "b"]);
}

/// PROV-110 corner (2) on the System One wire. The body is pi's `JSON.stringify(payload)`, so a
/// number is spelled as JS spells it and an integer past 2^53 is the nearest double — the value
/// pi's server receives, and the value the llama.cpp prompt and the Decisions `input` already
/// carried for the same state. Asserted on the raw bytes, which a parsed `Value` cannot show. Red
/// at the base, which sent serde's text (`9007199254740993`, `1.0`, `1e21`).
#[tokio::test]
async fn the_system_one_body_spells_numbers_as_json_stringify_does() {
    let server = FakeServer::start().await;
    let mut ctx = context();
    ctx.state =
        serde_json::from_str(r#"{"id":9007199254740993,"f":1.0,"e":1e21,"s":0.000001}"#).unwrap();
    typesafe_system_one_api()
        .classify(&model(TYPESAFE, &server.base_url), &ctx, &keyed())
        .await;
    let raw = &server.requests_to("/v1/systemone")[0].raw_body;
    assert!(
        raw.contains(r#""state":{"id":9007199254740992,"f":1,"e":1e+21,"s":0.000001}"#),
        "{raw}"
    );
}

/// Every known classifier api names its own implementation, and each is the api it says it is.
#[tokio::test]
async fn every_known_classifier_api_has_its_implementation() {
    let server = FakeServer::start().await;
    // All four since PROV-147 added `openai-decisions` (and llama.cpp's api, which this used to
    // leave out, refuses a foreign api the same way).
    for api in KnownClassifierApi::ALL {
        // A model of a DIFFERENT api is refused by name, which identifies the implementation.
        let other = if api == KnownClassifierApi::TypesafeSystemOne {
            CLOUDFLARE
        } else {
            TYPESAFE
        };
        let result = api
            .implementation()
            .classify(&model(other, &server.base_url), &context(), &keyed())
            .await;
        assert_eq!(
            result.error_message.as_deref(),
            Some(format!("Unsupported classifier API: {other}").as_str())
        );
    }
    assert!(server.requests().is_empty());
}

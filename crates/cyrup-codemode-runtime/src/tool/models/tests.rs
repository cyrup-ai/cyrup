//! The `models.*` script globals (pi `createModelGlobals`, `extensions/codemode/execute.ts:524-630`
//! @v1.0.1), ported from upstream's `describe("codemode models")`
//! (`test/agent-session-codemode.test.ts:536-845`) at the tool's own seam: the five functions are
//! called as a script calls them, over a scripted [`CodemodeModels`].
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, Content, ToolCallId, ToolUpdate, Usage};
use cyrup_provider::{
    AnyModel, AssistantImages, ClassifierAnswer, ClassifierModel, ClassifierResult,
    ClassifierStopReason, HeaderMap, ImageModel, ImagesStopReason, Modality, ModelCost, OrderedMap,
};
use serde_json::{Value, json};

use super::models_globals;
use crate::testkit::FakeModels;
use crate::tool::CodemodeNestedCallStatus;
use crate::tool::recorder::Recorder;

/// An absolute docs path other than the production one, so a test fails when the path is not the one
/// handed in.
const CODEMODE_DOCS_PATH: &str = "/opt/cyrup/docs/codemode.md";
use crate::types::{CodemodeToolContext, ToolResult};

const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

fn scorer_model() -> ClassifierModel {
    ClassifierModel {
        id: "judge".into(),
        name: "Judge".into(),
        api: "test-classifier".into(),
        provider: "scorer".into(),
        base_url: "https://classifier.test/v1".into(),
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        headers: Some(HeaderMap::from([(
            "X-Secret".to_owned(),
            Some("hunter2".to_owned()),
        )])),
        context_window: 1000,
    }
}

fn painter_model() -> ImageModel {
    ImageModel {
        id: "painter".into(),
        name: "Painter".into(),
        api: "test-images".into(),
        provider: "scorer".into(),
        base_url: "https://images.test/v1".into(),
        input: vec![Modality::Text, Modality::Image],
        output: vec![Modality::Text, Modality::Image],
        cost: ModelCost::default(),
        headers: None,
    }
}

fn usage(input: u64, cost: f64) -> Usage {
    let mut usage = Usage {
        input,
        total_tokens: input,
        ..Usage::default()
    };
    usage.cost.total = cost;
    usage
}

struct Observed {
    classify_inputs: Mutex<Vec<(String, Value)>>,
    image_inputs: Mutex<Vec<(String, Vec<Content>)>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

struct Rig {
    models: Arc<FakeModels>,
    observed: Arc<Observed>,
    recorder: Arc<Recorder>,
    updates: Arc<Mutex<Vec<ToolUpdate>>>,
    globals: Vec<crate::types::CodemodeTool>,
}

fn rig() -> Rig {
    let observed = Arc::new(Observed {
        classify_inputs: Mutex::new(Vec::new()),
        image_inputs: Mutex::new(Vec::new()),
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
    });
    let classify_observed = Arc::clone(&observed);
    let images_observed = Arc::clone(&observed);
    let models = Arc::new(FakeModels::new(
        vec![
            AnyModel::Classifier(scorer_model()),
            AnyModel::Image(painter_model()),
        ],
        Arc::new(move |model, context| {
            let observed = Arc::clone(&classify_observed);
            Box::pin(async move {
                let now = observed.active.fetch_add(1, Ordering::SeqCst) + 1;
                observed.max_active.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                observed.active.fetch_sub(1, Ordering::SeqCst);
                let text = context.state.get("text").cloned().unwrap_or(Value::Null);
                observed
                    .classify_inputs
                    .lock()
                    .unwrap()
                    .push((model.base_url.clone(), text.clone()));
                let mut result = ClassifierResult::new(&model);
                if text == "explode" {
                    result.stop_reason = ClassifierStopReason::Error;
                    result.error_message = Some("classifier exploded".to_owned());
                    return result;
                }
                let mut answers = OrderedMap::new();
                answers.insert(
                    "approved",
                    ClassifierAnswer::Bool {
                        probability: if text == "good" { 0.9 } else { 0.1 },
                    },
                );
                result.answers = answers;
                result.usage = Some(usage(300, 0.001));
                result
            })
        }),
        Arc::new(move |model, context| {
            let observed = Arc::clone(&images_observed);
            Box::pin(async move {
                let prompt = context
                    .input
                    .iter()
                    .find_map(|block| match block {
                        Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .unwrap_or_default();
                observed
                    .image_inputs
                    .lock()
                    .unwrap()
                    .push((model.base_url.clone(), context.input.clone()));
                let mut result = AssistantImages::new(&model);
                if prompt == "explode" {
                    result.stop_reason = ImagesStopReason::Error;
                    result.error_message = Some("painter exploded".to_owned());
                    return result;
                }
                result.output = vec![
                    Content::text(format!("painted {prompt}")),
                    Content::Image {
                        data: TINY_PNG.to_owned(),
                        mime_type: "image/png".to_owned(),
                    },
                ];
                result.usage = Some(usage(100, 0.04));
                result
            })
        }),
    ));
    let updates = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&updates);
    let recorder = Arc::new(Recorder::new(
        ToolCallId::from("call-1"),
        Box::new(move |update| sink.lock().unwrap().push(update)),
    ));
    let globals = models_globals(models.clone(), Arc::clone(&recorder), CODEMODE_DOCS_PATH);
    Rig {
        models,
        observed,
        recorder,
        updates,
        globals,
    }
}

impl Rig {
    async fn call(&self, name: &str, args: Vec<Value>) -> ToolResult {
        let global = self
            .globals
            .iter()
            .find(|global| global.declaration.name == name)
            .unwrap_or_else(|| panic!("no global {name}"));
        assert!(global.declaration.spread);
        (global.execute)(
            Some(Value::Array(args)),
            CodemodeToolContext {
                cancel: CancelToken::new(),
            },
        )
        .await
    }

    async fn ok(&self, name: &str, args: Vec<Value>) -> Value {
        self.call(name, args).await.unwrap().unwrap()
    }

    async fn err(&self, name: &str, args: Vec<Value>) -> String {
        self.call(name, args).await.unwrap_err()
    }
}

const QUESTIONS: &str = r#"{ "approved": { "type": "bool", "instructions": "Approval?", "criteria": { "true": "yes", "false": "no" } } }"#;

fn questions() -> Value {
    serde_json::from_str(QUESTIONS).unwrap()
}

/// The five functions are declared as spread globals under `models.`.
#[test]
fn the_five_globals_are_declared() {
    let rig = rig();
    assert_eq!(
        rig.globals
            .iter()
            .map(|global| global.declaration.name.as_str())
            .collect::<Vec<_>>(),
        [
            "models.getModelsOfType",
            "models.getAvailableOfType",
            "models.getModelOfType",
            "models.classify",
            "models.generateImages",
        ]
    );
}

/// Upstream `lists models and classifies with catalog auth, ignoring script-supplied fields`.
#[tokio::test]
async fn lists_models_without_headers_and_classifies_with_catalog_auth() {
    let rig = rig();
    let available = rig
        .ok(
            "models.getAvailableOfType",
            vec![json!("classifier"), json!("scorer")],
        )
        .await;
    let model = available[0].clone();
    assert_eq!(model["id"], "judge");
    assert!(
        model.get("headers").is_none(),
        "headers can carry credentials"
    );
    assert_eq!(model["type"], "classifier");

    let listed = rig
        .ok("models.getModelsOfType", vec![json!("classifier")])
        .await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["provider"] == "scorer" && entry["id"] == "judge")
    );
    let same = rig
        .ok(
            "models.getModelOfType",
            vec![json!("classifier"), json!("scorer"), json!("judge")],
        )
        .await;
    assert_eq!(same["id"], "judge");
    assert!(same.get("headers").is_none());
    assert_eq!(
        rig.call(
            "models.getModelOfType",
            vec![json!("classifier"), json!("scorer"), json!("nope")]
        )
        .await,
        Ok(None),
        "an unknown model is undefined"
    );

    // Six classifications, with a baseUrl the script made up.
    let mut evil = model.clone();
    evil["baseUrl"] = json!("https://evil.test");
    let texts = ["good", "bad", "good", "bad", "good", "bad"];
    let results = futures::future::join_all(texts.iter().map(|text| {
        rig.call(
            "models.classify",
            vec![
                evil.clone(),
                json!({ "state": { "text": text }, "questions": questions() }),
            ],
        )
    }))
    .await;
    let probabilities: Vec<f64> = results
        .iter()
        .map(|result| {
            result.clone().unwrap().unwrap()["answers"]["approved"]["probability"]
                .as_f64()
                .unwrap()
        })
        .collect();
    assert_eq!(probabilities, [0.9, 0.1, 0.9, 0.1, 0.9, 0.1]);
    let first = results[0].clone().unwrap().unwrap();
    assert_eq!(first["usage"]["cost"]["total"], 0.001);
    assert_eq!(first["stopReason"], "stop");

    // The provider saw the catalog's endpoint, never the script's.
    let seen = rig.observed.classify_inputs.lock().unwrap();
    assert_eq!(seen.len(), 6);
    assert!(
        seen.iter()
            .all(|(base_url, _)| base_url == "https://classifier.test/v1")
    );
    // Six classifications with at most four in flight.
    assert_eq!(rig.observed.max_active.load(Ordering::SeqCst), 4);

    let calls = rig.recorder.snapshot().calls;
    assert_eq!(calls.len(), 6);
    for call in &calls {
        assert_eq!(
            (
                call.name.as_str(),
                call.args.as_str(),
                call.status,
                call.cost
            ),
            (
                "models.classify",
                "scorer/judge",
                CodemodeNestedCallStatus::Ok,
                Some(0.001)
            )
        );
        assert!(call.duration_ms.is_some());
    }
    // `${toolCallId}/${name}/${n}`.
    assert_eq!(calls[0].id, "call-1/models.classify/1");
    // The classifications' usage becomes the codemode result's usage.
    let total = rig.recorder.model_usage().unwrap();
    assert_eq!(total.input, 1800);
    assert!((total.cost.total - 0.006).abs() < 1e-10);
}

/// Upstream `generates images with catalog auth and attaches them through image()`.
#[tokio::test]
async fn generates_images_with_catalog_auth_and_reports_provider_errors_as_results() {
    let rig = rig();
    let available = rig
        .ok(
            "models.getAvailableOfType",
            vec![json!("image"), json!("scorer")],
        )
        .await;
    let model = available[0].clone();
    let mut evil = model.clone();
    evil["baseUrl"] = json!("https://evil.test");
    let reference = json!({ "type": "image", "data": TINY_PNG, "mimeType": "image/png" });

    let generated = rig
        .ok(
            "models.generateImages",
            vec![
                evil,
                json!({ "input": [{ "type": "text", "text": "a fox" }, reference.clone()] }),
            ],
        )
        .await;
    assert_eq!(generated["stopReason"], "stop");
    assert_eq!(
        generated["output"],
        json!([
            { "type": "text", "text": "painted a fox" },
            { "type": "image", "data": TINY_PNG, "mimeType": "image/png" },
        ])
    );
    let failed = rig
        .ok(
            "models.generateImages",
            vec![
                model,
                json!({ "input": [{ "type": "text", "text": "explode" }] }),
            ],
        )
        .await;
    assert_eq!(failed["stopReason"], "error");
    assert_eq!(failed["errorMessage"], "painter exploded");

    // A classifier is not an image model.
    assert_eq!(
        rig.err(
            "models.generateImages",
            vec![
                json!({ "provider": "scorer", "id": "judge" }),
                json!({ "input": [] })
            ]
        )
        .await,
        "\"scorer/judge\" is a classifier model, not an image model. List the image models you can use with models.getAvailableOfType(\"image\")."
    );

    let seen = rig.observed.image_inputs.lock().unwrap();
    assert!(
        seen.iter()
            .all(|(base_url, _)| base_url == "https://images.test/v1")
    );
    assert_eq!(
        seen[0].1,
        vec![
            Content::text("a fox"),
            Content::Image {
                data: TINY_PNG.to_owned(),
                mime_type: "image/png".to_owned()
            }
        ]
    );
    let calls = rig.recorder.snapshot().calls;
    assert_eq!(
        calls
            .iter()
            .map(|c| (
                c.name.as_str(),
                c.args.as_str(),
                c.status,
                c.cost,
                c.error.as_deref()
            ))
            .collect::<Vec<_>>(),
        [
            (
                "models.generateImages",
                "scorer/painter",
                CodemodeNestedCallStatus::Ok,
                Some(0.04),
                None
            ),
            (
                "models.generateImages",
                "scorer/painter",
                CodemodeNestedCallStatus::Error,
                None,
                Some("painter exploded")
            ),
        ]
    );
    assert!((rig.recorder.model_usage().unwrap().cost.total - 0.04).abs() < 1e-10);
    // One image from the first call; the failed call returned none.
    assert_eq!(rig.recorder.generated_images(), 1);
}

/// Upstream `reports provider errors as results and invalid arguments as exceptions`.
#[tokio::test]
async fn reports_provider_errors_as_results_and_invalid_arguments_as_exceptions() {
    let rig = rig();
    let model = rig
        .ok(
            "models.getModelOfType",
            vec![json!("classifier"), json!("scorer"), json!("judge")],
        )
        .await;

    let failed = rig
        .ok(
            "models.classify",
            vec![
                model.clone(),
                json!({ "state": { "text": "explode" }, "questions": questions() }),
            ],
        )
        .await;
    assert_eq!(failed["stopReason"], "error");
    assert_eq!(failed["errorMessage"], "classifier exploded");

    assert!(
        rig.err("models.getModelsOfType", vec![json!("video")])
            .await
            .contains("Unknown model type \"video\"")
    );
    assert_eq!(
        rig.err(
            "models.classify",
            vec![json!({ "provider": "scorer", "id": "nope" }), json!({})]
        )
        .await,
        "Unknown classifier model \"scorer/nope\". List the classifier models you can use with models.getAvailableOfType(\"classifier\")."
    );
    assert!(
        rig.err("models.classify", vec![json!("judge"), json!({})])
            .await
            .contains(
                "models.classify() expects a classifier model as its first argument, got a string."
            )
    );
    // `undefined` arrives as null.
    assert!(
        rig.err("models.classify", vec![Value::Null, json!({})])
            .await
            .contains("models.getModelOfType() returns undefined for an unknown provider or id.")
    );
    let no_state = rig
        .err(
            "models.classify",
            vec![model.clone(), json!({ "questions": questions() })],
        )
        .await;
    assert!(no_state.contains("models.classify() context.state must be an object, got undefined."));
    assert!(no_state.contains("codemode.md"));
    assert!(no_state.contains("Expected context: { state: { ... }, questions:"));
    assert!(
        rig.err(
            "models.classify",
            vec![
                model.clone(),
                json!({ "state": {}, "questions": { "kind": { "type": "choice", "instructions": "Kind?", "criteria": ["a", "b"] } } })
            ]
        )
        .await
        .contains("context.questions.kind is a \"choice\" question, so criteria must map each label to its meaning.")
    );
    let painter = json!({ "provider": "scorer", "id": "painter" });
    assert!(
        rig.err("models.generateImages", vec![painter, json!({ "prompt": "a fox" })])
            .await
            .contains("models.generateImages() context.input must be a non-empty array of blocks, got undefined.")
    );
    assert!(
        rig.err(
            "models.getModelOfType",
            vec![json!("classifier"), json!("scorer/judge")]
        )
        .await
        .contains("The provider and the id are separate arguments")
    );

    // Only the call that reached the provider is a row, and nothing but its usage-less error.
    let calls = rig.recorder.snapshot().calls;
    assert_eq!(
        calls
            .iter()
            .map(|c| (c.name.as_str(), c.status, c.error.as_deref()))
            .collect::<Vec<_>>(),
        [(
            "models.classify",
            CodemodeNestedCallStatus::Error,
            Some("classifier exploded")
        )]
    );
    assert!(rig.recorder.model_usage().is_none());
}

/// The context checks fail with the expected shape before anything reaches a provider.
#[tokio::test]
async fn context_mistakes_never_reach_the_provider() {
    let rig = rig();
    let model = json!({ "provider": "scorer", "id": "judge" });
    for (context, expected) in [
        (
            json!(null),
            "expects a context object as its second argument, got null",
        ),
        (
            json!([1]),
            "expects a context object as its second argument, got an array",
        ),
        (
            json!({ "state": [], "questions": {} }),
            "context.state must be an object, got an empty array",
        ),
        (
            json!({ "state": {}, "questions": {} }),
            "context.questions must map question IDs to questions, got {}",
        ),
        (
            json!({ "state": {}, "questions": { "q": 5 } }),
            "context.questions.q must be a question object, got a number",
        ),
        (
            json!({ "state": {}, "questions": { "q": { "type": "bool" } } }),
            "context.questions.q.instructions must be a string",
        ),
        (
            json!({ "state": {}, "questions": { "q": { "type": "score", "instructions": "i", "criteria": [] } } }),
            "context.questions.q is a \"score\" question, so criteria must list the levels as strings, lowest first",
        ),
        (
            json!({ "state": {}, "questions": { "q": { "type": "bool", "instructions": "i", "criteria": { "true": "y" } } } }),
            "context.questions.q is a \"bool\" question, so criteria must be { true: string, false: string }",
        ),
        (
            json!({ "state": {}, "questions": { "q": { "type": "rank", "instructions": "i", "criteria": {} } } }),
            "context.questions.q.type must be \"choice\", \"score\", or \"bool\", got \"rank\"",
        ),
        (
            json!({ "state": {}, "questions": { "q": { "instructions": "i", "criteria": {} } } }),
            "context.questions.q.type must be \"choice\", \"score\", or \"bool\", got undefined",
        ),
    ] {
        let message = rig
            .err("models.classify", vec![model.clone(), context])
            .await;
        assert!(
            message.starts_with(&format!("models.classify() {expected}. Expected context: ")),
            "{message}"
        );
        assert!(
            message.ends_with(&format!(". See \"Classify\" in {CODEMODE_DOCS_PATH}.")),
            "{message}"
        );
    }
    let painter = json!({ "provider": "scorer", "id": "painter" });
    for (context, expected) in [
        (
            json!(1),
            "expects a context object as its second argument, got a number",
        ),
        (
            json!({ "input": [] }),
            "context.input must be a non-empty array of blocks, got an empty array",
        ),
        (
            json!({ "input": [{ "type": "text", "text": "ok" }, { "type": "video" }] }),
            "context.input[1] must be a text or image block, got { type }",
        ),
        (
            json!({ "input": [{ "type": "image", "data": "x" }] }),
            "context.input[0] must be a text or image block, got { type, data }",
        ),
    ] {
        let message = rig
            .err("models.generateImages", vec![painter.clone(), context])
            .await;
        assert!(
            message.starts_with(&format!(
                "models.generateImages() {expected}. Expected context: "
            )),
            "{message}"
        );
        assert!(
            message.ends_with(&format!(
                ". See \"Generate images\" in {CODEMODE_DOCS_PATH}."
            )),
            "{message}"
        );
    }
    assert!(
        rig.models.calls.lock().unwrap().is_empty(),
        "no provider was called"
    );
    assert!(rig.recorder.snapshot().calls.is_empty(), "no row either");
    assert!(rig.updates.lock().unwrap().is_empty());
}

#[tokio::test]
async fn type_and_provider_arguments_are_validated() {
    let rig = rig();
    assert_eq!(
        rig.err("models.getModelsOfType", vec![]).await,
        "Unknown model type undefined. Use \"chat\", \"image\", or \"classifier\"."
    );
    assert_eq!(
        rig.err("models.getModelsOfType", vec![json!(3)]).await,
        "Unknown model type 3. Use \"chat\", \"image\", or \"classifier\"."
    );
    assert_eq!(
        rig.err(
            "models.getModelsOfType",
            vec![json!("classifier"), json!(4)]
        )
        .await,
        "provider must be a string"
    );
    // A null provider is "all providers".
    assert_eq!(
        rig.ok("models.getModelsOfType", vec![json!("image"), Value::Null])
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        rig.err(
            "models.getModelOfType",
            vec![json!("classifier"), json!("scorer")]
        )
        .await,
        "models.getModelOfType(type, provider, id) expects three strings, got (a string, a string). The provider and the id are separate arguments, for example models.getModelOfType(\"classifier\", \"typesafe\", \"jev-latest\")."
    );
    assert_eq!(
        rig.err(
            "models.getModelOfType",
            vec![json!({ "provider": "p", "id": "i" }), json!(1)]
        )
        .await,
        "models.getModelOfType(type, provider, id) expects three strings, got ({ provider, id }, a number). The provider and the id are separate arguments, for example models.getModelOfType(\"classifier\", \"typesafe\", \"jev-latest\")."
    );
    // `getAvailableOfType` lists what the registry reports available.
    let rig2 = rig;
    assert_eq!(
        rig2.ok("models.getAvailableOfType", vec![json!("chat")])
            .await
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

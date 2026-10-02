//! The classifier type surface (`packages/ai/src/types.ts` @v0.99.2-17): wire shapes, key order,
//! options defaults and hooks, the api registry, and the model-type tags.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use super::classifier_support::{
    LLAMA_API, RecordingClassifier, chat_model, classifier_model, empty_context,
};
use crate::classifier::{
    AnyModel, ClassifierAnswer, ClassifierApiRegistry, ClassifierContext, ClassifierModel,
    ClassifierOptions, ClassifierQuestion, ClassifierResult, ClassifierStopReason,
    DEFAULT_MAX_RETRIES, DEFAULT_TEMPERATURE, KnownClassifierApi, ModelType, OrderedMap,
    ProviderClassifier,
};
use crate::stream::ProviderResponse;
use cyrup_core::{ApiId, CancelToken};
use serde_json::json;

// ------------------------------------------------------------------------------- model types --

#[test]
fn known_classifier_api_names_the_llama_api() {
    assert_eq!(
        KnownClassifierApi::LlamaCppClassify.as_str(),
        "llama-cpp-classify"
    );
    assert_eq!(
        KnownClassifierApi::from_api("llama-cpp-classify"),
        Some(KnownClassifierApi::LlamaCppClassify)
    );
    assert_eq!(KnownClassifierApi::from_api("typesafe-system-one"), None);
    assert_eq!(
        ApiId::from(KnownClassifierApi::LlamaCppClassify).as_str(),
        "llama-cpp-classify"
    );
}

/// `ClassifierModel.type` is the literal `"classifier"` (types.ts:1153); the rest is `BaseModel`
/// in camelCase (types.ts:1097-1108) plus `contextWindow` (:1154).
#[test]
fn classifier_model_wire_shape() {
    let mut model = classifier_model("llama.cpp", "qwen", LLAMA_API);
    model.headers = Some([("x-a".to_string(), Some("1".to_string()))].into());

    let value = serde_json::to_value(&model).unwrap();

    assert_eq!(value["type"], "classifier");
    assert_eq!(value["id"], "qwen");
    assert_eq!(value["api"], LLAMA_API);
    assert_eq!(value["provider"], "llama.cpp");
    assert_eq!(value["baseUrl"], "http://model.test/v1");
    assert_eq!(value["contextWindow"], 4096);
    assert_eq!(value["input"], json!(["text"]));
    assert_eq!(value["headers"], json!({"x-a": "1"}));
    let back: ClassifierModel = serde_json::from_value(value).unwrap();
    assert_eq!(back, model);
}

/// A chat model is not a classifier model: the tag is checked on the way in.
#[test]
fn classifier_model_rejects_another_type_tag() {
    let mut value = serde_json::to_value(classifier_model("p", "c", LLAMA_API)).unwrap();
    value["type"] = json!("chat");
    assert!(serde_json::from_value::<ClassifierModel>(value).is_err());
}

/// The `type` member is required on read: a classifier model's JSON without it is refused, so a
/// chat model's JSON (which carries no `type`) cannot be read as a classifier model.
#[test]
fn classifier_model_requires_its_type_tag() {
    let mut value = serde_json::to_value(classifier_model("p", "c", LLAMA_API)).unwrap();
    value.as_object_mut().unwrap().remove("type");
    assert!(serde_json::from_value::<ClassifierModel>(value).is_err());

    let chat_json = serde_json::to_value(chat_model("p", "m")).unwrap();
    assert!(serde_json::from_value::<ClassifierModel>(chat_json).is_err());
}

/// `BaseModel.headers` is optional on the wire: a model without headers writes no `headers`
/// member, and reads back without one.
#[test]
fn classifier_model_without_headers_writes_no_headers_member() {
    let model = classifier_model("p", "c", LLAMA_API);
    let value = serde_json::to_value(&model).unwrap();
    assert!(value.get("headers").is_none(), "{value}");
    let back: ClassifierModel = serde_json::from_value(value).unwrap();
    assert_eq!(back.headers, None);
}

/// The chat-[`Model`] shim auth resolution reads carries the classifier model's identity, base
/// URL, input, cost, context window and headers, and none of the chat-only capabilities.
#[test]
fn the_auth_shim_carries_what_auth_strategies_read() {
    let mut model = classifier_model("llama.cpp", "qwen", LLAMA_API);
    model.headers = Some([("x-a".to_string(), Some("1".to_string()))].into());

    let shim = model.to_auth_model();

    assert_eq!(shim.id, model.id);
    assert_eq!(shim.name, model.name);
    assert_eq!(shim.api, model.api);
    assert_eq!(shim.provider, model.provider);
    assert_eq!(shim.base_url, "http://model.test/v1");
    assert_eq!(shim.input, model.input);
    assert_eq!(shim.context_window, 4096);
    assert_eq!(shim.headers, model.headers);
    assert!(!shim.reasoning);
    assert_eq!(shim.max_tokens, 0);
}

/// `getModelType` (`model-operations.ts:17-19`): no `type` means chat; `classifier` routes to the
/// classifier shape; a type this build does not know is an error, not a chat model.
#[test]
fn any_model_routes_by_type_tag() {
    let chat = chat_model("p", "m");
    let chat_json = serde_json::to_value(&chat).unwrap();
    assert!(
        chat_json.get("type").is_none(),
        "a chat model carries no `type`"
    );
    let classifier = classifier_model("p", "c", LLAMA_API);

    let parsed_chat: AnyModel = serde_json::from_value(chat_json).unwrap();
    let parsed_classifier: AnyModel =
        serde_json::from_value(serde_json::to_value(&classifier).unwrap()).unwrap();

    assert_eq!(parsed_chat, AnyModel::Chat(chat));
    assert_eq!(parsed_classifier, AnyModel::Classifier(classifier));
    assert_eq!(parsed_chat.model_type(), ModelType::Chat);
    assert_eq!(parsed_classifier.model_type(), ModelType::Classifier);
    assert_eq!(parsed_classifier.as_chat(), None);
    assert!(parsed_classifier.as_classifier().is_some());

    let mut image = serde_json::to_value(chat_model("p", "i")).unwrap();
    image["type"] = json!("image");
    assert!(serde_json::from_value::<AnyModel>(image).is_err());
}

#[test]
fn model_type_spelling() {
    assert_eq!(ModelType::Chat.to_string(), "chat");
    assert_eq!(ModelType::Classifier.to_string(), "classifier");
    assert_eq!(
        serde_json::to_value(ModelType::Classifier).unwrap(),
        json!("classifier")
    );
    assert_eq!(ModelType::ALL.len(), 2);
}

// ----------------------------------------------------------------------------- ordered objects --

/// The order of a choice question's `criteria` decides which option gets which letter, and the
/// order of `questions` is the order they are asked (`llama-cpp-classify.ts:105-110`, `:445`).
#[test]
fn context_preserves_key_order_from_json() {
    let wire = r#"{
        "state": {"zeta": 1, "alpha": {"b": 2, "a": 1}},
        "questions": {
            "second": {"type": "bool", "instructions": "Is it?", "criteria": {"true": "yes", "false": "no"}},
            "first": {"type": "choice", "instructions": "Which?", "criteria": {"z": "last letter", "a": "first letter", "m": "middle"}},
            "third": {"type": "score", "instructions": "How much?", "criteria": ["none", "some", "lots"]}
        }
    }"#;

    let context: ClassifierContext = serde_json::from_str(wire).unwrap();

    assert_eq!(context.state.keys().collect::<Vec<_>>(), ["zeta", "alpha"]);
    assert_eq!(
        context.questions.keys().collect::<Vec<_>>(),
        ["second", "first", "third"]
    );
    let Some(ClassifierQuestion::Choice {
        criteria,
        instructions,
    }) = context.questions.get("first")
    else {
        panic!("first is a choice question");
    };
    assert_eq!(instructions, "Which?");
    assert_eq!(criteria.keys().collect::<Vec<_>>(), ["z", "a", "m"]);
    let Some(ClassifierQuestion::Bool { criteria, .. }) = context.questions.get("second") else {
        panic!("second is a bool question");
    };
    assert_eq!(
        (criteria.when_true.as_str(), criteria.when_false.as_str()),
        ("yes", "no")
    );
    let Some(ClassifierQuestion::Score { criteria, .. }) = context.questions.get("third") else {
        panic!("third is a score question");
    };
    assert_eq!(criteria, &["none", "some", "lots"]);

    // And the order survives going back out.
    let out = serde_json::to_string(&context).unwrap();
    let questions = &out[out.find("\"questions\"").unwrap()..];
    assert!(questions.find("\"second\"").unwrap() < questions.find("\"first\"").unwrap());
    assert!(questions.find("\"z\":").unwrap() < questions.find("\"a\":").unwrap());
    let again: ClassifierContext = serde_json::from_str(&out).unwrap();
    assert_eq!(again, context);
}

/// Assigning to an existing JS object key replaces the value in place.
#[test]
fn ordered_map_replaces_in_place() {
    let mut map = OrderedMap::new();
    assert_eq!(map.insert("a", 1), None);
    map.insert("b", 2);
    assert_eq!(map.insert("a", 9), Some(1));

    assert_eq!(map.iter().collect::<Vec<_>>(), [("a", &9), ("b", &2)]);
    assert_eq!(map.len(), 2);
    assert!(map.contains_key("b"));
    assert_eq!(map.get("zzz"), None);

    let collected: OrderedMap<i32> = [("x", 1), ("y", 2), ("x", 3)].into_iter().collect();
    assert_eq!(collected.iter().collect::<Vec<_>>(), [("x", &3), ("y", &2)]);
}

/// A JS object lists canonical array-index keys first, ascending, then the other keys in insertion
/// order (`OrdinaryOwnPropertyKeys`). Non-canonical numerals (`"01"`, `"-1"`, `"1.5"`, the empty
/// string, 2^32 - 1 and above) are ordinary keys.
#[test]
fn ordered_map_hoists_integer_like_keys_the_way_js_objects_do() {
    let mut map = OrderedMap::new();
    for key in [
        "b",
        "10",
        "a",
        "2",
        "01",
        "-1",
        "1.5",
        "",
        "4294967295",
        "4294967294",
        "0",
    ] {
        map.insert(key, ());
    }

    assert_eq!(
        map.keys().collect::<Vec<_>>(),
        [
            "0",
            "2",
            "10",
            "4294967294",
            "b",
            "a",
            "01",
            "-1",
            "1.5",
            "",
            "4294967295"
        ]
    );
}

/// The hoist applies to what comes off the wire too: `{"b": .., "1": ..}` is `1`, `b` in pi, so
/// the first label goes to `1`, and it survives going back out.
#[test]
fn ordered_map_hoists_on_deserialize_and_serialize_in_that_order() {
    let map: OrderedMap<i32> = serde_json::from_str(r#"{"b": 1, "1": 2, "a": 3, "0": 4}"#).unwrap();

    assert_eq!(map.keys().collect::<Vec<_>>(), ["0", "1", "b", "a"]);
    assert_eq!(
        serde_json::to_string(&map).unwrap(),
        r#"{"0":4,"1":2,"b":1,"a":3}"#
    );
}

// ------------------------------------------------------------------------------------- results --

/// `ClassifierAnswer` shapes (types.ts:658-676) and `ClassifierResult` (:679-688).
#[test]
fn result_wire_shape() {
    let model = classifier_model("p", "c", LLAMA_API);
    let mut result = ClassifierResult::new(&model);
    result.answers.insert(
        "which",
        ClassifierAnswer::Choice {
            choice: "b".into(),
            probabilities: [("a", 0.25), ("b", 0.75)].into_iter().collect(),
            confidence: 0.5,
        },
    );
    result.answers.insert(
        "how",
        ClassifierAnswer::Score {
            score: 1.5,
            confidence: 0.4,
        },
    );
    result
        .answers
        .insert("is", ClassifierAnswer::Bool { probability: 0.9 });

    let value = serde_json::to_value(&result).unwrap();

    assert_eq!(value["api"], LLAMA_API);
    assert_eq!(value["provider"], "p");
    assert_eq!(value["model"], "c");
    assert_eq!(value["stopReason"], "stop");
    assert!(value.get("errorMessage").is_none());
    assert!(value.get("usage").is_none());
    assert_eq!(
        value["answers"]["which"],
        json!({"type": "choice", "choice": "b", "probabilities": {"a": 0.25, "b": 0.75}, "confidence": 0.5})
    );
    assert_eq!(
        value["answers"]["how"],
        json!({"type": "score", "score": 1.5, "confidence": 0.4})
    );
    assert_eq!(
        value["answers"]["is"],
        json!({"type": "bool", "probability": 0.9})
    );
    let back: ClassifierResult = serde_json::from_value(value).unwrap();
    assert_eq!(back, result);
}

/// `classifierErrorResult` (`model-operations.ts:56-70`): no answers, the stop reason follows
/// `aborted`, and the message is carried.
#[test]
fn errored_result_follows_the_aborted_flag() {
    let model = classifier_model("p", "c", LLAMA_API);

    let failed = ClassifierResult::errored(&model, "boom", false);
    let aborted = ClassifierResult::errored(&model, "stopped", true);
    let fresh = ClassifierResult::new(&model);

    assert_eq!(failed.stop_reason, ClassifierStopReason::Error);
    assert_eq!(failed.error_message.as_deref(), Some("boom"));
    assert!(failed.answers.is_empty());
    assert_eq!(aborted.stop_reason, ClassifierStopReason::Aborted);
    assert_eq!(
        serde_json::to_value(&aborted).unwrap()["stopReason"],
        "aborted"
    );
    assert_eq!(fresh.stop_reason, ClassifierStopReason::Stop);
    assert!(fresh.error_message.is_none());
}

// ------------------------------------------------------------------------------------ options --

/// `options?.temperature ?? 1` and `options?.maxRetries ?? 2` (`llama-cpp-classify.ts:437`,
/// `:265`) are the defaults an api reads directly.
#[test]
fn options_defaults() {
    let options = ClassifierOptions::default();

    assert_eq!(options.temperature, 1.0);
    assert_eq!(options.temperature, DEFAULT_TEMPERATURE);
    assert_eq!(options.max_retries, 2);
    assert_eq!(options.max_retries, DEFAULT_MAX_RETRIES);
    assert!(options.timeout_ms.is_none());
    assert!(options.api_key.is_none());
    assert!(!options.is_aborted());
}

#[test]
fn options_report_cancellation() {
    let cancel = CancelToken::new();
    let options = ClassifierOptions {
        cancel: Some(cancel.clone()),
        ..ClassifierOptions::default()
    };
    assert!(!options.is_aborted());
    cancel.cancel();
    assert!(options.is_aborted());
}

/// `const transformed = await options?.onPayload?.(payload, model); if (transformed !==
/// undefined) payload = transformed` (`llama-cpp-classify.ts:231-232`).
#[tokio::test]
async fn on_payload_replaces_or_keeps_the_payload() {
    let model = classifier_model("p", "c", LLAMA_API);
    let seen_model = Arc::new(Mutex::new(String::new()));

    let none = ClassifierOptions::default();
    assert_eq!(
        none.apply_on_payload(&model, json!({"a": 1})).await,
        json!({"a": 1})
    );

    let keep = ClassifierOptions {
        on_payload: Some(Arc::new(|_payload, _model| Box::pin(async { None }))),
        ..ClassifierOptions::default()
    };
    assert_eq!(
        keep.apply_on_payload(&model, json!({"a": 1})).await,
        json!({"a": 1})
    );

    let sink = seen_model.clone();
    let replace = ClassifierOptions {
        on_payload: Some(Arc::new(move |payload, model| {
            let sink = sink.clone();
            Box::pin(async move {
                *sink.lock().unwrap() = model.id.as_str().to_string();
                Some(json!({"was": payload}))
            })
        })),
        ..ClassifierOptions::default()
    };
    assert_eq!(
        replace.apply_on_payload(&model, json!({"a": 1})).await,
        json!({"was": {"a": 1}})
    );
    assert_eq!(
        &*seen_model.lock().unwrap(),
        "c",
        "the hook is handed the model"
    );
}

/// `await options?.onResponse?.({ status, headers }, model)` (`llama-cpp-classify.ts:268`).
#[tokio::test]
async fn on_response_is_invoked_with_status_and_model() {
    let model = classifier_model("p", "c", LLAMA_API);
    let seen = Arc::new(Mutex::new(Vec::<(u16, String)>::new()));
    let sink = seen.clone();
    let options = ClassifierOptions {
        on_response: Some(Arc::new(move |response, model| {
            let sink = sink.clone();
            Box::pin(async move {
                sink.lock()
                    .unwrap()
                    .push((response.status, model.id.as_str().to_string()));
            })
        })),
        ..ClassifierOptions::default()
    };

    options
        .emit_on_response(
            &model,
            ProviderResponse {
                status: 200,
                ..ProviderResponse::default()
            },
        )
        .await;
    ClassifierOptions::default()
        .emit_on_response(&model, ProviderResponse::default())
        .await;

    assert_eq!(&*seen.lock().unwrap(), &[(200, "c".to_string())]);
}

// ------------------------------------------------------------------------------------ registry --

/// pi `createProvider({ classifiers })` (`models.ts:1161-1171`): dispatch on `model.api`.
#[tokio::test]
async fn registry_dispatches_on_the_models_api() {
    let llama = RecordingClassifier::new("llama");
    let other = RecordingClassifier::new("other");
    let mut registry = ClassifierApiRegistry::new();
    registry.register(
        KnownClassifierApi::LlamaCppClassify,
        llama.clone() as Arc<dyn ProviderClassifier>,
    );
    registry.register("custom-api", other.clone() as Arc<dyn ProviderClassifier>);

    let from_llama = registry
        .classify(
            &classifier_model("p", "c", LLAMA_API),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;
    let from_other = registry
        .classify(
            &classifier_model("p", "c", "custom-api"),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;

    assert_eq!(from_llama.answers.keys().collect::<Vec<_>>(), ["llama"]);
    assert_eq!(from_other.answers.keys().collect::<Vec<_>>(), ["other"]);
    assert_eq!(registry.apis(), ["custom-api", "llama-cpp-classify"]);
    assert!(registry.contains(LLAMA_API));
    assert!(registry.get("nope").is_none());
    assert!(!registry.is_empty());
    assert!(ClassifierApiRegistry::new().is_empty());
}

/// Registering an api again replaces its implementation (a `Record` assignment), and a registry
/// holding only a fallback is not empty.
#[tokio::test]
async fn registry_replaces_a_registered_api_and_counts_a_fallback_as_content() {
    let first = RecordingClassifier::new("first");
    let second = RecordingClassifier::new("second");
    let mut registry = ClassifierApiRegistry::new();
    registry.register(LLAMA_API, first as Arc<dyn ProviderClassifier>);
    registry.register(LLAMA_API, second as Arc<dyn ProviderClassifier>);

    let result = registry
        .classify(
            &classifier_model("p", "c", LLAMA_API),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;

    assert_eq!(result.answers.keys().collect::<Vec<_>>(), ["second"]);
    assert_eq!(registry.apis(), [LLAMA_API]);

    let only_fallback = ClassifierApiRegistry::new()
        .with_fallback(RecordingClassifier::new("fallback") as Arc<dyn ProviderClassifier>);
    assert!(!only_fallback.is_empty());
    assert!(only_fallback.apis().is_empty());
}

/// An api with no entry and no fallback: `Provider ${id} has no classifier implementation for
/// "${model.api}"` (`models.ts:1167`).
#[tokio::test]
async fn registry_without_a_match_is_an_error_result() {
    let mut registry = ClassifierApiRegistry::new();
    registry.register(
        LLAMA_API,
        RecordingClassifier::new("llama") as Arc<dyn ProviderClassifier>,
    );

    let result = registry
        .classify(
            &classifier_model("p", "c", "other-api"),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Provider p has no classifier implementation for \"other-api\"")
    );
    assert!(result.answers.is_empty());
}

/// `composeProvider` (`provider-composer.ts:652-664`): an extension's implementation for the api
/// wins; the base provider's `classify` answers every other api.
#[tokio::test]
async fn registry_consults_its_entries_before_the_fallback() {
    let base = RecordingClassifier::new("base");
    let extension = RecordingClassifier::new("extension");
    let mut registry =
        ClassifierApiRegistry::new().with_fallback(base.clone() as Arc<dyn ProviderClassifier>);
    registry.register(LLAMA_API, extension.clone() as Arc<dyn ProviderClassifier>);

    let overridden = registry
        .classify(
            &classifier_model("p", "c", LLAMA_API),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;
    let delegated = registry
        .classify(
            &classifier_model("p", "c", "other-api"),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;

    assert_eq!(overridden.answers.keys().collect::<Vec<_>>(), ["extension"]);
    assert_eq!(delegated.answers.keys().collect::<Vec<_>>(), ["base"]);
    assert_eq!(base.seen().calls, 1);
    assert_eq!(extension.seen().calls, 1);
}

//! `classify` dispatch (pi `Models.classify`, `packages/ai/src/models.ts:966-982` @v0.99.2-17, and
//! the registry pass-through `core/model-registry.ts:168-173`): the default
//! [`crate::Provider::classify`], routing to the owning provider, auth application, and the
//! multi-type model reads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use super::classifier_support::{
    ClassifyingProvider, FixedAuth, LLAMA_API, PlainProvider, RecordingClassifier,
    UnconfiguredAuth, chat_model, classifier_model, empty_context,
};
use crate::auth::ProviderAuth;
use crate::classifier::{
    AnyModel, ClassifierApiRegistry, ClassifierOptions, ClassifierStopReason, ModelType,
    ProviderClassifier,
};
use crate::provider::Provider;
use crate::{CreateModelsOptions, HeaderMap, Models, ProviderEnv, create_models};
use cyrup_core::CancelToken;

fn models_with(providers: Vec<Arc<dyn Provider>>) -> Models {
    let mut models = create_models(CreateModelsOptions::default());
    for provider in providers {
        models.set_provider(provider);
    }
    models
}

fn registry_of(classifier: &Arc<RecordingClassifier>) -> ClassifierApiRegistry {
    let mut registry = ClassifierApiRegistry::new();
    registry.register(LLAMA_API, classifier.clone() as Arc<dyn ProviderClassifier>);
    registry
}

fn headers(pairs: &[(&str, Option<&str>)]) -> HeaderMap {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.map(str::to_string)))
        .collect()
}

// ------------------------------------------------------------------ the default `classify` --

/// pi's `classify?` is absent on a provider without classifier models, and `Models.classify` turns
/// that into `Provider ${model.provider} does not support classification` (`models.ts:974-976`).
/// The trait default is that absent case: an error result, never a panic, never an `Err`.
#[tokio::test]
async fn default_provider_classify_is_an_error_result_naming_the_provider() {
    let provider = PlainProvider::new("plain", vec![chat_model("plain", "m")]);
    let model = classifier_model("plain", "c", LLAMA_API);

    let result = provider
        .classify(&model, &empty_context(), &ClassifierOptions::default())
        .await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Provider plain does not support classification")
    );
    assert!(result.answers.is_empty());
    // The envelope identifies the request it answers (`classifierErrorResult`,
    // `model-operations.ts:56-70`).
    assert_eq!(result.api.as_str(), LLAMA_API);
    assert_eq!(result.provider.as_str(), "plain");
    assert_eq!(result.model, "c");
    assert!(result.timestamp > 0);
}

/// `models.ts:973-980`: the `does not support classification` throw sits inside the `try` whose
/// `catch` is `classifierErrorResult(model, error, options?.signal?.aborted)`, so the default is
/// `aborted` when the request was already cancelled.
#[tokio::test]
async fn default_provider_classify_is_aborted_when_the_request_was_cancelled() {
    let provider = PlainProvider::new("plain", vec![chat_model("plain", "m")]);
    let model = classifier_model("plain", "c", LLAMA_API);
    let cancel = CancelToken::new();
    cancel.cancel();
    let options = ClassifierOptions {
        cancel: Some(cancel),
        ..ClassifierOptions::default()
    };

    let result = provider.classify(&model, &empty_context(), &options).await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Aborted);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Provider plain does not support classification")
    );
}

/// `Provider::get_all_models`' default is the chat catalog, wrapped (`entry.getAllModels?.() ??
/// entry.getModels()`, `models.ts:451`).
#[test]
fn default_get_all_models_wraps_the_chat_catalog() {
    let provider = PlainProvider::new(
        "plain",
        vec![chat_model("plain", "a"), chat_model("plain", "b")],
    );

    let all = provider.get_all_models();

    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|m| m.model_type() == ModelType::Chat));
    assert_eq!(all.iter().map(AnyModel::id).collect::<Vec<_>>(), ["a", "b"]);
}

// ------------------------------------------------------------------------- `Models::classify` --

#[tokio::test]
async fn models_classify_routes_to_the_provider_that_owns_the_model() {
    let for_a = RecordingClassifier::new("a");
    let for_b = RecordingClassifier::new("b");
    let models = models_with(vec![
        Arc::new(ClassifyingProvider::new(
            "pa",
            vec![],
            vec![classifier_model("pa", "ca", LLAMA_API)],
            None,
            registry_of(&for_a),
        )),
        Arc::new(ClassifyingProvider::new(
            "pb",
            vec![],
            vec![classifier_model("pb", "cb", LLAMA_API)],
            None,
            registry_of(&for_b),
        )),
    ]);

    let result = models
        .classify(
            &classifier_model("pb", "cb", LLAMA_API),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
    assert_eq!(result.answers.keys().collect::<Vec<_>>(), ["b"]);
    assert_eq!(for_b.seen().calls, 1);
    assert_eq!(
        for_a.seen().calls,
        0,
        "the other provider must not be asked"
    );
}

/// pi `requireProvider` (`models.ts:824-829`): `Unknown provider: ${model.provider}`, delivered as
/// an error result (`:979-981`).
#[tokio::test]
async fn models_classify_unknown_provider_is_an_error_result() {
    let models = models_with(vec![Arc::new(PlainProvider::new("plain", vec![]))]);

    let result = models
        .classify(
            &classifier_model("ghost", "c", LLAMA_API),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Unknown provider: ghost")
    );
    assert!(result.answers.is_empty());
    assert_eq!(result.provider.as_str(), "ghost");
    assert_eq!(result.model, "c");
}

/// `classifierErrorResult(model, error, options?.signal?.aborted)` (`models.ts:980`): the same
/// failure is `aborted` when the request was cancelled and `error` when it was not.
#[tokio::test]
async fn models_classify_marks_a_failure_aborted_only_when_cancelled() {
    let models = models_with(vec![]);
    let model = classifier_model("ghost", "c", LLAMA_API);

    let cancel = CancelToken::new();
    cancel.cancel();
    let cancelled = ClassifierOptions {
        cancel: Some(cancel),
        ..ClassifierOptions::default()
    };

    let aborted = models.classify(&model, &empty_context(), &cancelled).await;
    let failed = models
        .classify(&model, &empty_context(), &ClassifierOptions::default())
        .await;

    assert_eq!(aborted.stop_reason, ClassifierStopReason::Aborted);
    assert_eq!(failed.stop_reason, ClassifierStopReason::Error);
}

/// A registered provider that has no classifier support answers with the trait default through
/// `Models::classify`, not with an unknown-provider error.
#[tokio::test]
async fn models_classify_through_a_provider_without_support_reports_unsupported() {
    let models = models_with(vec![Arc::new(PlainProvider::new("plain", vec![]))]);

    let result = models
        .classify(
            &classifier_model("plain", "c", LLAMA_API),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Provider plain does not support classification")
    );
}

/// pi `applyAuth` (`models.ts:853-855`): a provider whose auth resolves to nothing is
/// `Provider is not configured: ...`, and the api implementation is never reached.
#[tokio::test]
async fn models_classify_requires_a_configured_provider() {
    let recorder = RecordingClassifier::new("x");
    let models = models_with(vec![Arc::new(ClassifyingProvider::new(
        "p",
        vec![],
        vec![classifier_model("p", "c", LLAMA_API)],
        Some(ProviderAuth::with_api_key(Arc::new(UnconfiguredAuth))),
        registry_of(&recorder),
    ))]);

    let result = models
        .classify(
            &classifier_model("p", "c", LLAMA_API),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Provider is not configured: p")
    );
    assert_eq!(recorder.seen().calls, 0);
}

/// `models.ts:972-977`: support is checked BEFORE `applyAuth`. A provider that cannot classify and
/// whose auth would resolve to nothing answers `does not support classification`, not
/// `Provider is not configured` (and so never reads a credential or refreshes a token).
#[tokio::test]
async fn models_classify_checks_support_before_applying_auth() {
    let recorder = RecordingClassifier::new("x");
    // A strategy that is never configured, and no classifier models: no classify member.
    let models = models_with(vec![Arc::new(ClassifyingProvider::new(
        "p",
        vec![chat_model("p", "m")],
        vec![],
        Some(ProviderAuth::with_api_key(Arc::new(UnconfiguredAuth))),
        registry_of(&recorder),
    ))]);

    let result = models
        .classify(
            &classifier_model("p", "c", LLAMA_API),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Provider p does not support classification")
    );
    assert_eq!(recorder.seen().calls, 0);
}

/// The same pre-auth refusal is `aborted` for a cancelled request (`:980`).
#[tokio::test]
async fn models_classify_marks_an_unsupported_provider_aborted_when_cancelled() {
    let models = models_with(vec![Arc::new(PlainProvider::new("plain", vec![]))]);
    let cancel = CancelToken::new();
    cancel.cancel();
    let options = ClassifierOptions {
        cancel: Some(cancel),
        ..ClassifierOptions::default()
    };

    let result = models
        .classify(
            &classifier_model("plain", "c", LLAMA_API),
            &empty_context(),
            &options,
        )
        .await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Aborted);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Provider plain does not support classification")
    );
}

/// `classifierErrorResult(model, error, options?.signal?.aborted)` (`models.ts:980`): an auth
/// failure of a cancelled request is `aborted`, not `error`.
#[tokio::test]
async fn models_classify_marks_an_auth_failure_aborted_when_cancelled() {
    let recorder = RecordingClassifier::new("x");
    let models = models_with(vec![Arc::new(ClassifyingProvider::new(
        "p",
        vec![],
        vec![classifier_model("p", "c", LLAMA_API)],
        Some(ProviderAuth::with_api_key(Arc::new(UnconfiguredAuth))),
        registry_of(&recorder),
    ))]);
    let cancel = CancelToken::new();
    cancel.cancel();
    let options = ClassifierOptions {
        cancel: Some(cancel),
        ..ClassifierOptions::default()
    };

    let result = models
        .classify(
            &classifier_model("p", "c", LLAMA_API),
            &empty_context(),
            &options,
        )
        .await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Aborted);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Provider is not configured: p")
    );
    assert_eq!(recorder.seen().calls, 0);
}

/// pi `applyAuth` (`models.ts:859-868`): the resolved base URL replaces the model's, the resolved
/// key is used when the request has none, and headers and env merge per key with the request side
/// winning.
#[tokio::test]
async fn models_classify_applies_resolved_auth_and_merges_request_options() {
    let recorder = RecordingClassifier::new("x");
    let auth = FixedAuth {
        key: Some("resolved-key"),
        base_url: Some("http://resolved.test"),
        headers: headers(&[("x-from-auth", Some("a")), ("x-shared", Some("auth"))]),
        env: Some(ProviderEnv::from([
            ("FROM_AUTH".to_string(), "1".to_string()),
            ("SHARED".to_string(), "auth".to_string()),
        ])),
    };
    let models = models_with(vec![Arc::new(ClassifyingProvider::new(
        "p",
        vec![],
        vec![classifier_model("p", "c", LLAMA_API)],
        Some(ProviderAuth::with_api_key(Arc::new(auth))),
        registry_of(&recorder),
    ))]);

    let options = ClassifierOptions {
        headers: Some(headers(&[
            ("x-shared", Some("request")),
            ("x-from-request", Some("r")),
        ])),
        env: Some(ProviderEnv::from([
            ("SHARED".to_string(), "request".to_string()),
            ("FROM_REQUEST".to_string(), "1".to_string()),
        ])),
        ..ClassifierOptions::default()
    };
    let result = models
        .classify(
            &classifier_model("p", "c", LLAMA_API),
            &empty_context(),
            &options,
        )
        .await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
    let seen = recorder.seen();
    assert_eq!(seen.base_url.as_deref(), Some("http://resolved.test"));
    assert_eq!(seen.api_key.as_deref(), Some("resolved-key"));
    let seen_headers = seen.headers.unwrap();
    assert_eq!(
        seen_headers.get("x-from-auth"),
        Some(&Some("a".to_string()))
    );
    assert_eq!(
        seen_headers.get("x-from-request"),
        Some(&Some("r".to_string()))
    );
    assert_eq!(
        seen_headers.get("x-shared"),
        Some(&Some("request".to_string()))
    );
    let seen_env = seen.env.unwrap();
    assert_eq!(seen_env.get("FROM_AUTH").map(String::as_str), Some("1"));
    assert_eq!(seen_env.get("FROM_REQUEST").map(String::as_str), Some("1"));
    assert_eq!(seen_env.get("SHARED").map(String::as_str), Some("request"));
}

/// `const apiKey = options?.apiKey ?? auth.apiKey` (`models.ts:859`).
#[tokio::test]
async fn models_classify_prefers_the_explicit_api_key() {
    let recorder = RecordingClassifier::new("x");
    let auth = FixedAuth {
        key: Some("resolved-key"),
        base_url: None,
        headers: HeaderMap::new(),
        env: None,
    };
    let models = models_with(vec![Arc::new(ClassifyingProvider::new(
        "p",
        vec![],
        vec![classifier_model("p", "c", LLAMA_API)],
        Some(ProviderAuth::with_api_key(Arc::new(auth))),
        registry_of(&recorder),
    ))]);

    let options = ClassifierOptions {
        api_key: Some("explicit-key".to_string()),
        ..ClassifierOptions::default()
    };
    models
        .classify(
            &classifier_model("p", "c", LLAMA_API),
            &empty_context(),
            &options,
        )
        .await;

    assert_eq!(recorder.seen().api_key.as_deref(), Some("explicit-key"));
}

/// Neither the resolution nor the request carries an env: the provider sees none, not an empty one
/// (`merge_env` is `None` when both sides are `None`).
#[tokio::test]
async fn models_classify_leaves_env_unset_when_neither_side_has_one() {
    let recorder = RecordingClassifier::new("x");
    let auth = FixedAuth {
        key: Some("resolved-key"),
        base_url: None,
        headers: HeaderMap::new(),
        env: None,
    };
    let models = models_with(vec![Arc::new(ClassifyingProvider::new(
        "p",
        vec![],
        vec![classifier_model("p", "c", LLAMA_API)],
        Some(ProviderAuth::with_api_key(Arc::new(auth))),
        registry_of(&recorder),
    ))]);

    models
        .classify(
            &classifier_model("p", "c", LLAMA_API),
            &empty_context(),
            &ClassifierOptions::default(),
        )
        .await;

    assert_eq!(recorder.seen().env, None);
}

/// pi `applyAuth` (`models.ts:860-861`, `:864`): `transformHeaders` runs LAST over the merged
/// headers, its return value is what the provider receives, and the member itself is stripped.
#[tokio::test]
async fn models_classify_runs_transform_headers_last_and_strips_it() {
    let recorder = RecordingClassifier::new("x");
    let auth = FixedAuth {
        key: None,
        base_url: None,
        headers: headers(&[("x-from-auth", Some("a")), ("x-drop", Some("d"))]),
        env: None,
    };
    let models = models_with(vec![Arc::new(ClassifyingProvider::new(
        "p",
        vec![],
        vec![classifier_model("p", "c", LLAMA_API)],
        Some(ProviderAuth::with_api_key(Arc::new(auth))),
        registry_of(&recorder),
    ))]);

    let transform: crate::TransformHeadersFn = Arc::new(|mut merged: HeaderMap| {
        Box::pin(async move {
            // It is handed the MERGED set: auth header plus request header.
            assert!(merged.contains_key("x-from-auth"));
            assert!(merged.contains_key("x-from-request"));
            merged.remove("x-drop");
            merged.insert("x-added".to_string(), Some("t".to_string()));
            merged
        })
    });
    let options = ClassifierOptions {
        headers: Some(headers(&[("x-from-request", Some("r"))])),
        transform_headers: Some(transform),
        ..ClassifierOptions::default()
    };
    models
        .classify(
            &classifier_model("p", "c", LLAMA_API),
            &empty_context(),
            &options,
        )
        .await;

    let seen = recorder.seen();
    let seen_headers = seen.headers.unwrap();
    assert_eq!(seen_headers.get("x-added"), Some(&Some("t".to_string())));
    assert!(
        !seen_headers.contains_key("x-drop"),
        "a deletion by the transform must hold"
    );
    assert!(
        !seen.had_transform,
        "the transform is Models-only and must not reach the provider"
    );
}

/// `getAuth(model, ...)` folds the model's own headers over the resolved auth headers
/// (`models.ts:746-752`) before `applyAuth` runs `transformHeaders` (`:860-861`): the transform
/// sees the model's header, so a rewrite of it is what the provider receives. The request's own
/// header still wins over the model's.
#[tokio::test]
async fn models_classify_hands_the_models_own_headers_to_the_transform() {
    let recorder = RecordingClassifier::new("x");
    let auth = FixedAuth {
        key: None,
        base_url: None,
        headers: headers(&[("x-from-auth", Some("a")), ("x-shared", Some("auth"))]),
        env: None,
    };
    let models = models_with(vec![Arc::new(ClassifyingProvider::new(
        "p",
        vec![],
        vec![classifier_model("p", "c", LLAMA_API)],
        Some(ProviderAuth::with_api_key(Arc::new(auth))),
        registry_of(&recorder),
    ))]);
    let mut model = classifier_model("p", "c", LLAMA_API);
    model.headers = Some(headers(&[
        ("x-foo", Some("a")),
        ("x-shared", Some("model")),
        ("x-request-wins", Some("model")),
    ]));

    let transform: crate::TransformHeadersFn = Arc::new(|mut merged: HeaderMap| {
        Box::pin(async move {
            let seen = merged.get("x-foo").cloned();
            assert_eq!(
                seen,
                Some(Some("a".to_string())),
                "the transform must see it"
            );
            merged.insert("x-foo".to_string(), Some("rewritten".to_string()));
            merged
        })
    });
    let options = ClassifierOptions {
        headers: Some(headers(&[("x-request-wins", Some("request"))])),
        transform_headers: Some(transform),
        ..ClassifierOptions::default()
    };
    models.classify(&model, &empty_context(), &options).await;

    let seen_headers = recorder.seen().headers.unwrap();
    assert_eq!(
        seen_headers.get("x-foo"),
        Some(&Some("rewritten".to_string()))
    );
    // The model's header overrides the resolved one (`mergeHeaders(auth.headers, model.headers)`).
    assert_eq!(
        seen_headers.get("x-shared"),
        Some(&Some("model".to_string()))
    );
    assert_eq!(
        seen_headers.get("x-from-auth"),
        Some(&Some("a".to_string()))
    );
    assert_eq!(
        seen_headers.get("x-request-wins"),
        Some(&Some("request".to_string()))
    );
}

/// A provider with no auth strategy (it encapsulates its own auth) still hands the model's
/// headers to the transform.
#[tokio::test]
async fn models_classify_hands_the_models_own_headers_to_the_transform_without_a_strategy() {
    let recorder = RecordingClassifier::new("x");
    let models = models_with(vec![Arc::new(ClassifyingProvider::new(
        "p",
        vec![],
        vec![classifier_model("p", "c", LLAMA_API)],
        None,
        registry_of(&recorder),
    ))]);
    let mut model = classifier_model("p", "c", LLAMA_API);
    model.headers = Some(headers(&[("x-foo", Some("a"))]));
    let transform: crate::TransformHeadersFn = Arc::new(|mut merged: HeaderMap| {
        Box::pin(async move {
            let seen = merged.get("x-foo").cloned();
            assert_eq!(
                seen,
                Some(Some("a".to_string())),
                "the transform must see it"
            );
            merged.insert("x-foo".to_string(), Some("rewritten".to_string()));
            merged
        })
    });
    let options = ClassifierOptions {
        transform_headers: Some(transform),
        ..ClassifierOptions::default()
    };

    models.classify(&model, &empty_context(), &options).await;

    assert_eq!(
        recorder.seen().headers.unwrap().get("x-foo"),
        Some(&Some("rewritten".to_string()))
    );
}

// ----------------------------------------------------------------------- multi-type model reads --

fn mixed_models() -> Models {
    models_with(vec![
        Arc::new(ClassifyingProvider::new(
            "llama",
            vec![chat_model("llama", "m1"), chat_model("llama", "m2")],
            vec![classifier_model("llama", "m1", LLAMA_API)],
            None,
            ClassifierApiRegistry::new(),
        )),
        Arc::new(PlainProvider::new("plain", vec![chat_model("plain", "p1")])),
    ])
}

/// `Models::get_all_models` lists every type from every provider (pi `getAllModels`,
/// `models.ts:446-463`), while `get_models` stays the chat catalog (`:432-445`).
#[test]
fn models_get_all_models_spans_types_and_providers() {
    let models = mixed_models();

    let all = models.get_all_models(None);
    let ids: Vec<(ModelType, &str)> = all.iter().map(|m| (m.model_type(), m.id())).collect();
    assert_eq!(
        ids,
        [
            (ModelType::Chat, "m1"),
            (ModelType::Chat, "m2"),
            (ModelType::Classifier, "m1"),
            (ModelType::Chat, "p1"),
        ]
    );
    assert_eq!(models.get_all_models(Some("plain")).len(), 1);
    assert!(models.get_all_models(Some("ghost")).is_empty());
    // The chat-only view must not leak the classifier model.
    assert_eq!(models.get_models(Some("llama")).len(), 2);
}

/// `getModelsOfType` / `getModelOfType` (`models.ts:468-478`): filtered by type, and an id is unique
/// within a type, not across types (`m1` is both a chat and a classifier model above).
#[test]
fn models_typed_reads_filter_by_type_and_scope_ids_to_the_type() {
    let models = mixed_models();

    let classifiers = models.get_models_of_type(ModelType::Classifier, None);
    assert_eq!(classifiers.len(), 1);
    assert_eq!(models.get_models_of_type(ModelType::Chat, None).len(), 3);

    let chat = models
        .get_model_of_type(ModelType::Chat, "llama", "m1")
        .unwrap();
    assert_eq!(chat.model_type(), ModelType::Chat);
    let classifier = models
        .get_model_of_type(ModelType::Classifier, "llama", "m1")
        .unwrap();
    assert_eq!(classifier.model_type(), ModelType::Classifier);
    assert!(
        models
            .get_model_of_type(ModelType::Classifier, "llama", "m2")
            .is_none()
    );
    assert!(
        models
            .get_model_of_type(ModelType::Classifier, "plain", "p1")
            .is_none()
    );

    assert_eq!(models.get_classifier_models(Some("llama")).len(), 1);
    assert!(models.get_classifier_models(Some("plain")).is_empty());
    assert_eq!(
        models
            .get_classifier_model("llama", "m1")
            .unwrap()
            .api
            .as_str(),
        LLAMA_API
    );
    assert!(models.get_classifier_model("llama", "m2").is_none());
}

/// A single-model lookup is scoped to the named provider: another provider that sorts first and
/// lists the same id of the same type must not answer for it.
#[test]
fn single_model_lookups_are_scoped_to_the_named_provider() {
    let models = models_with(vec![
        Arc::new(ClassifyingProvider::new(
            "alpha",
            vec![chat_model("alpha", "m1")],
            vec![classifier_model("alpha", "m1", "other-api")],
            None,
            ClassifierApiRegistry::new(),
        )),
        Arc::new(ClassifyingProvider::new(
            "llama",
            vec![chat_model("llama", "m1")],
            vec![classifier_model("llama", "m1", LLAMA_API)],
            None,
            ClassifierApiRegistry::new(),
        )),
    ]);

    let classifier = models
        .get_model_of_type(ModelType::Classifier, "llama", "m1")
        .unwrap();
    assert_eq!(classifier.provider().as_str(), "llama");
    assert_eq!(classifier.api().as_str(), LLAMA_API);
    let chat = models
        .get_model_of_type(ModelType::Chat, "llama", "m1")
        .unwrap();
    assert_eq!(chat.provider().as_str(), "llama");

    let direct = models.get_classifier_model("llama", "m1").unwrap();
    assert_eq!(direct.provider.as_str(), "llama");
    assert_eq!(direct.api.as_str(), LLAMA_API);
    assert_eq!(models.get_classifier_models(None).len(), 2);
}

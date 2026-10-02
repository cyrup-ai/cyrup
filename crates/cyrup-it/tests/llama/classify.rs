//! Scenario 6: the classifier twin.
//!
//! Pi lists every selectable llama.cpp model twice: as a chat model (`api: "openai-completions"`,
//! `<server>/v1`) and as a classifier model (`api: "llama-cpp-classify"`, the server ROOT), and
//! `getAllModels` returns both (`extensions/llama/provider.ts:73-78` `toPiClassifierModel`,
//! `:200`; `setCatalog`/`refreshModels` build the pair from one filtered catalog, `:143-150`,
//! `:244-246`). `classify` is dispatched to the `llama-cpp-classify` api, which reads the answer
//! from next-token probabilities through the server's `/tokenize`, `/apply-template` and
//! `/completion` (`packages/ai/src/api/llama-cpp-classify.ts`).
//!
//! **Surface.** There is no CLI flag and no `cyrup-sdk` method for `classify`: the only public
//! entry is `cyrup_provider::Models::classify`, so that is where this drives it, with the session's
//! own credential store and the provider the extension registered. The binary cannot reach it, so
//! no binary-level classifier test exists or could exist today.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_config::login::StoreAuthContext;
use cyrup_provider::{
    AnyModel, ClassifierAnswer, ClassifierContext, ClassifierModel, ClassifierOptions,
    ClassifierQuestion, ClassifierStopReason, CreateModelsOptions, CredentialStore, OrderedMap,
    ProviderEnv, create_models,
};
use serde_json::json;

use super::fake::{FakeLlama, PLAIN_TEMPLATE, THINKING_TEMPLATE, model, model_with};
use super::fixture::{Opts, PROVIDER, fixture, refresh_llama, session, store_credential};

const KEY: &str = "sk-llama-fixture";

async fn router() -> FakeLlama {
    let fake = FakeLlama::start(vec![
        model_with("qwen3", "loaded", json!({ "meta": { "n_ctx": 8192 } })),
        model_with("plain", "loaded", json!({ "meta": { "n_ctx": 4096 } })),
        model("cold", "unloaded"),
    ])
    .await;
    fake.set_chat_template("qwen3", THINKING_TEMPLATE);
    fake.set_chat_template("plain", PLAIN_TEMPLATE);
    fake
}

/// The twin is listed beside the chat model, one per selectable model, on the server root.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_selectable_model_is_listed_as_a_chat_model_and_as_a_classifier() {
    let fx = fixture();
    let fake = router().await;
    store_credential(&fx, fake.url(), Some(KEY));
    let session = session(&fx, &Opts::default()).await;
    assert!(refresh_llama(&session).await.is_empty());

    let provider = session
        .services()
        .guest_providers
        .provider(PROVIDER)
        .unwrap();
    let all = provider.get_all_models();

    let mut chat: Vec<String> = all
        .iter()
        .filter_map(AnyModel::as_chat)
        .map(|m| m.id.as_str().to_string())
        .collect();
    let mut classifiers: Vec<ClassifierModel> = all
        .iter()
        .filter_map(AnyModel::as_classifier)
        .cloned()
        .collect();
    chat.sort();
    classifiers.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    // `cold` (unloaded, not a preset) is in neither list (`provider.ts:49-56`).
    assert_eq!(chat, ["plain", "qwen3"]);
    assert_eq!(
        classifiers
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["plain", "qwen3"],
        "one classifier per chat model"
    );
    for classifier in &classifiers {
        assert_eq!(classifier.api.as_str(), "llama-cpp-classify");
        assert_eq!(classifier.provider.as_str(), PROVIDER);
        // The server ROOT, where `/tokenize` lives; the chat models use `<root>/v1`.
        assert_eq!(classifier.base_url, fake.url());
    }
    assert_eq!(
        classifiers
            .iter()
            .find(|c| c.id.as_str() == "qwen3")
            .map(|c| c.context_window),
        Some(8192),
        "the classifier twin carries the model's context window (`provider.ts:77`)"
    );
    // The chat catalog the session offers carries only the chat models: a classifier is not a
    // selectable chat model.
    assert!(
        session
            .configured_model_catalog()
            .iter()
            .filter(|m| m.provider.as_str() == PROVIDER)
            .all(|m| m.api.as_str() == "openai-completions")
    );
}

/// `Models::classify` against the fake's `/tokenize`, `/apply-template` and `/completion`, with the
/// stored credential's key and the model id on every request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classify_answers_from_the_routers_next_token_probabilities() {
    let fx = fixture();
    let fake = router().await;
    store_credential(&fx, fake.url(), Some(KEY));
    let session = session(&fx, &Opts::default()).await;
    assert!(refresh_llama(&session).await.is_empty());

    let provider = session
        .services()
        .guest_providers
        .provider(PROVIDER)
        .unwrap();
    let classifier = provider
        .get_all_models()
        .iter()
        .filter_map(AnyModel::as_classifier)
        .find(|c| c.id.as_str() == "qwen3")
        .cloned()
        .expect("the classifier twin of qwen3");

    // The same collection shape the host composes: the live provider over the session's credential
    // store and ambient environment, so the key and the server come from `/login`'s credential.
    let auth = session.services().auth.clone();
    let mut models = create_models(CreateModelsOptions {
        credentials: Some(auth.clone() as Arc<dyn CredentialStore>),
        auth_context: Some(Arc::new(StoreAuthContext(auth))),
        catalog_overlay: None,
    });
    models.set_provider(provider);

    let mut state = serde_json::Map::new();
    state.insert(
        "message".to_string(),
        json!("Help! My payouts have been failing for 3 days."),
    );
    let context = ClassifierContext {
        state,
        questions: [(
            "team",
            ClassifierQuestion::Choice {
                instructions: "Which team should handle this?".to_string(),
                criteria: [
                    ("billing", "Payments".to_string()),
                    ("technical", String::new()),
                ]
                .into_iter()
                .collect::<OrderedMap<String>>(),
            },
        )]
        .into_iter()
        .collect(),
    };
    // Pin proxy resolution off, as the provider's own tests do, so an ambient proxy variable cannot
    // send the loopback request off-box.
    let mut env = ProviderEnv::new();
    env.insert("no_proxy".to_string(), "*".to_string());
    let options = ClassifierOptions {
        env: Some(env),
        ..ClassifierOptions::default()
    };

    let result = models.classify(&classifier, &context, &options).await;

    assert_eq!(result.error_message, None, "{result:?}");
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
    assert_eq!(result.model, "qwen3");
    // The fake ranks `B` above `A`; labels are assigned to the criteria keys in order, so `B` is
    // the second key.
    match result.answers.get("team") {
        Some(ClassifierAnswer::Choice {
            choice,
            probabilities,
            confidence,
        }) => {
            assert_eq!(choice, "technical");
            let billing = *probabilities.get("billing").unwrap();
            let technical = *probabilities.get("technical").unwrap();
            assert!(technical > billing, "{technical} vs {billing}");
            assert!((billing + technical - 1.0).abs() < 1e-9);
            assert!(*confidence > 0.0);
        }
        other => panic!("expected a choice answer, got {other:?}"),
    }

    // The wire: all three endpoints, on the server root, for this model, with the stored key.
    for path in ["/tokenize", "/apply-template", "/completion"] {
        let calls = fake.requests_to("POST", path);
        assert!(!calls.is_empty(), "`POST {path}` was never called");
        for call in calls {
            assert_eq!(call.json()["model"], "qwen3", "{path}: {call:?}");
            assert_eq!(
                call.header("authorization"),
                Some(format!("Bearer {KEY}").as_str()),
                "{path}: the stored key authenticates the classifier too"
            );
        }
    }
    assert_eq!(
        fake.requests_to("POST", "/completion")[0].json()["n_predict"],
        1,
        "the answer is read from ONE predicted token's probabilities"
    );
}

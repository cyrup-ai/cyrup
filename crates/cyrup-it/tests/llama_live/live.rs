//! The live run: one real router, every llama.cpp path cyrup has, every definition checked.
//!
//! One test, in phases, because the phases share state a real server takes seconds to build (a
//! loaded model) and depend on each other's order (load before classify, unload last).

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use cyrup_core::{CancelToken, Content, Message, StopReason};
use cyrup_llama::LlamaError;
use cyrup_llama::client::{LlamaClient, LlamaModelInfo, LlamaModelStatus, LlamaProgress};
use cyrup_llama::model::{is_chat_model, is_decision_model};
use cyrup_llama::provider::{
    CatalogPublication, CatalogPublisher, LlamaController, LlamaControllerOptions,
    LlamaRefreshContext, RegisterProviderFn, SetCatalogOptions,
};
use cyrup_llama_cpp_wire::classify::{self, TokenLogprob};
use cyrup_llama_cpp_wire::{golden, router, systemone};
use cyrup_provider::stream::{StreamOptions, collect_message};
use cyrup_provider::{
    AnyModel, AuthContext, BoolCriteria, ClassifierAnswer, ClassifierContext, ClassifierModel,
    ClassifierOptions, ClassifierQuestion, ClassifierStopReason, Context, CreateModelsOptions,
    Credential, CredentialStore, InMemoryCredentialStore, Models, OrderedMap, Provider,
    ProviderEnv, create_models,
};

use super::conformance::assert_conforms;
use super::router::{API_KEY, CHAT_MODEL, DECISION_MODEL, Router, setup};

// ---------------------------------------------------------------------------------------- harness --

/// Loopback only: never let an ambient proxy carry a request to the router off-box.
fn no_proxy_env() -> ProviderEnv {
    ProviderEnv::from([("no_proxy".to_string(), "*".to_string())])
}

/// The `llama.cpp` credential `/login` stores: the key and `LLAMA_BASE_URL`.
fn credential(url: &str) -> Credential {
    let mut env = no_proxy_env();
    env.insert("LLAMA_BASE_URL".to_string(), url.to_string());
    Credential::ApiKey {
        key: Some(API_KEY.to_string()),
        env: Some(env),
    }
}

/// No ambient environment at all: auth comes from the stored credential only.
struct NoEnv;

#[async_trait::async_trait]
impl AuthContext for NoEnv {
    async fn env(&self, _name: &str) -> Option<String> {
        None
    }

    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

/// pi's `context.publish`: persist (nowhere), then run the update.
struct Publish;

#[async_trait::async_trait]
impl CatalogPublisher for Publish {
    async fn publish(&self, publication: CatalogPublication) -> Result<bool, LlamaError> {
        if let Some(update) = publication.update {
            update();
        }
        Ok(true)
    }
}

fn entry<'a>(catalog: &'a Value, id: &str) -> &'a Value {
    catalog["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == id)
        .unwrap_or_else(|| panic!("{id} is not in the real catalog: {catalog}"))
}

fn status_of(models: &[LlamaModelInfo], id: &str) -> LlamaModelStatus {
    models
        .iter()
        .find(|model| model.id == id)
        .unwrap_or_else(|| panic!("{id} is not in the catalog LlamaClient::list returned"))
        .status
        .value
        .clone()
}

fn user_context(text: &str) -> Context {
    Context {
        system_prompt: None,
        messages: vec![Message::User {
            content: vec![Content::Text {
                text: text.into(),
                text_signature: None,
            }],
            timestamp: 0,
        }],
        tools: Vec::new(),
    }
}

fn state(text: &str) -> serde_json::Map<String, Value> {
    serde_json::Map::from_iter([("text".to_string(), json!(text))])
}

/// pi's own System One test questions (`test/typesafe-system-one.test.ts` @f1b2e77f5): one
/// choice, one score, one bool.
fn three_questions() -> OrderedMap<ClassifierQuestion> {
    OrderedMap::from_iter([
        (
            "category",
            ClassifierQuestion::Choice {
                instructions: "What kind of message is this?".to_string(),
                criteria: OrderedMap::from_iter([
                    ("success", "A success report".to_string()),
                    ("failure", "A failure report".to_string()),
                ]),
            },
        ),
        (
            "satisfaction",
            ClassifierQuestion::Score {
                instructions: "How satisfied is the writer?".to_string(),
                criteria: vec!["low".to_string(), "neutral".to_string(), "high".to_string()],
            },
        ),
        (
            "approved",
            ClassifierQuestion::Bool {
                instructions: "Is the change approved?".to_string(),
                criteria: BoolCriteria {
                    when_true: "yes".to_string(),
                    when_false: "no".to_string(),
                },
            },
        ),
    ])
}

const STATE_TEXT: &str = "The deployment succeeded, thank you.";

// ------------------------------------------------------------------------------------------- test --

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_llama_server_agrees_with_the_wire_definitions_and_cyrups_llama_paths() {
    let Some(setup) =
        setup("a_real_llama_server_agrees_with_the_wire_definitions_and_cyrups_llama_paths")
    else {
        return;
    };
    let router = Router::start(&setup).await;
    let url = router.url().to_string();
    let cancel = CancelToken::new();
    let client = LlamaClient::new(&url, Some(API_KEY.to_string()), Some(&no_proxy_env()))
        .await
        .unwrap();

    // ---- 1. Auth and the literal error bodies, byte for byte. ----
    let (status, body) = router::invalid_api_key();
    assert_eq!(
        router.raw_get_unauthenticated("/models").await,
        (status, body.to_string()),
        "a request without the key"
    );
    assert_eq!(
        router.raw_get("/no-such-route").await,
        (404, golden::FILE_NOT_FOUND.to_string()),
        "an unknown route"
    );
    let (status, body) = router::not_found("model is not found");
    assert_eq!(
        router
            .raw_post("/models/load", &json!({ "model": "no-such-model" }))
            .await,
        (status, body.to_string()),
        "POST /models/load of a model the router does not have"
    );
    let (status, body) = router::invalid_request("model is not running");
    assert_eq!(
        router
            .raw_post("/models/unload", &json!({ "model": CHAT_MODEL }))
            .await,
        (status, body.to_string()),
        "POST /models/unload of a model that is not running"
    );
    let (status, body) = router::invalid_request("model is not found");
    assert_eq!(
        router
            .raw_post("/models/unload", &json!({ "model": "no-such-model" }))
            .await,
        (status, body.to_string()),
        "POST /models/unload of a model the router does not have"
    );
    // ... and the client surfaces the server's message for each.
    assert_eq!(
        client
            .load("no-such-model", &cancel)
            .await
            .unwrap_err()
            .to_string(),
        "File Not Found"
    );
    assert_eq!(
        client
            .unload(CHAT_MODEL, &cancel)
            .await
            .unwrap_err()
            .to_string(),
        "model is not running"
    );
    let unauthenticated = LlamaClient::new(&url, None, Some(&no_proxy_env()))
        .await
        .unwrap();
    assert_eq!(
        unauthenticated
            .list(false, &cancel)
            .await
            .unwrap_err()
            .to_string(),
        "Invalid API Key"
    );

    // ---- 2. The catalog before any load: LlamaClient::list and the raw shape. ----
    let listed = client.list(false, &cancel).await.unwrap();
    let mut ids: Vec<&str> = listed.iter().map(|model| model.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, [CHAT_MODEL, DECISION_MODEL]);
    for model in &listed {
        assert_eq!(model.status.value, LlamaModelStatus::Unloaded, "{model:?}");
        assert_eq!(model.source.as_deref(), Some("models_dir"), "{model:?}");
        assert!(
            model.meta.is_none(),
            "an unloaded entry has no meta: {model:?}"
        );
    }
    let chat_info = listed.iter().find(|model| model.id == CHAT_MODEL).unwrap();
    let decision_info = listed
        .iter()
        .find(|model| model.id == DECISION_MODEL)
        .unwrap();
    assert!(is_chat_model(chat_info) && !is_decision_model(chat_info));
    assert!(is_decision_model(decision_info) && !is_chat_model(decision_info));

    let catalog = router.get_json("/models").await;
    assert_conforms(
        "models_envelope",
        &router::models_envelope(Vec::new()),
        &catalog,
    );
    assert_conforms(
        "unloaded_preset_entry (a models_dir entry carries status.preset too)",
        &router::unloaded_preset_entry(CHAT_MODEL, &[]),
        entry(&catalog, CHAT_MODEL),
    );
    assert_conforms(
        "decision_entry, unloaded",
        &router::decision_entry(DECISION_MODEL, "unloaded"),
        entry(&catalog, DECISION_MODEL),
    );
    assert_eq!(
        entry(&catalog, DECISION_MODEL).get("meta"),
        None,
        "decision_entry: an unloaded entry carries no meta"
    );

    // ---- 3. The router's own props. ----
    let props = router.get_json("/props").await;
    assert_conforms("router_props", &router::router_props(true), &props);
    assert_eq!(
        client.props(None, &cancel).await.unwrap().models_autoload,
        Some(true),
        "LlamaClient::props reads the router's models_autoload"
    );

    // ---- 4. Load both models through LlamaClient::load_and_wait (SSE + polling), tailing SSE. ----
    let tail = router.tail_sse();
    let progress: Arc<Mutex<Vec<LlamaProgress>>> = Arc::default();
    let sink = progress.clone();
    let on_progress = move |update: LlamaProgress| sink.lock().unwrap().push(update);
    for model in [CHAT_MODEL, DECISION_MODEL] {
        let loaded = client
            .load_and_wait(model, &on_progress, &cancel)
            .await
            .unwrap_or_else(|error| panic!("load_and_wait({model}): {error}"));
        assert_eq!(loaded.id, model);
        assert_eq!(loaded.status.value, LlamaModelStatus::Loaded, "{loaded:?}");
        assert!(
            loaded.meta.and_then(|meta| meta.n_ctx).is_some(),
            "{model}: meta.n_ctx"
        );
    }
    let messages: Vec<String> = progress
        .lock()
        .unwrap()
        .iter()
        .map(|update| update.message.clone())
        .collect();
    assert!(
        messages.iter().any(|message| message == "Loading model"),
        "{messages:?}"
    );
    // A stage progress message (`Loading text model`) is the proof the SSE path parsed a real
    // `status_change` progress payload.
    assert!(
        messages
            .iter()
            .any(|message| message == "Loading text model"),
        "no SSE stage progress reached load_and_wait: {messages:?}"
    );
    // Give the tail a moment to see the final frames, then check every frame's shape.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let sse = tail.stop();
    let frames: Vec<&str> = sse.split_terminator("\n\n").collect();
    assert!(!frames.is_empty(), "no SSE frame arrived");
    let mut events = Vec::new();
    for frame in &frames {
        let payload = frame
            .strip_prefix("data: ")
            .unwrap_or_else(|| panic!("an SSE frame is not `data: <json>`: {frame:?}"));
        let event: Value = serde_json::from_str(payload).unwrap();
        assert_eq!(
            router::sse_frame(&event).len(),
            frame.len() + 2,
            "sse_frame is `data: <json>\\n\\n`"
        );
        events.push(event);
    }
    let status_change = |model: &str, status: &str, with_progress: bool| {
        events
            .iter()
            .find(|event| {
                event["model"] == model
                    && event["event"] == "status_change"
                    && event["data"]["status"] == status
                    && event["data"].get("progress").is_some() == with_progress
            })
            .unwrap_or_else(|| {
                panic!("no {model} status_change {status} (progress: {with_progress}) in {sse}")
            })
    };
    for model in [CHAT_MODEL, DECISION_MODEL] {
        assert_conforms(
            "status_change_event + load_progress",
            &router::status_change_event(
                model,
                "loading",
                Some(router::load_progress(&["text_model"], "text_model", 0.5)),
            ),
            status_change(model, "loading", true),
        );
        assert_conforms(
            "status_change_event, loaded",
            &router::status_change_event(model, "loaded", None),
            status_change(model, "loaded", false),
        );
    }

    // ---- 5. The catalog with both loaded, and the child's props. ----
    let catalog = router.get_json("/models").await;
    assert_conforms(
        "loaded_entry",
        &router::loaded_entry(CHAT_MODEL, &[]),
        entry(&catalog, CHAT_MODEL),
    );
    assert_conforms(
        "decision_entry, loaded",
        &router::decision_entry(DECISION_MODEL, "loaded"),
        entry(&catalog, DECISION_MODEL),
    );
    let child = router
        .get_json(&format!("/props?model={CHAT_MODEL}&autoload=false"))
        .await;
    assert_conforms("child_props", &router::child_props(""), &child);
    let child_props = client.props(Some(CHAT_MODEL), &cancel).await.unwrap();
    assert_eq!(
        child_props.models_autoload, None,
        "a child's props carry no models_autoload"
    );
    assert_eq!(
        child_props.chat_template.as_deref(),
        child["chat_template"].as_str()
    );

    // ---- 6. The classifier wire, with the bodies llama-cpp-classify sends. ----
    let a = TokenLogprob {
        id: 1,
        token: "a",
        logprob: -0.5,
    };
    let tokenized = router
        .post_json(
            "/tokenize",
            &json!({ "model": CHAT_MODEL, "content": "Once upon", "add_special": false, "parse_special": false }),
        )
        .await;
    assert_conforms("classify::tokenize", &classify::tokenize(&[1]), &tokenized);
    let pieces = router
        .post_json(
            "/tokenize",
            &json!({ "model": CHAT_MODEL, "content": "A", "with_pieces": true }),
        )
        .await;
    assert_conforms(
        "classify::tokenize_with_pieces",
        &classify::tokenize_with_pieces(&[(1, "a")]),
        &pieces,
    );
    let templated = router
        .post_json(
            "/apply-template",
            &json!({
                "model": CHAT_MODEL,
                "messages": [{ "role": "system", "content": "s" }, { "role": "user", "content": "u" }],
                "chat_template_kwargs": { "enable_thinking": false },
            }),
        )
        .await;
    assert_conforms(
        "classify::apply_template",
        &classify::apply_template("p"),
        &templated,
    );
    let completed = router
        .post_json(
            "/completion",
            &json!({
                "model": CHAT_MODEL,
                "prompt": templated["prompt"],
                "n_predict": 1,
                "n_probs": 2,
                "post_sampling_probs": false,
                "cache_prompt": true,
                "temperature": 0,
            }),
        )
        .await;
    assert_conforms(
        "classify::completion",
        &classify::completion(CHAT_MODEL, "p", 1, a, &[a, a]),
        &completed,
    );
    let keys: Vec<&str> = completed
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        golden::COMPLETION_KEYS,
        "the /completion keys, in order"
    );
    assert_eq!(
        completed["completion_probabilities"][0]["top_logprobs"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "n_probs: 2 gives two candidates"
    );
    assert_eq!(completed["tokens_cached"], completed["tokens_evaluated"]);
    assert_eq!(completed["stop_type"], "limit");

    // ---- 7. The System One wire. ----
    let questions = json!({
        "category": { "type": "choice", "instructions": "What kind of message is this?",
                      "criteria": { "success": "A success report", "failure": "A failure report" } },
        "satisfaction": { "type": "score", "instructions": "How satisfied is the writer?",
                          "criteria": ["low", "neutral", "high"] },
        "approved": { "type": "noul", "instructions": "Is the change approved?",
                      "criteria": { "true": "yes", "false": "no" } },
    });
    let answered = router
        .post_json(
            "/v1/systemone",
            &json!({ "model": DECISION_MODEL, "state": { "text": STATE_TEXT }, "questions": questions }),
        )
        .await;
    assert_conforms(
        "systemone::response",
        &systemone::response(
            DECISION_MODEL,
            &[
                (
                    "category",
                    systemone::choice_answer("success", &[("success", 0.5), ("failure", 0.5)], 0.1),
                ),
                (
                    "satisfaction",
                    systemone::score_answer(
                        1.0,
                        &["low", "neutral", "high"],
                        &[0.3, 0.3, 0.4],
                        0.1,
                    ),
                ),
                ("approved", systemone::noul_answer(0.5)),
            ],
            1,
        ),
        &answered,
    );
    let (status, body) = systemone::not_a_decision_model();
    assert_eq!(
        router
            .raw_post(
                "/v1/systemone",
                &json!({ "model": CHAT_MODEL, "state": { "text": STATE_TEXT }, "questions": questions }),
            )
            .await,
        (status, body.to_string()),
        "System One against a text model"
    );

    // ---- 8. LlamaProvider::refresh against the real router (EXT-110 included). ----
    let store: Arc<dyn CredentialStore> = Arc::new(
        InMemoryCredentialStore::new().with_credential("llama.cpp".into(), credential(&url)),
    );
    let register: RegisterProviderFn = Arc::new(|_provider| Ok(()));
    let controller = LlamaController::new(
        LlamaControllerOptions::new(store.clone(), register).with_auth_context(Arc::new(NoEnv)),
    );
    let stored_credential = credential(&url);
    controller
        .provider()
        .refresh(&LlamaRefreshContext {
            credential: Some(&stored_credential),
            stored: None,
            publisher: &Publish,
            allow_network: true,
            cancel: &cancel,
        })
        .await
        .unwrap();
    let provider = controller.provider();
    let chat_ids: Vec<&str> = provider.models().iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        chat_ids,
        [CHAT_MODEL],
        "EXT-110: the decision model is NOT a chat model"
    );
    let chat_model = provider.models()[0].clone();
    assert_eq!(chat_model.api.as_str(), "openai-completions");
    assert_eq!(chat_model.base_url, format!("{url}/v1"));
    assert_eq!(
        chat_model.context_window, 2048,
        "meta.n_ctx of the loaded child"
    );
    let classifier = |id: &str| -> ClassifierModel {
        provider
            .classifier_models()
            .iter()
            .find(|model| model.id.as_str() == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} is not among the classifiers"))
    };
    let chat_classifier = classifier(CHAT_MODEL);
    let decision_classifier = classifier(DECISION_MODEL);
    assert_eq!(chat_classifier.api.as_str(), "llama-cpp-classify");
    assert_eq!(chat_classifier.base_url, url);
    assert_eq!(
        decision_classifier.api.as_str(),
        "typesafe-system-one",
        "EXT-110: the decision model IS a classifier, on the native System One api"
    );
    assert_eq!(decision_classifier.base_url, format!("{url}/v1"));
    assert_eq!(
        provider
            .get_all_models()
            .iter()
            .filter(|model| matches!(model, AnyModel::Chat(_)))
            .count(),
        1
    );

    // `/llama`'s path: `set_catalog` over the catalog `LlamaClient::list` read from the router
    // (no props, so no reasoning), with the router's own autoload answer.
    let listed = client.list(false, &cancel).await.unwrap();
    let autoload = client.props(None, &cancel).await.unwrap().models_autoload;
    controller
        .set_catalog(
            &listed,
            &url,
            SetCatalogOptions {
                router_autoload: autoload == Some(true),
            },
        )
        .unwrap();
    let from_catalog = controller.provider();
    assert_eq!(
        from_catalog
            .models()
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        [CHAT_MODEL],
        "set_catalog: the decision model is not a chat model (EXT-110)"
    );
    let mut catalog_classifiers: Vec<(&str, &str)> = from_catalog
        .classifier_models()
        .iter()
        .map(|model| (model.id.as_str(), model.api.as_str()))
        .collect();
    catalog_classifiers.sort_unstable();
    assert_eq!(
        catalog_classifiers,
        [
            (CHAT_MODEL, "llama-cpp-classify"),
            (DECISION_MODEL, "typesafe-system-one")
        ]
    );

    // ---- 9. Through `Models`, as the host composes it: auth from the stored credential. ----
    let mut models: Models = create_models(CreateModelsOptions {
        credentials: Some(store),
        auth_context: Some(Arc::new(NoEnv)),
        catalog_overlay: None,
    });
    models.set_provider(provider.clone() as Arc<dyn Provider>);

    // One streamed turn.
    let message = collect_message(models.stream(
        &chat_model,
        &user_context("Once upon a time"),
        &StreamOptions {
            max_tokens: Some(8),
            env: Some(no_proxy_env()),
            ..StreamOptions::default()
        },
    ))
    .await;
    assert_eq!(message.error_message, None, "{message:?}");
    assert!(
        matches!(message.stop_reason, StopReason::Stop | StopReason::Length),
        "{message:?}"
    );
    let text: String = message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect();
    assert!(
        !text.is_empty(),
        "the streamed turn produced no text: {message:?}"
    );
    assert!(message.usage.output > 0, "{:?}", message.usage);

    let options = ClassifierOptions {
        env: Some(no_proxy_env()),
        ..ClassifierOptions::default()
    };

    // llama-cpp-classify against the text model.
    let result = models
        .classify(
            &chat_classifier,
            &ClassifierContext {
                state: state(STATE_TEXT),
                images: None,
                questions: OrderedMap::from_iter([(
                    "category",
                    ClassifierQuestion::Choice {
                        instructions: "What kind of message is this?".to_string(),
                        criteria: OrderedMap::from_iter([
                            ("success", "A success report".to_string()),
                            ("failure", "A failure report".to_string()),
                        ]),
                    },
                )]),
            },
            &options,
        )
        .await;
    assert_eq!(result.error_message, None, "{result:?}");
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop, "{result:?}");
    match result.answers.get("category") {
        Some(ClassifierAnswer::Choice {
            choice,
            probabilities,
            ..
        }) => {
            assert!(
                ["success", "failure"].contains(&choice.as_str()),
                "{choice}"
            );
            let total: f64 = probabilities.values().sum();
            assert!((total - 1.0).abs() < 1e-9, "{probabilities:?}");
        }
        other => panic!("llama-cpp-classify: expected a choice answer, got {other:?}"),
    }

    // typesafe-system-one against the decision model.
    let result = models
        .classify(
            &decision_classifier,
            &ClassifierContext {
                state: state(STATE_TEXT),
                images: None,
                questions: three_questions(),
            },
            &options,
        )
        .await;
    assert_eq!(result.error_message, None, "{result:?}");
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop, "{result:?}");
    assert!(
        matches!(
            result.answers.get("category"),
            Some(ClassifierAnswer::Choice { .. })
        ),
        "{result:?}"
    );
    assert!(
        matches!(
            result.answers.get("satisfaction"),
            Some(ClassifierAnswer::Score { .. })
        ),
        "{result:?}"
    );
    assert!(
        matches!(result.answers.get("approved"), Some(ClassifierAnswer::Bool { probability }) if (0.0..=1.0).contains(probability)),
        "{result:?}"
    );
    assert!(
        result.usage.as_ref().is_some_and(|usage| usage.input > 0),
        "{result:?}"
    );

    // ---- 10. Unload: the decision model raw (the success body), the text model through
    // LlamaClient::unload_and_wait. ----
    assert_eq!(
        router
            .raw_post("/models/unload", &json!({ "model": DECISION_MODEL }))
            .await,
        (200, router::success().to_string()),
        "router::success"
    );
    client.unload_and_wait(CHAT_MODEL, &cancel).await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let listed = loop {
        let listed = client.list(false, &cancel).await.unwrap();
        if status_of(&listed, DECISION_MODEL) == LlamaModelStatus::Unloaded {
            break listed;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{DECISION_MODEL} did not unload"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    for model in [CHAT_MODEL, DECISION_MODEL] {
        assert_eq!(status_of(&listed, model), LlamaModelStatus::Unloaded);
    }
    let catalog = router.get_json("/models").await;
    assert_conforms(
        "decision_entry, unloaded after a load",
        &router::decision_entry(DECISION_MODEL, "unloaded"),
        entry(&catalog, DECISION_MODEL),
    );
    assert_eq!(entry(&catalog, CHAT_MODEL).get("meta"), None);
}

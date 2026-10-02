//! Parity tests for the `llama-cpp-classify` api: every case of pi's
//! `packages/ai/test/llama-cpp-classify.test.ts` @v0.99.2-17 (15 cases, quoted by line), driven
//! against a loopback fake `llama-server` instead of pi's fake `fetch`
//! (see [`super::llama_cpp_classify_fake_server`]).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use cyrup_core::CancelToken;
use futures::FutureExt as _;
use serde_json::{Value, json};

use super::llama_cpp_classify_fake_server::{
    Behavior, FakeServer, answer_by_prompt, boolean, char_tokens, choice, classify, context_of,
    model, options, pick_context, ticket_context, word_tokens,
};
use crate::api::llama_cpp_classify::{
    answer_from_probabilities, label_probabilities, llama_server_root, peak_confidence,
    render_question,
};
use crate::classifier::{ClassifierAnswer, ClassifierQuestion, ClassifierStopReason, OrderedMap};
use crate::stream::ProviderResponse;

fn probabilities_of(answer: &ClassifierAnswer) -> Vec<(String, f64)> {
    match answer {
        ClassifierAnswer::Choice { probabilities, .. } => probabilities
            .iter()
            .map(|(key, value)| (key.to_string(), *value))
            .collect(),
        other => panic!("not a choice answer: {other:?}"),
    }
}

/// test:117-155
#[tokio::test]
async fn answers_choice_bool_and_score_questions_from_label_log_probabilities() {
    let server = FakeServer::with(
        Behavior::default()
            .next(answer_by_prompt)
            .tokenize(word_tokens),
    )
    .await;
    let classifier_model = model(&server.base_url);
    let mut opts = options();
    opts.api_key = Some("local".to_string());

    let result = classify(&classifier_model, &ticket_context(), &opts).await;

    assert_eq!(result.error_message, None);
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
    let choice = label_probabilities(&[-1.5, -0.3, -3.0], 1.0);
    assert_eq!(
        result.answers.get("team"),
        Some(&ClassifierAnswer::Choice {
            choice: "technical".to_string(),
            probabilities: [
                ("billing", choice[0]),
                ("technical", choice[1]),
                ("sales", choice[2]),
            ]
            .into_iter()
            .collect::<OrderedMap<f64>>(),
            confidence: peak_confidence(&choice),
        })
    );
    assert_eq!(
        result.answers.get("urgent"),
        Some(&ClassifierAnswer::Bool {
            probability: label_probabilities(&[-0.05, -3.0], 1.0)[0]
        })
    );
    let levels = label_probabilities(&[-4.0, -1.8, -0.2], 1.0);
    assert_eq!(
        result.answers.get("severity"),
        Some(&ClassifierAnswer::Score {
            score: levels[1] + 2.0 * levels[2],
            confidence: peak_confidence(&levels),
        })
    );

    // Every request goes to the server root, not under /v1, with the model id and the bearer key.
    let requests = server.requests();
    assert!(!requests.is_empty());
    for request in &requests {
        assert!(
            ["/tokenize", "/apply-template", "/completion"].contains(&request.path.as_str()),
            "unexpected path {}",
            request.path
        );
        assert_eq!(request.body["model"], "qwen");
        assert_eq!(request.header("authorization"), Some("Bearer local"));
    }
    let completion = &server.requests_to("/completion")[0].body;
    assert_eq!(completion["n_predict"], 1);
    assert_eq!(completion["n_probs"], 256);
    assert_eq!(completion["post_sampling_probs"], false);
    assert_eq!(completion["cache_prompt"], true);
    let template = &server.requests_to("/apply-template")[0].body;
    assert_eq!(
        template["chat_template_kwargs"],
        json!({ "enable_thinking": false })
    );
}

/// test:157-199
#[test]
fn repeats_the_state_around_all_questions_and_ends_with_this_questions_labels() {
    let rendered = render_question(&ticket_context(), "team").unwrap();
    let state = "State:\n{\n \"message\": \"Help! My payouts have been failing for 3 days.\"\n}";
    assert_eq!(rendered.labels, ["A", "B", "C"]);
    assert_eq!(rendered.keys, ["billing", "technical", "sales"]);
    assert_eq!(
        rendered.content,
        [
            state,
            "",
            "Task: answer each of the following questions about the state.",
            "",
            "Question: Which team should handle this?",
            "",
            "Options:",
            "- billing: Payments and refunds",
            "- technical: Bugs and outages",
            "- sales",
            "",
            "Question: Does this convey urgency?",
            "",
            "Yes means: The user needs help soon",
            "No means: No time pressure",
            "",
            "Question: How severe is this?",
            "",
            "Levels:",
            "0. low",
            "1. medium",
            "2. high",
            "",
            state,
            "",
            "Question: Which team should handle this?",
            "",
            "Options:",
            "A. billing: Payments and refunds",
            "B. technical: Bugs and outages",
            "C. sales",
            "",
            "Answer with one letter.",
        ]
        .join("\n")
    );
}

/// test:201-214
#[test]
fn shares_everything_before_the_final_question_across_the_questions_of_a_request() {
    let context = ticket_context();
    let prefix = |id: &str| {
        let content = render_question(&context, id).unwrap().content;
        let cut = content.rfind("Question:").unwrap();
        content[..cut].to_string()
    };
    assert_eq!(prefix("urgent"), prefix("team"));
    assert_eq!(prefix("severity"), prefix("team"));
    assert!(
        render_question(&context, "urgent")
            .unwrap()
            .content
            .ends_with("No means: No time pressure\n\nAnswer Yes or No.")
    );
    assert!(
        render_question(&context, "severity")
            .unwrap()
            .content
            .ends_with("2. high\n\nAnswer with one level number.")
    );
}

/// test:216-227
#[tokio::test]
async fn divides_label_log_probabilities_by_the_temperature() {
    let server = FakeServer::with(
        Behavior::default().next(|_, _| vec![("A".to_string(), -0.1), ("B".to_string(), -2.5)]),
    )
    .await;
    let mut opts = options();
    opts.temperature = 2.0;

    let result = classify(&model(&server.base_url), &pick_context(), &opts).await;

    let expected = label_probabilities(&[-0.1 / 2.0, -2.5 / 2.0], 1.0);
    let answer = result.answers.get("pick").unwrap();
    assert_eq!(
        probabilities_of(answer),
        vec![
            ("a".to_string(), expected[0]),
            ("b".to_string(), expected[1])
        ]
    );
    assert_eq!(label_probabilities(&[-0.1, -2.5], 2.0), expected);
}

/// test:229-236
#[tokio::test]
async fn rejects_non_positive_temperatures_before_sending_requests() {
    let server = FakeServer::start().await;
    let mut opts = options();
    opts.temperature = 0.0;

    let result = classify(&model(&server.base_url), &ticket_context(), &opts).await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert!(
        result
            .error_message
            .as_deref()
            .unwrap()
            .contains("Temperature must be a positive number, got 0")
    );
    assert!(server.requests().is_empty());
}

/// test:238-255
#[tokio::test]
async fn retries_deeper_readouts_when_a_label_is_missing_and_fails_without_inventing_zeros() {
    let deep = FakeServer::with(Behavior::default().next(|_, depth| {
        if depth < 4096 {
            vec![("A".to_string(), -0.1)]
        } else {
            vec![("A".to_string(), -0.1), ("B".to_string(), -9.0)]
        }
    }))
    .await;
    let recovered = classify(&model(&deep.base_url), &pick_context(), &options()).await;
    assert_eq!(recovered.stop_reason, ClassifierStopReason::Stop);
    assert_eq!(deep.completion_depths(), [256, 4096]);

    let never =
        FakeServer::with(Behavior::default().next(|_, _| vec![("A".to_string(), -0.1)])).await;
    let failed = classify(&model(&never.base_url), &pick_context(), &options()).await;
    assert_eq!(failed.stop_reason, ClassifierStopReason::Error);
    assert!(failed.answers.is_empty());
    assert!(
        failed
            .error_message
            .as_deref()
            .unwrap()
            .contains("did not rank labels B for pick within the top 32768 tokens"),
        "{:?}",
        failed.error_message
    );
    assert_eq!(never.completion_depths(), [256, 4096, 32768]);
}

/// test:257-267
#[tokio::test]
async fn closes_a_reasoning_block_the_template_leaves_open() {
    let server =
        FakeServer::with(Behavior::default().template(|_| "<|assistant|>\n<think>".to_string()))
            .await;
    classify(&model(&server.base_url), &pick_context(), &options()).await;

    let completion = &server.requests_to("/completion")[0].body;
    assert_eq!(completion["prompt"], "<|assistant|>\n<think></think>");
}

/// test:269-290
#[tokio::test]
async fn reads_labels_in_reply_position_and_rejects_labels_that_are_not_one_token() {
    // A tokenizer that merges a newline with a following letter falls back to the label alone.
    let merging = FakeServer::with(Behavior::default().tokenize(|content| {
        if content.starts_with('\n') && content.chars().count() > 1 {
            vec![1000]
        } else {
            char_tokens(content)
        }
    }))
    .await;
    let merged = classify(&model(&merging.base_url), &pick_context(), &options()).await;
    assert_eq!(merged.stop_reason, ClassifierStopReason::Stop);

    // The default fake tokenizer splits "Yes" into three tokens.
    let split = FakeServer::start().await;
    let ok = context_of(vec![("ok", boolean("OK?", "", ""))]);
    let result = classify(&model(&split.base_url), &ok, &options()).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert!(
        result
            .error_message
            .as_deref()
            .unwrap()
            .contains("Label \"Yes\" is not a single token for qwen"),
        "{:?}",
        result.error_message
    );
}

/// test:292-304
#[tokio::test]
async fn caches_label_tokens_per_server_and_model() {
    let server = FakeServer::start().await;
    let classifier_model = model(&server.base_url);
    let context = pick_context();
    classify(&classifier_model, &context, &options()).await;
    let first = server.requests_to("/tokenize").len();
    classify(&classifier_model, &context, &options()).await;

    assert!(first > 0);
    assert_eq!(server.requests_to("/tokenize").len(), first);

    // The cache is per server: the same model id and labels on another server are looked up
    // again, because that server's vocabulary may differ.
    let other_server = FakeServer::start().await;
    classify(&model(&other_server.base_url), &context, &options()).await;
    assert!(
        !other_server.requests_to("/tokenize").is_empty(),
        "a second server must not be served from the first server's label tokens"
    );
}

/// llama-cpp-classify.ts:323 evicts a label whose lookup rejected (`pending.catch(() =>
/// labelTokenCache.delete(key))`): a failed lookup leaves nothing in the process-global cache, and a
/// lookup that resolved stays.
#[tokio::test]
async fn a_failed_label_lookup_leaves_no_cache_entry_and_a_resolved_one_stays() {
    use super::llama_cpp_classify_fake_server::Reply;
    use crate::api::llama_cpp_classify::label_cache_entries_for;

    let failing = FakeServer::with(Behavior::default().intercept(|request, _| {
        (request.path == "/tokenize").then(|| Reply::status(400, "no tokenizer"))
    }))
    .await;
    let failing_model = model(&failing.base_url);
    let failing_root = llama_server_root(&failing_model.base_url);
    let mut opts = options();
    opts.max_retries = 0;

    let result = classify(&failing_model, &pick_context(), &opts).await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert_eq!(
        label_cache_entries_for(&failing_root),
        0,
        "a failed lookup must not be retained"
    );

    let server = FakeServer::start().await;
    let ok_model = model(&server.base_url);
    let result = classify(&ok_model, &pick_context(), &options()).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
    assert_eq!(
        label_cache_entries_for(&llama_server_root(&ok_model.base_url)),
        2,
        "the two resolved labels stay cached"
    );
}

/// test:306-323
#[tokio::test]
async fn validates_option_counts_before_sending_requests() {
    let server = FakeServer::start().await;
    let many: Vec<(String, &str)> = (0..63)
        .map(|index| (format!("option{index}"), ""))
        .collect();
    let many: Vec<(&str, &str)> = many
        .iter()
        .map(|(key, value)| (key.as_str(), *value))
        .collect();
    let too_many = classify(
        &model(&server.base_url),
        &context_of(vec![("pick", choice("Pick", &many))]),
        &options(),
    )
    .await;
    let too_few = classify(
        &model(&server.base_url),
        &context_of(vec![(
            "rate",
            ClassifierQuestion::Score {
                instructions: "Rate".to_string(),
                criteria: vec!["only".to_string()],
            },
        )]),
        &options(),
    )
    .await;

    assert!(
        too_many
            .error_message
            .as_deref()
            .unwrap()
            .contains("A choice question needs 2 to 62 options, got 63")
    );
    assert!(
        too_few
            .error_message
            .as_deref()
            .unwrap()
            .contains("A score question needs 2 to 10 levels, got 1")
    );
    assert!(server.requests().is_empty());
}

/// test:325-350
#[tokio::test]
async fn passes_completion_payloads_and_responses_through_the_request_hooks() {
    let server = FakeServer::start().await;
    let payloads: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let statuses: Arc<Mutex<Vec<u16>>> = Arc::new(Mutex::new(Vec::new()));
    let mut opts = options();
    let seen = payloads.clone();
    opts.on_payload = Some(Arc::new(move |payload: Value, _model| {
        seen.lock().unwrap().push(payload.clone());
        let mut replaced = payload;
        replaced["id_slot"] = json!(1);
        async move { Some(replaced) }.boxed()
    }));
    let seen = statuses.clone();
    opts.on_response = Some(Arc::new(move |response: ProviderResponse, _model| {
        seen.lock().unwrap().push(response.status);
        async {}.boxed()
    }));

    classify(&model(&server.base_url), &pick_context(), &opts).await;

    let payloads = payloads.lock().unwrap();
    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0]["n_predict"], 1);
    assert_eq!(*statuses.lock().unwrap(), [200]);
    assert_eq!(server.requests_to("/completion")[0].body["id_slot"], 1);
}

/// test:352-371
#[tokio::test]
async fn reports_server_errors_and_cancellation() {
    let failing = FakeServer::with(Behavior::default().intercept(|_, _| {
        Some(super::llama_cpp_classify_fake_server::Reply::status(
            400,
            r#"{"error":{"message":"context overflow"}}"#,
        ))
    }))
    .await;
    let mut opts = options();
    opts.max_retries = 0;
    let result = classify(&model(&failing.base_url), &ticket_context(), &opts).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    let message = result.error_message.unwrap();
    assert!(message.contains("llama.cpp error (400)"), "{message}");
    assert!(message.contains("context overflow"), "{message}");

    let server = FakeServer::start().await;
    let cancel = CancelToken::new();
    cancel.cancel();
    let mut opts = options();
    opts.cancel = Some(cancel);
    let aborted = classify(&model(&server.base_url), &ticket_context(), &opts).await;
    assert_eq!(aborted.stop_reason, ClassifierStopReason::Aborted);
    assert!(server.requests().is_empty());
}

/// A successful reply larger than the cap is refused, not buffered for the length of the timeout,
/// and an oversized error body is cut, not refused.
#[tokio::test]
async fn an_oversized_reply_is_refused_and_an_oversized_error_body_is_cut() {
    use super::llama_cpp_classify_fake_server::Reply;
    use crate::api::llama_cpp_classify::MAX_RESPONSE_BYTES;

    let big = FakeServer::with(Behavior::default().intercept(|request, _| {
        (request.path == "/completion")
            .then(|| Reply::status(200, &"x".repeat(MAX_RESPONSE_BYTES + 1)))
    }))
    .await;
    let mut opts = options();
    opts.max_retries = 0;
    let result = classify(&model(&big.base_url), &pick_context(), &opts).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    let message = result.error_message.unwrap();
    assert!(
        message.contains(&format!("response exceeds {MAX_RESPONSE_BYTES} bytes")),
        "{message}"
    );

    let noisy = FakeServer::with(Behavior::default().intercept(|request, _| {
        (request.path == "/completion").then(|| Reply::status(400, &"e".repeat(3 * 1024 * 1024)))
    }))
    .await;
    let result = classify(&model(&noisy.base_url), &pick_context(), &opts).await;
    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    let message = result.error_message.unwrap();
    assert!(message.starts_with("llama.cpp error (400): e"), "{message}");
    assert!(
        message.len() < 64 * 1024,
        "the shown body is cut: {}",
        message.len()
    );
}

/// test:373-379
#[tokio::test]
async fn rejects_models_for_other_classifier_apis() {
    let server = FakeServer::start().await;
    let mut other = model(&server.base_url);
    other.api = "typesafe-system-one".into();

    let result = classify(&other, &ticket_context(), &options()).await;

    assert!(
        result
            .error_message
            .as_deref()
            .unwrap()
            .contains("Unsupported classifier API: typesafe-system-one")
    );
    assert!(server.requests().is_empty());
}

/// test:381-385
#[test]
fn derives_the_server_root_from_openai_compatible_base_urls() {
    assert_eq!(
        llama_server_root("http://127.0.0.1:8080/v1/"),
        "http://127.0.0.1:8080"
    );
    assert_eq!(
        llama_server_root("https://example.com/prefix/v1"),
        "https://example.com/prefix"
    );
    assert_eq!(
        llama_server_root("http://127.0.0.1:8080"),
        "http://127.0.0.1:8080"
    );
}

/// test:387-398
#[test]
fn computes_typesafes_confidence_and_expected_scores() {
    assert!((peak_confidence(&[0.89, 0.06, 0.05]) - 0.835).abs() < 1e-9);
    assert_eq!(peak_confidence(&[0.5, 0.5]), 0.0);
    assert_eq!(peak_confidence(&[1.0, 0.0, 0.0]), 1.0);
    assert_eq!(
        answer_from_probabilities(
            &ClassifierQuestion::Score {
                instructions: String::new(),
                criteria: vec!["a".into(), "b".into(), "c".into()],
            },
            &["0".to_string(), "1".to_string(), "2".to_string()],
            &[0.2, 0.3, 0.5],
        ),
        ClassifierAnswer::Score {
            score: 1.3,
            confidence: peak_confidence(&[0.2, 0.3, 0.5]),
        }
    );
}

//! Prompt rendering and probability arithmetic of `llama-cpp-classify`, beyond the cases of pi's
//! test file: the exact prompt layout for each question type and edge, `JSON.stringify(state, null,
//! 1)` text for numbers and nesting (`llama-cpp-classify.ts:99-101`), the label sets and their
//! limits (`:39-41`, `:103-120`), and the softmax / confidence / answer helpers (`:179-219`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use serde_json::json;

use super::llama_cpp_classify_fake_server::{
    FakeServer, boolean, choice, classify, context_of, model, options, score,
};
use crate::api::llama_cpp_classify::{
    answer_from_probabilities, label_probabilities, peak_confidence, render_question,
};
use crate::classifier::{ClassifierAnswer, ClassifierContext, ClassifierStopReason};

fn with_state(
    state: serde_json::Value,
    questions: Vec<(&str, crate::classifier::ClassifierQuestion)>,
) -> ClassifierContext {
    let mut context = context_of(questions);
    context.state = state.as_object().unwrap().clone();
    context
}

// ------------------------------------------------------------------------------------ rendering --

/// llama-cpp-classify.ts:99-101: `JSON.stringify(state, null, 1)` indents by one space, nests
/// arrays and objects, keeps key order, leaves non-ASCII alone and prints empty containers as
/// `{}` / `[]`.
#[test]
fn the_state_is_json_with_a_one_space_indent() {
    let context = with_state(
        json!({
            "zeta": 1,
            "alpha": [1, "two", { "three": null }],
            "empty": {},
            "none": [],
            "text": "h\u{e9}llo \"q\"\n",
            "flag": true,
        }),
        vec![("q", boolean("Q?", "", ""))],
    );
    let content = render_question(&context, "q").unwrap().content;
    let expected = [
        "State:",
        "{",
        " \"zeta\": 1,",
        " \"alpha\": [",
        "  1,",
        "  \"two\",",
        "  {",
        "   \"three\": null",
        "  }",
        " ],",
        " \"empty\": {},",
        " \"none\": [],",
        " \"text\": \"h\u{e9}llo \\\"q\\\"\\n\",",
        " \"flag\": true",
        "}",
    ]
    .join("\n");
    assert!(content.starts_with(&expected), "{content}");
}

/// llama-cpp-classify.ts:99-101: JS prints numbers by `Number::toString`, not by serde's rules.
#[test]
fn state_numbers_print_as_javascript_prints_them() {
    let context = with_state(
        json!({
            "whole": 1.0,
            "negative_whole": -3.0,
            "negative_zero": -0.0,
            "half": 0.5,
            "small": 0.000001,
            "tiny": 1e-7,
            "wide": 123456789012345680000.0,
            "huge": 1e21,
            "huge_digits": 1.5e300,
            "tiny_digits": 1.5e-9,
            "int": 42,
        }),
        vec![("q", boolean("Q?", "", ""))],
    );
    let content = render_question(&context, "q").unwrap().content;
    for line in [
        " \"whole\": 1,",
        " \"negative_whole\": -3,",
        " \"negative_zero\": 0,",
        " \"half\": 0.5,",
        " \"small\": 0.000001,",
        " \"tiny\": 1e-7,",
        " \"wide\": 123456789012345680000,",
        " \"huge\": 1e+21,",
        " \"huge_digits\": 1.5e+300,",
        " \"tiny_digits\": 1.5e-9,",
        " \"int\": 42",
    ] {
        assert!(content.contains(line), "missing {line:?} in\n{content}");
    }
}

/// `JSON.stringify` prints a JS number: every integer is a double, so one beyond 2^53 prints as the
/// nearest double, where serde would write the exact digits.
#[test]
fn state_integers_beyond_two_to_the_53_print_as_the_nearest_double() {
    let mut context = context_of(vec![("q", boolean("Q?", "", ""))]);
    context.state = serde_json::from_str(
        r#"{"safe": 9007199254740991, "odd": 9007199254740993, "max": 18446744073709551615, "negative": -9007199254740993, "small": -7}"#,
    )
    .unwrap();

    let content = render_question(&context, "q").unwrap().content;

    for line in [
        " \"safe\": 9007199254740991,",
        " \"odd\": 9007199254740992,",
        " \"max\": 18446744073709552000,",
        " \"negative\": -9007199254740992,",
        " \"small\": -7",
    ] {
        assert!(content.contains(line), "missing {line:?} in\n{content}");
    }
}

/// JS lists integer-like keys first and ascending, at every nesting level, in the printed state.
#[test]
fn state_keys_print_in_js_own_key_order() {
    let mut context = context_of(vec![("q", boolean("Q?", "", ""))]);
    context.state =
        serde_json::from_str(r#"{"b": 1, "1": {"y": 1, "2": 2, "1": [{"z": 0, "0": 0}]}, "0": 3}"#)
            .unwrap();

    let content = render_question(&context, "q").unwrap().content;

    let expected = [
        "State:",
        "{",
        " \"0\": 3,",
        " \"1\": {",
        "  \"1\": [",
        "   {",
        "    \"0\": 0,",
        "    \"z\": 0",
        "   }",
        "  ],",
        "  \"2\": 2,",
        "  \"y\": 1",
        " },",
        " \"b\": 1",
        "}",
    ]
    .join("\n");
    assert!(content.starts_with(&expected), "{content}");
}

/// A choice question's letters go to its options in JS key order, not insertion order: pi's
/// `Object.keys({b: .., "1": ..})` is `["1", "b"]`, so `A` is `1`.
#[test]
fn choice_labels_follow_js_key_order() {
    let context = context_of(vec![(
        "q",
        choice("Pick", &[("b", "bee"), ("1", "one"), ("a", "ay")]),
    )]);

    let rendered = render_question(&context, "q").unwrap();

    assert_eq!(rendered.keys, ["1", "b", "a"]);
    assert_eq!(rendered.labels, ["A", "B", "C"]);
    assert!(
        rendered
            .content
            .contains("Options:\nA. 1: one\nB. b: bee\nC. a: ay"),
        "{}",
        rendered.content
    );
}

/// llama-cpp-classify.ts:99-101: an empty state is `{}`.
#[test]
fn an_empty_state_is_an_empty_object() {
    let content = render_question(&context_of(vec![("q", boolean("Q?", "", ""))]), "q")
        .unwrap()
        .content;
    assert!(content.starts_with("State:\n{}\n\nTask:"), "{content}");
}

/// llama-cpp-classify.ts:149-157: one question gets the singular task line.
#[test]
fn a_single_question_gets_the_singular_task_line() {
    let context = context_of(vec![("q", boolean("Is it?", "", ""))]);
    assert_eq!(
        render_question(&context, "q").unwrap().content,
        [
            "State:\n{}",
            "Task: answer the following question about the state.\n\nQuestion: Is it?",
            "State:\n{}",
            "Question: Is it?\n\nAnswer Yes or No.",
        ]
        .join("\n\n")
    );
}

/// llama-cpp-classify.ts:136-140: each bool meaning is printed only when it is non-empty, and a
/// bool question with neither has no meaning lines at all.
#[test]
fn bool_meanings_are_printed_only_when_given() {
    let both = render_question(
        &context_of(vec![("q", boolean("Q?", "yes text", "no text"))]),
        "q",
    )
    .unwrap()
    .content;
    assert!(
        both.ends_with(
            "Question: Q?\n\nYes means: yes text\nNo means: no text\n\nAnswer Yes or No."
        )
    );

    let only_yes = render_question(&context_of(vec![("q", boolean("Q?", "yes text", ""))]), "q")
        .unwrap()
        .content;
    assert!(only_yes.ends_with("Question: Q?\n\nYes means: yes text\n\nAnswer Yes or No."));

    let only_no = render_question(&context_of(vec![("q", boolean("Q?", "", "no text"))]), "q")
        .unwrap()
        .content;
    assert!(only_no.ends_with("Question: Q?\n\nNo means: no text\n\nAnswer Yes or No."));

    let neither = render_question(&context_of(vec![("q", boolean("Q?", "", ""))]), "q")
        .unwrap()
        .content;
    assert!(neither.ends_with("Question: Q?\n\nAnswer Yes or No."));
}

/// llama-cpp-classify.ts:103-120: the label and key sets of each question type.
#[test]
fn labels_and_keys_follow_the_question_type() {
    let bool_question =
        render_question(&context_of(vec![("q", boolean("Q?", "", ""))]), "q").unwrap();
    assert_eq!(bool_question.labels, ["Yes", "No"]);
    assert_eq!(bool_question.keys, ["true", "false"]);

    let score_question = render_question(
        &context_of(vec![("q", score("Rate", &["a", "b", "c", "d"]))]),
        "q",
    )
    .unwrap();
    assert_eq!(score_question.labels, ["0", "1", "2", "3"]);
    assert_eq!(score_question.keys, ["0", "1", "2", "3"]);

    let pairs: Vec<(String, &str)> = (0..62).map(|index| (format!("k{index}"), "")).collect();
    let pairs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(key, value)| (key.as_str(), *value))
        .collect();
    let widest = render_question(&context_of(vec![("q", choice("Pick", &pairs))]), "q").unwrap();
    assert_eq!(widest.labels.len(), 62);
    assert_eq!(widest.labels.first().map(String::as_str), Some("A"));
    assert_eq!(widest.labels.get(25).map(String::as_str), Some("Z"));
    assert_eq!(widest.labels.get(26).map(String::as_str), Some("a"));
    assert_eq!(widest.labels.get(51).map(String::as_str), Some("z"));
    assert_eq!(widest.labels.get(52).map(String::as_str), Some("0"));
    assert_eq!(widest.labels.last().map(String::as_str), Some("9"));
    assert_eq!(widest.keys.last().map(String::as_str), Some("k61"));
}

/// llama-cpp-classify.ts:107-108, :113-114: the option-count limits are inclusive at both ends.
#[test]
fn option_count_limits_are_inclusive() {
    let build = |count: usize| {
        let pairs: Vec<(String, &str)> =
            (0..count).map(|index| (format!("k{index}"), "")).collect();
        let pairs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (key.as_str(), *value))
            .collect();
        render_question(&context_of(vec![("q", choice("Pick", &pairs))]), "q")
    };
    assert!(build(1).is_err());
    assert!(build(2).is_ok());
    assert!(build(62).is_ok());
    assert!(build(63).is_err());
    assert_eq!(
        build(1).unwrap_err().to_string(),
        "A choice question needs 2 to 62 options, got 1"
    );

    let levels = |count: usize| {
        let levels: Vec<String> = (0..count).map(|index| format!("l{index}")).collect();
        let levels: Vec<&str> = levels.iter().map(String::as_str).collect();
        render_question(&context_of(vec![("q", score("Rate", &levels))]), "q")
    };
    assert!(levels(1).is_err());
    assert!(levels(2).is_ok());
    assert!(levels(10).is_ok());
    assert_eq!(
        levels(11).unwrap_err().to_string(),
        "A score question needs 2 to 10 levels, got 11"
    );
}

/// llama-cpp-classify.ts:172: asking for a question the context does not have is an error.
#[test]
fn an_unknown_question_is_an_error() {
    let context = context_of(vec![("q", boolean("Q?", "", ""))]);
    assert_eq!(
        render_question(&context, "missing")
            .unwrap_err()
            .to_string(),
        "Unknown question: missing"
    );
}

/// llama-cpp-classify.ts:442: a bad question anywhere in the request fails the whole call before
/// any request is sent, even when it is not the first.
#[tokio::test]
async fn a_bad_question_after_a_good_one_fails_before_any_request() {
    let server = FakeServer::start().await;
    let context = context_of(vec![
        ("good", choice("Pick", &[("a", ""), ("b", "")])),
        ("bad", score("Rate", &["only"])),
    ]);

    let result = classify(&model(&server.base_url), &context, &options()).await;

    assert_eq!(result.stop_reason, ClassifierStopReason::Error);
    assert!(result.answers.is_empty());
    assert!(server.requests().is_empty());
}

/// llama-cpp-classify.ts:437-440: the temperature message prints the number as JS does.
#[tokio::test]
async fn invalid_temperatures_are_reported_as_javascript_prints_them() {
    let server = FakeServer::start().await;
    for (temperature, text) in [
        (f64::NAN, "NaN"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
        (-1.0, "-1"),
        (-0.0, "0"),
        (-0.25, "-0.25"),
    ] {
        let mut opts = options();
        opts.temperature = temperature;
        let result = classify(
            &model(&server.base_url),
            &context_of(vec![("q", boolean("Q?", "", ""))]),
            &opts,
        )
        .await;
        assert_eq!(result.stop_reason, ClassifierStopReason::Error);
        assert_eq!(
            result.error_message.as_deref(),
            Some(format!("Temperature must be a positive number, got {text}").as_str())
        );
    }
    assert!(server.requests().is_empty());
}

// --------------------------------------------------------------------------------- probabilities --

/// llama-cpp-classify.ts:179-186: a softmax that is stable for very negative inputs and that
/// sharpens below a temperature of 1 and softens above it.
#[test]
fn label_probabilities_are_a_stable_softmax() {
    let huge_negative = label_probabilities(&[-1000.0, -1001.0], 1.0);
    assert!((huge_negative.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    assert!(huge_negative[0] > huge_negative[1]);

    let cold = label_probabilities(&[-0.1, -2.5], 0.5);
    let warm = label_probabilities(&[-0.1, -2.5], 1.0);
    let hot = label_probabilities(&[-0.1, -2.5], 4.0);
    assert!(cold[0] > warm[0] && warm[0] > hot[0]);
    assert!((warm[0] - 1.0 / (1.0 + (-2.4_f64).exp())).abs() < 1e-12);
    assert_eq!(
        label_probabilities(&[-1.0, -1.0, -1.0], 1.0),
        vec![1.0 / 3.0; 3]
    );
}

/// llama-cpp-classify.ts:188-193: `(n * peak - 1) / (n - 1)` clamped to `[0, 1]`.
#[test]
fn peak_confidence_is_scaled_by_the_label_count() {
    assert!((peak_confidence(&[0.5, 0.25, 0.25]) - 0.25).abs() < 1e-12);
    assert!((peak_confidence(&[0.25; 4])).abs() < 1e-12);
    assert_eq!(peak_confidence(&[0.0, 1.0]), 1.0);
    // Below 1/n is impossible for a real distribution; the clamp still bounds it.
    assert_eq!(peak_confidence(&[0.1, 0.1, 0.1]), 0.0);
}

/// llama-cpp-classify.ts:201-203: a bool answer is the probability of the `true` key, wherever it
/// sits in the key order.
#[test]
fn a_bool_answer_reads_the_true_key() {
    let question = boolean("Q?", "", "");
    let keys = ["true".to_string(), "false".to_string()];
    assert_eq!(
        answer_from_probabilities(&question, &keys, &[0.8, 0.2]),
        ClassifierAnswer::Bool { probability: 0.8 }
    );
    let swapped = ["false".to_string(), "true".to_string()];
    assert_eq!(
        answer_from_probabilities(&question, &swapped, &[0.8, 0.2]),
        ClassifierAnswer::Bool { probability: 0.2 }
    );
}

/// llama-cpp-classify.ts:209-218: the choice is the first of the most probable keys, and every key
/// keeps its probability in key order.
#[test]
fn a_choice_answer_takes_the_first_most_probable_key() {
    let question = choice("Pick", &[("x", ""), ("y", ""), ("z", "")]);
    let keys = ["x".to_string(), "y".to_string(), "z".to_string()];

    let answer = answer_from_probabilities(&question, &keys, &[0.2, 0.4, 0.4]);
    match answer {
        ClassifierAnswer::Choice {
            choice,
            probabilities,
            confidence,
        } => {
            assert_eq!(choice, "y");
            assert_eq!(
                probabilities
                    .iter()
                    .map(|(key, value)| (key.to_string(), *value))
                    .collect::<Vec<_>>(),
                [
                    ("x".to_string(), 0.2),
                    ("y".to_string(), 0.4),
                    ("z".to_string(), 0.4)
                ]
            );
            assert_eq!(confidence, peak_confidence(&[0.2, 0.4, 0.4]));
        }
        other => panic!("{other:?}"),
    }

    match answer_from_probabilities(&question, &keys, &[0.6, 0.2, 0.2]) {
        ClassifierAnswer::Choice { choice, .. } => assert_eq!(choice, "x"),
        other => panic!("{other:?}"),
    }
    match answer_from_probabilities(&question, &keys, &[0.1, 0.1, 0.8]) {
        ClassifierAnswer::Choice { choice, .. } => assert_eq!(choice, "z"),
        other => panic!("{other:?}"),
    }
}

/// llama-cpp-classify.ts:205-208: a score answer is the probability-weighted level.
#[test]
fn a_score_answer_is_the_expected_level() {
    let question = score("Rate", &["a", "b", "c"]);
    let keys = ["0".to_string(), "1".to_string(), "2".to_string()];
    match answer_from_probabilities(&question, &keys, &[0.0, 0.0, 1.0]) {
        ClassifierAnswer::Score { score, confidence } => {
            assert_eq!(score, 2.0);
            assert_eq!(confidence, 1.0);
        }
        other => panic!("{other:?}"),
    }
    match answer_from_probabilities(&question, &keys, &[1.0, 0.0, 0.0]) {
        ClassifierAnswer::Score { score, .. } => assert_eq!(score, 0.0),
        other => panic!("{other:?}"),
    }
}

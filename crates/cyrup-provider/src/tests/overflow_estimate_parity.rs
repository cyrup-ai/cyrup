//! Parity regressions for `utils/overflow.rs` and `utils/estimate.rs` against pi v0.83.0
//! (`packages/ai/src/utils/{overflow,estimate}.ts`).
//!
//! Two ported-behaviour gaps are pinned here:
//!
//! 1. `OVERFLOW_PATTERNS` must carry all 25 of Pi's entries — the DS4-server and DashScope/Qwen
//!    patterns (`overflow.ts:55` and `:58` at v0.83.0) were missing, so those servers' 400s were
//!    classified as ordinary errors and failed the turn instead of triggering compaction.
//! 2. `getLastAssistantUsageInfo`'s `latestPrefixTimestamp` guard (`estimate.ts:64-84`) — a usage
//!    block only describes the current prefix when no NEWER message precedes it in the list. A
//!    compaction summary spliced in ahead of an older assistant turn invalidates that turn's usage.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::utils::estimate::estimate_message_list_tokens;
use crate::utils::overflow::{is_context_overflow, overflow_patterns};
use cyrup_core::{AssistantMessage, Content, Message, ProviderId, StopReason, Usage};

fn err(message: &str) -> AssistantMessage {
    AssistantMessage::errored(
        ProviderId::from("openai-completions"),
        "some-model",
        None,
        StopReason::Error,
        message,
    )
}

/// The two patterns Pi carries that cyrup dropped (`overflow.ts:55` DS4, `:58` DashScope/Qwen),
/// with the verbatim error strings Pi's own header comment documents (`overflow.ts:28` / `:34`).
#[test]
fn detects_ds4_and_dashscope_overflow_errors() {
    let cases = [
        // DS4 server — `/prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?/i`
        "Prompt has 9,000 tokens, but the configured context size is 8,192 tokens",
        // Unpunctuated / singular variants the same pattern must still cover.
        "prompt has 1 token, but the configured context size is 8192 tokens",
        // DashScope / Qwen Token Plan — `/range of input length should be/i`, HTTP 400
        // invalid_parameter_error.
        "Range of input length should be [1, 129024]",
    ];
    for c in cases {
        assert!(
            is_context_overflow(&err(c), None),
            "should detect overflow: {c}"
        );
    }
}

/// Guard the whole set, not just the two additions: Pi v0.83.0 `OVERFLOW_PATTERNS` has 25 entries
/// (`overflow.ts:37-62`). A count assertion catches a future silent drop the same way.
#[test]
fn overflow_pattern_set_matches_pi_cardinality() {
    assert_eq!(
        overflow_patterns().len(),
        25,
        "pi v0.83.0 overflow.ts:37-62 defines 25 patterns; got {:?}",
        overflow_patterns()
    );
    // The two restored entries are present verbatim, in Pi's source form.
    assert!(overflow_patterns().contains(
        &r"prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?"
    ));
    assert!(overflow_patterns().contains(&r"range of input length should be"));
}

/// The DashScope pattern must not swallow unrelated errors that merely mention input length.
#[test]
fn dashscope_pattern_does_not_overmatch() {
    assert!(!is_context_overflow(
        &err("input length should be validated before dispatch"),
        None
    ));
    assert!(!is_context_overflow(
        &err("prompt has 9000 tokens, but the model is unavailable"),
        None
    ));
}

fn user(text: &str, timestamp: i64) -> Message {
    Message::User {
        content: vec![Content::text(text)],
        timestamp,
    }
}

fn assistant_with_usage(total_tokens: u64, timestamp: i64) -> Message {
    let mut m = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude",
        None,
        StopReason::Stop,
        "",
    );
    m.error_message = None;
    m.usage = Usage {
        total_tokens,
        ..Usage::default()
    };
    m.timestamp = timestamp;
    Message::Assistant(m)
}

/// Pi `getLastAssistantUsageInfo`, estimate.ts:64-84: an assistant response whose timestamp is
/// OLDER than a message already seen earlier in the list cannot describe the current prefix, so its
/// usage is skipped.
///
/// The shape below is what a second compaction produces: the summary user message is written with
/// `Date.now()` and spliced in at the head, ahead of the retained tail of the previous conversation
/// (whose assistant turns keep their original, older timestamps).
#[test]
fn stale_usage_behind_a_newer_prefix_message_is_ignored() {
    let messages = vec![
        // Freshly-minted compaction summary (newest timestamp, sits first).
        user("<compaction summary of the previous session>", 5_000),
        // Retained tail: an OLD assistant turn reporting a large pre-compaction context.
        assistant_with_usage(150_000, 1_000),
        user("what next?", 6_000),
    ];

    let estimate = estimate_message_list_tokens(&messages);

    assert_eq!(
        estimate.last_usage_index, None,
        "the 150k usage predates the compaction summary and must not be adopted"
    );
    assert_eq!(estimate.usage_tokens, 0);
    // Falls back to the character-count estimate over every message, which is nowhere near 150k.
    assert!(
        estimate.tokens < 1_000,
        "expected the char-count fallback, got {}",
        estimate.tokens
    );
}

/// The complementary case: once a NEW assistant turn lands after the summary, its usage is the
/// applicable one — and the stale older turn still does not win, even though it appears later than
/// nothing and carries a bigger number.
#[test]
fn usage_after_the_newer_prefix_message_is_adopted() {
    let messages = vec![
        user("<compaction summary>", 5_000),
        assistant_with_usage(150_000, 1_000), // stale — older than the summary
        user("what next?", 6_000),
        assistant_with_usage(9_000, 7_000), // fresh — describes the post-compaction prefix
    ];

    let estimate = estimate_message_list_tokens(&messages);

    assert_eq!(estimate.last_usage_index, Some(3));
    assert_eq!(estimate.usage_tokens, 9_000);
    assert_eq!(estimate.trailing_tokens, 0);
    assert_eq!(estimate.tokens, 9_000);
}

/// A monotonically-timestamped conversation is unaffected: the LAST applicable assistant usage
/// still wins, exactly as before the guard was ported.
#[test]
fn monotonic_conversation_still_uses_the_last_assistant_usage() {
    let messages = vec![
        user("hi", 1),
        assistant_with_usage(100, 2),
        user("more", 3),
        assistant_with_usage(400, 4),
        user("trailing text", 5),
    ];

    let estimate = estimate_message_list_tokens(&messages);

    assert_eq!(estimate.last_usage_index, Some(3));
    assert_eq!(estimate.usage_tokens, 400);
    assert_eq!(estimate.tokens, 400 + estimate.trailing_tokens);
}

/// Equal timestamps are inclusive (`>=`, estimate.ts:70) — a same-millisecond prefix message does
/// NOT invalidate the response, which matters because Pi stamps several messages from one
/// `Date.now()` read.
#[test]
fn equal_timestamps_do_not_invalidate_usage() {
    let messages = vec![user("hi", 42), assistant_with_usage(321, 42)];

    let estimate = estimate_message_list_tokens(&messages);

    assert_eq!(estimate.last_usage_index, Some(1));
    assert_eq!(estimate.usage_tokens, 321);
}

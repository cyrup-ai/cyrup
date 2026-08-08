//! Characterization test: the `getLastAssistantUsageInfo` newer-prefix-timestamp guard belongs to
//! Pi's `packages/ai/src/utils/estimate.ts` ONLY — it does **not** exist in either copy of
//! `compaction.ts`, which is what `cyrup-session`'s estimators port.
//!
//! Ground truth at the ported baseline (pi v0.83.0), re-derived with `git show`, not from memory:
//!
//! * `packages/ai/src/utils/estimate.ts` — `getLastAssistantUsageInfo` opens with
//!   `let latestPrefixTimestamp = Number.NEGATIVE_INFINITY`, gates each candidate on
//!   `const usageAppliesToPrefix = assistant.timestamp >= latestPrefixTimestamp`, and closes the
//!   loop body with `latestPrefixTimestamp = Math.max(latestPrefixTimestamp, message.timestamp)`.
//!   Ported by `cyrup-provider/src/utils/estimate.rs`.
//! * `packages/agent/src/harness/compaction/compaction.ts` — `getLastAssistantUsageInfo` is
//!   `for (let i = messages.length - 1; i >= 0; i--) { const usage = getAssistantUsage(messages[i]);
//!   if (usage) return { usage, index: i }; }`. No timestamps anywhere. Ported by
//!   `cyrup-session/src/compaction/tokens.rs`.
//! * `packages/coding-agent/src/core/compaction/compaction.ts` — byte-identical reverse scan, also
//!   guard-free. This is the copy `agent-session.ts` imports (`from "./compaction/index.ts"`), so
//!   the guard is absent on Pi's live auto-compaction path too.
//!
//! So the two estimators legitimately DISAGREE on a history whose head carries a timestamp newer
//! than a later assistant response — the classic post-compaction shape, where the summary is
//! spliced at index 0 with `Date.now()` while the kept tail retains its original, older stamps.
//! Adding the guard to `tokens.rs` to make them agree would *invent* a divergence from Pi, not
//! close one. This file pins both halves so that stays true.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cyrup_core::{AssistantMessage, Content, Message, StopReason, Usage};
use cyrup_provider::Context;
use cyrup_session::agent_message::{AgentMessage, CompactionSummaryMessage};
use cyrup_session::compaction::tokens::{estimate_context_tokens, estimate_context_tokens_raw};

/// Tokens reported by the one assistant message that carries usage in every fixture below.
const ANCHOR_USAGE_TOKENS: u64 = 5_000;

/// Timestamp stamped on the spliced compaction summary sitting at index 0.
const SUMMARY_TS: i64 = 1_000;

/// Timestamp on the surviving pre-compaction tail — older than the summary above it.
const KEPT_TAIL_TS: i64 = 500;

fn user(text: &str, timestamp: i64) -> Message {
    Message::User {
        content: vec![Content::text(text)],
        timestamp,
    }
}

fn assistant_with_usage(text: &str, timestamp: i64, total_tokens: u64) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::text(text)],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage {
            total_tokens,
            ..Usage::default()
        },
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp,
    })
}

/// The core-`Message` projection of a post-compaction history: a summary spliced at the head with a
/// **newer** stamp, then the kept tail whose assistant still carries the pre-compaction usage.
fn spliced_history(assistant_ts: i64) -> Vec<Message> {
    vec![
        user("[compaction summary] earlier work elided", SUMMARY_TS),
        user("continue please", KEPT_TAIL_TS),
        assistant_with_usage("resuming", assistant_ts, ANCHOR_USAGE_TOKENS),
    ]
}

/// Same history in the raw `AgentMessage` projection, with the head as a real
/// `compactionSummary` entry rather than its rendered user text.
fn spliced_history_raw(assistant_ts: i64) -> Vec<AgentMessage> {
    vec![
        AgentMessage::CompactionSummary(CompactionSummaryMessage {
            summary: "earlier work elided".to_string(),
            tokens_before: 0,
            timestamp: SUMMARY_TS,
        }),
        AgentMessage::Core(user("continue please", KEPT_TAIL_TS)),
        AgentMessage::Core(assistant_with_usage(
            "resuming",
            assistant_ts,
            ANCHOR_USAGE_TOKENS,
        )),
    ]
}

/// Literal transcription of the *pre-guard* algorithm — Pi `compaction.ts`'s reverse scan. Kept in
/// the test so the discriminating assertions below cannot be vacuous: whenever this returns
/// `Some(_)` and `cyrup_provider`'s estimator returns `None`, the guard is demonstrably live, and
/// whenever they agree the guard is demonstrably inert. Neither assertion can pass by accident.
fn reverse_scan_last_usage(messages: &[Message]) -> Option<usize> {
    for (i, message) in messages.iter().enumerate().rev() {
        if let Message::Assistant(a) = message {
            let valid = !matches!(a.stop_reason, StopReason::Error | StopReason::Aborted);
            let tokens = if a.usage.total_tokens != 0 {
                a.usage.total_tokens
            } else {
                a.usage.input + a.usage.output + a.usage.cache_read + a.usage.cache_write
            };
            if valid && tokens > 0 {
                return Some(i);
            }
        }
    }
    None
}

fn provider_estimate(messages: Vec<Message>) -> cyrup_provider::ContextUsageEstimate {
    cyrup_provider::estimate_context_tokens(&Context {
        system_prompt: None,
        messages,
        tools: Vec::new(),
    })
}

// ------------------------------------------------------------------ the guard is ABSENT here ----

/// Pi `packages/agent/src/harness/compaction/compaction.ts` `getLastAssistantUsageInfo` reverse-
/// scans with no timestamp comparison, so the older assistant's usage IS adopted even though a
/// newer-stamped summary sits ahead of it. `estimate_context_tokens` must match that exactly.
#[test]
fn session_estimator_adopts_usage_behind_a_newer_prefix_like_pi_compaction_ts() {
    let messages = spliced_history(KEPT_TAIL_TS);

    // The pre-guard reverse scan finds the anchor — this is the behaviour Pi's compaction.ts has.
    assert_eq!(
        reverse_scan_last_usage(&messages),
        Some(2),
        "fixture is wrong: the reverse scan must find the assistant at index 2"
    );

    let estimate = estimate_context_tokens(&messages);
    assert_eq!(
        estimate.last_usage_index,
        Some(2),
        "compaction.ts has no latestPrefixTimestamp guard; the anchor must NOT be skipped"
    );
    assert_eq!(
        u64::from(estimate.usage_tokens),
        ANCHOR_USAGE_TOKENS,
        "usage_tokens must be the anchor's reported total"
    );
    assert_eq!(
        estimate.trailing_tokens, 0,
        "nothing follows the anchor, so there is no trailing estimate"
    );
}

/// Same for the raw `AgentMessage` projection used by `prepare_compaction`'s `tokens_before`
/// (Pi `compaction.ts:668`, `estimateContextTokens(buildSessionContext(pathEntries).messages)`).
#[test]
fn raw_session_estimator_adopts_usage_behind_a_newer_prefix_like_pi_compaction_ts() {
    let estimate = estimate_context_tokens_raw(&spliced_history_raw(KEPT_TAIL_TS));
    assert_eq!(
        estimate.last_usage_index,
        Some(2),
        "estimate_context_tokens_raw ports the same guard-free reverse scan"
    );
    assert_eq!(u64::from(estimate.usage_tokens), ANCHOR_USAGE_TOKENS);
}

// ----------------------------------------------------------------- the guard is PRESENT here ----

/// Pi `packages/ai/src/utils/estimate.ts` DOES carry the guard, so on the very same history the
/// provider-side estimator must reject the anchor and fall back to a whole-history char estimate.
/// The contrast with the two tests above is the divergence, and it is intentional on both sides.
#[test]
fn provider_estimator_rejects_usage_behind_a_newer_prefix_like_pi_estimate_ts() {
    let messages = spliced_history(KEPT_TAIL_TS);

    // Discriminator: the pre-guard algorithm accepts the anchor...
    assert_eq!(reverse_scan_last_usage(&messages), Some(2));

    // ...and the guarded port rejects it. If the guard were ever removed from
    // cyrup-provider/src/utils/estimate.rs, these two would agree and this assertion would fail.
    let estimate = provider_estimate(messages);
    assert_eq!(
        estimate.last_usage_index, None,
        "estimate.ts's `usageAppliesToPrefix` gate must skip an assistant older than its prefix"
    );
    assert_eq!(
        estimate.usage_tokens, 0,
        "no applicable usage means usage_tokens is 0, not the stale 5000"
    );
    assert!(
        estimate.tokens < ANCHOR_USAGE_TOKENS,
        "with the anchor rejected the figure is a char estimate, far below the stale usage; got {}",
        estimate.tokens
    );
}

// ------------------------------------------------------------------------------- mirror case ----

/// MIRROR: identical shape, except the assistant's own stamp is NEWER than everything ahead of it,
/// so `usageAppliesToPrefix` is satisfied and the guard is inert. All three estimators must now
/// agree. This case stays green whether or not the guard exists, which is precisely what shows the
/// failing-side assertions above are discriminating on the guard and not on the fixture.
#[test]
fn mirror_all_estimators_agree_when_the_anchor_is_the_newest_message() {
    let newest_ts = SUMMARY_TS + 1_000;
    let messages = spliced_history(newest_ts);

    assert_eq!(reverse_scan_last_usage(&messages), Some(2));

    let session = estimate_context_tokens(&messages);
    assert_eq!(session.last_usage_index, Some(2));
    assert_eq!(u64::from(session.usage_tokens), ANCHOR_USAGE_TOKENS);

    let raw = estimate_context_tokens_raw(&spliced_history_raw(newest_ts));
    assert_eq!(raw.last_usage_index, Some(2));
    assert_eq!(u64::from(raw.usage_tokens), ANCHOR_USAGE_TOKENS);

    let provider = provider_estimate(messages);
    assert_eq!(
        provider.last_usage_index,
        Some(2),
        "the guard must be inert when the anchor is the newest message"
    );
    assert_eq!(provider.usage_tokens, ANCHOR_USAGE_TOKENS);
    assert_eq!(provider.tokens, ANCHOR_USAGE_TOKENS);
}

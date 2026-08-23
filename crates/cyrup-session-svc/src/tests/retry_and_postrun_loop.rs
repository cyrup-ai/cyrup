//! AUTO-RETRY and AUTO-COMPACTION: the two dials, and the post-run loop that reads them.
//!
//! The loop first — proved end to end over a real run — then the dials it reads
//! (`set_retry`/`retry`, `set_auto_compaction`/`auto_compaction`, `is_compacting`, the retryable
//! classification and the backoff schedule), each with its own setter/getter round-trip.
//!
//! ASSEMBLED-RUN proofs for the post-run execution loop (`_runAgentPrompt` /
//! `_handlePostAgentRun`, Pi agent-session.ts:973-1022). These do NOT hand-call `prepare_retry` /
//! `check_compaction`; they drive a REAL `AgentSession` turn to completion over the scripted
//! `FauxProvider` and assert that auto-retry, post-run auto-compaction, the `agent_end.willRetry`
//! payload, and `auto_retry_end{success}` ACTUALLY fire from the wired run path. The session is bound
//! via `into_shared()` exactly as the runtime / SDK / print-mode bind it in production.

use std::sync::Arc;

use cyrup_core::{AssistantMessage, StopReason};
use cyrup_provider::faux::{
    faux_assistant_message, faux_assistant_message_with, faux_text, FauxMessageOptions, FauxProvider,
};
use cyrup_provider::Provider;
use super::common::{base_config, fixture};
use crate::{AgentSessionEvent, InputSource, SessionBuilder, UserInput};
use futures::StreamExt;

/// A near-instant retry backoff so the success path completes promptly.
fn fast_retry_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("retry", serde_json::json!({"enabled": true, "maxRetries": 3, "baseDelayMs": 1}))
        .unwrap();
    cli
}

fn kinds(events: &[AgentSessionEvent]) -> Vec<&'static str> {
    events.iter().map(AgentSessionEvent::kind).collect()
}

// ============================================================================ A.1/A.2/A.3 retry ====

/// The CRITICAL proof: a retryable transient error from a COMPLETED turn is auto-retried by the
/// assembled run path (not a hand-called `prepare_retry`). The provider must be hit a SECOND time
/// (the continuation), `auto_retry_start` then `auto_retry_end{success:true}` must fire, and the
/// first `agent_end` must carry `willRetry:true`.
#[tokio::test]
async fn assembled_run_auto_retries_a_transient_error_then_recovers() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Turn 1 = retryable transient error; turn 2 (the continuation) = clean success.
    faux.set_responses(vec![
        faux_assistant_message_with(
            Vec::new(),
            StopReason::Error,
            FauxMessageOptions { error_message: Some("overloaded".into()), ..Default::default() },
        ),
        faux_assistant_message(vec![faux_text("recovered")], StopReason::Stop),
    ]);

    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(fast_retry_settings())
        .build()
        .await
        .expect("build")
        .into_shared(); // bind the self-handle: the post-run loop is now LIVE.

    let stream = session
        .prompt(UserInput::text("please do the thing", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;
    let ks = kinds(&events);

    // The continuation actually happened in the assembled run: BOTH scripted responses consumed.
    assert_eq!(faux.call_count(), 2, "provider must be hit a second time (the auto-retry continuation)");

    // auto_retry_start fired from the completed turn (NOT a hand call).
    assert!(ks.contains(&"auto_retry_start"), "auto_retry_start must fire from the run: {ks:?}");

    // auto_retry_end{success:true} fired on the recovered message_end + the retry counter reset.
    let retry_end_success = events.iter().any(|e| {
        matches!(e, AgentSessionEvent::AutoRetryEnd { success: true, attempt: 1, .. })
    });
    assert!(retry_end_success, "auto_retry_end{{success:true, attempt:1}} must fire: {ks:?}");
    assert_eq!(session.retry_attempt(), 0, "retry counter resets on the successful continuation");

    // The FIRST agent_end carried willRetry:true; the LAST carried willRetry:false.
    let agent_ends: Vec<bool> = events
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::AgentEnd { will_retry, .. } => Some(*will_retry),
            _ => None,
        })
        .collect();
    assert_eq!(agent_ends.len(), 2, "two agent_end events (error turn + recovered turn): {ks:?}");
    assert!(agent_ends[0], "first agent_end.willRetry must be true (transient error pending retry)");
    assert!(!agent_ends[1], "final agent_end.willRetry must be false (clean success)");

    // The session settled on the recovered answer.
    assert_eq!(session.last_assistant_text().await.as_deref(), Some("recovered"));
}

// ============================================================================ A.1 post-run compact ====

/// A context-overflow error from a COMPLETED turn triggers post-run auto-compaction in the assembled
/// run path — `run_auto_compaction` (a previously-dead method) now fires `compaction_start{overflow}`
/// from a real turn, not a hand call.
#[tokio::test]
async fn assembled_run_triggers_post_run_overflow_compaction() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .build()
        .await
        .expect("build")
        .into_shared();

    // The overflow error must be attributed to the SAME model the session runs (Pi `_checkCompaction`
    // same-model guard), so build it from the live model address.
    let model = session.model().expect("session must have a resolved model");
    let overflow = AssistantMessage::errored(
        model.provider.clone(),
        model.model.as_str(),
        None,
        StopReason::Error,
        "context_length_exceeded",
    );
    faux.set_responses(vec![overflow]);

    assert!(session.auto_compaction_enabled(), "auto-compaction on by default");

    let stream = session
        .prompt(UserInput::text("overflow me", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;
    let ks = kinds(&events);

    // The post-run compaction PRODUCER fired from the assembled run, tagged `overflow`.
    let overflow_start = events.iter().any(|e| {
        serde_json::to_value(e)
            .ok()
            .and_then(|v| {
                Some(
                    v.get("type")?.as_str()? == "compaction_start"
                        && v.get("reason")?.as_str()? == "overflow",
                )
            })
            .unwrap_or(false)
    });
    assert!(overflow_start, "compaction_start{{reason:overflow}} must fire from the run: {ks:?}");
    // A retryable-error path was NOT taken (overflow is excluded from retry).
    assert!(!ks.contains(&"auto_retry_start"), "overflow must NOT be retried: {ks:?}");
}

/// SEAM-112 — after a successful OVERFLOW compaction the interrupted turn must actually be RETRIED.
///
/// pi `agent-session.ts:2307-2317`, its own comment: *"The overflow response was persisted on
/// message_end before _checkCompaction() removed it from agent state. Rebuilding state from the new
/// compaction can restore that kept entry, leaving an assistant as the final message.
/// agent.continue() rejects that state, so remove the retriable error or truncated-length response
/// again before continuing the interrupted turn."*
///
/// cyrup's `run_auto_compaction` had no such re-drop, so the chain broke at its last link:
/// `check_compaction` dropped the trailing assistant, the compaction's re-seed pulled it back out
/// of the session file, `handle_post_agent_run` returned `true`, and `Agent::continue_run`
/// (`cyrup-agent/src/agent.rs:2004-2029`) saw `last_is_assistant` with both queues empty and
/// returned `ContinueFromAssistant` — which `drive_run` (`session.rs:797`) turns into a silent
/// `break`. Overflow recovery compacted and then simply stopped: the user's turn never ran.
///
/// **RED before the fix:** exactly ONE `agent_end` (the overflow turn), `call_count == 3`
/// (turn 1 + the overflow turn + the summarization) and no retried answer — the third scripted
/// response is never requested.
#[tokio::test]
async fn a_successful_overflow_compaction_retries_the_interrupted_turn() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    // keepRecentTokens/reserveTokens at 0 so the two-turn branch really has a preparation.
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(cli)
        .build()
        .await
        .expect("build")
        .into_shared();

    // Turn 1: an ordinary answer, so the branch has something to compact.
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("first answer worth some tokens")],
        StopReason::Stop,
    )]);
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;

    // Turn 2 overflows on the SAME model (pi's `_checkCompaction` same-model guard) with a
    // stop reason other than `Stop`, which is pi's `willRetry` predicate (`agent-session.ts:2032`):
    // `check_compaction` drops the trailing assistant and compacts with `willRetry: true`.
    // Response 2 is the summarization; response 3 is the answer the RETRY must fetch.
    let model = session.model().expect("session must have a resolved model");
    faux.set_responses(vec![
        AssistantMessage::errored(
            model.provider.clone(),
            model.model.as_str(),
            None,
            StopReason::Error,
            "context_length_exceeded",
        ),
        faux_assistant_message(vec![faux_text("CONTEXT SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("RETRIED ANSWER")], StopReason::Stop),
    ]);

    let stream = session.prompt("tell me two").await.expect("prompt accepted");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;
    let ks = kinds(&events);

    // The compaction ran, succeeded, and carried pi's `willRetry` through to its end event.
    let end = events.iter().find_map(|e| match e {
        AgentSessionEvent::CompactionEnd { reason, result, will_retry, .. } => {
            Some((*reason, result.is_some(), *will_retry))
        }
        _ => None,
    });
    assert_eq!(
        end,
        Some((crate::CompactionReason::Overflow, true, true)),
        "the overflow compaction must SUCCEED and report willRetry:true: {ks:?}"
    );

    // …and the interrupted turn was then actually driven. Two `agent_end`s: the overflow turn and
    // the continuation.
    let agent_ends = ks.iter().filter(|k| **k == "agent_end").count();
    assert_eq!(
        agent_ends, 2,
        "overflow recovery must compact AND retry — one agent_end means `continue_run` refused the \
         restored trailing assistant (pi re-drops it, agent-session.ts:2312-2317): {ks:?}"
    );
    assert_eq!(
        faux.call_count(),
        4,
        "turn 1 + the overflow turn + the summarization + the RETRY; 3 means the retry never \
         reached the provider"
    );
    assert_eq!(
        session.last_assistant_text().await.as_deref(),
        Some("RETRIED ANSWER"),
        "the user's interrupted turn must end on the retried answer, not on the overflow error"
    );
}

// ============================================================================ unbound = legacy ====

/// An UNBOUND (plain by-value) session keeps the legacy single-turn behavior: the post-run loop does
/// not run, so a transient error is NOT auto-retried (the provider is hit exactly once) — this guards
/// the bound/unbound split that keeps existing by-value callers unchanged.
#[tokio::test]
async fn unbound_session_does_not_run_the_post_run_loop() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message_with(
            Vec::new(),
            StopReason::Error,
            FauxMessageOptions { error_message: Some("overloaded".into()), ..Default::default() },
        ),
        faux_assistant_message(vec![faux_text("unreached")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(fast_retry_settings())
        .build()
        .await
        .expect("build"); // NOT bound — plain by-value session.

    let stream = session.prompt(UserInput::text("go", InputSource::Sdk)).await.expect("prompt");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;

    assert_eq!(faux.call_count(), 1, "unbound session runs a single turn (no post-run retry)");
    assert!(!kinds(&events).contains(&"auto_retry_start"), "no auto-retry on an unbound session");
}

// ================================================================ the DIALS themselves ====

/// Facade parity vs Pi `agent-session.ts`: the retry subsystem — the enable/disable toggle, which errors are classified
/// retryable, and the backoff schedule handed to the caller.
#[tokio::test]
async fn retry_toggles_classification_and_backoff() {
    let fx = fixture();
    // Fast backoff so the success path completes quickly.
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("retry", serde_json::json!({"enabled": true, "maxRetries": 2, "baseDelayMs": 3}))
        .unwrap();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).cli_settings(cli).build().await.unwrap();

    // Toggle mirrors the settings default, then the override.
    assert!(session.auto_retry_enabled(), "settings default retry.enabled = true");
    session.set_auto_retry_enabled(false);
    assert!(!session.auto_retry_enabled());
    session.set_auto_retry_enabled(true);

    // Classification: a transient error is retryable; a clean stop is not.
    let transient = AssistantMessage::errored(
        "faux".into(),
        "faux-1",
        None,
        StopReason::Error,
        "overloaded: please retry",
    );
    assert!(session.is_retryable_error(&transient), "overloaded is retryable");
    let clean = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
    assert!(!session.is_retryable_error(&clean), "a clean stop is never retryable");

    // will_retry_after_agent_end scans the last assistant message.
    assert!(session
        .will_retry_after_agent_end(&[cyrup_agent::AgentMessage::Assistant(transient.clone())]));
    assert!(!session
        .will_retry_after_agent_end(&[cyrup_agent::AgentMessage::Assistant(clean.clone())]));

    // prepare_retry: first attempt waits the backoff and signals continue; the budget then exhausts.
    assert_eq!(session.retry_attempt(), 0);
    assert!(session.prepare_retry(&transient).await, "attempt 1 continues");
    assert_eq!(session.retry_attempt(), 1);
    assert!(session.prepare_retry(&transient).await, "attempt 2 continues");
    assert_eq!(session.retry_attempt(), 2);
    assert!(!session.prepare_retry(&transient).await, "budget exhausted at maxRetries");
    assert_eq!(session.retry_attempt(), 2, "attempt count is preserved on exhaustion");
    assert!(!session.is_retrying(), "no backoff is in flight after prepare returns");
}

// -------------------------------------------------------------------------- auto-compaction ----

/// Facade parity vs Pi `agent-session.ts`: the auto-compaction toggles — `set_auto_compaction`/`auto_compaction` round-trip
/// and `is_compacting` reports the in-flight state.
#[tokio::test]
async fn auto_compaction_toggle_and_is_compacting() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();

    assert!(session.auto_compaction_enabled(), "settings default compaction.enabled = true");
    assert!(!session.is_compacting(), "nothing compacting at rest");
    session.set_auto_compaction_enabled(false);
    assert!(!session.auto_compaction_enabled());

    // With auto-compaction disabled, check_compaction is a no-op.
    let small = faux_assistant_message(vec![faux_text("hi")], StopReason::Stop);
    assert!(!session.check_compaction(&small, false).await.unwrap(), "disabled = never compacts");
    session.set_auto_compaction_enabled(true);
    // A tiny session is well under threshold → still no compaction.
    assert!(!session.check_compaction(&small, false).await.unwrap(), "small session under threshold");
}

//! The `compaction_end` PAYLOAD: that it carries pi's full field set, and that its
//! `estimatedTokensAfter` is measured over the right projection.
//!
//! `compaction_end`'s `estimatedTokensAfter` must be measured over pi's RAW `AgentMessage`
//! context, not over the `convertToLlm`-flattened one.
//!
//! pi v0.83.0 `packages/coding-agent/src/core/agent-session.ts:1876` (manual `compact`) and
//! `:2157` (`_runAutoCompaction`) both compute
//! `const estimatedTokensAfter = estimateMessagesTokens(sessionContext.messages)`.
//! `estimateMessagesTokens` (`:284-288`) sums `estimateTokens` — the coding-agent compaction fork,
//! `compaction/compaction.ts:266-300` — over `AgentMessage[]` **with roles intact**, because
//! `sessionContext` comes from `buildSessionContext`, i.e.
//! `buildContextEntries(...).flatMap(sessionEntryToContextMessages)`
//! (`session-manager.ts:461-469` composed with `:383-408`), which returns
//! `createCompactionSummaryMessage(...)` / raw `bashExecution` messages and never calls
//! `convertToLlm`. Consequently, in pi:
//!
//! * a retained `compactionSummary` costs `Math.ceil(summary.length / 4)` — **no wrapper prose**;
//! * an `excludeFromContext` (`!!`-prefixed) `bashExecution` still costs
//!   `Math.ceil((command.length + output.length) / 4)`.
//!
//! cyrup measured `guard.build_context()` — the LLM boundary — instead. That projection wraps every
//! compaction summary in `COMPACTION_SUMMARY_PREFIX`/`SUFFIX` (~107 chars ≈ 27 tokens,
//! `cyrup-session/src/context.rs:16-18`) and DROPS every `exclude_from_context` bash message
//! (`AgentMessage::push_llm`). Since a compacted context always leads with a compaction summary,
//! the over-count fired on every single compaction, and it put the two halves of one
//! `compaction_end` event on different bases: `tokens_before` is computed by `prepare_compaction`
//! over the raw projection (`cyrup-session/src/compaction/prepare.rs`).

use std::sync::Arc;
use std::time::Duration;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session::agent_message::AgentMessage;
use cyrup_session::compaction::tokens::estimate_agent_message;
use super::common::{base_config, fixture};
use crate::{AgentSessionEvent, InputSource, SessionBuilder, UserInput};
use futures::StreamExt;

/// Compaction settings that force even a small session to compact (keep nothing, reserve nothing).
fn aggressive_compaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

/// pi's `estimateMessagesTokens` over the raw context — the number `estimatedTokensAfter` is
/// DEFINED as (agent-session.ts:284-288 + :1876).
fn pi_estimate_messages_tokens(messages: &[AgentMessage]) -> u64 {
    u64::from(messages.iter().map(estimate_agent_message).fold(0u32, u32::saturating_add))
}

/// The manual `/compact` path (agent-session.ts:1876).
///
/// Asserts the emitted `estimated_tokens_after` equals pi's `estimateMessagesTokens` over the raw
/// post-compaction context, and — independently of any absolute number — that it is STRICTLY LESS
/// than the same sum taken over the LLM-flattened context, which is what the wrapper prose costs.
/// The second assertion is what makes the first one meaningful: the two projections must disagree
/// here, otherwise the equality proves nothing.
#[tokio::test]
async fn manual_compaction_reports_tokens_after_over_pi_s_raw_context() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
        // Ample summary completions so summarization never starves.
        faux_assistant_message(vec![faux_text("CONTEXT SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("TURN PREFIX SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build");

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let result = session.compact(None).await.expect("compaction succeeds");

    // The post-compaction context, in both projections.
    let raw = session.raw_context_messages().await;
    assert!(
        raw.iter().any(|m| matches!(m, AgentMessage::CompactionSummary(_))),
        "a compacted context leads with a compactionSummary — without one the two projections \
         cannot differ and this test would be vacuous (raw = {raw:?})"
    );
    // Exactly the pre-fix expression: `build_context().messages` summed with
    // `cyrup_provider::estimate_message_tokens`.
    let flattened = session.messages().await;
    let flattened_tokens: u64 = flattened.iter().map(cyrup_provider::estimate_message_tokens).sum();

    let pi_tokens = pi_estimate_messages_tokens(&raw);
    assert!(
        pi_tokens < flattened_tokens,
        "the raw and LLM-flattened projections MUST differ for this assertion to have teeth: the \
         COMPACTION_SUMMARY wrapper (~107 chars, context.rs:16-18) is pi-invisible. \
         raw={pi_tokens} flattened={flattened_tokens}"
    );
    assert_eq!(
        result.estimated_tokens_after, Some(pi_tokens),
        "estimatedTokensAfter must be estimateMessagesTokens(sessionContext.messages) over pi's \
         RAW AgentMessage context (agent-session.ts:1876 + :284-288), not over the convertToLlm \
         projection (which reports {flattened_tokens} — the wrapper prose pi never bills)"
    );
}

/// The auto/overflow path (`_runAutoCompaction`, agent-session.ts:2157) computes the field with the
/// same expression, and the two blocks are byte-identical — a fix applied to only one site would
/// leave the other reporting the flattened number. Assert on the source so a future edit cannot
/// silently un-fix half of it. The two paths live in `session/compaction.rs` (manual `/compact`)
/// and `session/auto_compaction.rs` (the threshold/overflow trigger).
#[test]
fn both_compaction_paths_measure_the_raw_projection() {
    let src = [
        include_str!("../session/compaction.rs"),
        include_str!("../session/auto_compaction.rs"),
    ]
    .concat();
    let sites = src.matches("let estimated_tokens_after: u64").count();
    assert_eq!(
        sites, 2,
        "`compact` (agent-session.ts:1876) and `_run_auto_compaction` (:2157) are the two sites \
         that compute the field; found {sites}"
    );
    assert!(
        !src.contains("estimate_message_tokens"),
        "no compaction path may sum the LLM-flattened `build_context()` with \
         `cyrup_provider::estimate_message_tokens` for estimatedTokensAfter — pi bills the raw \
         AgentMessage projection (agent-session.ts:284-288)"
    );
}

// ============================================================ A.7 compaction_end payload shape ====

/// A.7: a real (driven) MANUAL compaction emits `compaction_end` carrying the FULL Pi payload
/// `{reason,result,aborted,willRetry,errorMessage?}` — the `result` object
/// (summary/firstKeptEntryId/tokensBefore/estimatedTokensAfter), `aborted:false`, `willRetry:false`,
/// and NO `errorMessage` key (Pi agent-session.ts:142-148 / 2062-2069).
#[tokio::test]
async fn compaction_end_carries_full_pi_payload() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Two real turns to build a transcript, then the compaction summary completion.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
        // The compaction may issue a split-turn (history + turn-prefix) pair of summaries; supply
        // ample summary completions so summarization never starves.
        faux_assistant_message(vec![faux_text("CONTEXT SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("TURN PREFIX SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build");

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let mut sub = session.subscribe();
    let _result = session
        .compact(None)
        .await
        .expect("an aggressive-keep compaction over two turns produces a result");

    // Find the compaction_end on the live stream and assert its serialized shape.
    let mut end: Option<serde_json::Value> = None;
    for _ in 0..12 {
        match tokio::time::timeout(Duration::from_millis(300), sub.next()).await {
            Ok(Some(ev)) => {
                if ev.kind() == "compaction_end" {
                    end = Some(serde_json::to_value(&ev).unwrap());
                    break;
                }
            }
            _ => break,
        }
    }
    let v = end.expect("compaction_end must be emitted");
    assert_eq!(v["type"], "compaction_end");
    assert_eq!(v["reason"], "manual");
    assert_eq!(v["aborted"], serde_json::json!(false));
    assert_eq!(v["willRetry"], serde_json::json!(false), "manual compaction never retries");
    assert!(v.get("errorMessage").is_none(), "no errorMessage on a clean compaction: {v}");
    let r = v.get("result").expect("result present on a successful compaction");
    assert!(r.get("summary").and_then(|s| s.as_str()).is_some(), "result.summary present: {r}");
    assert!(r.get("firstKeptEntryId").is_some(), "result.firstKeptEntryId present: {r}");
    assert!(r.get("tokensBefore").is_some(), "result.tokensBefore present: {r}");
    assert!(r.get("estimatedTokensAfter").is_some(), "result.estimatedTokensAfter present: {r}");
}

/// check_compaction Case-2 (threshold, direct-usage) + A.7: a real BOUND run whose assistant usage
/// exceeds `window − reserve` triggers post-run auto-compaction tagged `threshold`, and its
/// `compaction_end` carries `willRetry:false` (Pi agent-session.ts:1900-1927 / 2069).
#[tokio::test]
async fn real_run_threshold_compaction_emits_threshold_end() {
    let fx = fixture();
    // reserveTokens just below the window so any real usage trips the threshold.
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 127999}),
    )
    .unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a real answer worth some tokens")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("THRESHOLD SUMMARY")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(cli)
        .build()
        .await
        .expect("build")
        .into_shared();

    let stream = session.prompt(UserInput::text("go", InputSource::Sdk)).await.expect("prompt");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;

    let starts: Vec<String> = events
        .iter()
        .filter_map(|e| serde_json::to_value(e).ok())
        .filter(|v| v["type"] == "compaction_start")
        .filter_map(|v| v["reason"].as_str().map(str::to_string))
        .collect();
    assert_eq!(starts, vec!["threshold".to_string()], "exactly one threshold compaction from the run");

    let end = events
        .iter()
        .filter_map(|e| serde_json::to_value(e).ok())
        .find(|v| v["type"] == "compaction_end")
        .expect("compaction_end must fire from the real run");
    assert_eq!(end["reason"], "threshold");
    assert_eq!(end["willRetry"], serde_json::json!(false), "a threshold compaction does not retry");
}

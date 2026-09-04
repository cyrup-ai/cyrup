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
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session::agent_message::AgentMessage;
use cyrup_session::compaction::tokens::estimate_agent_message;
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

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
    u64::from(
        messages
            .iter()
            .map(estimate_agent_message)
            .fold(0u32, u32::saturating_add),
    )
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
        raw.iter()
            .any(|m| matches!(m, AgentMessage::CompactionSummary(_))),
        "a compacted context leads with a compactionSummary — without one the two projections \
         cannot differ and this test would be vacuous (raw = {raw:?})"
    );
    // Exactly the pre-fix expression: `build_context().messages` summed with
    // `cyrup_provider::estimate_message_tokens`.
    let flattened = session.messages().await;
    let flattened_tokens: u64 = flattened
        .iter()
        .map(cyrup_provider::estimate_message_tokens)
        .sum();

    let pi_tokens = pi_estimate_messages_tokens(&raw);
    assert!(
        pi_tokens < flattened_tokens,
        "the raw and LLM-flattened projections MUST differ for this assertion to have teeth: the \
         COMPACTION_SUMMARY wrapper (~107 chars, context.rs:16-18) is pi-invisible. \
         raw={pi_tokens} flattened={flattened_tokens}"
    );
    assert_eq!(
        result.estimated_tokens_after,
        Some(pi_tokens),
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

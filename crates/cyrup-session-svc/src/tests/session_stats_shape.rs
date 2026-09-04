//! SEAM-031 — `getSessionStats` must be Pi's `SessionStats`, in shape AND in semantics.
//!
//! Pi's type (`agent-session.ts:260-277`, named as the `get_session_stats` response `data` at
//! `rpc-types.ts:183`):
//!
//! ```text
//! interface SessionStats {
//!     sessionFile: string | undefined;
//!     sessionId: string;
//!     userMessages: number;
//!     assistantMessages: number;
//!     toolCalls: number;
//!     toolResults: number;
//!     totalMessages: number;
//!     tokens: { input; output; cacheRead; cacheWrite; total };
//!     cost: number;
//!     contextUsage?: ContextUsage;
//! }
//! ```
//!
//! The naming half is asserted on the wire in `cyrup-modes/tests/modes.rs`. THIS file covers the
//! worse half, which is independent of naming: Pi's own docstring (`agent-session.ts:3107-3111`)
//! says the aggregation runs over ALL session entries, "including history that was compacted away,
//! so token/cost totals reflect what was actually billed across the session", and its loop folds
//! `branch_summary`/`compaction` `entry.usage` back in (`agent-session.ts:3120-3122`). cyrup used to
//! recompute from `messages()` — the rebuilt, LLM-flattened, POST-compaction context — so every
//! compaction silently erased the tokens it had already billed the user for.
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

/// A compaction must NEVER reduce the reported token/cost spend. Pi aggregates over
/// `sessionManager.getEntries()` precisely so the totals keep reflecting what was billed; cyrup's
/// old `SessionStats::from_messages(&self.messages())` read the rebuilt context, in which the
/// compacted-away assistant turns — and their usage — no longer exist.
#[tokio::test]
async fn a_compaction_does_not_erase_the_tokens_it_already_billed() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_text("first answer, reasonably long")],
            StopReason::Stop,
        ),
        faux_assistant_message(
            vec![faux_text("second answer, reasonably long")],
            StopReason::Stop,
        ),
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

    let before = session.session_stats().await;
    assert_eq!(
        before.user_messages, 2,
        "two user turns were billed: {before:?}"
    );
    assert_eq!(
        before.assistant_messages, 2,
        "two assistant turns were billed: {before:?}"
    );
    assert!(
        before.tokens.output > 0,
        "the scripted turns must have reported output tokens, else this test proves nothing: \
         {before:?}"
    );

    session
        .compact(None)
        .await
        .expect("the compaction succeeds");

    let after = session.session_stats().await;
    assert!(
        after.tokens.output >= before.tokens.output,
        "a compaction must never reduce the reported spend — Pi aggregates over getEntries() \
         'including history that was compacted away' (agent-session.ts:3107-3111). \
         before={} after={} (full: {after:?})",
        before.tokens.output,
        after.tokens.output,
    );
    assert!(
        after.user_messages >= before.user_messages
            && after.assistant_messages >= before.assistant_messages,
        "the compacted-away turns are still session entries and still count \
         (before user={} assistant={}, after user={} assistant={})",
        before.user_messages,
        before.assistant_messages,
        after.user_messages,
        after.assistant_messages,
    );
}

/// The derived `tokens.total` is Pi's exact sum (`agent-session.ts:3157`), and the identity fields
/// Pi carries — `sessionId`, and `sessionFile` for a persisted session — are populated.
#[tokio::test]
async fn stats_carry_pi_s_identity_fields_and_derived_total() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("hi")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .build()
        .await
        .expect("build");

    let _ = session.prompt("hello").await.expect("prompt");
    session.wait_for_idle().await;

    let stats = session.session_stats().await;
    assert!(
        !stats.session_id.is_empty(),
        "sessionId is populated: {stats:?}"
    );
    assert_eq!(
        stats.session_id,
        session.session_id().to_string(),
        "sessionId is the session's own id"
    );
    assert_eq!(
        stats.session_file,
        session
            .session_file()
            .await
            .map(|p| p.display().to_string()),
        "sessionFile mirrors the manager's file (Pi `sessionFile: string | undefined`)"
    );
    assert_eq!(
        stats.tokens.total,
        stats.tokens.input
            + stats.tokens.output
            + stats.tokens.cache_read
            + stats.tokens.cache_write,
        "tokens.total is the derived sum (agent-session.ts:3157): {stats:?}"
    );
    assert_eq!(
        stats.total_messages,
        stats.user_messages + stats.assistant_messages + stats.tool_results,
        "this run has only user/assistant/toolResult entries, so totalMessages is their sum: {stats:?}"
    );
}

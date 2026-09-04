//! SEAM-115 — the three `context_usage` correctness fixes of SEAM-114 (`2086366`) pinned over a
//! BRANCHED session, so each regresses loudly instead of as a plausible wrong number.
//!
//! Pi `getContextUsage` (`packages/coding-agent/src/core/agent-session.ts:3375-3413` @v0.84.4,
//! byte-identical to `:3164-3208` @v0.83.0) answers from `this.sessionManager.getBranch()`
//! (`:3384`) — the ACTIVE BRANCH, never `getEntries()` — scans backwards from the branch tail to
//! the latest compaction on that branch (`:3390-3403`), and `getSessionStats` does not re-derive
//! the number: it returns `contextUsage: this.getContextUsage()` (`:3371`).
//!
//! cyrup's seams: [`crate::AgentSession::context_usage`] (the raw occupancy),
//! [`crate::AgentSession::stats_context_usage`] (the three-state `{tokens, contextWindow, percent}`
//! whose `tokens: None` arm is the post-compaction guard) and
//! [`crate::AgentSession::state_view`] (RPC `get_state`, which must carry the SAME number
//! `GetContextUsage` reports). All three live in `crates/cyrup-session-svc/src/session/stats.rs`.
//!
//! Each case states the regression it is RED against, and asserts its own precondition so a
//! fixture drift cannot turn it into a vacuous pass.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::{DeferredHandle, Message, StopReason};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxMessageOptions, FauxProvider, faux_assistant_message, faux_assistant_message_with,
    faux_text,
};
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

/// Compaction settings that force even a small session to compact and keep NOTHING after the
/// cut — so every compaction here leaves an empty kept window (the SEAM-114 defect-3 shape).
fn aggressive_compaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

fn settled(text: &str) -> cyrup_core::AssistantMessage {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop)
}

/// A turn that occupies context (the faux provider stamps `usage.input` from the serialized
/// prompt, `faux.rs` `apply_usage_estimate`) so the occupancy numbers below are non-zero and
/// distinct from turn to turn.
async fn prompt_and_settle(session: &crate::AgentSession, text: &str) {
    let _ = session.prompt(text).await.expect("prompt");
    session.wait_for_idle().await;
}

/// **(a) Branch isolation** — SEAM-114 defect 2. The post-compaction guard must be answered from
/// the ACTIVE branch. RED against `has_post_compaction_usage` scanning `entries()`: the flat
/// store then holds a LATER, off-branch compaction with a valid assistant after it, so `rposition`
/// latches that off-branch boundary, the guard answers `true`, and a stale pre-compaction
/// occupancy is printed as current on a branch whose own compaction has no assistant after it.
///
/// Tree built (parent links left→right; `C` = compaction):
///
/// ```text
/// u1 → a1 → u2 → a2 → C1              ← main line, then navigated back to
///   └→ u3 → a3 → C2 → u4 → a4         ← the abandoned branch, appended LATER in the flat store
/// ```
///
/// With the leaf on `C1`, pi's `getBranch()` is `[u1,a1,u2,a2,C1]`: a compaction with nothing
/// after it → `{tokens: null}`. `getEntries()` ends `…C2,u4,a4` → a false "post-compaction
/// usage".
#[tokio::test]
async fn post_compaction_guard_ignores_an_off_branch_compaction_and_its_assistant() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        settled("first answer, reasonably long"),
        settled("second answer, reasonably long"),
        settled("CONTEXT SUMMARY ONE"),
        settled("third answer on the side branch"),
        settled("CONTEXT SUMMARY TWO"),
        settled("fourth answer after the second compaction"),
        // Slack so summarization never starves whichever prompt it uses.
        settled("EXTRA SUMMARY"),
        settled("EXTRA SUMMARY"),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build");

    // Main line: u1 → a1 → u2 → a2 → C1.
    prompt_and_settle(&session, "tell me one").await;
    let u1 = session.user_messages_for_forking().await[0]
        .entry_id
        .clone();
    prompt_and_settle(&session, "tell me two").await;
    let occupancy_at_a2 = session.context_usage().await;
    assert!(
        occupancy_at_a2.used_tokens > 0,
        "the settled turns must occupy context or the numbers below prove nothing: \
         {occupancy_at_a2:?}"
    );
    session.compact(None).await.expect("first compaction");
    let c1 = session
        .leaf_id()
        .await
        .expect("the compaction entry is the leaf");

    // Precondition of the guard itself, on the un-branched session: a compaction with no
    // assistant after it reports `tokens: None` (pi `:3406-3408`).
    let on_c1_before_fork = session
        .stats_context_usage()
        .await
        .expect("a model with a window is set");
    assert_eq!(
        on_c1_before_fork.tokens, None,
        "no assistant has responded since C1 — the count is unknown: {on_c1_before_fork:?}"
    );

    // Side branch off u1: u3 → a3 → C2 → u4 → a4. Every one of these lands AFTER C1 in the flat
    // append-only store.
    session.branch(u1).await.expect("navigate to u1");
    prompt_and_settle(&session, "tell me three").await;
    session.compact(None).await.expect("second compaction");
    prompt_and_settle(&session, "tell me four").await;
    let on_side_branch = session
        .stats_context_usage()
        .await
        .expect("a model with a window is set");
    assert!(
        on_side_branch.tokens.is_some(),
        "a4 responded after C2 on this branch, so its occupancy is trusted: {on_side_branch:?}"
    );

    // Back to C1. The active branch is `[u1,a1,u2,a2,C1]` again — the same answer as before the
    // fork — while `entries()` now ends `…C2,u4,a4`.
    session.branch(c1).await.expect("navigate back to C1");
    let on_c1_after_fork = session
        .stats_context_usage()
        .await
        .expect("a model with a window is set");
    assert_eq!(
        on_c1_after_fork.tokens, None,
        "the off-branch C2/a4 pair must not count as post-compaction usage for C1's branch \
         (pi scans `getBranch()`, agent-session.ts:3384 @v0.84.4): {on_c1_after_fork:?}"
    );
    assert_eq!(
        on_c1_after_fork, on_c1_before_fork,
        "navigating away and back must not change what this branch reports"
    );
    // The raw occupancy is branch-local too: it is a2's, not a4's.
    let raw_on_c1 = session.context_usage().await;
    assert_eq!(
        raw_on_c1, occupancy_at_a2,
        "`context_usage` walks the active branch, so on C1 it reads a2 (pre-compaction), not \
         the off-branch a4"
    );
}

/// **(b) One producer** — SEAM-114 defect 3. `state_view().context_usage` must be the SAME number
/// `context_usage()` reports, on the exact session shape where an inline re-derivation from the
/// rebuilt (windowed) context diverges: a compaction whose kept window holds no assistant while
/// an earlier pre-compaction assistant exists. The branch walk reads that earlier assistant
/// (non-zero); a `messages()`-based derivation sees only the summary and reports zero. RED
/// against re-inlining the derivation in `state_view`.
///
/// The shape is the one the ledger names — a compaction with NO `firstKeptEntryId`, which is what
/// pi's `migrateV1ToV2` leaves behind for an unresolvable v1 `firstKeptEntryIndex`
/// (`cyrup-session/src/entry.rs:82-96`; `build_context_messages` keeps nothing before it,
/// `cyrup-session/src/context.rs:187-192`). A live `keepRecentTokens: 0` compaction still keeps
/// the split tail of the last turn (its assistant included), so the file is edited to the
/// pi-written shape and resumed.
#[tokio::test]
async fn state_view_and_context_usage_are_one_producer_after_an_empty_kept_window() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        settled("first answer, reasonably long"),
        settled("second answer, reasonably long"),
        settled("CONTEXT SUMMARY"),
        settled("EXTRA SUMMARY"),
        settled("EXTRA SUMMARY"),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider.clone(), base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build");
    let file = session
        .session_file()
        .await
        .expect("a persisted session to resume from");

    prompt_and_settle(&session, "tell me one").await;
    prompt_and_settle(&session, "tell me two").await;
    session.compact(None).await.expect("compaction");
    drop(session);

    // Rewrite the compaction line to pi's unresolvable-v1 shape: the `firstKeptEntryId` key
    // absent. Everything else — ids, parent links, the summary — is left byte-for-byte.
    let text = std::fs::read_to_string(&file).unwrap();
    let mut stripped = 0;
    let rewritten: Vec<String> = text
        .lines()
        .map(|line| {
            let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v.get("type").and_then(|t| t.as_str()) == Some("compaction")
                && let Some(obj) = v.as_object_mut()
                && obj.remove("firstKeptEntryId").is_some()
            {
                stripped += 1;
            }
            v.to_string()
        })
        .collect();
    assert_eq!(stripped, 1, "exactly one compaction entry was persisted");
    std::fs::write(&file, rewritten.join("\n") + "\n").unwrap();

    let mut resume_cfg = base_config(&fx);
    resume_cfg.target = crate::SessionTarget::Resume(file);
    let session = SessionBuilder::new(provider, resume_cfg)
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("resume");

    // Precondition: the kept window is empty — the rebuilt context carries no assistant at all.
    let rebuilt = session.messages().await;
    assert!(
        !rebuilt.iter().any(|m| matches!(m, Message::Assistant(_))),
        "an absent firstKeptEntryId must leave no assistant in the rebuilt context, else this \
         case is not the divergent shape: {rebuilt:?}"
    );

    let raw = session.context_usage().await;
    assert!(
        raw.used_tokens > 0,
        "the branch walk reaches the pre-compaction assistant, so the raw occupancy is non-zero \
         (a `messages()`-based derivation would report 0 here): {raw:?}"
    );
    let view = session.state_view().await;
    assert_eq!(
        view.context_usage, raw,
        "RPC `get_state` and `get_context_usage` must agree on one session state — pi's \
         `getSessionStats` returns `contextUsage: this.getContextUsage()` (agent-session.ts:3371 \
         @v0.84.4) rather than re-deriving it"
    );
    // And the stats sub-object answers the same question the same way: the guard sees a
    // compaction with no assistant after it, so `tokens` is unknown while the window is known.
    let stats = view
        .stats
        .context_usage
        .expect("a model with a window is set");
    assert_eq!(
        stats.tokens, None,
        "no post-compaction assistant: {stats:?}"
    );
    assert_eq!(stats.context_window, raw.context_window);
}

/// **(c) Deferred tail** — SEAM-114 defect 4. A branch whose tail assistant is
/// `StopReason::Deferred` (a durable provider receipt with empty content, not a settled
/// measurement) over an earlier settled assistant must report the SETTLED one's occupancy. Pins
/// both the filter and the `filter_map(..).find(..)` shape: a scan that stops at the first
/// assistant and then rejects it (`find_map(..).filter(..)`) would report zero; a scan that does
/// not filter at all would report the receipt's larger context estimate.
#[tokio::test]
async fn a_deferred_tail_does_not_stop_the_scan_nor_drive_the_occupancy() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let handle = DeferredHandle {
        provider: "faux".into(),
        model_id: "faux-model".into(),
        api: "faux".into(),
        id: "deferred-receipt-1".into(),
        expires_at: None,
        poll_after_ms: None,
        data: None,
    };
    faux.set_responses(vec![
        settled("first answer, reasonably long"),
        faux_assistant_message_with(
            Vec::new(),
            StopReason::Deferred,
            FauxMessageOptions {
                deferred: Some(handle),
                ..FauxMessageOptions::default()
            },
        ),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .build()
        .await
        .expect("build");

    prompt_and_settle(&session, "tell me one").await;
    let settled_occupancy = session.context_usage().await;
    assert!(
        settled_occupancy.used_tokens > 0,
        "the settled turn must occupy context: {settled_occupancy:?}"
    );

    prompt_and_settle(&session, "tell me two, deferred").await;

    // Precondition: the branch tail really is a deferred assistant, and its stamped usage differs
    // from the settled one's (the faux provider estimates `input` from the now-longer prompt), so
    // "reports the settled one" is a distinguishable claim.
    let dag = session.session_dag().await;
    let tail = dag.iter().find(|n| n.is_leaf).expect("one leaf");
    assert!(
        tail.label.starts_with("assistant"),
        "the leaf is the deferred assistant turn: {tail:?}"
    );
    let stats = session.session_stats().await;
    assert_eq!(
        stats.assistant_messages, 2,
        "both the settled and the deferred assistant are entries on the branch: {stats:?}"
    );
    // `SessionStats.tokens` sums every assistant entry, so the receipt's own four-field sum is
    // the aggregate minus the settled turn's.
    let receipt_context_tokens = stats.tokens.total - settled_occupancy.used_tokens;
    assert!(
        receipt_context_tokens > 0 && receipt_context_tokens != settled_occupancy.used_tokens,
        "the deferred receipt was stamped with its own, different usage estimate, so a scan that \
         used it would report a different number: receipt={receipt_context_tokens} settled={} \
         ({stats:?})",
        settled_occupancy.used_tokens
    );

    let after_deferred = session.context_usage().await;
    assert_eq!(
        after_deferred, settled_occupancy,
        "a deferred tail is skipped AND does not stop the scan: the settled assistant before it \
         drives the occupancy"
    );
    let three_state = session
        .stats_context_usage()
        .await
        .expect("a model with a window is set");
    assert_eq!(
        three_state.tokens,
        Some(settled_occupancy.used_tokens),
        "no compaction on the branch, so the settled occupancy is trusted as-is: {three_state:?}"
    );
}

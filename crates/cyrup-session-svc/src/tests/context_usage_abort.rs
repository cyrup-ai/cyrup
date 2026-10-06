//! TUI-056 — the footer's context meter must hold its reading across an aborted turn.
//!
//! The footer reads [`crate::AgentSession::stats_context_usage`], the port of pi's
//! `getContextUsage()` (`packages/coding-agent/src/core/agent-session.ts:3858-3901` @v0.87.1):
//!
//! ```ts
//! if (latestCompaction) {
//!     const projectedAssistants = new Set(projection.entries.flatMap((entry) =>
//!         entry.messages.some((message) => message.role === "assistant" &&
//!             message.stopReason !== "aborted" && message.stopReason !== "error" &&
//!             calculateContextTokens(message.usage) > 0) ? [entry.sourceEntry.id] : []));
//!     …
//!     if (!hasPostCompactionUsage) return { tokens: null, contextWindow, percent: null };
//! }
//! const estimate = estimateProjectedContextTokens(projection, branch);
//! const percent = (estimate.tokens / contextWindow) * 100;
//! ```
//!
//! cyrup answered with the LAST assistant's four-field usage sum, aborted or not, so an abort whose
//! partial carried no usage read `0.0%/131k` while the whole conversation was still in context.
//! Each case below states which clause of the port it is RED against.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::{AgentSessionEvent, SessionBuilder, SessionConfig, SessionTarget};
use cyrup_core::{Content, Message, StopReason, Usage};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxConfig, FauxProvider, faux_assistant_message, faux_text};
use cyrup_session::AgentMessage;
use cyrup_session::compaction::tokens::estimate_agent_message;
use futures::StreamExt;
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
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
        tmp,
        cwd,
        agent_dir,
    }
}

fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// The row's own scenario, through the real prompt → abort path: one settled turn, then a paced
/// turn aborted mid-stream. pi's anchor skips the aborted assistant (`stopReason !== "aborted"`,
/// `compaction.ts:170-183`) and lands on the settled one, then adds a chars/4 estimate of the new
/// prompt and the aborted partial — so the meter reads MORE than before the abort, never zero.
///
/// RED against the pre-fix `stats_context_usage`, which reported the aborted partial's own usage.
#[tokio::test]
async fn an_aborted_turn_keeps_the_previous_reading_plus_its_trailing_estimate() {
    let fx = fixture();
    let mut body = String::from("PARTIAL");
    for i in 0..64 {
        body.push_str(&format!(" tail-{i:02}"));
    }
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        tokens_per_second: Some(20.0),
        ..Default::default()
    }));
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text(body)], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .build()
        .await
        .expect("build");

    let _ = session.prompt("first question").await.expect("prompt 1");
    session.wait_for_idle().await;
    let before = session
        .stats_context_usage()
        .await
        .expect("a model with a window is set");

    // Abort the second turn once it is visibly streaming — no wall-clock guesses.
    let mut feed = session.subscribe();
    let _ = session.prompt("second question").await.expect("prompt 2");
    let streaming = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(ev) = feed.next().await {
            if let AgentSessionEvent::MessageUpdate {
                message: cyrup_agent::AgentMessage::Assistant(a),
                ..
            } = &ev
                && a.content
                    .iter()
                    .any(|c| matches!(c, Content::Text { text, .. } if text.contains("PARTIAL")))
            {
                return true;
            }
        }
        false
    })
    .await
    .expect("the second turn must stream");
    assert!(streaming, "fixture precondition: the second turn streams");
    session.abort_and_settle().await;

    // Preconditions: the projection is [u1, a1, u2, a2(aborted)] and a1 is the only valid anchor.
    // The loadout declaration the loop records ahead of the first request is a system message and
    // is not part of the conversation this test is about.
    let projected: Vec<AgentMessage> = session
        .raw_context_messages()
        .await
        .into_iter()
        .filter(|m| !matches!(m, AgentMessage::Core(Message::System(_))))
        .collect();
    let [_, AgentMessage::Core(Message::Assistant(a1)), u2, a2] = projected.as_slice() else {
        panic!("expected u1, a1, u2, aborted a2 on the branch: {projected:?}");
    };
    assert!(
        matches!(a2, AgentMessage::Core(Message::Assistant(a)) if a.stop_reason == StopReason::Aborted),
        "the second turn must have settled as aborted: {a2:?}"
    );

    let after = session
        .stats_context_usage()
        .await
        .expect("a model with a window is set");
    let anchor = u64::from(cyrup_session::compaction::context_tokens_from_usage(
        &a1.usage,
    ));
    let trailing = u64::from(estimate_agent_message(u2)) + u64::from(estimate_agent_message(a2));
    assert_eq!(
        after.tokens,
        Some(anchor + trailing),
        "pi skips the aborted assistant and estimates everything after the settled one: \
         before={before:?} after={after:?}"
    );
    assert!(
        after.percent.unwrap() > before.percent.unwrap(),
        "the conversation only grew, so the meter must not drop: before={before:?} after={after:?}"
    );
}

/// A pi-written session: `u1 → a1 → C1 → u2 → a2`, then the tail `extra` entries. Serialised from
/// real cyrup messages so the assistant lines carry every field the reader requires.
fn pi_session(dir: &Path, cwd: &Path, a2_usage: Usage, extra: &[serde_json::Value]) -> PathBuf {
    let ts = "2026-01-01T00:00:00.000Z";
    let assistant = |text: &str, usage: Usage| {
        let mut a = faux_assistant_message(vec![faux_text(text)], StopReason::Stop);
        a.usage = usage;
        serde_json::to_value(Message::Assistant(a)).unwrap()
    };
    let user = |text: &str| serde_json::json!({"role": "user", "content": text, "timestamp": 1});
    let big = Usage {
        input: 90_000,
        total_tokens: 90_000,
        ..Usage::default()
    };
    let mut lines = vec![
        serde_json::json!({"type": "session", "version": 3, "id": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0002", "timestamp": ts, "cwd": cwd}),
        serde_json::json!({"type": "message", "id": "u1", "parentId": null, "timestamp": ts, "message": user("one")}),
        serde_json::json!({"type": "message", "id": "a1", "parentId": "u1", "timestamp": ts, "message": assistant("answer one", big)}),
        serde_json::json!({"type": "compaction", "id": "c1", "parentId": "a1", "timestamp": ts, "summary": "summary", "firstKeptEntryId": "u1", "tokensBefore": 90_000}),
        serde_json::json!({"type": "message", "id": "u2", "parentId": "c1", "timestamp": ts, "message": user("two")}),
        serde_json::json!({"type": "message", "id": "a2", "parentId": "u2", "timestamp": ts, "message": assistant("answer two", a2_usage)}),
    ];
    lines.extend(extra.iter().cloned());
    let path = dir.join("pi.jsonl");
    let text: Vec<String> = lines.iter().map(|v| v.to_string()).collect();
    std::fs::write(&path, text.join("\n") + "\n").unwrap();
    path
}

async fn resume(fx: &Fixture, path: PathBuf) -> crate::AgentSession {
    let mut cfg = config(fx);
    cfg.target = SessionTarget::Resume(path);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    SessionBuilder::new(provider, cfg)
        .build()
        .await
        .expect("resume")
}

/// `calculateContextTokens(message.usage) > 0` is `usage.totalTokens || sum-of-four`
/// (`compaction.ts:162-164`): a provider that reports only `totalTokens` still anchors. RED against
/// the pre-fix guard, which summed the four components alone, found zero, and reported `null`.
#[tokio::test]
async fn a_total_tokens_only_usage_counts_as_post_compaction_usage() {
    let fx = fixture();
    let total_only = Usage {
        total_tokens: 40_000,
        ..Usage::default()
    };
    let path = pi_session(fx.tmp.path(), &fx.cwd, total_only, &[]);
    let session = resume(&fx, path).await;
    let usage = session
        .stats_context_usage()
        .await
        .expect("a model with a window is set");
    assert_eq!(
        usage.tokens,
        Some(40_000),
        "a2's totalTokens is the anchor and nothing trails it: {usage:?}"
    );
}

/// `projectedAssistants` is built from the PROJECTION (`agent-session.ts:3873-3885`): a
/// post-compaction assistant that a later `context_edit` omits is not in it, so the count is
/// unknown again. RED against the pre-fix guard, which scanned raw branch entries and trusted a2.
#[tokio::test]
async fn a_post_compaction_assistant_omitted_by_a_context_edit_does_not_count() {
    let fx = fixture();
    let settled = Usage {
        input: 30_000,
        total_tokens: 30_000,
        ..Usage::default()
    };
    let omit = serde_json::json!({
        "type": "context_edit", "id": "e1", "parentId": "a2",
        "timestamp": "2026-01-01T00:00:00.000Z", "targetId": "a2", "replacement": null
    });
    let path = pi_session(fx.tmp.path(), &fx.cwd, settled.clone(), &[omit]);
    let session = resume(&fx, path.clone()).await;
    let usage = session
        .stats_context_usage()
        .await
        .expect("a model with a window is set");
    assert_eq!(
        (usage.tokens, usage.percent),
        (None, None),
        "the only post-compaction assistant is edited out of the projection: {usage:?}"
    );

    // The negative control: the same file without the edit trusts a2.
    std::fs::remove_file(&path).unwrap();
    let path = pi_session(fx.tmp.path(), &fx.cwd, settled, &[]);
    let session = resume(&fx, path).await;
    let usage = session
        .stats_context_usage()
        .await
        .expect("a model with a window is set");
    assert_eq!(usage.tokens, Some(30_000), "{usage:?}");
}

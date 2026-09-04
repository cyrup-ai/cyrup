//! SEAM-009 — forking or cloning a NON-PERSISTED session must branch its transcript, not throw it
//! away, and a bogus entry id must be rejected on that path too.
//!
//! Pi ground truth, `packages/coding-agent/src/core/agent-session-runtime.ts:262-350`. Two facts:
//!
//!  * The anchor is resolved and VALIDATED above the persistence split —
//!    `const selectedEntry = this.session.sessionManager.getEntry(entryId); if (!selectedEntry)
//!    throw new Error("Invalid entry ID for forking");` at :274-276 (and again at :282-283), while
//!    `if (this.session.sessionManager.isPersisted())` is only at :290.
//!  * The non-persisted branch reuses the LIVE manager and branches it:
//!    `const sessionManager = this.session.sessionManager; … sessionManager.createBranchedSession(
//!    targetLeafId); … this.createRuntime({ …, sessionManager })` at :333-341.
//!
//! cyrup did neither: `fork_anchor` lived INSIDE the persisted arm, and the in-memory arm was a
//! bare `self.factory.build(SessionTarget::New, None)` — a brand-new EMPTY session. For a session
//! with no file, that transcript is the only copy in existence, so the loss is unrecoverable and
//! silent (the call returned `cancelled: false`).
//!
//! These assertions are on transcript CONTENT, never on a length: a length check would pass against
//! any implementation that happened to carry the right NUMBER of messages.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    AgentSessionEvent, AgentSessionRuntime, ForkPosition, SessionConfig, SessionFactory,
    SessionTarget,
};
use cyrup_core::{EntryId, Message, StopReason};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use futures::StreamExt;
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

/// A NON-PERSISTED session — `persist: false` is what `--no-save` / a `SessionTarget::New` embedder
/// session with persistence off produces, and it is the whole point of this file.
fn in_memory_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.persist = false;
    cfg
}

/// The visible text of every user / assistant message on a session's current branch.
async fn transcript_text(session: &crate::AgentSession) -> Vec<String> {
    session
        .messages()
        .await
        .iter()
        .filter_map(|m| match m {
            Message::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            Message::Assistant(a) => Some(
                a.content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            _ => None,
        })
        .collect()
}

/// Build an in-memory runtime and drive TWO complete exchanges through it.
async fn runtime_with_two_exchanges(fx: &Fixture) -> Arc<AgentSessionRuntime> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("ANSWER-ONE")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ANSWER-TWO")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let factory = Arc::new(SessionFactory::new(provider, in_memory_config(fx)));
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .expect("runtime");

    let session = runtime.session().await;
    assert!(
        session.session_file().await.is_none(),
        "precondition: this session must be NON-persisted (no session file)"
    );
    let _s1 = session.prompt("QUESTION-ONE").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _s2 = session.prompt("QUESTION-TWO").await.expect("prompt 2");
    session.wait_for_idle().await;
    runtime
}

// =============================================================================== data loss ====

/// THE SEAM-009 proof. Fork an unsaved two-exchange session BEFORE its second user message: the
/// child must be a real branch carrying the first exchange verbatim, and must not carry the second.
///
/// Pre-fix the child was an empty `SessionTarget::New` session — `[]` — and the call still reported
/// success, so the first exchange was gone with no diagnostic and no file to recover it from.
#[tokio::test]
async fn forking_an_unsaved_session_carries_the_transcript_into_the_branch() {
    let fx = fixture();
    let runtime = runtime_with_two_exchanges(&fx).await;

    let session = runtime.session().await;
    let before = transcript_text(&session).await;
    assert_eq!(
        before,
        vec![
            "QUESTION-ONE".to_string(),
            "ANSWER-ONE".to_string(),
            "QUESTION-TWO".to_string(),
            "ANSWER-TWO".to_string(),
        ],
        "fixture precondition: both exchanges are on the parent branch"
    );

    // Anchor: the SECOND user message. `position: "before"` therefore branches at its parent — the
    // first assistant reply — so the first exchange is retained and the second is dropped (Pi
    // `targetLeafId = selectedEntry.parentId`, agent-session-runtime.ts:286).
    let anchors = session.user_messages_for_forking().await;
    assert_eq!(anchors.len(), 2, "two user-message anchors");
    assert_eq!(anchors[1].text, "QUESTION-TWO");
    let anchor: EntryId = anchors[1].entry_id.clone();
    drop(session);

    let fork = runtime
        .fork(anchor, ForkPosition::Before)
        .await
        .expect("fork must succeed");
    assert!(!fork.cancelled);
    assert_eq!(
        fork.selected_text.as_deref(),
        Some("QUESTION-TWO"),
        "position:\"before\" must return the anchor's text for the editor, on the in-memory path \
         too (Pi computes `selectedText` above the persistence split, :287)"
    );

    let child = runtime.session().await;
    let after = transcript_text(&child).await;
    assert_eq!(
        after,
        vec!["QUESTION-ONE".to_string(), "ANSWER-ONE".to_string()],
        "the forked child must BE the branch — the first exchange verbatim, the second dropped. \
         Pre-fix this was [] and the whole conversation was destroyed."
    );
}

/// The same loss through `ForkPosition::At`, which is what `/clone`-style "branch at this entry"
/// drives: the anchored entry itself is the new leaf, so the ENTIRE transcript is retained.
#[tokio::test]
async fn cloning_an_unsaved_session_at_its_leaf_retains_the_whole_transcript() {
    let fx = fixture();
    let runtime = runtime_with_two_exchanges(&fx).await;

    let session = runtime.session().await;
    let anchors = session.user_messages_for_forking().await;
    let anchor: EntryId = anchors[1].entry_id.clone();
    drop(session);

    let fork = runtime
        .fork(anchor, ForkPosition::At)
        .await
        .expect("fork must succeed");
    assert!(!fork.cancelled);

    let child = runtime.session().await;
    assert_eq!(
        transcript_text(&child).await,
        vec![
            "QUESTION-ONE".to_string(),
            "ANSWER-ONE".to_string(),
            "QUESTION-TWO".to_string(),
        ],
        "position:\"at\" makes the anchored entry the leaf, so everything up to and including it \
         survives into the clone"
    );
}

// ================================================================================ ordering ====

/// The ORDERING half of SEAM-009, and the one that survives a fork issued while a turn is still
/// streaming — which nothing guards against (`AgentSessionRuntime::fork` has no `is_streaming`
/// check, so a `/fork` typed mid-response reaches here).
///
/// Pi's non-persisted arm is three statements in this order (agent-session-runtime.ts:333-341):
///
/// ```text
/// const sessionManager = this.session.sessionManager;   // the LIVE object, not a copy
/// sessionManager.createBranchedSession(targetLeafId);   // branched IN PLACE
/// await this.teardownCurrent("fork", …);                // ONLY THEN abort + settle
/// this.apply(await this.createRuntime({ …, sessionManager }));
/// ```
///
/// Because the outgoing session still points at that same object, everything the dying run appends
/// while it settles lands in the branched manager — i.e. in the fork. That is Pi's own teardown
/// contract ("Settle any active response first so the aborted turn (including tool results) is
/// persisted to the outgoing session before it is replaced", :167-169) applied to the fork path.
///
/// cyrup's `build_from_manager` takes the manager BY VALUE, so the move is a real event that Pi
/// does not have, and doing it first destroys exactly that content: the outgoing session goes on
/// writing into a throwaway placeholder which is then dropped. For a session with no file, the
/// aborted turn's output is then gone for good.
///
/// The assertion is on CONTENT: the partial text the model had already produced must be readable in
/// the fork's transcript.
#[tokio::test]
async fn a_fork_during_a_live_turn_keeps_the_dying_turns_content_in_the_branch() {
    let fx = fixture();

    // A deliberately slow second turn. Nothing below waits on a wall-clock guess: the test blocks
    // until the marker has actually been observed in a `message_update`, and the body is long
    // enough (~590 chars ≈ 37 paced chunks) that the run is unambiguously still streaming after it.
    let mut body = String::from("LATE-TOKEN");
    for i in 0..64 {
        body.push_str(&format!(" tail-{i:02}"));
    }
    let faux = Arc::new(FauxProvider::with_config(
        cyrup_provider::faux::FauxConfig {
            tokens_per_second: Some(20.0),
            ..Default::default()
        },
    ));
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("ANSWER-ONE")], StopReason::Stop),
        faux_assistant_message(vec![faux_text(body)], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let factory = Arc::new(SessionFactory::new(provider, in_memory_config(&fx)));
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .expect("runtime");

    let session = runtime.session().await;
    assert!(
        session.session_file().await.is_none(),
        "precondition: this session must be NON-persisted (no session file)"
    );

    // Exchange one, complete.
    let _s1 = session.prompt("QUESTION-ONE").await.expect("prompt 1");
    session.wait_for_idle().await;

    // Exchange two: left STREAMING. Watch the session's own event feed and stop the moment the
    // partial assistant message actually carries the marker — no sleeps, no races.
    let mut feed = session.subscribe();
    let _s2 = session.prompt("QUESTION-TWO").await.expect("prompt 2");
    let saw_marker = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(ev) = feed.next().await {
            if let AgentSessionEvent::MessageUpdate {
                message: cyrup_agent::AgentMessage::Assistant(a),
                ..
            } = &ev
                && a.content.iter().any(|c| {
                    matches!(c, cyrup_core::Content::Text { text, .. } if text.contains("LATE-TOKEN"))
                })
            {
                return true;
            }
        }
        false
    })
    .await
    .expect("the slow turn must stream");
    assert!(
        saw_marker,
        "fixture precondition: the second turn must stream the marker"
    );
    assert!(
        !session.is_idle(),
        "precondition: the turn is STILL RUNNING when the fork lands"
    );

    // Fork AT the in-flight user message, so the branch path ends at it and the dying turn's
    // assistant message is appended as its child — the entry Pi keeps and cyrup was dropping.
    let anchors = session.user_messages_for_forking().await;
    assert_eq!(anchors.len(), 2, "two user-message anchors");
    assert_eq!(anchors[1].text, "QUESTION-TWO");
    let anchor: EntryId = anchors[1].entry_id.clone();
    drop(session);

    let fork = runtime
        .fork(anchor, ForkPosition::At)
        .await
        .expect("fork must succeed");
    assert!(!fork.cancelled);

    let child = runtime.session().await;
    let after = transcript_text(&child).await;
    assert_eq!(
        after.get(..3).map(<[String]>::to_vec),
        Some(vec![
            "QUESTION-ONE".to_string(),
            "ANSWER-ONE".to_string(),
            "QUESTION-TWO".to_string(),
        ]),
        "the branch path itself must survive: {after:?}"
    );
    let tail = after.get(3).map(String::as_str).unwrap_or("<nothing>");
    assert!(
        tail.contains("LATE-TOKEN"),
        "the aborted turn's own output must be persisted INTO the fork, exactly as Pi's shared \
         sessionManager gives it (agent-session-runtime.ts:333-341 branches before \
         teardownCurrent). Moving the manager out before the settle sends it to a placeholder that \
         is then dropped, and a non-persisted session has no file to recover it from. Got: \
         {after:?}"
    );
}

// ============================================================================== validation ====

/// The other half of the hoist: a bogus entry id must ERROR on the non-persisted path, exactly as
/// it already did on the persisted one. Pi throws `Invalid entry ID for forking` above the
/// `isPersisted()` split (agent-session-runtime.ts:275-276), so persistence cannot change the
/// answer. Pre-fix the in-memory arm never looked at the id at all and reported success.
#[tokio::test]
async fn a_bogus_entry_id_errors_on_the_unsaved_path_instead_of_silently_succeeding() {
    let fx = fixture();
    let runtime = runtime_with_two_exchanges(&fx).await;
    let generation_before = runtime.generation().await;

    let err = runtime
        .fork(EntryId::from("no-such-entry"), ForkPosition::Before)
        .await
        .expect_err("a bogus entry id must be rejected on an unsaved session");
    assert!(
        matches!(err, crate::SessionServiceError::InvalidForkEntry(ref id)
            if id == "no-such-entry"),
        "expected InvalidForkEntry, got {err:?}"
    );

    assert_eq!(
        runtime.generation().await,
        generation_before,
        "a rejected fork must not replace the active session"
    );
    let session = runtime.session().await;
    assert_eq!(
        transcript_text(&session).await.len(),
        4,
        "and must leave the live transcript untouched"
    );
}

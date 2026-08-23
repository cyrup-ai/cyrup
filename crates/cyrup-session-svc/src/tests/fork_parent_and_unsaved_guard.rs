//! SEAM-049 + SEAM-056 — the two defects on the runtime's fork path that are about the *edge to the
//! parent session* and about *what a user is told when there is no file yet*.
//!
//! Both are read from `packages/coding-agent/src/core/agent-session-runtime.ts` @v0.83.0
//! (byte-identical at v0.84.1 for these lines):
//!
//! * **SEAM-049**, the no-anchor branch. BOTH of pi's no-leaf arms record the outgoing file as the
//!   new session's parent — `sessionManager.newSession({ parentSession: currentSessionFile })` at
//!   `:296-299` (persisted) and `:336-337` (in-memory) — before `teardownCurrent`. cyrup's arm was
//!   `self.factory.build(SessionTarget::New, None)`, and `SessionFactory::build` is defined as
//!   `build_with_parent(target, cwd, None)`: the value was already in hand (bound as `previous` for
//!   the `session_start{reason:"fork"}` event) and discarded. Silent — the fork reports
//!   `cancelled:false` and the session tree simply loses the edge, so ancestry walks, `--fork`
//!   resumption chains and transcript-linking see an orphan where pi shows a child.
//!
//! * **SEAM-056**, the persisted has-leaf branch. pi guards the reopen with an ACTIONABLE sentence:
//!   `if (!existsSync(currentSessionFile)) { throw new Error("This session has not been saved yet.
//!   Wait for the first assistant response before cloning or forking it."); }` (`:312-316`),
//!   immediately above `SessionManager.open` at `:317`. cyrup went straight to
//!   `SessionManager::open(&file)?`, so `/fork` or `/clone` before the first assistant response —
//!   an ordinary user mistake, because cyrup defers the first file write until then — surfaced a
//!   filesystem error naming an internal path, with no remedy. Over RPC that string is what a
//!   client renders (`rpc.rs`'s `fork`/`clone` arms relay it verbatim).

use std::sync::Arc;

use super::common::{fixture, Fixture};
use crate::{
    AgentSessionRuntime, ForkPosition, SessionConfig, SessionFactory, SessionServiceError,
    SessionTarget,
};
use cyrup_core::{EntryId, StopReason};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};

fn persisted_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.persist = true;
    cfg
}

async fn runtime(fx: &Fixture, replies: usize) -> Arc<AgentSessionRuntime> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(
        (0..replies)
            .map(|i| faux_assistant_message(vec![faux_text(format!("ANSWER-{i}"))], StopReason::Stop))
            .collect(),
    );
    let provider: Arc<dyn Provider> = faux;
    let factory = Arc::new(SessionFactory::new(provider, persisted_config(fx)));
    AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .expect("runtime")
}

/// SEAM-049 — forking at the FIRST user entry takes pi's no-leaf arm (`fork_anchor` resolves the
/// parent of the first user message to `None`), and the session it produces must carry the outgoing
/// file as its `parentSession`.
///
/// RED before the fix: the arm was `factory.build(SessionTarget::New, None)`, whose parent is
/// hard-coded `None`, so the header carried no parent at all and this assertion read `None`.
#[tokio::test]
async fn forking_before_the_first_message_records_the_parent_session() {
    let fx = fixture();
    let runtime = runtime(&fx, 1).await;

    let session = runtime.session().await;
    let _ = session.prompt("QUESTION-ONE").await.expect("prompt");
    session.wait_for_idle().await;
    let parent_file = session
        .session_file()
        .await
        .expect("a persisted session has a file once an assistant message exists");

    // The FIRST user-message anchor: its parent is `None`, which is pi's `!targetLeafId` branch.
    let anchors = session.user_messages_for_forking().await;
    let anchor: EntryId = anchors[0].entry_id.clone();
    drop(session);

    let forked = runtime
        .fork(anchor, ForkPosition::Before)
        .await
        .expect("fork must succeed");
    assert!(!forked.cancelled);

    let child = runtime.session().await;
    let child_file = child
        .session_file()
        .await
        .expect("the forked session is persisted too");
    assert_ne!(child_file, parent_file, "the fork is a NEW session file");

    // pi: `newSession({ parentSession: currentSessionFile })` — the header's `parentSession` is the
    // OUTGOING file's path, verbatim.
    let header = child.session_header().await;
    let recorded = serde_json::to_value(&header)
        .expect("header serializes")
        .get("parentSession")
        .and_then(|v| v.as_str().map(str::to_string));
    assert_eq!(
        recorded.as_deref(),
        Some(parent_file.display().to_string().as_str()),
        "SEAM-049: a fork before the first message must record the session it came from as its \
         parentSession (agent-session-runtime.ts:296-299)"
    );
}

/// SEAM-056 — `/clone` (and `/fork` at a real leaf) on a persisted session whose file has not been
/// written yet must produce pi's sentence, verbatim, not an IO error naming an internal path.
///
/// The precondition is the one pi's own message describes: a session that has been prompted but has
/// no assistant response yet, so cyrup's deferred first write has not happened.
///
/// RED before the fix: `SessionManager::open(&file)` was reached unguarded and the error was
/// whatever the filesystem said. The assertion is on the exact text because that text is
/// user-facing — it is relayed straight through the RPC `fork`/`clone` `error` field.
#[tokio::test]
async fn cloning_an_unwritten_persisted_session_gives_pis_actionable_sentence() {
    let fx = fixture();
    let runtime = runtime(&fx, 1).await;
    let session = runtime.session().await;
    let _ = session.prompt("QUESTION-ONE").await.expect("prompt");
    session.wait_for_idle().await;

    // Delete the file out from under the live session: this reproduces the "persisted target whose
    // file is not on disk" state pi's `existsSync` guard exists for, deterministically and without
    // depending on when cyrup's deferred write happens to fire.
    let file = session.session_file().await.expect("session file");
    std::fs::remove_file(&file).expect("remove the session file");

    let anchors = session.user_messages_for_forking().await;
    let anchor: EntryId = anchors[0].entry_id.clone();
    drop(session);

    // `ForkPosition::At` on a real user entry resolves a target leaf, so this takes pi's
    // has-target-leaf branch — the one the guard sits inside (`:312-316`, above the open at `:317`).
    let err = runtime
        .fork(anchor, ForkPosition::At)
        .await
        .expect_err("an unwritten session file must be refused");
    assert!(
        matches!(err, SessionServiceError::SessionNotSaved),
        "SEAM-056: expected the dedicated variant, got {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "This session has not been saved yet. Wait for the first assistant response before \
         cloning or forking it.",
        "the Display must be pi's sentence verbatim (agent-session-runtime.ts:313-315)"
    );
}

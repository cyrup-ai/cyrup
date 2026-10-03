//! ADR-0030 §2.1 invariant 7 / `G-INV-7`, the other half of the **kernel** seam: the handle a commit
//! consumed is gone, so two commits cannot overlap and a settled Session cannot be re-entered.
//!
//! `SessionMut::commit` takes `self` **by value** and returns the handle inside the three non-fatal
//! [`CommitOutcome`] arms. That is what makes *"one Session has one mutation line"* a move rather than
//! a promise: upstream serialises with a promise tail (`session.ts`'s `#enqueue`/`#tail`) and a second
//! concurrent commit is a legal program that queues, which is also why a nested commit deadlocks
//! there with no diagnostic.
//!
//! The mutation this case exists to catch is `commit(&mut self, ..)`. That signature compiles, reads
//! as a convenience, and silently re-admits every overlapping-commit program the move forbids — and no
//! runtime test turns red, because the serialisation still happens to hold while the tests are
//! single-threaded.
//!
//! [`CommitOutcome`]: cyrup_pico::CommitOutcome

use cyrup_pico::{CallbackError, SessionMut};
use cyrup_pico_store::Cx;

/// The handle is moved into the first commit, so the second mention of it is a use after move.
async fn two_commits(session: SessionMut, cx: &Cx) {
    let first = session.commit(cx, async |_tx| {
        Err::<((), _), _>(CallbackError::Abandoned("not reached".into()))
    });
    let second = session.commit(cx, async |_tx| {
        Err::<((), _), _>(CallbackError::Abandoned("not reached".into()))
    });
    let _ = first.await;
    let _ = second.await;
}

/// Sequentially is the same error: continuing requires the handle the outcome gave back, which is the
/// point of returning it there rather than leaving it in the caller's hands.
async fn reuse_after_settling(session: SessionMut, cx: &Cx) {
    let _ = session
        .commit(cx, async |_tx| {
            Err::<((), _), _>(CallbackError::Abandoned("not reached".into()))
        })
        .await;
    let _ = session
        .commit(cx, async |_tx| {
            Err::<((), _), _>(CallbackError::Abandoned("not reached".into()))
        })
        .await;
}

fn main() {}

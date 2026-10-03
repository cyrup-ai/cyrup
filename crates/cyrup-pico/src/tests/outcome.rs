//! `CommitOutcome`'s four arms, as behaviour.

use cyrup_pico_store::Cx;

use super::store_double::{Fault, FaultyStore};
use super::{live_address, path};
use crate::{CommitOutcome, RollbackReason, SessionMut, UncertainKind};

/// `spec.md:1357` and ADR-0030 F1: *"the callback produced no writes: nothing reached storage, **no
/// sequence was allocated**, nothing was published, nothing can poison."*
///
/// Asserted against the backend's own call counter rather than against its sequence: the point is not
/// that the sequence is unchanged, it is that `Storage::commit` was **never entered**, which is what
/// [`BatchBuilder::build`](cyrup_pico_store::BatchBuilder::build)'s `Option` makes unavoidable.
#[tokio::test]
async fn nothing_to_commit_allocates_no_sequence() {
    let (store, switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();

    let outcome = handle
        .commit(&cx, async |tx| Ok((7_u32, tx.writing())))
        .await;

    let CommitOutcome::NothingToCommit { result, session: _ } = outcome else {
        panic!("a callback that staged nothing must be NothingToCommit");
    };
    assert_eq!(result, 7);
    assert_eq!(switch.commits(), 0, "Storage::commit must not be entered");
    assert_eq!(
        session.publications(),
        0,
        "nothing was published, so no observer can have seen a commit that did not happen"
    );
}

/// Every [`RollbackReason`] arm hands the mutation handle back, so the Session is *fully usable* —
/// ADR-0030 F1's `RolledBack` contract and `G-PRE-ADMISSION-ROLLBACK`.
#[tokio::test]
async fn rolled_back_leaves_the_session_usable() {
    let (store, switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();

    // Arm 1: the callback itself fails.
    let outcome = handle
        .commit::<(), _>(&cx, async |_tx| {
            Err(crate::CallbackError::Abandoned("no".into()))
        })
        .await;
    let CommitOutcome::RolledBack {
        reason,
        session: handle,
    } = outcome
    else {
        panic!("a failing callback must roll back");
    };
    assert!(matches!(reason, RollbackReason::Callback(_)));
    assert_eq!(switch.commits(), 0, "a callback failure is pre-admission");

    // Arm 2: storage rejects, with rollback guaranteed.
    switch.arm(Fault::Reject);
    let address = live_address();
    let outcome = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await;
    let CommitOutcome::RolledBack {
        reason,
        session: handle,
    } = outcome
    else {
        panic!("a Rejected commit must roll back, not poison");
    };
    assert!(matches!(reason, RollbackReason::Rejected(_)));
    assert_eq!(switch.commits(), 1);
    assert_eq!(
        session.publications(),
        0,
        "`G-REJECTED-NO-DURABLE-EFFECT`: a rejection publishes nothing"
    );

    // And the handle still works: the same change, with no fault armed, commits.
    let outcome = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await;
    assert!(
        matches!(outcome, CommitOutcome::Committed { .. }),
        "a rolled-back Session is fully usable"
    );
    assert_eq!(session.publications(), 1);
}

/// The first poison path: storage would not say whether the batch committed.
///
/// The handle is **not** in this variant, so the only thing this test can do with the outcome is read
/// it — which is the guarantee. `committed_at()` is `None`, because there is no honest answer.
#[tokio::test]
async fn an_uncertain_commit_keeps_no_handle_and_names_no_sequence() {
    let (store, switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let address = live_address();

    switch.arm(Fault::Uncertain);
    let outcome = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await;

    let CommitOutcome::Uncertain(fatal) = outcome else {
        panic!("a non-Rejected storage failure is fatal");
    };
    assert_eq!(fatal.kind(), UncertainKind::CommitStateUnknown);
    assert_eq!(
        fatal.committed_at(),
        None,
        "nobody knows whether it committed, and None says so"
    );
    assert_eq!(session.publications(), 0);
    // `or_fatal` is the ergonomic helper, and it is the Err arm here — so it hands back no handle.
    let outcome: CommitOutcome<()> = CommitOutcome::Uncertain(fatal);
    assert!(outcome.or_fatal().is_err());
}

/// The **second** poison path, which ADR-0030 F1 calls *the dropped second poison path*: storage
/// committed, memory did not.
///
/// `UncertainKind::AdoptionFailedAfterCommit` carries the sequence, because the two paths need
/// different recovery — the first means reopen *and reconcile*, this one means storage is known-good
/// and there is nothing to reconcile.
#[tokio::test]
async fn adoption_failing_after_a_commit_is_fatal_and_names_the_sequence() {
    let (store, _switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let address = live_address();

    // Establish the document, so the second commit adopts into an existing tracker.
    let CommitOutcome::Committed {
        session: handle, ..
    } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the creating commit must succeed");
    };
    let incarnation = session
        .current_incarnation(address.address())
        .expect("the authority is healthy")
        .expect("the document was created");

    // Open the window ADR-0030 §7 names as the third fault-injection point and nothing else can
    // reach: between a successful `Storage::commit` and adoption.
    crate::fault::set_before_adopt(move |window| window.forget(incarnation));
    let outcome = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "discharging")?;
            Ok(((), tx.writing()))
        })
        .await;
    crate::fault::clear();

    let CommitOutcome::Uncertain(fatal) = outcome else {
        panic!("a failed adoption after a successful commit is fatal");
    };
    let UncertainKind::AdoptionFailedAfterCommit { seq } = fatal.kind() else {
        panic!("the kind must distinguish this path from CommitStateUnknown");
    };
    assert_eq!(
        fatal.committed_at(),
        Some(seq),
        "durable state is known-good and known AHEAD of memory"
    );
    assert_eq!(
        session.publications(),
        1,
        "the second commit published nothing: an unadopted change must not reach an observer"
    );
}

/// `close()` seals mutation admission one-way, and a commit after it is a **rollback**, not a poison.
///
/// `spec.md:1545-1548`. The seal being one-way is what makes the second half of invariant 8 —
/// *"preparation and checkpoint failures occur before storage admission and roll back normally"* —
/// extend to the close path: a sealed Session still reads, still hands its handle back, and still
/// refuses to write.
#[tokio::test]
async fn closing_seals_admission_and_a_later_commit_rolls_back() {
    let (store, switch) = FaultyStore::new();
    let (session, mut handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let address = live_address();

    assert!(handle.is_admitting());
    handle.close(&cx).await.expect("the memory store closes");
    assert!(!handle.is_admitting(), "the seal is one-way");

    let outcome = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await;
    let CommitOutcome::RolledBack {
        reason,
        session: handle,
    } = outcome
    else {
        panic!("a commit on a sealed Session must roll back, not poison");
    };
    assert!(matches!(reason, RollbackReason::AdmissionSealed));
    assert_eq!(
        switch.commits(),
        0,
        "the seal is checked before the callback runs, so no storage call is made"
    );
    assert_eq!(session.publications(), 0);
    // And the handle came back: a sealed Session is refused, not poisoned.
    assert!(!handle.is_admitting());
}

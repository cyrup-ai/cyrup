//! The four-way migration ladder, the required base that follows it, the checkpoint predicate's
//! exactly-once evaluation, and the token-versus-record agreement check.
//!
//! PICO5-PLAN S5 names two of these cases by hand — *"the migration ladder (four arms)"* and *"the
//! first-write-after-migration base even when the migrated value is deeply equal"* — and the other
//! two are the guarantees this slice claims `checked` and `guarded`, so they need the same kind of
//! artefact.
//!
//! # How a base is observed without spying on the batch
//!
//! `StoredDocument::deltas_since_base` is the backend's own count of records after the newest base
//! (`spec.md:1396-1399`). A base resets it to 0; a delta advances it. So "a base was written" is a
//! question the storage contract already answers, and no test here has to inspect a
//! [`Batch`](cyrup_pico_store::Batch) — which is right, because ADR-0030 F3 keeps the assembled batch
//! opaque on purpose.

use core::sync::atomic::Ordering;

use cyrup_pico_doc::{DocValue, VersionFit, classify_version};
use cyrup_pico_store::{Cx, ROOT_CONVERSATION_ID};

use super::defs::{
    CHECKPOINT_CALLS, CHECKPOINT_SAW, Chatter, ChatterRewindable, Checkpointed, Live, LiveV2,
    LiveV2NoMigration, LiveV2Refusing,
};
use super::path;
use super::store_double::SharedStore;
use crate::{CallbackError, CommitOutcome, DocToken, RollbackReason, SessionMut, TxError};

/// Commit one `test.live` v1 document carrying `{"phase":"charging"}` and leave the store open.
async fn with_a_v1_document() -> SharedStore {
    let shared = SharedStore::new();
    let (store, _switch) = shared.open();
    let (_session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = DocToken::<Live>::define().expect("a definition").at();

    let CommitOutcome::Committed { .. } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the creating commit must succeed");
    };
    shared
}

// ---------------------------------------------------------------------------------------------
// Arm 1 of 4: `stored == token` -> use the value.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_equal_version_uses_the_stored_value() {
    let shared = with_a_v1_document().await;
    let (store, switch) = shared.open();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = DocToken::<Live>::define().expect("a definition").at();

    let CommitOutcome::Committed { .. } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("extra"), &1_u32)?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("an equal version is not a migration and must commit");
    };
    assert_eq!(switch.commits(), 1);

    let value = session
        .snapshot(at.address())
        .expect("healthy")
        .expect("present");
    assert!(
        matches!(value.get("phase"), Some(DocValue::Str(s)) if &**s == "charging"),
        "the stored value is used as read, not re-initialised"
    );
    let record = shared
        .find(at.address())
        .await
        .expect("a read")
        .expect("a record");
    let stored = shared
        .document(record.id)
        .await
        .expect("a read")
        .expect("content");
    assert_eq!(stored.def_version().get(), 1);
    assert_eq!(
        stored.deltas_since_base, 1,
        "an ordinary later mutation with no checkpoint is a delta"
    );
}

// ---------------------------------------------------------------------------------------------
// Arm 2 of 4: `stored < token` with a callback -> migrate.
// ---------------------------------------------------------------------------------------------

/// `spec.md:1447-1449` plus `spec.md:1463-1465`, in one case: the older version migrates, and the
/// first write afterwards is a **required base** *"even when the migrated JSON is deeply equal"*.
///
/// [`LiveV2`]'s migration is the identity — it returns `m.stored().clone()` — so the migrated value
/// is not merely equal to the stored one, it is the same `Arc`. If the base were written because the
/// value changed, this case would write a delta and `deltas_since_base` would be 2.
#[tokio::test]
async fn an_older_version_migrates_and_the_first_write_is_a_required_base() {
    let shared = with_a_v1_document().await;
    let at_v1 = DocToken::<Live>::define().expect("a definition").at();
    let before = {
        let record = shared
            .find(at_v1.address())
            .await
            .expect("a read")
            .expect("a record");
        shared
            .document(record.id)
            .await
            .expect("a read")
            .expect("content")
    };
    assert_eq!(before.def_version().get(), 1);
    assert_eq!(before.deltas_since_base, 0, "the creation wrote a base");

    let (store, switch) = shared.open();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = DocToken::<LiveV2>::define().expect("a definition").at();

    // Not one draft edit: the migration alone is the change.
    let CommitOutcome::Committed { .. } = handle
        .commit(&cx, async |mut tx| {
            let _doc = tx.doc(&at, ()).await?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("a migration is itself a change and must commit");
    };
    assert_eq!(
        switch.commits(),
        1,
        "the migration is staged in this transaction"
    );

    let after = {
        let record = shared
            .find(at.address())
            .await
            .expect("a read")
            .expect("a record");
        assert_eq!(record.id, before.record.id, "`id` is never reused");
        shared
            .document(record.id)
            .await
            .expect("a read")
            .expect("content")
    };
    assert_eq!(
        after.def_version().get(),
        2,
        "the required base is written at the TOKEN's version"
    );
    assert_eq!(
        after.deltas_since_base, 0,
        "a base, not a delta — `spec.md:4366-4367` forbids a delta crossing a version boundary"
    );
    assert_eq!(
        after.value, before.value,
        "the identity migration changed nothing, and the base was written anyway"
    );

    // The authority was rebased, not left at the pre-migration revision.
    let live = session
        .snapshot(at.address())
        .expect("healthy")
        .expect("present");
    assert_eq!(live, after.value);
}

/// The required base survives coalescing: `spec.md:1460`'s *"later draft edits coalesce into one
/// final required base"*.
#[tokio::test]
async fn draft_edits_after_a_migration_coalesce_into_one_base() {
    let shared = with_a_v1_document().await;
    let (store, switch) = shared.open();
    let (_session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = DocToken::<LiveV2>::define().expect("a definition").at();

    let CommitOutcome::Committed { .. } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("one"), &1_u32)?;
            // A second acquisition of the same address, after the migration: the slot is memoised,
            // and there is no method that could clear its `Continuity::Migrated`.
            let again = tx.doc(&at, ()).await?;
            tx.draft(again)?.set(&path("two"), &2_u32)?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the migrating commit must succeed");
    };
    assert_eq!(switch.commits(), 1, "one commit, not one per edit");

    let record = shared
        .find(at.address())
        .await
        .expect("a read")
        .expect("a record");
    let stored = shared
        .document(record.id)
        .await
        .expect("a read")
        .expect("content");
    assert_eq!(stored.def_version().get(), 2);
    assert_eq!(stored.deltas_since_base, 0, "one final base");
    assert_eq!(stored.value.len(), 3, "phase, one and two");
}

/// `spec.md:1459-1460`: *"callback failure persists nothing."*
#[tokio::test]
async fn a_refusing_migration_persists_nothing() {
    let shared = with_a_v1_document().await;
    let (store, switch) = shared.open();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = DocToken::<LiveV2Refusing>::define()
        .expect("a definition")
        .at();

    let outcome = handle
        .commit::<(), _>(&cx, async |mut tx| {
            let _doc = tx.doc(&at, ()).await?;
            Ok(((), tx.writing()))
        })
        .await;

    let CommitOutcome::RolledBack { reason, .. } = outcome else {
        panic!("a failing migration is a pre-admission rejection, not a poison");
    };
    assert!(matches!(
        reason,
        RollbackReason::Callback(CallbackError::Tx(TxError::MigrationFailed { .. }))
    ));
    assert_eq!(switch.commits(), 0, "nothing reached storage");
    assert_eq!(session.publications(), 0);

    let record = shared
        .find(at.address())
        .await
        .expect("a read")
        .expect("a record");
    let stored = shared
        .document(record.id)
        .await
        .expect("a read")
        .expect("content");
    assert_eq!(stored.def_version().get(), 1, "still at the stored version");
}

// ---------------------------------------------------------------------------------------------
// Arms 3 and 4 of 4: `stored > token` rejects; `stored < token` with no callback rejects.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_older_version_with_no_migration_is_refused() {
    let shared = with_a_v1_document().await;
    let (store, switch) = shared.open();
    let (_session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = DocToken::<LiveV2NoMigration>::define()
        .expect("a definition")
        .at();

    let outcome = handle
        .commit::<(), _>(&cx, async |mut tx| {
            let _doc = tx.doc(&at, ()).await?;
            Ok(((), tx.writing()))
        })
        .await;

    let CommitOutcome::RolledBack { reason, .. } = outcome else {
        panic!("`no migrate -> reject older stored version`");
    };
    assert!(matches!(
        reason,
        RollbackReason::Callback(CallbackError::Tx(TxError::NoMigration {
            stored,
            token,
            ..
        })) if stored.get() == 1 && token.get() == 2
    ));
    assert_eq!(switch.commits(), 0);
}

/// `spec.md:1443`: *"`stored > token` -> reject typed access."* Extension code older than its own
/// data must not reinterpret it.
#[tokio::test]
async fn a_newer_stored_version_is_refused() {
    // Write at version 2 first.
    let shared = SharedStore::new();
    {
        let (store, _switch) = shared.open();
        let (_session, handle) = SessionMut::open(Box::new(store));
        let cx = Cx::detached();
        let at = DocToken::<LiveV2>::define().expect("a definition").at();
        let CommitOutcome::Committed { .. } = handle
            .commit(&cx, async |mut tx| {
                let doc = tx.doc(&at, ()).await?;
                tx.draft(doc)?.set(&path("phase"), "charging")?;
                Ok(((), tx.writing()))
            })
            .await
        else {
            panic!("the v2 creation must succeed");
        };
    }

    let (store, switch) = shared.open();
    let (_session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = DocToken::<Live>::define().expect("a definition").at();

    let outcome = handle
        .commit::<(), _>(&cx, async |mut tx| {
            let _doc = tx.doc(&at, ()).await?;
            Ok(((), tx.writing()))
        })
        .await;

    let CommitOutcome::RolledBack { reason, .. } = outcome else {
        panic!("a newer stored version rejects typed access");
    };
    assert!(matches!(
        reason,
        RollbackReason::Callback(CallbackError::Tx(TxError::StoredVersionIsNewer {
            stored,
            token,
            ..
        })) if stored.get() == 2 && token.get() == 1
    ));
    assert_eq!(switch.commits(), 0);
}

/// The ladder is one pure function, and the four arms are exhaustive. Asserting that here as well as
/// through the Session is deliberate: this is the case that turns red if a fifth outcome is ever
/// added and a `_ =>` arm is reached for it.
#[test]
fn the_ladder_has_exactly_four_arms() {
    use cyrup_pico_doc::{DefVersion, ReplayPlan, StoredContent};

    let v = |n: u32| DefVersion::new(core::num::NonZeroU32::new(n).expect("a positive version"));
    let stored_at = |n: u32| {
        let records = [StoredContent::Base {
            version: v(n),
            value: cyrup_pico_doc::DocRoot::empty(),
        }];
        ReplayPlan::parse(&records)
            .expect("one base is a legal tail")
            .version()
    };

    assert!(matches!(
        classify_version(stored_at(1), v(1), false),
        VersionFit::Current
    ));
    assert!(matches!(
        classify_version(stored_at(1), v(2), true),
        VersionFit::Migrate { .. }
    ));
    assert!(matches!(
        classify_version(stored_at(2), v(1), true),
        VersionFit::StoredIsNewer { .. }
    ));
    assert!(matches!(
        classify_version(stored_at(1), v(2), false),
        VersionFit::NoMigration { .. }
    ));
}

// ---------------------------------------------------------------------------------------------
// The token-versus-record agreement check (`G-TOKEN-AGREES-WITH-RECORD`).
// ---------------------------------------------------------------------------------------------

/// `spec.md:1063-1065` and `:1117-1119`: a token that redeclares a conversation document's history is
/// refused, and the **record** is what refuses it.
///
/// A runtime check, correctly: [`ChatterRewindable`] is what reloaded extension code looks like, and
/// the alternative — preferring the token — is how a `latest` document starts answering historical
/// reads from legitimately reclaimed records.
#[tokio::test]
async fn a_token_that_redeclares_history_is_refused() {
    let shared = SharedStore::new();
    let latest = DocToken::<Chatter>::define().expect("a definition");
    let at_latest = latest.in_conversation(ROOT_CONVERSATION_ID);
    {
        let (store, _switch) = shared.open();
        let (_session, handle) = SessionMut::open(Box::new(store));
        let cx = Cx::detached();
        let CommitOutcome::Committed { .. } = handle
            .commit(&cx, async |mut tx| {
                let doc = tx.doc(&at_latest, ()).await?;
                tx.draft(doc)?.set(&path("phase"), "charging")?;
                Ok(((), tx.writing()))
            })
            .await
        else {
            panic!("the creating commit must succeed");
        };
    }

    let (store, switch) = shared.open();
    let (_session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let rewindable = DocToken::<ChatterRewindable>::define().expect("a definition");
    let at_rewindable = rewindable.in_conversation(ROOT_CONVERSATION_ID);
    assert_eq!(
        at_rewindable.address(),
        at_latest.address(),
        "the same logical address: only the policy differs"
    );

    let outcome = handle
        .commit::<(), _>(&cx, async |mut tx| {
            let _doc = tx.doc(&at_rewindable, ()).await?;
            Ok(((), tx.writing()))
        })
        .await;

    let CommitOutcome::RolledBack { reason, .. } = outcome else {
        panic!("a disagreeing policy rejects typed access");
    };
    assert!(matches!(
        reason,
        RollbackReason::Callback(CallbackError::Tx(TxError::TokenDisagreesWithRecord { .. }))
    ));
    assert_eq!(switch.commits(), 0, "nothing was rewritten");

    // And the record still says what it always said: the data did not disappear.
    let record = shared
        .find(at_latest.address())
        .await
        .expect("a read")
        .expect("a record");
    assert!(
        !record.retains_history(),
        "the stored policy is untouched by a token that disagreed with it"
    );
}

/// The same check guards the retirement path, which resolves an address without creating it.
#[tokio::test]
async fn retirement_also_checks_the_record() {
    let shared = SharedStore::new();
    let at_latest = DocToken::<Chatter>::define()
        .expect("a definition")
        .in_conversation(ROOT_CONVERSATION_ID);
    {
        let (store, _switch) = shared.open();
        let (_session, handle) = SessionMut::open(Box::new(store));
        let cx = Cx::detached();
        let CommitOutcome::Committed { .. } = handle
            .commit(&cx, async |mut tx| {
                let doc = tx.doc(&at_latest, ()).await?;
                tx.draft(doc)?.set(&path("phase"), "charging")?;
                Ok(((), tx.writing()))
            })
            .await
        else {
            panic!("the creating commit must succeed");
        };
    }

    let (store, switch) = shared.open();
    let (_session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at_rewindable = DocToken::<ChatterRewindable>::define()
        .expect("a definition")
        .in_conversation(ROOT_CONVERSATION_ID);

    let outcome = handle
        .commit::<(), _>(&cx, async |mut tx| {
            tx.retire_doc(&at_rewindable).await?;
            Ok(((), tx.writing()))
        })
        .await;

    assert!(matches!(
        outcome,
        CommitOutcome::RolledBack {
            reason: RollbackReason::Callback(CallbackError::Tx(
                TxError::TokenDisagreesWithRecord { .. }
            )),
            ..
        }
    ));
    assert_eq!(switch.commits(), 0);
}

// ---------------------------------------------------------------------------------------------
// Retirement, and the new incarnation a later acquisition creates.
// ---------------------------------------------------------------------------------------------

/// `spec.md:1242-1246`: retirement resolves without creating; an acquired draft's final content is
/// persisted first; and a later `tx.doc()` at that address **in the same transaction** creates a new
/// incarnation with a new id.
#[tokio::test]
async fn retirement_persists_final_content_and_a_later_acquisition_is_a_new_incarnation() {
    let shared = SharedStore::new();
    let at = DocToken::<Live>::define().expect("a definition").at();

    let (store, switch) = shared.open();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();

    // Retiring an address that does not exist creates nothing and stages nothing.
    let outcome = handle
        .commit::<(), _>(&cx, async |mut tx| {
            tx.retire_doc(&at).await?;
            Ok(((), tx.writing()))
        })
        .await;
    let CommitOutcome::NothingToCommit {
        session: handle, ..
    } = outcome
    else {
        panic!("`retireDoc()` resolves the logical address without creating it");
    };
    assert_eq!(switch.commits(), 0);
    assert!(shared.find(at.address()).await.expect("a read").is_none());

    // Create, write, retire and re-create, all in one transaction.
    let CommitOutcome::Committed { result: ids, .. } = handle
        .commit(&cx, async |mut tx| {
            let first = tx.doc(&at, ()).await?;
            tx.draft(first)?.set(&path("phase"), "charging")?;
            tx.retire_doc(&at).await?;
            let second = tx.doc(&at, ()).await?;
            tx.draft(second)?.set(&path("phase"), "discharging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the commit must succeed");
    };
    let () = ids;
    assert_eq!(switch.commits(), 1);

    let record = shared
        .find(at.address())
        .await
        .expect("a read")
        .expect("the NEW incarnation is live at the address");
    let stored = shared
        .document(record.id)
        .await
        .expect("a read")
        .expect("content");
    assert!(
        matches!(stored.value.get("phase"), Some(DocValue::Str(s)) if &**s == "discharging"),
        "the live incarnation is the one created after the retirement"
    );
    let live = session
        .snapshot(at.address())
        .expect("healthy")
        .expect("present");
    assert_eq!(live, stored.value);
}

// ---------------------------------------------------------------------------------------------
// `G-CHECKPOINT-ONCE`.
// ---------------------------------------------------------------------------------------------

/// `spec.md:1395-1399`: the predicate is evaluated **exactly once** per prepared change, with a
/// `deltasSinceBase` that **excludes** the change being evaluated, and performs no storage read.
///
/// The third clause is structural — [`CheckpointInput`](cyrup_pico_doc::CheckpointInput) has no
/// storage handle — so what is left to assert is the first two, and the counter is how.
#[tokio::test]
async fn the_checkpoint_predicate_runs_once_per_change_on_a_count_that_excludes_it() {
    CHECKPOINT_CALLS.store(0, Ordering::SeqCst);
    let shared = SharedStore::new();
    let (store, _switch) = shared.open();
    let (_session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = DocToken::<Checkpointed>::define()
        .expect("a definition")
        .at();

    // 1. Creation: a required base, and `spec.md:1400` says the predicate is NOT called.
    let CommitOutcome::Committed {
        session: mut handle,
        ..
    } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("n"), &0_u32)?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the creating commit must succeed");
    };
    assert_eq!(
        CHECKPOINT_CALLS.load(Ordering::SeqCst),
        0,
        "creation requires a base and does not call the predicate"
    );

    // 2., 3. and 4.: three later mutations, each one evaluation.
    for n in 1_u32..=3 {
        let CommitOutcome::Committed { session: next, .. } = handle
            .commit(&cx, async |mut tx| {
                let doc = tx.doc(&at, ()).await?;
                // Two draft edits in one change: still ONE evaluation, because the predicate runs
                // after preparation and preparation happens once.
                let mut draft = tx.draft(doc)?;
                draft.set(&path("n"), &n)?;
                draft.set(&path("m"), &n)?;
                Ok(((), tx.writing()))
            })
            .await
        else {
            panic!("mutation {n} must commit");
        };
        handle = next;
        assert_eq!(
            CHECKPOINT_CALLS.load(Ordering::SeqCst),
            n,
            "exactly one evaluation per prepared change"
        );
        assert_eq!(
            CHECKPOINT_SAW.load(Ordering::SeqCst),
            n - 1,
            "the count excludes the change being evaluated"
        );
    }

    // The predicate returned true at `deltas_since_base >= 2`, which is the third mutation, so the
    // tail was reset there.
    let record = shared
        .find(at.address())
        .await
        .expect("a read")
        .expect("a record");
    let stored = shared
        .document(record.id)
        .await
        .expect("a read")
        .expect("content");
    assert_eq!(
        stored.deltas_since_base, 0,
        "the selected checkpoint wrote a base and reset the replay tail"
    );
}

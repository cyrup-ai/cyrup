//! Drafts and the in-flight document set: the halves of invariants 6 and 2 that a runtime test can
//! still see.

use cyrup_pico_doc::DocValue;
use cyrup_pico_store::Cx;

use super::defs::Counter;
use super::store_double::FaultyStore;
use super::{live_address, path};
use crate::{CommitOutcome, DocToken, FamilyKey, SessionMut};

/// `spec.md:1232-1234`: a logical address is acquired once per transaction, and the second
/// acquisition returns the same slot rather than reading storage again.
#[tokio::test]
async fn a_second_acquisition_of_one_address_is_the_same_document() {
    let (store, _switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let address = live_address();

    let CommitOutcome::Committed { result, .. } = handle
        .commit(&cx, async |mut tx| {
            let first = tx.doc(&address, ()).await?;
            tx.draft(first)?.set(&path("a"), &1_u32)?;
            // A second acquisition, with a DIFFERENT seed. The first call's seed wins, because the
            // later one is never consulted.
            let second = tx.doc(&address, ()).await?;
            tx.draft(second)?.set(&path("b"), &2_u32)?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the commit must succeed");
    };
    let () = result;

    let value = session
        .snapshot(address.address())
        .expect("the authority is healthy")
        .expect("one incarnation, not two");
    assert_eq!(value.len(), 2, "both writes landed in one document");
    assert_eq!(session.publications(), 1);
}

/// `spec.md:1357`: *"an existing current-version document with an empty batch writes and publishes
/// nothing."*
#[tokio::test]
async fn an_empty_change_on_an_existing_document_writes_nothing() {
    let (store, switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let address = live_address();

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
    assert_eq!(switch.commits(), 1);

    // Acquire it and write nothing.
    let outcome = handle
        .commit(&cx, async |mut tx| {
            let _doc = tx.doc(&address, ()).await?;
            Ok(((), tx.writing()))
        })
        .await;
    assert!(
        matches!(outcome, CommitOutcome::NothingToCommit { .. }),
        "an acquisition with no write is not a change"
    );
    assert_eq!(switch.commits(), 1, "no second storage commit");
    assert_eq!(
        session.publications(),
        1,
        "a throttled live document must not amplify every tick into a publication"
    );
}

/// The one strict-JSON check that survives into Rust, at the assignment where upstream puts it.
///
/// ADR-0030 §7 calls the `NaN` case *"the regression this type exists to prevent"*, and the reason it
/// matters here rather than only in `cyrup-pico-doc` is the failure mode: a `NaN` that reaches storage
/// makes the session unopenable, detected one restart later — total loss.
#[tokio::test]
async fn a_non_finite_float_is_rejected_at_the_assignment() {
    let (store, switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let address = live_address();

    let outcome = handle
        .commit::<(), _>(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            let mut draft = tx.draft(doc)?;
            draft.set(&path("ok"), &1_u32)?;
            draft.set(&path("bad"), &f64::NAN)?;
            Ok(((), tx.writing()))
        })
        .await;

    assert!(
        matches!(outcome, CommitOutcome::RolledBack { .. }),
        "the assignment fails, and a pre-admission failure rolls back"
    );
    assert_eq!(switch.commits(), 0, "nothing reached storage");
    assert_eq!(session.publications(), 0);
}

/// `spec.md:1232-1234`: *"for a missing family, the first call's detached seed wins and later seeds
/// are ignored"*, and `spec.md:1230-1232`: a seed is consumed **only** when the member is absent.
///
/// Both halves are observable here because [`Counter`]'s value records the seed it was created from.
/// The second acquisition in the creating transaction carries a different seed, and the third — in a
/// later transaction, against a member that now exists — carries a third: neither reaches the stored
/// value, and the second one is never even evaluated, because
/// [`DocDef::initial`](crate::DocDef::initial) is called inside the absent branch.
#[tokio::test]
async fn a_family_seed_is_consumed_only_when_the_member_is_absent() {
    let (store, switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let token: DocToken<Counter> = DocToken::define().expect("a definition");
    let at = token.at_key(FamilyKey::parse("alpha").expect("a key"));

    let CommitOutcome::Committed {
        session: handle, ..
    } = handle
        .commit(&cx, async |mut tx| {
            let first = tx.doc(&at, 1_u32).await?;
            let second = tx.doc(&at, 2_u32).await?;
            // The same slot: one document, not two.
            tx.draft(second)?.set(&path("touched"), &true)?;
            let _ = first;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the creating commit must succeed");
    };
    assert_eq!(switch.commits(), 1);

    let CommitOutcome::Committed { .. } = handle
        .commit(&cx, async |mut tx| {
            // A third seed, against a member that now exists.
            let existing = tx.doc(&at, 99_u32).await?;
            tx.draft(existing)?.set(&path("again"), &true)?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the second commit must succeed");
    };

    let value = session
        .snapshot(at.address())
        .expect("the authority is healthy")
        .expect("the member exists");
    let seed = match value.get("seed") {
        Some(DocValue::Num(n)) => n.as_u64(),
        _ => None,
    };
    assert_eq!(
        seed,
        Some(1),
        "the first call's seed wins; the second and third are never evaluated"
    );
}

/// A published revision is shareable and immutable: what a reader retains cannot be reached mutably,
/// and the next commit does not change it.
#[tokio::test]
async fn a_retained_revision_does_not_change_under_a_later_commit() {
    let (store, _switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let address = live_address();

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
    let retained = session
        .snapshot(address.address())
        .expect("healthy")
        .expect("present");

    let CommitOutcome::Committed { .. } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "discharging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the second commit must succeed");
    };

    let phase = match retained.get("phase") {
        Some(DocValue::Str(s)) => Some(&**s),
        _ => None,
    };
    assert_eq!(
        phase,
        Some("charging"),
        "a retained revision is immutable for all time (`spec.md:4528-4530`)"
    );
    let current = session
        .snapshot(address.address())
        .expect("healthy")
        .expect("present");
    assert!(
        !current.shares_allocation_with(&retained),
        "the new revision is a different value, structurally shared where it can be"
    );
}

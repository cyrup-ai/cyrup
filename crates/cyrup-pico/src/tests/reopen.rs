//! PICO5-PLAN S5's fourth case: *"unaccessed documents with unavailable definitions surviving a
//! reopen byte-identically."*
//!
//! `spec.md:1468-1469`: *"unaccessed documents and documents with unavailable definitions preserve their
//! stored instances, versions, and bytes."* This is the clause that makes `spec.md:1447-1449`'s
//! access-driven migration more than a performance decision — a sweep at open would rewrite every
//! extension's documents on every start, and *"one failing migration makes the session unopenable"*
//! (ADR-0030 §2.3).
//!
//! # What "byte-identically" is checked as
//!
//! The backend's representation is deliberately private (ADR-0030 F3: *"the base and the tail are not
//! here"*), so the strongest honest comparison is the one the storage contract exposes: the record,
//! the stored definition **version**, the materialized value's serialization, and
//! `deltas_since_base` — which is the count of records after the newest base and therefore turns red
//! if anything was appended. A migration or a touched document changes at least one of the four.

use cyrup_pico_store::Cx;

use super::defs::{Live, LiveV2, Other};
use super::path;
use super::store_double::SharedStore;
use crate::{CommitOutcome, DocToken, SessionMut};

/// Everything the storage contract says about one document, as one comparable value.
fn fingerprint(stored: &cyrup_pico_store::StoredDocument) -> String {
    let record = serde_json::to_string(&stored.record).unwrap_or_else(|e| e.to_string());
    let value = serde_json::to_string(&stored.value).unwrap_or_else(|e| e.to_string());
    format!(
        "{record}|v{}|deltas={}|{value}",
        stored.def_version(),
        stored.deltas_since_base
    )
}

#[tokio::test]
async fn an_unaccessed_document_survives_a_reopen_and_a_neighbours_migration_untouched() {
    let shared = SharedStore::new();
    let live = DocToken::<Live>::define().expect("a definition").at();
    let other = DocToken::<Other>::define().expect("a definition").at();

    // Session 1: create both documents at version 1.
    {
        let (store, _switch) = shared.open();
        let (_session, handle) = SessionMut::open(Box::new(store));
        let cx = Cx::detached();
        let CommitOutcome::Committed { .. } = handle
            .commit(&cx, async |mut tx| {
                let a = tx.doc(&live, ()).await?;
                tx.draft(a)?.set(&path("phase"), "charging")?;
                let b = tx.doc(&other, ()).await?;
                tx.draft(b)?.set(&path("note"), "do not touch")?;
                Ok(((), tx.writing()))
            })
            .await
        else {
            panic!("the creating commit must succeed");
        };
    }

    let other_record = shared
        .find(other.address())
        .await
        .expect("a read")
        .expect("a record");
    let before = fingerprint(
        &shared
            .document(other_record.id)
            .await
            .expect("a read")
            .expect("content"),
    );

    // Session 2: a fresh Session over the same durable state. `test.other`'s definition is
    // "unavailable" in the only sense that matters — this Session never names it — and `test.live`
    // is acquired through a token at a NEWER version, so a sweep-shaped implementation would have
    // migrated both.
    {
        let (store, switch) = shared.open();
        let (_session, handle) = SessionMut::open(Box::new(store));
        let cx = Cx::detached();
        let live_v2 = DocToken::<LiveV2>::define().expect("a definition").at();
        let CommitOutcome::Committed { .. } = handle
            .commit(&cx, async |mut tx| {
                let a = tx.doc(&live_v2, ()).await?;
                tx.draft(a)?.set(&path("phase"), "discharging")?;
                Ok(((), tx.writing()))
            })
            .await
        else {
            panic!("the reopening commit must succeed");
        };
        assert_eq!(
            switch.commits(),
            1,
            "one commit, for the one document touched"
        );
    }

    // The accessed one moved to version 2.
    let live_record = shared
        .find(live.address())
        .await
        .expect("a read")
        .expect("a record");
    let live_stored = shared
        .document(live_record.id)
        .await
        .expect("a read")
        .expect("content");
    assert_eq!(
        live_stored.def_version().get(),
        2,
        "migration is access-driven, and this one was accessed"
    );

    // The unaccessed one did not.
    let after_record = shared
        .find(other.address())
        .await
        .expect("a read")
        .expect("a record");
    assert_eq!(
        after_record.id, other_record.id,
        "no new incarnation was created for it"
    );
    let after = fingerprint(
        &shared
            .document(after_record.id)
            .await
            .expect("a read")
            .expect("content"),
    );
    assert_eq!(
        after, before,
        "an unaccessed document preserves its stored instance, version and bytes"
    );
}

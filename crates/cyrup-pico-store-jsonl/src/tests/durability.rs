//! The two tiers, and the asymmetry §11.3 turns on.
//!
//! > ADR-0030 §2.2: *"a marker surviving without its data is corruption; losing a tail commit is merely
//! > recoverable — the asymmetry is the subtle part."*
//!
//! Both halves are asserted for **both** tiers, because the asymmetry is a property of the protocol's
//! ordering rather than of the flush: the flush is what makes the ordering survive power loss, and the
//! ordering is what makes the two outcomes different in the first place.
//!
//! # What cannot be asserted here, stated rather than left implied
//!
//! That the strong tier's `fdatasync` *happened*. There is no in-process way to drop the page cache, so
//! the outcome a flush protects against cannot be produced from a test — which is exactly why
//! `cyrup-session`'s `durable_rename` is tested by observing the syscall instead of its effect, and why
//! [`a_replacement_flushes_its_directory_entry`] does the same here. The cases below assert the two
//! recovery outcomes, which are what a user sees.

use cyrup_pico_store::{Document, DocumentId, Storage as _, StorageFailure};

use super::fixture::{self, open, open_strong};
use crate::Durability;

/// Half one of the asymmetry: a commit whose **marker** was lost is simply absent, and the store opens.
///
/// `spec.md:4424-4426`: *"ordinary publication does not explicitly flush `main.jsonl`; an acknowledged
/// tail commit may therefore still disappear."* Its sidecar record is still on disk, unconfirmed, and
/// recovery removes it.
#[tokio::test]
async fn a_lost_tail_commit_is_recoverable_under_both_tiers() {
    for tier in [Durability::ProcessCrash, Durability::PowerLoss] {
        let dir = fixture::dir();
        let mut store = open(dir.path(), tier);
        fixture::seed_root(&mut store).await;
        let doc: DocumentId = fixture::mint::<Document>(&mut store).await;
        fixture::create_document(
            &mut store,
            fixture::rewindable_doc(doc, fixture::kind("cyrup.state")),
            fixture::value("n", 1),
        )
        .await;
        fixture::append_delta(&mut store, doc, "n", 2).await;
        drop(store);

        let sidecar = fixture::sidecar(dir.path(), doc, 0);
        let full = std::fs::metadata(&sidecar).expect("the sidecar").len();

        // Lose the newest marker, as a power cut between the sidecar flush and the marker's writeback
        // would. The delta's bytes are still there; nothing confirms them.
        let mut lines = fixture::main_lines(dir.path());
        let marker = lines
            .iter()
            .rposition(|l| l.contains("\"t\":\"marker\""))
            .expect("a marker");
        lines.truncate(marker);
        fixture::write_main_lines(dir.path(), &lines);

        let store = open(dir.path(), tier);
        let value = fixture::read(&store, doc).await;
        assert_eq!(
            value.value,
            fixture::value("n", 1),
            "{tier:?}: the surviving commits are exactly the confirmed ones"
        );
        assert_eq!(
            value.deltas_since_base, 0,
            "{tier:?}: the unconfirmed delta must not be replayed"
        );
        let trimmed = std::fs::metadata(&sidecar).expect("the sidecar").len();
        assert!(
            trimmed < full,
            "{tier:?}: `spec.md:4429` asks for the unconfirmed sidecar tail to be removed, not only ignored"
        );
    }
}

/// Half two: a **marker without its data** is corruption, and the open fails rather than returning less.
///
/// `spec.md:4433`: *"missing required confirmed data is corruption and opening fails."* This is also the
/// past-EOF offset case ADR-0030 §14 open question 3 asks recovery to treat as corruption.
#[tokio::test]
async fn a_marker_without_its_data_fails_the_open_under_both_tiers() {
    for tier in [Durability::ProcessCrash, Durability::PowerLoss] {
        let dir = fixture::dir();
        let mut store = open(dir.path(), tier);
        fixture::seed_root(&mut store).await;
        let doc: DocumentId = fixture::mint::<Document>(&mut store).await;
        fixture::create_document(
            &mut store,
            fixture::rewindable_doc(doc, fixture::kind("cyrup.state")),
            fixture::value("n", 1),
        )
        .await;
        fixture::append_delta(&mut store, doc, "n", 2).await;
        drop(store);

        // The marker survived; its data did not.
        fixture::truncate(&fixture::sidecar(dir.path(), doc, 0), 1);

        let lock = crate::StoreLock::acquire(dir.path()).expect("the lock");
        let refused =
            crate::JsonlStore::open_for_write(dir.path(), lock, crate::JsonlOptions::new(tier))
                .expect_err("a marker whose data is missing must fail the open");

        match refused {
            StorageFailure::Corrupt(c) => {
                let rendered = c.to_string();
                assert!(
                    rendered.contains("confirm"),
                    "{tier:?}: the failure must name the offsets that are not backed by bytes: {rendered}"
                );
            }
            other => panic!("{tier:?}: expected corruption, got {other}"),
        }
    }
}

/// A sidecar that is gone entirely is the same answer, and for the same reason.
#[tokio::test]
async fn a_missing_sidecar_fails_the_open() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let doc: DocumentId = fixture::mint::<Document>(&mut store).await;
    fixture::create_document(
        &mut store,
        fixture::latest_doc(doc, fixture::kind("cyrup.state")),
        fixture::value("n", 1),
    )
    .await;
    drop(store);
    std::fs::remove_file(fixture::sidecar(dir.path(), doc, 0)).expect("removing the sidecar");

    let lock = crate::StoreLock::acquire(dir.path()).expect("the lock");
    let refused = crate::JsonlStore::open_for_write(
        dir.path(),
        lock,
        crate::JsonlOptions::new(Durability::PowerLoss),
    )
    .expect_err("a document record with no sidecar must fail the open");

    assert!(
        matches!(refused, StorageFailure::Corrupt(_)),
        "expected corruption, got {refused}"
    );
}

/// Both tiers produce the same bytes and the same published state. The tier is a claim about *when* the
/// bytes reach the device, never about what they say.
#[tokio::test]
async fn the_tiers_write_the_same_log() {
    let mut logs = Vec::new();
    for tier in [Durability::ProcessCrash, Durability::PowerLoss] {
        let dir = fixture::dir();
        let mut store = open(dir.path(), tier);
        assert_eq!(store.durability(), tier);
        fixture::seed_root(&mut store).await;
        let doc: DocumentId = fixture::mint::<Document>(&mut store).await;
        fixture::create_document(
            &mut store,
            fixture::rewindable_doc(doc, fixture::kind("cyrup.state")),
            fixture::value("n", 1),
        )
        .await;
        fixture::append_delta(&mut store, doc, "n", 2).await;
        store.close(&fixture::cx()).await.expect("a clean close");
        logs.push(fixture::main_lines(dir.path()).join("\n"));
    }
    let (weak, strong) = (
        logs.first().expect("two logs"),
        logs.get(1).expect("two logs"),
    );
    assert_eq!(
        weak, strong,
        "the two tiers differ in flushes, not in records"
    );
}

/// The mechanism assertion for ADR-0030 F6 §D's *"durable rename"*: the directory whose entry the
/// replacement rewrote is `fsync`ed.
///
/// Observed at the real syscall, after it returned `Ok` ([`crate::durable`]), so this test cannot pass
/// with the call removed. Power loss itself is not simulable in-process, which is why the mechanism and
/// not the outcome is what is asserted — the same reasoning `crates/cyrup-session/src/durable.rs` gives.
#[cfg(unix)]
#[tokio::test]
async fn a_replacement_flushes_its_directory_entry() {
    let dir = fixture::dir();
    assert!(
        !crate::durable::dir_was_fsynced(dir.path()),
        "a fresh temp directory cannot have been flushed yet"
    );

    // `open_for_write` claims the identity record through a durable replacement, so one open is enough
    // to exercise the sequence — and every reclamation takes the same path.
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    drop(store);
    let _second_open = open_strong(dir.path());

    assert!(
        crate::durable::dir_was_fsynced(dir.path()),
        "a replacement must flush the directory entry naming it, or the rename is durable only as far \
         as the page cache"
    );
}

/// `spec.md:4418`: *"every commit uses this protocol; there is no standalone-sidecar fast path."*
///
/// A commit that writes **only** document content has no main record of its own, and still appends its
/// marker — so `main.jsonl` grows by exactly one line and the commit is published by that line and by
/// nothing else. A backend that took a shortcut here would publish content no marker confirms, which is
/// the one corruption `spec.md:4425` names.
#[tokio::test]
async fn a_content_only_commit_still_appends_its_marker() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let doc: DocumentId = fixture::mint::<Document>(&mut store).await;
    fixture::create_document(
        &mut store,
        fixture::rewindable_doc(doc, fixture::kind("cyrup.state")),
        fixture::value("n", 1),
    )
    .await;
    let before = fixture::main_lines(dir.path());

    // A delta: one sidecar record, no main record.
    fixture::append_delta(&mut store, doc, "n", 2).await;

    let after = fixture::main_lines(dir.path());
    assert_eq!(
        after.len(),
        before.len().saturating_add(1),
        "a content-only commit must add exactly its marker to the main log"
    );
    assert!(
        after.last().is_some_and(|l| l.contains("\"t\":\"marker\"")),
        "and that line must be the marker: {after:?}"
    );
    assert_eq!(
        after.last().map(|l| l.contains("\"main\":0")),
        Some(true),
        "the marker must say it confirms no main records rather than omitting the count"
    );
}

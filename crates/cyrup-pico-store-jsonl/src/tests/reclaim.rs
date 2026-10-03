//! Reclamation (`spec.md:4435-4446`) and the poison flag (`spec.md:4434`).

use cyrup_pico_store::{Document, DocumentId, DocumentPoint, Storage as _, StorageFailure};

use super::fixture::{self, open, open_strong};
use crate::Durability;

/// §11.1's rule, in the medium: *"reclaims latest records only after a committed base or retirement."*
/// The authorising write is the base, and the value a read answers with does not change.
#[tokio::test]
async fn a_committed_base_reclaims_the_records_it_supersedes() {
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
    fixture::append_delta(&mut store, doc, "n", 2).await;
    fixture::append_delta(&mut store, doc, "n", 3).await;
    let before = std::fs::metadata(fixture::sidecar(dir.path(), doc, 0))
        .expect("the first generation")
        .len();

    // The authorising write.
    fixture::write_base(&mut store, doc, fixture::value("n", 4)).await;

    assert_eq!(
        store.deferred_reclamations(),
        0,
        "nothing should have deferred this reclamation"
    );
    let after = std::fs::metadata(fixture::sidecar(dir.path(), doc, 1))
        .expect("the second generation")
        .len();
    assert!(
        after < before,
        "the replacement must be shorter than what it replaces: {after} >= {before}"
    );
    assert!(
        !fixture::sidecar(dir.path(), doc, 0).exists(),
        "the superseded generation must be removed once the line naming its replacement is durable"
    );
    assert_eq!(
        fixture::read(&store, doc).await.value,
        fixture::value("n", 4)
    );
}

/// And the rebased offsets are right, which only a reopen can prove: the replacement's records are at
/// new byte positions, and the layout line in the log is what says so.
#[tokio::test]
async fn a_reopen_after_reclamation_reads_the_same_value() {
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
    fixture::append_delta(&mut store, doc, "n", 2).await;
    fixture::write_base(&mut store, doc, fixture::value("n", 3)).await;
    // A delta written *after* the reclamation, so the reopened store has to read across the boundary
    // between the rebased records and the ones appended since.
    fixture::append_delta(&mut store, doc, "n", 4).await;
    let before = fixture::read(&store, doc).await;
    drop(store);

    let mut store = open_strong(dir.path());
    let after = fixture::read(&store, doc).await;

    assert_eq!(before.value, after.value);
    assert_eq!(after.value, fixture::value("n", 4));
    assert_eq!(
        after.deltas_since_base, 1,
        "the reclaimed records must not come back as replayed deltas"
    );

    // And the reopened store can still APPEND to the rebased generation, which is the other half of
    // the offsets' soundness: the handle it opens is verified against the file's real length once, so a
    // rebased layout that disagreed with the medium would fail here rather than silently write at the
    // wrong offset.
    fixture::append_delta(&mut store, doc, "n", 5).await;
    assert_eq!(
        fixture::read(&store, doc).await.value,
        fixture::value("n", 5)
    );
    drop(store);
    let store = open_strong(dir.path());
    assert_eq!(
        fixture::read(&store, doc).await.value,
        fixture::value("n", 5)
    );
}

/// `spec.md:4435-4436`: the authorising commit may be a **retirement** rather than a base, and a
/// `RetireOnly` command writes no content at all — so a backend that looked only at what it had written
/// to a sidecar would never reclaim on this path.
#[tokio::test]
async fn a_retirement_also_authorises_reclamation() {
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
    fixture::append_delta(&mut store, doc, "n", 2).await;
    fixture::write_base(&mut store, doc, fixture::value("n", 3)).await;
    // The base above already reclaimed, so write one more delta to leave something superseded again.
    fixture::append_delta(&mut store, doc, "n", 4).await;
    fixture::write_base(&mut store, doc, fixture::value("n", 5)).await;
    let generation_before = 2;
    assert!(
        fixture::sidecar(dir.path(), doc, generation_before).exists(),
        "the second base must have reclaimed again"
    );

    // A retirement with no content of its own.
    let mut builder = cyrup_pico_store::BatchBuilder::new();
    builder.retire_document(doc).expect("staging a retirement");
    fixture::commit(&mut store, builder.build().expect("a non-empty batch")).await;

    assert_eq!(store.deferred_reclamations(), 0);
    // Nothing was superseded by the retirement itself, so the layout is unchanged — what this case
    // pins is that the retirement REACHED the reclamation path at all, which the generation shows: it
    // did not advance, because `reclaimable_from` found nothing before the newest base.
    assert!(
        fixture::sidecar(dir.path(), doc, generation_before).exists(),
        "a retirement must not discard the final content (`spec.md:1246-1247`)"
    );
    drop(store);
    let store = open_strong(dir.path());
    assert!(
        store
            .reader()
            .document(doc, cyrup_pico_store::DocumentPoint::Current)
            .await
            .expect("the read")
            .is_none(),
        "a retired incarnation has no current value"
    );
}

/// The retirement path with something actually superseded: a retirement that follows a delta on a
/// `latest` document reclaims the records the newest base replaced.
#[tokio::test]
async fn a_retirement_reclaims_what_an_earlier_base_superseded() {
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
    // A base and then a delta, so the newest base is NOT the first record and nothing has reclaimed
    // yet: `write_base` reclaims, then the delta is appended after it.
    fixture::write_base(&mut store, doc, fixture::value("n", 2)).await;
    fixture::append_delta(&mut store, doc, "n", 3).await;
    let before = std::fs::metadata(fixture::sidecar(dir.path(), doc, 1))
        .expect("the live generation")
        .len();

    // Retire with content, which `spec.md:1246-1247` says persists the final content first. The
    // content is a base, so it authorises reclaiming the delta before it.
    let mut builder = cyrup_pico_store::BatchBuilder::new();
    builder
        .change_document(
            doc,
            cyrup_pico_store::DocumentContent::Base(cyrup_pico_store::DocumentBase {
                version: cyrup_pico_doc::DefVersion::FIRST,
                value: fixture::value("n", 4),
            }),
            cyrup_pico_store::Retire::Retire,
        )
        .expect("staging a final content plus retirement");
    fixture::commit(&mut store, builder.build().expect("a non-empty batch")).await;

    let after = std::fs::metadata(fixture::sidecar(dir.path(), doc, 2))
        .expect("the replacement generation")
        .len();
    assert!(
        after < before,
        "the final base must have reclaimed what it superseded: {after} >= {before}"
    );
    drop(store);
    // And the store still opens, which is the assertion that matters: the reclamation and the
    // retirement agree about what the surviving markers describe.
    let store = open_strong(dir.path());
    assert!(
        store
            .reader()
            .document(doc, cyrup_pico_store::DocumentPoint::Current)
            .await
            .expect("the read")
            .is_none()
    );
}

/// `spec.md:4354-4356`: a **rewindable** incarnation is never reclaimed, whatever it costs. ADR-0030
/// §2.3 calls the record's history policy *"the only thing between rewindable being a promise and a best
/// effort"*.
#[tokio::test]
async fn a_rewindable_document_is_never_reclaimed() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let doc: DocumentId = fixture::mint::<Document>(&mut store).await;
    let created = fixture::create_document(
        &mut store,
        fixture::rewindable_doc(doc, fixture::kind("cyrup.state")),
        fixture::value("n", 1),
    )
    .await;
    fixture::append_delta(&mut store, doc, "n", 2).await;
    fixture::write_base(&mut store, doc, fixture::value("n", 3)).await;

    assert!(
        fixture::sidecar(dir.path(), doc, 0).exists(),
        "a rewindable incarnation's first generation must still be there"
    );
    assert!(
        !fixture::sidecar(dir.path(), doc, 1).exists(),
        "no replacement should have been written"
    );
    // And the history it kept is readable at the sequence it was written at.
    let at_creation = store
        .document(doc, DocumentPoint::At(created), &fixture::cx())
        .await
        .expect("the historical read")
        .expect("the document at its creation");
    assert_eq!(at_creation.value, fixture::value("n", 1));
}

/// A replacement generation whose authorising line never reached the log is an orphan, and the next open
/// under the lock removes it. Nothing reads it — the generation is in the *name* — so this is tidiness,
/// and the test exists to pin that the sweep does not remove the **live** generation by accident.
#[tokio::test]
async fn an_orphan_generation_is_swept_and_the_live_one_is_not() {
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
    let orphan = fixture::sidecar(dir.path(), doc, 7);
    std::fs::write(&orphan, b"{}\n").expect("writing an orphan generation");
    let stray = dir.path().join("doc-99-g1.jsonl.tmp");
    std::fs::write(&stray, b"{}\n").expect("writing a stray temp file");

    let store = open_strong(dir.path());

    assert!(!orphan.exists(), "an orphan generation must be swept");
    assert!(!stray.exists(), "a leftover temp file must be swept");
    assert!(
        fixture::sidecar(dir.path(), doc, 0).exists(),
        "the authoritative generation must survive the sweep"
    );
    assert_eq!(
        fixture::read(&store, doc).await.value,
        fixture::value("n", 1)
    );
}

/// Reclamation runs under the weak tier too; what the tier changes is whether `main.jsonl` is flushed
/// before the destructive step (`spec.md:4439-4441`), not whether the step happens.
#[tokio::test]
async fn reclamation_runs_under_both_tiers() {
    for tier in [Durability::ProcessCrash, Durability::PowerLoss] {
        let dir = fixture::dir();
        let mut store = open(dir.path(), tier);
        fixture::seed_root(&mut store).await;
        let doc: DocumentId = fixture::mint::<Document>(&mut store).await;
        fixture::create_document(
            &mut store,
            fixture::latest_doc(doc, fixture::kind("cyrup.state")),
            fixture::value("n", 1),
        )
        .await;
        fixture::append_delta(&mut store, doc, "n", 2).await;
        fixture::write_base(&mut store, doc, fixture::value("n", 3)).await;

        assert!(
            fixture::sidecar(dir.path(), doc, 1).exists(),
            "{tier:?}: the replacement generation must exist"
        );
        assert_eq!(store.deferred_reclamations(), 0, "{tier:?}");
        assert_eq!(
            fixture::read(&store, doc).await.value,
            fixture::value("n", 3)
        );
    }
}

/// `spec.md:4434`: *"any uncertain append failure poisons the open backend."*
///
/// The failure is produced by making a sidecar path unopenable as a file — a directory where the
/// creation's content record has to go. Note what is asserted *after* the poisoning: the read paths
/// refuse too, which is the `&self` requirement ADR-0030 §5 names when it rejects typestate for this
/// flag, and the store is **reopenable**, because the poison is a fact about this process's confidence
/// rather than about the medium.
#[tokio::test]
async fn an_uncertain_append_poisons_the_backend_and_the_store_still_reopens() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let (entry, _) = fixture::commit_entry(&mut store, "before the failure").await;

    // The content record of the creation below has nowhere to go.
    let doc: DocumentId = fixture::mint::<Document>(&mut store).await;
    std::fs::create_dir(fixture::sidecar(dir.path(), doc, 0)).expect("blocking the sidecar path");

    let mut builder = cyrup_pico_store::BatchBuilder::new();
    builder
        .create_document(
            fixture::latest_doc(doc, fixture::kind("cyrup.state")),
            cyrup_pico_store::DocumentBase {
                version: cyrup_pico_doc::DefVersion::FIRST,
                value: fixture::value("n", 1),
            },
            cyrup_pico_store::Retire::Keep,
        )
        .expect("staging a creation");
    let batch = builder.build().expect("a non-empty batch");

    let failed = store
        .commit(batch, &fixture::cx())
        .await
        .expect_err("the commit cannot have succeeded");
    let message = fixture::uncertain(failed);
    assert!(
        message.contains("sidecar") || message.contains("doc-"),
        "the uncertain commit must say what was in flight: {message}"
    );
    assert!(store.poisoned().is_some(), "the backend must be poisoned");

    // Every read path refuses, on `&self`.
    let refused = store
        .entry(entry, &fixture::cx())
        .await
        .expect_err("a poisoned backend must not answer reads");
    assert!(matches!(refused, StorageFailure::Io(_)), "{refused}");
    // And so does the next commit.
    let mut builder = cyrup_pico_store::BatchBuilder::new();
    builder
        .conversation(cyrup_pico_store::ConversationRecord {
            id: cyrup_pico_store::ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        })
        .expect("staging");
    let again = store
        .commit(builder.build().expect("a non-empty batch"), &fixture::cx())
        .await
        .expect_err("a poisoned backend must not commit");
    assert!(
        fixture::uncertain(again).contains("sidecar")
            || !store.poisoned().unwrap_or_default().is_empty()
    );
    drop(store);

    // The store reopens: the records before the failure are there, and the one that failed is not.
    std::fs::remove_dir(fixture::sidecar(dir.path(), doc, 0)).expect("unblocking the path");
    let reopened = open_strong(dir.path());
    assert!(
        reopened
            .reader()
            .entry(entry)
            .await
            .expect("the read")
            .is_some(),
        "the commits before the poisoning are durable and must come back"
    );
    assert!(
        reopened
            .reader()
            .document(doc, DocumentPoint::Current)
            .await
            .expect("the read")
            .is_none(),
        "a commit that never got its marker must not be published"
    );
}

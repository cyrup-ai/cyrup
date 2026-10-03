//! `spec.md:4428-4434`'s recovery rules, one case per rule, plus the open-question-3 case that is the
//! whole point of carrying offsets.

use cyrup_pico_store::{
    Document, DocumentId, DocumentPoint, EntryQuery, PageLimit, ROOT_CONVERSATION_ID, Storage as _,
    StorageFailure,
};

use super::fixture::{self, open_strong};
use crate::{Durability, JsonlOptions, JsonlStore, StoreLock};

/// Open `dir` for writing, returning the failure rather than the store.
fn refuse(dir: &std::path::Path) -> StorageFailure {
    let lock = StoreLock::acquire(dir).expect("the lock");
    match JsonlStore::open_for_write(dir, lock, JsonlOptions::new(Durability::PowerLoss)) {
        Ok(_) => panic!("the open must be refused"),
        Err(e) => e,
    }
}

/// A torn final line is removed, and everything before it is kept (`spec.md:4429`).
#[tokio::test]
async fn a_torn_final_line_is_removed() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let (entry, _) = fixture::commit_entry(&mut store, "kept").await;
    drop(store);
    fixture::append_raw(dir.path(), b"{\"t\":\"marker\",\"seq\":99,\"ma");

    let store = open_strong(dir.path());

    assert!(
        store
            .reader()
            .entry(entry)
            .await
            .expect("the read")
            .is_some(),
        "a torn write must not cost the commits before it"
    );
    let text = std::fs::read_to_string(crate::wire::main_path(dir.path())).expect("main.jsonl");
    assert!(
        !text.contains("\"seq\":99"),
        "the torn line must be gone from the log"
    );
}

/// An unknown line kind fails the open rather than being skipped.
///
/// `cyrup-session`'s reader skips what it does not understand because it has no cross-record invariants
/// to break. This format has nothing but, so a line whose effect is unknown is
/// `spec.md:4433`'s *"missing required confirmed data"*.
#[tokio::test]
async fn an_unknown_line_kind_fails_the_open() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    drop(store);
    let mut lines = fixture::main_lines(dir.path());
    lines.insert(0, "{\"t\":\"compaction\",\"seq\":1}".to_owned());
    fixture::write_main_lines(dir.path(), &lines);

    assert!(
        matches!(refuse(dir.path()), StorageFailure::Corrupt(_)),
        "an unreadable line must fail the open"
    );
}

/// A line that is not the final one and cannot be decoded is corruption: only the last line can be torn.
#[tokio::test]
async fn an_undecodable_interior_line_fails_the_open() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    fixture::commit_entry(&mut store, "after").await;
    drop(store);
    let mut lines = fixture::main_lines(dir.path());
    lines.insert(1, "{\"t\":\"entr".to_owned());
    fixture::write_main_lines(dir.path(), &lines);

    assert!(matches!(refuse(dir.path()), StorageFailure::Corrupt(_)));
}

/// Markers strictly increase, at the recovery boundary as well as at the write boundary
/// (`spec.md:99-100`; ADR-0030 §2.2 classifies it **checked** for exactly this reason).
#[tokio::test]
async fn a_marker_that_does_not_increase_fails_the_open() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    fixture::commit_entry(&mut store, "one").await;
    drop(store);
    // A marker for a sequence the log has already published. Written by hand with no records of its
    // own, so the failure is the sequence and not the record count.
    let mut lines = fixture::main_lines(dir.path());
    lines.push("{\"t\":\"marker\",\"seq\":1,\"main\":0}".to_owned());
    fixture::write_main_lines(dir.path(), &lines);

    match refuse(dir.path()) {
        StorageFailure::Corrupt(c) => assert!(
            c.to_string().contains("does not follow"),
            "the failure must name the sequence that did not increase: {c}"
        ),
        other => panic!("expected corruption, got {other}"),
    }
}

/// A marker that confirms more main records than are present fails the open.
///
/// This is the residue of *"ordinary publication does not explicitly flush `main.jsonl`"*: the count is
/// the only thing that can tell a log whose records vanished under a surviving marker from a log that
/// simply committed less.
#[tokio::test]
async fn a_marker_whose_records_are_not_all_present_fails_the_open() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    fixture::commit_entry(&mut store, "one").await;
    drop(store);
    let mut lines = fixture::main_lines(dir.path());
    let entry = lines
        .iter()
        .rposition(|l| l.contains("\"t\":\"entry\""))
        .expect("an entry line");
    lines.remove(entry);
    fixture::write_main_lines(dir.path(), &lines);

    match refuse(dir.path()) {
        StorageFailure::Corrupt(c) => assert!(
            c.to_string().contains("present"),
            "the failure must say how many records the marker named: {c}"
        ),
        other => panic!("expected corruption, got {other}"),
    }
}

/// One number cannot be owned by two kinds of record (`spec.md:4274-4275`). ADR-0030 F6 §A names this
/// as the residue the type system cannot remove, *"because the number carries no tag"*.
#[tokio::test]
async fn an_id_filed_under_two_kinds_fails_the_open() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let (entry, _) = fixture::commit_entry(&mut store, "one").await;
    drop(store);

    // The same number, filed as a task. Only a damaged file can hold this: in process there is no
    // construction path from an entry id to a task id.
    let mut lines = fixture::main_lines(dir.path());
    let n = cyrup_pico_store::RawId::from(entry).get().get();
    lines.push(format!(
        "{{\"t\":\"task\",\"seq\":99,\"r\":{{\"id\":{n},\"conversation_id\":1,\"kind\":\"cyrup.generation\",\"version\":1,\"input\":null,\"background\":false,\"abort_requested\":false,\"state\":{{\"status\":\"terminal\",\"outcome\":{{\"status\":\"aborted\"}}}}}}}}"
    ));
    lines.push("{\"t\":\"marker\",\"seq\":99,\"main\":1}".to_owned());
    fixture::write_main_lines(dir.path(), &lines);

    match refuse(dir.path()) {
        StorageFailure::Corrupt(c) => assert!(
            c.to_string().contains("index"),
            "the failure must name the index the number was filed under: {c}"
        ),
        other => panic!("expected corruption, got {other}"),
    }
}

/// **ADR-0030 §14 open question 3, as a test.** The open pass validates every marker-carried offset
/// against the sidecar's length and reads **no payload byte** — so a sidecar whose payload is
/// unreadable still opens, and only a *read* of that document fails.
///
/// If a future change made the open pass parse sidecars, this test would fail — which is what makes it
/// the pin for the open-time saving the offsets exist to buy.
#[tokio::test]
async fn the_open_pass_validates_offsets_without_reading_a_payload() {
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

    // Same length, unreadable content: every byte replaced, the file's size untouched.
    let path = fixture::sidecar(dir.path(), doc, 0);
    let len = std::fs::metadata(&path).expect("the sidecar").len();
    let mut rubbish = vec![b'x'; usize::try_from(len).expect("a small sidecar")];
    if let Some(last) = rubbish.last_mut() {
        *last = b'\n';
    }
    std::fs::write(&path, &rubbish).expect("overwriting the payload");

    let store = open_strong(dir.path());
    assert!(
        store.reader().last_commit().is_some(),
        "the open must succeed: offsets are validated against the file's LENGTH, not its content"
    );
    let failed = store
        .document(doc, DocumentPoint::Current, &fixture::cx())
        .await
        .expect_err("reading the document must fail");
    assert!(
        matches!(failed, StorageFailure::Corrupt(_)),
        "expected corruption from the read, got {failed}"
    );
}

/// A standalone line inside an unconfirmed commit cannot happen honestly, so it is corruption: it would
/// mean the log interleaved a reservation with a commit's records, which `&mut self` makes impossible.
#[tokio::test]
async fn a_reservation_inside_an_unconfirmed_commit_fails_the_open() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    fixture::commit_entry(&mut store, "one").await;
    drop(store);
    let mut lines = fixture::main_lines(dir.path());
    let entry = lines
        .iter()
        .rposition(|l| l.contains("\"t\":\"entry\""))
        .expect("an entry line");
    lines.insert(
        entry.saturating_add(1),
        "{\"t\":\"mint\",\"high\":9000}".to_owned(),
    );
    fixture::write_main_lines(dir.path(), &lines);

    assert!(matches!(refuse(dir.path()), StorageFailure::Corrupt(_)));
}

/// Recovery leaves a store that can be committed to, scanned and reopened again: the point of removing
/// the unconfirmed bytes is that the next append starts where the next marker can describe it.
#[tokio::test]
async fn a_recovered_store_takes_new_commits() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    fixture::commit_entry(&mut store, "before").await;
    drop(store);
    fixture::append_raw(dir.path(), b"{\"t\":\"entry\",\"seq\":77,\"r\":{\"id");

    let mut store = open_strong(dir.path());
    fixture::commit_entry(&mut store, "after").await;
    drop(store);

    let store = open_strong(dir.path());
    let page = store
        .reader()
        .scan_entries(
            &EntryQuery::all(ROOT_CONVERSATION_ID),
            PageLimit::parse(50).expect("a limit"),
            None,
        )
        .await
        .expect("the scan");
    assert_eq!(
        page.items.len(),
        2,
        "the commit after recovery must survive its own reopen"
    );
}

/// A rejection writes **no byte**, which is what makes [`CommitError::Rejected`]'s claim — *"the batch
/// was rejected before any durable effect"* (`spec.md:4310-4311`) — true of this backend rather than
/// merely asserted by it.
///
/// The mechanism is that validation is pure ([`crate::state::Committed::validate`]) and runs before the
/// first append, so there is no partially-applied state to unwind and no window in which a rejected
/// batch could leave a trace. Asserted against the medium, because that is where a future change would
/// break it.
///
/// [`CommitError::Rejected`]: cyrup_pico_store::CommitError::Rejected
#[tokio::test]
async fn a_rejected_batch_writes_no_byte() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let before = std::fs::metadata(crate::wire::main_path(dir.path()))
        .expect("main.jsonl")
        .len();

    // `spec.md:4274`: conversation creation is immutable, so a second write of the root is rejected.
    let mut builder = cyrup_pico_store::BatchBuilder::new();
    builder
        .conversation(cyrup_pico_store::ConversationRecord {
            id: ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        })
        .expect("staging");
    let failed = store
        .commit(builder.build().expect("a non-empty batch"), &fixture::cx())
        .await
        .expect_err("a second root conversation must be rejected");

    let reason = fixture::rejected(failed);
    assert!(
        reason.contains("immutable"),
        "the rejection must name the rule it enforced: {reason}"
    );
    let after = std::fs::metadata(crate::wire::main_path(dir.path()))
        .expect("main.jsonl")
        .len();
    assert_eq!(
        before, after,
        "a rejected batch must not reach the medium at all"
    );
    assert!(
        store.poisoned().is_none(),
        "a rejection is not a reason to poison: the store's state is not in doubt"
    );
}

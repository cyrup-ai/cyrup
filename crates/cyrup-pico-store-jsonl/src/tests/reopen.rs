//! The reopen suite: *"exactly the set of commits whose markers survived"*, and the id high-water mark
//! across a crash (PICO5-PLAN S7).

use cyrup_pico_store::{
    Document, DocumentId, DocumentPoint, Entry, EntryQuery, PageLimit, ROOT_CONVERSATION_ID, RawId,
    Storage as _, StorageExt as _,
};

use super::fixture::{self, cx, open_strong};
use crate::JsonlStore;

/// The suite's central case. Three commits land; a fourth is interrupted after its record line and
/// before its marker; a fifth line is torn mid-write. The reopened store holds exactly the three.
#[tokio::test]
async fn a_reopen_holds_exactly_the_commits_whose_markers_survived() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let (first, _) = fixture::commit_entry(&mut store, "one").await;
    let (second, last_seq) = fixture::commit_entry(&mut store, "two").await;
    drop(store);

    // A commit whose marker never arrived: the record line is there, nothing confirms it.
    let mut lines = fixture::main_lines(dir.path());
    let template = lines
        .iter()
        .rev()
        .find(|l| l.contains("\"t\":\"entry\""))
        .expect("an entry line")
        .clone();
    lines.push(template.replace("\"id\":3", "\"id\":4096"));
    fixture::write_main_lines(dir.path(), &lines);
    // And a torn write on top of it.
    fixture::append_raw(dir.path(), b"{\"t\":\"entry\",\"seq\":9,\"r\":{\"id\":40");

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

    let ids: Vec<RawId> = page.items.iter().map(|e| RawId::from(e.id)).collect();
    assert_eq!(
        ids,
        vec![RawId::from(second), RawId::from(first)],
        "a reopen must hold exactly the confirmed commits, newest first"
    );
    assert_eq!(
        store.reader().last_commit(),
        Some(last_seq),
        "the newest surviving marker is the store's last commit"
    );

    // And the bytes no marker confirmed are gone, so the next append starts where the next marker can
    // describe it.
    let text = std::fs::read_to_string(crate::wire::main_path(dir.path())).expect("main.jsonl");
    assert!(
        !text.contains("4096"),
        "the unconfirmed record line must be removed, not left for the next open to re-ignore"
    );
    assert!(
        text.ends_with('\n'),
        "the log must end on a line boundary after recovery"
    );
}

/// A new commit after a reopen continues the sequence, which is `spec.md:99-100` across the process
/// boundary — the half ADR-0030 §2.2 calls **checked** because it cannot be lifted.
#[tokio::test]
async fn the_sequence_keeps_increasing_across_reopen() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let (_, before) = fixture::commit_entry(&mut store, "before").await;
    drop(store);

    let mut store = open_strong(dir.path());
    let (_, after) = fixture::commit_entry(&mut store, "after").await;

    assert!(
        after > before,
        "sequence {after} after reopen does not follow {before}"
    );
}

/// A crash **between a mint and the commit that would use it** must not reissue the id
/// (`spec.md:4282`). This is the case a backend that kept its high-water mark only in memory would fail.
#[tokio::test]
async fn a_crash_between_mint_and_commit_does_not_reissue_an_id() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    let minted: Vec<u64> = {
        let mut ids = Vec::new();
        for _ in 0..3_u32 {
            ids.push(
                RawId::from(store.mint::<Entry>(&cx()).await.expect("a minted id"))
                    .get()
                    .get(),
            );
        }
        ids
    };
    // No commit at all: the process simply ends here.
    drop(store);

    let mut store = open_strong(dir.path());
    let again = RawId::from(store.mint::<Entry>(&cx()).await.expect("a minted id"))
        .get()
        .get();

    let highest = minted.iter().copied().max().expect("three ids");
    assert!(
        again > highest,
        "id {again} was reissued after a crash: the earlier open had already handed out {minted:?}"
    );
}

/// A document's value survives a reopen with its base and its whole delta tail, which is the read path
/// the marker-carried offsets exist to make cheap.
#[tokio::test]
async fn a_document_materialises_the_same_value_after_reopen() {
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
    fixture::append_delta(&mut store, doc, "n", 2).await;
    let third = fixture::append_delta(&mut store, doc, "n", 3).await;
    let before = fixture::read(&store, doc).await;
    drop(store);

    let store = open_strong(dir.path());
    let after = fixture::read(&store, doc).await;

    assert_eq!(before.value, after.value);
    assert_eq!(
        after.deltas_since_base, 2,
        "both deltas must be replayed after the base"
    );
    // And a historical read inside the lifetime still answers from the records before that point.
    let earlier = store
        .document(doc, DocumentPoint::At(third), &cx())
        .await
        .expect("the historical read")
        .expect("the document at that point");
    assert_eq!(earlier.value, after.value);
}

/// A cursor issued before a reopen resumes in the reopened store, because the store identity is durable
/// rather than per-process (ADR-0030 F6 §B).
#[tokio::test]
async fn a_cursor_survives_a_reopen() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    for n in 0..3_u32 {
        fixture::commit_entry(&mut store, &format!("entry {n}")).await;
    }
    let first_page = store
        .reader()
        .scan_entries(
            &EntryQuery::all(ROOT_CONVERSATION_ID),
            PageLimit::parse(1).expect("a limit"),
            None,
        )
        .await
        .expect("the scan");
    let cursor = first_page
        .next
        .expect("a cursor: three entries, one per page");
    drop(store);

    let store = open_strong(dir.path());
    let second_page = store
        .reader()
        .scan_entries(
            &EntryQuery::all(ROOT_CONVERSATION_ID),
            PageLimit::parse(1).expect("a limit"),
            Some(cursor),
        )
        .await
        .expect("resuming the scan after a reopen");

    assert_eq!(second_page.items.len(), 1);
    assert_ne!(
        second_page.items.first().map(|e| e.id),
        first_page.items.first().map(|e| e.id),
        "the resumed page must continue the scan rather than repeat it"
    );
}

/// A cursor from a *different* store is refused rather than resumed at a position meaning something
/// else — the one `StoreId` comparison ADR-0030 F6 §B calls *"easy to dismiss as ceremony"*.
#[tokio::test]
async fn a_cursor_from_another_store_is_refused() {
    let one = fixture::dir();
    let two = fixture::dir();
    let mut first = open_strong(one.path());
    fixture::seed_root(&mut first).await;
    fixture::commit_entry(&mut first, "a").await;
    fixture::commit_entry(&mut first, "b").await;
    let page = first
        .reader()
        .scan_entries(
            &EntryQuery::all(ROOT_CONVERSATION_ID),
            PageLimit::parse(1).expect("a limit"),
            None,
        )
        .await
        .expect("the scan");
    let foreign = page.next.expect("a cursor");

    let mut second = open_strong(two.path());
    fixture::seed_root(&mut second).await;
    let refused = second
        .reader()
        .scan_entries(
            &EntryQuery::all(ROOT_CONVERSATION_ID),
            PageLimit::parse(1).expect("a limit"),
            Some(foreign),
        )
        .await
        .expect_err("a cursor from another store must be refused");

    match refused {
        cyrup_pico_store::StorageFailure::Io(e) => {
            assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        }
        other => panic!("expected an invalid-argument failure, got {other}"),
    }
}

/// An empty directory opens as an empty store, and nothing about that is a special case.
#[tokio::test]
async fn a_fresh_store_opens_empty_and_reopens_empty() {
    let dir = fixture::dir();
    let store = open_strong(dir.path());
    assert_eq!(store.reader().last_commit(), None);
    drop(store);
    let again = open_strong(dir.path());
    assert_eq!(again.reader().last_commit(), None);
    assert!(
        JsonlStore::open_read_only(dir.path()).is_ok(),
        "a store with an identity record and no commits is still a store"
    );
}

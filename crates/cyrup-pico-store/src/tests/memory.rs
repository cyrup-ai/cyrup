//! [`MemoryStore`] tests: §11.1's reference semantics, and the cross-batch checks the keyed batch does
//! not lift.
//!
//! The *shared* semantics — read-your-commit, sequence increase, mint monotonicity, detachment, the two
//! cross-batch rejections — are the conformance suite's, not this module's, and are driven against this
//! backend from `tests/conformance.rs`. What is here is what is specific to this backend or needs a
//! structure a case cannot reach: ancestry visibility, head markers, paging and reclamation.

use core::num::NonZeroU64;
use std::sync::Arc;

use cyrup_pico_doc::{DefVersion, DocMap, DocRoot, DocValue, Op, OpBatch, Path};

use crate::{
    Batch, BatchBuilder, CommitError, ConversationId, ConversationParent, ConversationRecord,
    ConversationSemantics, Cx, DocumentBase, DocumentContent, DocumentCreate, DocumentId,
    DocumentPoint, DocumentScope, EntryQuery, EntryRecord, Id, Kind, LatestFork, MemoryStore,
    PageLimit, ROOT_CONVERSATION_ID, RejectedReason, Retire, RewindableFork, Seq, Storage,
    StorageExt, StorageFailure,
};

fn id<K: crate::IdKind>(n: u64) -> Id<K> {
    Id::new(NonZeroU64::new(n).expect("test ids are nonzero"))
}

fn kind(s: &str) -> Kind {
    Kind::parse(s).expect("test kinds parse")
}

fn pages(n: u32) -> PageLimit {
    PageLimit::parse(n).expect("test page limits are in range")
}

fn value(key: &str, n: i64) -> DocRoot {
    let mut map = DocMap::new();
    map.insert(Arc::from(key), DocValue::integer(n));
    DocRoot::from_map(map)
}

fn conversation(id: ConversationId, parent: Option<ConversationParent>) -> ConversationRecord {
    ConversationRecord {
        id,
        parent,
        owner: None,
    }
}

fn entry(n: u64, conversation_id: ConversationId, head: Option<crate::EntryId>) -> EntryRecord {
    EntryRecord {
        id: id(n),
        conversation_id,
        kind: kind("cyrup.message"),
        model: None,
        data: None,
        head,
        edits: None,
        by_task_id: None,
    }
}

fn rewindable(document: DocumentId, conversation_id: ConversationId) -> DocumentCreate {
    DocumentCreate {
        id: document,
        kind: kind("cyrup.state"),
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id,
            semantics: ConversationSemantics::Rewindable {
                fork: RewindableFork::AsOf,
            },
        },
    }
}

fn latest(document: DocumentId, conversation_id: ConversationId) -> DocumentCreate {
    DocumentCreate {
        id: document,
        kind: kind("cyrup.state"),
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id,
            semantics: ConversationSemantics::Latest {
                fork: LatestFork::Current,
            },
        },
    }
}

/// Build a batch from one staging closure, panicking in the test on a staging collision.
fn batch(stage: impl FnOnce(&mut BatchBuilder)) -> Batch {
    let mut builder = BatchBuilder::new();
    stage(&mut builder);
    builder.build().expect("the test batch is non-empty")
}

async fn seed(store: &mut MemoryStore, cx: &Cx) -> Seq {
    store
        .commit(
            batch(|b| {
                b.conversation(conversation(ROOT_CONVERSATION_ID, None))
                    .expect("stages");
            }),
            cx,
        )
        .await
        .expect("the seed commit")
}

#[tokio::test]
async fn two_stores_have_different_identities_so_a_cursor_cannot_cross() {
    let a = MemoryStore::new();
    let b = MemoryStore::new();
    assert_ne!(a.store_id(), b.store_id());
}

#[tokio::test]
async fn a_fresh_store_has_no_commits() {
    let store = MemoryStore::new();
    assert_eq!(store.last_commit(), None);
}

/// `spec.md:263`: *"Fork traversal is child entries followed by parent entries through each `parent.at`
/// cap."* The cap is applied **inside** the backend, which is why the method exists at all.
#[tokio::test]
async fn an_ancestry_cap_hides_the_parents_later_entries() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    let child: ConversationId = id(2);

    store
        .commit(
            batch(|b| {
                b.entry(entry(10, ROOT_CONVERSATION_ID, None))
                    .expect("stages");
                b.entry(entry(11, ROOT_CONVERSATION_ID, None))
                    .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("parent entries");
    store
        .commit(
            batch(|b| {
                b.conversation(conversation(
                    child,
                    Some(ConversationParent {
                        conversation_id: ROOT_CONVERSATION_ID,
                        at: id(10),
                    }),
                ))
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("the fork");
    store
        .commit(
            batch(|b| {
                // A parent entry written after the fork point: invisible to the child.
                b.entry(entry(12, ROOT_CONVERSATION_ID, None))
                    .expect("stages");
                b.entry(entry(13, child, None)).expect("stages");
            }),
            &cx,
        )
        .await
        .expect("later entries");

    let page = store
        .scan_entries(&EntryQuery::all(child), pages(10), None, &cx)
        .await
        .expect("the scan");
    let seen: Vec<u64> = page
        .items
        .iter()
        .map(|e| crate::RawId::from(e.id).get().get())
        .collect();
    // Newest-first, capped at the fork point: 13 (child), 10 (parent) — never 11 or 12.
    assert_eq!(seen, vec![13, 10]);

    // And the same cap applies to the exact lookup.
    assert!(
        store
            .entry_in(child, id(11), &cx)
            .await
            .expect("the lookup")
            .is_none(),
        "an entry above the fork cap must not be visible through the child"
    );
    assert!(
        store
            .entry_in(child, id(10), &cx)
            .await
            .expect("the lookup")
            .is_some()
    );
    // While the global lookup still finds it: `entry(id)` and `entry(conversationId, id)` answer
    // different questions (`spec.md:4347-4348`).
    assert!(
        store
            .entry(id(11), &cx)
            .await
            .expect("the lookup")
            .is_some()
    );
}

/// `spec.md:4331-4334`: the newest visible entry carrying `head` at or below the cutoff, and the
/// returned entry *is* the marker.
#[tokio::test]
async fn the_head_marker_is_the_newest_visible_one_at_or_before_the_cutoff() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    store
        .commit(
            batch(|b| {
                b.entry(entry(10, ROOT_CONVERSATION_ID, Some(id(10))))
                    .expect("stages");
                b.entry(entry(11, ROOT_CONVERSATION_ID, None))
                    .expect("stages");
                b.entry(entry(12, ROOT_CONVERSATION_ID, Some(id(12))))
                    .expect("stages");
                b.entry(entry(13, ROOT_CONVERSATION_ID, None))
                    .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("entries");

    let newest = store
        .find_latest_head_marker(ROOT_CONVERSATION_ID, None, &cx)
        .await
        .expect("the lookup")
        .expect("a marker");
    assert_eq!(newest.entry.id, id(12));
    assert_eq!(newest.head, id(12));

    let capped = store
        .find_latest_head_marker(ROOT_CONVERSATION_ID, Some(id(11)), &cx)
        .await
        .expect("the lookup")
        .expect("a marker");
    assert_eq!(capped.entry.id, id(10));

    assert!(
        store
            .find_latest_head_marker(ROOT_CONVERSATION_ID, Some(id(9)), &cx)
            .await
            .expect("the lookup")
            .is_none()
    );
}

#[tokio::test]
async fn a_scan_pages_and_the_cursor_resumes_where_it_stopped() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    store
        .commit(
            batch(|b| {
                for n in 10..=14 {
                    b.entry(entry(n, ROOT_CONVERSATION_ID, None))
                        .expect("stages");
                }
            }),
            &cx,
        )
        .await
        .expect("entries");

    let first = store
        .scan_entries(&EntryQuery::all(ROOT_CONVERSATION_ID), pages(2), None, &cx)
        .await
        .expect("the first page");
    assert_eq!(first.items.len(), 2);
    let cursor = first.next.clone().expect("more to come");
    let second = store
        .scan_entries(
            &EntryQuery::all(ROOT_CONVERSATION_ID),
            pages(2),
            Some(cursor),
            &cx,
        )
        .await
        .expect("the second page");
    let ids: Vec<u64> = first
        .items
        .iter()
        .chain(second.items.iter())
        .map(|e| crate::RawId::from(e.id).get().get())
        .collect();
    assert_eq!(ids, vec![14, 13, 12, 11]);
}

/// A cursor from another store does not resume this one's scan at a position meaning something else.
#[tokio::test]
async fn a_cursor_from_another_store_is_refused() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    let mut other = MemoryStore::new();
    seed(&mut store, &cx).await;
    seed(&mut other, &cx).await;
    other
        .commit(
            batch(|b| {
                b.entry(entry(10, ROOT_CONVERSATION_ID, None))
                    .expect("stages");
                b.entry(entry(11, ROOT_CONVERSATION_ID, None))
                    .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("entries");
    let foreign = other
        .scan_entries(&EntryQuery::all(ROOT_CONVERSATION_ID), pages(1), None, &cx)
        .await
        .expect("the scan")
        .next
        .expect("more to come");

    let refused = store
        .scan_entries(
            &EntryQuery::all(ROOT_CONVERSATION_ID),
            pages(1),
            Some(foreign),
            &cx,
        )
        .await
        .expect_err("a foreign cursor must not be honoured");
    assert!(matches!(refused, StorageFailure::Io(_)), "{refused}");
}

/// ADR-0030 F6 §C: a delta *"cannot claim a version it did not read"*. The witness makes the honest
/// case easy; the residue is a **stale** witness, which is cross-batch and is this rejection.
#[tokio::test]
async fn a_stale_version_witness_is_rejected() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    let document: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    store
        .commit(
            batch(|b| {
                b.create_document(
                    rewindable(document, ROOT_CONVERSATION_ID),
                    DocumentBase {
                        version: DefVersion::FIRST,
                        value: value("n", 1),
                    },
                    Retire::Keep,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("the creation");

    let read = store
        .document(document, DocumentPoint::Current, &cx)
        .await
        .expect("the read")
        .expect("the document");
    let stale = read.version;

    // A version transition: a complete base at version 2.
    let two = DefVersion::new(core::num::NonZeroU32::new(2).expect("2 is nonzero"));
    store
        .commit(
            batch(|b| {
                b.change_document(
                    document,
                    DocumentContent::Base(DocumentBase {
                        version: two,
                        value: value("n", 2),
                    }),
                    Retire::Keep,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("the transition");

    // The witness read before the transition is now stale.
    let ops = OpBatch::new(vec![Op::Set {
        path: Path::keys(["n"]).expect("a path"),
        value: DocValue::integer(3),
    }]);
    let refused = store
        .commit(
            batch(|b| {
                b.change_document(
                    document,
                    DocumentContent::Delta {
                        continues: stale,
                        ops,
                    },
                    Retire::Keep,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect_err("a delta across a version boundary must not commit");
    assert!(matches!(
        refused,
        CommitError::Rejected(RejectedReason::VersionTransitionRequiresBase { .. })
    ));
}

#[tokio::test]
async fn a_write_to_an_unknown_or_retired_incarnation_is_rejected() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    let document: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");

    let unknown = store
        .commit(
            batch(|b| b.retire_document(document).map(|_| ()).expect("stages")),
            &cx,
        )
        .await
        .expect_err("retiring an unknown incarnation");
    assert!(matches!(
        unknown,
        CommitError::Rejected(RejectedReason::UnknownDocument(_))
    ));

    store
        .commit(
            batch(|b| {
                b.create_document(
                    latest(document, ROOT_CONVERSATION_ID),
                    DocumentBase {
                        version: DefVersion::FIRST,
                        value: value("n", 1),
                    },
                    Retire::Retire,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("create-and-retire");

    let retired = store
        .commit(
            batch(|b| b.retire_document(document).map(|_| ()).expect("stages")),
            &cx,
        )
        .await
        .expect_err("retiring a retired incarnation");
    assert!(matches!(
        retired,
        CommitError::Rejected(RejectedReason::DocumentRetired { .. })
    ));
}

/// §11.1: *"It preserves rewindable records and reclaims latest records only after a committed base or
/// retirement."* A rewindable incarnation's history survives, so a numeric read inside its lifetime
/// reconstructs the selected value.
#[tokio::test]
async fn a_rewindable_incarnation_reconstructs_a_historical_value() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    let document: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    let created = store
        .commit(
            batch(|b| {
                b.create_document(
                    rewindable(document, ROOT_CONVERSATION_ID),
                    DocumentBase {
                        version: DefVersion::FIRST,
                        value: value("n", 1),
                    },
                    Retire::Keep,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("the creation");
    let witness = store
        .document(document, DocumentPoint::Current, &cx)
        .await
        .expect("the read")
        .expect("the document")
        .version;

    store
        .commit(
            batch(|b| {
                b.change_document(
                    document,
                    DocumentContent::Delta {
                        continues: witness,
                        ops: OpBatch::new(vec![Op::Set {
                            path: Path::keys(["n"]).expect("a path"),
                            value: DocValue::integer(2),
                        }]),
                    },
                    Retire::Keep,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("the change");

    let then = store
        .document(document, DocumentPoint::At(created), &cx)
        .await
        .expect("the historical read")
        .expect("the document");
    assert_eq!(then.value, value("n", 1));
    assert_eq!(then.deltas_since_base, 0);

    let now = store
        .document(document, DocumentPoint::Current, &cx)
        .await
        .expect("the current read")
        .expect("the document");
    assert_eq!(now.value, value("n", 2));
    assert_eq!(now.deltas_since_base, 1);
}

/// `spec.md:4313-4318`, and the order-independence ADR-0030 F3 cares about: a copy reads **committed
/// pre-batch** source state, so the child does not depend on how the batch was assembled.
#[tokio::test]
async fn a_copy_persists_an_independent_base_and_later_source_changes_cannot_reach_it() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    let source: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    let child_conversation: ConversationId =
        store.mint::<crate::Conversation>(&cx).await.expect("mints");

    store
        .commit(
            batch(|b| {
                b.create_document(
                    rewindable(source, ROOT_CONVERSATION_ID),
                    DocumentBase {
                        version: DefVersion::FIRST,
                        value: value("n", 1),
                    },
                    Retire::Keep,
                )
                .expect("stages");
                b.conversation(conversation(
                    child_conversation,
                    Some(ConversationParent {
                        conversation_id: ROOT_CONVERSATION_ID,
                        at: id(10),
                    }),
                ))
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("the source");

    let child: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    store
        .commit(
            batch(|b| {
                b.copy_document(
                    rewindable(child, child_conversation),
                    crate::CopySource {
                        id: source,
                        at: DocumentPoint::Current,
                    },
                    Retire::Keep,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("the copy");

    let copied = store
        .document(child, DocumentPoint::Current, &cx)
        .await
        .expect("the read")
        .expect("the child");
    assert_eq!(copied.value, value("n", 1));

    // Change the source, then retire it. Neither can reach the child.
    let witness = store
        .document(source, DocumentPoint::Current, &cx)
        .await
        .expect("the read")
        .expect("the source")
        .version;
    store
        .commit(
            batch(|b| {
                b.change_document(
                    source,
                    DocumentContent::Delta {
                        continues: witness,
                        ops: OpBatch::new(vec![Op::Set {
                            path: Path::keys(["n"]).expect("a path"),
                            value: DocValue::integer(99),
                        }]),
                    },
                    Retire::Retire,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("the source change");

    let after = store
        .document(child, DocumentPoint::Current, &cx)
        .await
        .expect("the read")
        .expect("the child");
    assert_eq!(after.value, value("n", 1), "the copy is not independent");
}

/// `spec.md:4316`: *"A batch may not create, change, or retire a selected source."* Without the
/// rejection the child's value would depend on command order — which, under the keyed batch, is not even
/// two different batches, so the rejection is the whole mechanism.
#[tokio::test]
async fn a_batch_that_also_writes_its_copy_source_is_rejected() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    let source: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    store
        .commit(
            batch(|b| {
                b.create_document(
                    rewindable(source, ROOT_CONVERSATION_ID),
                    DocumentBase {
                        version: DefVersion::FIRST,
                        value: value("n", 1),
                    },
                    Retire::Keep,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("the source");

    let child: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    let refused = store
        .commit(
            batch(|b| {
                b.copy_document(
                    rewindable(child, id(2)),
                    crate::CopySource {
                        id: source,
                        at: DocumentPoint::Current,
                    },
                    Retire::Keep,
                )
                .expect("stages");
                b.retire_document(source).expect("stages");
            }),
            &cx,
        )
        .await
        .expect_err("a batch that writes its own copy source");
    assert!(matches!(
        refused,
        CommitError::Rejected(RejectedReason::CopySourceInBatch { .. })
    ));
}

/// A rejection must leave nothing behind, which is the only thing [`CommitError::Rejected`] claims.
#[tokio::test]
async fn a_rejected_batch_allocates_no_sequence() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    let seeded = seed(&mut store, &cx).await;
    let document: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    let refused = store
        .commit(
            batch(|b| b.retire_document(document).map(|_| ()).expect("stages")),
            &cx,
        )
        .await
        .expect_err("retiring an unknown incarnation");
    assert!(refused.is_rejected());
    assert_eq!(
        store.last_commit(),
        Some(seeded),
        "a rejected batch must not move the commit sequence"
    );
}

#[tokio::test]
async fn a_scope_is_enumerated_without_touching_another() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    let in_conversation: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    let in_session: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    store
        .commit(
            batch(|b| {
                b.create_document(
                    latest(in_conversation, ROOT_CONVERSATION_ID),
                    DocumentBase {
                        version: DefVersion::FIRST,
                        value: value("n", 1),
                    },
                    Retire::Keep,
                )
                .expect("stages");
                b.create_document(
                    DocumentCreate {
                        id: in_session,
                        kind: kind("cyrup.state"),
                        key: None,
                        scope: DocumentScope::Session,
                    },
                    DocumentBase {
                        version: DefVersion::FIRST,
                        value: value("n", 2),
                    },
                    Retire::Keep,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("two scopes");

    let page = store
        .scan_documents(
            &crate::DocumentQuery {
                scope: crate::ScopeRef::Session,
                at: DocumentPoint::Current,
                kind: None,
            },
            pages(10),
            None,
            &cx,
        )
        .await
        .expect("the scan");
    assert_eq!(
        page.items.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![in_session]
    );
}

/// `spec.md:4362-4366` is about the **address**, not the map key: two creations at one logical address
/// in one batch have two different incarnation ids, so the keyed batch cannot see them and the backend
/// `spec.md:1245-1246` and `spec.md:1113` together: a creation this batch also **retires** has an empty
/// half-open lifetime, so it never occupies the address, and a second creation at that address in
/// the same batch is legal.
///
/// This is the in-batch twin of the exemption [`MemoryStore::check_address_free`] already made for a
/// committed occupant the batch retires, and it is what lets *"a later `tx.doc()` at that address in
/// the same transaction create a new incarnation with a new draft and ID"* reach storage at all.
#[tokio::test]
async fn a_creation_retired_in_the_same_batch_does_not_occupy_its_address() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    let first: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    let second: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");

    let seq = store
        .commit(
            batch(|b| {
                b.create_document(
                    latest(first, ROOT_CONVERSATION_ID),
                    DocumentBase {
                        version: DefVersion::FIRST,
                        value: value("n", 1),
                    },
                    Retire::Retire,
                )
                .expect("stages");
                b.create_document(
                    latest(second, ROOT_CONVERSATION_ID),
                    DocumentBase {
                        version: DefVersion::FIRST,
                        value: value("n", 2),
                    },
                    Retire::Keep,
                )
                .expect("stages");
            }),
            &cx,
        )
        .await
        .expect("the retired creation does not occupy the address");

    let address = latest(first, ROOT_CONVERSATION_ID).address();
    let live = store
        .find_document(&address, crate::DocumentPoint::Current, &cx)
        .await
        .expect("a read")
        .expect("one live incarnation");
    assert_eq!(live.id, second, "the surviving incarnation is the second");
    // And the retired incarnation is `history: "latest"`, so a numeric lookup of it **rejects**
    // rather than guessing from whatever records survived — `spec.md:4357-4358`.
    let numeric = store
        .document(first, crate::DocumentPoint::At(seq), &cx)
        .await;
    assert!(
        matches!(
            numeric,
            Err(crate::StorageFailure::HistoryNotRetained { document, .. }) if document == first
        ),
        "a numeric lookup of a current-only incarnation rejects rather than guessing"
    );
}

/// must. The cross-batch half has its own conformance case; this is the in-batch half.
#[tokio::test]
async fn two_creations_at_one_address_in_one_batch_are_rejected() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();
    seed(&mut store, &cx).await;
    let first: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");
    let second: DocumentId = store.mint::<crate::Document>(&cx).await.expect("mints");

    let refused = store
        .commit(
            batch(|b| {
                for id in [first, second] {
                    b.create_document(
                        latest(id, ROOT_CONVERSATION_ID),
                        DocumentBase {
                            version: DefVersion::FIRST,
                            value: value("n", 1),
                        },
                        Retire::Keep,
                    )
                    .expect("stages");
                }
            }),
            &cx,
        )
        .await
        .expect_err("two incarnations at one address");
    match refused {
        CommitError::Rejected(RejectedReason::AddressOccupied { occupant, .. }) => {
            assert_eq!(occupant, first);
        }
        other => panic!("wrong rejection: {other}"),
    }
}

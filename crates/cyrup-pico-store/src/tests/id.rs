//! `Id<K>`, `IdKindTag` and `RawId`.

use core::num::NonZeroU64;

use super::REJECTED_SHAPES;
use crate::{
    ConversationId, DocumentId, EntryId, IdKind, IdKindTag, ROOT_CONVERSATION_ID, RawId, Seq,
    SubmissionId, TaskId,
};

fn entry(n: u64) -> EntryId {
    serde_json::from_str(&n.to_string()).expect("a nonzero number is an entry id")
}

// ------------------------------------------------------------------ the parser ----------------

/// One test per rejected shape, per ADR-0030 §7. A derived impl over the private `NonZeroU64`
/// would accept several of these depending on the format; this is the artefact that stops someone
/// replacing the hand-written impl with `#[derive(Deserialize)]`.
#[test]
fn entry_id_rejects_every_shape_that_is_not_an_unsigned_integer() {
    for (shape, json) in REJECTED_SHAPES {
        let parsed = serde_json::from_str::<EntryId>(json);
        assert!(
            parsed.is_err(),
            "EntryId accepted a {shape} ({json}); the hand-written Deserialize must reject it"
        );
    }
}

#[test]
fn entry_id_accepts_an_unsigned_integer() {
    assert_eq!(entry(41).to_string(), "entry#41");
}

#[test]
fn entry_id_round_trips_through_json_as_a_bare_number() {
    let id = entry(41);
    let json = serde_json::to_string(&id).expect("ids serialize");
    assert_eq!(
        json, "41",
        "spec.md:242-245 requires the bare number on the wire"
    );
    assert_eq!(
        serde_json::from_str::<EntryId>(&json).expect("round trip"),
        id
    );
}

#[test]
fn entry_id_accepts_the_largest_representable_id() {
    let json = u64::MAX.to_string();
    assert!(serde_json::from_str::<EntryId>(&json).is_ok());
}

/// The kind is type-directed at the decode site and the number carries no tag, so the *same* bytes
/// decode as any kind. ADR-0030 F6 §A says so explicitly and names the residual runtime obligation:
/// the backend must still check number-to-kind ownership against its committed index.
#[test]
fn the_same_number_decodes_as_any_kind_because_the_number_carries_no_tag() {
    let as_entry: EntryId = serde_json::from_str("41").expect("entry");
    let as_task: TaskId = serde_json::from_str("41").expect("task");
    assert_eq!(as_entry.kind(), IdKindTag::Entry);
    assert_eq!(as_task.kind(), IdKindTag::Task);
    // And they are not comparable to each other at all — see tests/compile-fail/.
}

// ------------------------------------------------------------------ the type -------------------

/// ADR-0030 F6 §A: `PhantomData<fn() -> K>` rather than `PhantomData<K>` so `Id<K>` stays
/// covariant and `Send`/`Sync` whatever `K` is. Asserted statically so that changing the phantom
/// to `PhantomData<K>` fails the build here rather than surfacing as an unsendable future in S4.
#[test]
fn ids_are_send_sync_and_copy_regardless_of_kind() {
    const fn assert_send_sync_copy<T: Send + Sync + Copy + 'static>() {}
    assert_send_sync_copy::<ConversationId>();
    assert_send_sync_copy::<EntryId>();
    assert_send_sync_copy::<TaskId>();
    assert_send_sync_copy::<SubmissionId>();
    assert_send_sync_copy::<DocumentId>();
    assert_send_sync_copy::<Seq>();
}

/// `NonZeroU64` is what makes this free, and `Lifetime::retired_at` is the `Option<Seq>` that
/// collects the benefit.
#[test]
fn an_optional_id_is_niche_packed() {
    assert_eq!(
        core::mem::size_of::<Option<EntryId>>(),
        core::mem::size_of::<EntryId>()
    );
    assert_eq!(core::mem::size_of::<EntryId>(), 8);
}

#[test]
fn ids_order_by_their_number_so_a_btreemap_can_key_a_table() {
    let mut ids = [entry(3), entry(1), entry(2)];
    ids.sort_unstable();
    assert_eq!(ids, [entry(1), entry(2), entry(3)]);
}

#[test]
fn the_root_conversation_is_one() {
    assert_eq!(ROOT_CONVERSATION_ID.to_string(), "conversation#1");
    let decoded: ConversationId = serde_json::from_str("1").expect("one is an id");
    assert_eq!(decoded, ROOT_CONVERSATION_ID);
}

#[test]
fn every_kind_names_itself_as_upstream_brands_it() {
    assert_eq!(IdKindTag::Conversation.as_str(), "conversation");
    assert_eq!(IdKindTag::Entry.as_str(), "entry");
    assert_eq!(IdKindTag::Task.as_str(), "task");
    assert_eq!(IdKindTag::Submission.as_str(), "submission");
    assert_eq!(IdKindTag::Document.as_str(), "document");
    assert_eq!(
        serde_json::to_string(&IdKindTag::Submission).expect("tags serialize"),
        "\"submission\""
    );
}

#[test]
fn each_marker_type_reports_its_own_tag() {
    assert_eq!(
        <crate::Conversation as IdKind>::KIND,
        IdKindTag::Conversation
    );
    assert_eq!(<crate::Entry as IdKind>::KIND, IdKindTag::Entry);
    assert_eq!(<crate::Task as IdKind>::KIND, IdKindTag::Task);
    assert_eq!(<crate::Submission as IdKind>::KIND, IdKindTag::Submission);
    assert_eq!(<crate::Document as IdKind>::KIND, IdKindTag::Document);
}

#[test]
fn debug_names_the_kind_so_a_log_line_is_unambiguous() {
    assert_eq!(format!("{:?}", entry(41)), "Id<entry>(41)");
}

// ------------------------------------------------------------------ RawId ----------------------

#[test]
fn erasing_an_id_to_a_raw_id_keeps_the_number_and_drops_the_kind() {
    let raw = RawId::from(entry(41));
    assert_eq!(raw.get(), NonZeroU64::new(41).expect("nonzero"));
    assert_eq!(raw.to_string(), "41");
    // The same number under a different kind erases to the same `RawId`: that is the point of the
    // type — `IdAlreadyOwned { id, by }` has to name a number whose kind is what is in dispute.
    let task: TaskId = serde_json::from_str("41").expect("task");
    assert_eq!(raw, RawId::from(task));
}

#[test]
fn a_raw_id_serializes_for_a_diagnostic_and_has_no_way_back() {
    let raw = RawId::from(entry(41));
    assert_eq!(
        serde_json::to_string(&raw).expect("raw ids serialize"),
        "41"
    );
    // There is no `Deserialize for RawId` and no `From<RawId> for Id<K>`; both are
    // tests/compile-fail/raw_id_has_no_way_back.rs.
}

// ------------------------------------------------- the id range a scan pages (PICO5-PLAN S10) ---

/// `Borrow<NonZeroU64>` is what makes `spec.md:4334-4336`'s *"inclusive ID range"* an `O(log n)`
/// descent instead of a filtered walk, and `Borrow`'s contract is that the borrowed ordering agrees
/// with the owned one. This asserts the agreement on the operation that depends on it: the same
/// bounds, expressed both ways, select the same ids.
#[test]
fn an_id_range_selects_the_same_ids_through_a_number_as_through_a_typed_bound() {
    let ids: std::collections::BTreeSet<EntryId> =
        [2, 3, 5, 8, 13].into_iter().map(entry).collect();
    let (lo, hi) = (entry(3), entry(8));
    let typed: Vec<EntryId> = ids.range(lo..=hi).copied().collect();
    let borrowed: Vec<EntryId> = ids
        .range::<NonZeroU64, _>(RawId::from(lo).get()..=RawId::from(hi).get())
        .copied()
        .collect();
    assert_eq!(typed, borrowed);
    assert_eq!(typed, vec![entry(3), entry(5), entry(8)]);
}

/// The reverse half, which is the one `scan_entries` actually uses: newest-first below an exclusive
/// upper bound, because a cursor resumes *strictly* below the page it ended.
#[test]
fn an_id_range_walks_newest_first_below_an_exclusive_bound() {
    let ids: std::collections::BTreeSet<EntryId> =
        [2, 3, 5, 8, 13].into_iter().map(entry).collect();
    let resume = RawId::from(entry(8)).get();
    let newest_first: Vec<EntryId> = ids
        .range::<NonZeroU64, _>(..resume)
        .rev()
        .copied()
        .collect();
    assert_eq!(newest_first, vec![entry(5), entry(3), entry(2)]);
}

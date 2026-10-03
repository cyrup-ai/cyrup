//! Tests for the keyed batch (ADR-0030 F3).
//!
//! Four of F3's five illegal states are **unspellable**, so their canaries are compile-fail cases in
//! `tests/compile-fail/` rather than assertions here — ADR-0030 §7's rule: *"keep one canary each, on
//! the module's privacy boundary, rather than a matrix."* What is left to test at runtime is the part
//! the shape does not decide: that staging the same id twice is reported rather than silently
//! last-wins, and that there is no empty batch.

use core::num::NonZeroU64;

use cyrup_pico_doc::{DefVersion, DocRoot};

use crate::{
    BatchBuilder, ConversationId, ConversationRecord, ConversationSemantics, DocumentBase,
    DocumentCommand, DocumentCreate, DocumentId, DocumentScope, EntryRecord, Id, Kind, LatestFork,
    Retire,
};

fn id<K: crate::IdKind>(n: u64) -> Id<K> {
    Id::new(NonZeroU64::new(n).expect("test ids are nonzero"))
}

fn kind(s: &str) -> Kind {
    Kind::parse(s).expect("test kinds parse")
}

fn conversation(n: u64) -> ConversationRecord {
    ConversationRecord {
        id: id(n),
        parent: None,
        owner: None,
    }
}

fn entry(n: u64, conversation_id: ConversationId) -> EntryRecord {
    EntryRecord {
        id: id(n),
        conversation_id,
        kind: kind("cyrup.message"),
        model: None,
        data: None,
        head: None,
        edits: None,
        by_task_id: None,
    }
}

fn create(n: u64) -> DocumentCreate {
    DocumentCreate {
        id: id(n),
        kind: kind("cyrup.state"),
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id: id(1),
            semantics: ConversationSemantics::Latest {
                fork: LatestFork::Current,
            },
        },
    }
}

fn base() -> DocumentBase {
    DocumentBase {
        version: DefVersion::FIRST,
        value: DocRoot::empty(),
    }
}

#[test]
fn an_empty_batch_does_not_exist() {
    assert!(
        BatchBuilder::new().build().is_none(),
        "a batch with nothing in it must not be constructible: no backend should ever be asked to \
         allocate a commit sequence for nothing"
    );
}

#[test]
fn staging_one_id_twice_is_reported_rather_than_overwritten() {
    let mut builder = BatchBuilder::new();
    builder
        .conversation(conversation(1))
        .expect("the first stage");
    let refused = builder
        .conversation(conversation(1))
        .expect_err("the second stage of one id");
    assert_eq!(refused.kind, crate::IdKindTag::Conversation);
    assert_eq!(refused.id, crate::RawId::from(id::<crate::Conversation>(1)));
}

/// F3's second failure mode, in the shape it actually takes: *"`assemble()` pushes a
/// `document.change` in a loop over open changes, the loop runs twice for one incarnation after a
/// memoisation bug."* With an array the effect depends on order; with a map it would be a silent
/// last-wins; here it is an error.
#[test]
fn a_second_command_for_one_incarnation_is_reported() {
    let mut builder = BatchBuilder::new();
    builder
        .create_document(create(9), base(), Retire::Keep)
        .expect("the first command");
    let refused = builder
        .retire_document(id(9))
        .expect_err("a second command for one incarnation");
    assert_eq!(refused.kind, crate::IdKindTag::Document);
}

/// A map key cannot disagree with its record, because no method takes both.
#[test]
fn the_key_is_derived_from_the_record() {
    let mut builder = BatchBuilder::new();
    builder
        .create_document(create(9), base(), Retire::Keep)
        .expect("stages");
    let batch = builder.build().expect("non-empty");
    let document: DocumentId = id(9);
    assert_eq!(
        batch.documents().keys().copied().collect::<Vec<_>>(),
        vec![document]
    );
}

/// Content and retirement are **one value**, which is why `spec.md:4363`'s ordering rule has nothing
/// left to impose: there is no second command whose position could change the outcome.
#[test]
fn retirement_rides_inside_the_content_command() {
    let mut builder = BatchBuilder::new();
    builder
        .create_document(create(9), base(), Retire::Retire)
        .expect("stages");
    let batch = builder.build().expect("non-empty");
    let command = batch
        .documents()
        .get(&id::<crate::Document>(9))
        .expect("staged");
    assert!(command.retires());
    assert!(command.creates());
    assert!(matches!(command, DocumentCommand::Create { .. }));
}

#[test]
fn a_batch_iterates_in_id_order_so_a_marker_is_byte_reproducible() {
    let mut builder = BatchBuilder::new();
    for n in [5_u64, 2, 9, 1] {
        builder.entry(entry(n, id(1))).expect("stages");
    }
    let batch = builder.build().expect("non-empty");
    let ids: Vec<u64> = batch
        .entries()
        .keys()
        .map(|id| crate::RawId::from(*id).get().get())
        .collect();
    assert_eq!(ids, vec![1, 2, 5, 9]);
}

#[test]
fn len_counts_every_table() {
    let mut builder = BatchBuilder::new();
    builder.conversation(conversation(1)).expect("stages");
    builder.entry(entry(2, id(1))).expect("stages");
    builder
        .create_document(create(3), base(), Retire::Keep)
        .expect("stages");
    let batch = builder.build().expect("non-empty");
    assert_eq!(batch.len(), 3);
    assert!(!batch.is_empty());
}

#[test]
fn into_parts_preserves_every_table() {
    let mut builder = BatchBuilder::new();
    builder.conversation(conversation(1)).expect("stages");
    builder.entry(entry(2, id(1))).expect("stages");
    let parts = builder.build().expect("non-empty").into_parts();
    assert_eq!(parts.conversations.len(), 1);
    assert_eq!(parts.entries.len(), 1);
    assert!(parts.tasks.is_empty());
    assert!(parts.submissions.is_empty());
    assert!(parts.documents.is_empty());
}

//! Record serde tests.
//!
//! ADR-0030 §10's last serde row allows `derive` for persisted records, on one condition: no
//! `#[serde(borrow)]` and no `Cow`. What these tests pin is the part that is easy to lose — that the
//! *shapes* carrying an invariant reject the illegal wire form, because a record is re-read on every
//! open and serde is therefore the main construction path (`RUST-DESIGN-REVIEW.md:73`).

use core::num::NonZeroU64;

use cyrup_pico_doc::DocValue;

use crate::{
    ConversationSemantics, DocumentAddress, DocumentCreate, DocumentRecord, DocumentScope, Id,
    InputSubmission, Kind, LatestFork, Lifetime, RewindableFork, ScopeRef, Seq, SubmissionRecord,
    SubmissionState, SubmissionStatus, TaskOutcome, TaskRecord, TaskState, TaskStatus,
};

fn id<K: crate::IdKind>(n: u64) -> Id<K> {
    Id::new(NonZeroU64::new(n).expect("test ids are nonzero"))
}

fn seq(n: u64) -> Seq {
    Seq::new(NonZeroU64::new(n).expect("test sequences are nonzero"))
}

fn kind(s: &str) -> Kind {
    Kind::parse(s).expect("test kinds parse")
}

/// `spec.md:1053-1054`: *"`fork: "asOf"` requires `history: "rewindable"`."* The two histories carry
/// different fork types, so the illegal pair is not a value — and a derived `Deserialize` cannot forge
/// it either, which is the half that matters for a persisted record.
#[test]
fn a_latest_document_cannot_be_forked_as_of() {
    let legal = r#"{"history":"rewindable","fork":"asOf"}"#;
    assert_eq!(
        serde_json::from_str::<ConversationSemantics>(legal).expect("rewindable + asOf is legal"),
        ConversationSemantics::Rewindable {
            fork: RewindableFork::AsOf
        }
    );
    let illegal = r#"{"history":"latest","fork":"asOf"}"#;
    let refused = serde_json::from_str::<ConversationSemantics>(illegal)
        .expect_err("latest + asOf must not decode");
    let message = refused.to_string();
    assert!(message.contains("asOf"), "{message}");
}

#[test]
fn a_document_record_round_trips_with_its_semantics() {
    let record = DocumentRecord {
        id: id(9),
        kind: kind("cyrup.state"),
        key: Some("left".to_owned()),
        lifetime: Lifetime::open(seq(3)),
        scope: DocumentScope::Conversation {
            conversation_id: id(1),
            semantics: ConversationSemantics::Rewindable {
                fork: RewindableFork::AsOf,
            },
        },
    };
    let json = serde_json::to_string(&record).expect("serializes");
    assert_eq!(
        serde_json::from_str::<DocumentRecord>(&json).expect("re-decodes"),
        record
    );
    assert!(record.retains_history());
    assert!(record.is_alive_at(seq(3)));
}

/// `spec.md:4341-4342`: *"A missing key means the singleton, not every family member."*
#[test]
fn a_singleton_address_is_not_a_keyless_family_address() {
    let singleton = DocumentAddress {
        kind: kind("cyrup.state"),
        scope: ScopeRef::Conversation(id(1)),
        key: None,
    };
    let member = DocumentAddress {
        key: Some("left".to_owned()),
        ..singleton.clone()
    };
    assert_ne!(singleton, member);
}

/// A creation carries no lifetime, so `createdAt` cannot disagree with the commit that created it.
#[test]
fn a_creation_is_stamped_by_the_commit_not_by_the_caller() {
    let create = DocumentCreate {
        id: id(9),
        kind: kind("cyrup.state"),
        key: None,
        scope: DocumentScope::Session,
    };
    let address = create.address();
    let live = create.clone().stamp(seq(4), false);
    assert_eq!(live.lifetime, Lifetime::open(seq(4)));
    assert_eq!(live.address(), address);
    assert!(!live.retains_history());

    // `spec.md:1114`: a creation retired in its own commit has an empty lifetime.
    let empty = create.stamp(seq(4), true);
    assert!(empty.lifetime.is_empty());
    assert!(!empty.is_alive_at(seq(4)));
}

/// Upstream writes this union with eleven `?: never` fields. Here the states are variants, so a write
/// submission that claims to be `placed` has no spelling at all.
#[test]
fn a_submission_state_carries_only_its_own_fields() {
    let placed = SubmissionRecord {
        id: id(2),
        conversation_id: id(1),
        request_id: Some("req-1".to_owned()),
        state: SubmissionState::Input(InputSubmission::Placed { entry: id(3) }),
    };
    assert_eq!(placed.status(), SubmissionStatus::Placed);
    let json = serde_json::to_string(&placed).expect("serializes");
    assert_eq!(
        serde_json::from_str::<SubmissionRecord>(&json).expect("re-decodes"),
        placed
    );
    // A write submission has no `placed` state to decode.
    let illegal = r#"{"id":2,"conversation_id":1,"type":"write","status":"placed","entry":3}"#;
    assert!(serde_json::from_str::<SubmissionRecord>(illegal).is_err());
}

/// `spec.md:1610-1618`: memos exist on a live state and not on a terminal one. Here they are fields of
/// the live variants, so a terminal task with memos is not a value that can be built or decoded.
#[test]
fn a_terminal_task_has_no_memos_field() {
    let terminal = TaskRecord {
        id: id(4),
        conversation_id: id(1),
        kind: kind("cyrup.generation"),
        version: cyrup_pico_doc::DefVersion::FIRST,
        input: DocValue::Null,
        owner: None,
        background: false,
        abort_requested: false,
        state: TaskState::Terminal {
            outcome: TaskOutcome::Completed {
                result: DocValue::integer(1),
            },
        },
    };
    assert_eq!(terminal.status(), TaskStatus::Terminal);
    assert!(terminal.state.is_terminal());
    let json = serde_json::to_string(&terminal).expect("serializes");
    assert!(!json.contains("memos"), "{json}");
    assert_eq!(
        serde_json::from_str::<TaskRecord>(&json).expect("re-decodes"),
        terminal
    );
}

#[test]
fn the_fork_policy_widens_in_one_direction_only() {
    assert_eq!(
        ConversationSemantics::Latest {
            fork: LatestFork::Initial
        }
        .fork(),
        RewindableFork::Initial
    );
    assert_eq!(
        ConversationSemantics::Rewindable {
            fork: RewindableFork::AsOf
        }
        .fork(),
        RewindableFork::AsOf
    );
}

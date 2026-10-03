//! Tests for the two-class failure outcome — one of the three things ADR-0030 §9 calls irreversible.

use core::num::NonZeroU64;
use std::io;

use crate::{
    CommitError, Corruption, DocumentId, Id, IdKindTag, RawId, RejectedReason, Seq, StorageFailure,
    UncertainCommit,
};

fn raw(n: u64) -> RawId {
    RawId::new(NonZeroU64::new(n).expect("test ids are nonzero"))
}

fn document(n: u64) -> DocumentId {
    Id::new(NonZeroU64::new(n).expect("test ids are nonzero"))
}

#[test]
fn the_two_classes_are_distinguishable_without_downcasting() {
    let rejected = CommitError::Rejected(RejectedReason::IdAlreadyOwned {
        id: raw(41),
        owner: IdKindTag::Entry,
    });
    let uncertain = CommitError::Uncertain(UncertainCommit::new("appending a marker"));
    assert!(rejected.is_rejected());
    assert!(!uncertain.is_rejected());
}

/// `RUST-DESIGN-REVIEW.md:73` and ADR-0030 §10: a rejection must be loggable without becoming a
/// construction path, so `RejectedReason` is `Serialize` **only**.
#[test]
fn a_rejection_serializes_for_a_diagnostic() {
    let json = serde_json::to_string(&RejectedReason::VersionTransitionRequiresBase {
        document: document(9),
        stored: cyrup_pico_doc::DefVersion::FIRST,
        continues: cyrup_pico_doc::DefVersion::FIRST,
    })
    .expect("serializes");
    assert!(json.contains("VersionTransitionRequiresBase"), "{json}");
}

#[test]
fn an_uncertain_commit_keeps_its_source_chain_and_renders_it() {
    let uncertain = UncertainCommit::caused_by(
        "appending the main marker",
        io::Error::other("device detached"),
    );
    let rendered = uncertain.to_string();
    assert!(rendered.contains("appending the main marker"), "{rendered}");
    assert!(rendered.contains("device detached"), "{rendered}");
    assert_eq!(
        serde_json::to_string(&uncertain).expect("serializes"),
        format!("{:?}", rendered.as_str())
    );
}

/// The channels `spec.md:4352-4360` keeps apart: absence is `Ok(None)` and is not in this enum at all.
#[test]
fn corruption_absence_and_unretained_history_are_three_different_answers() {
    let corrupt = StorageFailure::Corrupt(Corruption::SequenceNotIncreasing {
        previous: Seq::FIRST,
        found: Seq::FIRST,
    });
    let unretained = StorageFailure::HistoryNotRetained {
        document: document(9),
        at: Seq::FIRST,
    };
    assert!(matches!(corrupt, StorageFailure::Corrupt(_)));
    assert!(matches!(
        unretained,
        StorageFailure::HistoryNotRetained { .. }
    ));
    // And absence: there is no variant for it, because it is `Ok(None)`.
    let absent: Result<Option<u8>, StorageFailure> = Ok(None);
    assert!(matches!(absent, Ok(None)));
}

/// The storage-level `Corruption` wraps the replay-level one rather than merging with it, so a
/// document's replay failure keeps the incarnation's name.
#[test]
fn a_replay_corruption_names_the_document_it_came_from() {
    let corruption = Corruption::Replay {
        document: document(12),
        source: cyrup_pico_doc::Corruption::MissingRequiredBase,
    };
    let rendered = corruption.to_string();
    assert!(rendered.contains("12"), "{rendered}");
    assert!(rendered.contains("no base"), "{rendered}");
}

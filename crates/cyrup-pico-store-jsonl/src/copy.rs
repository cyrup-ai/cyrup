//! `document.copy`'s source checks (`spec.md:4313-4318`).
//!
//! Split out of [`crate::state`] because it is the one command whose validation is a *relation* between
//! two records and a batch, and because `spec.md:4316`'s prohibition — *"a batch may not create, change,
//! or retire a selected source"* — is the rejection that makes a copy order-independent. ADR-0030 §2.3
//! states the failure it prevents: *"without it, the child's value depends on whether the fork command
//! was ordered before or after the parent's write — the same program producing two different
//! children"*. Under ADR-0030 F3's keyed batch the two orders are not even two different batches, so the
//! check is all that is left of the problem.
//!
//! Everything here is **pure**. The source's *value* needs the medium and is read by
//! [`crate::store`] before it allocates a sequence, so a copy that cannot be read is still a commit
//! with nothing durable behind it. PICO5-PLAN S9 is the slice that builds forks on top of this; what is
//! here is the storage obligation alone.

use cyrup_pico_store::{
    Batch, CopySource, CopySourceMismatch, DocumentCreate, DocumentPoint, DocumentScope,
    RejectedReason,
};

use crate::state::{Committed, DocState};

/// Check everything about a copy that can be decided from records.
///
/// # Errors
///
/// [`RejectedReason::CopySourceInBatch`] when this batch also writes the source;
/// [`RejectedReason::CopySourceNotAlive`] when the source is unknown, retired at
/// [`DocumentPoint::Current`], or outside a rewindable incarnation's lifetime at a numeric point;
/// [`RejectedReason::CopySourceMismatch`] when the source disagrees with the child record on kind, key,
/// scope, history or fork.
pub(crate) fn check_source(
    state: &Committed,
    child: &DocumentCreate,
    source: CopySource,
    batch: &Batch,
) -> Result<(), RejectedReason> {
    if batch.documents().contains_key(&source.id) {
        return Err(RejectedReason::CopySourceInBatch {
            document: source.id,
        });
    }
    let found = state
        .documents
        .get(&source.id)
        .ok_or(RejectedReason::CopySourceNotAlive {
            document: source.id,
        })?;
    let mismatch = |what: CopySourceMismatch| RejectedReason::CopySourceMismatch {
        document: source.id,
        what,
    };
    if found.record.kind != child.kind {
        return Err(mismatch(CopySourceMismatch::Kind));
    }
    if found.record.key != child.key {
        return Err(mismatch(CopySourceMismatch::Key));
    }
    let (
        DocumentScope::Conversation {
            semantics: source_semantics,
            ..
        },
        DocumentScope::Conversation {
            semantics: child_semantics,
            ..
        },
    ) = (&found.record.scope, &child.scope)
    else {
        // `spec.md:4314`: a copy source must be a conversation document.
        return Err(mismatch(CopySourceMismatch::Scope));
    };
    if source_semantics.retains_history() != child_semantics.retains_history() {
        return Err(mismatch(CopySourceMismatch::History));
    }
    if source_semantics.fork() != child_semantics.fork() {
        return Err(mismatch(CopySourceMismatch::Fork));
    }
    if readable_at(found, source.at).is_none() {
        return Err(RejectedReason::CopySourceNotAlive {
            document: source.id,
        });
    }
    Ok(())
}

/// The cutoff a read of this source at `at` uses, or `None` when it is not a usable source.
///
/// `None` carries both honest answers — the incarnation is not alive at that point, or its history is
/// not retained there — and the caller turns it into [`RejectedReason::CopySourceNotAlive`] for the
/// same reason `MemoryStore` does: a copy from an unreadable source is one rejection, not two.
pub(crate) fn readable_at(
    state: &DocState,
    at: DocumentPoint,
) -> Option<Option<cyrup_pico_store::Seq>> {
    match at {
        DocumentPoint::Current => {
            if state.record.lifetime.is_open() {
                Some(None)
            } else {
                None
            }
        }
        DocumentPoint::At(seq) => {
            if state.record.retains_history() && state.record.lifetime.contains(seq) {
                Some(Some(seq))
            } else {
                None
            }
        }
    }
}

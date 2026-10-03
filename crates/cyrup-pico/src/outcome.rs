//! ADR-0030 F1's commit outcome: four named variants, with the mutation handle **inside** the three
//! non-fatal ones and absent from the fourth.
//!
//! # What the shape replaces
//!
//! Upstream health is a `#poison` flag checked on every entry point, and the failure classification
//! is one `instanceof StorageRejected` test over an exception subclass each backend picks by hand.
//! The poisoned object stays in the caller's hands, so the mistake is a `catch` away — and
//! `spec.md:4581` has to *ask* people not to make it.
//!
//! Here reusing a dead Session is a **compile error inside the kernel**: the handle is in the success
//! variants, not beside the error, so `?` cannot smuggle one out and a catch-and-continue has nothing
//! to continue with. `spec.md:4581`'s request becomes a rule, and
//! `tests/compile-fail/a_session_mut_cannot_be_obtained_from_an_uncertain_outcome.rs` is the proof.
//!
//! # Both poison paths, which is why [`UncertainKind`] has two variants
//!
//! F1 calls the second one *the dropped second poison path*, and it is the one a port will skip.
//! Upstream poisons on a non-`StorageRejected` failure from `storage.commit()` **and** on a throw
//! from `tx.adopt(seq)` *after storage has already committed* — `#runCommit`'s own comment reads
//! *"Storage already committed; a failed adoption leaves memory behind durable state."* The two need
//! different recovery: the first means reopen **and reconcile**, the second means storage is
//! known-good and there is nothing to reconcile.

use std::sync::Arc;

use cyrup_pico_store::{AlreadyStaged, RejectedReason, Seq};
use serde::ser::SerializeStruct as _;

use crate::SessionMut;
use crate::docs::AuthorityPoisoned;
use crate::tx::{CallbackError, PreparationFailed};
use crate::witness::AdoptionFailed;

/// What one commit did.
///
/// `#[must_use]` because dropping it on the floor drops the Session's one mutation handle with it.
#[derive(Debug)]
#[must_use]
pub enum CommitOutcome<R> {
    /// The callback produced no writes.
    ///
    /// Nothing reached storage, **no sequence was allocated**, nothing was published, nothing can
    /// poison. A named variant rather than a fabricated sequence, so no caller can order a commit
    /// that never happened against one that did.
    NothingToCommit {
        /// The callback's value.
        result: R,
        /// The handle, returned.
        session: SessionMut,
    },
    /// The batch is durable at `seq`, adopted, and published.
    Committed {
        /// The callback's value.
        result: R,
        /// The sequence storage allocated.
        seq: Seq,
        /// The handle, returned.
        session: SessionMut,
    },
    /// A failure **before** storage admission. Nothing durable, nothing published, Session fully
    /// usable — `G-PRE-ADMISSION-ROLLBACK`.
    RolledBack {
        /// Which pre-admission step refused.
        reason: RollbackReason,
        /// The handle, returned.
        session: SessionMut,
    },
    /// The Session is dead. **There is no handle in this variant.**
    Uncertain(UncertainCommit),
}

/// What a settled, non-fatal commit decided.
///
/// # Why this type exists, against ADR-0030 F1's sketch
///
/// F1 offers one ergonomic helper, `or_fatal(self) -> Result<(R, Option<Seq>, SessionMut), UncertainCommit>`,
/// and that signature **cannot be written**: [`CommitOutcome::RolledBack`] carries no `R`, because a
/// callback that failed produced no value. Returning `Option<R>` would push the same three-way
/// decision into the caller's `match` with the names removed, which is the opposite of F1's point. So
/// the helper returns this three-variant enum instead, and the property F1 actually wanted is
/// preserved exactly: the handle is reachable only through the `Ok` arm, and [`UncertainCommit`] has
/// no handle in it at all.
#[derive(Debug)]
pub enum Settled<R> {
    /// Nothing was staged.
    NothingToCommit(R),
    /// Durable and adopted.
    Committed {
        /// The callback's value.
        result: R,
        /// The sequence.
        seq: Seq,
    },
    /// Refused before admission.
    RolledBack(RollbackReason),
}

impl<R> CommitOutcome<R> {
    /// Split the fatal arm off from the three usable ones.
    ///
    /// # Errors
    /// [`UncertainCommit`] — and with it no [`SessionMut`], which is the whole point.
    pub fn or_fatal(self) -> Result<(Settled<R>, SessionMut), UncertainCommit> {
        match self {
            Self::NothingToCommit { result, session } => {
                Ok((Settled::NothingToCommit(result), session))
            }
            Self::Committed {
                result,
                seq,
                session,
            } => Ok((Settled::Committed { result, seq }, session)),
            Self::RolledBack { reason, session } => Ok((Settled::RolledBack(reason), session)),
            Self::Uncertain(u) => Err(u),
        }
    }

    /// Whether this outcome killed the Session.
    #[must_use]
    pub const fn is_fatal(&self) -> bool {
        matches!(self, Self::Uncertain(_))
    }
}

/// What a ticket holder learns.
///
/// The [`Committer`](crate::Committer) boundary is where ADR-0030 F1 says the guarantee is
/// **checked, not consuming**, and this document says so rather than claiming the consuming form
/// while shipping an `Arc`: a ticket cannot be consumed, so its holder learns the Session is dead
/// from [`CommitReply::SessionDead`] or, for every later call, from a closed channel. That is weaker,
/// and the failure is weaker too — a ticket holder cannot reach adoption, publication or any
/// in-memory baseline, so it can receive an error but cannot cause divergence.
#[derive(Debug)]
#[must_use]
pub enum CommitReply<R> {
    /// Nothing was staged.
    NothingToCommit(R),
    /// Durable and adopted.
    Committed {
        /// The callback's value.
        result: R,
        /// The sequence.
        seq: Seq,
    },
    /// Refused before admission; the line is still alive.
    RolledBack(RollbackReason),
    /// This commit killed the line. Every later ticket call fails with
    /// [`LineClosed`](crate::LineClosed).
    SessionDead(UncertainCommit),
}

/// Which pre-admission step refused the commit.
///
/// Every arm means *nothing durable happened and the Session is fully usable*. That is the whole
/// content of the type, and it is why `Rejected` is in here and `Uncertain` is not: a backend's
/// [`RejectedReason`] is a promise that rollback is guaranteed (`spec.md:4310-4311`), and
/// [`RejectedReason`] is a **closed** enum with no string arm and no `From<std::io::Error>`, so a
/// timeout or an `ENOSPC` has no way to arrive here.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RollbackReason {
    /// The callback returned an error, or abandoned the change.
    #[error("the commit callback failed")]
    Callback(#[source] CallbackError),
    /// Preparation could not assemble the batch.
    #[error("preparation failed")]
    Preparation(#[source] PreparationFailed),
    /// A draft operation left an unapplicable path at preparation time.
    #[error("a prepared change is not applicable")]
    Path(#[source] cyrup_pico_doc::PathError),
    /// Two commands for one record were staged.
    #[error("batch assembly failed")]
    Assembly(#[source] AlreadyStaged),
    /// The document authority was poisoned by an earlier panic.
    #[error("the document authority is unusable")]
    Authority(#[source] AuthorityPoisoned),
    /// Storage refused the batch and guaranteed rollback.
    #[error("storage rejected the batch")]
    Rejected(#[source] RejectedReason),
    /// [`close()`](crate::SessionMut::close) already sealed mutation admission.
    ///
    /// `spec.md:1545-1548`: *"Closing seals mutation admission and task reservation."* The seal is
    /// one-way, so this arm is terminal for writes while leaving reads and the handle intact — a
    /// rollback, not a poison.
    #[error("mutation admission is sealed: this Session is closing or closed")]
    AdmissionSealed,
}

impl From<PreparationFailed> for RollbackReason {
    fn from(e: PreparationFailed) -> Self {
        Self::Preparation(e)
    }
}

impl From<cyrup_pico_doc::PathError> for RollbackReason {
    fn from(e: cyrup_pico_doc::PathError) -> Self {
        Self::Path(e)
    }
}

impl From<AlreadyStaged> for RollbackReason {
    fn from(e: AlreadyStaged) -> Self {
        Self::Assembly(e)
    }
}

impl From<AuthorityPoisoned> for RollbackReason {
    fn from(e: AuthorityPoisoned) -> Self {
        Self::Authority(e)
    }
}

/// Which side of the fence the commit died on.
///
/// Both arms are fatal and the Session must be reopened; they differ in what the **host** then owes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UncertainKind {
    /// Storage did not say whether the batch committed. **Reopen and reconcile.**
    CommitStateUnknown,
    /// Storage committed at `seq`; in-memory adoption failed. Durable state is known-good and known
    /// **ahead** of memory. **Reopen; nothing to reconcile.**
    AdoptionFailedAfterCommit {
        /// The sequence storage committed at.
        seq: Seq,
    },
}

/// The fatal outcome: what happened, and the error that said so.
///
/// # Why `source` is not a [`StorageFailure`](cyrup_pico_store::StorageFailure)
///
/// ADR-0030 F1 writes `pub struct UncertainCommit { pub kind: UncertainKind, pub source: StorageFailure }`.
/// That field type is wrong for half of its own enum: [`UncertainKind::AdoptionFailedAfterCommit`] is
/// by definition **not** a storage failure — storage succeeded — so filling a `StorageFailure` there
/// would mean fabricating one, which is exactly the misclassification F1 exists to prevent. The source
/// is therefore the erased error that actually occurred, shared behind an `Arc` so that the
/// committing ticket can be told and the diagnostic can be logged without the type becoming a
/// construction path. The discrepancy is recorded rather than resolved silently.
///
/// Per ADR-0030 §10's serde table this type is **`Serialize` only**: it must be loggable and
/// surfaceable in a diagnostic without a deserialisable form through which a corrupted log line or an
/// IPC boundary could reintroduce the misclassification.
#[derive(Clone)]
pub struct UncertainCommit {
    kind: UncertainKind,
    source: Arc<dyn core::error::Error + Send + Sync>,
}

impl UncertainCommit {
    /// Storage did not say whether the batch committed.
    pub(crate) fn commit_state_unknown(source: cyrup_pico_store::UncertainCommit) -> Self {
        Self {
            kind: UncertainKind::CommitStateUnknown,
            source: Arc::new(source),
        }
    }

    /// Storage committed; adoption did not.
    pub(crate) fn adoption_failed(failed: AdoptionFailed) -> Self {
        Self {
            kind: UncertainKind::AdoptionFailedAfterCommit { seq: failed.seq },
            source: Arc::new(failed),
        }
    }

    /// Which side of the fence it died on.
    #[must_use]
    pub const fn kind(&self) -> UncertainKind {
        self.kind
    }

    /// The sequence storage committed at, when it is known.
    ///
    /// `Some` exactly for [`UncertainKind::AdoptionFailedAfterCommit`]. For
    /// [`UncertainKind::CommitStateUnknown`] there is no honest answer, and `None` says so.
    #[must_use]
    pub const fn committed_at(&self) -> Option<Seq> {
        match self.kind {
            UncertainKind::CommitStateUnknown => None,
            UncertainKind::AdoptionFailedAfterCommit { seq } => Some(seq),
        }
    }

    /// The error that reported the failure.
    #[must_use]
    pub fn source_error(&self) -> &(dyn core::error::Error + Send + Sync + 'static) {
        &*self.source
    }
}

impl core::fmt::Debug for UncertainCommit {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UncertainCommit")
            .field("kind", &self.kind)
            .field("source", &format_args!("{}", self.source))
            .finish()
    }
}

impl core::fmt::Display for UncertainCommit {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.kind {
            UncertainKind::CommitStateUnknown => f.write_str(
                "the commit state is unknown: this Session must be reopened and reconciled",
            )?,
            UncertainKind::AdoptionFailedAfterCommit { seq } => write!(
                f,
                "storage committed at sequence {seq} but adoption failed: this Session must be reopened, with nothing to reconcile"
            )?,
        }
        write!(f, ": {}", self.source)
    }
}

impl core::error::Error for UncertainCommit {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&*self.source)
    }
}

impl serde::Serialize for UncertainCommit {
    /// Hand-written, and `Serialize` only.
    ///
    /// The source is rendered rather than structured for the same reason
    /// [`Corruption::RecordMalformed`](cyrup_pico_store::Corruption::RecordMalformed) renders its
    /// detail: an erased error is neither `Serialize` nor a closed set, and a diagnostic needs the
    /// message rather than the type.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("UncertainCommit", 2)?;
        s.serialize_field("kind", &self.kind)?;
        s.serialize_field("source", &self.source.to_string())?;
        s.end()
    }
}

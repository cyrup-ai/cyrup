//! The two-class commit failure, and the read failure that keeps corruption apart from absence.
//!
//! ADR-0030 §9 names three things that *"are irreversible and must be right before any backend
//! exists"*. This module is one of them: **the two-class failure outcome**. The other two are the
//! keyed batch ([`crate::Batch`]) and the `&mut self` single-committer signature
//! ([`crate::Storage::commit`]).
//!
//! # Why two classes and not one error type
//!
//! `spec.md:4307-4311`: *"`StorageRejected` means a batch was rejected before any durable effect
//! and is guaranteed not to have committed… unknown failures after Storage admission remain fatal
//! because their commit state is uncertain."* Upstream that distinction is an exception subclass and
//! one `instanceof` test, and ADR-0030 §2.2 states the cost of getting it wrong in either
//! direction: *"classified wrong one way, a bad fork source costs the user the whole live session;
//! wrong the other way, the Session runs on a baseline that already disagrees with the disk."*
//!
//! So the choice is not optional and not defaultable. [`CommitError`] has exactly two variants,
//! [`RejectedReason`] is **closed** — no `Other(String)`, no `#[non_exhaustive]` escape hatch — and
//! there is deliberately **no `From<std::io::Error>` for `CommitError`**: an I/O error has no honest
//! class, because whether the write reached the medium is exactly what is unknown. A backend that
//! hits one must say which it means, and `tests/compile-fail/io_error_is_not_a_rejection.rs` is the
//! proof that `?` cannot make that decision for it.
//!
//! # Why a read failure is a different type
//!
//! `spec.md:4352-4360` and ADR-0030 §2.2: *"a legitimately absent record is never reported as
//! damaged data, or vice versa."* Upstream the split *"is carried entirely by which call path
//! throws versus returns `undefined`"*. Here every read returns
//! `Result<Option<T>, StorageFailure>`: `Ok(None)` is absence, `Err(Corrupt(..))` fails the open,
//! and confusing them would require writing the other constructor.

use core::fmt;
use std::error::Error;
use std::io;
use std::sync::Arc;

use serde::{Serialize, Serializer};

use crate::id::{IdKindTag, RawId};
use crate::records::DocumentAddress;
use crate::{ConversationId, DefVersion, DocumentId, EntryId, Seq};

/// Why one admitted commit did not happen.
///
/// Two variants, and the line between them is a durability claim rather than a severity: see this
/// module's documentation.
#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    /// Rejected **before any durable effect**. The caller may roll back and carry on.
    ///
    /// A backend returns this only when it can guarantee nothing reached the medium
    /// (`spec.md:4310-4311`: *"Backends use `StorageRejected` for deterministic `document.copy`
    /// source, replay, and consistency failures only when rollback is guaranteed."*). Nothing in
    /// any type checks that the claim is true — ADR-0030 §2.2 classifies that half **checked**, and
    /// the conformance suite's dishonest-backend cases (PICO5-PLAN S8) are where it is tested.
    #[error("the batch was rejected before any durable effect: {0}")]
    Rejected(#[from] RejectedReason),
    /// The commit's state is **unknown**. Fatal to the Session that issued it.
    ///
    /// `spec.md:78` and §1.8: the Session publishes nothing, closes every outstanding ticket and
    /// must be reopened. Continuing would prepare the next change against a baseline that may
    /// already disagree with the disk.
    #[error("the commit state is unknown and the session must be reopened: {0}")]
    Uncertain(#[source] UncertainCommit),
}

impl CommitError {
    /// Whether this failure leaves the store in a known state.
    ///
    /// Provided so a caller matching on the class does not have to spell the match out; the match
    /// itself stays exhaustive because the enum is closed.
    #[must_use]
    pub const fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected(_))
    }
}

/// An uncertain commit, with whatever the backend knows about it.
///
/// `Serialize` only, per ADR-0030 §10's serde table: it must be loggable and surfaceable in a
/// diagnostic without becoming a construction path. A `Deserialize` would let a damaged log line or
/// an IPC boundary mint a claim about a commit nobody made.
///
/// ADR-0030 §10 sketches this slot as `Box<dyn Error + Send + Sync>`. It is a named struct instead
/// for one reason: the same table requires the type to be `Serialize`, and a `dyn Error` is not. The
/// source chain is kept and rendered.
#[derive(Debug)]
pub struct UncertainCommit {
    what: Arc<str>,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl UncertainCommit {
    /// Record an uncertain commit, describing what was in flight.
    #[must_use]
    pub fn new(what: impl Into<Arc<str>>) -> Self {
        Self {
            what: what.into(),
            source: None,
        }
    }

    /// Record an uncertain commit caused by `source`.
    #[must_use]
    pub fn caused_by(
        what: impl Into<Arc<str>>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            what: what.into(),
            source: Some(Box::new(source)),
        }
    }

    /// What was in flight.
    #[must_use]
    pub fn what(&self) -> &str {
        &self.what
    }
}

impl fmt::Display for UncertainCommit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.what)?;
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl Error for UncertainCommit {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_ref().map(|s| &**s as &(dyn Error + 'static))
    }
}

impl Serialize for UncertainCommit {
    /// Renders as the message plus the source chain, flattened: a diagnostic, not a record.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&format_args!("{self}"))
    }
}

/// Why a batch was rejected, with nothing durable written.
///
/// **Closed on purpose.** There is no `Other(String)` arm, because a string arm is how a backend
/// avoids deciding what happened — and every variant here is a fact the Session can act on. Adding
/// a variant is a deliberate edit to this enum and to every exhaustive match on it, which is the
/// point.
///
/// Each variant is one of the checks ADR-0030 F3 lists as **not** lifted by the keyed batch: a
/// cross-batch fact about already-committed records. The in-batch illegal states — two content
/// commands for one incarnation, two retirements, a delta with no base, a dependence on write order
/// — have no variants here because they have no spelling (see [`crate::Batch`]).
#[derive(Debug, Serialize, thiserror::Error)]
pub enum RejectedReason {
    /// `spec.md:4274-4275`: one number is owned by one record of one type.
    ///
    /// ADR-0030 F6 §A: derived from the per-table indexes, not from a separate durable
    /// number-to-kind table.
    #[error("id {id} is already owned by a {owner} record")]
    IdAlreadyOwned {
        /// The number.
        id: RawId,
        /// The kind of record that owns it.
        owner: IdKindTag,
    },
    /// A conversation record was written twice. `spec.md:4274` makes conversation creation
    /// immutable.
    #[error("conversation {0} already exists and conversation records are immutable")]
    ConversationExists(ConversationId),
    /// An entry record was written twice. `spec.md:69`'s invariant 5: entries are immutable.
    #[error("entry {0} already exists and entry records are immutable")]
    EntryExists(EntryId),
    /// A document incarnation was created twice.
    #[error("document {0} already exists")]
    DocumentExists(DocumentId),
    /// A content command or retirement named an incarnation this store has never seen.
    #[error("document {0} does not exist")]
    UnknownDocument(DocumentId),
    /// A content command or retirement named an already-retired incarnation.
    #[error("document {document} was retired at sequence {retired_at}")]
    DocumentRetired {
        /// The incarnation.
        document: DocumentId,
        /// When it was retired.
        retired_at: Seq,
    },
    /// `spec.md:4362-4365`: a logical address holds at most one live incarnation.
    ///
    /// The cross-batch half of F3's keyed batch, and the reason the map key is not enough: the
    /// occupant is already committed. Legal when the same batch retires the occupant — *"retire plus
    /// create at one logical address makes the new incarnation current at that sequence"*.
    #[error("{address} already has a live incarnation, document {occupant}")]
    AddressOccupied {
        /// The address.
        address: DocumentAddress,
        /// The incarnation already there.
        occupant: DocumentId,
    },
    /// `spec.md:4366-4367`: *"Deltas cannot cross a stored version boundary; a version transition
    /// must be a base."*
    ///
    /// Partly lifted by [`crate::DocumentContent::Delta`]'s `StoredVersion` witness — a delta cannot
    /// claim a version nobody read — and partly cross-batch, which is this variant: the witness is
    /// real but stale, because a later commit moved the document's stored version.
    #[error(
        "document {document} is stored at version {stored} but the delta continues version {continues}: a version transition must be a base"
    )]
    VersionTransitionRequiresBase {
        /// The incarnation.
        document: DocumentId,
        /// The version its newest applicable base is at.
        stored: DefVersion,
        /// The version the delta claims to continue.
        continues: DefVersion,
    },
    /// A copy named a source this store does not have, or that is not alive at the selected point.
    #[error("copy source document {document} is not alive at the selected point")]
    CopySourceNotAlive {
        /// The source named by the command.
        document: DocumentId,
    },
    /// `spec.md:4313-4316`: a copy's source must agree with the child create record on kind, key,
    /// history and fork.
    #[error("copy source document {document} does not match the child record: {what}")]
    CopySourceMismatch {
        /// The source.
        document: DocumentId,
        /// Which field disagrees.
        what: CopySourceMismatch,
    },
    /// `spec.md:4316`: *"A batch may not create, change, or retire a selected source."*
    ///
    /// This is the rejection that makes a copy order-independent: with it, the child cannot depend
    /// on whether the parent's write was assembled before or after the copy — and under F3's keyed
    /// batch the two orders are not even two different batches.
    #[error("copy source document {document} is also written by this batch")]
    CopySourceInBatch {
        /// The source.
        document: DocumentId,
    },
    /// The id namespace is exhausted. Returned by [`crate::Storage::mint_raw`].
    ///
    /// `memory.ts:410-413` checks this with `Number.isSafeInteger`; here it is the `u64` ceiling,
    /// and it is fallible rather than wrapping because a wrapped id is a reissued id.
    #[error("the store's id namespace is exhausted")]
    IdSpaceExhausted,
    /// The commit sequence space is exhausted. See [`RejectedReason::IdSpaceExhausted`].
    #[error("the store's commit sequence space is exhausted")]
    SequenceExhausted,
}

/// Which field of a copy's source disagrees with the child create record.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum CopySourceMismatch {
    /// The definition kind.
    Kind,
    /// The family key, or its absence.
    Key,
    /// The scope: a copy source must be a conversation document (`spec.md:4314`).
    Scope,
    /// The history policy.
    History,
    /// The fork policy.
    Fork,
}

impl fmt::Display for CopySourceMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Kind => "kind",
            Self::Key => "key",
            Self::Scope => "scope",
            Self::History => "history",
            Self::Fork => "fork",
        })
    }
}

/// Why a read could not answer.
///
/// Note what is **not** here: *"the record does not exist"*. Absence is `Ok(None)`, and that is the
/// whole point of the type (`spec.md:4352-4360`).
#[derive(Debug, thiserror::Error)]
pub enum StorageFailure {
    /// The stored data is damaged. **Fails the open** rather than returning less
    /// (`spec.md:4360`, `:4433`).
    #[error("storage is corrupt: {0}")]
    Corrupt(#[from] Corruption),
    /// The medium failed.
    #[error("storage i/o failed: {0}")]
    Io(#[from] io::Error),
    /// `spec.md:4356-4357`: *"A numeric lookup of a known current-only incarnation rejects rather
    /// than depending on reclaimed content."*
    ///
    /// Not absence and not corruption: the content was legitimately reclaimed because the document's
    /// persisted history policy does not retain it. Answering `Ok(None)` would say the incarnation
    /// was not there; answering from surviving records would guess.
    #[error("document {document} does not retain history: sequence {at} cannot be answered")]
    HistoryNotRetained {
        /// The incarnation asked about.
        document: DocumentId,
        /// The sequence asked about.
        at: Seq,
    },
}

/// What is wrong with the stored data.
///
/// `Serialize` only, per ADR-0030 §10: a corruption report is a diagnostic, not a record. A
/// `Deserialize` would let a damaged log line reintroduce a classification nobody made.
///
/// **Note the name.** ADR-0030 §10 writes `StorageFailure::Corrupt(Corruption)`, and this is that
/// `Corruption` — the storage-level one. [`cyrup_pico_doc::Corruption`] is the *replay*-level one,
/// the three arms [`cyrup_pico_doc::ReplayPlan::parse`] produces, and it arrives here inside
/// [`Corruption::Replay`]. The two are deliberately not one type: the replay arms are facts about a
/// record set and are decided by a pure function with no storage, which is what makes them unit
/// testable with literal values.
#[derive(Debug, Serialize, thiserror::Error)]
pub enum Corruption {
    /// A document's stored content records cannot be replayed.
    #[error("document {document}: {source}")]
    Replay {
        /// The incarnation.
        document: DocumentId,
        /// Which of `ReplayPlan::parse`'s arms fired.
        source: cyrup_pico_doc::Corruption,
    },
    /// `spec.md:99-100`: commit sequences strictly increase across the store's life, including
    /// reopen.
    ///
    /// ADR-0030 §2.2 classifies this **checked** and says why it cannot be lifted: it crosses a
    /// process boundary. A decoded non-increasing sequence is corruption and must fail the open
    /// rather than be normalised (`memory.ts:253`, `jsonl/storage.ts:543-546`).
    #[error("commit sequence {found} does not follow {previous}")]
    SequenceNotIncreasing {
        /// The sequence already seen.
        previous: Seq,
        /// The sequence read next.
        found: Seq,
    },
    /// A record was filed under an index for a different kind of id.
    ///
    /// ADR-0030 F6 §A names this as the residue the type system cannot remove: *"the number carries
    /// no tag"*, so a record read from the task index deserialises its id as `Id<Task>` because that
    /// is the field's type. A damaged file that files an entry id under tasks is caught here.
    #[error("id {id} is filed under the {found} index but is owned by a {owner} record")]
    IdFiledUnderWrongKind {
        /// The number.
        id: RawId,
        /// The index it was found in.
        found: IdKindTag,
        /// The kind that owns it.
        owner: IdKindTag,
    },
    /// A stored record could not be decoded.
    ///
    /// The message is rendered rather than typed: a decode error is the format's, not the domain's,
    /// and a `serde` error is neither `Serialize` nor a closed set.
    #[error("a stored {what} record could not be decoded: {detail}")]
    RecordMalformed {
        /// Which table or sidecar.
        what: &'static str,
        /// The decoder's message.
        detail: String,
    },
    /// A record a surviving marker requires is not present.
    ///
    /// `spec.md:4433`: *"Missing required confirmed data is corruption and opening fails."* The arm
    /// lives here rather than in the JSONL crate because the memory backend can hit it too, through
    /// a reclamation that an authorising base did not justify.
    #[error("required confirmed data is missing: {what}")]
    MissingConfirmedData {
        /// What is missing.
        what: String,
    },
}

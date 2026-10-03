//! The keyed commit batch (ADR-0030 F3; `spec.md:4250-4270`, `:4362-4367`).
//!
//! # What this replaces, and what it deletes
//!
//! Upstream a commit is `readonly StorageWrite[]` — *"a sequence of independent writes"* — over which
//! `spec.md:4362-4367` imposes rules the representation cannot express. ADR-0030 F3 lists the
//! consequence: **all three backends re-validate every one of them by hand**
//! (`memory.ts:689-695, 716-759`, SQLite's `checkDocumentActions()`), including the rule
//! `spec.md:4363` has to impose on a representation that *has* an order: *"Storage applies content
//! before retirement independent of write-array order."*
//!
//! Five keyed maps and a four-variant command delete that. Of F3's five illegal states, four stop
//! being shapes that exist:
//!
//! | illegal state | why it cannot be written |
//! |---|---|
//! | two content commands for one incarnation | one map key, one value |
//! | an incarnation retired twice | [`Retire`] is a choice, not a count |
//! | a delta with no base | [`DocumentCommand::Create`] and [`DocumentCommand::Copy`] take a [`DocumentBase`], never a [`DocumentContent`] |
//! | a dependence on write order | there is no array — see `tests/compile-fail/there_is_no_ordered_write_list.rs` |
//!
//! The fifth, *"the address already has a live incarnation"*, is a fact about **already-committed**
//! records and stays a backend check ([`crate::RejectedReason::AddressOccupied`]). So does global id
//! ownership. ADR-0030 F3 is explicit that those two do not lift, and they are the two cross-batch
//! rejections the conformance suite covers.
//!
//! # `BTreeMap`, and the reason it is not `HashMap`
//!
//! ADR-0030 F3: *"`BTreeMap` — not `HashMap`, not `IndexMap` — so iteration is deterministic by id
//! and the JSONL marker and SQL statement order are byte-reproducible, which the conformance suite
//! depends on."* A `HashMap` would make two runs of the same commit produce two different files.
//!
//! # No serde, in either direction
//!
//! ADR-0030 §10's serde table withholds both from [`Batch`]: *"an in-process assembly product. A
//! derived `Deserialize` would reintroduce every illegal state through a map with duplicate keys
//! silently last-wins, which is the quiet version of F3's bug."* [`DocumentCommand`] and
//! [`DocumentContent`] have none either, and that is this slice's one departure from the table — see
//! [`DocumentContent`]'s own documentation for why it is forced rather than chosen.
//!
//! What a backend persists is [`cyrup_pico_doc::StoredContent`] (via
//! [`DocumentContent::to_stored`]) and the [`DocumentRecord`](crate::DocumentRecord) — the two shapes
//! that already have an audited `Deserialize`.

use std::collections::{BTreeMap, btree_map};

use cyrup_pico_doc::{DocRoot, OpBatch, StoredContent, StoredVersion};

use crate::id::IdKind;
use crate::records::{
    ConversationRecord, DocumentCreate, EntryRecord, SubmissionRecord, TaskRecord,
};
use crate::{
    ConversationId, DefVersion, DocumentId, DocumentPoint, EntryId, Id, RawId, SubmissionId, TaskId,
};

/// Whether a content command also retires the incarnation it writes.
///
/// `spec.md:4362-4363`: *"One normalized batch contains at most one create/change content command per
/// incarnation and **may also retire that incarnation**."* Riding inside the command is what makes the
/// ordering rule unnecessary: there is no second write to sequence against, so *"content before
/// retirement"* is structural rather than imposed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Retire {
    /// Leave the incarnation live.
    Keep,
    /// Retire it at this batch's sequence.
    Retire,
}

impl Retire {
    /// Whether this is a retirement.
    #[must_use]
    pub const fn is_retire(self) -> bool {
        matches!(self, Self::Retire)
    }
}

/// A complete document value at a definition version.
///
/// `spec.md:1374`: a creation always stores one, and so does every required version transition. The
/// type is what makes *"a delta with no base"* unspellable — [`DocumentCommand::Create`] takes a
/// `DocumentBase`, not a [`DocumentContent`], so there is no creation that could carry operations.
#[derive(Clone, PartialEq, Debug)]
pub struct DocumentBase {
    /// The definition version that wrote it.
    pub version: DefVersion,
    /// The complete value.
    pub value: DocRoot,
}

impl DocumentBase {
    /// This base as the persisted content record.
    #[must_use]
    pub fn to_stored(&self) -> StoredContent {
        StoredContent::Base {
            version: self.version,
            value: self.value.clone(),
        }
    }
}

/// What one commit writes to one incarnation's content.
///
/// # Why this type has no `Deserialize`, and no `Serialize` either
///
/// ADR-0030 §10's serde table gives `DocumentContent` *"yes, audited with `StoredVersion`"* in both
/// columns, and two rows above gives [`StoredVersion`] **no** in both — *"an in-process witness. A
/// `Deserialize` would let a recovered record mint a version claim."* Both cannot hold: a derived impl
/// for this enum requires one for the witness inside it.
///
/// The witness is the half worth keeping, because it is the half that buys a guarantee: F6 §C's
/// *"a `Delta { continues }` cannot claim a version it did not read"*. So serde is withheld here and
/// the **persisted** shape is [`cyrup_pico_doc::StoredContent`], which has the audited `Deserialize`
/// and which [`DocumentContent::to_stored`] produces. Nothing is lost: a recovered delta's version is
/// checked by [`cyrup_pico_doc::ReplayPlan::parse`], which is where `spec.md:4366-4367`'s rule already
/// lives. The discrepancy is recorded against ADR-0030 rather than resolved silently.
#[derive(Clone, PartialEq, Debug)]
pub enum DocumentContent {
    /// A complete replacement value.
    Base(DocumentBase),
    /// An ordered operation batch extending the base it was prepared against.
    Delta {
        /// The version this delta continues.
        ///
        /// Not a number the assembler picks: [`StoredVersion`] is minted only by
        /// [`cyrup_pico_doc::ReplayPlan::parse`], so holding one is proof that a storage read produced
        /// it. A **stale** witness is still possible — a later commit can move the document's stored
        /// version — and that residue is [`crate::RejectedReason::VersionTransitionRequiresBase`], checked by
        /// the backend against committed state.
        continues: StoredVersion,
        /// The operations, in order. `spec.md:4350` makes a delta tail ordered, which is why this
        /// stays a sequence while the batch around it does not.
        ops: OpBatch,
    },
}

impl DocumentContent {
    /// The version this content is written at.
    #[must_use]
    pub const fn version(&self) -> DefVersion {
        match self {
            Self::Base(base) => base.version,
            Self::Delta { continues, .. } => continues.version(),
        }
    }

    /// Whether this content is a complete base.
    #[must_use]
    pub const fn is_base(&self) -> bool {
        matches!(self, Self::Base(_))
    }

    /// This content as the persisted record.
    #[must_use]
    pub fn to_stored(&self) -> StoredContent {
        match self {
            Self::Base(base) => base.to_stored(),
            Self::Delta { continues, ops } => StoredContent::Delta {
                version: continues.version(),
                ops: ops.clone(),
            },
        }
    }
}

/// The source of a document copy (`spec.md:4256-4259`, `:4313-4318`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CopySource {
    /// The incarnation to copy from.
    pub id: DocumentId,
    /// The point to read it at. The read is of **committed pre-batch** state, whatever this batch
    /// does — and `spec.md:4316` forbids the batch from touching the source at all
    /// ([`crate::RejectedReason::CopySourceInBatch`]).
    pub at: DocumentPoint,
}

/// What one commit does to one incarnation.
///
/// Four variants, and the shape of each is the argument: content and retirement are **one value**, so
/// there is no order between them to get wrong and no way to say *"content twice"* or
/// *"retire twice"*.
#[derive(Clone, PartialEq, Debug)]
pub enum DocumentCommand {
    /// Create an incarnation with a complete base.
    Create {
        /// The record to stamp.
        record: DocumentCreate,
        /// Its first content, which is always complete (`spec.md:1374`).
        base: DocumentBase,
        /// Whether the same commit retires it, giving the empty lifetime of `spec.md:1114`.
        then: Retire,
    },
    /// Create an incarnation from a copy of another's committed value (`spec.md:4313-4318`).
    Copy {
        /// The record to stamp.
        record: DocumentCreate,
        /// What to copy. Storage persists *"one independent complete child base at the source's stored
        /// version"* — a value, never a reference, which is what makes the child immune to the
        /// source's later changes, reclamation, retirement and reopen.
        source: CopySource,
        /// Whether the same commit retires the child.
        then: Retire,
    },
    /// Write content to an existing incarnation.
    Change {
        /// The content.
        content: DocumentContent,
        /// Whether the same commit retires it. This is `spec.md:1246-1247`'s
        /// *"retiring an acquired draft persists its final content before retirement"* — one value, so
        /// the retirement cannot win the race against the content it is supposed to follow.
        then: Retire,
    },
    /// Retire an existing incarnation without writing content.
    RetireOnly,
}

impl DocumentCommand {
    /// Whether this command retires the incarnation.
    #[must_use]
    pub const fn retires(&self) -> bool {
        match self {
            Self::Create { then, .. } | Self::Copy { then, .. } | Self::Change { then, .. } => {
                then.is_retire()
            }
            Self::RetireOnly => true,
        }
    }

    /// Whether this command creates the incarnation.
    #[must_use]
    pub const fn creates(&self) -> bool {
        matches!(self, Self::Create { .. } | Self::Copy { .. })
    }

    /// The record this command creates, if it creates one.
    #[must_use]
    pub const fn create_record(&self) -> Option<&DocumentCreate> {
        match self {
            Self::Create { record, .. } | Self::Copy { record, .. } => Some(record),
            Self::Change { .. } | Self::RetireOnly => None,
        }
    }
}

/// One admitted commit: at most one thing done to each record, keyed by id.
///
/// Private fields and no public constructor. The only way to build one is [`BatchBuilder`], and the
/// only way to take it apart is [`Batch::into_parts`] — which is a decomposition, not a round trip:
/// there is no `Batch::from_parts`.
///
/// **Non-empty by construction.** [`BatchBuilder::build`] answers `None` when nothing was staged, so
/// a backend is never asked to allocate a commit sequence for a batch with nothing in it. That is why
/// [`Batch::is_empty`] exists only to be documented as always `false`.
#[derive(Debug)]
pub struct Batch {
    conversations: BTreeMap<ConversationId, ConversationRecord>,
    entries: BTreeMap<EntryId, EntryRecord>,
    tasks: BTreeMap<TaskId, TaskRecord>,
    submissions: BTreeMap<SubmissionId, SubmissionRecord>,
    documents: BTreeMap<DocumentId, DocumentCommand>,
}

/// The five maps of a batch, after [`Batch::into_parts`].
///
/// A backend applies a commit by consuming these. The fields are public because a backend owns them
/// once it has them, and nothing is weakened by that: every invariant the keyed batch buys is a
/// property of *a map keyed by id*, which this still is. What a backend cannot do is turn these back
/// into a [`Batch`].
#[derive(Debug)]
pub struct BatchParts {
    /// Conversation records, by id.
    pub conversations: BTreeMap<ConversationId, ConversationRecord>,
    /// Entry records, by id.
    pub entries: BTreeMap<EntryId, EntryRecord>,
    /// Task records, by id.
    pub tasks: BTreeMap<TaskId, TaskRecord>,
    /// Submission records, by id.
    pub submissions: BTreeMap<SubmissionId, SubmissionRecord>,
    /// Document commands, by incarnation.
    pub documents: BTreeMap<DocumentId, DocumentCommand>,
}

impl Batch {
    /// The conversation records, in id order.
    #[must_use]
    pub const fn conversations(&self) -> &BTreeMap<ConversationId, ConversationRecord> {
        &self.conversations
    }

    /// The entry records, in id order.
    #[must_use]
    pub const fn entries(&self) -> &BTreeMap<EntryId, EntryRecord> {
        &self.entries
    }

    /// The task records, in id order.
    #[must_use]
    pub const fn tasks(&self) -> &BTreeMap<TaskId, TaskRecord> {
        &self.tasks
    }

    /// The submission records, in id order.
    #[must_use]
    pub const fn submissions(&self) -> &BTreeMap<SubmissionId, SubmissionRecord> {
        &self.submissions
    }

    /// The document commands, in incarnation order.
    #[must_use]
    pub const fn documents(&self) -> &BTreeMap<DocumentId, DocumentCommand> {
        &self.documents
    }

    /// How many records and commands this batch holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.conversations.len()
            + self.entries.len()
            + self.tasks.len()
            + self.submissions.len()
            + self.documents.len()
    }

    /// Always `false`.
    ///
    /// A [`Batch`] is non-empty by construction ([`BatchBuilder::build`] answers `None` instead), so
    /// this exists to say so in one place rather than to be branched on.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Take the batch apart for application.
    #[must_use]
    pub fn into_parts(self) -> BatchParts {
        BatchParts {
            conversations: self.conversations,
            entries: self.entries,
            tasks: self.tasks,
            submissions: self.submissions,
            documents: self.documents,
        }
    }
}

/// A record or command was staged twice in one batch.
///
/// The in-batch half of `spec.md:4274`'s *"is written more than once"*. It is an error rather than a
/// silent last-wins because last-wins is precisely ADR-0030 F3's second failure mode: *"`assemble()`
/// pushes a `document.change` in a loop over open changes, the loop runs twice for one incarnation
/// after a memoisation bug"*. A map makes the outcome deterministic; returning it makes the bug
/// visible.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error("{kind} {id} is already staged in this batch")]
pub struct AlreadyStaged {
    /// Which table.
    pub kind: crate::IdKindTag,
    /// The id staged twice.
    pub id: RawId,
}

/// Assembles one [`Batch`].
///
/// The assembler is the only construction path, which is what ADR-0030 F3 means by *"built only by
/// the assembler"*. Two properties are worth naming because they are enforced here rather than
/// checked later:
///
/// 1. **A key cannot disagree with its record.** Every method derives the map key from the record's
///    own `id`, so there is no parameter pair to get wrong — and for a document creation the key comes
///    from [`DocumentCreate::id`] for the same reason.
/// 2. **A second stage is an error, not an overwrite.** See [`AlreadyStaged`].
#[derive(Debug, Default)]
pub struct BatchBuilder {
    conversations: BTreeMap<ConversationId, ConversationRecord>,
    entries: BTreeMap<EntryId, EntryRecord>,
    tasks: BTreeMap<TaskId, TaskRecord>,
    submissions: BTreeMap<SubmissionId, SubmissionRecord>,
    documents: BTreeMap<DocumentId, DocumentCommand>,
}

impl BatchBuilder {
    /// An empty assembler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stage a conversation record.
    ///
    /// # Errors
    ///
    /// [`AlreadyStaged`] when this batch already holds a conversation with that id.
    pub fn conversation(&mut self, record: ConversationRecord) -> Result<&mut Self, AlreadyStaged> {
        stage(&mut self.conversations, record.id, record)?;
        Ok(self)
    }

    /// Stage an entry record.
    ///
    /// # Errors
    ///
    /// [`AlreadyStaged`] when this batch already holds an entry with that id.
    pub fn entry(&mut self, record: EntryRecord) -> Result<&mut Self, AlreadyStaged> {
        stage(&mut self.entries, record.id, record)?;
        Ok(self)
    }

    /// Stage a task record.
    ///
    /// # Errors
    ///
    /// [`AlreadyStaged`] when this batch already holds a task with that id.
    pub fn task(&mut self, record: TaskRecord) -> Result<&mut Self, AlreadyStaged> {
        stage(&mut self.tasks, record.id, record)?;
        Ok(self)
    }

    /// Stage a submission record.
    ///
    /// # Errors
    ///
    /// [`AlreadyStaged`] when this batch already holds a submission with that id.
    pub fn submission(&mut self, record: SubmissionRecord) -> Result<&mut Self, AlreadyStaged> {
        stage(&mut self.submissions, record.id, record)?;
        Ok(self)
    }

    /// Stage a document creation, with its complete first base.
    ///
    /// # Errors
    ///
    /// [`AlreadyStaged`] when this batch already holds a command for that incarnation.
    pub fn create_document(
        &mut self,
        record: DocumentCreate,
        base: DocumentBase,
        then: Retire,
    ) -> Result<&mut Self, AlreadyStaged> {
        let id = record.id;
        stage(
            &mut self.documents,
            id,
            DocumentCommand::Create { record, base, then },
        )?;
        Ok(self)
    }

    /// Stage a document copy.
    ///
    /// # Errors
    ///
    /// [`AlreadyStaged`] when this batch already holds a command for that incarnation.
    pub fn copy_document(
        &mut self,
        record: DocumentCreate,
        source: CopySource,
        then: Retire,
    ) -> Result<&mut Self, AlreadyStaged> {
        let id = record.id;
        stage(
            &mut self.documents,
            id,
            DocumentCommand::Copy {
                record,
                source,
                then,
            },
        )?;
        Ok(self)
    }

    /// Stage content for an existing incarnation, optionally retiring it.
    ///
    /// # Errors
    ///
    /// [`AlreadyStaged`] when this batch already holds a command for that incarnation.
    pub fn change_document(
        &mut self,
        id: DocumentId,
        content: DocumentContent,
        then: Retire,
    ) -> Result<&mut Self, AlreadyStaged> {
        stage(
            &mut self.documents,
            id,
            DocumentCommand::Change { content, then },
        )?;
        Ok(self)
    }

    /// Stage a retirement with no content.
    ///
    /// # Errors
    ///
    /// [`AlreadyStaged`] when this batch already holds a command for that incarnation — including a
    /// content command, because retiring *with* content is
    /// [`BatchBuilder::change_document`] with [`Retire::Retire`] and not a second command.
    pub fn retire_document(&mut self, id: DocumentId) -> Result<&mut Self, AlreadyStaged> {
        stage(&mut self.documents, id, DocumentCommand::RetireOnly)?;
        Ok(self)
    }

    /// Finish the batch.
    ///
    /// `None` when nothing was staged: there is no empty [`Batch`], so no backend can be asked to
    /// allocate a commit sequence for nothing. ADR-0030 F1's `CommitOutcome::NothingToCommit` is the
    /// kernel-side name for that case (PICO5-PLAN S4); this is the storage-side shape that makes it
    /// unavoidable.
    #[must_use]
    pub fn build(self) -> Option<Batch> {
        let batch = Batch {
            conversations: self.conversations,
            entries: self.entries,
            tasks: self.tasks,
            submissions: self.submissions,
            documents: self.documents,
        };
        if batch.is_empty() { None } else { Some(batch) }
    }
}

/// Insert into one of the five maps, rejecting a second stage of the same id.
///
/// Generic over the id kind rather than written five times, which also means the key type *is* the
/// table: there is no call site at which an entry id could be staged into the task map.
fn stage<K: IdKind, V>(
    map: &mut BTreeMap<Id<K>, V>,
    key: Id<K>,
    value: V,
) -> Result<(), AlreadyStaged> {
    match map.entry(key) {
        btree_map::Entry::Vacant(slot) => {
            slot.insert(value);
            Ok(())
        }
        btree_map::Entry::Occupied(_) => Err(AlreadyStaged {
            kind: key.kind(),
            id: RawId::from(key),
        }),
    }
}

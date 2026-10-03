//! Forks: one visible entry, one commit, and the policy the **persisted record** carries.
//!
//! PICO5-PLAN **S9**. `spec.md` §3.7 is four paragraphs and they are four separate obligations, so
//! this module is written against them one at a time:
//!
//! 1. *"A conversation fork points to one concrete visible entry `E`. … Document state at `E` is the
//!    final state of the commit containing `E`."* ([`Tx::fork_conversation`] takes one
//!    [`ConversationParent`] and resolves it through [`Storage::entry_in`], which applies every
//!    `parent.at` ancestry cap, and keeps the **commit sequence** that entry landed in —
//!    `G-FORK-POINT-ONE-ENTRY`.)
//! 2. *"Each conversation document follows the history/fork policy persisted in its
//!    `DocumentRecord`."* (The policy is read from [`DocumentRecord::scope`] and nowhere else. There
//!    is no [`DocDef`](crate::DocDef), no [`DocToken`](crate::DocToken) and no
//!    [`DocAddress`](crate::DocAddress) anywhere in the fork path, so a token *cannot* reach it —
//!    `G-FORK-POLICY-PERSISTED`.)
//! 3. *"A transaction that creates a fork therefore rejects if it also writes one of the parent's
//!    `fork: "current"` documents."* ([`Fork`]'s copies are checked against the assembled batch in
//!    `Tx::prepare`, which is **before storage admission**, so the rejection is a
//!    [`RollbackReason`](crate::RollbackReason) and the Session stays usable.)
//! 4. *"Every copy reads committed pre-batch source state independent of write-array order … Storage
//!    materializes each source and persists its stored value/version as the child's independent
//!    initial base."* (That half is the **storage** obligation and landed with S3/S7:
//!    [`DocumentCommand::Copy`] and [`RejectedReason::CopySourceInBatch`]. What is here is the
//!    Session half — `G-COPY-SOURCE-SNAPSHOT-ISOLATION`.)
//!
//! # Why order-independence is not a check here
//!
//! ADR-0030 §2.3 states the failure the rule prevents: *"without it, the child's value depends on
//! whether the fork command was ordered before or after the parent's write — the same program
//! producing two different children."* Under ADR-0030 F3 there is no write array: a [`Batch`] is five
//! `BTreeMap`s, so *"the copy command assembled before the parent's write"* and *"after"* are not two
//! batches, they are one. The order-dependence is therefore gone from the representation and all that
//! is left of the problem is the cross-record rule in point 3 — which is why PICO5-PLAN S9 asks for
//! *"one canary plus a behaviour test"* rather than a pair of differing-order cases.
//!
//! # What is deliberately absent
//!
//! No fork copies a **task** document and no fork copies a task. That is not a filter: the
//! enumeration is [`Storage::scan_documents`] over one exact
//! [`ScopeRef::Conversation`](cyrup_pico_store::ScopeRef::Conversation), and `spec.md:4343` makes
//! that method *"enumerate only the incarnations alive in one exact scope"* — a task document lives in
//! [`ScopeRef::Task`](cyrup_pico_store::ScopeRef::Task) and a session document in
//! [`ScopeRef::Session`](cyrup_pico_store::ScopeRef::Session), so neither is in the answer. Session
//! documents *"remain shared and are not rewindable"* for the same structural reason.
//!
//! [`ConversationParent`]: cyrup_pico_store::ConversationParent
//! [`Storage::entry_in`]: cyrup_pico_store::Storage::entry_in
//! [`Storage::scan_documents`]: cyrup_pico_store::Storage::scan_documents
//! [`DocumentRecord::scope`]: cyrup_pico_store::DocumentRecord::scope
//! [`DocumentCommand::Copy`]: cyrup_pico_store::DocumentCommand::Copy
//! [`RejectedReason::CopySourceInBatch`]: cyrup_pico_store::RejectedReason::CopySourceInBatch
//! [`Batch`]: cyrup_pico_store::Batch
//! [`Tx::fork_conversation`]: crate::Tx::fork_conversation

use cyrup_pico_store::{
    ConversationId, ConversationRecord, CopySource, DocumentAddress, DocumentCreate, DocumentId,
    Retire, RewindableFork,
};

/// What one [`Tx::fork_conversation`](crate::Tx::fork_conversation) call decided.
///
/// The child conversation record, and one [`ForkedDocument`] per conversation document the parent had
/// — including the ones the fork deliberately did **not** copy, because
/// `fork: "initial"`'s *"no copied instance; initializer on first child access"* is a decision and a
/// caller that cannot see it cannot tell it apart from a document the fork missed.
#[derive(Clone, PartialEq, Debug)]
pub struct Fork {
    conversation: ConversationRecord,
    documents: Vec<ForkedDocument>,
}

impl Fork {
    /// Assemble the outcome. Crate-private: a `Fork` exists because a transaction forked.
    pub(crate) const fn new(
        conversation: ConversationRecord,
        documents: Vec<ForkedDocument>,
    ) -> Self {
        Self {
            conversation,
            documents,
        }
    }

    /// The child conversation record this fork staged.
    #[must_use]
    pub const fn conversation(&self) -> &ConversationRecord {
        &self.conversation
    }

    /// The child conversation's id.
    #[must_use]
    pub const fn child(&self) -> ConversationId {
        self.conversation.id
    }

    /// What the fork decided about each of the parent's conversation documents, by ascending source
    /// incarnation id within each policy group.
    #[must_use]
    pub fn documents(&self) -> &[ForkedDocument] {
        &self.documents
    }
}

/// What a fork decided about **one** of the parent's conversation documents.
///
/// # Why every field is private
///
/// [`ForkedDocument::policy`] is read from the parent's persisted
/// [`DocumentRecord`](cyrup_pico_store::DocumentRecord) and from nothing else
/// (`spec.md:1478-1479`). A struct literal would let a caller assert a policy the record does not
/// carry and hand it to code that then behaves as though the fork had honoured it — which is exactly
/// the *"the token reinterprets the record"* hazard `spec.md:1117-1120` exists to prevent, one layer
/// out. `tests/compile-fail/a_forked_document_cannot_be_forged.rs` is the canary.
#[derive(Clone, PartialEq, Debug)]
pub struct ForkedDocument {
    source: DocumentId,
    address: DocumentAddress,
    policy: RewindableFork,
    child: Option<DocumentId>,
}

impl ForkedDocument {
    /// Record one decision. Crate-private, which is the whole of this type's guarantee.
    pub(crate) const fn new(
        source: DocumentId,
        address: DocumentAddress,
        policy: RewindableFork,
        child: Option<DocumentId>,
    ) -> Self {
        Self {
            source,
            address,
            policy,
            child,
        }
    }

    /// The parent's incarnation this decision is about.
    #[must_use]
    pub const fn source(&self) -> DocumentId {
        self.source
    }

    /// That incarnation's logical address **in the parent**.
    #[must_use]
    pub const fn address(&self) -> &DocumentAddress {
        &self.address
    }

    /// The policy, as the parent's persisted record carries it.
    ///
    /// One enum for both histories: [`ConversationSemantics::fork`] widens
    /// [`LatestFork`] into [`RewindableFork`] in the one sound direction, so a fork matches on three
    /// variants rather than on a history and then a policy.
    ///
    /// [`ConversationSemantics::fork`]: cyrup_pico_store::ConversationSemantics::fork
    /// [`LatestFork`]: cyrup_pico_store::LatestFork
    #[must_use]
    pub const fn policy(&self) -> RewindableFork {
        self.policy
    }

    /// The child incarnation the fork minted, or `None` for [`RewindableFork::Initial`].
    ///
    /// `spec.md:1487-1489`: *"`initial` copies no instance; first access in the child creates it from
    /// the supplied definition."* So `None` is not a failure — it is the policy working.
    #[must_use]
    pub const fn child(&self) -> Option<DocumentId> {
        self.child
    }
}

/// One copy a fork staged, waiting for `Tx::prepare` or for a typed acquisition in the same
/// transaction.
///
/// # Why this is not pushed straight into the [`BatchBuilder`]
///
/// `spec.md:1499-1502`: *"An unaccessed copy retains only its descriptor in Session memory. Typed
/// access inside the creating transaction lazily reads the detached source, migrates when required,
/// and replaces the copy with one ordinary child create containing the final prepared value."* A
/// [`BatchBuilder`] has no removal path — deliberately, because F3 makes the assembler the only
/// builder — so a copy that is staged eagerly could not be *replaced*. Holding the descriptor here
/// and staging it in preparation is what makes the replacement a possibility rather than a second
/// command for one incarnation.
///
/// [`BatchBuilder`]: cyrup_pico_store::BatchBuilder
pub(crate) struct PendingCopy {
    /// The child record to stamp.
    pub(crate) create: DocumentCreate,
    /// The child's logical address, so a typed acquisition in this transaction can recognise it.
    pub(crate) address: DocumentAddress,
    /// The source incarnation and the point to read it at.
    pub(crate) source: CopySource,
    /// The policy that selected it, from the parent's persisted record.
    pub(crate) policy: RewindableFork,
    /// Whether the same transaction also retires the child.
    pub(crate) retire: Retire,
    /// Whether a typed acquisition replaced this copy with an ordinary create.
    pub(crate) consumed: bool,
}

impl PendingCopy {
    /// Whether writing this copy's source in the same batch must be rejected before admission.
    ///
    /// Two spec rules meet here and they do **not** have the same extent, so this is one function
    /// rather than one condition written twice:
    ///
    /// * `spec.md:1491-1494` — *"A transaction that creates a fork therefore rejects if it also
    ///   writes one of the parent's `fork: "current"` documents; commit the parent change first so
    ///   the fork has one unambiguous stored source revision."* That is about **which revision**
    ///   `current` means, so it holds whether or not the copy was later replaced by a create: the
    ///   ambiguity is in the program, not in the command.
    /// * `spec.md:4316` — *"A batch may not create, change, or retire a selected source."* That is
    ///   about a **selected** source, so it stops applying once a typed acquisition has replaced the
    ///   copy with an ordinary create and the batch no longer selects anything. For an `asOf` source
    ///   that is the honest answer: the parent's value *at the fork point* is historical and a write
    ///   now cannot change it.
    ///
    /// The backend enforces the second rule too ([`RejectedReason::CopySourceInBatch`]); doing it
    /// here as well is not duplication but the difference between a rejection the Session guarantees
    /// is pre-admission and one it has to trust a backend about (ADR-0030 §2.3's
    /// *"the Session rejects the conflicting transaction pre-admission"*).
    ///
    /// [`RejectedReason::CopySourceInBatch`]: cyrup_pico_store::RejectedReason::CopySourceInBatch
    pub(crate) const fn source_is_untouchable(&self) -> bool {
        matches!(self.policy, RewindableFork::Current) || !self.consumed
    }
}

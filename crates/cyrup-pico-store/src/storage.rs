//! The storage contract (`spec.md:4283-4331`; ADR-0030 F3, §10).
//!
//! # The signature ADR-0030 §9 calls irreversible
//!
//! `commit(&mut self, ..)`. `spec.md:4321-4323` states the property in prose — *"The Session owns the
//! mutation line, so storage implementations do not add a second caller-facing commit mutex"* — and
//! ADR-0030 §2.2 records what enforces it upstream: *"nothing. An ordinary method on a shared
//! interface; the property is documented of the caller and unverifiable by the callee."*
//!
//! `&mut self` makes it the callee's, and makes a second concurrent committer a **borrow error**:
//! `tests/compile-fail/one_committer_at_a_time.rs` is the proof. The read paths stay `&self`, so a
//! reader is not serialised behind a writer — and that split is also why ADR-0030 §5 rejects typestate
//! for a backend's poison flag: the flag must be readable from the `&self` paths.
//!
//! # Why the trust split is written into the trait's documentation
//!
//! `spec.md:4272-4277` divides the work: *"Trusts the owning Session to supply semantically valid
//! records, references, ancestry, and transitions. Enforces atomicity, global ID ownership, immutable
//! conversation/entry creation, document record consistency, and detached values; Session serializes
//! commits."* Every obligation on the storage side of that line is named on the method that owes it,
//! because the conformance suite tests exactly this split and nothing else in the tree states it.
//!
//! # What a trait cannot give, and what tests it instead
//!
//! Atomicity in the medium. ADR-0030 F3: *"one owned `Batch` means the Session cannot split a logical
//! change in two, and nothing stops a backend partially applying it — that is conformance plus fault
//! injection."* Likewise the honesty of a [`CommitError::Rejected`] classification, and whether a
//! named access path is answered from an index or by a scan. The conformance suite
//! (`cyrup_pico_store::conformance`, behind `feature = "conformance"`) is the contract for those,
//! which is `spec.md:4369`'s own position:
//! *"The semantic conformance suite covers memory, SQLite, and JSONL."*

use async_trait::async_trait;

use crate::batch::Batch;
use crate::cursor::{
    ConversationCursor, DocumentCursor, EntryCursor, SubmissionCursor, TaskCursor,
};
use crate::error::{CommitError, StorageFailure};
use crate::id::{Id, IdKind, IdKindTag, RawId};
use crate::query::{
    CommittedEntry, ConversationQuery, DocumentQuery, EntryQuery, Page, StoredDocument,
    SubmissionQuery, TaskQuery,
};
use crate::records::{
    ConversationRecord, DocumentAddress, DocumentRecord, EntryRecord, HeadMarker, SubmissionRecord,
    TaskRecord,
};
use crate::{
    ConversationId, Cx, DocumentId, DocumentPoint, EntryId, PageLimit, Seq, StoreId, SubmissionId,
    TaskId,
};

/// One durable store.
///
/// Implemented by [`MemoryStore`](crate::MemoryStore) as §11.1's reference semantics, by
/// `cyrup-pico-store-jsonl` (PICO5-PLAN S7) and, if ADR-0030 §9's trigger fires, by
/// `cyrup-pico-store-sqlite` (S12).
///
/// `#[async_trait]` rather than a native `async fn`, for one concrete reason: a native async method is
/// not dyn-compatible, and the conformance suite drives a `&mut dyn Storage`. ADR-0030 §8's first
/// load-bearing reason for the crate split is that *"the conformance suite must not depend on the
/// Session"*, and a suite that could not hold a trait object would need a generic parameter threaded
/// through every case.
#[async_trait]
pub trait Storage: Send {
    /// This store's durable identity.
    ///
    /// Stamped into every cursor this store issues, so a cursor presented to a different store is
    /// caught rather than resumed at a position meaning something else
    /// ([`CursorBytes::payload_for`](crate::CursorBytes::payload_for)).
    fn store_id(&self) -> StoreId;

    /// Commit one batch atomically, returning its sequence.
    ///
    /// # Obligations
    ///
    /// * **Atomicity** across every record table and every document command (`spec.md:63`,
    ///   invariant 1). Partial application is corruption no later repair can resolve.
    /// * **A strictly increasing sequence**, gaps permitted, surviving reopen (`spec.md:99-100`).
    /// * **Content before retirement**, independent of anything (`spec.md:4363`) — which the keyed
    ///   batch already makes structural, since a [`DocumentCommand`](crate::DocumentCommand) carries
    ///   both as one value.
    /// * **Read-your-commit**: *"Once `commit()` resolves, later reads through that Storage observe
    ///   it"* (`spec.md:4281`).
    /// * **Detachment**: nothing retained may alias the caller's memory, and nothing returned by a
    ///   read may alias the store's own indexes (`spec.md:4375-4378`).
    /// * **The cross-batch checks the batch shape cannot lift**: global id ownership against committed
    ///   records, immutable conversation and entry creation, address occupancy, and a delta whose
    ///   witnessed version is stale. Each has a closed [`RejectedReason`](crate::RejectedReason).
    ///
    /// # Errors
    ///
    /// [`CommitError::Rejected`] **only** when nothing durable happened and rollback is guaranteed
    /// (`spec.md:4310-4311`). Everything else is [`CommitError::Uncertain`], which is fatal to the
    /// Session that issued it. There is deliberately no `From<std::io::Error>` for [`CommitError`]:
    /// the choice cannot be made by `?`.
    async fn commit(&mut self, batch: Batch, cx: &Cx) -> Result<Seq, CommitError>;

    /// Allocate one id from the store's single global namespace (`spec.md:4282`).
    ///
    /// `&mut self` for the same reason as [`Storage::commit`]: allocation moves a durable high-water
    /// mark. [`StorageExt::mint`] is the typed form callers use; this is the dyn-compatible primitive it
    /// is built on, and a backend implements only this one.
    ///
    /// # What `kind` is, and what it must not become
    ///
    /// It is the caller's intent, available here because this is the one call that knows it, and a
    /// backend may use it for a log line or a metric. It is **not** an ownership record. ADR-0030 F6 §A
    /// is explicit — *"do not build a separate durable number→kind table; derive ownership from the
    /// per-table indexes, which exist anyway"* — and the reason is a fact about this method: a minted
    /// number is not owned by anything until a committed record carries it, so a table written here
    /// would claim an ownership that a batch may never establish, and would then need reconciling with
    /// the records themselves. The ownership check belongs in [`Storage::commit`]
    /// ([`RejectedReason::IdAlreadyOwned`](crate::RejectedReason::IdAlreadyOwned)).
    ///
    /// # Obligations
    ///
    /// Durable and monotone, including across a crash between a mint and the commit that uses the
    /// minted id: *"ids come from one durable monotone global namespace"* (`spec.md:4282`). An id is
    /// never reissued, so this is fallible rather than wrapping
    /// ([`RejectedReason::IdSpaceExhausted`](crate::RejectedReason::IdSpaceExhausted)).
    ///
    /// # Errors
    ///
    /// [`CommitError`], because a failure to persist the high-water mark has the same two classes as
    /// a commit: a rejected allocation left nothing behind, an uncertain one may have.
    async fn mint_raw(&mut self, kind: IdKindTag, cx: &Cx) -> Result<RawId, CommitError>;

    /// Release the store.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`] when a final flush fails. A caller that has already published committed
    /// state cannot un-publish it, so this reports rather than poisons.
    async fn close(&mut self, cx: &Cx) -> Result<(), StorageFailure>;

    /// One conversation by id.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    async fn conversation(
        &self,
        id: ConversationId,
        cx: &Cx,
    ) -> Result<Option<ConversationRecord>, StorageFailure>;

    /// Page conversations by indexed, conjunctive owner filters (`spec.md:4337-4339`).
    ///
    /// # Obligations
    ///
    /// Indexed. *"They support ownership traversal without an all-conversation scan;
    /// application-maintained registries are not a substitute for these kernel indexes."*
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    async fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: PageLimit,
        from: Option<ConversationCursor>,
        cx: &Cx,
    ) -> Result<Page<ConversationRecord, ConversationCursor>, StorageFailure>;

    /// One entry by id, with the commit sequence (`spec.md:4347`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    async fn entry(&self, id: EntryId, cx: &Cx) -> Result<Option<CommittedEntry>, StorageFailure>;

    /// One entry by id, **only if visible through `conversation_id`'s ancestry** (`spec.md:4348`).
    ///
    /// A separate method rather than a flag, because the two answer different questions and
    /// `spec.md:4347-4348` gives them different contracts: this one applies every `parent.at` cap on
    /// the way up.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Not visible is `Ok(None)`, not an error: an entry outside the requested
    /// ancestry is legitimately absent *from that conversation*.
    async fn entry_in(
        &self,
        conversation_id: ConversationId,
        id: EntryId,
        cx: &Cx,
    ) -> Result<Option<CommittedEntry>, StorageFailure>;

    /// The newest visible entry carrying a head at or before a cutoff (`spec.md:4331-4334`).
    ///
    /// # Obligations
    ///
    /// Indexed, and ancestry-capped. The returned entry *is* the marker; its `head` is the lower bound
    /// — which is why the return type carries both ([`HeadMarker`]).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    async fn find_latest_head_marker(
        &self,
        conversation_id: ConversationId,
        at_or_before: Option<EntryId>,
        cx: &Cx,
    ) -> Result<Option<HeadMarker>, StorageFailure>;

    /// Page one conversation's visible history, newest first (`spec.md:4334-4336`).
    ///
    /// # Obligations
    ///
    /// Inclusive id bounds; newest-first; **every ancestry cap applied inside**. With no bounds it
    /// pages complete visible history.
    ///
    /// The items are bare records, not [`CommittedEntry`]s, because `spec.md:4336` pages
    /// `Page<EntryRecord, Cursor>`: the commit sequence belongs to [`Storage::entry`], which
    /// `spec.md:4347` says exists precisely to *"combine exact global lookup with the commit sequence
    /// required by historical document reads"*. Attaching it to every scanned item would make every
    /// page carry a field only the one lookup needs.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    async fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: PageLimit,
        from: Option<EntryCursor>,
        cx: &Cx,
    ) -> Result<Page<EntryRecord, EntryCursor>, StorageFailure>;

    /// One task by id.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    async fn task(&self, id: TaskId, cx: &Cx) -> Result<Option<TaskRecord>, StorageFailure>;

    /// Page tasks by the five indexed fields (`spec.md:4348`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    async fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: PageLimit,
        from: Option<TaskCursor>,
        cx: &Cx,
    ) -> Result<Page<TaskRecord, TaskCursor>, StorageFailure>;

    /// One submission by id.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    async fn submission(
        &self,
        id: SubmissionId,
        cx: &Cx,
    ) -> Result<Option<SubmissionRecord>, StorageFailure>;

    /// Page submissions.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    async fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: PageLimit,
        from: Option<SubmissionCursor>,
        cx: &Cx,
    ) -> Result<Page<SubmissionRecord, SubmissionCursor>, StorageFailure>;

    /// One submission by its caller-supplied request id (`spec.md:4314`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    async fn submission_by_request(
        &self,
        conversation_id: ConversationId,
        request_id: &str,
        cx: &Cx,
    ) -> Result<Option<SubmissionRecord>, StorageFailure>;

    /// Resolve one exact logical document address at a point (`spec.md:4340-4342`).
    ///
    /// # Obligations
    ///
    /// Exact. *"A missing key means the singleton, not every family member."*
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. No incarnation alive at that point is `Ok(None)`.
    async fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
        cx: &Cx,
    ) -> Result<Option<DocumentRecord>, StorageFailure>;

    /// Materialize one specific incarnation (`spec.md:4345-4360`).
    ///
    /// # Obligations
    ///
    /// Never follows a replacement at the same address. Selects the newest applicable base and applies
    /// its ordered delta tail. An unknown id is `Ok(None)`; at
    /// [`DocumentPoint::Current`] a retired incarnation is `Ok(None)`;
    /// a numeric point outside a rewindable incarnation's half-open lifetime is `Ok(None)`; a numeric
    /// point on a **current-only** incarnation is
    /// [`StorageFailure::HistoryNotRetained`], *not* a
    /// guess over whatever survived reclamation. A missing required base, a version change inside a
    /// delta tail, or an inapplicable operation is
    /// [`StorageFailure::Corrupt`].
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    async fn document(
        &self,
        id: DocumentId,
        at: DocumentPoint,
        cx: &Cx,
    ) -> Result<Option<StoredDocument>, StorageFailure>;

    /// Enumerate the incarnations alive in one exact scope at a point (`spec.md:4342-4345`).
    ///
    /// # Obligations
    ///
    /// Ascending incarnation ids. One scope only. No open-time all-document scan exists, and this is
    /// not one.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    async fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: PageLimit,
        from: Option<DocumentCursor>,
        cx: &Cx,
    ) -> Result<Page<DocumentRecord, DocumentCursor>, StorageFailure>;
}

/// The typed half of the storage contract.
///
/// An extension trait with a blanket impl, so a generic method does not make [`Storage`] itself
/// non-dyn-compatible and the conformance suite keeps its `&mut dyn Storage`. Callers use
/// [`StorageExt::mint`]; backends implement [`Storage::mint_raw`].
#[async_trait]
pub trait StorageExt: Storage {
    /// Allocate one id of a named kind.
    ///
    /// The only way to obtain an [`Id<K>`] that is not already in a record: ADR-0030 F6 §A puts
    /// allocation on the storage handle so that *"an in-memory fast path or a stale cached high-water
    /// mark"* cannot reissue one. There is no free function, no `Default` allocator and no public
    /// `Id` constructor.
    ///
    /// # Errors
    ///
    /// [`CommitError`], as [`Storage::mint_raw`].
    async fn mint<K: IdKind>(&mut self, cx: &Cx) -> Result<Id<K>, CommitError> {
        let raw = self.mint_raw(K::KIND, cx).await?;
        Ok(Id::new(raw.get()))
    }
}

#[async_trait]
impl<S: Storage + ?Sized> StorageExt for S {}

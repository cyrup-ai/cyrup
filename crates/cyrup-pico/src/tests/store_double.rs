//! A storage double that delegates to [`MemoryStore`] and can fail a commit on cue.
//!
//! §11.1 makes [`MemoryStore`] the **normative reference semantics**, so the double delegates rather
//! than reimplements: what it adds is the one thing a correct backend never does on purpose, which is
//! fail. ADR-0030 §7's *"fault injection must hit three distinguishable points"* needs two of them
//! here — before admission, and admitted-then-unknown — and the third, committed-then-adoption-failed,
//! is reached through [`crate::fault`] instead because it is a failure of the **kernel**, not of a
//! backend.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cyrup_pico_store::{
    Batch, CommitError, CommittedEntry, ConversationCursor, ConversationId, ConversationQuery,
    ConversationRecord, Cx, DocumentAddress, DocumentCursor, DocumentId, DocumentPoint,
    DocumentQuery, DocumentRecord, EntryCursor, EntryId, EntryQuery, EntryRecord, HeadMarker,
    IdKindTag, MemoryStore, Page, PageLimit, RawId, RejectedReason, Seq, Storage, StorageFailure,
    StoreId, StoredDocument, SubmissionCursor, SubmissionId, SubmissionQuery, SubmissionRecord,
    TaskCursor, TaskId, TaskQuery, TaskRecord,
};

/// What the next commit should do.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Fault {
    /// Commit normally.
    #[default]
    None,
    /// Refuse, with rollback guaranteed. `spec.md:4310-4311`'s `Rejected` class.
    Reject,
    /// Fail without saying whether the batch committed. The fatal class.
    Uncertain,
}

/// A counter shared with the test, so a case can arm one fault and observe the commit count.
#[derive(Clone, Debug, Default)]
pub(crate) struct FaultSwitch {
    /// `0` none, `1` reject, `2` uncertain. An integer rather than a `Mutex<Fault>` so arming it
    /// from a test while the line holds it is lock-free.
    mode: Arc<AtomicU32>,
    commits: Arc<AtomicU32>,
}

impl FaultSwitch {
    pub(crate) fn arm(&self, fault: Fault) {
        let mode = match fault {
            Fault::None => 0,
            Fault::Reject => 1,
            Fault::Uncertain => 2,
        };
        self.mode.store(mode, Ordering::SeqCst);
    }

    /// How many times `Storage::commit` was entered, successful or not.
    ///
    /// This is how *"`NothingToCommit` allocates no sequence"* is asserted without trusting the
    /// backend's own bookkeeping: the call never happens.
    pub(crate) fn commits(&self) -> u32 {
        self.commits.load(Ordering::SeqCst)
    }

    fn take(&self) -> Fault {
        match self.mode.swap(0, Ordering::SeqCst) {
            1 => Fault::Reject,
            2 => Fault::Uncertain,
            _ => Fault::None,
        }
    }
}

/// A [`MemoryStore`] that outlives the Session that owned it.
///
/// PICO5-PLAN S5's fourth case needs a **reopen** — *"unaccessed documents with unavailable
/// definitions surviving a reopen byte-identically"* — and [`crate::SessionMut`] takes
/// `Box<dyn Storage>` by value and never hands it back, which is `G-SINGLE-COMMITTER` working
/// correctly. So the store itself is shared and the handle is what is opened twice.
///
/// `tokio::sync::Mutex` and not `std::sync::Mutex`: `Storage` is `Send`, and a `std` guard held
/// across the `.await` in every delegation would make these futures `!Send`. Nothing here nests a
/// lock, so there is no ordering to get wrong.
type Shared = Arc<tokio::sync::Mutex<MemoryStore>>;

/// Durable state that outlives the Session built over it.
///
/// Clone it, open it twice, and the second [`FaultyStore`] sees everything the first committed —
/// which is what *"a reopen"* means for a backend that has no files.
#[derive(Clone)]
pub(crate) struct SharedStore {
    inner: Shared,
    store_id: StoreId,
}

impl SharedStore {
    pub(crate) fn new() -> Self {
        let store = MemoryStore::new();
        let store_id = store.store_id();
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(store)),
            store_id,
        }
    }

    /// A backend over this state, with its own fault switch.
    pub(crate) fn open(&self) -> (FaultyStore, FaultSwitch) {
        let switch = FaultSwitch::default();
        (
            FaultyStore {
                inner: Arc::clone(&self.inner),
                store_id: self.store_id,
                switch: switch.clone(),
            },
            switch,
        )
    }

    /// The materialized state of one document, for a byte-for-byte comparison across a reopen.
    pub(crate) async fn document(
        &self,
        id: DocumentId,
    ) -> Result<Option<StoredDocument>, StorageFailure> {
        self.inner
            .lock()
            .await
            .document(id, DocumentPoint::Current, &Cx::detached())
            .await
    }

    /// The record at one address, or `None`.
    pub(crate) async fn find(
        &self,
        address: &DocumentAddress,
    ) -> Result<Option<DocumentRecord>, StorageFailure> {
        self.inner
            .lock()
            .await
            .find_document(address, DocumentPoint::Current, &Cx::detached())
            .await
    }
}

/// [`MemoryStore`] plus a fault switch.
///
/// `tokio::sync::Mutex` and not `std::sync::Mutex`: `Storage` is `Send`, and a `std` guard held
/// across the `.await` in every delegation would make these futures `!Send`. Nothing here nests a
/// lock, so there is no ordering to get wrong.
pub(crate) struct FaultyStore {
    inner: Shared,
    /// Cached, because `Storage::store_id` is synchronous and the store is behind an async lock. A
    /// store's identity never changes, so there is nothing to go stale.
    store_id: StoreId,
    switch: FaultSwitch,
}

impl FaultyStore {
    pub(crate) fn new() -> (Self, FaultSwitch) {
        SharedStore::new().open()
    }
}

#[async_trait::async_trait]
impl Storage for FaultyStore {
    fn store_id(&self) -> StoreId {
        self.store_id
    }

    async fn commit(&mut self, batch: Batch, cx: &Cx) -> Result<Seq, CommitError> {
        self.switch.commits.fetch_add(1, Ordering::SeqCst);
        match self.switch.take() {
            Fault::None => self.inner.lock().await.commit(batch, cx).await,
            // Any closed arm will do: what the case asserts is the CLASS, and the class is what
            // `RejectedReason` being closed makes honest. `SequenceExhausted` is chosen because it
            // carries no id a test would have to forge.
            Fault::Reject => Err(CommitError::Rejected(RejectedReason::SequenceExhausted)),
            Fault::Uncertain => Err(CommitError::Uncertain(
                cyrup_pico_store::UncertainCommit::new("the injected fault did not say"),
            )),
        }
    }

    async fn mint_raw(&mut self, kind: IdKindTag, cx: &Cx) -> Result<RawId, CommitError> {
        self.inner.lock().await.mint_raw(kind, cx).await
    }

    async fn close(&mut self, cx: &Cx) -> Result<(), StorageFailure> {
        self.inner.lock().await.close(cx).await
    }

    async fn conversation(
        &self,
        id: ConversationId,
        cx: &Cx,
    ) -> Result<Option<ConversationRecord>, StorageFailure> {
        self.inner.lock().await.conversation(id, cx).await
    }

    async fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: PageLimit,
        from: Option<ConversationCursor>,
        cx: &Cx,
    ) -> Result<Page<ConversationRecord, ConversationCursor>, StorageFailure> {
        self.inner
            .lock()
            .await
            .scan_conversations(query, limit, from, cx)
            .await
    }

    async fn entry(&self, id: EntryId, cx: &Cx) -> Result<Option<CommittedEntry>, StorageFailure> {
        self.inner.lock().await.entry(id, cx).await
    }

    async fn entry_in(
        &self,
        conversation_id: ConversationId,
        id: EntryId,
        cx: &Cx,
    ) -> Result<Option<CommittedEntry>, StorageFailure> {
        self.inner
            .lock()
            .await
            .entry_in(conversation_id, id, cx)
            .await
    }

    async fn find_latest_head_marker(
        &self,
        conversation_id: ConversationId,
        at_or_before: Option<EntryId>,
        cx: &Cx,
    ) -> Result<Option<HeadMarker>, StorageFailure> {
        self.inner
            .lock()
            .await
            .find_latest_head_marker(conversation_id, at_or_before, cx)
            .await
    }

    async fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: PageLimit,
        from: Option<EntryCursor>,
        cx: &Cx,
    ) -> Result<Page<EntryRecord, EntryCursor>, StorageFailure> {
        self.inner
            .lock()
            .await
            .scan_entries(query, limit, from, cx)
            .await
    }

    async fn task(&self, id: TaskId, cx: &Cx) -> Result<Option<TaskRecord>, StorageFailure> {
        self.inner.lock().await.task(id, cx).await
    }

    async fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: PageLimit,
        from: Option<TaskCursor>,
        cx: &Cx,
    ) -> Result<Page<TaskRecord, TaskCursor>, StorageFailure> {
        self.inner
            .lock()
            .await
            .scan_tasks(query, limit, from, cx)
            .await
    }

    async fn submission(
        &self,
        id: SubmissionId,
        cx: &Cx,
    ) -> Result<Option<SubmissionRecord>, StorageFailure> {
        self.inner.lock().await.submission(id, cx).await
    }

    async fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: PageLimit,
        from: Option<SubmissionCursor>,
        cx: &Cx,
    ) -> Result<Page<SubmissionRecord, SubmissionCursor>, StorageFailure> {
        self.inner
            .lock()
            .await
            .scan_submissions(query, limit, from, cx)
            .await
    }

    async fn submission_by_request(
        &self,
        conversation_id: ConversationId,
        request_id: &str,
        cx: &Cx,
    ) -> Result<Option<SubmissionRecord>, StorageFailure> {
        self.inner
            .lock()
            .await
            .submission_by_request(conversation_id, request_id, cx)
            .await
    }

    async fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
        cx: &Cx,
    ) -> Result<Option<DocumentRecord>, StorageFailure> {
        self.inner.lock().await.find_document(address, at, cx).await
    }

    async fn document(
        &self,
        id: DocumentId,
        at: DocumentPoint,
        cx: &Cx,
    ) -> Result<Option<StoredDocument>, StorageFailure> {
        self.inner.lock().await.document(id, at, cx).await
    }

    async fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: PageLimit,
        from: Option<DocumentCursor>,
        cx: &Cx,
    ) -> Result<Page<DocumentRecord, DocumentCursor>, StorageFailure> {
        self.inner
            .lock()
            .await
            .scan_documents(query, limit, from, cx)
            .await
    }
}

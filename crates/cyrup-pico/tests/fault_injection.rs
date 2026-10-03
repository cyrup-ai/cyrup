//! PICO5-PLAN **S8** — fault injection at the three distinguishable points, over a **real** store.
//!
//! ADR-0030 §7: *"Fault injection must hit three distinguishable points, not two. Before admission;
//! admitted-then-unknown; and **committed-then-adoption-failed**. The third is the one a port will skip
//! and the one pi poisons on that nobody expects, and it is why `UncertainKind` has two variants."*
//!
//! | point | injected where | the outcome the kernel owes | what the medium owes |
//! |---|---|---|---|
//! | 1. before admission | the callback, and the backend's `Rejected` class | `CommitOutcome::RolledBack`, the handle returned | **nothing durable**, on a reopen |
//! | 2. admitted, then unknown | the backend's `Uncertain` class | `Uncertain { kind: CommitStateUnknown }`, no handle | nothing is claimed — there is no honest answer |
//! | 3. committed, then adoption failed | `cyrup_pico::fault`'s adoption window | `Uncertain { kind: AdoptionFailedAfterCommit { seq } }` | the batch **is** there on a reopen |
//!
//! # Why this suite needs the JSONL backend and not `MemoryStore`
//!
//! Two of the three points make a claim about a **medium**: point 1 claims nothing durable happened,
//! and point 3 claims something durable did. ADR-0030 §7 puts exactly that class of case in the
//! *"remain necessary as behaviour tests"* list — *"whether a backend's atomicity claim is true;
//! whether a `Rejected` classification is honest … whether a poisoned Session's batch actually
//! committed"* — and §11.1 makes `MemoryStore` the reference *semantics*, which has no medium to
//! reopen. So every case here opens a real directory, and *"on a reopen"* means the bytes were read
//! back by a second `JsonlStore` after the first one's handle was dropped.
//!
//! The suite therefore lives in `cyrup-pico`'s own `tests/`, not in
//! `cyrup_pico_store::conformance`, even though that module's header anticipates *"fault injection at
//! three points (S8)"* extending it through `StoreFactory`. The reason is the same one S7 gave for its
//! reopen suite, one step further on: the conformance harness drives a `&mut dyn Storage` with **no
//! Session** — that is ADR-0030 §8's first load-bearing reason for the crate split — and the three
//! outcomes above are `CommitOutcome` variants. A suite with no Session cannot name them. What *is*
//! Session-free is the backend-honesty half, and `a_second_live_incarnation_at_one_address_is_rejected`
//! already carries it in the shared suite for every backend.
//!
//! # Point 1 and point 3 are the same audit, read in opposite directions
//!
//! `G-REJECTED-NO-DURABLE-EFFECT` and the durability claim in point 3 are one function here —
//! `trace()`, which reopens the directory and reports what a second process would see. A rejection
//! must leave it unchanged; an adoption failure must leave the batch in it. One probe, two
//! directions, so neither assertion can drift away from the other's definition of *durable*.
//!
//! # The dishonest backend, and why it is a test rather than a type
//!
//! PICO5-PLAN S8: *"a `Rejected` returned after a partial durable write must be caught by the suite,
//! not inferred — because no type catches it."* ADR-0030 §2.2 agrees in the enforcement column for
//! that row: *"**unrepresentable** to omit the choice or to classify I/O as recoverable; **checked**
//! for whether the claim is true."* `RejectedReason` being a closed enum makes the *classification*
//! honest about its own meaning; nothing makes a backend's *use* of it honest. So two cases below arm
//! a backend that lies — one that commits the caller's whole batch and then claims rollback, one that
//! commits a side effect of its own and then claims rollback — and assert that the kernel **believes
//! it** (`RolledBack`, handle returned, Session usable) while the audit **catches it**. The second
//! assertion is what stops the honest cases from being vacuous: a `trace()` that could not tell the
//! difference would pass them for the wrong reason.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path as FsPath;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cyrup_pico::{
    CommitOutcome, CommitReply, Committer, DocToken, Line, RollbackReason, SessionMut,
    UncertainKind,
};
use cyrup_pico_doc::{DocRoot, DocValue, Path as DocPath};
use cyrup_pico_store::{
    Batch, BatchBuilder, CommitError, CommittedEntry, ConversationCursor, ConversationId,
    ConversationQuery, ConversationRecord, Cx, DocumentAddress, DocumentCursor, DocumentId,
    DocumentPoint, DocumentQuery, DocumentRecord, EntryCursor, EntryId, EntryQuery, EntryRecord,
    HeadMarker, IdKindTag, Page, PageLimit, ROOT_CONVERSATION_ID, RawId, RejectedReason, Seq,
    Storage, StorageFailure, StoreId, StoredDocument, SubmissionCursor, SubmissionId,
    SubmissionQuery, SubmissionRecord, TaskCursor, TaskId, TaskQuery, TaskRecord,
};
use cyrup_pico_store_jsonl::{Durability, JsonlOptions, JsonlStore, StoreLock};
use tempfile::TempDir;
use tokio::task::LocalSet;

// ---------------------------------------------------------------------------------------------
// The definition this file speaks.
// ---------------------------------------------------------------------------------------------

/// `spec.md:1224-1225`: *"ordinary definitions are not registered"*, so a definition is a type and the
/// token is built where it is used.
struct Live;

/// `Live`'s value: the empty object, so a document's whole state is whatever the draft set.
#[derive(serde::Serialize)]
struct Empty {}

impl cyrup_pico::DocDef for Live {
    type Value = Empty;
    type Place = cyrup_pico::SessionScoped;
    type Shape = cyrup_pico::Singleton;
    type Seed = ();
    const KIND: &'static str = "test.live";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

/// The Session's `test.live` singleton, as a typed address.
fn address() -> cyrup_pico::DocAddress<Live> {
    DocToken::<Live>::define()
        .expect("test.live is a definition")
        .at()
}

/// A one-segment document path.
fn path(key: &str) -> DocPath {
    DocPath::keys([key]).expect("a one-key path")
}

/// The value a document holds after one `set` of `phase`.
fn phase_is(phase: &str) -> DocValue {
    DocValue::string(phase)
}

// ---------------------------------------------------------------------------------------------
// The audit: what a second opener sees.
// ---------------------------------------------------------------------------------------------

/// What the medium holds, as a reopen reports it.
///
/// This is the whole definition of *durable* this suite uses, and it is deliberately small: the
/// document under test's materialized value, and whether the sentinel record a dishonest backend
/// writes is there. A rejection must not change it; an adoption failure must have changed it.
#[derive(PartialEq, Debug)]
struct Trace {
    /// The `test.live` singleton's materialized value, or `None` if no incarnation is alive.
    document: Option<DocRoot>,
    /// Whether the root conversation record exists — the sentinel a *partially* dishonest backend
    /// leaves behind.
    sentinel: bool,
}

impl Trace {
    /// The trace of a store that has never committed anything.
    const fn empty() -> Self {
        Self {
            document: None,
            sentinel: false,
        }
    }

    /// The `phase` key of the document this trace found, if it found one.
    fn phase(&self) -> Option<&DocValue> {
        self.document.as_ref().and_then(|value| value.get("phase"))
    }
}

/// Reopen `dir` read-only and report what it holds.
///
/// `open_read_only` takes **no** lock and applies only what a marker confirms
/// (`JsonlStore::open_read_only`), which is exactly the question the audit asks: not *"what does the
/// writer think"* but *"what would a second process find"*. It is therefore callable while the Session
/// still holds the store, which is what lets point 1 probe before and after one failed commit.
async fn trace(dir: &FsPath, at: &DocumentAddress) -> Trace {
    let store = JsonlStore::open_read_only(dir).expect("the store reopens read-only");
    let document = match store
        .find_document(at, DocumentPoint::Current)
        .await
        .expect("the address lookup")
    {
        Some(record) => Some(
            store
                .document(record.id, DocumentPoint::Current)
                .await
                .expect("the materialisation")
                .expect("an incarnation its own record named")
                .value,
        ),
        None => None,
    };
    let sentinel = store
        .conversation(ROOT_CONVERSATION_ID)
        .await
        .expect("the conversation lookup")
        .is_some();
    Trace { document, sentinel }
}

// ---------------------------------------------------------------------------------------------
// The injector.
// ---------------------------------------------------------------------------------------------

/// What the next `Storage::commit` should do.
///
/// The first three are the honest classes a backend may legitimately return; the last two are
/// **lies**, and they exist because `spec.md:4307-4311`'s split is a promise a backend makes and not
/// one a type can keep.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Fault {
    /// Commit normally.
    #[default]
    None,
    /// Refuse, with rollback guaranteed, having written nothing. The honest `Rejected`.
    Reject,
    /// Fail without saying whether the batch committed. The fatal class.
    Uncertain,
    /// **Dishonest:** commit the caller's whole batch, then claim rollback.
    RejectAfterCommitting,
    /// **Dishonest:** commit a record of the backend's own, then claim rollback for the caller's
    /// batch.
    ///
    /// This is as close as a keyed `Batch` lets a test get to ADR-0030 F3's *"nothing stops a backend
    /// partially applying it"*: the batch's fields are private and it is taken by value, so it cannot
    /// be split from outside. What *is* reproducible — and is the thing the audit has to catch — is a
    /// durable effect accompanying a `Rejected`.
    RejectAfterSideEffect,
}

/// A fault armed from the test while the line holds the store.
///
/// An integer rather than a `Mutex<Fault>` so arming it is lock-free, exactly as
/// `src/tests/store_double.rs` does for the `MemoryStore` half of the same job.
#[derive(Clone, Debug, Default)]
struct FaultSwitch(Arc<AtomicU32>);

impl FaultSwitch {
    /// Arm the **next** commit, and only the next one.
    fn arm(&self, fault: Fault) {
        let mode = match fault {
            Fault::None => 0,
            Fault::Reject => 1,
            Fault::Uncertain => 2,
            Fault::RejectAfterCommitting => 3,
            Fault::RejectAfterSideEffect => 4,
        };
        self.0.store(mode, Ordering::SeqCst);
    }

    /// Take the armed fault, disarming it.
    fn take(&self) -> Fault {
        match self.0.swap(0, Ordering::SeqCst) {
            1 => Fault::Reject,
            2 => Fault::Uncertain,
            3 => Fault::RejectAfterCommitting,
            4 => Fault::RejectAfterSideEffect,
            _ => Fault::None,
        }
    }
}

/// A real `JsonlStore` that can fail, or lie, on cue.
///
/// Every read path delegates unchanged, because the subject of this suite is the **commit** boundary:
/// a double that modelled the reads would be modelling the thing the audit is supposed to measure.
struct Faulty {
    inner: JsonlStore,
    switch: FaultSwitch,
}

impl Faulty {
    /// Open `dir` for writing, exclusively, at the strong tier.
    ///
    /// `Durability::PowerLoss` and not `ProcessCrash`, because the audit reopens the directory while
    /// the writer is still alive: the strong tier flushes every affected sidecar before its marker, so
    /// *"what a second process would find"* is a question about the store and not about when the page
    /// cache happened to be written back.
    fn open(dir: &FsPath) -> (Self, FaultSwitch) {
        let lock = StoreLock::acquire(dir).expect("the store lock");
        let inner = JsonlStore::open_for_write(dir, lock, JsonlOptions::new(Durability::PowerLoss))
            .expect("the store opens");
        let switch = FaultSwitch::default();
        (
            Self {
                inner,
                switch: switch.clone(),
            },
            switch,
        )
    }

    /// The sentinel batch a partially dishonest backend writes behind the caller's back.
    fn sentinel() -> Batch {
        let mut builder = BatchBuilder::new();
        builder
            .conversation(ConversationRecord {
                id: ROOT_CONVERSATION_ID,
                parent: None,
                owner: None,
            })
            .expect("staging the root conversation");
        builder.build().expect("a non-empty batch")
    }

    /// The rejection every refusing arm returns.
    ///
    /// Any closed arm will do: what every case asserts is the **class**, and the class is what
    /// `RejectedReason` being closed — no string arm, no `From<std::io::Error>` — makes honest.
    /// `SequenceExhausted` is chosen because it carries no id a test would have to forge.
    const fn refusal() -> CommitError {
        CommitError::Rejected(RejectedReason::SequenceExhausted)
    }
}

#[async_trait::async_trait]
impl Storage for Faulty {
    fn store_id(&self) -> StoreId {
        self.inner.store_id()
    }

    async fn commit(&mut self, batch: Batch, cx: &Cx) -> Result<Seq, CommitError> {
        match self.switch.take() {
            Fault::None => self.inner.commit(batch, cx).await,
            Fault::Reject => Err(Self::refusal()),
            Fault::Uncertain => Err(CommitError::Uncertain(
                cyrup_pico_store::UncertainCommit::new("the injected fault did not say"),
            )),
            Fault::RejectAfterCommitting => {
                self.inner.commit(batch, cx).await?;
                Err(Self::refusal())
            }
            Fault::RejectAfterSideEffect => {
                self.inner.commit(Self::sentinel(), cx).await?;
                drop(batch);
                Err(Self::refusal())
            }
        }
    }

    async fn mint_raw(&mut self, kind: IdKindTag, cx: &Cx) -> Result<RawId, CommitError> {
        self.inner.mint_raw(kind, cx).await
    }

    async fn close(&mut self, cx: &Cx) -> Result<(), StorageFailure> {
        self.inner.close(cx).await
    }

    async fn conversation(
        &self,
        id: ConversationId,
        cx: &Cx,
    ) -> Result<Option<ConversationRecord>, StorageFailure> {
        self.inner.conversation(id, cx).await
    }

    async fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: PageLimit,
        from: Option<ConversationCursor>,
        cx: &Cx,
    ) -> Result<Page<ConversationRecord, ConversationCursor>, StorageFailure> {
        self.inner.scan_conversations(query, limit, from, cx).await
    }

    async fn entry(&self, id: EntryId, cx: &Cx) -> Result<Option<CommittedEntry>, StorageFailure> {
        self.inner.entry(id, cx).await
    }

    async fn entry_in(
        &self,
        conversation_id: ConversationId,
        id: EntryId,
        cx: &Cx,
    ) -> Result<Option<CommittedEntry>, StorageFailure> {
        self.inner.entry_in(conversation_id, id, cx).await
    }

    async fn find_latest_head_marker(
        &self,
        conversation_id: ConversationId,
        at_or_before: Option<EntryId>,
        cx: &Cx,
    ) -> Result<Option<HeadMarker>, StorageFailure> {
        self.inner
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
        self.inner.scan_entries(query, limit, from, cx).await
    }

    async fn task(&self, id: TaskId, cx: &Cx) -> Result<Option<TaskRecord>, StorageFailure> {
        self.inner.task(id, cx).await
    }

    async fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: PageLimit,
        from: Option<TaskCursor>,
        cx: &Cx,
    ) -> Result<Page<TaskRecord, TaskCursor>, StorageFailure> {
        self.inner.scan_tasks(query, limit, from, cx).await
    }

    async fn submission(
        &self,
        id: SubmissionId,
        cx: &Cx,
    ) -> Result<Option<SubmissionRecord>, StorageFailure> {
        self.inner.submission(id, cx).await
    }

    async fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: PageLimit,
        from: Option<SubmissionCursor>,
        cx: &Cx,
    ) -> Result<Page<SubmissionRecord, SubmissionCursor>, StorageFailure> {
        self.inner.scan_submissions(query, limit, from, cx).await
    }

    async fn submission_by_request(
        &self,
        conversation_id: ConversationId,
        request_id: &str,
        cx: &Cx,
    ) -> Result<Option<SubmissionRecord>, StorageFailure> {
        self.inner
            .submission_by_request(conversation_id, request_id, cx)
            .await
    }

    async fn find_document(
        &self,
        at: &DocumentAddress,
        point: DocumentPoint,
        cx: &Cx,
    ) -> Result<Option<DocumentRecord>, StorageFailure> {
        self.inner.find_document(at, point, cx).await
    }

    async fn document(
        &self,
        id: DocumentId,
        at: DocumentPoint,
        cx: &Cx,
    ) -> Result<Option<StoredDocument>, StorageFailure> {
        self.inner.document(id, at, cx).await
    }

    async fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: PageLimit,
        from: Option<DocumentCursor>,
        cx: &Cx,
    ) -> Result<Page<DocumentRecord, DocumentCursor>, StorageFailure> {
        self.inner.scan_documents(query, limit, from, cx).await
    }
}

/// A store directory deleted when the case ends.
fn dir() -> TempDir {
    tempfile::tempdir().expect("a temp directory")
}

// ---------------------------------------------------------------------------------------------
// Point 1 — before admission.
// ---------------------------------------------------------------------------------------------

/// A callback that abandons the change rolls back, and the medium never heard of it.
///
/// `spec.md:76-78`'s second sentence, and `G-PRE-ADMISSION-ROLLBACK`: *"pre-admission failures roll
/// back normally"*. The handle comes back in the variant, which is `CommitOutcome`'s whole shape, and
/// the proof that it is a *type* and not a convention is S4's
/// `tests/compile-fail/a_session_mut_cannot_be_obtained_from_an_uncertain_outcome.rs` — the fatal arm
/// has no such field.
#[tokio::test(flavor = "current_thread")]
async fn a_callback_failure_rolls_back_and_leaves_no_durable_trace() {
    let dir = dir();
    let (store, _switch) = Faulty::open(dir.path());
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = address();

    let before = trace(dir.path(), at.address()).await;
    assert_eq!(before, Trace::empty(), "a fresh store holds nothing");

    // The hazard: a callback that wrote a draft and *then* failed. The draft work is real, which is
    // what makes "nothing durable" a claim rather than a tautology.
    //
    // The annotation is not noise: a callback that only ever fails never names `R`, so `E0282` is
    // the diagnostic ADR-0030 F2's `CallbackError` documentation predicts for exactly this shape.
    let outcome: CommitOutcome<()> = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Err(cyrup_pico::CallbackError::Abandoned(
                "the injected fault".into(),
            ))
        })
        .await;
    let CommitOutcome::RolledBack {
        reason,
        session: handle,
    } = outcome
    else {
        panic!("a callback failure is a rollback, not a poison: {outcome:?}");
    };
    assert!(
        matches!(reason, RollbackReason::Callback(_)),
        "the rollback names the step that refused: {reason}"
    );
    assert_eq!(session.publications(), 0, "a rollback publishes nothing");
    assert_eq!(
        trace(dir.path(), at.address()).await,
        Trace::empty(),
        "a pre-admission failure reached the medium"
    );

    // And the Session is fully usable: the same change, committed.
    let CommitOutcome::Committed {
        session: handle,
        seq,
        ..
    } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the Session must still be usable after a rollback");
    };
    assert_eq!(session.publications(), 1);
    drop(handle);

    let after = trace(dir.path(), at.address()).await;
    assert_eq!(
        after.phase(),
        Some(&phase_is("charging")),
        "the commit that was allowed through is durable at {seq}"
    );
}

/// An honest `Rejected` from the backend rolls back, and the medium never heard of it either.
///
/// This is **`G-REJECTED-NO-DURABLE-EFFECT`**, which ADR-0030 §2.2 classifies *"**checked** for
/// whether the claim is true"* — so this case is the whole of its enforcement, and the two dishonest
/// cases below are what prove the check can fail.
#[tokio::test(flavor = "current_thread")]
async fn a_rejected_batch_rolls_back_and_leaves_no_durable_trace() {
    let dir = dir();
    let (store, switch) = Faulty::open(dir.path());
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = address();

    switch.arm(Fault::Reject);
    let outcome = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await;
    let CommitOutcome::RolledBack {
        reason,
        session: handle,
    } = outcome
    else {
        panic!("`Rejected` is a rollback: a closed reason promises nothing happened: {outcome:?}");
    };
    assert!(
        matches!(reason, RollbackReason::Rejected(_)),
        "the rollback carries the backend's closed reason: {reason}"
    );
    assert_eq!(session.publications(), 0);

    drop(handle);
    assert_eq!(
        trace(dir.path(), at.address()).await,
        Trace::empty(),
        "a rejected batch left a durable trace: `Rejected` claims nothing happened"
    );
}

// ---------------------------------------------------------------------------------------------
// Point 2 — admitted, then unknown.
// ---------------------------------------------------------------------------------------------

/// An admitted commit whose outcome is unknown is fatal, closes every ticket, and releases the store.
///
/// `spec.md:76-78`: *"an uncertain storage failure is fatal to the open Session. It publishes nothing
/// and must be reopened."* Three halves, each asserted:
///
/// * **publishes nothing** — `publications()` never moves;
/// * **every ticket dead** — the committing one is told through `CommitReply::SessionDead`, every
///   other through a closed channel, which is the weaker `checked` guarantee ADR-0030 F1 says the
///   `Committer` boundary has and says out loud rather than claiming the consuming form;
/// * **must be reopened** — and *can* be: the handle was consumed, so the `Box<dyn Storage>` and the
///   `StoreLock` inside it dropped with it, and the next `open_for_write` succeeds. A fatal commit
///   that left the directory locked would turn one lost session into a lost repository.
///
/// It deliberately asserts **nothing** about what the medium holds. There is no honest answer — that
/// is what the variant's name means — and `committed_at()` returning `None` is the type saying so.
#[tokio::test(flavor = "current_thread")]
async fn an_unknown_commit_state_is_fatal_closes_every_ticket_and_releases_the_store() {
    let dir = dir();
    let (store, switch) = Faulty::open(dir.path());
    let (session, handle) = SessionMut::open(Box::new(store));
    let (committer, line) = Line::new(handle, 4);
    // A second ticket, held by somebody else entirely.
    let bystander: Committer = committer.clone();
    let local = LocalSet::new();
    let driving = local.spawn_local(line.run());

    switch.arm(Fault::Uncertain);
    let at = address();
    let reply = local
        .run_until(committer.commit(Cx::detached(), async move |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        }))
        .await
        .expect("the reply arrives before the loop returns");
    let CommitReply::SessionDead(fatal) = reply else {
        panic!("an uncertain commit kills the line: {reply:?}");
    };
    assert_eq!(fatal.kind(), UncertainKind::CommitStateUnknown);
    assert_eq!(
        fatal.committed_at(),
        None,
        "there is no honest sequence to report, and `None` is how the type says so"
    );

    drop(committer);
    local.await;
    assert!(
        driving
            .await
            .expect("the line task did not panic")
            .is_none(),
        "the loop returns with no handle: there is nothing to hand back"
    );
    assert!(bystander.is_closed(), "every later ticket call fails");
    let second: Result<CommitReply<()>, _> = bystander
        .commit(Cx::detached(), async move |tx| Ok(((), tx.writing())))
        .await;
    assert!(
        second.is_err(),
        "a bystander that never committed learns from the closed channel, which is the `checked` \
         half ADR-0030 F1 says the `Committer` boundary has"
    );
    assert_eq!(
        session.publications(),
        0,
        "a fatal commit publishes nothing"
    );

    // *"must be reopened"* — and nothing in the directory prevents it.
    let reopened = StoreLock::acquire(dir.path())
        .map(|lock| {
            JsonlStore::open_for_write(dir.path(), lock, JsonlOptions::new(Durability::PowerLoss))
        })
        .expect("the store lock the dead Session released");
    assert!(
        reopened.is_ok(),
        "a poisoned Session must leave a reopenable store"
    );
}

// ---------------------------------------------------------------------------------------------
// Point 3 — committed, then adoption failed. The one a port skips.
// ---------------------------------------------------------------------------------------------

/// Storage committed, adoption did not: fatal, nothing published, and the batch **is** on disk.
///
/// This is the point ADR-0030 §7 says *"a port will skip"*, and `src/fault.rs` says why it needs a
/// seam at all: between a successful `Storage::commit` and a successful adoption there is nothing a
/// caller can reach, which is F1 and F5 working — and a guarantee whose failure mode is unreachable
/// is also untestable.
///
/// The assertion that makes it worth the seam is the last one. `UncertainKind` has two variants
/// because the two sides of the fence owe the **host** different things: `CommitStateUnknown` means
/// reopen *and reconcile*, `AdoptionFailedAfterCommit` means reopen with *nothing to reconcile* —
/// and that second claim is only true if the batch really is durable. So the case reopens the
/// directory and finds it, at the sequence the variant named.
#[tokio::test(flavor = "current_thread")]
async fn an_adoption_failure_after_a_committed_batch_is_fatal_and_the_batch_is_durable() {
    let dir = dir();
    let (store, _switch) = Faulty::open(dir.path());
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = address();

    // Create the document first, so the fatal commit is an adoption into an existing tracker — the
    // throttled-live-document path, which is the one that runs thousands of times in a session.
    let CommitOutcome::Committed {
        session: handle,
        seq: created_at,
        ..
    } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the creating commit must succeed");
    };
    let incarnation = session
        .current_incarnation(at.address())
        .expect("the authority is healthy")
        .expect("the document was created");

    // The window: drop the incarnation from the authority between `Storage::commit` returning and
    // `Tx::adopt` running, so adoption fails with storage already committed.
    cyrup_pico::fault::set_before_adopt(move |window| window.forget(incarnation));
    let outcome = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "discharging")?;
            Ok(((), tx.writing()))
        })
        .await;
    cyrup_pico::fault::clear();

    let CommitOutcome::Uncertain(fatal) = outcome else {
        panic!("a failed adoption after a committed batch is fatal: {outcome:?}");
    };
    let UncertainKind::AdoptionFailedAfterCommit { seq } = fatal.kind() else {
        panic!(
            "the kernel misclassified the adoption side of the fence as the storage side: {:?}",
            fatal.kind()
        );
    };
    assert!(
        seq > created_at,
        "the reported sequence is the one storage allocated for this batch: {seq} after {created_at}"
    );
    assert_eq!(fatal.committed_at(), Some(seq));
    assert_eq!(
        session.publications(),
        1,
        "only the first commit published: `spec.md:76-78` — a fatal commit publishes nothing"
    );

    // The claim `AdoptionFailedAfterCommit` makes about the world, checked against the world.
    let after = trace(dir.path(), at.address()).await;
    assert_eq!(
        after.phase(),
        Some(&phase_is("discharging")),
        "storage committed, so a reopen must find the batch: there is nothing to reconcile only if \
         it is really there"
    );
}

// ---------------------------------------------------------------------------------------------
// The dishonest backend. No type catches this; the audit does.
// ---------------------------------------------------------------------------------------------

/// A backend that commits the whole batch and then claims rollback is believed by the kernel, and
/// caught by the audit.
///
/// Both halves matter. The first is not a defect to fix: `RejectedReason` is a *promise*
/// (`spec.md:4310-4311`), the kernel has no second channel to check it against, and inventing one
/// would mean a read after every rejection on the held line. The second is why
/// `a_rejected_batch_rolls_back_and_leaves_no_durable_trace` above is a real assertion rather than a
/// shape that passes whatever the backend does.
#[tokio::test(flavor = "current_thread")]
async fn a_dishonest_rejection_after_a_full_durable_write_is_caught_by_the_audit() {
    let dir = dir();
    let (store, switch) = Faulty::open(dir.path());
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = address();

    switch.arm(Fault::RejectAfterCommitting);
    let outcome = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await;

    // Believed: the kernel cannot tell, and says so by returning the handle.
    let CommitOutcome::RolledBack {
        reason,
        session: handle,
    } = outcome
    else {
        panic!("the kernel has only the backend's word, and the word was `Rejected`: {outcome:?}");
    };
    assert!(matches!(reason, RollbackReason::Rejected(_)));
    assert_eq!(
        session.publications(),
        0,
        "the kernel published nothing, which is the one thing it could still get right"
    );
    drop(handle);

    // Caught: the audit the honest case relies on fires here, which is what makes it not vacuous.
    let after = trace(dir.path(), at.address()).await;
    assert_ne!(
        after,
        Trace::empty(),
        "the audit cannot see a durable write, so it cannot have proved the honest case"
    );
    assert_eq!(
        after.phase(),
        Some(&phase_is("charging")),
        "the whole batch landed behind a `Rejected`: no type catches this, and the suite must"
    );
}

/// A backend that writes a record of its own and then claims rollback is caught the same way.
///
/// PICO5-PLAN S8's *"a `Rejected` returned after a **partial** durable write"*. The partiality is in
/// the only place a keyed `Batch` leaves room for it — see `Fault::RejectAfterSideEffect` — and the
/// audit's answer is the same: the store moved, and `Rejected` said it would not.
#[tokio::test(flavor = "current_thread")]
async fn a_dishonest_rejection_after_a_partial_durable_write_is_caught_by_the_audit() {
    let dir = dir();
    let (store, switch) = Faulty::open(dir.path());
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let at = address();

    switch.arm(Fault::RejectAfterSideEffect);
    let outcome = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await;
    let CommitOutcome::RolledBack {
        reason,
        session: handle,
    } = outcome
    else {
        panic!("the kernel is told `Rejected` and has nothing to check it against: {outcome:?}");
    };
    assert!(matches!(reason, RollbackReason::Rejected(_)));
    assert_eq!(session.publications(), 0);
    drop(handle);

    let after = trace(dir.path(), at.address()).await;
    assert!(
        after.sentinel,
        "the backend's own write must be visible to the audit, or the audit measures nothing"
    );
    assert_eq!(
        after.document, None,
        "the caller's half genuinely did not land: this is the partial case, not the full one"
    );
    assert_ne!(after, Trace::empty(), "the store moved behind a `Rejected`");
}

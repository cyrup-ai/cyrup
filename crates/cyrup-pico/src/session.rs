//! ADR-0030 F1's two handles: a clonable [`Session`] that can only read, and a single
//! [`SessionMut`] that is the only thing in the process that can commit.
//!
//! # The sole-mutator rule as a method set
//!
//! `spec.md:1126` states it as API shape and convention — *"There is no mutable `session.document()`
//! API"* — and `spec.md:3822` repeats it. Here it is the fact that [`Session`] **has no `commit`
//! method and no mutable document accessor**: §3's rule is this type's method set, and a second
//! writer is not something the code declines to offer, it is something there is no type to express.
//!
//! # Why [`SessionMut`] is not `Clone` and never in an `Arc`
//!
//! `spec.md:4321-4323` licenses backends to add no second commit mutex, on the ground that the
//! Session is the sole committer — a property documented *of the caller* and unverifiable by the
//! callee. [`SessionMut`] owns `Box<dyn Storage>`, and `Storage::commit(&mut self, ..)` therefore
//! type-checks only because that ownership is exclusive. The licence stops being an assumption and
//! becomes the reason the signature compiles (`G-SINGLE-COMMITTER`).
//!
//! F1 names the reflex this forecloses: `Arc<Mutex<Session>>`, where a read path that takes the lock
//! separately from the commit path lands a snapshot read between storage success and in-memory
//! adoption and renders a durably stale value.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cyrup_pico_doc::DocRoot;
use cyrup_pico_store::{Cx, DocumentAddress, DocumentId, Storage, StorageFailure, StoreId};

use crate::docs::{AuthorityPoisoned, DocIndex};
use crate::hold::LineHold;
use crate::observe::{CloseObserver, Closing, CommitObserver, Disposer, Observers, Retirable};
use crate::outcome::{CommitOutcome, RollbackReason};
use crate::revision::FrameContext;
use crate::slot::{DocSlot, Subscriptions};
use crate::state::Attachment;
use crate::tx::{CallbackError, Reading, Run, Tx, Writing, run_change};
use crate::watch::DocWatch;
use crate::witness::Publication;

/// Everything both handles share.
///
/// Not public: a [`Session`] is the read view of it and [`SessionMut`] is the write view, and there
/// is no third view to hand out. In particular the transaction does **not** hold one of these — see
/// [`crate::tx::TxInner`]'s documentation for why that absence is invariant 4's reachable path.
#[derive(Debug)]
pub struct SessionShared {
    store: StoreId,
    docs: DocIndex,
    /// How many publications this Session has made.
    ///
    /// A **count**, not the last sequence, and the reason is S1's: [`Seq`](cyrup_pico_store::Seq) has
    /// no accessor that returns its number, deliberately — *"no casual round-tripping accessor"*
    /// (ADR-0030 F6 §A) — so storing one in an `AtomicU64` would mean adding exactly the accessor
    /// that newtype exists to withhold. The sequence a caller needs is in
    /// [`CommitOutcome::Committed`], where it is typed.
    publications: AtomicU64,
    /// PICO5-PLAN S6's synchronous line observers — [`CommitObserver`] and [`CloseObserver`].
    observers: Observers,
    /// PICO5-PLAN S6's off-line document subscriptions — [`crate::DocState`] and [`crate::DocWatch`].
    subscriptions: Subscriptions,
}

impl SessionShared {
    /// Publish one commit. **The only publish there is.**
    ///
    /// Takes the [`Publication`] by value, so the witness is consumed and a publication cannot be
    /// replayed. There is no second emitter, and one could not be written: `Publication`'s fields are
    /// private and sealed, so nothing outside [`crate::witness`] can construct the argument this
    /// method requires (`G-INV-2`, `G-INV-3`).
    ///
    /// # What happens here, in this order
    ///
    /// 1. **The frames are buffered** into every live document subscription
    ///    ([`Subscriptions::deliver`]), with the document's post-adoption tracker revision as the
    ///    watermark `crates/cyrup-pico/src/slot.rs`'s table compares against. This is a buffer append
    ///    under a mutex: no user code runs, so *"document-state and watch user callbacks run later"*
    ///    (`spec.md:1524`) is a property of this step rather than of a scheduler.
    /// 2. **The synchronous observers run** ([`Observers::publish_to`]), each under `catch_unwind`, so
    ///    one panicking observer advances the others instead of truncating the iteration and instead of
    ///    reporting a durable, adopted commit as a failure.
    ///
    /// Buffering is first deliberately. An observer is caller code and the frames are not, so a
    /// publication that reached step 1 has reached every consumer that cannot misbehave before it
    /// reaches any consumer that can.
    pub(crate) fn publish(&self, publication: Publication, cx: &Cx) {
        self.publications.fetch_add(1, Ordering::Release);
        // Values without the token. `FrameContext` has no field for a `CancellationToken`, which is
        // why a delivered frame cannot inherit this commit's cancellation (`spec.md:3889`).
        let context = FrameContext::for_commit(cx);
        for change in publication.changes() {
            // The watermark, read in one acquisition with nothing between it and the push.
            let revision = self
                .docs
                .baseline_of(change.document())
                .ok()
                .flatten()
                .map(|(_, revision)| revision);
            self.subscriptions.deliver(change, revision, &context);
        }
        self.observers.publish_to(&publication);
        // The witness is dropped here, which is what makes it single-use.
        drop(publication);
    }

    /// Acquire one logical address: register a buffering subscription, then arm it with the baseline.
    ///
    /// **Registration comes first and the baseline second**, which is the order
    /// `crates/cyrup-pico/src/slot.rs` argues for: a slot registered after a fan-out misses a frame
    /// with nothing able to detect it, while a slot registered before one can only buffer a frame the
    /// watermark then removes.
    ///
    /// `Ok(None)` is absence, and it is absence in `spec.md:3826`'s sense — *"state and watch
    /// acquisition never create; absent lookup returns `undefined`"*. Nothing here reads storage,
    /// creates, or stages a migration (`spec.md:1235-1237`).
    fn acquire(
        &self,
        address: &DocumentAddress,
    ) -> Result<Option<(Arc<DocSlot>, Disposer)>, AuthorityPoisoned> {
        let Some(id) = self.docs.current_id(address)? else {
            return Ok(None);
        };
        let slot = Arc::new(DocSlot::new(id));
        self.subscriptions.register(&slot);
        let retirable: Arc<dyn Retirable> = Arc::clone(&slot) as Arc<dyn Retirable>;
        let disposer = Disposer::new(&retirable);
        // The incarnation can have been retired between the two reads. Returning absence is correct:
        // a subscription binds one concrete incarnation, and that one is gone.
        let Some((value, revision)) = self.docs.baseline_of(id)? else {
            return Ok(None);
        };
        slot.arm(value, revision);
        Ok(Some((slot, disposer)))
    }
}

/// A shared, cheap-to-clone read handle.
///
/// **Reads and subscriptions only.** There is deliberately no `commit`, and no accessor that returns
/// a `&mut` to anything in a document: every value out of here is a [`DocRoot`], an `Arc` over a type
/// with no interior-mutable variant, so *"every published revision is immutable for all time"*
/// (`spec.md:4528-4530`) holds for whatever a caller retains (`G-DOC-SOURCE-NO-MUTABLE-OBJECT`).
#[derive(Clone, Debug)]
pub struct Session(Arc<SessionShared>);

impl Session {
    /// The store this Session is open on.
    #[must_use]
    pub fn store_id(&self) -> StoreId {
        self.0.store
    }

    /// How many commits this Session has published.
    #[must_use]
    pub fn publications(&self) -> u64 {
        self.0.publications.load(Ordering::Acquire)
    }

    /// The current shareable immutable revision at one logical address.
    ///
    /// `spec.md:1251-1253`: *"`snapshot()` returns the current shareable immutable revision. Mutation
    /// of it or any retained descendant is unsupported"* — upstream has to say *unsupported* because
    /// nothing stops it. Here the returned value has no mutable view at all, so the sentence has no
    /// work left to do.
    ///
    /// `Ok(None)` is absence. Only documents this Session has loaded or created are here; a
    /// historical read goes to storage and is PICO5-PLAN S5's.
    ///
    /// # Errors
    /// [`AuthorityPoisoned`] when a panic unwound through the authority.
    pub fn snapshot(
        &self,
        address: &DocumentAddress,
    ) -> Result<Option<DocRoot>, AuthorityPoisoned> {
        match self.0.docs.current_id(address)? {
            Some(id) => self.0.docs.value_of(id),
            None => Ok(None),
        }
    }

    /// The live incarnation at one logical address, if this Session has loaded it.
    ///
    /// # Errors
    /// [`AuthorityPoisoned`].
    pub fn current_incarnation(
        &self,
        address: &DocumentAddress,
    ) -> Result<Option<DocumentId>, AuthorityPoisoned> {
        self.0.docs.current_id(address)
    }

    /// Register a synchronous observer of committed publications.
    ///
    /// `spec.md:1528`: *"`subscribeCommits()` observes complete immutable publications synchronously
    /// on the line after adoption."* The observer runs on the mutation line, cannot await, cannot
    /// report failure, and is given no Session handle — see [`CommitObserver`] for which of those is
    /// enforced how.
    ///
    /// The returned [`Disposer`] **is the registration's lifetime**: dropping it unregisters, which is
    /// why it is `#[must_use]` and why [`Disposer::leak`] exists for an observer meant to last the
    /// Session's life.
    pub fn subscribe_commits<O: CommitObserver>(&self, observer: O) -> Disposer {
        self.0.observers.add_commit(Box::new(observer))
    }

    /// Register a synchronous observer of the Session closing.
    ///
    /// `spec.md:1530-1531`: *"`subscribeClose()` observes close synchronously when it begins, after
    /// admission is sealed."* Runs from [`SessionMut::close`], after the one-way seal and before
    /// storage closes.
    pub fn subscribe_close<O: CloseObserver>(&self, observer: O) -> Disposer {
        self.0.observers.add_close(Box::new(observer))
    }

    /// Acquire a document state: one baseline and one buffering registration, as one value.
    ///
    /// `spec.md:1252`, `spec.md:3820-3824`. `Ok(None)` is absence — acquisition never creates and
    /// never stages a migration (`spec.md:1235-1237`). `cx` is the acquisition context and governs the
    /// subscription's lifetime (`spec.md:3900`).
    ///
    /// # Errors
    /// [`AuthorityPoisoned`] when a panic unwound through the authority.
    pub fn attach_doc_state<T>(
        &self,
        address: &DocumentAddress,
        cx: &Cx,
    ) -> Result<Option<Attachment<T>>, AuthorityPoisoned> {
        Ok(self
            .0
            .acquire(address)?
            .map(|(slot, disposer)| Attachment::new(slot, disposer, cx.clone())))
    }

    /// Acquire a document watch: the acquisition revision, with later frames buffering until
    /// [`DocWatch::start`].
    ///
    /// `spec.md:3848-3856`. `Ok(None)` is absence, on the same terms as [`Session::attach_doc_state`].
    ///
    /// # Errors
    /// [`AuthorityPoisoned`].
    pub fn watch_doc<T>(
        &self,
        address: &DocumentAddress,
        cx: &Cx,
    ) -> Result<Option<DocWatch<T>>, AuthorityPoisoned> {
        Ok(self
            .0
            .acquire(address)?
            .map(|(slot, disposer)| DocWatch::new(slot, disposer, cx.clone())))
    }

    /// How many incarnations have at least one registered document subscription.
    ///
    /// A test-only window onto the registry, so a case can assert that disposal and close actually
    /// drained it rather than leaving dead [`std::sync::Weak`] entries behind.
    #[cfg(test)]
    pub(crate) fn subscription_documents(&self) -> usize {
        self.0.subscriptions.documents()
    }

    /// How many synchronous observer calls have unwound in this Session.
    ///
    /// A diagnostic, in the same spirit as [`crate::hold::overruns`]: a panicking observer is a bug in
    /// the observer, and the kernel's obligation is to carry on and make the bug visible rather than to
    /// convert it into a durability incident. `src/tests/observation.rs` asserts against it.
    #[must_use]
    pub fn observer_panics(&self) -> u64 {
        self.0.observers.panics()
    }
}

/// **The one mutation handle per open store.**
///
/// Not `Clone`, never in an `Arc`, never behind a lock. It owns the storage handle, which is what
/// makes `Storage::commit(&mut self, ..)` satisfiable, and [`SessionMut::commit`] **consumes** it —
/// so two concurrent commits are not expressible and a dead Session cannot be reused.
pub struct SessionMut {
    shared: Arc<SessionShared>,
    storage: Box<dyn Storage>,
    /// One-way: `true` until [`SessionMut::close`] seals it, never back.
    admitting: bool,
}

impl core::fmt::Debug for SessionMut {
    /// Hand-written because `dyn Storage` is not `Debug`, and widening the trait to require it would
    /// put a formatting obligation on every backend for the sake of one line here.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SessionMut")
            .field("store", &self.shared.store)
            .field("admitting", &self.admitting)
            .finish_non_exhaustive()
    }
}

impl SessionMut {
    /// Open a Session over a storage handle.
    ///
    /// Returns both views: the clonable read handle, and the single mutation handle. Taking the
    /// `Box<dyn Storage>` **by value** here is where `G-SINGLE-COMMITTER` is established — a caller
    /// that kept a second handle to the same store never had one to keep.
    #[must_use]
    pub fn open(storage: Box<dyn Storage>) -> (Session, Self) {
        let shared = Arc::new(SessionShared {
            store: storage.store_id(),
            docs: DocIndex::new(),
            publications: AtomicU64::new(0),
            observers: Observers::default(),
            subscriptions: Subscriptions::default(),
        });
        let session = Session(Arc::clone(&shared));
        (
            session,
            Self {
                shared,
                storage,
                admitting: true,
            },
        )
    }

    /// A read handle onto the same Session.
    #[must_use]
    pub fn session(&self) -> Session {
        Session(Arc::clone(&self.shared))
    }

    /// Whether mutation admission is still open.
    #[must_use]
    pub const fn is_admitting(&self) -> bool {
        self.admitting
    }

    /// Run one commit, holding the mutation line from before the callback through publication.
    ///
    /// # The window
    ///
    /// `spec.md:1521-1526`: *"[the commit] owns the Session mutation line through callback execution,
    /// preparation, storage settlement, committed baseline adoption, and publication enqueue."* That
    /// is this function's body, in order, with no branch that leaves it early while holding something
    /// half-done — which is why F1 says the continuity of the hold is a property of the actor's
    /// straight-line code rather than of a type.
    ///
    /// # The outcome
    ///
    /// Four named variants. The handle is inside the three non-fatal ones and **absent** from
    /// [`CommitOutcome::Uncertain`], so a caller that wants to continue must have been given
    /// something to continue with (`G-INV-8`).
    pub async fn commit<R, F>(mut self, cx: &Cx, change: F) -> CommitOutcome<R>
    where
        F: for<'tx> AsyncFnOnce(Tx<'tx, Reading>) -> Result<(R, Tx<'tx, Writing>), CallbackError>,
    {
        if !self.admitting {
            return CommitOutcome::RolledBack {
                reason: RollbackReason::AdmissionSealed,
                session: self,
            };
        }
        // Held for exactly the window above, and dropped on every path out of it.
        let hold = LineHold::begin();
        let run = run_change(&mut *self.storage, &self.shared.docs, cx, change).await;
        let outcome = match run {
            Run::Nothing(result) => CommitOutcome::NothingToCommit {
                result,
                session: self,
            },
            Run::Committed {
                result,
                seq,
                publication,
            } => {
                // The one publish, consuming the one witness, while the line is still held.
                self.shared.publish(publication, cx);
                CommitOutcome::Committed {
                    result,
                    seq,
                    session: self,
                }
            }
            Run::RolledBack(reason) => CommitOutcome::RolledBack {
                reason,
                session: self,
            },
            // `self` is dropped here: the handle was moved into this function and is not returned.
            // There is nothing to put back, which is the whole of invariant 8 at the kernel.
            Run::Uncertain(u) => CommitOutcome::Uncertain(u),
        };
        drop(hold);
        outcome
    }

    /// Seal mutation admission and release the store.
    ///
    /// `spec.md:1545-1548`: *"Closing seals mutation admission and task reservation. Already-admitted
    /// commits settle before storage closes."* The second sentence is structural here rather than
    /// scheduled: an admitted commit **owns** this handle for its duration, so a close cannot begin
    /// while one is in flight — the borrow checker, not a queue, is what orders them.
    ///
    /// The seal is taken **first** and is one-way, so a failure to flush cannot reopen admission and a
    /// later commit is a [`RollbackReason::AdmissionSealed`] rather than a write into a closed store.
    ///
    /// # Why `&mut self` rather than `self`
    ///
    /// Consuming the handle would make the one-way flag unobservable: there would be no later commit
    /// to refuse, and `spec.md:908`'s *"every later `close()` awaits the same shutdown"* would have no
    /// receiver. The handle survives close, sealed — which is also what lets PICO5-PLAN S6's close
    /// observers run *after admission is sealed* and still have a Session to be about.
    ///
    /// # Errors
    /// [`StorageFailure`] when the final flush fails. A caller that has already published committed
    /// state cannot un-publish it, so this reports rather than poisons.
    pub async fn close(&mut self, cx: &Cx) -> Result<(), StorageFailure> {
        self.admitting = false;
        // After the seal and before storage closes, which is the window `spec.md:1530-1531` names.
        // `Closing` is not a Session: a close observer has nothing to re-enter.
        let closing = Closing::new(
            self.shared.store,
            self.shared.publications.load(Ordering::Acquire),
        );
        self.shared.observers.close_to(&closing);
        // `spec.md:1531`'s *"the Harness stops watches ... there"*. A watch whose Session has closed
        // can never receive another committed frame, so leaving it open would leave a consumer
        // waiting on something that cannot happen.
        self.shared.subscriptions.close_all();
        self.storage.close(cx).await
    }
}

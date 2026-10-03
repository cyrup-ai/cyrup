//! ADR-0030 F2's mutation transaction: `Tx<Reading> → Tx<Writing>`, consuming, with a [`Draft`] that
//! borrows it and no capability to reach anything outside it.
//!
//! # What the two states delete
//!
//! Upstream carries the same three guarantees with three mechanisms: a monotone `#hasTableWrite`
//! boolean plus a thrown `ReadAfterWrite` class for the ordering; a `#sealed` flag plus a revoked
//! `Proxy` plus `#assertOpen()` on every draft access and every `Tx` method for draft lifetime; and,
//! for effects and nested commits, **nothing at all** — a nested `session.commit()` is not diagnosed,
//! it queues on `#tail` and deadlocks the Session permanently.
//!
//! Here:
//!
//! * the four table readers exist on [`Tx<'_, Reading>`] and **not** on [`Tx<'_, Writing>`], so
//!   `ReadAfterWrite` is deleted as an error class rather than ported (`G-READ-BEFORE-FIRST-TABLE-WRITE`);
//! * [`Draft`] borrows the transaction, so the revoking `Proxy`, the `#sealed` flag and every
//!   per-access assertion are deleted — a draft that outlives the callback does not compile
//!   (`G-INV-6`);
//! * [`Tx`] holds no [`Session`] handle and no effect capability, and is `!Send` and not `'static`,
//!   so a nested commit has no method to call and a future borrowing the transaction cannot be
//!   spawned (`G-INV-4`, for the reachable path).
//!
//! The residue is stated rather than hidden: a closure that captured an `Arc<ModelClient>` from its
//! *environment* still compiles and still awaits inside the held line. That is `guarded`, not
//! `unrepresentable`, and [`crate::hold`] is the runtime check that catches it.
//!
//! # The S0 verdict, which this module is built on
//!
//! PICO5-PLAN S0 wrote the `for<'tx> AsyncFnOnce(Tx<'tx, Reading>) -> Result<(R, Tx<'tx, Writing>), E>`
//! bound against a real borrow checker and reported **yes on all three counts**: it compiles, it is
//! callable with an ordinary `async |tx| { .. }` with no annotation and no boxing, and the five
//! programs it is supposed to reject are rejected with readable errors. So `G-INV-4` and
//! `G-READ-BEFORE-FIRST-TABLE-WRITE` stay `typestate` in ADR-0030 §2 and the fallback is not taken.
//! S0 also found that the callback's error type **must be concrete** — a third generic `E` leaves an
//! infallible callback's `E` unconstrained and every call site needs an annotation — which is why
//! [`CallbackError`] is a named enum and not a parameter.
//!
//! [`Session`]: crate::Session

use core::marker::PhantomData;
use std::sync::Arc;

use cyrup_pico_doc::{
    BaseRequirement, CheckpointInput, DefVersion, DocRoot, DocValue, OpenChange, PathError,
    Representation, StoredVersion, VersionFit, choose_representation, classify_version,
};
use cyrup_pico_store::{
    AlreadyStaged, Batch, BatchBuilder, CommitError, CommittedEntry, ContextEdit,
    ConversationCursor, ConversationId, ConversationOwner, ConversationParent, ConversationQuery,
    ConversationRecord, ConversationSemantics, CopySource, Cx, DocumentAddress, DocumentBase,
    DocumentContent, DocumentCreate, DocumentCursor, DocumentId, DocumentPoint, DocumentQuery,
    DocumentRecord, DocumentScope, EntryCursor, EntryId, EntryQuery, EntryRecord, HeadMarker, Id,
    IdKind, Kind, Page, PageLimit, Retire, RewindableFork, ScopeRef, Seq, Storage, StorageExt,
    StorageFailure, SubmissionId, SubmissionRecord, TaskCursor, TaskId, TaskQuery, TaskRecord,
};

use crate::def::{DocAddress, DocDef, Migration};
use crate::docs::{AuthorityPoisoned, DocIndex, Swap};
use crate::fork::{Fork, ForkedDocument, PendingCopy};
use crate::outcome::{RollbackReason, UncertainCommit};
use crate::witness::{AdoptionCause, AdoptionFailed, Change, Copied, Durable, Publication};

/// The page the fork enumerates the parent's document records in.
///
/// A `const`, so the number is parsed at compile time and the impossible arm is a *compile* error
/// rather than a runtime one: [`PageLimit`] has no `Default` and no `From<u32>` (ADR-0030 F6 §B), so
/// there is no honest value to fall back to and nothing to `unwrap`. 256 and not
/// [`PageLimit::MAX`]: a page is materialised into one owned `Vec` by every backend, and a
/// conversation with more than 256 live documents should cost several small reads rather than one
/// enormous allocation on the held line.
const FORK_SCAN_PAGE: PageLimit = match PageLimit::parse(256) {
    Ok(limit) => limit,
    // 256 is between 1 and `PageLimit::MAX`, so this arm is evaluated by `const` evaluation and
    // never at run time.
    Err(_) => unreachable!(),
};

/// The storage handle a transaction borrows.
///
/// A named alias rather than `dyn Storage + 'tx`, and the reason is a borrow-checker fact rather than
/// a style choice: `&mut T` is **invariant** in `T`, so `&mut (dyn Storage + 'a)` does not coerce to
/// `&mut (dyn Storage + 'tx)` even when `'a: 'tx`. [`crate::SessionMut`] owns a
/// `Box<dyn Storage>` — that is, `dyn Storage + 'static` — so pinning the trait object's lifetime to
/// `'static` here is what lets the transaction's own `'tx` be the generative region it has to be.
type DynStorage = dyn Storage + 'static;

/// The transaction has taken no table write yet, so every table reader is available.
///
/// A unit struct with a **private** field: there is no third state, and no state can be named from
/// outside this crate. ADR-0030 §10.
#[derive(Debug)]
pub struct Reading(());

/// The transaction has taken its first table write, so the table readers are gone.
///
/// Reached only through [`Tx::writing`], which **consumes** the `Reading` transaction — so the
/// transition is how the transaction gets home rather than optional bookkeeping. A callback that
/// writes nothing calls `writing()` as its last statement; it is infallible and free.
#[derive(Debug)]
pub struct Writing(());

/// A checkpoint predicate, as `spec.md:1389-1401` defines it.
///
/// A plain `fn` pointer over [`CheckpointInput`] — which carries exactly
/// `(candidate, ops, deltas_since_base)` and no storage handle — so *"evaluation performs no Storage
/// read"* is a property of the type. A pointer rather than a boxed closure because a definition is a
/// **type** ([`DocDef`]) and has nothing per-instance to capture, so the box would be an allocation
/// per acquisition that bought nothing, and the pointer is `Copy + Send + Sync` for free.
///
/// [`DocDef`]: crate::DocDef
pub type CheckpointPredicate = fn(CheckpointInput<'_>) -> bool;

/// What the typed acquisition hands the untyped get-or-create path.
///
/// **Crate-private, and that is `G-ONLY-TYPED-ACQUISITION-CREATES`.** `spec.md:1219` is explicit —
/// *"only public typed `tx.doc()` is get-or-create"* — and S4 could only honour it by convention,
/// because its `Tx::doc` was public and took an already-evaluated initial value. Now
/// [`Tx::doc_at`] is `pub(crate)` and this is its argument, so an application has no creating path
/// that does not go through a [`DocToken`](crate::DocToken).
///
/// # Why `initial` and `migrate` are closures and not values
///
/// `spec.md:1230-1232` calls `initial()` **only** when the address is empty, and
/// `spec.md:1447-1449` calls `migrate()` **only** when the stored version is older. An
/// already-evaluated value would run the definition's code on every acquisition of an existing
/// current-version document — which is both wasted work and, for a family, a consumed seed where
/// `spec.md:1232-1234` says *"later seeds are ignored"*. Taking thunks makes "consumed only when
/// absent" structural: the closure is moved into a branch that is not taken.
pub(crate) struct Seed<I> {
    /// The full scope, **with** its policy, as the typed address carries it.
    ///
    /// Not checked against the address any more, and the deleted check is the point: a
    /// [`DocAddress`] writes both from one argument in one constructor, so
    /// S4's `TxError::ScopeDisagreesWithAddress` describes a state with no spelling. What remains
    /// checked is the agreement with the *persisted record*
    /// ([`TxError::TokenDisagreesWithRecord`]), which `spec.md:1063-1065` requires to stay a check.
    pub(crate) scope: DocumentScope,
    /// The definition version the token speaks.
    pub(crate) version: DefVersion,
    /// Builds the value to store if the address is empty.
    pub(crate) initial: I,
    /// Migrates an older stored value. `None` is `spec.md:1444`'s *"no migrate"* arm.
    pub(crate) migrate: Option<MigrateThunk>,
    /// The definition's checkpoint predicate, if it declared one.
    pub(crate) checkpoint: Option<CheckpointPredicate>,
}

/// A boxed, already-addressed migration step.
///
/// Boxed rather than a second generic parameter on [`Seed`], for a type-inference reason worth
/// stating: with `Option<M>` generic, a `Seed` whose migration is `None` has **no type to infer `M`
/// from**, so every call site that does not migrate would need a turbofish naming a function type it
/// never calls. The box costs one allocation and only for a definition that declares a migration.
pub(crate) type MigrateThunk = Box<dyn FnOnce(Migration<'_>) -> Result<DocRoot, TxError>>;

/// Whatever a table read or a document acquisition can fail with.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TxError {
    /// A storage read failed. **Not** absence: absence is `Ok(None)` (`G-CORRUPTION-NOT-ABSENCE`).
    #[error("a transaction read failed")]
    Storage(#[from] StorageFailure),
    /// Minting an id failed. Carries a [`CommitError`] because a failure to move the durable
    /// high-water mark has the same two classes as a commit.
    #[error("minting an id failed")]
    Mint(#[source] CommitError),
    /// A path operation on a draft was illegal.
    #[error("a draft operation was illegal")]
    Path(#[from] PathError),
    /// The token's scope or conversation policy disagrees with the **persisted record's**.
    ///
    /// `spec.md:1063-1065`: *"typed access rejects when the token's scope or conversation
    /// history/fork policy disagrees with the persisted incarnation. A migration cannot reinterpret
    /// those lifetime semantics."*
    ///
    /// **A runtime check, and correctly so.** ADR-0030 §2.3 classifies it `checked` and
    /// `spec.md:1117-1120` says why: *"the record preserves scope and conversation history/fork
    /// semantics so unavailable extension code does not make existing data disappear."* Extension
    /// code reloads independently of its data, so the only way a redefined token can be caught is by
    /// comparing it against what is stored — and the record wins, because preferring the token is how
    /// a `latest` document is reinterpreted as `rewindable` and historical reads start answering from
    /// legitimately reclaimed records.
    #[error("document {address} is stored with different scope semantics than this token declares")]
    TokenDisagreesWithRecord {
        /// The address.
        address: DocumentAddress,
        /// What the record says. Boxed to keep this variant from widening the whole enum.
        stored: Box<DocumentScope>,
        /// What the token declared.
        token: Box<DocumentScope>,
    },
    /// The stored records are **newer** than the definition this caller speaks
    /// (`spec.md:1443`, the ladder's third arm).
    ///
    /// Extension code older than its own data must not reinterpret it. There is no migration in this
    /// direction and inventing one would silently discard fields a newer definition added.
    #[error(
        "document {address} is stored at definition version {stored}, newer than the {token} this caller speaks"
    )]
    StoredVersionIsNewer {
        /// The address.
        address: DocumentAddress,
        /// The version the stored records are at.
        stored: DefVersion,
        /// The version the token speaks.
        token: DefVersion,
    },
    /// The stored records are older and the definition declared no migration
    /// (`spec.md:1444`, the ladder's fourth arm).
    ///
    /// The honest answer, because applying the current definition's paths against an older shape
    /// produces *plausible-looking corrupt state, not an error* (ADR-0030 §2.3).
    #[error(
        "document {address} is stored at definition version {stored}, older than the {token} this caller speaks, and the definition declares no migration"
    )]
    NoMigration {
        /// The address.
        address: DocumentAddress,
        /// The version the stored records are at.
        stored: DefVersion,
        /// The version the token speaks.
        token: DefVersion,
    },
    /// The definition's migration callback refused the stored value.
    ///
    /// `spec.md:1459-1460`: *"callback failure persists nothing"*. Raised before storage admission,
    /// so this is a [`RollbackReason`] arm and never a poison.
    #[error("migrating document {address} from version {from} failed")]
    MigrationFailed {
        /// The address.
        address: DocumentAddress,
        /// The version the migration started from.
        from: DefVersion,
        /// What the callback said.
        #[source]
        cause: crate::MigrationFailed,
    },
    /// A definition's `initial()` or a migration's result is not a strict-JSON object.
    ///
    /// `spec.md:1065`: *"`initial()` and `migrate()` return JSON objects."* The value crosses
    /// [`DocRoot::parse`], so a non-finite float or a non-object root is caught here rather than
    /// reaching storage — which is the failure ADR-0030 F4 calls total loss, detected one restart
    /// later.
    #[error("the definition's value for {address} is not a strict-JSON object")]
    NotStrictJson {
        /// The address.
        address: DocumentAddress,
        /// What was wrong with it.
        #[source]
        cause: cyrup_pico_doc::NotStrictJson,
    },
    /// The batch already holds a command for that record.
    #[error("that record is already staged in this transaction")]
    AlreadyStaged(#[from] AlreadyStaged),
    /// A handle from another transaction, or from before a retirement.
    ///
    /// Unreachable while the brand holds: a [`DocHandle`] carries the generative `'tx`, so one minted
    /// under another `commit` call is a *different type*. The arm exists because the alternative is
    /// `unwrap`, which the workspace denies and which would turn an impossible state into a panic on
    /// the held line.
    #[error("that document handle does not belong to this transaction")]
    ForeignHandle,
    /// A panic unwound through the document authority.
    #[error("the document authority is unusable")]
    Authority(#[from] AuthorityPoisoned),
    /// An owning task named for a new conversation is not in this transaction or in storage.
    ///
    /// `spec.md:1213-1216`: the persisted owner pair is **derived** from the named task's final
    /// candidate record, so a name that resolves to nothing is a pre-admission rejection rather than a
    /// conversation with a dangling owner.
    #[error("task {0} is not staged in this transaction and not in storage")]
    UnknownOwnerTask(TaskId),
    /// A fork named an entry that is not in the parent's **visible** history.
    ///
    /// `spec.md:258-259`: *"`parent.at` is an entry in the parent history visible to the child."*
    /// Visibility is a cross-record fact — every `parent.at` cap on the way up the ancestry — which
    /// `spec.md:4272-4274` keeps the Session's to establish and
    /// [`Storage::entry_in`] answers inside the backend so a
    /// caller cannot answer it by scanning. A fork point that is not visible would give the child a
    /// transcript cap pointing at an entry it cannot see, so this is a pre-admission rejection.
    #[error("entry {at} is not visible in conversation {conversation_id}'s history")]
    ForkPointNotVisible {
        /// The parent conversation the fork named.
        conversation_id: ConversationId,
        /// The entry it named.
        at: EntryId,
    },
    /// A typed acquisition of a forked child's address could not read the source it copies.
    ///
    /// `spec.md:4314-4315`: *"the source must be an alive conversation document at the selected
    /// point."* The enumeration that selected it read committed state, so reaching this means the
    /// source's **content** is gone where its record said it was alive — which for a `latest`
    /// document is legitimate reclamation racing nothing (the Session holds the line), and is
    /// therefore reported rather than guessed at. A copy left unaccessed fails the same way, but in
    /// the backend and as [`RejectedReason::CopySourceNotAlive`].
    ///
    /// [`RejectedReason::CopySourceNotAlive`]: cyrup_pico_store::RejectedReason::CopySourceNotAlive
    ///
    /// The field is named `document` and not `source` because `thiserror` reserves that name for the
    /// error cause, and the cause here is not an error — it is a record.
    #[error("the source {document} of the forked document at {address} cannot be read at {at}")]
    ForkSourceNotReadable {
        /// The source incarnation.
        document: DocumentId,
        /// The point it was selected at.
        at: DocumentPoint,
        /// The child's address.
        address: DocumentAddress,
    },
}

/// What a commit callback is allowed to fail with.
///
/// **Concrete, and that is part of S0's answer rather than a simplification.** With the error as a
/// third generic parameter, a callback that cannot fail leaves it unconstrained: S0 tried that form
/// first and six of its nine compile-fail cases stopped failing for their own reason and started
/// failing with `E0282 type annotations needed`, which is also what every infallible real call site
/// would do.
#[derive(Debug, thiserror::Error)]
pub enum CallbackError {
    /// A transaction read, acquisition or staging failed.
    #[error("the transaction failed")]
    Tx(#[from] TxError),
    /// The callback decided to abandon the change for a reason of its own.
    #[error("the commit callback abandoned the change")]
    Abandoned(#[source] Box<dyn core::error::Error + Send + Sync>),
}

/// A handle to a document acquired in **this** transaction.
///
/// The `PhantomData<&'tx mut ()>` brand is **invariant**, so a handle minted under one `commit`
/// call's `'tx` is a different type from one minted under another's — a hazard upstream has no
/// concept of, and the one case whose diagnostic S0 found poor.
///
/// # The error you will see, and its fix
///
/// S0's verdict singles this out: four of its five negative cases name the thing the developer did,
/// and this one does not. Passing a handle from one `commit` to another is rejected with a bare
/// ``lifetime may not live long enough ... requires that `'1` must outlive `'2` `` which says nothing
/// about handles or transactions. The fix is always the same shape — acquire the document again
/// inside the second transaction:
///
/// ```ignore
/// // Rejected: `live` is branded with the first transaction's 'tx.
/// let live = session.commit(async |tx| { let h = tx.doc(&addr, seed).await?; Ok((h, tx.writing())) }).await;
/// session.commit(async |mut tx| { let d = tx.draft(live)?; /* ... */ }).await;
///
/// // Accepted: the handle is acquired where it is used. The acquisition is memoised per
/// // transaction, so this is not a second storage read within one commit.
/// session.commit(async |mut tx| { let h = tx.doc(&addr, seed).await?; let d = tx.draft(h)?; /* ... */ }).await;
/// ```
#[derive(Debug)]
pub struct DocHandle<'tx, T> {
    index: u32,
    _brand: PhantomData<&'tx mut ()>,
    _value: PhantomData<fn() -> T>,
}

impl<T> Clone for DocHandle<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for DocHandle<'_, T> {}

/// How one slot relates to what was already stored at its address.
///
/// **This enum is the required-base flag, and making it an enum rather than a `bool` beside
/// `created` is what makes `spec.md:1463-1465` survive coalescing.** A slot is created once, at the
/// first acquisition of its address; every later acquisition in the transaction returns the same slot
/// ([`Tx::doc_at`]'s memoisation), and there is no method that rewrites this field. So *"later draft
/// edits coalesce into one final required base"* holds because the requirement cannot be cleared, not
/// because nothing clears it.
enum Continuity {
    /// The slot continues the authority's current revision at the stored version.
    /// [`BaseRequirement::Optional`]: the definition's predicate decides.
    Continue,
    /// This transaction creates the incarnation. *"Creation always stores a complete base"*
    /// (`spec.md:1386`).
    Created(cyrup_pico_store::DocumentCreate),
    /// This transaction migrated the stored value to the token's version.
    ///
    /// `spec.md:1463-1465`: *"the first `tx.doc()` transaction after any stored-version migration
    /// writes a required current-version base, even when the migrated JSON is deeply equal."* The
    /// base is required for a storage reason as well as a semantic one — `spec.md:4366-4367`'s
    /// *"deltas cannot cross a stored version boundary"* — and a required base is one rule that
    /// satisfies both.
    Migrated,
}

impl Continuity {
    /// Whether a base is required regardless of the definition's predicate.
    const fn base(&self) -> BaseRequirement {
        match self {
            Self::Continue => BaseRequirement::Optional,
            Self::Created(_) | Self::Migrated => BaseRequirement::Required,
        }
    }
}

/// One document in flight in this transaction.
struct DocSlot {
    address: DocumentAddress,
    id: DocumentId,
    /// The open change, or `None` once the slot was retired without content.
    change: Option<OpenChange>,
    /// How this slot relates to what was stored. Set once; see [`Continuity`].
    continuity: Continuity,
    version: DefVersion,
    /// The witness a storage read produced. `None` for a creation, which has nothing to continue.
    stored: Option<StoredVersion>,
    deltas_since_base: u32,
    retire: Retire,
    checkpoint: Option<CheckpointPredicate>,
}

/// Everything preparation computed, waiting for adoption to move it.
struct Staged {
    swaps: Vec<Swap>,
    changes: Arc<[Change]>,
    copies: Arc<[Copied]>,
}

/// What the transaction owns for the duration of one change.
///
/// `&'tx mut DynStorage` is the shape that matters: it is why a transaction cannot be duplicated and
/// why two cannot coexist. There is **no `Session` handle and no effect capability** here, which is
/// how invariant 4's reachable path and the nested-commit deadlock are closed.
pub(crate) struct TxInner<'tx> {
    storage: &'tx mut DynStorage,
    docs: &'tx DocIndex,
    cx: &'tx Cx,
    builder: BatchBuilder,
    slots: Vec<DocSlot>,
    /// Each task staged in **this** transaction, with its conversation.
    ///
    /// `spec.md:1216-1219`: a new conversation's or task's owner is judged *"on the owner's final
    /// candidate record in that transaction"* — a task committed or staged earlier in the same
    /// transaction counts. [`Batch`] exposes its five maps only after
    /// [`BatchBuilder::build`](cyrup_pico_store::BatchBuilder::build), which is deliberate (F3: the
    /// assembler is the only builder), so the candidate set the Session needs is kept here rather than
    /// read back out of a half-built batch.
    staged_tasks: Vec<(TaskId, ConversationId)>,
    /// Every document copy a fork staged in **this** transaction (PICO5-PLAN S9).
    ///
    /// Held here rather than in the [`BatchBuilder`] for the reason
    /// [`PendingCopy`] documents: `spec.md:1500-1502` lets a typed
    /// acquisition **replace** a copy with an ordinary create, and a builder has no removal path.
    copies: Vec<PendingCopy>,
    staged: Option<Staged>,
}

impl<'tx> TxInner<'tx> {
    fn new(storage: &'tx mut DynStorage, docs: &'tx DocIndex, cx: &'tx Cx) -> Self {
        Self {
            storage,
            docs,
            cx,
            builder: BatchBuilder::new(),
            slots: Vec::new(),
            staged_tasks: Vec::new(),
            copies: Vec::new(),
            staged: None,
        }
    }
}

/// The mutation transaction, in state `S`.
///
/// ADR-0030 §10's field list: `'tx` is **invariant** (that is what `PhantomData<&'tx mut ()>` buys,
/// so the borrow cannot be widened), not `Send`, not `Sync`, not `Clone`, not `'static`.
pub struct Tx<'tx, S> {
    inner: &'tx mut TxInner<'tx>,
    _state: PhantomData<S>,
    _not_send: PhantomData<*const ()>,
    _invariant: PhantomData<&'tx mut ()>,
}

impl<'tx> Tx<'tx, Reading> {
    fn open(inner: &'tx mut TxInner<'tx>) -> Self {
        Self {
            inner,
            _state: PhantomData,
            _not_send: PhantomData,
            _invariant: PhantomData,
        }
    }

    /// One conversation by id. **A table read.**
    ///
    /// # Errors
    /// [`TxError::Storage`]. Absence is `Ok(None)`.
    pub async fn conversation(
        &mut self,
        id: ConversationId,
    ) -> Result<Option<ConversationRecord>, TxError> {
        Ok(self.inner.storage.conversation(id, self.inner.cx).await?)
    }

    /// One entry, with the commit sequence a historical document read needs. **A table read.**
    ///
    /// # Errors
    /// [`TxError::Storage`]. Absence is `Ok(None)`.
    pub async fn entry(&mut self, id: EntryId) -> Result<Option<CommittedEntry>, TxError> {
        Ok(self.inner.storage.entry(id, self.inner.cx).await?)
    }

    /// One task by id. **A table read.**
    ///
    /// # Errors
    /// [`TxError::Storage`]. Absence is `Ok(None)`.
    pub async fn task(&mut self, id: TaskId) -> Result<Option<TaskRecord>, TxError> {
        Ok(self.inner.storage.task(id, self.inner.cx).await?)
    }

    /// One submission by id. **A table read.**
    ///
    /// # Errors
    /// [`TxError::Storage`]. Absence is `Ok(None)`.
    pub async fn submission(
        &mut self,
        id: SubmissionId,
    ) -> Result<Option<SubmissionRecord>, TxError> {
        Ok(self.inner.storage.submission(id, self.inner.cx).await?)
    }

    /// One submission by its idempotency request id. **A table read.**
    ///
    /// # Errors
    /// [`TxError::Storage`]. Absence is `Ok(None)`.
    pub async fn submission_by_request(
        &mut self,
        conversation_id: ConversationId,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, TxError> {
        Ok(self
            .inner
            .storage
            .submission_by_request(conversation_id, request_id, self.inner.cx)
            .await?)
    }

    /// The newest visible entry of a conversation that carries a head marker. **A table read.**
    ///
    /// # Errors
    /// [`TxError::Storage`]. Absence is `Ok(None)`.
    pub async fn latest_head_marker(
        &mut self,
        conversation_id: ConversationId,
        at_or_before: Option<EntryId>,
    ) -> Result<Option<HeadMarker>, TxError> {
        Ok(self
            .inner
            .storage
            .find_latest_head_marker(conversation_id, at_or_before, self.inner.cx)
            .await?)
    }

    /// Scan conversations. **A table read.**
    ///
    /// # Errors
    /// [`TxError::Storage`].
    pub async fn scan_conversations(
        &mut self,
        query: &ConversationQuery,
        limit: PageLimit,
        from: Option<ConversationCursor>,
    ) -> Result<Page<ConversationRecord, ConversationCursor>, TxError> {
        Ok(self
            .inner
            .storage
            .scan_conversations(query, limit, from, self.inner.cx)
            .await?)
    }

    /// Scan entries. **A table read.**
    ///
    /// # Errors
    /// [`TxError::Storage`].
    pub async fn scan_entries(
        &mut self,
        query: &EntryQuery,
        limit: PageLimit,
        from: Option<EntryCursor>,
    ) -> Result<Page<EntryRecord, EntryCursor>, TxError> {
        Ok(self
            .inner
            .storage
            .scan_entries(query, limit, from, self.inner.cx)
            .await?)
    }

    /// Scan tasks. **A table read.**
    ///
    /// # Errors
    /// [`TxError::Storage`].
    pub async fn scan_tasks(
        &mut self,
        query: &TaskQuery,
        limit: PageLimit,
        from: Option<TaskCursor>,
    ) -> Result<Page<TaskRecord, TaskCursor>, TxError> {
        Ok(self
            .inner
            .storage
            .scan_tasks(query, limit, from, self.inner.cx)
            .await?)
    }

    /// Take the first table write.
    ///
    /// **Consuming**, and taken whether or not the write that follows succeeds — upstream's behaviour
    /// exactly (`#write` sets `#hasTableWrite` before writing). Infallible and free: a callback that
    /// writes nothing calls this as its last statement.
    #[must_use]
    pub fn writing(self) -> Tx<'tx, Writing> {
        Tx {
            inner: self.inner,
            _state: PhantomData,
            _not_send: PhantomData,
            _invariant: PhantomData,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Available in BOTH states — ADR-0030 F2's deliberate asymmetry, from `spec.md:1556`: document
// access and read-your-writes survive the first table write.
// ---------------------------------------------------------------------------------------------
impl<'tx, S> Tx<'tx, S> {
    /// Allocate one id from the store's single global namespace.
    ///
    /// # Errors
    /// [`TxError::Mint`].
    pub async fn mint<K: IdKind>(&mut self) -> Result<Id<K>, TxError> {
        self.inner
            .storage
            .mint(self.inner.cx)
            .await
            .map_err(TxError::Mint)
    }

    /// Get-or-create the document one typed address names, returning a handle branded with this
    /// transaction.
    ///
    /// **This is the only creating path** (`spec.md:1219`: *"only public typed `tx.doc()` is
    /// get-or-create"*). The untyped [`Tx::doc_at`] it delegates to is `pub(crate)`, so an
    /// application cannot create an incarnation without a [`DocToken`](crate::DocToken) — which is
    /// what makes the kind, the version, the scope policy and the initial value all come from one
    /// definition instead of from four arguments.
    ///
    /// # What each of the definition's three callbacks costs
    ///
    /// * `initial(seed)` runs **only** when the address is empty (`spec.md:1230-1232`). The seed is
    ///   moved into that branch, so for a family *"the first call's detached seed wins and later
    ///   seeds are ignored"* (`spec.md:1232-1234`) is true because a later seed is moved into a call
    ///   that does not happen.
    /// * `migrate(value, from)` runs **only** on the ladder's `older` arm (`spec.md:1447-1449`).
    /// * `checkpoint_when(..)` does not run here at all — it runs once, in
    ///   [`Tx::prepare`], after preparation (`spec.md:1395`).
    ///
    /// # Errors
    ///
    /// [`TxError::Storage`] for a failed read; [`TxError::TokenDisagreesWithRecord`] when the stored
    /// record's scope semantics are not the token's; [`TxError::StoredVersionIsNewer`],
    /// [`TxError::NoMigration`] and [`TxError::MigrationFailed`] for the migration ladder's three
    /// failing arms; [`TxError::NotStrictJson`] when a definition's value is not a JSON object;
    /// [`TxError::Mint`] for a failed allocation.
    pub async fn doc<D: DocDef>(
        &mut self,
        at: &DocAddress<D>,
        seed: D::Seed,
    ) -> Result<DocHandle<'tx, D::Value>, TxError> {
        let address = at.address().clone();
        let initial = {
            let address = address.clone();
            move || {
                DocRoot::parse(&D::initial(seed))
                    .map_err(|cause| TxError::NotStrictJson { address, cause })
            }
        };
        let migrate: Option<MigrateThunk> = D::MIGRATE.map(|callback| {
            let address = address.clone();
            let thunk: MigrateThunk = Box::new(move |m: Migration<'_>| {
                let from = m.from();
                let value = callback(m).map_err(|cause| TxError::MigrationFailed {
                    address: address.clone(),
                    from,
                    cause,
                })?;
                DocRoot::parse(&value).map_err(|cause| TxError::NotStrictJson { address, cause })
            });
            thunk
        });
        self.doc_at(
            at.address(),
            Seed {
                scope: at.scope().clone(),
                version: at.version(),
                initial,
                migrate,
                checkpoint: D::CHECKPOINT_WHEN,
            },
        )
        .await
    }

    /// Retire the document one typed address names, **without creating it**.
    ///
    /// `spec.md:1242-1246`: *"`retireDoc()` resolves the logical address without creating it.
    /// Retirement of an acquired draft persists its final content before retirement. A later
    /// `tx.doc()` at that address in the same transaction creates a new incarnation with a new draft
    /// and ID."* All three clauses are here: an absent address is `Ok(())`, an already-acquired slot
    /// keeps its change so its content is written with the retirement as **one**
    /// [`DocumentCommand`](cyrup_pico_store::DocumentCommand) (ADR-0030 F3, so the ordering is
    /// structural rather than maintained), and a retired slot is skipped by the memoisation in
    /// [`Tx::doc_at`] so a later acquisition mints a new incarnation.
    ///
    /// # Errors
    /// [`TxError::Storage`], [`TxError::TokenDisagreesWithRecord`].
    pub async fn retire_doc<D: DocDef>(&mut self, at: &DocAddress<D>) -> Result<(), TxError> {
        if let Some(index) = self.live_slot(at.address()) {
            // `live_slot` produced this index from `slots.iter().position`, so the lookup cannot
            // miss. It is written as an error arm rather than an index because the workspace denies
            // both `unwrap` and raw indexing, and a panic on the held line is the worse of the two.
            let slot = self
                .inner
                .slots
                .get_mut(index)
                .ok_or(TxError::ForeignHandle)?;
            slot.retire = Retire::Retire;
            return Ok(());
        }
        // A fork's copy is staged but not committed, so the storage read below would answer `None`
        // and the copy would land unretired. `DocumentCommand::Copy` carries its own `then: Retire`
        // for exactly this (ADR-0030 F3: content and retirement are one value), so retiring a
        // not-yet-acquired copy is one field and not a second command.
        if let Some(index) = self.pending_copy(at.address()) {
            let copy = self
                .inner
                .copies
                .get_mut(index)
                .ok_or(TxError::ForeignHandle)?;
            copy.retire = Retire::Retire;
            return Ok(());
        }
        let found = self
            .inner
            .storage
            .find_document(at.address(), DocumentPoint::Current, self.inner.cx)
            .await?;
        let Some(record) = found else {
            // "Resolves the logical address without creating it." Nothing is there, so nothing is
            // retired and nothing is staged: this commit may still be `NothingToCommit`.
            return Ok(());
        };
        agrees(at.address(), &record.scope, at.scope())?;
        // Read the final content and open a change over it, **without** consulting the version
        // ladder. Two reasons, and they pull the same way:
        //
        // * the publication has to carry the retired incarnation's final value, because that is what
        //   terminates an observer's view of it (PICO5-PLAN S6) — a retirement with no
        //   [`Change`](crate::Change) is a document that silently stops existing;
        // * a retirement does not *interpret* the value, so refusing to retire a document whose
        //   stored version this caller cannot migrate would strand exactly the data
        //   `spec.md:1468-1469` says must stay reachable. The empty prepared batch means
        //   `Representation::Nothing`, so the content is **not** rewritten either.
        let stored = self
            .inner
            .storage
            .document(record.id, DocumentPoint::Current, self.inner.cx)
            .await?;
        let (change, witness, deltas) = match stored {
            Some(stored) => {
                let change = match self.inner.docs.begin_change(record.id)? {
                    Some(change) => change,
                    None => self.inner.docs.load(
                        record.id,
                        at.address().clone(),
                        stored.value.clone(),
                    )?,
                };
                (Some(change), Some(stored.version), stored.deltas_since_base)
            }
            // A record whose content is absent is corruption, not absence
            // (`spec.md:4352-4360`) — but it is still retirable, and inventing a final value for it
            // would be worse than publishing none.
            None => (None, None, 0),
        };
        self.inner.slots.push(DocSlot {
            address: at.address().clone(),
            id: record.id,
            change,
            continuity: Continuity::Continue,
            version: at.version(),
            stored: witness,
            deltas_since_base: deltas,
            retire: Retire::Retire,
            checkpoint: None,
        });
        Ok(())
    }

    /// The index of the slot already open at `address` and not retired in this transaction.
    fn live_slot(&self, address: &DocumentAddress) -> Option<usize> {
        self.inner
            .slots
            .iter()
            .position(|slot| &slot.address == address && !slot.retire.is_retire())
    }

    /// The index of the fork copy staged at `address` that no typed acquisition has replaced yet.
    fn pending_copy(&self, address: &DocumentAddress) -> Option<usize> {
        self.inner
            .copies
            .iter()
            .position(|copy| &copy.address == address && !copy.consumed)
    }

    /// Replace a staged fork copy with an ordinary child create, over the source's value.
    ///
    /// `spec.md:1465-1467` and `:1500-1502` together: *"a fork obtains the selected stored
    /// value/version from Storage rather than a typed migrated tracker cache. The child copies that
    /// stored pair and migrates on later typed access"*, and that access *"replaces the copy with one
    /// ordinary child create containing the final prepared value"*.
    ///
    /// The read is of **committed** state at the point the fork selected, so it is the same value
    /// storage would have materialised had the copy been left unaccessed — which is what keeps the
    /// child independent of where in the callback this acquisition happens.
    ///
    /// # Errors
    ///
    /// [`TxError::TokenDisagreesWithRecord`] when the token redeclares the policy the **parent's**
    /// record carried into the child create; [`TxError::ForkSourceNotReadable`]; the migration
    /// ladder's three failing arms.
    async fn acquire_copy<T, I>(
        &mut self,
        index: usize,
        seed: Seed<I>,
    ) -> Result<DocHandle<'tx, T>, TxError>
    where
        I: FnOnce() -> Result<DocRoot, TxError>,
    {
        // `ForeignHandle` is unreachable: `index` came from `pending_copy`'s own `position`. It is an
        // error arm rather than an index because the workspace denies raw indexing and a panic on the
        // held line is the worse of the two.
        let (create, source, address, retire) = {
            let copy = self.inner.copies.get(index).ok_or(TxError::ForeignHandle)?;
            (
                copy.create.clone(),
                copy.source,
                copy.address.clone(),
                copy.retire,
            )
        };
        // The child record's semantics were copied from the PARENT'S persisted record, so this is the
        // same check an ordinary acquisition makes and for the same reason: the record wins.
        agrees(&address, &create.scope, &seed.scope)?;
        let stored = self
            .inner
            .storage
            .document(source.id, source.at, self.inner.cx)
            .await?
            .ok_or_else(|| TxError::ForkSourceNotReadable {
                document: source.id,
                at: source.at,
                address: address.clone(),
            })?;
        let value = match classify_version(stored.version, seed.version, seed.migrate.is_some()) {
            VersionFit::Current => stored.value,
            VersionFit::Migrate { from, to } => {
                let Some(migrate) = seed.migrate else {
                    // Unreachable: `classify_version` returns this arm only when `has_migration` was
                    // true, and that argument *is* this `Option`.
                    return Err(TxError::NoMigration {
                        address,
                        stored: from,
                        token: to,
                    });
                };
                migrate(Migration::new(&stored.value, from, to))?
            }
            VersionFit::StoredIsNewer { stored, token } => {
                return Err(TxError::StoredVersionIsNewer {
                    address,
                    stored,
                    token,
                });
            }
            VersionFit::NoMigration { stored, token } => {
                return Err(TxError::NoMigration {
                    address,
                    stored,
                    token,
                });
            }
        };
        if let Some(copy) = self.inner.copies.get_mut(index) {
            // The descriptor stays, with `consumed` set: `Tx::prepare` still has to decide whether
            // writing this source in the same batch is legal, and `spec.md:1491-1494`'s answer for a
            // `current` fork does not change just because the copy became a create.
            copy.consumed = true;
        }
        // A creation's change is opened over the source's value, so the draft reads what it inherited
        // and preparation has a complete candidate either way (`spec.md:1239`).
        let change = cyrup_pico_doc::Tracker::track(value).begin_change();
        let slot = DocSlot {
            address,
            id: create.id,
            change: Some(change),
            continuity: Continuity::Created(create),
            version: seed.version,
            // A creation writes a complete base, so there is no stored version to continue.
            stored: None,
            deltas_since_base: 0,
            retire,
            checkpoint: seed.checkpoint,
        };
        let slot_index = self.inner.slots.len();
        self.inner.slots.push(slot);
        Ok(handle(slot_index))
    }

    /// Get-or-create at one **untyped** address. `pub(crate)`: see [`Seed`].
    ///
    /// # Memoisation
    ///
    /// `spec.md:1232-1234`: *"The first acquisition of one logical address is memoized before
    /// awaiting. Later acquisitions in that transaction return the same draft; for a missing family,
    /// the first call's detached seed wins and later seeds are ignored."*
    ///
    /// Upstream has to memoise *before* awaiting because two JavaScript acquisitions of one address
    /// can be in flight at once. Here they cannot: this method takes `&mut self`, so a second
    /// acquisition cannot begin until the first has returned, and the slot is registered before the
    /// method yields control back to the callback. Both halves of the rule are therefore true for a
    /// stronger reason than upstream's — the same draft is returned because the slot is found, and a
    /// later seed is ignored because it is never consulted.
    ///
    /// A slot [`Tx::retire_doc`] or [`Tx::retire`] marked is **not** matched, which is
    /// `spec.md:1245-1246`'s *"a later `tx.doc()` at that address in the same transaction creates a new
    /// incarnation with a new draft and ID"*.
    ///
    /// # The migration ladder
    ///
    /// `spec.md:1441-1444` is four outcomes and they are four different actions, so the comparison is
    /// [`classify_version`]'s
    /// [`VersionFit`] — one pure function in `cyrup-pico-doc`, matched
    /// exhaustively here. `has_migration` is `seed.migrate.is_some()`, which is
    /// [`DocDef::MIGRATE`]'s own `Option` and therefore cannot disagree with
    /// whether a callback exists.
    ///
    /// # Errors
    ///
    /// See [`Tx::doc`].
    pub(crate) async fn doc_at<T, I>(
        &mut self,
        address: &DocumentAddress,
        seed: Seed<I>,
    ) -> Result<DocHandle<'tx, T>, TxError>
    where
        I: FnOnce() -> Result<DocRoot, TxError>,
    {
        if let Some(index) = self.live_slot(address) {
            return Ok(handle(index));
        }
        // `spec.md:1500-1502`: *"typed access inside the creating transaction lazily reads the
        // detached source, migrates when required, and replaces the copy with one ordinary child
        // create containing the final prepared value."* This has to come before the storage read,
        // because the copy is not committed yet: `find_document` would answer `None`, the seed's
        // `initial()` would win, and the child would silently start from the definition's value
        // instead of the parent's — the fork's whole point, lost.
        if let Some(index) = self.pending_copy(address) {
            return self.acquire_copy(index, seed).await;
        }
        let found = self
            .inner
            .storage
            .find_document(address, DocumentPoint::Current, self.inner.cx)
            .await?;
        let slot = match found {
            Some(record) => {
                agrees(address, &record.scope, &seed.scope)?;
                let stored = self
                    .inner
                    .storage
                    .document(record.id, DocumentPoint::Current, self.inner.cx)
                    .await?;
                let Some(stored) = stored else {
                    // The record resolves but its content does not. `spec.md:4352-4360` keeps
                    // corruption and absence in different channels, and a record whose content is
                    // absent is neither a legitimate absence nor this layer's to repair.
                    return Err(TxError::Storage(StorageFailure::Corrupt(
                        cyrup_pico_store::Corruption::MissingConfirmedData {
                            what: format!("content for document {} at {address}", record.id),
                        },
                    )));
                };
                match classify_version(stored.version, seed.version, seed.migrate.is_some()) {
                    VersionFit::Current => {
                        let change = match self.inner.docs.begin_change(record.id)? {
                            Some(change) => change,
                            None => self.inner.docs.load(
                                record.id,
                                address.clone(),
                                stored.value.clone(),
                            )?,
                        };
                        DocSlot {
                            address: address.clone(),
                            id: record.id,
                            change: Some(change),
                            continuity: Continuity::Continue,
                            version: seed.version,
                            stored: Some(stored.version),
                            deltas_since_base: stored.deltas_since_base,
                            retire: Retire::Keep,
                            checkpoint: seed.checkpoint,
                        }
                    }
                    VersionFit::Migrate { from, to } => {
                        let Some(migrate) = seed.migrate else {
                            // Unreachable: `classify_version` returns this arm only when
                            // `has_migration` was true, and that argument *is* this `Option`.
                            // Written rather than unwrapped because the workspace denies both.
                            return Err(TxError::NoMigration {
                                address: address.clone(),
                                stored: from,
                                token: to,
                            });
                        };
                        let migrated = migrate(Migration::new(&stored.value, from, to))?;
                        // A detached tracker, NOT `DocIndex::load`: the migrated value is not
                        // durable yet, and the authority may only hold what is. `Swap::Rebase`
                        // installs it after storage settles.
                        let change = cyrup_pico_doc::Tracker::track(migrated).begin_change();
                        DocSlot {
                            address: address.clone(),
                            id: record.id,
                            change: Some(change),
                            continuity: Continuity::Migrated,
                            version: to,
                            // Deliberately dropped: a required base writes no delta, so there is
                            // nothing to continue, and the witness names the *older* version.
                            stored: None,
                            deltas_since_base: stored.deltas_since_base,
                            retire: Retire::Keep,
                            checkpoint: seed.checkpoint,
                        }
                    }
                    VersionFit::StoredIsNewer { stored, token } => {
                        return Err(TxError::StoredVersionIsNewer {
                            address: address.clone(),
                            stored,
                            token,
                        });
                    }
                    VersionFit::NoMigration { stored, token } => {
                        return Err(TxError::NoMigration {
                            address: address.clone(),
                            stored,
                            token,
                        });
                    }
                }
            }
            None => {
                // The one place a seed is consumed, and it is the branch where the address is empty.
                let initial = (seed.initial)()?;
                let id: DocumentId = self
                    .inner
                    .storage
                    .mint(self.inner.cx)
                    .await
                    .map_err(TxError::Mint)?;
                let record = cyrup_pico_store::DocumentCreate {
                    id,
                    kind: address.kind.clone(),
                    key: address.key.clone(),
                    scope: seed.scope,
                };
                // A creation's change is opened over the seed value, so the draft reads its own
                // initial state and `prepare()` has a complete candidate either way.
                let change = cyrup_pico_doc::Tracker::track(initial).begin_change();
                DocSlot {
                    address: address.clone(),
                    id,
                    change: Some(change),
                    continuity: Continuity::Created(record),
                    version: seed.version,
                    stored: None,
                    deltas_since_base: 0,
                    retire: Retire::Keep,
                    checkpoint: seed.checkpoint,
                }
            }
        };
        let index = self.inner.slots.len();
        self.inner.slots.push(slot);
        Ok(handle(index))
    }

    /// The draft for a handle minted in **this** transaction.
    ///
    /// # Errors
    /// [`TxError::ForeignHandle`] — see that variant for why it exists and why it is unreachable.
    pub fn draft<T>(&mut self, handle: DocHandle<'tx, T>) -> Result<Draft<'_, T>, TxError> {
        let slot = self
            .inner
            .slots
            .get_mut(handle.index as usize)
            .ok_or(TxError::ForeignHandle)?;
        let change = slot.change.as_mut().ok_or(TxError::ForeignHandle)?;
        Ok(Draft {
            change,
            _not_send: PhantomData,
            _value: PhantomData,
        })
    }

    /// Retire the incarnation a handle names, persisting its final content first.
    ///
    /// `spec.md:1242-1244`: *"Retirement of an acquired draft persists its final content before
    /// retirement."* That ordering is not maintained here — it is structural, because
    /// [`DocumentCommand`](cyrup_pico_store::DocumentCommand) carries content and retirement as **one
    /// value** (ADR-0030 F3), so the retirement cannot win the race against the content it follows.
    ///
    /// # Errors
    /// [`TxError::ForeignHandle`].
    pub fn retire<T>(&mut self, handle: DocHandle<'tx, T>) -> Result<(), TxError> {
        let slot = self
            .inner
            .slots
            .get_mut(handle.index as usize)
            .ok_or(TxError::ForeignHandle)?;
        slot.retire = Retire::Retire;
        Ok(())
    }
}

/// The token-versus-persisted-record agreement check (`spec.md:1063-1065`).
///
/// One function, so the comparison is written once and every acquisition path — [`Tx::doc_at`] and
/// [`Tx::retire_doc`] — makes the same one. See [`TxError::TokenDisagreesWithRecord`] for why this is
/// a runtime check and why the record is the authority.
fn agrees(
    address: &DocumentAddress,
    stored: &DocumentScope,
    token: &DocumentScope,
) -> Result<(), TxError> {
    if stored == token {
        return Ok(());
    }
    Err(TxError::TokenDisagreesWithRecord {
        address: address.clone(),
        stored: Box::new(stored.clone()),
        token: Box::new(token.clone()),
    })
}

/// One document record's conversation semantics, or `None` if it is not a conversation document.
///
/// `spec.md:923-925`: *"only conversation documents declare history and fork behavior."* So a fork has
/// exactly one place to read a policy from, and a scope that has no policy has no arm here that
/// invents one — which is `G-FORK-POLICY-PERSISTED`'s *"never from the token"* restated from the other
/// side: there is no second source, not even a default.
const fn conversation_semantics(record: &DocumentRecord) -> Option<&ConversationSemantics> {
    match &record.scope {
        DocumentScope::Conversation { semantics, .. } => Some(semantics),
        // `spec.md:1489-1490`: task documents are never copied; session documents remain shared.
        DocumentScope::Session | DocumentScope::Task { .. } => None,
    }
}

/// Build a handle for a slot index.
fn handle<'tx, T>(index: usize) -> DocHandle<'tx, T> {
    DocHandle {
        // A transaction with four billion in-flight documents has a different problem; saturating
        // keeps this free of a panic and of an error arm nobody can reach.
        index: u32::try_from(index).unwrap_or(u32::MAX),
        _brand: PhantomData,
        _value: PhantomData,
    }
}

// ---------------------------------------------------------------------------------------------
// Table WRITES. No `conversation()`, no `entry()`, no `task()`, no `submission()` on this state:
// that absence IS `G-READ-BEFORE-FIRST-TABLE-WRITE`, and it is why `ReadAfterWrite` is deleted
// rather than ported. It is affordable precisely because `spec.md:1557` guarantees creations
// return their ids.
// ---------------------------------------------------------------------------------------------
impl Tx<'_, Writing> {
    /// Append an entry, returning the record it created.
    ///
    /// # Errors
    /// [`TxError::Mint`], [`TxError::AlreadyStaged`].
    pub async fn append_entry(
        &mut self,
        conversation_id: ConversationId,
        draft: EntryDraft,
    ) -> Result<EntryRecord, TxError> {
        let id: EntryId = self
            .inner
            .storage
            .mint(self.inner.cx)
            .await
            .map_err(TxError::Mint)?;
        let record = EntryRecord {
            id,
            conversation_id,
            kind: draft.kind,
            model: draft.model,
            data: draft.data,
            head: draft.head,
            edits: draft.edits,
            by_task_id: draft.by_task_id,
        };
        self.inner.builder.entry(record.clone())?;
        Ok(record)
    }

    /// Create a conversation, returning the inert record it created.
    ///
    /// `spec.md:1213-1216`: *"Creation always requires explicit ownership; no transaction wrapper
    /// injects the executing task. For task ownership, the caller supplies only a typed task ID. The
    /// Session derives the persisted owner conversation from the task's final candidate record."*
    /// [`ConversationOwnership`] is the caller-facing two-variant enum that makes omitting the choice
    /// impossible; the derivation below is why the persisted [`ConversationOwner`] pair is not a
    /// caller input.
    ///
    /// The owner lookup consults this transaction's staged tasks first and committed state second.
    /// That second read is **internal** and is not a caller table read, exactly as
    /// `spec.md:1240-1241` rules for task-scoped document access — which is why it is reachable from
    /// this state at all.
    ///
    /// # Errors
    /// [`TxError::Mint`], [`TxError::UnknownOwnerTask`], [`TxError::AlreadyStaged`].
    pub async fn create_conversation(
        &mut self,
        ownership: ConversationOwnership,
        parent: Option<ConversationParent>,
    ) -> Result<ConversationRecord, TxError> {
        let owner = match ownership {
            ConversationOwnership::Ownerless => None,
            ConversationOwnership::Task(task_id) => {
                let staged = self
                    .inner
                    .staged_tasks
                    .iter()
                    .find(|(id, _)| *id == task_id)
                    .map(|(_, conversation_id)| *conversation_id);
                let candidate = match staged {
                    Some(conversation_id) => conversation_id,
                    None => {
                        self.inner
                            .storage
                            .task(task_id, self.inner.cx)
                            .await?
                            .ok_or(TxError::UnknownOwnerTask(task_id))?
                            .conversation_id
                    }
                };
                Some(ConversationOwner {
                    conversation_id: candidate,
                    task_id,
                })
            }
        };
        let id: ConversationId = self
            .inner
            .storage
            .mint(self.inner.cx)
            .await
            .map_err(TxError::Mint)?;
        let record = ConversationRecord { id, parent, owner };
        self.inner.builder.conversation(record.clone())?;
        Ok(record)
    }

    /// Fork a conversation at one concrete visible entry, copying the parent's conversation
    /// documents by the policy **each one's persisted record** carries (PICO5-PLAN S9).
    ///
    /// # The three things this does, and the spec sentence for each
    ///
    /// 1. **One entry, one commit.** `spec.md:1473-1477`: *"a conversation fork points to one concrete
    ///    visible entry `E` … Document state at `E` is the final state of the commit containing
    ///    `E`."* The entry is resolved through
    ///    [`Storage::entry_in`], which applies every `parent.at`
    ///    ancestry cap **inside the backend**, and the commit sequence it returns is the point every
    ///    `asOf` copy reads at. There is no second entry and no second point: both are one
    ///    [`ConversationParent`] and one [`Seq`].
    /// 2. **The policy comes from the record.** `spec.md:1478-1479`: *"each conversation document
    ///    follows the history/fork policy persisted in its `DocumentRecord`."* This method takes no
    ///    definition, no token and no address, so there is nothing a caller could present that would
    ///    be consulted instead — see [`ForkedDocument::policy`].
    /// 3. **Task documents and tasks are never copied.** `spec.md:1489-1490`. Not a filter: the
    ///    enumeration is one exact
    ///    [`ScopeRef::Conversation`] scan, and a task
    ///    document is not in that scope. Session documents *"remain shared"* for the same reason.
    ///
    /// # Why `current` and `asOf` are two separate scans
    ///
    /// `spec.md:1485-1486` copies *"logically present"* instances, and the two policies disagree about
    /// *when*: `current` is *"the committed parent value selected when the fork commit runs"* and
    /// `asOf` is *"the parent value at `E`'s commit"*. A document created after the fork point is
    /// logically present now and not then; one retired since is present then and not now. One scan at
    /// each point, with each record handled under its own policy, is the only reading that gives both
    /// sentences their meaning — and because a record's policy is a single value, no incarnation is
    /// considered twice.
    ///
    /// # What is staged, and when it is admitted
    ///
    /// Nothing durable happens here. Each copy becomes a [`PendingCopy`]
    /// descriptor; `Tx::prepare` turns the untouched ones into
    /// [`DocumentCommand::Copy`](cyrup_pico_store::DocumentCommand::Copy) commands and a typed
    /// acquisition in this same transaction turns a touched one into an ordinary create
    /// (`spec.md:1500-1502`). Preparation is also where `spec.md:1491-1494`'s rejection is applied —
    /// **before storage admission**, so a transaction that forks and writes the parent's
    /// `fork: "current"` document comes back as a
    /// [`RollbackReason::Preparation`] with the Session intact.
    ///
    /// # Errors
    ///
    /// [`TxError::ForkPointNotVisible`] when the named entry is not visible in the parent's history;
    /// [`TxError::Mint`]; [`TxError::UnknownOwnerTask`]; [`TxError::AlreadyStaged`];
    /// [`TxError::Storage`].
    pub async fn fork_conversation(
        &mut self,
        ownership: ConversationOwnership,
        parent: ConversationParent,
    ) -> Result<Fork, TxError> {
        // 1. One concrete visible entry, and the commit that contains it.
        let fork_point = self
            .inner
            .storage
            .entry_in(parent.conversation_id, parent.at, self.inner.cx)
            .await?
            .ok_or(TxError::ForkPointNotVisible {
                conversation_id: parent.conversation_id,
                at: parent.at,
            })?
            .commit_seq;

        // 2. The child, with `parent` as its transcript cap. `create_conversation` derives the
        //    persisted owner pair, so ownership stays one caller-facing choice here too.
        let child = self.create_conversation(ownership, Some(parent)).await?;
        let child_id = child.id;

        let mut documents = Vec::new();

        // 3a. `current` and `initial`: judged on membership NOW.
        let present = self
            .conversation_documents(parent.conversation_id, DocumentPoint::Current)
            .await?;
        for record in present {
            // A record outside the conversation scope cannot be in a conversation-scoped scan. The
            // skip is written rather than asserted because skipping is also the correct behaviour:
            // `spec.md:1489-1490`'s *"task documents and tasks are never copied"*.
            let Some(semantics) = conversation_semantics(&record) else {
                continue;
            };
            let policy = semantics.fork();
            let decision = match policy {
                RewindableFork::Current => Some(
                    self.stage_fork_copy(child_id, &record, semantics, DocumentPoint::Current)
                        .await?,
                ),
                // `spec.md:1487-1489`: no copied instance; the initializer runs on first child
                // access. Reported so a caller can tell a policy from an omission.
                RewindableFork::Initial => None,
                // Selected from the fork point's membership below, not from today's.
                RewindableFork::AsOf => continue,
            };
            documents.push(ForkedDocument::new(
                record.id,
                record.address(),
                policy,
                decision,
            ));
        }

        // 3b. `asOf`: judged on membership AT the fork point.
        let then = self
            .conversation_documents(parent.conversation_id, DocumentPoint::At(fork_point))
            .await?;
        for record in then {
            let Some(semantics) = conversation_semantics(&record) else {
                continue;
            };
            if semantics.fork() != RewindableFork::AsOf {
                continue;
            }
            let child_doc = self
                .stage_fork_copy(child_id, &record, semantics, DocumentPoint::At(fork_point))
                .await?;
            documents.push(ForkedDocument::new(
                record.id,
                record.address(),
                RewindableFork::AsOf,
                Some(child_doc),
            ));
        }

        Ok(Fork::new(child, documents))
    }

    /// Every document record alive in one conversation's scope at one point, paged to exhaustion.
    ///
    /// `spec.md:4342-4345`: *"`scanDocuments()` enumerates only the incarnations alive in one exact
    /// scope at its selected point … It uses ascending incarnation IDs. There is no ordinary open-time
    /// all-document scan."* This is not one: it is one scope, and a fork is the one operation that has
    /// to know everything in it.
    async fn conversation_documents(
        &mut self,
        conversation_id: ConversationId,
        at: DocumentPoint,
    ) -> Result<Vec<DocumentRecord>, TxError> {
        let query = DocumentQuery {
            scope: ScopeRef::Conversation(conversation_id),
            at,
            kind: None,
        };
        let mut records = Vec::new();
        let mut from: Option<DocumentCursor> = None;
        loop {
            let page = self
                .inner
                .storage
                .scan_documents(&query, FORK_SCAN_PAGE, from, self.inner.cx)
                .await?;
            records.extend(page.items);
            match page.next {
                Some(next) => from = Some(next),
                None => return Ok(records),
            }
        }
    }

    /// Mint the child incarnation and record the copy's descriptor.
    ///
    /// The child record is the source's kind and family key with the **child's** conversation and the
    /// **source's** semantics — which is what makes `spec.md:4315`'s *"kind/key/history/fork must match
    /// the child create record"* true by construction rather than by a check that could be got wrong.
    async fn stage_fork_copy(
        &mut self,
        child: ConversationId,
        source: &DocumentRecord,
        semantics: &ConversationSemantics,
        at: DocumentPoint,
    ) -> Result<DocumentId, TxError> {
        let id: DocumentId = self
            .inner
            .storage
            .mint(self.inner.cx)
            .await
            .map_err(TxError::Mint)?;
        let create = DocumentCreate {
            id,
            kind: source.kind.clone(),
            key: source.key.clone(),
            scope: DocumentScope::Conversation {
                conversation_id: child,
                semantics: *semantics,
            },
        };
        self.inner.copies.push(PendingCopy {
            address: create.address(),
            create,
            source: CopySource { id: source.id, at },
            policy: semantics.fork(),
            retire: Retire::Keep,
            consumed: false,
        });
        Ok(id)
    }

    /// Stage a complete task record.
    ///
    /// Named `stage_` rather than `create_task` deliberately: `spec.md:1176-1178`'s `createTask`
    /// derives the record from a `TaskDefinition`, and §5's durable task machine is **out of build
    /// scope** (ADR-0029). What is in scope is that a task record reaches storage through the one
    /// keyed batch, so this takes the record the caller assembled — with [`Tx::mint`] for its id —
    /// and `cyrup-pico`'s §5 layer, if it is ever built, wraps it.
    ///
    /// # Errors
    /// [`TxError::AlreadyStaged`].
    pub fn stage_task(&mut self, record: TaskRecord) -> Result<(), TxError> {
        let candidate = (record.id, record.conversation_id);
        self.inner.builder.task(record)?;
        self.inner.staged_tasks.push(candidate);
        Ok(())
    }

    /// Stage a complete submission record. See [`Tx::stage_task`] for why it is `stage_`.
    ///
    /// # Errors
    /// [`TxError::AlreadyStaged`].
    pub fn stage_submission(&mut self, record: SubmissionRecord) -> Result<(), TxError> {
        self.inner.builder.submission(record)?;
        Ok(())
    }

    /// Prepare every open change and assemble the batch.
    ///
    /// This is invariant 6's *"the Session synchronously prepares or aborts every open change at that
    /// point"*, and it runs **before storage admission**, which is what makes every failure here a
    /// [`RollbackReason`] rather than a poison (`G-PRE-ADMISSION-ROLLBACK`).
    ///
    /// `Ok(None)` is `spec.md:1357`'s *"an existing current-version document with an empty batch
    /// writes and publishes nothing"*, lifted to the whole commit: nothing was staged, so there is no
    /// [`Batch`] to build — [`BatchBuilder::build`] has no empty case — and therefore no sequence is
    /// allocated and nothing can poison.
    ///
    /// Everything that can allocate happens here, so that [`Tx::adopt`] cannot.
    fn prepare(&mut self) -> Result<Option<Batch>, RollbackReason> {
        let slots = core::mem::take(&mut self.inner.slots);
        let pending_copies = core::mem::take(&mut self.inner.copies);
        let mut swaps = Vec::with_capacity(slots.len());
        let mut changes = Vec::with_capacity(slots.len());
        let mut copies = Vec::with_capacity(pending_copies.len());
        let mut creations = 0usize;
        for slot in slots {
            let retired = slot.retire.is_retire();
            let Some(change) = slot.change else {
                // Retired without ever being drafted: no content, just the retirement.
                if retired {
                    self.inner.builder.retire_document(slot.id)?;
                    swaps.push(Swap::Retire {
                        id: slot.id,
                        address: slot.address,
                    });
                }
                continue;
            };
            let prepared = change.prepare()?;
            let base = slot.continuity.base();
            match slot.continuity {
                Continuity::Created(record) => {
                    // "Creation stores an initial base" (`spec.md:1239`), and
                    // `BaseRequirement::Required` is why the checkpoint predicate is not consulted.
                    let value = prepared.value().clone();
                    self.inner.builder.create_document(
                        record,
                        DocumentBase {
                            version: slot.version,
                            value: value.clone(),
                        },
                        slot.retire,
                    )?;
                    changes.push(Change::new(
                        slot.id,
                        slot.address.clone(),
                        prepared.ops().clone(),
                        value.clone(),
                        true,
                        retired,
                    ));
                    if retired {
                        swaps.push(Swap::Retire {
                            id: slot.id,
                            address: slot.address,
                        });
                    } else {
                        creations += 1;
                        swaps.push(Swap::Create {
                            id: slot.id,
                            address: slot.address,
                            value,
                        });
                    }
                }
                Continuity::Migrated => {
                    // `spec.md:1463-1465`: a required current-version base, *"even when the migrated
                    // JSON is deeply equal"*. `choose_representation` returns before the predicate is
                    // reachable for `Required`, which is `spec.md:1400`'s *"creation and version
                    // transitions require bases and do not call it"* — and the empty-batch case is
                    // covered by the same early return, so an untouched migrated document still
                    // writes its base.
                    let value = prepared.value().clone();
                    self.inner.builder.change_document(
                        slot.id,
                        DocumentContent::Base(DocumentBase {
                            version: slot.version,
                            value: value.clone(),
                        }),
                        slot.retire,
                    )?;
                    changes.push(Change::new(
                        slot.id,
                        slot.address.clone(),
                        prepared.ops().clone(),
                        value.clone(),
                        false,
                        retired,
                    ));
                    if retired {
                        swaps.push(Swap::Retire {
                            id: slot.id,
                            address: slot.address,
                        });
                    } else {
                        // Counted with the creations: the authority may not hold this incarnation
                        // yet, so the rebase can grow both maps and that growth must happen here.
                        creations += 1;
                        swaps.push(Swap::Rebase {
                            id: slot.id,
                            address: slot.address,
                            value,
                        });
                    }
                }
                Continuity::Continue => {
                    let representation = choose_representation(
                        base,
                        &prepared,
                        slot.deltas_since_base,
                        slot.checkpoint
                            .as_ref()
                            .map(|f| f as &dyn Fn(CheckpointInput<'_>) -> bool),
                    );
                    let content = match representation {
                        Representation::Nothing => None,
                        Representation::Base => Some(DocumentContent::Base(DocumentBase {
                            version: slot.version,
                            value: prepared.value().clone(),
                        })),
                        Representation::Delta => {
                            let Some(continues) = slot.stored else {
                                return Err(RollbackReason::Preparation(
                                    PreparationFailed::DeltaWithoutWitness(slot.id),
                                ));
                            };
                            Some(DocumentContent::Delta {
                                continues,
                                ops: prepared.ops().clone(),
                            })
                        }
                    };
                    match content {
                        Some(content) => {
                            self.inner
                                .builder
                                .change_document(slot.id, content, slot.retire)?;
                        }
                        None if retired => {
                            self.inner.builder.retire_document(slot.id)?;
                        }
                        None => continue,
                    }
                    changes.push(Change::new(
                        slot.id,
                        slot.address.clone(),
                        prepared.ops().clone(),
                        prepared.value().clone(),
                        false,
                        retired,
                    ));
                    if retired {
                        swaps.push(Swap::Retire {
                            id: slot.id,
                            address: slot.address,
                        });
                    } else {
                        swaps.push(Swap::Adopt {
                            id: slot.id,
                            prepared,
                        });
                    }
                }
            }
        }
        // The fork copies no typed acquisition replaced. `spec.md:1496-1499`: *"forks stage
        // backend-side `document.copy` commands carrying the child create record and an exact source
        // incarnation/point. Storage materializes each source and persists its stored value/version as
        // the child's independent initial base."* The Session writes **no value** for these, which is
        // the point: it does not have one, and inventing one would be the reference-instead-of-value
        // mistake ADR-0030 §2.3 says the independence rests on not making.
        for copy in &pending_copies {
            if copy.consumed {
                continue;
            }
            self.inner
                .builder
                .copy_document(copy.create.clone(), copy.source, copy.retire)?;
            copies.push(Copied::new(
                copy.create.id,
                copy.address.clone(),
                copy.source.id,
                copy.source.at,
                copy.retire.is_retire(),
            ));
        }
        let builder = core::mem::replace(&mut self.inner.builder, BatchBuilder::new());
        let Some(batch) = builder.build() else {
            return Ok(None);
        };
        // `spec.md:1491-1494`'s rejection, applied HERE — before storage admission — which is what
        // makes it a `RollbackReason` with the Session intact rather than a `RejectedReason` the
        // Session has to trust a backend to have raised without a durable effect.
        //
        // It is order-independent for a structural reason rather than a careful one: `batch` is five
        // `BTreeMap`s (ADR-0030 F3), so *"the copy assembled before the parent's write"* and *"after"*
        // are the same batch, and this loop cannot see a difference that does not exist.
        for copy in &pending_copies {
            if copy.source_is_untouchable() && batch.documents().contains_key(&copy.source.id) {
                return Err(RollbackReason::Preparation(
                    PreparationFailed::ForkSourceWritten {
                        parent: copy.source.id,
                        child: copy.create.id,
                        policy: copy.policy,
                    },
                ));
            }
        }
        // The one allocation adoption must not make.
        self.inner.docs.reserve(creations)?;
        self.inner.staged = Some(Staged {
            swaps,
            changes: changes.into(),
            copies: copies.into(),
        });
        Ok(Some(batch))
    }

    /// **The kernel's one and only `Storage::commit` call site.**
    ///
    /// # Errors
    /// [`CommitError`], whose two classes the caller must distinguish — there is no third option and
    /// no `?` that can collapse them.
    async fn store(&mut self, batch: Batch) -> Result<Durable, CommitError> {
        let seq = self.inner.storage.commit(batch, self.inner.cx).await?;
        // The one place `Durable` is minted.
        Ok(Durable::new(seq))
    }

    /// Pointer-swap adoption. Consumes the transaction **and** the durability witness.
    ///
    /// `spec.md:1305-1310`: *"adoption performs no diffing, application, allocation, or callback."*
    /// Everything that could fail or allocate happened in [`Tx::prepare`], before storage was
    /// admitted; what is left is two `HashMap` writes and one revision increment per document.
    ///
    /// # Errors
    /// [`AdoptionFailed`], which carries the `seq` because the durable state is known-good and known
    /// **ahead** of memory: the host reopens and has nothing to reconcile.
    fn adopt(self, durable: Durable) -> Result<Publication, AdoptionFailed> {
        let seq = durable.seq();
        let Some(staged) = self.inner.staged.take() else {
            return Err(AdoptionFailed {
                seq,
                cause: AdoptionCause::NothingStaged,
            });
        };
        self.inner
            .docs
            .adopt(staged.swaps)
            .map_err(|cause| AdoptionFailed { seq, cause })?;
        Ok(Publication::new(seq, staged.changes, staged.copies))
    }
}

/// Why preparation could not assemble the batch.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PreparationFailed {
    /// A delta was chosen for a document whose stored version was never witnessed.
    ///
    /// Unreachable while [`DocSlot::stored`] is set by every non-creating acquisition; the arm exists
    /// because [`DocumentContent::Delta`] requires a [`StoredVersion`] and fabricating one is the
    /// thing `G-VERSION-PER-RECORD` forbids. A `None` here is a bug in this module, and rolling the
    /// commit back is the only answer that cannot corrupt a document.
    #[error("document {0} would be stored as a delta with no witnessed version")]
    DeltaWithoutWitness(DocumentId),
    /// A fork's copy source is also written by the same batch (PICO5-PLAN S9).
    ///
    /// `spec.md:1491-1494`: *"a transaction that creates a fork therefore rejects if it also writes
    /// one of the parent's `fork: "current"` documents; commit the parent change first so the fork has
    /// one unambiguous stored source revision."* `spec.md:4316` states the storage half — *"a batch
    /// may not create, change, or retire a selected source"* — and
    /// [`PendingCopy::source_is_untouchable`](crate::fork::PendingCopy::source_is_untouchable)
    /// documents why the two rules have different extents.
    ///
    /// Raised in [`Tx::prepare`], so the Session comes back: the caller's fix is `spec.md`'s own —
    /// commit the parent change first, then fork.
    ///
    /// The field is named `parent` and not `source` because `thiserror` reserves that name for the
    /// error cause.
    #[error(
        "the fork of document {parent} into {child} selects a {policy:?}-policy source that this batch also writes"
    )]
    ForkSourceWritten {
        /// The parent incarnation the copy reads.
        parent: DocumentId,
        /// The child incarnation the fork staged.
        child: DocumentId,
        /// The policy that selected it.
        policy: RewindableFork,
    },
}

/// A new entry, without the fields the transaction assigns.
///
/// `spec.md:1170` takes an `EntryDraft`; the absent fields are `id` (minted) and `conversation_id`
/// (the argument), so a caller cannot propose an id and cannot file an entry under a conversation it
/// did not name.
#[derive(Clone, PartialEq, Debug)]
pub struct EntryDraft {
    /// What kind of entry it is.
    pub kind: Kind,
    /// The provider-facing messages, if any.
    pub model: Option<Vec<DocValue>>,
    /// The application payload.
    pub data: Option<DocValue>,
    /// The head marker this entry establishes.
    pub head: Option<EntryId>,
    /// Context edits it contributes.
    pub edits: Option<Vec<ContextEdit>>,
    /// The task that wrote it.
    pub by_task_id: Option<TaskId>,
}

/// Who owns a new conversation (`spec.md:276-279`).
///
/// A two-variant enum, so **the caller cannot omit the choice** — `spec.md:1213` requires creation to
/// supply ownership explicitly and ADR-0030 §2.4 keeps this an enum rather than a typestate, because
/// it is attribution and not an access-control capability. The *persisted* form is
/// [`ConversationOwner`], derived by [`Tx::create_conversation`] from the named task.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConversationOwnership {
    /// No owning task. `spec.md:252`: the root conversation is one.
    Ownerless,
    /// Owned by this task. The caller supplies only the typed id.
    Task(TaskId),
}

/// A draft of one document, borrowing the transaction.
///
/// This is the whole of invariant 6's lifetime half. Upstream needs four mechanisms — a revoking
/// `Proxy`, a `#sealed` flag, `#assertOpen()` on every access and a validating copy walk — and here
/// `'d` simply cannot outlive the callback body. The draft cannot be stored in an outer binding,
/// cannot be returned, cannot be cloned and cannot be sent.
///
/// The copy walk is replaced by [`DocValue::parse`]: Rust disposes of eight of Chord's ten rejected
/// shapes with no check at all, and the one that survives — a non-finite float — is rejected at the
/// assignment, before the draft changes.
pub struct Draft<'d, T> {
    change: &'d mut OpenChange,
    _not_send: PhantomData<*const ()>,
    _value: PhantomData<fn() -> T>,
}

impl<T> Draft<'_, T> {
    /// Read the candidate: read-your-writes inside the change.
    #[must_use]
    pub fn read(&self) -> &DocValue {
        self.change.read()
    }

    /// Read one path of the candidate.
    #[must_use]
    pub fn read_at(&self, path: &cyrup_pico_doc::Path) -> Option<&DocValue> {
        self.change.read_at(path)
    }

    /// Assign a value at one path.
    ///
    /// The value crosses [`DocValue::parse`], so a non-finite float is rejected **here**, before the
    /// draft changes — `spec.md:1356-1369`'s *"rejected at the offending assignment"*. The failure
    /// upstream calls total loss is a `NaN` that reaches storage and makes the session unopenable one
    /// restart later.
    ///
    /// # Errors
    /// [`DraftError::NotStrictJson`] for a value Chord would reject, [`DraftError::Path`] for a path
    /// that does not address a container.
    pub fn set<V: serde::Serialize + ?Sized>(
        &mut self,
        path: &cyrup_pico_doc::Path,
        value: &V,
    ) -> Result<(), DraftError> {
        let value = DocValue::parse(value)?;
        self.change.set(path, value)?;
        Ok(())
    }

    /// Delete one path.
    ///
    /// # Errors
    /// [`DraftError::Path`].
    pub fn delete(&mut self, path: &cyrup_pico_doc::Path) -> Result<(), DraftError> {
        self.change.delete(path)?;
        Ok(())
    }

    /// Append to a string at one path.
    ///
    /// # Errors
    /// [`DraftError::Path`].
    pub fn append_str(
        &mut self,
        path: &cyrup_pico_doc::Path,
        text: &str,
    ) -> Result<(), DraftError> {
        self.change.append_str(path, text)?;
        Ok(())
    }

    /// Replace the whole document.
    ///
    /// # Errors
    /// [`DraftError::NotStrictJson`] for a value Chord would reject, [`DraftError::NotAnObject`] when
    /// the value is not a JSON object — a document root is one (`spec.md:1063`).
    pub fn replace<V: serde::Serialize + ?Sized>(&mut self, value: &V) -> Result<(), DraftError> {
        let DocValue::Map(map) = DocValue::parse(value)? else {
            return Err(DraftError::NotAnObject);
        };
        let map = Arc::try_unwrap(map).unwrap_or_else(|shared| (*shared).clone());
        self.change.replace_root(DocRoot::from_map(map));
        Ok(())
    }
}

/// Why a draft operation was refused.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DraftError {
    /// The value is not strict JSON.
    #[error("the assigned value is not strict JSON")]
    NotStrictJson(#[from] cyrup_pico_doc::NotStrictJson),
    /// The path does not address what the operation needs.
    #[error("the path is not assignable")]
    Path(#[from] PathError),
    /// A whole-document replacement was given something that is not a JSON object.
    #[error("a document root must be a JSON object")]
    NotAnObject,
}

impl From<DraftError> for CallbackError {
    fn from(e: DraftError) -> Self {
        match e {
            DraftError::NotStrictJson(_) | DraftError::NotAnObject => Self::Abandoned(Box::new(e)),
            DraftError::Path(p) => Self::Tx(TxError::Path(p)),
        }
    }
}

/// What one run of a commit callback decided.
///
/// Internal, because [`CommitOutcome`](crate::CommitOutcome) is the public shape and this one still
/// carries the [`Publication`] the session has yet to publish.
pub(crate) enum Run<R> {
    Nothing(R),
    Committed {
        result: R,
        seq: Seq,
        publication: Publication,
    },
    RolledBack(RollbackReason),
    Uncertain(UncertainCommit),
}

/// Run one commit callback and settle it. **The whole of ADR-0030 F5's witness chain is here.**
///
/// A free function rather than a method on [`crate::SessionMut`] for a borrow-checker reason worth
/// stating: `Tx<'tx, _>` holds `&'tx mut TxInner<'tx>`, so the `TxInner` local is borrowed for its own
/// type's lifetime — that is, for the rest of the enclosing body. Owning that local in a function
/// that *returns* an owned `Run<R>` is what lets `SessionMut::commit` hand the handle back afterwards.
pub(crate) async fn run_change<R, F>(
    storage: &mut DynStorage,
    docs: &DocIndex,
    cx: &Cx,
    change: F,
) -> Run<R>
where
    F: for<'tx> AsyncFnOnce(Tx<'tx, Reading>) -> Result<(R, Tx<'tx, Writing>), CallbackError>,
{
    let mut inner = TxInner::new(storage, docs, cx);
    let tx = Tx::open(&mut inner);
    // Every `Draft` handed out inside the callback borrows `tx`, so all of them are dead by the time
    // this `await` returns. That is invariant 6's revocation, and there is no flag to check.
    let (result, mut settled) = match change(tx).await {
        Ok(pair) => pair,
        Err(e) => return Run::RolledBack(RollbackReason::Callback(e)),
    };
    let batch = match settled.prepare() {
        Ok(Some(batch)) => batch,
        // Nothing was staged: no sequence is allocated, nothing is published, nothing can poison.
        Ok(None) => return Run::Nothing(result),
        Err(reason) => return Run::RolledBack(reason),
    };
    let durable = match settled.store(batch).await {
        Ok(durable) => durable,
        // "Rejected" MUST mean nothing durable happened and rollback is guaranteed.
        Err(CommitError::Rejected(reason)) => {
            return Run::RolledBack(RollbackReason::Rejected(reason));
        }
        // Everything else. Commit state unknown by definition: reopen AND reconcile.
        Err(CommitError::Uncertain(source)) => {
            return Run::Uncertain(UncertainCommit::commit_state_unknown(source));
        }
    };
    let seq = durable.seq();
    #[cfg(feature = "fault-injection")]
    crate::fault::fire_before_adopt(docs);
    let adopted = settled.adopt(durable);
    #[cfg(feature = "fault-injection")]
    crate::fault::fire_after_adopt(docs);
    match adopted {
        Ok(publication) => Run::Committed {
            result,
            seq,
            publication,
        },
        // Storage committed; memory is behind durable state. Reopen; nothing to reconcile.
        Err(failed) => Run::Uncertain(UncertainCommit::adoption_failed(failed)),
    }
}

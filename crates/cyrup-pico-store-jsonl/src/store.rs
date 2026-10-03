//! [`JsonlStore`] — §11.3's publication protocol, and the two things ADR-0030 §9 adds to it.
//!
//! # The protocol, verbatim (`spec.md:4415-4418`)
//!
//! > 1. Append complete prepared records to every affected sidecar.
//! > 2. Append one complete main marker listing those records.
//! > 3. Publish in memory only after the marker write succeeds.
//! >
//! > Every commit uses this protocol; there is no standalone-sidecar fast path.
//!
//! [`JsonlStore::commit`] is those three steps in that order, with step 3 being
//! [`crate::state::Committed::apply_line`] over the lines it just wrote — the same function recovery uses, so a
//! reopen cannot disagree with the session that committed.
//!
//! # The asymmetry the two tiers exist to express
//!
//! ADR-0030 §2.2: *"a marker surviving without its data is corruption; losing a tail commit is merely
//! recoverable — the asymmetry is the subtle part."* [`Durability::PowerLoss`] flushes every affected
//! sidecar **and waits** before the marker is written (`spec.md:4421-4424`), so the marker can never
//! overtake its data. Neither tier flushes `main.jsonl` on the ordinary path (`spec.md:4424-4426`), so
//! an acknowledged tail commit can still disappear — and that is the recoverable direction.
//!
//! # Uncertain is the only honest answer after admission
//!
//! There is no `From<io::Error>` for [`CommitError`] (ADR-0030 F1), and that is not an inconvenience
//! here: once a byte of this commit has been handed to the kernel, whether it reached the medium is
//! exactly what is unknown. So every post-admission failure is [`CommitError::Uncertain`] and poisons
//! the backend (`spec.md:4434`: *"any uncertain append failure poisons the open backend"*). Everything
//! decided **before** the first append is [`CommitError::Rejected`], and
//! [`crate::state::Committed::validate`] is pure, so that claim is structural.

use std::collections::BTreeMap;
use std::io;
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use cyrup_pico_doc::{DefVersion, StoredContent};
use cyrup_pico_store::{
    Batch, CommitError, CommittedEntry, ConversationCursor, ConversationId, ConversationQuery,
    ConversationRecord, Cx, DocumentAddress, DocumentBase, DocumentCommand, DocumentCursor,
    DocumentId, DocumentPoint, DocumentQuery, DocumentRecord, EntryCursor, EntryId, EntryQuery,
    EntryRecord, HeadMarker, IdKindTag, Page, PageLimit, RawId, RejectedReason, Seq, Storage,
    StorageFailure, StoreId, StoredDocument, SubmissionCursor, SubmissionId, SubmissionQuery,
    SubmissionRecord, TaskCursor, TaskId, TaskQuery, TaskRecord, UncertainCommit,
};

use crate::appender::Appender;
use crate::copy;
use crate::durable::durable_rename;
use crate::identity::StoreIdentity;
use crate::lock::StoreLock;
use crate::read::Reader;
use crate::recover;
use crate::wire::{ContentLine, MainLine, MarkerDoc, Slice, main_path, sidecar_path};

/// §11.3's two durability tiers, named rather than spelled as a bare `bool`.
///
/// ADR-0030 F6 §D: *"NAMED rather than a bare bool, because `fsync: false` is a durability CLAIM and
/// should read like one at the call site."* pi defaults it to false (`jsonl/storage.ts:77-78`); this
/// enum has no default at all, so a caller says which claim it is making.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Durability {
    /// Survive a process crash: a `SIGKILL`, an `abort`, a panic, a `std::process::exit`.
    ///
    /// `spec.md:4419-4421`: *"Without `fsync`, it guarantees ordinary process-crash consistency, not
    /// survival of power, host, kernel, or filesystem failure."* Every append is a single synchronous
    /// `write(2)`, so the bytes are the kernel's before the call returns; the device flush is handed to
    /// [`crate::syncer`] and not waited for.
    ProcessCrash,
    /// Survive power loss, as far as §11.3's protocol can.
    ///
    /// Every affected sidecar is flushed **and waited for** before the marker is appended, so a
    /// surviving marker cannot name data that did not survive. `main.jsonl` is still not flushed on the
    /// ordinary path, so a tail commit can be lost — which is the recoverable direction and is
    /// `spec.md:4424-4426`'s own envelope. Before destructive reclamation, `main.jsonl` *is* flushed
    /// once (`spec.md:4439-4441`).
    PowerLoss,
}

/// How many ids one durable reservation covers.
///
/// A mint has to be durable before it returns, or a crash between a mint and the commit that uses the
/// minted id could reissue it (`spec.md:4282`). Writing one line per id would double `main.jsonl`'s
/// line count and therefore ADR-0030 §9's open cost, so a mint reserves a **block** and persists its
/// ceiling: a crash wastes at most the rest of the block, and `spec.md:99-100` permits gaps while
/// forbidding reuse. 64 keeps the amortised cost under two bytes of log per id.
const ID_BLOCK: u64 = 64;

/// How a store is opened.
///
/// Deliberately not `Default`: [`Durability`] is a claim, and a default would make the weaker claim
/// silently — which is the shape ADR-0030 F6 §D rejects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct JsonlOptions {
    /// Which durability tier this store promises.
    pub durability: Durability,
}

impl JsonlOptions {
    /// Open with one named tier.
    #[must_use]
    pub const fn new(durability: Durability) -> Self {
        Self { durability }
    }

    const fn flushes(&self) -> bool {
        matches!(self.durability, Durability::PowerLoss)
    }
}

/// A read-only view of a store another process may be writing.
///
/// Takes **no lock** and has **no `commit`** — not a refused one, an absent one
/// (`tests/compile-fail/a_read_only_store_cannot_commit.rs`). ADR-0030 F6 §D: the lock *"says nothing
/// about a second process reading while this one writes, which is legal and desirable and which the
/// marker protocol already makes safe"*: a reader applies only what a marker confirms, and a marker is
/// the last line of its commit.
///
/// Every read path is [`Reader`]'s, reached through [`Deref`](core::ops::Deref).
#[derive(Debug)]
pub struct ReadOnlyStore {
    reader: Reader,
}

impl core::ops::Deref for ReadOnlyStore {
    type Target = Reader;

    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

/// The JSONL backend.
///
/// Cannot be constructed without a [`StoreLock`], which is ADR-0030 F6 §D's *"a writable backend cannot
/// be built without the proof, so 'forgot to lock' is a compile error"*.
#[derive(Debug)]
pub struct JsonlStore {
    reader: Reader,
    /// Held for this store's whole life, and released when it drops.
    ///
    /// Named `_lock` nowhere: it is read by [`JsonlStore::lock_holder`] and it is the field whose
    /// *presence in the constructor's signature* is the guarantee.
    lock: StoreLock,
    main: Appender,
    sidecars: BTreeMap<DocumentId, Appender>,
    options: JsonlOptions,
    /// Why this backend is poisoned, if it is.
    ///
    /// `spec.md:4434`: *"any uncertain append failure poisons the open backend."* ADR-0030 §5 rejects
    /// typestate for exactly this flag and says why: it must be readable from the `&self` read paths,
    /// and a consuming transition cannot be. [`CommitError::Uncertain`] is its only producer.
    poison: Option<Arc<str>>,
    /// How many reclamations were deferred, for a diagnostic and for the test that asserts a failed
    /// authorising flush defers rather than fails (`spec.md:4441-4442`).
    deferred_reclamations: u32,
}

impl JsonlStore {
    /// Open `dir` for writing, which requires the proof that this process holds it.
    ///
    /// Creates the directory and its identity record if they do not exist, replays the log, and removes
    /// everything no marker confirms ([`crate::recover`]).
    ///
    /// # Errors
    ///
    /// [`StorageFailure::Corrupt`] for every damage shape `spec.md:4428-4434` calls corruption — the
    /// open **fails** rather than returning less. [`StorageFailure::Io`] from the medium, and with
    /// [`io::ErrorKind::InvalidInput`] when `lock` does not cover `dir`.
    pub fn open_for_write(
        dir: &Path,
        lock: StoreLock,
        options: JsonlOptions,
    ) -> Result<Self, StorageFailure> {
        // ADR-0030 F6 §D writes this signature with both a `dir` and a `lock`, and nothing in it makes
        // the two agree. See this crate's documentation: the objection is recorded, and the check is
        // the implementation of the signature as written.
        if !same_dir(dir, lock.dir()) {
            return Err(StorageFailure::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the store lock covers {} and the store being opened is {}",
                    lock.dir().display(),
                    dir.display()
                ),
            )));
        }
        std::fs::create_dir_all(dir).map_err(StorageFailure::Io)?;
        let identity = match StoreIdentity::read(dir)? {
            Some(identity) => identity.claim(dir)?,
            None => StoreIdentity::create(dir)?,
        };
        let recovered = recover::open(dir, identity, true)?;
        let main = Appender::new(main_path(dir), recovered.main_len);
        Ok(Self {
            reader: recovered.reader,
            lock,
            main,
            sidecars: BTreeMap::new(),
            options,
            poison: None,
            deferred_reclamations: 0,
        })
    }

    /// Open `dir` for reading, taking no lock.
    ///
    /// # Errors
    ///
    /// [`StorageFailure::Corrupt`] as [`JsonlStore::open_for_write`], plus
    /// [`io::ErrorKind::NotFound`] when the directory holds no store: a reader cannot create one,
    /// because creating a store is a write.
    pub fn open_read_only(dir: &Path) -> Result<ReadOnlyStore, StorageFailure> {
        let Some(identity) = StoreIdentity::read(dir)? else {
            return Err(StorageFailure::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} holds no store", dir.display()),
            )));
        };
        let recovered = recover::open(dir, identity, false)?;
        Ok(ReadOnlyStore {
            reader: recovered.reader,
        })
    }

    /// Which durability tier this store promises.
    #[must_use]
    pub const fn durability(&self) -> Durability {
        self.options.durability
    }

    /// This store's directory, from the lock that proves this process holds it.
    #[must_use]
    pub fn lock_holder(&self) -> &Path {
        self.lock.dir()
    }

    /// Why this backend is poisoned, if it is.
    #[must_use]
    pub fn poisoned(&self) -> Option<&str> {
        self.poison.as_deref()
    }

    /// How many reclamations have been deferred (`spec.md:4441-4442`).
    #[must_use]
    pub const fn deferred_reclamations(&self) -> u32 {
        self.deferred_reclamations
    }

    /// Every read path over this store's committed state.
    #[must_use]
    pub const fn reader(&self) -> &Reader {
        &self.reader
    }

    /// Refuse a commit against a poisoned backend.
    fn guard_commit(&self) -> Result<(), CommitError> {
        match &self.poison {
            Some(why) => Err(CommitError::Uncertain(UncertainCommit::new(Arc::clone(
                why,
            )))),
            None => Ok(()),
        }
    }

    /// Refuse a read against a poisoned backend.
    ///
    /// This is the `&self` path ADR-0030 §5 names when it rejects typestate for the flag. A poisoned
    /// store's in-memory state may disagree with the medium, so answering a read would be answering
    /// from a baseline nobody can vouch for — and `spec.md:4352-4360` has no arm for *"probably"*.
    fn guard_read(&self) -> Result<(), StorageFailure> {
        match &self.poison {
            Some(why) => Err(StorageFailure::Io(io::Error::other(why.to_string()))),
            None => Ok(()),
        }
    }

    /// Poison the backend and render the failure as the uncertain commit it is.
    fn poisoned_by(&mut self, what: String, source: io::Error) -> CommitError {
        let why: Arc<str> = Arc::from(format!("{what}: {source}"));
        if self.poison.is_none() {
            self.poison = Some(Arc::clone(&why));
        }
        CommitError::Uncertain(UncertainCommit::caused_by(why, source))
    }

    /// Resolve what this commit writes, reading only committed state.
    ///
    /// Runs **before** the first append, which is what keeps every failure here in the rejected class.
    fn plan(&self, batch: Batch, seq: Seq) -> Result<Plan, CommitError> {
        let parts = batch.into_parts();
        let mut plan = Plan::default();
        for (_, record) in parts.conversations {
            plan.main.push(MainLine::Conversation { seq, r: record });
        }
        for (_, record) in parts.entries {
            plan.main.push(MainLine::Entry { seq, r: record });
        }
        for (_, record) in parts.tasks {
            plan.main.push(MainLine::Task { seq, r: record });
        }
        for (_, record) in parts.submissions {
            plan.main.push(MainLine::Submission { seq, r: record });
        }
        for (id, command) in parts.documents {
            match command {
                DocumentCommand::Create { record, base, then } => {
                    plan.create(seq, record.stamp(seq, then.is_retire()), &base);
                }
                DocumentCommand::Copy {
                    record,
                    source,
                    then,
                } => {
                    // `spec.md:4317`: *"storage persists one independent complete child base at the
                    // source's stored version"* — a value, never a reference, which is what makes later
                    // source changes, reclamation, retirement and reopen unable to reach the child.
                    let base = self.copy_source_base(source)?;
                    plan.create(seq, record.stamp(seq, then.is_retire()), &base);
                }
                DocumentCommand::Change { content, then } => {
                    plan.content.push(ContentWrite {
                        id,
                        line: ContentLine {
                            seq,
                            content: content.to_stored(),
                        },
                        base: content.is_base(),
                        version: content.version(),
                    });
                    if then.is_retire() {
                        plan.main.push(MainLine::Retire { seq, id });
                    }
                }
                DocumentCommand::RetireOnly => plan.main.push(MainLine::Retire { seq, id }),
            }
        }
        Ok(plan)
    }

    /// Read a copy source's committed pre-batch value.
    ///
    /// Every *deterministic* reason this can fail was already decided by
    /// [`copy::check_source`] against the records, so what is left is the medium. A corrupt replay
    /// folds into [`RejectedReason::CopySourceNotAlive`], as `MemoryStore` does, because a copy from an
    /// unreadable source is one rejection and not two. An I/O error is
    /// [`CommitError::Uncertain`] **without** poisoning: nothing has been appended, so the store's
    /// state is not in doubt — only this read is.
    fn copy_source_base(
        &self,
        source: cyrup_pico_store::CopySource,
    ) -> Result<DocumentBase, CommitError> {
        let state = self.reader.state.documents.get(&source.id).ok_or(
            RejectedReason::CopySourceNotAlive {
                document: source.id,
            },
        )?;
        let upto =
            copy::readable_at(state, source.at).ok_or(RejectedReason::CopySourceNotAlive {
                document: source.id,
            })?;
        match self.reader.content(state, upto) {
            Ok((version, value, _)) => Ok(DocumentBase {
                version: version.version(),
                value,
            }),
            Err(StorageFailure::Corrupt(_) | StorageFailure::HistoryNotRetained { .. }) => {
                Err(CommitError::Rejected(RejectedReason::CopySourceNotAlive {
                    document: source.id,
                }))
            }
            Err(StorageFailure::Io(e)) => Err(CommitError::Uncertain(UncertainCommit::caused_by(
                format!("reading copy source document {}", source.id),
                e,
            ))),
        }
    }

    /// Perform the protocol, publish, then consider reclamation.
    fn write(&mut self, plan: Plan, seq: Seq) -> Result<Seq, CommitError> {
        self.drain_deferred()?;
        // Step 1: append complete records to every affected sidecar.
        let mut docs: Vec<MarkerDoc> = Vec::with_capacity(plan.content.len());
        for write in &plan.content {
            let line = encode(&write.line).map_err(|e| {
                self.poisoned_by(format!("encoding document {}'s content", write.id), e)
            })?;
            let expected = self
                .reader
                .state
                .documents
                .get(&write.id)
                .map_or(0, crate::state::DocState::end);
            let appended = match self.sidecar(write.id, expected) {
                Ok(appender) => {
                    let what = format!("appending to {}", appender.path().display());
                    appender.append(&line).map_err(|e| (what, e))
                }
                Err(e) => Err((format!("opening document {}'s sidecar", write.id), e)),
            };
            let end = match appended {
                Ok(end) => end,
                Err((what, e)) => return Err(self.poisoned_by(what, e)),
            };
            docs.push(MarkerDoc {
                id: write.id,
                end,
                base: write.base,
                version: write.version,
            });
        }

        // Step 2's precondition, for the strong tier only: the sidecar bytes have to have REACHED the
        // device before the marker is written, or a surviving marker could name data that did not
        // survive. One coalesced flush round covers every sidecar this commit touched
        // (`spec.md:4421-4424`), and a main-only commit skips it entirely (`spec.md:4426`).
        if self.options.flushes() && !plan.content.is_empty() {
            for write in &plan.content {
                let failed = self
                    .sidecars
                    .get_mut(&write.id)
                    .and_then(|appender| appender.flush_now().err());
                if let Some(e) = failed {
                    return Err(
                        self.poisoned_by(format!("flushing document {}'s sidecar", write.id), e)
                    );
                }
            }
        }

        // The main record lines, then the one marker that publishes them. The marker is last, always.
        for line in &plan.main {
            let encoded = encode(line)
                .map_err(|e| self.poisoned_by("encoding a main record".to_owned(), e))?;
            if let Err(e) = self.main.append(&encoded) {
                return Err(self.poisoned_by("appending a main record".to_owned(), e));
            }
        }
        let marker = MainLine::Marker {
            seq,
            main: u32::try_from(plan.main.len()).unwrap_or(u32::MAX),
            docs,
        };
        let encoded =
            encode(&marker).map_err(|e| self.poisoned_by("encoding the marker".to_owned(), e))?;
        if let Err(e) = self.main.append(&encoded) {
            return Err(self.poisoned_by(format!("appending commit {seq}'s marker"), e));
        }

        // Step 3: publish in memory, and only now (`spec.md:4417`).
        for line in &plan.main {
            self.apply(line)?;
        }
        self.apply(&marker)?;

        self.reclaim(seq, &plan);
        Ok(seq)
    }

    /// Report, once, any failure a **deferred** flush discovered, and poison on it.
    ///
    /// The flush is the only part of an append this backend does not wait for, so it is the only part
    /// whose failure arrives late — and `spec.md:4434`'s *"any uncertain append failure poisons the open
    /// backend"* covers it: an `fdatasync` that fails after the `write(2)` succeeded means the writeback
    /// may have dropped the pages, so neither tier's claim survives it.
    ///
    /// Checked **before** the next commit writes anything rather than after, which is the opposite of
    /// `cyrup-session`'s `append_line` and for a reason this backend has and that one does not: there, a
    /// stale flush error must not cost the caller *this* entry, because the entries are independent;
    /// here the failure is fatal to the session either way, and writing more into a medium that has
    /// started rejecting writeback makes the recovery harder rather than the commit safer.
    fn drain_deferred(&mut self) -> Result<(), CommitError> {
        let mut first = self.main.take_error().err();
        for appender in self.sidecars.values_mut() {
            if let Err(e) = appender.take_error()
                && first.is_none()
            {
                first = Some(e);
            }
        }
        match first {
            Some(e) => Err(self.poisoned_by("a deferred flush".to_owned(), e)),
            None => Ok(()),
        }
    }

    /// Apply one line this store has already written.
    ///
    /// A [`cyrup_pico_store::Corruption`] here is unreachable — [`crate::state::Committed::validate`]
    /// rejected every batch that could produce one, and the sequence it allocated is this store's
    /// highest. It is still handled rather than asserted, because the workspace forbids a panic and
    /// because the honest answer for *"the state I just published does not reconcile"* is the same as
    /// for a failed append: the commit's state is not something this process can vouch for.
    fn apply(&mut self, line: &MainLine) -> Result<(), CommitError> {
        match self.reader.state.apply_line(line) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.poisoned_by(
                "publishing a committed line".to_owned(),
                io::Error::other(e.to_string()),
            )),
        }
    }

    /// The append handle for one incarnation's current sidecar generation.
    ///
    /// The length is **verified once** against the medium when the handle is created, and tracked in
    /// memory from then on. That one `metadata()` is what stops a stale file — an orphan generation the
    /// open sweep somehow left behind — from receiving an `O_APPEND` write at a byte this process
    /// thinks is 0: the offsets in the markers would then describe bytes that are not there, which is
    /// the one way ADR-0030 F6 §D's mechanism could lie.
    fn sidecar(&mut self, id: DocumentId, expected: u64) -> io::Result<&mut Appender> {
        let generation = self
            .reader
            .state
            .documents
            .get(&id)
            .map_or(0, |doc| doc.generation);
        if !self.sidecars.contains_key(&id) {
            let path = sidecar_path(&self.reader.dir, id, generation);
            let actual = match std::fs::metadata(&path) {
                Ok(meta) => meta.len(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
                Err(e) => return Err(e),
            };
            if actual != expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{} is {actual} bytes and the confirmed layout says {expected}",
                        path.display()
                    ),
                ));
            }
            self.sidecars.insert(id, Appender::new(path, actual));
        }
        self.sidecars
            .get_mut(&id)
            .ok_or_else(|| io::Error::other("a sidecar handle vanished between insert and lookup"))
    }

    /// Reclaim what this commit's bases and retirements authorise (`spec.md:4435-4446`).
    ///
    /// **Never fails the commit.** `spec.md:4441-4442`: *"if that flush fails, the committed state
    /// remains published and reclamation is deferred."* Every failure here takes that branch, because
    /// the commit is already durable and published and nothing about reclamation can un-publish it.
    fn reclaim(&mut self, seq: Seq, plan: &Plan) {
        // `spec.md:4435-4436`: *"reclamation starts only after the authorizing base/retirement
        // commits"* — **or retirement**, which is why a `RetireOnly` command is a candidate although it
        // wrote no content. Every candidate is then filtered by
        // [`DocState::reclaimable_from`](crate::state::DocState::reclaimable_from), which is where the
        // *"only after a committed base"* half and the rewindable exemption live.
        let mut candidates: Vec<DocumentId> = plan.content.iter().map(|w| w.id).collect();
        candidates.extend(plan.main.iter().filter_map(|line| match line {
            MainLine::Retire { id, .. } => Some(*id),
            _ => None,
        }));
        candidates.sort_unstable();
        candidates.dedup();
        let candidates: Vec<DocumentId> = candidates
            .into_iter()
            .filter(|id| {
                self.reader
                    .state
                    .documents
                    .get(id)
                    .is_some_and(|doc| doc.reclaimable_from().is_some())
            })
            .collect();
        for id in candidates {
            if self.reclaim_one(seq, id).is_none() {
                self.deferred_reclamations = self.deferred_reclamations.saturating_add(1);
            }
        }
    }

    /// Reclaim one incarnation, answering `None` for *"deferred"*.
    ///
    /// # The generation in the name, and why the rename is not in place
    ///
    /// `spec.md:4443-4446` writes a temporary replacement and renames it **over** the live sidecar.
    /// That is sound for pi, whose markers list *records*. It is not sound once a marker carries byte
    /// **offsets** (ADR-0030 F6 §D): a crash between the rename and the line authorising it would leave
    /// surviving markers describing offsets into a file that is now shorter, which [`crate::recover`]
    /// must — and does — treat as corruption. So the replacement is renamed to the **next generation's
    /// name** and a [`MainLine::Reclaim`] names the generation that becomes authoritative. Every crash
    /// point is then safe: before the reclaim line, the old generation is intact and authoritative and
    /// the new one is an orphan the open sweep removes; after it, the new one is authoritative and the
    /// old one is the orphan. **[CYRUP-DELTA]** Same guarantee — `spec.md:4439-4441`'s *"the
    /// authorizing marker cannot disappear while its replacement or removal survives"* — reached by
    /// renaming to a new name rather than over the old one, which is the mechanism the offsets require.
    fn reclaim_one(&mut self, seq: Seq, id: DocumentId) -> Option<()> {
        let doc = self.reader.state.documents.get(&id)?;
        let from = doc.reclaimable_from()?;
        let keep_slices = doc.slices.get(from..)?.to_vec();
        let start = doc.slices.get(from.checked_sub(1)?)?.end;
        let end = doc.end();
        let generation = doc.generation.checked_add(1)?;
        let old = sidecar_path(&self.reader.dir, id, doc.generation);

        // `spec.md:4439-4441`: before destructive reclamation, flush `main.jsonl` once, so the
        // authorising marker cannot disappear while the replacement survives.
        if self.options.flushes() && self.main.flush_now().is_err() {
            return None;
        }

        // The kept records are copied as BYTES, so the replacement is byte-identical to the tail it
        // replaces and the new offsets are the old ones less `start`. Nothing is re-encoded, so
        // reclamation cannot change what a record says.
        let tail = crate::read::read_range(&old, start, end).ok()?;
        let tmp = self.reader.dir.join(format!(
            "doc-{}-g{generation}.jsonl.tmp",
            RawId::from(id).get()
        ));
        std::fs::write(&tmp, &tail).ok()?;
        let fresh = sidecar_path(&self.reader.dir, id, generation);
        // MANDATORY before the rename: this handle names the file the reclamation replaces, and an
        // append into an unlinked inode succeeds and is then destroyed (`crate::appender`).
        if let Some(appender) = self.sidecars.get_mut(&id) {
            appender.invalidate();
        }
        durable_rename(&tmp, &fresh).ok()?;

        let keep: Vec<Slice> = keep_slices
            .iter()
            .map(|slice| Slice {
                seq: slice.seq,
                end: slice.end.saturating_sub(start),
                base: slice.base,
                version: slice.version,
            })
            .collect();
        let line = MainLine::Reclaim {
            seq,
            id,
            generation,
            keep,
        };
        let encoded = encode(&line).ok()?;
        self.main.append(&encoded).ok()?;
        // Applied whatever happens next: the line is in the log, so the next open will apply it too.
        self.reader.state.apply_line(&line).ok()?;
        let new_len = self
            .reader
            .state
            .documents
            .get(&id)
            .map_or(0, crate::state::DocState::end);
        if let Some(appender) = self.sidecars.get_mut(&id) {
            appender.rebind(fresh, new_len);
        }
        // The old generation is removed only once the line naming its replacement is at least as
        // durable as it is: under the strong tier that means flushed.
        if self.options.flushes() && self.main.flush_now().is_err() {
            return Some(());
        }
        let _ = std::fs::remove_file(&old);
        Some(())
    }
}

/// What one commit writes to one incarnation's sidecar.
struct ContentWrite {
    id: DocumentId,
    line: ContentLine,
    base: bool,
    version: DefVersion,
}

/// One commit's resolved writes: the main lines, and one content record per affected incarnation.
#[derive(Default)]
struct Plan {
    main: Vec<MainLine>,
    content: Vec<ContentWrite>,
}

impl Plan {
    /// A creation or a copy: the stamped record in `main.jsonl`, its complete base in the sidecar.
    ///
    /// No [`MainLine::Retire`] is emitted for a creation the same commit retires:
    /// [`DocumentCreate::stamp`](cyrup_pico_store::DocumentCreate::stamp) already carries both bounds
    /// (`spec.md:4364-4365`), and a second line would be a second retirement.
    fn create(&mut self, seq: Seq, record: DocumentRecord, base: &DocumentBase) {
        let id = record.id;
        let version = base.version;
        self.main.push(MainLine::Document { seq, r: record });
        self.content.push(ContentWrite {
            id,
            line: ContentLine {
                seq,
                content: StoredContent::Base {
                    version,
                    value: base.value.clone(),
                },
            },
            base: true,
            version,
        });
    }
}

/// Render one line.
///
/// A `serde_json` failure on a value this crate owns can only come from the writer, so it is reported
/// as an I/O-class failure rather than given a domain meaning it does not have.
fn encode<T: serde::Serialize>(value: &T) -> io::Result<String> {
    serde_json::to_string(value).map_err(io::Error::other)
}

/// Whether two paths name the same directory.
///
/// Canonicalised when both resolve, compared literally when they do not: a directory that does not
/// exist yet is the ordinary case for a first open, and `create_dir_all` has not run at this point.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[async_trait]
impl Storage for JsonlStore {
    fn store_id(&self) -> StoreId {
        self.reader.store_id()
    }

    async fn commit(&mut self, batch: Batch, _cx: &Cx) -> Result<Seq, CommitError> {
        self.guard_commit()?;
        self.reader.state.validate(&batch)?;
        let seq = self.reader.state.allocate_seq()?;
        let plan = self.plan(batch, seq)?;
        self.write(plan, seq)
    }

    /// `kind` is deliberately unused, which is ADR-0030 F6 §A's explicit recommendation rather than a
    /// shortcut: *"do not build a separate durable number-to-kind table; derive ownership from the
    /// per-table indexes, which exist anyway."* A minted number is owned by nothing until a committed
    /// record carries it, so a table written here would claim an ownership a batch may never establish.
    async fn mint_raw(&mut self, _kind: IdKindTag, _cx: &Cx) -> Result<RawId, CommitError> {
        self.guard_commit()?;
        self.drain_deferred()?;
        let (next, reserved) = (
            self.reader.state.next_id,
            self.reader.state.reserved_through,
        );
        if next > reserved {
            let high = next.saturating_add(ID_BLOCK.saturating_sub(1));
            let line = MainLine::Mint { high };
            let encoded = encode(&line)
                .map_err(|e| self.poisoned_by("encoding an id reservation".to_owned(), e))?;
            if let Err(e) = self.main.append(&encoded) {
                return Err(self.poisoned_by("appending an id reservation".to_owned(), e));
            }
            // **Not** `apply_line`. The reservation's recovery meaning is deliberately stronger than
            // its write-time meaning: a reopened store cannot know which ids inside a block were handed
            // out, so recovery spends the whole block (`Committed::apply_line`'s `Mint` arm), while the
            // writer that owns the block hands out its ids one at a time. Gaps are permitted
            // (`spec.md:99-100`); reuse is not.
            self.reader.state.reserve(high);
        }
        let state = &mut self.reader.state;
        let n = NonZeroU64::new(state.next_id).ok_or(RejectedReason::IdSpaceExhausted)?;
        state.next_id = state
            .next_id
            .checked_add(1)
            .ok_or(RejectedReason::IdSpaceExhausted)?;
        Ok(RawId::new(n))
    }

    /// Flush everything this store wrote and wait for it.
    ///
    /// # Errors
    ///
    /// [`StorageFailure::Io`] when a final flush fails. A caller that has already published committed
    /// state cannot un-publish it, so this reports rather than poisons — and the lock is released when
    /// this value drops, not here, so a failed close does not leave the store unlockable.
    async fn close(&mut self, _cx: &Cx) -> Result<(), StorageFailure> {
        let mut first: Option<io::Error> = None;
        for appender in self.sidecars.values_mut() {
            if let Err(e) = appender.flush_now()
                && first.is_none()
            {
                first = Some(e);
            }
        }
        if let Err(e) = self.main.flush_now()
            && first.is_none()
        {
            first = Some(e);
        }
        match first {
            Some(e) => Err(StorageFailure::Io(e)),
            None => Ok(()),
        }
    }

    async fn conversation(
        &self,
        id: ConversationId,
        _cx: &Cx,
    ) -> Result<Option<ConversationRecord>, StorageFailure> {
        self.guard_read()?;
        self.reader.conversation(id).await
    }

    async fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: PageLimit,
        from: Option<ConversationCursor>,
        _cx: &Cx,
    ) -> Result<Page<ConversationRecord, ConversationCursor>, StorageFailure> {
        self.guard_read()?;
        self.reader.scan_conversations(query, limit, from).await
    }

    async fn entry(&self, id: EntryId, _cx: &Cx) -> Result<Option<CommittedEntry>, StorageFailure> {
        self.guard_read()?;
        self.reader.entry(id).await
    }

    async fn entry_in(
        &self,
        conversation_id: ConversationId,
        id: EntryId,
        _cx: &Cx,
    ) -> Result<Option<CommittedEntry>, StorageFailure> {
        self.guard_read()?;
        self.reader.entry_in(conversation_id, id).await
    }

    async fn find_latest_head_marker(
        &self,
        conversation_id: ConversationId,
        at_or_before: Option<EntryId>,
        _cx: &Cx,
    ) -> Result<Option<HeadMarker>, StorageFailure> {
        self.guard_read()?;
        self.reader
            .find_latest_head_marker(conversation_id, at_or_before)
            .await
    }

    async fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: PageLimit,
        from: Option<EntryCursor>,
        _cx: &Cx,
    ) -> Result<Page<EntryRecord, EntryCursor>, StorageFailure> {
        self.guard_read()?;
        self.reader.scan_entries(query, limit, from).await
    }

    async fn task(&self, id: TaskId, _cx: &Cx) -> Result<Option<TaskRecord>, StorageFailure> {
        self.guard_read()?;
        self.reader.task(id).await
    }

    async fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: PageLimit,
        from: Option<TaskCursor>,
        _cx: &Cx,
    ) -> Result<Page<TaskRecord, TaskCursor>, StorageFailure> {
        self.guard_read()?;
        self.reader.scan_tasks(query, limit, from).await
    }

    async fn submission(
        &self,
        id: SubmissionId,
        _cx: &Cx,
    ) -> Result<Option<SubmissionRecord>, StorageFailure> {
        self.guard_read()?;
        self.reader.submission(id).await
    }

    async fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: PageLimit,
        from: Option<SubmissionCursor>,
        _cx: &Cx,
    ) -> Result<Page<SubmissionRecord, SubmissionCursor>, StorageFailure> {
        self.guard_read()?;
        self.reader.scan_submissions(query, limit, from).await
    }

    async fn submission_by_request(
        &self,
        conversation_id: ConversationId,
        request_id: &str,
        _cx: &Cx,
    ) -> Result<Option<SubmissionRecord>, StorageFailure> {
        self.guard_read()?;
        self.reader
            .submission_by_request(conversation_id, request_id)
            .await
    }

    async fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
        _cx: &Cx,
    ) -> Result<Option<DocumentRecord>, StorageFailure> {
        self.guard_read()?;
        self.reader.find_document(address, at).await
    }

    async fn document(
        &self,
        id: DocumentId,
        at: DocumentPoint,
        _cx: &Cx,
    ) -> Result<Option<StoredDocument>, StorageFailure> {
        self.guard_read()?;
        self.reader.document(id, at).await
    }

    async fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: PageLimit,
        from: Option<DocumentCursor>,
        _cx: &Cx,
    ) -> Result<Page<DocumentRecord, DocumentCursor>, StorageFailure> {
        self.guard_read()?;
        self.reader.scan_documents(query, limit, from).await
    }
}

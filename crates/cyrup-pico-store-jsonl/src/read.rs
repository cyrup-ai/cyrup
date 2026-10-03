//! [`Reader`] — every read path, shared by the writable store and the read-only one.
//!
//! # Why the reads live in a type of their own
//!
//! ADR-0030 F6 §D asks for `open_read_only` *"taking no lock and having no `commit`"*. Written as two
//! independent types that each implement [`Storage`](cyrup_pico_store::Storage), the read-only one
//! would have a `commit` — the trait has one — and *"having no commit"* would be a runtime refusal
//! rather than a fact. So the reads are here, as inherent methods;
//! [`ReadOnlyStore`](crate::ReadOnlyStore) is this type and nothing else, and
//! [`JsonlStore`](crate::JsonlStore) delegates its `Storage` read methods here after consulting its
//! poison flag. *"No commit"* is then structural:
//! `tests/compile-fail/a_read_only_store_cannot_commit.rs` is the proof.
//!
//! # Where the read bodies actually are
//!
//! In [`crate::query`], on [`Planned`], which is the same walk plus the [`Plan`](crate::Plan) naming the
//! index that answered it. The methods here are one-line delegations that drop the plan, so there is one
//! implementation of each path rather than two — see that module's documentation for why the plan is
//! produced by the walk instead of counted beside it.
//!
//! # The one place a read touches the medium
//!
//! [`Reader::content`], and only through the byte span the marker-carried offsets already named
//! (ADR-0030 F6 §D). Every other read is answered from an in-memory index, which is
//! `spec.md:4325-4351`'s requirement rather than an optimisation.
//!
//! # Every method here is `async` and only one of them awaits anything
//!
//! The signatures mirror [`Storage`](cyrup_pico_store::Storage)'s so the delegation in [`crate::store`]
//! is a plain `await` rather than a second shape, and so a future backend that *does* need to await —
//! an engine behind a connection pool — is a change of body rather than of surface. That every other
//! read answers from memory is the property `spec.md:4325-4351` asks for, not an accident.
//!
//! # Blocking I/O inside an `async fn`, stated rather than hidden
//!
//! `Storage` is async because the trait has to serve a backend that needs a runtime; this one does
//! not. [`Reader::content`] does blocking file I/O on the caller's task, which is the same posture
//! `cyrup-session`'s `DiskStore` takes for the same reason: the work is one seek and one short read,
//! and wrapping it in `spawn_blocking` would require the whole state to be `Sync + 'static` and would
//! add a scheduling hop to a read that is usually a page-cache hit. If a profile ever says otherwise,
//! the change is local to this function.

use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use cyrup_pico_doc::{DocRoot, ReplayPlan, StoredContent, StoredVersion, materialize};
use cyrup_pico_store::{
    CommittedEntry, ConversationCursor, ConversationId, ConversationQuery, ConversationRecord,
    Corruption, CursorBytes, DocumentAddress, DocumentCursor, DocumentId, DocumentPoint,
    DocumentQuery, DocumentRecord, EntryCursor, EntryId, EntryQuery, EntryRecord, HeadMarker, Page,
    PageLimit, RawId, Seq, StorageFailure, StoreId, StoredDocument, SubmissionCursor, SubmissionId,
    SubmissionQuery, SubmissionRecord, TaskCursor, TaskId, TaskQuery, TaskRecord, WrongStore,
};

use crate::identity::StoreIdentity;
use crate::query::Planned;
use crate::state::{Committed, DocState};
use crate::wire::{ContentLine, sidecar_path};

/// Every read path over one store's committed state.
#[derive(Debug)]
pub struct Reader {
    pub(crate) dir: PathBuf,
    pub(crate) identity: StoreIdentity,
    pub(crate) state: Committed,
}

impl Reader {
    /// This store's directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// This store's durable identity record.
    #[must_use]
    pub const fn identity(&self) -> &StoreIdentity {
        &self.identity
    }

    /// This store's durable identity, as the cursors it issues carry it.
    #[must_use]
    pub const fn store_id(&self) -> StoreId {
        self.identity.store
    }

    /// The newest commit's sequence, if there is one.
    #[must_use]
    pub const fn last_commit(&self) -> Option<Seq> {
        self.state.last_seq
    }

    /// How many commits this store holds.
    ///
    /// Not derivable from [`Reader::last_commit`], because `spec.md:99-100` permits gaps in the sequence.
    /// It is the **count** that ADR-0030 §9's *"open is O(commits + table records)"* is linear in, which
    /// is why a benchmark needs it reported rather than inferred.
    #[must_use]
    pub const fn commit_count(&self) -> u64 {
        self.state.commits
    }

    /// The same read paths, each paired with the index that answered it.
    ///
    /// This is the surface the query-plan suite drives, and the one PICO5-PLAN S10 exists to make
    /// assertable: ADR-0030 §2.2 classifies *"whether a backend uses an index"* **checked**, and
    /// ADR-0030 §6 names the check — *"query-plan and benchmark suites"*. Every method on [`Reader`]
    /// below delegates to one of these, so a plan is a statement about the code that ran.
    #[must_use]
    pub const fn plans(&self) -> Planned<'_> {
        Planned::new(self)
    }

    /// One conversation by id.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    pub async fn conversation(
        &self,
        id: ConversationId,
    ) -> Result<Option<ConversationRecord>, StorageFailure> {
        self.plans().conversation(id).map(first)
    }

    /// Page conversations by indexed, conjunctive owner filters (`spec.md:4337-4339`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub async fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: PageLimit,
        from: Option<ConversationCursor>,
    ) -> Result<Page<ConversationRecord, ConversationCursor>, StorageFailure> {
        self.plans()
            .scan_conversations(query, limit, from.as_ref())
            .map(first)
    }

    /// One entry by id, with the commit sequence (`spec.md:4347`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    pub async fn entry(&self, id: EntryId) -> Result<Option<CommittedEntry>, StorageFailure> {
        self.plans().entry(id).map(first)
    }

    /// One entry by id, only if visible through `conversation_id`'s ancestry (`spec.md:4348`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Not visible is `Ok(None)`: an entry outside the requested ancestry is
    /// legitimately absent *from that conversation*.
    pub async fn entry_in(
        &self,
        conversation_id: ConversationId,
        id: EntryId,
    ) -> Result<Option<CommittedEntry>, StorageFailure> {
        self.plans().entry_in(conversation_id, id).map(first)
    }

    /// The newest visible entry carrying a head at or before a cutoff (`spec.md:4331-4334`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub async fn find_latest_head_marker(
        &self,
        conversation_id: ConversationId,
        at_or_before: Option<EntryId>,
    ) -> Result<Option<HeadMarker>, StorageFailure> {
        self.plans()
            .find_latest_head_marker(conversation_id, at_or_before)
            .map(first)
    }

    /// Page one conversation's visible history, newest first (`spec.md:4334-4336`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub async fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: PageLimit,
        from: Option<EntryCursor>,
    ) -> Result<Page<EntryRecord, EntryCursor>, StorageFailure> {
        self.plans()
            .scan_entries(query, limit, from.as_ref())
            .map(first)
    }

    /// One task by id.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    pub async fn task(&self, id: TaskId) -> Result<Option<TaskRecord>, StorageFailure> {
        self.plans().task(id).map(first)
    }

    /// Page tasks by the five indexed fields (`spec.md:4348`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub async fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: PageLimit,
        from: Option<TaskCursor>,
    ) -> Result<Page<TaskRecord, TaskCursor>, StorageFailure> {
        self.plans()
            .scan_tasks(query, limit, from.as_ref())
            .map(first)
    }

    /// One submission by id.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    pub async fn submission(
        &self,
        id: SubmissionId,
    ) -> Result<Option<SubmissionRecord>, StorageFailure> {
        self.plans().submission(id).map(first)
    }

    /// Page submissions.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub async fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: PageLimit,
        from: Option<SubmissionCursor>,
    ) -> Result<Page<SubmissionRecord, SubmissionCursor>, StorageFailure> {
        self.plans()
            .scan_submissions(query, limit, from.as_ref())
            .map(first)
    }

    /// One submission by its caller-supplied request id (`spec.md:4314`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    pub async fn submission_by_request(
        &self,
        conversation_id: ConversationId,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, StorageFailure> {
        self.plans()
            .submission_by_request(conversation_id, request_id)
            .map(first)
    }

    /// Resolve one exact logical document address at a point (`spec.md:4340-4342`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. No incarnation alive at that point is `Ok(None)`.
    pub async fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
    ) -> Result<Option<DocumentRecord>, StorageFailure> {
        self.plans().find_document(address, at).map(first)
    }

    /// Materialize one specific incarnation (`spec.md:4345-4360`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. An unknown id is `Ok(None)`; a retired incarnation at
    /// [`DocumentPoint::Current`] is `Ok(None)`; a numeric point outside a rewindable incarnation's
    /// lifetime is `Ok(None)`; a numeric point on a current-only incarnation is
    /// [`StorageFailure::HistoryNotRetained`].
    pub async fn document(
        &self,
        id: DocumentId,
        at: DocumentPoint,
    ) -> Result<Option<StoredDocument>, StorageFailure> {
        self.materialize(id, at)
    }

    /// Enumerate the incarnations alive in one exact scope at a point (`spec.md:4342-4345`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub async fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: PageLimit,
        from: Option<DocumentCursor>,
    ) -> Result<Page<DocumentRecord, DocumentCursor>, StorageFailure> {
        self.plans()
            .scan_documents(query, limit, from.as_ref())
            .map(first)
    }

    /// Select, read and replay one incarnation's content.
    ///
    /// The body of both [`Reader::document`] and [`Planned::document`]: a keyed probe for the record,
    /// then the byte span the markers named. `spec.md:4350`'s *"the lookup never scans unrelated
    /// documents"* is a property of this function having no iteration in it at all.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`], as [`Reader::document`].
    pub(crate) fn materialize(
        &self,
        id: DocumentId,
        at: DocumentPoint,
    ) -> Result<Option<StoredDocument>, StorageFailure> {
        let Some(state) = self.state.documents.get(&id) else {
            return Ok(None);
        };
        let upto = match at {
            DocumentPoint::Current => {
                if !state.record.lifetime.is_open() {
                    return Ok(None);
                }
                None
            }
            DocumentPoint::At(seq) => {
                if !state.record.retains_history() {
                    // `spec.md:4356-4357`: reject rather than depend on reclaimed content.
                    return Err(StorageFailure::HistoryNotRetained {
                        document: id,
                        at: seq,
                    });
                }
                if !state.record.lifetime.contains(seq) {
                    return Ok(None);
                }
                Some(seq)
            }
        };
        let (version, value, deltas_since_base) = self.content(state, upto)?;
        Ok(Some(StoredDocument {
            record: state.record.clone(),
            version,
            value,
            deltas_since_base,
        }))
    }

    /// Materialize one incarnation's content, considering only records at or before `upto`.
    ///
    /// **One seek plus the tail.** The byte span comes from the marker-carried offsets (ADR-0030 F6 §D),
    /// so this reads the newest applicable base and the deltas after it, and never the records a later
    /// base superseded — whether or not reclamation has got round to removing them.
    ///
    /// # Errors
    ///
    /// [`StorageFailure::Corrupt`] when the records in the span cannot be replayed, when there are
    /// fewer of them than the markers promised, or when one cannot be decoded.
    /// [`StorageFailure::Io`] from the read itself.
    pub(crate) fn content(
        &self,
        state: &DocState,
        upto: Option<Seq>,
    ) -> Result<(StoredVersion, DocRoot, u32), StorageFailure> {
        let records = self.read_span(state, upto)?;
        let plan = ReplayPlan::parse(&records).map_err(|source| {
            StorageFailure::Corrupt(Corruption::Replay {
                document: state.record.id,
                source,
            })
        })?;
        Ok((plan.version(), materialize(&plan), plan.deltas_since_base()))
    }

    /// Read and decode the content records a read at `upto` selects.
    fn read_span(
        &self,
        state: &DocState,
        upto: Option<Seq>,
    ) -> Result<Vec<StoredContent>, StorageFailure> {
        let span = state.span(upto).ok_or_else(|| {
            StorageFailure::Corrupt(Corruption::MissingConfirmedData {
                what: format!(
                    "document {}'s content: its record is present with no content record",
                    state.record.id
                ),
            })
        })?;
        let path = sidecar_path(&self.dir, state.record.id, state.generation);
        let bytes = read_range(&path, span.start, span.end).map_err(StorageFailure::Io)?;
        let text = String::from_utf8(bytes).map_err(|e| {
            StorageFailure::Corrupt(Corruption::RecordMalformed {
                what: "document content",
                detail: e.to_string(),
            })
        })?;
        let mut records = Vec::with_capacity(span.records());
        for (offset, line) in text.lines().enumerate() {
            let decoded: ContentLine = serde_json::from_str(line).map_err(|e| {
                StorageFailure::Corrupt(Corruption::RecordMalformed {
                    what: "document content",
                    detail: format!("{}: {e}", path.display()),
                })
            })?;
            // The cross-check the marker's offsets buy: a record whose sequence disagrees with the
            // slice it was read from means the sidecar and the markers describe different histories,
            // which is corruption rather than a different answer.
            let expected = state.slices.get(span.first.saturating_add(offset));
            if expected.is_some_and(|slice| slice.seq != decoded.seq) {
                return Err(StorageFailure::Corrupt(Corruption::RecordMalformed {
                    what: "document content",
                    detail: format!(
                        "{}: record at offset {} is from commit {}, but marker {} claims that span",
                        path.display(),
                        offset,
                        decoded.seq,
                        expected.map_or_else(|| "?".to_owned(), |s| s.seq.to_string()),
                    ),
                }));
            }
            records.push(decoded.content);
        }
        if records.len() != span.records() {
            return Err(StorageFailure::Corrupt(Corruption::MissingConfirmedData {
                what: format!(
                    "document {}: {} content records in {}..{} of {}, {} confirmed by markers",
                    state.record.id,
                    records.len(),
                    span.start,
                    span.end,
                    path.display(),
                    span.records(),
                ),
            }));
        }
        Ok(records)
    }

    /// Decode a cursor this store issued.
    pub(crate) fn resume(
        &self,
        payload: Option<Result<&[u8], WrongStore>>,
    ) -> Result<Option<core::num::NonZeroU64>, StorageFailure> {
        let Some(payload) = payload else {
            return Ok(None);
        };
        let payload = payload.map_err(wrong_store)?;
        let bytes: [u8; 8] = payload.try_into().map_err(|_| {
            StorageFailure::Corrupt(Corruption::RecordMalformed {
                what: "cursor",
                detail: format!("expected an 8-byte scan position, found {}", payload.len()),
            })
        })?;
        // Zero is not an id (`spec.md:249`), so a cursor carrying it is a cursor this store did not
        // write — reported as a bad argument for the same reason a foreign store's cursor is.
        core::num::NonZeroU64::new(u64::from_be_bytes(bytes))
            .map(Some)
            .ok_or_else(|| {
                StorageFailure::Corrupt(Corruption::RecordMalformed {
                    what: "cursor",
                    detail: "a scan position of zero is not an id".to_owned(),
                })
            })
    }

    /// Stamp a scan position for this store.
    pub(crate) fn cursor_at(&self, id: RawId) -> CursorBytes {
        CursorBytes::new(self.store_id(), id.get().get().to_be_bytes().to_vec())
    }
}

/// Drop a plan from a planned read's answer.
fn first<T, P>((answer, _plan): (T, P)) -> T {
    answer
}

/// Apply a limit to one already-ordered, already-filtered scan.
pub(crate) fn page<T, C>(items: Vec<T>, limit: PageLimit, cursor: impl Fn(&T) -> C) -> Page<T, C> {
    let max = limit.get() as usize;
    if items.len() > max {
        let mut items = items;
        items.truncate(max);
        match items.last() {
            Some(last) => {
                let next = cursor(last);
                Page::more(items, next)
            }
            None => Page::last(items),
        }
    } else {
        Page::last(items)
    }
}

/// A cursor presented to a different store is a bad argument, not damaged data.
///
/// ADR-0030 §10 fixes [`StorageFailure`] at three arms and this is none of them: the store is not
/// corrupt, nothing is absent, and no history was reclaimed. `spec.md:4320-4321` calls cross-storage
/// cursor use *"unsupported"*, so it is reported as [`io::ErrorKind::InvalidInput`] — the standard name
/// for an argument that cannot be honoured — rather than by widening the enum or, worse, by resuming the
/// scan at a position meaning something else. `MemoryStore` answers the same way, so the conformance
/// suite cannot tell the two backends apart by it.
fn wrong_store(e: WrongStore) -> StorageFailure {
    StorageFailure::Io(io::Error::new(io::ErrorKind::InvalidInput, e))
}

/// Read `start..end` of a file.
///
/// Also used by reclamation, which copies the records it keeps as **bytes** so the replacement is
/// byte-identical to the tail it replaces.
pub(crate) fn read_range(path: &Path, start: u64, end: u64) -> io::Result<Vec<u8>> {
    let len = end.saturating_sub(start);
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    f.take(len).read_to_end(&mut buf)?;
    Ok(buf)
}

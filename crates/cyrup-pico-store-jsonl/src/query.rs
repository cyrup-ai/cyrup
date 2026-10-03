//! [`Planned`] — every named access path, as the walk of one index plus the plan that walk is
//! (PICO5-PLAN S10).
//!
//! # Why the reads are here and not in [`crate::read`]
//!
//! [`Reader`]'s methods mirror [`Storage`](cyrup_pico_store::Storage)'s signatures so the delegation in
//! [`crate::store`] is a plain `await`. A read also has to be able to say **which index answered it**,
//! because ADR-0030 §2.2 classifies that half of `spec.md:4325-4351` *checked* and ADR-0030 §6 says what
//! does the checking: *"nothing forces a backend to use an index (query-plan and benchmark suites do
//! that)"*. Those two shapes are different — one returns a page, the other a page and a [`Plan`] — and
//! the wrong way to have both is to keep two implementations.
//!
//! So this module holds the implementations and `Reader`'s methods are one-line delegations that drop
//! the plan. A read has exactly one body, the plan is the body's own account of itself, and a plan
//! naming `EntriesByConversation` cannot be a read that walked the table.
//!
//! # Every path here, and what bounds it
//!
//! | path | driven by | visits |
//! |---|---|---|
//! | `conversation`, `entry`, `task`, `submission`, `document` | the primary table | 1 |
//! | `entry_in` | the ancestry chain | chain depth |
//! | `find_latest_head_marker` | `heads_by_conversation`, per level | ≤ 2 × chain depth |
//! | `scan_entries` | `entries_by_conversation`, ranged per level | chain depth + page |
//! | `scan_conversations` | the smaller owner index | matching conversations |
//! | `scan_tasks` | the smallest named task index | that index's members |
//! | `scan_submissions` | the smallest named submission index | that index's members |
//! | `submission_by_request` | `submissions_by_request` | 1 |
//! | `find_document` | `documents_by_address` | incarnations ever at that address |
//! | `scan_documents` | `documents_by_scope` | incarnations ever in that scope |
//!
//! Not one of those bounds mentions total history, which is the property ADR-0030 §2.2 asks for and the
//! one `tests/query_plan.rs` asserts by growing a store and re-reading.

use cyrup_pico_store::{
    CommittedEntry, ConversationCursor, ConversationId, ConversationQuery, ConversationRecord,
    DocumentAddress, DocumentCursor, DocumentId, DocumentPoint, DocumentQuery, DocumentRecord,
    EntryCursor, EntryId, EntryQuery, EntryRecord, HeadMarker, Page, PageLimit, RawId,
    StorageFailure, StoredDocument, SubmissionCursor, SubmissionId, SubmissionQuery,
    SubmissionRecord, TaskCursor, TaskId, TaskQuery, TaskRecord,
};

use crate::index::Footprint;
use crate::plan::{Index, Plan};
use crate::read::{Reader, page};
use crate::state::saturating_page;

/// Every read path, with the plan it was answered by.
///
/// Obtained from [`Reader::plans`]. The methods take the same arguments as
/// [`Storage`](cyrup_pico_store::Storage)'s and return the same answers, paired with a [`Plan`].
///
/// They are **not** `async`, unlike `Reader`'s: every one of them is answered from memory except
/// [`Planned::document`], and that one does blocking I/O for one byte span
/// ([`Reader::content`](crate::Reader)). `Reader`'s async surface exists so the trait's shape is met and
/// so a future backend behind a connection pool is a change of body; nothing here needs it, and a
/// non-`async` query-plan surface is one less thing for a test to drive through a runtime.
#[derive(Clone, Copy, Debug)]
pub struct Planned<'r> {
    reader: &'r Reader,
}

impl<'r> Planned<'r> {
    /// The query-plan surface over one reader.
    pub(crate) const fn new(reader: &'r Reader) -> Self {
        Self { reader }
    }

    /// What this store costs in memory, for ADR-0030 §14 open question 2.
    ///
    /// `O(records)`, and it re-serialises every record, so it belongs to a benchmark rather than to a
    /// read path. See [`Footprint`] for why it is two numbers.
    #[must_use]
    pub fn footprint(self) -> Footprint {
        self.reader.state.footprint()
    }

    /// One conversation by id.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    pub fn conversation(
        self,
        id: ConversationId,
    ) -> Result<(Option<ConversationRecord>, Plan), StorageFailure> {
        Ok((
            self.reader.state.conversations.get(&id).cloned(),
            Plan::keyed(),
        ))
    }

    /// Page conversations by indexed, conjunctive owner filters (`spec.md:4337-4339`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub fn scan_conversations(
        self,
        query: &ConversationQuery,
        limit: PageLimit,
        from: Option<&ConversationCursor>,
    ) -> Result<(Page<ConversationRecord, ConversationCursor>, Plan), StorageFailure> {
        let after = self
            .reader
            .resume(from.map(|c| c.payload_for(self.store())))?;
        let (index, candidates) = self.reader.state.conversation_candidates(query, after);
        let mut visited = 0u64;
        let mut items = Vec::new();
        for id in candidates {
            visited = visited.saturating_add(1);
            let Some(record) = self.reader.state.conversations.get(id) else {
                continue;
            };
            // Conjunctive: an absent filter does not constrain (`spec.md:4337-4338`). The driving index
            // already decided one of these; the other is checked here.
            let matches = query
                .owner_conversation_id
                .is_none_or(|want| record.owner.is_some_and(|o| o.conversation_id == want))
                && query
                    .owner_task_id
                    .is_none_or(|want| record.owner.is_some_and(|o| o.task_id == want));
            if !matches {
                continue;
            }
            items.push(record.clone());
            if items.len() >= saturating_page(limit) {
                break;
            }
        }
        let page = page(items, limit, |record| {
            ConversationCursor::new(self.reader.cursor_at(RawId::from(record.id)))
        });
        Ok((page, Plan::new(index, visited)))
    }

    /// One entry by id, with the commit sequence (`spec.md:4347`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    pub fn entry(self, id: EntryId) -> Result<(Option<CommittedEntry>, Plan), StorageFailure> {
        let found = self
            .reader
            .state
            .entries
            .get(&id)
            .map(|(entry, commit_seq)| CommittedEntry {
                entry: entry.clone(),
                commit_seq: *commit_seq,
            });
        Ok((found, Plan::keyed()))
    }

    /// One entry by id, only if visible through `conversation_id`'s ancestry (`spec.md:4348`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Not visible is `Ok(None)`.
    pub fn entry_in(
        self,
        conversation_id: ConversationId,
        id: EntryId,
    ) -> Result<(Option<CommittedEntry>, Plan), StorageFailure> {
        let Some((entry, commit_seq)) = self.reader.state.entries.get(&id) else {
            return Ok((None, Plan::keyed()));
        };
        let (visited, visible) =
            self.reader
                .state
                .entry_is_visible(conversation_id, id, entry.conversation_id);
        let found = visible.then(|| CommittedEntry {
            entry: entry.clone(),
            commit_seq: *commit_seq,
        });
        Ok((found, Plan::new(Index::Ancestry, visited)))
    }

    /// The newest visible entry carrying a head at or before a cutoff (`spec.md:4331-4334`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub fn find_latest_head_marker(
        self,
        conversation_id: ConversationId,
        at_or_before: Option<EntryId>,
    ) -> Result<(Option<HeadMarker>, Plan), StorageFailure> {
        let (visited, found) = self.reader.state.latest_head(conversation_id, at_or_before);
        let marker = found
            .and_then(|id| self.reader.state.entries.get(&id))
            // The index holds only entries whose `head` is present, so this cannot be `None` for an id
            // the index yielded; `filter_map` rather than an assertion because a damaged store is
            // reported by the open, not by a read (`spec.md:4433`).
            .and_then(|(entry, _)| {
                entry.head.map(|head| HeadMarker {
                    entry: entry.clone(),
                    head,
                })
            });
        Ok((marker, Plan::new(Index::HeadsByConversation, visited)))
    }

    /// Page one conversation's visible history, newest first (`spec.md:4334-4336`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub fn scan_entries(
        self,
        query: &EntryQuery,
        limit: PageLimit,
        from: Option<&EntryCursor>,
    ) -> Result<(Page<EntryRecord, EntryCursor>, Plan), StorageFailure> {
        let after = self
            .reader
            .resume(from.map(|c| c.payload_for(self.store())))?;
        // The range bounds and every ancestry cap are applied by the walk, before an entry is drawn:
        // `spec.md:4334-4336`'s *"inclusive ID range … while applying every conversation ancestry cap"*
        // is the iterator's construction, not a filter over it.
        let mut walk = self.reader.state.visible_entries(query, after);
        let mut items = Vec::new();
        for id in walk.by_ref() {
            if let Some((entry, _)) = self.reader.state.entries.get(&id) {
                items.push(entry.clone());
            }
            if items.len() >= saturating_page(limit) {
                break;
            }
        }
        let visited = walk.visited();
        let page = page(items, limit, |entry| {
            EntryCursor::new(self.reader.cursor_at(RawId::from(entry.id)))
        });
        Ok((page, Plan::new(Index::EntriesByConversation, visited)))
    }

    /// One task by id.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    pub fn task(self, id: TaskId) -> Result<(Option<TaskRecord>, Plan), StorageFailure> {
        Ok((self.reader.state.tasks.get(&id).cloned(), Plan::keyed()))
    }

    /// Page tasks by the five indexed fields (`spec.md:4348`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub fn scan_tasks(
        self,
        query: &TaskQuery,
        limit: PageLimit,
        from: Option<&TaskCursor>,
    ) -> Result<(Page<TaskRecord, TaskCursor>, Plan), StorageFailure> {
        let after = self
            .reader
            .resume(from.map(|c| c.payload_for(self.store())))?;
        let (index, candidates) = self.reader.state.task_candidates(query, after);
        let mut visited = 0u64;
        let mut items = Vec::new();
        for id in candidates {
            visited = visited.saturating_add(1);
            let Some(record) = self.reader.state.tasks.get(id) else {
                continue;
            };
            let matches = query
                .conversation_id
                .is_none_or(|want| record.conversation_id == want)
                && query.kind.as_ref().is_none_or(|want| record.kind == *want)
                && query.status.is_none_or(|want| record.status() == want)
                && query
                    .abort_requested
                    .is_none_or(|want| record.abort_requested == want)
                && query
                    .background
                    .is_none_or(|want| record.background == want);
            if !matches {
                continue;
            }
            items.push(record.clone());
            if items.len() >= saturating_page(limit) {
                break;
            }
        }
        let page = page(items, limit, |record| {
            TaskCursor::new(self.reader.cursor_at(RawId::from(record.id)))
        });
        Ok((page, Plan::new(index, visited)))
    }

    /// One submission by id.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    pub fn submission(
        self,
        id: SubmissionId,
    ) -> Result<(Option<SubmissionRecord>, Plan), StorageFailure> {
        Ok((
            self.reader.state.submissions.get(&id).cloned(),
            Plan::keyed(),
        ))
    }

    /// Page submissions.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub fn scan_submissions(
        self,
        query: &SubmissionQuery,
        limit: PageLimit,
        from: Option<&SubmissionCursor>,
    ) -> Result<(Page<SubmissionRecord, SubmissionCursor>, Plan), StorageFailure> {
        let after = self
            .reader
            .resume(from.map(|c| c.payload_for(self.store())))?;
        let (index, candidates) = self.reader.state.submission_candidates(query, after);
        let mut visited = 0u64;
        let mut items = Vec::new();
        for id in candidates {
            visited = visited.saturating_add(1);
            let Some(record) = self.reader.state.submissions.get(id) else {
                continue;
            };
            let matches = query
                .conversation_id
                .is_none_or(|want| record.conversation_id == want)
                && query.status.is_none_or(|want| record.status() == want)
                && query
                    .submission_type
                    .is_none_or(|want| record.submission_type() == want);
            if !matches {
                continue;
            }
            items.push(record.clone());
            if items.len() >= saturating_page(limit) {
                break;
            }
        }
        let page = page(items, limit, |record| {
            SubmissionCursor::new(self.reader.cursor_at(RawId::from(record.id)))
        });
        Ok((page, Plan::new(index, visited)))
    }

    /// One submission by its caller-supplied request id (`spec.md:4314`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. Absence is `Ok(None)`.
    pub fn submission_by_request(
        self,
        conversation_id: ConversationId,
        request_id: &str,
    ) -> Result<(Option<SubmissionRecord>, Plan), StorageFailure> {
        let found = self
            .reader
            .state
            .submission_by_request(conversation_id, request_id)
            .and_then(|id| self.reader.state.submissions.get(&id))
            .cloned();
        Ok((found, Plan::new(Index::SubmissionsByRequest, 1)))
    }

    /// Resolve one exact logical document address at a point (`spec.md:4340-4342`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. No incarnation alive at that point is `Ok(None)`.
    pub fn find_document(
        self,
        address: &DocumentAddress,
        at: DocumentPoint,
    ) -> Result<(Option<DocumentRecord>, Plan), StorageFailure> {
        let mut visited = 0u64;
        let mut found = None;
        // Newest incarnation first, because a retire-plus-create at one address leaves the new one
        // current at that sequence (`spec.md:4364-4365`).
        for id in self.reader.state.address_candidates(address) {
            visited = visited.saturating_add(1);
            if let Some(state) = self.reader.state.documents.get(id)
                && alive_at(&state.record, at)
            {
                found = Some(state.record.clone());
                break;
            }
        }
        Ok((found, Plan::new(Index::DocumentsByAddress, visited)))
    }

    /// Materialize one specific incarnation (`spec.md:4345-4360`).
    ///
    /// *"The lookup never scans unrelated documents"* (`spec.md:4350`): the incarnation is a keyed probe
    /// and its content is the byte span the marker offsets already named, so no other document is
    /// touched — which is the half of this path a [`Plan`] can state and a scan could not fake.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`]. See [`Reader::document`] for which absence is which.
    pub fn document(
        self,
        id: DocumentId,
        at: DocumentPoint,
    ) -> Result<(Option<StoredDocument>, Plan), StorageFailure> {
        self.reader.materialize(id, at).map(|d| (d, Plan::keyed()))
    }

    /// Enumerate the incarnations alive in one exact scope at a point (`spec.md:4342-4345`).
    ///
    /// # Errors
    ///
    /// [`StorageFailure`].
    pub fn scan_documents(
        self,
        query: &DocumentQuery,
        limit: PageLimit,
        from: Option<&DocumentCursor>,
    ) -> Result<(Page<DocumentRecord, DocumentCursor>, Plan), StorageFailure> {
        let after = self
            .reader
            .resume(from.map(|c| c.payload_for(self.store())))?;
        let mut visited = 0u64;
        let mut items = Vec::new();
        // Ascending incarnation ids (`spec.md:4344`), which is the index's own order.
        for id in self.reader.state.document_candidates(query, after) {
            visited = visited.saturating_add(1);
            let Some(state) = self.reader.state.documents.get(id) else {
                continue;
            };
            let matches = query
                .kind
                .as_ref()
                .is_none_or(|want| state.record.kind == *want)
                && alive_at(&state.record, query.at);
            if !matches {
                continue;
            }
            items.push(state.record.clone());
            if items.len() >= saturating_page(limit) {
                break;
            }
        }
        let page = page(items, limit, |record| {
            DocumentCursor::new(self.reader.cursor_at(RawId::from(record.id)))
        });
        Ok((page, Plan::new(Index::DocumentsByScope, visited)))
    }

    /// The store whose cursors these scans accept.
    fn store(self) -> cyrup_pico_store::StoreId {
        self.reader.store_id()
    }
}

/// Whether an incarnation is a member of the store at `at`.
///
/// `spec.md:4358`: *"metadata membership remains queryable historically"* — so this is answered from the
/// record's lifetime for **every** scope, including the current-only ones whose *content* at a numeric
/// point is refused.
pub(crate) fn alive_at(record: &DocumentRecord, at: DocumentPoint) -> bool {
    match at {
        DocumentPoint::Current => record.lifetime.is_open(),
        DocumentPoint::At(seq) => record.lifetime.contains(seq),
    }
}

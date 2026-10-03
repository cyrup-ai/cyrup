//! The kernel indexes, and the one thing that chooses between them (PICO5-PLAN S10).
//!
//! # What makes these *kernel* indexes rather than caches
//!
//! `spec.md:4337-4339`, on the conversation owner filters, and the sentence that generalises:
//!
//! > They support ownership traversal without an all-conversation scan; application-maintained
//! > registries are not a substitute for these kernel indexes.
//!
//! So the named access paths of `spec.md:4325-4351` are the *only* place this information is allowed to
//! live. A caller cannot keep its own map and get the same answer, because `scan_entries` applies the
//! ancestry caps **inside** and a caller's registry has no way to. ADR-0030 §2.2 prices the alternative
//! in one line: *"if they degrade to scans, cost grows with total history: a long session gets slower
//! until it is unusable — the agent dies of old age."*
//!
//! # One writer, which is the whole reason these cannot drift
//!
//! Every index here is written by [`Committed::apply_line`](crate::state::Committed::apply_line) and by
//! nothing else — the same function that writes the record tables, for the reason
//! [`crate::state`]'s module documentation gives about the tables themselves: a durable backend reaches
//! a state either by replaying the log at open or by applying a commit it just wrote, and writing those
//! twice is how a file backend's recovery drifts from its commit path. An index with a second writer
//! would reintroduce exactly that drift one layer down, and it would be invisible until a reopen
//! answered a *query* differently from the session that wrote it. That is also why
//! `G-COMMIT-VISIBLE-TO-LATER-READS` (`spec.md:4281`: *"once `commit()` resolves, later reads through
//! that Storage observe it"*) survives the addition of these indexes rather than being endangered by
//! it: a commit that is applied is indexed, in one statement, or neither.
//!
//! # Replacement, which is where an index actually does get it wrong
//!
//! `spec.md:4383`: *"live task transitions replace one row"*, and the same holds for submissions. A
//! record whose `status` is indexed therefore has to be **removed from its old sets** when it is
//! replaced, and the obvious bug is to insert into the new set and leave the old membership behind — a
//! task that is reported both `running` and `terminal`. [`Indexes::put_task`] and
//! [`Indexes::put_submission`] take the previous record for that reason: the removal is not a separate
//! step a caller can forget.
//!
//! # What is *not* indexed, stated rather than left to be discovered
//!
//! * **A document's liveness at a point.** `alive_at` is evaluated on the record, after the index
//!   narrowed the candidates to one address or one scope, because a lifetime is an interval and the
//!   point is part of the query (`spec.md:4342-4344`). The bound is therefore *"incarnations ever at
//!   this address"*, not *"alive now"*, and a retired incarnation is a legitimate candidate — it is what
//!   a historical membership read is looking for.
//! * **`scan_documents`'s optional kind.** The scope is the index and the kind is a restriction within
//!   it, which is how `spec.md:4342-4345` words it: *"enumerates only the incarnations alive in one
//!   exact scope at its selected point **and may** restrict one family/singleton kind."* What the
//!   specification forbids in that sentence is the open-time all-document scan, and a scope-bounded walk
//!   is not one.
//! * **A task's or submission's residual fields.** A conjunctive query drives from its most selective
//!   present filter and checks the rest on the candidates, which is what a query planner does; the
//!   selectivity is read from the index's own `len`, so the choice is made from the data rather than
//!   from a guess baked into the code.

use std::collections::{BTreeMap, BTreeSet, btree_set};
use std::num::NonZeroU64;
use std::ops::Bound;

use cyrup_pico_store::{
    ConversationId, ConversationQuery, ConversationRecord, DocumentAddress, DocumentId,
    DocumentQuery, DocumentRecord, EntryId, EntryQuery, EntryRecord, Id, IdKind, IdKindTag, Kind,
    RawId, ScopeRef, SubmissionId, SubmissionQuery, SubmissionRecord, SubmissionStatus,
    SubmissionType, TaskId, TaskQuery, TaskRecord, TaskStatus,
};

use crate::plan::Index;
use crate::state::Committed;

/// Every secondary index this backend keeps, and nothing else.
///
/// Separate from [`Committed`]'s record tables so that *"which index answered this"* has one place to
/// be true, and so the accounting [`Indexes::resident_bytes`] does has one struct to walk.
#[derive(Debug, Default)]
pub(crate) struct Indexes {
    /// Conversations owned by each task (`spec.md:4337-4339`).
    conversations_by_owner_task: BTreeMap<TaskId, BTreeSet<ConversationId>>,
    /// Conversations owned by a task of each conversation (`spec.md:4337-4339`).
    conversations_by_owner_conversation: BTreeMap<ConversationId, BTreeSet<ConversationId>>,
    /// Each conversation's own entries, ordered by id.
    ///
    /// Only its own: the ancestry walk is what turns these into *visible* history, and keeping the
    /// inherited entries here too would mean every fork copied its parent's index.
    entries_by_conversation: BTreeMap<ConversationId, BTreeSet<EntryId>>,
    /// Each conversation's entries that carry a head (`spec.md:4331-4334`).
    ///
    /// A second, much smaller set rather than a flag checked while walking the first, because
    /// `find_latest_head_marker` asks for *the newest one at or before a cutoff* and a flag would make
    /// that a walk backwards through every entry since the last head.
    heads_by_conversation: BTreeMap<ConversationId, BTreeSet<EntryId>>,
    tasks_by_conversation: BTreeMap<ConversationId, BTreeSet<TaskId>>,
    tasks_by_kind: BTreeMap<Kind, BTreeSet<TaskId>>,
    tasks_by_status: BTreeMap<TaskStatus, BTreeSet<TaskId>>,
    /// Tasks partitioned by their durable abort mark.
    ///
    /// Keyed by the `bool` rather than kept as one "aborting" set, so that `abort_requested: Some(false)`
    /// is as indexed as `Some(true)`. The two sets partition the table, so this costs one id per task in
    /// total — the same as a single set would — and it is what lets the query-plan suite make one
    /// assertion for both polarities instead of excusing one of them.
    tasks_by_abort_mark: BTreeMap<bool, BTreeSet<TaskId>>,
    /// Tasks partitioned by background status, for the same reason.
    tasks_by_background: BTreeMap<bool, BTreeSet<TaskId>>,
    submissions_by_conversation: BTreeMap<ConversationId, BTreeSet<SubmissionId>>,
    submissions_by_status: BTreeMap<SubmissionStatus, BTreeSet<SubmissionId>>,
    submissions_by_type: BTreeMap<SubmissionType, BTreeSet<SubmissionId>>,
    /// One conversation's submissions by the caller's idempotency key (`spec.md:4314`).
    ///
    /// Nested rather than keyed by a `(ConversationId, String)` tuple so a lookup borrows the request id
    /// as a `&str` instead of allocating a `String` to build a key with.
    submissions_by_request: BTreeMap<ConversationId, BTreeMap<Box<str>, SubmissionId>>,
    /// The incarnations ever created at each logical address, ascending by id (`spec.md:4340-4342`).
    documents_by_address: BTreeMap<DocumentAddress, BTreeSet<DocumentId>>,
    /// The incarnations ever created in each scope, ascending by id (`spec.md:4342-4345`).
    documents_by_scope: BTreeMap<ScopeRef, BTreeSet<DocumentId>>,
}

impl Indexes {
    /// File a newly created conversation.
    pub(crate) fn add_conversation(&mut self, record: &ConversationRecord) {
        if let Some(owner) = record.owner {
            self.conversations_by_owner_task
                .entry(owner.task_id)
                .or_default()
                .insert(record.id);
            self.conversations_by_owner_conversation
                .entry(owner.conversation_id)
                .or_default()
                .insert(record.id);
        }
    }

    /// File a newly created entry, and its head if it carries one.
    pub(crate) fn add_entry(&mut self, record: &EntryRecord) {
        self.entries_by_conversation
            .entry(record.conversation_id)
            .or_default()
            .insert(record.id);
        if record.head.is_some() {
            self.heads_by_conversation
                .entry(record.conversation_id)
                .or_default()
                .insert(record.id);
        }
    }

    /// File a task record, retiring the memberships of the record it replaces.
    ///
    /// `previous` is the row this one replaces (`spec.md:4383`), or `None` for a first write. Passing it
    /// is not optional bookkeeping: without the removals a transitioned task stays in its old status set
    /// and `scan_tasks` reports it in two states at once.
    pub(crate) fn put_task(&mut self, previous: Option<&TaskRecord>, record: &TaskRecord) {
        if let Some(old) = previous {
            remove(
                &mut self.tasks_by_conversation,
                &old.conversation_id,
                old.id,
            );
            remove(&mut self.tasks_by_kind, &old.kind, old.id);
            remove(&mut self.tasks_by_status, &old.status(), old.id);
            remove(&mut self.tasks_by_abort_mark, &old.abort_requested, old.id);
            remove(&mut self.tasks_by_background, &old.background, old.id);
        }
        add(
            &mut self.tasks_by_conversation,
            record.conversation_id,
            record.id,
        );
        add(&mut self.tasks_by_kind, record.kind.clone(), record.id);
        add(&mut self.tasks_by_status, record.status(), record.id);
        add(
            &mut self.tasks_by_abort_mark,
            record.abort_requested,
            record.id,
        );
        add(&mut self.tasks_by_background, record.background, record.id);
    }

    /// File a submission record, retiring the memberships of the record it replaces.
    pub(crate) fn put_submission(
        &mut self,
        previous: Option<&SubmissionRecord>,
        record: &SubmissionRecord,
    ) {
        if let Some(old) = previous {
            remove(
                &mut self.submissions_by_conversation,
                &old.conversation_id,
                old.id,
            );
            remove(&mut self.submissions_by_status, &old.status(), old.id);
            remove(
                &mut self.submissions_by_type,
                &old.submission_type(),
                old.id,
            );
            if let Some(request) = old.request_id.as_deref()
                && let Some(by_request) = self.submissions_by_request.get_mut(&old.conversation_id)
            {
                by_request.remove(request);
            }
        }
        add(
            &mut self.submissions_by_conversation,
            record.conversation_id,
            record.id,
        );
        add(&mut self.submissions_by_status, record.status(), record.id);
        add(
            &mut self.submissions_by_type,
            record.submission_type(),
            record.id,
        );
        if let Some(request) = record.request_id.as_deref() {
            self.submissions_by_request
                .entry(record.conversation_id)
                .or_default()
                .insert(Box::from(request), record.id);
        }
    }

    /// File a newly created document incarnation.
    ///
    /// Nothing removes one. An incarnation's id is never reused (`spec.md:1113-1114`) and retirement
    /// moves its lifetime rather than its address, so both memberships are facts for the store's whole
    /// life — which is exactly what makes a historical membership read answerable.
    pub(crate) fn add_document(&mut self, record: &DocumentRecord) {
        add(&mut self.documents_by_address, record.address(), record.id);
        add(
            &mut self.documents_by_scope,
            record.scope.as_ref(),
            record.id,
        );
    }

    /// An accounting of what these indexes cost in memory, for ADR-0030 §14 open question 2.
    ///
    /// Rust has no heap introspection, so this is a **model** and its terms are stated rather than
    /// hidden. Each `BTreeMap`/`BTreeSet` node holds up to 11 key-value pairs in one allocation, so the
    /// per-element overhead a container adds is small and dominated by the keys themselves; the model
    /// charges each element its key and value sizes plus [`NODE_OVERHEAD_PER_ELEMENT`] bytes, and each
    /// non-empty container one [`CONTAINER_OVERHEAD`]. A `Kind` or a request id charges its string bytes
    /// as well, because those are separate allocations.
    ///
    /// It is deliberately an **over**-estimate of the id sets and an under-estimate of nothing: the
    /// number it feeds is a trigger threshold, and a model that flattered the file backend would be the
    /// one way this measurement could mislead.
    pub(crate) fn resident_bytes(&self) -> u64 {
        let mut total = 0u64;
        total = total.saturating_add(set_map_bytes(&self.conversations_by_owner_task));
        total = total.saturating_add(set_map_bytes(&self.conversations_by_owner_conversation));
        total = total.saturating_add(set_map_bytes(&self.entries_by_conversation));
        total = total.saturating_add(set_map_bytes(&self.heads_by_conversation));
        total = total.saturating_add(set_map_bytes(&self.tasks_by_conversation));
        total = total.saturating_add(set_map_bytes(&self.tasks_by_status));
        total = total.saturating_add(set_map_bytes(&self.tasks_by_abort_mark));
        total = total.saturating_add(set_map_bytes(&self.tasks_by_background));
        total = total.saturating_add(set_map_bytes(&self.submissions_by_conversation));
        total = total.saturating_add(set_map_bytes(&self.submissions_by_status));
        total = total.saturating_add(set_map_bytes(&self.submissions_by_type));
        total = total.saturating_add(set_map_bytes(&self.documents_by_scope));
        // The three with string-bearing keys, charged their bytes as well.
        for map_exists in [
            !self.tasks_by_kind.is_empty(),
            !self.documents_by_address.is_empty(),
            !self.submissions_by_request.is_empty(),
        ] {
            if map_exists {
                total = total.saturating_add(CONTAINER_OVERHEAD);
            }
        }
        for (kind, ids) in &self.tasks_by_kind {
            total = total.saturating_add(set_bytes(ids));
            total = total.saturating_add(key_bytes::<Kind>(kind.as_str().len()));
        }
        for (address, ids) in &self.documents_by_address {
            total = total.saturating_add(set_bytes(ids));
            let text = address.kind.as_str().len()
                + address.key.as_ref().map_or(0, std::string::String::len);
            total = total.saturating_add(key_bytes::<DocumentAddress>(text));
        }
        for by_request in self.submissions_by_request.values() {
            total = total.saturating_add(CONTAINER_OVERHEAD);
            for request in by_request.keys() {
                total = total.saturating_add(key_bytes::<SubmissionId>(request.len()));
            }
        }
        total
    }
}

/// What the model charges a non-empty `BTreeMap` or `BTreeSet` for existing at all.
const CONTAINER_OVERHEAD: u64 = 64;

/// What the model charges each element on top of its key and value sizes.
///
/// A `BTreeMap` node holds up to 11 pairs plus 12 child pointers and a length, so the true figure is
/// under ten bytes per element at a full node and larger at a sparse one. Sixteen is the pessimistic
/// end of that range, chosen because this number feeds a threshold.
const NODE_OVERHEAD_PER_ELEMENT: u64 = 16;

/// The model's charge for one element whose key carries `text` bytes of separately allocated string.
fn key_bytes<K>(text: usize) -> u64 {
    let key = u64::try_from(size_of::<K>()).unwrap_or(u64::MAX);
    let text = u64::try_from(text).unwrap_or(u64::MAX);
    key.saturating_add(text)
        .saturating_add(NODE_OVERHEAD_PER_ELEMENT)
}

/// The model's charge for one set of ids.
fn set_bytes<T>(ids: &BTreeSet<T>) -> u64 {
    let each = u64::try_from(size_of::<T>()).unwrap_or(u64::MAX);
    let n = u64::try_from(ids.len()).unwrap_or(u64::MAX);
    CONTAINER_OVERHEAD
        .saturating_add(n.saturating_mul(each.saturating_add(NODE_OVERHEAD_PER_ELEMENT)))
}

/// The model's charge for a map of id sets with fixed-size keys.
fn set_map_bytes<K, T>(map: &BTreeMap<K, BTreeSet<T>>) -> u64 {
    let mut total = if map.is_empty() {
        0
    } else {
        CONTAINER_OVERHEAD
    };
    for ids in map.values() {
        total = total.saturating_add(key_bytes::<K>(0));
        total = total.saturating_add(set_bytes(ids));
    }
    total
}

/// Insert `id` under `key`.
fn add<K: Ord, T: Ord>(map: &mut BTreeMap<K, BTreeSet<T>>, key: K, id: T) {
    map.entry(key).or_default().insert(id);
}

/// Remove `id` from `key`'s set, and the set itself once it is empty.
///
/// The emptying matters: without it a long session accumulates one empty set per transitioned task's
/// every past status, and `resident_bytes` would be measuring the history of the index rather than its
/// contents.
fn remove<K: Ord, T: Ord>(map: &mut BTreeMap<K, BTreeSet<T>>, key: &K, id: T) {
    if let Some(ids) = map.get_mut(key) {
        ids.remove(&id);
        if ids.is_empty() {
            map.remove(key);
        }
    }
}

/// The candidate ids one read will visit, in the order it will visit them.
type Candidates<'a, T> = Box<dyn Iterator<Item = &'a T> + 'a>;

/// The empty walk, for a query whose driving index holds nothing.
fn none<'a, T>() -> Candidates<'a, T> {
    Box::new(core::iter::empty())
}

/// The ids of `set` strictly above `after`, ascending.
fn ascending<T>(set: &BTreeSet<T>, after: Option<NonZeroU64>) -> Candidates<'_, T>
where
    T: Ord + std::borrow::Borrow<NonZeroU64>,
{
    Box::new(set.range::<NonZeroU64, _>((exclusive(after), Bound::Unbounded)))
}

/// `after` as an **exclusive** bound, because a cursor resumes strictly past the item it names.
///
/// Which end of the range it bounds depends on the scan's direction, and both uses are here: an
/// ascending scan takes it as the lower bound (`> after`), a newest-first entry scan as the upper one
/// (`< after`). One function for both, so *"strictly"* is decided once.
fn exclusive(after: Option<NonZeroU64>) -> Bound<NonZeroU64> {
    match after {
        Some(at) => Bound::Excluded(at),
        None => Bound::Unbounded,
    }
}

/// The tighter of two upper bounds, with `Excluded` beating `Included` at the same value.
fn tighter(a: Bound<NonZeroU64>, b: Bound<NonZeroU64>) -> Bound<NonZeroU64> {
    match (a, b) {
        (Bound::Unbounded, other) | (other, Bound::Unbounded) => other,
        (Bound::Included(x), Bound::Included(y)) => Bound::Included(x.min(y)),
        (Bound::Excluded(x), Bound::Excluded(y)) => Bound::Excluded(x.min(y)),
        (Bound::Included(i), Bound::Excluded(e)) | (Bound::Excluded(e), Bound::Included(i)) => {
            if e <= i {
                Bound::Excluded(e)
            } else {
                Bound::Included(i)
            }
        }
    }
}

/// An inclusive upper bound from an optional typed id.
fn at_most<K: IdKind>(id: Option<Id<K>>) -> Bound<NonZeroU64> {
    match id {
        Some(id) => Bound::Included(RawId::from(id).get()),
        None => Bound::Unbounded,
    }
}

/// A newest-first walk of one conversation's **visible** entries, bounded before it starts.
///
/// `spec.md:4334-4336`: *"`scanEntries()` pages the inclusive ID range in newest-first order while
/// applying every conversation ancestry cap."* The caps are per level and only tighten going up
/// (`spec.md:258`, `:263`), so one descending range per ancestry level, merged, answers it — and
/// because each range is bounded by the query's own upper bound and by the cursor, a page costs
/// `O(page × depth)` rather than `O(visible history)`.
///
/// Entry ids are session-global and ordered (`spec.md:257`), so the merge's *"largest peek wins"* is
/// the specified order and not an approximation of it.
pub(crate) struct VisibleEntries<'a> {
    levels: Vec<core::iter::Peekable<core::iter::Rev<btree_set::Range<'a, EntryId>>>>,
    /// Index entries visited so far, including the ancestry records the walk was built from.
    visited: u64,
}

impl VisibleEntries<'_> {
    /// How many index entries this walk has visited.
    pub(crate) const fn visited(&self) -> u64 {
        self.visited
    }
}

impl Iterator for VisibleEntries<'_> {
    type Item = EntryId;

    fn next(&mut self) -> Option<EntryId> {
        let mut best: Option<(usize, EntryId)> = None;
        for (at, level) in self.levels.iter_mut().enumerate() {
            if let Some(&&id) = level.peek()
                && best.is_none_or(|(_, so_far)| id > so_far)
            {
                best = Some((at, id));
            }
        }
        let (at, id) = best?;
        self.levels.get_mut(at)?.next();
        self.visited = self.visited.saturating_add(1);
        Some(id)
    }
}

impl Committed {
    /// Which conversations [`ConversationQuery`] selects, ascending by id, and the index that narrowed
    /// them.
    ///
    /// Conjunctive (`spec.md:4337-4338`): with both owner filters set the walk is driven by whichever
    /// index holds fewer candidates and the other is checked on them.
    pub(crate) fn conversation_candidates(
        &self,
        query: &ConversationQuery,
        after: Option<NonZeroU64>,
    ) -> (Index, Candidates<'_, ConversationId>) {
        let named = [
            (
                Index::ConversationsByOwnerTask,
                query
                    .owner_task_id
                    .map(|want| self.indexes.conversations_by_owner_task.get(&want)),
            ),
            (
                Index::ConversationsByOwnerConversation,
                query
                    .owner_conversation_id
                    .map(|want| self.indexes.conversations_by_owner_conversation.get(&want)),
            ),
        ];
        match driver(&named) {
            Driver::Index(index, set) => (index, ascending(set, after)),
            Driver::Empty(index) => (index, none()),
            Driver::Unfiltered => (
                Index::ConversationsUnfiltered,
                Box::new(
                    self.conversations
                        .range::<NonZeroU64, _>((exclusive(after), Bound::Unbounded))
                        .map(|(id, _)| id),
                ),
            ),
        }
    }

    /// Which tasks [`TaskQuery`] selects, ascending by id, and the index that narrowed them.
    ///
    /// All five of `spec.md:4348`'s fields are indexed, so the driving set is the smallest among those
    /// the query named and the rest are residual predicates. The two booleans are indexed by
    /// *partition* rather than by their true half, which is why `abort_requested: Some(false)` narrows
    /// as well as `Some(true)` does.
    pub(crate) fn task_candidates(
        &self,
        query: &TaskQuery,
        after: Option<NonZeroU64>,
    ) -> (Index, Candidates<'_, TaskId>) {
        let ix = &self.indexes;
        let named = [
            (
                Index::TasksByConversation,
                query
                    .conversation_id
                    .map(|want| ix.tasks_by_conversation.get(&want)),
            ),
            (
                Index::TasksByKind,
                query.kind.as_ref().map(|want| ix.tasks_by_kind.get(want)),
            ),
            (
                Index::TasksByStatus,
                query.status.map(|want| ix.tasks_by_status.get(&want)),
            ),
            (
                Index::TasksByAbortMark,
                query
                    .abort_requested
                    .map(|want| ix.tasks_by_abort_mark.get(&want)),
            ),
            (
                Index::TasksByBackground,
                query
                    .background
                    .map(|want| ix.tasks_by_background.get(&want)),
            ),
        ];
        match driver(&named) {
            Driver::Index(index, set) => (index, ascending(set, after)),
            Driver::Empty(index) => (index, none()),
            Driver::Unfiltered => (
                Index::TasksUnfiltered,
                Box::new(
                    self.tasks
                        .range::<NonZeroU64, _>((exclusive(after), Bound::Unbounded))
                        .map(|(id, _)| id),
                ),
            ),
        }
    }

    /// Which submissions [`SubmissionQuery`] selects, ascending by id, and the index that narrowed them.
    pub(crate) fn submission_candidates(
        &self,
        query: &SubmissionQuery,
        after: Option<NonZeroU64>,
    ) -> (Index, Candidates<'_, SubmissionId>) {
        let ix = &self.indexes;
        let named = [
            (
                Index::SubmissionsByConversation,
                query
                    .conversation_id
                    .map(|want| ix.submissions_by_conversation.get(&want)),
            ),
            (
                Index::SubmissionsByStatus,
                query.status.map(|want| ix.submissions_by_status.get(&want)),
            ),
            (
                Index::SubmissionsByType,
                query
                    .submission_type
                    .map(|want| ix.submissions_by_type.get(&want)),
            ),
        ];
        match driver(&named) {
            Driver::Index(index, set) => (index, ascending(set, after)),
            Driver::Empty(index) => (index, none()),
            Driver::Unfiltered => (
                Index::SubmissionsUnfiltered,
                Box::new(
                    self.submissions
                        .range::<NonZeroU64, _>((exclusive(after), Bound::Unbounded))
                        .map(|(id, _)| id),
                ),
            ),
        }
    }

    /// One conversation's submission by the caller's request id (`spec.md:4314`).
    pub(crate) fn submission_by_request(
        &self,
        conversation: ConversationId,
        request: &str,
    ) -> Option<SubmissionId> {
        self.indexes
            .submissions_by_request
            .get(&conversation)?
            .get(request)
            .copied()
    }

    /// The incarnations ever created at one logical address, **newest first**.
    ///
    /// Newest first because `spec.md:4364-4365` makes a retire-plus-create at one address leave the new
    /// incarnation current at that sequence, so the first candidate alive at the point is the answer.
    pub(crate) fn address_candidates(
        &self,
        address: &DocumentAddress,
    ) -> impl Iterator<Item = &DocumentId> {
        self.indexes
            .documents_by_address
            .get(address)
            .into_iter()
            .flat_map(|ids| ids.iter().rev())
    }

    /// Which incarnations [`DocumentQuery`] selects, **ascending** by id (`spec.md:4344`).
    pub(crate) fn document_candidates(
        &self,
        query: &DocumentQuery,
        after: Option<NonZeroU64>,
    ) -> Candidates<'_, DocumentId> {
        match self.indexes.documents_by_scope.get(&query.scope) {
            Some(ids) => ascending(ids, after),
            None => none(),
        }
    }

    /// The newest entry carrying a head in one conversation's visible history, at or before a cutoff.
    ///
    /// One `range(..=bound).next_back()` per ancestry level, then the largest. The ancestry walk is the
    /// only unbounded part and it is bounded by the chain's depth, which
    /// [`MAX_ANCESTRY_DEPTH`](crate::state::MAX_ANCESTRY_DEPTH) caps.
    pub(crate) fn latest_head(
        &self,
        conversation: ConversationId,
        at_or_before: Option<EntryId>,
    ) -> (u64, Option<EntryId>) {
        let cutoff = at_most(at_or_before);
        let mut visited = 0u64;
        let mut best: Option<EntryId> = None;
        for (level, cap) in self.ancestry(conversation) {
            visited = visited.saturating_add(1);
            let Some(heads) = self.indexes.heads_by_conversation.get(&level) else {
                continue;
            };
            let bound = tighter(cutoff, at_most(cap));
            if let Some(&found) = heads
                .range::<NonZeroU64, _>((Bound::Unbounded, bound))
                .next_back()
            {
                visited = visited.saturating_add(1);
                if best.is_none_or(|so_far| found > so_far) {
                    best = Some(found);
                }
            }
        }
        (visited, best)
    }

    /// A newest-first, bounded walk of one conversation's visible entries.
    ///
    /// `after` is a cursor position, so the walk resumes strictly below it.
    pub(crate) fn visible_entries(
        &self,
        query: &EntryQuery,
        after: Option<NonZeroU64>,
    ) -> VisibleEntries<'_> {
        let ceiling = tighter(at_most(query.max_entry_id), exclusive(after));
        let floor = match query.min_entry_id {
            Some(min) => Bound::Included(RawId::from(min).get()),
            None => Bound::Unbounded,
        };
        let chain = self.ancestry(query.conversation_id);
        let visited = u64::try_from(chain.len()).unwrap_or(u64::MAX);
        let levels = chain
            .into_iter()
            .filter_map(|(level, cap)| {
                let entries = self.indexes.entries_by_conversation.get(&level)?;
                let bound = tighter(ceiling, at_most(cap));
                Some(
                    entries
                        .range::<NonZeroU64, _>((floor, bound))
                        .rev()
                        .peekable(),
                )
            })
            .collect();
        VisibleEntries { levels, visited }
    }

    /// Whether an entry is visible through one conversation's ancestry, and how many levels that took.
    ///
    /// `spec.md:4348`: `entry(conversationId, id)` *"returns that pair only when the entry is visible
    /// through the requested conversation's ancestry"*. `belongs_to` is the conversation the entry's own
    /// record names, so the question is whether that conversation is a level of `through`'s chain whose
    /// cap still admits the entry.
    pub(crate) fn entry_is_visible(
        &self,
        through: ConversationId,
        id: EntryId,
        belongs_to: ConversationId,
    ) -> (u64, bool) {
        let mut visited = 0u64;
        for (level, cap) in self.ancestry(through) {
            visited = visited.saturating_add(1);
            if belongs_to == level && cap.is_none_or(|cap| id <= cap) {
                return (visited, true);
            }
        }
        (visited, false)
    }
}

/// Which index will drive a conjunctive read.
///
/// The three cases are genuinely different answers and collapsing any two of them is a bug with a name.
/// *"The query named nothing"* is the whole table, and `spec.md:4337-4338` is explicit that an absent
/// filter *"does not constrain"* rather than matching nothing. *"The query named something whose index
/// is absent"* is the empty answer — nothing owns that id, nothing is in that state — and answering it
/// by walking the table would be the scan this module exists to rule out. Only the third case has a set
/// to walk.
enum Driver<'a, T> {
    /// Walk this index's candidates.
    Index(Index, &'a BTreeSet<T>),
    /// The answer is empty; this is the index that said so.
    Empty(Index),
    /// No filter was named, so the table is the answer.
    Unfiltered,
}

/// Choose the driving index: the smallest set among the filters the query named.
///
/// The selectivity comes from the sets' own `len`, so the choice is made from the data rather than from
/// an ordering baked into the code — which matters because the right driver depends on the session: a
/// conversation with one task and a status shared by four hundred tasks is the opposite of the case a
/// hand-written ordering would be tuned for.
///
/// `named` pairs each filterable field's index with `None` when the query did not name it, `Some(None)`
/// when it named a value no index entry exists for, and `Some(Some(set))` otherwise.
fn driver<'a, T>(named: &[(Index, Option<Option<&'a BTreeSet<T>>>)]) -> Driver<'a, T> {
    // A named filter with no index entry means no record matches it, whatever the other filters select:
    // the conjunction is empty. Checked first, because it decides the answer on its own.
    if let Some((index, _)) = named.iter().find(|(_, found)| matches!(found, Some(None))) {
        return Driver::Empty(*index);
    }
    match named
        .iter()
        .filter_map(|(index, found)| found.flatten().map(|set| (*index, set)))
        .min_by_key(|(_, set)| set.len())
    {
        Some((index, set)) => Driver::Index(index, set),
        None => Driver::Unfiltered,
    }
}

/// What one open store costs in memory, split the way ADR-0030 §9's trigger needs it (open question 2).
///
/// # Why this is two numbers and not one
///
/// ADR-0030 §9 states the trigger as *"64 MB resident index"* and backs the figure with a model: *"the
/// indexes are id→offset maps plus small key tuples, tens of bytes per record, so 100 k records is a few
/// megabytes."* That model is right about the **indexes** and silent about the rest of what a file
/// backend holds open: `spec.md:4325-4351`'s access paths are answered from memory here, so every
/// conversation, entry, task and submission **record** is resident too, with its payload. For a
/// transcript whose entries carry model messages that is the larger number by an order of magnitude, and
/// a measurement that reported only the index half would make the file backend look cheaper than it is
/// at exactly the moment the threshold was being decided.
///
/// So both are reported. [`Footprint::index_bytes`] is the quantity ADR-0030 §9's model predicts, and
/// [`Footprint::record_bytes`] is the one it omits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Footprint {
    /// The secondary indexes, the id-ownership map, and each document's content layout.
    ///
    /// This is ADR-0030 §9's *"id→offset maps plus small key tuples"*.
    pub index_bytes: u64,
    /// The resident record tables, each record charged its own size plus its serialized payload.
    pub record_bytes: u64,
    /// How many commits the store holds, which is one half of *"open is O(commits + table records)"*.
    pub commits: u64,
    /// How many records the four tables and the document index hold, which is the other half.
    pub records: u64,
}

impl Footprint {
    /// Both halves together: what the process actually holds for this store.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.index_bytes.saturating_add(self.record_bytes)
    }
}

impl Committed {
    /// Measure this state, for ADR-0030 §14 open question 2.
    ///
    /// `O(records)` and it re-serialises each record to charge its payload, so it is a measurement
    /// surface rather than something a read path calls. See [`Footprint`] for what each half is and why
    /// there are two.
    pub(crate) fn footprint(&self) -> Footprint {
        let mut index_bytes = self.indexes.resident_bytes();
        index_bytes =
            index_bytes.saturating_add(map_bytes::<NonZeroU64, IdKindTag>(self.owners.len()));
        for state in self.documents.values() {
            index_bytes = index_bytes
                .saturating_add(set_like_bytes::<crate::wire::Slice>(state.slices.len()));
        }
        let mut record_bytes = table_bytes(self.conversations.values());
        record_bytes = record_bytes.saturating_add(table_bytes(self.entries.values()));
        record_bytes = record_bytes.saturating_add(table_bytes(self.tasks.values()));
        record_bytes = record_bytes.saturating_add(table_bytes(self.submissions.values()));
        record_bytes =
            record_bytes.saturating_add(table_bytes(self.documents.values().map(|s| &s.record)));
        let records = [
            self.conversations.len(),
            self.entries.len(),
            self.tasks.len(),
            self.submissions.len(),
            self.documents.len(),
        ]
        .into_iter()
        .fold(0u64, |total, n| {
            total.saturating_add(u64::try_from(n).unwrap_or(u64::MAX))
        });
        Footprint {
            index_bytes,
            record_bytes,
            commits: self.commits,
            records,
        }
    }
}

/// The model's charge for one id-keyed table of records, each charged its serialized payload.
///
/// A record that will not serialise is charged its struct size alone. Nothing in a committed state can
/// fail to serialise — every one of these records was decoded from a line this backend wrote — so the
/// arm exists only because a measurement must not be the thing that fails.
fn table_bytes<'a, R: serde::Serialize + 'a>(records: impl Iterator<Item = &'a R>) -> u64 {
    let each = u64::try_from(size_of::<R>()).unwrap_or(u64::MAX);
    records.fold(0u64, |total, record| {
        let payload = serde_json::to_vec(record)
            .map_or(0, |bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        total
            .saturating_add(each)
            .saturating_add(payload)
            .saturating_add(NODE_OVERHEAD_PER_ELEMENT)
    })
}

/// The model's charge for a flat map of `n` fixed-size key-value pairs.
fn map_bytes<K, V>(n: usize) -> u64 {
    let each = u64::try_from(size_of::<K>().saturating_add(size_of::<V>())).unwrap_or(u64::MAX);
    let n = u64::try_from(n).unwrap_or(u64::MAX);
    CONTAINER_OVERHEAD
        .saturating_add(n.saturating_mul(each.saturating_add(NODE_OVERHEAD_PER_ELEMENT)))
}

/// The model's charge for a `Vec` of `n` fixed-size elements.
fn set_like_bytes<T>(n: usize) -> u64 {
    let each = u64::try_from(size_of::<T>()).unwrap_or(u64::MAX);
    let n = u64::try_from(n).unwrap_or(u64::MAX);
    n.saturating_mul(each)
}

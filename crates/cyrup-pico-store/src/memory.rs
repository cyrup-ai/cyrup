//! [`MemoryStore`] — §11.1's normative reference semantics (`spec.md:4373-4379`).
//!
//! > Memory storage is the reference semantics. It copies retained write values and all read
//! > results. This deliberately simulates the ownership boundary naturally created by SQLite
//! > encoding/decoding and JSONL serialization; it is **not** defensive validation. It preserves
//! > rewindable records and reclaims latest records only after a committed base or retirement.
//!
//! # It is simpler than pi's, and the reason is a type
//!
//! ADR-0030 F4, and the plan says it outright: this backend is *"simpler than pi's, because
//! `memory.ts:92-100`'s recursive clone has no Rust equivalent"*. That function's own comment at
//! `memory.ts:216` admits what it is for — *"to match the ownership boundary"* that SQLite and JSONL
//! get free from encode/decode. Here [`DocRoot`] and [`DocValue`] have no interior mutability and
//! share structurally through `Arc`, so a stored value **cannot** be reached mutably by anyone,
//! including this module. Detachment is therefore a property of the types rather than a pass over the
//! data, and the `CYRUP-DELTA` is that the recursive clone is deleted rather than ported: same
//! guarantee (`spec.md:4375-4378`), mechanism removed.
//!
//! # What is left for this backend to check
//!
//! Exactly the cross-batch facts ADR-0030 F3 says the keyed [`Batch`] does **not** lift:
//!
//! * **global id ownership** against already-committed records — *"one number is owned by one record
//!   of one type"* (`spec.md:4274-4275`), derived from the per-table indexes rather than from a
//!   separate durable number-to-kind table, which is F6 §A's explicit recommendation;
//! * **address occupancy** — *"one normalized batch contains at most one create/change content
//!   command per incarnation"* is in-batch and lifted; *"the address already has a live
//!   incarnation"* is about committed state and is not;
//! * **immutable conversation and entry creation** (`spec.md:4274`);
//! * **a stale version witness**: a [`DocumentContent::Delta`] whose [`StoredVersion`] was read
//!   before another commit moved the document's stored version.
//!
//! Every one of them is a closed [`RejectedReason`], returned **before any mutation**: this type
//! validates the whole batch, then applies it, so its `Rejected` claim — *"nothing durable
//! happened"* — is true by construction rather than by care.
//!
//! # What this backend does not pretend to be
//!
//! Fast. `spec.md:4325-4339`'s indexed access paths are an obligation on a *durable* backend. The only
//! index kept here is entries by conversation, because entry scans are ancestry-capped and
//! newest-first and that ordering has to be right; everything else is an ordered scan of a
//! `BTreeMap`, chosen so the semantics are *obviously* correct, which is what a reference
//! implementation is for.
//!
//! PICO5-PLAN S10 landed that obligation where it bites — `cyrup-pico-store-jsonl`'s secondary indexes,
//! with the query-plan suite that proves each named path is answered from one and does not grow with
//! total history — and deliberately left this backend alone. The reason is the division this module
//! exists for: a reference implementation's job is to make the *semantics* checkable by reading it, and
//! the conformance suite judges both backends by answers rather than by cost, so an indexed
//! `MemoryStore` would buy nothing the suite can see while making the semantics harder to read. S12's
//! SQLite backend owes the indexed half; this one does not.
//!
//! [`DocValue`]: cyrup_pico_doc::DocValue
//! [`Batch`]: crate::Batch
//! [`DocumentContent::Delta`]: crate::DocumentContent::Delta

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::num::{NonZeroU64, NonZeroU128};
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use cyrup_pico_doc::{DefVersion, DocRoot, ReplayPlan, StoredContent, StoredVersion, materialize};

use crate::batch::{Batch, CopySource, DocumentCommand, DocumentContent};
use crate::cursor::{
    ConversationCursor, CursorBytes, DocumentCursor, EntryCursor, SubmissionCursor, TaskCursor,
    WrongStore,
};
use crate::error::{CommitError, CopySourceMismatch, Corruption, RejectedReason, StorageFailure};
use crate::id::{Id, IdKind, IdKindTag, RawId};
use crate::query::{
    CommittedEntry, ConversationQuery, DocumentQuery, EntryQuery, Page, StoredDocument,
    SubmissionQuery, TaskQuery,
};
use crate::records::{
    ConversationRecord, DocumentAddress, DocumentCreate, DocumentRecord, DocumentScope,
    EntryRecord, HeadMarker, SubmissionRecord, TaskRecord,
};
use crate::storage::Storage;
use crate::{
    ConversationId, Cx, DocumentId, DocumentPoint, EntryId, Lifetime, PageLimit, Seq, StoreId,
    SubmissionId, TaskId,
};

/// How deep a conversation ancestry chain may be before this backend stops walking it.
///
/// A cycle in `parent` is not reachable from any commit — a fork's parent is always an existing
/// conversation, so the graph a Session builds is acyclic — but it **is** reachable from a damaged
/// store, and `#![forbid(unsafe_code)]` does not stop a stack overflow. The walk is iterative and
/// also carries a visited set; this bound is the second line of defence and a documented envelope.
const MAX_ANCESTRY_DEPTH: usize = 4_096;

/// Distinguishes two [`MemoryStore`]s so a cursor from one cannot be resumed in the other.
static NEXT_STORE_ID: AtomicU64 = AtomicU64::new(1);

/// One document incarnation's committed state.
struct DocumentState {
    /// The persisted record.
    record: DocumentRecord,
    /// The definition version of the newest applicable base.
    ///
    /// Tracked rather than recomputed so that validating a [`DocumentContent::Delta`]'s witness never
    /// has to replay a tail — a replay inside the commit path would be work that can fail, on the path
    /// whose failure class is hardest to get right.
    version: DefVersion,
    /// Content records in commit order, newest last.
    content: Vec<(Seq, StoredContent)>,
}

/// The reference backend.
///
/// Holds everything in memory, has no durability envelope at all, and is **not** a placeholder:
/// ADR-0030 §9 says *"memory and files are not placeholders. Memory is §11.1's normative reference
/// semantics and the substrate for the whole conformance suite."*
pub struct MemoryStore {
    store_id: StoreId,
    next_seq: u64,
    last_seq: Option<Seq>,
    next_id: u64,
    conversations: BTreeMap<ConversationId, ConversationRecord>,
    entries: BTreeMap<EntryId, (EntryRecord, Seq)>,
    entries_by_conversation: BTreeMap<ConversationId, BTreeSet<EntryId>>,
    tasks: BTreeMap<TaskId, TaskRecord>,
    submissions: BTreeMap<SubmissionId, SubmissionRecord>,
    documents: BTreeMap<DocumentId, DocumentState>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    /// An empty store, with an identity no other store shares.
    #[must_use]
    pub fn new() -> Self {
        let n = NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            // The counter starts at 1 and only increases, so the `unwrap_or` arm is unreachable; it
            // is written rather than asserted because an `expect` here would be a panic on a path
            // that cannot be taken, and `MIN` is as good a store identity as any other number.
            store_id: StoreId::new(NonZeroU128::new(u128::from(n)).unwrap_or(NonZeroU128::MIN)),
            next_seq: 1,
            last_seq: None,
            // **2, not 1.** `spec.md:249` fixes `ROOT_CONVERSATION_ID` at 1 and ADR-0030 F6 §A calls it
            // *"the one id the specification fixes rather than mints"* — so 1 is already owned before
            // this store has committed anything, and a namespace that handed it out would collide with
            // the root conversation on its first allocation. The first mintable number is 2.
            next_id: 2,
            conversations: BTreeMap::new(),
            entries: BTreeMap::new(),
            entries_by_conversation: BTreeMap::new(),
            tasks: BTreeMap::new(),
            submissions: BTreeMap::new(),
            documents: BTreeMap::new(),
        }
    }

    /// The sequence of the newest commit, if there is one.
    #[must_use]
    pub const fn last_commit(&self) -> Option<Seq> {
        self.last_seq
    }

    /// Which kind of record owns `raw`, if any.
    ///
    /// ADR-0030 F6 §A: *"do **not** build a separate durable number-to-kind table; derive ownership
    /// from the per-table indexes, which exist anyway."* This is that derivation — five keyed probes,
    /// no second table to keep consistent, and nothing to get out of step with the records themselves.
    fn owner_of(&self, raw: NonZeroU64) -> Option<IdKindTag> {
        if self.conversations.contains_key(&Id::new(raw)) {
            return Some(IdKindTag::Conversation);
        }
        if self.entries.contains_key(&Id::new(raw)) {
            return Some(IdKindTag::Entry);
        }
        if self.tasks.contains_key(&Id::new(raw)) {
            return Some(IdKindTag::Task);
        }
        if self.submissions.contains_key(&Id::new(raw)) {
            return Some(IdKindTag::Submission);
        }
        if self.documents.contains_key(&Id::new(raw)) {
            return Some(IdKindTag::Document);
        }
        None
    }

    /// Reject a number this batch claims for `kind` when another kind of record already owns it.
    fn check_foreign_owner<K: IdKind>(
        &self,
        id: Id<K>,
    ) -> Result<Option<IdKindTag>, RejectedReason> {
        let owner = self.owner_of(RawId::from(id).get());
        match owner {
            Some(owner) if owner != K::KIND => Err(RejectedReason::IdAlreadyOwned {
                id: RawId::from(id),
                owner,
            }),
            other => Ok(other),
        }
    }

    /// The conversation ancestry of `id`, newest first, each with the inclusive entry cap that applies
    /// at that level.
    ///
    /// `spec.md:258` (*"`parent.at` is an entry in the parent history visible to the child"*) and
    /// `:263` (*"Fork traversal is child entries followed by parent entries through each `parent.at`
    /// cap"*). Caps only tighten going up, which is why each level's cap is the minimum of its own and
    /// everything below it.
    fn ancestry(&self, id: ConversationId) -> Vec<(ConversationId, Option<EntryId>)> {
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut current = Some((id, None));
        while let Some((conversation, cap)) = current {
            if !seen.insert(conversation) || chain.len() >= MAX_ANCESTRY_DEPTH {
                break;
            }
            chain.push((conversation, cap));
            current = self
                .conversations
                .get(&conversation)
                .and_then(|record| record.parent)
                .map(|parent| {
                    let next = cap.map_or(parent.at, |cap: EntryId| cap.min(parent.at));
                    (parent.conversation_id, Some(next))
                });
        }
        chain
    }

    /// Every entry visible through `conversation`'s ancestry, **newest first** by id.
    fn visible_entries(&self, conversation: ConversationId) -> Vec<EntryId> {
        let mut ids = Vec::new();
        for (level, cap) in self.ancestry(conversation) {
            if let Some(level_ids) = self.entries_by_conversation.get(&level) {
                ids.extend(
                    level_ids
                        .iter()
                        .copied()
                        .filter(|entry| cap.is_none_or(|cap| *entry <= cap)),
                );
            }
        }
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids
    }

    /// Materialize one incarnation's content, considering only records at or before `upto`.
    fn replay(
        &self,
        state: &DocumentState,
        upto: Option<Seq>,
    ) -> Result<(StoredVersion, DocRoot, u32), StorageFailure> {
        let records: Vec<StoredContent> = state
            .content
            .iter()
            .filter(|(at, _)| upto.is_none_or(|upto| *at <= upto))
            .map(|(_, content)| content.clone())
            .collect();
        let plan = ReplayPlan::parse(&records).map_err(|source| {
            StorageFailure::Corrupt(Corruption::Replay {
                document: state.record.id,
                source,
            })
        })?;
        Ok((plan.version(), materialize(&plan), plan.deltas_since_base()))
    }

    /// The committed pre-batch value of a copy source at the selected point, if it can be read.
    ///
    /// `None` means *"not a usable source"* and the caller turns it into
    /// [`RejectedReason::CopySourceNotAlive`], which is honest in both arms: the incarnation is not
    /// alive at that point, or its history is not retained there. A corrupt replay is folded into the
    /// same answer, and that is sound **here specifically** — every content record in this store was
    /// appended by this type, a base first and then deltas at that base's version, so
    /// `ReplayPlan::parse` has no reachable failure. Folding it keeps the two-class contract exact
    /// (nothing durable happened) instead of inventing a rejection reason for a state this backend
    /// cannot reach.
    fn copy_source_value(
        &self,
        state: &DocumentState,
        at: DocumentPoint,
    ) -> Option<DocumentBaseValue> {
        let upto = match at {
            DocumentPoint::Current => {
                if !state.record.lifetime.is_open() {
                    return None;
                }
                None
            }
            DocumentPoint::At(seq) => {
                if !state.record.retains_history() || !state.record.lifetime.contains(seq) {
                    return None;
                }
                Some(seq)
            }
        };
        self.replay(state, upto)
            .ok()
            .map(|(version, value, _)| DocumentBaseValue {
                version: version.version(),
                value,
            })
    }

    /// Validate the whole batch against committed state, then report what the apply pass needs.
    fn validate(&self, batch: &Batch) -> Result<(), RejectedReason> {
        // One number, one kind — including within this batch, where two kinds of id for one number
        // cannot arise from honest minting but can arise from a decode boundary.
        let mut claimed: BTreeMap<NonZeroU64, IdKindTag> = BTreeMap::new();
        let mut claim = |raw: RawId, kind: IdKindTag| -> Result<(), RejectedReason> {
            match claimed.insert(raw.get(), kind) {
                Some(previous) if previous != kind => Err(RejectedReason::IdAlreadyOwned {
                    id: raw,
                    owner: previous,
                }),
                _ => Ok(()),
            }
        };

        for id in batch.conversations().keys() {
            claim(RawId::from(*id), IdKindTag::Conversation)?;
            if self.check_foreign_owner(*id)?.is_some() {
                // `spec.md:4274`: conversation creation is immutable.
                return Err(RejectedReason::ConversationExists(*id));
            }
        }
        for id in batch.entries().keys() {
            claim(RawId::from(*id), IdKindTag::Entry)?;
            if self.check_foreign_owner(*id)?.is_some() {
                // `spec.md:69`, invariant 5: entries are immutable.
                return Err(RejectedReason::EntryExists(*id));
            }
        }
        // Tasks and submissions are replaced as they advance (`spec.md:4383`), so an id already
        // owned by the same table is expected rather than rejected.
        for id in batch.tasks().keys() {
            claim(RawId::from(*id), IdKindTag::Task)?;
            self.check_foreign_owner(*id)?;
        }
        for id in batch.submissions().keys() {
            claim(RawId::from(*id), IdKindTag::Submission)?;
            self.check_foreign_owner(*id)?;
        }

        // One logical address holds at most one LIVE incarnation, and two creations in ONE batch are
        // the half the keyed batch cannot see: the map is keyed by incarnation, and two new
        // incarnations at one address have two different keys. `spec.md:4362-4366` is about the
        // address, not the key, so this check is the backend's as much as the cross-batch one below.
        // Only creations this batch leaves live are registered; see the `retires()` test below.
        let mut staged_addresses: BTreeMap<DocumentAddress, DocumentId> = BTreeMap::new();

        for (id, command) in batch.documents() {
            claim(RawId::from(*id), IdKindTag::Document)?;
            let existing = self.check_foreign_owner(*id)?;
            match command {
                DocumentCommand::Create { record, .. } | DocumentCommand::Copy { record, .. } => {
                    if existing.is_some() {
                        return Err(RejectedReason::DocumentExists(*id));
                    }
                    let address = record.address();
                    // A creation this same batch retires has an EMPTY half-open lifetime
                    // (`spec.md:1113`: *"a creation retired in the same commit has an empty
                    // one"*), so it never occupies the address and cannot be the occupant of
                    // anything. Registering it would reject `spec.md:1245-1246`'s *"a later
                    // `tx.doc()` at that address in the same transaction creates a new
                    // incarnation with a new draft and ID"* — which is the same exemption
                    // [`MemoryStore::check_address_free`] already makes for a *committed*
                    // occupant this batch retires, so the two halves now agree.
                    if !command.retires()
                        && let Some(occupant) = staged_addresses.insert(address.clone(), *id)
                    {
                        return Err(RejectedReason::AddressOccupied { address, occupant });
                    }
                    self.check_address_free(&address, batch)?;
                }
                DocumentCommand::Change { content, .. } => {
                    let state = self
                        .documents
                        .get(id)
                        .ok_or(RejectedReason::UnknownDocument(*id))?;
                    self.check_not_retired(state)?;
                    if let DocumentContent::Delta { continues, .. } = content
                        && continues.version() != state.version
                    {
                        return Err(RejectedReason::VersionTransitionRequiresBase {
                            document: *id,
                            stored: state.version,
                            continues: continues.version(),
                        });
                    }
                }
                DocumentCommand::RetireOnly => {
                    let state = self
                        .documents
                        .get(id)
                        .ok_or(RejectedReason::UnknownDocument(*id))?;
                    self.check_not_retired(state)?;
                }
            }
            if let DocumentCommand::Copy { record, source, .. } = command {
                self.check_copy_source(record, *source, batch)?;
            }
        }
        Ok(())
    }

    /// `spec.md:4362-4366`: at most one live incarnation per logical address, unless this batch
    /// retires the occupant.
    fn check_address_free(
        &self,
        address: &DocumentAddress,
        batch: &Batch,
    ) -> Result<(), RejectedReason> {
        for (id, state) in &self.documents {
            if state.record.lifetime.is_open() && state.record.address() == *address {
                let retired_here = batch
                    .documents()
                    .get(id)
                    .is_some_and(DocumentCommand::retires);
                if !retired_here {
                    return Err(RejectedReason::AddressOccupied {
                        address: address.clone(),
                        occupant: *id,
                    });
                }
            }
        }
        Ok(())
    }

    /// A write to an already-retired incarnation is rejected rather than reviving it.
    fn check_not_retired(&self, state: &DocumentState) -> Result<(), RejectedReason> {
        match state.record.lifetime.retired_at() {
            Some(retired_at) => Err(RejectedReason::DocumentRetired {
                document: state.record.id,
                retired_at,
            }),
            None => Ok(()),
        }
    }

    /// `spec.md:4313-4318`: the source must be an alive conversation document at the selected point,
    /// must agree with the child record on kind, key, history and fork, and must not be touched by
    /// this batch.
    fn check_copy_source(
        &self,
        child: &DocumentCreate,
        source: CopySource,
        batch: &Batch,
    ) -> Result<(), RejectedReason> {
        if batch.documents().contains_key(&source.id) {
            return Err(RejectedReason::CopySourceInBatch {
                document: source.id,
            });
        }
        let state = self
            .documents
            .get(&source.id)
            .ok_or(RejectedReason::CopySourceNotAlive {
                document: source.id,
            })?;
        let mismatch = |what: CopySourceMismatch| RejectedReason::CopySourceMismatch {
            document: source.id,
            what,
        };
        if state.record.kind != child.kind {
            return Err(mismatch(CopySourceMismatch::Kind));
        }
        if state.record.key != child.key {
            return Err(mismatch(CopySourceMismatch::Key));
        }
        let (
            DocumentScope::Conversation {
                semantics: source_semantics,
                ..
            },
            DocumentScope::Conversation {
                semantics: child_semantics,
                ..
            },
        ) = (&state.record.scope, &child.scope)
        else {
            return Err(mismatch(CopySourceMismatch::Scope));
        };
        if source_semantics.retains_history() != child_semantics.retains_history() {
            return Err(mismatch(CopySourceMismatch::History));
        }
        if source_semantics.fork() != child_semantics.fork() {
            return Err(mismatch(CopySourceMismatch::Fork));
        }
        if self.copy_source_value(state, source.at).is_none() {
            return Err(RejectedReason::CopySourceNotAlive {
                document: source.id,
            });
        }
        Ok(())
    }

    /// Allocate this commit's sequence.
    fn allocate_seq(&mut self) -> Result<Seq, RejectedReason> {
        let n = NonZeroU64::new(self.next_seq).ok_or(RejectedReason::SequenceExhausted)?;
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .ok_or(RejectedReason::SequenceExhausted)?;
        let seq = Seq::new(n);
        self.last_seq = Some(seq);
        Ok(seq)
    }

    /// Apply a validated batch at `seq`.
    ///
    /// Infallible, and that is the point: every way this could fail was decided by
    /// [`MemoryStore::validate`] before anything was written, so there is no partially-applied state to
    /// unwind and no path on which this backend has to say *"uncertain"*.
    fn apply(&mut self, batch: Batch, seq: Seq) {
        let parts = batch.into_parts();
        for (id, record) in parts.conversations {
            self.conversations.insert(id, record);
        }
        for (id, record) in parts.entries {
            self.entries_by_conversation
                .entry(record.conversation_id)
                .or_default()
                .insert(id);
            self.entries.insert(id, (record, seq));
        }
        for (id, record) in parts.tasks {
            self.tasks.insert(id, record);
        }
        for (id, record) in parts.submissions {
            self.submissions.insert(id, record);
        }
        for (id, command) in parts.documents {
            self.apply_document(id, command, seq);
        }
    }

    /// Apply one document command.
    ///
    /// **Content before retirement, structurally.** The command carries both as one value, so this
    /// function writes the content and then stamps the bound; there is no second command whose
    /// position could change the outcome, which is why `spec.md:4363`'s ordering rule has nothing left
    /// to impose (ADR-0030 F3).
    fn apply_document(&mut self, id: DocumentId, command: DocumentCommand, seq: Seq) {
        match command {
            DocumentCommand::Create { record, base, then } => {
                let stored = base.to_stored();
                self.documents.insert(
                    id,
                    DocumentState {
                        record: record.stamp(seq, then.is_retire()),
                        version: base.version,
                        content: vec![(seq, stored)],
                    },
                );
            }
            DocumentCommand::Copy {
                record,
                source,
                then,
            } => {
                // `spec.md:4317`: *"Storage persists one independent complete child base at the
                // source's stored version."* A complete base, never a reference, which is what makes
                // later source changes, reclamation, retirement and reopen unable to affect the child.
                let copied = self
                    .documents
                    .get(&source.id)
                    .and_then(|state| self.copy_source_value(state, source.at));
                if let Some(DocumentBaseValue { version, value }) = copied {
                    self.documents.insert(
                        id,
                        DocumentState {
                            record: record.stamp(seq, then.is_retire()),
                            version,
                            content: vec![(seq, StoredContent::Base { version, value })],
                        },
                    );
                }
            }
            DocumentCommand::Change { content, then } => {
                if let Some(state) = self.documents.get_mut(&id) {
                    state.content.push((seq, content.to_stored()));
                    if content.is_base() {
                        state.version = content.version();
                    }
                    if then.is_retire() {
                        state.record.lifetime = retire_at(state.record.lifetime, seq);
                    }
                }
                self.reclaim(id);
            }
            DocumentCommand::RetireOnly => {
                if let Some(state) = self.documents.get_mut(&id) {
                    state.record.lifetime = retire_at(state.record.lifetime, seq);
                }
                self.reclaim(id);
            }
        }
    }

    /// §11.1's reclamation: *"preserves rewindable records and reclaims latest records only after a
    /// committed base or retirement."*
    ///
    /// The rule here is the smallest one that honours it: for an incarnation whose **persisted** policy
    /// does not retain history, drop every content record strictly older than the newest base. The
    /// authorising write is that base. Final content always survives, because `spec.md:1242-1244` makes
    /// persisting it the whole point of retiring with content.
    ///
    /// Note what gates this: [`DocumentRecord::retains_history`], read from the record — ADR-0030 §2.3
    /// calls that *"the only thing between `rewindable` being a promise and a best effort"*. A numeric
    /// read of a current-only incarnation is refused by policy
    /// ([`StorageFailure::HistoryNotRetained`]) and never by what happens to have survived.
    fn reclaim(&mut self, id: DocumentId) {
        let Some(state) = self.documents.get_mut(&id) else {
            return;
        };
        if state.record.retains_history() {
            return;
        }
        let newest_base = state
            .content
            .iter()
            .rposition(|(_, content)| content.is_base());
        if let Some(at) = newest_base
            && at > 0
        {
            state.content.drain(..at);
        }
    }

    /// Decode a cursor this store issued.
    fn resume_from(&self, payload: &[u8]) -> Result<Option<NonZeroU64>, StorageFailure> {
        let bytes: [u8; 8] = payload.try_into().map_err(|_| {
            StorageFailure::Corrupt(Corruption::RecordMalformed {
                what: "cursor",
                detail: format!("expected an 8-byte scan position, found {}", payload.len()),
            })
        })?;
        Ok(NonZeroU64::new(u64::from_be_bytes(bytes)))
    }

    /// Stamp a scan position for this store.
    fn cursor_at(&self, id: NonZeroU64) -> CursorBytes {
        CursorBytes::new(self.store_id, id.get().to_be_bytes().to_vec())
    }
}

/// A copy source's value at the selected point.
struct DocumentBaseValue {
    version: DefVersion,
    value: DocRoot,
}

/// Retire a lifetime at `at`, keeping the existing bound if one is somehow already there.
///
/// [`Lifetime::retire`] is fallible because a retirement before the creation is inverted, and that is
/// unreachable from a commit: `at` is the sequence this commit allocated, and every sequence this
/// store has ever issued is smaller. The unreachable arm keeps the lifetime unchanged rather than
/// panicking, because a reference backend that aborts the process on an impossible input is a worse
/// reference than one that refuses to corrupt a record.
fn retire_at(lifetime: Lifetime, at: Seq) -> Lifetime {
    lifetime.retire(at).unwrap_or(lifetime)
}

/// Apply a limit to one already-ordered, already-filtered scan.
fn page<T, C>(items: Vec<T>, limit: PageLimit, cursor: impl Fn(&T) -> C) -> Page<T, C> {
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
/// ADR-0030 §10 fixes [`StorageFailure`] at three arms, and this is none of them: the store is not
/// corrupt, nothing is absent, and no history was reclaimed. `spec.md:4320-4321` calls cross-storage
/// cursor use *"unsupported"*, so it is reported as `io::ErrorKind::InvalidInput` — the standard name
/// for an argument that cannot be honoured — rather than by widening the enum or, worse, by resuming
/// the scan at a position meaning something else.
fn wrong_store(e: WrongStore) -> StorageFailure {
    StorageFailure::Io(io::Error::new(io::ErrorKind::InvalidInput, e))
}

#[async_trait]
impl Storage for MemoryStore {
    fn store_id(&self) -> StoreId {
        self.store_id
    }

    async fn commit(&mut self, batch: Batch, _cx: &Cx) -> Result<Seq, CommitError> {
        self.validate(&batch)?;
        let seq = self.allocate_seq()?;
        self.apply(batch, seq);
        Ok(seq)
    }

    /// `kind` is deliberately unused. See [`Storage::mint_raw`]: a minted number is not owned by
    /// anything until a committed record carries it, so this backend records nothing here and derives
    /// ownership from its per-table indexes at commit time ([`MemoryStore::owner_of`]) — which is
    /// ADR-0030 F6 §A's explicit recommendation rather than a shortcut.
    async fn mint_raw(&mut self, _kind: IdKindTag, _cx: &Cx) -> Result<RawId, CommitError> {
        let n = NonZeroU64::new(self.next_id).ok_or(RejectedReason::IdSpaceExhausted)?;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(RejectedReason::IdSpaceExhausted)?;
        Ok(RawId::new(n))
    }

    async fn close(&mut self, _cx: &Cx) -> Result<(), StorageFailure> {
        Ok(())
    }

    async fn conversation(
        &self,
        id: ConversationId,
        _cx: &Cx,
    ) -> Result<Option<ConversationRecord>, StorageFailure> {
        Ok(self.conversations.get(&id).cloned())
    }

    async fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: PageLimit,
        from: Option<ConversationCursor>,
        _cx: &Cx,
    ) -> Result<Page<ConversationRecord, ConversationCursor>, StorageFailure> {
        let resume = match &from {
            Some(cursor) => {
                self.resume_from(cursor.payload_for(self.store_id).map_err(wrong_store)?)?
            }
            None => None,
        };
        let items: Vec<ConversationRecord> = self
            .conversations
            .iter()
            .filter(|(id, _)| resume.is_none_or(|resume| RawId::from(**id).get() > resume))
            .filter(|(_, record)| {
                // Conjunctive: an absent filter does not constrain (`spec.md:4337-4338`).
                query
                    .owner_conversation_id
                    .is_none_or(|want| record.owner.is_some_and(|o| o.conversation_id == want))
                    && query
                        .owner_task_id
                        .is_none_or(|want| record.owner.is_some_and(|o| o.task_id == want))
            })
            .map(|(_, record)| record.clone())
            .take(saturating_page(limit))
            .collect();
        Ok(page(items, limit, |record| {
            ConversationCursor::new(self.cursor_at(RawId::from(record.id).get()))
        }))
    }

    async fn entry(&self, id: EntryId, _cx: &Cx) -> Result<Option<CommittedEntry>, StorageFailure> {
        Ok(self
            .entries
            .get(&id)
            .map(|(entry, commit_seq)| CommittedEntry {
                entry: entry.clone(),
                commit_seq: *commit_seq,
            }))
    }

    async fn entry_in(
        &self,
        conversation_id: ConversationId,
        id: EntryId,
        _cx: &Cx,
    ) -> Result<Option<CommittedEntry>, StorageFailure> {
        let visible = self
            .ancestry(conversation_id)
            .into_iter()
            .any(|(level, cap)| {
                self.entries
                    .get(&id)
                    .is_some_and(|(entry, _)| entry.conversation_id == level)
                    && cap.is_none_or(|cap| id <= cap)
            });
        if !visible {
            return Ok(None);
        }
        Ok(self
            .entries
            .get(&id)
            .map(|(entry, commit_seq)| CommittedEntry {
                entry: entry.clone(),
                commit_seq: *commit_seq,
            }))
    }

    async fn find_latest_head_marker(
        &self,
        conversation_id: ConversationId,
        at_or_before: Option<EntryId>,
        _cx: &Cx,
    ) -> Result<Option<HeadMarker>, StorageFailure> {
        for id in self.visible_entries(conversation_id) {
            if at_or_before.is_some_and(|cutoff| id > cutoff) {
                continue;
            }
            if let Some((entry, _)) = self.entries.get(&id)
                && let Some(head) = entry.head
            {
                return Ok(Some(HeadMarker {
                    entry: entry.clone(),
                    head,
                }));
            }
        }
        Ok(None)
    }

    async fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: PageLimit,
        from: Option<EntryCursor>,
        _cx: &Cx,
    ) -> Result<Page<EntryRecord, EntryCursor>, StorageFailure> {
        let resume = match &from {
            Some(cursor) => {
                self.resume_from(cursor.payload_for(self.store_id).map_err(wrong_store)?)?
            }
            None => None,
        };
        let items: Vec<EntryRecord> = self
            .visible_entries(query.conversation_id)
            .into_iter()
            // Newest-first, so a cursor resumes strictly below it.
            .filter(|id| resume.is_none_or(|resume| RawId::from(*id).get() < resume))
            .filter(|id| query.min_entry_id.is_none_or(|min| *id >= min))
            .filter(|id| query.max_entry_id.is_none_or(|max| *id <= max))
            .filter_map(|id| self.entries.get(&id).map(|(entry, _)| entry.clone()))
            .take(saturating_page(limit))
            .collect();
        Ok(page(items, limit, |entry| {
            EntryCursor::new(self.cursor_at(RawId::from(entry.id).get()))
        }))
    }

    async fn task(&self, id: TaskId, _cx: &Cx) -> Result<Option<TaskRecord>, StorageFailure> {
        Ok(self.tasks.get(&id).cloned())
    }

    async fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: PageLimit,
        from: Option<TaskCursor>,
        _cx: &Cx,
    ) -> Result<Page<TaskRecord, TaskCursor>, StorageFailure> {
        let resume = match &from {
            Some(cursor) => {
                self.resume_from(cursor.payload_for(self.store_id).map_err(wrong_store)?)?
            }
            None => None,
        };
        let items: Vec<TaskRecord> = self
            .tasks
            .iter()
            .filter(|(id, _)| resume.is_none_or(|resume| RawId::from(**id).get() > resume))
            .filter(|(_, record)| {
                query
                    .conversation_id
                    .is_none_or(|want| record.conversation_id == want)
                    && query.kind.as_ref().is_none_or(|want| record.kind == *want)
                    && query.status.is_none_or(|want| record.status() == want)
                    && query
                        .abort_requested
                        .is_none_or(|want| record.abort_requested == want)
                    && query
                        .background
                        .is_none_or(|want| record.background == want)
            })
            .map(|(_, record)| record.clone())
            .take(saturating_page(limit))
            .collect();
        Ok(page(items, limit, |record| {
            TaskCursor::new(self.cursor_at(RawId::from(record.id).get()))
        }))
    }

    async fn submission(
        &self,
        id: SubmissionId,
        _cx: &Cx,
    ) -> Result<Option<SubmissionRecord>, StorageFailure> {
        Ok(self.submissions.get(&id).cloned())
    }

    async fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: PageLimit,
        from: Option<SubmissionCursor>,
        _cx: &Cx,
    ) -> Result<Page<SubmissionRecord, SubmissionCursor>, StorageFailure> {
        let resume = match &from {
            Some(cursor) => {
                self.resume_from(cursor.payload_for(self.store_id).map_err(wrong_store)?)?
            }
            None => None,
        };
        let items: Vec<SubmissionRecord> = self
            .submissions
            .iter()
            .filter(|(id, _)| resume.is_none_or(|resume| RawId::from(**id).get() > resume))
            .filter(|(_, record)| {
                query
                    .conversation_id
                    .is_none_or(|want| record.conversation_id == want)
                    && query.status.is_none_or(|want| record.status() == want)
                    && query
                        .submission_type
                        .is_none_or(|want| record.submission_type() == want)
            })
            .map(|(_, record)| record.clone())
            .take(saturating_page(limit))
            .collect();
        Ok(page(items, limit, |record| {
            SubmissionCursor::new(self.cursor_at(RawId::from(record.id).get()))
        }))
    }

    async fn submission_by_request(
        &self,
        conversation_id: ConversationId,
        request_id: &str,
        _cx: &Cx,
    ) -> Result<Option<SubmissionRecord>, StorageFailure> {
        Ok(self
            .submissions
            .values()
            .find(|record| {
                record.conversation_id == conversation_id
                    && record.request_id.as_deref() == Some(request_id)
            })
            .cloned())
    }

    async fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
        _cx: &Cx,
    ) -> Result<Option<DocumentRecord>, StorageFailure> {
        // Newest incarnation first, because a retire-plus-create at one address leaves the new one
        // current at that sequence (`spec.md:4364-4365`).
        Ok(self
            .documents
            .values()
            .rev()
            .find(|state| state.record.address() == *address && alive_at(&state.record, at))
            .map(|state| state.record.clone()))
    }

    async fn document(
        &self,
        id: DocumentId,
        at: DocumentPoint,
        _cx: &Cx,
    ) -> Result<Option<StoredDocument>, StorageFailure> {
        let Some(state) = self.documents.get(&id) else {
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
        let (version, value, deltas_since_base) = self.replay(state, upto)?;
        Ok(Some(StoredDocument {
            record: state.record.clone(),
            version,
            value,
            deltas_since_base,
        }))
    }

    async fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: PageLimit,
        from: Option<DocumentCursor>,
        _cx: &Cx,
    ) -> Result<Page<DocumentRecord, DocumentCursor>, StorageFailure> {
        let resume = match &from {
            Some(cursor) => {
                self.resume_from(cursor.payload_for(self.store_id).map_err(wrong_store)?)?
            }
            None => None,
        };
        let items: Vec<DocumentRecord> = self
            .documents
            .iter()
            // Ascending incarnation ids (`spec.md:4344`).
            .filter(|(id, _)| resume.is_none_or(|resume| RawId::from(**id).get() > resume))
            .filter(|(_, state)| {
                state.record.scope.as_ref() == query.scope
                    && query
                        .kind
                        .as_ref()
                        .is_none_or(|want| state.record.kind == *want)
                    && alive_at(&state.record, query.at)
            })
            .map(|(_, state)| state.record.clone())
            .take(saturating_page(limit))
            .collect();
        Ok(page(items, limit, |record| {
            DocumentCursor::new(self.cursor_at(RawId::from(record.id).get()))
        }))
    }
}

/// Whether an incarnation is a member of the store at `at`.
///
/// `spec.md:4358`: *"Metadata membership remains queryable historically"* — so this is answered from
/// the record's [`Lifetime`] for **every** scope, including the current-only ones whose *content* at a
/// numeric point is refused.
fn alive_at(record: &DocumentRecord, at: DocumentPoint) -> bool {
    match at {
        DocumentPoint::Current => record.lifetime.is_open(),
        DocumentPoint::At(seq) => record.lifetime.contains(seq),
    }
}

/// One more than the page limit, so a scan can tell a full page from the end of the scan without a
/// second pass. Saturating, because `PageLimit::MAX` is far below `usize::MAX` on every target cyrup
/// builds for and a wrap here would silently truncate a page.
fn saturating_page(limit: PageLimit) -> usize {
    (limit.get() as usize).saturating_add(1)
}

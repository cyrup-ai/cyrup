//! The committed state this backend holds in memory, and the **one** function that changes it.
//!
//! # Why there is one application path and not two
//!
//! A durable backend has two ways to reach a state: replay it at open, or apply a commit it just
//! wrote. Writing those twice is how a file backend's recovery drifts from its commit path, and the
//! drift is invisible until a reopen answers differently from the session that wrote it. So there is
//! exactly one: [`Committed::apply_line`], over [`MainLine`]. [`crate::recover`] calls it for every
//! confirmed line it reads; [`crate::store`]'s commit calls it for the very lines it just appended,
//! **after** the marker write succeeded (`spec.md:4417`: *"publish in memory only after the marker write
//! succeeds"*). The in-memory state and the log therefore cannot disagree about what a line means.
//!
//! # What is in memory and what is not
//!
//! Every record table is, because `spec.md:4325-4351`'s named access paths must be answered from an
//! index and ADR-0030 §2.2 prices the alternative: *"if they degrade to scans, cost grows with total
//! history: a long session gets slower until it is unusable."* Document **content** is not: a
//! [`DocState`] holds only its record and its sidecar's *layout* — one [`Slice`] per content record,
//! built from the markers — so ADR-0030 §9's *"the sidecars are never read at open"* holds literally
//! and `document(id, at)` is one seek plus the tail (ADR-0030 F6 §D).
//!
//! # The cross-batch checks, which are the ones the keyed batch does not lift
//!
//! [`Committed::validate`] is ADR-0030 F3's residue and nothing more: global id ownership against
//! committed records, immutable conversation and entry creation, address occupancy, write-after-retire,
//! and a stale version witness. It is **pure** — no I/O at all — which is what makes
//! [`CommitError::Rejected`]'s claim *"nothing durable happened"* true by construction here rather than
//! by care. The one part of a copy that needs the medium is its value, and
//! [`crate::store`] reads that before it allocates a sequence.
//!
//! [`CommitError::Rejected`]: cyrup_pico_store::CommitError::Rejected

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use cyrup_pico_doc::DefVersion;
use cyrup_pico_store::{
    Batch, ConversationId, ConversationRecord, Corruption, DocumentAddress, DocumentCommand,
    DocumentContent, DocumentId, DocumentRecord, EntryId, EntryRecord, Id, IdKind, IdKindTag,
    PageLimit, RawId, RejectedReason, Seq, SubmissionId, SubmissionRecord, TaskId, TaskRecord,
};

use crate::index::Indexes;
use crate::wire::{MainLine, Slice};

/// How deep a conversation ancestry chain may be before this backend stops walking it.
///
/// A cycle in `parent` is not reachable from any commit, but it **is** reachable from a damaged store,
/// and `#![forbid(unsafe_code)]` does not stop a stack overflow. The walk is iterative and carries a
/// visited set; this bound is the second line of defence and a documented envelope. The number is
/// `MemoryStore`'s, so the two backends cap at the same depth and the conformance suite cannot
/// distinguish them by it.
pub(crate) const MAX_ANCESTRY_DEPTH: usize = 4_096;

/// One document incarnation's committed state: its record, and where its content lives.
#[derive(Clone, Debug)]
pub(crate) struct DocState {
    /// The persisted record, with its half-open lifetime.
    pub record: DocumentRecord,
    /// Which sidecar generation is authoritative. Advanced only by reclamation.
    pub generation: u32,
    /// One entry per content record, ascending by sequence and by offset.
    pub slices: Vec<Slice>,
}

/// Which bytes of a sidecar one read needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct SliceSpan {
    /// The index of the first selected record in [`DocState::slices`].
    pub first: usize,
    /// The index of the last selected record.
    pub last: usize,
    /// The first selected record's start offset.
    pub start: u64,
    /// The last selected record's end offset.
    pub end: u64,
}

impl SliceSpan {
    /// How many records lie in `start..end`.
    pub(crate) const fn records(&self) -> usize {
        self.last.saturating_sub(self.first).saturating_add(1)
    }
}

impl DocState {
    /// The sidecar's confirmed length: the next append's offset.
    pub(crate) fn end(&self) -> u64 {
        self.slices.last().map_or(0, |s| s.end)
    }

    /// The definition version of the newest applicable base.
    ///
    /// Derived rather than stored, so it cannot drift from the slices it summarises, and so a
    /// [`DocState`] has no field that would need a placeholder between a creation line and the marker
    /// that confirms it. `None` means *"no base at all"*, which only a damaged store can produce and
    /// which [`cyrup_pico_doc::ReplayPlan::parse`] is the right place to report.
    pub(crate) fn stored_version(&self) -> Option<DefVersion> {
        self.slices.iter().rev().find(|s| s.base).map(|s| s.version)
    }

    /// The byte span a read at `upto` needs: the newest applicable base through the newest selected
    /// record.
    ///
    /// This is the mechanism ADR-0030 F6 §D buys with the store lock — *"one seek plus the tail"* —
    /// and it is computed entirely from the marker-carried offsets, with no payload read.
    pub(crate) fn span(&self, upto: Option<Seq>) -> Option<SliceSpan> {
        let last = self
            .slices
            .iter()
            .rposition(|s| upto.is_none_or(|upto| s.seq <= upto))?;
        let selected = self.slices.get(..=last)?;
        // No base among the selected records is corruption, not an absence — so the span starts at the
        // first record and `ReplayPlan::parse` reports `MissingRequiredBase` from the records
        // themselves.
        let first = selected.iter().rposition(|s| s.base).unwrap_or(0);
        let start = match first.checked_sub(1) {
            Some(previous) => self.slices.get(previous).map_or(0, |s| s.end),
            None => 0,
        };
        Some(SliceSpan {
            first,
            last,
            start,
            end: self.slices.get(last)?.end,
        })
    }

    /// Which records a reclamation would keep: the newest base and everything after it.
    ///
    /// `spec.md:4354-4356` and §11.1: *"preserves rewindable records and reclaims latest records only
    /// after a committed base or retirement."* The authorising write **is** that base, which is why the
    /// predicate is *"is there anything before the newest base"* and not a size or an age.
    pub(crate) fn reclaimable_from(&self) -> Option<usize> {
        if self.record.retains_history() {
            return None;
        }
        match self.slices.iter().rposition(|s| s.base) {
            Some(at) if at > 0 => Some(at),
            _ => None,
        }
    }
}

/// Everything this backend knows about committed state.
#[derive(Debug)]
pub(crate) struct Committed {
    /// The newest commit's sequence, if there is one.
    ///
    /// The *only* sequence state there is: the next one to allocate is this one's successor, so
    /// "what was the last commit" and "what comes next" cannot disagree.
    pub last_seq: Option<Seq>,
    /// How many commits this state was built from.
    ///
    /// Not derivable from [`Committed::last_seq`], because `spec.md:99-100` permits gaps, and it is the
    /// **count** that ADR-0030 §9's *"open is O(commits + table records)"* is linear in.
    pub commits: u64,
    /// The next id to hand out.
    pub next_id: u64,
    /// Every id up to and including this number is durably spent.
    pub reserved_through: u64,
    pub conversations: BTreeMap<ConversationId, ConversationRecord>,
    pub entries: BTreeMap<EntryId, (EntryRecord, Seq)>,
    pub tasks: BTreeMap<TaskId, TaskRecord>,
    pub submissions: BTreeMap<SubmissionId, SubmissionRecord>,
    pub documents: BTreeMap<DocumentId, DocState>,
    /// Which kind of record owns each number.
    ///
    /// ADR-0030 F6 §A forbids *"a separate **durable** number-to-kind table"* and asks for ownership to
    /// be *"derived from the per-table indexes, which exist anyway"*. This is that derivation, kept as
    /// a map rather than recomputed by five probes per check: it is in-memory only, nothing persists
    /// it, and [`Committed::apply_line`] is its sole writer — the same function that writes the tables
    /// — so there is no second path by which it could disagree with them.
    pub owners: BTreeMap<NonZeroU64, IdKindTag>,
    /// The secondary indexes `spec.md:4325-4351`'s named access paths are answered from
    /// (PICO5-PLAN S10).
    ///
    /// Written by [`Committed::apply_line`] and by nothing else, which is why a commit that is applied
    /// is indexed in the same statement — see [`crate::index`] for why that is the whole mechanism
    /// behind `G-COMMIT-VISIBLE-TO-LATER-READS` surviving their addition.
    pub(crate) indexes: Indexes,
}

impl Committed {
    /// An empty state.
    ///
    /// The store's identity is not here: it belongs to the identity record, which
    /// [`Reader`](crate::Reader) holds, because it is durable and this state is derived.
    pub(crate) fn new() -> Self {
        Self {
            last_seq: None,
            commits: 0,
            // **2, not 1.** `spec.md:249` fixes `ROOT_CONVERSATION_ID` at 1, so 1 is owned before this
            // store has committed anything and a namespace that handed it out would collide with the
            // root conversation on its first allocation.
            next_id: 2,
            reserved_through: 1,
            conversations: BTreeMap::new(),
            entries: BTreeMap::new(),
            tasks: BTreeMap::new(),
            submissions: BTreeMap::new(),
            documents: BTreeMap::new(),
            owners: BTreeMap::new(),
            indexes: Indexes::default(),
        }
    }

    /// Apply one confirmed `main.jsonl` line.
    ///
    /// The only mutation path. See the module documentation for why that is the design and not a
    /// convenience.
    ///
    /// # Errors
    ///
    /// [`Corruption`] when the line cannot be reconciled with what is already applied: a second
    /// creation of an immutable record, a non-increasing marker, a retirement of something absent or
    /// already retired, an id filed under two kinds. Every one of those fails the open
    /// (`spec.md:4433`) rather than being normalised.
    pub(crate) fn apply_line(&mut self, line: &MainLine) -> Result<(), Corruption> {
        match line {
            MainLine::Conversation { r, .. } => {
                self.claim(r.id, IdKindTag::Conversation)?;
                if self.conversations.insert(r.id, r.clone()).is_some() {
                    return Err(twice("conversation", RawId::from(r.id)));
                }
                self.indexes.add_conversation(r);
            }
            MainLine::Entry { seq, r } => {
                self.claim(r.id, IdKindTag::Entry)?;
                if self.entries.insert(r.id, (r.clone(), *seq)).is_some() {
                    return Err(twice("entry", RawId::from(r.id)));
                }
                self.indexes.add_entry(r);
            }
            // `spec.md:4383`: live task and submission transitions replace one row, so a second line
            // for one id is how progress is recorded rather than damage.
            MainLine::Task { r, .. } => {
                self.claim(r.id, IdKindTag::Task)?;
                // The replaced row is handed to the index so its old memberships are retired in the same
                // statement that files the new ones (`spec.md:4383`; see [`crate::index`]).
                let previous = self.tasks.insert(r.id, r.clone());
                self.indexes.put_task(previous.as_ref(), r);
            }
            MainLine::Submission { r, .. } => {
                self.claim(r.id, IdKindTag::Submission)?;
                let previous = self.submissions.insert(r.id, r.clone());
                self.indexes.put_submission(previous.as_ref(), r);
            }
            MainLine::Document { r, .. } => {
                self.claim(r.id, IdKindTag::Document)?;
                let state = DocState {
                    record: r.clone(),
                    generation: 0,
                    slices: Vec::new(),
                };
                if self.documents.insert(r.id, state).is_some() {
                    return Err(twice("document", RawId::from(r.id)));
                }
                self.indexes.add_document(r);
            }
            MainLine::Retire { seq, id } => {
                let state = self
                    .documents
                    .get_mut(id)
                    .ok_or_else(|| missing(format!("document {id}, retired at sequence {seq}")))?;
                if state.record.lifetime.retired_at().is_some() {
                    return Err(Corruption::RecordMalformed {
                        what: "document retirement",
                        detail: format!("{id} is retired twice"),
                    });
                }
                state.record.lifetime = state.record.lifetime.retire(*seq).map_err(|e| {
                    Corruption::RecordMalformed {
                        what: "document retirement",
                        detail: e.to_string(),
                    }
                })?;
            }
            MainLine::Marker { seq, docs, .. } => {
                // `spec.md:99-100`: commit sequences strictly increase across the store's life,
                // including reopen. ADR-0030 §2.2 classifies it **checked** and says why it cannot be
                // lifted — it crosses a process boundary — so this is the check, at both boundaries,
                // because both go through this function.
                if let Some(previous) = self.last_seq
                    && *seq <= previous
                {
                    return Err(Corruption::SequenceNotIncreasing {
                        previous,
                        found: *seq,
                    });
                }
                for doc in docs {
                    let state = self.documents.get_mut(&doc.id).ok_or_else(|| {
                        missing(format!("document {} named by marker {seq}", doc.id))
                    })?;
                    if doc.end <= state.end() && !state.slices.is_empty() {
                        return Err(Corruption::RecordMalformed {
                            what: "commit marker",
                            detail: format!(
                                "document {}'s offset {} does not advance past {}",
                                doc.id,
                                doc.end,
                                state.end()
                            ),
                        });
                    }
                    state.slices.push(Slice {
                        seq: *seq,
                        end: doc.end,
                        base: doc.base,
                        version: doc.version,
                    });
                }
                self.last_seq = Some(*seq);
                self.commits = self.commits.saturating_add(1);
            }
            MainLine::Mint { high } => {
                self.reserved_through = self.reserved_through.max(*high);
                self.next_id = self.next_id.max(high.saturating_add(1));
            }
            MainLine::Reclaim {
                id,
                generation,
                keep,
                ..
            } => {
                let state = self
                    .documents
                    .get_mut(id)
                    .ok_or_else(|| missing(format!("document {id}, reclaimed")))?;
                state.generation = *generation;
                state.slices = keep.clone();
            }
        }
        Ok(())
    }

    /// Record which kind owns a number, and keep the id namespace above it.
    ///
    /// The kind check is ADR-0030 F6 §A's residue: *"the number carries no tag"*, so a record read from
    /// the task index deserialises its id as `Id<Task>` because that is the field's type, and a damaged
    /// file that files an entry id under tasks is caught **here** or not at all.
    fn claim<K: IdKind>(&mut self, id: Id<K>, kind: IdKindTag) -> Result<(), Corruption> {
        let raw = RawId::from(id);
        match self.owners.insert(raw.get(), kind) {
            Some(previous) if previous != kind => Err(Corruption::IdFiledUnderWrongKind {
                id: raw,
                found: kind,
                owner: previous,
            }),
            _ => {
                self.next_id = self.next_id.max(raw.get().get().saturating_add(1));
                Ok(())
            }
        }
    }

    /// Record a durable id reservation without spending it.
    ///
    /// The write-time half of [`MainLine::Mint`]: the writer that owns a block hands out its ids one at
    /// a time, while a *reopened* store spends the whole block because it cannot know which of them were
    /// handed out. See `crate::store`'s `mint_raw` for why that asymmetry is the conservative direction.
    pub(crate) fn reserve(&mut self, high: u64) {
        self.reserved_through = self.reserved_through.max(high);
    }

    /// Which kind of record owns `raw`, if any.
    pub(crate) fn owner_of(&self, raw: NonZeroU64) -> Option<IdKindTag> {
        self.owners.get(&raw).copied()
    }

    /// Reject a number this batch claims for `K` when another kind of record already owns it.
    fn check_foreign_owner<K: IdKind>(
        &self,
        id: Id<K>,
    ) -> Result<Option<IdKindTag>, RejectedReason> {
        match self.owner_of(RawId::from(id).get()) {
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
    /// `spec.md:258` and `:263`: caps only tighten going up, which is why each level's cap is the
    /// minimum of its own and everything below it.
    pub(crate) fn ancestry(&self, id: ConversationId) -> Vec<(ConversationId, Option<EntryId>)> {
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

    /// Validate a whole batch against committed state, touching nothing.
    ///
    /// # Errors
    ///
    /// [`RejectedReason`], always before any write. The copy-source *value* is the one thing not
    /// checked here, because reading it needs the medium; everything about a copy that can be decided
    /// from records — the in-batch prohibition, the four field agreements and liveness at the selected
    /// point — is decided here.
    pub(crate) fn validate(&self, batch: &Batch) -> Result<(), RejectedReason> {
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
                return Err(RejectedReason::ConversationExists(*id));
            }
        }
        for id in batch.entries().keys() {
            claim(RawId::from(*id), IdKindTag::Entry)?;
            if self.check_foreign_owner(*id)?.is_some() {
                return Err(RejectedReason::EntryExists(*id));
            }
        }
        for id in batch.tasks().keys() {
            claim(RawId::from(*id), IdKindTag::Task)?;
            self.check_foreign_owner(*id)?;
        }
        for id in batch.submissions().keys() {
            claim(RawId::from(*id), IdKindTag::Submission)?;
            self.check_foreign_owner(*id)?;
        }

        // Two creations at one address in ONE batch is the half the keyed batch cannot see: the map is
        // keyed by incarnation, and two new incarnations at one address have two different keys.
        let mut staged: BTreeMap<DocumentAddress, DocumentId> = BTreeMap::new();

        for (id, command) in batch.documents() {
            claim(RawId::from(*id), IdKindTag::Document)?;
            let existing = self.check_foreign_owner(*id)?;
            match command {
                DocumentCommand::Create { record, .. } | DocumentCommand::Copy { record, .. } => {
                    if existing.is_some() {
                        return Err(RejectedReason::DocumentExists(*id));
                    }
                    let address = record.address();
                    if let Some(occupant) = staged.insert(address.clone(), *id) {
                        return Err(RejectedReason::AddressOccupied { address, occupant });
                    }
                    self.check_address_free(&address, batch)?;
                }
                DocumentCommand::Change { content, .. } => {
                    let state = self
                        .documents
                        .get(id)
                        .ok_or(RejectedReason::UnknownDocument(*id))?;
                    check_not_retired(state)?;
                    if let DocumentContent::Delta { continues, .. } = content {
                        // A witness that is real but **stale**: a later commit moved the document's
                        // stored version. The non-stale half is already unrepresentable, because
                        // `StoredVersion` is minted only by a storage read (ADR-0030 F6 §C).
                        let stored = state
                            .stored_version()
                            .unwrap_or_else(|| continues.version());
                        if continues.version() != stored {
                            return Err(RejectedReason::VersionTransitionRequiresBase {
                                document: *id,
                                stored,
                                continues: continues.version(),
                            });
                        }
                    }
                }
                DocumentCommand::RetireOnly => {
                    let state = self
                        .documents
                        .get(id)
                        .ok_or(RejectedReason::UnknownDocument(*id))?;
                    check_not_retired(state)?;
                }
            }
            if let DocumentCommand::Copy { record, source, .. } = command {
                crate::copy::check_source(self, record, *source, batch)?;
            }
        }
        Ok(())
    }

    /// `spec.md:4362-4365`: at most one live incarnation per logical address, unless this batch retires
    /// the occupant.
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

    /// Allocate this commit's sequence.
    ///
    /// # Errors
    ///
    /// [`RejectedReason::SequenceExhausted`], which is fallible rather than wrapping because a wrapped
    /// sequence is a sequence that does not increase.
    pub(crate) fn allocate_seq(&self) -> Result<Seq, RejectedReason> {
        match self.last_seq {
            None => Ok(Seq::FIRST),
            Some(previous) => previous
                .successor()
                .ok_or(RejectedReason::SequenceExhausted),
        }
    }
}

/// A write to an already-retired incarnation is rejected rather than reviving it.
fn check_not_retired(state: &DocState) -> Result<(), RejectedReason> {
    match state.record.lifetime.retired_at() {
        Some(retired_at) => Err(RejectedReason::DocumentRetired {
            document: state.record.id,
            retired_at,
        }),
        None => Ok(()),
    }
}

fn twice(what: &'static str, id: RawId) -> Corruption {
    Corruption::RecordMalformed {
        what,
        detail: format!("id {id} is created twice, and creation is immutable"),
    }
}

fn missing(what: String) -> Corruption {
    Corruption::MissingConfirmedData { what }
}

/// One more than the page limit, so a scan can tell a full page from the end of the scan without a
/// second pass. Saturating, because [`PageLimit::MAX`] is far below `usize::MAX` on every target cyrup
/// builds for and a wrap here would silently truncate a page.
pub(crate) fn saturating_page(limit: PageLimit) -> usize {
    (limit.get() as usize).saturating_add(1)
}

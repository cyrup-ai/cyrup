//! The scan queries and the shapes reads return (`spec.md:4196-4247`).
//!
//! Every query here is a struct of `Option` filters, which is how upstream writes them and is the
//! right shape for one reason worth stating: `spec.md:4337-4338` makes conversation owner filters
//! *"indexed and conjunctive"*, so an absent filter means *"do not constrain"* and never
//! *"match nothing"*. An enum would be wrong — these are not alternatives.
//!
//! What is **not** here is a general predicate, a sort order or a projection. `spec.md:4325-4339`
//! gives storage a closed set of named access paths precisely so a caller cannot answer them by
//! scanning, and ADR-0030 §2.2 prices that requirement: *"if they degrade to scans, cost grows with
//! total history: a long session gets slower until it is unusable."*

use cyrup_pico_doc::{DefVersion, DocRoot, StoredVersion};

use crate::records::{DocumentRecord, ScopeRef, SubmissionStatus, SubmissionType, TaskStatus};
use crate::{ConversationId, DocumentPoint, Kind, Seq, TaskId};

/// Which conversations to return (`spec.md:4204-4207`).
///
/// Both filters are *conjunctive*: with both set, a conversation must match both.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ConversationQuery {
    /// Only conversations owned by a task of this conversation.
    pub owner_conversation_id: Option<ConversationId>,
    /// Only conversations owned by this task.
    pub owner_task_id: Option<TaskId>,
}

/// Which entries to return (`spec.md:4209-4213`).
///
/// The conversation is **required**: there is no all-conversation entry scan.
/// `spec.md:4334-4336`: *"`scanEntries()` pages the inclusive ID range in newest-first order while
/// applying every conversation ancestry cap."* Both bounds are inclusive, and the cap application is
/// the backend's — a caller cannot opt out of it, which is the whole point of the method existing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EntryQuery {
    /// The conversation whose visible history to page.
    pub conversation_id: ConversationId,
    /// Inclusive lower bound on the entry id.
    pub min_entry_id: Option<crate::EntryId>,
    /// Inclusive upper bound on the entry id.
    pub max_entry_id: Option<crate::EntryId>,
}

impl EntryQuery {
    /// Every visible entry of one conversation.
    #[must_use]
    pub const fn all(conversation_id: ConversationId) -> Self {
        Self {
            conversation_id,
            min_entry_id: None,
            max_entry_id: None,
        }
    }
}

/// Which tasks to return (`spec.md:4215-4222`).
///
/// The five fields `spec.md:4348` names: *"conversation, kind, live/terminal status, abort mark, and
/// background status."*
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct TaskQuery {
    /// Only tasks of this conversation.
    pub conversation_id: Option<ConversationId>,
    /// Only tasks of this definition kind.
    pub kind: Option<Kind>,
    /// Only tasks in this state.
    pub status: Option<TaskStatus>,
    /// Only tasks with (or without) a durable abort mark.
    pub abort_requested: Option<bool>,
    /// Only background (or foreground) tasks.
    pub background: Option<bool>,
}

/// Which submissions to return (`spec.md:4224-4227`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SubmissionQuery {
    /// Only submissions of this conversation.
    pub conversation_id: Option<ConversationId>,
    /// Only submissions in this state.
    pub status: Option<SubmissionStatus>,
    /// Only input, or only write, submissions.
    ///
    /// Not in upstream's `SubmissionQuery`, which types `status` as `SubmissionRecord["status"]` —
    /// the union of both kinds' statuses, so `placed` silently means *"input only"* and the caller
    /// cannot ask for one kind without naming a status. The field is here because the two kinds have
    /// different state sets (see [`SubmissionState`](crate::SubmissionState)) and a filter that
    /// cannot say which kind it means is a filter whose answer depends on that coincidence.
    pub submission_type: Option<SubmissionType>,
}

/// Which document incarnations to return (`spec.md:4236-4240`).
///
/// `spec.md:4342-4345`: *"`scanDocuments()` enumerates only the incarnations alive in one exact scope
/// at its selected point and may restrict one family/singleton kind. It uses ascending incarnation
/// IDs. There is no ordinary open-time all-document scan."* The scope is therefore required, and the
/// point is part of the query rather than of the cursor, because membership is what is being
/// enumerated.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DocumentQuery {
    /// The exact scope to enumerate.
    pub scope: ScopeRef,
    /// The point at which to test membership.
    pub at: DocumentPoint,
    /// Only this definition kind.
    pub kind: Option<Kind>,
}

/// One page of a scan (`spec.md:4190-4193`).
///
/// `next` is `None` at the end of the scan. `spec.md:4351`: *"`limit` is always the maximum page
/// size"* — a short page does **not** mean the scan is over, which is why the end is signalled by the
/// absent cursor and not by `items.len() < limit`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Page<T, C> {
    /// This page's items.
    pub items: Vec<T>,
    /// Where to resume, or `None` at the end.
    pub next: Option<C>,
}

impl<T, C> Page<T, C> {
    /// A final page.
    #[must_use]
    pub const fn last(items: Vec<T>) -> Self {
        Self { items, next: None }
    }

    /// A page with more to come.
    #[must_use]
    pub const fn more(items: Vec<T>, next: C) -> Self {
        Self {
            items,
            next: Some(next),
        }
    }

    /// Whether this page is the end of the scan.
    #[must_use]
    pub const fn is_last(&self) -> bool {
        self.next.is_none()
    }
}

/// One materialized document incarnation (`spec.md:4242-4247`).
///
/// `spec.md:4345-4350`: *"It selects the newest applicable base, applies its ordered Chord delta
/// tail, and returns the detached materialized value, stored definition version, and number of
/// replayed deltas after that base. Base/delta records are backend-private."*
///
/// Three things about this type are deliberate:
///
/// * **The value is a [`DocRoot`]**, so it is immutable and `Arc`-shared. That is how
///   `spec.md:4375-4378`'s detachment is met without the recursive copy `memory.ts:92-100` performs —
///   ADR-0030 F4's `CYRUP-DELTA`.
/// * **`deltas_since_base` is here**, because `spec.md:1392-1396`'s checkpoint predicate takes it and
///   must not perform a storage read to get it. The read that produced the value is where it is free.
/// * **The base and the tail are not here.** They are backend-private, so a caller cannot start
///   reasoning about the representation, and a backend is free to change it.
///
/// # The version is a witness, and that is a deliberate strengthening of ADR-0030 §10
///
/// `version` is a [`StoredVersion`], not a [`DefVersion`]. ADR-0030 F6 §C wants *"a `StoredVersion`
/// witness minted only by a storage read"* so that a `DocumentContent::Delta { continues }` *"cannot
/// claim a version it did not read"* — and [`cyrup_pico_doc::ReplayPlan::parse`], the only minting
/// site, runs **inside** the backend. If this field were a plain `DefVersion`, the witness would be
/// destroyed at the one boundary it has to cross, and the kernel would have no way to build a legal
/// delta at all. So the read carries the witness out. [`StoredDocument::def_version`] is there for the
/// callers that only want the number.
#[derive(Clone, PartialEq, Debug)]
pub struct StoredDocument {
    /// The incarnation's record.
    pub record: DocumentRecord,
    /// The definition version the value is stored at, as an unforgeable witness.
    pub version: StoredVersion,
    /// The materialized value.
    pub value: DocRoot,
    /// How many deltas were replayed after the selected base.
    pub deltas_since_base: u32,
}

impl StoredDocument {
    /// The stored definition version as a plain number-shaped value.
    #[must_use]
    pub const fn def_version(&self) -> DefVersion {
        self.version.version()
    }
}

/// An entry with the commit sequence a historical document read needs (`spec.md:4314`, `:4347`).
///
/// `spec.md:4347`: *"`entry(id)` combines exact global lookup with the commit sequence required by
/// historical document reads."* The pair is one struct so the sequence cannot be dropped on the way
/// to the document read that needs it.
#[derive(Clone, PartialEq, Debug)]
pub struct CommittedEntry {
    /// The entry.
    pub entry: crate::EntryRecord,
    /// The sequence of the commit that wrote it.
    pub commit_seq: Seq,
}

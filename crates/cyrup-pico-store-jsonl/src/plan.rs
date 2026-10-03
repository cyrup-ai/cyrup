//! What a read did, as a value (PICO5-PLAN S10).
//!
//! # Why the plan is returned rather than counted
//!
//! `spec.md:4337-4339` requires the conversation owner filters to be *"indexed and conjunctive"* and
//! adds the sentence that makes it an obligation rather than advice: *"application-maintained
//! registries are not a substitute for these kernel indexes."* ADR-0030 §2.2 classifies the whole row
//! **checked** on exactly this half — *"nothing forces a backend to use an index (query-plan and
//! benchmark suites do that)"* (ADR-0030 §6) — and prices the failure: *"if they degrade to scans, cost
//! grows with total history: a long session gets slower until it is unusable."*
//!
//! A check needs something to check. The usual shape is a counter the backend bumps and a test reads,
//! and it has a specific weakness: the counter is a second description of what the code did, and a
//! refactor can leave it describing the old path. So here the plan is **produced by the walk itself**.
//! [`Planned`](crate::Planned)'s methods are the primitives — they select the driving index, walk it,
//! and return the page beside the [`Plan`] that walk *is* — and [`Reader`](crate::Reader)'s
//! `Storage`-shaped methods are one-line delegations that drop the plan. There is no second path a
//! read can take, so a plan that says `EntriesByConversation` cannot be a read that scanned.
//!
//! # What `visited` counts, exactly
//!
//! Every index entry the walk looked at: each candidate id drawn from the driving index, plus each
//! conversation record walked to establish an ancestry cap. It is **not** the number of items
//! returned, and the difference is the point — a read that visits a thousand entries to return ten has
//! a `visited` of a thousand, and the suite's assertion is that the number does not move when
//! unrelated history is added.
//!
//! # Why this is a public surface and not `#[cfg(test)]`
//!
//! Two reasons. The conformance suite is driven from another crate (`tests/conformance.rs`), so a
//! `cfg(test)` plan would be invisible to exactly the tests that are the contract. And PICO5-PLAN S12's
//! SQLite backend has to answer the same question, where the native idiom is `EXPLAIN QUERY PLAN`
//! against the connection; naming the answer in the storage layer rather than in a test harness is what
//! lets the two backends be compared at all.

/// Which index answered a read, and how many of its entries the read visited.
///
/// Returned by every [`Planned`](crate::Planned) method beside the answer itself. See this module's
/// documentation for why the plan is produced by the walk rather than counted alongside it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Plan {
    /// The index the read was driven by.
    pub index: Index,
    /// How many index entries the read visited, including the ancestry records it walked.
    pub visited: u64,
}

impl Plan {
    /// A plan naming one index and one visit count.
    #[must_use]
    pub const fn new(index: Index, visited: u64) -> Self {
        Self { index, visited }
    }

    /// A keyed lookup in a primary id-keyed table: one probe, whatever the store holds.
    #[must_use]
    pub const fn keyed() -> Self {
        Self::new(Index::Primary, 1)
    }
}

/// The named index a read was driven by (`spec.md:4325-4351`).
///
/// Closed, and deliberately not `#[non_exhaustive]`: the variants are this backend's index set, and a
/// new one is a change to the backend that the query-plan suite should have to name.
///
/// The three `…Unfiltered` variants are **not** a scan admitted under a nicer name. They are the case
/// where no filter was supplied at all, so the table *is* the answer and visiting a record is yielding
/// it. `spec.md:4337-4338` makes an absent filter mean *"do not constrain"*, which is why the
/// distinction exists; what the specification forbids is answering a *filtered* question by walking
/// everything, and that is what the other variants rule out.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Index {
    /// A primary id-keyed table.
    Primary,
    /// The conversation ancestry chain, walked to establish the caps a read applies.
    Ancestry,
    /// Conversations owned by one task (`spec.md:4337-4339`).
    ConversationsByOwnerTask,
    /// Conversations owned by a task of one conversation (`spec.md:4337-4339`).
    ConversationsByOwnerConversation,
    /// Every conversation, because the query named no owner.
    ConversationsUnfiltered,
    /// One conversation's entries, ordered by id, ranged and capped (`spec.md:4334-4336`).
    EntriesByConversation,
    /// One conversation's entries that carry a head (`spec.md:4331-4334`).
    HeadsByConversation,
    /// Tasks of one conversation (`spec.md:4348`).
    TasksByConversation,
    /// Tasks of one definition kind (`spec.md:4348`).
    TasksByKind,
    /// Tasks in one state (`spec.md:4348`).
    TasksByStatus,
    /// Tasks with, or without, a durable abort mark (`spec.md:4348`).
    TasksByAbortMark,
    /// Background, or foreground, tasks (`spec.md:4348`).
    TasksByBackground,
    /// Every task, because the query named no field.
    TasksUnfiltered,
    /// Submissions of one conversation.
    SubmissionsByConversation,
    /// Submissions in one state.
    SubmissionsByStatus,
    /// Input, or write, submissions.
    SubmissionsByType,
    /// One submission by its caller-supplied request id (`spec.md:4314`).
    SubmissionsByRequest,
    /// Every submission, because the query named no field.
    SubmissionsUnfiltered,
    /// The incarnations ever created at one logical address (`spec.md:4340-4342`).
    DocumentsByAddress,
    /// The incarnations ever created in one scope (`spec.md:4342-4345`).
    DocumentsByScope,
}

impl Index {
    /// Whether this index narrows the read before the records are inspected.
    ///
    /// False for [`Index::Primary`], which needs no narrowing, and for the three `…Unfiltered`
    /// variants, which were not asked to narrow anything. The query-plan suite uses it to say *"this
    /// query carried a filter, so a filtering index must have driven it"* in one assertion rather than
    /// by listing variants.
    #[must_use]
    pub const fn narrows(self) -> bool {
        !matches!(
            self,
            Self::Primary
                | Self::Ancestry
                | Self::ConversationsUnfiltered
                | Self::TasksUnfiltered
                | Self::SubmissionsUnfiltered
        )
    }
}

impl core::fmt::Display for Index {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Primary => "the primary table",
            Self::Ancestry => "the ancestry chain",
            Self::ConversationsByOwnerTask => "conversations_by_owner_task",
            Self::ConversationsByOwnerConversation => "conversations_by_owner_conversation",
            Self::ConversationsUnfiltered => "every conversation",
            Self::EntriesByConversation => "entries_by_conversation",
            Self::HeadsByConversation => "heads_by_conversation",
            Self::TasksByConversation => "tasks_by_conversation",
            Self::TasksByKind => "tasks_by_kind",
            Self::TasksByStatus => "tasks_by_status",
            Self::TasksByAbortMark => "tasks_by_abort_mark",
            Self::TasksByBackground => "tasks_by_background",
            Self::TasksUnfiltered => "every task",
            Self::SubmissionsByConversation => "submissions_by_conversation",
            Self::SubmissionsByStatus => "submissions_by_status",
            Self::SubmissionsByType => "submissions_by_type",
            Self::SubmissionsByRequest => "submissions_by_request",
            Self::SubmissionsUnfiltered => "every submission",
            Self::DocumentsByAddress => "documents_by_address",
            Self::DocumentsByScope => "documents_by_scope",
        })
    }
}

//! Task records (`spec.md:1570-1620`).
//!
//! # Scope, stated because this file looks like §5 and is not
//!
//! ADR-0029 puts §5 — the scheduler, the phase machine, the join protocol, the built-in tasks —
//! **out of build scope**. What is here is only what `spec.md:4294` and `:4348` make a *storage*
//! obligation: the record, and the five fields [`TaskQuery`] filters on (*"Task queries support
//! conversation, kind, live/terminal status, abort mark, and background status."*). No transition,
//! no handler, no scheduling.
//!
//! The representation is ADR-0030 §2.4's, and that section exists precisely so this file does not
//! reach for typestate: a task is *"a durable state machine attached to one conversation"*
//! (`spec.md:51`) whose states are persisted as JSON, reconstructed at open, held heterogeneously in
//! one id-keyed table and selected by external events — `RUST-DESIGN-REVIEW.md:57` and `:61` reject
//! typestate on all four counts, and the compiler-visible lifecycle (one phase invocation) ends long
//! before the real one. **So it is an enum, however stateful it looks.**
//!
//! [`TaskQuery`]: crate::TaskQuery

use cyrup_pico_doc::{DocRoot, DocValue};
use serde::{Deserialize, Serialize};

use crate::{ConversationId, DefVersion, Kind, TaskId};

/// A durable task record.
///
/// Unlike a conversation or an entry, a task record is **replaced** as it advances: `spec.md:4383`
/// says *"live task transitions replace one row"*. So a second write of the same id in a later batch
/// is legal and is how progress is recorded.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct TaskRecord {
    /// This task.
    ///
    /// Upstream carries the result type as a second brand parameter (`TaskId<Result>`,
    /// `spec.md:245`). That half is deliberately absent — see [`crate::TaskId`].
    pub id: TaskId,
    /// The conversation it is attached to.
    pub conversation_id: ConversationId,
    /// Its definition kind.
    pub kind: Kind,
    /// Its definition version.
    pub version: DefVersion,
    /// Its input. Opaque to storage.
    pub input: DocValue,
    /// The owning task, absent for a task its conversation owns (`spec.md:1601`). Immutable.
    ///
    /// Upstream's caller-facing `TaskOwnership` is a two-variant union
    /// (`{ kind: "conversation" } | { kind: "task"; taskId }`) and ADR-0030 §2.4 keeps it an enum.
    /// The **persisted** form is this optional field: the enum belongs to the caller-facing surface
    /// in `cyrup-pico`, because it is there that *"the caller cannot omit the choice"* has any
    /// meaning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<TaskId>,
    /// Whether it runs in the background.
    pub background: bool,
    /// Whether an abort has been durably requested.
    pub abort_requested: bool,
    /// Wall-clock milliseconds of the record's first change to `running` (pi `startedAt?`,
    /// `packages/durable/src/types.ts` and `spec.md:1645-1646` @v1.1.0, commit 36a686ee8): *"Kept
    /// through waits and recovery, so the span to `endedAt` includes them. Absent before the task
    /// first runs, and on records written by earlier versions."*
    ///
    /// Storage carries it because the record is a storage obligation; STAMPING it is not. Upstream
    /// stamps it in the Session as part of a task transition (`spec.md:1913-1923`, §5.3), and
    /// ADR-0029 keeps §5 — the task machine and its transitions — out of build scope, so nothing in
    /// cyrup writes it: it is preserved, as a record written elsewhere carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// Wall-clock milliseconds of the change to `terminal` (pi `endedAt?`, `spec.md:1647-1648`
    /// @v1.1.0); absent while live. Carried, not stamped, for the reason [`Self::started_at`] is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    /// Its state, and — for a live state only — its memos.
    pub state: TaskState,
}

impl TaskRecord {
    /// This record's status, for [`TaskQuery`]'s live/terminal filter.
    ///
    /// [`TaskQuery`]: crate::TaskQuery
    #[must_use]
    pub const fn status(&self) -> TaskStatus {
        self.state.status()
    }
}

/// A task's durable state (`spec.md:1582-1594`).
///
/// Upstream ties memos to liveness with a `memos?: never` arm: *"a terminal task has no memos."*
/// Here the memos live **inside** the three live variants, so a terminal task with memos is not a
/// value that can be built or decoded.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum TaskState {
    /// Admitted, not yet running.
    Pending {
        /// Where it resumes. Opaque to storage.
        checkpoint: DocValue,
        /// Its memos.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memos: Option<DocRoot>,
    },
    /// Running.
    Running {
        /// Where it resumes. Opaque to storage.
        checkpoint: DocValue,
        /// Its memos.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memos: Option<DocRoot>,
    },
    /// Parked without an invocation until every task in `on` is terminal (`spec.md:1586-1591`).
    Waiting {
        /// Where it resumes. Opaque to storage.
        checkpoint: DocValue,
        /// Its memos.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memos: Option<DocRoot>,
        /// The tasks it waits on.
        on: Vec<TaskId>,
        /// How their outcomes combine.
        policy: JoinPolicy,
    },
    /// Outcome decided; terminal once no ordinary owned work below is live. Runs no more code.
    Completing {
        /// The decided outcome.
        outcome: TaskOutcome,
    },
    /// Terminal.
    Terminal {
        /// The decided outcome.
        outcome: TaskOutcome,
    },
}

impl TaskState {
    /// This state's status tag.
    #[must_use]
    pub const fn status(&self) -> TaskStatus {
        match self {
            Self::Pending { .. } => TaskStatus::Pending,
            Self::Running { .. } => TaskStatus::Running,
            Self::Waiting { .. } => TaskStatus::Waiting,
            Self::Completing { .. } => TaskStatus::Completing,
            Self::Terminal { .. } => TaskStatus::Terminal,
        }
    }

    /// Whether this state is terminal.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal { .. })
    }
}

/// A task state's tag, as [`TaskQuery`] filters on it (`spec.md:4215`).
///
/// A separate type from [`TaskState`] because a query carries the tag **without** the state's data,
/// and reusing the state enum would make a filter carry a checkpoint it does not have.
///
/// [`TaskQuery`]: crate::TaskQuery
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// [`TaskState::Pending`].
    Pending,
    /// [`TaskState::Running`].
    Running,
    /// [`TaskState::Waiting`].
    Waiting,
    /// [`TaskState::Completing`].
    Completing,
    /// [`TaskState::Terminal`].
    Terminal,
}

/// How a join combines the outcomes it waits on (`spec.md:1580`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JoinPolicy {
    /// Resume as soon as one of them fails.
    FailFast,
    /// Wait for every one of them.
    AllSettled,
}

/// A task's result (`spec.md:1572-1578`).
///
/// Five named domain variants, per `RUST-DESIGN-REVIEW.md:84-88`. Note that three of them may carry
/// a partial result and two cannot — which upstream states with `result?: R` on some arms and not on
/// others, and which here is simply the fields each variant has.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum TaskOutcome {
    /// It finished.
    Completed {
        /// Its result. Opaque to storage.
        result: DocValue,
    },
    /// It failed.
    Failed {
        /// Why.
        error: TaskOutcomeError,
        /// A partial result, if it produced one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<DocValue>,
    },
    /// It was aborted.
    Aborted {
        /// Why, if a reason was given.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// A partial result, if it produced one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<DocValue>,
    },
    /// Its owner disappeared.
    Orphaned {
        /// Why.
        reason: String,
    },
    /// It faulted: a failure of the machinery rather than of the work.
    Faulted {
        /// Why.
        error: TaskOutcomeError,
    },
}

/// Why a task failed or faulted (`spec.md:34`).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct TaskOutcomeError {
    /// The message.
    pub message: String,
    /// Structured detail. Opaque to storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<DocValue>,
}

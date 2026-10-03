//! Submission records (`spec.md:177-237`).

use cyrup_pico_doc::DocValue;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, EntryId, SubmissionId};

/// A queued request against a conversation, and how it settled.
///
/// Upstream this is a two-by-four union written with eleven `?: never` fields — the TypeScript idiom
/// for *"this field does not exist in this state"*. Every one of those `never`s is a state that cannot
/// be built here, because the states are [`SubmissionState`]'s variants and each carries exactly its
/// own fields. There is no runtime check to delete, because there was never a Rust version with one.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SubmissionRecord {
    /// This submission.
    pub id: SubmissionId,
    /// The conversation it was submitted to.
    pub conversation_id: ConversationId,
    /// The caller's idempotency key, if it supplied one.
    ///
    /// [`Storage::submission_by_request`] is the indexed lookup over it (`spec.md:4314`).
    ///
    /// [`Storage::submission_by_request`]: crate::Storage::submission_by_request
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// What it is, and where it got to.
    #[serde(flatten)]
    pub state: SubmissionState,
}

impl SubmissionRecord {
    /// Whether this is an input or a write submission, for [`SubmissionQuery`].
    ///
    /// [`SubmissionQuery`]: crate::SubmissionQuery
    #[must_use]
    pub const fn submission_type(&self) -> SubmissionType {
        match self.state {
            SubmissionState::Input(_) => SubmissionType::Input,
            SubmissionState::Write(_) => SubmissionType::Write,
        }
    }

    /// This submission's status, for [`SubmissionQuery`]'s status filter.
    ///
    /// [`SubmissionQuery`]: crate::SubmissionQuery
    #[must_use]
    pub const fn status(&self) -> SubmissionStatus {
        match &self.state {
            SubmissionState::Input(s) => s.status(),
            SubmissionState::Write(s) => s.status(),
        }
    }
}

/// Which kind of submission, and its state.
///
/// The two kinds have **different state sets** — an input can be `placed` and a write cannot — which
/// is why this is one enum over two state enums rather than a kind tag beside a shared status.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SubmissionState {
    /// A turn's input.
    Input(InputSubmission),
    /// A write into the transcript.
    Write(WriteSubmission),
}

/// An input submission's state (`spec.md:187-215`).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum InputSubmission {
    /// Accepted, not yet placed in the transcript.
    Queued,
    /// Placed as an entry.
    Placed {
        /// The entry it became.
        entry: EntryId,
    },
    /// Placed and answered.
    Done {
        /// The entry it became.
        entry: EntryId,
        /// The entry that answered it.
        answer: EntryId,
    },
    /// Settled without an answer.
    Unanswered {
        /// The entry it became, if it was placed before settling.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<EntryId>,
        /// Why.
        reason: String,
        /// Structured detail. Opaque to storage.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<DocValue>,
    },
}

impl InputSubmission {
    /// This state's tag.
    #[must_use]
    pub const fn status(&self) -> SubmissionStatus {
        match self {
            Self::Queued => SubmissionStatus::Queued,
            Self::Placed { .. } => SubmissionStatus::Placed,
            Self::Done { .. } => SubmissionStatus::Done,
            Self::Unanswered { .. } => SubmissionStatus::Unanswered,
        }
    }
}

/// A write submission's state (`spec.md:217-236`).
///
/// Three states, not four: a write is never `placed`, because placing it *is* finishing it.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum WriteSubmission {
    /// Accepted, not yet written.
    Queued,
    /// Written.
    Done {
        /// The entry it became.
        entry: EntryId,
    },
    /// Settled without being written.
    Unanswered {
        /// Why.
        reason: String,
        /// Structured detail. Opaque to storage.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<DocValue>,
    },
}

impl WriteSubmission {
    /// This state's tag.
    #[must_use]
    pub const fn status(&self) -> SubmissionStatus {
        match self {
            Self::Queued => SubmissionStatus::Queued,
            Self::Done { .. } => SubmissionStatus::Done,
            Self::Unanswered { .. } => SubmissionStatus::Unanswered,
        }
    }
}

/// A submission's status tag, as [`SubmissionQuery`] filters on it (`spec.md:4220`).
///
/// [`SubmissionQuery`]: crate::SubmissionQuery
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubmissionStatus {
    /// Accepted, not yet acted on.
    Queued,
    /// Placed in the transcript. Input submissions only.
    Placed,
    /// Finished.
    Done,
    /// Settled without an answer.
    Unanswered,
}

/// Which kind of submission.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubmissionType {
    /// A turn's input.
    Input,
    /// A write into the transcript.
    Write,
}

//! Conversation records (`spec.md:112-123`).

use serde::{Deserialize, Serialize};

use crate::{ConversationId, EntryId, TaskId};

/// A conversation: a transcript scope, optionally forked from another and optionally owned by a task.
///
/// `spec.md:280-285` keeps the two relations apart, and so does this struct: *"`parent` controls
/// inherited entries and historical documents. `owner` records task attribution and connects scopes
/// for subtree abort and idle traversal. It is not an access-control capability."* The second
/// sentence is prose here exactly as it is upstream — nothing in this type enforces it, and
/// ADR-0030 §2.4 says so rather than pretending otherwise.
///
/// Creation is immutable (`spec.md:4274`): a second write of the same id is
/// [`RejectedReason::ConversationExists`].
///
/// [`RejectedReason::ConversationExists`]: crate::RejectedReason::ConversationExists
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ConversationRecord {
    /// This conversation.
    pub id: ConversationId,
    /// The conversation this one forked from, and the entry it forked at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ConversationParent>,
    /// The task that owns this conversation, with that task's conversation.
    ///
    /// `spec.md:286-288`: *"Creation always supplies `ConversationOwnership` explicitly. The Session
    /// derives the persisted owner's conversation from the named task; callers never construct the
    /// persisted owner pair."* This is the **persisted** pair, so it is the derived one — the
    /// caller-facing `ConversationOwnership` two-variant enum belongs to `cyrup-pico` (PICO5-PLAN
    /// S4), which is the layer that holds the task index to derive from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ConversationOwner>,
}

/// Where a forked conversation came from.
///
/// `spec.md:258-259`: *"`parent.at` is an entry in the parent history visible to the child."* That
/// visibility is a cross-record fact and stays the Session's to establish (`spec.md:4272-4274`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ConversationParent {
    /// The parent conversation.
    pub conversation_id: ConversationId,
    /// The entry in the parent's visible history that the fork points at.
    pub at: EntryId,
}

/// The task that owns a conversation, and that task's own conversation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ConversationOwner {
    /// The owning task's conversation.
    pub conversation_id: ConversationId,
    /// The owning task. `spec.md:252`: it stays recorded after that task becomes terminal.
    pub task_id: TaskId,
}

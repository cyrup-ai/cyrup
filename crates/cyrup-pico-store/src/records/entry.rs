//! Entry records (`spec.md:165-175`, `:255-279`).

use cyrup_pico_doc::DocValue;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, EntryId, Kind, TaskId};

/// An immutable transcript record.
///
/// `spec.md:69` (invariant 5): *"Entries and IDs are immutable and never reused after a committed
/// write."* The storage half of that is enforced: a second write of the same id is
/// [`RejectedReason::EntryExists`]. The in-process half is [`crate::Id`]'s — no public constructor,
/// so a number cannot be turned back into an entry id.
///
/// [`RejectedReason::EntryExists`]: crate::RejectedReason::EntryExists
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct EntryRecord {
    /// This entry. Session-global and ordered (`spec.md:257`).
    pub id: EntryId,
    /// The conversation it belongs to.
    pub conversation_id: ConversationId,
    /// What kind of entry it is.
    pub kind: Kind,
    /// The provider-facing messages, if any.
    ///
    /// Opaque to storage — see this module's parent documentation for why the type is [`DocValue`]
    /// and not a `cyrup-core` message. `spec.md:296` excludes model-less entries from provider
    /// requests, which is a Session rule over this field's absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Vec<DocValue>>,
    /// The entry's application payload. Opaque to storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<DocValue>,
    /// This entry's head marker: the lower bound of the active transcript it establishes.
    ///
    /// `spec.md:261-262`: *"A head on an entry changes subsequent context; it does not remove older
    /// entries from storage."* [`Storage::find_latest_head_marker`] is the indexed path that finds
    /// the newest one at or before a cutoff.
    ///
    /// [`Storage::find_latest_head_marker`]: crate::Storage::find_latest_head_marker
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<EntryId>,
    /// Context edits this entry contributes (`spec.md:157-163`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<Vec<ContextEdit>>,
    /// The task that wrote this entry, for attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_task_id: Option<TaskId>,
}

/// An edit applied to an earlier entry's contribution to model context (`spec.md:157-163`).
///
/// Upstream this is an intersection with a two-arm union whose arms use `messages?: never` to say
/// *"an omit carries no messages"*. Here the arms are an enum, so the illegal pair — an `omit` with
/// messages, or a `replace` without them — has no spelling, in memory or on the wire.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ContextEdit {
    /// The entry whose contribution is edited.
    pub target: EntryId,
    /// What to do with it.
    #[serde(flatten)]
    pub action: EditAction,
}

/// What a [`ContextEdit`] does.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum EditAction {
    /// Contribute no model messages for the target.
    Omit,
    /// Contribute these messages instead of the target's.
    Replace {
        /// The replacement messages. Opaque to storage.
        messages: Vec<DocValue>,
    },
}

/// What [`Storage::find_latest_head_marker`] found.
///
/// `spec.md:4331-4334`: *"returns the newest visible entry carrying `head` at or below its optional
/// inclusive cutoff. The returned entry **is** the marker; its `head` value is the actual lower bound
/// for context."* Upstream the return type is `EntryRecord & { head: EntryId }` — an intersection that
/// narrows an optional field to a required one. Here it is a struct with the bound beside the record,
/// so a caller cannot reach the marker without also having the bound, and no code path has to
/// re-check that `head` is present.
///
/// # A deliberate departure from ADR-0030 §10's sketch
///
/// §10 writes the signature as `find_latest_head_marker(..) -> Result<Option<EntryId>, StorageFailure>`
/// — one id. That is lossy, and `spec.md:265-271`'s context derivation says why: step 5 is *"if `H`
/// exists, context entries are `H` followed by non-head entries in the range"*, so the caller needs both
/// the marker's **identity** and its **`head`**, and one `EntryId` cannot be both. Returning the pair is
/// what `spec.md:4331` specifies (`EntryRecord & { head: EntryId }`) and is the shape implemented here.
///
/// [`Storage::find_latest_head_marker`]: crate::Storage::find_latest_head_marker
#[derive(Clone, PartialEq, Debug)]
pub struct HeadMarker {
    /// The marker entry.
    pub entry: EntryRecord,
    /// Its `head`: the lower bound for context derivation.
    pub head: EntryId,
}

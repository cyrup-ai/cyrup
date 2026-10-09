//! EXT-078 — the entries an extension appends at a run boundary (pi `SessionBoundaryDraft`,
//! `core/extensions/types.ts:943-977` @v1.1.0), the context edit they may carry
//! (`appendContextEdit`, `core/session-manager.ts:1360-1395`), and the in-memory copy pi previews
//! them on (`_createBoundaryPreviewManager`, `core/agent-session.ts:987-993`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use cyrup_core::{Content, EntryId, Usage};

use crate::agent_message::AgentMessage;
use crate::entry::{ContextEditReplacement, ContextEditableContent, Entry, KnownEntry};
use crate::error::SessionError;
use crate::store::MemStore;

use super::SessionManager;

/// One entry a `turn_end` / `agent_before_settle` handler asks to append — pi
/// `SessionBoundaryDraft`, the union of `CustomEntryDraft`, `CustomMessageEntryDraft`,
/// `ContextEditEntryDraft` and `CompactionEntryDraft`. The JSON is pi's, key for key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionBoundaryDraft {
    /// `{ type: "custom", customType, data? }` — state only, never model context.
    Custom {
        #[serde(rename = "customType")]
        custom_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    /// `{ type: "custom_message", customType, content, display, details? }` — a message that
    /// participates in model context.
    CustomMessage {
        #[serde(rename = "customType")]
        custom_type: String,
        content: ContextEditableContent,
        display: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
    /// `{ type: "context_edit", targetId, replacement }` — omit (`null`) or replace an earlier
    /// entry's contribution to model context.
    ContextEdit {
        #[serde(rename = "targetId")]
        target_id: EntryId,
        replacement: Option<ContextEditReplacement>,
    },
    /// `{ type: "compaction", summary, firstKeptEntryId, details?, usage? }` — `firstKeptEntryId:
    /// null` keeps no preceding entries.
    Compaction {
        summary: String,
        #[serde(rename = "firstKeptEntryId")]
        first_kept_entry_id: Option<EntryId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
}

impl SessionManager {
    /// pi `appendContextEdit(targetId, replacement)` (`core/session-manager.ts:1360-1395` @v1.1.0):
    /// append a branch-local edit to an earlier model-visible entry. Refused, with pi's text, when
    /// the target does not exist, is not on the active branch, or contributes no editable model
    /// content (a `custom_message`, or a `user` / `assistant` / `toolResult` message). A string
    /// replacement of an `assistant` or `toolResult` message is stored as one text block.
    pub fn append_context_edit(
        &mut self,
        target: &EntryId,
        replacement: Option<ContextEditReplacement>,
    ) -> Result<EntryId, SessionError> {
        let Some(entry) = self.entry(target) else {
            return Err(SessionError::ContextEdit(format!(
                "Entry {target} not found"
            )));
        };
        if !self.branch_path(None).iter().any(|e| &e.id() == target) {
            return Err(SessionError::ContextEdit(format!(
                "Entry {target} is not on the active branch"
            )));
        }
        let normalized_to_blocks = match entry {
            Entry::Known(KnownEntry::CustomMessage { .. }) => false,
            Entry::Known(KnownEntry::Message {
                message: AgentMessage::Core(message),
                ..
            }) => match message {
                cyrup_core::Message::User { .. } => false,
                cyrup_core::Message::Assistant(_) | cyrup_core::Message::ToolResult { .. } => true,
                _ => {
                    return Err(not_editable(target));
                }
            },
            _ => return Err(not_editable(target)),
        };
        let replacement = replacement.map(|r| match r.content {
            ContextEditableContent::Text(text) if normalized_to_blocks => ContextEditReplacement {
                content: ContextEditableContent::Blocks(vec![Content::text(text)]),
            },
            content => ContextEditReplacement { content },
        });
        self.push_entry(Entry::known(KnownEntry::ContextEdit {
            base: self.make_base(),
            target_id: target.clone(),
            replacement,
        }))
    }

    /// pi `_applyBoundaryDrafts(manager, drafts)` (`core/agent-session.ts:946-985` @v1.1.0): append
    /// the drafts in order and return the entries appended. A compaction draft is charged the
    /// projected context's estimate as its `tokensBefore` and is recorded as from an extension
    /// (`fromHook: true`). The first draft that cannot be appended ends the walk with its error;
    /// the drafts before it stay appended, as pi's do.
    pub fn apply_boundary_drafts(
        &mut self,
        drafts: &[SessionBoundaryDraft],
    ) -> Result<Vec<Entry>, SessionError> {
        let mut appended = Vec::with_capacity(drafts.len());
        for draft in drafts {
            let id = match draft {
                SessionBoundaryDraft::Custom { custom_type, data } => {
                    self.append_custom_entry(custom_type, data.clone())?
                }
                SessionBoundaryDraft::CustomMessage {
                    custom_type,
                    content,
                    display,
                    details,
                } => self.append_custom_message(
                    custom_type,
                    serde_json::to_value(content)?,
                    *display,
                    details.clone(),
                )?,
                SessionBoundaryDraft::ContextEdit {
                    target_id,
                    replacement,
                } => self.append_context_edit(target_id, replacement.clone())?,
                SessionBoundaryDraft::Compaction {
                    summary,
                    first_kept_entry_id,
                    details,
                    usage,
                } => {
                    let tokens_before = u64::from(
                        crate::compaction::tokens::estimate_projected_context_tokens(
                            &self.branch_path(None),
                        )
                        .tokens,
                    );
                    self.append_compaction_keeping(
                        summary.clone(),
                        first_kept_entry_id.clone(),
                        tokens_before,
                        details.clone(),
                        usage.clone(),
                        true,
                    )?
                }
            };
            if let Some(entry) = self.entry(&id) {
                appended.push(entry.clone());
            }
        }
        Ok(appended)
    }

    /// pi `_createBoundaryPreviewManager` (`core/agent-session.ts:987-993` @v1.1.0): an in-memory
    /// copy of this session's header and ACTIVE BRANCH, which drafts are applied to before any of
    /// them reaches the file. Nothing written to it is persisted.
    #[must_use]
    pub fn branch_preview(&self) -> SessionManager {
        let entries: Vec<Entry> = self.branch_path(None).into_iter().cloned().collect();
        Self::assemble(
            self.header.clone(),
            self.cwd.clone(),
            Box::new(MemStore),
            entries,
            false,
        )
    }

    /// pi `buildSessionProjection().entries` over the active branch: each admitted entry with the
    /// model-visible messages it contributes (see [`crate::context::build_session_projection`]).
    pub fn session_projection(&self) -> Vec<(Entry, Vec<AgentMessage>)> {
        crate::context::build_session_projection(&self.branch_path(None))
            .into_iter()
            .map(|(entry, messages)| (entry.clone(), messages))
            .collect()
    }
}

fn not_editable(target: &EntryId) -> SessionError {
    SessionError::ContextEdit(format!(
        "Entry {target} does not contribute editable model content"
    ))
}

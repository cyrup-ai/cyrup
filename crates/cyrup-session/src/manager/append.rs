//! The typed entry constructors (R-04-016) — each wraps a [`KnownEntry`] variant and hands it to
//! `push_entry`, the single write path in [`super`].

use serde_json::Value;

use cyrup_core::{EntryId, Message, ModelId, ProviderId, Usage};

use crate::agent_message::AgentMessage;
use crate::entry::{Entry, KnownEntry};
use crate::error::SessionError;

use super::SessionManager;

impl SessionManager {
    /// Append a core `user`/`assistant`/`toolResult` message (Pi `appendMessage`,
    /// `session-manager.ts:954`). Backward-compatible: callers still pass a [`cyrup_core::Message`].
    pub fn append_message(&mut self, message: Message) -> Result<EntryId, SessionError> {
        self.append_agent_message(AgentMessage::Core(message))
    }

    /// Append any Pi `AgentMessage` (including the `bashExecution`/`custom` roles) inside a
    /// `type:"message"` entry (Pi `appendMessage(Message | CustomMessage | BashExecutionMessage)`,
    /// `session-manager.ts:954`).
    pub fn append_agent_message(
        &mut self,
        message: AgentMessage,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::Message { base: self.make_base(), message }))
    }

    pub fn append_model_change(
        &mut self,
        provider: ProviderId,
        model_id: ModelId,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::ModelChange {
            base: self.make_base(),
            provider,
            model_id,
        }))
    }

    pub fn append_thinking_level_change(
        &mut self,
        level: &str,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::ThinkingLevelChange {
            base: self.make_base(),
            thinking_level: level.to_string(),
        }))
    }

    /// Append a compaction entry (Pi `appendCompaction(summary, firstKeptEntryId, tokensBefore,
    /// details, fromHook, usage)`, `session-manager.ts:1096-1116`). `usage` is the token spend of
    /// the summarization call(s); `None` when the summary came from a hook that reported none.
    pub fn append_compaction(
        &mut self,
        summary: String,
        first_kept: EntryId,
        tokens_before: u64,
        details: Option<Value>,
        usage: Option<Usage>,
        from_hook: bool,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::Compaction {
            base: self.make_base(),
            summary,
            first_kept_entry_id: Some(first_kept),
            tokens_before,
            details,
            usage,
            from_hook: Some(from_hook),
        }))
    }

    pub fn append_custom_entry(
        &mut self,
        ty: &str,
        data: Option<Value>,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::Custom {
            base: self.make_base(),
            custom_type: ty.to_string(),
            data,
        }))
    }

    pub fn append_custom_message(
        &mut self,
        ty: &str,
        content: Value,
        display: bool,
        details: Option<Value>,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::CustomMessage {
            base: self.make_base(),
            custom_type: ty.to_string(),
            content,
            display,
            details,
        }))
    }

    pub fn append_session_info(&mut self, name: &str) -> Result<EntryId, SessionError> {
        // Pi sanitizes on write: `name.replace(/[\r\n]+/g, " ").trim()` (`session-manager.ts:1031`),
        // so newlines never corrupt the JSONL line and the persisted bytes match Pi.
        self.push_entry(Entry::known(KnownEntry::SessionInfo {
            base: self.make_base(),
            name: Some(sanitize_session_name(name)),
        }))
    }

    pub fn append_label(
        &mut self,
        target: &EntryId,
        label: Option<&str>,
    ) -> Result<EntryId, SessionError> {
        if !self.by_id.contains_key(target) {
            return Err(SessionError::EntryNotFound(target.clone()));
        }
        self.push_entry(Entry::known(KnownEntry::Label {
            base: self.make_base(),
            target_id: target.clone(),
            label: label.map(str::to_string),
        }))
    }
}

/// Pi name sanitization for `appendSessionInfo`: collapse any run of `\r`/`\n` to a single space,
/// then trim (`session-manager.ts:1031`).
fn sanitize_session_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut in_newline_run = false;
    for ch in name.chars() {
        if ch == '\r' || ch == '\n' {
            if !in_newline_run {
                out.push(' ');
                in_newline_run = true;
            }
        } else {
            out.push(ch);
            in_newline_run = false;
        }
    }
    out.trim().to_string()
}

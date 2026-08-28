//! The manager-side context projection (R-04-011/012/013): walk the active path and render it for
//! the model or for a UI replay.
//!
//! Distinct from [`crate::context`], the pure builder this delegates to — that module turns a
//! borrowed path into messages; this one supplies the path and the model/thinking state carried
//! alongside it.

use cyrup_core::{EntryId, Message, ModelRef};

use crate::agent_message::AgentMessage;
use crate::context::{
    build_context_agent_messages_tagged, build_context_messages, SessionContext,
};
use crate::entry::{Entry, KnownEntry};

use super::SessionManager;

impl SessionManager {
    pub fn build_context(&self) -> SessionContext {
        let path = self.branch_path(None);
        if path.is_empty() {
            return SessionContext::empty();
        }

        let mut thinking = "off".to_string();
        let mut model: Option<ModelRef> = None;
        for e in &path {
            if let Entry::Known(k) = e {
                match k {
                    KnownEntry::ThinkingLevelChange { thinking_level, .. } => {
                        thinking = thinking_level.clone();
                    }
                    KnownEntry::ModelChange { provider, model_id, .. } => {
                        model = Some(ModelRef {
                            provider: provider.clone(),
                            api: None,
                            model: model_id.clone(),
                        });
                    }
                    KnownEntry::Message { message: AgentMessage::Core(Message::Assistant(a)), .. } => {
                        model = Some(a.model_ref());
                    }
                    _ => {}
                }
            }
        }

        let messages = build_context_messages(&path);
        SessionContext { messages, thinking_level: thinking, model }
    }

    /// The active-path context with its **roles intact** — Pi's
    /// `buildContextEntries().flatMap(sessionEntryToContextMessages)` (`session-manager.ts:441-453`
    /// composed with `:383-408`), i.e. [`build_context`](Self::build_context) *without* the
    /// `convertToLlm` flattening.
    ///
    /// [`build_context`](Self::build_context) is the LLM boundary: it renders a `compaction`,
    /// `branch_summary`, `custom_message` or `bashExecution` entry down to a `user` message carrying
    /// the wrapper prose the model conditions on. A UI that replays a resumed session must NOT see
    /// that flattening — Pi's `renderSessionEntries` feeds the raw projection so each role still
    /// reaches its own component (`CompactionSummaryMessageComponent`, `BranchSummaryMessageComponent`,
    /// `CustomMessageComponent`, `BashExecutionComponent`; interactive-mode.ts:3506-3516, :3308-3350).
    /// This is that projection.
    ///
    /// Note a `!!`-prefixed (`excludeFromContext`) bash message is PRESENT here and absent from
    /// [`build_context`](Self::build_context) — Pi's raw context keeps it too (`messages.ts:153-155`
    /// drops it only in `convertToLlm`), which is why the user still sees their own `!!` command
    /// after a resume.
    pub fn build_context_raw(&self) -> Vec<AgentMessage> {
        self.build_context_raw_tagged().into_iter().map(|(_, m)| m).collect()
    }

    /// [`build_context_raw`](Self::build_context_raw), with each message paired with the
    /// [`EntryId`] of the entry it was projected from — see
    /// [`build_context_agent_messages_tagged`] for why the pairing exists and what it is a stand-in
    /// for. `build_context_raw` is this with the ids dropped, so the two cannot drift.
    pub fn build_context_raw_tagged(&self) -> Vec<(EntryId, AgentMessage)> {
        let path = self.branch_path(None);
        if path.is_empty() {
            return Vec::new();
        }
        build_context_agent_messages_tagged(&path)
    }
}

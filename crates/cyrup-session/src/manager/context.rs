//! The manager-side context projection (R-04-011/012/013): walk the active path and render it for
//! the model or for a UI replay.
//!
//! Distinct from [`crate::context`], the pure builder this delegates to — that module turns a
//! borrowed path into messages; this one supplies the path and the model/thinking state carried
//! alongside it.

use cyrup_core::{EntryId, Message, ModelRef};

use crate::agent_message::AgentMessage;
use crate::compaction::tokens::{ContextUsageEstimate, estimate_projected_context_tokens};
use crate::context::{SessionContext, build_context_agent_messages_tagged, build_context_messages};
use crate::entry::{Entry, KnownEntry};

use super::SessionManager;

impl SessionManager {
    /// The projection's own model/thinking pass — Pi `getSessionContextSettings`
    /// (`session-manager.ts:418-433`).
    ///
    /// # This is NOT the virtual-aware selection (`SESS-067`)
    ///
    /// The `model` this records is a FORWARD last-wins scan, so under a virtual selection it names
    /// the physical model that answered last, not the router the user selected. That is upstream's
    /// shape and is deliberately left alone: `getSessionContextSettings`'s body is **byte-identical
    /// at v0.87.1, v1.0.0 and v1.0.4** (`sha256 1f011ecc8400ee6a…` over `:418-433` at all three),
    /// i.e. the virtual-models feature did not touch it, and at v1.0.4 `existingSession.model` has
    /// no reader anywhere in `packages/coding-agent/src` — `sdk.ts` reads only `.messages` (`:201`)
    /// and `.thinkingLevel` (`:248`).
    ///
    /// Upstream applies the hold rule in exactly two places, both against the RAW branch rather
    /// than this projection: the restore step (`sdk.ts:207-222`) and `_recordSelection`
    /// (`agent-session.ts:609-625`). A caller that wants the selection asks
    /// [`Self::branch_selection`]; see [`SessionContext::model`] for the same note at the field.
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
                    // Last wins, FORWARD — including over a virtual `model_change` that the
                    // assistant arm below then overwrites. See the note on `build_context`: this
                    // is the projection value, not the selection.
                    KnownEntry::ModelChange {
                        provider, model_id, ..
                    } => {
                        model = Some(ModelRef {
                            provider: provider.clone(),
                            api: None,
                            model: model_id.clone(),
                        });
                    }
                    // Every assistant message counts here, including one left by FAILED ROUTING
                    // (which names the virtual model, `virtual-models.ts:104`).
                    // [`crate::virtual_models::branch_selection`] skips those; this pass does not,
                    // because upstream's does not.
                    KnownEntry::Message {
                        message: AgentMessage::Core(Message::Assistant(a)),
                        ..
                    } => {
                        model = Some(a.model_ref());
                    }
                    _ => {}
                }
            }
        }

        let messages = build_context_messages(&path);
        SessionContext {
            messages,
            thinking_level: thinking,
            model,
        }
    }

    /// The model selection the active branch records — Pi `getBranchSelection(
    /// sessionManager.getBranch(), getModel)` (`sdk.ts:209-211`, `agent-session.ts:620`).
    ///
    /// This is the virtual-aware answer and the one a RESTORE or a selection check wants:
    /// a virtual `model_change` HOLDS across the physical responses it routed to, where
    /// [`build_context`](Self::build_context)'s forward pass would report the model that answered.
    /// See [`crate::virtual_models::branch_selection`] for the rule and for why `get_model` need
    /// only answer for virtual entries.
    ///
    /// Exists so a caller does not have to pair `branch_path(None)` with the free function itself:
    /// the branch path is borrowed state only the manager holds, exactly as it is for
    /// [`Self::projected_context_estimate`].
    #[must_use]
    pub fn branch_selection(
        &self,
        get_model: impl Fn(&str, &str) -> Option<ModelRef>,
    ) -> Option<ModelRef> {
        crate::virtual_models::branch_selection(&self.branch_path(None), get_model)
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
        self.build_context_raw_tagged()
            .into_iter()
            .map(|(_, m)| m)
            .collect()
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

    /// The live context estimate every compaction trigger must read — Pi
    /// `estimateProjectedContextTokens(this.buildSessionProjection(), this.getBranch())`
    /// (`agent-session.ts:594-600`, `:758`, `:2708`, `:3893` @v0.87.1).
    ///
    /// Exists so a service-layer trigger does not have to reassemble the `(projection, branch)` pair
    /// itself: the branch path is borrowed state only the manager holds, and passing just the
    /// projected messages — as the callers did before SESS-052 — structurally cannot apply the
    /// edit/compaction invalidation rule, because the rule is a scan over BRANCH ENTRIES.
    pub fn projected_context_estimate(&self) -> ContextUsageEstimate {
        let path = self.branch_path(None);
        if path.is_empty() {
            return ContextUsageEstimate::default();
        }
        estimate_projected_context_tokens(&path)
    }

    /// The current branch's ENTRIES with compaction applied — Pi
    /// `SessionManager.buildContextEntries()` (`session-manager.ts:1277-1279` @v0.84.4, delegating
    /// to the free function at `:418-453`), the input its interactive host replays from
    /// (`renderSessionEntries(this.sessionManager.buildContextEntries())`,
    /// `modes/interactive/interactive-mode.ts:3910`).
    ///
    /// [`build_context_raw`](Self::build_context_raw) is this list flat-mapped through the raw
    /// projection. A caller wants THIS one when it needs the entries that project no message —
    /// `custom` entries, which a front-end replays through its registered entry renderer (EXT-041).
    pub fn context_entries(&self) -> Vec<&Entry> {
        let path = self.branch_path(None);
        if path.is_empty() {
            return Vec::new();
        }
        crate::context::build_context_entries(&path)
    }
}

//! EXT-078 — the run boundaries extensions can append entries at and continue from: `turn_end`
//! and `agent_before_settle` (pi v0.87.0; `core/agent-session.ts` @v1.1.0).
//!
//! At either boundary the extension chain is handed the drafts so far, whether one more provider
//! request is wanted, and a PREVIEW of the model context with the drafts appended — built on an
//! in-memory copy of the branch, so nothing reaches the session file until the chain is done
//! (`_buildBoundaryContext`, `:999-1022`). The drafts the chain leaves are then committed in order
//! and announced as `entry_appended` (`_commitBoundaryDrafts`, `:1024-1028`), and a continuation is
//! honoured only when the context can be continued from (`_reportInvalidBoundaryContinuation`,
//! `:1030-1036`).
//!
//! `turn_end` is dispatched from the agent's `finishTurn` (`_installAgentBoundaryHooks`,
//! `:885-895`), so its `continue` reaches the loop as [`cyrup_agent::TurnDecision::Continue`]; a
//! turn the loop ended without `finishTurn` (a run failure) is dispatched from its `turn_end`
//! event instead (`_emitExtensionEvent`, `:1297-1300`). `agent_before_settle` runs in the post-run
//! loop once nothing else will continue the run (`_runBeforeSettleBoundary`, `:1893-1915`).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cyrup_agent::{AgentMessage, ToolResultMessage};
use cyrup_core::{AssistantMessage, Message, StopReason};
use cyrup_ext::{BoundaryState, ExtensionError, HostEvent};
use cyrup_session::{Entry, KnownEntry, SessionBoundaryDraft, SessionManager};
use serde_json::{Value, json};

use super::AgentSession;
use crate::event::AgentSessionEvent;

/// The two boundaries (pi `"turn_end" | "agent_before_settle"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Boundary {
    TurnEnd,
    BeforeSettle,
}

impl Boundary {
    fn name(self) -> &'static str {
        match self {
            Boundary::TurnEnd => "turn_end",
            Boundary::BeforeSettle => "agent_before_settle",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "turn_end" => Some(Boundary::TurnEnd),
            "agent_before_settle" => Some(Boundary::BeforeSettle),
            _ => None,
        }
    }
}

/// What the preview needs from the session, read once per dispatch: the branch, the messages
/// waiting to be delivered, and whether the agent has queued ones (pi reads these live; nothing a
/// boundary handler does through the host changes them before the chain ends).
struct PreviewBase {
    branch: std::sync::Mutex<SessionManager>,
    pending: Vec<AgentMessage>,
    pending_custom: bool,
    has_queued: bool,
}

impl PreviewBase {
    /// pi `_buildBoundaryContext(drafts, boundary)`.
    fn context(
        &self,
        drafts: &[SessionBoundaryDraft],
        boundary: Boundary,
    ) -> Result<Value, String> {
        let mut preview = self
            .branch
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .branch_preview();
        preview
            .apply_boundary_drafts(drafts)
            .map_err(|e| e.to_string())?;
        let projection = preview.session_projection();
        let messages: Vec<cyrup_session::AgentMessage> = projection
            .iter()
            .flat_map(|(_, messages)| messages.iter().cloned())
            .collect();
        let llm = cyrup_session::convert_to_llm(&messages);
        let has_non_system = llm.iter().any(|m| !matches!(m, Message::System(_)));
        let final_is_assistant = matches!(llm.last(), Some(Message::Assistant(_)));
        let context_can_continue = has_non_system && !final_is_assistant;
        let can_continue = context_can_continue
            || self.pending_custom
            || match boundary {
                Boundary::TurnEnd => self.has_queued,
                Boundary::BeforeSettle => final_is_assistant && self.has_queued,
            };
        Ok(json!({
            "contextEntries": projection
                .iter()
                .map(|(entry, messages)| json!({"sourceEntry": entry, "messages": messages}))
                .collect::<Vec<_>>(),
            "contextMessages": messages,
            "llmMessages": llm,
            "pendingMessages": self.pending,
            "canContinue": can_continue,
        }))
    }

    fn context_json(&self, entries: &Value, boundary: Boundary) -> Result<Value, String> {
        let drafts = parse_drafts(entries)?;
        self.context(&drafts, boundary)
    }
}

/// The drafts a handler returned, as pi's `SessionBoundaryDraft[]`.
fn parse_drafts(entries: &Value) -> Result<Vec<SessionBoundaryDraft>, String> {
    serde_json::from_value(entries.clone()).map_err(|e| e.to_string())
}

/// pi `AgentActivityOutcome` of a finished assistant message (`_dispatchTurnEndBoundary`,
/// `agent-session.ts:850-851`).
fn outcome_of(message: &AssistantMessage) -> &'static str {
    match message.stop_reason {
        StopReason::Aborted => "aborted",
        StopReason::Error => "error",
        _ => "completed",
    }
}

impl AgentSession {
    /// Report a boundary fault the session itself found (pi `emitError({extensionPath:
    /// "<boundary>", …})`).
    fn report_boundary_error(&self, boundary: Boundary, error: String) {
        self.services
            .ext_host
            .dispatcher()
            .report_external(ExtensionError {
                extension: cyrup_core::ExtensionId::from("<boundary>"),
                event: boundary.name(),
                error,
            });
    }

    /// Snapshot what the preview reads.
    async fn preview_base(&self) -> Arc<PreviewBase> {
        let branch = self.manager.lock().await.branch_preview();
        let pending_custom: Vec<AgentMessage> = Self::lock(&self.pending_custom_messages).clone();
        let mut pending = self.agent.peek_queued_messages();
        let pending_custom_any = !pending_custom.is_empty();
        pending.extend(pending_custom);
        Arc::new(PreviewBase {
            branch: std::sync::Mutex::new(branch),
            pending,
            pending_custom: pending_custom_any,
            has_queued: self.agent.has_queued_messages(),
        })
    }

    /// pi `emitBoundary` + `_commitBoundaryDrafts` + the continuation check: run the chain, commit
    /// what it left, and answer whether one more provider request should run.
    async fn run_boundary(&self, event: HostEvent, boundary: Boundary) -> (bool, bool) {
        let base = self.preview_base().await;
        let host_preview = {
            let base = Arc::clone(&base);
            move |entries: &Value| base.context_json(entries, boundary)
        };
        // A guest that folds several handlers previews through the same copy (EXT-078's
        // `preview-boundary` import).
        {
            let base = Arc::clone(&base);
            self.services
                .host_services
                .set_boundary_preview(Some(Arc::new(move |name: &str, entries: &Value| {
                    let boundary = Boundary::parse(name)
                        .ok_or_else(|| format!("not a boundary event: {name}"))?;
                    base.context_json(entries, boundary)
                })));
        }
        let cancel = self.session_cancel.child_token();
        let result = self
            .services
            .ext_host
            .dispatcher()
            .dispatch_boundary(event, &host_preview, &cancel)
            .await;
        self.services.host_services.set_boundary_preview(None);
        let drafts = parse_drafts(&result.entries).unwrap_or_default();
        self.commit_boundary_drafts(&drafts).await;
        (
            result.continue_,
            self.can_continue_after_commit(boundary).await,
        )
    }

    /// pi `_buildBoundaryContext([], boundary).canContinue` over the committed session.
    async fn can_continue_after_commit(&self, boundary: Boundary) -> bool {
        self.preview_base()
            .await
            .context(&[], boundary)
            .ok()
            .and_then(|c| c.get("canContinue").and_then(Value::as_bool))
            .unwrap_or(false)
    }

    /// pi `_commitBoundaryDrafts(drafts)` (`agent-session.ts:1024-1028` @v1.1.0): append the drafts
    /// to the session, refresh the model context from it, and announce each entry.
    ///
    /// pi refreshes `agent.state.messages` from the projection. An agent loop in flight keeps its
    /// own copy of the transcript, so a commit made while it runs (`turn_end`) is handed to it at
    /// its next turn boundary instead (`PolicyHooks::prepare_next_turn`, the same route an in-run
    /// compaction takes); once the loop has ended (`agent_before_settle`) the agent's transcript is
    /// replaced directly, and the continuation runs on it.
    async fn commit_boundary_drafts(&self, drafts: &[SessionBoundaryDraft]) {
        if drafts.is_empty() {
            return;
        }
        let (appended, context) = {
            let mut manager = self.manager.lock().await;
            let appended = match manager.apply_boundary_drafts(drafts) {
                Ok(appended) => appended,
                Err(e) => {
                    tracing::warn!(error = %e, "a boundary draft could not be committed");
                    Vec::new()
                }
            };
            let context: Vec<AgentMessage> = manager
                .build_context_raw()
                .iter()
                .map(crate::event::raw_message_to_agent)
                .collect();
            (appended, context)
        };
        if appended.is_empty() {
            return;
        }
        if appended.iter().any(changes_context) {
            if self.agent.is_running() {
                self.boundary_context_dirty.store(true, Ordering::SeqCst);
            } else {
                self.agent.set_messages(context).await;
            }
        }
        for entry in appended {
            let entry = serde_json::to_value(&entry).unwrap_or(Value::Null);
            self.fanout_emit(AgentSessionEvent::EntryAppended { entry })
                .await;
        }
    }

    /// The transcript a boundary commit changed, for `PolicyHooks::prepare_next_turn` to hand to
    /// the running loop; `None` when no commit since the last call changed it.
    pub(crate) async fn take_boundary_context(&self) -> Option<Vec<AgentMessage>> {
        if !self.boundary_context_dirty.swap(false, Ordering::SeqCst) {
            return None;
        }
        Some(
            self.manager
                .lock()
                .await
                .build_context_raw()
                .iter()
                .map(crate::event::raw_message_to_agent)
                .collect(),
        )
    }

    /// pi `_dispatchTurnEndBoundary(message, toolResults)` (`agent-session.ts:846-883` @v1.1.0):
    /// whether the `turn_end` chain asked for — and the context allows — one more provider request.
    pub(crate) async fn dispatch_turn_end_boundary(
        &self,
        message: &AssistantMessage,
        tool_results: &[ToolResultMessage],
    ) -> bool {
        self.turn_end_dispatched.store(true, Ordering::SeqCst);
        let outcome = outcome_of(message);
        *Self::lock(&self.last_activity_outcome) = outcome;
        let (message_entry_id, tool_result_entry_ids) = {
            let manager = self.manager.lock().await;
            let branch = manager.branch_path(None);
            let message_entry_id = branch.iter().rev().find_map(|e| match e {
                Entry::Known(KnownEntry::Message {
                    message: cyrup_session::AgentMessage::Core(Message::Assistant(a)),
                    ..
                }) if a == message => Some(e.id().to_string()),
                _ => None,
            });
            let tool_result_entry_ids: Vec<String> = tool_results
                .iter()
                .filter_map(|result| {
                    branch.iter().rev().find_map(|e| match e {
                        Entry::Known(KnownEntry::Message {
                            message:
                                cyrup_session::AgentMessage::Core(Message::ToolResult {
                                    tool_call_id,
                                    ..
                                }),
                            ..
                        }) if *tool_call_id == result.tool_call_id => Some(e.id().to_string()),
                        _ => None,
                    })
                })
                .collect();
            (message_entry_id, tool_result_entry_ids)
        };
        if self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::TurnEnd)
        {
            return false;
        }
        let Some(message_entry_id) = message_entry_id else {
            self.report_boundary_error(
                Boundary::TurnEnd,
                "turn_end could not resolve the persisted assistant entry ID".into(),
            );
            return false;
        };
        let event = HostEvent::TurnEnd {
            turn_index: self.turn_index.load(Ordering::SeqCst),
            message: AgentMessage::Assistant(Arc::new(message.clone())),
            tool_results: tool_results.to_vec(),
            message_entry_id,
            tool_result_entry_ids,
            boundary: BoundaryState {
                outcome: outcome.to_string(),
                ..BoundaryState::default()
            },
        };
        let (wants, can) = self.run_boundary(event, Boundary::TurnEnd).await;
        if wants && !can {
            self.report_invalid_continuation(Boundary::TurnEnd);
            return false;
        }
        wants
    }

    /// pi `_reportInvalidBoundaryContinuation(event)` (`agent-session.ts:1030-1036` @v1.1.0).
    fn report_invalid_continuation(&self, boundary: Boundary) {
        self.report_boundary_error(
            boundary,
            format!(
                "{} requested continuation without runnable model context",
                boundary.name()
            ),
        );
    }

    /// The `turn_end` event's half of pi's dispatch (`_emitExtensionEvent`, `agent-session.ts:
    /// 1297-1300` @v1.1.0): a turn whose `finishTurn` did not run — the loop ended it on a failure —
    /// is dispatched here; then the turn index advances.
    pub(crate) async fn on_turn_end_event(
        &self,
        message: &AgentMessage,
        tool_results: &[ToolResultMessage],
    ) {
        let dispatched = self.turn_end_dispatched.swap(false, Ordering::SeqCst);
        if !dispatched && let AgentMessage::Assistant(assistant) = message {
            self.dispatch_turn_end_boundary(assistant, tool_results)
                .await;
            self.turn_end_dispatched.store(false, Ordering::SeqCst);
        }
        self.turn_index.fetch_add(1, Ordering::SeqCst);
    }

    /// pi `_emitExtensionEvent`'s `agent_start` arm: `this._turnIndex = 0`.
    pub(crate) fn on_agent_start_event(&self) {
        self.turn_index.store(0, Ordering::SeqCst);
        self.turn_end_dispatched.store(false, Ordering::SeqCst);
    }

    /// pi `_runBeforeSettleBoundary()` (`agent-session.ts:1893-1915` @v1.1.0): whether the run
    /// continues for one more provider request instead of settling.
    pub(crate) async fn run_before_settle_boundary(&self) -> bool {
        if self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::AgentBeforeSettle)
        {
            return self.agent.has_queued_messages();
        }
        self.is_before_settle.store(true, Ordering::SeqCst);
        self.abort_during_before_settle
            .store(false, Ordering::SeqCst);
        let outcome = *Self::lock(&self.last_activity_outcome);
        let event = HostEvent::AgentBeforeSettle {
            boundary: BoundaryState {
                outcome: outcome.to_string(),
                ..BoundaryState::default()
            },
        };
        let (wants, _) = self.run_boundary(event, Boundary::BeforeSettle).await;
        self.flush_pending_custom_messages().await;
        let can = self.can_continue_after_commit(Boundary::BeforeSettle).await;
        self.is_before_settle.store(false, Ordering::SeqCst);
        if self.abort_during_before_settle.load(Ordering::SeqCst) {
            return false;
        }
        let should_continue = wants || self.agent.has_queued_messages();
        if should_continue && !can {
            if wants {
                self.report_invalid_continuation(Boundary::BeforeSettle);
            }
            return false;
        }
        should_continue
    }
}

/// Whether a committed entry changes what the model is sent.
fn changes_context(entry: &Entry) -> bool {
    matches!(
        entry,
        Entry::Known(
            KnownEntry::CustomMessage { .. }
                | KnownEntry::ContextEdit { .. }
                | KnownEntry::Compaction { .. }
        )
    )
}

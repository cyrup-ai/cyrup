//! In-memory agent state + the event reducer (arch-02 §4.1 / func-02 §10).

use crate::event::{AgentEvent, AgentMessage};
use cyrup_core::{ModelRef, StopReason, ThinkingLevel, Tool, ToolCallId};
use std::collections::HashSet;
use std::sync::Arc;

/// Live agent state (arch-02 §4.1). Mutated only by the loop's reducer ([`reduce`]) and the
/// `Agent` setters; the state lock is never held across a subscriber `await`.
pub struct StateInner {
    pub system_prompt: String,
    pub model: ModelRef,
    pub thinking_level: ThinkingLevel,
    pub tools: Vec<Arc<dyn Tool>>,
    pub messages: Vec<AgentMessage>,
    pub is_streaming: bool,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tool_calls: HashSet<ToolCallId>,
    pub error_message: Option<String>,
}

impl StateInner {
    pub fn snapshot(&self) -> AgentStateSnapshot {
        AgentStateSnapshot {
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            thinking_level: self.thinking_level,
            messages: self.messages.clone(),
            tool_count: self.tools.len(),
            is_streaming: self.is_streaming,
            streaming_message: self.streaming_message.clone(),
            pending_tool_calls: self.pending_tool_calls.iter().cloned().collect(),
            error_message: self.error_message.clone(),
        }
    }
}

/// A `Clone` read-only projection of state (arch-02 §4.1). Consumers never hold the state lock;
/// mutating a snapshot does NOT affect agent state.
#[derive(Clone, Debug)]
pub struct AgentStateSnapshot {
    pub system_prompt: String,
    pub model: ModelRef,
    pub thinking_level: ThinkingLevel,
    pub messages: Vec<AgentMessage>,
    pub tool_count: usize,
    pub is_streaming: bool,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tool_calls: Vec<ToolCallId>,
    pub error_message: Option<String>,
}

/// Reduce one event into managed state (arch-02 §5.1). Cheap and synchronous; called while the
/// state lock is held, BEFORE subscribers are awaited.
///
/// - `streamingMessage` holds the partial assistant message between start..end (R-02-040).
/// - the transcript grows on `message_end` (so the assistant message is visible to `before_tool_call`
///   preflight that runs after the assistant `message_end` barrier, R-02-033).
/// - `pendingToolCalls` tracks ids between tool start..end (R-02-041).
/// - `errorMessage` is set on an error/aborted turn (R-02-042).
pub fn reduce(st: &mut StateInner, ev: &AgentEvent) {
    match ev {
        AgentEvent::MessageStart { message } => {
            if message.is_assistant() {
                st.streaming_message = Some(message.clone());
            }
        }
        AgentEvent::MessageUpdate { message, .. } => {
            st.streaming_message = Some(message.clone());
        }
        AgentEvent::MessageEnd { message } => {
            st.streaming_message = None;
            st.messages.push(message.clone());
        }
        AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
            st.pending_tool_calls.insert(tool_call_id.clone());
        }
        AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
            st.pending_tool_calls.remove(tool_call_id);
        }
        AgentEvent::TurnEnd { message, .. } => {
            if let AgentMessage::Assistant(a) = message
                && matches!(a.stop_reason, StopReason::Error | StopReason::Aborted) {
                    st.error_message =
                        a.error_message.clone().or_else(|| Some("turn ended with error".to_string()));
                }
        }
        AgentEvent::AgentEnd { .. } => {
            st.streaming_message = None;
            st.pending_tool_calls.clear();
        }
        AgentEvent::AgentStart | AgentEvent::TurnStart | AgentEvent::ToolExecutionUpdate { .. } => {}
    }
}

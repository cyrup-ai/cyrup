//! The agent transcript message wrapper, tool-result message, and the ordered event enum
//! (arch-02 §3.1 / func-02 §4). Serde follows arch-00 §4: structs use `rename_all = "camelCase"`;
//! tagged enums add `rename_all_fields = "camelCase"` so payload fields are camelCase for
//! Pi-interop (R-00-013).

use cyrup_core::{AssistantMessage, Content, ToolCallId};
use cyrup_provider::StreamEvent;
use serde_json::Value;

/// A message in the agent transcript: a real LLM message (`user`/`assistant`/`toolResult`) OR an
/// app/extension `Custom` message that is NOT sent to the model (func-02 R-02-052). `convert_to_llm`
/// is responsible for dropping/transforming `Custom`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "role", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AgentMessage {
    User {
        content: Vec<Content>,
        #[serde(default)]
        timestamp: Option<i64>,
    },
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    /// App/extension role, never serialized into the LLM request (func-02 R-02-052).
    Custom {
        kind: String,
        payload: Value,
        #[serde(default)]
        timestamp: Option<i64>,
    },
}

impl AgentMessage {
    /// Convenience: a plain user text message.
    pub fn user_text(text: impl Into<String>) -> Self {
        AgentMessage::User { content: vec![Content::text(text)], timestamp: None }
    }

    pub fn is_assistant(&self) -> bool {
        matches!(self, AgentMessage::Assistant(_))
    }
}

/// A finalized tool result as it appears in the transcript (arch-02 §3.1). NB: `terminate` is a
/// runtime hint and is intentionally absent here (func-02 R-02-027 — it never leaks into history).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    /// `Text`/`Image` only.
    pub content: Vec<Content>,
    /// Structured app/extension metadata, NOT sent to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default)]
    pub is_error: bool,
    pub timestamp: i64,
}

/// The ordered event stream (func-02 §4 / §6).
///
/// snake_case `type` tag values match Pi's event-type names (`agent_start`, `turn_start`, …);
/// `rename_all_fields` makes PAYLOAD fields camelCase (`assistantMessageEvent`, `toolCallId`,
/// `toolName`, `partialResult`, `isError`, `toolResults`) for Pi-interop (R-00-013).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AgentEvent {
    AgentStart,
    TurnStart,
    MessageStart {
        message: AgentMessage,
    },
    /// Assistant only: carries the provider delta (func-02 R-02-009). The delta is boxed to keep
    /// `AgentEvent` small (the common variants are tiny; only this one carries a full `StreamEvent`).
    MessageUpdate {
        message: AgentMessage,
        assistant_message_event: Box<StreamEvent>,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        tool_call_id: ToolCallId,
        tool_name: String,
        args: Value,
    },
    ToolExecutionUpdate {
        tool_call_id: ToolCallId,
        tool_name: String,
        args: Value,
        partial_result: Value,
    },
    ToolExecutionEnd {
        tool_call_id: ToolCallId,
        tool_name: String,
        result: Value,
        is_error: bool,
    },
    TurnEnd {
        message: AgentMessage,
        tool_results: Vec<ToolResultMessage>,
    },
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
}

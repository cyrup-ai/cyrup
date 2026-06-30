//! The seam event super-set + the user-input value type (arch-11 §3.1/§3.2).
//!
//! [`AgentSessionEvent`] forwards every `cyrup_agent::AgentEvent` (func-02) and adds the
//! session-level events (queue/compaction) the facade itself emits. One schema serves the json and
//! rpc front-ends (func-11 Open-Question resolved: yes, one schema). Snake_case `type` tags match
//! Pi's event-type names; payload fields are camelCase via the embedded agent types.

use cyrup_agent::{AgentEvent, AgentMessage, ToolResultMessage};
use cyrup_core::{Content, ToolCallId};
use cyrup_provider::StreamEvent;
use cyrup_session::compaction::CompactionReason;
use serde_json::Value;

/// Where a user submission originated (func-11 §5/§6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InputSource {
    Cli,
    Stdin,
    Rpc,
    Sdk,
    Tui,
}

/// A user submission: text + optional images + provenance (arch-11 §3.1).
#[derive(Clone, Debug)]
pub struct UserInput {
    pub text: String,
    /// `Content::Image` payloads to attach alongside the text.
    pub images: Vec<Content>,
    pub source: InputSource,
    /// Skill / prompt-template expansion requested (R-11-016). Reserved for the expander.
    pub expand_templates: bool,
}

impl UserInput {
    /// A plain text submission from the given source.
    pub fn text(text: impl Into<String>, source: InputSource) -> Self {
        Self { text: text.into(), images: Vec::new(), source, expand_templates: true }
    }

    /// Build the agent transcript message for this input (text first, then any images).
    pub fn into_agent_message(self) -> AgentMessage {
        let mut content = Vec::with_capacity(1 + self.images.len());
        content.push(Content::text(self.text));
        content.extend(self.images);
        AgentMessage::User { content, timestamp: None }
    }
}

impl From<&str> for UserInput {
    fn from(s: &str) -> Self {
        UserInput::text(s, InputSource::Sdk)
    }
}

impl From<String> for UserInput {
    fn from(s: String) -> Self {
        UserInput::text(s, InputSource::Sdk)
    }
}

/// The preflight outcome of [`crate::AgentSession::prompt`] — the *acceptance*, not the full run
/// (mirrors Pi `PromptOptions.preflightResult`; the run is observed via events).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptAccepted {
    Started,
    Queued(StreamingBehavior),
}

/// Steering-behavior selector for a prompt submitted while the agent runs (func-02 §9; R-11-016).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamingBehavior {
    Steer,
    FollowUp,
}

/// Delivery timing for a custom message (Pi `deliverAs`, agent-session.ts:1309): `steer`/`followUp`
/// queue onto the active run, while `nextTurn` stages the message to ride the next prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliverAs {
    Steer,
    FollowUp,
    NextTurn,
}

/// The event super-set the seam exposes (arch-11 §3.2).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentSessionEvent {
    // --- forwarded AgentEvent (cyrup-agent / func-02) ---
    AgentStart,
    TurnStart,
    MessageStart {
        message: AgentMessage,
    },
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
    // --- session-level (cyrup-session-svc) ---
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    CompactionStart {
        reason: CompactionReason,
    },
    CompactionEnd {
        reason: CompactionReason,
        aborted: bool,
    },
    /// A retry-after-agent-end backoff began (Pi `auto_retry_start`, agent-session.ts:2508).
    AutoRetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error_message: String,
    },
    /// A retry sequence ended (Pi `auto_retry_end`, agent-session.ts:551/2528).
    AutoRetryEnd {
        success: bool,
        attempt: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        final_error: Option<String>,
    },
    ModelChanged {
        provider: String,
        model: String,
    },
    /// The active thinking level changed (Pi `thinking_level_changed`, agent-session.ts:1566).
    ThinkingLevelChanged {
        level: String,
    },
    /// A session was started/replaced by the runtime (Pi `session_start`,
    /// agent-session-runtime.ts:215). `reason` ∈ `new`/`resume`/`fork`/`reload`.
    SessionStart {
        reason: String,
        previous_session_file: Option<String>,
    },
    /// A session is being torn down by the runtime or disposed (Pi `session_shutdown`,
    /// agent-session-runtime.ts:168/391). `reason` ∈ `new`/`resume`/`fork`/`quit`/`reload`.
    SessionShutdown {
        reason: String,
    },
    /// The active session was atomically replaced; every prior subscription is now invalid and the
    /// consumer must re-subscribe against the runtime's new generation (R-11-021, arch-11 §3.2).
    SessionReplaced {
        generation: u64,
    },
}

impl AgentSessionEvent {
    /// Forward a `cyrup_agent::AgentEvent` into the seam super-set (arch-11 §3.2).
    pub fn from_agent(ev: &AgentEvent) -> Self {
        match ev {
            AgentEvent::AgentStart => AgentSessionEvent::AgentStart,
            AgentEvent::TurnStart => AgentSessionEvent::TurnStart,
            AgentEvent::MessageStart { message } => {
                AgentSessionEvent::MessageStart { message: message.clone() }
            }
            AgentEvent::MessageUpdate { message, assistant_message_event } => {
                AgentSessionEvent::MessageUpdate {
                    message: message.clone(),
                    assistant_message_event: assistant_message_event.clone(),
                }
            }
            AgentEvent::MessageEnd { message } => {
                AgentSessionEvent::MessageEnd { message: message.clone() }
            }
            AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                AgentSessionEvent::ToolExecutionStart {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    args: args.clone(),
                }
            }
            AgentEvent::ToolExecutionUpdate { tool_call_id, tool_name, args, partial_result } => {
                AgentSessionEvent::ToolExecutionUpdate {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    args: args.clone(),
                    partial_result: partial_result.clone(),
                }
            }
            AgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error } => {
                AgentSessionEvent::ToolExecutionEnd {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    result: result.clone(),
                    is_error: *is_error,
                }
            }
            AgentEvent::TurnEnd { message, tool_results } => AgentSessionEvent::TurnEnd {
                message: message.clone(),
                tool_results: tool_results.clone(),
            },
            AgentEvent::AgentEnd { messages } => {
                AgentSessionEvent::AgentEnd { messages: messages.clone() }
            }
        }
    }

    /// A short discriminant string (diagnostics / test assertions).
    pub fn kind(&self) -> &'static str {
        match self {
            AgentSessionEvent::AgentStart => "agent_start",
            AgentSessionEvent::TurnStart => "turn_start",
            AgentSessionEvent::MessageStart { .. } => "message_start",
            AgentSessionEvent::MessageUpdate { .. } => "message_update",
            AgentSessionEvent::MessageEnd { .. } => "message_end",
            AgentSessionEvent::ToolExecutionStart { .. } => "tool_execution_start",
            AgentSessionEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
            AgentSessionEvent::ToolExecutionEnd { .. } => "tool_execution_end",
            AgentSessionEvent::TurnEnd { .. } => "turn_end",
            AgentSessionEvent::AgentEnd { .. } => "agent_end",
            AgentSessionEvent::QueueUpdate { .. } => "queue_update",
            AgentSessionEvent::CompactionStart { .. } => "compaction_start",
            AgentSessionEvent::CompactionEnd { .. } => "compaction_end",
            AgentSessionEvent::AutoRetryStart { .. } => "auto_retry_start",
            AgentSessionEvent::AutoRetryEnd { .. } => "auto_retry_end",
            AgentSessionEvent::ModelChanged { .. } => "model_changed",
            AgentSessionEvent::ThinkingLevelChanged { .. } => "thinking_level_changed",
            AgentSessionEvent::SessionStart { .. } => "session_start",
            AgentSessionEvent::SessionShutdown { .. } => "session_shutdown",
            AgentSessionEvent::SessionReplaced { .. } => "session_replaced",
        }
    }
}

/// Convert an `AgentMessage` to a persisted `cyrup_core::Message` (drops `Custom`, which is never
/// sent to the model / never persisted as an LLM message).
pub(crate) fn agent_message_to_core(m: &AgentMessage) -> Option<cyrup_core::Message> {
    use cyrup_core::Message;
    match m {
        AgentMessage::User { content, timestamp } => {
            Some(Message::User { content: content.clone(), timestamp: timestamp.unwrap_or(0) })
        }
        AgentMessage::Assistant(a) => Some(Message::Assistant(a.clone())),
        AgentMessage::ToolResult(t) => Some(Message::ToolResult {
            tool_call_id: t.tool_call_id.clone(),
            tool_name: t.tool_name.clone(),
            content: t.content.clone(),
            is_error: t.is_error,
            details: t.details.clone(),
            timestamp: t.timestamp,
        }),
        AgentMessage::Custom { .. } => None,
    }
}

/// Convert a persisted `cyrup_core::Message` back to an `AgentMessage` (resume seeding).
pub(crate) fn core_message_to_agent(m: &cyrup_core::Message) -> AgentMessage {
    use cyrup_core::Message;
    match m {
        Message::User { content, timestamp } => {
            AgentMessage::User { content: content.clone(), timestamp: Some(*timestamp) }
        }
        Message::Assistant(a) => AgentMessage::Assistant(a.clone()),
        Message::ToolResult { tool_call_id, tool_name, content, is_error, details, timestamp } => {
            AgentMessage::ToolResult(ToolResultMessage {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                content: content.clone(),
                details: details.clone(),
                is_error: *is_error,
                timestamp: *timestamp,
            })
        }
    }
}

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
    /// An `input` extension handler fully serviced the submission (Pi `inputResult.action ===
    /// "handled"`, agent-session.ts:1025); no run was started and nothing was queued.
    Handled,
}

/// Per-call prompt options (Pi `PromptOptions`, agent-session.ts:200-212). `streaming_behavior`
/// selects the queue (steer/follow-up) when the agent is already streaming; without it a prompt
/// submitted while streaming is rejected (Pi throws at agent-session.ts:1044).
#[derive(Clone, Copy, Debug, Default)]
pub struct PromptOptions {
    /// How to deliver the message if the agent is mid-run (Pi `streamingBehavior`).
    pub streaming_behavior: Option<StreamingBehavior>,
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

/// Which summarization a retry belongs to — Pi's discriminated pair on
/// `summarization_retry_attempt_start` (`agent-session.ts:173-178`) and the `source` argument of
/// `_summarizationRetryCallbacks` (`:2641-2643`). Serialized flattened, so `source` is a sibling
/// key of `type` and `reason` only appears on the compaction arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "source", rename_all = "camelCase")]
pub enum SummarizationRetrySource {
    /// Pi `{ source: "branchSummary" }` — `generateBranchSummary` (`agent-session.ts:2998`).
    BranchSummary,
    /// Pi `{ source: "compaction", reason }` — manual `compact` (`:1859`) and `_runAutoCompaction`
    /// (`:2133`), which passes the live threshold/overflow reason through.
    Compaction { reason: CompactionReason },
}

/// The event super-set the seam exposes (arch-11 §3.2).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
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
        /// Whether the run that just ended will be auto-retried (Pi `agent_end.willRetry`,
        /// agent-session.ts:132/541). Computed in the persist+fan-out subscriber from the session's
        /// live retry state; `false` for an unbound session (no post-run driver, never retries).
        will_retry: bool,
    },
    // --- session-level (cyrup-session-svc) ---
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    CompactionStart {
        reason: CompactionReason,
    },
    /// A compaction settled (Pi `compaction_end`, agent-session.ts:142-148). Carries the produced
    /// [`CompactionResult`] (absent on cancel/abort/error), the `aborted` flag, whether the run will
    /// be retried after the compaction (`will_retry`, only meaningful for the overflow recovery
    /// path), and an `error_message` on the failure paths. `result`/`error_message` are omitted from
    /// the JSON when absent, matching Pi's `JSON.stringify` of `undefined`/optional fields.
    CompactionEnd {
        reason: CompactionReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<crate::state::CompactionResult>,
        aborted: bool,
        will_retry: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
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
    /// A summarization retry was scheduled — emitted BEFORE the backoff sleep (Pi
    /// `summarization_retry_scheduled`, agent-session.ts:166-172, emitted from
    /// `_summarizationRetryCallbacks.onRetryScheduled`, :2648-2656). Distinct from
    /// [`Self::AutoRetryStart`], which covers the turn-level retry: this one fires while a
    /// compaction / branch summarization is in flight, so the front-end can replace the
    /// compaction indicator with a retry countdown instead of appearing hung.
    SummarizationRetryScheduled {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error_message: String,
    },
    /// The backoff elapsed and the summarization call is being re-issued (Pi
    /// `summarization_retry_attempt_start`, agent-session.ts:173-178 / :2657-2663). `source`
    /// carries the context the TUI needs to recreate the underlying indicator; it is flattened so
    /// the wire shape is Pi's discriminated pair — `{"source":"branchSummary"}` or
    /// `{"source":"compaction","reason":"manual"}`.
    SummarizationRetryAttemptStart {
        #[serde(flatten)]
        source: SummarizationRetrySource,
    },
    /// The summarization retry loop settled (Pi `summarization_retry_finished`,
    /// agent-session.ts:179 / :2664-2667). Deliberately payload-free: Pi's `onRetryFinished`
    /// receives `(success, attempt, finalError)` but `_summarizationRetryCallbacks` discards all
    /// three, so adding fields here would diverge the RPC shape. Fires at most ONCE per
    /// summarization call, and only when at least one retry was actually scheduled — a call that
    /// succeeds on its first attempt emits none of these three events
    /// (`retry.ts:176/183/188`, ported at `cyrup-provider/src/utils/retry.rs`'s `last_retry` guard).
    SummarizationRetryFinished,
    /// A chunk of combined output from the out-of-loop [`crate::AgentSession::execute_bash`] seam
    /// (Pi `bash_execution_update`, agent-session.ts:181, emitted from `executeBash`'s `onChunk`
    /// wrapper at :2785-2787). Pi emits this for EVERY caller, including ones that pass no
    /// `onChunk` sink, so a front-end that only observes events still renders live bash output.
    /// `id` is `options?.id` and is omitted from the JSON when absent, matching Pi's optional field.
    BashExecutionUpdate {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        delta: String,
    },
    ModelChanged {
        provider: String,
        model: String,
    },
    /// The active thinking level changed (Pi `thinking_level_changed`, agent-session.ts:1566).
    ThinkingLevelChanged {
        level: String,
    },
    /// The session's display name / info entry changed (Pi `session_info_changed`,
    /// agent-session.ts:2686). Emitted by [`crate::AgentSession::set_session_name`] after the
    /// `session_info` entry is persisted.
    SessionInfoChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// A loaded extension appended a custom (non-LLM) entry to the running session tree (Pi
    /// `entry_appended`, agent-session.ts:140/2265-2271). Emitted by
    /// [`crate::host_services::LiveHostServices::append_entry`] once the entry is persisted; `entry`
    /// is the serialized [`cyrup_session::entry::Entry`] that now lives in the tree.
    EntryAppended {
        entry: Value,
    },
    /// The agent run has FULLY settled: no automatic retry, post-run compaction or queued
    /// continuation will follow (Pi `{ type: "agent_settled" }`, agent-session.ts:146 /
    /// `AgentSettledEvent`, extensions/types.ts:721-725). Distinct from `agent_end`, which fires
    /// once per `agent.prompt`/`agent.continue` — a turn that auto-retries emits TWO `agent_end`s
    /// and exactly ONE `agent_settled`. Pi emits it from the `finally` of `_runAgentPrompt`
    /// (:1063-1072), after `_flushPendingBashMessages()`, and its hosts key shutdown + idle
    /// bookkeeping off it (rpc-mode.ts:355-358, interactive-mode.ts:3137).
    AgentSettled,
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
                AgentSessionEvent::AgentEnd { messages: messages.clone(), will_retry: false }
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
            AgentSessionEvent::AgentSettled => "agent_settled",
            AgentSessionEvent::QueueUpdate { .. } => "queue_update",
            AgentSessionEvent::CompactionStart { .. } => "compaction_start",
            AgentSessionEvent::CompactionEnd { .. } => "compaction_end",
            AgentSessionEvent::AutoRetryStart { .. } => "auto_retry_start",
            AgentSessionEvent::AutoRetryEnd { .. } => "auto_retry_end",
            AgentSessionEvent::SummarizationRetryScheduled { .. } => {
                "summarization_retry_scheduled"
            }
            AgentSessionEvent::SummarizationRetryAttemptStart { .. } => {
                "summarization_retry_attempt_start"
            }
            AgentSessionEvent::SummarizationRetryFinished => "summarization_retry_finished",
            AgentSessionEvent::BashExecutionUpdate { .. } => "bash_execution_update",
            AgentSessionEvent::ModelChanged { .. } => "model_changed",
            AgentSessionEvent::ThinkingLevelChanged { .. } => "thinking_level_changed",
            AgentSessionEvent::SessionInfoChanged { .. } => "session_info_changed",
            AgentSessionEvent::EntryAppended { .. } => "entry_appended",
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
            // The PERSIST direction. `added_tool_names` is the deferred-tool anchor and is
            // recomputed from the transcript on every request, so dropping it here would silently
            // demote a resumed session back to prefix tool placement.
            usage: t.usage.clone(),
            added_tool_names: t.added_tool_names.clone(),
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
        Message::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
            details,
            usage,
            added_tool_names,
            timestamp,
        } => AgentMessage::ToolResult(ToolResultMessage {
            tool_call_id: tool_call_id.clone(),
            tool_name: tool_name.clone(),
            content: content.clone(),
            details: details.clone(),
            // The RESUME direction — the counterpart of the persist copy above.
            usage: usage.clone(),
            added_tool_names: added_tool_names.clone(),
            is_error: *is_error,
            timestamp: *timestamp,
        }),
    }
}

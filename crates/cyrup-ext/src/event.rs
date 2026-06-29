//! The host event enum (host -> guest direction), the `EventKind` discriminant, and the
//! `Subscriptions` bitset (arch-08 §3.4). `HostEvent` mirrors the func-08 §5 catalog; `EventKind`
//! indexes the 64-bit subscription bitset that gates dispatch (R-08-034 / R-ARCH-EXT-014).

use cyrup_agent::{AgentEvent, AgentMessage, ToolResultMessage};
use cyrup_core::{Content, Message, ToolCallId};
use serde_json::Value;

/// A C-like discriminant — one per `HostEvent` arm. Used to index the `Subscriptions` bitset and
/// (for serialization across the WIT boundary) as the `u8` the guest passes to `subscribe`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EventKind {
    // mutating seams
    ToolCall = 0,
    ToolResult = 1,
    Context = 2,
    MessageEnd = 3,
    BeforeAgentStart = 4,
    ResourcesDiscover = 5,
    ProjectTrust = 6,
    // notify-only
    AgentStart = 7,
    AgentEnd = 8,
    TurnStart = 9,
    TurnEnd = 10,
    MessageStart = 11,
    MessageUpdate = 12, // HIGH-FREQ
    ToolExecStart = 13,
    ToolExecUpdate = 14, // HIGH-FREQ
    ToolExecEnd = 15,
    SessionStart = 16,
    SessionShutdown = 17,
    // input / provider / model — mutating + notify (Pi types.ts:1158-1163)
    Input = 18,
    UserBash = 19,
    BeforeProviderRequest = 20,
    AfterProviderResponse = 21,
    ModelSelect = 22,
    ThinkingLevelSelect = 23,
    // session control lifecycle (Pi types.ts:1148-1156)
    SessionBeforeSwitch = 24,
    SessionBeforeFork = 25,
    SessionBeforeCompact = 26,
    SessionCompact = 27,
    SessionBeforeTree = 28,
    SessionTree = 29,
}

impl EventKind {
    /// The number of distinct kinds (must stay <= 64 for the bitset). 1:1 with Pi's 30-event
    /// catalog (extensions/types.ts:1133-1171).
    pub const COUNT: u8 = 30;

    /// Parse the `u8` a guest passes via `subscribe(event-kinds)`.
    pub fn from_u8(v: u8) -> Option<EventKind> {
        use EventKind::*;
        Some(match v {
            0 => ToolCall,
            1 => ToolResult,
            2 => Context,
            3 => MessageEnd,
            4 => BeforeAgentStart,
            5 => ResourcesDiscover,
            6 => ProjectTrust,
            7 => AgentStart,
            8 => AgentEnd,
            9 => TurnStart,
            10 => TurnEnd,
            11 => MessageStart,
            12 => MessageUpdate,
            13 => ToolExecStart,
            14 => ToolExecUpdate,
            15 => ToolExecEnd,
            16 => SessionStart,
            17 => SessionShutdown,
            18 => Input,
            19 => UserBash,
            20 => BeforeProviderRequest,
            21 => AfterProviderResponse,
            22 => ModelSelect,
            23 => ThinkingLevelSelect,
            24 => SessionBeforeSwitch,
            25 => SessionBeforeFork,
            26 => SessionBeforeCompact,
            27 => SessionCompact,
            28 => SessionBeforeTree,
            29 => SessionTree,
            _ => return None,
        })
    }

    /// The snake_case event name (Pi extensions/types.ts:1133-1171; used in `ExtensionError.event`).
    pub fn name(&self) -> &'static str {
        use EventKind::*;
        match self {
            ToolCall => "tool_call",
            ToolResult => "tool_result",
            Context => "context",
            MessageEnd => "message_end",
            BeforeAgentStart => "before_agent_start",
            ResourcesDiscover => "resources_discover",
            ProjectTrust => "project_trust",
            AgentStart => "agent_start",
            AgentEnd => "agent_end",
            TurnStart => "turn_start",
            TurnEnd => "turn_end",
            MessageStart => "message_start",
            MessageUpdate => "message_update",
            ToolExecStart => "tool_execution_start",
            ToolExecUpdate => "tool_execution_update",
            ToolExecEnd => "tool_execution_end",
            SessionStart => "session_start",
            SessionShutdown => "session_shutdown",
            Input => "input",
            UserBash => "user_bash",
            BeforeProviderRequest => "before_provider_request",
            AfterProviderResponse => "after_provider_response",
            ModelSelect => "model_select",
            ThinkingLevelSelect => "thinking_level_select",
            SessionBeforeSwitch => "session_before_switch",
            SessionBeforeFork => "session_before_fork",
            SessionBeforeCompact => "session_before_compact",
            SessionCompact => "session_compact",
            SessionBeforeTree => "session_before_tree",
            SessionTree => "session_tree",
        }
    }

    /// Map a notify-only `cyrup_agent::AgentEvent` to its kind (mutating kinds come via `Hooks`).
    pub fn from_agent(ev: &AgentEvent) -> Option<EventKind> {
        use EventKind::*;
        Some(match ev {
            AgentEvent::AgentStart => AgentStart,
            AgentEvent::TurnStart => TurnStart,
            AgentEvent::MessageStart { .. } => MessageStart,
            AgentEvent::MessageUpdate { .. } => MessageUpdate,
            AgentEvent::MessageEnd { .. } => MessageEnd,
            AgentEvent::ToolExecutionStart { .. } => ToolExecStart,
            AgentEvent::ToolExecutionUpdate { .. } => ToolExecUpdate,
            AgentEvent::ToolExecutionEnd { .. } => ToolExecEnd,
            AgentEvent::TurnEnd { .. } => TurnEnd,
            AgentEvent::AgentEnd { .. } => AgentEnd,
        })
    }
}

/// 64-bit subscription bitset over `EventKind` (arch-08 §3.2). An event with zero subscribers
/// never serializes or crosses the boundary — a single `bitset & kind` test (R-ARCH-EXT-014).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Subscriptions(u64);

impl Subscriptions {
    pub const fn empty() -> Self {
        Subscriptions(0)
    }

    pub fn with(mut self, kind: EventKind) -> Self {
        self.0 |= 1u64 << (kind as u8);
        self
    }

    pub fn add(&mut self, kind: EventKind) {
        self.0 |= 1u64 << (kind as u8);
    }

    pub fn contains(&self, kind: EventKind) -> bool {
        self.0 & (1u64 << (kind as u8)) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Union of two bitsets (used to build the dispatcher's aggregate gate).
    pub fn union(self, other: Subscriptions) -> Subscriptions {
        Subscriptions(self.0 | other.0)
    }

    /// Build from an iterator of kinds (e.g. the guest's `subscribe` list).
    pub fn from_kinds(kinds: impl IntoIterator<Item = EventKind>) -> Self {
        let mut s = Subscriptions::empty();
        for k in kinds {
            s.add(k);
        }
        s
    }
}

/// The host event (host -> guest). One arm per func-08 §5 event; payload = minimum-spec record.
/// Open-shaped fields carry `serde_json::Value`; fixed-shape ids/roles are typed (arch-08 §3.4).
#[derive(Clone, Debug)]
pub enum HostEvent {
    // 5.4 tools — mutating seams
    ToolCall { call_id: ToolCallId, name: String, input: Value },
    ToolResult {
        call_id: ToolCallId,
        name: String,
        content: Vec<Content>,
        details: Option<Value>,
        is_error: bool,
    },
    // 5.3 agent & turn — mutating + notify
    Context { messages: Vec<AgentMessage> },
    MessageEnd { message: Message },
    /// `before_agent_start` (Pi types.ts:665): the user prompt + images + the (chainable)
    /// system prompt and its options. A handler may inject a message and/or replace the prompt;
    /// injected messages ACCUMULATE across handlers (Pi `runner.ts:980` `messages.push`).
    BeforeAgentStart {
        prompt: String,
        images: Value,
        system_prompt: String,
        options: Value,
        /// Messages injected by handlers so far (folded across the chain, R-08-011).
        injected: Vec<Message>,
    },
    AgentStart,
    AgentEnd { messages: Vec<AgentMessage> },
    TurnStart { turn_index: u32 },
    TurnEnd { turn_index: u32, message: AgentMessage, tool_results: Vec<ToolResultMessage> },
    MessageStart { role: String },
    MessageUpdate { delta: Value },
    ToolExecStart { call_id: ToolCallId, name: String, args: Value },
    ToolExecUpdate { call_id: ToolCallId, chunk: Value },
    ToolExecEnd { call_id: ToolCallId, result: Value, is_error: bool },
    // 5.1/5.2 startup & session
    SessionStart { reason: String },
    SessionShutdown { reason: String },
    ResourcesDiscover,
    ProjectTrust,
    // 5.5 input / 5.6 provider / model (Pi types.ts:1158-1163)
    Input { text: String },
    UserBash { command: String, operations: Value },
    BeforeProviderRequest { payload: Value },
    AfterProviderResponse { status: u32, headers: Value },
    ModelSelect { model: Value },
    ThinkingLevelSelect { level: String },
    // session control lifecycle (Pi types.ts:1148-1156)
    SessionBeforeSwitch { target_id: String },
    SessionBeforeFork { entry_id: String },
    SessionBeforeCompact,
    SessionCompact { summary: String },
    SessionBeforeTree,
    SessionTree { tree: Value },
}

impl HostEvent {
    pub fn kind(&self) -> EventKind {
        use EventKind as K;
        match self {
            HostEvent::ToolCall { .. } => K::ToolCall,
            HostEvent::ToolResult { .. } => K::ToolResult,
            HostEvent::Context { .. } => K::Context,
            HostEvent::MessageEnd { .. } => K::MessageEnd,
            HostEvent::BeforeAgentStart { .. } => K::BeforeAgentStart,
            HostEvent::AgentStart => K::AgentStart,
            HostEvent::AgentEnd { .. } => K::AgentEnd,
            HostEvent::TurnStart { .. } => K::TurnStart,
            HostEvent::TurnEnd { .. } => K::TurnEnd,
            HostEvent::MessageStart { .. } => K::MessageStart,
            HostEvent::MessageUpdate { .. } => K::MessageUpdate,
            HostEvent::ToolExecStart { .. } => K::ToolExecStart,
            HostEvent::ToolExecUpdate { .. } => K::ToolExecUpdate,
            HostEvent::ToolExecEnd { .. } => K::ToolExecEnd,
            HostEvent::SessionStart { .. } => K::SessionStart,
            HostEvent::SessionShutdown { .. } => K::SessionShutdown,
            HostEvent::ResourcesDiscover => K::ResourcesDiscover,
            HostEvent::ProjectTrust => K::ProjectTrust,
            HostEvent::Input { .. } => K::Input,
            HostEvent::UserBash { .. } => K::UserBash,
            HostEvent::BeforeProviderRequest { .. } => K::BeforeProviderRequest,
            HostEvent::AfterProviderResponse { .. } => K::AfterProviderResponse,
            HostEvent::ModelSelect { .. } => K::ModelSelect,
            HostEvent::ThinkingLevelSelect { .. } => K::ThinkingLevelSelect,
            HostEvent::SessionBeforeSwitch { .. } => K::SessionBeforeSwitch,
            HostEvent::SessionBeforeFork { .. } => K::SessionBeforeFork,
            HostEvent::SessionBeforeCompact => K::SessionBeforeCompact,
            HostEvent::SessionCompact { .. } => K::SessionCompact,
            HostEvent::SessionBeforeTree => K::SessionBeforeTree,
            HostEvent::SessionTree { .. } => K::SessionTree,
        }
    }

    /// Build a notify-only `HostEvent` from an `AgentEvent` (the `ExtSubscriber` seam, arch-08 §5.4).
    /// Mutating events (`ToolCall`/`ToolResult`/`Context`) are produced by the `Hooks` seam instead.
    pub fn from_agent(ev: &AgentEvent) -> Option<HostEvent> {
        Some(match ev {
            AgentEvent::AgentStart => HostEvent::AgentStart,
            AgentEvent::TurnStart => HostEvent::TurnStart { turn_index: 0 },
            AgentEvent::MessageStart { message } => {
                HostEvent::MessageStart { role: role_of(message) }
            }
            AgentEvent::MessageUpdate { assistant_message_event, .. } => HostEvent::MessageUpdate {
                delta: serde_json::to_value(assistant_message_event).unwrap_or(Value::Null),
            },
            AgentEvent::MessageEnd { message } => {
                HostEvent::MessageEnd { message: to_llm_message(message)? }
            }
            AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                HostEvent::ToolExecStart {
                    call_id: tool_call_id.clone(),
                    name: tool_name.clone(),
                    args: args.clone(),
                }
            }
            AgentEvent::ToolExecutionUpdate { tool_call_id, partial_result, .. } => {
                HostEvent::ToolExecUpdate {
                    call_id: tool_call_id.clone(),
                    chunk: partial_result.clone(),
                }
            }
            AgentEvent::ToolExecutionEnd { tool_call_id, result, is_error, .. } => {
                HostEvent::ToolExecEnd {
                    call_id: tool_call_id.clone(),
                    result: result.clone(),
                    is_error: *is_error,
                }
            }
            AgentEvent::TurnEnd { message, tool_results } => HostEvent::TurnEnd {
                turn_index: 0,
                message: message.clone(),
                tool_results: tool_results.clone(),
            },
            AgentEvent::AgentEnd { messages } => HostEvent::AgentEnd { messages: messages.clone() },
        })
    }
}

fn role_of(m: &AgentMessage) -> String {
    match m {
        AgentMessage::User { .. } => "user",
        AgentMessage::Assistant(_) => "assistant",
        AgentMessage::ToolResult(_) => "toolResult",
        AgentMessage::Custom { .. } => "custom",
    }
    .to_string()
}

fn to_llm_message(m: &AgentMessage) -> Option<Message> {
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

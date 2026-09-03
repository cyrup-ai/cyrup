//! The agent transcript message wrapper, tool-result message, and the ordered event enum
//! (arch-02 §3.1 / func-02 §4). Serde follows arch-00 §4: structs use `rename_all = "camelCase"`;
//! tagged enums add `rename_all_fields = "camelCase"` so payload fields are camelCase for
//! Pi-interop (R-00-013).

use cyrup_core::{AssistantMessage, Content, SharedStr, ToolCallId, Usage};
use std::sync::Arc;
use cyrup_provider::StreamEvent;
use serde_json::Value;

/// A message in the agent transcript: a real LLM message (`user`/`assistant`/`toolResult`) OR an
/// app/extension message that is NOT sent to the model verbatim (func-02 R-02-052).
/// `convert_to_llm` is responsible for dropping/transforming the app roles.
///
/// Pi's base union is `type AgentMessage = Message | CustomAgentMessages[keyof CustomAgentMessages]`
/// (`packages/agent/src/types.ts:319` @v0.83.0), where `CustomAgentMessages` is an EMPTY interface
/// (`:310-312`) that the app fills by declaration merging. cyrup's coding-agent layer merges the
/// same four roles pi's does — `bashExecution`, `custom`, `branchSummary`, `compactionSummary`
/// (`packages/coding-agent/src/core/messages.ts:68-77` @v0.83.0). [`AgentMessage::Custom`] is the
/// typed arm for `custom` because extensions construct it directly through `sendMessage`;
/// [`AgentMessage::App`] carries the other three, which only ever enter the transcript as a whole
/// raw context projection handed over by `cyrup-session` (SESS-043).
#[derive(Clone, Debug, PartialEq)]
pub enum AgentMessage {
    User {
        content: Vec<Content>,
        timestamp: Option<i64>,
    },
    /// The assistant turn, as a SHARED handle (PERF-001).
    ///
    /// This message is carried by every `message_update`, is also embedded in that event's
    /// `assistant_message_event`, and is then cloned once per live subscriber by the session
    /// facade's fan-out. Owning it here meant a deep copy at each of those points, on every
    /// stream delta. The wire is unchanged: serde's `rc` feature serializes an `Arc<T>`
    /// transparently as `T`, and this enum's hand-written serializer delegates to it.
    Assistant(Arc<AssistantMessage>),
    ToolResult(ToolResultMessage),
    /// App/extension role, never serialized into the LLM request verbatim (func-02 R-02-052).
    Custom {
        kind: String,
        payload: Value,
        /// Pi's `CustomMessage.details` (`messages.ts:46-53`) — the opaque structured payload a
        /// registered message renderer reads (`getMessageRenderer(customType)(message, …)`).
        ///
        /// Carried HERE, and not only on the persisted `CustomMessage` entry, because the LIVE
        /// renderer surface is this message serialized off `message_end`
        /// (`cyrup-tui/src/app/extension_render.rs` takes `to_value(ev)?["message"]`). Without it an
        /// injected card could be drawn on `--resume` and not on the turn that produced it.
        details: Option<Value>,
        timestamp: Option<i64>,
    },
    /// SESS-043 — one of pi's declaration-merged coding-agent roles that this crate has no type
    /// for, held as its pi wire object (the `role` key included) so every field round-trips
    /// unread.
    ///
    /// Field ORDER does not survive: `serde_json::Map` is a `BTreeMap` without the `preserve_order`
    /// feature, so the keys are sorted on the way in and pi's declaration order cannot be restored
    /// on the way out. That is why this variant is confined to the in-memory transcript — the two
    /// surfaces where byte-shape is load-bearing (the session file, and the extension
    /// `session_before_compact` payload) both serialize
    /// `cyrup_session::agent_message::AgentMessage` through its hand-written `SerializeMap`, which
    /// spells pi's order out and never sees an `App`.
    ///
    /// This crate is cyrup's `pi-agent-core` counterpart and, exactly like pi's, must not know the
    /// coding-agent's message shapes: `BashExecutionMessage` / `BranchSummaryMessage` /
    /// `CompactionSummaryMessage` are declared one layer up in `cyrup-session`
    /// (`agent_message.rs`), which depends on this crate and not the other way round. The layer
    /// that constructs an `App` is the layer that renders it — `cyrup-session-svc`'s
    /// `convert_to_llm` hook, which is cyrup's `convertToLlm` (`coding-agent/src/core/sdk.ts:301`
    /// @v0.83.0) — so the payload is opaque here and never inspected.
    App {
        /// The pi `role` discriminant — one of the three [`AppRole`]s, closed.
        role: AppRole,
        /// The full pi wire object for the message, `role` included.
        payload: serde_json::Map<String, Value>,
    },
}

/// The three declaration-merged coding-agent roles [`AgentMessage::App`] carries
/// (`coding-agent/src/core/messages.ts:68-77` @v0.83.0, minus `custom`, which has its own arm).
/// Closed: an `App` with any other role is unrepresentable, and deserialization of any OTHER
/// unknown role still fails exactly as it did before `App` existed — pi's union is closed over the
/// merged set too, and widening the tolerance would silently swallow a malformed transcript.
///
/// No `From<String>` / `From<&str>`: [`Self::parse`] is the one fallible door.
///
/// ```compile_fail
/// // An arbitrary string is not a role; only `AppRole::parse` can say whether it is.
/// let _role: cyrup_agent::AppRole = String::from("bashExecution").into();
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppRole {
    BashExecution,
    BranchSummary,
    CompactionSummary,
}

impl AppRole {
    pub const ALL: [AppRole; 3] =
        [AppRole::BashExecution, AppRole::BranchSummary, AppRole::CompactionSummary];

    /// The pi `role` tag — the exact wire string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BashExecution => "bashExecution",
            Self::BranchSummary => "branchSummary",
            Self::CompactionSummary => "compactionSummary",
        }
    }

    /// The deserialize gate. `None` for every other tag, including the four typed roles.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

impl serde::Serialize for AgentMessage {
    /// Manual serializer so the `role` discriminant appears EXACTLY ONCE — the same defect, and the
    /// same fix, as [`cyrup_core::Message`]'s serializer.
    ///
    /// A derived internally-tagged `Serialize` writes `role` itself and THEN delegates the
    /// `Assistant` newtype payload to [`AssistantMessage`], whose serializer self-tags. The result
    /// was literally `{"role":"assistant","role":"assistant",…}` — a duplicate key on a contract
    /// surface (`--json` and RPC stdout, and every transcript this wrapper reaches). JSON permits
    /// duplicate keys syntactically, so nothing errored; `JSON.parse` silently keeps the LAST, while
    /// stricter parsers reject the document outright. It was found by capturing real `--json` output
    /// during the G43 review, not by any test — every existing assertion looked at parsed values,
    /// where the duplicate is invisible.
    ///
    /// The three non-`Assistant` arms delegate to a private mirror enum carrying the identical serde
    /// attributes, so their bytes are unchanged by construction; only the `Assistant` arm's spurious
    /// outer tag is removed.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        /// Byte-identical to the old derive for every arm it carries. `AgentMessage::Assistant` is
        /// deliberately absent: that arm is the one whose payload self-tags.
        #[derive(serde::Serialize)]
        #[serde(tag = "role", rename_all = "camelCase", rename_all_fields = "camelCase")]
        enum TaggedNonAssistant<'a> {
            User { content: &'a Vec<Content>, timestamp: &'a Option<i64> },
            ToolResult(&'a ToolResultMessage),
            Custom {
                kind: &'a str,
                payload: &'a Value,
                /// Emitted only when present, so every message written before `details` existed
                /// serializes byte-identically.
                #[serde(skip_serializing_if = "Option::is_none")]
                details: &'a Option<Value>,
                timestamp: &'a Option<i64>,
            },
        }

        match self {
            // Self-tagging: emits `role:"assistant"` first, then Pi's field order.
            AgentMessage::Assistant(m) => m.serialize(serializer),
            AgentMessage::User { content, timestamp } => {
                TaggedNonAssistant::User { content, timestamp }.serialize(serializer)
            }
            AgentMessage::ToolResult(m) => TaggedNonAssistant::ToolResult(m).serialize(serializer),
            AgentMessage::Custom { kind, payload, details, timestamp } => {
                TaggedNonAssistant::Custom { kind, payload, details, timestamp }.serialize(serializer)
            }
            // SESS-043 — the payload IS the pi wire object (`role` included), so it is emitted as
            // it stands; `role` is held out separately only so this crate can classify without
            // parsing. Key order is `serde_json::Map`'s, not pi's — see the variant's docs.
            AgentMessage::App { payload, .. } => payload.serialize(serializer),
        }
    }
}

impl<'de> serde::Deserialize<'de> for AgentMessage {
    /// Mirror of [`AgentMessage`]'s serializer. Byte-for-byte the old derive
    /// (`tag = "role", rename_all = "camelCase", rename_all_fields = "camelCase"`) for the four
    /// typed arms, plus SESS-043's [`AgentMessage::App`] fallback for exactly the three
    /// declaration-merged roles in [`AppRole`].
    ///
    /// Written by hand rather than derived because an internally-tagged derive has no way to route
    /// a set of tag values into one catch-all variant. Every other unknown role still errors, and
    /// the two `timestamp` fields keep the derive's `#[serde(default)]`, because both are
    /// `Option<i64>` and `serde_json::from_value` supplies `None` for an absent key.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        /// The four typed arms, attribute-for-attribute the old derive.
        #[derive(serde::Deserialize)]
        #[serde(tag = "role", rename_all = "camelCase", rename_all_fields = "camelCase")]
        enum Typed {
            User {
                content: Vec<Content>,
                #[serde(default)]
                timestamp: Option<i64>,
            },
            Assistant(AssistantMessage),
            ToolResult(ToolResultMessage),
            Custom {
                kind: String,
                payload: Value,
                #[serde(default)]
                details: Option<Value>,
                #[serde(default)]
                timestamp: Option<i64>,
            },
        }

        let v = Value::deserialize(d)?;
        if let Some(role) = v.get("role").and_then(Value::as_str).and_then(AppRole::parse) {
            let Value::Object(payload) = v else {
                // Unreachable: `v.get` only yields on an object.
                return Err(D::Error::custom("agent message must be a JSON object"));
            };
            return Ok(AgentMessage::App { role, payload });
        }
        Ok(match serde_json::from_value::<Typed>(v).map_err(D::Error::custom)? {
            Typed::User { content, timestamp } => AgentMessage::User { content, timestamp },
            Typed::Assistant(a) => AgentMessage::Assistant(Arc::new(a)),
            Typed::ToolResult(t) => AgentMessage::ToolResult(t),
            Typed::Custom { kind, payload, details, timestamp } => {
                AgentMessage::Custom { kind, payload, details, timestamp }
            }
        })
    }
}

impl AgentMessage {
    /// Convenience: a plain user text message.
    pub fn user_text(text: impl Into<SharedStr>) -> Self {
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
    /// Usage from the tool execution itself, if available. NOT part of main LLM context accounting
    /// (Pi `ToolResultMessage.usage`, ai/src/types.ts:421-422). Absent when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Names of tools that became available from this transcript point onward (Pi
    /// `ToolResultMessage.addedToolNames`, ai/src/types.ts:423-428). Absent when empty, matching
    /// Pi's conditional spread (agent-loop.ts:783).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added_tool_names: Vec<String>,
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
        messages: Vec<Arc<AgentMessage>>,
    },
}

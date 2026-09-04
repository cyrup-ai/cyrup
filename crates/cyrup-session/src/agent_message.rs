//! The extended message model Pi calls `AgentMessage` (`messages.ts:8,26-66` + the
//! `CustomAgentMessages` declaration merging at `messages.ts:68-77`). Pi's union is
//! `user`/`assistant`/`toolResult` **plus** four coding-agent roles: `bashExecution` (`!` shell
//! command, `messages.ts:29-40`), `custom` (extension `sendMessage`, `messages.ts:46-53`),
//! `branchSummary` (`messages.ts:55-60`) and `compactionSummary` (`messages.ts:62-67`).
//!
//! `cyrup_core::Message` is a closed `user`/`assistant`/`toolResult` enum, so none of the four can
//! live there. [`AgentMessage`] is the cyrup-session-local superset that mirrors Pi's union exactly.
//! It serves two roles:
//!
//! - the inner type of [`crate::entry::KnownEntry::Message`], keeping bash/custom-role entries
//!   parseable (instead of degrading to `Entry::Unknown` and being silently dropped from context /
//!   cut-points / token estimation); and
//! - the element type of Pi's **raw context projection** — `sessionEntryToContextMessages`
//!   (`session-manager.ts:383-408`) returns `AgentMessage[]`, and that projection is what
//!   `findCutPoint` classifies, what `estimateTokens` measures, and what `prepareCompaction` hands
//!   to extensions (see [`crate::context::raw_context_messages`]).
//!
//! [`convert_to_llm`] renders the union down to core `user` messages exactly like Pi's
//! `convertToLlm` (`messages.ts:148-195`) — applied at the LLM boundary, never earlier.

use cyrup_core::{Content, EntryId, Message};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// A `bashExecution`-role message: the `!`-command feature (Pi `BashExecutionMessage`,
/// `messages.ts:29-40`). Rendered to a `user` text message for the LLM (unless
/// [`Self::exclude_from_context`]).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub command: String,
    pub output: String,
    /// `number | undefined` (Pi `messages.ts:33`): omitted on the wire when unset.
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub full_output_path: Option<String>,
    #[serde(default)]
    pub timestamp: i64,
    /// `!!` prefix: excluded from LLM context (Pi `messages.ts:38-39,153-155`).
    #[serde(default)]
    pub exclude_from_context: Option<bool>,
}

/// A `custom`-role message: an extension-injected message via `sendMessage()` (Pi `CustomMessage`,
/// `messages.ts:46-53`). Rendered to a `user` message for the LLM (Pi `messages.ts:162-168`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomRoleMessage {
    pub custom_type: String,
    /// `string | (Text|Image)[]` — mirrored as raw JSON to match Pi exactly.
    pub content: Value,
    #[serde(default)]
    pub display: bool,
    #[serde(default)]
    pub details: Option<Value>,
    #[serde(default)]
    pub timestamp: i64,
}

/// A `branchSummary`-role message (Pi `BranchSummaryMessage`, `messages.ts:55-60`), produced by
/// `createBranchSummaryMessage` from a `branch_summary` entry.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryMessage {
    pub summary: String,
    pub from_id: EntryId,
    #[serde(default)]
    pub timestamp: i64,
}

/// A `compactionSummary`-role message (Pi `CompactionSummaryMessage`, `messages.ts:62-67`),
/// produced by `createCompactionSummaryMessage` from a `compaction` entry.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummaryMessage {
    pub summary: String,
    #[serde(default)]
    pub tokens_before: u64,
    #[serde(default)]
    pub timestamp: i64,
}

/// The full Pi `AgentMessage` union. `Core` covers `user`/`assistant`/`toolResult`
/// ([`cyrup_core::Message`]); the four extra arms carry the coding-agent roles the closed core enum
/// cannot represent (`messages.ts:68-77`).
// `Core` is intentionally inline (no `Box`) — the same rationale, and the same call, as
// `Entry::Known` and `KnownEntry::Message` in `entry.rs:30-32,184-186`: it is the dominant variant
// on every path (a transcript is overwhelmingly `Core`), and boxing it would force `box`-patterns
// (unstable) at all 52 `AgentMessage::Core(` match sites to save a branch nothing takes.
//
// The gap sat at EXACTLY the 200-byte lint threshold before `AssistantMessage` gained Pi's
// `rawStopReason`/`deferred` (`v0.84.1 ai/src/types.ts:424,426`), so it was going to trip on
// whichever field landed next. `deferred` is already boxed at its declaration
// (`cyrup-core/src/message.rs:466-475`) because it is large AND ~always absent; `rawStopReason`
// is a plain `Option<String>` because Pi sets it on nearly every settled turn, so indirection
// there would cost an allocation on the common path to save 16 bytes on the rare one.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum AgentMessage {
    Core(Message),
    BashExecution(BashExecutionMessage),
    Custom(CustomRoleMessage),
    BranchSummary(BranchSummaryMessage),
    CompactionSummary(CompactionSummaryMessage),
}

/// The role of an [`AgentMessage`], as the `switch (message.role)` predicates in Pi's cut-point
/// layer see it (`compaction.ts:308-336`). Carried separately so an entry can be classified without
/// cloning its message (see [`crate::context::context_message_role`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    ToolResult,
    BashExecution,
    Custom,
    BranchSummary,
    CompactionSummary,
}

impl MessageRole {
    /// Pi `isCutPointMessage` (`compaction.ts:308-321`): every role EXCEPT `toolResult` may serve
    /// as a cut boundary — a tool result must stay with its call.
    pub fn is_cut_point(self) -> bool {
        !matches!(self, MessageRole::ToolResult)
    }

    /// Pi `isTurnStartMessage` (`compaction.ts:323-336`): `user`, `bashExecution`, `custom`,
    /// `branchSummary` and `compactionSummary` start a turn; `assistant` and `toolResult` do not.
    pub fn is_turn_start(self) -> bool {
        matches!(
            self,
            MessageRole::User
                | MessageRole::BashExecution
                | MessageRole::Custom
                | MessageRole::BranchSummary
                | MessageRole::CompactionSummary
        )
    }
}

impl AgentMessage {
    /// Convenience: wrap a core message.
    pub fn core(m: Message) -> Self {
        AgentMessage::Core(m)
    }

    /// The wire `role` of this message.
    pub fn role(&self) -> MessageRole {
        match self {
            AgentMessage::Core(Message::User { .. }) => MessageRole::User,
            AgentMessage::Core(Message::Assistant(_)) => MessageRole::Assistant,
            AgentMessage::Core(Message::ToolResult { .. }) => MessageRole::ToolResult,
            AgentMessage::BashExecution(_) => MessageRole::BashExecution,
            AgentMessage::Custom(_) => MessageRole::Custom,
            AgentMessage::BranchSummary(_) => MessageRole::BranchSummary,
            AgentMessage::CompactionSummary(_) => MessageRole::CompactionSummary,
        }
    }

    /// `true` for the core `assistant` role (turn/model detection).
    pub fn is_core_assistant(&self) -> bool {
        matches!(self, AgentMessage::Core(Message::Assistant(_)))
    }

    /// `true` for the core `toolResult` role (never a valid cut point — Pi `compaction.ts:320-321`).
    pub fn is_tool_result(&self) -> bool {
        matches!(self, AgentMessage::Core(Message::ToolResult { .. }))
    }

    /// Pi `isTurnStartMessage` (`compaction.ts:323-336`) — see [`MessageRole::is_turn_start`].
    pub fn is_turn_start(&self) -> bool {
        self.role().is_turn_start()
    }

    /// Render to the LLM `user`-form message(s), per Pi `convertToLlm` (`messages.ts:148-195`).
    /// `Core` is passed through verbatim; a `bashExecution` becomes a `user` text message (or is
    /// dropped when `excludeFromContext`); a `custom` message unwraps its `content` to a `user`
    /// message; a `branchSummary`/`compactionSummary` becomes its wrapped `user` note. Pushes
    /// nothing for a dropped bash message.
    pub fn push_llm(&self, out: &mut Vec<Message>) {
        match self {
            AgentMessage::Core(m) => out.push(m.clone()),
            AgentMessage::BashExecution(b) => {
                if b.exclude_from_context.unwrap_or(false) {
                    return;
                }
                out.push(Message::User {
                    content: vec![Content::text(bash_execution_to_text(b))],
                    timestamp: b.timestamp,
                });
            }
            AgentMessage::Custom(c) => out.push(custom_to_message(&c.content, c.timestamp)),
            AgentMessage::BranchSummary(b) => {
                out.push(crate::context::branch_summary_message(
                    &b.summary,
                    b.timestamp,
                ));
            }
            AgentMessage::CompactionSummary(c) => {
                out.push(crate::context::compaction_summary_message(
                    &c.summary,
                    c.tokens_before,
                    c.timestamp,
                ));
            }
        }
    }
}

/// Pi `convertToLlm(messages)` (`messages.ts:148-195`): render a raw `AgentMessage` list down to
/// core LLM messages, dropping `excludeFromContext` bash messages. Applied at the LLM boundary only
/// — the cut-point layer and the extension-facing compaction preparation both carry the RAW list.
pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        m.push_llm(&mut out);
    }
    out
}

/// Convert a `BashExecutionMessage` to user-message text (Pi `bashExecutionToText`,
/// `messages.ts:82-98`).
pub fn bash_execution_to_text(msg: &BashExecutionMessage) -> String {
    let mut text = format!("Ran `{}`\n", msg.command);
    if msg.output.is_empty() {
        text.push_str("(no output)");
    } else {
        text.push_str(&format!("```\n{}\n```", msg.output));
    }
    if msg.cancelled {
        text.push_str("\n\n(command cancelled)");
    } else if let Some(code) = msg.exit_code
        && code != 0
    {
        text.push_str(&format!("\n\nCommand exited with code {code}"));
    }
    if msg.truncated
        && let Some(path) = &msg.full_output_path
    {
        text.push_str(&format!("\n\n[Output truncated. Full output: {path}]"));
    }
    text
}

/// Convert a `custom`/`custom_message` `content` (`string | (Text|Image)[]`) to a user message (Pi
/// `convertToLlm` custom arm + `createCustomMessage`, `messages.ts:122-138,162-168`).
pub fn custom_to_message(content: &Value, timestamp: i64) -> Message {
    let blocks = match content {
        Value::String(s) => vec![Content::text(s.clone())],
        Value::Array(_) => serde_json::from_value::<Vec<Content>>(content.clone())
            .unwrap_or_else(|_| vec![Content::text(content.to_string())]),
        // Pi normalizes a null/absent `content` to `[]` BEFORE `createCustomMessage` ever sees it:
        // `createCustomMessage(entry.customType, entry.content ?? [], …)`
        // (`session-manager.ts:396-399`). The catch-all below stringifies its input, so without
        // this arm a `"content": null` entry injected the four characters `null` into a user turn.
        Value::Null => Vec::new(),
        other => vec![Content::text(other.to_string())],
    };
    Message::User {
        content: blocks,
        timestamp,
    }
}

impl Serialize for AgentMessage {
    /// Emits the role discriminant first, then payload fields in Pi declaration order, so a
    /// bash/custom-role message round-trips byte-faithfully. `Core` delegates to
    /// [`cyrup_core::Message`]'s own role-tagged serializer.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        match self {
            AgentMessage::Core(m) => m.serialize(s),
            AgentMessage::BashExecution(b) => {
                let len = 6
                    + usize::from(b.exit_code.is_some())
                    + usize::from(b.full_output_path.is_some())
                    + usize::from(b.exclude_from_context.is_some());
                let mut map = s.serialize_map(Some(len))?;
                map.serialize_entry("role", "bashExecution")?;
                map.serialize_entry("command", &b.command)?;
                map.serialize_entry("output", &b.output)?;
                if let Some(code) = &b.exit_code {
                    map.serialize_entry("exitCode", code)?;
                }
                map.serialize_entry("cancelled", &b.cancelled)?;
                map.serialize_entry("truncated", &b.truncated)?;
                if let Some(p) = &b.full_output_path {
                    map.serialize_entry("fullOutputPath", p)?;
                }
                map.serialize_entry("timestamp", &b.timestamp)?;
                if let Some(x) = &b.exclude_from_context {
                    map.serialize_entry("excludeFromContext", x)?;
                }
                map.end()
            }
            AgentMessage::Custom(c) => {
                let len = 4 + usize::from(c.details.is_some());
                let mut map = s.serialize_map(Some(len))?;
                map.serialize_entry("role", "custom")?;
                map.serialize_entry("customType", &c.custom_type)?;
                map.serialize_entry("content", &c.content)?;
                map.serialize_entry("display", &c.display)?;
                if let Some(d) = &c.details {
                    map.serialize_entry("details", d)?;
                }
                map.serialize_entry("timestamp", &c.timestamp)?;
                map.end()
            }
            AgentMessage::BranchSummary(b) => {
                let mut map = s.serialize_map(Some(4))?;
                map.serialize_entry("role", "branchSummary")?;
                map.serialize_entry("summary", &b.summary)?;
                map.serialize_entry("fromId", &b.from_id)?;
                map.serialize_entry("timestamp", &b.timestamp)?;
                map.end()
            }
            AgentMessage::CompactionSummary(c) => {
                let mut map = s.serialize_map(Some(4))?;
                map.serialize_entry("role", "compactionSummary")?;
                map.serialize_entry("summary", &c.summary)?;
                map.serialize_entry("tokensBefore", &c.tokens_before)?;
                map.serialize_entry("timestamp", &c.timestamp)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for AgentMessage {
    /// Dispatches on `role`: `bashExecution`/`custom` parse into the extended arms; everything else
    /// delegates to [`cyrup_core::Message`] (which rejects unknown roles, keeping the entry
    /// strict-but-tolerant via [`crate::entry::Entry`]'s `Unknown` fallback).
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        match v.get("role").and_then(Value::as_str) {
            Some("bashExecution") => {
                let b =
                    serde_json::from_value::<BashExecutionMessage>(v).map_err(D::Error::custom)?;
                Ok(AgentMessage::BashExecution(b))
            }
            Some("custom") => {
                let c = serde_json::from_value::<CustomRoleMessage>(v).map_err(D::Error::custom)?;
                Ok(AgentMessage::Custom(c))
            }
            Some("branchSummary") => {
                let b =
                    serde_json::from_value::<BranchSummaryMessage>(v).map_err(D::Error::custom)?;
                Ok(AgentMessage::BranchSummary(b))
            }
            Some("compactionSummary") => {
                let c = serde_json::from_value::<CompactionSummaryMessage>(v)
                    .map_err(D::Error::custom)?;
                Ok(AgentMessage::CompactionSummary(c))
            }
            _ => {
                let m = serde_json::from_value::<Message>(v).map_err(D::Error::custom)?;
                Ok(AgentMessage::Core(m))
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::collapsible_if
)]
mod tests {
    use super::*;

    #[test]
    fn bash_execution_roundtrips_and_renders() {
        let json = serde_json::json!({
            "role": "bashExecution",
            "command": "ls -la",
            "output": "file.txt",
            "exitCode": 0,
            "cancelled": false,
            "truncated": false,
            "timestamp": 7,
        });
        let m: AgentMessage = serde_json::from_value(json.clone()).unwrap();
        assert!(matches!(m, AgentMessage::BashExecution(_)));
        // Round-trips (role preserved).
        let back = serde_json::to_value(&m).unwrap();
        assert_eq!(back["role"], "bashExecution");
        assert_eq!(back["command"], "ls -la");
        // Renders to a user message (Pi bashExecutionToText).
        let mut out = Vec::new();
        m.push_llm(&mut out);
        assert_eq!(out.len(), 1);
        if let Message::User { content, .. } = &out[0] {
            if let Content::Text { text, .. } = &content[0] {
                assert!(text.starts_with("Ran `ls -la`"));
                assert!(text.contains("```\nfile.txt\n```"));
            } else {
                panic!("expected text");
            }
        } else {
            panic!("expected user message");
        }
    }

    #[test]
    fn bash_exclude_from_context_drops_from_llm() {
        let m = AgentMessage::BashExecution(BashExecutionMessage {
            command: "secret".into(),
            output: "x".into(),
            exit_code: None,
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 0,
            exclude_from_context: Some(true),
        });
        let mut out = Vec::new();
        m.push_llm(&mut out);
        assert!(
            out.is_empty(),
            "!! bash messages are excluded from LLM context"
        );
    }

    #[test]
    fn bash_nonzero_exit_and_truncation_text() {
        let m = AgentMessage::BashExecution(BashExecutionMessage {
            command: "false".into(),
            output: String::new(),
            exit_code: Some(1),
            cancelled: false,
            truncated: true,
            full_output_path: Some("/tmp/out".into()),
            timestamp: 0,
            exclude_from_context: None,
        });
        let mut out = Vec::new();
        m.push_llm(&mut out);
        if let Message::User { content, .. } = &out[0] {
            if let Content::Text { text, .. } = &content[0] {
                assert!(text.contains("(no output)"));
                assert!(text.contains("Command exited with code 1"));
                assert!(text.contains("[Output truncated. Full output: /tmp/out]"));
                return;
            }
        }
        panic!("expected user text message");
    }

    #[test]
    fn custom_role_roundtrips_and_renders() {
        let json = serde_json::json!({
            "role": "custom",
            "customType": "ext.note",
            "content": "hello",
            "display": true,
            "timestamp": 3,
        });
        let m: AgentMessage = serde_json::from_value(json).unwrap();
        assert!(matches!(m, AgentMessage::Custom(_)));
        let back = serde_json::to_value(&m).unwrap();
        assert_eq!(back["role"], "custom");
        assert_eq!(back["customType"], "ext.note");
        let mut out = Vec::new();
        m.push_llm(&mut out);
        assert_eq!(
            out,
            vec![Message::User {
                content: vec![Content::text("hello")],
                timestamp: 3
            }]
        );
    }

    #[test]
    fn core_roles_pass_through() {
        let json = serde_json::json!({ "role": "user", "content": "hi", "timestamp": 1 });
        let m: AgentMessage = serde_json::from_value(json).unwrap();
        assert!(matches!(m, AgentMessage::Core(Message::User { .. })));
        let mut out = Vec::new();
        m.push_llm(&mut out);
        assert_eq!(
            out,
            vec![Message::User {
                content: vec![Content::text("hi")],
                timestamp: 1
            }]
        );
    }
}

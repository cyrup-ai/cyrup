//! The extended message model carried inside a `type:"message"` entry (Pi `AgentMessage`,
//! `messages.ts:8,69-77`). Pi stores **every** message role inside a `SessionMessageEntry.message`
//! field — not just `user`/`assistant`/`toolResult`, but also the `bashExecution` (`!` shell
//! command, `messages.ts:29-40`) and `custom` (extension `sendMessage`, `messages.ts:46-53`) roles.
//!
//! `cyrup_core::Message` is a closed `user`/`assistant`/`toolResult` enum, so a bash/custom-role
//! message cannot live there. [`AgentMessage`] is the cyrup-session-local superset used ONLY as the
//! inner type of [`crate::entry::KnownEntry::Message`]; it keeps such entries parseable (instead of
//! degrading to `Entry::Unknown` and being silently dropped from context / cut-points / token
//! estimation), and renders them to LLM-form `user` messages exactly like Pi's `convertToLlm`
//! (`messages.ts:148-195`).

use cyrup_core::{Content, Message};
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

/// The superset of message roles Pi stores inside a `type:"message"` entry (Pi `AgentMessage`).
/// `Core` covers `user`/`assistant`/`toolResult` ([`cyrup_core::Message`]); the extra arms carry the
/// `bashExecution`/`custom` roles that the closed core enum cannot represent.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentMessage {
    Core(Message),
    BashExecution(BashExecutionMessage),
    Custom(CustomRoleMessage),
}

impl AgentMessage {
    /// Convenience: wrap a core message.
    pub fn core(m: Message) -> Self {
        AgentMessage::Core(m)
    }

    /// `true` for the core `assistant` role (turn/model detection).
    pub fn is_core_assistant(&self) -> bool {
        matches!(self, AgentMessage::Core(Message::Assistant(_)))
    }

    /// `true` for the core `toolResult` role (never a valid cut point — Pi `compaction.ts:321-322`).
    pub fn is_tool_result(&self) -> bool {
        matches!(self, AgentMessage::Core(Message::ToolResult { .. }))
    }

    /// `true` for a turn-start role: core `user`, or `bashExecution` (Pi treats bash as a
    /// user/turn boundary — `compaction.ts:313,359`).
    pub fn is_turn_start(&self) -> bool {
        matches!(self, AgentMessage::Core(Message::User { .. }) | AgentMessage::BashExecution(_))
    }

    /// Render to the LLM `user`-form message(s), per Pi `convertToLlm` (`messages.ts:148-195`).
    /// `Core` is passed through verbatim; a `bashExecution` becomes a `user` text message (or is
    /// dropped when `excludeFromContext`); a `custom` message unwraps its `content` to a `user`
    /// message. Pushes nothing for a dropped bash message.
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
        }
    }
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
        other => vec![Content::text(other.to_string())],
    };
    Message::User { content: blocks, timestamp }
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
                let b = serde_json::from_value::<BashExecutionMessage>(v)
                    .map_err(D::Error::custom)?;
                Ok(AgentMessage::BashExecution(b))
            }
            Some("custom") => {
                let c =
                    serde_json::from_value::<CustomRoleMessage>(v).map_err(D::Error::custom)?;
                Ok(AgentMessage::Custom(c))
            }
            _ => {
                let m = serde_json::from_value::<Message>(v).map_err(D::Error::custom)?;
                Ok(AgentMessage::Core(m))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, clippy::collapsible_if)]
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
        assert!(out.is_empty(), "!! bash messages are excluded from LLM context");
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
        assert_eq!(out, vec![Message::User { content: vec![Content::text("hello")], timestamp: 3 }]);
    }

    #[test]
    fn core_roles_pass_through() {
        let json = serde_json::json!({ "role": "user", "content": "hi", "timestamp": 1 });
        let m: AgentMessage = serde_json::from_value(json).unwrap();
        assert!(matches!(m, AgentMessage::Core(Message::User { .. })));
        let mut out = Vec::new();
        m.push_llm(&mut out);
        assert_eq!(out, vec![Message::User { content: vec![Content::text("hi")], timestamp: 1 }]);
    }
}

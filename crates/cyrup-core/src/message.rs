//! The message & content model (arch-00 §3.3; conformance: func-01 §4).
//!
//! Serde follows arch-00 §4: structs use `rename_all = "camelCase"`; tagged enums add
//! `rename_all_fields = "camelCase"` so payload fields are camelCase for Pi-interop (R-00-013).

use crate::{ApiId, ModelId, ModelRef, ProviderId, ToolCallId};

/// Unified reasoning level (func-01 §12).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

/// How a generation ended (func-01 §9).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

/// A model-issued tool call (func-01 §4.4).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
    /// Provider-opaque (Google); stripped on cross-provider handoff (func-01 R-01-030).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thought_signature: Option<String>,
}

/// A typed content block (func-01 §4.4).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Content {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        text_signature: Option<String>,
    },
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        thinking_signature: Option<String>,
        #[serde(default)]
        redacted: bool,
    },
    ToolCall(ToolCall),
    Image {
        /// base64-encoded.
        data: String,
        mime_type: String,
    },
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text { text: s.into(), text_signature: None }
    }
    pub fn thinking(s: impl Into<String>) -> Self {
        Content::Thinking { thinking: s.into(), thinking_signature: None, redacted: false }
    }
}

/// Token + cost accounting (func-01 §4.5).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_write_1h: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning: Option<u64>,
    pub total_tokens: u64,
    pub cost: Cost,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

/// A completed assistant turn (func-01 §4.3).
///
/// Pi-interop flat shape: `provider`/`model`/`api` are separate fields (not a nested `ModelRef`);
/// use [`AssistantMessage::model_ref`] for handoff-equality.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: Vec<Content>,
    pub provider: ProviderId,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub api: Option<ApiId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub response_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub response_id: Option<String>,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error_message: Option<String>,
    pub timestamp: i64,
}

impl AssistantMessage {
    pub fn model_ref(&self) -> ModelRef {
        ModelRef {
            provider: self.provider.clone(),
            api: self.api.clone(),
            model: ModelId::from(self.model.as_str()),
        }
    }

    /// Build a terminal error/aborted assistant message (func-01 R-01-045).
    pub fn errored(
        provider: ProviderId,
        model: impl Into<String>,
        stop_reason: StopReason,
        message: impl Into<String>,
    ) -> Self {
        Self {
            content: Vec::new(),
            provider,
            model: model.into(),
            api: None,
            response_model: None,
            response_id: None,
            usage: Usage::default(),
            stop_reason,
            error_message: Some(message.into()),
            timestamp: 0,
        }
    }
}

/// A conversation message (func-01 §4.2). Custom (extension/app) message types live in
/// `cyrup-agent`'s `AgentMessage` wrapper and are filtered before the model call (func-02).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "role", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Message {
    User {
        content: Vec<Content>,
        timestamp: i64,
    },
    Assistant(AssistantMessage),
    ToolResult {
        tool_call_id: ToolCallId,
        tool_name: String,
        content: Vec<Content>,
        #[serde(default)]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        details: Option<serde_json::Value>,
        timestamp: i64,
    },
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn content_serializes_camelcase_tagged() {
        let c = Content::Thinking {
            thinking: "hm".into(),
            thinking_signature: Some("sig".into()),
            redacted: false,
        };
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v["type"], "thinking");
        assert_eq!(v["thinking"], "hm");
        assert_eq!(v["thinkingSignature"], "sig");
    }

    #[test]
    fn tool_result_message_uses_camelcase_fields() {
        let m = Message::ToolResult {
            tool_call_id: "tc1".into(),
            tool_name: "read".into(),
            content: vec![Content::text("ok")],
            is_error: false,
            details: None,
            timestamp: 0,
        };
        let v = serde_json::to_value(&m).expect("serialize");
        assert_eq!(v["role"], "toolResult");
        assert_eq!(v["toolCallId"], "tc1");
        assert_eq!(v["toolName"], "read");
        assert_eq!(v["isError"], false);
    }

    #[test]
    fn assistant_message_roundtrips_and_tags_role() {
        let m = Message::Assistant(AssistantMessage {
            content: vec![Content::text("hi")],
            provider: "faux".into(),
            model: "faux-1".into(),
            api: Some("faux".into()),
            response_model: None,
            response_id: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        });
        let v = serde_json::to_value(&m).expect("serialize");
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["stopReason"], "stop");
        let back: Message = serde_json::from_value(v).expect("deserialize");
        assert_eq!(back, m);
    }
}

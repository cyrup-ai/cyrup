//! Wire protocol — the bandwidth-reduced events the server emits (proxy.ts:36-57).

use cyrup_core::Usage;
use cyrup_provider::stream::{DoneReason, ErrorReason};

/// The server-sent proxy event (Pi `ProxyAssistantMessageEvent`, proxy.ts:36-57). The `partial`
/// field is stripped server-side; the client reconstructs it. The `type` discriminant is byte-1:1
/// with Pi's literal tags (note `toolcall_*` has NO boundary between `tool` and `call`, exactly like
/// [`cyrup_provider::StreamEvent`]); payload fields stay camelCase via `rename_all_fields`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum ProxyAssistantMessageEvent {
    #[serde(rename = "start")]
    Start,
    #[serde(rename = "text_start")]
    TextStart { content_index: usize },
    #[serde(rename = "text_delta")]
    TextDelta { content_index: usize, delta: String },
    #[serde(rename = "text_end")]
    TextEnd {
        content_index: usize,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        content_signature: Option<String>,
    },
    #[serde(rename = "thinking_start")]
    ThinkingStart { content_index: usize },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { content_index: usize, delta: String },
    #[serde(rename = "thinking_end")]
    ThinkingEnd {
        content_index: usize,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        content_signature: Option<String>,
    },
    #[serde(rename = "toolcall_start")]
    ToolCallStart {
        content_index: usize,
        id: String,
        tool_name: String,
    },
    #[serde(rename = "toolcall_delta")]
    ToolCallDelta { content_index: usize, delta: String },
    #[serde(rename = "toolcall_end")]
    ToolCallEnd { content_index: usize },
    /// Terminal: normal completion. `reason` ∈ {stop, length, toolUse} (Pi narrows `done.reason`,
    /// proxy.ts:49).
    #[serde(rename = "done")]
    Done { reason: DoneReason, usage: Usage },
    /// Terminal: error/abort. `reason` ∈ {error, aborted} (Pi narrows `error.reason`, proxy.ts:54).
    #[serde(rename = "error")]
    Error {
        reason: ErrorReason,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        error_message: Option<String>,
        usage: Usage,
    },
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::proxy::{ev, usage_json};

    #[test]
    fn wire_enum_deserializes_pi_camelcase_tags() {
        assert_eq!(
            ev(serde_json::json!({"type": "start"})),
            ProxyAssistantMessageEvent::Start
        );
        assert_eq!(
            ev(serde_json::json!({"type": "text_delta", "contentIndex": 2, "delta": "hi"})),
            ProxyAssistantMessageEvent::TextDelta {
                content_index: 2,
                delta: "hi".into()
            }
        );
        assert_eq!(
            ev(
                serde_json::json!({"type": "toolcall_start", "contentIndex": 0, "id": "t1", "toolName": "read"})
            ),
            ProxyAssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "t1".into(),
                tool_name: "read".into()
            }
        );
        // `done.reason` is the narrowed DoneReason ("toolUse"); `error.reason` the ErrorReason.
        assert_eq!(
            ev(serde_json::json!({"type": "done", "reason": "toolUse", "usage": usage_json()})),
            ProxyAssistantMessageEvent::Done {
                reason: DoneReason::ToolUse,
                usage: Usage {
                    input: 10,
                    output: 20,
                    total_tokens: 30,
                    ..Usage::default()
                }
            }
        );
        assert!(matches!(
            ev(
                serde_json::json!({"type": "error", "reason": "aborted", "errorMessage": "x", "usage": usage_json()})
            ),
            ProxyAssistantMessageEvent::Error {
                reason: ErrorReason::Aborted,
                ..
            }
        ));
    }
}

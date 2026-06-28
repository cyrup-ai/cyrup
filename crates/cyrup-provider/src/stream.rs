//! The streaming event model + per-request options (arch-01 §8 / func-01 §8).

use cyrup_core::{
    AssistantMessage, CancelToken, EventStream, ProviderId, SessionId, StopReason, ThinkingLevel,
    ToolCall,
};
use futures::StreamExt;

/// Direct-wire HTTP + SSE transport (arch-01 §7.1). Submodule of `stream` so the request side and
/// the event side share one module root.
pub mod sse;

/// Prompt-cache retention preference (func-01 §11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheRetention {
    None,
    #[default]
    Short,
    Long,
}

/// Per-request options (func-01 §13). Errors never throw; cancellation is delivered as a terminal
/// `StreamEvent::Error` with `stop_reason: Aborted` (func-01 R-01-044).
#[derive(Clone, Default)]
pub struct StreamOptions {
    pub cancel: Option<CancelToken>,
    pub api_key: Option<String>,
    /// Forwarded for cache routing / session affinity (func-01 R-01-039).
    pub session_id: Option<SessionId>,
    pub cache_retention: CacheRetention,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u64>,
    /// Unified reasoning level (func-01 R-01-040). Additive, backward-compatible (defaulted to
    /// `Off`); a non-reasoning model silently ignores it (R-01-041).
    pub reasoning: ThinkingLevel,
    /// Per-request header overlay; a `None` value suppresses a default header (func-01 §4.1).
    pub headers: Option<crate::HeaderMap>,
}

/// One streaming event (func-01 §8.1).
///
/// Ordering (func-01 §8.2): first event is `Start`; each content block at `content_index` follows
/// `*Start → (*Delta)* → *End`; exactly one terminal (`Done` or `Error`) closes the stream.
///
/// Slice note: the per-event `partial` snapshot (func-01 R-01-022) is deferred — consumers
/// reconstruct the message from deltas, and the terminal event carries the full `AssistantMessage`.
// Serde added so `cyrup-agent`'s `AgentEvent::MessageUpdate` (which carries a `StreamEvent`
// delta as `assistantMessageEvent`) can derive Serialize/Deserialize for the json/rpc wire
// (func-02 R-02-009 / arch-02 §3.1). Additive, backward-compatible (no field/variant renames
// beyond the camelCase tagging that matches arch-00 §4).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum StreamEvent {
    Start,
    TextStart { content_index: usize },
    TextDelta { content_index: usize, delta: String },
    TextEnd { content_index: usize, content: String },
    ThinkingStart { content_index: usize },
    ThinkingDelta { content_index: usize, delta: String },
    ThinkingEnd { content_index: usize, content: String },
    ToolCallStart { content_index: usize },
    ToolCallDelta { content_index: usize, delta: String },
    ToolCallEnd { content_index: usize, tool_call: ToolCall },
    /// Terminal: normal completion. `message.stop_reason` ∈ {stop, length, toolUse}.
    Done { message: AssistantMessage },
    /// Terminal: error/abort. `message.stop_reason` ∈ {error, aborted}.
    Error { message: AssistantMessage },
}

impl StreamEvent {
    /// The final message iff this is a terminal event (func-01 R-01-023).
    pub fn terminal_message(&self) -> Option<&AssistantMessage> {
        match self {
            StreamEvent::Done { message } | StreamEvent::Error { message } => Some(message),
            _ => None,
        }
    }
}

/// Drain a stream to its terminal event and return the final message (func-01 R-01-005/023).
/// Never panics: a stream that ends without a terminal event yields a synthesized error message.
pub async fn collect_message(mut stream: EventStream<StreamEvent>) -> AssistantMessage {
    let mut last: Option<AssistantMessage> = None;
    while let Some(ev) = stream.next().await {
        if let Some(msg) = ev.terminal_message() {
            last = Some(msg.clone());
        }
    }
    last.unwrap_or_else(|| {
        AssistantMessage::errored(
            ProviderId::from("unknown"),
            "unknown",
            StopReason::Error,
            "stream ended without a terminal event",
        )
    })
}

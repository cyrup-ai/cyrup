//! The streaming event model + per-request options (arch-01 §8 / func-01 §8).

use cyrup_core::{
    AssistantMessage, CancelToken, EventStream, ProviderId, SessionId, StopReason, ModelThinkingLevel,
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

/// Caller-specified tool-choice constraint (Pi `OpenAICompletionsOptions.toolChoice`:
/// `"auto" | "none" | "required" | { type: "function"; function: { name } }`). When `None`, the
/// wire impl omits `tool_choice` entirely (matching Pi's default — it never auto-injects `"auto"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Function { name: String },
}

impl ToolChoice {
    /// The OpenAI `tool_choice` wire JSON for this choice.
    pub fn to_wire(&self) -> serde_json::Value {
        match self {
            ToolChoice::Auto => serde_json::Value::String("auto".to_string()),
            ToolChoice::None => serde_json::Value::String("none".to_string()),
            ToolChoice::Required => serde_json::Value::String("required".to_string()),
            ToolChoice::Function { name } => serde_json::json!({
                "type": "function",
                "function": { "name": name },
            }),
        }
    }
}

/// Per-request options (func-01 §13). Errors never throw; cancellation is delivered as a terminal
/// `StreamEvent::Error` with `stop_reason: Aborted` (func-01 R-01-044).
#[derive(Clone, Default)]
pub struct StreamOptions {
    pub cancel: Option<CancelToken>,
    pub api_key: Option<String>,
    /// Forwarded for cache routing / session affinity (func-01 R-01-039).
    pub session_id: Option<SessionId>,
    /// Caller-specified prompt-cache retention. `None` = unset: the encoder then consults the
    /// `PI_CACHE_RETENTION` env var (Pi `resolveCacheRetention`, openai-completions.ts:141-149).
    /// An explicit `Some(_)` always wins over the env. Additive, backward-compatible (defaults to
    /// `None`, which resolves to `Short` unless the env promotes it to `Long`).
    pub cache_retention: Option<CacheRetention>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u64>,
    /// Unified reasoning level (func-01 R-01-040). Additive, backward-compatible (defaulted to
    /// `Off`); a non-reasoning model silently ignores it (R-01-041).
    pub reasoning: ModelThinkingLevel,
    /// Per-request header overlay; a `None` value suppresses a default header (func-01 §4.1).
    pub headers: Option<crate::HeaderMap>,
    /// Optional tool-choice constraint (Pi `OpenAICompletionsOptions.toolChoice`). Additive,
    /// backward-compatible (defaults to `None`, which omits the `tool_choice` field).
    pub tool_choice: Option<ToolChoice>,
}

/// One streaming event (func-01 §8.1; 1:1 with Pi `AssistantMessageEvent`, types.ts:453-465).
///
/// Ordering (func-01 §8.2): first event is `Start`; each content block at `content_index` follows
/// `*Start → (*Delta)* → *End`; exactly one terminal (`Done` or `Error`) closes the stream.
///
/// Every NON-terminal variant carries a `partial: AssistantMessage` — the live snapshot of the
/// message assembled so far (Pi `partial: AssistantMessage` on each event, types.ts:454-463;
/// func-01 R-01-022) — so consumers render the growing message without reconstructing it from
/// deltas. The terminals carry the `reason` discriminant plus the full message (Pi
/// `{type:"done", reason, message}` / `{type:"error", reason, error}`, types.ts:464-465).
// Serde so `cyrup-agent`'s `AgentEvent::MessageUpdate` (which carries a `StreamEvent` delta as
// `assistantMessageEvent`) can derive Serialize/Deserialize for the json/rpc wire (func-02
// R-02-009 / arch-02 §3.1). The `type` discriminant is byte-1:1 with Pi's `AssistantMessageEvent`
// literal tags (types.ts:453-465): `start`, `text_start`/`text_delta`/`text_end`,
// `thinking_start`/`thinking_delta`/`thinking_end`, `toolcall_start`/`toolcall_delta`/`toolcall_end`,
// `done`, `error`. These are lowercase-with-underscore (note `toolcall_*` has NO boundary between
// `tool` and `call`), so neither serde's `camelCase` NOR `snake_case` reproduces them — each
// non-`start`/`done`/`error` variant carries an explicit `#[serde(rename = "…")]`. Payload FIELDS
// stay camelCase (Pi `contentIndex`/`partial`/`toolCall`/…), via `rename_all_fields`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum StreamEvent {
    #[serde(rename = "start")]
    Start { partial: AssistantMessage },
    #[serde(rename = "text_start")]
    TextStart { content_index: usize, partial: AssistantMessage },
    #[serde(rename = "text_delta")]
    TextDelta { content_index: usize, delta: String, partial: AssistantMessage },
    #[serde(rename = "text_end")]
    TextEnd { content_index: usize, content: String, partial: AssistantMessage },
    #[serde(rename = "thinking_start")]
    ThinkingStart { content_index: usize, partial: AssistantMessage },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { content_index: usize, delta: String, partial: AssistantMessage },
    #[serde(rename = "thinking_end")]
    ThinkingEnd { content_index: usize, content: String, partial: AssistantMessage },
    #[serde(rename = "toolcall_start")]
    ToolCallStart { content_index: usize, partial: AssistantMessage },
    #[serde(rename = "toolcall_delta")]
    ToolCallDelta { content_index: usize, delta: String, partial: AssistantMessage },
    /// Pi `toolcall_end` (types.ts:463). The `tool_call` field carries Pi's `type:"toolCall"`
    /// discriminant first (Pi `ToolCall.type`, types.ts:345) because [`ToolCall`] now self-tags via
    /// its own [`serde::Serialize`] impl — the single source of the discriminant. Deserialize uses
    /// `ToolCall`'s derived impl, which tolerates the extra `type` key.
    #[serde(rename = "toolcall_end")]
    ToolCallEnd { content_index: usize, tool_call: ToolCall, partial: AssistantMessage },
    /// Terminal: normal completion. `reason` ∈ {stop, length, toolUse}; `message.stop_reason` matches.
    #[serde(rename = "done")]
    Done { reason: StopReason, message: AssistantMessage },
    /// Terminal: error/abort. `reason` ∈ {error, aborted}; the final message is keyed `error` (Pi).
    #[serde(rename = "error")]
    Error { reason: StopReason, error: AssistantMessage },
}

impl StreamEvent {
    /// The final message iff this is a terminal event (func-01 R-01-023).
    pub fn terminal_message(&self) -> Option<&AssistantMessage> {
        match self {
            StreamEvent::Done { message, .. } => Some(message),
            StreamEvent::Error { error, .. } => Some(error),
            _ => None,
        }
    }

    /// The per-event `partial` snapshot for a non-terminal event (Pi `event.partial`); `None` for
    /// the terminals (which carry the full `message`/`error` instead).
    pub fn partial(&self) -> Option<&AssistantMessage> {
        match self {
            StreamEvent::Start { partial }
            | StreamEvent::TextStart { partial, .. }
            | StreamEvent::TextDelta { partial, .. }
            | StreamEvent::TextEnd { partial, .. }
            | StreamEvent::ThinkingStart { partial, .. }
            | StreamEvent::ThinkingDelta { partial, .. }
            | StreamEvent::ThinkingEnd { partial, .. }
            | StreamEvent::ToolCallStart { partial, .. }
            | StreamEvent::ToolCallDelta { partial, .. }
            | StreamEvent::ToolCallEnd { partial, .. } => Some(partial),
            StreamEvent::Done { .. } | StreamEvent::Error { .. } => None,
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
            None,
            StopReason::Error,
            "stream ended without a terminal event",
        )
    })
}

/// A push-driven [`StreamEvent`] stream that resolves to the final [`AssistantMessage`] — the
/// extension-facing authoring path (1:1 with Pi `AssistantMessageEventStream` +
/// `createAssistantMessageEventStream`, event-stream.ts:69-88). Specializes cyrup-core's generic
/// [`cyrup_core::FinalizingStream`] over `StreamEvent`/`AssistantMessage`, keying completion on the
/// `Done`/`Error` terminals and extracting their message (Pi `isComplete`/`extractResult`).
pub type AssistantMessageEventStream =
    cyrup_core::FinalizingStream<StreamEvent, AssistantMessage>;

/// The producer half an extension drives to author an [`AssistantMessageEventStream`].
pub type AssistantMessageEventSink = cyrup_core::FinalizingSink<StreamEvent, AssistantMessage>;

/// Create an [`AssistantMessageEventStream`] for extensions to drive (Pi
/// `createAssistantMessageEventStream()`, event-stream.ts:85-88). The sink's `push`/`end` feed the
/// stream; `result()` resolves to the terminal message (or a synthesized error if it ends without
/// a terminal, matching [`collect_message`]'s no-panic policy).
pub fn create_assistant_message_event_stream(
) -> (AssistantMessageEventSink, AssistantMessageEventStream) {
    cyrup_core::finalizing_channel(
        |e: &StreamEvent| matches!(e, StreamEvent::Done { .. } | StreamEvent::Error { .. }),
        |e: &StreamEvent| {
            e.terminal_message().cloned().unwrap_or_else(synth_terminal_less_message)
        },
        synth_terminal_less_message,
    )
}

/// The synthesized final message for a stream that ended without a terminal (shared by
/// [`collect_message`] and [`create_assistant_message_event_stream`]).
fn synth_terminal_less_message() -> AssistantMessage {
    AssistantMessage::errored(
        ProviderId::from("unknown"),
        "unknown",
        None,
        StopReason::Error,
        "stream ended without a terminal event",
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use cyrup_core::{ToolCallId, Usage};

    fn empty_partial() -> AssistantMessage {
        AssistantMessage {
            content: Vec::new(),
            provider: ProviderId::from("faux"),
            model: "faux-1".into(),
            api: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }
    }

    /// Gap 1: every `type` discriminant is byte-1:1 with Pi's `AssistantMessageEvent` literals
    /// (types.ts:453-465) — in particular the underscored `text_*`/`thinking_*`/`toolcall_*` tags,
    /// not serde's camelCase.
    #[test]
    fn stream_event_type_tags_are_pi_literals() {
        let p = empty_partial();
        let cases = [
            (StreamEvent::Start { partial: p.clone() }, "start"),
            (StreamEvent::TextStart { content_index: 0, partial: p.clone() }, "text_start"),
            (
                StreamEvent::TextDelta { content_index: 0, delta: "d".into(), partial: p.clone() },
                "text_delta",
            ),
            (
                StreamEvent::TextEnd { content_index: 0, content: "c".into(), partial: p.clone() },
                "text_end",
            ),
            (StreamEvent::ThinkingStart { content_index: 0, partial: p.clone() }, "thinking_start"),
            (
                StreamEvent::ThinkingDelta {
                    content_index: 0,
                    delta: "d".into(),
                    partial: p.clone(),
                },
                "thinking_delta",
            ),
            (
                StreamEvent::ThinkingEnd {
                    content_index: 0,
                    content: "c".into(),
                    partial: p.clone(),
                },
                "thinking_end",
            ),
            (StreamEvent::ToolCallStart { content_index: 0, partial: p.clone() }, "toolcall_start"),
            (
                StreamEvent::ToolCallDelta {
                    content_index: 0,
                    delta: "d".into(),
                    partial: p.clone(),
                },
                "toolcall_delta",
            ),
            (
                StreamEvent::Done { reason: StopReason::Stop, message: p.clone() },
                "done",
            ),
            (
                StreamEvent::Error { reason: StopReason::Error, error: p.clone() },
                "error",
            ),
        ];
        for (ev, tag) in cases {
            let v = serde_json::to_value(&ev).expect("serialize");
            assert_eq!(v["type"], tag, "wrong tag for {ev:?}");
            // Payload fields stay camelCase (Pi `contentIndex`/`partial`).
            let back: StreamEvent = serde_json::from_value(v).expect("roundtrip");
            assert_eq!(back, ev);
        }
    }

    /// Gap 2: `toolcall_end.toolCall` carries Pi's `type:"toolCall"` discriminant first, then
    /// `id`/`name`/`arguments`/`thoughtSignature?` in Pi declaration order (types.ts:344-350,463) —
    /// with no duplicate `type` key — and round-trips.
    #[test]
    fn toolcall_end_tool_call_carries_type_discriminant() {
        let ev = StreamEvent::ToolCallEnd {
            content_index: 0,
            tool_call: ToolCall {
                id: ToolCallId::from("tc1"),
                name: "read".into(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
            },
            partial: empty_partial(),
        };
        let s = serde_json::to_string(&ev).expect("serialize");
        assert_eq!(s.matches("\"type\"").count(), 2, "event tag + toolCall tag, no dup: {s}");
        let v: serde_json::Value = serde_json::from_str(&s).expect("json");
        assert_eq!(v["type"], "toolcall_end");
        assert_eq!(v["toolCall"]["type"], "toolCall");
        assert_eq!(v["toolCall"]["id"], "tc1");
        assert_eq!(v["toolCall"]["name"], "read");
        assert!(v["toolCall"]["arguments"].is_object());
        // `type` is emitted first, byte-1:1 with Pi's `ToolCall` field order.
        let tc = &s[s.find("\"toolCall\":{").expect("toolCall obj")..];
        assert!(tc.starts_with("\"toolCall\":{\"type\":\"toolCall\""), "{tc}");
        let back: StreamEvent = serde_json::from_str(&s).expect("roundtrip");
        assert_eq!(back, ev);
    }
}

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

/// Preferred transport for providers that support multiple transports (Pi `Transport`,
/// types.ts:98). Providers that do not support the option ignore it. `kebab-case` makes the wire
/// bytes byte-1:1 with Pi: `"sse"`, `"websocket"`, `"websocket-cached"`, `"auto"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    Sse,
    Websocket,
    WebsocketCached,
    Auto,
}

/// The HTTP response metadata handed to [`StreamOptions::on_response`] before the body is consumed
/// (Pi `ProviderResponse`, types.ts:104-107).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderResponse {
    pub status: u16,
    pub headers: std::collections::BTreeMap<String, String>,
}

/// Inspect or replace a provider payload before sending (Pi `StreamOptions.onPayload`,
/// types.ts:130-134). Returning `None` keeps the payload unchanged.
pub type OnPayload =
    std::sync::Arc<dyn Fn(&serde_json::Value, &crate::model::Model) -> Option<serde_json::Value> + Send + Sync>;

/// Invoked after an HTTP response is received and before its body stream is consumed (Pi
/// `StreamOptions.onResponse`, types.ts:135-139).
pub type OnResponseHook =
    std::sync::Arc<dyn Fn(&ProviderResponse, &crate::model::Model) + Send + Sync>;

/// Provider-scoped environment overrides (Pi `ProviderEnv`, types.ts:100-101). Values take
/// precedence over the process environment for provider configuration.
pub type ProviderEnv = std::collections::BTreeMap<String, String>;

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
    /// Per-level custom thinking token budgets for token-budget providers (Pi
    /// `SimpleStreamOptions.thinkingBudgets`, types.ts:293). `build_base_options` threads the unified
    /// `SimpleStreamOptions.thinking_budgets` here so the API wire (e.g. anthropic-messages'
    /// `adjustMaxTokensForThinking`, anthropic-messages.ts:792-797) can honor it. A non-budget
    /// provider ignores it. Additive, backward-compatible (defaults to `None`).
    pub thinking_budgets: Option<crate::utils::simple_options::ThinkingBudgets>,
    /// Per-request header overlay; a `None` value suppresses a default header (func-01 §4.1).
    pub headers: Option<crate::HeaderMap>,
    /// Optional tool-choice constraint (Pi `OpenAICompletionsOptions.toolChoice`). Additive,
    /// backward-compatible (defaults to `None`, which omits the `tool_choice` field).
    pub tool_choice: Option<ToolChoice>,
    /// Preferred transport for providers that support multiple transports (Pi
    /// `StreamOptions.transport`, types.ts:118). Providers that do not support it ignore it.
    pub transport: Option<Transport>,
    /// HTTP request timeout in milliseconds for providers/SDKs that support it (Pi
    /// `StreamOptions.timeoutMs`, types.ts:153).
    pub timeout_ms: Option<u64>,
    /// WebSocket connect (handshake) timeout in milliseconds for WebSocket transports (Pi
    /// `StreamOptions.websocketConnectTimeoutMs`, types.ts:159).
    pub websocket_connect_timeout_ms: Option<u64>,
    /// Maximum retry attempts for providers/SDKs that support client-side retries (Pi
    /// `StreamOptions.maxRetries`, types.ts:164).
    pub max_retries: Option<u32>,
    /// Maximum delay (ms) to wait for a server-requested retry before failing immediately (Pi
    /// `StreamOptions.maxRetryDelayMs`, types.ts:172). `Some(0)` disables the cap.
    pub max_retry_delay_ms: Option<u64>,
    /// Provider-extracted request metadata; providers take the fields they understand and ignore
    /// the rest (Pi `StreamOptions.metadata`, types.ts:178 — e.g. Anthropic `user_id`).
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    /// Provider-scoped environment overrides, taking precedence over the process environment (Pi
    /// `StreamOptions.env`, types.ts:184).
    pub env: Option<ProviderEnv>,
    /// Inspect or replace the provider payload before sending (Pi `StreamOptions.onPayload`,
    /// types.ts:130). Additive; defaults to `None`.
    pub on_payload: Option<OnPayload>,
    /// Invoked after an HTTP response is received, before its body is consumed (Pi
    /// `StreamOptions.onResponse`, types.ts:135). Additive; defaults to `None`.
    pub on_response: Option<OnResponseHook>,
}

/// Terminal-`done` reason. Pi narrows the `done` event's `reason` to
/// `Extract<StopReason, "stop" | "length" | "toolUse">` (Pi types.ts:464), so cyrup mirrors that
/// with a dedicated enum rather than the full [`StopReason`] (arch-01 §3.3). `rename_all="camelCase"`
/// makes the wire bytes byte-1:1 with the matching [`StopReason`] values: `"stop"`, `"length"`,
/// `"toolUse"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DoneReason {
    Stop,
    Length,
    ToolUse,
}

/// Terminal-`error` reason. Pi narrows the `error` event's `reason` to
/// `Extract<StopReason, "aborted" | "error">` (Pi types.ts:465), mirrored here (arch-01 §3.3).
/// `rename_all="camelCase"` makes the wire bytes byte-1:1 with the matching [`StopReason`] values:
/// `"error"`, `"aborted"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorReason {
    Error,
    Aborted,
}

impl From<DoneReason> for StopReason {
    fn from(reason: DoneReason) -> Self {
        match reason {
            DoneReason::Stop => StopReason::Stop,
            DoneReason::Length => StopReason::Length,
            DoneReason::ToolUse => StopReason::ToolUse,
        }
    }
}

impl From<ErrorReason> for StopReason {
    fn from(reason: ErrorReason) -> Self {
        match reason {
            ErrorReason::Error => StopReason::Error,
            ErrorReason::Aborted => StopReason::Aborted,
        }
    }
}

impl TryFrom<StopReason> for DoneReason {
    /// A non-`done` [`StopReason`] (`error`/`aborted`) carries the [`ErrorReason`] it maps to, so the
    /// caller can route it straight to the `error` terminal without a separate lookup (and without
    /// ever panicking).
    type Error = ErrorReason;

    fn try_from(reason: StopReason) -> Result<Self, ErrorReason> {
        match reason {
            StopReason::Stop => Ok(DoneReason::Stop),
            StopReason::Length => Ok(DoneReason::Length),
            StopReason::ToolUse => Ok(DoneReason::ToolUse),
            StopReason::Error => Err(ErrorReason::Error),
            StopReason::Aborted => Err(ErrorReason::Aborted),
        }
    }
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
    /// Terminal: normal completion. `reason` ∈ {stop, length, toolUse} (Pi narrows the `done` reason
    /// to `Extract<StopReason,"stop"|"length"|"toolUse">`, types.ts:464); `message.stop_reason`
    /// matches.
    #[serde(rename = "done")]
    Done { reason: DoneReason, message: AssistantMessage },
    /// Terminal: error/abort. `reason` ∈ {error, aborted} (Pi narrows the `error` reason to
    /// `Extract<StopReason,"aborted"|"error">`, types.ts:465); the final message is keyed `error`.
    #[serde(rename = "error")]
    Error { reason: ErrorReason, error: AssistantMessage },
}

impl StreamEvent {
    /// Build the correct terminal event for a final `message`, narrowing `message.stop_reason` into a
    /// [`DoneReason`] (`done` terminal) or an [`ErrorReason`] (`error` terminal). The mapping is total
    /// and never panics: `error`/`aborted` route to the `error` terminal, every other reason to the
    /// `done` terminal — matching Pi's `done`/`error` split (types.ts:464-465).
    pub fn terminal(message: AssistantMessage) -> Self {
        match DoneReason::try_from(message.stop_reason) {
            Ok(reason) => StreamEvent::Done { reason, message },
            Err(reason) => StreamEvent::Error { reason, error: message },
        }
    }

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
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
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
                StreamEvent::Done { reason: DoneReason::Stop, message: p.clone() },
                "done",
            ),
            (
                StreamEvent::Error { reason: ErrorReason::Error, error: p.clone() },
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

    /// Gap 3: the terminal `reason` is narrowed to Pi's `Extract<StopReason,…>` subsets
    /// (types.ts:464-465: `done.reason ∈ {"stop","length","toolUse"}`,
    /// `error.reason ∈ {"error","aborted"}`) yet the emitted bytes stay byte-1:1 with the old full
    /// [`StopReason`] strings — and every value round-trips.
    #[test]
    fn terminal_reasons_are_pi_narrowed_subsets_and_byte_stable() {
        let p = empty_partial();
        // `done` reasons serialize EXACTLY as the matching `StopReason` did before the narrowing.
        let done_cases = [
            (DoneReason::Stop, StopReason::Stop, "stop"),
            (DoneReason::Length, StopReason::Length, "length"),
            (DoneReason::ToolUse, StopReason::ToolUse, "toolUse"),
        ];
        for (reason, stop, wire) in done_cases {
            let ev = StreamEvent::Done { reason, message: p.clone() };
            let v = serde_json::to_value(&ev).expect("serialize");
            assert_eq!(v["type"], "done");
            assert_eq!(v["reason"], wire, "done reason wire byte for {reason:?}");
            // Byte-identical to the full-`StopReason` encoding it replaced.
            assert_eq!(v["reason"], serde_json::to_value(stop).expect("stop"));
            let back: StreamEvent = serde_json::from_value(v).expect("roundtrip");
            assert_eq!(back, ev);
        }
        // `error` reasons likewise.
        let err_cases = [
            (ErrorReason::Error, StopReason::Error, "error"),
            (ErrorReason::Aborted, StopReason::Aborted, "aborted"),
        ];
        for (reason, stop, wire) in err_cases {
            let ev = StreamEvent::Error { reason, error: p.clone() };
            let v = serde_json::to_value(&ev).expect("serialize");
            assert_eq!(v["type"], "error");
            assert_eq!(v["reason"], wire, "error reason wire byte for {reason:?}");
            assert_eq!(v["reason"], serde_json::to_value(stop).expect("stop"));
            let back: StreamEvent = serde_json::from_value(v).expect("roundtrip");
            assert_eq!(back, ev);
        }
    }

    /// `StreamEvent::terminal` routes by `stop_reason`: stop/length/toolUse → `done` with the
    /// matching [`DoneReason`]; error/aborted → `error` with the matching [`ErrorReason`]. Total and
    /// never panics.
    #[test]
    fn terminal_routes_stop_reason_without_panic() {
        let mk = |stop: StopReason| {
            let mut m = empty_partial();
            m.stop_reason = stop;
            m
        };
        match StreamEvent::terminal(mk(StopReason::Stop)) {
            StreamEvent::Done { reason: DoneReason::Stop, .. } => {}
            other => panic!("expected done/stop, got {other:?}"),
        }
        match StreamEvent::terminal(mk(StopReason::ToolUse)) {
            StreamEvent::Done { reason: DoneReason::ToolUse, .. } => {}
            other => panic!("expected done/toolUse, got {other:?}"),
        }
        match StreamEvent::terminal(mk(StopReason::Error)) {
            StreamEvent::Error { reason: ErrorReason::Error, .. } => {}
            other => panic!("expected error/error, got {other:?}"),
        }
        match StreamEvent::terminal(mk(StopReason::Aborted)) {
            StreamEvent::Error { reason: ErrorReason::Aborted, .. } => {}
            other => panic!("expected error/aborted, got {other:?}"),
        }
    }
}

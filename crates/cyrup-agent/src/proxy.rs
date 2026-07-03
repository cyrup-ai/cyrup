//! Proxy `StreamFn` for apps that route LLM calls through an auth-managing server (1:1 port of Pi
//! `packages/agent/src/proxy.ts`, exported via `index.ts:42`).
//!
//! The server proxies the model call and streams back [`ProxyAssistantMessageEvent`]s with the heavy
//! `partial` snapshot **stripped** to save bandwidth (proxy.ts:33-34,84). The client rebuilds the
//! growing [`AssistantMessage`] locally — including streaming tool-call argument JSON via
//! [`cyrup_provider::parse_streaming_json_object`] (Pi `parseStreamingJson`, proxy.ts:324) — and
//! re-emits the full `cyrup_provider::StreamEvent` stream the agent loop already consumes.
//!
//! Transport reuses cyrup-provider's existing SSE client ([`cyrup_provider::open_sse`],
//! arch-01 §7.1) — the same `reqwest`+`rustls`+`eventsource-stream` path every direct provider uses
//! — so no new dependency is introduced. `POST {proxyUrl}/api/stream` with `Authorization: Bearer`
//! and the `{ model, context, options }` body matches Pi `streamProxy` (proxy.ts:152-164); the
//! `cancel` token drives the abort that Pi performs via `reader.cancel` (proxy.ts:141-145).

use crate::stream_fn::StreamFn;
use cyrup_core::{
    AssistantMessage, CancelToken, Content, EventStream, ModelRef, ToolCall, ToolCallId, Usage,
};
use cyrup_provider::stream::{DoneReason, ErrorReason};
use cyrup_provider::{
    build_client, open_sse, parse_streaming_json_object, CacheRetention, Context, HeaderMap,
    SseRequest, StreamEvent, StreamOptions, ThinkingBudgets, Transport,
};
use cyrup_core::{ModelThinkingLevel, SessionId, ThinkingLevel};
use futures::StreamExt;
use serde_json::{Map, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Wire protocol — the bandwidth-reduced events the server emits (proxy.ts:36-57).
// ---------------------------------------------------------------------------

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
    ToolCallStart { content_index: usize, id: String, tool_name: String },
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

// ---------------------------------------------------------------------------
// Client-side partial reconstruction (Pi `processProxyEvent` + `partial`, proxy.ts:121-367).
// ---------------------------------------------------------------------------

/// Rebuilds the streaming [`AssistantMessage`] from bandwidth-reduced proxy events (Pi keeps the
/// `partial` object + a per-tool-call `partialJson` side-field, proxy.ts:121-137,323-324). cyrup
/// holds the streaming tool-call arg JSON in a side map keyed by content index rather than mutating
/// the typed [`ToolCall`] (which has no `partialJson` field) — observably identical: the rebuilt
/// `arguments` map is refreshed on every delta exactly as Pi does.
pub struct ProxyMessageBuilder {
    partial: AssistantMessage,
    tool_json: HashMap<usize, String>,
}

impl ProxyMessageBuilder {
    /// Seed the empty partial from the model identity (Pi `partial: AssistantMessage = {...}`,
    /// proxy.ts:121-137). `stopReason` starts at `stop`; `usage` is zeroed; content is empty.
    pub fn new(model: &ModelRef) -> Self {
        Self { partial: empty_partial(model), tool_json: HashMap::new() }
    }

    /// The message assembled so far.
    pub fn partial(&self) -> &AssistantMessage {
        &self.partial
    }

    /// Process one proxy event, mutating the partial and returning the reconstructed
    /// [`StreamEvent`] to forward (Pi `processProxyEvent`, proxy.ts:238-367). Returns `Ok(None)` for
    /// a `toolcall_end` whose content slot is not a tool call (Pi returns `undefined`,
    /// proxy.ts:347). Returns `Err(msg)` for a delta/end whose content slot has the wrong type — Pi
    /// `throw`s the identical message (proxy.ts:261,275,293,307,333), which its outer loop turns into
    /// a terminal `error` event.
    pub fn process(
        &mut self,
        event: ProxyAssistantMessageEvent,
    ) -> Result<Option<StreamEvent>, String> {
        match event {
            ProxyAssistantMessageEvent::Start => {
                Ok(Some(StreamEvent::Start { partial: self.partial.clone() }))
            }

            ProxyAssistantMessageEvent::TextStart { content_index } => {
                self.set_content(content_index, Content::text(""));
                Ok(Some(StreamEvent::TextStart {
                    content_index,
                    partial: self.partial.clone(),
                }))
            }
            ProxyAssistantMessageEvent::TextDelta { content_index, delta } => {
                match self.partial.content.get_mut(content_index) {
                    Some(Content::Text { text, .. }) => text.push_str(&delta),
                    _ => return Err("Received text_delta for non-text content".into()),
                }
                Ok(Some(StreamEvent::TextDelta {
                    content_index,
                    delta,
                    partial: self.partial.clone(),
                }))
            }
            ProxyAssistantMessageEvent::TextEnd { content_index, content_signature } => {
                let text = match self.partial.content.get_mut(content_index) {
                    Some(Content::Text { text, text_signature }) => {
                        *text_signature = content_signature;
                        text.clone()
                    }
                    _ => return Err("Received text_end for non-text content".into()),
                };
                Ok(Some(StreamEvent::TextEnd {
                    content_index,
                    content: text,
                    partial: self.partial.clone(),
                }))
            }

            ProxyAssistantMessageEvent::ThinkingStart { content_index } => {
                self.set_content(content_index, Content::thinking(""));
                Ok(Some(StreamEvent::ThinkingStart {
                    content_index,
                    partial: self.partial.clone(),
                }))
            }
            ProxyAssistantMessageEvent::ThinkingDelta { content_index, delta } => {
                match self.partial.content.get_mut(content_index) {
                    Some(Content::Thinking { thinking, .. }) => thinking.push_str(&delta),
                    _ => return Err("Received thinking_delta for non-thinking content".into()),
                }
                Ok(Some(StreamEvent::ThinkingDelta {
                    content_index,
                    delta,
                    partial: self.partial.clone(),
                }))
            }
            ProxyAssistantMessageEvent::ThinkingEnd { content_index, content_signature } => {
                let thinking = match self.partial.content.get_mut(content_index) {
                    Some(Content::Thinking { thinking, thinking_signature, .. }) => {
                        *thinking_signature = content_signature;
                        thinking.clone()
                    }
                    _ => return Err("Received thinking_end for non-thinking content".into()),
                };
                Ok(Some(StreamEvent::ThinkingEnd {
                    content_index,
                    content: thinking,
                    partial: self.partial.clone(),
                }))
            }

            ProxyAssistantMessageEvent::ToolCallStart { content_index, id, tool_name } => {
                self.set_content(
                    content_index,
                    Content::ToolCall(ToolCall {
                        id: ToolCallId::from(id),
                        name: tool_name,
                        arguments: Map::new(),
                        thought_signature: None,
                    }),
                );
                self.tool_json.insert(content_index, String::new());
                Ok(Some(StreamEvent::ToolCallStart {
                    content_index,
                    partial: self.partial.clone(),
                }))
            }
            ProxyAssistantMessageEvent::ToolCallDelta { content_index, delta } => {
                let arguments = match self.partial.content.get(content_index) {
                    Some(Content::ToolCall(_)) => {
                        let buf = self.tool_json.entry(content_index).or_default();
                        buf.push_str(&delta);
                        // Re-parse the accumulated partial JSON on every delta, recovering as much of
                        // the (possibly-truncated) object as possible — Pi
                        // `parseStreamingJson(content.partialJson) || {}` (proxy.ts:324).
                        parse_streaming_json_object(Some(buf.as_str()))
                    }
                    _ => return Err("Received toolcall_delta for non-toolCall content".into()),
                };
                if let Some(Content::ToolCall(tc)) = self.partial.content.get_mut(content_index) {
                    tc.arguments = arguments;
                }
                Ok(Some(StreamEvent::ToolCallDelta {
                    content_index,
                    delta,
                    partial: self.partial.clone(),
                }))
            }
            ProxyAssistantMessageEvent::ToolCallEnd { content_index } => {
                // Drop the streaming-JSON side buffer (Pi `delete content.partialJson`, proxy.ts:339).
                self.tool_json.remove(&content_index);
                match self.partial.content.get(content_index) {
                    Some(Content::ToolCall(tc)) => {
                        let tool_call = tc.clone();
                        Ok(Some(StreamEvent::ToolCallEnd {
                            content_index,
                            tool_call,
                            partial: self.partial.clone(),
                        }))
                    }
                    // Pi returns `undefined` (no throw) for a non-toolCall slot (proxy.ts:347).
                    _ => Ok(None),
                }
            }

            ProxyAssistantMessageEvent::Done { reason, usage } => {
                self.partial.stop_reason = reason.into();
                self.partial.usage = usage;
                Ok(Some(StreamEvent::Done { reason, message: self.partial.clone() }))
            }
            ProxyAssistantMessageEvent::Error { reason, error_message, usage } => {
                self.partial.stop_reason = reason.into();
                self.partial.error_message = error_message;
                self.partial.usage = usage;
                Ok(Some(StreamEvent::Error { reason, error: self.partial.clone() }))
            }
        }
    }

    /// Assign `content` at `index`, growing the content vector with empty-text fillers if the server
    /// skips ahead (Pi relies on JS sparse-array assignment, proxy.ts:247). In practice the server
    /// emits contiguous indices, so no filler is observable.
    fn set_content(&mut self, index: usize, content: Content) {
        if index >= self.partial.content.len() {
            self.partial.content.resize(index + 1, Content::text(""));
        }
        if let Some(slot) = self.partial.content.get_mut(index) {
            *slot = content;
        }
    }
}

fn empty_partial(model: &ModelRef) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        provider: model.provider.clone(),
        model: model.model.to_string(),
        api: model
            .api
            .clone()
            .unwrap_or_else(|| cyrup_core::ApiId::from(cyrup_core::UNRESOLVED_API)),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: cyrup_core::StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

// ---------------------------------------------------------------------------
// Options + request body (Pi `ProxyStreamOptions` / `buildProxyRequestOptions`, proxy.ts:59-114).
// ---------------------------------------------------------------------------

/// Options for a single proxied request (Pi `ProxyStreamOptions`, proxy.ts:73-80). The serializable
/// subset (Pi `ProxySerializableStreamOptions`, proxy.ts:59-71) is forwarded in the request body;
/// `cancel`/`auth_token`/`proxy_url` are local-only. `reasoning` is the **unified** on-level and
/// `thinking_budgets` overrides per-level token budgets — the server lowers both via `streamSimple`,
/// so per-level token budgets ARE honored on the proxy path.
#[derive(Clone, Default)]
pub struct ProxyStreamOptions {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u64>,
    pub reasoning: Option<ThinkingLevel>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<SessionId>,
    pub headers: Option<HeaderMap>,
    pub metadata: Option<Map<String, Value>>,
    pub transport: Option<Transport>,
    pub thinking_budgets: Option<ThinkingBudgets>,
    pub max_retry_delay_ms: Option<u64>,
    /// Local abort signal for the proxy request (Pi `signal`, proxy.ts:74-75).
    pub cancel: Option<CancelToken>,
    /// Auth token for the proxy server (Pi `authToken`, proxy.ts:76-77).
    pub auth_token: String,
    /// Proxy server URL, e.g. `https://genai.example.com` (Pi `proxyUrl`, proxy.ts:78-79).
    pub proxy_url: String,
}

/// The serializable request-options body (Pi `ProxySerializableStreamOptions`, proxy.ts:59-71 /
/// `buildProxyRequestOptions`, proxy.ts:101-114). Only present fields are emitted, matching Pi's
/// `JSON.stringify` (which drops `undefined`).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyRequestOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ThinkingLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_retention: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    headers: Option<HeaderMap>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<Transport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budgets: Option<ProxyThinkingBudgets>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_retry_delay_ms: Option<u64>,
}

/// Wire mirror of [`cyrup_provider::ThinkingBudgets`] (which is not itself `Serialize`). Per-level
/// fields are omitted when unset, matching Pi `ThinkingBudgets` (types.ts:88-94).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyThinkingBudgets {
    #[serde(skip_serializing_if = "Option::is_none")]
    minimal: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    low: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    medium: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    high: Option<u64>,
}

fn cache_retention_wire(r: CacheRetention) -> &'static str {
    match r {
        CacheRetention::None => "none",
        CacheRetention::Short => "short",
        CacheRetention::Long => "long",
    }
}

/// Project the serializable request body from [`ProxyStreamOptions`] (Pi `buildProxyRequestOptions`,
/// proxy.ts:101-114).
fn build_proxy_request_options(options: &ProxyStreamOptions) -> ProxyRequestOptions {
    ProxyRequestOptions {
        temperature: options.temperature,
        max_tokens: options.max_tokens,
        reasoning: options.reasoning,
        cache_retention: options.cache_retention.map(cache_retention_wire),
        session_id: options.session_id.clone(),
        headers: options.headers.clone(),
        metadata: options.metadata.clone(),
        transport: options.transport,
        thinking_budgets: options.thinking_budgets.map(|b| ProxyThinkingBudgets {
            minimal: b.minimal,
            low: b.low,
            medium: b.medium,
            high: b.high,
        }),
        max_retry_delay_ms: options.max_retry_delay_ms,
    }
}

/// Serialize the `model` field of the request body. Pi sends the full `Model`; cyrup-agent's
/// provider-agnostic `StreamFn` seam only carries a [`ModelRef`], so the routing identity
/// (`provider`/`api`/`model`) is sent — a documented `[CYRUP-DELTA]`. ([`ModelRef`] is not itself
/// `Serialize`.)
fn model_wire(model: &ModelRef) -> Value {
    serde_json::json!({
        "provider": model.provider.as_str(),
        "api": model.api.as_ref().map(|a| a.as_str()),
        "model": model.model.as_str(),
    })
}

// ---------------------------------------------------------------------------
// Transport (Pi `streamProxy`, proxy.ts:116-233).
// ---------------------------------------------------------------------------

/// Stream a model call through an auth-managing proxy server (1:1 port of Pi `streamProxy`,
/// proxy.ts:116-233). `POST {proxyUrl}/api/stream` with `Authorization: Bearer {authToken}` and the
/// `{ model, context, options }` body; decode the SSE `data:` frames as
/// [`ProxyAssistantMessageEvent`]s; rebuild the partial client-side via [`ProxyMessageBuilder`] and
/// re-emit the agent-facing [`StreamEvent`] stream.
///
/// Like every cyrup stream source it NEVER returns `Err`: a transport/HTTP failure, a malformed
/// frame, or a content-type mismatch arrives as a terminal `StreamEvent::Error` (Pi pushes a
/// terminal `error` event from its `catch`, proxy.ts:214-224). Abort (a cancelled `cancel` token)
/// yields the `aborted` reason, matching Pi's `signal?.aborted ? "aborted" : "error"`
/// (proxy.ts:216).
pub fn stream_proxy(
    model: ModelRef,
    context: Context,
    options: ProxyStreamOptions,
) -> EventStream<StreamEvent> {
    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(16);
    tokio::spawn(run_proxy(model, context, options, tx));
    Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    }))
}

async fn run_proxy(
    model: ModelRef,
    context: Context,
    options: ProxyStreamOptions,
    tx: tokio::sync::mpsc::Sender<StreamEvent>,
) {
    let cancel = options.cancel.clone().unwrap_or_default();
    let mut builder = ProxyMessageBuilder::new(&model);

    // Build the request: POST {proxyUrl}/api/stream, Bearer auth, JSON body (proxy.ts:152-164).
    let body = serde_json::json!({
        "model": model_wire(&model),
        "context": context,
        "options": build_proxy_request_options(&options),
    });
    let url = format!("{}/api/stream", options.proxy_url);
    let req = SseRequest::post_json(url, body)
        .header("Authorization", format!("Bearer {}", options.auth_token));

    let client = match build_client() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(error_terminal(&builder, &cancel, e.to_string())).await;
            return;
        }
    };

    // `open_sse` maps a non-2xx response / transport failure / connect-time cancel to a typed error
    // (Pi throws on `!response.ok`, proxy.ts:166-177).
    let mut frames = match open_sse(&client, req, cancel.clone(), None, None).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(error_terminal(&builder, &cancel, e.to_string())).await;
            return;
        }
    };

    while let Some(frame) = frames.next().await {
        let frame = match frame {
            Ok(f) => f,
            // A mid-stream transport error / cancellation (Pi: the read loop throws, proxy.ts:184).
            Err(e) => {
                let _ = tx.send(error_terminal(&builder, &cancel, e.to_string())).await;
                return;
            }
        };
        // The SSE decoder already strips the `data: ` prefix (Pi does it by hand, proxy.ts:196-197).
        // Empty `data` payloads are skipped (Pi `if (data)`, proxy.ts:198).
        if frame.data.is_empty() {
            continue;
        }
        let proxy_event: ProxyAssistantMessageEvent = match serde_json::from_str(&frame.data) {
            Ok(ev) => ev,
            // A malformed frame: Pi's `JSON.parse` throws into the outer catch (proxy.ts:199,214).
            Err(e) => {
                let _ = tx.send(error_terminal(&builder, &cancel, e.to_string())).await;
                return;
            }
        };
        match builder.process(proxy_event) {
            Ok(Some(event)) => {
                if tx.send(event).await.is_err() {
                    // Consumer dropped (the agent stopped reading): nothing left to do.
                    return;
                }
            }
            Ok(None) => {}
            // A content-type mismatch: Pi `throw`s into the outer catch (proxy.ts:261 etc.).
            Err(msg) => {
                let _ = tx.send(error_terminal(&builder, &cancel, msg)).await;
                return;
            }
        }
    }
    // Clean end: the `done`/`error` event already carried the terminal (proxy.ts:213). Dropping `tx`
    // ends the stream.
}

/// Build a terminal `error` event from the partial assembled so far (Pi sets
/// `partial.stopReason`/`errorMessage` then pushes `{type:"error", error: partial}`, proxy.ts:217-223).
/// The reason is `aborted` iff the request was cancelled, else `error` (Pi `signal?.aborted`,
/// proxy.ts:216).
fn error_terminal(
    builder: &ProxyMessageBuilder,
    cancel: &CancelToken,
    message: String,
) -> StreamEvent {
    let reason =
        if cancel.is_cancelled() { ErrorReason::Aborted } else { ErrorReason::Error };
    let mut error = builder.partial().clone();
    error.stop_reason = reason.into();
    error.error_message = Some(message);
    StreamEvent::Error { reason, error }
}

// ---------------------------------------------------------------------------
// StreamFn adapter (Pi's `streamFn: (model, context, options) => streamProxy(...)` closure,
// proxy.ts:92-98).
// ---------------------------------------------------------------------------

/// A [`StreamFn`] that routes every model call through a proxy server — the cyrup analog of Pi's
/// example closure (proxy.ts:92-98). Construct with the proxy URL + auth token; the per-request
/// [`StreamOptions`] the agent builds are mapped onto [`ProxyStreamOptions`].
///
/// Every field Pi's closure forwards via its `{...options}` spread is forwarded here, including
/// `thinking_budgets`: the agent loop threads `AgentBuilder::thinking_budgets()` into
/// [`StreamOptions::thinking_budgets`] (`Option<cyrup_provider::ThinkingBudgets>`, stream.rs:165),
/// and [`ProxyStreamFn::options_from`] copies that same-typed field straight onto
/// [`ProxyStreamOptions`], from where `build_proxy_request_options` puts it on the wire body — 1:1
/// with Pi (`buildProxyRequestOptions`, proxy.ts:111). The one field that cannot map 1:1 is the
/// model identity: cyrup's provider-agnostic `StreamFn` seam carries only a [`ModelRef`], not Pi's
/// full `Model` (see [`model_wire`]).
pub struct ProxyStreamFn {
    proxy_url: String,
    auth_token: String,
}

impl ProxyStreamFn {
    pub fn new(proxy_url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self { proxy_url: proxy_url.into(), auth_token: auth_token.into() }
    }

    /// Map the agent's provider-level [`StreamOptions`] onto [`ProxyStreamOptions`] — the cyrup
    /// analogue of Pi's `{...options}` spread (proxy.ts:93-97). Every forwardable field, including
    /// `thinking_budgets`, is carried through unchanged; `reasoning` is lowered from the provider
    /// [`ModelThinkingLevel`] to the unified [`ThinkingLevel`] the proxy body carries.
    fn options_from(&self, opts: &StreamOptions) -> ProxyStreamOptions {
        ProxyStreamOptions {
            temperature: opts.temperature,
            max_tokens: opts.max_tokens,
            reasoning: model_thinking_to_unified(opts.reasoning),
            cache_retention: opts.cache_retention,
            session_id: opts.session_id.clone(),
            headers: opts.headers.clone(),
            metadata: opts.metadata.clone(),
            transport: opts.transport,
            // Copy the per-level budgets straight through — this is the cyrup analogue of Pi's
            // `{...options}` spread (proxy.ts:93-97), which carries `thinkingBudgets` into
            // `ProxyStreamOptions`; `build_proxy_request_options` then forwards it onto the wire body
            // (Pi `buildProxyRequestOptions`, proxy.ts:111). Both fields are the SAME
            // `Option<cyrup_provider::ThinkingBudgets>` (stream.rs:165 / proxy.rs field decl), so no
            // conversion is needed here — the private `ProxyThinkingBudgets` wire mirror is applied
            // one stage later at `build_proxy_request_options`.
            thinking_budgets: opts.thinking_budgets,
            max_retry_delay_ms: opts.max_retry_delay_ms,
            cancel: opts.cancel.clone(),
            auth_token: self.auth_token.clone(),
            proxy_url: self.proxy_url.clone(),
        }
    }
}

impl StreamFn for ProxyStreamFn {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        stream_proxy(model.clone(), ctx.clone(), self.options_from(opts))
    }
}

/// Lower a provider-level [`ModelThinkingLevel`] to the unified [`ThinkingLevel`] the proxy body
/// carries: `off` → `None` (reasoning disabled), every on-level maps across.
fn model_thinking_to_unified(level: ModelThinkingLevel) -> Option<ThinkingLevel> {
    match level {
        ModelThinkingLevel::Off => None,
        ModelThinkingLevel::Minimal => Some(ThinkingLevel::Minimal),
        ModelThinkingLevel::Low => Some(ThinkingLevel::Low),
        ModelThinkingLevel::Medium => Some(ThinkingLevel::Medium),
        ModelThinkingLevel::High => Some(ThinkingLevel::High),
        ModelThinkingLevel::Xhigh => Some(ThinkingLevel::Xhigh),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use cyrup_core::StopReason;
    use futures::StreamExt;

    fn model() -> ModelRef {
        ModelRef { provider: "anthropic".into(), api: Some("anthropic-messages".into()), model: "claude".into() }
    }

    fn usage_json() -> Value {
        serde_json::json!({
            "input": 10, "output": 20, "cacheRead": 0, "cacheWrite": 0,
            "totalTokens": 30,
            "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0 }
        })
    }

    fn ev(json: Value) -> ProxyAssistantMessageEvent {
        serde_json::from_value(json).expect("proxy event must deserialize")
    }

    // --- wire enum: deserialize Pi-shaped JSON (proxy.ts:36-57) ---------------

    #[test]
    fn wire_enum_deserializes_pi_camelcase_tags() {
        assert_eq!(ev(serde_json::json!({"type": "start"})), ProxyAssistantMessageEvent::Start);
        assert_eq!(
            ev(serde_json::json!({"type": "text_delta", "contentIndex": 2, "delta": "hi"})),
            ProxyAssistantMessageEvent::TextDelta { content_index: 2, delta: "hi".into() }
        );
        assert_eq!(
            ev(serde_json::json!({"type": "toolcall_start", "contentIndex": 0, "id": "t1", "toolName": "read"})),
            ProxyAssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "t1".into(),
                tool_name: "read".into()
            }
        );
        // `done.reason` is the narrowed DoneReason ("toolUse"); `error.reason` the ErrorReason.
        assert_eq!(
            ev(serde_json::json!({"type": "done", "reason": "toolUse", "usage": usage_json()})),
            ProxyAssistantMessageEvent::Done { reason: DoneReason::ToolUse, usage: Usage { input: 10, output: 20, total_tokens: 30, ..Usage::default() } }
        );
        assert!(matches!(
            ev(serde_json::json!({"type": "error", "reason": "aborted", "errorMessage": "x", "usage": usage_json()})),
            ProxyAssistantMessageEvent::Error { reason: ErrorReason::Aborted, .. }
        ));
    }

    // --- client-side partial rebuild (Pi processProxyEvent, proxy.ts:238-367) -

    #[test]
    fn rebuilds_text_block_across_start_delta_end() {
        let mut b = ProxyMessageBuilder::new(&model());
        assert!(matches!(b.process(ev(serde_json::json!({"type": "start"}))).unwrap(), Some(StreamEvent::Start { .. })));
        b.process(ev(serde_json::json!({"type": "text_start", "contentIndex": 0}))).unwrap();
        b.process(ev(serde_json::json!({"type": "text_delta", "contentIndex": 0, "delta": "Hel"}))).unwrap();
        b.process(ev(serde_json::json!({"type": "text_delta", "contentIndex": 0, "delta": "lo"}))).unwrap();
        let end = b.process(ev(serde_json::json!({"type": "text_end", "contentIndex": 0, "contentSignature": "sig"}))).unwrap();
        match end {
            Some(StreamEvent::TextEnd { content, .. }) => assert_eq!(content, "Hello"),
            other => panic!("expected text_end, got {other:?}"),
        }
        match b.partial().content.first() {
            Some(Content::Text { text, text_signature }) => {
                assert_eq!(text, "Hello");
                assert_eq!(text_signature.as_deref(), Some("sig"));
            }
            other => panic!("expected text content, got {other:?}"),
        }
    }

    #[test]
    fn rebuilds_thinking_block_with_signature() {
        let mut b = ProxyMessageBuilder::new(&model());
        b.process(ev(serde_json::json!({"type": "thinking_start", "contentIndex": 0}))).unwrap();
        b.process(ev(serde_json::json!({"type": "thinking_delta", "contentIndex": 0, "delta": "ponder"}))).unwrap();
        b.process(ev(serde_json::json!({"type": "thinking_end", "contentIndex": 0, "contentSignature": "ts"}))).unwrap();
        match b.partial().content.first() {
            Some(Content::Thinking { thinking, thinking_signature, .. }) => {
                assert_eq!(thinking, "ponder");
                assert_eq!(thinking_signature.as_deref(), Some("ts"));
            }
            other => panic!("expected thinking content, got {other:?}"),
        }
    }

    #[test]
    fn rebuilds_tool_call_args_from_streaming_json() {
        let mut b = ProxyMessageBuilder::new(&model());
        b.process(ev(serde_json::json!({"type": "toolcall_start", "contentIndex": 0, "id": "call_1", "toolName": "read_file"}))).unwrap();
        // Stream the arguments JSON in fragments; each delta re-parses the accumulated buffer.
        b.process(ev(serde_json::json!({"type": "toolcall_delta", "contentIndex": 0, "delta": "{\"path\":\"a."}))).unwrap();
        // Mid-stream the (truncated) JSON is recovered as much as possible (Pi parseStreamingJson).
        if let Some(Content::ToolCall(tc)) = b.partial().content.first() {
            assert_eq!(tc.arguments.get("path").and_then(Value::as_str), Some("a."));
        } else {
            panic!("expected tool call");
        }
        b.process(ev(serde_json::json!({"type": "toolcall_delta", "contentIndex": 0, "delta": "txt\"}"}))).unwrap();
        let end = b.process(ev(serde_json::json!({"type": "toolcall_end", "contentIndex": 0}))).unwrap();
        match end {
            Some(StreamEvent::ToolCallEnd { tool_call, .. }) => {
                assert_eq!(tool_call.id.as_str(), "call_1");
                assert_eq!(tool_call.name, "read_file");
                assert_eq!(tool_call.arguments.get("path").and_then(Value::as_str), Some("a.txt"));
            }
            other => panic!("expected toolcall_end, got {other:?}"),
        }
    }

    #[test]
    fn content_type_mismatch_returns_err_like_pi_throw() {
        let mut b = ProxyMessageBuilder::new(&model());
        b.process(ev(serde_json::json!({"type": "text_start", "contentIndex": 0}))).unwrap();
        // A toolcall_delta against a text slot: Pi throws; cyrup returns Err with the same message.
        let r = b.process(ev(serde_json::json!({"type": "toolcall_delta", "contentIndex": 0, "delta": "x"})));
        assert_eq!(r, Err("Received toolcall_delta for non-toolCall content".to_string()));
    }

    #[test]
    fn toolcall_end_on_non_toolcall_slot_returns_none() {
        // Pi returns `undefined` (no throw) for this case (proxy.ts:347).
        let mut b = ProxyMessageBuilder::new(&model());
        b.process(ev(serde_json::json!({"type": "text_start", "contentIndex": 0}))).unwrap();
        assert_eq!(b.process(ev(serde_json::json!({"type": "toolcall_end", "contentIndex": 0}))).unwrap(), None);
    }

    #[test]
    fn done_event_sets_stop_reason_and_usage() {
        let mut b = ProxyMessageBuilder::new(&model());
        let done = b.process(ev(serde_json::json!({"type": "done", "reason": "stop", "usage": usage_json()}))).unwrap();
        match done {
            Some(StreamEvent::Done { reason, message }) => {
                assert_eq!(reason, DoneReason::Stop);
                assert_eq!(message.stop_reason, StopReason::Stop);
                assert_eq!(message.usage.total_tokens, 30);
            }
            other => panic!("expected done, got {other:?}"),
        }
    }

    #[test]
    fn error_event_maps_reason_and_message() {
        let mut b = ProxyMessageBuilder::new(&model());
        let e = b.process(ev(serde_json::json!({"type": "error", "reason": "error", "errorMessage": "boom", "usage": usage_json()}))).unwrap();
        match e {
            Some(StreamEvent::Error { reason, error }) => {
                assert_eq!(reason, ErrorReason::Error);
                assert_eq!(error.stop_reason, StopReason::Error);
                assert_eq!(error.error_message.as_deref(), Some("boom"));
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    // --- request body (Pi buildProxyRequestOptions, proxy.ts:101-114) ---------

    #[test]
    fn request_body_serializes_pi_serializable_subset() {
        let opts = ProxyStreamOptions {
            temperature: Some(0.5),
            max_tokens: Some(1024),
            reasoning: Some(ThinkingLevel::High),
            cache_retention: Some(CacheRetention::Long),
            session_id: Some("sess-1".into()),
            transport: Some(Transport::Sse),
            thinking_budgets: Some(ThinkingBudgets { medium: Some(5000), ..ThinkingBudgets::default() }),
            max_retry_delay_ms: Some(2000),
            auth_token: "secret".into(),
            proxy_url: "https://proxy.example".into(),
            ..ProxyStreamOptions::default()
        };
        let body = serde_json::to_value(build_proxy_request_options(&opts)).unwrap();
        assert_eq!(body["temperature"], serde_json::json!(0.5));
        assert_eq!(body["maxTokens"], serde_json::json!(1024));
        assert_eq!(body["reasoning"], serde_json::json!("high"));
        assert_eq!(body["cacheRetention"], serde_json::json!("long"));
        assert_eq!(body["sessionId"], serde_json::json!("sess-1"));
        assert_eq!(body["transport"], serde_json::json!("sse"));
        assert_eq!(body["thinkingBudgets"]["medium"], serde_json::json!(5000));
        // Unset per-level budgets are omitted (Pi drops undefined).
        assert!(body["thinkingBudgets"].get("low").is_none());
        assert_eq!(body["maxRetryDelayMs"], serde_json::json!(2000));
        // Local-only fields are NOT in the wire body.
        assert!(body.get("authToken").is_none());
        assert!(body.get("proxyUrl").is_none());
    }

    #[test]
    fn request_body_omits_unset_fields() {
        let body = serde_json::to_value(build_proxy_request_options(&ProxyStreamOptions::default())).unwrap();
        assert_eq!(body, serde_json::json!({}));
    }

    #[test]
    fn model_thinking_lowers_to_unified() {
        assert_eq!(model_thinking_to_unified(ModelThinkingLevel::Off), None);
        assert_eq!(model_thinking_to_unified(ModelThinkingLevel::High), Some(ThinkingLevel::High));
        assert_eq!(model_thinking_to_unified(ModelThinkingLevel::Xhigh), Some(ThinkingLevel::Xhigh));
    }

    // --- StreamFn adapter threads thinking_budgets end-to-end -----------------
    // Pi's proxy closure spreads `...options` (which carries `thinkingBudgets` from `AgentLoopConfig`
    // — agent.ts:441, reaching `options` via agent-loop.ts:304-308) straight into
    // `ProxyStreamOptions` (proxy.ts:92-98), and `buildProxyRequestOptions` forwards
    // `options.thinkingBudgets` unchanged onto the wire body (proxy.ts:111). The cyrup analogue of
    // that spread is `ProxyStreamFn::options_from`; it must COPY `StreamOptions.thinking_budgets`, not
    // drop it. (Before the fix this dropped it — `options_from` hardcoded `thinking_budgets: None`.)
    #[test]
    fn proxy_stream_fn_threads_thinking_budgets_into_wire_body() {
        let budgets =
            ThinkingBudgets { medium: Some(4096), high: Some(8192), ..ThinkingBudgets::default() };
        let stream_opts =
            StreamOptions { thinking_budgets: Some(budgets), ..StreamOptions::default() };

        let proxy_fn = ProxyStreamFn::new("https://proxy.example", "secret");
        // The transform output carries the budgets (Pi `{...options}` spread, proxy.ts:93-97).
        let proxy_opts = proxy_fn.options_from(&stream_opts);
        assert_eq!(
            proxy_opts.thinking_budgets,
            Some(budgets),
            "options_from must thread StreamOptions.thinking_budgets through, not drop it"
        );

        // And they reach the OUTGOING request/wire body (Pi buildProxyRequestOptions, proxy.ts:111).
        let body = serde_json::to_value(build_proxy_request_options(&proxy_opts)).unwrap();
        assert_eq!(body["thinkingBudgets"]["medium"], serde_json::json!(4096));
        assert_eq!(body["thinkingBudgets"]["high"], serde_json::json!(8192));
        // A `None` budgets stays absent on the wire (Pi drops undefined).
        let none_body = serde_json::to_value(build_proxy_request_options(
            &proxy_fn.options_from(&StreamOptions::default()),
        ))
        .unwrap();
        assert!(none_body.get("thinkingBudgets").is_none());
    }

    // --- transport (Pi streamProxy, proxy.ts:116-233) -------------------------

    #[tokio::test]
    async fn transport_connection_failure_yields_terminal_error_event() {
        // Port 1 (tcpmux) is not listening locally → connection refused → terminal error event,
        // never an Err return (cyrup stream contract; Pi pushes a terminal `error`, proxy.ts:214).
        let opts = ProxyStreamOptions {
            proxy_url: "http://127.0.0.1:1".into(),
            auth_token: "t".into(),
            ..ProxyStreamOptions::default()
        };
        let mut stream = stream_proxy(model(), Context::default(), opts);
        let mut last = None;
        while let Some(ev) = stream.next().await {
            last = Some(ev);
        }
        match last {
            Some(StreamEvent::Error { reason: ErrorReason::Error, error }) => {
                assert!(error.error_message.is_some());
                assert_eq!(error.provider.as_str(), "anthropic");
            }
            other => panic!("expected terminal error event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_cancelled_request_yields_aborted_terminal() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let opts = ProxyStreamOptions {
            proxy_url: "http://127.0.0.1:1".into(),
            auth_token: "t".into(),
            cancel: Some(cancel),
            ..ProxyStreamOptions::default()
        };
        let mut stream = stream_proxy(model(), Context::default(), opts);
        let mut last = None;
        while let Some(ev) = stream.next().await {
            last = Some(ev);
        }
        // A cancelled token → Pi's `signal?.aborted ? "aborted" : "error"` → aborted (proxy.ts:216).
        match last {
            Some(StreamEvent::Error { reason: ErrorReason::Aborted, error }) => {
                assert_eq!(error.stop_reason, StopReason::Aborted);
            }
            other => panic!("expected aborted terminal event, got {other:?}"),
        }
    }
}

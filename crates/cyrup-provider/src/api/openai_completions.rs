//! The `openai-completions` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking the OpenAI Chat Completions streaming API (`POST {baseUrl}/chat/
//! completions`, SSE chunks with `choices[].delta.{content,reasoning_content,tool_calls[]}` +
//! `finish_reason`, and a final `usage` chunk via `stream_options.include_usage=true`). Shared by
//! every OpenAI-compatible provider (openai, together, groq, …) — they differ only in base URL,
//! auth, and catalog (R-01-007). Ports Pi's proven `openai-completions.ts` encoder/decoder.
//!
//! Wire JSON uses the vendor's own field names (snake_case), NOT the cyrup camelCase convention.

use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::context::{Context, ToolDef};
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::{build_client, open_sse, SseFrame, SseRequest};
use crate::stream::{StreamEvent, StreamOptions};
use crate::usage::apply_cost;
use crate::HeaderMap;
use cyrup_core::{
    ApiId, AssistantMessage, CancelToken, Content, Message, StopReason, ThinkingLevel, ToolCall,
    ToolCallId, Usage,
};
use futures::{Stream, StreamExt};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::OPENAI_COMPLETIONS;

/// The `ApiImpl` for `"openai-completions"`.
pub struct OpenAiCompletionsApi {
    api: ApiId,
}

impl Default for OpenAiCompletionsApi {
    fn default() -> Self {
        Self { api: ApiId::from(API_ID) }
    }
}

impl OpenAiCompletionsApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(OpenAiCompletionsApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for OpenAiCompletionsApi {
    fn api(&self) -> &ApiId {
        &self.api
    }

    async fn run(
        &self,
        model: &Model,
        ctx: &Context,
        auth: &AuthResult,
        opts: &StreamOptions,
        cancel: CancelToken,
        sink: EventSink,
    ) {
        let provider = model.provider.clone();
        let model_id = model.id.as_str().to_string();

        let url = match resolve_url(model, auth) {
            Some(url) => url,
            None => {
                let e = ProviderError::Transport("no base URL configured for model".into());
                sink.send(e.into_error_event(provider, &model_id)).await;
                return;
            }
        };

        let body = build_body(model, ctx, opts);
        let headers = build_headers(auth, opts);
        let req = SseRequest { method: reqwest::Method::POST, url, headers, body: Some(body) };

        let client = match build_client() {
            Ok(c) => c,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id)).await;
                return;
            }
        };

        let frames = match open_sse(&client, req, cancel, None, None).await {
            Ok(s) => s,
            Err(e) => {
                // transport / non-2xx / abort-during-connect → terminal Error (R-01-018/045)
                sink.send(e.into_error_event(provider, &model_id)).await;
                return;
            }
        };

        decode_stream(frames, model, &self.api, &sink).await;
    }
}

// ---------------------------------------------------------------------------
// Request encoding
// ---------------------------------------------------------------------------

/// Resolve the `POST` target: an auth base-url override wins over `model.base_url`. The endpoint is
/// `{base}/chat/completions` (appended unless `base` already names it).
fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth.auth.base_url.as_deref().or(model.base_url.as_deref())?;
    Some(chat_completions_url(base))
}

/// Normalize a base URL to the `chat/completions` endpoint.
pub(crate) fn chat_completions_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

/// Build the request headers: `Authorization: Bearer <key>` plus the auth/opts header overlay
/// (a `None` value suppresses a default; opts win over auth, auth over the default).
pub(crate) fn build_headers(auth: &AuthResult, opts: &StreamOptions) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(key) = &auth.auth.api_key {
        headers.insert("Authorization".to_string(), Some(format!("Bearer {key}")));
    }
    if let Some(overlay) = &auth.auth.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    headers
}

/// Map a unified [`ThinkingLevel`] to the OpenAI `reasoning_effort` value (None for `Off`).
fn reasoning_effort(level: ThinkingLevel) -> Option<&'static str> {
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some("minimal"),
        ThinkingLevel::Low => Some("low"),
        ThinkingLevel::Medium => Some("medium"),
        ThinkingLevel::High => Some("high"),
        ThinkingLevel::Xhigh => Some("xhigh"),
    }
}

/// Build the Chat Completions request JSON body from the [`Context`] + [`StreamOptions`].
pub(crate) fn build_body(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert("messages".to_string(), Value::Array(convert_messages(model, ctx)));
    obj.insert("stream".to_string(), json!(true));
    obj.insert("stream_options".to_string(), json!({ "include_usage": true }));

    if let Some(max) = opts.max_tokens {
        obj.insert("max_tokens".to_string(), json!(max));
    }
    if let Some(temp) = opts.temperature {
        obj.insert("temperature".to_string(), json!(temp));
    }

    let has_tool_history = ctx.messages.iter().any(message_has_tool_use);
    if !ctx.tools.is_empty() {
        obj.insert("tools".to_string(), Value::Array(convert_tools(&ctx.tools)));
        obj.insert("tool_choice".to_string(), json!("auto"));
    } else if has_tool_history {
        // Some OpenAI-compatible proxies require `tools` to be present whenever the conversation
        // already contains tool calls / tool results.
        obj.insert("tools".to_string(), Value::Array(Vec::new()));
    }

    if model.reasoning
        && let Some(effort) = reasoning_effort(opts.reasoning)
    {
        obj.insert("reasoning_effort".to_string(), json!(effort));
    }

    Value::Object(obj)
}

/// `true` if a message carries a tool call (assistant) or is a tool result.
fn message_has_tool_use(msg: &Message) -> bool {
    match msg {
        Message::ToolResult { .. } => true,
        Message::Assistant(am) => am.content.iter().any(|c| matches!(c, Content::ToolCall(_))),
        Message::User { .. } => false,
    }
}

/// Map cyrup [`ToolDef`]s to OpenAI `tools` entries.
pub(crate) fn convert_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                },
            })
        })
        .collect()
}

/// Map cyrup [`Message`]s to OpenAI chat messages.
pub(crate) fn convert_messages(model: &Model, ctx: &Context) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();

    if let Some(system) = &ctx.system_prompt {
        out.push(json!({ "role": "system", "content": system }));
    }

    for msg in &ctx.messages {
        match msg {
            Message::User { content, .. } => {
                out.push(json!({
                    "role": "user",
                    "content": user_content(content, model.supports_image_input()),
                }));
            }
            Message::Assistant(am) => {
                if let Some(value) = assistant_message(am) {
                    out.push(value);
                }
            }
            Message::ToolResult { tool_call_id, content, .. } => {
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id.as_str(),
                    "content": tool_result_text(content),
                }));
            }
        }
    }

    out
}

/// User content: a plain string when text-only, else an array of `text`/`image_url` parts.
fn user_content(content: &[Content], supports_image: bool) -> Value {
    let only_text = content.iter().all(|c| matches!(c, Content::Text { .. }));
    if only_text {
        return Value::String(join_text(content));
    }

    let mut parts: Vec<Value> = Vec::new();
    for block in content {
        match block {
            Content::Text { text, .. } => parts.push(json!({ "type": "text", "text": text })),
            Content::Image { data, mime_type } if supports_image => parts.push(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{mime_type};base64,{data}") },
            })),
            _ => {}
        }
    }
    Value::Array(parts)
}

/// Build an assistant chat message; `None` when it has neither content nor tool calls.
fn assistant_message(am: &AssistantMessage) -> Option<Value> {
    let text = join_text(&am.content);
    let tool_calls: Vec<Value> = am
        .content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall(tc) => Some(json!({
                "id": tc.id.as_str(),
                "type": "function",
                "function": {
                    "name": tc.name,
                    "arguments": serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".to_string()),
                },
            })),
            _ => None,
        })
        .collect();

    if text.is_empty() && tool_calls.is_empty() {
        return None;
    }

    let mut obj = Map::new();
    obj.insert("role".to_string(), json!("assistant"));
    // Some providers reject a null content alongside tool calls; send "" instead.
    obj.insert("content".to_string(), Value::String(text));
    if !tool_calls.is_empty() {
        obj.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    Some(Value::Object(obj))
}

/// Concatenate the text blocks of a content vector.
fn join_text(content: &[Content]) -> String {
    let mut s = String::new();
    for block in content {
        if let Content::Text { text, .. } = block {
            s.push_str(text);
        }
    }
    s
}

/// Tool-result text: joined text blocks, or a placeholder when only non-text content is present.
fn tool_result_text(content: &[Content]) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for block in content {
        if let Content::Text { text, .. } = block {
            parts.push(text);
        }
    }
    if parts.is_empty() {
        "(see attached content)".to_string()
    } else {
        parts.join("\n")
    }
}

// ---------------------------------------------------------------------------
// Response decoding
// ---------------------------------------------------------------------------

/// One in-progress content block, in first-appearance order (its index == `content_index`).
enum Block {
    Text(String),
    Thinking(String),
    Tool { id: String, name: String, args: String },
}

/// Streaming-decode state.
#[derive(Default)]
struct Decoder {
    blocks: Vec<Block>,
    text_idx: Option<usize>,
    thinking_idx: Option<usize>,
    tool_by_stream: HashMap<i64, usize>,
    tool_by_id: HashMap<String, usize>,
    usage: Option<Usage>,
    response_id: Option<String>,
    response_model: Option<String>,
    stop_reason: Option<StopReason>,
    error_message: Option<String>,
}

/// Reasoning delta field names emitted by OpenAI-compatible endpoints (first non-empty wins).
const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];

/// Drive the SSE frame stream into ordered [`StreamEvent`]s pushed to `sink`. Emits `Start` first,
/// then per-block `*Start/*Delta/*End`, then exactly one terminal (`Done`/`Error`).
pub(crate) async fn decode_stream<S>(mut frames: S, model: &Model, api: &ApiId, sink: &EventSink)
where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    let provider = model.provider.clone();
    let model_id = model.id.as_str().to_string();

    if !sink.send(StreamEvent::Start).await {
        return;
    }

    let mut dec = Decoder::default();

    while let Some(frame) = frames.next().await {
        let frame = match frame {
            Ok(f) => f,
            Err(e) => {
                // transport/decode/abort mid-stream → terminal Error (R-01-018/044/045)
                sink.send(e.into_error_event(provider, &model_id)).await;
                return;
            }
        };
        let data = frame.data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            break;
        }
        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            // Be robust to keep-alive / non-JSON comment frames.
            Err(_) => continue,
        };
        if !process_chunk(&chunk, &mut dec, model, sink).await {
            return; // consumer dropped
        }
    }

    // Finalize each open block in appearance order.
    for (idx, block) in dec.blocks.iter().enumerate() {
        let ev = match block {
            Block::Text(text) => {
                StreamEvent::TextEnd { content_index: idx, content: text.clone() }
            }
            Block::Thinking(thinking) => {
                StreamEvent::ThinkingEnd { content_index: idx, content: thinking.clone() }
            }
            Block::Tool { id, name, args } => StreamEvent::ToolCallEnd {
                content_index: idx,
                tool_call: ToolCall {
                    id: ToolCallId::from(id.as_str()),
                    name: name.clone(),
                    arguments: parse_partial_json(args),
                    thought_signature: None,
                },
            },
        };
        if !sink.send(ev).await {
            return;
        }
    }

    let message = build_final_message(dec, model, api);
    let terminal = if message.stop_reason == StopReason::Error {
        StreamEvent::Error { message }
    } else {
        StreamEvent::Done { message }
    };
    sink.send(terminal).await;
}

/// Process one decoded chunk. Returns `false` if the consumer dropped the stream.
async fn process_chunk(
    chunk: &Value,
    dec: &mut Decoder,
    model: &Model,
    sink: &EventSink,
) -> bool {
    if dec.response_id.is_none()
        && let Some(id) = chunk.get("id").and_then(Value::as_str)
        && !id.is_empty()
    {
        dec.response_id = Some(id.to_string());
    }
    if dec.response_model.is_none()
        && let Some(m) = chunk.get("model").and_then(Value::as_str)
        && !m.is_empty()
        && m != model.id.as_str()
    {
        dec.response_model = Some(m.to_string());
    }
    if let Some(usage) = chunk.get("usage").filter(|u| !u.is_null()) {
        dec.usage = Some(parse_usage(usage, model));
    }

    let choice = match chunk.get("choices").and_then(Value::as_array).and_then(|c| c.first()) {
        Some(c) => c,
        None => return true,
    };

    // Some providers (e.g. Moonshot) place usage on the choice instead of the chunk.
    if dec.usage.is_none()
        && let Some(usage) = choice.get("usage").filter(|u| !u.is_null())
    {
        dec.usage = Some(parse_usage(usage, model));
    }

    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        let (stop, err) = map_stop_reason(reason);
        dec.stop_reason = Some(stop);
        if let Some(err) = err {
            dec.error_message = Some(err);
        }
    }

    let delta = match choice.get("delta") {
        Some(d) if d.is_object() => d,
        _ => return true,
    };

    // 1. Text content.
    if let Some(text) = delta.get("content").and_then(Value::as_str)
        && !text.is_empty()
    {
        let idx = match ensure_text_block(dec, sink).await {
            Some(idx) => idx,
            None => return false,
        };
        if let Some(Block::Text(buf)) = dec.blocks.get_mut(idx) {
            buf.push_str(text);
        }
        if !sink
            .send(StreamEvent::TextDelta { content_index: idx, delta: text.to_string() })
            .await
        {
            return false;
        }
    }

    // 2. Reasoning / thinking content (first non-empty reasoning field).
    if let Some(reason_text) = first_reasoning_delta(delta)
        && !reason_text.is_empty()
    {
        let idx = match ensure_thinking_block(dec, sink).await {
            Some(idx) => idx,
            None => return false,
        };
        if let Some(Block::Thinking(buf)) = dec.blocks.get_mut(idx) {
            buf.push_str(reason_text);
        }
        if !sink
            .send(StreamEvent::ThinkingDelta { content_index: idx, delta: reason_text.to_string() })
            .await
        {
            return false;
        }
    }

    // 3. Streamed tool calls.
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tool_calls {
            if !process_tool_call_delta(tc, dec, sink).await {
                return false;
            }
        }
    }

    true
}

/// Ensure a text block exists, emitting `TextStart` on first appearance. Returns its index, or
/// `None` if the consumer dropped the stream.
async fn ensure_text_block(dec: &mut Decoder, sink: &EventSink) -> Option<usize> {
    if let Some(idx) = dec.text_idx {
        return Some(idx);
    }
    let idx = dec.blocks.len();
    dec.blocks.push(Block::Text(String::new()));
    dec.text_idx = Some(idx);
    if !sink.send(StreamEvent::TextStart { content_index: idx }).await {
        return None;
    }
    Some(idx)
}

/// Ensure a thinking block exists, emitting `ThinkingStart` on first appearance.
async fn ensure_thinking_block(dec: &mut Decoder, sink: &EventSink) -> Option<usize> {
    if let Some(idx) = dec.thinking_idx {
        return Some(idx);
    }
    let idx = dec.blocks.len();
    dec.blocks.push(Block::Thinking(String::new()));
    dec.thinking_idx = Some(idx);
    if !sink.send(StreamEvent::ThinkingStart { content_index: idx }).await {
        return None;
    }
    Some(idx)
}

/// Apply one `tool_calls[]` delta fragment, assembling id/name/arguments across chunks.
async fn process_tool_call_delta(tc: &Value, dec: &mut Decoder, sink: &EventSink) -> bool {
    let stream_index = tc.get("index").and_then(Value::as_i64);
    let id = tc.get("id").and_then(Value::as_str).filter(|s| !s.is_empty());
    let name = tc
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let args_fragment = tc
        .get("function")
        .and_then(|f| f.get("arguments"))
        .and_then(Value::as_str)
        .unwrap_or("");

    // Locate the block: by stream index first, then by id.
    let existing = stream_index
        .and_then(|si| dec.tool_by_stream.get(&si).copied())
        .or_else(|| id.and_then(|i| dec.tool_by_id.get(i).copied()));

    let idx = match existing {
        Some(idx) => idx,
        None => {
            let idx = dec.blocks.len();
            dec.blocks.push(Block::Tool {
                id: id.unwrap_or("").to_string(),
                name: name.unwrap_or("").to_string(),
                args: String::new(),
            });
            if let Some(si) = stream_index {
                dec.tool_by_stream.insert(si, idx);
            }
            if let Some(i) = id {
                dec.tool_by_id.insert(i.to_string(), idx);
            }
            if !sink.send(StreamEvent::ToolCallStart { content_index: idx }).await {
                return false;
            }
            idx
        }
    };

    if let Some(Block::Tool { id: bid, name: bname, args }) = dec.blocks.get_mut(idx) {
        if let Some(i) = id
            && bid.is_empty()
        {
            *bid = i.to_string();
        }
        if let Some(n) = name
            && bname.is_empty()
        {
            *bname = n.to_string();
        }
        if !args_fragment.is_empty() {
            args.push_str(args_fragment);
        }
    }
    // Maintain the id index if the id only arrived now.
    if let Some(i) = id {
        dec.tool_by_id.entry(i.to_string()).or_insert(idx);
    }

    sink.send(StreamEvent::ToolCallDelta {
        content_index: idx,
        delta: args_fragment.to_string(),
    })
    .await
}

/// First non-empty reasoning delta string across the known field names.
fn first_reasoning_delta(delta: &Value) -> Option<&str> {
    for field in REASONING_FIELDS {
        if let Some(s) = delta.get(field).and_then(Value::as_str)
            && !s.is_empty()
        {
            return Some(s);
        }
    }
    None
}

/// Build the terminal [`AssistantMessage`] from accumulated decoder state.
fn build_final_message(dec: Decoder, model: &Model, api: &ApiId) -> AssistantMessage {
    let mut content: Vec<Content> = Vec::new();
    for block in dec.blocks {
        match block {
            Block::Text(text) => content.push(Content::text(text)),
            Block::Thinking(thinking) => content.push(Content::thinking(thinking)),
            Block::Tool { id, name, args } => content.push(Content::ToolCall(ToolCall {
                id: ToolCallId::from(id.as_str()),
                name,
                arguments: parse_partial_json(&args),
                thought_signature: None,
            })),
        }
    }

    let mut usage = dec.usage.unwrap_or_default();
    apply_cost(&model.cost, &mut usage);

    let stop_reason = dec.stop_reason.unwrap_or(StopReason::Stop);

    AssistantMessage {
        content,
        provider: model.provider.clone(),
        model: model.id.as_str().to_string(),
        api: Some(api.clone()),
        response_model: dec.response_model,
        response_id: dec.response_id,
        usage,
        stop_reason,
        error_message: dec.error_message,
        timestamp: now_millis(),
    }
}

/// Parse usage from a chunk's `usage` object (cache-read/write split + reasoning), applying cost.
fn parse_usage(raw: &Value, model: &Model) -> Usage {
    let u64_at = |v: &Value, key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);

    let prompt = u64_at(raw, "prompt_tokens");
    let completion = u64_at(raw, "completion_tokens");
    let details = raw.get("prompt_tokens_details");
    let cache_read = details
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| raw.get("prompt_cache_hit_tokens").and_then(Value::as_u64))
        .unwrap_or(0);
    let cache_write = details
        .and_then(|d| d.get("cache_write_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning = raw
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(Value::as_u64);

    let input = prompt.saturating_sub(cache_read).saturating_sub(cache_write);
    let mut usage = Usage {
        input,
        output: completion,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning,
        total_tokens: 0,
        cost: Default::default(),
    };
    apply_cost(&model.cost, &mut usage);
    usage
}

/// Map an OpenAI `finish_reason` to a [`StopReason`] (plus an optional error message).
fn map_stop_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" | "end" => (StopReason::Stop, None),
        "length" => (StopReason::Length, None),
        "tool_calls" | "function_call" => (StopReason::ToolUse, None),
        other => (StopReason::Error, Some(format!("Provider finish_reason: {other}"))),
    }
}

/// Best-effort parse of accumulated tool-call argument JSON. An empty/incomplete buffer yields an
/// empty object so a truncated stream still produces a valid (if empty) tool call.
fn parse_partial_json(s: &str) -> Value {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| Value::Object(Map::new()))
}

/// Current unix time in milliseconds (0 on a clock error — never panics).
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::api::channel;
    use crate::auth::ModelAuth;
    use crate::model::{Modality, ModelCost};
    use crate::stream::sse::decode_sse_bytes;
    use cyrup_core::Usage;

    fn model() -> Model {
        Model {
            id: "openai/gpt-oss-120b".into(),
            name: "GPT OSS".into(),
            api: API_ID.into(),
            provider: "together".into(),
            base_url: Some("https://api.together.ai/v1".to_string()),
            reasoning: true,
            input: vec![Modality::Text],
            output: vec![Modality::Text],
            cost: ModelCost { input: 1.0, output: 2.0, cache_read: 0.5, cache_write: 0.0 },
            context_window: 131072,
            max_tokens: 131072,
        }
    }

    fn auth_with_key() -> AuthResult {
        AuthResult {
            auth: ModelAuth { api_key: Some("sk-xyz".into()), ..Default::default() },
            env: None,
            source: Some("env".into()),
        }
    }

    // ---- Request builder ----

    #[test]
    fn url_appends_chat_completions() {
        assert_eq!(
            chat_completions_url("https://api.together.ai/v1"),
            "https://api.together.ai/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://api.together.ai/v1/"),
            "https://api.together.ai/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://x/v1/chat/completions"),
            "https://x/v1/chat/completions"
        );
    }

    #[test]
    fn headers_set_bearer_auth() {
        let headers = build_headers(&auth_with_key(), &StreamOptions::default());
        assert_eq!(headers.get("Authorization"), Some(&Some("Bearer sk-xyz".to_string())));
    }

    #[test]
    fn request_body_matches_openai_shape() {
        let ctx = Context {
            system_prompt: Some("be terse".to_string()),
            messages: vec![
                Message::User { content: vec![Content::text("hi")], timestamp: 0 },
                Message::Assistant(AssistantMessage {
                    content: vec![Content::ToolCall(ToolCall {
                        id: ToolCallId::from("call_1"),
                        name: "get_weather".into(),
                        arguments: json!({ "city": "Paris" }),
                        thought_signature: None,
                    })],
                    provider: "together".into(),
                    model: "m".into(),
                    api: None,
                    response_model: None,
                    response_id: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::ToolUse,
                    error_message: None,
                    timestamp: 0,
                }),
                Message::ToolResult {
                    tool_call_id: ToolCallId::from("call_1"),
                    tool_name: "get_weather".into(),
                    content: vec![Content::text("sunny")],
                    is_error: false,
                    details: None,
                    timestamp: 0,
                },
            ],
            tools: vec![ToolDef {
                name: "get_weather".into(),
                description: "Get weather".into(),
                parameters: json!({
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "required": ["city"],
                }),
            }],
        };

        let opts = StreamOptions {
            max_tokens: Some(256),
            temperature: Some(0.5),
            reasoning: ThinkingLevel::High,
            ..Default::default()
        };

        let body = build_body(&model(), &ctx, &opts);

        assert_eq!(body["model"], "openai/gpt-oss-120b");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["max_tokens"], 256);
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["reasoning_effort"], "high");

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0], json!({ "role": "system", "content": "be terse" }));
        assert_eq!(messages[1], json!({ "role": "user", "content": "hi" }));
        // assistant tool call
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["content"], "");
        let tcs = messages[2]["tool_calls"].as_array().unwrap();
        assert_eq!(tcs[0]["id"], "call_1");
        assert_eq!(tcs[0]["type"], "function");
        assert_eq!(tcs[0]["function"]["name"], "get_weather");
        assert_eq!(tcs[0]["function"]["arguments"], "{\"city\":\"Paris\"}");
        // tool result
        assert_eq!(
            messages[3],
            json!({ "role": "tool", "tool_call_id": "call_1", "content": "sunny" })
        );

        // tools
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "get_weather");
        assert_eq!(tools[0]["function"]["description"], "Get weather");
        assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
        assert_eq!(body["tool_choice"], "auto");
    }

    #[test]
    fn reasoning_effort_omitted_for_non_reasoning_model() {
        let mut m = model();
        m.reasoning = false;
        let opts = StreamOptions { reasoning: ThinkingLevel::High, ..Default::default() };
        let body = build_body(&m, &Context::default(), &opts);
        assert!(body.get("reasoning_effort").is_none());
    }

    // ---- SSE decode ----

    async fn collect_events(raw: &'static str) -> Vec<StreamEvent> {
        let (sink, mut rx) = channel(256);
        let m = model();
        let api = ApiId::from(API_ID);
        let frames = decode_sse_bytes(raw.as_bytes().to_vec());
        let handle = tokio::spawn(async move {
            decode_stream(frames, &m, &api, &sink).await;
        });
        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        handle.await.unwrap();
        events
    }

    #[tokio::test]
    async fn decodes_text_with_usage_terminal() {
        let raw = concat!(
            "data: {\"id\":\"chatcmpl-1\",\"model\":\"openai/gpt-oss-120b\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect_events(raw).await;

        assert!(matches!(events.first(), Some(StreamEvent::Start)));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::TextStart { content_index: 0 })));
        let deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["Hel", "lo"]);

        match events.last() {
            Some(StreamEvent::Done { message }) => {
                assert_eq!(message.stop_reason, StopReason::Stop);
                assert_eq!(message.content, vec![Content::text("Hello")]);
                assert_eq!(message.response_id.as_deref(), Some("chatcmpl-1"));
                assert_eq!(message.usage.input, 10);
                assert_eq!(message.usage.output, 5);
                assert!(message.usage.cost.total > 0.0);
            }
            other => panic!("expected Done terminal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn decodes_multichunk_tool_call() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_9\",\"function\":{\"name\":\"add\",\"arguments\":\"{\\\"a\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1,\\\"b\\\":2}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect_events(raw).await;

        assert!(events.iter().any(|e| matches!(e, StreamEvent::ToolCallStart { content_index: 0 })));
        let delta_count =
            events.iter().filter(|e| matches!(e, StreamEvent::ToolCallDelta { .. })).count();
        assert_eq!(delta_count, 2);

        let tc_end = events.iter().find_map(|e| match e {
            StreamEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
            _ => None,
        });
        let tc = tc_end.expect("toolcall_end");
        assert_eq!(tc.id.as_str(), "call_9");
        assert_eq!(tc.name, "add");
        assert_eq!(tc.arguments, json!({ "a": 1, "b": 2 }));

        match events.last() {
            Some(StreamEvent::Done { message }) => {
                assert_eq!(message.stop_reason, StopReason::ToolUse);
                assert_eq!(message.content.len(), 1);
            }
            other => panic!("expected Done terminal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn decodes_reasoning_to_thinking() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hard\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect_events(raw).await;

        assert!(events.iter().any(|e| matches!(e, StreamEvent::ThinkingStart { content_index: 0 })));
        let think: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ThinkingDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(think, "think hard");

        match events.last() {
            Some(StreamEvent::Done { message }) => {
                assert_eq!(
                    message.content,
                    vec![Content::thinking("think hard"), Content::text("answer")]
                );
            }
            other => panic!("expected Done terminal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn finish_reason_length_maps_to_length() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect_events(raw).await;
        match events.last() {
            Some(StreamEvent::Done { message }) => {
                assert_eq!(message.stop_reason, StopReason::Length)
            }
            other => panic!("expected Done terminal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn content_filter_maps_to_error_terminal() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"content_filter\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect_events(raw).await;
        match events.last() {
            Some(StreamEvent::Error { message }) => {
                assert_eq!(message.stop_reason, StopReason::Error);
                assert!(message
                    .error_message
                    .as_deref()
                    .unwrap()
                    .contains("content_filter"));
            }
            other => panic!("expected Error terminal, got {other:?}"),
        }
    }
}

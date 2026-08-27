//! The `mistral-conversations` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking Mistral's `chat.stream` API (`POST {baseUrl}/v1/chat/completions`,
//! `stream: true`, SSE `data:`-framed `CompletionChunk` JSON ending with `data: [DONE]`). Shared by
//! the `mistral` provider. Pure JSON-over-SSE — no SDK, no new dependency.
//!
//! 1:1 port of Pi's `api/mistral-conversations.ts`: the chat-payload encoder (`buildChatPayload` /
//! `toChatMessages` / `toFunctionTools`), the deterministic 9-char tool-call-id normalizer
//! (`createMistralToolCallIdNormalizer` / `deriveMistralToolCallId` over `shortHash`), the
//! `promptMode`/`reasoningEffort` reasoning lowering, `x-affinity` + `promptCacheKey` prefix caching,
//! and the `CompletionChunk` streaming decoder (string / `text` / `thinking` content chunks +
//! incremental tool calls).
//!
//! Wire JSON uses Mistral's own field names (camelCase: `maxTokens`, `toolChoice`, `promptMode`,
//! `reasoningEffort`, `toolCalls`, `toolCallId`).

use crate::HeaderMap;
use crate::api::compat::sanitize_surrogates;
use crate::api::openai_completions::transform_messages_with;
use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::collection::clamp_thinking_level;
use crate::context::{Context, ToolDef};
use crate::error::ProviderError;
use crate::model::{Modality, Model};
use crate::stream::sse::{SseFrame, SseRequest, build_client_for_target, open_sse};
use crate::stream::{CacheRetention, StreamEvent, StreamOptions, ToolChoice};
use crate::usage::compute_cost;
use crate::utils::constrained_sampling::{
    ConstrainedSamplingError, json_schema_tool_parameters, resolve_json_schema_strict_sampling,
};
use crate::utils::hash::short_hash;
use crate::utils::json_parse::{parse_json_with_repair, parse_streaming_json_object};
use crate::utils::provider_retry::ProviderRetry;
use cyrup_core::{
    ApiId, AssistantMessage, CancelToken, Content, Message, ModelThinkingLevel, StopReason,
    ToolCall, ToolCallId, Usage,
};
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::MISTRAL_CONVERSATIONS;

/// Mistral tool-call ids are exactly 9 alphanumerics (Pi `MISTRAL_TOOL_CALL_ID_LENGTH`,
/// mistral-conversations.ts:31).
const MISTRAL_TOOL_CALL_ID_LENGTH: usize = 9;

/// The `ApiImpl` for `"mistral-conversations"`.
pub struct MistralConversationsApi {
    api: ApiId,
}

impl Default for MistralConversationsApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl MistralConversationsApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(MistralConversationsApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for MistralConversationsApi {
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

        let api_key = match &auth.auth.api_key {
            Some(k) if !k.is_empty() => k.clone(),
            _ => {
                let e =
                    ProviderError::Transport(format!("No API key for provider: {provider}").into());
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        let url = match resolve_url(model, auth) {
            Some(url) => url,
            None => {
                let e = ProviderError::Transport("no base URL configured for model".into());
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        // PROV-011: an unsatisfiable `constrainedSampling` fails the turn before any HTTP, with
        // pi's own message.
        let params = match build_chat_payload(model, ctx, opts) {
            Ok(p) => p,
            Err(e) => {
                let e = ProviderError::from(e);
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };
        // gap-08 #2: `before_provider_request` may inspect/replace the outbound body.
        let body = crate::stream::apply_on_payload(opts, model, params).await;
        let headers = build_headers(model, opts, &api_key);
        let req = SseRequest {
            method: reqwest::Method::POST,
            url,
            headers,
            body: Some(body),
        };

        // Honor HTTP(S)_PROXY for the live client (Pi resolveHttpProxyUrlForTarget,
        // node-http-proxy.ts:92-112).
        // PROV-006: the request idle timeout. `StreamOptions.timeout_ms` overrides the
        // process-global `configure_http_idle_timeout` default, exactly as Pi layers the SDK
        // client's `timeout` on top of the global undici dispatcher (sdk.ts:304-309).
        let client = match build_client_for_target(
            &req.url,
            &crate::auth::types::EnvAuthContext,
            auth.env.as_ref(),
            opts.timeout_ms,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        // gap-08 #3: capture {status, headers} at connect, then fire `after_provider_response`.
        let capture = crate::stream::ResponseCapture::default();
        let on_resp = capture.sse_hook(opts);
        let frames = match open_sse(
            &client,
            req,
            cancel,
            None,
            on_resp,
            ProviderRetry::from_options(opts),
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };
        capture.fire(opts, model).await;

        decode_stream(frames, model, &self.api, &sink).await;
    }
}

// ---------------------------------------------------------------------------
// Request encoding
// ---------------------------------------------------------------------------

/// Resolve the `POST` target (Pi `new Mistral({ serverURL: model.baseUrl })`,
/// mistral-conversations.ts:65-68). An auth base-url override wins over `model.base_url`. The
/// endpoint is `{base}/v1/chat/completions`.
fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
    Some(chat_url(base))
}

/// Normalize a base URL to the `/v1/chat/completions` endpoint.
pub(crate) fn chat_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/v1/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1/chat/completions")
    }
}

/// `cacheRetention !== "none" && !!sessionId` (Pi `shouldUsePromptCaching`,
/// mistral-conversations.ts:270-272). cyrup's `cache_retention` defaults to `None` (unset), which —
/// like Pi's `undefined` — is `!== "none"`.
fn should_use_prompt_caching(opts: &StreamOptions) -> Option<&str> {
    let retention_ok = opts.cache_retention != Some(CacheRetention::None);
    if retention_ok && let Some(sid) = &opts.session_id {
        return Some(sid.as_str());
    }
    None
}

/// Build the request headers (Pi `buildRequestOptions`, mistral-conversations.ts:213-238). Mistral
/// authenticates with `Authorization: Bearer`. `x-affinity` carries the session id for KV-cache
/// reuse. The model/opts header overlays layer last (a `None` value suppresses a default).
pub(crate) fn build_headers(model: &Model, opts: &StreamOptions, api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type".to_string(),
        Some("application/json".to_string()),
    );
    headers.insert(
        "authorization".to_string(),
        Some(format!("Bearer {api_key}")),
    );

    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }

    // `x-affinity` for prefix caching, only when not already set by an overlay
    // (mistral-conversations.ts:229-231).
    if let Some(sid) = should_use_prompt_caching(opts)
        && !headers.contains_key("x-affinity")
    {
        headers.insert("x-affinity".to_string(), Some(sid.to_string()));
    }

    headers
}

/// Mistral `promptMode` (Pi `MistralOptions.promptMode`, mistral-conversations.ts:41). The only
/// value Pi defines is `"reasoning"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MistralPromptMode {
    /// `"reasoning"`.
    Reasoning,
}

impl MistralPromptMode {
    /// The exact `promptMode` wire string.
    pub fn as_wire(self) -> &'static str {
        match self {
            MistralPromptMode::Reasoning => "reasoning",
        }
    }
}

/// Mistral `reasoningEffort` (Pi `MistralReasoningEffort = "none" | "high"`,
/// mistral-conversations.ts:37). Read verbatim from `MistralOptions.reasoningEffort` in
/// `buildChatPayload` (mistral-conversations.ts:257).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MistralReasoningEffort {
    /// `"none"`.
    None,
    /// `"high"`.
    High,
}

impl MistralReasoningEffort {
    /// The exact `reasoningEffort` wire string.
    pub fn as_wire(self) -> &'static str {
        match self {
            MistralReasoningEffort::None => "none",
            MistralReasoningEffort::High => "high",
        }
    }
}

/// Per-API typed options for the `mistral-conversations` wire protocol (Pi `MistralOptions`,
/// mistral-conversations.ts:39-43). `toolChoice` folds onto `StreamOptions.tool_choice` and the
/// simple reasoning level onto `StreamOptions.reasoning`; only a direct `promptMode` per-request
/// override has no other home. Carried via
/// [`StreamOptions::api_options`](crate::StreamOptions::api_options); defaults to `None` (no
/// override), reproducing the streamSimple-driven behavior exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MistralOptions {
    /// Direct `promptMode` override (Pi `buildChatPayload` reads `options.promptMode`,
    /// mistral-conversations.ts:256). `None` = no override: the unified `reasoning` level drives
    /// `promptMode` as before.
    pub prompt_mode: Option<MistralPromptMode>,
    /// Direct `reasoningEffort` override (Pi `buildChatPayload` reads `options.reasoningEffort`
    /// verbatim, mistral-conversations.ts:257). `None` = no override: the unified `reasoning` level
    /// drives `reasoningEffort` via `lower_reasoning`. Set independently of `prompt_mode`, exactly
    /// like Pi's two independent `if (options?.…)` guards.
    pub reasoning_effort: Option<MistralReasoningEffort>,
}

/// Test-only convenience wrapper for [`build_chat_payload`].
#[cfg(test)]
// Test-only fixture wrapper: the deny-list allowance the crate's `mod tests` blocks carry.
#[allow(clippy::expect_used)]
pub(crate) fn build_body(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    build_chat_payload(model, ctx, opts)
        .expect("fixture declares no unsatisfiable constrained sampling")
}

/// Build the `chat/completions` request body (1:1 port of Pi `buildChatPayload` + the `streamSimple`
/// reasoning lowering, mistral-conversations.ts:110-131,240-268).
pub(crate) fn build_chat_payload(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
) -> Result<Value, ConstrainedSamplingError> {
    let supports_images = model.input.contains(&Modality::Image);

    // Stateful 9-char tool-call-id normalizer (Pi createMistralToolCallIdNormalizer).
    let normalizer = MistralToolCallIdNormalizer::default();
    let transformed = transform_messages_with(&ctx.messages, model, |id| normalizer.normalize(id));

    let mut messages = to_chat_messages(&transformed, supports_images);

    // System prompt is prepended (Pi mistral-conversations.ts:260-265).
    if let Some(sp) = &ctx.system_prompt {
        messages.insert(
            0,
            json!({ "role": "system", "content": sanitize_surrogates(sp) }),
        );
    }

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert("stream".to_string(), json!(true));
    obj.insert("messages".to_string(), Value::Array(messages));

    if !ctx.tools.is_empty() {
        obj.insert(
            "tools".to_string(),
            Value::Array(to_function_tools(&ctx.tools)?),
        );
    }
    if let Some(temp) = opts.temperature {
        obj.insert("temperature".to_string(), json!(temp));
    }
    if let Some(max) = opts.max_tokens {
        obj.insert("maxTokens".to_string(), json!(max));
    }
    if let Some(tc) = &opts.tool_choice {
        obj.insert("toolChoice".to_string(), map_tool_choice(tc));
    }

    // Reasoning lowering (Pi `streamSimple`, mistral-conversations.ts:120-130). Direct
    // `MistralOptions.promptMode`/`reasoningEffort` per-request overrides (Pi `buildChatPayload`
    // reads `options.promptMode`/`options.reasoningEffort` verbatim, mistral-conversations.ts:256-257)
    // each win over the computed value, independently of one another.
    let (mut prompt_mode, mut reasoning_effort) = lower_reasoning(model, opts.reasoning);
    if let Some(pm) = opts.mistral_options().and_then(|m| m.prompt_mode) {
        prompt_mode = Some(pm.as_wire());
    }
    if let Some(re) = opts.mistral_options().and_then(|m| m.reasoning_effort) {
        reasoning_effort = Some(re.as_wire().to_string());
    }
    if let Some(pm) = prompt_mode {
        obj.insert("promptMode".to_string(), json!(pm));
    }
    if let Some(re) = reasoning_effort {
        obj.insert("reasoningEffort".to_string(), json!(re));
    }

    if let Some(sid) = should_use_prompt_caching(opts) {
        obj.insert("promptCacheKey".to_string(), json!(sid));
    }

    Ok(Value::Object(obj))
}

/// Lower the unified reasoning level to Mistral's `promptMode`/`reasoningEffort` pair (Pi
/// `streamSimple` + `usesPromptModeReasoning`/`usesReasoningEffort`/`mapReasoningEffort`,
/// mistral-conversations.ts:120-130,621-634).
fn lower_reasoning(
    model: &Model,
    reasoning: ModelThinkingLevel,
) -> (Option<&'static str>, Option<String>) {
    if !reasoning.is_on() {
        return (None, None);
    }
    let clamped = clamp_thinking_level(model, reasoning);
    if !clamped.is_on() {
        return (None, None);
    }
    let should_use = model.reasoning;
    if !should_use {
        return (None, None);
    }

    if uses_reasoning_effort(model) {
        (None, Some(map_reasoning_effort(model, clamped)))
    } else if uses_prompt_mode_reasoning(model) {
        (Some("reasoning"), None)
    } else {
        (None, None)
    }
}

/// `model.id` ∈ the explicit reasoning-effort set (Pi `usesReasoningEffort`,
/// mistral-conversations.ts:621-623).
fn uses_reasoning_effort(model: &Model) -> bool {
    matches!(
        model.id.as_str(),
        "mistral-small-2603" | "mistral-small-latest" | "mistral-medium-3.5"
    )
}

/// `model.reasoning && !usesReasoningEffort` (Pi `usesPromptModeReasoning`,
/// mistral-conversations.ts:625-627).
fn uses_prompt_mode_reasoning(model: &Model) -> bool {
    model.reasoning && !uses_reasoning_effort(model)
}

/// `model.thinkingLevelMap?.[level] ?? "high"` (Pi `mapReasoningEffort`,
/// mistral-conversations.ts:629-634). The result is a Mistral `reasoningEffort` (`"none"`/`"high"`).
fn map_reasoning_effort(model: &Model, level: ModelThinkingLevel) -> String {
    let key = crate::api::compat::thinking_level_key(level);
    if let Some(Some(mapped)) = model.thinking_level_map.as_ref().and_then(|m| m.get(key)) {
        return mapped.clone();
    }
    "high".to_string()
}

/// Map a tool-choice to Mistral's `toolChoice` (Pi `mapToolChoice`,
/// mistral-conversations.ts:636-647). cyrup's [`ToolChoice`] maps onto `"auto"`/`"none"`/
/// `"required"` and the `{type:"function",function:{name}}` object form.
fn map_tool_choice(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Function { name } => {
            json!({ "type": "function", "function": { "name": name } })
        }
    }
}

/// Convert tools to Mistral `FunctionTool`s (Pi `toFunctionTools`,
/// `mistral-conversations.ts:495-507` @**v0.83.0**).
///
/// PROV-011 — `strict` is `resolveJsonSchemaStrictSampling(tool, true) ?? false` (`:497`). Mistral
/// is the one route that passes `true` unconditionally: every Mistral model supports strict
/// schemas, so a `strict: "require"` tool can never fail here and the resolver is infallible in
/// practice — the `Result` is kept so the call reads exactly like pi's.
pub(crate) fn to_function_tools(
    tools: &[ToolDef],
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    tools
        .iter()
        .map(|t| {
            let strict = resolve_json_schema_strict_sampling(t, true)?;
            Ok(json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": json_schema_tool_parameters(t, strict == Some(true))?,
                    "strict": strict.unwrap_or(false),
                },
            }))
        })
        .collect()
}

/// The text for a tool-result message (Pi `buildToolResultText`, mistral-conversations.ts:600-619).
fn build_tool_result_text(
    text: &str,
    has_images: bool,
    supports_images: bool,
    is_error: bool,
) -> String {
    let trimmed = text.trim();
    let error_prefix = if is_error { "[tool error] " } else { "" };
    if !trimmed.is_empty() {
        let image_suffix = if has_images && !supports_images {
            "\n[tool image omitted: model does not support images]"
        } else {
            ""
        };
        return format!("{error_prefix}{trimmed}{image_suffix}");
    }
    if has_images {
        if supports_images {
            return if is_error {
                "[tool error] (see attached image)".to_string()
            } else {
                "(see attached image)".to_string()
            };
        }
        return if is_error {
            "[tool error] (image omitted: model does not support images)".to_string()
        } else {
            "(image omitted: model does not support images)".to_string()
        };
    }
    if is_error {
        "[tool error] (no tool output)".to_string()
    } else {
        "(no tool output)".to_string()
    }
}

/// Convert cyrup [`Message`]s to Mistral chat messages (Pi `toChatMessages`,
/// mistral-conversations.ts:513-598).
pub(crate) fn to_chat_messages(messages: &[Message], supports_images: bool) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        match msg {
            Message::User { content, .. } => {
                let had_images = content.iter().any(|c| matches!(c, Content::Image { .. }));
                let parts: Vec<Value> = content
                    .iter()
                    .filter(|c| matches!(c, Content::Text { .. }) || supports_images)
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => {
                            Some(json!({ "type": "text", "text": sanitize_surrogates(text) }))
                        }
                        Content::Image { data, mime_type } => Some(json!({
                            "type": "image_url",
                            "imageUrl": format!("data:{mime_type};base64,{data}"),
                        })),
                        _ => None,
                    })
                    .collect();
                if !parts.is_empty() {
                    result.push(json!({ "role": "user", "content": parts }));
                } else if had_images && !supports_images {
                    result.push(json!({
                        "role": "user",
                        "content": "(image omitted: model does not support images)",
                    }));
                }
            }
            Message::Assistant(am) => {
                let mut content_parts: Vec<Value> = Vec::new();
                let mut tool_calls: Vec<Value> = Vec::new();
                for block in &am.content {
                    match block {
                        Content::Text { text, .. } => {
                            if !text.trim().is_empty() {
                                content_parts.push(
                                    json!({ "type": "text", "text": sanitize_surrogates(text) }),
                                );
                            }
                        }
                        Content::Thinking { thinking, .. } => {
                            if !thinking.trim().is_empty() {
                                content_parts.push(json!({
                                    "type": "thinking",
                                    "thinking": [{ "type": "text", "text": sanitize_surrogates(thinking) }],
                                }));
                            }
                        }
                        Content::ToolCall(tc) => {
                            let args = serde_json::to_string(&Value::Object(tc.arguments.clone()))
                                .unwrap_or_else(|_| "{}".to_string());
                            tool_calls.push(json!({
                                "id": tc.id.as_str(),
                                "type": "function",
                                "function": { "name": tc.name, "arguments": args },
                            }));
                        }
                        _ => {}
                    }
                }
                if !content_parts.is_empty() || !tool_calls.is_empty() {
                    let mut o = Map::new();
                    o.insert("role".to_string(), json!("assistant"));
                    if !content_parts.is_empty() {
                        o.insert("content".to_string(), Value::Array(content_parts));
                    }
                    if !tool_calls.is_empty() {
                        o.insert("toolCalls".to_string(), Value::Array(tool_calls));
                    }
                    result.push(Value::Object(o));
                }
            }
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                ..
            } => {
                let text_result = content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(sanitize_surrogates(text)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let has_images = content.iter().any(|c| matches!(c, Content::Image { .. }));
                let tool_text =
                    build_tool_result_text(&text_result, has_images, supports_images, *is_error);
                let mut tool_content = vec![json!({ "type": "text", "text": tool_text })];
                if supports_images {
                    for part in content {
                        if let Content::Image { data, mime_type } = part {
                            tool_content.push(json!({
                                "type": "image_url",
                                "imageUrl": format!("data:{mime_type};base64,{data}"),
                            }));
                        }
                    }
                }
                result.push(json!({
                    "role": "tool",
                    "toolCallId": tool_call_id.as_str(),
                    "name": tool_name,
                    "content": tool_content,
                }));
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Tool-call-id normalization (Pi createMistralToolCallIdNormalizer, :153-183)
// ---------------------------------------------------------------------------

/// A deterministic, collision-avoiding 9-char tool-call-id normalizer (Pi
/// `createMistralToolCallIdNormalizer`). Stateful within one request: stable per source id, and
/// distinct source ids never collapse to the same candidate.
#[derive(Default)]
struct MistralToolCallIdNormalizer {
    id_map: RefCell<HashMap<String, String>>,
    reverse_map: RefCell<HashMap<String, String>>,
}

impl MistralToolCallIdNormalizer {
    fn normalize(&self, id: &str) -> String {
        if let Some(existing) = self.id_map.borrow().get(id) {
            return existing.clone();
        }
        let mut attempt = 0u32;
        loop {
            let candidate = derive_mistral_tool_call_id(id, attempt);
            let owner = self.reverse_map.borrow().get(&candidate).cloned();
            if owner.as_deref().map(|o| o == id).unwrap_or(true) {
                self.id_map
                    .borrow_mut()
                    .insert(id.to_string(), candidate.clone());
                self.reverse_map
                    .borrow_mut()
                    .insert(candidate.clone(), id.to_string());
                return candidate;
            }
            attempt += 1;
        }
    }
}

/// Derive a candidate 9-char id for `id` at `attempt` (Pi `deriveMistralToolCallId`,
/// mistral-conversations.ts:175-183).
fn derive_mistral_tool_call_id(id: &str, attempt: u32) -> String {
    let normalized: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if attempt == 0 && normalized.len() == MISTRAL_TOOL_CALL_ID_LENGTH {
        return normalized;
    }
    let seed_base = if normalized.is_empty() {
        id.to_string()
    } else {
        normalized
    };
    let seed = if attempt == 0 {
        seed_base
    } else {
        format!("{seed_base}:{attempt}")
    };
    short_hash(&seed)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(MISTRAL_TOOL_CALL_ID_LENGTH)
        .collect()
}

// ---------------------------------------------------------------------------
// Response decoding
// ---------------------------------------------------------------------------

/// The in-progress text/thinking block being accumulated (Pi `currentBlock`,
/// mistral-conversations.ts:301).
#[derive(Clone, Copy, PartialEq, Eq)]
enum CurrentKind {
    Text,
    Thinking,
}

/// Streaming-decode state (mirrors Pi's `output` accumulation + `consumeChatStream`,
/// mistral-conversations.ts:295-483).
#[derive(Default)]
struct Decoder {
    blocks: Vec<Content>,
    /// Tool-call scratch buffers (the `partialArgs` Pi strips before persisting), keyed by block idx.
    tool_partial_args: HashMap<usize, String>,
    /// `{callId}:{index}` → content-block index (Pi `toolBlocksByKey`).
    tool_blocks_by_key: HashMap<String, usize>,
    current: Option<CurrentKind>,
    usage: Usage,
    response_id: Option<String>,
    /// The settled stop reason, or `None` while none has been delivered — cyrup's spelling of Pi's
    /// `output.stopReason = "pending"` seed (mistral-conversations.ts:153), which is where the
    /// derived `Default` now starts. It previously seeded `Stop`, so a Mistral stream that ended
    /// without a truthy `finishReason` was transcribed as a cleanly completed turn (PROV-010).
    stop_reason: Option<StopReason>,
    /// The choice's own `finishReason`, kept verbatim beside the narrowed [`StopReason`] (pi
    /// `output.rawStopReason = choice.finishReason`,
    /// `v0.84.1 ai/src/api/mistral-conversations.ts:356`). PORT BUG, not version lag: the write is
    /// present at v0.83.0 too, at the same line (`v0.83.0
    /// ai/src/api/mistral-conversations.ts:356`), and cyrup never ported it. Written under the same
    /// truthiness guard pi uses (`:355`), so a `null`/`""` `finishReason` leaves it unset.
    raw_stop_reason: Option<String>,
    error_message: Option<String>,
}

impl Decoder {
    fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone();
        usage.cost = compute_cost(&model.cost, &usage);
        AssistantMessage {
            content: self.content_snapshot(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: self.response_id.clone(),
            diagnostics: None,
            usage,
            // In-flight snapshots carry Pi's `"pending"` seed (mistral-conversations.ts:153). The
            // TERMINAL event never takes this value: it goes through
            // `StreamEvent::end_of_stream`, which routes `None`/`Pending` to the `error` terminal.
            stop_reason: self.stop_reason.unwrap_or(StopReason::Pending),
            deferred: None,
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
            timestamp: now_millis(),
        }
    }

    /// The content blocks with tool arguments re-parsed from their scratch buffers.
    fn content_snapshot(&self) -> Vec<Content> {
        self.blocks
            .iter()
            .enumerate()
            .map(|(i, b)| match b {
                Content::ToolCall(tc) => {
                    let args = self
                        .tool_partial_args
                        .get(&i)
                        .map(|p| parse_streaming_json_object(Some(p)))
                        .unwrap_or_else(|| tc.arguments.clone());
                    Content::ToolCall(ToolCall {
                        arguments: args,
                        ..tc.clone()
                    })
                }
                other => other.clone(),
            })
            .collect()
    }

    fn block_index(&self) -> usize {
        self.blocks.len().saturating_sub(1)
    }
}

/// Drive the Mistral SSE frame stream into ordered [`StreamEvent`]s (1:1 with Pi `consumeChatStream`,
/// mistral-conversations.ts:295-483).
pub(crate) async fn decode_stream<S>(mut frames: S, model: &Model, api: &ApiId, sink: &EventSink)
where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    let provider = model.provider.clone();
    let model_id = model.id.as_str().to_string();

    let mut dec = Decoder::default();
    if !sink
        .send(StreamEvent::Start {
            partial: dec.snapshot(model, api),
        })
        .await
    {
        return;
    }

    while let Some(frame) = frames.next().await {
        let frame = match frame {
            Ok(f) => f,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };
        let data = frame.data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Some(chunk) = parse_json_with_repair(data) else {
            emit_error(
                &dec,
                model,
                api,
                sink,
                "Could not parse Mistral SSE chunk".to_string(),
            )
            .await;
            return;
        };
        if !process_chunk(&chunk, &mut dec, model, api, sink).await {
            return; // consumer dropped
        }
    }

    // Close a trailing text/thinking block, then finalize every tool block (Pi
    // mistral-conversations.ts:467-482).
    if !close_current(&mut dec, model, api, sink).await {
        return;
    }
    if !finalize_tool_blocks(&mut dec, model, api, sink).await {
        return;
    }

    if matches!(
        dec.stop_reason,
        Some(StopReason::Aborted) | Some(StopReason::Error)
    ) {
        emit_error(
            &dec,
            model,
            api,
            sink,
            dec.error_message
                .clone()
                .unwrap_or_else(|| "An unknown error occurred".to_string()),
        )
        .await;
        return;
    }

    // No chunk carried a truthy `finishReason` → TRUNCATED. Pi throws
    // "Mistral stream ended without a finish reason" (mistral-conversations.ts:88-90).
    sink.send(StreamEvent::end_of_stream(
        dec.snapshot(model, api),
        dec.stop_reason,
        "Mistral stream ended without a finish reason",
    ))
    .await;
}

/// Process one decoded `CompletionChunk`. Returns `false` if the consumer dropped the stream.
async fn process_chunk(
    chunk: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    if dec.response_id.is_none()
        && let Some(id) = chunk.get("id").and_then(Value::as_str)
        && !id.is_empty()
    {
        dec.response_id = Some(id.to_string());
    }

    if let Some(usage) = chunk.get("usage") {
        apply_usage(&mut dec.usage, usage);
    }

    let choice = match chunk.get("choices").and_then(|c| c.get(0)) {
        Some(c) => c,
        None => return true,
    };

    // Pi guards with `if (choice.finishReason)` (mistral-conversations.ts:355) — a JS TRUTHINESS
    // test, so `null`, `undefined` and `""` all leave `output.stopReason` at its `"pending"` seed
    // and end the stream as truncated. The previous `else if is_null → map(None)` branch settled
    // such a stream on a clean `Stop`, which is the PROV-010 defect in its second form: a Mistral
    // stream whose final chunk carries `"finishReason": null` was transcribed as a completed turn.
    if let Some(reason) = choice
        .get("finishReason")
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty())
    {
        // pi records the raw reason first (`v0.84.1 ai/src/api/mistral-conversations.ts:356`), so a
        // `content_filter` / future reason names itself on the turn even after the narrowing map.
        dec.raw_stop_reason = Some(reason.to_string());
        let (stop, err) = map_chat_stop_reason(Some(reason));
        dec.stop_reason = Some(stop);
        if let Some(err) = err {
            dec.error_message = Some(err);
        }
    }

    let delta = match choice.get("delta") {
        Some(d) => d,
        None => return true,
    };

    // Content (string OR an array of content chunks).
    if let Some(content) = delta.get("content").filter(|c| !c.is_null())
        && !process_content(content, dec, model, api, sink).await
    {
        return false;
    }

    // Tool calls.
    if let Some(tool_calls) = delta.get("toolCalls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            if !process_tool_call(tool_call, dec, model, api, sink).await {
                return false;
            }
        }
    }

    true
}

/// Handle a `delta.content` value (Pi mistral-conversations.ts:355-416).
async fn process_content(
    content: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    // `string` content collapses to a single text item.
    if let Some(s) = content.as_str() {
        return push_text(dec, model, api, sink, &sanitize_surrogates(s)).await;
    }
    let Some(items) = content.as_array() else {
        return true;
    };
    for item in items {
        if let Some(s) = item.as_str() {
            if !push_text(dec, model, api, sink, &sanitize_surrogates(s)).await {
                return false;
            }
            continue;
        }
        match item.get("type").and_then(Value::as_str) {
            Some("thinking") => {
                let text = item
                    .get("thinking")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p.get("text").and_then(Value::as_str))
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                let delta = sanitize_surrogates(&text);
                if delta.is_empty() {
                    continue;
                }
                if !push_thinking(dec, model, api, sink, &delta).await {
                    return false;
                }
            }
            Some("text") => {
                let text = item.get("text").and_then(Value::as_str).unwrap_or("");
                if !push_text(dec, model, api, sink, &sanitize_surrogates(text)).await {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// Append a text delta, opening/closing blocks as needed.
async fn push_text(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    delta: &str,
) -> bool {
    if dec.current != Some(CurrentKind::Text) {
        if !close_current(dec, model, api, sink).await {
            return false;
        }
        dec.blocks.push(Content::text(""));
        dec.current = Some(CurrentKind::Text);
        let idx = dec.block_index();
        let partial = dec.snapshot(model, api);
        if !sink
            .send(StreamEvent::TextStart {
                content_index: idx,
                partial,
            })
            .await
        {
            return false;
        }
    }
    let idx = dec.block_index();
    if let Some(Content::Text { text, .. }) = dec.blocks.get_mut(idx) {
        text.push_str(delta);
    }
    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::TextDelta {
        content_index: idx,
        delta: delta.to_string(),
        partial,
    })
    .await
}

/// Append a thinking delta, opening/closing blocks as needed.
async fn push_thinking(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    delta: &str,
) -> bool {
    if dec.current != Some(CurrentKind::Thinking) {
        if !close_current(dec, model, api, sink).await {
            return false;
        }
        dec.blocks.push(Content::thinking(""));
        dec.current = Some(CurrentKind::Thinking);
        let idx = dec.block_index();
        let partial = dec.snapshot(model, api);
        if !sink
            .send(StreamEvent::ThinkingStart {
                content_index: idx,
                partial,
            })
            .await
        {
            return false;
        }
    }
    let idx = dec.block_index();
    if let Some(Content::Thinking { thinking, .. }) = dec.blocks.get_mut(idx) {
        thinking.push_str(delta);
    }
    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::ThinkingDelta {
        content_index: idx,
        delta: delta.to_string(),
        partial,
    })
    .await
}

/// Handle one streamed tool-call delta (Pi mistral-conversations.ts:418-464).
async fn process_tool_call(
    tool_call: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    // A tool call closes any open text/thinking block.
    if dec.current.is_some() && !close_current(dec, model, api, sink).await {
        return false;
    }

    let index = tool_call.get("index").and_then(Value::as_i64).unwrap_or(0);
    let provided_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && *id != "null");
    let call_id = match provided_id {
        Some(id) => id.to_string(),
        None => derive_mistral_tool_call_id(&format!("toolcall:{index}"), 0),
    };
    let key = format!("{call_id}:{index}");

    // Open a new tool block on first sight of this key (Pi mistral-conversations.ts:439-450).
    if !dec.tool_blocks_by_key.contains_key(&key) {
        let name = tool_call
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        dec.blocks.push(Content::ToolCall(ToolCall {
            id: ToolCallId::from(call_id.as_str()),
            name,
            arguments: Map::new(),
            thought_signature: None,
        }));
        let block_idx = dec.block_index();
        dec.tool_blocks_by_key.insert(key.clone(), block_idx);
        dec.tool_partial_args.insert(block_idx, String::new());
        let partial = dec.snapshot(model, api);
        if !sink
            .send(StreamEvent::ToolCallStart {
                content_index: block_idx,
                partial,
            })
            .await
        {
            return false;
        }
    }

    let block_idx = match dec.tool_blocks_by_key.get(&key) {
        Some(i) => *i,
        None => return true,
    };

    let args_delta = match tool_call.get("function").and_then(|f| f.get("arguments")) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
        None => String::new(),
    };
    if let Some(buf) = dec.tool_partial_args.get_mut(&block_idx) {
        buf.push_str(&args_delta);
    }

    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::ToolCallDelta {
        content_index: block_idx,
        delta: args_delta,
        partial,
    })
    .await
}

/// Emit the `*_end` for the in-progress text/thinking block, if any.
async fn close_current(dec: &mut Decoder, model: &Model, api: &ApiId, sink: &EventSink) -> bool {
    let Some(kind) = dec.current.take() else {
        return true;
    };
    let idx = dec.block_index();
    let partial = dec.snapshot(model, api);
    let ev = match (kind, dec.blocks.get(idx)) {
        (CurrentKind::Text, Some(Content::Text { text, .. })) => StreamEvent::TextEnd {
            content_index: idx,
            content: text.clone(),
            partial,
        },
        (CurrentKind::Thinking, Some(Content::Thinking { thinking, .. })) => {
            StreamEvent::ThinkingEnd {
                content_index: idx,
                content: thinking.clone(),
                partial,
            }
        }
        _ => return true,
    };
    sink.send(ev).await
}

/// Finalize every tool block: parse its scratch buffer and emit `toolcall_end` (Pi
/// mistral-conversations.ts:468-482). Emitted in ascending content-block order.
async fn finalize_tool_blocks(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let mut indices: Vec<usize> = dec.tool_blocks_by_key.values().copied().collect();
    indices.sort_unstable();
    for idx in indices {
        let (id, name) = match dec.blocks.get(idx) {
            Some(Content::ToolCall(tc)) => (tc.id.clone(), tc.name.clone()),
            _ => continue,
        };
        let args = dec
            .tool_partial_args
            .get(&idx)
            .map(|p| parse_streaming_json_object(Some(p)))
            .unwrap_or_default();
        let tool_call = ToolCall {
            id,
            name,
            arguments: args.clone(),
            thought_signature: None,
        };
        if let Some(Content::ToolCall(tc)) = dec.blocks.get_mut(idx) {
            tc.arguments = args;
        }
        let partial = dec.snapshot(model, api);
        if !sink
            .send(StreamEvent::ToolCallEnd {
                content_index: idx,
                tool_call,
                partial,
            })
            .await
        {
            return false;
        }
    }
    true
}

/// Apply Mistral `usage` (Pi mistral-conversations.ts:333-345).
fn apply_usage(usage: &mut Usage, raw: &Value) {
    let prompt = raw.get("promptTokens").and_then(Value::as_u64).unwrap_or(0);
    let cached = mistral_cached_prompt_tokens(raw, prompt);
    usage.input = prompt.saturating_sub(cached);
    usage.output = raw
        .get("completionTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.cache_read = cached;
    usage.cache_write = 0;
    usage.total_tokens = raw
        .get("totalTokens")
        .and_then(Value::as_u64)
        .filter(|t| *t > 0)
        .unwrap_or(usage.input + usage.output + usage.cache_read + usage.cache_write);
}

/// Extract the cached prompt tokens across Mistral's several spellings (Pi
/// `getMistralCachedPromptTokens`, mistral-conversations.ts:274-293), clamped to `[0, promptTokens]`.
fn mistral_cached_prompt_tokens(raw: &Value, prompt_tokens: u64) -> u64 {
    let candidates = [
        raw.get("promptTokensDetails")
            .and_then(|d| d.get("cachedTokens")),
        raw.get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens")),
        raw.get("promptTokenDetails")
            .and_then(|d| d.get("cachedTokens")),
        raw.get("prompt_token_details")
            .and_then(|d| d.get("cached_tokens")),
        raw.get("numCachedTokens"),
        raw.get("num_cached_tokens"),
    ];
    let cached = candidates
        .into_iter()
        .flatten()
        .find_map(Value::as_u64)
        .unwrap_or(0);
    cached.min(prompt_tokens)
}

/// Map a Mistral `finishReason` to a cyrup [`StopReason`] (Pi `mapChatStopReason`,
/// mistral-conversations.ts:662-677).
///
/// The `Option` mirrors Pi's `reason: string | null` signature; the `None` arm is unreachable from
/// the streaming decoder, because Pi guards the call with the truthiness test
/// `if (choice.finishReason)` (`:355`) and cyrup matches that — a null/empty `finishReason` leaves
/// the turn unsettled rather than mapping it to `Stop`.
///
/// Returns `(stop_reason, error_message)`, mirroring pi's `{ stopReason, errorMessage? }` tuple —
/// the same shape [`crate::api::anthropic_messages`]'s `map_stop_reason` already uses.
///
/// The unknown arm is the point. This previously returned a bare [`StopReason`] whose catch-all was
/// `Some(_) => StopReason::Stop`, so ANY finish reason outside the five known values — `content_filter`,
/// or anything Mistral adds later — was transcribed as a clean, successful turn: the agent loop saw
/// `Stop`, emitted `turn_end` with no tool calls, and ended the run with no error banner and no
/// retry, carrying only whatever partial text arrived before the cutoff. pi's `default` arm returns
/// `{ stopReason: "error", errorMessage: `Provider stopped with: ${reason}` }` (`:674-675`), and its
/// `"error"` arm likewise carries `"Provider stopped with: error"` (`:672-673`) rather than letting
/// the call site fall back to the generic `"An unknown error occurred"`.
fn map_chat_stop_reason(reason: Option<&str>) -> (StopReason, Option<String>) {
    match reason {
        None | Some("stop") => (StopReason::Stop, None),
        Some("length") | Some("model_length") => (StopReason::Length, None),
        Some("tool_calls") => (StopReason::ToolUse, None),
        Some("error") => (
            StopReason::Error,
            Some("Provider stopped with: error".to_string()),
        ),
        Some(other) => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
    }
}

/// Emit a terminal error event carrying the partial snapshot (Pi catch block,
/// mistral-conversations.ts:92-101).
async fn emit_error(dec: &Decoder, model: &Model, api: &ApiId, sink: &EventSink, message: String) {
    let mut msg = dec.snapshot(model, api);
    msg.stop_reason = StopReason::Error;
    msg.error_message = Some(message);
    sink.send(StreamEvent::terminal(msg)).await;
}

/// Current unix time in milliseconds (0 on a clock error — never panics).
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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

    /// pi `mapChatStopReason` (`mistral-conversations.ts:662-677`). The unknown arm is the whole
    /// point: before this, `Some(_) => StopReason::Stop` meant a provider-terminated turn was
    /// transcribed as a clean success — no error banner, no retry, just the partial text that
    /// arrived before the cutoff.
    #[test]
    fn an_unrecognized_finish_reason_is_an_error_not_a_clean_stop() {
        // The reason that motivated this: a real Mistral value outside the known five.
        let (stop, err) = map_chat_stop_reason(Some("content_filter"));
        assert_eq!(stop, StopReason::Error, "must NOT be transcribed as Stop");
        assert_eq!(err.as_deref(), Some("Provider stopped with: content_filter"));

        // Anything Mistral adds later behaves the same way, by construction.
        let (stop, err) = map_chat_stop_reason(Some("some_future_reason"));
        assert_eq!(stop, StopReason::Error);
        assert_eq!(err.as_deref(), Some("Provider stopped with: some_future_reason"));

        // pi's explicit `"error"` arm carries its own message rather than letting the call site
        // fall back to the generic "An unknown error occurred".
        let (stop, err) = map_chat_stop_reason(Some("error"));
        assert_eq!(stop, StopReason::Error);
        assert_eq!(err.as_deref(), Some("Provider stopped with: error"));

        // The known-good arms stay clean and carry no message.
        for (reason, expected) in [
            (None, StopReason::Stop),
            (Some("stop"), StopReason::Stop),
            (Some("length"), StopReason::Length),
            (Some("model_length"), StopReason::Length),
            (Some("tool_calls"), StopReason::ToolUse),
        ] {
            let (stop, err) = map_chat_stop_reason(reason);
            assert_eq!(stop, expected, "{reason:?}");
            assert_eq!(err, None, "{reason:?} must carry no errorMessage");
        }
    }

    use crate::api::channel;
    use crate::model::ModelCost;
    use crate::stream::sse::decode_sse_bytes;
    use cyrup_core::{SessionId, ToolCallId as CoreToolCallId};

    fn model_with(id: &str, reasoning: bool) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            api: API_ID.into(),
            provider: "mistral".into(),
            base_url: "https://api.mistral.ai".to_string(),
            reasoning,
            input: vec![Modality::Text],
            cost: ModelCost {
                input: 0.4,
                output: 2.0,
                cache_read: 0.04,
                cache_write: 0.0,
                tiers: None,
            },
            context_window: 256_000,
            max_tokens: 4096,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn user_ctx(text: &str) -> Context {
        Context {
            system_prompt: Some("be brief".to_string()),
            messages: vec![Message::User {
                content: vec![Content::text(text)],
                timestamp: 0,
            }],
            tools: Vec::new(),
        }
    }

    #[test]
    fn url_appends_chat_completions() {
        assert_eq!(
            chat_url("https://api.mistral.ai"),
            "https://api.mistral.ai/v1/chat/completions"
        );
        assert_eq!(
            chat_url("https://api.mistral.ai/"),
            "https://api.mistral.ai/v1/chat/completions"
        );
    }

    #[test]
    fn headers_use_bearer_and_affinity() {
        let m = model_with("mistral-medium-2604", true);
        let opts = StreamOptions {
            session_id: Some(SessionId::from("sess-1")),
            ..Default::default()
        };
        let headers = build_headers(&m, &opts, "sk-mistral");
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|v| v.clone())
                .as_deref(),
            Some("Bearer sk-mistral")
        );
        // x-affinity set from the session id (default cacheRetention is not "none").
        assert_eq!(
            headers.get("x-affinity").and_then(|v| v.clone()).as_deref(),
            Some("sess-1")
        );
    }

    #[test]
    fn build_payload_basic_shape() {
        let m = model_with("codestral-latest", false);
        let opts = StreamOptions {
            max_tokens: Some(1000),
            temperature: Some(0.3),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("hello"), &opts);
        assert_eq!(body["model"], "codestral-latest");
        assert_eq!(body["stream"], true);
        assert_eq!(body["maxTokens"], 1000);
        // system prompt prepended.
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "be brief");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"][0]["text"], "hello");
        // non-reasoning: no promptMode / reasoningEffort.
        assert!(body.get("promptMode").is_none());
        assert!(body.get("reasoningEffort").is_none());
    }

    #[test]
    fn reasoning_effort_models_emit_effort() {
        let m = model_with("mistral-small-latest", true);
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["reasoningEffort"], "high");
        assert!(body.get("promptMode").is_none());
    }

    #[test]
    fn prompt_mode_reasoning_for_other_reasoning_models() {
        let m = model_with("magistral-medium-latest", true);
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::Medium,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["promptMode"], "reasoning");
        assert!(body.get("reasoningEffort").is_none());
    }

    /// Byte-diff vs Pi `buildChatPayload` (mistral-conversations.ts:256): a direct
    /// `MistralOptions.promptMode` override is written verbatim, overriding the unified-`reasoning`
    /// lowering. Proven two ways: (a) it ADDS `promptMode` on a non-reasoning request that the
    /// lowering leaves bare, and (b) it overrides on a model whose lowering would otherwise emit
    /// `reasoningEffort` only.
    #[test]
    fn mistral_prompt_mode_override_threads_to_payload() {
        // (a) Non-reasoning model + no unified reasoning ⇒ lowering yields no promptMode; the direct
        //     override supplies it. Pi: `if (options?.promptMode) payload.promptMode = options.promptMode`.
        let m = model_with("codestral-latest", false);
        let opts = StreamOptions {
            api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
                prompt_mode: Some(MistralPromptMode::Reasoning),
                ..Default::default()
            })),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["promptMode"], "reasoning");
        // The lowering contributed nothing here, so reasoningEffort stays absent.
        assert!(body.get("reasoningEffort").is_none());

        // (b) A reasoning-effort model at High would lower to `reasoningEffort:"high"` with no
        //     promptMode; the override adds `promptMode:"reasoning"` on top (Pi reads both fields
        //     independently from `options`).
        let m = model_with("mistral-small-latest", true);
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
                prompt_mode: Some(MistralPromptMode::Reasoning),
                ..Default::default()
            })),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["promptMode"], "reasoning");
        assert_eq!(body["reasoningEffort"], "high");

        // Control: without the override the same request omits promptMode (proving the override drove
        // the bytes above, not the lowering).
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert!(body.get("promptMode").is_none());
        assert_eq!(body["reasoningEffort"], "high");
    }

    /// Byte-diff vs Pi `buildChatPayload` (mistral-conversations.ts:257): a direct
    /// `MistralOptions.reasoningEffort` override is written verbatim, independent of `promptMode` and
    /// of the unified-`reasoning` lowering. Pi: `if (options?.reasoningEffort) payload.reasoningEffort
    /// = options.reasoningEffort`. Proven three ways.
    #[test]
    fn mistral_reasoning_effort_override_threads_to_payload() {
        // (a) Non-reasoning model + no unified reasoning ⇒ lowering yields no reasoningEffort; the
        //     direct override supplies `"high"` with no promptMode (independent of the promptMode field).
        let m = model_with("codestral-latest", false);
        let opts = StreamOptions {
            api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
                reasoning_effort: Some(MistralReasoningEffort::High),
                ..Default::default()
            })),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["reasoningEffort"], "high");
        assert!(body.get("promptMode").is_none());

        // (b) `"none"` is also written verbatim (Pi's `if (options?.reasoningEffort)` is truthy for
        //     the non-empty string `"none"`).
        let opts = StreamOptions {
            api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
                reasoning_effort: Some(MistralReasoningEffort::None),
                ..Default::default()
            })),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["reasoningEffort"], "none");

        // (c) The override REPLACES the value the lowering would otherwise compute. A reasoning-effort
        //     model at High lowers to `reasoningEffort:"high"`; overriding with `"none"` wins.
        let m = model_with("mistral-small-latest", true);
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
                reasoning_effort: Some(MistralReasoningEffort::None),
                ..Default::default()
            })),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["reasoningEffort"], "none");
        // Control: without the override the same request lowers to "high" (proving the override drove
        // the bytes above).
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["reasoningEffort"], "high");
    }

    #[test]
    fn prompt_cache_key_set_with_session() {
        let m = model_with("codestral-latest", false);
        let opts = StreamOptions {
            session_id: Some(SessionId::from("s9")),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["promptCacheKey"], "s9");
    }

    #[test]
    fn tools_encode_function_shape() {
        let mut ctx = user_ctx("use a tool");
        ctx.tools = vec![ToolDef {
            name: "read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({ "type": "object", "properties": { "p": { "type": "string" } } }),
            constrained_sampling: None,
        }];
        let m = model_with("codestral-latest", false);
        let opts = StreamOptions {
            tool_choice: Some(ToolChoice::Required),
            ..Default::default()
        };
        let body = build_body(&m, &ctx, &opts);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert_eq!(body["tools"][0]["function"]["strict"], false);
        assert_eq!(body["toolChoice"], "required");
    }

    #[test]
    fn tool_result_message_shape() {
        let messages = vec![Message::ToolResult {
            tool_call_id: CoreToolCallId::from("call12345"),
            tool_name: "read".to_string(),
            content: vec![Content::text("file body")],
            is_error: false,
            details: None,
            timestamp: 0,
            usage: None,
            added_tool_names: Vec::new(),
        }];
        let out = to_chat_messages(&messages, false);
        assert_eq!(out[0]["role"], "tool");
        assert_eq!(out[0]["toolCallId"], "call12345");
        assert_eq!(out[0]["name"], "read");
        assert_eq!(out[0]["content"][0]["text"], "file body");
    }

    #[test]
    fn tool_call_id_normalizer_is_9_chars_and_stable() {
        let n = MistralToolCallIdNormalizer::default();
        let a = n.normalize("call_abc/def!");
        assert_eq!(a.chars().count(), 9);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
        // stable for the same source id.
        assert_eq!(n.normalize("call_abc/def!"), a);
        // an already-9-char alphanumeric id passes through unchanged.
        assert_eq!(n.normalize("abcdefghi"), "abcdefghi");
    }

    /// PORT BUG (present at v0.83.0, never ported): pi writes
    /// `output.rawStopReason = choice.finishReason`
    /// (`v0.84.1 ai/src/api/mistral-conversations.ts:356`, same line at v0.83.0). cyrup filled
    /// `raw_stop_reason: None`, so a `content_filter` stop and an unrecognized future reason were
    /// indistinguishable once both collapsed into [`StopReason::Error`].
    #[tokio::test]
    async fn a_finish_reason_is_recorded_raw_beside_the_narrowed_one() {
        let m = model_with("codestral-latest", false);

        let raw = "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"content_filter\"}]}\n\ndata: [DONE]\n\n";
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        let Some(StreamEvent::Error { error, .. }) = events.last() else {
            panic!("expected an error terminal, got {:?}", events.last());
        };
        assert_eq!(error.raw_stop_reason.as_deref(), Some("content_filter"));

        // MIRROR 1: a clean `stop` keeps its raw word on the `done` terminal.
        let raw = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finishReason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        let Some(StreamEvent::Done { message, .. }) = events.last() else {
            panic!("expected a done terminal, got {:?}", events.last());
        };
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("stop"));

        // MIRROR 2: pi's guard is `if (choice.finishReason)` (`:355`), so a null one assigns
        // nothing — the field stays absent on the truncation terminal.
        let raw = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finishReason\":null}]}\n\ndata: [DONE]\n\n";
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        let last = events.last().expect("a terminal");
        assert_eq!(
            last.terminal_message()
                .and_then(|t| t.raw_stop_reason.clone()),
            None
        );
    }

    async fn collect(frames_bytes: Vec<u8>, m: &Model) -> Vec<StreamEvent> {
        let (sink, mut rx) = channel(64);
        let api = ApiId::from(API_ID);
        let frames = decode_sse_bytes(frames_bytes);
        let m2 = m.clone();
        let api2 = api.clone();
        let task = tokio::spawn(async move {
            decode_stream(frames, &m2, &api2, &sink).await;
        });
        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        task.await.unwrap();
        events
    }

    #[tokio::test]
    async fn decodes_text_and_tool_stream() {
        let raw = concat!(
            "data: {\"id\":\"resp_1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"toolCalls\":[{\"id\":\"abcdefghi\",\"index\":0,\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"a\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finishReason\":\"tool_calls\"}],\"usage\":{\"promptTokens\":10,\"completionTokens\":4,\"totalTokens\":14}}\n\n",
            "data: [DONE]\n\n",
        );
        let m = model_with("codestral-latest", false);
        let events = collect(raw.as_bytes().to_vec(), &m).await;

        assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::TextDelta { delta, .. } if delta == "Hello"))
        );
        let tool = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
                _ => None,
            })
            .expect("toolcall_end");
        assert_eq!(tool.id.as_str(), "abcdefghi");
        assert_eq!(tool.name, "read");
        assert_eq!(
            tool.arguments.get("path").and_then(Value::as_str),
            Some("a")
        );

        let msg = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::Done { message, .. } => Some(message.clone()),
                _ => None,
            })
            .expect("done terminal");
        assert_eq!(msg.stop_reason, StopReason::ToolUse);
        assert_eq!(msg.response_id.as_deref(), Some("resp_1"));
        assert_eq!(msg.usage.input, 10);
        assert_eq!(msg.usage.output, 4);
        assert_eq!(msg.usage.total_tokens, 14);
    }

    #[tokio::test]
    async fn decodes_thinking_chunks() {
        let raw = concat!(
            "data: {\"id\":\"r\",\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"thinking\",\"thinking\":[{\"type\":\"text\",\"text\":\"ponder\"}]}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":[{\"type\":\"text\",\"text\":\"answer\"}]},\"finishReason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let m = model_with("magistral-small", true);
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        assert!(
            events.iter().any(
                |e| matches!(e, StreamEvent::ThinkingDelta { delta, .. } if delta == "ponder")
            )
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::TextDelta { delta, .. } if delta == "answer"))
        );
        let msg = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::Done { message, .. } => Some(message.clone()),
                _ => None,
            })
            .expect("done");
        assert_eq!(msg.stop_reason, StopReason::Stop);
        assert!(matches!(msg.content[0], Content::Thinking { .. }));
        assert!(matches!(msg.content[1], Content::Text { .. }));
    }
}

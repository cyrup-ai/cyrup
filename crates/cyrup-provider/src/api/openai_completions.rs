//! The `openai-completions` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking the OpenAI Chat Completions streaming API (`POST {baseUrl}/chat/
//! completions`, SSE chunks with `choices[].delta.{content,reasoning_content,tool_calls[]}` +
//! `finish_reason`, and a final `usage` chunk via `stream_options.include_usage=true`). Shared by
//! every OpenAI-compatible provider (openai, together, groq, …) — they differ only in base URL,
//! auth, and catalog (R-01-007). Ports Pi's proven `openai-completions.ts` encoder/decoder.
//!
//! Wire JSON uses the vendor's own field names (snake_case), NOT the cyrup camelCase convention.

use crate::HeaderMap;
use crate::api::compat::{
    CacheControlFormat, MaxTokensField, ResolvedCompat, ThinkingFormat,
    clamp_openai_prompt_cache_key, get_compat, level_map_lookup, mapped_effort_or, off_is_not_null,
    off_value_or, sanitize_surrogates, thinking_level_key,
};
use crate::api::{ApiImpl, EventSink};
use crate::auth::{AuthResult, ProviderEnv};
use crate::context::{Context, ToolDef};
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::{SseFrame, SseRequest, build_client_for_target, open_sse};
use crate::stream::{CacheRetention, StreamEvent, StreamOptions};
use crate::usage::apply_cost;
use crate::utils::hash::short_hash;
use cyrup_core::{
    ApiId, AssistantMessage, CancelToken, Content, Message, ModelThinkingLevel, StopReason,
    ToolCall, ToolCallId, Usage,
};
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::OPENAI_COMPLETIONS;

/// The `ApiImpl` for `"openai-completions"`.
pub struct OpenAiCompletionsApi {
    api: ApiId,
}

impl Default for OpenAiCompletionsApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
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
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        // Resolve compat + the effective cache retention once (Pi `stream` L179-181), so both the
        // header build (session affinity) and the body build see the same view.
        let compat = get_compat(model);
        let cache = resolve_cache_retention(opts.cache_retention, auth.env.as_ref());
        // Pi: `cacheSessionId = cacheRetention === "none" ? undefined : options?.sessionId` (L181).
        let cache_session_id = match cache {
            CacheRetention::None => None,
            _ => opts.session_id.as_ref().map(|s| s.as_str()),
        };

        // gap-08 #2: let a `before_provider_request` extension inspect/replace the outbound body.
        let body = crate::stream::apply_on_payload(
            opts,
            model,
            build_body_with_env(model, ctx, opts, auth.env.as_ref()),
        )
        .await;
        let headers = build_headers(model, auth, opts, &compat, cache_session_id);
        let req = SseRequest {
            method: reqwest::Method::POST,
            url,
            headers,
            body: Some(body),
        };

        // Honor HTTP(S)_PROXY for the live client (Pi resolveHttpProxyUrlForTarget,
        // node-http-proxy.ts:92-112).
        let client = match build_client_for_target(
            &req.url,
            &crate::auth::types::EnvAuthContext,
            auth.env.as_ref(),
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
        let frames = match open_sse(&client, req, cancel, None, on_resp).await {
            Ok(s) => s,
            Err(e) => {
                // transport / non-2xx / abort-during-connect → terminal Error (R-01-018/045)
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

/// Resolve the `POST` target: an auth base-url override wins over `model.base_url`. The endpoint is
/// `{base}/chat/completions` (appended unless `base` already names it).
fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
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

/// Build the request headers: `Authorization: Bearer <key>` plus the auth / model / opts header
/// overlays (a `None` value suppresses a default). Precedence (lowest → highest): auth overlay <
/// `model.headers` < session-affinity < `opts.headers` — matching Pi `createClient`
/// (openai-completions.ts:505-524), which seeds `{ ...model.headers }`, then layers session
/// affinity, then merges `optionsHeaders` last so per-request headers win.
///
/// `cache_session_id` is the cache-gated session id (Pi `cacheSessionId`): when the resolved
/// `send_session_affinity_headers` compat flag is set and a session id is available, the
/// `session_id` / `x-client-request-id` / `x-session-affinity` headers are injected (Pi
/// `createClient`, openai-completions.ts:515-519). They are placed after `model.headers` but
/// before the opts overlay so per-request headers can still override them (matching Pi, which
/// merges `optionsHeaders` last).
pub(crate) fn build_headers(
    model: &Model,
    auth: &AuthResult,
    opts: &StreamOptions,
    compat: &ResolvedCompat,
    cache_session_id: Option<&str>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(key) = &auth.auth.api_key {
        headers.insert("Authorization".to_string(), Some(format!("Bearer {key}")));
    }
    if let Some(overlay) = &auth.auth.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    // Top-level per-provider headers (Pi `createClient` seeds `{ ...model.headers }`,
    // openai-completions.ts:505). Layered above the auth overlay and below session affinity / opts
    // so a `None` value suppresses a default header.
    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    // Session-affinity headers (Pi createClient openai-completions.ts:515-519). The flag is
    // currently `false` for every provider in `detect_compat`, but the emission is ported for 1:1
    // parity so an explicit `model.compat.sendSessionAffinityHeaders` override takes effect.
    if compat.send_session_affinity_headers
        && let Some(sid) = cache_session_id
    {
        headers.insert("session_id".to_string(), Some(sid.to_string()));
        headers.insert("x-client-request-id".to_string(), Some(sid.to_string()));
        headers.insert("x-session-affinity".to_string(), Some(sid.to_string()));
    }
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    headers
}

/// Map a unified [`ModelThinkingLevel`] to the OpenAI `reasoning_effort` value (None for `Off`).
fn reasoning_effort(level: ModelThinkingLevel) -> Option<&'static str> {
    match level {
        ModelThinkingLevel::Off => None,
        ModelThinkingLevel::Minimal => Some("minimal"),
        ModelThinkingLevel::Low => Some("low"),
        ModelThinkingLevel::Medium => Some("medium"),
        ModelThinkingLevel::High => Some("high"),
        ModelThinkingLevel::Xhigh => Some("xhigh"),
        // Pi `reasoningEffort` is the level string verbatim (openai-completions.ts:621) and its
        // `OpenAICompletionsOptions.reasoningEffort` union includes `"max"` (:143).
        ModelThinkingLevel::Max => Some("max"),
    }
}

/// Build the Chat Completions request JSON body from the [`Context`] + [`StreamOptions`].
///
/// 1:1 port of Pi `buildParams` (openai-completions.ts L534-687): resolves the compatibility
/// matrix for the model, encodes prompt-cache options, the max-tokens field, `store`, tools +
/// `tool_choice`, the per-provider reasoning encoding, and routing preferences.
/// Test-only convenience wrapper for [`build_body_with_env`] with no env overlay (the request path
/// uses [`build_body_with_env`] directly so it can forward the provider-scoped env).
#[cfg(test)]
pub(crate) fn build_body(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    build_body_with_env(model, ctx, opts, None)
}

/// Resolve a provider env value (Pi `getProviderEnvValue`, provider-env.ts:45-52): the scoped
/// `env` overlay wins over the process environment.
fn provider_env_value(name: &str, env: Option<&ProviderEnv>) -> Option<String> {
    if let Some(map) = env
        && let Some(v) = map.get(name).filter(|v| !v.is_empty())
    {
        return Some(v.clone());
    }
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Resolve the effective cache retention (1:1 port of Pi `resolveCacheRetention`,
/// openai-completions.ts:141-149): an explicit caller value wins; otherwise `PI_CACHE_RETENTION ==
/// "long"` promotes to `Long`; otherwise `Short`.
fn resolve_cache_retention(
    cache_retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> CacheRetention {
    if let Some(c) = cache_retention {
        return c;
    }
    if provider_env_value("PI_CACHE_RETENTION", env).as_deref() == Some("long") {
        return CacheRetention::Long;
    }
    CacheRetention::Short
}

/// Env-aware [`build_body`]: `env` is the provider-scoped overlay (Pi `options.env`) consulted by
/// [`resolve_cache_retention`] for the `PI_CACHE_RETENTION` fallback.
pub(crate) fn build_body_with_env(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    env: Option<&ProviderEnv>,
) -> Value {
    let compat = get_compat(model);
    let cache = resolve_cache_retention(opts.cache_retention, env);
    let mut messages = convert_messages(model, ctx, &compat);
    let cache_control = compat_cache_control(&compat, cache);
    let base_url = model.base_url.as_str();

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert("stream".to_string(), json!(true));

    // Prompt caching (OpenAI `prompt_cache_key` / `prompt_cache_retention`).
    let want_cache_key = (base_url.contains("api.openai.com") && cache != CacheRetention::None)
        || (cache == CacheRetention::Long && compat.supports_long_cache_retention);
    if want_cache_key && let Some(sid) = &opts.session_id {
        obj.insert(
            "prompt_cache_key".to_string(),
            json!(clamp_openai_prompt_cache_key(sid.as_str())),
        );
    }
    if cache == CacheRetention::Long && compat.supports_long_cache_retention {
        obj.insert("prompt_cache_retention".to_string(), json!("24h"));
    }

    if compat.supports_usage_in_streaming {
        obj.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );
    }
    if compat.supports_store {
        obj.insert("store".to_string(), json!(false));
    }

    if let Some(max) = opts.max_tokens {
        match compat.max_tokens_field {
            MaxTokensField::MaxTokens => {
                obj.insert("max_tokens".to_string(), json!(max));
            }
            MaxTokensField::MaxCompletionTokens => {
                obj.insert("max_completion_tokens".to_string(), json!(max));
            }
        }
    }
    if let Some(temp) = opts.temperature {
        obj.insert("temperature".to_string(), json!(temp));
    }

    // Tools (+ z.ai tool_stream) / empty-tools-for-tool-history.
    let has_tool_history = ctx.messages.iter().any(message_has_tool_use);
    let mut tools: Option<Vec<Value>> = None;
    if !ctx.tools.is_empty() {
        tools = Some(convert_tools(&ctx.tools, &compat));
        if compat.zai_tool_stream {
            obj.insert("tool_stream".to_string(), json!(true));
        }
    } else if has_tool_history {
        // Some OpenAI-compatible proxies require `tools` to be present whenever the conversation
        // already contains tool calls / tool results.
        tools = Some(Vec::new());
    }

    // Anthropic-style cache_control markers (OpenRouter `anthropic/*`).
    if let Some(cc) = &cache_control {
        apply_anthropic_cache_control(&mut messages, tools.as_mut(), cc);
    }

    obj.insert("messages".to_string(), Value::Array(messages));
    if let Some(t) = tools {
        obj.insert("tools".to_string(), Value::Array(t));
    }

    // tool_choice — emitted ONLY when the caller specifies one (matches Pi; no auto-injection).
    if let Some(tc) = &opts.tool_choice {
        obj.insert("tool_choice".to_string(), tc.to_wire());
    }

    apply_reasoning(&mut obj, model, opts, &compat);

    // OpenRouter / Vercel AI Gateway routing preferences (read from raw `model.compat`).
    if let Some(c) = &model.compat {
        if let Some(routing) = &c.open_router_routing {
            obj.insert("provider".to_string(), routing.clone());
        }
        if let Some(vg) = &c.vercel_gateway_routing
            && (vg.only.is_some() || vg.order.is_some())
        {
            let mut gateway = Map::new();
            if let Some(only) = &vg.only {
                gateway.insert("only".to_string(), json!(only));
            }
            if let Some(order) = &vg.order {
                gateway.insert("order".to_string(), json!(order));
            }
            obj.insert(
                "providerOptions".to_string(),
                json!({ "gateway": Value::Object(gateway) }),
            );
        }
    }

    Value::Object(obj)
}

/// Apply the per-provider reasoning encoding (Pi `buildParams` reasoning chain, L594-668). Each
/// branch is gated on `model.reasoning` and the resolved `thinking_format`.
fn apply_reasoning(
    obj: &mut Map<String, Value>,
    model: &Model,
    opts: &StreamOptions,
    compat: &ResolvedCompat,
) {
    if !model.reasoning {
        return;
    }
    let map = model.thinking_level_map.as_ref();
    let level = opts.reasoning;
    let key = thinking_level_key(level);
    // `options.reasoningEffort`: `Some(effort)` when reasoning is on, `None` when off.
    let eff: Option<&'static str> = reasoning_effort(level);
    let sre = compat.supports_reasoning_effort;

    match compat.thinking_format {
        ThinkingFormat::Zai => {
            obj.insert(
                "thinking".to_string(),
                json!({ "type": if eff.is_some() { "enabled" } else { "disabled" } }),
            );
            if let Some(e) = eff
                && sre
            {
                // mappedEffort === undefined ? reasoningEffort : mappedEffort; emit only if string.
                let effort = match level_map_lookup(map, key) {
                    None => Some(e.to_string()),
                    Some(None) => None,
                    Some(Some(s)) => Some(s.clone()),
                };
                if let Some(s) = effort {
                    obj.insert("reasoning_effort".to_string(), json!(s));
                }
            }
        }
        ThinkingFormat::Qwen => {
            obj.insert("enable_thinking".to_string(), json!(eff.is_some()));
        }
        ThinkingFormat::QwenChatTemplate => {
            obj.insert(
                "chat_template_kwargs".to_string(),
                json!({ "enable_thinking": eff.is_some(), "preserve_thinking": true }),
            );
        }
        ThinkingFormat::ChatTemplate => {
            if let Some(kwargs) = build_chat_template_kwargs(model, opts, compat) {
                obj.insert("chat_template_kwargs".to_string(), Value::Object(kwargs));
            }
        }
        ThinkingFormat::Deepseek => {
            if eff.is_some() {
                obj.insert("thinking".to_string(), json!({ "type": "enabled" }));
            } else if off_is_not_null(map) {
                obj.insert("thinking".to_string(), json!({ "type": "disabled" }));
            }
            if let Some(e) = eff
                && sre
            {
                obj.insert(
                    "reasoning_effort".to_string(),
                    json!(mapped_effort_or(map, level, e)),
                );
            }
        }
        ThinkingFormat::Openrouter => {
            if let Some(e) = eff {
                obj.insert(
                    "reasoning".to_string(),
                    json!({ "effort": mapped_effort_or(map, level, e) }),
                );
            } else if off_is_not_null(map) {
                obj.insert(
                    "reasoning".to_string(),
                    json!({ "effort": off_value_or(map, "none") }),
                );
            }
        }
        ThinkingFormat::AntLing => {
            if eff.is_some()
                && let Some(Some(s)) = level_map_lookup(map, key)
            {
                obj.insert("reasoning".to_string(), json!({ "effort": s }));
            }
        }
        ThinkingFormat::Together => {
            obj.insert("reasoning".to_string(), json!({ "enabled": eff.is_some() }));
            if let Some(e) = eff
                && sre
            {
                obj.insert(
                    "reasoning_effort".to_string(),
                    json!(mapped_effort_or(map, level, e)),
                );
            }
        }
        ThinkingFormat::StringThinking => {
            if let Some(e) = eff {
                obj.insert(
                    "thinking".to_string(),
                    json!(mapped_effort_or(map, level, e)),
                );
            } else if off_is_not_null(map) {
                obj.insert("thinking".to_string(), json!(off_value_or(map, "none")));
            }
        }
        ThinkingFormat::Openai => {
            // OpenAI-style `reasoning_effort` (Pi's two fallthrough branches).
            if let Some(e) = eff {
                if sre {
                    obj.insert(
                        "reasoning_effort".to_string(),
                        json!(mapped_effort_or(map, level, e)),
                    );
                }
            } else if sre && let Some(Some(s)) = level_map_lookup(map, "off") {
                obj.insert("reasoning_effort".to_string(), json!(s));
            }
        }
    }
}

/// Build `chat_template_kwargs` from `compat.chatTemplateKwargs` (Pi `buildChatTemplateKwargs`).
fn build_chat_template_kwargs(
    model: &Model,
    opts: &StreamOptions,
    compat: &ResolvedCompat,
) -> Option<Map<String, Value>> {
    let mut kwargs = Map::new();
    for (key, value) in &compat.chat_template_kwargs {
        if let Some(resolved) = resolve_chat_template_kwarg_value(model, opts, value) {
            kwargs.insert(key.clone(), resolved);
        }
    }
    if kwargs.is_empty() {
        None
    } else {
        Some(kwargs)
    }
}

/// Resolve one `ChatTemplateKwargValue` (Pi `resolveChatTemplateKwargValue`).
fn resolve_chat_template_kwarg_value(
    model: &Model,
    opts: &StreamOptions,
    value: &Value,
) -> Option<Value> {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Some(value.clone()),
    };
    let map = model.thinking_level_map.as_ref();
    let level = opts.reasoning;
    let eff = reasoning_effort(level);

    if eff.is_none() && obj.get("omitWhenOff").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    if obj.get("$var").and_then(Value::as_str) == Some("thinking.enabled") {
        return Some(json!(eff.is_some()));
    }

    let mapped = if eff.is_some() {
        level_map_lookup(map, thinking_level_key(level))
    } else {
        level_map_lookup(map, "off")
    };
    match mapped {
        None => eff.map(|e| json!(e)),
        Some(Some(s)) => Some(json!(s)),
        Some(None) => None,
    }
}

/// Anthropic-style ephemeral cache-control marker (Pi `getCompatCacheControl`).
fn compat_cache_control(compat: &ResolvedCompat, cache: CacheRetention) -> Option<Value> {
    if compat.cache_control_format != Some(CacheControlFormat::Anthropic)
        || cache == CacheRetention::None
    {
        return None;
    }
    let mut cc = Map::new();
    cc.insert("type".to_string(), json!("ephemeral"));
    if cache == CacheRetention::Long && compat.supports_long_cache_retention {
        cc.insert("ttl".to_string(), json!("1h"));
    }
    Some(Value::Object(cc))
}

/// Apply Anthropic `cache_control` to the system prompt, last tool, and last conversation message
/// (Pi `applyAnthropicCacheControl`).
fn apply_anthropic_cache_control(
    messages: &mut [Value],
    tools: Option<&mut Vec<Value>>,
    cc: &Value,
) {
    add_cache_control_to_system_prompt(messages, cc);
    if let Some(tools) = tools
        && let Some(last) = tools.last_mut()
        && let Some(o) = last.as_object_mut()
    {
        o.insert("cache_control".to_string(), cc.clone());
    }
    add_cache_control_to_last_conversation_message(messages, cc);
}

fn add_cache_control_to_system_prompt(messages: &mut [Value], cc: &Value) {
    for msg in messages.iter_mut() {
        let role = msg.get("role").and_then(Value::as_str);
        if role == Some("system") || role == Some("developer") {
            if let Some(o) = msg.as_object_mut() {
                add_cache_control_to_text_content(o, cc);
            }
            return;
        }
    }
}

fn add_cache_control_to_last_conversation_message(messages: &mut [Value], cc: &Value) {
    for msg in messages.iter_mut().rev() {
        let role = msg.get("role").and_then(Value::as_str);
        if (role == Some("user") || role == Some("assistant"))
            && let Some(o) = msg.as_object_mut()
            && add_cache_control_to_text_content(o, cc)
        {
            return;
        }
    }
}

/// Add `cache_control` to a message's last text content (Pi `addCacheControlToTextContent`).
fn add_cache_control_to_text_content(message: &mut Map<String, Value>, cc: &Value) -> bool {
    match message.get("content") {
        Some(Value::String(s)) => {
            if s.is_empty() {
                return false;
            }
            let text = s.clone();
            let mut part = Map::new();
            part.insert("type".to_string(), json!("text"));
            part.insert("text".to_string(), json!(text));
            part.insert("cache_control".to_string(), cc.clone());
            message.insert(
                "content".to_string(),
                Value::Array(vec![Value::Object(part)]),
            );
            true
        }
        Some(Value::Array(_)) => {
            if let Some(Value::Array(arr)) = message.get_mut("content") {
                for part in arr.iter_mut().rev() {
                    if part.get("type").and_then(Value::as_str) == Some("text")
                        && let Some(o) = part.as_object_mut()
                    {
                        o.insert("cache_control".to_string(), cc.clone());
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// `true` if a message carries a tool call (assistant) or is a tool result.
fn message_has_tool_use(msg: &Message) -> bool {
    match msg {
        Message::ToolResult { .. } => true,
        Message::Assistant(am) => am.content.iter().any(|c| matches!(c, Content::ToolCall(_))),
        Message::User { .. } => false,
    }
}

/// Map cyrup [`ToolDef`]s to OpenAI `tools` entries (Pi `convertTools`). `strict: false` is added
/// only when the provider supports it.
pub(crate) fn convert_tools(tools: &[ToolDef], compat: &ResolvedCompat) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let mut function = Map::new();
            function.insert("name".to_string(), json!(t.name));
            function.insert("description".to_string(), json!(t.description));
            function.insert("parameters".to_string(), t.parameters.clone());
            if compat.supports_strict_mode {
                function.insert("strict".to_string(), json!(false));
            }
            json!({ "type": "function", "function": Value::Object(function) })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Message transformation + conversion (Pi transform-messages.ts + convertMessages)
// ---------------------------------------------------------------------------

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

/// `true` if `am` was produced by exactly this model (Pi `isSameModel`).
fn is_same_model(am: &AssistantMessage, model: &Model) -> bool {
    am.provider == model.provider && am.api == model.api && am.model == model.id.as_str()
}

/// Sanitize an id fragment to the `[a-zA-Z0-9_-]` alphabet OpenAI accepts for tool-call ids
/// (Pi `replace(/[^a-zA-Z0-9_-]/g, "_")`). Output is always ASCII.
fn sanitize_tool_call_id_part(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Normalize a tool-call id for cross-provider replay (Pi `convertMessages.normalizeToolCallId`,
/// openai-completions.ts:1006-1030).
///
/// Responses-API ids arrive as `{call_id}|{item_id}`. Several tool calls in one turn can share a
/// `call_id` and differ only by `item_id`, so keeping just the `call_id` half collapses them into
/// duplicate `tool_call_id`s — which Chat Completions rejects with a 400 when the conversation is
/// replayed onto an openai-completions model. Pi therefore keeps BOTH halves (`{call}_{item}`) and
/// falls back to `{call-prefix}_{8-char shortHash of the whole id}` when the join exceeds the
/// 40-char limit.
fn normalize_tool_call_id(model: &Model, id: &str) -> String {
    if let Some(separator_index) = id.find('|') {
        let call_id = sanitize_tool_call_id_part(id.get(..separator_index).unwrap_or(""));
        let item_id =
            sanitize_tool_call_id_part(id.get(separator_index.saturating_add(1)..).unwrap_or(""));
        let combined_id = if item_id.is_empty() {
            call_id.clone()
        } else {
            format!("{call_id}_{item_id}")
        };
        if combined_id.len() <= 40 {
            return combined_id;
        }
        let hash: String = short_hash(id).chars().take(8).collect();
        // `Math.max(1, 40 - hash.length - 1)` — never truncate the call id to nothing.
        let prefix_len = 40usize.saturating_sub(hash.len().saturating_add(1)).max(1);
        let prefix: String = call_id.chars().take(prefix_len).collect();
        return format!("{prefix}_{hash}");
    }
    if model.provider.as_str() == "openai" {
        return if id.chars().count() > 40 {
            id.chars().take(40).collect()
        } else {
            id.to_string()
        };
    }
    id.to_string()
}

/// Replace image blocks with a text placeholder, de-duplicating consecutive placeholders
/// (Pi `replaceImagesWithPlaceholder`).
fn replace_images_with_placeholder(content: &[Content], placeholder: &str) -> Vec<Content> {
    let mut result = Vec::new();
    let mut previous_was_placeholder = false;
    for block in content {
        match block {
            Content::Image { .. } => {
                if !previous_was_placeholder {
                    result.push(Content::text(placeholder));
                }
                previous_was_placeholder = true;
            }
            Content::Text { text, .. } => {
                let is_placeholder = text == placeholder;
                result.push(block.clone());
                previous_was_placeholder = is_placeholder;
            }
            other => {
                result.push(other.clone());
                previous_was_placeholder = false;
            }
        }
    }
    result
}

/// Downgrade unsupported images to placeholders for non-vision models (Pi
/// `downgradeUnsupportedImages`).
fn downgrade_unsupported_images(messages: &[Message], model: &Model) -> Vec<Message> {
    if model.supports_image_input() {
        return messages.to_vec();
    }
    messages
        .iter()
        .map(|m| match m {
            Message::User { content, timestamp } => Message::User {
                content: replace_images_with_placeholder(
                    content,
                    NON_VISION_USER_IMAGE_PLACEHOLDER,
                ),
                timestamp: *timestamp,
            },
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                details,
                timestamp,
            } => Message::ToolResult {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                content: replace_images_with_placeholder(
                    content,
                    NON_VISION_TOOL_IMAGE_PLACEHOLDER,
                ),
                is_error: *is_error,
                details: details.clone(),
                timestamp: *timestamp,
            },
            other => other.clone(),
        })
        .collect()
}

/// Insert synthetic empty tool results for orphaned tool calls (Pi
/// `transformMessages.insertSyntheticToolResults`).
fn insert_synthetic_tool_results(
    result: &mut Vec<Message>,
    pending: &mut Vec<ToolCall>,
    existing: &mut HashSet<String>,
) {
    if pending.is_empty() {
        return;
    }
    for tc in pending.iter() {
        if !existing.contains(tc.id.as_str()) {
            result.push(Message::ToolResult {
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                content: vec![Content::text("No result provided")],
                is_error: true,
                details: None,
                timestamp: now_millis(),
            });
        }
    }
    pending.clear();
    existing.clear();
}

/// 1:1 port of Pi `transformMessages` (transform-messages.ts): downgrade images, drop/convert
/// cross-model thinking, normalize tool-call ids, skip errored/aborted assistant turns, and
/// synthesize results for orphaned tool calls.
pub(crate) fn transform_messages(messages: &[Message], model: &Model) -> Vec<Message> {
    transform_messages_with(messages, model, |id| normalize_tool_call_id(model, id))
}

/// [`transform_messages`] parameterized by the per-api tool-call-id normalizer (Pi
/// `transformMessages(messages, model, normalizeToolCallId)`, transform-messages.ts:64-67). The
/// `openai-completions` caller passes [`normalize_tool_call_id`]; the `anthropic-messages` caller
/// passes its own 64-char/`^[a-zA-Z0-9_-]+$` normalizer.
pub(crate) fn transform_messages_with(
    messages: &[Message],
    model: &Model,
    normalize: impl Fn(&str) -> String,
) -> Vec<Message> {
    transform_messages_with_source(messages, model, |id, _src| normalize(id))
}

/// [`transform_messages_with`] whose normalizer also receives the source [`AssistantMessage`] (Pi
/// `normalizeToolCallId(id, model, source)`, transform-messages.ts:67/134). The `openai-responses`
/// caller needs `source` to decide whether a tool call is *foreign* (a different provider/api)
/// when rewriting its `call_id|item_id` (openai-responses-shared.ts:109-121).
pub(crate) fn transform_messages_with_source(
    messages: &[Message],
    model: &Model,
    normalize: impl Fn(&str, &AssistantMessage) -> String,
) -> Vec<Message> {
    let mut tool_call_id_map: HashMap<String, String> = HashMap::new();
    let image_aware = downgrade_unsupported_images(messages, model);

    // First pass: per-message transform.
    let transformed: Vec<Message> = image_aware
        .iter()
        .map(|msg| match msg {
            Message::User { .. } => msg.clone(),
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                details,
                timestamp,
            } => {
                if let Some(norm) = tool_call_id_map.get(tool_call_id.as_str()).cloned()
                    && norm != tool_call_id.as_str()
                {
                    return Message::ToolResult {
                        tool_call_id: ToolCallId::from(norm.as_str()),
                        tool_name: tool_name.clone(),
                        content: content.clone(),
                        is_error: *is_error,
                        details: details.clone(),
                        timestamp: *timestamp,
                    };
                }
                msg.clone()
            }
            Message::Assistant(am) => {
                let same = is_same_model(am, model);
                let mut new_content: Vec<Content> = Vec::new();
                for block in &am.content {
                    match block {
                        Content::Thinking {
                            thinking,
                            thinking_signature,
                            redacted,
                        } => {
                            if *redacted {
                                if same {
                                    new_content.push(block.clone());
                                }
                                continue;
                            }
                            if same && thinking_signature.is_some() {
                                new_content.push(block.clone());
                                continue;
                            }
                            if thinking.trim().is_empty() {
                                continue;
                            }
                            if same {
                                new_content.push(block.clone());
                            } else {
                                new_content.push(Content::text(thinking.clone()));
                            }
                        }
                        Content::Text { text, .. } => {
                            if same {
                                new_content.push(block.clone());
                            } else {
                                new_content.push(Content::text(text.clone()));
                            }
                        }
                        Content::ToolCall(tc) => {
                            let mut ntc = tc.clone();
                            if !same && ntc.thought_signature.is_some() {
                                ntc.thought_signature = None;
                            }
                            if !same {
                                let norm = normalize(tc.id.as_str(), am);
                                if norm != tc.id.as_str() {
                                    tool_call_id_map
                                        .insert(tc.id.as_str().to_string(), norm.clone());
                                    ntc.id = ToolCallId::from(norm.as_str());
                                }
                            }
                            new_content.push(Content::ToolCall(ntc));
                        }
                        other => new_content.push(other.clone()),
                    }
                }
                let mut nam = am.clone();
                nam.content = new_content;
                Message::Assistant(nam)
            }
        })
        .collect();

    // Second pass: skip errored/aborted assistants; synthesize orphaned tool results.
    let mut result: Vec<Message> = Vec::new();
    let mut pending: Vec<ToolCall> = Vec::new();
    let mut existing: HashSet<String> = HashSet::new();

    for msg in transformed {
        match &msg {
            Message::Assistant(am) => {
                insert_synthetic_tool_results(&mut result, &mut pending, &mut existing);
                if matches!(am.stop_reason, StopReason::Error | StopReason::Aborted) {
                    continue;
                }
                let tool_calls: Vec<ToolCall> = am
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::ToolCall(tc) => Some(tc.clone()),
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    pending = tool_calls;
                    existing.clear();
                }
                result.push(msg);
            }
            Message::ToolResult { tool_call_id, .. } => {
                existing.insert(tool_call_id.as_str().to_string());
                result.push(msg);
            }
            Message::User { .. } => {
                insert_synthetic_tool_results(&mut result, &mut pending, &mut existing);
                result.push(msg);
            }
        }
    }
    insert_synthetic_tool_results(&mut result, &mut pending, &mut existing);
    result
}

/// Map cyrup [`Message`]s to OpenAI chat messages (Pi `convertMessages`, applying the compat flags).
pub(crate) fn convert_messages(
    model: &Model,
    ctx: &Context,
    compat: &ResolvedCompat,
) -> Vec<Value> {
    let transformed = transform_messages(&ctx.messages, model);
    let mut params: Vec<Value> = Vec::new();

    if let Some(system) = &ctx.system_prompt {
        let role = if model.reasoning && compat.supports_developer_role {
            "developer"
        } else {
            "system"
        };
        params.push(json!({ "role": role, "content": sanitize_surrogates(system) }));
    }

    let mut last_role: Option<&'static str> = None;
    let mut i = 0;
    while let Some(msg) = transformed.get(i) {
        // Bridge a synthetic assistant message between tool results and a following user message.
        if compat.requires_assistant_after_tool_result
            && last_role == Some("toolResult")
            && matches!(msg, Message::User { .. })
        {
            params.push(json!({
                "role": "assistant",
                "content": "I have processed the tool results.",
            }));
        }

        match msg {
            Message::User { content, .. } => {
                if content.is_empty() {
                    i += 1;
                    continue;
                }
                let uc = user_content(content, model.supports_image_input());
                if matches!(&uc, Value::Array(a) if a.is_empty()) {
                    i += 1;
                    continue;
                }
                params.push(json!({ "role": "user", "content": uc }));
                last_role = Some("user");
            }
            Message::Assistant(am) => match build_assistant(am, model, compat) {
                Some(value) => {
                    params.push(value);
                    last_role = Some("assistant");
                }
                None => {
                    i += 1;
                    continue;
                }
            },
            Message::ToolResult { .. } => {
                let mut image_blocks: Vec<Value> = Vec::new();
                let mut j = i;
                while let Some(Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    ..
                }) = transformed.get(j)
                {
                    let text_result = content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let has_images = content.iter().any(|c| matches!(c, Content::Image { .. }));
                    let has_text = !text_result.is_empty();

                    let mut tr = Map::new();
                    tr.insert("role".to_string(), json!("tool"));
                    tr.insert(
                        "content".to_string(),
                        json!(sanitize_surrogates(if has_text {
                            &text_result
                        } else {
                            "(see attached image)"
                        })),
                    );
                    tr.insert("tool_call_id".to_string(), json!(tool_call_id.as_str()));
                    if compat.requires_tool_result_name && !tool_name.is_empty() {
                        tr.insert("name".to_string(), json!(tool_name));
                    }
                    params.push(Value::Object(tr));

                    if has_images && model.supports_image_input() {
                        for c in content {
                            if let Content::Image { data, mime_type } = c {
                                image_blocks.push(json!({
                                    "type": "image_url",
                                    "image_url": { "url": format!("data:{mime_type};base64,{data}") },
                                }));
                            }
                        }
                    }
                    j += 1;
                }
                i = j;

                if image_blocks.is_empty() {
                    last_role = Some("toolResult");
                } else {
                    if compat.requires_assistant_after_tool_result {
                        params.push(json!({
                            "role": "assistant",
                            "content": "I have processed the tool results.",
                        }));
                    }
                    let mut arr = vec![
                        json!({ "type": "text", "text": "Attached image(s) from tool result:" }),
                    ];
                    arr.extend(image_blocks);
                    params.push(json!({ "role": "user", "content": Value::Array(arr) }));
                    last_role = Some("user");
                }
                continue;
            }
        }
        i += 1;
    }

    params
}

/// Build an assistant chat message (Pi `convertMessages` assistant branch, L913-1013); `None` when
/// it has neither content nor tool calls.
fn build_assistant(am: &AssistantMessage, model: &Model, compat: &ResolvedCompat) -> Option<Value> {
    let mut obj = Map::new();
    obj.insert("role".to_string(), json!("assistant"));

    // Default content: "" when an assistant message is required after tool results, else null.
    let mut content_val: Value = if compat.requires_assistant_after_tool_result {
        json!("")
    } else {
        Value::Null
    };

    let text_parts: Vec<String> = am
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } if !text.trim().is_empty() => {
                Some(sanitize_surrogates(text))
            }
            _ => None,
        })
        .collect();
    let assistant_text: String = text_parts.concat();

    let thinking_blocks: Vec<(&String, &Option<String>)> = am
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Thinking {
                thinking,
                thinking_signature,
                ..
            } if !thinking.trim().is_empty() => Some((thinking, thinking_signature)),
            _ => None,
        })
        .collect();

    if let Some(first) = thinking_blocks.first() {
        let first_thinking_sig = first.1;
        if compat.requires_thinking_as_text {
            let thinking_text = thinking_blocks
                .iter()
                .map(|(t, _)| sanitize_surrogates(t))
                .collect::<Vec<_>>()
                .join("\n\n");
            let mut arr = vec![json!({ "type": "text", "text": thinking_text })];
            for tp in &text_parts {
                arr.push(json!({ "type": "text", "text": tp }));
            }
            content_val = Value::Array(arr);
        } else {
            if !assistant_text.is_empty() {
                content_val = json!(assistant_text);
            }
            // Replay reasoning under the original field name (llama.cpp server + gpt-oss).
            let mut signature = first_thinking_sig.clone();
            if model.provider.as_str() == "opencode-go" && signature.as_deref() == Some("reasoning")
            {
                signature = Some("reasoning_content".to_string());
            }
            if let Some(sig) = signature
                && !sig.is_empty()
            {
                let joined = thinking_blocks
                    .iter()
                    .map(|(t, _)| (*t).clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                obj.insert(sig, json!(joined));
            }
        }
    } else if !assistant_text.is_empty() {
        content_val = json!(assistant_text);
    }

    let tool_calls: Vec<&ToolCall> = am
        .content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .collect();
    let has_tool_calls = !tool_calls.is_empty();
    if has_tool_calls {
        let mut tc_values: Vec<Value> = Vec::new();
        let mut reasoning_details: Vec<Value> = Vec::new();
        for tc in &tool_calls {
            tc_values.push(json!({
                "id": tc.id.as_str(),
                "type": "function",
                "function": {
                    "name": tc.name,
                    "arguments": serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".to_string()),
                },
            }));
            if let Some(sig) = &tc.thought_signature
                && let Ok(parsed) = serde_json::from_str::<Value>(sig)
                && !parsed.is_null()
            {
                reasoning_details.push(parsed);
            }
        }
        obj.insert("tool_calls".to_string(), Value::Array(tc_values));
        if !reasoning_details.is_empty() {
            obj.insert(
                "reasoning_details".to_string(),
                Value::Array(reasoning_details),
            );
        }
    }

    if compat.requires_reasoning_content_on_assistant_messages
        && model.reasoning
        && !obj.contains_key("reasoning_content")
    {
        obj.insert("reasoning_content".to_string(), json!(""));
    }

    let has_content = match &content_val {
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => true,
    };
    if !has_content && !has_tool_calls {
        return None;
    }

    obj.insert("content".to_string(), content_val);
    Some(Value::Object(obj))
}

/// User content: a plain string when text-only, else an array of `text`/`image_url` parts.
fn user_content(content: &[Content], supports_image: bool) -> Value {
    let only_text = content.iter().all(|c| matches!(c, Content::Text { .. }));
    if only_text {
        return Value::String(sanitize_surrogates(&join_text(content)));
    }

    let mut parts: Vec<Value> = Vec::new();
    for block in content {
        match block {
            Content::Text { text, .. } => {
                parts.push(json!({ "type": "text", "text": sanitize_surrogates(text) }))
            }
            Content::Image { data, mime_type } if supports_image => parts.push(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{mime_type};base64,{data}") },
            })),
            _ => {}
        }
    }
    Value::Array(parts)
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

// ---------------------------------------------------------------------------
// Response decoding
// ---------------------------------------------------------------------------

/// One in-progress content block, in first-appearance order (its index == `content_index`).
enum Block {
    Text(String),
    Thinking {
        text: String,
        signature: Option<String>,
    },
    Tool {
        id: String,
        name: String,
        args: String,
        thought_signature: Option<String>,
    },
}

/// Streaming-decode state.
#[derive(Default)]
struct Decoder {
    blocks: Vec<Block>,
    text_idx: Option<usize>,
    thinking_idx: Option<usize>,
    tool_by_stream: HashMap<i64, usize>,
    tool_by_id: HashMap<String, usize>,
    /// Encrypted reasoning details whose tool call hasn't been seen yet (Pi
    /// `pendingReasoningDetailsByToolCallId`).
    pending_reasoning_by_tool_id: HashMap<String, String>,
    usage: Option<Usage>,
    response_id: Option<String>,
    response_model: Option<String>,
    stop_reason: Option<StopReason>,
    error_message: Option<String>,
    /// Whether any chunk carried a `finish_reason` (Pi `hasFinishReason`). A stream that ends
    /// without one is a protocol error (Pi openai-completions.ts:452-454).
    saw_finish_reason: bool,
}

impl Decoder {
    /// Build the live `partial` snapshot (Pi `output`, the mutated AssistantMessage attached to
    /// every non-terminal event, openai-completions.ts:158-175 + `partial: output`). Mirrors
    /// [`build_final_message`] but borrows (the stream is still in progress, so `stop_reason`
    /// defaults to `Stop` until a `finish_reason` arrives — Pi inits `output.stopReason = "stop"`).
    fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone().unwrap_or_default();
        apply_cost(&model.cost, &mut usage);
        AssistantMessage {
            content: blocks_to_content(&self.blocks),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: self.response_model.clone(),
            response_id: self.response_id.clone(),
            diagnostics: None,
            usage,
            stop_reason: self.stop_reason.unwrap_or(StopReason::Stop),
            error_message: self.error_message.clone(),
            timestamp: now_millis(),
        }
    }
}

/// Convert in-progress decoder blocks to content (shared by the live `partial` snapshot and the
/// terminal message). Tool args are parsed best-effort (`{}` for incomplete/invalid JSON).
fn blocks_to_content(blocks: &[Block]) -> Vec<Content> {
    blocks
        .iter()
        .map(|block| match block {
            Block::Text(text) => Content::text(text.clone()),
            Block::Thinking { text, signature } => Content::Thinking {
                thinking: text.clone(),
                thinking_signature: signature.clone(),
                redacted: false,
            },
            Block::Tool {
                id,
                name,
                args,
                thought_signature,
            } => Content::ToolCall(ToolCall {
                id: ToolCallId::from(id.as_str()),
                name: name.clone(),
                arguments: parse_partial_json(args),
                thought_signature: thought_signature.clone(),
            }),
        })
        .collect()
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
                // transport/decode/abort mid-stream → terminal Error (R-01-018/044/045)
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
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
        if !process_chunk(&chunk, &mut dec, model, api, sink).await {
            return; // consumer dropped
        }
    }

    // Finalize each open block in appearance order. The `partial` snapshot reflects all assembled
    // blocks (Pi `finishBlock` pushes `*_end` with `partial: output`, openai-completions.ts:214-246).
    let block_count = dec.blocks.len();
    for idx in 0..block_count {
        let partial = dec.snapshot(model, api);
        let ev = match dec.blocks.get(idx) {
            Some(Block::Text(text)) => StreamEvent::TextEnd {
                content_index: idx,
                content: text.clone(),
                partial,
            },
            Some(Block::Thinking { text, .. }) => StreamEvent::ThinkingEnd {
                content_index: idx,
                content: text.clone(),
                partial,
            },
            Some(Block::Tool {
                id,
                name,
                args,
                thought_signature,
            }) => StreamEvent::ToolCallEnd {
                content_index: idx,
                tool_call: ToolCall {
                    id: ToolCallId::from(id.as_str()),
                    name: name.clone(),
                    arguments: parse_partial_json(args),
                    thought_signature: thought_signature.clone(),
                },
                partial,
            },
            None => continue,
        };
        if !sink.send(ev).await {
            return;
        }
    }

    let saw_finish_reason = dec.saw_finish_reason;
    let mut message = build_final_message(dec, model, api);

    // Stream-end guard (Pi openai-completions.ts:452-454): if the stream ended ([DONE] or EOF)
    // without ever receiving a `finish_reason`, and we are not already in an error/aborted terminal,
    // surface a protocol error instead of a (defaulted) success.
    if !saw_finish_reason
        && message.stop_reason != StopReason::Error
        && message.stop_reason != StopReason::Aborted
    {
        message.stop_reason = StopReason::Error;
        message.error_message = Some("Stream ended without finish_reason".to_string());
    }

    // `StreamEvent::terminal` narrows `stop_reason` into the `done`/`error` reason (error/aborted →
    // error terminal, everything else → done) exactly as before, but with Pi's narrowed reason types.
    sink.send(StreamEvent::terminal(message)).await;
}

/// Process one decoded chunk. Returns `false` if the consumer dropped the stream.
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

    // Provider error chunk (e.g. OpenRouter streams `{"error": {...}}` instead of throwing). Pi
    // surfaces this as the OpenAI SDK throwing; the catch block sets `errorMessage` and, when the
    // error carries `error.metadata.raw`, appends it (openai-completions.ts:466-469).
    if let Some(err) = chunk.get("error").filter(|e| !e.is_null()) {
        let mut message = err
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "Provider returned an error".to_string());
        if let Some(raw) = err.get("metadata").and_then(|m| m.get("raw")) {
            let raw_str = raw
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| raw.to_string());
            if !raw_str.is_empty() {
                message.push('\n');
                message.push_str(&raw_str);
            }
        }
        dec.stop_reason = Some(StopReason::Error);
        dec.error_message = Some(message);
        return true;
    }

    let choice = match chunk
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
    {
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
        dec.saw_finish_reason = true;
    }

    let delta = match choice.get("delta") {
        Some(d) if d.is_object() => d,
        _ => return true,
    };

    // 1. Text content.
    if let Some(text) = delta.get("content").and_then(Value::as_str)
        && !text.is_empty()
    {
        let idx = match ensure_text_block(dec, model, api, sink).await {
            Some(idx) => idx,
            None => return false,
        };
        if let Some(Block::Text(buf)) = dec.blocks.get_mut(idx) {
            buf.push_str(text);
        }
        let partial = dec.snapshot(model, api);
        if !sink
            .send(StreamEvent::TextDelta {
                content_index: idx,
                delta: text.to_string(),
                partial,
            })
            .await
        {
            return false;
        }
    }

    // 2. Reasoning / thinking content (first non-empty reasoning field).
    if let Some((field, reason_text)) = first_reasoning_delta(delta)
        && !reason_text.is_empty()
    {
        // The thinking signature records which field carried the reasoning, so a same-model replay
        // can echo it back under the same key (Pi `thinkingSignature` logic).
        let signature = if model.provider.as_str() == "opencode-go" && field == "reasoning" {
            "reasoning_content"
        } else {
            field
        };
        let idx = match ensure_thinking_block(dec, signature, model, api, sink).await {
            Some(idx) => idx,
            None => return false,
        };
        if let Some(Block::Thinking { text, .. }) = dec.blocks.get_mut(idx) {
            text.push_str(reason_text);
        }
        let partial = dec.snapshot(model, api);
        if !sink
            .send(StreamEvent::ThinkingDelta {
                content_index: idx,
                delta: reason_text.to_string(),
                partial,
            })
            .await
        {
            return false;
        }
    }

    // 3. Streamed tool calls.
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tool_calls {
            if !process_tool_call_delta(tc, dec, model, api, sink).await {
                return false;
            }
        }
    }

    // 4. Encrypted reasoning details — attach as the thought signature of the matching tool call,
    // or stash until that tool call appears (Pi `reasoning_details` handling, L422-435).
    if let Some(details) = delta.get("reasoning_details").and_then(Value::as_array) {
        for detail in details {
            if let Some(id) = encrypted_reasoning_detail_id(detail) {
                let serialized = detail.to_string();
                if let Some(&idx) = dec.tool_by_id.get(id) {
                    if let Some(Block::Tool {
                        thought_signature, ..
                    }) = dec.blocks.get_mut(idx)
                    {
                        *thought_signature = Some(serialized);
                    }
                } else {
                    dec.pending_reasoning_by_tool_id
                        .insert(id.to_string(), serialized);
                }
            }
        }
    }

    true
}

/// The id of a `reasoning.encrypted` detail (Pi `isEncryptedReasoningDetail`): requires
/// `type == "reasoning.encrypted"` plus non-empty `id` and `data` strings.
fn encrypted_reasoning_detail_id(detail: &Value) -> Option<&str> {
    if detail.get("type").and_then(Value::as_str) != Some("reasoning.encrypted") {
        return None;
    }
    let id = detail
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let _data = detail
        .get("data")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    Some(id)
}

/// Ensure a text block exists, emitting `TextStart` on first appearance. Returns its index, or
/// `None` if the consumer dropped the stream.
async fn ensure_text_block(
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> Option<usize> {
    if let Some(idx) = dec.text_idx {
        return Some(idx);
    }
    let idx = dec.blocks.len();
    dec.blocks.push(Block::Text(String::new()));
    dec.text_idx = Some(idx);
    let partial = dec.snapshot(model, api);
    if !sink
        .send(StreamEvent::TextStart {
            content_index: idx,
            partial,
        })
        .await
    {
        return None;
    }
    Some(idx)
}

/// Ensure a thinking block exists, emitting `ThinkingStart` on first appearance. The `signature`
/// (the reasoning field name) is recorded on first creation only (matching Pi).
async fn ensure_thinking_block(
    dec: &mut Decoder,
    signature: &str,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> Option<usize> {
    if let Some(idx) = dec.thinking_idx {
        return Some(idx);
    }
    let idx = dec.blocks.len();
    dec.blocks.push(Block::Thinking {
        text: String::new(),
        signature: Some(signature.to_string()),
    });
    dec.thinking_idx = Some(idx);
    let partial = dec.snapshot(model, api);
    if !sink
        .send(StreamEvent::ThinkingStart {
            content_index: idx,
            partial,
        })
        .await
    {
        return None;
    }
    Some(idx)
}

/// Apply one `tool_calls[]` delta fragment, assembling id/name/arguments across chunks.
async fn process_tool_call_delta(
    tc: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let stream_index = tc.get("index").and_then(Value::as_i64);
    let id = tc
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
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
                thought_signature: None,
            });
            if let Some(si) = stream_index {
                dec.tool_by_stream.insert(si, idx);
            }
            if let Some(i) = id {
                dec.tool_by_id.insert(i.to_string(), idx);
            }
            let partial = dec.snapshot(model, api);
            if !sink
                .send(StreamEvent::ToolCallStart {
                    content_index: idx,
                    partial,
                })
                .await
            {
                return false;
            }
            idx
        }
    };

    // Attach any reasoning detail that arrived before this tool call (Pi
    // `applyPendingReasoningDetail`).
    let pending = id.and_then(|i| dec.pending_reasoning_by_tool_id.remove(i));

    if let Some(Block::Tool {
        id: bid,
        name: bname,
        args,
        thought_signature,
    }) = dec.blocks.get_mut(idx)
    {
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
        if let Some(sig) = pending {
            *thought_signature = Some(sig);
        }
    }
    // Maintain the id index if the id only arrived now.
    if let Some(i) = id {
        dec.tool_by_id.entry(i.to_string()).or_insert(idx);
    }

    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::ToolCallDelta {
        content_index: idx,
        delta: args_fragment.to_string(),
        partial,
    })
    .await
}

/// First non-empty reasoning delta across the known field names, returned as `(field, value)`.
fn first_reasoning_delta(delta: &Value) -> Option<(&'static str, &str)> {
    for field in REASONING_FIELDS {
        if let Some(s) = delta.get(field).and_then(Value::as_str)
            && !s.is_empty()
        {
            return Some((field, s));
        }
    }
    None
}

/// Build the terminal [`AssistantMessage`] from accumulated decoder state.
fn build_final_message(dec: Decoder, model: &Model, api: &ApiId) -> AssistantMessage {
    let content = blocks_to_content(&dec.blocks);

    let mut usage = dec.usage.unwrap_or_default();
    apply_cost(&model.cost, &mut usage);

    let stop_reason = dec.stop_reason.unwrap_or(StopReason::Stop);

    AssistantMessage {
        content,
        provider: model.provider.clone(),
        model: model.id.as_str().to_string(),
        api: api.clone(),
        response_model: dec.response_model,
        response_id: dec.response_id,
        diagnostics: None,
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

    let input = prompt
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);
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
        other => (
            StopReason::Error,
            Some(format!("Provider finish_reason: {other}")),
        ),
    }
}

/// Best-effort parse of accumulated tool-call argument JSON. An empty/incomplete buffer yields an
/// empty object so a truncated stream still produces a valid (if empty) tool call.
/// Parse a (possibly partial / streaming) tool-call argument string into the JSON object Pi's
/// `ToolCall.arguments: Record<string, any>` requires (types.ts:348). Incomplete, invalid, or
/// non-object input yields an empty object `{}` rather than a scalar, so the decoder always produces
/// a well-typed object.
fn parse_partial_json(s: &str) -> Map<String, Value> {
    // Best-effort recovery of truncated/streamed tool-call args (Pi `parseStreamingJson`,
    // utils/json-parse.ts): a strict parse first, then repair, then a tolerant partial parse that
    // preserves a truncated string/number/array instead of discarding the whole object (#28).
    crate::utils::json_parse::parse_streaming_json_object(Some(s))
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
            base_url: "https://api.together.ai/v1".to_string(),
            reasoning: true,
            input: vec![Modality::Text],
            cost: ModelCost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.5,
                cache_write: 0.0,
                tiers: None,
            },
            context_window: 131072,
            max_tokens: 131072,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn auth_with_key() -> AuthResult {
        AuthResult {
            auth: ModelAuth {
                api_key: Some("sk-xyz".into()),
                ..Default::default()
            },
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
        let compat = get_compat(&model());
        let headers = build_headers(
            &model(),
            &auth_with_key(),
            &StreamOptions::default(),
            &compat,
            None,
        );
        assert_eq!(
            headers.get("Authorization"),
            Some(&Some("Bearer sk-xyz".to_string()))
        );
        // No session-affinity headers: the flag is false for `together` and there is no session id.
        assert!(!headers.contains_key("session_id"));
    }

    /// `model.headers` merge precedence (Pi `createClient`, openai-completions.ts:505-524):
    /// auth overlay < `model.headers` < `opts.headers`, and a `None` value suppresses a default.
    #[test]
    fn model_headers_merge_precedence_and_suppression() {
        let compat = get_compat(&model());
        let mut auth = auth_with_key();
        auth.auth.headers = Some(crate::HeaderMap::from([
            ("X-All".to_string(), Some("auth".to_string())),
            ("X-AM".to_string(), Some("auth".to_string())),
            ("X-Auth".to_string(), Some("a".to_string())),
            ("X-Drop".to_string(), Some("keep".to_string())),
        ]));
        let mut m = model();
        m.headers = Some(crate::HeaderMap::from([
            ("X-All".to_string(), Some("model".to_string())),
            ("X-AM".to_string(), Some("model".to_string())),
            ("X-Model".to_string(), Some("m".to_string())),
            ("X-Drop".to_string(), None), // suppress the auth default
        ]));
        let opts = StreamOptions {
            headers: Some(crate::HeaderMap::from([(
                "X-All".to_string(),
                Some("opts".to_string()),
            )])),
            ..Default::default()
        };
        let headers = build_headers(&m, &auth, &opts, &compat, None);
        // opts wins the key present at all three layers.
        assert_eq!(headers.get("X-All"), Some(&Some("opts".to_string())));
        // model overrides auth on a key present at both (and not in opts).
        assert_eq!(headers.get("X-AM"), Some(&Some("model".to_string())));
        // Layer-exclusive keys survive.
        assert_eq!(headers.get("X-Auth"), Some(&Some("a".to_string())));
        assert_eq!(headers.get("X-Model"), Some(&Some("m".to_string())));
        // `None` from model.headers suppresses the auth default (carried as `Some(None)`).
        assert_eq!(headers.get("X-Drop"), Some(&None));
        // Bearer auth still present.
        assert_eq!(
            headers.get("Authorization"),
            Some(&Some("Bearer sk-xyz".to_string()))
        );
    }

    /// Build a `{assistant with N tool calls} + {N tool results}` context for the given raw ids.
    fn ctx_with_tool_call_ids(ids: &[&str]) -> Context {
        let calls: Vec<Content> = ids
            .iter()
            .map(|id| {
                Content::ToolCall(ToolCall {
                    id: ToolCallId::from(*id),
                    name: "read".into(),
                    arguments: Map::new(),
                    thought_signature: None,
                })
            })
            .collect();
        let mut messages = vec![Message::Assistant(AssistantMessage {
            content: calls,
            // A DIFFERENT provider/api produced these — the cross-provider replay case.
            provider: "openai".into(),
            model: "gpt-5.4".into(),
            api: "openai-responses".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
        })];
        for id in ids {
            messages.push(Message::ToolResult {
                tool_call_id: ToolCallId::from(*id),
                tool_name: "read".into(),
                content: vec![Content::text("ok")],
                is_error: false,
                details: None,
                timestamp: 0,
            });
        }
        Context {
            system_prompt: None,
            messages,
            tools: vec![],
        }
    }

    /// DRIFT-002: Responses-API ids are `{call_id}|{item_id}`. Two calls in the same turn can share
    /// a `call_id`; keeping only that half collapses them into one id, and Chat Completions rejects
    /// the replayed request with a 400 for duplicate `tool_call_id`s (and the tool results then
    /// point at the wrong call). Pi keeps both halves.
    #[test]
    fn shared_call_id_with_distinct_item_ids_stays_distinct_on_replay() {
        let ctx = ctx_with_tool_call_ids(&["call_abc|item_1", "call_abc|item_2"]);
        let body = build_body(&model(), &ctx, &StreamOptions::default());
        let messages = body["messages"].as_array().unwrap();

        let tcs = messages[0]["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 2);
        let first = tcs[0]["id"].as_str().unwrap();
        let second = tcs[1]["id"].as_str().unwrap();
        assert_ne!(
            first, second,
            "both tool calls replayed as `{first}` — a duplicate tool_call_id is a provider 400"
        );
        assert_eq!(first, "call_abc_item_1");
        assert_eq!(second, "call_abc_item_2");

        // The tool results must follow their own call, not both bind to the first.
        assert_eq!(messages[1]["tool_call_id"], "call_abc_item_1");
        assert_eq!(messages[2]["tool_call_id"], "call_abc_item_2");
    }

    /// The over-40-char fallback: `{call-prefix}_{8-char shortHash of the WHOLE id}`, so two ids
    /// sharing a call_id still differ.
    #[test]
    fn overlong_pipe_ids_fall_back_to_a_hash_suffix() {
        let long_item = "a".repeat(400);
        let id_a = format!("call_abc|{long_item}1");
        let id_b = format!("call_abc|{long_item}2");
        let ctx = ctx_with_tool_call_ids(&[id_a.as_str(), id_b.as_str()]);
        let body = build_body(&model(), &ctx, &StreamOptions::default());
        let tcs = body["messages"][0]["tool_calls"].as_array().unwrap();
        let a = tcs[0]["id"].as_str().unwrap();
        let b = tcs[1]["id"].as_str().unwrap();
        assert_ne!(a, b, "hashed ids collided: {a}");
        assert!(a.len() <= 40, "id too long for OpenAI: {a} ({})", a.len());
        assert!(b.len() <= 40, "id too long for OpenAI: {b} ({})", b.len());
        assert_eq!(a, format!("call_abc_{}", &short_hash(&id_a)[..8]));
        assert_eq!(b, format!("call_abc_{}", &short_hash(&id_b)[..8]));
    }

    /// Unit coverage for the branches the wire test cannot reach cheaply.
    #[test]
    fn normalize_tool_call_id_matches_pi() {
        let m = model();
        // No pipe, non-openai provider: untouched.
        assert_eq!(normalize_tool_call_id(&m, "call_plain"), "call_plain");
        // Empty item id keeps just the sanitized call id (Pi's `itemId.length > 0` guard).
        assert_eq!(normalize_tool_call_id(&m, "call_abc|"), "call_abc");
        // Disallowed characters in either half become `_`.
        assert_eq!(
            normalize_tool_call_id(&m, "call+a/b=|item=1"),
            "call_a_b__item_1"
        );
        // Exactly 40 chars is kept whole (`<= 40`).
        let exact = format!("call_{}|{}", "a".repeat(17), "b".repeat(17));
        assert_eq!(normalize_tool_call_id(&m, &exact).len(), 40);
        // A single-char call id still keeps at least one char of prefix (`Math.max(1, …)`).
        let tiny = format!("c|{}", "d".repeat(60));
        let out = normalize_tool_call_id(&m, &tiny);
        assert_eq!(out, format!("c_{}", &short_hash(&tiny)[..8]));
    }

    #[test]
    fn request_body_matches_openai_shape() {
        let ctx = Context {
            system_prompt: Some("be terse".to_string()),
            messages: vec![
                Message::User {
                    content: vec![Content::text("hi")],
                    timestamp: 0,
                },
                Message::Assistant(AssistantMessage {
                    content: vec![Content::ToolCall(ToolCall {
                        id: ToolCallId::from("call_1"),
                        name: "get_weather".into(),
                        arguments: json!({ "city": "Paris" })
                            .as_object()
                            .cloned()
                            .expect("object"),
                        thought_signature: None,
                    })],
                    provider: "together".into(),
                    model: "m".into(),
                    api: "openai-completions".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
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
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };

        let body = build_body(&model(), &ctx, &opts);

        assert_eq!(body["model"], "openai/gpt-oss-120b");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        // Together uses `max_tokens` (not `max_completion_tokens`) and omits `store`
        // (supportsStore=false), unlike standard OpenAI which sends `store: false`.
        assert_eq!(body["max_tokens"], 256);
        assert!(body.get("max_completion_tokens").is_none());
        assert!(body.get("store").is_none());
        assert_eq!(body["temperature"], 0.5);
        // Together encodes reasoning as `reasoning: { enabled }` and NEVER `reasoning_effort`.
        assert_eq!(body["reasoning"], json!({ "enabled": true }));
        assert!(body.get("reasoning_effort").is_none());

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(
            messages[0],
            json!({ "role": "system", "content": "be terse" })
        );
        assert_eq!(messages[1], json!({ "role": "user", "content": "hi" }));
        // assistant tool call — content is `null` (Pi sends null unless an assistant message is
        // required after tool results; Together does not require that).
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["content"], Value::Null);
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
        // Together does not support `strict` on tools.
        assert!(tools[0]["function"].get("strict").is_none());
        // No caller `tool_choice` => the field is omitted (Pi never auto-injects "auto").
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn reasoning_effort_omitted_for_non_reasoning_model() {
        let mut m = model();
        m.reasoning = false;
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_body(&m, &Context::default(), &opts);
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("reasoning").is_none());
    }

    fn openai_model() -> Model {
        let mut m = model();
        m.id = "gpt-5".into();
        m.provider = "openai".into();
        m.base_url = "https://api.openai.com/v1".to_string();
        m
    }

    #[test]
    fn openai_uses_max_completion_tokens_store_and_reasoning_effort() {
        let m = openai_model();
        let opts = StreamOptions {
            max_tokens: Some(100),
            reasoning: ModelThinkingLevel::Medium,
            ..Default::default()
        };
        let body = build_body(&m, &Context::default(), &opts);
        // OpenAI uses `max_completion_tokens`, `store: false`, and `reasoning_effort`.
        assert_eq!(body["max_completion_tokens"], 100);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["store"], false);
        assert_eq!(body["reasoning_effort"], "medium");
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn openai_reasoning_effort_uses_thinking_level_map() {
        let mut m = openai_model();
        // Map "high" -> "xhigh" wire value (Pi `thinkingLevelMap`).
        m.thinking_level_map = Some(crate::model::ThinkingLevelMap::from([(
            "high".to_string(),
            Some("xhigh".to_string()),
        )]));
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_body(&m, &Context::default(), &opts);
        assert_eq!(body["reasoning_effort"], "xhigh");
    }

    /// PROV-002: `max` is a first-class `reasoning_effort` value. Pi passes the level string
    /// verbatim (`reasoningEffort = clampedReasoning`, openai-completions.ts:621) and its option
    /// union lists `"max"` (:143).
    #[test]
    fn openai_reasoning_effort_encodes_max() {
        let m = openai_model();
        let body = build_body(
            &m,
            &Context::default(),
            &StreamOptions {
                reasoning: ModelThinkingLevel::Max,
                ..Default::default()
            },
        );
        assert_eq!(body["reasoning_effort"], "max");
    }

    /// The real corrected catalog: `deepseek-v4-pro` maps `max -> "max"` (pi deepseek.models.ts
    /// @91585d9a) and must send it, proving the DRIFT-008 catalog values reach the wire.
    #[test]
    fn deepseek_catalog_sends_max_effort() {
        use crate::collection::get_supported_thinking_levels;
        let m = crate::providers::fleet::DEEPSEEK
            .models()
            .iter()
            .find(|m| m.id.as_str() == "deepseek-v4-pro")
            .expect("deepseek-v4-pro")
            .clone();
        assert!(get_supported_thinking_levels(&m).contains(&ModelThinkingLevel::Max));
        let body = build_body(
            &m,
            &Context::default(),
            &StreamOptions {
                reasoning: ModelThinkingLevel::Max,
                max_tokens: Some(64),
                ..Default::default()
            },
        );
        assert_eq!(body["reasoning_effort"], "max", "body={body}");
    }

    #[test]
    fn tool_choice_emitted_only_when_caller_sets_it() {
        let ctx = Context {
            tools: vec![ToolDef {
                name: "t".into(),
                description: "d".into(),
                parameters: json!({}),
            }],
            ..Default::default()
        };
        // No caller tool_choice => omitted even with tools present.
        let body = build_body(&model(), &ctx, &StreamOptions::default());
        assert!(body.get("tool_choice").is_none());

        // Mode constraint => bare string.
        let opts = StreamOptions {
            tool_choice: Some(crate::stream::ToolChoice::Required),
            ..Default::default()
        };
        assert_eq!(build_body(&model(), &ctx, &opts)["tool_choice"], "required");

        // Named-function constraint => object.
        let opts = StreamOptions {
            tool_choice: Some(crate::stream::ToolChoice::Function { name: "t".into() }),
            ..Default::default()
        };
        assert_eq!(
            build_body(&model(), &ctx, &opts)["tool_choice"],
            json!({ "type": "function", "function": { "name": "t" } })
        );
    }

    #[test]
    fn strict_tools_only_when_supported() {
        let ctx = Context {
            tools: vec![ToolDef {
                name: "t".into(),
                description: "d".into(),
                parameters: json!({ "type": "object" }),
            }],
            ..Default::default()
        };
        // Together => no strict.
        let body = build_body(&model(), &ctx, &StreamOptions::default());
        assert!(body["tools"][0]["function"].get("strict").is_none());
        // OpenAI => strict: false.
        let body = build_body(&openai_model(), &ctx, &StreamOptions::default());
        assert_eq!(body["tools"][0]["function"]["strict"], false);
    }

    #[test]
    fn prompt_cache_key_for_openai_with_session() {
        let m = openai_model();
        let opts = StreamOptions {
            session_id: Some(cyrup_core::SessionId::from("sess-123")),
            ..Default::default()
        };
        let body = build_body(&m, &Context::default(), &opts);
        assert_eq!(body["prompt_cache_key"], "sess-123");
        // Together does not get a prompt_cache_key (no api.openai.com, short retention).
        let body = build_body(&model(), &Context::default(), &opts);
        assert!(body.get("prompt_cache_key").is_none());
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

        assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::TextStart {
                content_index: 0,
                ..
            }
        )));
        let deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["Hel", "lo"]);

        match events.last() {
            Some(StreamEvent::Done { message, .. }) => {
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
    async fn non_terminal_events_carry_running_partial() {
        // Pi parity (R-01-022): every non-terminal event carries a `partial` snapshot that grows
        // as deltas arrive, so consumers never reconstruct from deltas.
        let raw = concat!(
            "data: {\"id\":\"resp-1\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect_events(raw).await;

        // Start's partial is empty; every non-terminal carries a partial (the terminal does not).
        assert!(matches!(&events[0], StreamEvent::Start { partial } if partial.content.is_empty()));
        for ev in &events {
            match ev {
                StreamEvent::Done { .. } | StreamEvent::Error { .. } => {
                    assert!(ev.partial().is_none())
                }
                _ => assert!(
                    ev.partial().is_some(),
                    "non-terminal must carry partial: {ev:?}"
                ),
            }
        }

        // The last text_delta's partial reflects the full accumulated text + the response id.
        let last_delta = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta { partial, .. } => Some(partial),
                _ => None,
            })
            .next_back()
            .expect("a text_delta");
        assert_eq!(last_delta.content, vec![Content::text("Hello")]);
        assert_eq!(last_delta.response_id.as_deref(), Some("resp-1"));
        // Still streaming → partial keeps the default stop reason until the terminal arrives.
        assert_eq!(last_delta.stop_reason, StopReason::Stop);
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

        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolCallStart {
                content_index: 0,
                ..
            }
        )));
        let delta_count = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolCallDelta { .. }))
            .count();
        assert_eq!(delta_count, 2);

        let tc_end = events.iter().find_map(|e| match e {
            StreamEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
            _ => None,
        });
        let tc = tc_end.expect("toolcall_end");
        assert_eq!(tc.id.as_str(), "call_9");
        assert_eq!(tc.name, "add");
        assert_eq!(
            serde_json::Value::Object(tc.arguments),
            json!({ "a": 1, "b": 2 })
        );

        match events.last() {
            Some(StreamEvent::Done { message, .. }) => {
                assert_eq!(message.stop_reason, StopReason::ToolUse);
                assert_eq!(message.content.len(), 1);
            }
            other => panic!("expected Done terminal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn decodes_encrypted_reasoning_details_onto_tool_calls() {
        // `call_a`: detail arrives AFTER the tool call (matched path).
        // `call_b`: detail arrives BEFORE the tool call (pending path).
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.encrypted\",\"id\":\"call_b\",\"data\":\"BBB\"}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"function\":{\"name\":\"f\",\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.encrypted\",\"id\":\"call_a\",\"data\":\"AAA\"}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_b\",\"function\":{\"name\":\"g\",\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect_events(raw).await;
        let sigs: std::collections::HashMap<String, Option<String>> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolCallEnd { tool_call, .. } => Some((
                    tool_call.id.as_str().to_string(),
                    tool_call.thought_signature.clone(),
                )),
                _ => None,
            })
            .collect();
        assert!(
            sigs["call_a"]
                .as_deref()
                .unwrap()
                .contains("reasoning.encrypted")
        );
        assert!(sigs["call_a"].as_deref().unwrap().contains("AAA"));
        assert!(sigs["call_b"].as_deref().unwrap().contains("BBB"));
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

        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ThinkingStart {
                content_index: 0,
                ..
            }
        )));
        let think: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ThinkingDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(think, "think hard");

        match events.last() {
            Some(StreamEvent::Done { message, .. }) => {
                // The thinking block records which reasoning field carried it (`reasoning_content`).
                assert_eq!(
                    message.content,
                    vec![
                        Content::Thinking {
                            thinking: "think hard".to_string(),
                            thinking_signature: Some("reasoning_content".to_string()),
                            redacted: false,
                        },
                        Content::text("answer"),
                    ]
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
            Some(StreamEvent::Done { message, .. }) => {
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
            Some(StreamEvent::Error { error: message, .. }) => {
                assert_eq!(message.stop_reason, StopReason::Error);
                assert!(
                    message
                        .error_message
                        .as_deref()
                        .unwrap()
                        .contains("content_filter")
                );
            }
            other => panic!("expected Error terminal, got {other:?}"),
        }
    }

    // ---- Pi parity gaps ----

    // Gap 1: Pi openai-completions.ts:452-454 — a stream that ends without ever emitting a
    // `finish_reason` is a protocol error, not a defaulted success.
    #[tokio::test]
    async fn stream_without_finish_reason_errors() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect_events(raw).await;
        match events.last() {
            Some(StreamEvent::Error { error: message, .. }) => {
                assert_eq!(message.stop_reason, StopReason::Error);
                assert_eq!(
                    message.error_message.as_deref(),
                    Some("Stream ended without finish_reason")
                );
            }
            other => panic!("expected Error terminal, got {other:?}"),
        }
    }

    // Gap 2: Pi `resolveCacheRetention` (openai-completions.ts:141-149) — when the caller did not
    // set retention, `PI_CACHE_RETENTION == "long"` promotes to Long; an explicit value wins.
    #[test]
    fn pi_cache_retention_env_promotes_long() {
        use std::collections::BTreeMap;
        let m = openai_model();
        let mut env = BTreeMap::new();
        env.insert("PI_CACHE_RETENTION".to_string(), "long".to_string());

        // Unset caller retention + PI_CACHE_RETENTION=long (scoped overlay) => promoted to Long,
        // which (on api.openai.com, supportsLongCacheRetention) emits `prompt_cache_retention`.
        let opts = StreamOptions {
            cache_retention: None,
            ..Default::default()
        };
        let body = build_body_with_env(&m, &Context::default(), &opts, Some(&env));
        assert_eq!(body["prompt_cache_retention"], "24h");

        // Explicit caller value wins over env: Short stays Short (no 24h).
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::Short),
            ..Default::default()
        };
        let body = build_body_with_env(&m, &Context::default(), &opts, Some(&env));
        assert!(body.get("prompt_cache_retention").is_none());

        // resolve_cache_retention precedence, directly (overlay-driven, deterministic).
        assert_eq!(
            resolve_cache_retention(None, Some(&env)),
            CacheRetention::Long
        );
        assert_eq!(
            resolve_cache_retention(Some(CacheRetention::None), Some(&env)),
            CacheRetention::None
        );
        let empty = BTreeMap::new();
        assert_eq!(
            resolve_cache_retention(None, Some(&empty)),
            CacheRetention::Short
        );
    }

    // Gap 3: Pi `createClient` (openai-completions.ts:515-519) — session-affinity headers are
    // injected when the compat flag is set and a (cache-gated) session id is available.
    #[test]
    fn session_affinity_headers_emitted_when_flag_and_session_set() {
        let mut m = model();
        m.compat = Some(crate::api::compat::OpenAiCompletionsCompat {
            send_session_affinity_headers: Some(true),
            ..Default::default()
        });
        let compat = get_compat(&m);
        let headers = build_headers(
            &m,
            &auth_with_key(),
            &StreamOptions::default(),
            &compat,
            Some("sess-7"),
        );
        assert_eq!(headers.get("session_id"), Some(&Some("sess-7".to_string())));
        assert_eq!(
            headers.get("x-client-request-id"),
            Some(&Some("sess-7".to_string()))
        );
        assert_eq!(
            headers.get("x-session-affinity"),
            Some(&Some("sess-7".to_string()))
        );

        // Flag off (default for every provider) => not emitted even with a session id present.
        let compat_off = get_compat(&model());
        let headers = build_headers(
            &model(),
            &auth_with_key(),
            &StreamOptions::default(),
            &compat_off,
            Some("sess-7"),
        );
        assert!(!headers.contains_key("session_id"));

        // Flag on but no session id => not emitted.
        let headers = build_headers(
            &m,
            &auth_with_key(),
            &StreamOptions::default(),
            &compat,
            None,
        );
        assert!(!headers.contains_key("session_id"));
    }

    // Gap 4: Pi error enrichment (openai-completions.ts:466-469) — an OpenRouter-style error chunk
    // with `error.metadata.raw` appends the raw provider detail to the terminal error message.
    #[tokio::test]
    async fn provider_error_chunk_appends_raw_metadata() {
        let raw = concat!(
            "data: {\"error\":{\"message\":\"upstream failed\",\"metadata\":{\"raw\":\"503 Service Unavailable\"}}}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect_events(raw).await;
        match events.last() {
            Some(StreamEvent::Error { error: message, .. }) => {
                assert_eq!(message.stop_reason, StopReason::Error);
                let em = message.error_message.as_deref().unwrap();
                assert!(em.contains("upstream failed"), "got: {em}");
                assert!(em.contains("503 Service Unavailable"), "got: {em}");
            }
            other => panic!("expected Error terminal, got {other:?}"),
        }
    }
}

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
    CacheControlFormat, DeferredToolsMode, MaxTokensField, ResolvedCompat, SessionAffinityFormat,
    ThinkingFormat,
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
use crate::utils::constrained_sampling::{
    ConstrainedSamplingError, resolve_json_schema_strict_sampling,
};
use crate::utils::hash::short_hash;
use crate::utils::provider_retry::ProviderRetry;
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

        // PROV-011: an unsatisfiable `constrainedSampling` fails the turn before any HTTP, with
        // pi's own message — upstream `buildParams` throws into `stream`'s catch.
        let params = match build_body_with_env(model, ctx, opts, auth.env.as_ref()) {
            Ok(p) => p,
            Err(e) => {
                let e = ProviderError::from(e);
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };
        // gap-08 #2: let a `before_provider_request` extension inspect/replace the outbound body.
        let body = crate::stream::apply_on_payload(opts, model, params).await;
        let headers = build_headers(model, ctx, auth, opts, &compat, cache_session_id);
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
    ctx: &Context,
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
    // PROV-028: the per-request Copilot headers, layered on top of `model.headers` exactly where Pi
    // puts `Object.assign(headers, copilotHeaders)` (openai-completions.ts:638-645) — after the
    // `{ ...model.headers }` seed and before session affinity / the opts overlay.
    crate::api::github_copilot_headers::apply_copilot_dynamic_headers(
        &mut headers,
        model.provider.as_str(),
        &ctx.messages,
    );
    // Session-affinity headers (Pi `createClient`, openai-completions.ts:647-656 @v0.83.0). The
    // enabling flag is `false` for every provider in `detect_compat`, but the emission is ported
    // for 1:1 parity so an explicit `model.compat.sendSessionAffinityHeaders` override takes
    // effect. PROV-024: the header SET is chosen by `sessionAffinityFormat`, not fixed — an
    // OpenRouter model reads `x-session-id` and none of the other three.
    if compat.send_session_affinity_headers
        && let Some(sid) = cache_session_id
    {
        if compat.session_affinity_format == SessionAffinityFormat::Openrouter {
            headers.insert("x-session-id".to_string(), Some(sid.to_string()));
        } else {
            if compat.session_affinity_format == SessionAffinityFormat::Openai {
                headers.insert("session_id".to_string(), Some(sid.to_string()));
            }
            headers.insert("x-client-request-id".to_string(), Some(sid.to_string()));
            headers.insert("x-session-affinity".to_string(), Some(sid.to_string()));
        }
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
// Test-only fixture wrapper: the deny-list allowance the crate's `mod tests` blocks carry.
#[allow(clippy::expect_used)]
pub(crate) fn build_body(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    build_body_with_env(model, ctx, opts, None)
        .expect("fixture declares no unsatisfiable constrained sampling")
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

/// Env-aware `build_body`: `env` is the provider-scoped overlay (Pi `options.env`) consulted by
/// [`resolve_cache_retention`] for the `PI_CACHE_RETENTION` fallback.
/// `[CYRUP-DELTA]` — fallible where pi's `buildParams` throws. `convertTools` can throw for a
/// `strict: "require"` tool on a provider without strict mode (`constrained-sampling.ts:91-95`
/// @v0.83.0); upstream that unwinds into `stream`'s catch and becomes the turn's terminal error
/// message. cyrup returns the same message through `Result` and the caller emits the identical
/// terminal event (PROV-011).
pub(crate) fn build_body_with_env(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    env: Option<&ProviderEnv>,
) -> Result<Value, ConstrainedSamplingError> {
    let compat = get_compat(model);
    let cache = resolve_cache_retention(opts.cache_retention, env);
    let mut messages = convert_messages(model, ctx, &compat)?;
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

    // PROV-069 — an output ceiling is ALWAYS sent: the caller's when it supplied one, otherwise the
    // model's own `max_tokens` from the catalog.
    //
    // [CYRUP-DELTA] Upstream sends this key only when the caller supplies one
    // (`ai/src/api/openai-completions.ts:716`, `if (options?.maxTokens)`), and nothing in cyrup's
    // turn path ever does — `GenConfig::max_tokens` has no production writer, so the key was never
    // emitted and the server applied its OWN default ceiling. On Together that default truncates a
    // reply mid-sentence within a few hundred tokens, on every turn, with `finish_reason: length`,
    // while the session sits at ~3% of a 1M context window. The catalog's `max_tokens` — ported for
    // all 1087 rows and covered by tests — reached no request at all.
    //
    // The fallback is upstream's OWN rule, taken from the two APIs where it is explicit rather than
    // invented here: `anthropic-messages.ts:989` sends `options?.maxTokens ?? model.maxTokens`, and
    // `adjustMaxTokensForThinking` (`simple-options.ts:61-64`) documents the same intent in words —
    // "Undefined means no explicit caller cap. Use the model cap and fit thinking inside it."
    // Applying it here makes the three wire paths agree instead of leaving this one uncapped.
    //
    // A caller-supplied value still wins, so `maxTokens` in settings / `modelOverrides` keeps its
    // precedence. When neither exists (`max_tokens == 0`, the modelless fallback), nothing is sent
    // and upstream's behaviour is unchanged.
    let ceiling = opts.max_tokens.or(if model.max_tokens > 0 {
        Some(model.max_tokens)
    } else {
        None
    });
    if let Some(max) = ceiling {
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
    // PROV-025 — `const deferredToolNames = compat.deferredToolsMode === "kimi" ?
    // getDeferredToolNames(context.messages) : new Set(); const activeTools =
    // context.tools?.filter((tool) => !deferredToolNames.has(tool.name));`
    // (`openai-completions.ts:719-721` @v0.83.0). A tool introduced mid-transcript is emitted ONCE
    // inline by `convert_messages` and must NOT be repeated in the top-level array — that
    // repetition is exactly the prompt-prefix churn the mode exists to avoid. Note the emptiness
    // test below is on the FILTERED list, so a transcript whose every tool is deferred falls
    // through to the `has_tool_history` arm, as upstream's `activeTools.length > 0` does.
    let active_tools: Vec<ToolDef> = if compat.deferred_tools_mode == Some(DeferredToolsMode::Kimi)
    {
        let deferred = deferred_tool_names(&ctx.messages);
        ctx.tools
            .iter()
            .filter(|t| !deferred.iter().any(|n| n == &t.name))
            .cloned()
            .collect()
    } else {
        ctx.tools.clone()
    };
    if !active_tools.is_empty() {
        tools = Some(convert_tools(&active_tools, &compat)?);
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
        // PROV-066: the typed `OpenRouterRouting` serializes back to the same JSON object the
        // `Value` form carried, so the wire payload is unchanged; `to_value` on a plain struct of
        // primitives cannot fail, and if it somehow did, omitting the key is the safe direction
        // (OpenRouter routes by its own defaults) rather than sending a partial object.
        if let Some(routing) = &c.open_router_routing
            && let Ok(value) = serde_json::to_value(routing)
        {
            obj.insert("provider".to_string(), value);
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

    // Last so custom keys override the named request fields (Pi's own comment,
    // `openai-completions.ts:884-887` @v0.84.1: `if (options?.samplingParams)
    // Object.assign(params, options.samplingParams)`). AGENT-026. The merge with
    // `Model.sampling_params` already happened in `build_base_options`
    // (`simple-options.ts:27-33`), so what arrives here is the resolved map — and being LAST is the
    // whole point: an operator's `top_p` must beat the named `temperature`/`max_tokens` block above.
    apply_sampling_params(&mut obj, opts);

    Ok(Value::Object(obj))
}

/// `Object.assign(params, options.samplingParams)` — the identical three-line tail of all three
/// OpenAI-compatible `buildParams` (`openai-completions.ts:884-887`, `openai-responses.ts:330-333`,
/// `azure-openai-responses.ts:324-327` @v0.84.1). Shared here rather than triplicated so the three
/// cannot drift apart; the absent-map case is a no-op exactly as pi's `if` guard is. AGENT-026.
pub(crate) fn apply_sampling_params(obj: &mut Map<String, Value>, opts: &StreamOptions) {
    let Some(params) = &opts.sampling_params else {
        return;
    };
    for (k, v) in params {
        obj.insert(k.clone(), v.clone());
    }
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

/// Pi `addCacheControlToLastConversationMessage` — `openai-completions.ts:913-925` @**v0.83.0**
/// (byte-identical at v0.84.1, `:964-976`), with pi's `addCacheControlToMessage` (`:946-954`
/// @v0.83.0) inlined because its role test is the same three-way test.
///
/// DRIFT-028: the `"tool"` arm was dropped in the port, so in an agent loop — where the last
/// message is almost always a tool result — the cache breakpoint landed one message too early on
/// every turn. Filed `upstream-drift`; it is **not-ported**: `git show
/// v0.83.0:packages/ai/src/api/openai-completions.ts` already has `message.role === "tool"` at
/// `:918` and `:947`, inside the ported baseline, so no rebase would have swept it up.
fn add_cache_control_to_last_conversation_message(messages: &mut [Value], cc: &Value) {
    for msg in messages.iter_mut().rev() {
        let role = msg.get("role").and_then(Value::as_str);
        if (role == Some("user") || role == Some("assistant") || role == Some("tool"))
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

/// Map cyrup [`ToolDef`]s to OpenAI `tools` entries — Pi `convertTools`,
/// `openai-completions.ts:1286-1320` @**v0.83.0**.
///
/// `strict` is emitted only when the provider supports it (some reject unknown fields), and its
/// value is `resolveJsonSchemaStrictSampling(tool, …) ?? false` — so a tool that opted into
/// JSON-schema constrained sampling gets `strict: true` and every other tool keeps `false`
/// (PROV-011). A `strict: "require"` tool on a provider without strict mode fails the request with
/// pi's exact message.
/// Every tool name introduced mid-transcript by a `toolResult`'s `addedToolNames` — Pi
/// `getDeferredToolNames`, `openai-completions.ts:91-101` @v0.83.0.
///
/// PROV-025. Insertion-ordered for the same reason [`tools_by_name`] is: upstream's `Set` walks in
/// insertion order and that order reaches the wire. This is a DIFFERENT accessor from
/// [`crate::utils::deferred_tools`]'s placement map — it works off message names only, with no
/// notion of WHERE the tool became available.
fn deferred_tool_names(messages: &[cyrup_core::Message]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for message in messages {
        if let cyrup_core::Message::ToolResult { added_tool_names, .. } = message {
            for name in added_tool_names {
                if !names.iter().any(|n| n == name) {
                    names.push(name.clone());
                }
            }
        }
    }
    names
}

/// `names.map(n => toolsByName.get(n)).filter(Boolean)` — Pi `getToolsByName`,
/// `openai-completions.ts:103-110` @v0.83.0. Walks `names` (not `tools`), so the emitted order is
/// the order the tools were introduced, and a name with no matching tool is dropped.
fn tools_by_name(tools: &[ToolDef], names: &[String]) -> Vec<ToolDef> {
    names
        .iter()
        .filter_map(|name| tools.iter().find(|t| &t.name == name).cloned())
        .collect()
}

pub(crate) fn convert_tools(
    tools: &[ToolDef],
    compat: &ResolvedCompat,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    tools
        .iter()
        .map(|t| {
            let strict = resolve_json_schema_strict_sampling(t, compat.supports_strict_mode)?;
            let mut function = Map::new();
            function.insert("name".to_string(), json!(t.name));
            function.insert("description".to_string(), json!(t.description));
            function.insert("parameters".to_string(), t.parameters.clone());
            if compat.supports_strict_mode {
                function.insert("strict".to_string(), json!(strict.unwrap_or(false)));
            }
            Ok(json!({ "type": "function", "function": Value::Object(function) }))
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
                usage,
                added_tool_names,
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
                // This transform only swaps image blocks for a placeholder; every other field must
                // survive it. `added_tool_names` in particular is the deferred-tool anchor, and a
                // request-path transform that silently dropped it would move the tool definition
                // back to the prefix and wipe the prompt cache.
                usage: usage.clone(),
                added_tool_names: added_tool_names.clone(),
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
                usage: None,
                added_tool_names: Vec::new(),
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
                usage,
                added_tool_names,
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
                        // Only the tool-call id is being rewritten; carry the rest through
                        // untouched (see `downgrade_unsupported_images`).
                        usage: usage.clone(),
                        added_tool_names: added_tool_names.clone(),
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
) -> Result<Vec<Value>, ConstrainedSamplingError> {
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
                // PROV-025 — pi's per-RUN `const deferredToolNames = new Set<string>()`
                // (`openai-completions.ts:1194` @v0.83.0), declared inside the tool-result branch
                // so each run emits its OWN inline tool block. A `Vec` rather than a set because
                // upstream's `Array.from(names)` walks a JS `Set` in INSERTION order and
                // `getToolsByName` preserves it (`:104-110`); a `HashSet` would randomize the
                // emitted tool order and a `BTreeSet` would sort it, and neither is what the wire
                // sees upstream.
                let mut deferred_tool_names: Vec<String> = Vec::new();
                let mut j = i;
                while let Some(Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    added_tool_names,
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

                    // `if (compat.deferredToolsMode === "kimi") { for (const name of
                    // toolMsg.addedToolNames ?? []) deferredToolNames.add(name); }`
                    // (`openai-completions.ts:1221-1226` @v0.83.0) — immediately after the tool
                    // message is pushed, before the image handling.
                    if compat.deferred_tools_mode == Some(DeferredToolsMode::Kimi) {
                        for name in added_tool_names {
                            if !deferred_tool_names.iter().any(|n| n == name) {
                                deferred_tool_names.push(name.clone());
                            }
                        }
                    }

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

                // `if (deferredToolNames.size > 0) { … params.push(kimiToolMessage) }`
                // (`openai-completions.ts:1266-1276` @v0.83.0), positioned exactly here: AFTER the
                // image/`lastRole` handling and immediately before the `continue`. Kimi accepts a
                // system message carrying `tools` and omitting the standard `content` field, so
                // the object has exactly the two keys upstream emits.
                if !deferred_tool_names.is_empty() {
                    let deferred_tools = tools_by_name(&ctx.tools, &deferred_tool_names);
                    if !deferred_tools.is_empty() {
                        params.push(json!({
                            "role": "system",
                            "tools": convert_tools(&deferred_tools, compat)?,
                        }));
                    }
                }
                continue;
            }
        }
        i += 1;
    }

    Ok(params)
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
    /// The choice's own `finish_reason`, kept verbatim beside the narrowed [`StopReason`] (pi
    /// `output.rawStopReason = choice.finish_reason`,
    /// `v0.84.1 ai/src/api/openai-completions.ts:463`). PORT BUG, not version lag: the write is
    /// present at v0.83.0 too (`v0.83.0 ai/src/api/openai-completions.ts:459`) and cyrup never
    /// ported it. This is the widest-reach one of the five — `openai-completions` is the fleet wire
    /// api shared by 16 built-in providers (`providers/fleet.rs`), so it carried the gap for xAI,
    /// Groq, DeepSeek, Moonshot and the rest.
    raw_stop_reason: Option<String>,
    error_message: Option<String>,
    /// Whether any chunk carried a `finish_reason` (Pi `hasFinishReason`). A stream that ends
    /// without one is a protocol error (Pi openai-completions.ts:452-454).
    saw_finish_reason: bool,
}

impl Decoder {
    /// Build the live `partial` snapshot (Pi `output`, the mutated AssistantMessage attached to
    /// every non-terminal event, openai-completions.ts:158-175 + `partial: output`). Mirrors
    /// [`build_final_message`] but borrows: the stream is still in progress, so `stop_reason` is
    /// the in-flight sentinel until a `finish_reason` arrives — Pi seeds
    /// `output.stopReason = "pending"` (openai-completions.ts:218) and attaches that same `output`
    /// as every non-terminal event's `partial`.
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
            stop_reason: self.stop_reason.unwrap_or(StopReason::Pending),
            deferred: None,
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
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
    let settled = dec.stop_reason;
    let message = build_final_message(dec, model, api);

    // Which stop reason the provider actually DELIVERED — `None` is Pi's still-`"pending"` output.
    // Pi's end-of-stream ladder (v0.84.1 `ai/src/api/openai-completions.ts:571-586`):
    //   1. `aborted`/`error` already settled by an abort or an error chunk → throw with THAT
    //      message, so the reason and the recorded `error_message` are used verbatim;
    //   2. a `finish_reason` actually arrived → use it;
    //   3. `!hasFinishReason && !compat.supportsFinishReason` (`:578-580`) → the provider never
    //      reports one, so INFER: `toolUse` when the turn produced a tool call, else `stop`;
    //   4. otherwise `(supportsFinishReason && !hasFinishReason) || stopReason === "pending"`
    //      (`:584-586`) → throw "Stream ended without finish_reason".
    //
    // VERSION LAG (v0.83.0 → v0.84.1): at v0.83.0 (`openai-completions.ts:577`) step 4 was the
    // unconditional `if (!hasFinishReason || output.stopReason === "pending")` and there was no
    // `supportsFinishReason` compat key at all (absent from v0.83.0 `ai/src/types.ts`), so a
    // provider that never sends `finish_reason` always produced the truncated-stream error.
    //
    // Step 3 cannot mask a settled `error`: pi only assigns `stopReason = "error"` from
    // `mapStopReason` (`:465`), which also sets `hasFinishReason = true` (`:469`), so the inference
    // branch is unreachable whenever the reason is `error`.
    let delivered = match settled {
        Some(r @ (StopReason::Error | StopReason::Aborted)) => Some(r),
        other if saw_finish_reason => other,
        _ if !get_compat(model).supports_finish_reason => Some(
            if message
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall(_)))
            {
                StopReason::ToolUse
            } else {
                StopReason::Stop
            },
        ),
        _ => None,
    };

    sink.send(StreamEvent::end_of_stream(
        message,
        delivered,
        "Stream ended without finish_reason",
    ))
    .await;
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
        // pi records the raw reason first (`v0.84.1 ai/src/api/openai-completions.ts:463`), before
        // the narrowing map, so `content_filter` and every provider-specific reason survive on the
        // turn instead of collapsing into `StopReason::Error`.
        dec.raw_stop_reason = Some(reason.to_string());
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

    // Pi's `output.stopReason` when no `finish_reason` ever arrived: still the `"pending"` seed
    // (openai-completions.ts:218). The sole caller hands this straight to
    // `StreamEvent::end_of_stream`, which rewrites it — but seeding `Stop` here would have made a
    // truncated message look complete to anyone who called this helper directly.
    let stop_reason = dec.stop_reason.unwrap_or(StopReason::Pending);

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
        deferred: None,
        error_message: dec.error_message,
        raw_stop_reason: dec.raw_stop_reason,
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
    use crate::api::compat::ModelCompat;
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
            sampling_params: None,
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
            &Context::default(),
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
        let headers = build_headers(&m, &Context::default(), &auth, &opts, &compat, None);
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
    /// PROV-028 — `buildCopilotDynamicHeaders` on the chat-completions route
    /// (openai-completions.ts:638-645). Copilot's Fable/Kimi rows ride this api.
    #[test]
    fn copilot_dynamic_headers_on_the_completions_route() {
        let mut m = model();
        m.provider = "github-copilot".into();
        let compat = get_compat(&m);

        let ctx = Context {
            system_prompt: None,
            messages: vec![cyrup_core::Message::User {
                content: vec![cyrup_core::Content::Image {
                    data: "aGk=".to_string(),
                    mime_type: "image/png".to_string(),
                }],
                timestamp: 0,
            }],
            tools: vec![],
        };
        let headers = build_headers(
            &m,
            &ctx,
            &auth_with_key(),
            &StreamOptions::default(),
            &compat,
            None,
        );
        assert_eq!(headers.get("X-Initiator"), Some(&Some("user".to_string())));
        assert_eq!(
            headers.get("Openai-Intent"),
            Some(&Some("conversation-edits".to_string()))
        );
        assert_eq!(
            headers.get("Copilot-Vision-Request"),
            Some(&Some("true".to_string())),
            "an image turn requires the vision header or Copilot rejects the request"
        );

        // Non-Copilot providers get none of them.
        let plain = build_headers(
            &model(),
            &ctx,
            &auth_with_key(),
            &StreamOptions::default(),
            &get_compat(&model()),
            None,
        );
        assert!(!plain.contains_key("X-Initiator"));
        assert!(!plain.contains_key("Copilot-Vision-Request"));
    }

    /// PROV-025 — `deferredToolsMode: "kimi"`.
    ///
    /// BEFORE: `rg 'deferred_tools_mode|DeferredToolsMode' crates/` returned nothing, so a Kimi
    /// model received the FULL tool schema set on every single turn and the prompt-prefix cache
    /// churned on exactly the provider family upstream added the mode for.
    ///
    /// Pi's mechanism, reproduced here clause for clause: the deferred tool is dropped from
    /// `params.tools` (`openai-completions.ts:719-721` @v0.83.0) and emitted ONCE as a
    /// `{role: "system", tools: [...]}` message directly after the tool-result run that introduced
    /// it (`:1266-1276`).
    #[test]
    fn kimi_deferred_tools_move_from_the_tools_array_into_an_inline_system_message() {
        fn tool(name: &str) -> ToolDef {
            ToolDef {
                name: name.into(),
                description: format!("the {name} tool"),
                parameters: json!({ "type": "object", "properties": {} }),
                constrained_sampling: None,
            }
        }
        fn transcript() -> Vec<Message> {
            vec![
                Message::User { content: vec![Content::text("go")], timestamp: 0 },
                Message::Assistant(AssistantMessage {
                    content: vec![Content::ToolCall(ToolCall {
                        id: ToolCallId::from("c1"),
                        name: "early".into(),
                        arguments: Map::new(),
                        thought_signature: None,
                    })],
                    provider: "moonshotai".into(),
                    model: "kimi-k2".into(),
                    api: API_ID.into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::ToolUse,
                    deferred: None,
                    error_message: None,
                    raw_stop_reason: None,
                    timestamp: 0,
                }),
                Message::ToolResult {
                    tool_call_id: ToolCallId::from("c1"),
                    tool_name: "early".into(),
                    content: vec![Content::text("ok")],
                    is_error: false,
                    details: None,
                    timestamp: 0,
                    usage: None,
                    // The anchor: this result introduced `late`.
                    added_tool_names: vec!["late".to_string()],
                },
            ]
        }
        let ctx = Context {
            system_prompt: None,
            messages: transcript(),
            tools: vec![tool("early"), tool("late")],
        };

        // ---- WITHOUT the flag: today's behaviour, and the control that keeps the assertions
        // below from passing vacuously.
        let plain = build_body(&model(), &ctx, &StreamOptions::default());
        let plain_tools = plain.get("tools").and_then(Value::as_array).expect("tools array");
        assert_eq!(
            plain_tools.len(),
            2,
            "without deferredToolsMode both tools stay in the top-level array"
        );
        assert!(
            !plain
                .get("messages")
                .and_then(Value::as_array)
                .expect("messages")
                .iter()
                .any(|m| m.get("tools").is_some()),
            "without the flag no message carries an inline `tools` key"
        );

        // ---- WITH `compat: {"deferredToolsMode": "kimi"}`.
        let mut kimi_model = model();
        kimi_model.compat = Some(ModelCompat {
            deferred_tools_mode: Some(DeferredToolsMode::Kimi),
            ..Default::default()
        });
        let body = build_body(&kimi_model, &ctx, &StreamOptions::default());

        // `late` is gone from the top-level array; `early` (never deferred) stays.
        let tools = body.get("tools").and_then(Value::as_array).expect("tools array");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, vec!["early"], "a deferred tool must not be repeated in `tools`");

        // …and appears exactly once, inline, in a `{role: "system", tools: [...]}` message with no
        // `content` key, positioned after the tool-result run that introduced it.
        let messages = body.get("messages").and_then(Value::as_array).expect("messages");
        let inline_at = messages
            .iter()
            .position(|m| m.get("tools").is_some())
            .expect("an inline kimi tool message is emitted");
        let inline = &messages[inline_at];
        assert_eq!(inline.get("role").and_then(Value::as_str), Some("system"));
        assert!(
            inline.get("content").is_none(),
            "Kimi's tool system message omits the standard `content` field"
        );
        let inline_names: Vec<&str> = inline
            .get("tools")
            .and_then(Value::as_array)
            .expect("inline tools")
            .iter()
            .filter_map(|t| t.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(inline_names, vec!["late"]);
        assert_eq!(
            messages[inline_at - 1].get("role").and_then(Value::as_str),
            Some("tool"),
            "the inline block follows the tool-result run that introduced the tool"
        );
        assert_eq!(
            messages.iter().filter(|m| m.get("tools").is_some()).count(),
            1,
            "the schema is emitted ONCE — repeating it is the churn the mode exists to avoid"
        );
    }

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
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
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
                usage: None,
                added_tool_names: Vec::new(),
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

    /// PROV-069 — the production path: NO caller cap, so the MODEL's `max_tokens` must reach the
    /// wire. Reported from live use — every reply truncated mid-sentence with `finish_reason:
    /// length` at ~3% of a 1M context window.
    ///
    /// RED before the fix, and this is the test the suite was missing rather than getting wrong:
    /// `GenConfig::max_tokens` has no production writer (`grep -rn '\.max_tokens(' crates/ | grep -v
    /// tests` is empty), so `opts.max_tokens` is always `None` in the product, the key was never
    /// emitted, and the server applied its own small default. Every OTHER wire test here supplies
    /// `max_tokens: Some(...)` by hand — which proves serialisation and hides the one path that
    /// actually ships.
    #[test]
    fn with_no_caller_cap_the_models_own_max_tokens_reaches_the_body() {
        let ctx = Context { system_prompt: None, messages: vec![], tools: vec![] };

        // Exactly what the turn path passes today: nothing.
        let body = build_body(&model(), &ctx, &StreamOptions::default());
        assert_eq!(
            body["max_tokens"], 131_072,
            "the catalog's max_tokens must reach the request, not sit decorative: {body}"
        );

        // A caller cap still wins, so a `maxTokens` setting / modelOverrides keeps precedence.
        let capped =
            build_body(&model(), &ctx, &StreamOptions { max_tokens: Some(256), ..Default::default() });
        assert_eq!(capped["max_tokens"], 256, "an explicit caller cap beats the model's");

        // Modelless fallback (`max_tokens: 0`) sends nothing, leaving upstream behaviour unchanged.
        let mut modelless = model();
        modelless.max_tokens = 0;
        let none = build_body(&modelless, &ctx, &StreamOptions::default());
        assert!(
            none.get("max_tokens").is_none(),
            "a zero model ceiling means unknown — send no key: {none}"
        );
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
                    deferred: None,
                    error_message: None,
                    raw_stop_reason: None,
                    timestamp: 0,
                }),
                Message::ToolResult {
                    tool_call_id: ToolCallId::from("call_1"),
                    tool_name: "get_weather".into(),
                    content: vec![Content::text("sunny")],
                    is_error: false,
                    details: None,
                    timestamp: 0,
                    usage: None,
                    added_tool_names: Vec::new(),
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
                constrained_sampling: None,
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
                constrained_sampling: None,
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
                constrained_sampling: None,
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

    /// PORT BUG (present at v0.83.0, never ported): pi writes
    /// `output.rawStopReason = choice.finish_reason`
    /// (`v0.84.1 ai/src/api/openai-completions.ts:463`; `v0.83.0 …:459`). This is the widest-reach
    /// of the five missing writers — `openai-completions` is the fleet wire api behind 16 built-in
    /// providers, whose finish reasons are the least standardized in the workspace.
    #[tokio::test]
    async fn a_finish_reason_is_recorded_raw_beside_the_narrowed_one() {
        let events =
            collect_events("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"content_filter\"}]}\n\ndata: [DONE]\n\n")
                .await;
        let Some(StreamEvent::Error { error, .. }) = events.last() else {
            panic!("expected an error terminal, got {:?}", events.last());
        };
        assert_eq!(error.raw_stop_reason.as_deref(), Some("content_filter"));

        // MIRROR 1: a clean `stop` keeps its raw word on the `done` terminal.
        let events = collect_events(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        )
        .await;
        let Some(StreamEvent::Done { message, .. }) = events.last() else {
            panic!("expected a done terminal, got {:?}", events.last());
        };
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("stop"));

        // MIRROR 2: no `finish_reason` ever arrives → nothing recorded, and the terminal is the
        // truncation error, not a fabricated raw value.
        let events =
            collect_events("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n")
                .await;
        let Some(StreamEvent::Error { error, .. }) = events.last() else {
            panic!("expected an error terminal, got {:?}", events.last());
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("Stream ended without finish_reason")
        );
        assert_eq!(error.raw_stop_reason, None);
    }

    async fn collect_events_with(m: Model, raw: &'static str) -> Vec<StreamEvent> {
        let (sink, mut rx) = channel(256);
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

    /// VERSION LAG (v0.83.0 → v0.84.1): the new `compat.supportsFinishReason` key
    /// (v0.84.1 `ai/src/types.ts:547-548`, detected default `true` at
    /// `ai/src/api/openai-completions.ts:1499`) makes a stream that ends with no `finish_reason`
    /// INFER its stop reason instead of erroring:
    /// `output.stopReason = output.content.some(b => b.type === "toolCall") ? "toolUse" : "stop"`
    /// (v0.84.1 `ai/src/api/openai-completions.ts:578-580`). At v0.83.0 (`…:577`) the guard was the
    /// unconditional `if (!hasFinishReason || output.stopReason === "pending") throw`.
    #[tokio::test]
    async fn absent_finish_reason_is_inferred_when_the_provider_reports_none() {
        let quiet = || {
            let mut m = model();
            m.compat = Some(ModelCompat {
                supports_finish_reason: Some(false),
                ..ModelCompat::default()
            });
            m
        };

        // No tool call in the turn → `"stop"`.
        let events = collect_events_with(
            quiet(),
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
        )
        .await;
        let Some(StreamEvent::Done { message, .. }) = events.last() else {
            panic!("expected a done terminal, got {:?}", events.last());
        };
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.error_message, None);

        // A tool call in the turn → `"toolUse"`.
        let events = collect_events_with(
            quiet(),
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_9\",\"function\":{\"name\":\"add\",\"arguments\":\"{}\"}}]}}]}\n\ndata: [DONE]\n\n",
        )
        .await;
        let Some(StreamEvent::Done { message, .. }) = events.last() else {
            panic!("expected a done terminal, got {:?}", events.last());
        };
        assert_eq!(message.stop_reason, StopReason::ToolUse);

        // MIRROR 1: the inference is a FALLBACK — a delivered `finish_reason` still wins, even for
        // a provider flagged `supportsFinishReason: false` (pi's guard is `!hasFinishReason && …`).
        let events = collect_events_with(
            quiet(),
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n",
        )
        .await;
        let Some(StreamEvent::Done { message, .. }) = events.last() else {
            panic!("expected a done terminal, got {:?}", events.last());
        };
        assert_eq!(message.stop_reason, StopReason::Length);

        // MIRROR 2: at the DEFAULT `supportsFinishReason: true`, a missing reason is still the
        // truncated-stream error — the fix must not turn every truncation into a clean `stop`
        // (v0.84.1 `…:584-586`).
        let events = collect_events(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
        )
        .await;
        let Some(StreamEvent::Error { error, .. }) = events.last() else {
            panic!("expected an error terminal, got {:?}", events.last());
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("Stream ended without finish_reason")
        );

        // MIRROR 3: `detectCompat` leaves the flag `true` for every provider (v0.84.1 `…:1499`),
        // so an unconfigured model never infers.
        assert!(get_compat(&model()).supports_finish_reason);
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
        // PROV-010 / AGENT-014 / DRIFT-012, wire half — CLOSED. Pi's in-flight `partial` carries
        // `stopReason: "pending"` (openai-completions.ts:218), and cyrup now does too. This
        // assertion previously pinned `Stop` and said so in its own comment ("pins the CURRENT wire
        // value; it is NOT a statement that `stop` is meaningful mid-stream") — i.e. it pinned the
        // defect. It is REWRITTEN, not removed: a mid-stream partial that claims a completed turn
        // is the bug, so the correct value is the one Pi emits.
        //
        // The containment invariant it used to gesture at still holds and is enforced elsewhere:
        // `Pending` can never reach a TERMINAL event, because `decode_stream` routes end-of-stream
        // through `StreamEvent::end_of_stream` and `StreamEvent::terminal` normalizes a surviving
        // `Pending` to `Error` (see `api::truncation_parity` and
        // `stream_end_without_finish_reason_is_an_error` below).
        assert_eq!(last_delta.stop_reason, StopReason::Pending);
        assert_eq!(
            serde_json::to_value(last_delta).unwrap()["stopReason"],
            "pending",
            "wire spelling must be Pi's"
        );
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
        let body = build_body_with_env(&m, &Context::default(), &opts, Some(&env)).unwrap();
        assert_eq!(body["prompt_cache_retention"], "24h");

        // Explicit caller value wins over env: Short stays Short (no 24h).
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::Short),
            ..Default::default()
        };
        let body = build_body_with_env(&m, &Context::default(), &opts, Some(&env)).unwrap();
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

    // PROV-011 — `openai-completions.ts:1309`/`:1317` @v0.83.0: `strict` is
    // `resolveJsonSchemaStrictSampling(tool, compat.supportsStrictMode) ?? false`, emitted only
    // when the provider supports strict mode.
    #[test]
    fn constrained_sampling_drives_completions_strict_flag() {
        use crate::context::{ConstrainedSampling, ConstrainedSamplingConfig, StrictSampling};
        use crate::utils::constrained_sampling::ConstrainedSamplingError;

        let tool = |strict| ToolDef {
            name: "calc".into(),
            description: "calculate".into(),
            parameters: json!({"type": "object", "properties": {}, "required": []}),
            constrained_sampling: Some(ConstrainedSampling::Config(
                ConstrainedSamplingConfig::JsonSchema { strict },
            )),
        };

        // openai detects `supportsStrictMode: true` ⇒ `strict: true` for a constrained tool…
        let mut ctx = Context::default();
        ctx.tools = vec![tool(StrictSampling::Prefer)];
        let body = build_body(&openai_model(), &ctx, &StreamOptions::default());
        assert_eq!(body["tools"][0]["function"]["strict"], json!(true));

        // …and `false` for an unconstrained one, which is the pre-existing behaviour.
        ctx.tools = vec![ToolDef {
            name: "calc".into(),
            description: "calculate".into(),
            parameters: json!({"type": "object", "properties": {}, "required": []}),
            constrained_sampling: None,
        }];
        let body = build_body(&openai_model(), &ctx, &StreamOptions::default());
        assert_eq!(body["tools"][0]["function"]["strict"], json!(false));

        // `model()` is a `together` model, which detects `supportsStrictMode: false`: no `strict`
        // key at all, and a `require` tool fails the turn with pi's message.
        ctx.tools = vec![tool(StrictSampling::Prefer)];
        let body = build_body(&model(), &ctx, &StreamOptions::default());
        assert!(body["tools"][0]["function"].get("strict").is_none());

        ctx.tools = vec![tool(StrictSampling::Require)];
        assert_eq!(
            build_body_with_env(&model(), &ctx, &StreamOptions::default(), None),
            Err(ConstrainedSamplingError(
                "Tool \"calc\" requires JSON-schema constrained sampling, but strict tools are unsupported."
                    .to_string()
            ))
        );
    }

    // DRIFT-028 — pi `addCacheControlToLastConversationMessage` (openai-completions.ts:913-925
    // @v0.83.0) walks backwards accepting `user`, `assistant` AND `tool`. cyrup dropped the `tool`
    // arm, so in an agent loop (where the last message is almost always a tool result) the
    // breakpoint landed one message too early on every turn.
    #[test]
    fn cache_breakpoint_lands_on_a_trailing_tool_result() {
        let cc = json!({"type": "ephemeral"});

        // Conversation ending in a tool result: the breakpoint is on THAT message.
        let mut messages = vec![
            json!({"role": "system", "content": "sys"}),
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "content": "calling"}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "tool output"}),
        ];
        add_cache_control_to_last_conversation_message(&mut messages, &cc);
        assert_eq!(
            messages[3]["content"][0]["cache_control"], cc,
            "the trailing tool result must carry the breakpoint"
        );
        assert!(
            messages[2].get("content").and_then(Value::as_array).is_none(),
            "the assistant message must be left as a plain string — untouched"
        );

        // Conversation ending in an assistant message is unchanged by the widening.
        let mut messages = vec![
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "content": "answer"}),
        ];
        add_cache_control_to_last_conversation_message(&mut messages, &cc);
        assert_eq!(messages[1]["content"][0]["cache_control"], cc);
        assert_eq!(messages[0]["content"], json!("hi"));

        // A `system`/`developer` message is still never a conversation breakpoint: an empty
        // trailing tool result makes `addCacheControlToTextContent` return false and the walk
        // continues past it to the user turn, skipping the system prompt entirely.
        let mut messages = vec![
            json!({"role": "system", "content": "sys"}),
            json!({"role": "user", "content": "hi"}),
            json!({"role": "tool", "tool_call_id": "c1", "content": ""}),
        ];
        add_cache_control_to_last_conversation_message(&mut messages, &cc);
        assert_eq!(messages[1]["content"][0]["cache_control"], cc);
        assert_eq!(messages[2]["content"], json!(""));
        assert_eq!(messages[0]["content"], json!("sys"));
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
            &Context::default(),
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
        assert!(
            headers.get("x-session-id").is_none(),
            "the openai form never sends OpenRouter's header"
        );

        // Flag off (default for every provider) => not emitted even with a session id present.
        let compat_off = get_compat(&model());
        let headers = build_headers(
            &model(),
            &Context::default(),
            &auth_with_key(),
            &StreamOptions::default(),
            &compat_off,
            Some("sess-7"),
        );
        assert!(!headers.contains_key("session_id"));

        // Flag on but no session id => not emitted.
        let headers = build_headers(
            &m,
            &Context::default(),
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

    /// PROV-024. `sessionAffinityFormat` selects the header SET
    /// (openai-completions.ts:647-656 @v0.83.0); the detector is `isOpenRouter ? "openrouter" :
    /// "openai"` (`:1473`) and the catalog override resolves at `:1515`. Red before the fix:
    /// cyrup emitted the fixed OpenAI triple with no provider branch, so an OpenRouter completions
    /// model got three headers it does not read and never got the one it does.
    #[test]
    fn session_affinity_format_selects_the_completions_header_set() {
        let headers_for = |m: &Model| {
            let compat = get_compat(m);
            build_headers(
                m,
                &Context::default(),
                &auth_with_key(),
                &StreamOptions::default(),
                &compat,
                Some("sess-7"),
            )
        };

        // OpenRouter, detected from the provider id: `x-session-id` ONLY.
        let mut router = model();
        router.provider = "openrouter".into();
        router.compat = Some(crate::api::compat::OpenAiCompletionsCompat {
            send_session_affinity_headers: Some(true),
            ..Default::default()
        });
        let h = headers_for(&router);
        assert_eq!(h.get("x-session-id"), Some(&Some("sess-7".to_string())));
        assert!(h.get("session_id").is_none());
        assert!(h.get("x-client-request-id").is_none());
        assert!(h.get("x-session-affinity").is_none());

        // "openai-nosession": the pair WITHOUT `session_id` — pi's documented migration target for
        // the `sendSessionIdHeader: false` flag it deleted (packages/ai/CHANGELOG.md:168).
        let mut nos = model();
        nos.compat = Some(crate::api::compat::OpenAiCompletionsCompat {
            send_session_affinity_headers: Some(true),
            session_affinity_format: Some(crate::api::compat::SessionAffinityFormat::OpenaiNosession),
            ..Default::default()
        });
        let h = headers_for(&nos);
        assert!(h.get("session_id").is_none());
        assert_eq!(
            h.get("x-client-request-id"),
            Some(&Some("sess-7".to_string()))
        );
        assert_eq!(
            h.get("x-session-affinity"),
            Some(&Some("sess-7".to_string()))
        );
        assert!(h.get("x-session-id").is_none());
    }
}

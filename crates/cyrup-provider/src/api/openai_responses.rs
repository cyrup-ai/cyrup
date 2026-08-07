//! The `openai-responses` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking the OpenAI Responses streaming API (`POST {baseUrl}/responses`, SSE
//! events `response.created` / `response.output_item.{added,done}` /
//! `response.{output_text,reasoning_text,reasoning_summary_text,refusal,function_call_arguments}.delta`
//! / `response.completed` / …). Shared by the `openai` provider (and, with their own variants,
//! azure / cloudflare-ai-gateway / github-copilot). Pure JSON-over-SSE — no SDK, no new dependency.
//! 1:1 port of Pi's `api/openai-responses.ts` + `api/openai-responses-shared.ts` (reasoning items,
//! encrypted-content include, prompt-cache key/retention, structured text signatures, foreign
//! tool-call-id rewriting, and the full streaming decoder).
//!
//! Wire JSON uses OpenAI's own field names (snake_case), NOT the cyrup camelCase convention.

use crate::HeaderMap;
use crate::api::compat::{
    clamp_openai_prompt_cache_key, get_responses_compat, mapped_effort_or, off_is_not_null,
    off_value_or, sanitize_surrogates, thinking_level_key,
};
use crate::api::openai_completions::transform_messages_with_source;
use crate::api::{ApiImpl, EventSink};
use crate::auth::{AuthResult, ProviderEnv};
use crate::collection::clamp_thinking_level;
use crate::context::{Context, ToolDef};
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::{SseFrame, SseRequest, build_client_for_target, open_sse};
use crate::stream::{CacheRetention, StreamEvent, StreamOptions};
use crate::usage::apply_cost;
use crate::utils::deferred_tools::split_deferred_tools;
use crate::utils::hash::short_hash;
use crate::utils::json_parse::parse_streaming_json_object;
use crate::utils::provider_retry::ProviderRetry;
use cyrup_core::{
    ApiId, AssistantMessage, CancelToken, Content, Message, ModelThinkingLevel, StopReason,
    TextPhase, TextSignatureV1, ToolCall, ToolCallId, Usage,
};
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::OPENAI_RESPONSES;

/// Providers whose tool-call ids carry the `call_id|item_id` Responses shape (Pi
/// `OPENAI_TOOL_CALL_PROVIDERS`, openai-responses.ts:26).
const OPENAI_TOOL_CALL_PROVIDERS: &[&str] = &["openai", "openai-codex", "opencode"];

/// Reasoning-summary verbosity (Pi `reasoningSummary`, openai-responses.ts:80:
/// `"auto" | "detailed" | "concise" | null`). `Null` reproduces Pi's explicit `null`, which — like
/// an absent value — falls back to `"auto"` (`options?.reasoningSummary || "auto"`, line 257).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummary {
    Auto,
    Detailed,
    Concise,
    Null,
}

impl ReasoningSummary {
    /// The wire string for the `reasoning.summary` field, or `None` for `null`.
    fn as_wire(self) -> Option<&'static str> {
        match self {
            ReasoningSummary::Auto => Some("auto"),
            ReasoningSummary::Detailed => Some("detailed"),
            ReasoningSummary::Concise => Some("concise"),
            ReasoningSummary::Null => None,
        }
    }
}

/// Per-API typed options for the `openai-responses` wire protocol (Pi `OpenAIResponsesOptions`,
/// openai-responses.ts:78-82). `reasoningEffort` already maps onto `StreamOptions.reasoning`; the
/// remaining fields live here, carried via
/// [`StreamOptions::api_options`](crate::StreamOptions::api_options). Defaults reproduce Pi exactly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenAiResponsesOptions {
    /// Reasoning-summary verbosity (Pi `reasoningSummary`, openai-responses.ts:80). `None` = Pi
    /// default (`"auto"` when a reasoning request is built).
    pub reasoning_summary: Option<ReasoningSummary>,
    /// Service tier (Pi `serviceTier`, openai-responses.ts:81). `None` = omit the field (Pi's
    /// default — `params.service_tier` is set only when `serviceTier` is defined, line 242).
    pub service_tier: Option<String>,
}

/// The wire `reasoning.summary` value for an optional [`OpenAiResponsesOptions`] (Pi
/// `options?.reasoningSummary || "auto"`, openai-responses.ts:257): a concrete non-null value wins;
/// `None`/`Null` fall back to `"auto"`.
fn reasoning_summary_or_auto(opts: Option<&OpenAiResponsesOptions>) -> &'static str {
    opts.and_then(|o| o.reasoning_summary)
        .and_then(ReasoningSummary::as_wire)
        .unwrap_or("auto")
}

/// The `ApiImpl` for `"openai-responses"`.
pub struct OpenAiResponsesApi {
    api: ApiId,
}

impl Default for OpenAiResponsesApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl OpenAiResponsesApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(OpenAiResponsesApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for OpenAiResponsesApi {
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

        // getClientApiKey (openai-responses.ts:37-41): an explicit key wins; otherwise an
        // authorization / cf-aig-authorization header lets the key be the literal "unused".
        let api_key = match resolve_api_key(auth, opts) {
            Some(k) => k,
            None => {
                let e = ProviderError::Transport(
                    format!("No API key for provider: {}", model.provider).into(),
                );
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        // gap-08 #2: `before_provider_request` may inspect/replace the outbound body.
        let body = crate::stream::apply_on_payload(
            opts,
            model,
            build_params(model, ctx, opts, auth.env.as_ref()),
        )
        .await;
        let headers = build_headers(model, auth, opts, &api_key);
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

/// Resolve the `POST` target: an auth base-url override wins over `model.base_url`. The endpoint is
/// `{base}/responses` (the OpenAI SDK's `client.responses.create` path).
fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/responses") {
        Some(trimmed.to_string())
    } else {
        Some(format!("{trimmed}/responses"))
    }
}

/// Pi `getClientApiKey` (openai-responses.ts:37-41) + the WireProvider-resolved key. A resolved key
/// wins; otherwise an `authorization`/`cf-aig-authorization` header (from the auth or opts overlay)
/// lets the SDK send the literal `"unused"`; otherwise `None` (caller errors).
fn resolve_api_key(auth: &AuthResult, opts: &StreamOptions) -> Option<String> {
    if let Some(key) = &auth.auth.api_key {
        return Some(key.clone());
    }
    let has = |name: &str| {
        header_present(auth.auth.headers.as_ref(), name)
            || header_present(opts.headers.as_ref(), name)
    };
    if has("authorization") || has("cf-aig-authorization") {
        return Some("unused".to_string());
    }
    None
}

/// Pi `hasHeader` (openai-responses.ts:28-35): a header is "present" only when set to a non-empty,
/// non-`None` value (case-insensitive name match).
fn header_present(headers: Option<&HeaderMap>, name: &str) -> bool {
    let Some(map) = headers else { return false };
    let want = name.to_ascii_lowercase();
    map.iter().any(|(k, v)| {
        k.to_ascii_lowercase() == want
            && v.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
    })
}

/// Resolve a provider env value (Pi `getProviderEnvValue`): the scoped `env` overlay wins over the
/// process environment.
pub(crate) fn provider_env_value(name: &str, env: Option<&ProviderEnv>) -> Option<String> {
    if let Some(map) = env
        && let Some(v) = map.get(name).filter(|v| !v.is_empty())
    {
        return Some(v.clone());
    }
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// 1:1 port of Pi `resolveCacheRetention` (openai-responses.ts:47-55).
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

/// Build the Responses request body (1:1 port of Pi `buildParams`, openai-responses.ts:231-279).
pub(crate) fn build_params(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    env: Option<&ProviderEnv>,
) -> Value {
    let compat = get_responses_compat(model);

    // --- DRIFT-001 deferred-tool placement (Pi openai-responses.ts:267-274) ---
    //
    // The Responses rendering is the MIRROR IMAGE of the Anthropic one: there, deferred tools stay
    // in `params.tools` carrying `defer_loading: true`; here they are omitted from `tools`
    // ENTIRELY and reach the model only inside the synthetic `tool_search_output` anchored at
    // their marker. Sending them in both places would re-inflate the cache-unstable prefix this
    // feature exists to avoid; sending them in neither would hand the model tool calls it has no
    // schema for.
    //
    // Two deliberate divergences from the Anthropic caller, both verified against Pi:
    //  * the split runs over the RAW `ctx.messages` — Pi passes `context`, not a transformed list,
    //    because `convertResponsesMessages` does its own `transformMessages` internally (:267);
    //  * there is NO safety valve. Pi's "promote everything back when the prefix would be empty"
    //    rule is Anthropic-only (anthropic-messages.ts:955-959); openai-responses.ts:301 guards on
    //    `immediate.length > 0`, so an all-deferred request ships a body with no `tools` key at
    //    all and the definitions live purely in the transcript.
    // The name normalizer is identity: Pi calls `splitDeferredTools(context, enabled)` with the
    // default `identityToolName`, so the deferred map is keyed by the raw tool name and
    // `deferredTools.get(name)` at the anchor site matches on the raw `addedToolNames` entry.
    let placement = split_deferred_tools(
        &ctx.messages,
        &ctx.tools,
        compat.supports_tool_search,
        &|name: &str| name.to_string(),
    );

    let messages =
        convert_responses_messages(model, ctx, OPENAI_TOOL_CALL_PROVIDERS, &placement.deferred);
    let cache = resolve_cache_retention(opts.cache_retention, env);

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert("input".to_string(), Value::Array(messages));
    obj.insert("stream".to_string(), json!(true));

    // prompt_cache_key: omitted when retention is none or no session id is available.
    if cache != CacheRetention::None
        && let Some(sid) = &opts.session_id
    {
        obj.insert(
            "prompt_cache_key".to_string(),
            json!(clamp_openai_prompt_cache_key(sid.as_str())),
        );
    }
    // prompt_cache_retention: "24h" only for long retention on a long-cache-capable model.
    if cache == CacheRetention::Long && compat.supports_long_cache_retention {
        obj.insert("prompt_cache_retention".to_string(), json!("24h"));
    }
    obj.insert("store".to_string(), json!(false));

    if let Some(max) = opts.max_tokens {
        obj.insert("max_output_tokens".to_string(), json!(max));
    }
    if let Some(temp) = opts.temperature {
        obj.insert("temperature".to_string(), json!(temp));
    }

    // service_tier (Pi `if (options?.serviceTier !== undefined) params.service_tier = …`,
    // openai-responses.ts:242-244). Omitted when unset (Pi default).
    if let Some(tier) = opts
        .openai_responses_options()
        .and_then(|o| o.service_tier.as_deref())
    {
        obj.insert("service_tier".to_string(), json!(tier));
    }

    // Only the IMMEDIATE tools reach `body.tools` (Pi `if (toolPlacement.immediate.length > 0)`,
    // openai-responses.ts:301-306). With tool search off the split is a pass-through, so this is
    // byte-identical to the old `if !ctx.tools.is_empty()` for every model that does not opt in.
    if !placement.immediate.is_empty() {
        obj.insert(
            "tools".to_string(),
            Value::Array(convert_responses_tools(&placement.immediate, false)),
        );
    }

    if model.reasoning {
        // The unified `reasoning` level maps to Pi's `reasoningEffort` (clamped; `off` => none).
        let clamped = clamp_thinking_level(model, opts.reasoning);
        if clamped != ModelThinkingLevel::Off {
            let key = thinking_level_key(clamped);
            let effort = mapped_effort_or(model.thinking_level_map.as_ref(), clamped, key);
            // Pi `summary: options?.reasoningSummary || "auto"` (openai-responses.ts:257).
            let summary = reasoning_summary_or_auto(opts.openai_responses_options());
            obj.insert(
                "reasoning".to_string(),
                json!({ "effort": effort, "summary": summary }),
            );
            obj.insert(
                "include".to_string(),
                json!(["reasoning.encrypted_content"]),
            );
        } else if model.provider.as_str() != "github-copilot"
            && off_is_not_null(model.thinking_level_map.as_ref())
        {
            let effort = off_value_or(model.thinking_level_map.as_ref(), "none");
            obj.insert("reasoning".to_string(), json!({ "effort": effort }));
        }
    }

    Value::Object(obj)
}

/// Build the request headers: `Authorization: Bearer <key>` plus the model / session / opts header
/// overlays (Pi `createClient`, openai-responses.ts:193-229). Precedence (low → high): auth Bearer
/// < `model.headers` < session affinity < `opts.headers`.
fn build_headers(
    model: &Model,
    auth: &AuthResult,
    opts: &StreamOptions,
    api_key: &str,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization".to_string(),
        Some(format!("Bearer {api_key}")),
    );
    if let Some(overlay) = &auth.auth.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    // `{ ...model.headers }` (openai-responses.ts:201).
    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }

    // Session headers (openai-responses.ts:211-216). Gated on cache retention != none.
    let cache = resolve_cache_retention(opts.cache_retention, auth.env.as_ref());
    let compat = get_responses_compat(model);
    if cache != CacheRetention::None
        && let Some(sid) = &opts.session_id
    {
        if compat.send_session_id_header {
            headers.insert("session_id".to_string(), Some(sid.as_str().to_string()));
        }
        headers.insert(
            "x-client-request-id".to_string(),
            Some(sid.as_str().to_string()),
        );
    }

    // Merge opts headers last so they override defaults (openai-responses.ts:219-221).
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    headers
}

// ---------------------------------------------------------------------------
// Message + tool conversion (Pi openai-responses-shared.ts)
// ---------------------------------------------------------------------------

/// Sanitize one id part to `^[a-zA-Z0-9_-]{1,64}$` with trailing `_` trimmed (Pi `normalizeIdPart`,
/// openai-responses-shared.ts:98-102).
fn normalize_id_part(part: &str) -> String {
    let sanitized: String = part
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let truncated: String = sanitized.chars().take(64).collect();
    truncated.trim_end_matches('_').to_string()
}

/// `fc_<shortHash>` clamped to 64 chars (Pi `buildForeignResponsesItemId`,
/// openai-responses-shared.ts:104-107).
fn build_foreign_responses_item_id(item_id: &str) -> String {
    let normalized = format!("fc_{}", short_hash(item_id));
    normalized.chars().take(64).collect()
}

/// 1:1 port of Pi `convertResponsesMessages` (openai-responses-shared.ts:90-267).
///
/// `deferred_tools` is Pi's `options.deferredTools` map (`ConvertResponsesMessagesOptions`,
/// openai-responses-shared.ts:118) in insertion order: the tools that [`build_params`] withheld
/// from `body.tools` and that must instead be anchored at their `addedToolNames` marker as a
/// synthetic client `tool_search_call`/`tool_search_output` pair. Pass an empty slice to disable
/// the rendering entirely — that is what `azure-openai-responses` does (Pi
/// `azure-openai-responses.ts:280` passes options WITHOUT `deferredTools` and never imports
/// `splitDeferredTools`).
pub(crate) fn convert_responses_messages(
    model: &Model,
    ctx: &Context,
    allowed_tool_call_providers: &[&str],
    deferred_tools: &[(String, ToolDef)],
) -> Vec<Value> {
    let provider = model.provider.as_str().to_string();
    let api = model.api.clone();
    let model_id = model.id.as_str().to_string();
    let allow = allowed_tool_call_providers.contains(&provider.as_str());

    let normalize = |id: &str, source: &AssistantMessage| -> String {
        if !allow {
            return normalize_id_part(id);
        }
        if !id.contains('|') {
            return normalize_id_part(id);
        }
        let parts: Vec<&str> = id.split('|').collect();
        let call_id = parts.first().copied().unwrap_or("");
        let item_id = parts.get(1).copied().unwrap_or("");
        let normalized_call_id = normalize_id_part(call_id);
        let is_foreign = source.provider.as_str() != provider || source.api != api;
        let mut normalized_item_id = if is_foreign {
            build_foreign_responses_item_id(item_id)
        } else {
            normalize_id_part(item_id)
        };
        if !normalized_item_id.starts_with("fc_") {
            normalized_item_id = normalize_id_part(&format!("fc_{normalized_item_id}"));
        }
        format!("{normalized_call_id}|{normalized_item_id}")
    };

    let transformed = transform_messages_with_source(&ctx.messages, model, normalize);

    let mut messages: Vec<Value> = Vec::new();
    // Declared once per conversion so a deferred tool is loaded EXACTLY ONCE per request even when
    // several tool results name it (Pi `const loadedToolNames = new Set<string>()`,
    // openai-responses-shared.ts:143). Keys are RAW names — unlike the Anthropic path there is no
    // normalizer on this side.
    let mut loaded_tool_names: HashSet<String> = HashSet::new();

    if let Some(system) = &ctx.system_prompt {
        let compat = get_responses_compat(model);
        let role = if model.reasoning && compat.supports_developer_role {
            "developer"
        } else {
            "system"
        };
        messages.push(json!({ "role": role, "content": sanitize_surrogates(system) }));
    }

    let mut msg_index: i64 = 0;
    for msg in &transformed {
        match msg {
            Message::User { content, .. } => {
                let parts: Vec<Value> = content
                    .iter()
                    .filter_map(|item| match item {
                        Content::Text { text, .. } => Some(json!({
                            "type": "input_text",
                            "text": sanitize_surrogates(text),
                        })),
                        Content::Image { data, mime_type } => Some(json!({
                            "type": "input_image",
                            "detail": "auto",
                            "image_url": format!("data:{mime_type};base64,{data}"),
                        })),
                        _ => None,
                    })
                    .collect();
                if parts.is_empty() {
                    continue;
                }
                messages.push(json!({ "role": "user", "content": parts }));
            }
            Message::Assistant(am) => {
                let is_different_model =
                    am.model != model_id && am.provider.as_str() == provider && am.api == api;
                let mut output: Vec<Value> = Vec::new();
                let mut text_block_index: i64 = 0;
                for block in &am.content {
                    match block {
                        Content::Thinking {
                            thinking_signature, ..
                        } => {
                            if let Some(sig) = thinking_signature {
                                // The signature is the JSON-encoded reasoning item; replay verbatim.
                                if let Ok(item) = serde_json::from_str::<Value>(sig) {
                                    output.push(item);
                                }
                            }
                        }
                        Content::Text {
                            text,
                            text_signature,
                        } => {
                            let parsed = text_signature.as_deref().and_then(parse_text_signature);
                            let fallback = if text_block_index == 0 {
                                format!("msg_pi_{msg_index}")
                            } else {
                                format!("msg_pi_{msg_index}_{text_block_index}")
                            };
                            text_block_index += 1;
                            let mut msg_id = parsed.as_ref().map(|p| p.id.clone());
                            match &msg_id {
                                None => msg_id = Some(fallback),
                                Some(id) if id.chars().count() > 64 => {
                                    msg_id = Some(format!("msg_{}", short_hash(id)));
                                }
                                Some(_) => {}
                            }
                            let mut item = Map::new();
                            item.insert("type".to_string(), json!("message"));
                            item.insert("role".to_string(), json!("assistant"));
                            item.insert(
                                "content".to_string(),
                                json!([{
                                    "type": "output_text",
                                    "text": sanitize_surrogates(text),
                                    "annotations": [],
                                }]),
                            );
                            item.insert("status".to_string(), json!("completed"));
                            item.insert("id".to_string(), json!(msg_id));
                            if let Some(phase) = parsed.and_then(|p| p.phase) {
                                item.insert("phase".to_string(), json!(phase_wire(phase)));
                            }
                            output.push(Value::Object(item));
                        }
                        Content::ToolCall(tc) => {
                            let id = tc.id.as_str();
                            let parts: Vec<&str> = id.split('|').collect();
                            let call_id = parts.first().copied().unwrap_or("");
                            let mut item_id = parts.get(1).copied().map(|s| s.to_string());
                            // Drop a different-model `fc_*` item id to avoid pairing validation.
                            if is_different_model
                                && item_id
                                    .as_deref()
                                    .map(|s| s.starts_with("fc_"))
                                    .unwrap_or(false)
                            {
                                item_id = None;
                            }
                            let mut item = Map::new();
                            item.insert("type".to_string(), json!("function_call"));
                            if let Some(iid) = item_id {
                                item.insert("id".to_string(), json!(iid));
                            }
                            item.insert("call_id".to_string(), json!(call_id));
                            item.insert("name".to_string(), json!(tc.name));
                            item.insert(
                                "arguments".to_string(),
                                json!(serde_json::to_string(&tc.arguments).unwrap_or_default()),
                            );
                            output.push(Value::Object(item));
                        }
                        Content::Image { .. } => {}
                    }
                }
                if output.is_empty() {
                    continue;
                }
                messages.extend(output);
            }
            Message::ToolResult {
                tool_call_id,
                content,
                added_tool_names,
                ..
            } => {
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
                let call_id = tool_call_id
                    .as_str()
                    .split('|')
                    .next()
                    .unwrap_or(tool_call_id.as_str());

                let output: Value = if has_images && model.supports_image_input() {
                    let mut parts: Vec<Value> = Vec::new();
                    if has_text {
                        parts.push(json!({
                            "type": "input_text",
                            "text": sanitize_surrogates(&text_result),
                        }));
                    }
                    for block in content {
                        if let Content::Image { data, mime_type } = block {
                            parts.push(json!({
                                "type": "input_image",
                                "detail": "auto",
                                "image_url": format!("data:{mime_type};base64,{data}"),
                            }));
                        }
                    }
                    Value::Array(parts)
                } else {
                    Value::String(sanitize_surrogates(if has_text {
                        &text_result
                    } else {
                        "(see attached image)"
                    }))
                };

                messages.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));

                // --- DRIFT-001 anchor: the Responses rendering (Pi
                // openai-responses-shared.ts:304-332) ---
                //
                // Injected IMMEDIATELY AFTER the `function_call_output` at this transcript index,
                // so the definitions land at the point in the conversation where the tools became
                // available. A marked name that is absent from `deferred_tools` produces nothing:
                // either the model can already see it (it was left immediate) or it is not in
                // `Context.tools` at all.
                let mut loaded: Vec<&ToolDef> = Vec::new();
                for name in added_tool_names {
                    if loaded_tool_names.contains(name) {
                        continue;
                    }
                    // Pi `options?.deferredTools?.get(name)` — the map is keyed by the (identity-)
                    // normalized name, so this is a raw-name lookup.
                    let Some((_, tool)) = deferred_tools.iter().find(|(key, _)| key == name) else {
                        continue;
                    };
                    loaded_tool_names.insert(name.clone());
                    loaded.push(tool);
                }
                if !loaded.is_empty() {
                    let names: Vec<&str> = loaded.iter().map(|t| t.name.as_str()).collect();
                    // The hash input uses the FULL `tool_call_id`, INCLUDING any `|item_id`
                    // suffix — NOT the `call_id` split off above for `function_call_output`
                    // (Pi `${msg.toolCallId}:${names.join(",")}`, :306). Comma-joined for the
                    // hash, SPACE-joined for the query.
                    //
                    // The `pi_tool_load_` prefix is kept VERBATIM despite cyrup's `pi` → `cyrup`
                    // rebrand: this string is on the wire and is what the differential/golden
                    // parity harnesses diff. Renaming it would be a silent wire divergence, so it
                    // is deliberately NOT a `[CYRUP-DELTA]`.
                    let search_call_id = format!(
                        "pi_tool_load_{}",
                        short_hash(&format!("{}:{}", tool_call_id.as_str(), names.join(",")))
                    );
                    messages.push(json!({
                        "type": "tool_search_call",
                        "call_id": search_call_id,
                        "execution": "client",
                        "status": "completed",
                        "arguments": {
                            "query": names.join(" "),
                            "limit": names.len(),
                        },
                    }));
                    let defs: Vec<ToolDef> = loaded.into_iter().cloned().collect();
                    messages.push(json!({
                        "type": "tool_search_output",
                        "call_id": search_call_id,
                        "execution": "client",
                        "status": "completed",
                        // `defer_loading: true` on every definition in the output (Pi
                        // `{ ...options?.toolOptions, deferLoading: true }`, :330).
                        "tools": Value::Array(convert_responses_tools(&defs, true)),
                    }));
                }
            }
        }
        msg_index += 1;
    }

    messages
}

/// Pi `parseTextSignature` (openai-responses-shared.ts:46-64): structured V1 JSON or a legacy
/// plain-string id.
fn parse_text_signature(signature: &str) -> Option<TextSignatureV1> {
    if signature.starts_with('{')
        && let Some(v1) = TextSignatureV1::parse(signature)
    {
        return Some(v1);
    }
    Some(TextSignatureV1 {
        v: 1,
        id: signature.to_string(),
        phase: None,
    })
}

/// The wire string for a [`TextPhase`] (Pi `commentary` / `final_answer`).
fn phase_wire(phase: TextPhase) -> &'static str {
    match phase {
        TextPhase::Commentary => "commentary",
        TextPhase::FinalAnswer => "final_answer",
    }
}

/// 1:1 port of Pi `convertResponsesTools` (openai-responses-shared.ts:273-282). `strict` defaults
/// to `false`.
///
/// `defer_loading` is Pi's `options.deferLoading` (`ConvertResponsesToolsOptions`, :127): set only
/// for the definitions carried inside a `tool_search_output`, never for `body.tools`. Cyrup ports
/// only Pi's function-tool branch (no grammar/custom tools), so there is a single emission site.
pub(crate) fn convert_responses_tools(tools: &[ToolDef], defer_loading: bool) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let mut o = Map::new();
            o.insert("type".to_string(), json!("function"));
            o.insert("name".to_string(), json!(t.name));
            o.insert("description".to_string(), json!(t.description));
            o.insert("parameters".to_string(), t.parameters.clone());
            // Pi spreads `...(options?.deferLoading ? { defer_loading: true } : {})` — the key is
            // ABSENT, not `false`, when the tool is part of the request prefix.
            if defer_loading {
                o.insert("defer_loading".to_string(), json!(true));
            }
            o.insert("strict".to_string(), json!(false));
            Value::Object(o)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    Thinking,
    Text,
    Tool,
}

enum RBlock {
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    Text {
        text: String,
        signature: Option<String>,
    },
    Tool {
        call_id: String,
        item_id: String,
        name: String,
        partial_json: String,
    },
}

struct RDecoder {
    blocks: Vec<RBlock>,
    /// Active output-index → (block position, kind). Removed on `output_item.done`.
    slots: HashMap<i64, (usize, SlotKind)>,
    usage: Usage,
    response_id: Option<String>,
    stop_reason: StopReason,
    saw_terminal: bool,
}

impl Default for RDecoder {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            slots: HashMap::new(),
            usage: Usage::default(),
            response_id: None,
            stop_reason: StopReason::Stop,
            saw_terminal: false,
        }
    }
}

impl RDecoder {
    fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        AssistantMessage {
            content: blocks_to_content(&self.blocks),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: self.response_id.clone(),
            diagnostics: None,
            usage: self.usage.clone(),
            stop_reason: self.stop_reason,
            error_message: None,
            timestamp: now_millis(),
        }
    }

    fn slot(&self, output_index: i64, kind: SlotKind) -> Option<usize> {
        self.slots
            .get(&output_index)
            .filter(|(_, k)| *k == kind)
            .map(|(i, _)| *i)
    }
}

fn blocks_to_content(blocks: &[RBlock]) -> Vec<Content> {
    blocks
        .iter()
        .map(|b| match b {
            RBlock::Thinking {
                thinking,
                signature,
            } => Content::Thinking {
                thinking: thinking.clone(),
                thinking_signature: signature.clone(),
                redacted: false,
            },
            RBlock::Text { text, signature } => Content::Text {
                text: text.clone(),
                text_signature: signature.clone(),
            },
            RBlock::Tool {
                call_id,
                item_id,
                name,
                partial_json,
            } => Content::ToolCall(ToolCall {
                id: ToolCallId::from(format!("{call_id}|{item_id}").as_str()),
                name: name.clone(),
                arguments: parse_streaming_json_object(Some(partial_json)),
                thought_signature: None,
            }),
        })
        .collect()
}

/// Drive the Responses SSE frame stream into ordered [`StreamEvent`]s (1:1 with Pi's stream loop).
pub(crate) async fn decode_stream<S>(mut frames: S, model: &Model, api: &ApiId, sink: &EventSink)
where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    let provider = model.provider.clone();
    let model_id = model.id.as_str().to_string();

    let mut dec = RDecoder::default();
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
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            emit_error(
                &dec,
                model,
                api,
                sink,
                "Could not parse OpenAI Responses SSE event".into(),
            )
            .await;
            return;
        };
        match process_event(&event, &mut dec, model, api, sink).await {
            ProcessResult::Continue => {}
            ProcessResult::Dropped => return,
            ProcessResult::Error(msg) => {
                emit_error(&dec, model, api, sink, msg).await;
                return;
            }
        }
    }

    // `saw_terminal` is this decoder's spelling of "the provider delivered a stop reason": only a
    // terminal `response.*` event sets `dec.stop_reason`, so without one the seeded `Stop` is a
    // guess. Routed through the same `end_of_stream` seam as the other four wire APIs so the
    // truncated-stream rule lives in exactly one place (Pi openai-responses.ts:170-172).
    sink.send(StreamEvent::end_of_stream(
        dec.snapshot(model, api),
        dec.saw_terminal.then_some(dec.stop_reason),
        "OpenAI Responses stream ended before a terminal response event",
    ))
    .await;
}

enum ProcessResult {
    Continue,
    Dropped,
    Error(String),
}

async fn process_event(
    event: &Value,
    dec: &mut RDecoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> ProcessResult {
    let etype = event.get("type").and_then(Value::as_str).unwrap_or("");
    let oi = event
        .get("output_index")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    macro_rules! emit {
        ($ev:expr) => {
            if !sink.send($ev).await {
                return ProcessResult::Dropped;
            }
        };
    }

    match etype {
        "response.created" => {
            if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                dec.response_id = Some(id.to_string());
            }
        }
        "response.output_item.added" => {
            if let Some(item) = event.get("item")
                && let Some((ci, kind)) = create_slot(dec, oi, item)
            {
                let ev = match kind {
                    SlotKind::Thinking => StreamEvent::ThinkingStart {
                        content_index: ci,
                        partial: dec.snapshot(model, api),
                    },
                    SlotKind::Text => StreamEvent::TextStart {
                        content_index: ci,
                        partial: dec.snapshot(model, api),
                    },
                    SlotKind::Tool => StreamEvent::ToolCallStart {
                        content_index: ci,
                        partial: dec.snapshot(model, api),
                    },
                };
                emit!(ev);
            }
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
            if let Some(ci) = dec.slot(oi, SlotKind::Thinking) {
                if let Some(RBlock::Thinking { thinking, .. }) = dec.blocks.get_mut(ci) {
                    thinking.push_str(delta);
                }
                emit!(StreamEvent::ThinkingDelta {
                    content_index: ci,
                    delta: delta.to_string(),
                    partial: dec.snapshot(model, api),
                });
            }
        }
        "response.reasoning_summary_part.done" => {
            if let Some(ci) = dec.slot(oi, SlotKind::Thinking) {
                if let Some(RBlock::Thinking { thinking, .. }) = dec.blocks.get_mut(ci) {
                    thinking.push_str("\n\n");
                }
                emit!(StreamEvent::ThinkingDelta {
                    content_index: ci,
                    delta: "\n\n".to_string(),
                    partial: dec.snapshot(model, api),
                });
            }
        }
        "response.output_text.delta" | "response.refusal.delta" => {
            let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
            if let Some(ci) = dec.slot(oi, SlotKind::Text) {
                if let Some(RBlock::Text { text, .. }) = dec.blocks.get_mut(ci) {
                    text.push_str(delta);
                }
                emit!(StreamEvent::TextDelta {
                    content_index: ci,
                    delta: delta.to_string(),
                    partial: dec.snapshot(model, api),
                });
            }
        }
        "response.function_call_arguments.delta" => {
            let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
            if let Some(ci) = dec.slot(oi, SlotKind::Tool) {
                if let Some(RBlock::Tool { partial_json, .. }) = dec.blocks.get_mut(ci) {
                    partial_json.push_str(delta);
                }
                emit!(StreamEvent::ToolCallDelta {
                    content_index: ci,
                    delta: delta.to_string(),
                    partial: dec.snapshot(model, api),
                });
            }
        }
        "response.function_call_arguments.done" => {
            let arguments = event.get("arguments").and_then(Value::as_str).unwrap_or("");
            if let Some(ci) = dec.slot(oi, SlotKind::Tool) {
                let mut maybe_delta: Option<String> = None;
                if let Some(RBlock::Tool { partial_json, .. }) = dec.blocks.get_mut(ci) {
                    let previous = partial_json.clone();
                    *partial_json = arguments.to_string();
                    if let Some(rest) = arguments
                        .strip_prefix(previous.as_str())
                        .filter(|r| !r.is_empty())
                    {
                        maybe_delta = Some(rest.to_string());
                    }
                }
                if let Some(delta) = maybe_delta {
                    emit!(StreamEvent::ToolCallDelta {
                        content_index: ci,
                        delta,
                        partial: dec.snapshot(model, api),
                    });
                }
            }
        }
        "response.output_item.done" => {
            let Some(item) = event.get("item") else {
                return ProcessResult::Continue;
            };
            let Some((ci, kind)) = get_or_create_slot(dec, oi, item, model, api, sink).await else {
                return ProcessResult::Continue;
            };
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            match (item_type, kind) {
                ("reasoning", SlotKind::Thinking) => {
                    let summary = join_text_array(item.get("summary"));
                    let content = join_text_array(item.get("content"));
                    let final_text = if !summary.is_empty() {
                        summary
                    } else if !content.is_empty() {
                        content
                    } else if let Some(RBlock::Thinking { thinking, .. }) = dec.blocks.get(ci) {
                        thinking.clone()
                    } else {
                        String::new()
                    };
                    let sig = serde_json::to_string(item).ok();
                    if let Some(RBlock::Thinking {
                        thinking,
                        signature,
                    }) = dec.blocks.get_mut(ci)
                    {
                        *thinking = final_text.clone();
                        *signature = sig;
                    }
                    dec.slots.remove(&oi);
                    emit!(StreamEvent::ThinkingEnd {
                        content_index: ci,
                        content: final_text,
                        partial: dec.snapshot(model, api),
                    });
                }
                ("message", SlotKind::Text) => {
                    let final_text = join_message_content(item.get("content"));
                    let id = item.get("id").and_then(Value::as_str).unwrap_or("");
                    let phase = item
                        .get("phase")
                        .and_then(Value::as_str)
                        .and_then(parse_phase);
                    let sig = TextSignatureV1::new(id, phase).encode();
                    if let Some(RBlock::Text { text, signature }) = dec.blocks.get_mut(ci) {
                        *text = final_text.clone();
                        *signature = Some(sig);
                    }
                    dec.slots.remove(&oi);
                    emit!(StreamEvent::TextEnd {
                        content_index: ci,
                        content: final_text,
                        partial: dec.snapshot(model, api),
                    });
                }
                ("function_call", SlotKind::Tool) => {
                    let raw = item.get("arguments").and_then(Value::as_str).unwrap_or("");
                    if let Some(RBlock::Tool { partial_json, .. }) = dec.blocks.get_mut(ci) {
                        if !raw.is_empty() {
                            *partial_json = raw.to_string();
                        } else if partial_json.is_empty() {
                            *partial_json = "{}".to_string();
                        }
                    }
                    let tool_call = match blocks_to_content(&dec.blocks).get(ci) {
                        Some(Content::ToolCall(tc)) => tc.clone(),
                        _ => ToolCall {
                            id: ToolCallId::from(""),
                            name: String::new(),
                            arguments: Map::new(),
                            thought_signature: None,
                        },
                    };
                    dec.slots.remove(&oi);
                    emit!(StreamEvent::ToolCallEnd {
                        content_index: ci,
                        tool_call,
                        partial: dec.snapshot(model, api),
                    });
                }
                _ => {}
            }
        }
        "response.completed" | "response.incomplete" => {
            finalize_response(event.get("response"), dec, model);
        }
        "error" => {
            let code = event.get("code").and_then(Value::as_str).unwrap_or("");
            let message = event
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Unknown error");
            return ProcessResult::Error(format!("Error Code {code}: {message}"));
        }
        "response.failed" => {
            dec.saw_terminal = true;
            let response = event.get("response");
            let error = response.and_then(|r| r.get("error"));
            let msg = if let Some(error) = error.filter(|e| !e.is_null()) {
                let code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("no message");
                format!("{code}: {message}")
            } else if let Some(reason) = response
                .and_then(|r| r.pointer("/incomplete_details/reason"))
                .and_then(Value::as_str)
            {
                format!("incomplete: {reason}")
            } else {
                "Unknown error (no error details in response)".to_string()
            };
            return ProcessResult::Error(msg);
        }
        _ => {}
    }
    ProcessResult::Continue
}

/// Create a content slot for a streamed output item (Pi `createSlot`). Returns the new block's
/// content index + kind, or `None` for an unrecognized item type.
fn create_slot(dec: &mut RDecoder, output_index: i64, item: &Value) -> Option<(usize, SlotKind)> {
    let item_type = item.get("type").and_then(Value::as_str)?;
    let (block, kind) = match item_type {
        "reasoning" => (
            RBlock::Thinking {
                thinking: String::new(),
                signature: None,
            },
            SlotKind::Thinking,
        ),
        "message" => (
            RBlock::Text {
                text: String::new(),
                signature: None,
            },
            SlotKind::Text,
        ),
        "function_call" => {
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let item_id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let partial_json = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            (
                RBlock::Tool {
                    call_id,
                    item_id,
                    name,
                    partial_json,
                },
                SlotKind::Tool,
            )
        }
        _ => return None,
    };
    dec.blocks.push(block);
    let ci = dec.blocks.len() - 1;
    dec.slots.insert(output_index, (ci, kind));
    Some((ci, kind))
}

/// Pi `getOrCreateSlot`: the existing slot, else create one (emitting its `*_start`). Returns the
/// content index + kind.
async fn get_or_create_slot(
    dec: &mut RDecoder,
    output_index: i64,
    item: &Value,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> Option<(usize, SlotKind)> {
    if let Some((ci, kind)) = dec.slots.get(&output_index).copied() {
        return Some((ci, kind));
    }
    let (ci, kind) = create_slot(dec, output_index, item)?;
    let ev = match kind {
        SlotKind::Thinking => StreamEvent::ThinkingStart {
            content_index: ci,
            partial: dec.snapshot(model, api),
        },
        SlotKind::Text => StreamEvent::TextStart {
            content_index: ci,
            partial: dec.snapshot(model, api),
        },
        SlotKind::Tool => StreamEvent::ToolCallStart {
            content_index: ci,
            partial: dec.snapshot(model, api),
        },
    };
    sink.send(ev).await;
    Some((ci, kind))
}

/// Apply a terminal `response.completed`/`response.incomplete` (Pi `finalizeResponse`).
fn finalize_response(response: Option<&Value>, dec: &mut RDecoder, model: &Model) {
    dec.saw_terminal = true;
    if let Some(id) = response.and_then(|r| r.get("id")).and_then(Value::as_str) {
        dec.response_id = Some(id.to_string());
    }
    if let Some(usage) = response.and_then(|r| r.get("usage")) {
        let cached = usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let input_tokens = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        dec.usage = Usage {
            input: input_tokens.saturating_sub(cached),
            output: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read: cached,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: Some(
                usage
                    .pointer("/output_tokens_details/reasoning_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            ),
            total_tokens: usage
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cost: Default::default(),
        };
    }
    apply_cost(&model.cost, &mut dec.usage);
    // Service-tier pricing (Pi applyServiceTierPricing): driven by the response's `service_tier`.
    let service_tier = response
        .and_then(|r| r.get("service_tier"))
        .and_then(Value::as_str);
    apply_service_tier_pricing(&mut dec.usage, service_tier, model);

    let status = response
        .and_then(|r| r.get("status"))
        .and_then(Value::as_str);
    dec.stop_reason = map_stop_reason(status);
    let has_tool = dec.blocks.iter().any(|b| matches!(b, RBlock::Tool { .. }));
    if has_tool && dec.stop_reason == StopReason::Stop {
        dec.stop_reason = StopReason::ToolUse;
    }
}

/// Pi `getServiceTierCostMultiplier` (openai-responses.ts:281-293).
fn service_tier_multiplier(model_id: &str, service_tier: Option<&str>) -> f64 {
    match service_tier {
        Some("flex") => 0.5,
        Some("priority") => {
            if model_id == "gpt-5.5" {
                2.5
            } else {
                2.0
            }
        }
        _ => 1.0,
    }
}

/// Pi `applyServiceTierPricing` (openai-responses.ts:295-308).
fn apply_service_tier_pricing(usage: &mut Usage, service_tier: Option<&str>, model: &Model) {
    let multiplier = service_tier_multiplier(model.id.as_str(), service_tier);
    if multiplier == 1.0 {
        return;
    }
    usage.cost.input *= multiplier;
    usage.cost.output *= multiplier;
    usage.cost.cache_read *= multiplier;
    usage.cost.cache_write *= multiplier;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}

/// Pi `mapStopReason` (openai-responses-shared.ts:533-552).
fn map_stop_reason(status: Option<&str>) -> StopReason {
    match status {
        None => StopReason::Stop,
        Some("completed") => StopReason::Stop,
        Some("incomplete") => StopReason::Length,
        Some("failed") | Some("cancelled") => StopReason::Error,
        Some("in_progress") | Some("queued") => StopReason::Stop,
        Some(_) => StopReason::Stop,
    }
}

fn parse_phase(s: &str) -> Option<TextPhase> {
    match s {
        "commentary" => Some(TextPhase::Commentary),
        "final_answer" => Some(TextPhase::FinalAnswer),
        _ => None,
    }
}

/// Join a reasoning item's `summary`/`content` array of `{text}` parts with `"\n\n"`.
fn join_text_array(value: Option<&Value>) -> String {
    let Some(Value::Array(arr)) = value else {
        return String::new();
    };
    arr.iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Join a message item's `content` parts: `output_text.text` or `refusal` (Pi `item.content?.map`).
fn join_message_content(value: Option<&Value>) -> String {
    let Some(Value::Array(arr)) = value else {
        return String::new();
    };
    arr.iter()
        .map(|c| {
            if c.get("type").and_then(Value::as_str) == Some("output_text") {
                c.get("text").and_then(Value::as_str).unwrap_or("")
            } else {
                c.get("refusal").and_then(Value::as_str).unwrap_or("")
            }
        })
        .collect::<String>()
}

/// Emit a terminal `error` event carrying the live snapshot + message (Pi catch block).
async fn emit_error(dec: &RDecoder, model: &Model, api: &ApiId, sink: &EventSink, message: String) {
    let mut msg = dec.snapshot(model, api);
    msg.stop_reason = StopReason::Error;
    msg.error_message = Some(message);
    sink.send(StreamEvent::terminal(msg)).await;
}

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
    use crate::model::{Modality, ModelCost};
    use crate::stream::collect_message;
    use crate::stream::sse::decode_sse_bytes;

    fn model() -> Model {
        Model {
            id: "gpt-5".into(),
            name: "GPT-5".into(),
            api: API_ID.into(),
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".to_string(),
            reasoning: true,
            input: vec![Modality::Text],
            cost: ModelCost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.5,
                cache_write: 0.0,
                tiers: None,
            },
            context_window: 400_000,
            max_tokens: 128_000,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn user_ctx(text: &str) -> Context {
        Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text(text)],
                timestamp: 0,
            }],
            tools: Vec::new(),
        }
    }

    fn auth() -> AuthResult {
        AuthResult {
            auth: crate::auth::ModelAuth {
                api_key: Some("sk-test".to_string()),
                headers: None,
                base_url: None,
            },
            env: None,
            source: Some("test".to_string()),
        }
    }

    #[test]
    fn url_appends_responses() {
        let m = model();
        assert_eq!(
            resolve_url(&m, &auth()).as_deref(),
            Some("https://api.openai.com/v1/responses")
        );
    }

    #[test]
    fn build_params_basic_shape() {
        let m = model();
        let opts = StreamOptions {
            max_tokens: Some(100),
            ..Default::default()
        };
        let body = build_params(&m, &user_ctx("hi"), &opts, None);
        assert_eq!(body["model"], "gpt-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["max_output_tokens"], 100);
        // input is the Responses message array with one user input_text.
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][0]["text"], "hi");
    }

    #[test]
    fn reasoning_effort_encodes_with_encrypted_content() {
        let m = model();
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_params(&m, &user_ctx("hi"), &opts, None);
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn reasoning_summary_per_api_option_overrides_auto() {
        // Pi `summary: options?.reasoningSummary || "auto"` (openai-responses.ts:257).
        let m = model();
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            api_options: Some(crate::stream::ApiStreamOptions::OpenAiResponses(
                OpenAiResponsesOptions {
                    reasoning_summary: Some(ReasoningSummary::Detailed),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        let body = build_params(&m, &user_ctx("hi"), &opts, None);
        assert_eq!(body["reasoning"]["summary"], "detailed");

        // Explicit `null` falls back to "auto" (Pi `|| "auto"`).
        let opts_null = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            api_options: Some(crate::stream::ApiStreamOptions::OpenAiResponses(
                OpenAiResponsesOptions {
                    reasoning_summary: Some(ReasoningSummary::Null),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        let body = build_params(&m, &user_ctx("hi"), &opts_null, None);
        assert_eq!(body["reasoning"]["summary"], "auto");
    }

    #[test]
    fn service_tier_per_api_option_threads_to_request() {
        // Pi sets `params.service_tier` only when `serviceTier` is defined (openai-responses.ts:242).
        let m = model();
        // Omitted by default.
        let body = build_params(&m, &user_ctx("hi"), &StreamOptions::default(), None);
        assert!(body.get("service_tier").is_none());

        let opts = StreamOptions {
            api_options: Some(crate::stream::ApiStreamOptions::OpenAiResponses(
                OpenAiResponsesOptions {
                    service_tier: Some("priority".to_string()),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        let body = build_params(&m, &user_ctx("hi"), &opts, None);
        assert_eq!(body["service_tier"], "priority");
    }

    #[test]
    fn reasoning_off_uses_off_map_value_when_supported() {
        let m = model();
        // With no thinkingLevelMap, `off` is a supported level, so a default (Off) request clamps to
        // `off` and takes the no-summary branch: reasoning.effort = off-map value ?? "none".
        let opts = StreamOptions::default(); // reasoning defaults to Off
        let body = build_params(&m, &user_ctx("hi"), &opts, None);
        assert_eq!(body["reasoning"]["effort"], "none");
        assert!(body["reasoning"].get("summary").is_none());
        // No `include` on the no-summary branch (only the effort-bearing branch sets it).
        assert!(body.get("include").is_none());
    }

    #[test]
    fn reasoning_off_null_clamps_up_to_lowest_supported() {
        let mut m = model();
        // thinkingLevelMap.off = null marks `off` unsupported, so a default (Off) request clamps UP
        // to the lowest supported level (minimal) and takes the effort branch — matching Pi's
        // streamSimple clamp + buildParams (the `off !== null` guard is never reached here).
        let mut map = crate::model::ThinkingLevelMap::new();
        map.insert("off".to_string(), None);
        m.thinking_level_map = Some(map);
        let body = build_params(&m, &user_ctx("hi"), &StreamOptions::default(), None);
        assert_eq!(body["reasoning"]["effort"], "minimal");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn long_cache_retention_emits_24h() {
        let m = model();
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::Long),
            session_id: Some("sess-1".into()),
            ..Default::default()
        };
        let body = build_params(&m, &user_ctx("hi"), &opts, None);
        assert_eq!(body["prompt_cache_retention"], "24h");
        assert_eq!(body["prompt_cache_key"], "sess-1");
    }

    #[test]
    fn tools_convert_to_responses_function_tools() {
        let mut ctx = user_ctx("hi");
        ctx.tools = vec![ToolDef {
            name: "echo".into(),
            description: "echoes".into(),
            parameters: json!({"type": "object"}),
        }];
        let body = build_params(&model(), &ctx, &StreamOptions::default(), None);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "echo");
        assert_eq!(body["tools"][0]["strict"], false);
    }

    #[test]
    fn headers_carry_bearer_and_session() {
        let m = model();
        let opts = StreamOptions {
            session_id: Some("sess-7".into()),
            ..Default::default()
        };
        let h = build_headers(&m, &auth(), &opts, "sk-test");
        assert_eq!(
            h.get("Authorization"),
            Some(&Some("Bearer sk-test".to_string()))
        );
        assert_eq!(h.get("session_id"), Some(&Some("sess-7".to_string())));
        assert_eq!(
            h.get("x-client-request-id"),
            Some(&Some("sess-7".to_string()))
        );
    }

    #[test]
    fn assistant_text_replay_carries_message_item() {
        let mut ctx = user_ctx("hi");
        let am = AssistantMessage {
            content: vec![Content::text("prior answer")],
            provider: "openai".into(),
            model: "gpt-5".into(),
            api: API_ID.into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        };
        ctx.messages.push(Message::Assistant(am));
        let body = build_params(&model(), &ctx, &StreamOptions::default(), None);
        let input = body["input"].as_array().unwrap();
        // user + assistant message item.
        let assistant = input.iter().find(|m| m["type"] == "message").unwrap();
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["content"][0]["type"], "output_text");
        assert_eq!(assistant["content"][0]["text"], "prior answer");
        // fallback id for a signature-less first text block.
        assert!(assistant["id"].as_str().unwrap().starts_with("msg_pi_"));
    }

    #[test]
    fn normalize_id_part_sanitizes_and_trims() {
        assert_eq!(normalize_id_part("abc|def#ghi"), "abc_def_ghi");
        assert_eq!(normalize_id_part("trailing___"), "trailing");
        assert_eq!(normalize_id_part(&"x".repeat(100)).chars().count(), 64);
    }

    #[tokio::test]
    async fn decodes_full_text_and_toolcall_stream() {
        // A scripted Responses SSE stream: text item + function_call item, then completed.
        let raw = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_a\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"Hello\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\" world\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_a\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello world\"}]}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"delta\":\"{\\\"x\\\":1}\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.done\",\"output_index\":1,\"arguments\":\"{\\\"x\\\":1}\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"{\\\"x\\\":1}\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15,\"input_tokens_details\":{\"cached_tokens\":2},\"output_tokens_details\":{\"reasoning_tokens\":3}}}}\n\n",
        );
        let frames = decode_sse_bytes(raw.as_bytes().to_vec());
        let (sink, rx) = crate::api::channel(64);
        let m = model();
        let api = ApiId::from(API_ID);
        decode_stream(frames, &m, &api, &sink).await;
        drop(sink);
        let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx));
        let msg = collect_message(stream).await;

        // Tool call present => toolUse terminal.
        assert_eq!(msg.stop_reason, StopReason::ToolUse);
        assert_eq!(msg.response_id.as_deref(), Some("resp_1"));
        // Usage: cached subtracted from input; reasoning carried.
        assert_eq!(msg.usage.input, 8);
        assert_eq!(msg.usage.output, 5);
        assert_eq!(msg.usage.cache_read, 2);
        assert_eq!(msg.usage.reasoning, Some(3));
        // Cost applied (input 8/1e6 * 1.0 + output 5/1e6*2.0 + cacheRead 2/1e6*0.5).
        assert!(msg.usage.cost.total > 0.0);
        // Content: text "Hello world" + tool call echo({"x":1}).
        let text = msg.content.iter().find_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert_eq!(text.as_deref(), Some("Hello world"));
        let tc = msg.content.iter().find_map(|c| match c {
            Content::ToolCall(tc) => Some(tc.clone()),
            _ => None,
        });
        let tc = tc.expect("tool call");
        assert_eq!(tc.name, "echo");
        assert_eq!(tc.id.as_str(), "call_1|fc_1");
        assert_eq!(tc.arguments.get("x"), Some(&json!(1)));
    }

    #[tokio::test]
    async fn missing_terminal_event_is_an_error() {
        let raw = "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r\"}}\n\n";
        let frames = decode_sse_bytes(raw.as_bytes().to_vec());
        let (sink, rx) = crate::api::channel(64);
        let m = model();
        let api = ApiId::from(API_ID);
        decode_stream(frames, &m, &api, &sink).await;
        drop(sink);
        let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx));
        let msg = collect_message(stream).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(
            msg.error_message
                .unwrap()
                .contains("terminal response event")
        );
    }

    #[tokio::test]
    async fn response_failed_emits_error() {
        let raw = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r\"}}\n\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"rate_limit\",\"message\":\"slow down\"}}}\n\n",
        );
        let frames = decode_sse_bytes(raw.as_bytes().to_vec());
        let (sink, rx) = crate::api::channel(64);
        let m = model();
        let api = ApiId::from(API_ID);
        decode_stream(frames, &m, &api, &sink).await;
        drop(sink);
        let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx));
        let msg = collect_message(stream).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("rate_limit: slow down"));
    }

    // -----------------------------------------------------------------------
    // DRIFT-001: message-anchored tool loading, the openai-responses rendering
    // (Pi `packages/ai/test/deferred-tools.test.ts`).
    //
    // Every assertion below reads the EMITTED WIRE JSON. The rule that makes this rendering the
    // mirror image of the Anthropic one: a deferred tool is omitted from `body.tools` ENTIRELY and
    // exists only inside the synthetic `tool_search_output`. Getting that backwards would ship the
    // model tool calls whose schemas it never received.
    // -----------------------------------------------------------------------

    /// A real catalog model, so the default-OFF proof runs against shipped data, not a fixture.
    fn catalog_model(id: &str) -> Model {
        crate::providers::openai::openai_models()
            .into_iter()
            .find(|m| m.id.as_str() == id)
            .unwrap_or_else(|| panic!("`{id}` missing from the embedded openai catalog"))
    }

    fn deferred_tool(name: &str) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: format!("The {name} tool"),
            parameters: json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"],
            }),
        }
    }

    /// Pi `makeAssistantToolCall` — a FOREIGN (anthropic-authored) assistant turn, exactly as the
    /// upstream test builds it.
    fn assistant_tool_call(id: &str, name: &str) -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![Content::ToolCall(ToolCall {
                id: ToolCallId::from(id),
                name: name.to_string(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
            })],
            provider: "anthropic".into(),
            model: "claude-opus-4-6".to_string(),
            api: "anthropic-messages".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 2,
        })
    }

    fn marked_tool_result(id: &str, added: &[&str]) -> Message {
        Message::ToolResult {
            tool_call_id: ToolCallId::from(id),
            tool_name: "base_tool".to_string(),
            content: vec![Content::text("done")],
            is_error: false,
            details: None,
            usage: None,
            added_tool_names: added.iter().map(|s| (*s).to_string()).collect(),
            timestamp: 3,
        }
    }

    /// Pi `makeContext(tools, addedToolNames)` — user / assistant toolCall / toolResult / user.
    fn deferred_ctx(tools: Vec<ToolDef>, added: &[&str]) -> Context {
        Context {
            system_prompt: None,
            messages: vec![
                Message::User {
                    content: vec![Content::text("Hello")],
                    timestamp: 1,
                },
                assistant_tool_call("call_1", "base_tool"),
                marked_tool_result("call_1", added),
                Message::User {
                    content: vec![Content::text("Hello")],
                    timestamp: 4,
                },
            ],
            tools,
        }
    }

    fn input_types(body: &Value) -> Vec<&str> {
        body["input"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|i| i.get("type").and_then(Value::as_str).unwrap_or("role-item"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn tool_names(body: &Value) -> Vec<&str> {
        body.get("tools")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.get("name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn loads_an_openai_responses_tool_through_client_tool_search() {
        // Pi "loads an OpenAI Responses tool through client tool search".
        let ctx = deferred_ctx(
            vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
            &["late_tool"],
        );
        let body = build_params(
            &catalog_model("gpt-5.4"),
            &ctx,
            &StreamOptions::default(),
            None,
        );

        // (1) The deferred tool is GONE from `body.tools` — not merely flagged there.
        assert_eq!(tool_names(&body), ["base_tool"]);
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
        assert!(
            body["tools"][0].get("defer_loading").is_none(),
            "an immediate tool must not carry defer_loading"
        );

        // (2) The pair is injected IMMEDIATELY AFTER the function_call_output at that index.
        assert_eq!(
            input_types(&body),
            [
                "role-item",            // user "Hello"
                "function_call",        // assistant tool call
                "function_call_output", // the tool result
                "tool_search_call",     // <- injected here
                "tool_search_output",
                "role-item", // trailing user "Hello"
            ]
        );

        let items = body["input"].as_array().unwrap();
        let call = &items[3];
        let out = &items[4];

        // (3) The exact `tool_search_call` shape, including the hash INPUT: the full toolCallId,
        // a COMMA-joined name list for the hash and a SPACE-joined one for the query.
        assert_eq!(
            *call,
            json!({
                "type": "tool_search_call",
                "call_id": "pi_tool_load_1co5fstaye10s",
                "execution": "client",
                "status": "completed",
                "arguments": { "query": "late_tool", "limit": 1 },
            })
        );
        assert_eq!(
            call["call_id"],
            json!(format!("pi_tool_load_{}", short_hash("call_1:late_tool"))),
        );

        // (4) The exact `tool_search_output` shape: same call id, and the definition carries
        // `defer_loading: true`.
        assert_eq!(
            *out,
            json!({
                "type": "tool_search_output",
                "call_id": "pi_tool_load_1co5fstaye10s",
                "execution": "client",
                "status": "completed",
                "tools": [{
                    "type": "function",
                    "name": "late_tool",
                    "description": "The late_tool tool",
                    "parameters": {
                        "type": "object",
                        "properties": { "value": { "type": "string" } },
                        "required": ["value"],
                    },
                    "defer_loading": true,
                    "strict": false,
                }],
            })
        );

        // (5) Nothing was displaced or lost: the tool result's own output is untouched.
        assert_eq!(
            items[2],
            json!({
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "done",
            })
        );
    }

    #[test]
    fn unsupported_openai_models_use_the_normal_tool_list() {
        // Pi it.each(["gpt-5.2", "gpt-5.4-nano", "gpt-5.5-pro"]) — a version PREFIX is not the
        // gate; the enabled set is an exact id list.
        for id in ["gpt-5.2", "gpt-5.4-nano", "gpt-5.5-pro"] {
            let ctx = deferred_ctx(
                vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
                &["late_tool"],
            );
            let body = build_params(&catalog_model(id), &ctx, &StreamOptions::default(), None);
            assert_eq!(tool_names(&body), ["base_tool", "late_tool"], "model {id}");
            assert!(
                !input_types(&body).contains(&"tool_search_output"),
                "model {id} must not emit a tool_search_output"
            );
            assert!(
                !input_types(&body).contains(&"tool_search_call"),
                "model {id} must not emit a tool_search_call"
            );
        }
    }

    #[test]
    fn explicit_compat_override_wins_in_both_directions() {
        // Pi "uses the normal tool list when OpenAI tool search is explicitly disabled" — an
        // enabled catalog model behind a proxy provider, turned off by hand.
        let mut off = catalog_model("gpt-5.4");
        off.provider = "openai-proxy".into();
        off.compat = Some(crate::api::compat::ModelCompat {
            supports_tool_search: Some(false),
            ..Default::default()
        });
        let ctx = deferred_ctx(
            vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
            &["late_tool"],
        );
        let body = build_params(&off, &ctx, &StreamOptions::default(), None);
        assert_eq!(tool_names(&body), ["base_tool", "late_tool"]);
        assert!(!input_types(&body).contains(&"tool_search_output"));

        // And the override turns it ON for a provider the catalog never enables — the gate is
        // `compat.supportsToolSearch ?? false`, with no provider predicate (Pi
        // openai-responses.ts:74).
        let mut on = model(); // provider "openai", id "gpt-5" — off by default
        let body = build_params(&on, &ctx, &StreamOptions::default(), None);
        assert_eq!(tool_names(&body), ["base_tool", "late_tool"]);
        on.compat = Some(crate::api::compat::ModelCompat {
            supports_tool_search: Some(true),
            ..Default::default()
        });
        let body = build_params(&on, &ctx, &StreamOptions::default(), None);
        assert_eq!(tool_names(&body), ["base_tool"]);
        assert!(input_types(&body).contains(&"tool_search_output"));
    }

    #[test]
    fn an_all_deferred_request_omits_the_tools_key_entirely() {
        // There is NO safety valve on this path (Pi openai-responses.ts:301 guards on
        // `immediate.length > 0`; the promote-everything-back rule is Anthropic-only). The body
        // ships with no `tools` key at all and the definition lives purely in the transcript.
        let ctx = deferred_ctx(vec![deferred_tool("late_tool")], &["late_tool"]);
        let body = build_params(
            &catalog_model("gpt-5.4"),
            &ctx,
            &StreamOptions::default(),
            None,
        );
        assert!(
            body.get("tools").is_none(),
            "expected no `tools` key, got {}",
            body["tools"]
        );
        let items = body["input"].as_array().unwrap();
        assert_eq!(items[4]["tools"][0]["name"], "late_tool");
        assert_eq!(items[4]["tools"][0]["defer_loading"], true);
    }

    #[test]
    fn a_deferred_name_is_loaded_at_most_once_per_request() {
        // `loadedToolNames` is declared once per conversion (Pi openai-responses-shared.ts:143),
        // so a second marker for the same tool anchors nothing.
        let mut ctx = deferred_ctx(
            vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
            &["late_tool"],
        );
        ctx.messages
            .insert(3, assistant_tool_call("call_2", "base_tool"));
        ctx.messages
            .insert(4, marked_tool_result("call_2", &["late_tool"]));
        let body = build_params(
            &catalog_model("gpt-5.4"),
            &ctx,
            &StreamOptions::default(),
            None,
        );
        let types = input_types(&body);
        assert_eq!(
            types.iter().filter(|t| **t == "tool_search_call").count(),
            1
        );
        assert_eq!(
            types.iter().filter(|t| **t == "tool_search_output").count(),
            1
        );
        // Both tool results still emitted their own output — nothing was swallowed.
        assert_eq!(
            types
                .iter()
                .filter(|t| **t == "function_call_output")
                .count(),
            2
        );
    }

    #[test]
    fn the_search_call_id_hashes_the_full_tool_call_id_and_comma_joined_names() {
        // Pi `shortHash(`${msg.toolCallId}:${names.join(",")}`)` — the FULL id, INCLUDING the
        // `|item_id` suffix that `function_call_output.call_id` drops.
        let mut ctx = deferred_ctx(
            vec![
                deferred_tool("base_tool"),
                deferred_tool("late_tool"),
                deferred_tool("later_tool"),
            ],
            &["late_tool", "later_tool"],
        );
        // A same-provider assistant turn so the `|item_id` survives normalization verbatim.
        ctx.messages[1] = Message::Assistant(AssistantMessage {
            content: vec![Content::ToolCall(ToolCall {
                id: ToolCallId::from("call_1|fc_item_1"),
                name: "base_tool".to_string(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
            })],
            provider: "openai".into(),
            model: "gpt-5.4".to_string(),
            api: API_ID.into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 2,
        });
        ctx.messages[2] = marked_tool_result("call_1|fc_item_1", &["late_tool", "later_tool"]);

        let body = build_params(
            &catalog_model("gpt-5.4"),
            &ctx,
            &StreamOptions::default(),
            None,
        );
        let items = body["input"].as_array().unwrap();
        // The output item drops the suffix …
        assert_eq!(items[2]["call_id"], "call_1");
        // … while the search id hashes the id WITH it.
        // Literal, cross-checked against Pi's JS `shortHash` for the exact input
        // `call_1|fc_item_1:late_tool,later_tool` — dropping the `|fc_item_1` suffix or
        // space-joining the names both change this string.
        assert_eq!(items[3]["call_id"], "pi_tool_load_1u3lpgp10pr00m");
        assert_eq!(
            items[3]["call_id"],
            json!(format!(
                "pi_tool_load_{}",
                short_hash("call_1|fc_item_1:late_tool,later_tool")
            ))
        );
        // Comma for the hash, SPACE for the query; `limit` is the count.
        assert_eq!(items[3]["arguments"]["query"], "late_tool later_tool");
        assert_eq!(items[3]["arguments"]["limit"], 2);
        assert_eq!(items[4]["tools"][0]["name"], "late_tool");
        assert_eq!(items[4]["tools"][1]["name"], "later_tool");
    }

    #[test]
    fn a_marked_name_absent_from_context_tools_anchors_nothing() {
        let ctx = deferred_ctx(vec![deferred_tool("base_tool")], &["ghost_tool"]);
        let body = build_params(
            &catalog_model("gpt-5.4"),
            &ctx,
            &StreamOptions::default(),
            None,
        );
        assert_eq!(tool_names(&body), ["base_tool"]);
        assert!(!input_types(&body).contains(&"tool_search_call"));
    }

    #[test]
    fn a_tool_used_before_its_marker_stays_in_the_prefix() {
        // The model already called it, so hiding it from the prefix would be a lie about what it
        // could see (`splitDeferredTools` suppression rule).
        let mut ctx = deferred_ctx(
            vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
            &["late_tool"],
        );
        ctx.messages[1] = assistant_tool_call("call_1", "late_tool");
        let body = build_params(
            &catalog_model("gpt-5.4"),
            &ctx,
            &StreamOptions::default(),
            None,
        );
        assert_eq!(tool_names(&body), ["base_tool", "late_tool"]);
        assert!(!input_types(&body).contains(&"tool_search_output"));
    }

    #[test]
    fn tool_search_is_off_for_every_openai_responses_model_but_the_seven() {
        // CONSTRAINT: default OFF is load-bearing. Turning this on for a model that has never seen
        // `tool_search_*` items changes its wire payload and would 400. Proven over the REAL
        // embedded catalogs, every provider, not a fixture.
        //
        // Pi's enabled set is baked into the generated catalog by
        // `ai/scripts/generate-models.ts:731-738` against `OPENAI_TOOL_SEARCH_MODEL_IDS` (:324-332);
        // cyrup carries the same data as `compat.supportsToolSearch` in
        // `providers/catalog/openai.json`. `openai-codex` contributes nothing — cyrup does not port
        // `openai-codex-responses`.
        const ENABLED: [&str; 7] = [
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.4-pro",
            "gpt-5.5",
            "gpt-5.6-luna",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
        ];

        let mut on: Vec<(String, String)> = Vec::new();
        let mut total = 0usize;
        for provider in crate::providers::all::all_providers() {
            for m in provider.models() {
                if m.api.as_str() != API_ID {
                    continue;
                }
                total += 1;
                if get_responses_compat(m).supports_tool_search {
                    on.push((
                        provider.id().as_str().to_string(),
                        m.id.as_str().to_string(),
                    ));
                }
            }
        }

        assert!(
            total > 70,
            "expected the real catalogs to carry many openai-responses models, saw {total}"
        );
        let mut got: Vec<String> = on
            .iter()
            .filter(|(p, _)| p == "openai")
            .map(|(_, id)| id.clone())
            .collect();
        got.sort();
        let mut want: Vec<String> = ENABLED.iter().map(|s| (*s).to_string()).collect();
        want.sort();
        assert_eq!(got, want, "enabled set drifted from Pi's id list");
        // No OTHER provider may enable it — every reseller shipping these ids stays off.
        assert!(
            on.iter().all(|(p, _)| p == "openai"),
            "tool search leaked to non-openai providers: {on:?}"
        );
    }

    #[test]
    fn a_disabled_model_emits_a_byte_identical_body_to_the_pre_drift_shape() {
        // The regression guard for constraint 3: with the flag off, the split is a pass-through and
        // `body.tools` is exactly `convert_responses_tools(ctx.tools, false)` — no `defer_loading`
        // key anywhere in the payload.
        let ctx = deferred_ctx(
            vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
            &["late_tool"],
        );
        let body = build_params(&model(), &ctx, &StreamOptions::default(), None);
        assert_eq!(
            body["tools"],
            Value::Array(convert_responses_tools(&ctx.tools, false))
        );
        assert!(
            !serde_json::to_string(&body)
                .unwrap()
                .contains("defer_loading"),
            "a disabled model must not mention defer_loading: {body}"
        );
    }
}

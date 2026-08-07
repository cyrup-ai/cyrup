//! The `google-generative-ai` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking the Gemini Generative Language streaming API
//! (`POST {baseUrl}/models/{model}:streamGenerateContent?alt=sse`, newline-delimited
//! `GenerateContentResponse` JSON over SSE). Shared by the `google` provider and the
//! `opencode` provider's google-tagged models. Pure JSON-over-SSE — no SDK, no new dependency.
//!
//! 1:1 port of Pi's `api/google-generative-ai.ts` + `api/google-shared.ts`: the `Content[]`
//! encoder (`convertMessages`), `convertTools` (`parametersJsonSchema`), the Gemini-3 / Gemma-4
//! thinking-level vs token-budget split, `thoughtSignature` retention (`isThinkingPart` /
//! `retainThoughtSignature` / base64 validation), unique tool-call-id synthesis, and the
//! `candidate.content.parts` streaming decoder.
//!
//! Wire JSON uses Google's own field names (camelCase: `functionCall`, `thoughtSignature`,
//! `maxOutputTokens`, `thinkingConfig`), NOT cyrup's serde camelCase convention.

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
use crate::stream::{StreamEvent, StreamOptions, ToolChoice};
use crate::usage::compute_cost;
use crate::utils::json_parse::parse_json_with_repair;
use crate::utils::provider_retry::ProviderRetry;
use crate::utils::simple_options::ThinkingBudgets;
use cyrup_core::{
    ApiId, AssistantMessage, CancelToken, Content, Message, ModelThinkingLevel, StopReason,
    ThinkingLevel, ToolCall, ToolCallId, Usage,
};
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::GOOGLE_GENERATIVE_AI;

/// Monotonic counter for synthesizing unique tool-call ids (Pi `toolCallCounter`,
/// google-generative-ai.ts:47).
static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The `ApiImpl` for `"google-generative-ai"`.
pub struct GoogleGenerativeAiApi {
    api: ApiId,
}

impl Default for GoogleGenerativeAiApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl GoogleGenerativeAiApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(GoogleGenerativeAiApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for GoogleGenerativeAiApi {
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

        // gap-08 #2: `before_provider_request` may inspect/replace the outbound body.
        let body = crate::stream::apply_on_payload(opts, model, build_params(model, ctx, opts)).await;
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

/// Resolve the `POST` target (Pi `createClient` httpOptions.baseUrl, google-generative-ai.ts:326).
/// An auth base-url override wins over `model.base_url`. The endpoint is
/// `{base}/models/{model}:streamGenerateContent?alt=sse`.
fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
    Some(stream_url(base, model.id.as_str()))
}

/// Normalize a base URL to the streaming-generate endpoint.
pub(crate) fn stream_url(base: &str, model_id: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    format!("{trimmed}/models/{model_id}:streamGenerateContent?alt=sse")
}

/// Build the request headers (Pi `createClient`, google-generative-ai.ts:321-340). The Gemini REST
/// API authenticates with the `x-goog-api-key` header. The model/opts header overlays layer last (a
/// `None` value suppresses a default).
pub(crate) fn build_headers(model: &Model, opts: &StreamOptions, api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type".to_string(),
        Some("application/json".to_string()),
    );
    headers.insert("x-goog-api-key".to_string(), Some(api_key.to_string()));

    // model.headers < opts.headers (a `None` suppresses a default — Pi `providerHeadersToRecord`,
    // google-generative-ai.ts:331).
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
    headers
}

/// A Gemini `thinkingLevel` value (Pi `GoogleThinkingLevel`, google-shared.ts:16). Serialized to the
/// exact wire string Pi passes through unchanged in `buildParams` (`options.thinking.level as any`,
/// google-generative-ai.ts:377-378).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoogleThinkingLevel {
    /// `"THINKING_LEVEL_UNSPECIFIED"`.
    Unspecified,
    /// `"MINIMAL"`.
    Minimal,
    /// `"LOW"`.
    Low,
    /// `"MEDIUM"`.
    Medium,
    /// `"HIGH"`.
    High,
}

impl GoogleThinkingLevel {
    /// The exact `thinkingLevel` wire string.
    pub fn as_wire(self) -> &'static str {
        match self {
            GoogleThinkingLevel::Unspecified => "THINKING_LEVEL_UNSPECIFIED",
            GoogleThinkingLevel::Minimal => "MINIMAL",
            GoogleThinkingLevel::Low => "LOW",
            GoogleThinkingLevel::Medium => "MEDIUM",
            GoogleThinkingLevel::High => "HIGH",
        }
    }
}

/// A direct per-request `thinking` override (Pi `GoogleOptions.thinking`,
/// google-generative-ai.ts:40-44). When present it is read verbatim by [`build_params`], bypassing
/// the unified-`reasoning`-driven lowering — mirroring Pi's `buildParams` reading `options.thinking`
/// directly (google-generative-ai.ts:373-384) rather than the value `streamSimple` would compute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GoogleThinking {
    /// `thinking.enabled` (google-generative-ai.ts:41). `false` lowers to the model's
    /// disabled-thinking config.
    pub enabled: bool,
    /// `thinking.budgetTokens` (google-generative-ai.ts:42): `-1` for dynamic, `0` to disable. Only
    /// honored when `level` is `None` (Pi prefers `level` over `budgetTokens`).
    pub budget_tokens: Option<i64>,
    /// `thinking.level` (google-generative-ai.ts:43). Takes precedence over `budget_tokens`.
    pub level: Option<GoogleThinkingLevel>,
}

/// Per-API typed options for the `google-generative-ai` wire protocol (Pi `GoogleOptions`,
/// google-generative-ai.ts:38-45). Only the fields cyrup does not already carry on the unified
/// [`StreamOptions`](crate::StreamOptions) live here: `toolChoice` folds onto
/// `StreamOptions.tool_choice` and the simple reasoning level onto `StreamOptions.reasoning`, but a
/// direct `thinking.{budgetTokens,level}` per-request override has no other home. Carried via
/// [`StreamOptions::api_options`](crate::StreamOptions::api_options); defaults to `None` (no
/// override), reproducing the streamSimple-driven behavior exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GoogleOptions {
    /// Direct `thinkingConfig` override (Pi `GoogleOptions.thinking`). `None` = no override: the
    /// unified `reasoning` level drives `thinkingConfig` as before.
    pub thinking: Option<GoogleThinking>,
}

/// Test-only convenience wrapper for [`build_params`].
#[cfg(test)]
pub(crate) fn build_body(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    build_params(model, ctx, opts)
}

/// Build the `:streamGenerateContent` request body (1:1 port of Pi `buildParams` + the `streamSimple`
/// thinking lowering, google-generative-ai.ts:283-400). The unified `opts.reasoning` level drives the
/// `thinkingConfig` (level-based for Gemini 3 / Gemma 4, token-budget-based otherwise).
pub(crate) fn build_params(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    let contents = convert_messages(model, ctx);

    let mut generation_config = Map::new();
    if let Some(temp) = opts.temperature {
        generation_config.insert("temperature".to_string(), json!(temp));
    }
    if let Some(max) = opts.max_tokens {
        generation_config.insert("maxOutputTokens".to_string(), json!(max));
    }

    // Thinking lowering (Pi `streamSimple`, google-generative-ai.ts:283-319). cyrup carries the
    // unified `reasoning` level directly, so the lowering happens inline (as in `anthropic_messages`).
    // A direct `GoogleOptions.thinking` per-request override (Pi `buildParams` reading
    // `options.thinking`, google-generative-ai.ts:373-384) bypasses that lowering and is read verbatim.
    if model.reasoning {
        let cfg = match opts.google_options().and_then(|g| g.thinking) {
            Some(thinking) => thinking_config_override(model, &thinking),
            None => thinking_config(model, opts.reasoning),
        };
        if let Some(cfg) = cfg {
            generation_config.insert("thinkingConfig".to_string(), cfg);
        }
    }

    let mut obj = Map::new();
    obj.insert("contents".to_string(), Value::Array(contents));

    // systemInstruction (Pi google-generative-ai.ts:359).
    if let Some(sp) = &ctx.system_prompt {
        obj.insert(
            "systemInstruction".to_string(),
            json!({ "parts": [{ "text": sanitize_surrogates(sp) }] }),
        );
    }

    // tools + toolConfig (Pi google-generative-ai.ts:360-371).
    if !ctx.tools.is_empty() {
        if let Some(tools) = convert_tools(&ctx.tools) {
            obj.insert("tools".to_string(), tools);
        }
        if let Some(tc) = &opts.tool_choice {
            obj.insert(
                "toolConfig".to_string(),
                json!({ "functionCallingConfig": { "mode": map_tool_choice(tc) } }),
            );
        }
    }

    if !generation_config.is_empty() {
        obj.insert(
            "generationConfig".to_string(),
            Value::Object(generation_config),
        );
    }

    Value::Object(obj)
}

/// Build `thinkingConfig` (Pi `buildParams` thinking branch + `streamSimple`,
/// google-generative-ai.ts:373-384,294-318). `None` omits the field entirely.
fn thinking_config(model: &Model, reasoning: ModelThinkingLevel) -> Option<Value> {
    if !reasoning.is_on() {
        // streamSimple `!options.reasoning` path → `thinking: { enabled: false }`, which lowers to
        // the model's disabled-thinking config (google-generative-ai.ts:294-296,382-384).
        return Some(disabled_thinking_config(model));
    }

    // streamSimple reasoning path: clamp to a supported level, then `off → high`
    // (google-generative-ai.ts:298-299).
    let clamped = clamp_thinking_level(model, reasoning);
    let effort = clamped.level().unwrap_or(ThinkingLevel::High);

    let mut cfg = Map::new();
    cfg.insert("includeThoughts".to_string(), json!(true));

    if is_gemini3_pro(model) || is_gemini3_flash(model) || is_gemma4(model) {
        if let Some(level) = thinking_level(effort, model) {
            cfg.insert("thinkingLevel".to_string(), json!(level));
        }
    } else if let Some(budget) = google_budget(model, effort, None) {
        cfg.insert("thinkingBudget".to_string(), json!(budget));
    }
    Some(Value::Object(cfg))
}

/// Lower a direct `GoogleOptions.thinking` override to `thinkingConfig` (1:1 with Pi `buildParams`,
/// google-generative-ai.ts:373-384). When `enabled`, `level` wins over `budgetTokens`; otherwise the
/// model's disabled-thinking config. The outer `model.reasoning` guard is applied by the caller,
/// mirroring Pi's `options.thinking?.enabled && model.reasoning` / `model.reasoning && … !enabled`.
fn thinking_config_override(model: &Model, thinking: &GoogleThinking) -> Option<Value> {
    if thinking.enabled {
        let mut cfg = Map::new();
        cfg.insert("includeThoughts".to_string(), json!(true));
        if let Some(level) = thinking.level {
            cfg.insert("thinkingLevel".to_string(), json!(level.as_wire()));
        } else if let Some(budget) = thinking.budget_tokens {
            cfg.insert("thinkingBudget".to_string(), json!(budget));
        }
        Some(Value::Object(cfg))
    } else {
        Some(disabled_thinking_config(model))
    }
}

/// The disabled-thinking config for a reasoning model (Pi `getDisabledThinkingConfig`,
/// google-generative-ai.ts:417-433).
fn disabled_thinking_config(model: &Model) -> Value {
    if is_gemini3_pro(model) {
        json!({ "thinkingLevel": "LOW" })
    } else if is_gemini3_flash(model) || is_gemma4(model) {
        // Gemini 3 Flash / Flash-Lite and Gemma 4 use the lowest level (Pi: MINIMAL).
        json!({ "thinkingLevel": "MINIMAL" })
    } else {
        // Gemini 2.x supports disabling via thinkingBudget = 0.
        json!({ "thinkingBudget": 0 })
    }
}

/// The Gemini-3 `thinkingLevel` for a clamped effort (Pi `getThinkingLevel`,
/// google-generative-ai.ts:435-466). `None` when the effort has no mapping (e.g. `xhigh` on a
/// Gemini-3-Pro model — Pi's switch returns `undefined`).
fn thinking_level(effort: ThinkingLevel, model: &Model) -> Option<&'static str> {
    if is_gemini3_pro(model) {
        return match effort {
            ThinkingLevel::Minimal | ThinkingLevel::Low => Some("LOW"),
            ThinkingLevel::Medium | ThinkingLevel::High => Some("HIGH"),
            ThinkingLevel::Xhigh | ThinkingLevel::Max => None,
        };
    }
    if is_gemma4(model) {
        return match effort {
            ThinkingLevel::Minimal | ThinkingLevel::Low => Some("MINIMAL"),
            ThinkingLevel::Medium | ThinkingLevel::High => Some("HIGH"),
            ThinkingLevel::Xhigh | ThinkingLevel::Max => None,
        };
    }
    match effort {
        ThinkingLevel::Minimal => Some("MINIMAL"),
        ThinkingLevel::Low => Some("LOW"),
        ThinkingLevel::Medium => Some("MEDIUM"),
        ThinkingLevel::High => Some("HIGH"),
        // Pi types the parameter `ClampedThinkingLevel = Exclude<ThinkingLevel, "xhigh"|"max">`
        // (google-generative-ai.ts:410), so both fall off every switch as `undefined`.
        ThinkingLevel::Xhigh | ThinkingLevel::Max => None,
    }
}

/// The token thinking-budget for a clamped effort (Pi `getGoogleBudget`,
/// google-generative-ai.ts:468-508). `None` when the model/effort pair has no budget (Pi returns
/// `undefined`, so the field is omitted); a non-Gemini-2.5 model returns `Some(-1)` (dynamic).
fn google_budget(
    model: &Model,
    effort: ThinkingLevel,
    custom: Option<&ThinkingBudgets>,
) -> Option<i64> {
    if let Some(c) = custom {
        let v = match effort {
            ThinkingLevel::Minimal => c.minimal,
            ThinkingLevel::Low => c.low,
            ThinkingLevel::Medium => c.medium,
            ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => c.high,
        };
        if let Some(v) = v {
            return Some(v as i64);
        }
    }

    let id = model.id.as_str();
    let table: Option<[i64; 4]> = if id.contains("2.5-pro") {
        Some([128, 2048, 8192, 32768])
    } else if id.contains("2.5-flash-lite") {
        Some([512, 2048, 8192, 24576])
    } else if id.contains("2.5-flash") {
        Some([128, 2048, 8192, 24576])
    } else {
        None
    };
    match table {
        Some([minimal, low, medium, high]) => match effort {
            ThinkingLevel::Minimal => Some(minimal),
            ThinkingLevel::Low => Some(low),
            ThinkingLevel::Medium => Some(medium),
            ThinkingLevel::High => Some(high),
            // Pi `budgets[xhigh]` / `budgets[max]` are `undefined` → omit.
            ThinkingLevel::Xhigh | ThinkingLevel::Max => None,
        },
        None => Some(-1),
    }
}

/// `/gemma-?4/` (Pi `isGemma4Model`, google-generative-ai.ts:404-406).
fn is_gemma4(model: &Model) -> bool {
    let id = model.id.as_str().to_lowercase();
    id.contains("gemma4") || id.contains("gemma-4")
}

/// `/gemini-3(?:\.\d+)?-pro/` (Pi `isGemini3ProModel`, google-generative-ai.ts:408-410).
fn is_gemini3_pro(model: &Model) -> bool {
    gemini3_variant(&model.id.as_str().to_lowercase(), "-pro")
}

/// `/gemini-3(?:\.\d+)?-flash/` or the two `*-latest` aliases (Pi `isGemini3FlashModel`,
/// google-generative-ai.ts:412-415).
fn is_gemini3_flash(model: &Model) -> bool {
    let id = model.id.as_str().to_lowercase();
    gemini3_variant(&id, "-flash")
        || id == "gemini-flash-latest"
        || id == "gemini-flash-lite-latest"
}

/// Match `gemini-3` optionally followed by `.<digits>`, then `suffix` (replicates the
/// `/gemini-3(?:\.\d+)?<suffix>/` regexes without a regex dependency).
fn gemini3_variant(id: &str, suffix: &str) -> bool {
    let needle = "gemini-3";
    let mut from = 0;
    while let Some(pos) = id[from..].find(needle) {
        let abs = from + pos;
        let rest = &id[abs + needle.len()..];
        let after_version = if let Some(stripped) = rest.strip_prefix('.') {
            let digits = stripped.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 {
                rest // `.` not followed by a digit → optional group does not match
            } else {
                &stripped[digits..]
            }
        } else {
            rest
        };
        if after_version.starts_with(suffix) {
            return true;
        }
        from = abs + 1;
    }
    false
}

/// `modelId.startsWith("claude-") || modelId.startsWith("gpt-oss-")` (Pi `requiresToolCallId`,
/// google-shared.ts:70-72).
fn requires_tool_call_id(model_id: &str) -> bool {
    model_id.starts_with("claude-") || model_id.starts_with("gpt-oss-")
}

/// `getGeminiMajorVersion >= 3` (Pi `supportsMultimodalFunctionResponse`, google-shared.ts:74-86).
/// A non-Gemini id (no major version) returns `true`.
fn supports_multimodal_function_response(model_id: &str) -> bool {
    match gemini_major_version(model_id) {
        Some(v) => v >= 3,
        None => true,
    }
}

/// `/^gemini(?:-live)?-(\d+)/` (Pi `getGeminiMajorVersion`, google-shared.ts:74-78).
fn gemini_major_version(model_id: &str) -> Option<u32> {
    let id = model_id.to_lowercase();
    let rest = id.strip_prefix("gemini")?;
    let rest = rest.strip_prefix("-live").unwrap_or(rest);
    let rest = rest.strip_prefix('-')?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Thought signatures must be valid base64 (`TYPE_BYTES`) — Pi `isValidThoughtSignature`,
/// google-shared.ts:52-58.
fn is_valid_thought_signature(sig: &str) -> bool {
    if sig.is_empty() || !sig.len().is_multiple_of(4) {
        return false;
    }
    let body = sig.trim_end_matches('=');
    // At most two `=` padding chars (validated by the length-mod-4 rule above).
    sig.len() - body.len() <= 2
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

/// Keep a signature only for the same provider/model and valid base64 (Pi `resolveThoughtSignature`,
/// google-shared.ts:60-65).
fn resolve_thought_signature(same: bool, sig: Option<&str>) -> Option<String> {
    match sig {
        Some(s) if same && is_valid_thought_signature(s) => Some(s.to_string()),
        _ => None,
    }
}

/// The Gemini tool-call-id normalizer (Pi `convertMessages` `normalizeToolCallId`,
/// google-shared.ts:93-96).
fn normalize_tool_call_id(model_id: &str, id: &str) -> String {
    if !requires_tool_call_id(model_id) {
        return id.to_string();
    }
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// Convert cyrup [`Message`]s to Gemini `Content[]` (1:1 port of Pi `convertMessages`,
/// google-shared.ts:91-235).
pub(crate) fn convert_messages(model: &Model, ctx: &Context) -> Vec<Value> {
    let model_id = model.id.as_str().to_string();
    let transformed = transform_messages_with(&ctx.messages, model, |id| {
        normalize_tool_call_id(&model_id, id)
    });

    let supports_image = model.input.contains(&Modality::Image);
    let multimodal_fr = supports_multimodal_function_response(&model_id);
    let include_id = requires_tool_call_id(&model_id);

    let mut contents: Vec<Value> = Vec::new();

    for msg in &transformed {
        match msg {
            Message::User { content, .. } => {
                let parts = user_parts(content);
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({ "role": "user", "parts": parts }));
            }
            Message::Assistant(am) => {
                let same = am.provider == model.provider && am.model == model_id;
                let parts = assistant_parts(am, same);
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            Message::ToolResult {
                tool_name,
                tool_call_id,
                content,
                is_error,
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
                let image_parts: Vec<Value> = if supports_image {
                    content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Image { data, mime_type } => Some(json!({
                                "inlineData": { "mimeType": mime_type, "data": data }
                            })),
                            _ => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                let has_text = !text_result.is_empty();
                let has_images = !image_parts.is_empty();
                let response_value = if has_text {
                    sanitize_surrogates(&text_result)
                } else if has_images {
                    "(see attached image)".to_string()
                } else {
                    String::new()
                };

                let mut response = Map::new();
                if *is_error {
                    response.insert("error".to_string(), json!(response_value));
                } else {
                    response.insert("output".to_string(), json!(response_value));
                }

                let mut fr = Map::new();
                fr.insert("name".to_string(), json!(tool_name));
                fr.insert("response".to_string(), Value::Object(response));
                if has_images && multimodal_fr {
                    fr.insert("parts".to_string(), Value::Array(image_parts.clone()));
                }
                if include_id {
                    fr.insert("id".to_string(), json!(tool_call_id.as_str()));
                }
                let function_response_part = json!({ "functionResponse": Value::Object(fr) });

                // Merge consecutive function responses into one user turn (Pi google-shared.ts:214-222).
                if let Some(last) = contents.last_mut()
                    && last.get("role").and_then(Value::as_str) == Some("user")
                    && last
                        .get("parts")
                        .and_then(Value::as_array)
                        .map(|p| p.iter().any(|x| x.get("functionResponse").is_some()))
                        .unwrap_or(false)
                    && let Some(Value::Array(parts)) = last.get_mut("parts")
                {
                    parts.push(function_response_part);
                } else {
                    contents.push(json!({ "role": "user", "parts": [function_response_part] }));
                }

                // Gemini < 3: images go in a separate user turn (Pi google-shared.ts:225-230).
                if has_images && !multimodal_fr {
                    let mut parts = vec![json!({ "text": "Tool result image:" })];
                    parts.extend(image_parts);
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            }
        }
    }

    contents
}

/// Build the `parts` for a user turn (Pi google-shared.ts:101-125).
fn user_parts(content: &[Content]) -> Vec<Value> {
    content
        .iter()
        .filter_map(|item| match item {
            Content::Text { text, .. } => Some(json!({ "text": sanitize_surrogates(text) })),
            Content::Image { data, mime_type } => {
                Some(json!({ "inlineData": { "mimeType": mime_type, "data": data } }))
            }
            _ => None,
        })
        .collect()
}

/// Build the `parts` for an assistant (`model`) turn (Pi google-shared.ts:127-182).
///
/// Empty text/thinking blocks are dropped only when they carry no usable thought signature
/// (Pi 6138f5a0, google-shared.ts:134-151); the cross-provider `else` branch keeps the old
/// unconditional skip because the signature is unusable there (google-shared.ts:157-162).
fn assistant_parts(am: &AssistantMessage, same: bool) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    for block in &am.content {
        match block {
            Content::Text {
                text,
                text_signature,
            } => {
                let sig = resolve_thought_signature(same, text_signature.as_deref());
                // Skip empty text blocks — unless they carry a thought signature. Gemini can
                // attach the signature to a part whose visible text is empty and requires it
                // echoed back; dropping it breaks the reasoning chain and the model
                // intermittently ends mid-task turns with a thought-only STOP (empty
                // completion, no tool call). (Pi google-shared.ts:134-139.)
                if text.trim().is_empty() && sig.is_none() {
                    continue;
                }
                let mut o = Map::new();
                o.insert("text".to_string(), json!(sanitize_surrogates(text)));
                if let Some(s) = sig {
                    o.insert("thoughtSignature".to_string(), json!(s));
                }
                parts.push(Value::Object(o));
            }
            Content::Thinking {
                thinking,
                thinking_signature,
                ..
            } => {
                // Only keep as thinking block if same provider AND same model; otherwise
                // convert to plain text (no tags to avoid model mimicking them).
                if same {
                    let sig = resolve_thought_signature(same, thinking_signature.as_deref());
                    // Same rule as text blocks: an empty thinking block is dropped only when it
                    // carries no signature (Pi google-shared.ts:148-151).
                    if thinking.trim().is_empty() && sig.is_none() {
                        continue;
                    }
                    let mut o = Map::new();
                    o.insert("thought".to_string(), json!(true));
                    o.insert("text".to_string(), json!(sanitize_surrogates(thinking)));
                    if let Some(s) = sig {
                        o.insert("thoughtSignature".to_string(), json!(s));
                    }
                    parts.push(Value::Object(o));
                } else {
                    // Cross-provider/model: the signature is unusable, empty blocks stay
                    // dropped unconditionally (Pi google-shared.ts:157-162).
                    if thinking.trim().is_empty() {
                        continue;
                    }
                    // Convert to plain text (no tags) for a different provider/model.
                    parts.push(json!({ "text": sanitize_surrogates(thinking) }));
                }
            }
            Content::ToolCall(tc) => {
                let sig = resolve_thought_signature(same, tc.thought_signature.as_deref());
                let mut fc = Map::new();
                fc.insert("name".to_string(), json!(tc.name));
                fc.insert("args".to_string(), Value::Object(tc.arguments.clone()));
                if requires_tool_call_id(am.model.as_str()) {
                    fc.insert("id".to_string(), json!(tc.id.as_str()));
                }
                let mut o = Map::new();
                o.insert("functionCall".to_string(), Value::Object(fc));
                if let Some(s) = sig {
                    o.insert("thoughtSignature".to_string(), json!(s));
                }
                parts.push(Value::Object(o));
            }
            _ => {}
        }
    }
    parts
}

/// Convert tools to Gemini `functionDeclarations` (Pi `convertTools`, google-shared.ts:272-288).
/// Uses `parametersJsonSchema` (full JSON Schema). `None` when there are no tools.
pub(crate) fn convert_tools(tools: &[ToolDef]) -> Option<Value> {
    if tools.is_empty() {
        return None;
    }
    let decls: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "parametersJsonSchema": t.parameters,
            })
        })
        .collect();
    Some(json!([{ "functionDeclarations": decls }]))
}

/// Map a tool-choice to a Gemini `functionCallingConfig.mode` (Pi `mapToolChoice`,
/// google-shared.ts:293-304). cyrup's [`ToolChoice`] maps `Auto/None/Required→Function?` onto
/// `AUTO/NONE/ANY`; a named-function choice constrains to `ANY` (Gemini has no per-name mode).
fn map_tool_choice(tc: &ToolChoice) -> &'static str {
    match tc {
        ToolChoice::None => "NONE",
        ToolChoice::Required | ToolChoice::Function { .. } => "ANY",
        ToolChoice::Auto => "AUTO",
    }
}

/// Map a raw Gemini `finishReason` to `(stop_reason, error_message)` (Pi `mapStopReason`,
/// google-shared.ts:309-336 — only `STOP`/`MAX_TOKENS` are non-error).
///
/// The message half is the point. Gemini's characteristic failures are all finish reasons rather
/// than HTTP errors — `SAFETY`, `RECITATION`, `PROHIBITED_CONTENT`, `BLOCKLIST`,
/// `MALFORMED_FUNCTION_CALL` — and this used to discard the raw string, so every one of them
/// surfaced as the identical, information-free "An unknown error occurred". A content-policy
/// refusal and a tool-schema bug demand completely different responses from the user, and the
/// message carried nothing to tell them apart.
///
/// pi keeps the raw value on `output.rawStopReason` (`google-generative-ai.ts:214-216`) and builds
/// the terminal error as ``output.rawStopReason ? `Provider stopped with: ${output.rawStopReason}`
/// : "An unknown error occurred"`` (`:269-273`), so the reason names itself.
fn map_stop_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "STOP" => (StopReason::Stop, None),
        "MAX_TOKENS" => (StopReason::Length, None),
        other => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
    }
}

// ---------------------------------------------------------------------------
// Response decoding
// ---------------------------------------------------------------------------

/// The in-progress text/thinking block being accumulated (Pi `currentBlock`,
/// google-generative-ai.ts:89).
#[derive(Clone, Copy, PartialEq, Eq)]
enum CurrentKind {
    Text,
    Thinking,
}

/// Streaming-decode state (mirrors Pi's `output` accumulation, google-generative-ai.ts:57-264).
#[derive(Default)]
struct Decoder {
    blocks: Vec<Content>,
    current: Option<CurrentKind>,
    usage: Usage,
    response_id: Option<String>,
    /// The settled stop reason, or `None` while none has been delivered — cyrup's spelling of Pi's
    /// `output.stopReason = "pending"` seed (google-generative-ai.ts:73), which is where the
    /// `Default` below now starts. Gemini only sets this from a candidate's `finishReason`, so
    /// `None` at EOF means the stream was TRUNCATED. It previously seeded `Stop` (on a misreading of
    /// upstream, which seeds `"pending"`, not `"stop"`), which is what let a truncated Gemini stream
    /// be transcribed as a cleanly completed turn (PROV-010).
    stop_reason: Option<StopReason>,
    error_message: Option<String>,
}

impl Decoder {
    /// Build the live `partial` snapshot. Usage cost is computed without overwriting the
    /// API-reported `total_tokens` (Pi `calculateCost` fills only `usage.cost`,
    /// google-generative-ai.ts:234).
    fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone();
        usage.cost = compute_cost(&model.cost, &usage);
        AssistantMessage {
            content: self.blocks.clone(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: self.response_id.clone(),
            diagnostics: None,
            usage,
            // Pi's live `partial` carries the raw `output.stopReason`, i.e. `"pending"` until a
            // `finishReason` lands (google-generative-ai.ts:73,229). The TERMINAL event never takes
            // this value — it goes through `StreamEvent::end_of_stream`, which routes
            // `None`/`Pending` to the `error` terminal.
            stop_reason: self.stop_reason.unwrap_or(StopReason::Pending),
            error_message: self.error_message.clone(),
            timestamp: now_millis(),
        }
    }

    fn block_index(&self) -> usize {
        self.blocks.len().saturating_sub(1)
    }
}

/// Drive the Gemini SSE frame stream into ordered [`StreamEvent`]s (1:1 with Pi's stream loop,
/// google-generative-ai.ts:88-265).
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
        if data.is_empty() {
            continue;
        }
        let Some(chunk) = parse_json_with_repair(data) else {
            emit_error(
                &dec,
                model,
                api,
                sink,
                "Could not parse Gemini SSE chunk".to_string(),
            )
            .await;
            return;
        };
        if !process_chunk(&chunk, &mut dec, model, api, sink).await {
            return; // consumer dropped
        }
    }

    // Close a trailing in-progress block (Pi google-generative-ai.ts:238-254).
    if !close_current(&mut dec, model, api, sink).await {
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

    // No candidate ever carried a `finishReason` → the stream was TRUNCATED. Pi throws
    // "Google stream ended without a finish reason" (google-generative-ai.ts:266-268); this used to
    // fall through to the `Stop` seed and report a clean turn.
    sink.send(StreamEvent::end_of_stream(
        dec.snapshot(model, api),
        dec.stop_reason,
        "Google stream ended without a finish reason",
    ))
    .await;
}

/// Process one decoded `GenerateContentResponse` chunk. Returns `false` if the consumer dropped.
async fn process_chunk(
    chunk: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    if dec.response_id.is_none()
        && let Some(id) = chunk.get("responseId").and_then(Value::as_str)
        && !id.is_empty()
    {
        dec.response_id = Some(id.to_string());
    }

    let candidate = chunk.get("candidates").and_then(|c| c.get(0));
    if let Some(candidate) = candidate
        && let Some(parts) = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
    {
        for part in parts {
            if part.get("text").and_then(Value::as_str).is_some()
                && !process_text_part(part, dec, model, api, sink).await
            {
                return false;
            }
            if part.get("functionCall").is_some()
                && !process_function_call(part, dec, model, api, sink).await
            {
                return false;
            }
        }
    }

    if let Some(reason) = candidate
        .and_then(|c| c.get("finishReason"))
        .and_then(Value::as_str)
    {
        let (stop, err) = map_stop_reason(reason);
        dec.stop_reason = Some(stop);
        if let Some(err) = err {
            dec.error_message = Some(err);
        }
        if dec.blocks.iter().any(|b| matches!(b, Content::ToolCall(_))) {
            // A tool call present alongside a non-STOP reason is still a tool-use turn; clear the
            // diagnostic with it so a successful turn never carries a stale error message.
            dec.stop_reason = Some(StopReason::ToolUse);
            dec.error_message = None;
        }
    }

    if let Some(meta) = chunk.get("usageMetadata") {
        apply_usage(&mut dec.usage, meta);
    }

    true
}

/// Handle a text (or thinking) part (Pi google-generative-ai.ts:99-158).
async fn process_text_part(
    part: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let text = part.get("text").and_then(Value::as_str).unwrap_or("");
    let signature = part.get("thoughtSignature").and_then(Value::as_str);
    let is_thinking = part.get("thought").and_then(Value::as_bool) == Some(true);
    let want = if is_thinking {
        CurrentKind::Thinking
    } else {
        CurrentKind::Text
    };

    // Transition: close the current block + open a new one when the kind changes.
    if dec.current != Some(want) {
        if !close_current(dec, model, api, sink).await {
            return false;
        }
        if is_thinking {
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
        } else {
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
    }

    let idx = dec.block_index();
    match dec.blocks.get_mut(idx) {
        Some(Content::Thinking {
            thinking,
            thinking_signature,
            ..
        }) => {
            thinking.push_str(text);
            if let Some(s) = retain_signature(thinking_signature.as_deref(), signature) {
                *thinking_signature = Some(s);
            }
            let partial = dec.snapshot(model, api);
            sink.send(StreamEvent::ThinkingDelta {
                content_index: idx,
                delta: text.to_string(),
                partial,
            })
            .await
        }
        Some(Content::Text {
            text: acc,
            text_signature,
        }) => {
            acc.push_str(text);
            if let Some(s) = retain_signature(text_signature.as_deref(), signature) {
                *text_signature = Some(s);
            }
            let partial = dec.snapshot(model, api);
            sink.send(StreamEvent::TextDelta {
                content_index: idx,
                delta: text.to_string(),
                partial,
            })
            .await
        }
        _ => true,
    }
}

/// Handle a function-call part (Pi google-generative-ai.ts:160-205).
async fn process_function_call(
    part: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    // Close any open text/thinking block first.
    if !close_current(dec, model, api, sink).await {
        return false;
    }

    let fc = match part.get("functionCall") {
        Some(fc) => fc,
        None => return true,
    };
    let name = fc
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let provided_id = fc
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());

    // Unique-id synthesis (Pi google-generative-ai.ts:181-186): mint a new id when absent or a dup.
    let dup = provided_id
        .map(|pid| {
            dec.blocks
                .iter()
                .any(|b| matches!(b, Content::ToolCall(tc) if tc.id.as_str() == pid))
        })
        .unwrap_or(false);
    let tool_call_id = match provided_id {
        Some(pid) if !dup => pid.to_string(),
        _ => {
            let n = TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
            format!("{name}_{}_{n}", now_millis())
        }
    };

    let arguments = fc
        .get("args")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let thought_signature = part
        .get("thoughtSignature")
        .and_then(Value::as_str)
        .map(str::to_string);

    let tool_call = ToolCall {
        id: ToolCallId::from(tool_call_id.as_str()),
        name,
        arguments,
        thought_signature,
    };

    dec.blocks.push(Content::ToolCall(tool_call.clone()));
    let idx = dec.block_index();

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
    let delta = serde_json::to_string(&Value::Object(tool_call.arguments.clone()))
        .unwrap_or_else(|_| "{}".to_string());
    let partial = dec.snapshot(model, api);
    if !sink
        .send(StreamEvent::ToolCallDelta {
            content_index: idx,
            delta,
            partial,
        })
        .await
    {
        return false;
    }
    let partial = dec.snapshot(model, api);
    sink.send(StreamEvent::ToolCallEnd {
        content_index: idx,
        tool_call,
        partial,
    })
    .await
}

/// Emit the `*_end` for the in-progress text/thinking block, if any (Pi google-generative-ai.ts:106-122).
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

/// Retain the last non-empty signature for the current block (Pi `retainThoughtSignature`,
/// google-shared.ts:46-49). Returns the new value when it should replace the existing one.
fn retain_signature(_existing: Option<&str>, incoming: Option<&str>) -> Option<String> {
    match incoming {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    }
}

/// Apply Gemini `usageMetadata` (Pi google-generative-ai.ts:216-235).
fn apply_usage(usage: &mut Usage, meta: &Value) {
    let prompt = meta
        .get("promptTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached = meta
        .get("cachedContentTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let candidates = meta
        .get("candidatesTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let thoughts = meta
        .get("thoughtsTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = meta
        .get("totalTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    usage.input = prompt.saturating_sub(cached);
    usage.output = candidates + thoughts;
    usage.cache_read = cached;
    usage.cache_write = 0;
    usage.reasoning = Some(thoughts);
    usage.total_tokens = total;
}

/// Emit a terminal error event carrying the partial snapshot (Pi catch block,
/// google-generative-ai.ts:266-277).
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

    /// pi `google-generative-ai.ts:214-216` + `:269-273`: the raw finishReason names itself in the
    /// terminal error. Gemini's real failure modes are all finish reasons, and before this they all
    /// collapsed to the identical "An unknown error occurred".
    #[test]
    fn a_non_stop_finish_reason_names_itself_in_the_error() {
        for reason in [
            "SAFETY",
            "RECITATION",
            "PROHIBITED_CONTENT",
            "BLOCKLIST",
            "MALFORMED_FUNCTION_CALL",
        ] {
            let (stop, err) = map_stop_reason(reason);
            assert_eq!(stop, StopReason::Error, "{reason}");
            assert_eq!(
                err.as_deref(),
                Some(format!("Provider stopped with: {reason}").as_str()),
                "{reason} must be distinguishable from every other block reason"
            );
        }

        // The two non-error arms stay clean and carry no diagnostic.
        assert_eq!(map_stop_reason("STOP"), (StopReason::Stop, None));
        assert_eq!(map_stop_reason("MAX_TOKENS"), (StopReason::Length, None));
    }

    use crate::api::channel;
    use crate::model::ModelCost;
    use crate::stream::sse::decode_sse_bytes;
    use cyrup_core::Message;

    fn model_with(id: &str, reasoning: bool) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            api: API_ID.into(),
            provider: "google".into(),
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
            reasoning,
            input: vec![Modality::Text, Modality::Image],
            cost: ModelCost {
                input: 0.3,
                output: 2.5,
                cache_read: 0.03,
                cache_write: 0.0,
                tiers: None,
            },
            context_window: 1_048_576,
            max_tokens: 65_536,
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
    fn url_appends_streaming_endpoint() {
        assert_eq!(
            stream_url(
                "https://generativelanguage.googleapis.com/v1beta",
                "gemini-2.5-pro"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        assert_eq!(
            stream_url("https://host/v1beta/", "gemini-2.0-flash"),
            "https://host/v1beta/models/gemini-2.0-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn headers_use_goog_api_key() {
        let m = model_with("gemini-2.0-flash", false);
        let headers = build_headers(&m, &StreamOptions::default(), "test-key");
        assert_eq!(
            headers
                .get("x-goog-api-key")
                .and_then(|v| v.clone())
                .as_deref(),
            Some("test-key")
        );
        assert_eq!(
            headers
                .get("content-type")
                .and_then(|v| v.clone())
                .as_deref(),
            Some("application/json")
        );
    }

    #[test]
    fn build_params_basic_shape() {
        let m = model_with("gemini-2.0-flash", false);
        let opts = StreamOptions {
            max_tokens: Some(1000),
            temperature: Some(0.4),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("hello"), &opts);
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hello");
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be brief");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 1000);
        assert!((body["generationConfig"]["temperature"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        // Non-reasoning model: no thinkingConfig.
        assert!(body["generationConfig"].get("thinkingConfig").is_none());
    }

    #[test]
    fn thinking_budget_for_gemini_2_5() {
        let m = model_with("gemini-2.5-pro", true);
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("think"), &opts);
        let tc = &body["generationConfig"]["thinkingConfig"];
        assert_eq!(tc["includeThoughts"], true);
        assert_eq!(tc["thinkingBudget"], 32768);
        assert!(tc.get("thinkingLevel").is_none());
    }

    #[test]
    fn thinking_level_for_gemini_3_pro() {
        let m = model_with("gemini-3-pro-preview", true);
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("think"), &opts);
        let tc = &body["generationConfig"]["thinkingConfig"];
        assert_eq!(tc["includeThoughts"], true);
        assert_eq!(tc["thinkingLevel"], "HIGH");
        assert!(tc.get("thinkingBudget").is_none());
    }

    #[test]
    fn disabled_thinking_when_reasoning_off() {
        // Gemini 2.x reasoning model with reasoning off → thinkingBudget: 0.
        let m = model_with("gemini-2.5-flash", true);
        let body = build_body(&m, &user_ctx("x"), &StreamOptions::default());
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            0
        );
        // Gemini 3 pro reasoning model with reasoning off → thinkingLevel: LOW.
        let m3 = model_with("gemini-3-pro-preview", true);
        let body = build_body(&m3, &user_ctx("x"), &StreamOptions::default());
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
    }

    /// Byte-diff vs Pi `buildParams` (google-generative-ai.ts:373-384): a direct
    /// `GoogleOptions.thinking` override is read verbatim, bypassing the unified-`reasoning` lowering.
    /// `level` wins over `budgetTokens`; `enabled:false` lowers to the disabled config; the override
    /// can DISABLE thinking on a request whose unified `reasoning` is High (proving the override path,
    /// not the lowering, drove the bytes).
    #[test]
    fn google_thinking_override_threads_budget_and_level() {
        // 1. budgetTokens override on a Gemini-2.x model: thinkingBudget = the supplied value
        //    (-1 dynamic), NOT the `getGoogleBudget`-computed 32768.
        let m = model_with("gemini-2.5-pro", true);
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            api_options: Some(crate::stream::ApiStreamOptions::Google(GoogleOptions {
                thinking: Some(GoogleThinking {
                    enabled: true,
                    budget_tokens: Some(-1),
                    level: None,
                }),
            })),
            ..Default::default()
        };
        let tc = build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"]
            .clone();
        // Pi: { includeThoughts: true, thinkingBudget: -1 }.
        assert_eq!(tc, json!({ "includeThoughts": true, "thinkingBudget": -1 }));

        // 2. level wins over budgetTokens (Pi reads `level` first, google-generative-ai.ts:376-381).
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::Low,
            api_options: Some(crate::stream::ApiStreamOptions::Google(GoogleOptions {
                thinking: Some(GoogleThinking {
                    enabled: true,
                    budget_tokens: Some(9999),
                    level: Some(GoogleThinkingLevel::Medium),
                }),
            })),
            ..Default::default()
        };
        let tc = build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"]
            .clone();
        // Pi: { includeThoughts: true, thinkingLevel: "MEDIUM" } (no thinkingBudget).
        assert_eq!(tc, json!({ "includeThoughts": true, "thinkingLevel": "MEDIUM" }));

        // 3. enabled:false override DISABLES thinking even though unified reasoning is High → the
        //    model's disabled config (Pi `model.reasoning && options.thinking && !enabled`,
        //    google-generative-ai.ts:383-384). For gemini-2.5 that is `{ thinkingBudget: 0 }`.
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            api_options: Some(crate::stream::ApiStreamOptions::Google(GoogleOptions {
                thinking: Some(GoogleThinking {
                    enabled: false,
                    budget_tokens: None,
                    level: None,
                }),
            })),
            ..Default::default()
        };
        let tc = build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"]
            .clone();
        assert_eq!(tc, json!({ "thinkingBudget": 0 }));

        // 4. without the override, the unified `reasoning` lowering still drives the bytes (32768),
        //    proving (1)/(2) came from the override path.
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let tc = build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"]
            .clone();
        assert_eq!(tc["thinkingBudget"], 32768);
    }

    #[test]
    fn tools_encode_function_declarations_and_tool_config() {
        let mut ctx = user_ctx("use a tool");
        ctx.tools = vec![ToolDef {
            name: "read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        }];
        let m = model_with("gemini-2.0-flash", false);
        let opts = StreamOptions {
            tool_choice: Some(ToolChoice::Required),
            ..Default::default()
        };
        let body = build_body(&m, &ctx, &opts);
        let decl = &body["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["name"], "read");
        assert_eq!(
            decl["parametersJsonSchema"]["properties"]["path"]["type"],
            "string"
        );
        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
    }

    #[test]
    fn model_id_detection() {
        assert!(is_gemini3_pro(&model_with("gemini-3-pro-preview", true)));
        assert!(is_gemini3_pro(&model_with("gemini-3.1-pro", true)));
        assert!(!is_gemini3_pro(&model_with("gemini-2.5-pro", true)));
        assert!(is_gemini3_flash(&model_with(
            "gemini-3-flash-preview",
            true
        )));
        assert!(is_gemini3_flash(&model_with("gemini-flash-latest", true)));
        assert!(is_gemma4(&model_with("gemma-4-2b", true)));
        assert_eq!(gemini_major_version("gemini-2.5-pro"), Some(2));
        assert_eq!(gemini_major_version("gemini-3-pro-preview"), Some(3));
        assert!(supports_multimodal_function_response(
            "gemini-3-pro-preview"
        ));
        assert!(!supports_multimodal_function_response("gemini-2.5-pro"));
        assert!(supports_multimodal_function_response("claude-opus-4-5"));
    }

    #[test]
    fn function_response_uses_output_and_error_keys() {
        let m = model_with("gemini-2.5-pro", true);
        let ctx = Context {
            system_prompt: None,
            messages: vec![
                Message::User {
                    content: vec![Content::text("hi")],
                    timestamp: 0,
                },
                Message::ToolResult {
                    tool_call_id: cyrup_core::ToolCallId::from("c1"),
                    tool_name: "read".to_string(),
                    content: vec![Content::text("file body")],
                    is_error: false,
                    details: None,
                    timestamp: 0,
                    usage: None,
                    added_tool_names: Vec::new(),
                },
            ],
            tools: Vec::new(),
        };
        let body = build_body(&m, &ctx, &StreamOptions::default());
        let contents = body["contents"].as_array().unwrap();
        // The tool result becomes a `user` turn with a functionResponse part.
        let fr = contents
            .iter()
            .find_map(|c| c["parts"][0].get("functionResponse"))
            .expect("functionResponse part");
        assert_eq!(fr["name"], "read");
        assert_eq!(fr["response"]["output"], "file body");
        // gemini-2.5-pro is < 3, so no `id` field (requiresToolCallId false for gemini).
        assert!(fr.get("id").is_none());
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
    async fn decodes_text_thinking_and_tool_stream() {
        // A Gemini SSE transcript: a thinking part, a text part, then a functionCall + finishReason.
        let raw = concat!(
            "data: {\"responseId\":\"resp_1\",\"candidates\":[{\"content\":{\"parts\":[{\"thought\":true,\"text\":\"reasoning\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}]}}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":2,\"thoughtsTokenCount\":3,\"totalTokenCount\":15}}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"read\",\"args\":{\"path\":\"a\"}}}]},\"finishReason\":\"STOP\"}]}\n\n",
        );
        let m = model_with("gemini-2.5-pro", true);
        let events = collect(raw.as_bytes().to_vec(), &m).await;

        assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
        assert!(events.iter().any(
            |e| matches!(e, StreamEvent::ThinkingDelta { delta, .. } if delta == "reasoning")
        ));
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
        // A tool call is present → toolUse overrides the STOP finishReason.
        assert_eq!(msg.stop_reason, StopReason::ToolUse);
        assert_eq!(msg.response_id.as_deref(), Some("resp_1"));
        // input = prompt - cached = 10; output = candidates + thoughts = 5; total preserved from API.
        assert_eq!(msg.usage.input, 10);
        assert_eq!(msg.usage.output, 5);
        assert_eq!(msg.usage.reasoning, Some(3));
        assert_eq!(msg.usage.total_tokens, 15);
        assert!(msg.usage.cost.total > 0.0);
        // Content order: thinking, text, toolCall.
        assert!(matches!(msg.content[0], Content::Thinking { .. }));
        assert!(matches!(msg.content[1], Content::Text { .. }));
        assert!(matches!(msg.content[2], Content::ToolCall(_)));
    }

    #[tokio::test]
    async fn synthesizes_tool_call_id_when_absent() {
        let raw = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"ping\",\"args\":{}}}]},\"finishReason\":\"STOP\"}]}\n\n";
        let m = model_with("gemini-2.5-pro", true);
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        let tool = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
                _ => None,
            })
            .expect("toolcall_end");
        // Synthesized id is `{name}_{millis}_{counter}`.
        assert!(
            tool.id.as_str().starts_with("ping_"),
            "got: {}",
            tool.id.as_str()
        );
    }

    /// A valid (multiple-of-4, base64) thought signature for the signed-empty-block tests.
    const VALID_SIG: &str = "AAAAAAAAAAAAAAAAAAAAAA==";

    /// Build a two-message context whose assistant turn is attributed to `(provider, model)`.
    fn signed_block_ctx(provider: &str, model: &str, content: Vec<Content>) -> Context {
        Context {
            system_prompt: None,
            messages: vec![
                Message::User {
                    content: vec![Content::text("Hi")],
                    timestamp: 0,
                },
                Message::Assistant(AssistantMessage {
                    content,
                    provider: provider.into(),
                    model: model.to_string(),
                    api: API_ID.into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::ToolUse,
                    error_message: None,
                    timestamp: 1,
                }),
            ],
            tools: Vec::new(),
        }
    }

    fn a_tool_call() -> Content {
        Content::ToolCall(ToolCall {
            id: ToolCallId::from("call_1"),
            name: "bash".to_string(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
        })
    }

    fn model_turn_parts(contents: &[Value]) -> Vec<Value> {
        contents
            .iter()
            .find(|c| c["role"] == "model")
            .and_then(|c| c["parts"].as_array().cloned())
            .expect("model turn")
    }

    /// Pi google-shared.ts:148-151 (commit 6138f5a0): Gemini can attach `thoughtSignature` to a
    /// part whose visible text is empty and requires it echoed back. A signed EMPTY thinking block
    /// must survive so the reasoning chain is not broken.
    #[test]
    fn keeps_signed_empty_thinking_block() {
        let m = model_with("gemini-3-pro-preview", true);
        let ctx = signed_block_ctx(
            "google",
            "gemini-3-pro-preview",
            vec![
                Content::Thinking {
                    thinking: String::new(),
                    thinking_signature: Some(VALID_SIG.to_string()),
                    redacted: false,
                },
                a_tool_call(),
            ],
        );
        let contents = convert_messages(&m, &ctx);
        let parts = model_turn_parts(&contents);
        let signed: Vec<&Value> = parts
            .iter()
            .filter(|p| p.get("thoughtSignature").and_then(Value::as_str) == Some(VALID_SIG))
            .collect();
        assert_eq!(signed.len(), 1, "parts: {parts:?}");
        assert_eq!(signed[0]["thought"], true);
        assert_eq!(signed[0]["text"], "");
    }

    /// Pi google-shared.ts:134-139: the same rule for a signed EMPTY text block.
    #[test]
    fn keeps_signed_empty_text_block() {
        let m = model_with("gemini-3-pro-preview", true);
        let ctx = signed_block_ctx(
            "google",
            "gemini-3-pro-preview",
            vec![
                Content::Text {
                    text: String::new(),
                    text_signature: Some(VALID_SIG.to_string()),
                },
                a_tool_call(),
            ],
        );
        let contents = convert_messages(&m, &ctx);
        let parts = model_turn_parts(&contents);
        let signed: Vec<&Value> = parts
            .iter()
            .filter(|p| p.get("thoughtSignature").and_then(Value::as_str) == Some(VALID_SIG))
            .collect();
        assert_eq!(signed.len(), 1, "parts: {parts:?}");
        assert!(signed[0].get("thought").is_none());
        assert_eq!(signed[0]["text"], "");
    }

    /// The skip is gated on the signature being ABSENT — UNSIGNED empty blocks are still dropped
    /// (Pi google-shared.ts:139/151).
    #[test]
    fn still_drops_unsigned_empty_blocks() {
        let m = model_with("gemini-3-pro-preview", true);
        let ctx = signed_block_ctx(
            "google",
            "gemini-3-pro-preview",
            vec![
                Content::Thinking {
                    thinking: String::new(),
                    thinking_signature: None,
                    redacted: false,
                },
                Content::Text {
                    text: "   ".to_string(),
                    text_signature: None,
                },
                a_tool_call(),
            ],
        );
        let contents = convert_messages(&m, &ctx);
        let parts = model_turn_parts(&contents);
        assert_eq!(parts.len(), 1, "parts: {parts:?}");
        assert!(parts[0].get("functionCall").is_some());
    }

    /// An empty text block whose signature is INVALID base64 resolves to no signature, so the
    /// unsigned rule applies and it is still dropped.
    #[test]
    fn still_drops_empty_block_with_invalid_signature() {
        let m = model_with("gemini-3-pro-preview", true);
        let ctx = signed_block_ctx(
            "google",
            "gemini-3-pro-preview",
            vec![
                Content::Text {
                    text: String::new(),
                    text_signature: Some("not base64!".to_string()),
                },
                a_tool_call(),
            ],
        );
        let contents = convert_messages(&m, &ctx);
        let parts = model_turn_parts(&contents);
        assert_eq!(parts.len(), 1, "parts: {parts:?}");
        assert!(parts[0].get("functionCall").is_some());
    }

    /// The cross-provider/model `else` branch keeps the OLD unconditional skip — the signature is
    /// unusable there, so signed empty blocks are still dropped and the signature never leaks
    /// (Pi google-shared.ts:157-162, deliberately retained by 6138f5a0).
    #[test]
    fn cross_provider_drops_signed_empty_blocks_unconditionally() {
        let m = model_with("gemini-3-pro-preview", true);
        // Assistant turn is attributed to a DIFFERENT model → `same` is false.
        let ctx = signed_block_ctx(
            "google",
            "other-model",
            vec![
                Content::Thinking {
                    thinking: String::new(),
                    thinking_signature: Some(VALID_SIG.to_string()),
                    redacted: false,
                },
                Content::Text {
                    text: String::new(),
                    text_signature: Some(VALID_SIG.to_string()),
                },
                a_tool_call(),
            ],
        );
        let contents = convert_messages(&m, &ctx);
        let parts = model_turn_parts(&contents);
        assert_eq!(parts.len(), 1, "parts: {parts:?}");
        assert!(parts[0].get("functionCall").is_some());
        assert!(!Value::Array(parts).to_string().contains(VALID_SIG));
    }

    /// The cross-provider branch still converts a NON-empty thinking block to plain text.
    #[test]
    fn cross_provider_keeps_non_empty_thinking_as_text() {
        let m = model_with("gemini-3-pro-preview", true);
        let ctx = signed_block_ctx(
            "google",
            "other-model",
            vec![Content::Thinking {
                thinking: "reasoned".to_string(),
                thinking_signature: Some(VALID_SIG.to_string()),
                redacted: false,
            }],
        );
        let contents = convert_messages(&m, &ctx);
        let parts = model_turn_parts(&contents);
        assert_eq!(parts.len(), 1, "parts: {parts:?}");
        assert_eq!(parts[0]["text"], "reasoned");
        assert!(parts[0].get("thought").is_none());
        assert!(parts[0].get("thoughtSignature").is_none());
    }

    #[test]
    fn base64_signature_validation() {
        assert!(is_valid_thought_signature("YWJjZA=="));
        assert!(!is_valid_thought_signature("not base64!"));
        assert!(!is_valid_thought_signature("abc")); // not a multiple of 4
    }
}

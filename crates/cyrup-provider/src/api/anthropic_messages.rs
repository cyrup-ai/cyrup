//! The `anthropic-messages` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking the Anthropic Messages streaming API (`POST {baseUrl}/v1/messages`,
//! SSE events `message_start` / `content_block_{start,delta,stop}` / `message_delta` /
//! `message_stop`). Shared by every Anthropic-compatible provider (anthropic, kimi-coding, minimax,
//! minimax-cn, vercel-ai-gateway). Pure JSON-over-SSE — no SDK, no new dependency. 1:1 port of Pi's
//! `api/anthropic-messages.ts` encoder/decoder (extended thinking, `cache_control` + 1h ttl,
//! thinking signatures, `redacted_thinking`, beta headers, eager tool input streaming, and the
//! 64-char `^[a-zA-Z0-9_-]+$` tool-call-id rule).
//!
//! Wire JSON uses Anthropic's own field names (snake_case), NOT the cyrup camelCase convention.

use crate::HeaderMap;
use crate::api::compat::{AnthropicMessagesCompat, sanitize_surrogates};
use crate::api::openai_completions::transform_messages_with;
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
use crate::utils::deferred_tools::split_deferred_tools;
use crate::utils::json_parse::{parse_json_with_repair, parse_streaming_json_object};
use crate::utils::provider_retry::ProviderRetry;
use crate::utils::simple_options::{adjust_max_tokens_for_thinking, clamp_max_tokens_to_context};
use cyrup_core::{
    ApiId, AssistantMessage, CancelToken, Content, Message, StopReason, ThinkingLevel, ToolCall,
    ToolCallId, Usage,
};
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::ANTHROPIC_MESSAGES;

/// The Anthropic API version header value the SDK pins by default.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Beta header tokens (Pi anthropic-messages.ts:167-168).
const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// How thinking content is returned (Pi `AnthropicThinkingDisplay`, anthropic-messages.ts:165).
/// `"summarized"` returns summarized thinking text; `"omitted"` returns an empty thinking field
/// (the encrypted signature still travels back for multi-turn continuity).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicThinkingDisplay {
    Summarized,
    Omitted,
}

impl AnthropicThinkingDisplay {
    /// The wire string for the `thinking.display` field.
    pub fn as_wire(self) -> &'static str {
        match self {
            AnthropicThinkingDisplay::Summarized => "summarized",
            AnthropicThinkingDisplay::Omitted => "omitted",
        }
    }
}

/// Per-API typed options for the `anthropic-messages` wire protocol (Pi `AnthropicOptions`,
/// anthropic-messages.ts:183-230). Only the fields cyrup does not already carry on the unified
/// [`StreamOptions`](crate::StreamOptions) live here; the rest (`thinkingEnabled`,
/// `thinkingBudgetTokens`, `effort`) map onto `StreamOptions.reasoning`/`thinking_budgets`. Carried
/// via [`StreamOptions::api_options`](crate::StreamOptions::api_options). All fields default to
/// `None`, reproducing Pi's defaults exactly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnthropicOptions {
    /// Request the interleaved-thinking beta header for non-adaptive thinking models (Pi
    /// `interleavedThinking`, anthropic-messages.ts:230). `None` = Pi default (`true`).
    pub interleaved_thinking: Option<bool>,
    /// How thinking content is returned (Pi `thinkingDisplay`, anthropic-messages.ts:223). `None` =
    /// Pi default (`"summarized"`).
    pub thinking_display: Option<AnthropicThinkingDisplay>,
}

/// Stealth-mode Claude Code identity (Pi anthropic-messages.ts:73).
const CLAUDE_CODE_VERSION: &str = "2.1.75";

/// Claude Code 2.x canonical tool names (Pi `claudeCodeTools`, anthropic-messages.ts:78-96).
const CLAUDE_CODE_TOOLS: [&str; 17] = [
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

/// The `ApiImpl` for `"anthropic-messages"`.
pub struct AnthropicMessagesApi {
    api: ApiId,
}

impl Default for AnthropicMessagesApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl AnthropicMessagesApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(AnthropicMessagesApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for AnthropicMessagesApi {
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

        let is_oauth = resolve_is_oauth(model, auth);
        // PROV-011: an unsatisfiable `constrainedSampling` fails the turn before any HTTP, with
        // pi's own message.
        let params = match build_params(model, ctx, opts, auth.env.as_ref(), is_oauth) {
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
        let headers = build_headers(model, ctx, auth, opts, is_oauth);
        let req = SseRequest {
            method: reqwest::Method::POST,
            url,
            headers,
            body: Some(body),
        };

        // Honor HTTP(S)_PROXY for the live client (Pi resolveHttpProxyUrlForTarget,
        // node-http-proxy.ts:92-112; applied per request as in bedrock-converse-stream.ts:187).
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

        decode_stream(frames, model, &self.api, &sink, is_oauth, &ctx.tools).await;
    }
}

// ---------------------------------------------------------------------------
// Compat resolution (Pi getAnthropicCompat, anthropic-messages.ts:170-181)
// ---------------------------------------------------------------------------

/// Resolved Anthropic compat (Pi `Required<Omit<AnthropicMessagesCompat,"forceAdaptiveThinking">>`).
struct ResolvedAnthropicCompat {
    supports_eager_tool_input_streaming: bool,
    supports_long_cache_retention: bool,
    send_session_affinity_headers: bool,
    supports_cache_control_on_tools: bool,
    supports_temperature: bool,
    allow_empty_signature: bool,
    /// Pi `supportsStrictTools: model.compat?.supportsStrictTools ?? false`
    /// (`anthropic-messages.ts:183` @v0.83.0, type at `types.ts:639`) — the model accepts
    /// `tools[].strict: true` plus the FULL JSON schema in `input_schema`. PROV-011.
    supports_strict_tools: bool,
    /// DRIFT-001: emit `tool_reference` blocks + `defer_loading` tools. Defaults from
    /// [`default_supports_tool_references`], NOT to a constant.
    supports_tool_references: bool,
}

/// 1:1 port of Pi `getAnthropicCompat` (anthropic-messages.ts:170-181): every field defaults on,
/// except `sendSessionAffinityHeaders`/`allowEmptySignature` which default off.
fn get_anthropic_compat(model: &Model) -> ResolvedAnthropicCompat {
    let c: Option<&AnthropicMessagesCompat> = model.compat.as_ref();
    ResolvedAnthropicCompat {
        supports_eager_tool_input_streaming: c
            .and_then(|c| c.supports_eager_tool_input_streaming)
            .unwrap_or(true),
        supports_long_cache_retention: c
            .and_then(|c| c.supports_long_cache_retention)
            .unwrap_or(true),
        send_session_affinity_headers: c
            .and_then(|c| c.send_session_affinity_headers)
            .unwrap_or(false),
        supports_cache_control_on_tools: c
            .and_then(|c| c.supports_cache_control_on_tools)
            .unwrap_or(true),
        supports_temperature: c.and_then(|c| c.supports_temperature).unwrap_or(true),
        allow_empty_signature: c.and_then(|c| c.allow_empty_signature).unwrap_or(false),
        supports_strict_tools: c.and_then(|c| c.supports_strict_tools).unwrap_or(false),
        supports_tool_references: c
            .and_then(|c| c.supports_tool_references)
            .unwrap_or_else(|| default_supports_tool_references(model)),
    }
}

/// Default for `supportsToolReferences` (1:1 port of Pi `defaultSupportsToolReferences`,
/// anthropic-messages.ts:193-199): first-party Anthropic models except Haiku (which rejects
/// client-side `tool_reference` blocks) and models that predate tool search (Claude 3.x,
/// Opus/Sonnet 4.0, Opus 4.1).
///
/// Pi's predicate is
/// `/^claude-(?:opus|sonnet|fable)-(\d+)(?:-(\d+))?(?:-|$)/`. cyrup has no `regex` dependency
/// (`utils/regexlite` is a case-insensitive substring matcher, not a capture engine), so the
/// capture is hand-rolled. Greedy-only scanning is EXACT here: if the greedy `(\d+)` overruns,
/// every backtracked position leaves a digit next, and a digit satisfies neither `-(\d+)` nor
/// `(?:-|$)`, so backtracking can never rescue a match.
///
/// The `version[2].length < 8` guard is the DATE-SUFFIX gate and is load-bearing:
/// `claude-sonnet-4-20250514` captures `"20250514"` (8 chars) → minor 0 → **false**, while
/// `claude-opus-4-5-20251101` captures `"5"` → minor 5 → **true**.
fn default_supports_tool_references(model: &Model) -> bool {
    let id = model.id.as_str();
    if model.provider.as_str() != "anthropic" || id.contains("haiku") {
        return false;
    }
    let Some(rest) = id.strip_prefix("claude-") else {
        return false;
    };
    // `(?:opus|sonnet|fable)-`
    let Some(rest) = ["opus-", "sonnet-", "fable-"]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
    else {
        return false;
    };

    // `(\d+)` — greedy.
    let major_len = rest.chars().take_while(char::is_ascii_digit).count();
    if major_len == 0 {
        return false;
    }
    let (Some(major_digits), Some(after_major)) = (rest.get(..major_len), rest.get(major_len..))
    else {
        return false;
    };
    let Ok(major) = major_digits.parse::<u32>() else {
        return false;
    };

    // `(?:-(\d+))?(?:-|$)`
    let mut minor: u32 = 0;
    if after_major.is_empty() {
        // `$` matches; the optional minor group did not participate.
    } else if let Some(tail) = after_major.strip_prefix('-') {
        let minor_len = tail.chars().take_while(char::is_ascii_digit).count();
        let minor_captured = tail.get(..minor_len).unwrap_or("");
        let remainder = tail.get(minor_len..).unwrap_or("");
        // The optional group participates only if it is followed by `-` or end of string;
        // otherwise the regex backtracks and `(?:-|$)` consumes the `-` we just stripped.
        if minor_len > 0 && (remainder.is_empty() || remainder.starts_with('-')) {
            // `version[2] && version[2].length < 8 ? Number(version[2]) : 0`
            minor = if minor_captured.len() < 8 {
                minor_captured.parse::<u32>().unwrap_or(0)
            } else {
                0
            };
        }
    } else {
        // Neither `-` nor end of string after the major version → no match at all.
        return false;
    }

    major > 4 || (major == 4 && minor >= 5)
}

/// `model.compat?.forceAdaptiveThinking === true` (Pi default false).
fn force_adaptive_thinking(model: &Model) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|c| c.force_adaptive_thinking)
        .unwrap_or(false)
}

/// `model.thinkingLevelMap?.off !== null` (a missing key is `undefined`, which `!== null`).
fn off_is_not_null(model: &Model) -> bool {
    !matches!(
        model.thinking_level_map.as_ref().and_then(|m| m.get("off")),
        Some(None)
    )
}

// ---------------------------------------------------------------------------
// Cache retention (Pi resolveCacheRetention / getCacheControl)
// ---------------------------------------------------------------------------

/// Resolve a provider env value (Pi `getProviderEnvValue`): the scoped overlay wins over the process
/// environment.
fn provider_env_value(name: &str, env: Option<&ProviderEnv>) -> Option<String> {
    if let Some(map) = env
        && let Some(v) = map.get(name).filter(|v| !v.is_empty())
    {
        return Some(v.clone());
    }
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// 1:1 port of Pi `resolveCacheRetention` (anthropic-messages.ts:46-54).
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

/// The `cache_control` ephemeral marker for the resolved retention (Pi `getCacheControl`,
/// anthropic-messages.ts:56-70). `None` when retention is `none`.
fn get_cache_control(
    model: &Model,
    cache_retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> Option<Value> {
    let retention = resolve_cache_retention(cache_retention, env);
    if retention == CacheRetention::None {
        return None;
    }
    let mut cc = Map::new();
    cc.insert("type".to_string(), json!("ephemeral"));
    if retention == CacheRetention::Long
        && get_anthropic_compat(model).supports_long_cache_retention
    {
        cc.insert("ttl".to_string(), json!("1h"));
    }
    Some(Value::Object(cc))
}

// ---------------------------------------------------------------------------
// Claude Code tool-name mapping (Pi anthropic-messages.ts:98-109)
// ---------------------------------------------------------------------------

/// Map a tool name to Claude Code canonical casing if it matches case-insensitively (Pi
/// `toClaudeCodeName`).
fn to_claude_code_name(name: &str) -> String {
    let lower = name.to_lowercase();
    for t in CLAUDE_CODE_TOOLS {
        if t.to_lowercase() == lower {
            return t.to_string();
        }
    }
    name.to_string()
}

/// Map a Claude Code tool name back to a caller-declared tool name by case-insensitive match (Pi
/// `fromClaudeCodeName`, anthropic-messages.ts:102-109).
fn remap_decoded_tool_name(tool_names: &[String], name: &str) -> String {
    let lower = name.to_lowercase();
    for declared in tool_names {
        if declared.to_lowercase() == lower {
            return declared.clone();
        }
    }
    name.to_string()
}

/// `apiKey.includes("sk-ant-oat")` (Pi `isOAuthToken`, anthropic-messages.ts:809-811).
fn is_oauth_token(api_key: &str) -> bool {
    api_key.contains("sk-ant-oat")
}

/// `model.provider === "github-copilot"` — the branch Pi tests FIRST inside `createClient`
/// (anthropic-messages.ts:868). Copilot's 9 anthropic-messages rows are routed here, not through
/// the `isOAuthToken` sniff, because a Copilot token (`tid=…;exp=…;proxy-ep=…`) contains no
/// `sk-ant-oat` marker and would otherwise fall through to `x-api-key` — which Copilot's edge
/// rejects (PROV-027).
fn is_github_copilot(model: &Model) -> bool {
    model.provider.as_str() == crate::api::github_copilot_headers::GITHUB_COPILOT_PROVIDER
}

/// The `isOAuthToken` value Pi's `createClient` RETURNS, which is what `buildParams` consumes
/// (anthropic-messages.ts:536-546, consumed by `buildParams` at `:938`). The Copilot branch returns
/// `false` unconditionally (`:887`), so Copilot never gets the Claude-Code tool-name normalization
/// even if its token happened to contain the marker; only the second branch (`:891`) reports `true`.
fn resolve_is_oauth(model: &Model, auth: &AuthResult) -> bool {
    if is_github_copilot(model) {
        return false;
    }
    auth.auth
        .api_key
        .as_deref()
        .map(is_oauth_token)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Request encoding
// ---------------------------------------------------------------------------

/// Resolve the `POST` target: an auth base-url override wins over `model.base_url`. The endpoint is
/// `{base}/v1/messages`.
fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
    Some(messages_url(base))
}

/// Normalize a base URL to the `/v1/messages` endpoint.
pub(crate) fn messages_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/v1/messages") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1/messages")
    }
}

/// `true` if the request should send the `fine-grained-tool-streaming` beta (Pi
/// `shouldUseFineGrainedToolStreamingBeta`, anthropic-messages.ts:1184-1186).
fn should_use_fine_grained_beta(model: &Model, ctx: &Context) -> bool {
    !ctx.tools.is_empty() && !get_anthropic_compat(model).supports_eager_tool_input_streaming
}

/// Build the request headers (1:1 port of Pi `createClient`, anthropic-messages.ts:813-899). The
/// auth/model/opts header overlays layer last (a `None` value suppresses a default).
pub(crate) fn build_headers(
    model: &Model,
    ctx: &Context,
    auth: &AuthResult,
    opts: &StreamOptions,
    is_oauth: bool,
) -> HeaderMap {
    // Pi `options?.interleavedThinking ?? true` (anthropic-messages.ts:520).
    let interleaved = opts
        .anthropic_options()
        .and_then(|o| o.interleaved_thinking)
        .unwrap_or(true);
    let needs_interleaved = interleaved && !force_adaptive_thinking(model);
    let mut betas: Vec<&str> = Vec::new();
    if should_use_fine_grained_beta(model, ctx) {
        betas.push(FINE_GRAINED_TOOL_STREAMING_BETA);
    }
    if needs_interleaved {
        betas.push(INTERLEAVED_THINKING_BETA);
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        "anthropic-version".to_string(),
        Some(ANTHROPIC_VERSION.to_string()),
    );
    headers.insert(
        "content-type".to_string(),
        Some("application/json".to_string()),
    );
    headers.insert("accept".to_string(), Some("application/json".to_string()));
    headers.insert(
        "anthropic-dangerous-direct-browser-access".to_string(),
        Some("true".to_string()),
    );

    if is_github_copilot(model) {
        // PROV-027 — Copilot: Bearer auth, SELECTIVE betas (Pi anthropic-messages.ts:867-888).
        // `new Anthropic({ apiKey: null, authToken: apiKey })` sends `Authorization: Bearer …`,
        // never `x-api-key`. Note what this branch deliberately does NOT send: the
        // `claude-code-20250219`/`oauth-2025-04-20` betas, the `claude-cli` user-agent, `x-app`, and
        // the session-affinity header — Copilot's edge is not Anthropic's.
        if let Some(key) = &auth.auth.api_key {
            headers.insert("authorization".to_string(), Some(format!("Bearer {key}")));
        }
        if !betas.is_empty() {
            headers.insert("anthropic-beta".to_string(), Some(betas.join(",")));
        }
    } else if is_oauth {
        // OAuth: Bearer auth + Claude Code identity headers (Pi anthropic-messages.ts:855-872).
        if let Some(key) = &auth.auth.api_key {
            headers.insert("authorization".to_string(), Some(format!("Bearer {key}")));
        }
        let mut oauth_betas = vec![
            "claude-code-20250219".to_string(),
            "oauth-2025-04-20".to_string(),
        ];
        oauth_betas.extend(betas.iter().map(|b| b.to_string()));
        headers.insert("anthropic-beta".to_string(), Some(oauth_betas.join(",")));
        headers.insert(
            "user-agent".to_string(),
            Some(format!("claude-cli/{CLAUDE_CODE_VERSION}")),
        );
        headers.insert("x-app".to_string(), Some("cli".to_string()));
    } else {
        // API key auth (Pi anthropic-messages.ts:877-896).
        if let Some(key) = &auth.auth.api_key {
            headers.insert("x-api-key".to_string(), Some(key.clone()));
        }
        if !betas.is_empty() {
            headers.insert("anthropic-beta".to_string(), Some(betas.join(",")));
        }
        // Session-affinity header when caching is enabled and the compat flag is set.
        let cache = resolve_cache_retention(opts.cache_retention, auth.env.as_ref());
        if cache != CacheRetention::None
            && get_anthropic_compat(model).send_session_affinity_headers
            && let Some(sid) = &opts.session_id
        {
            headers.insert(
                "x-session-affinity".to_string(),
                Some(sid.as_str().to_string()),
            );
        }
    }

    // Auth overlay < model.headers < opts.headers (a `None` suppresses a default).
    if let Some(overlay) = &auth.auth.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    // PROV-028: the per-request Copilot headers, in Pi's merge slot — `mergeHeaders(defaults,
    // model.headers, dynamicHeaders, optionsHeaders)` (anthropic-messages.ts:875-884; `model.headers`
    // at `:881`, `dynamicHeaders` at `:882`), computed at `:525-531`. No-op for every other provider.
    crate::api::github_copilot_headers::apply_copilot_dynamic_headers(
        &mut headers,
        model.provider.as_str(),
        &ctx.messages,
    );
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    headers
}

/// Map a unified [`ThinkingLevel`] to an Anthropic adaptive-thinking effort (Pi
/// `mapThinkingLevelToEffort`, anthropic-messages.ts:747-765). A `thinkingLevelMap` string overrides.
fn map_thinking_level_to_effort(model: &Model, level: ThinkingLevel) -> String {
    let key = match level {
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    };
    if let Some(Some(mapped)) = model.thinking_level_map.as_ref().and_then(|m| m.get(key)) {
        return mapped.clone();
    }
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => "low".to_string(),
        ThinkingLevel::Medium => "medium".to_string(),
        // Pi's switch has no `xhigh`/`max` case: both land on `default: "high"`
        // (anthropic-messages.ts:786-798). Only an explicit `thinkingLevelMap` entry (handled
        // above) promotes them to the native `xhigh`/`max` efforts.
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => "high".to_string(),
    }
}

/// Test-only convenience wrapper for [`build_params`] with no env overlay and API-key auth.
#[cfg(test)]
// Test-only fixture wrapper: the deny-list allowance the crate's `mod tests` blocks carry.
#[allow(clippy::expect_used)]
pub(crate) fn build_body(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    build_params(model, ctx, opts, None, false)
        .expect("fixture declares no unsatisfiable constrained sampling")
}

/// Build the Messages request JSON body (1:1 port of Pi `buildParams` + the `streamSimple` thinking
/// lowering, anthropic-messages.ts:767-1004). The unified `opts.reasoning` level drives the thinking
/// config and (for budget-based models) the `max_tokens` split.
/// `[CYRUP-DELTA]` — fallible where pi's `buildParams` throws: `convertTools` rejects a
/// `strict: "require"` tool on a model without `supportsStrictTools`
/// (`constrained-sampling.ts:91-95` @v0.83.0). Upstream that unwinds into `stream`'s catch and
/// becomes the turn's terminal error message; here the caller emits the identical event (PROV-011).
pub(crate) fn build_params(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    env: Option<&ProviderEnv>,
    is_oauth: bool,
) -> Result<Value, ConstrainedSamplingError> {
    let compat = get_anthropic_compat(model);
    let cache_control = get_cache_control(model, opts.cache_retention, env);

    // --- DRIFT-001 deferred-tool placement (Pi anthropic-messages.ts:947-960) ---
    //
    // The transform is HOISTED out of `convert_messages` because Pi splits over the TRANSFORMED
    // list (`{ ...context, messages: transformedMessages }`, :949-953) and then hands that same
    // list to `convertMessages` (:961). Splitting over the raw list would be a structural
    // divergence even though today's transform only rewrites tool-call ids.
    let transformed = transform_messages_with(&ctx.messages, model, normalize_tool_call_id);
    let normalize_tool_name: &dyn Fn(&str) -> String = if is_oauth {
        &|name: &str| to_claude_code_name(name)
    } else {
        &|name: &str| name.to_string()
    };
    let placement = split_deferred_tools(
        &transformed,
        &ctx.tools,
        compat.supports_tool_references,
        normalize_tool_name,
    );
    let mut deferred_tools = placement.deferred_tools();
    let mut immediate_tools = placement.immediate;
    // The SAFETY VALVE lives here and ONLY here (Pi :955-959). It is deliberately absent from
    // `split_deferred_tools` and from the openai-responses caller, which ships no `tools` key at
    // all when everything is deferred.
    if immediate_tools.is_empty() && !deferred_tools.is_empty() {
        immediate_tools = std::mem::take(&mut deferred_tools);
    }
    let deferred_tool_names: HashSet<String> = deferred_tools
        .iter()
        .map(|t| normalize_tool_name(&t.name))
        .collect();

    let reasoning_on = opts.reasoning.is_on();
    let thinking_enabled = model.reasoning && reasoning_on;
    let adaptive = force_adaptive_thinking(model);

    // max_tokens lowering (Pi `streamSimple`, anthropic-messages.ts:790-806). Budget-based models
    // split the cap between thinking and output; adaptive / non-thinking just clamp to the context.
    let mut budget_tokens: u64 = 1024;
    let max_tokens: u64 = if thinking_enabled && !adaptive {
        let level = opts.reasoning.level().unwrap_or(ThinkingLevel::High);
        let (adjusted, budget) = adjust_max_tokens_for_thinking(
            opts.max_tokens,
            model.max_tokens,
            level,
            opts.thinking_budgets.as_ref(),
        );
        let mt = clamp_max_tokens_to_context(model, ctx, adjusted);
        budget_tokens = budget.min(mt.saturating_sub(1024));
        mt
    } else {
        clamp_max_tokens_to_context(model, ctx, opts.max_tokens.unwrap_or(model.max_tokens))
    };

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert(
        "messages".to_string(),
        Value::Array(convert_messages(
            &transformed,
            is_oauth,
            cache_control.as_ref(),
            compat.allow_empty_signature,
            &deferred_tool_names,
            normalize_tool_name,
        )),
    );
    obj.insert("max_tokens".to_string(), json!(max_tokens));
    obj.insert("stream".to_string(), json!(true));

    // System prompt (+ OAuth Claude Code identity).
    let mut system: Vec<Value> = Vec::new();
    if is_oauth {
        system.push(system_text(
            "You are Claude Code, Anthropic's official CLI for Claude.",
            cache_control.as_ref(),
        ));
        if let Some(sp) = &ctx.system_prompt {
            system.push(system_text(
                &sanitize_surrogates(sp),
                cache_control.as_ref(),
            ));
        }
    } else if let Some(sp) = &ctx.system_prompt {
        system.push(system_text(
            &sanitize_surrogates(sp),
            cache_control.as_ref(),
        ));
    }
    if !system.is_empty() {
        obj.insert("system".to_string(), Value::Array(system));
    }

    // Temperature is incompatible with extended thinking and unsupported on Opus 4.7+.
    if let Some(temp) = opts.temperature
        && !thinking_enabled
        && compat.supports_temperature
    {
        obj.insert("temperature".to_string(), json!(temp));
    }

    // Tools: immediate prefix, then the deferred tail (Pi anthropic-messages.ts:1007-1021).
    // `cache_control` marks the last IMMEDIATE tool only — Pi passes `undefined` for the deferred
    // call, so the cache breakpoint never lands on a definition that is not part of the stable
    // prefix.
    if !immediate_tools.is_empty() || !deferred_tools.is_empty() {
        let tool_cc = if compat.supports_cache_control_on_tools {
            cache_control.as_ref()
        } else {
            None
        };
        let mut tools = convert_tools(
            &immediate_tools,
            is_oauth,
            compat.supports_eager_tool_input_streaming,
            compat.supports_strict_tools,
            tool_cc,
            false,
        )?;
        tools.extend(convert_tools(
            &deferred_tools,
            is_oauth,
            compat.supports_eager_tool_input_streaming,
            compat.supports_strict_tools,
            None,
            true,
        )?);
        obj.insert("tools".to_string(), Value::Array(tools));
    }

    // Thinking configuration (Pi anthropic-messages.ts:957-986).
    if model.reasoning {
        if thinking_enabled {
            // Pi `options.thinkingDisplay ?? "summarized"` (anthropic-messages.ts:962).
            let display = json!(
                opts.anthropic_options()
                    .and_then(|o| o.thinking_display)
                    .map(AnthropicThinkingDisplay::as_wire)
                    .unwrap_or("summarized")
            );
            if adaptive {
                let mut thinking = Map::new();
                thinking.insert("type".to_string(), json!("adaptive"));
                thinking.insert("display".to_string(), display);
                obj.insert("thinking".to_string(), Value::Object(thinking));
                if let Some(level) = opts.reasoning.level() {
                    let effort = map_thinking_level_to_effort(model, level);
                    obj.insert("output_config".to_string(), json!({ "effort": effort }));
                }
            } else {
                obj.insert(
                    "thinking".to_string(),
                    json!({
                        "type": "enabled",
                        "budget_tokens": budget_tokens.max(1),
                        "display": display,
                    }),
                );
            }
        } else if off_is_not_null(model) {
            obj.insert("thinking".to_string(), json!({ "type": "disabled" }));
        }
    }

    // metadata.user_id (Pi anthropic-messages.ts:988-993).
    if let Some(meta) = &opts.metadata
        && let Some(user_id) = meta.get("user_id").and_then(Value::as_str)
    {
        obj.insert("metadata".to_string(), json!({ "user_id": user_id }));
    }

    // tool_choice (Pi anthropic-messages.ts:995-1001). cyrup's unified ToolChoice maps onto
    // Anthropic's `{type:"auto"|"any"|"none"}` / `{type:"tool",name}`.
    if let Some(tc) = &opts.tool_choice {
        obj.insert("tool_choice".to_string(), tool_choice_wire(tc));
    }

    Ok(Value::Object(obj))
}

/// Map cyrup's unified [`crate::stream::ToolChoice`] onto Anthropic's tool-choice wire shape.
fn tool_choice_wire(tc: &crate::stream::ToolChoice) -> Value {
    use crate::stream::ToolChoice;
    match tc {
        ToolChoice::Auto => json!({ "type": "auto" }),
        ToolChoice::None => json!({ "type": "none" }),
        // Anthropic spells "required" as "any".
        ToolChoice::Required => json!({ "type": "any" }),
        ToolChoice::Function { name } => json!({ "type": "tool", "name": name }),
    }
}

/// A `system` text block, optionally cached.
fn system_text(text: &str, cache_control: Option<&Value>) -> Value {
    let mut o = Map::new();
    o.insert("type".to_string(), json!("text"));
    o.insert("text".to_string(), json!(text));
    if let Some(cc) = cache_control {
        o.insert("cache_control".to_string(), cc.clone());
    }
    Value::Object(o)
}

/// Anthropic tool-call-id normalization (Pi `normalizeToolCallId`, anthropic-messages.ts:1006-1009):
/// non-`[a-zA-Z0-9_-]` → `_`, truncated to 64 chars.
fn normalize_tool_call_id(id: &str) -> String {
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

/// Convert tool-result / user content blocks to Anthropic format (Pi `convertContentBlocks`,
/// anthropic-messages.ts:114-161). Text-only collapses to a string; mixed content becomes a block
/// array (a leading `(see attached image)` text block is added when only images are present).
fn convert_content_blocks(content: &[Content]) -> Value {
    let has_images = content.iter().any(|c| matches!(c, Content::Image { .. }));
    if !has_images {
        let joined = content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        return json!(sanitize_surrogates(&joined));
    }

    let mut blocks: Vec<Value> = Vec::new();
    let mut has_text = false;
    for block in content {
        match block {
            Content::Text { text, .. } => {
                has_text = true;
                blocks.push(json!({ "type": "text", "text": sanitize_surrogates(text) }));
            }
            Content::Image { data, mime_type } => blocks.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": mime_type, "data": data },
            })),
            _ => {}
        }
    }
    if !has_text {
        blocks.insert(0, json!({ "type": "text", "text": "(see attached image)" }));
    }
    Value::Array(blocks)
}

/// The per-request deferred-tool anchoring state threaded through [`convert_messages`] (Pi keeps
/// these as three separate parameters of `convertToolResult`, anthropic-messages.ts:1081-1086).
struct ToolAnchors<'a> {
    /// Normalized names that were split out of the request prefix and must be anchored.
    deferred_tool_names: &'a HashSet<String>,
    /// Names already referenced in THIS request — declared once per `convertMessages` call
    /// (Pi :1125) so a tool is loaded exactly once even if several results mark it.
    loaded_tool_names: HashSet<String>,
    /// `toClaudeCodeName` under OAuth, identity otherwise (Pi :948).
    normalize_tool_name: &'a dyn Fn(&str) -> String,
}

/// Convert ONE tool-result message into its `tool_result` block plus any content that had to be
/// DISPLACED out of it (1:1 port of Pi `convertToolResult`, anthropic-messages.ts:1081-1112).
///
/// Anthropic **rejects** a `tool_result` whose `content` mixes `tool_reference` blocks with
/// ordinary blocks, so when this result anchors a deferred tool the reference list REPLACES the
/// content and the real content is returned separately, to be re-appended as a sibling of the
/// `tool_result` in the same `user` message. Nothing is dropped — it is relocated.
///
/// A name is referenced at most once per request: `loaded_tool_names` is declared once per
/// [`convert_messages`] call (Pi :1125) and is shared across every tool result in the transcript.
fn convert_tool_result(
    tool_call_id: &str,
    content: &[Content],
    is_error: bool,
    added_tool_names: &[String],
    is_oauth: bool,
    anchors: &mut ToolAnchors<'_>,
) -> (Value, Vec<Value>) {
    let mut references: Vec<Value> = Vec::new();
    for name in added_tool_names {
        let normalized = (anchors.normalize_tool_name)(name);
        if !anchors.deferred_tool_names.contains(&normalized)
            || anchors.loaded_tool_names.contains(&normalized)
        {
            continue;
        }
        anchors.loaded_tool_names.insert(normalized);
        let wire_name = if is_oauth {
            to_claude_code_name(name)
        } else {
            name.clone()
        };
        references.push(json!({ "type": "tool_reference", "tool_name": wire_name }));
    }

    let converted = convert_content_blocks(content);
    let has_refs = !references.is_empty();

    let mut tr = Map::new();
    tr.insert("type".to_string(), json!("tool_result"));
    tr.insert("tool_use_id".to_string(), json!(tool_call_id));
    tr.insert(
        "content".to_string(),
        if has_refs {
            Value::Array(references)
        } else {
            converted.clone()
        },
    );
    // `is_error` rides on the `tool_result` regardless of whether it carries references.
    tr.insert("is_error".to_string(), json!(is_error));

    // Pi `typeof convertedContent === "string" ? [{type:"text",text:…}] : convertedContent`. Pi
    // has NO empty-string guard, so an empty tool result with a reference emits `text: ""`.
    let siblings: Vec<Value> = if !has_refs {
        Vec::new()
    } else {
        match converted {
            Value::String(s) => vec![json!({ "type": "text", "text": s })],
            Value::Array(a) => a,
            other => vec![other],
        }
    };
    (Value::Object(tr), siblings)
}

/// Map cyrup [`Message`]s to Anthropic `messages` (1:1 port of Pi `convertMessages`,
/// anthropic-messages.ts:1011-1182).
///
/// Takes messages that have ALREADY been through `transform_messages_with` — [`build_params`]
/// hoists that call so the deferred-tool split sees the same list this does (Pi :947-961).
pub(crate) fn convert_messages(
    transformed: &[Message],
    is_oauth: bool,
    cache_control: Option<&Value>,
    allow_empty_signature: bool,
    deferred_tool_names: &HashSet<String>,
    normalize_tool_name: &dyn Fn(&str) -> String,
) -> Vec<Value> {
    let mut params: Vec<Value> = Vec::new();
    // Declared once per request so a deferred tool is referenced exactly once (Pi :1125).
    let mut anchors = ToolAnchors {
        deferred_tool_names,
        loaded_tool_names: HashSet::new(),
        normalize_tool_name,
    };

    let mut i = 0;
    while let Some(msg) = transformed.get(i) {
        match msg {
            Message::User { content, .. } => {
                if let Some(value) = build_user(content) {
                    params.push(value);
                }
            }
            Message::Assistant(am) => {
                if let Some(value) = build_assistant(am, is_oauth, allow_empty_signature) {
                    params.push(value);
                }
            }
            Message::ToolResult { .. } => {
                // Collect consecutive tool results into one `user` message of `tool_result` blocks.
                let mut tool_results: Vec<Value> = Vec::new();
                // Displaced content is accumulated across the WHOLE consecutive run and flushed
                // once, AFTER every `tool_result` block of the batch (Pi :1226-1252) — not
                // interleaved per block.
                let mut sibling_content: Vec<Value> = Vec::new();
                let mut j = i;
                while let Some(Message::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                    added_tool_names,
                    ..
                }) = transformed.get(j)
                {
                    let (tr, siblings) = convert_tool_result(
                        tool_call_id.as_str(),
                        content,
                        *is_error,
                        added_tool_names,
                        is_oauth,
                        &mut anchors,
                    );
                    tool_results.push(tr);
                    sibling_content.extend(siblings);
                    j += 1;
                }
                i = j;
                tool_results.extend(sibling_content);
                params.push(json!({ "role": "user", "content": Value::Array(tool_results) }));
                continue;
            }
        }
        i += 1;
    }

    // cache_control on the last user message's last block (Pi anthropic-messages.ts:1157-1179).
    if let Some(cc) = cache_control {
        apply_last_user_cache_control(&mut params, cc);
    }

    params
}

/// Build a `user` message; `None` when it has no non-empty content (Pi anthropic-messages.ts:1026-1063).
fn build_user(content: &[Content]) -> Option<Value> {
    let only_text = content.iter().all(|c| matches!(c, Content::Text { .. }));
    if only_text {
        let joined = content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if joined.trim().is_empty() {
            return None;
        }
        return Some(json!({ "role": "user", "content": sanitize_surrogates(&joined) }));
    }

    let mut blocks: Vec<Value> = Vec::new();
    for block in content {
        match block {
            Content::Text { text, .. } => {
                if text.trim().is_empty() {
                    continue;
                }
                blocks.push(json!({ "type": "text", "text": sanitize_surrogates(text) }));
            }
            Content::Image { data, mime_type } => blocks.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": mime_type, "data": data },
            })),
            _ => {}
        }
    }
    if blocks.is_empty() {
        return None;
    }
    Some(json!({ "role": "user", "content": Value::Array(blocks) }))
}

/// Build an `assistant` message; `None` when it has no content blocks (Pi
/// anthropic-messages.ts:1064-1120).
fn build_assistant(
    am: &AssistantMessage,
    is_oauth: bool,
    allow_empty_signature: bool,
) -> Option<Value> {
    let mut blocks: Vec<Value> = Vec::new();
    for block in &am.content {
        match block {
            Content::Text { text, .. } => {
                if text.trim().is_empty() {
                    continue;
                }
                blocks.push(json!({ "type": "text", "text": sanitize_surrogates(text) }));
            }
            Content::Thinking {
                thinking,
                thinking_signature,
                redacted,
            } => {
                if *redacted {
                    blocks.push(json!({
                        "type": "redacted_thinking",
                        "data": thinking_signature.clone().unwrap_or_default(),
                    }));
                    continue;
                }
                if thinking.trim().is_empty() {
                    continue;
                }
                let sig_empty = thinking_signature
                    .as_ref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true);
                if sig_empty {
                    if allow_empty_signature {
                        blocks.push(json!({
                            "type": "thinking",
                            "thinking": sanitize_surrogates(thinking),
                            "signature": "",
                        }));
                    } else {
                        blocks
                            .push(json!({ "type": "text", "text": sanitize_surrogates(thinking) }));
                    }
                } else {
                    blocks.push(json!({
                        "type": "thinking",
                        "thinking": sanitize_surrogates(thinking),
                        "signature": thinking_signature.clone().unwrap_or_default(),
                    }));
                }
            }
            Content::ToolCall(tc) => {
                let name = if is_oauth {
                    to_claude_code_name(&tc.name)
                } else {
                    tc.name.clone()
                };
                blocks.push(json!({
                    "type": "tool_use",
                    "id": tc.id.as_str(),
                    "name": name,
                    "input": Value::Object(tc.arguments.clone()),
                }));
            }
            _ => {}
        }
    }
    if blocks.is_empty() {
        return None;
    }
    Some(json!({ "role": "assistant", "content": Value::Array(blocks) }))
}

/// Add `cache_control` to the last user message's last cache-eligible block (Pi
/// anthropic-messages.ts:1157-1179).
fn apply_last_user_cache_control(params: &mut [Value], cc: &Value) {
    let Some(last) = params.last_mut() else {
        return;
    };
    if last.get("role").and_then(Value::as_str) != Some("user") {
        return;
    }
    match last.get_mut("content") {
        Some(Value::Array(arr)) => {
            if let Some(block) = arr.last_mut()
                && let Some(o) = block.as_object_mut()
            {
                let kind = o.get("type").and_then(Value::as_str);
                if matches!(kind, Some("text") | Some("image") | Some("tool_result")) {
                    o.insert("cache_control".to_string(), cc.clone());
                }
            }
        }
        Some(Value::String(_)) => {
            if let Some(Value::String(s)) = last.get("content") {
                let text = s.clone();
                if let Some(o) = last.as_object_mut() {
                    o.insert(
                        "content".to_string(),
                        json!([{ "type": "text", "text": text, "cache_control": cc.clone() }]),
                    );
                }
            }
        }
        _ => {}
    }
}

/// Map cyrup [`ToolDef`]s to Anthropic `tools` (Pi `convertTools`, anthropic-messages.ts:1188-1211).
/// `cache_control` is applied to the last tool only; `eager_input_streaming` when supported.
///
/// `defer_loading` marks a tool as transcript-anchored (DRIFT-001): it still ships in
/// `params.tools`, but the model only "sees" it at the `tool_reference` that names it. It is
/// inserted where Pi spreads it — after `input_schema`, before `cache_control` (Pi :1315-1321) —
/// though the workspace's `serde_json` has no `preserve_order` feature, so the serialized key
/// order is alphabetical either way and only the key SET is observable on the wire.
pub(crate) fn convert_tools(
    tools: &[ToolDef],
    is_oauth: bool,
    supports_eager: bool,
    supports_strict_tools: bool,
    cache_control: Option<&Value>,
    defer_loading: bool,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    let last = tools.len().saturating_sub(1);
    tools
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            // PROV-011 — `anthropic-messages.ts:1298` @v0.83.0.
            let strict = resolve_json_schema_strict_sampling(tool, supports_strict_tools)?;
            let name = if is_oauth {
                to_claude_code_name(&tool.name)
            } else {
                tool.name.clone()
            };
            let properties = tool
                .parameters
                .get("properties")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let required = tool
                .parameters
                .get("required")
                .cloned()
                .unwrap_or_else(|| json!([]));
            // `legacyInputSchema` (`:1300-1304`) — the three-key subset Anthropic has always
            // accepted. Under strict sampling pi sends the WHOLE schema with that subset spread
            // over it (`:1305-1311`), so `type`/`properties`/`required` still win and any extra
            // keyword (`$defs`, `additionalProperties`, …) survives for the constrainer.
            //
            // Built as a `Map` rather than via `json!` so the strict arm can spread it without
            // an `as_object().expect(..)` round-trip — the workspace denies `expect_used`, and
            // an infallible construction is stronger than a justified panic either way.
            let mut legacy = Map::new();
            legacy.insert("type".to_string(), json!("object"));
            legacy.insert("properties".to_string(), properties);
            legacy.insert("required".to_string(), required);
            let input_schema = if strict == Some(true) {
                let mut merged = tool
                    .parameters
                    .as_object()
                    .cloned()
                    .unwrap_or_else(Map::new);
                for (k, v) in &legacy {
                    merged.insert(k.clone(), v.clone());
                }
                Value::Object(merged)
            } else {
                Value::Object(legacy)
            };
            let mut o = Map::new();
            o.insert("name".to_string(), json!(name));
            o.insert("description".to_string(), json!(tool.description));
            if supports_eager {
                o.insert("eager_input_streaming".to_string(), json!(true));
            }
            // `...(strict === true ? { strict: true } : {})` (`:1317`) — inserted where pi spreads
            // it, between `eager_input_streaming` and `input_schema`. As with `defer_loading`
            // above, that insertion order is for readability against pi only: `serde_json`'s `Map`
            // is a `BTreeMap` here, so the wire order is lexicographic regardless.
            if strict == Some(true) {
                o.insert("strict".to_string(), json!(true));
            }
            o.insert("input_schema".to_string(), input_schema);
            if defer_loading {
                o.insert("defer_loading".to_string(), json!(true));
            }
            if let Some(cc) = cache_control
                && index == last
            {
                o.insert("cache_control".to_string(), cc.clone());
            }
            Ok(Value::Object(o))
        })
        .collect()
}

/// Map an Anthropic `stop_reason` to a cyrup [`StopReason`] (Pi `mapStopReason`,
/// anthropic-messages.ts:1325-1351 @ v0.83.0). Unknown reasons return an error with a message.
///
/// Every arm that yields [`StopReason::Error`] must also yield a message: Pi surfaces it as
/// `throw new Error(output.errorMessage || "An unknown error occurred")`
/// (anthropic-messages.ts:755), so a `None` here silently degrades to that generic fallback.
fn map_stop_reason(reason: &str, stop_details: Option<&Value>) -> (StopReason, Option<String>) {
    match reason {
        "end_turn" => (StopReason::Stop, None),
        "max_tokens" => (StopReason::Length, None),
        "tool_use" => (StopReason::ToolUse, None),
        "refusal" => {
            let explanation = stop_details
                .and_then(|d| d.get("explanation"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| "The model refused to complete the request".to_string());
            (StopReason::Error, Some(explanation))
        }
        "pause_turn" | "stop_sequence" => (StopReason::Stop, None),
        // Content flagged by Anthropic's safety filters (not yet in the SDK types). The message is
        // load-bearing: without it the terminal falls through to the generic
        // `"An unknown error occurred"` fallback (`decode_stream`, below) and a content-policy stop
        // becomes indistinguishable from a transport failure.
        "sensitive" => (
            StopReason::Error,
            Some("Provider stopped with: sensitive".to_string()),
        ),
        other => (
            StopReason::Error,
            Some(format!("Unhandled stop reason: {other}")),
        ),
    }
}

// ---------------------------------------------------------------------------
// Response decoding
// ---------------------------------------------------------------------------

/// One in-progress content block, keyed by the Anthropic `index`.
enum Block {
    Text {
        index: i64,
        text: String,
    },
    Thinking {
        index: i64,
        thinking: String,
        signature: String,
        redacted: bool,
    },
    Tool {
        index: i64,
        id: String,
        name: String,
        partial_json: String,
    },
}

impl Block {
    fn index(&self) -> i64 {
        match self {
            Block::Text { index, .. }
            | Block::Thinking { index, .. }
            | Block::Tool { index, .. } => *index,
        }
    }
}

/// Streaming-decode state (mirrors Pi's `output` accumulation, anthropic-messages.ts:476-715).
#[derive(Default)]
struct Decoder {
    blocks: Vec<Block>,
    usage: Usage,
    response_id: Option<String>,
    stop_reason: Option<StopReason>,
    /// The provider's own `stop_reason` string, kept verbatim beside the narrowed [`StopReason`]
    /// (pi `output.rawStopReason = event.delta.stop_reason`,
    /// `v0.84.1 ai/src/api/anthropic-messages.ts:709`). PORT BUG, not version lag: the write is
    /// present at v0.83.0 too (`v0.83.0 ai/src/api/anthropic-messages.ts:709`) and cyrup never
    /// ported it, so `rawStopReason` was `None` on every anthropic turn. Set once, from
    /// `message_delta`, and never cleared — a mapped `tool_use`/`refusal` still names itself.
    raw_stop_reason: Option<String>,
    error_message: Option<String>,
    saw_message_start: bool,
    saw_message_stop: bool,
    /// OAuth replay remaps decoded tool names back to the caller's declared names (Pi
    /// `fromClaudeCodeName`, anthropic-messages.ts:592-594).
    is_oauth: bool,
    tool_names: Vec<String>,
}

impl Decoder {
    fn position_of(&self, index: i64) -> Option<usize> {
        self.blocks.iter().position(|b| b.index() == index)
    }

    /// Build the live `partial` snapshot.
    fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone();
        apply_cost(&model.cost, &mut usage);
        AssistantMessage {
            content: blocks_to_content(&self.blocks),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: self.response_id.clone(),
            diagnostics: None,
            usage,
            // In-flight: Pi's `output.stopReason` is still its `"pending"` seed until a
            // `message_delta` carries one (anthropic-messages.ts:509,714-717), and `output` IS the
            // `partial` attached to every non-terminal event. The TERMINAL never takes this value —
            // it goes through `StreamEvent::end_of_stream`, which rewrites `Pending` to the `error`
            // terminal Pi's throw produces.
            stop_reason: self.stop_reason.unwrap_or(StopReason::Pending),
            deferred: None,
            error_message: self.error_message.clone(),
            raw_stop_reason: self.raw_stop_reason.clone(),
            timestamp: now_millis(),
        }
    }
}

fn blocks_to_content(blocks: &[Block]) -> Vec<Content> {
    blocks
        .iter()
        .map(|b| match b {
            Block::Text { text, .. } => Content::text(text.clone()),
            Block::Thinking {
                thinking,
                signature,
                redacted,
                ..
            } => Content::Thinking {
                thinking: thinking.clone(),
                thinking_signature: if signature.is_empty() {
                    None
                } else {
                    Some(signature.clone())
                },
                redacted: *redacted,
            },
            Block::Tool {
                id,
                name,
                partial_json,
                ..
            } => Content::ToolCall(ToolCall {
                id: ToolCallId::from(id.as_str()),
                name: name.clone(),
                arguments: parse_streaming_json_object(Some(partial_json)),
                thought_signature: None,
            }),
        })
        .collect()
}

/// Drive the Anthropic SSE frame stream into ordered [`StreamEvent`]s (1:1 with Pi's stream loop,
/// anthropic-messages.ts:546-737).
pub(crate) async fn decode_stream<S>(
    mut frames: S,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    is_oauth: bool,
    tools: &[ToolDef],
) where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    let provider = model.provider.clone();
    let model_id = model.id.as_str().to_string();

    let mut dec = Decoder {
        is_oauth,
        tool_names: tools.iter().map(|t| t.name.clone()).collect(),
        ..Default::default()
    };
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
        // An `event: error` frame surfaces the data as an error (Pi anthropic-messages.ts:439-441).
        if frame.event == "error" {
            let msg = if frame.data.trim().is_empty() {
                "Anthropic stream error".to_string()
            } else {
                frame.data.clone()
            };
            emit_error(&dec, model, api, sink, msg).await;
            return;
        }
        if !is_message_event(&frame.event) {
            continue;
        }
        let data = frame.data.trim();
        if data.is_empty() {
            continue;
        }
        let Some(event) = parse_json_with_repair(data) else {
            emit_error(
                &dec,
                model,
                api,
                sink,
                format!("Could not parse Anthropic SSE event {}", frame.event),
            )
            .await;
            return;
        };
        if !process_event(&event, &mut dec, model, api, sink).await {
            return; // consumer dropped
        }
        if dec.stop_reason == Some(StopReason::Error) {
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
    }

    // Stream ended. A `message_start` with no `message_stop` is a protocol error (Pi
    // anthropic-messages.ts:463-465).
    if dec.saw_message_start && !dec.saw_message_stop {
        emit_error(
            &dec,
            model,
            api,
            sink,
            "Anthropic stream ended before message_stop".to_string(),
        )
        .await;
        return;
    }

    finish_blocks(&dec, model, api, sink).await;
    // A stream that ran to EOF without a `message_delta.stop_reason` is TRUNCATED, not complete.
    // `dec.stop_reason == None` is cyrup's spelling of Pi's still-`"pending"` output, and
    // `end_of_stream` turns it into the same `error` terminal Pi's throw produces
    // (anthropic-messages.ts:751-753) instead of the clean `stop` this used to default to.
    sink.send(StreamEvent::end_of_stream(
        dec.snapshot(model, api),
        dec.stop_reason,
        "Anthropic stream ended without a stop reason",
    ))
    .await;
}

/// Whether `event` is one of the six Anthropic message events (Pi `ANTHROPIC_MESSAGE_EVENTS`).
fn is_message_event(event: &str) -> bool {
    matches!(
        event,
        "message_start"
            | "message_delta"
            | "message_stop"
            | "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
    )
}

/// Process one decoded Anthropic event. Returns `false` if the consumer dropped the stream.
async fn process_event(
    event: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    match event.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            dec.saw_message_start = true;
            if let Some(message) = event.get("message") {
                if let Some(id) = message.get("id").and_then(Value::as_str) {
                    dec.response_id = Some(id.to_string());
                }
                if let Some(usage) = message.get("usage") {
                    apply_message_start_usage(&mut dec.usage, usage);
                }
            }
            true
        }
        Some("content_block_start") => process_block_start(event, dec, model, api, sink).await,
        Some("content_block_delta") => process_block_delta(event, dec, model, api, sink).await,
        Some("content_block_stop") => process_block_stop(event, dec, model, api, sink).await,
        Some("message_delta") => {
            // pi guards with `if (event.delta.stop_reason)` (`v0.84.1
            // ai/src/api/anthropic-messages.ts:708`) — a JS truthiness test, so `""` leaves the
            // `"pending"` seed alone rather than mapping to `Unhandled stop reason: `.
            if let Some(delta) = event.get("delta")
                && let Some(reason) = delta
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .filter(|r| !r.is_empty())
            {
                // The raw string is recorded FIRST and unconditionally, exactly where pi records it
                // (`v0.84.1 ai/src/api/anthropic-messages.ts:709`), so a turn that maps to
                // `tool_use`/`refusal`/an unknown reason still carries the provider's own word.
                dec.raw_stop_reason = Some(reason.to_string());
                let (stop, err) = map_stop_reason(reason, delta.get("stop_details"));
                dec.stop_reason = Some(stop);
                if let Some(err) = err {
                    dec.error_message = Some(err);
                }
            }
            if let Some(usage) = event.get("usage") {
                apply_message_delta_usage(&mut dec.usage, usage);
            }
            true
        }
        Some("message_stop") => {
            dec.saw_message_stop = true;
            true
        }
        _ => true,
    }
}

async fn process_block_start(
    event: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
    let cb = match event.get("content_block") {
        Some(c) => c,
        None => return true,
    };
    match cb.get("type").and_then(Value::as_str) {
        Some("text") => {
            // Seed from the payload Anthropic ships on the open event (Pi
            // `text: event.content_block.text ?? ""`, anthropic-messages.ts:591). Dropping it loses
            // the first chunk of the block whenever the server front-loads text here.
            dec.blocks.push(Block::Text {
                index,
                text: cb
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            });
            send_with_pos(dec, model, api, sink, |pos, partial| {
                StreamEvent::TextStart {
                    content_index: pos,
                    partial,
                }
            })
            .await
        }
        Some("thinking") => {
            // Same seeding for thinking (Pi `thinking: event.content_block.thinking ?? ""`,
            // `thinkingSignature: event.content_block.signature ?? ""`, anthropic-messages.ts:
            // 599-600). The signature especially: a thinking block replayed back to Anthropic
            // without its signature is rejected, so a server that delivers the signature on the
            // open event (and never as a `signature_delta`) must not have it discarded.
            dec.blocks.push(Block::Thinking {
                index,
                thinking: cb
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                signature: cb
                    .get("signature")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                redacted: false,
            });
            send_with_pos(dec, model, api, sink, |pos, partial| {
                StreamEvent::ThinkingStart {
                    content_index: pos,
                    partial,
                }
            })
            .await
        }
        Some("redacted_thinking") => {
            let data = cb
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            dec.blocks.push(Block::Thinking {
                index,
                thinking: "[Reasoning redacted]".to_string(),
                signature: data,
                redacted: true,
            });
            send_with_pos(dec, model, api, sink, |pos, partial| {
                StreamEvent::ThinkingStart {
                    content_index: pos,
                    partial,
                }
            })
            .await
        }
        Some("tool_use") => {
            let id = cb
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let raw_name = cb.get("name").and_then(Value::as_str).unwrap_or("");
            // OAuth: map the Claude-Code tool name back to the caller's declared name (Pi decode,
            // anthropic-messages.ts:592-594).
            let name = if dec.is_oauth {
                remap_decoded_tool_name(&dec.tool_names, raw_name)
            } else {
                raw_name.to_string()
            };
            dec.blocks.push(Block::Tool {
                index,
                id,
                name,
                partial_json: String::new(),
            });
            send_with_pos(dec, model, api, sink, |pos, partial| {
                StreamEvent::ToolCallStart {
                    content_index: pos,
                    partial,
                }
            })
            .await
        }
        _ => true,
    }
}

async fn process_block_delta(
    event: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
    let delta = match event.get("delta") {
        Some(d) => d,
        None => return true,
    };
    let pos = match dec.position_of(index) {
        Some(p) => p,
        None => return true,
    };
    match delta.get("type").and_then(Value::as_str) {
        Some("text_delta") => {
            let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
            if let Some(Block::Text { text: acc, .. }) = dec.blocks.get_mut(pos) {
                acc.push_str(text);
            }
            let partial = dec.snapshot(model, api);
            sink.send(StreamEvent::TextDelta {
                content_index: pos,
                delta: text.to_string(),
                partial,
            })
            .await
        }
        Some("thinking_delta") => {
            let text = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
            if let Some(Block::Thinking { thinking, .. }) = dec.blocks.get_mut(pos) {
                thinking.push_str(text);
            }
            let partial = dec.snapshot(model, api);
            sink.send(StreamEvent::ThinkingDelta {
                content_index: pos,
                delta: text.to_string(),
                partial,
            })
            .await
        }
        Some("input_json_delta") => {
            let text = delta
                .get("partial_json")
                .and_then(Value::as_str)
                .unwrap_or("");
            if let Some(Block::Tool { partial_json, .. }) = dec.blocks.get_mut(pos) {
                partial_json.push_str(text);
            }
            let partial = dec.snapshot(model, api);
            sink.send(StreamEvent::ToolCallDelta {
                content_index: pos,
                delta: text.to_string(),
                partial,
            })
            .await
        }
        Some("signature_delta") => {
            let sig = delta.get("signature").and_then(Value::as_str).unwrap_or("");
            if let Some(Block::Thinking { signature, .. }) = dec.blocks.get_mut(pos) {
                signature.push_str(sig);
            }
            true // signature deltas do not emit a stream event (Pi anthropic-messages.ts:640-647)
        }
        _ => true,
    }
}

async fn process_block_stop(
    event: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
    let pos = match dec.position_of(index) {
        Some(p) => p,
        None => return true,
    };
    let partial = dec.snapshot(model, api);
    let ev = match dec.blocks.get(pos) {
        Some(Block::Text { text, .. }) => StreamEvent::TextEnd {
            content_index: pos,
            content: text.clone(),
            partial,
        },
        Some(Block::Thinking { thinking, .. }) => StreamEvent::ThinkingEnd {
            content_index: pos,
            content: thinking.clone(),
            partial,
        },
        Some(Block::Tool {
            id,
            name,
            partial_json,
            ..
        }) => StreamEvent::ToolCallEnd {
            content_index: pos,
            tool_call: ToolCall {
                id: ToolCallId::from(id.as_str()),
                name: name.clone(),
                arguments: parse_streaming_json_object(Some(partial_json)),
                thought_signature: None,
            },
            partial,
        },
        None => return true,
    };
    sink.send(ev).await
}

/// Push a `*_start` event for the just-pushed block (its position is `len-1`).
async fn send_with_pos<F>(
    dec: &Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    make: F,
) -> bool
where
    F: FnOnce(usize, AssistantMessage) -> StreamEvent,
{
    let pos = dec.blocks.len().saturating_sub(1);
    let partial = dec.snapshot(model, api);
    sink.send(make(pos, partial)).await
}

/// Emit any block `*_end` events the stream did not already close (no-op when all closed cleanly,
/// which is the normal path — Anthropic always sends `content_block_stop`).
async fn finish_blocks(_dec: &Decoder, _model: &Model, _api: &ApiId, _sink: &EventSink) {
    // Anthropic always emits a `content_block_stop` per block, so the `*_end` events are already
    // sent by `process_block_stop`. This hook exists for symmetry with the openai-completions
    // decoder and is intentionally a no-op.
}

/// Emit a terminal error event carrying the partial snapshot's content (Pi's catch block,
/// anthropic-messages.ts:727-736).
async fn emit_error(dec: &Decoder, model: &Model, api: &ApiId, sink: &EventSink, message: String) {
    let mut msg = dec.snapshot(model, api);
    msg.stop_reason = StopReason::Error;
    msg.error_message = Some(message);
    sink.send(StreamEvent::terminal(msg)).await;
}

/// Apply `message_start` usage (Pi anthropic-messages.ts:551-558): seeds input/output/cache counts.
fn apply_message_start_usage(usage: &mut Usage, raw: &Value) {
    usage.input = raw.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
    usage.output = raw
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.cache_read = raw
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.cache_write = raw
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let long = raw
        .get("cache_creation")
        .and_then(|c| c.get("ephemeral_1h_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.cache_write_1h = Some(long);
}

/// Apply `message_delta` usage (Pi anthropic-messages.ts:690-709): only present fields update,
/// preserving `input_tokens` from `message_start` when a proxy omits it.
fn apply_message_delta_usage(usage: &mut Usage, raw: &Value) {
    if let Some(v) = raw.get("input_tokens").and_then(Value::as_u64) {
        usage.input = v;
    }
    if let Some(v) = raw.get("output_tokens").and_then(Value::as_u64) {
        usage.output = v;
    }
    if let Some(v) = raw.get("cache_read_input_tokens").and_then(Value::as_u64) {
        usage.cache_read = v;
    }
    if let Some(v) = raw
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
    {
        usage.cache_write = v;
    }
    if let Some(v) = raw
        .get("output_tokens_details")
        .and_then(|d| d.get("thinking_tokens"))
        .and_then(Value::as_u64)
    {
        usage.reasoning = Some(v);
    }
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
    use crate::auth::types::ModelAuth;
    use crate::model::{Modality, ModelCost};
    use crate::stream::sse::decode_sse_bytes;
    use cyrup_core::{AssistantMessage, Message, ModelThinkingLevel, ProviderId};

    fn auth_with(api_key: Option<&str>) -> AuthResult {
        AuthResult {
            auth: ModelAuth {
                api_key: api_key.map(String::from),
                ..Default::default()
            },
            env: None,
            source: None,
        }
    }

    fn model() -> Model {
        Model {
            id: "claude-opus-4-5".into(),
            name: "Claude Opus 4.5".into(),
            api: API_ID.into(),
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com".to_string(),
            reasoning: true,
            input: vec![Modality::Text, Modality::Image],
            cost: ModelCost {
                input: 5.0,
                output: 25.0,
                cache_read: 0.5,
                cache_write: 6.25,
                tiers: None,
            },
            context_window: 200_000,
            max_tokens: 64_000,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    // PROV-011 — `resolveJsonSchemaStrictSampling` on the Anthropic route
    // (`anthropic-messages.ts:1298` @v0.83.0, `supportsStrictTools` read at `:183`).
    #[test]
    fn constrained_sampling_drives_anthropic_strict_tools() {
        use crate::api::compat::AnthropicMessagesCompat;
        use crate::context::{ConstrainedSampling, ConstrainedSamplingConfig, StrictSampling};
        use crate::utils::constrained_sampling::ConstrainedSamplingError;

        let strict_tool = |strict| ToolDef {
            name: "Edit".into(),
            description: "edit a file".into(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
                "additionalProperties": false,
            }),
            constrained_sampling: Some(ConstrainedSampling::Config(
                ConstrainedSamplingConfig::JsonSchema { strict },
            )),
        };

        // (a) No `supportsStrictTools` (the default) + `prefer` ⇒ degrade silently: no `strict`
        // key, and the legacy three-key `input_schema` — byte-identical to a plain tool.
        let mut ctx = user_ctx("hi");
        ctx.tools = vec![strict_tool(StrictSampling::Prefer)];
        let body = build_body(&model(), &ctx, &StreamOptions::default());
        assert!(body["tools"][0].get("strict").is_none());
        assert!(
            body["tools"][0]["input_schema"]
                .get("additionalProperties")
                .is_none(),
            "the non-strict branch sends only pi's legacy type/properties/required subset"
        );

        // (b) `supportsStrictTools` ⇒ `strict: true` AND the whole schema, with pi's legacy subset
        // spread over it so `type`/`properties`/`required` still win.
        let mut m = model();
        m.compat = Some(AnthropicMessagesCompat {
            supports_strict_tools: Some(true),
            ..Default::default()
        });
        let body = build_body(&m, &ctx, &StreamOptions::default());
        assert_eq!(body["tools"][0]["strict"], json!(true));
        assert_eq!(
            body["tools"][0]["input_schema"]["additionalProperties"],
            json!(false)
        );
        assert_eq!(body["tools"][0]["input_schema"]["type"], json!("object"));
        // The EXACT key set of a strict tool. pi's object literal is written
        // `name, description, eager_input_streaming?, strict?, input_schema, defer_loading?,
        // cache_control?` (`anthropic-messages.ts:1313-1321` @v0.83.0), but a JSON object is
        // unordered and this workspace's `serde_json` has no `preserve_order` feature, so `Map` is
        // a `BTreeMap` and emission order is lexicographic — only the key SET is observable on the
        // wire. pi asserts key sets the same way, `.sort()`ed
        // (`packages/ai/test/bedrock-error-metadata.test.ts:117`). Equality on the whole vector
        // still pins it exactly: no missing key, no extra key.
        //
        // `cache_control` IS part of that set here. `StreamOptions::default()` leaves
        // `cacheRetention` unset, which resolves to "short" (`:49-57`), so `getCacheControl`
        // returns `{ type: "ephemeral" }` (`:59-73`); `supportsCacheControlOnTools` defaults to
        // TRUE (`:180`) so `buildParams` forwards it to `convertTools` (`:1014`), which stamps the
        // LAST tool — `index === tools.length - 1` (`:1320`) — and this context has exactly one.
        let keys: Vec<&str> = body["tools"][0]
            .as_object()
            .expect("tool is an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "cache_control",
                "description",
                "eager_input_streaming",
                "input_schema",
                "name",
                "strict"
            ]
        );
        // Its VALUE is the plain ephemeral marker for "short" retention. `build_body` passes no
        // env overlay, so `resolve_cache_retention` falls through to the ambient
        // `PI_CACHE_RETENTION` (`:371-378`, pi `:49-57`); pin the overlay so an exported "long" in
        // the developer's shell cannot swap in the 1h-ttl variant — that branch is pinned
        // separately by `tools_encode_eager_streaming_and_cache_control`.
        let short = ProviderEnv::from([("PI_CACHE_RETENTION".into(), "short".into())]);
        let pinned = build_params(&m, &ctx, &StreamOptions::default(), Some(&short), false)
            .expect("supports_strict_tools satisfies the `prefer` tool");
        assert_eq!(
            pinned["tools"][0]["cache_control"],
            json!({"type": "ephemeral"})
        );

        // (c) `require` on a model without strict tools fails the whole turn, with pi's text.
        ctx.tools = vec![strict_tool(StrictSampling::Require)];
        assert_eq!(
            build_params(&model(), &ctx, &StreamOptions::default(), None, false),
            Err(ConstrainedSamplingError(
                "Tool \"Edit\" requires JSON-schema constrained sampling, but strict tools are unsupported."
                    .to_string()
            ))
        );
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
    fn url_appends_v1_messages() {
        assert_eq!(
            messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            messages_url("https://api.anthropic.com/"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            messages_url("https://api.anthropic.com/v1/messages"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn build_params_basic_shape() {
        let m = model();
        let opts = StreamOptions {
            max_tokens: Some(1000),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("hello"), &opts);
        assert_eq!(body["model"], "claude-opus-4-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 1000);
        // system prompt is an array of text blocks.
        assert_eq!(body["system"][0]["type"], "text");
        assert_eq!(body["system"][0]["text"], "be brief");
        // user message text-only; default (short) retention applies cache_control to the last user
        // message, so the string is promoted to a single cached text block (Pi convertMessages).
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        // A reasoning model with reasoning Off (and `off` not null-marked) emits thinking:disabled
        // (Pi buildParams: `thinkingEnabled === false && thinkingLevelMap?.off !== null`).
        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn budget_thinking_encodes_enabled_with_budget() {
        let m = model(); // not adaptive
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            max_tokens: Some(4000),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("think"), &opts);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["display"], "summarized");
        assert!(body["thinking"]["budget_tokens"].as_u64().unwrap() > 0);
        // temperature omitted while thinking is enabled.
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn thinking_display_per_api_option_overrides_default() {
        // Pi `options.thinkingDisplay ?? "summarized"` (anthropic-messages.ts:962). The typed
        // per-API option flips the emitted `thinking.display` for both budget and adaptive thinking.
        let m = model(); // not adaptive
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            max_tokens: Some(4000),
            api_options: Some(crate::stream::ApiStreamOptions::Anthropic(
                AnthropicOptions {
                    thinking_display: Some(AnthropicThinkingDisplay::Omitted),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("think"), &opts);
        assert_eq!(body["thinking"]["display"], "omitted");

        // Adaptive model carries the same display through to the adaptive thinking block.
        let mut adaptive = model();
        adaptive.compat = Some(ModelCompat {
            force_adaptive_thinking: Some(true),
            ..Default::default()
        });
        let body = build_body(&adaptive, &user_ctx("deep"), &opts);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["thinking"]["display"], "omitted");
    }

    #[test]
    fn custom_thinking_budgets_override_default_budget_tokens() {
        // Pi `streamSimple` forwards `options.thinkingBudgets` into `adjustMaxTokensForThinking`
        // (anthropic-messages.ts:792-797). A custom `high` budget must override the built-in default
        // (16_384) in the emitted `thinking.budget_tokens`.
        let m = model(); // not adaptive, max_tokens 64_000, window 200_000
        let custom = crate::utils::simple_options::ThinkingBudgets {
            high: Some(30_000),
            ..Default::default()
        };
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            thinking_budgets: Some(custom),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("think"), &opts);
        assert_eq!(body["thinking"]["budget_tokens"].as_u64().unwrap(), 30_000);
        // Sanity: without the override the default (16_384) is used, proving the field threads.
        let default_opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let default_body = build_body(&m, &user_ctx("think"), &default_opts);
        assert_eq!(
            default_body["thinking"]["budget_tokens"].as_u64().unwrap(),
            16_384
        );
    }

    #[test]
    fn adaptive_thinking_encodes_effort() {
        let mut m = model();
        m.compat = Some(ModelCompat {
            force_adaptive_thinking: Some(true),
            ..Default::default()
        });
        m.thinking_level_map = Some(
            [("xhigh".to_string(), Some("xhigh".to_string()))]
                .into_iter()
                .collect(),
        );
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::Xhigh,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("deep"), &opts);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "xhigh");
    }

    /// PROV-002: the `max` rung must reach `output_config.effort` as `"max"` (Pi
    /// `mapThinkingLevelToEffort`, anthropic-messages.ts:781-799 — the map lookup wins).
    #[test]
    fn adaptive_thinking_encodes_max_effort() {
        let mut m = model();
        m.compat = Some(ModelCompat {
            force_adaptive_thinking: Some(true),
            ..Default::default()
        });
        m.thinking_level_map = Some(
            [("max".to_string(), Some("max".to_string()))]
                .into_iter()
                .collect(),
        );
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::Max,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("deepest"), &opts);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "max");
    }

    /// With no `thinkingLevelMap` entry, `max` falls to Pi's `default: "high"` arm — it must NOT
    /// leak a bare `"max"` to a model that never advertised the effort.
    #[test]
    fn unmapped_max_falls_back_to_high_effort() {
        let mut m = model();
        m.compat = Some(ModelCompat {
            force_adaptive_thinking: Some(true),
            ..Default::default()
        });
        m.thinking_level_map = None;
        let body = build_body(
            &m,
            &user_ctx("x"),
            &StreamOptions {
                reasoning: ModelThinkingLevel::Max,
                ..Default::default()
            },
        );
        assert_eq!(body["output_config"]["effort"], "high");
    }

    /// The real-catalog end of PROV-002: on `claude-opus-4-6` the level the UI DISPLAYS and the
    /// effort that goes on the wire must be the same string. Before the fix the catalog carried
    /// `{"xhigh":"max"}`, so the footer said `xhigh` while Anthropic received `max`.
    #[test]
    fn max_label_matches_the_wire_effort_on_opus_4_6() {
        use crate::collection::{clamp_thinking_level, get_supported_thinking_levels};
        let m = crate::providers::anthropic_models()
            .iter()
            .find(|m| m.id.as_str() == "claude-opus-4-6")
            .expect("opus-4-6")
            .clone();

        // The only top rung this model offers is `max`; a request for `xhigh` promotes onto it.
        let levels = get_supported_thinking_levels(&m);
        assert!(levels.contains(&ModelThinkingLevel::Max), "{levels:?}");
        assert!(!levels.contains(&ModelThinkingLevel::Xhigh), "{levels:?}");
        let selected = clamp_thinking_level(&m, ModelThinkingLevel::Xhigh);
        assert_eq!(selected, ModelThinkingLevel::Max);

        let body = build_body(
            &m,
            &user_ctx("x"),
            &StreamOptions {
                reasoning: selected,
                ..Default::default()
            },
        );
        let wire = body["output_config"]["effort"].as_str().expect("effort");
        assert_eq!(wire, "max");
        assert_eq!(
            crate::api::compat::thinking_level_key(selected),
            wire,
            "displayed level and wire effort must agree"
        );
    }

    /// `claude-sonnet-5` advertises BOTH top rungs and they must stay distinct on the wire.
    #[test]
    fn sonnet_5_sends_xhigh_and_max_distinctly() {
        let m = crate::providers::anthropic_models()
            .iter()
            .find(|m| m.id.as_str() == "claude-sonnet-5")
            .expect("sonnet-5")
            .clone();
        let effort = |level| {
            build_body(
                &m,
                &user_ctx("x"),
                &StreamOptions {
                    reasoning: level,
                    ..Default::default()
                },
            )["output_config"]["effort"]
                .as_str()
                .map(str::to_string)
        };
        assert_eq!(effort(ModelThinkingLevel::Xhigh).as_deref(), Some("xhigh"));
        assert_eq!(effort(ModelThinkingLevel::Max).as_deref(), Some("max"));
    }

    #[test]
    fn disabled_thinking_when_off_map_not_null() {
        let mut m = model();
        // off not present => off !== null => disabled emitted.
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::Off,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["thinking"]["type"], "disabled");
        // when off is null-marked, no thinking key.
        m.thinking_level_map = Some([("off".to_string(), None)].into_iter().collect());
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn temperature_only_without_thinking_and_when_supported() {
        let mut m = model();
        m.reasoning = false;
        let opts = StreamOptions {
            temperature: Some(0.7),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 1e-6);
        // supportsTemperature=false suppresses it (Opus 4.7+).
        m.compat = Some(ModelCompat {
            supports_temperature: Some(false),
            ..Default::default()
        });
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn tools_encode_eager_streaming_and_cache_control() {
        let mut ctx = user_ctx("use a tool");
        ctx.tools = vec![ToolDef {
            name: "read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
            constrained_sampling: None,
        }];
        let m = model();
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::Long),
            ..Default::default()
        };
        let body = build_body(&m, &ctx, &opts);
        let tool = &body["tools"][0];
        assert_eq!(tool["name"], "read");
        assert_eq!(tool["eager_input_streaming"], true);
        assert_eq!(tool["input_schema"]["type"], "object");
        assert_eq!(tool["input_schema"]["required"][0], "path");
        // long retention => cache_control with 1h ttl on the last tool.
        assert_eq!(tool["cache_control"]["type"], "ephemeral");
        assert_eq!(tool["cache_control"]["ttl"], "1h");
    }

    #[test]
    fn fine_grained_beta_when_eager_unsupported() {
        let mut m = model();
        m.compat = Some(ModelCompat {
            supports_eager_tool_input_streaming: Some(false),
            ..Default::default()
        });
        let mut ctx = user_ctx("x");
        ctx.tools = vec![ToolDef {
            name: "read".to_string(),
            description: "d".to_string(),
            parameters: json!({}),
            constrained_sampling: None,
        }];
        let auth = auth_with(None);
        let headers = build_headers(&m, &ctx, &auth, &StreamOptions::default(), false);
        let beta = headers
            .get("anthropic-beta")
            .and_then(|v| v.clone())
            .unwrap_or_default();
        assert!(
            beta.contains(FINE_GRAINED_TOOL_STREAMING_BETA),
            "got: {beta}"
        );
        // tools omit eager_input_streaming when unsupported.
        let body = build_body(&m, &ctx, &StreamOptions::default());
        assert!(body["tools"][0].get("eager_input_streaming").is_none());
    }

    #[test]
    fn api_key_headers_and_version() {
        let m = model();
        let auth = auth_with(Some("sk-ant-api03-xxx"));
        let headers = build_headers(
            &m,
            &Context::default(),
            &auth,
            &StreamOptions::default(),
            false,
        );
        assert_eq!(
            headers.get("x-api-key").and_then(|v| v.clone()).as_deref(),
            Some("sk-ant-api03-xxx")
        );
        assert_eq!(
            headers
                .get("anthropic-version")
                .and_then(|v| v.clone())
                .as_deref(),
            Some(ANTHROPIC_VERSION)
        );
        // interleaved-thinking beta is sent for non-adaptive reasoning models.
        let beta = headers
            .get("anthropic-beta")
            .and_then(|v| v.clone())
            .unwrap_or_default();
        assert!(beta.contains(INTERLEAVED_THINKING_BETA));
    }

    /// PROV-056 — the wire half. The catalog now carries `forceAdaptiveThinking: true` on every
    /// `kimi-coding` row (pi `ai/scripts/generate-models.ts:1861-1864` @v0.83.0), and this asserts
    /// what that flag actually changes on the request, driven by the SHIPPED catalog rather than a
    /// synthetic model — because the defect was never in this file's logic, which was already a
    /// faithful port, but in the data that reaches it.
    ///
    /// Two divergences per request, on all three models, which is every model the provider has:
    /// cyrup sent a budget-based `thinking` block where pi sends `{type: "adaptive"}`
    /// (pi `anthropic-messages.ts:1033`), and it sent the `interleaved-thinking-2025-05-14` beta
    /// that pi suppresses for adaptive models (`:858`,
    /// `needsInterleavedBeta = interleavedThinking && model.compat?.forceAdaptiveThinking !== true`).
    /// Pre-fix both assertions are RED for all three rows.
    #[test]
    fn kimi_coding_catalog_rows_send_adaptive_thinking_and_no_interleaved_beta() {
        let auth = auth_with(Some("sk-ant-api03-xxx"));
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let models = crate::providers::anthropic::anthropic_fleet_spec("kimi-coding")
            .expect("kimi-coding fleet spec")
            .models();
        assert_eq!(models.len(), 5, "every kimi-coding row must be covered");

        for m in &models {
            let body = build_body(m, &user_ctx("think"), &opts);
            assert_eq!(
                body["thinking"]["type"], "adaptive",
                "kimi-coding/{} sends a non-adaptive thinking block to an upstream pi flags as \
                 requiring the adaptive format",
                m.id.as_str()
            );

            let beta = build_headers(m, &Context::default(), &auth, &opts, false)
                .get("anthropic-beta")
                .and_then(|v| v.clone())
                .unwrap_or_default();
            assert!(
                !beta.contains(INTERLEAVED_THINKING_BETA),
                "kimi-coding/{} sent the interleaved-thinking beta pi suppresses for adaptive \
                 models; beta header was {beta:?}",
                m.id.as_str()
            );
        }
    }

    #[test]
    fn interleaved_thinking_per_api_option_suppresses_beta() {
        // Pi `options?.interleavedThinking ?? true` (anthropic-messages.ts:520): an explicit
        // `false` drops the interleaved-thinking beta header that is otherwise sent by default.
        let m = model();
        let auth = auth_with(Some("sk-ant-api03-xxx"));

        let default_headers = build_headers(
            &m,
            &Context::default(),
            &auth,
            &StreamOptions::default(),
            false,
        );
        let default_beta = default_headers
            .get("anthropic-beta")
            .and_then(|v| v.clone())
            .unwrap_or_default();
        assert!(
            default_beta.contains(INTERLEAVED_THINKING_BETA),
            "beta on by default"
        );

        let opts = StreamOptions {
            api_options: Some(crate::stream::ApiStreamOptions::Anthropic(
                AnthropicOptions {
                    interleaved_thinking: Some(false),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        let headers = build_headers(&m, &Context::default(), &auth, &opts, false);
        let beta = headers
            .get("anthropic-beta")
            .and_then(|v| v.clone())
            .unwrap_or_default();
        assert!(
            !beta.contains(INTERLEAVED_THINKING_BETA),
            "explicit false suppresses the beta"
        );
    }

    /// PROV-027/PROV-028 — the Copilot branch of `createClient`
    /// (anthropic-messages.ts:867-888) plus the dynamic Copilot headers computed at `:525-531`.
    ///
    /// A real Copilot token is a `tid=…;exp=…;proxy-ep=…` claim string with no `sk-ant-oat`
    /// marker, so the `isOAuthToken` sniff cyrup used to key off cannot select Bearer for it: every
    /// request on Copilot's 9 anthropic-messages rows went out as `x-api-key` and was rejected.
    #[test]
    fn copilot_uses_bearer_and_dynamic_headers_not_x_api_key() {
        const COPILOT_TOKEN: &str =
            "tid=abc123;exp=1789000000;proxy-ep=proxy.individual.githubcopilot.com;sku=copilot";

        let mut m = model();
        m.provider = "github-copilot".into();
        m.headers = Some(std::collections::BTreeMap::from([(
            "Editor-Version".to_string(),
            Some("vscode/1.107.0".to_string()),
        )]));
        let auth = auth_with(Some(COPILOT_TOKEN));

        // The sniff alone never selects Bearer for this token — that is the whole bug.
        assert!(!is_oauth_token(COPILOT_TOKEN));
        // …but the provider branch does, and it also keeps `isOAuthToken` false for `buildParams`
        // (anthropic-messages.ts:887).
        assert!(!resolve_is_oauth(&m, &auth));

        let ctx = user_ctx("hi");
        let headers = build_headers(&m, &ctx, &auth, &StreamOptions::default(), false);

        assert_eq!(
            headers
                .get("authorization")
                .and_then(|v| v.clone())
                .as_deref(),
            Some(format!("Bearer {COPILOT_TOKEN}").as_str()),
            "Copilot takes `authToken`, not `apiKey` (anthropic-messages.ts:870-871)"
        );
        assert!(
            !headers.contains_key("x-api-key"),
            "the api-key branch must not run for Copilot"
        );
        // Selective betas: none of the Claude-Code/OAuth identity is sent.
        let beta = headers
            .get("anthropic-beta")
            .and_then(|v| v.clone())
            .unwrap_or_default();
        assert!(!beta.contains("claude-code-20250219"), "got: {beta}");
        assert!(!beta.contains("oauth-2025-04-20"), "got: {beta}");
        assert!(!headers.contains_key("x-app"));

        // PROV-028: the dynamic headers, on top of the static `model.headers` identity.
        assert_eq!(
            headers.get("X-Initiator").and_then(|v| v.clone()).as_deref(),
            Some("user"),
            "the last turn is a user turn (github-copilot-headers.ts:5-8)"
        );
        assert_eq!(
            headers
                .get("Openai-Intent")
                .and_then(|v| v.clone())
                .as_deref(),
            Some("conversation-edits")
        );
        assert!(
            !headers.contains_key("Copilot-Vision-Request"),
            "no image in this turn"
        );
        assert_eq!(
            headers
                .get("Editor-Version")
                .and_then(|v| v.clone())
                .as_deref(),
            Some("vscode/1.107.0"),
            "the static model.headers identity still merges"
        );

        // An agent-loop follow-up (last turn is a toolResult carrying an image) flips both.
        let mut agent_ctx = ctx.clone();
        agent_ctx.messages.push(Message::ToolResult {
            tool_call_id: cyrup_core::ToolCallId::from("call_1"),
            tool_name: "screenshot".to_string(),
            content: vec![Content::Image {
                data: "aGk=".to_string(),
                mime_type: "image/png".to_string(),
            }],
            is_error: false,
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            timestamp: 0,
        });
        let headers = build_headers(&m, &agent_ctx, &auth, &StreamOptions::default(), false);
        assert_eq!(
            headers.get("X-Initiator").and_then(|v| v.clone()).as_deref(),
            Some("agent")
        );
        assert_eq!(
            headers
                .get("Copilot-Vision-Request")
                .and_then(|v| v.clone())
                .as_deref(),
            Some("true")
        );

        // Non-Copilot anthropic providers are untouched by any of this.
        let plain = build_headers(
            &model(),
            &ctx,
            &auth_with(Some("sk-ant-api03-xxx")),
            &StreamOptions::default(),
            false,
        );
        assert_eq!(
            plain.get("x-api-key").and_then(|v| v.clone()).as_deref(),
            Some("sk-ant-api03-xxx")
        );
        assert!(!plain.contains_key("X-Initiator"));
    }

    #[test]
    fn oauth_headers_use_bearer_and_identity() {
        let m = model();
        let auth = auth_with(Some("sk-ant-oat01-yyy"));
        let is_oauth = is_oauth_token(auth.auth.api_key.as_deref().unwrap());
        assert!(is_oauth);
        let headers = build_headers(
            &m,
            &Context::default(),
            &auth,
            &StreamOptions::default(),
            is_oauth,
        );
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|v| v.clone())
                .as_deref(),
            Some("Bearer sk-ant-oat01-yyy")
        );
        assert!(!headers.contains_key("x-api-key"));
        let beta = headers
            .get("anthropic-beta")
            .and_then(|v| v.clone())
            .unwrap_or_default();
        assert!(beta.contains("claude-code-20250219"));
        assert!(beta.contains("oauth-2025-04-20"));
        assert_eq!(
            headers.get("x-app").and_then(|v| v.clone()).as_deref(),
            Some("cli")
        );
    }

    #[test]
    fn oauth_remaps_tool_names_to_claude_code() {
        let mut ctx = user_ctx("x");
        ctx.tools = vec![ToolDef {
            name: "bash".to_string(),
            description: "run".to_string(),
            parameters: json!({}),
            constrained_sampling: None,
        }];
        let m = model();
        // build_params with is_oauth=true via direct call.
        let body = build_params(&m, &ctx, &StreamOptions::default(), None, true).unwrap();
        assert_eq!(body["tools"][0]["name"], "Bash");
    }

    #[test]
    fn tool_results_collapse_into_one_user_message() {
        let mut m = model();
        m.reasoning = false;
        let ctx = Context {
            system_prompt: None,
            messages: vec![
                Message::ToolResult {
                    tool_call_id: ToolCallId::from("toolu_1"),
                    tool_name: "read".to_string(),
                    content: vec![Content::text("result A")],
                    is_error: false,
                    details: None,
                    timestamp: 0,
                    usage: None,
                    added_tool_names: Vec::new(),
                },
                Message::ToolResult {
                    tool_call_id: ToolCallId::from("toolu_2"),
                    tool_name: "read".to_string(),
                    content: vec![Content::text("result B")],
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
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        let blocks = msgs[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["tool_use_id"], "toolu_1");
        assert_eq!(blocks[0]["content"], "result A");
    }

    #[test]
    fn normalize_tool_call_id_rule() {
        assert_eq!(
            normalize_tool_call_id("call/with|bad chars"),
            "call_with_bad_chars"
        );
        let long = "a".repeat(100);
        assert_eq!(normalize_tool_call_id(&long).chars().count(), 64);
    }

    #[test]
    fn tool_choice_mapping() {
        use crate::stream::ToolChoice;
        assert_eq!(tool_choice_wire(&ToolChoice::Auto), json!({"type":"auto"}));
        assert_eq!(
            tool_choice_wire(&ToolChoice::Required),
            json!({"type":"any"})
        );
        assert_eq!(
            tool_choice_wire(&ToolChoice::Function { name: "x".into() }),
            json!({"type":"tool","name":"x"})
        );
    }

    async fn collect(frames_bytes: Vec<u8>, m: &Model) -> Vec<StreamEvent> {
        let (sink, mut rx) = channel(64);
        let api = ApiId::from(API_ID);
        let frames = decode_sse_bytes(frames_bytes);
        let m2 = m.clone();
        let api2 = api.clone();
        let task = tokio::spawn(async move {
            decode_stream(frames, &m2, &api2, &sink, false, &[]).await;
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
        // A realistic Anthropic SSE transcript: message_start, a text block, a tool_use block, and
        // message_delta(tool_use) + message_stop.
        let raw = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"read\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"a\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let m = model();
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
        // text delta carried "Hello".
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::TextDelta { delta, .. } if delta == "Hello"))
        );
        // tool call end with parsed args.
        let tool_end = events.iter().find_map(|e| match e {
            StreamEvent::ToolCallEnd { tool_call, .. } => Some(tool_call.clone()),
            _ => None,
        });
        let tool = tool_end.expect("toolcall_end");
        assert_eq!(tool.id.as_str(), "toolu_9");
        assert_eq!(tool.name, "read");
        assert_eq!(
            tool.arguments.get("path").and_then(Value::as_str),
            Some("a")
        );
        // terminal done with ToolUse + usage/cost computed.
        let done = events.iter().find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        });
        let msg = done.expect("done terminal");
        assert_eq!(msg.stop_reason, StopReason::ToolUse);
        assert_eq!(msg.response_id.as_deref(), Some("msg_1"));
        assert_eq!(msg.usage.input, 10);
        assert_eq!(msg.usage.output, 7);
        assert!(msg.usage.cost.total > 0.0);
    }

    #[tokio::test]
    async fn decodes_thinking_with_signature() {
        let raw = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"reason\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"SIG\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let m = model();
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        let done = events.iter().find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        });
        let msg = done.expect("done");
        assert_eq!(msg.stop_reason, StopReason::Stop);
        let thinking = msg.content.iter().find_map(|c| match c {
            Content::Thinking {
                thinking,
                thinking_signature,
                ..
            } => Some((thinking.clone(), thinking_signature.clone())),
            _ => None,
        });
        let (thinking, sig) = thinking.expect("thinking block");
        assert_eq!(thinking, "reason");
        assert_eq!(sig.as_deref(), Some("SIG"));
    }

    /// DRIFT-003: `content_block_start` may already carry the head of a text block. Pi seeds the
    /// block with `event.content_block.text ?? ""`; dropping it silently truncates the reply.
    #[tokio::test]
    async fn content_block_start_text_payload_is_kept() {
        let raw = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"Hel\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let m = model();
        let events = collect(raw.as_bytes().to_vec(), &m).await;

        // The seeded head is visible on the very first partial snapshot, not only at the end.
        let start_partial = events.iter().find_map(|e| match e {
            StreamEvent::TextStart { partial, .. } => Some(partial.clone()),
            _ => None,
        });
        let start_text = start_partial
            .expect("text_start")
            .content
            .iter()
            .find_map(|c| match c {
                Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .expect("text block on the start partial");
        assert_eq!(start_text, "Hel");

        let done = events.iter().find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        });
        let msg = done.expect("done");
        let text = msg
            .content
            .iter()
            .find_map(|c| match c {
                Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .expect("text block");
        assert_eq!(text, "Hello", "the content_block_start head was dropped");
    }

    /// PORT BUG (present at v0.83.0, never ported): pi writes
    /// `output.rawStopReason = event.delta.stop_reason` at
    /// `v0.84.1 ai/src/api/anthropic-messages.ts:709`, and cyrup filled `raw_stop_reason: None` at
    /// every construction site. The narrowing map is lossy — `refusal`, `sensitive` and every
    /// unknown reason all become [`StopReason::Error`] — so without the raw string the turn no
    /// longer records WHICH one the provider sent.
    #[tokio::test]
    async fn message_delta_records_the_providers_own_stop_reason() {
        let head = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
        );
        let stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

        // A refusal maps to `error`; only the raw string says it was a refusal and not a transport
        // failure. `emit_error` builds its terminal from the same snapshot, so it must survive there.
        let refusal = format!(
            "{head}event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"refusal\"}},\"usage\":{{\"output_tokens\":1}}}}\n\n{stop}"
        );
        let m = model();
        let events = collect(refusal.into_bytes(), &m).await;
        let Some(StreamEvent::Error { error, .. }) = events.last() else {
            panic!("expected an error terminal, got {:?}", events.last());
        };
        assert_eq!(error.stop_reason, StopReason::Error);
        assert_eq!(error.raw_stop_reason.as_deref(), Some("refusal"));

        // MIRROR 1: a clean `end_turn` keeps its raw word too, on the `done` terminal AND on every
        // in-flight partial emitted after the `message_delta`.
        let clean = format!(
            "{head}event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":1}}}}\n\n{stop}"
        );
        let events = collect(clean.into_bytes(), &m).await;
        let Some(StreamEvent::Done { message, .. }) = events.last() else {
            panic!("expected a done terminal, got {:?}", events.last());
        };
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("end_turn"));

        // MIRROR 2: no `message_delta` at all → nothing to record. pi never assigns, so the field
        // stays absent rather than being invented from the truncation diagnostic.
        let truncated = format!("{head}{stop}");
        let events = collect(truncated.into_bytes(), &m).await;
        let last = events.last().expect("a terminal");
        assert_eq!(
            last.terminal_message()
                .and_then(|t| t.raw_stop_reason.clone()),
            None
        );
    }

    /// pi's guard is `if (event.delta.stop_reason)` (`v0.84.1
    /// ai/src/api/anthropic-messages.ts:708`) — JS truthiness, so `""` is not a stop reason. cyrup
    /// tested only for presence, so an empty string reached `map_stop_reason` and settled the turn
    /// on `Unhandled stop reason: ` instead of leaving the `"pending"` seed to be reported as the
    /// truncation it is.
    #[tokio::test]
    async fn an_empty_stop_reason_is_not_a_stop_reason() {
        let raw = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let m = model();
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        let Some(StreamEvent::Error { error, .. }) = events.last() else {
            panic!("expected an error terminal, got {:?}", events.last());
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("Anthropic stream ended without a stop reason")
        );
        assert_eq!(error.raw_stop_reason, None);
    }

    /// DRIFT-003: the same for thinking blocks. The signature matters most — a thinking block
    /// replayed to Anthropic without its signature is rejected, so a signature delivered only on
    /// the open event must survive.
    #[tokio::test]
    async fn content_block_start_thinking_and_signature_payload_is_kept() {
        let raw = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"rea\",\"signature\":\"SIG-FROM-START\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"son\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let m = model();
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        let done = events.iter().find_map(|e| match e {
            StreamEvent::Done { message, .. } => Some(message.clone()),
            _ => None,
        });
        let msg = done.expect("done");
        let (thinking, sig) = msg
            .content
            .iter()
            .find_map(|c| match c {
                Content::Thinking {
                    thinking,
                    thinking_signature,
                    ..
                } => Some((thinking.clone(), thinking_signature.clone())),
                _ => None,
            })
            .expect("thinking block");
        assert_eq!(thinking, "reason", "the thinking head was dropped");
        assert_eq!(
            sig.as_deref(),
            Some("SIG-FROM-START"),
            "the signature from content_block_start was dropped — the block is unreplayable"
        );
    }

    #[tokio::test]
    async fn missing_message_stop_is_error() {
        let raw = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        );
        let m = model();
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        let err = events.iter().find_map(|e| match e {
            StreamEvent::Error { error, .. } => Some(error.clone()),
            _ => None,
        });
        let msg = err.expect("error terminal");
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("message_stop"));
    }

    #[tokio::test]
    async fn sse_error_event_is_error_terminal() {
        let raw = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}\n\n",
        );
        let m = model();
        let events = collect(raw.as_bytes().to_vec(), &m).await;
        let err = events.iter().find_map(|e| match e {
            StreamEvent::Error { error, .. } => Some(error.clone()),
            _ => None,
        });
        assert!(err.is_some());
    }

    #[test]
    fn redacted_thinking_replays_as_redacted_block() {
        let mut m = model();
        m.reasoning = false;
        let am = AssistantMessage {
            content: vec![Content::Thinking {
                thinking: "[Reasoning redacted]".to_string(),
                thinking_signature: Some("OPAQUE".to_string()),
                redacted: true,
            }],
            provider: ProviderId::from("anthropic"),
            model: "claude-opus-4-5".into(),
            api: API_ID.into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        let value = build_assistant(&am, false, false).expect("assistant");
        assert_eq!(value["content"][0]["type"], "redacted_thinking");
        assert_eq!(value["content"][0]["data"], "OPAQUE");
    }

    #[test]
    fn empty_signature_thinking_becomes_text_unless_allowed() {
        let am = AssistantMessage {
            content: vec![Content::Thinking {
                thinking: "raw reasoning".to_string(),
                thinking_signature: None,
                redacted: false,
            }],
            provider: ProviderId::from("anthropic"),
            model: "claude-opus-4-5".into(),
            api: API_ID.into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        // default: convert to text.
        let v = build_assistant(&am, false, false).expect("assistant");
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "raw reasoning");
        // allowEmptySignature: keep as thinking with empty signature.
        let v = build_assistant(&am, false, true).expect("assistant");
        assert_eq!(v["content"][0]["type"], "thinking");
        assert_eq!(v["content"][0]["signature"], "");
    }

    // -----------------------------------------------------------------------
    // DRIFT-001: message-anchored tool loading (Pi `deferred-tools.test.ts`)
    //
    // These assert the EMITTED WIRE JSON, not helper return values. The rule that makes them
    // load-bearing: Anthropic REJECTS a `tool_result` whose content mixes `tool_reference` with
    // ordinary blocks, so the real content must be DISPLACED into siblings — relocated, never
    // dropped.
    // -----------------------------------------------------------------------

    fn tool_def(name: &str) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: format!("The {name} tool"),
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            constrained_sampling: None,
        }
    }

    fn tc_assistant(calls: &[(&str, &str)]) -> Message {
        Message::Assistant(AssistantMessage {
            content: calls
                .iter()
                .map(|(id, name)| {
                    Content::ToolCall(ToolCall {
                        id: ToolCallId::from(*id),
                        name: (*name).to_string(),
                        arguments: Map::new(),
                        thought_signature: None,
                    })
                })
                .collect(),
            provider: ProviderId::from("anthropic"),
            model: "claude-opus-4-6".into(),
            api: API_ID.into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 2,
        })
    }

    fn tr(id: &str, content: Vec<Content>, added: &[&str]) -> Message {
        Message::ToolResult {
            tool_call_id: ToolCallId::from(id),
            tool_name: "base_tool".to_string(),
            content,
            is_error: false,
            details: None,
            usage: None,
            added_tool_names: added.iter().map(|s| (*s).to_string()).collect(),
            timestamp: 3,
        }
    }

    /// Pi `makeContext`: user → assistant(toolCall base_tool) → toolResult(added) → user.
    fn deferred_ctx(tools: Vec<ToolDef>, added: &[&str]) -> Context {
        Context {
            system_prompt: None,
            messages: vec![
                Message::User {
                    content: vec![Content::text("Hello")],
                    timestamp: 1,
                },
                tc_assistant(&[("call_1", "base_tool")]),
                tr("call_1", vec![Content::text("done")], added),
                Message::User {
                    content: vec![Content::text("Hello")],
                    timestamp: 4,
                },
            ],
            tools,
        }
    }

    fn opus_4_6() -> Model {
        Model {
            id: "claude-opus-4-6".into(),
            ..model()
        }
    }

    /// The `content` array of the first `user` message that carries a `tool_result` block.
    fn tool_result_content(body: &Value) -> Vec<Value> {
        let msgs = body["messages"].as_array().expect("messages");
        for m in msgs {
            if let Some(arr) = m["content"].as_array()
                && arr.iter().any(|b| b["type"] == "tool_result")
            {
                return arr.clone();
            }
        }
        panic!("no tool_result in payload: {body:#}");
    }

    fn tool_names(body: &Value) -> Vec<String> {
        body["tools"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|t| t["name"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn deferred_tool_is_marked_defer_loading_and_anchored_by_a_tool_reference() {
        // Pi "loads an Anthropic tool at its tool-result marker".
        let ctx = deferred_ctx(
            vec![tool_def("base_tool"), tool_def("late_tool")],
            &["late_tool"],
        );
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        };
        let body = build_body(&opus_4_6(), &ctx, &opts);

        // EXACT tools array. `defer_loading` sits AFTER `input_schema` (Pi key order) and the
        // immediate tool carries no marker at all.
        assert_eq!(
            body["tools"],
            json!([
                {
                    "name": "base_tool",
                    "description": "The base_tool tool",
                    "eager_input_streaming": true,
                    "input_schema": { "type": "object", "properties": {}, "required": [] }
                },
                {
                    "name": "late_tool",
                    "description": "The late_tool tool",
                    "eager_input_streaming": true,
                    "input_schema": { "type": "object", "properties": {}, "required": [] },
                    "defer_loading": true
                }
            ])
        );

        // EXACT tool-result user message: the reference REPLACES the content, and the displaced
        // text follows the tool_result as a sibling.
        assert_eq!(
            tool_result_content(&body),
            vec![
                json!({
                    "type": "tool_result",
                    "tool_use_id": "call_1",
                    "content": [{ "type": "tool_reference", "tool_name": "late_tool" }],
                    "is_error": false
                }),
                json!({ "type": "text", "text": "done" }),
            ]
        );
        // Constraint 5: the original content still EXISTS in the payload.
        assert!(
            serde_json::to_string(&body)
                .expect("json")
                .contains("\"done\""),
            "displaced tool output must be relocated, never dropped"
        );
    }

    #[test]
    fn a_tool_reference_is_never_mixed_with_ordinary_content_in_one_tool_result() {
        // Constraint 4, stated directly as an invariant over the whole payload: any tool_result
        // whose content is an array is EITHER all tool_reference OR all ordinary blocks.
        let mut ctx = deferred_ctx(
            vec![tool_def("base_tool"), tool_def("late_tool")],
            &["late_tool"],
        );
        if let Some(Message::ToolResult { content, .. }) = ctx.messages.get_mut(2) {
            *content = vec![
                Content::text("work completed"),
                Content::Image {
                    data: "aW1hZ2U=".to_string(),
                    mime_type: "image/png".to_string(),
                },
            ];
        }
        let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());

        let mut saw_reference = false;
        for m in body["messages"].as_array().expect("messages") {
            let Some(blocks) = m["content"].as_array() else {
                continue;
            };
            for b in blocks {
                if b["type"] != "tool_result" {
                    continue;
                }
                let Some(inner) = b["content"].as_array() else {
                    continue;
                };
                let refs = inner
                    .iter()
                    .filter(|x| x["type"] == "tool_reference")
                    .count();
                if refs > 0 {
                    saw_reference = true;
                    assert_eq!(
                        refs,
                        inner.len(),
                        "tool_result mixes tool_reference with ordinary blocks — Anthropic 400s: {b:#}"
                    );
                }
            }
        }
        assert!(saw_reference, "expected at least one tool_reference");
    }

    #[test]
    fn displaced_content_is_flushed_after_every_tool_result_of_the_batch() {
        // Pi "preserves tool output as sibling content after emitting references". The
        // displacement of the FIRST result lands AFTER the SECOND result's block — siblings are
        // accumulated across the whole consecutive run and flushed once. Per-block interleaving
        // fails this.
        let mut ctx = deferred_ctx(
            vec![tool_def("base_tool"), tool_def("late_tool")],
            &["late_tool"],
        );
        ctx.messages[1] = tc_assistant(&[("call_1", "base_tool"), ("call_2", "base_tool")]);
        if let Some(Message::ToolResult { content, .. }) = ctx.messages.get_mut(2) {
            *content = vec![
                Content::text("work completed"),
                Content::Image {
                    data: "aW1hZ2U=".to_string(),
                    mime_type: "image/png".to_string(),
                },
            ];
        }
        ctx.messages
            .insert(3, tr("call_2", vec![Content::text("second result")], &[]));

        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        };
        let body = build_body(&opus_4_6(), &ctx, &opts);

        assert_eq!(
            tool_result_content(&body),
            vec![
                json!({
                    "type": "tool_result",
                    "tool_use_id": "call_1",
                    "content": [{ "type": "tool_reference", "tool_name": "late_tool" }],
                    "is_error": false
                }),
                json!({
                    "type": "tool_result",
                    "tool_use_id": "call_2",
                    "content": "second result",
                    "is_error": false
                }),
                json!({ "type": "text", "text": "work completed" }),
                json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": "image/png", "data": "aW1hZ2U=" }
                }),
            ]
        );
    }

    #[test]
    fn a_deferred_name_is_referenced_at_most_once_per_request() {
        // `loadedToolNames` is declared once per convertMessages call (Pi :1125), so a second
        // marker for the same tool emits no reference and displaces nothing.
        let mut ctx = deferred_ctx(
            vec![tool_def("base_tool"), tool_def("late_tool")],
            &["late_tool"],
        );
        ctx.messages
            .insert(3, tc_assistant(&[("call_2", "base_tool")]));
        ctx.messages.insert(
            4,
            tr("call_2", vec![Content::text("again")], &["late_tool"]),
        );

        let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());
        let refs = serde_json::to_string(&body)
            .expect("json")
            .matches("\"tool_reference\"")
            .count();
        assert_eq!(refs, 1, "a deferred name must be referenced exactly once");
        // The second result keeps its own content inline (no references → no displacement).
        let second = body["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .find_map(|m| {
                m["content"]
                    .as_array()?
                    .iter()
                    .find(|b| b["tool_use_id"] == "call_2")
                    .cloned()
            })
            .expect("second tool_result");
        assert_eq!(second["content"], json!("again"));
    }

    #[test]
    fn a_tool_used_before_its_marker_stays_immediate() {
        // Pi "keeps a tool immediate when it was used before its marker".
        let mut ctx = deferred_ctx(
            vec![tool_def("base_tool"), tool_def("late_tool")],
            &["late_tool"],
        );
        ctx.messages[1] = tc_assistant(&[("call_1", "late_tool")]);
        let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());

        assert_eq!(tool_names(&body), ["base_tool", "late_tool"]);
        assert!(
            body["tools"]
                .as_array()
                .expect("tools")
                .iter()
                .all(|t| t.get("defer_loading").is_none())
        );
        assert!(
            !serde_json::to_string(&body)
                .expect("json")
                .contains("tool_reference")
        );
    }

    #[test]
    fn a_marked_tool_absent_from_the_active_set_is_not_resurrected() {
        // Pi "does not resurrect a marked tool missing from Context.tools".
        let ctx = deferred_ctx(vec![tool_def("base_tool")], &["late_tool"]);
        let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());
        assert_eq!(tool_names(&body), ["base_tool"]);
        assert!(
            !serde_json::to_string(&body)
                .expect("json")
                .contains("tool_reference")
        );
    }

    #[test]
    fn the_safety_valve_promotes_every_tool_back_when_all_are_deferred() {
        // Pi "keeps one immediate Anthropic tool when every current tool is marked"
        // (anthropic-messages.ts:955-959). Anthropic rejects a request whose every tool is
        // deferred, so the valve fires HERE — and only here; openai-responses has none.
        let ctx = deferred_ctx(vec![tool_def("late_tool")], &["late_tool"]);
        let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());

        assert_eq!(tool_names(&body), ["late_tool"]);
        assert!(body["tools"][0].get("defer_loading").is_none());
        assert!(
            !serde_json::to_string(&body)
                .expect("json")
                .contains("tool_reference")
        );
    }

    #[test]
    fn cache_control_marks_the_last_immediate_tool_never_a_deferred_one() {
        // Pi passes `undefined` cacheControl to the deferred convertTools call (:1015-1021), so
        // the cache breakpoint stays inside the stable prefix.
        let ctx = deferred_ctx(
            vec![tool_def("base_tool"), tool_def("late_tool")],
            &["late_tool"],
        );
        let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());
        assert_eq!(
            body["tools"][0]["cache_control"],
            json!({ "type": "ephemeral" })
        );
        assert!(body["tools"][1].get("cache_control").is_none());
        assert_eq!(body["tools"][1]["defer_loading"], json!(true));
    }

    #[test]
    fn cache_control_lands_on_the_displaced_sibling_not_the_reference_block() {
        // The last block of a reference-bearing user message is now a displaced `text`, and
        // `applyLastUserCacheControl` marks it there (Pi :1259-1268). Only true when the
        // tool-result batch is the LAST message.
        let mut ctx = deferred_ctx(
            vec![tool_def("base_tool"), tool_def("late_tool")],
            &["late_tool"],
        );
        ctx.messages.pop(); // drop the trailing user turn
        let body = build_body(&opus_4_6(), &ctx, &StreamOptions::default());
        let content = tool_result_content(&body);
        assert_eq!(
            content.last().expect("last block"),
            &json!({ "type": "text", "text": "done", "cache_control": { "type": "ephemeral" } })
        );
        assert!(content[0].get("cache_control").is_none());
    }

    #[test]
    fn oauth_canonicalized_markers_match_active_tools() {
        // Pi "matches OAuth-canonicalized markers to active tools": marker "Read", tool "read".
        let ctx = deferred_ctx(vec![tool_def("base_tool"), tool_def("read")], &["Read"]);
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        };
        let body = build_params(&opus_4_6(), &ctx, &opts, None, true).unwrap();

        assert_eq!(tool_names(&body), ["base_tool", "Read"]);
        assert_eq!(body["tools"][1]["defer_loading"], json!(true));
        assert_eq!(
            tool_result_content(&body)[0]["content"],
            json!([{ "type": "tool_reference", "tool_name": "Read" }])
        );
    }

    #[test]
    fn oauth_names_are_normalized_before_the_prior_usage_check() {
        // Pi "normalizes OAuth names before checking prior tool usage": the call is `Read`, the
        // marker is `read` — same tool after canonicalization, so nothing defers.
        let mut ctx = deferred_ctx(vec![tool_def("base_tool"), tool_def("read")], &["read"]);
        ctx.messages[1] = tc_assistant(&[("call_1", "Read")]);
        let body = build_params(&opus_4_6(), &ctx, &StreamOptions::default(), None, true).unwrap();

        assert_eq!(tool_names(&body), ["base_tool", "Read"]);
        assert!(
            body["tools"]
                .as_array()
                .expect("tools")
                .iter()
                .all(|t| t.get("defer_loading").is_none())
        );
        assert!(
            !serde_json::to_string(&body)
                .expect("json")
                .contains("tool_reference")
        );
    }

    #[test]
    fn oauth_dedupe_collapses_case_variants_even_with_the_flag_off() {
        // Pi "deduplicates active tools after OAuth canonicalization". The unique-map collapse in
        // `splitDeferredTools` runs BEFORE the `!enabled` early return, so this lands on a model
        // with tool references OFF too. It is the ONE behavior change that is not gated by the
        // flag — and it is reachable only under OAuth, where the normalizer is not the identity.
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text("Hello")],
                timestamp: 1,
            }],
            tools: vec![
                tool_def("read"),
                ToolDef {
                    name: "Read".to_string(),
                    description: "Canonical definition".to_string(),
                    parameters: json!({ "type": "object", "properties": {}, "required": [] }),
                    constrained_sampling: None,
                },
            ],
        };
        // Tool references ON (opus 4.6).
        let body = build_params(&opus_4_6(), &ctx, &StreamOptions::default(), None, true).unwrap();
        assert_eq!(tool_names(&body), ["Read"]);
        assert_eq!(body["tools"][0]["description"], "Canonical definition");

        // ...and OFF (haiku): still deduped.
        let haiku = Model {
            id: "claude-haiku-4-5".into(),
            ..model()
        };
        let body = build_params(&haiku, &ctx, &StreamOptions::default(), None, true).unwrap();
        assert_eq!(tool_names(&body), ["Read"]);

        // Non-OAuth normalizer is the identity, so both survive — no silent collapse.
        let body = build_params(&opus_4_6(), &ctx, &StreamOptions::default(), None, false).unwrap();
        assert_eq!(tool_names(&body), ["read", "Read"]);
    }

    #[test]
    fn unsupported_models_emit_the_plain_tool_list() {
        // Pi "uses the normal tool list when Anthropic tool references are unsupported". The
        // second id is the date-suffix trap: `claude-sonnet-4-20250514` captures "20250514"
        // (8 chars) as the minor group, which the `< 8` guard folds to 0.
        let ctx = deferred_ctx(
            vec![tool_def("base_tool"), tool_def("late_tool")],
            &["late_tool"],
        );
        for id in ["claude-haiku-4-5", "claude-sonnet-4-20250514"] {
            let m = Model {
                id: id.into(),
                ..model()
            };
            let body = build_body(&m, &ctx, &StreamOptions::default());
            assert_eq!(tool_names(&body), ["base_tool", "late_tool"], "{id}");
            assert!(
                body["tools"]
                    .as_array()
                    .expect("tools")
                    .iter()
                    .all(|t| t.get("defer_loading").is_none()),
                "{id}"
            );
            assert!(
                !serde_json::to_string(&body)
                    .expect("json")
                    .contains("tool_reference"),
                "{id}"
            );
            // ...and the tool result keeps its content inline, exactly as before DRIFT-001.
            assert_eq!(tool_result_content(&body)[0]["content"], json!("done"));
        }
    }

    #[test]
    fn an_explicit_compat_override_enables_a_non_anthropic_provider() {
        // Pi "supports explicit Anthropic compatibility overrides": the override wins over the
        // provider gate, so a proxy fronting Claude can opt in.
        let m = Model {
            id: "claude-opus-4-6".into(),
            provider: ProviderId::from("anthropic-proxy"),
            compat: Some(ModelCompat {
                supports_tool_references: Some(true),
                ..Default::default()
            }),
            ..model()
        };
        assert!(
            !default_supports_tool_references(&m),
            "the default gate says no"
        );
        let ctx = deferred_ctx(
            vec![tool_def("base_tool"), tool_def("late_tool")],
            &["late_tool"],
        );
        let body = build_body(&m, &ctx, &StreamOptions::default());
        assert_eq!(body["tools"][1]["defer_loading"], json!(true));
        assert_eq!(
            tool_result_content(&body)[0]["content"],
            json!([{ "type": "tool_reference", "tool_name": "late_tool" }])
        );

        // ...and the override can force it OFF on a model the default enables.
        let off = Model {
            compat: Some(ModelCompat {
                supports_tool_references: Some(false),
                ..Default::default()
            }),
            ..opus_4_6()
        };
        let body = build_body(&off, &ctx, &StreamOptions::default());
        assert!(body["tools"][1].get("defer_loading").is_none());
    }

    #[test]
    fn default_supports_tool_references_parses_versions_like_pis_regex() {
        let probe = |id: &str, provider: &str| {
            default_supports_tool_references(&Model {
                id: id.into(),
                provider: ProviderId::from(provider),
                ..model()
            })
        };
        // major > 4, or major == 4 && minor >= 5.
        for id in [
            "claude-sonnet-4-5",
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-6",
            "claude-sonnet-5",
            "claude-opus-4-5",
            "claude-opus-4-5-20251101",
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-opus-4-8",
            "claude-fable-5",
        ] {
            assert!(probe(id, "anthropic"), "expected ON: {id}");
        }
        for id in [
            "claude-opus-4-1",            // minor 1 < 5
            "claude-opus-4-1-20250805",   // minor 1 < 5
            "claude-opus-4",              // minor absent → 0
            "claude-sonnet-4-20250514",   // 8-char date captured as minor → folded to 0
            "claude-haiku-4-5",           // haiku gate
            "claude-haiku-5",             // haiku gate
            "claude-3-5-sonnet-20241022", // family not at the anchored position
            "claude-mythos-5",            // unknown family
            "claude-opus-x-5",            // no major digits
            "claude-opus-45x",            // major run not followed by `-` or end
            "opus-5",                     // missing `claude-` prefix
        ] {
            assert!(!probe(id, "anthropic"), "expected OFF: {id}");
        }
        // The provider gate is exact-match: every reseller stays off on a byte-identical id.
        for p in [
            "vercel-ai-gateway",
            "cloudflare-ai-gateway",
            "fireworks",
            "opencode",
            "opencode-go",
            "kimi-coding",
            "minimax",
            "minimax-cn",
            "anthropic-proxy",
        ] {
            assert!(
                !probe("claude-opus-4-6", p),
                "expected OFF for provider {p}"
            );
        }
    }

    /// Constraint 3, proven against the REAL embedded catalogs rather than hand-built models.
    #[test]
    fn tool_references_default_off_across_every_embedded_catalog() {
        use crate::providers::all::all_providers;

        const EXPECTED_ON: [&str; 10] = [
            "claude-fable-5",
            "claude-opus-4-5",
            "claude-opus-4-5-20251101",
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-opus-4-8",
            "claude-sonnet-4-5",
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-6",
            "claude-sonnet-5",
        ];

        let mut on: Vec<String> = Vec::new();
        let mut total = 0usize;
        let mut providers_with_on: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for provider in all_providers() {
            for m in provider.models() {
                if m.api.as_str() != API_ID {
                    continue;
                }
                total += 1;
                if get_anthropic_compat(m).supports_tool_references {
                    on.push(m.id.as_str().to_string());
                    providers_with_on.insert(m.provider.as_str().to_string());
                }
            }
        }
        on.sort();
        on.dedup();

        assert!(
            total > 200,
            "expected the real catalogs, saw {total} models"
        );
        assert_eq!(
            on,
            EXPECTED_ON,
            "the wire-payload blast radius of DRIFT-001 changed; \
             {} of {total} anthropic-messages models are ON",
            on.len()
        );
        assert_eq!(
            providers_with_on.into_iter().collect::<Vec<_>>(),
            ["anthropic"],
            "only the first-party Anthropic provider may enable tool references"
        );
    }

    /// The Responses half of the same flag: catalog-driven, `?? false`, and enabled ONLY on the
    /// seven first-party OpenAI ids Pi's generator marks (`generate-models.ts:324-332`). Asserted
    /// here from the Anthropic side so that a catalog edit which leaked the flag onto an
    /// `anthropic-messages` model — where nothing reads it and it would be pure confusion — fails
    /// loudly. The exhaustive on/off partition lives with the rendering, in
    /// `openai_responses::tests::tool_search_is_off_for_every_openai_responses_model_but_the_seven`.
    #[test]
    fn tool_search_is_confined_to_the_openai_responses_catalog() {
        use crate::api::compat::get_responses_compat;
        use crate::providers::all::all_providers;

        let mut total = 0usize;
        let mut on: Vec<String> = Vec::new();
        for provider in all_providers() {
            for m in provider.models() {
                total += 1;
                if !get_responses_compat(m).supports_tool_search {
                    continue;
                }
                assert_ne!(
                    m.api.as_str(),
                    API_ID,
                    "{}/{} sets supportsToolSearch on an anthropic-messages model, where it is \
                     never read",
                    m.provider.as_str(),
                    m.id.as_str()
                );
                on.push(format!("{}/{}", m.provider.as_str(), m.id.as_str()));
            }
        }
        on.sort();
        assert_eq!(
            on,
            [
                // openai-codex, ported in the unported-work sweep. Its catalog carries the same
                // `supportsToolSearch` rows as `openai`, on the same `openai-responses` wire API —
                // the assertion is that tool-search stays confined to that API, not to one
                // provider, so a second responses-based provider legitimately widens this list.
                "openai-codex/gpt-5.4",
                "openai-codex/gpt-5.4-mini",
                "openai-codex/gpt-5.5",
                "openai-codex/gpt-5.6-luna",
                "openai-codex/gpt-5.6-sol",
                "openai-codex/gpt-5.6-terra",
                "openai/gpt-5.4",
                "openai/gpt-5.4-mini",
                "openai/gpt-5.4-pro",
                "openai/gpt-5.5",
                "openai/gpt-5.6-luna",
                "openai/gpt-5.6-sol",
                "openai/gpt-5.6-terra",
            ],
            "the tool-search blast radius changed"
        );
        assert!(
            total > 600,
            "expected the real catalogs, saw {total} models"
        );
        // ...and the flag is honored when a catalog/override does set it.
        let m = Model {
            compat: Some(ModelCompat {
                supports_tool_search: Some(true),
                ..Default::default()
            }),
            ..model()
        };
        assert!(get_responses_compat(&m).supports_tool_search);
    }

    /// Regression guard: with no `addedToolNames` anywhere, the payload must be byte-identical to
    /// the pre-DRIFT-001 shape even on a model where the flag is ON.
    #[test]
    fn an_unmarked_transcript_is_byte_identical_on_a_flag_on_model() {
        let ctx = deferred_ctx(vec![tool_def("base_tool"), tool_def("late_tool")], &[]);
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        };
        let on = build_body(&opus_4_6(), &ctx, &opts);
        let off = build_body(
            &Model {
                id: "claude-haiku-4-5".into(),
                ..model()
            },
            &ctx,
            &opts,
        );
        assert_eq!(on["tools"], off["tools"]);
        assert_eq!(on["messages"], off["messages"]);
        let s = serde_json::to_string(&on).expect("json");
        assert!(!s.contains("defer_loading"));
        assert!(!s.contains("tool_reference"));
    }
}

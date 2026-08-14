//! The `openai-codex-responses` wire protocol (arch-01 §3.4) — the ChatGPT-subscription Codex
//! backend (`POST {base}/codex/responses`).
//!
//! Port of pi v0.83.0 `packages/ai/src/api/openai-codex-responses.ts` (1636 lines). Codex speaks the
//! *same* Responses SSE wire format as [`openai-responses`](crate::api::openai_responses) — upstream
//! literally hands its event iterator to the shared `processResponsesStream`
//! (`openai-codex-responses.ts:664-669`) — so this module ports only what Codex adds on top:
//!
//! | upstream | ported here |
//! |---|---|
//! | `resolveCodexUrl` (`:637-643`) | [`resolve_codex_url`] |
//! | `extractAccountId` (`:1564-1575`) | [`extract_account_id`] |
//! | `buildBaseCodexHeaders`/`buildSSEHeaders` (`:1577-1617`) | [`build_sse_headers`] |
//! | `buildRequestBody` (`:529-596`) | [`build_request_body`] |
//! | `mapCodexEvents`/`normalizeCodexStatus` (`:721-757`) | [`map_codex_event`] + [`map_codex_frames`] |
//! | `resolveCodexServiceTier` (`:627-635`) | [`resolve_codex_service_tier`] |
//! | `isRetryableError`/`isTerminalRateLimitError` (`:130-144`) | [`is_retryable_error`] |
//! | `getRetryAfterDelayMs`/`validateRetryDelayMs` (`:146-183`) | [`get_retry_after_delay_ms`] |
//! | `parseErrorResponse` (`:1533-1558`) | [`parse_error_response`] |
//! | `stream`'s SSE attempt ladder (`:390-488`) | [`CodexResponsesApi::run`] |
//!
//! Everything below `processResponsesStream` — slot creation, reasoning/text/tool decoding, usage,
//! `mapStopReason`, service-tier pricing — is reached by delegating to
//! [`openai_responses::decode_stream`](crate::api::openai_responses), exactly as upstream shares
//! `openai-responses-shared.ts`. `getServiceTierCostMultiplier` (`:598-610`) is byte-identical to
//! `openai-responses.ts:281-293`, which that decoder already implements, so the codex-specific
//! `resolveCodexServiceTier` is applied by rewriting `response.service_tier` on the terminal event
//! before the shared decoder reads it (see [`map_codex_frames`]) rather than by duplicating the
//! pricing table.
//!
//! # Mechanism deltas (the language/dependency forces them; behaviour is unchanged)
//!
//! * **Request compression.** Upstream zstd-compresses the SSE body when `node:zlib` exposes
//!   `zstdCompressSync`, and *falls back to the uncompressed JSON when it does not*
//!   (`compressRequestBodyZstd`, `:225-238`, "Callers fall back to sending the uncompressed JSON
//!   when this returns null"). Cyrup's SSE transport carries a `serde_json::Value` body, not raw
//!   bytes, so this port always takes upstream's documented no-compression branch: the request is
//!   the same JSON and `content-encoding: zstd` is correspondingly not set.
//! * **WebSocket transport.** Upstream prefers a WebSocket for `transport != "sse"` and, when the
//!   runtime exposes no WebSocket constructor, throws
//!   `"WebSocket transport is not available in this runtime"` (`connectWebSocket`, `:1043-1045`),
//!   which is not a `CodexApiError`, so `stream` records the failure and **breaks to the SSE path**
//!   (`:358-377`). This port has no WebSocket client (the workspace has no ws dependency and adding
//!   one is outside this module), so every transport resolves to SSE — upstream's own
//!   no-WebSocket-runtime behaviour. The WS-only bookkeeping (connection cache, delta continuation
//!   via `previous_response_id`, `OpenAICodexWebSocketDebugStats`, the `provider_transport_failure`
//!   diagnostic) is therefore not reachable and not ported.
//! * **`extractAccountId` base64.** Upstream calls `atob`, i.e. the WHATWG *forgiving-base64*
//!   decode: the standard alphabet with optional padding, which rejects the URL-safe `-`/`_`.
//!   [`ATOB`] is configured to match that exactly rather than using a URL-safe engine.
//!
//! Wire JSON uses OpenAI's own field names (snake_case), NOT the cyrup camelCase convention.

use crate::HeaderMap;
use crate::api::compat::{clamp_openai_prompt_cache_key, level_map_lookup, thinking_level_key};
use crate::api::openai_responses::{
    ConvertResponsesToolsOptions, convert_responses_messages, convert_responses_tools, decode_stream,
};
use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::collection::clamp_thinking_level;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::{SseFrame, SseRequest, build_client_for_target, open_sse};
use crate::stream::{CacheRetention, StreamEvent, StreamOptions, ToolChoice};
use crate::utils::deferred_tools::split_deferred_tools;
use crate::utils::http_date::parse_http_date_ms;
use crate::utils::provider_retry::ProviderRetry;
use base64::Engine as _;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use cyrup_core::{ApiId, AssistantMessage, CancelToken, ModelThinkingLevel, StopReason};
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex};

/// The wire-protocol id this impl serves (pi `KnownApi`, `ai/src/types.ts:16-26`).
const API_ID: &str = "openai-codex-responses";

/// pi `DEFAULT_CODEX_BASE_URL` (`openai-codex-responses.ts:59`).
const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";

/// pi `JWT_CLAIM_PATH` (`:60`) — the namespaced claim carrying the ChatGPT account id.
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

/// pi `DEFAULT_MAX_RETRIES` (`:61`). Zero: a single attempt unless the caller raises it.
const DEFAULT_MAX_RETRIES: u32 = 0;

/// pi `BASE_DELAY_MS` (`:62`) — the `BASE_DELAY_MS * 2 ** attempt` ladder, no jitter. This is a
/// *different* ladder from [`crate::utils::provider_retry`] (pi's shared `provider-retry.ts`,
/// which Codex does not use), which is why [`open_sse`] is driven with [`ProviderRetry::NONE`] and
/// the loop lives here.
const BASE_DELAY_MS: u64 = 1_000;

/// pi `DEFAULT_MAX_RETRY_DELAY_MS` (`:63`).
const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// pi `CODEX_TOOL_CALL_PROVIDERS` (`:68`) — providers whose tool-call ids carry the
/// `call_id|item_id` Responses shape.
const CODEX_TOOL_CALL_PROVIDERS: &[&str] = &["openai", "openai-codex", "opencode"];

/// pi `CODEX_RESPONSE_STATUSES` (`:73-80`). A terminal `response.status` outside this set is
/// normalized to absent, which the shared `mapStopReason(undefined)` reads as `stop`.
const CODEX_RESPONSE_STATUSES: &[&str] = &[
    "completed",
    "incomplete",
    "failed",
    "cancelled",
    "queued",
    "in_progress",
];

/// WHATWG *forgiving-base64* decode, i.e. JS `atob` (`extractAccountId`, `:1568`): standard
/// alphabet, padding optional, `-`/`_` rejected.
const ATOB: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Boxed frame stream — the shape [`open_sse`] returns and [`decode_stream`] consumes.
type FrameStream = std::pin::Pin<Box<dyn Stream<Item = Result<SseFrame, ProviderError>> + Send>>;

// ---------------------------------------------------------------------------
// Typed per-API options
// ---------------------------------------------------------------------------

/// Reasoning-summary verbosity (pi `OpenAICodexResponsesOptions.reasoningSummary`, `:88`:
/// `"auto" | "concise" | "detailed" | "off" | "on" | null`). An absent value and an explicit `null`
/// both fall back to `"auto"` (`options.reasoningSummary ?? "auto"`, `:590`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexReasoningSummary {
    Auto,
    Concise,
    Detailed,
    Off,
    On,
}

impl CodexReasoningSummary {
    /// The wire string for this summary level.
    pub fn as_str(self) -> &'static str {
        match self {
            CodexReasoningSummary::Auto => "auto",
            CodexReasoningSummary::Concise => "concise",
            CodexReasoningSummary::Detailed => "detailed",
            CodexReasoningSummary::Off => "off",
            CodexReasoningSummary::On => "on",
        }
    }
}

/// The `tool_choice` values Codex accepts (pi `OpenAICodexResponsesOptions.toolChoice`, `:91`:
/// `"auto" | "none" | "required"` — note it has **no** named-function form, unlike the
/// `openai-completions` option of the same name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexToolChoice {
    Auto,
    None,
    Required,
}

impl CodexToolChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            CodexToolChoice::Auto => "auto",
            CodexToolChoice::None => "none",
            CodexToolChoice::Required => "required",
        }
    }

    /// Narrow cyrup's unified [`ToolChoice`] to the three values Codex's option type admits. The
    /// named-function form is not representable in `OpenAICodexResponsesOptions["toolChoice"]`, so
    /// it yields `None` and the caller falls back to upstream's `?? "auto"` default (`:562`).
    fn from_unified(choice: &ToolChoice) -> Option<Self> {
        match choice {
            ToolChoice::Auto => Some(CodexToolChoice::Auto),
            ToolChoice::None => Some(CodexToolChoice::None),
            ToolChoice::Required => Some(CodexToolChoice::Required),
            ToolChoice::Function { .. } => None,
        }
    }
}

/// Per-API typed options for `openai-codex-responses` (pi `OpenAICodexResponsesOptions`,
/// `openai-codex-responses.ts:86-92`).
///
/// `reasoningEffort` is not modelled here: cyrup carries the unified reasoning level on
/// [`StreamOptions::reasoning`] and [`build_request_body`] clamps it exactly as upstream's
/// `streamSimple` does (`:516-517`), matching how `openai-responses` and `azure-openai-responses`
/// already handle it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenAiCodexResponsesOptions {
    /// pi `reasoningSummary` (`:88`); `None` ⇒ `"auto"`.
    pub reasoning_summary: Option<CodexReasoningSummary>,
    /// pi `serviceTier` (`:89`); omitted from the body when `None` (`:570-572`).
    pub service_tier: Option<String>,
    /// pi `textVerbosity` (`:90`); `None` ⇒ `"low"` (`:559`).
    pub text_verbosity: Option<String>,
    /// pi `toolChoice` (`:91`); `None` ⇒ `"auto"` (`:562`).
    pub tool_choice: Option<CodexToolChoice>,
}

impl OpenAiCodexResponsesOptions {
    /// Derive the typed options reachable through cyrup's unified [`StreamOptions`].
    ///
    /// Only `toolChoice` has a unified spelling; `serviceTier`, `textVerbosity` and
    /// `reasoningSummary` are typed-options-only on every ported Responses api (the same position
    /// `azure-openai-responses` documents for its `reasoningSummary`) and keep upstream's defaults
    /// here. Note pi's own `buildBaseOptions` (`simple-options.ts`) forwards **no** `toolChoice`
    /// either, so upstream's `streamSimple` path is always `"auto"`.
    pub fn from_stream_options(opts: &StreamOptions) -> Self {
        // Typed options first, exactly as every sibling Responses api does. Without this branch
        // three of upstream's four options — `reasoningSummary`, `serviceTier`, `textVerbosity` —
        // were UNREACHABLE: they had no unified spelling, and nothing read
        // `StreamOptions::api_options`, so a caller could construct them and they would be
        // silently discarded.
        let typed = opts
            .api_options
            .as_ref()
            .and_then(crate::stream::ApiStreamOptions::openai_codex_responses);

        Self {
            reasoning_summary: typed.and_then(|t| t.reasoning_summary),
            service_tier: typed.and_then(|t| t.service_tier.clone()),
            text_verbosity: typed.and_then(|t| t.text_verbosity.clone()),
            // `toolChoice` is the one option with a unified spelling, so the unified value wins and
            // the typed one is the fallback — matching how the other Responses apis rank them.
            tool_choice: opts
                .tool_choice
                .as_ref()
                .and_then(CodexToolChoice::from_unified)
                .or_else(|| typed.and_then(|t| t.tool_choice)),
        }
    }
}

// ---------------------------------------------------------------------------
// ApiImpl
// ---------------------------------------------------------------------------

/// The `ApiImpl` for `"openai-codex-responses"`.
pub struct CodexResponsesApi {
    api: ApiId,
}

impl Default for CodexResponsesApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl CodexResponsesApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(CodexResponsesApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for CodexResponsesApi {
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
        // pi `if (!apiKey) throw new Error(\`No API key for provider: ${model.provider}\`)` (:271-274).
        let Some(api_key) = auth.auth.api_key.clone() else {
            sink.send(error_event(
                model,
                &self.api,
                format!("No API key for provider: {}", model.provider),
                false,
            ))
            .await;
            return;
        };

        // pi `extractAccountId(apiKey)` (:276) — throws before any request is made.
        let account_id = match extract_account_id(&api_key) {
            Ok(id) => id,
            Err(msg) => {
                sink.send(error_event(model, &self.api, msg, false)).await;
                return;
            }
        };

        // pi `const cacheSessionId = options?.cacheRetention === "none" ? undefined : options?.sessionId;`
        // then `clampOpenAIPromptCacheKey(cacheSessionId)` (:281-282).
        let codex_session_id = codex_session_id(opts);

        let codex_opts = OpenAiCodexResponsesOptions::from_stream_options(opts);
        // gap-08 #2: `before_provider_request` may inspect/replace the outbound body (pi
        // `options?.onPayload?.(body, model)`, :284-287).
        let body = crate::stream::apply_on_payload(
            opts,
            model,
            build_request_body(model, ctx, opts, &codex_opts, codex_session_id.as_deref()),
        )
        .await;

        let headers = build_sse_headers(
            model,
            auth,
            opts,
            &account_id,
            &api_key,
            codex_session_id.as_deref(),
        );
        let url = resolve_codex_url(resolved_base_url(model, auth));
        let req = SseRequest {
            method: reqwest::Method::POST,
            url,
            headers,
            body: Some(body),
        };

        // Honor HTTP(S)_PROXY for the live client (pi `resolveHttpProxyUrlForTarget`).
        //
        // PROV-051, second half. `None` — NOT `opts.timeout_ms` — is what makes this client pi's.
        // Codex is the one api that does not hand `timeoutMs` to an SDK client: it calls raw
        // `fetch` with `AbortSignal.timeout(httpTimeoutMs)` merged in for the HEADER phase only
        // (`openai-codex-responses.ts:401-410` @v0.83.0), and `combinedSignal.cleanup()` in the
        // `finally` at `:417` retires that deadline the instant headers arrive. The body is then
        // bounded solely by the process-global undici dispatcher
        // (`bodyTimeout`/`headersTimeout` = `DEFAULT_HTTP_IDLE_TIMEOUT_MS`, 300_000,
        // `coding-agent/src/core/http-dispatcher.ts:4,86-88`), whose cyrup analogue is
        // [`crate::stream::sse::configure_http_idle_timeout`] — reached by passing `None` here.
        // (Contrast `openai-responses.ts:146`, which really does pass
        // `timeout: options.timeoutMs` to its SDK client; that api keeps forwarding it.)
        //
        // Feeding `opts.timeout_ms` in as the client's `read_timeout` broke both halves: it capped
        // the BODY at a value pi had already stopped applying, and — because reqwest's read
        // deadline covers the header read too — it raced the header deadline below at the very same
        // duration and usually won, so a stalled endpoint terminated with
        // `transport error: … operation timed out` instead of pi's attributable message.
        let client = match build_client_for_target(
            &req.url,
            &crate::auth::types::EnvAuthContext,
            auth.env.as_ref(),
            None,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                sink.send(e.into_error_event(
                    model.provider.clone(),
                    model.id.as_str(),
                    Some(model.api.clone()),
                ))
                .await;
                return;
            }
        };

        // --- pi's SSE attempt ladder (:390-470) ---
        let max_retries = opts.max_retries.unwrap_or(DEFAULT_MAX_RETRIES);
        let mut attempt: u32 = 0;
        let frames: FrameStream = loop {
            // pi `if (options?.signal?.aborted) throw new Error("Request was aborted")` (:396-398).
            if cancel.is_cancelled() {
                sink.send(aborted_event(model, &self.api)).await;
                return;
            }

            let head: Arc<Mutex<Option<(u16, reqwest::header::HeaderMap)>>> =
                Arc::new(Mutex::new(None));
            let capture = crate::stream::ResponseCapture::default();
            let user_hook = capture.sse_hook(opts);
            let cell = head.clone();
            let on_resp: crate::stream::sse::OnResponse =
                Arc::new(move |status: u16, headers: &reqwest::header::HeaderMap| {
                    *cell.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some((status, headers.clone()));
                    if let Some(hook) = &user_hook {
                        hook(status, headers);
                    }
                });

            // PROV-051 — pi's HEADER-phase deadline (`openai-codex-responses.ts:401-419`
            // @v0.83.0): `AbortSignal.timeout(httpTimeoutMs)` merged with the caller's signal via
            // `combineAbortSignals`, with `combinedSignal.cleanup()` in a `finally` so the deadline
            // stops applying the moment headers arrive. cyrup fed `opts.timeout_ms` to the client
            // as a whole-stream `read_timeout` instead, which both bounded the body by a number pi
            // had stopped applying and lost the dedicated message.
            //
            // CYRUP-DELTA: `combineAbortSignals` itself (`utils/abort-signals.ts:6-41`) is NOT
            // ported — its only consumer at either tag is this call site, and `CancelToken` +
            // `tokio::time::timeout` covers it. The client's `read_timeout` stays for the body
            // phase, the analogue of pi's global undici `bodyTimeout`/`headersTimeout`
            // (`core/http-dispatcher.ts:87-88`).
            let header_timeout_ms = opts.timeout_ms.filter(|n| *n > 0);
            // Set only when the header phase actually elapsed, so the terminal carries pi's exact
            // wording rather than a `Display`-decorated transport error.
            let mut header_timeout_message: Option<String> = None;
            let open = open_sse(
                &client,
                req.clone(),
                cancel.clone(),
                None,
                Some(on_resp),
                // The ladder below IS pi's retry policy for this api; the shared provider-retry
                // ladder must not also fire.
                ProviderRetry::NONE,
            );
            let attempted = match header_timeout_ms {
                Some(ms) => {
                    match tokio::time::timeout(std::time::Duration::from_millis(ms), open).await {
                        Ok(result) => result,
                        // `if (headerTimeoutSignal?.aborted && !options?.signal?.aborted) throw new
                        // Error(...)` (:412-413) — the caller's own cancellation is reported as an
                        // abort, not as a timeout.
                        Err(_) if cancel.is_cancelled() => Err(ProviderError::Aborted),
                        Err(_) => {
                            let message =
                                format!("Codex SSE response headers timed out after {ms}ms");
                            header_timeout_message = Some(message.clone());
                            Err(ProviderError::Transport(message.into()))
                        }
                    }
                }
                None => open.await,
            };

            // pi fires `options?.onResponse?.(...)` on EVERY attempt that produced a response head
            // (:419-422), not only the last one.
            let observed = head.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if observed.is_some() {
                capture.fire(opts, model).await;
            }

            let failure = match attempted {
                Ok(stream) => break stream,
                // pi maps an `AbortError` to `throw new Error("Request was aborted")` (:448-451).
                Err(ProviderError::Aborted) => {
                    sink.send(aborted_event(model, &self.api)).await;
                    return;
                }
                Err(e) => e,
            };

            // A non-2xx head: pi's in-`try` retry branch (:429-438).
            if let ProviderError::Http { status, message } = &failure
                && attempt < max_retries
                && is_retryable_error(*status, message)
            {
                let requested = observed
                    .as_ref()
                    .and_then(|(_, h)| get_retry_after_delay_ms(h));
                let delay_ms = match requested {
                    None => backoff_delay_ms(attempt),
                    Some(ms) => match validate_retry_delay_ms(ms, opts.max_retry_delay_ms) {
                        Ok(ms) => ms,
                        // `RetryDelayExceededError` reaches the catch, which never retries it
                        // (:457) — the request fails with this message.
                        Err(text) => {
                            sink.send(error_event(model, &self.api, text, false)).await;
                            return;
                        }
                    },
                };
                if !sleep_or_abort(&cancel, delay_ms).await {
                    sink.send(aborted_event(model, &self.api)).await;
                    return;
                }
                attempt = attempt.saturating_add(1);
                continue;
            }

            // Everything else reaches pi's `catch` (:447-465): the error text is the friendly
            // usage-limit message when the body parses as one, else the raw provider text.
            let text = match (&failure, &header_timeout_message) {
                // pi throws a bare `Error` with this exact message (:412-413) and its catch runs
                // it through `formatProviderError(normalizeProviderError(error))`, which returns
                // `error.message` unchanged — so the terminal text is byte-identical to pi's.
                (_, Some(message)) => message.clone(),
                (ProviderError::Http { status, message }, _) => {
                    parse_error_response(*status, message, now_millis())
                }
                (other, _) => other.to_string(),
            };
            // pi retries network AND already-formatted errors alike, refusing only a
            // retry-delay-exceeded failure and anything mentioning a usage limit (:455-462).
            if attempt < max_retries && !text.contains("usage limit") {
                if !sleep_or_abort(&cancel, backoff_delay_ms(attempt)).await {
                    sink.send(aborted_event(model, &self.api)).await;
                    return;
                }
                attempt = attempt.saturating_add(1);
                continue;
            }
            sink.send(error_event(model, &self.api, text, false)).await;
            return;
        };

        // pi hands the codex-mapped event iterator to the SHARED Responses decoder
        // (`processResponsesStream`, :664-669).
        let mapped = map_codex_frames(frames, codex_opts.service_tier.clone());
        decode_stream(mapped, model, &self.api, &sink).await;
    }
}

// ---------------------------------------------------------------------------
// URL / session
// ---------------------------------------------------------------------------

/// The base URL a request targets. Upstream reads `model.baseUrl` (`:315`, `:405`); cyrup splits
/// pi's single `model.baseUrl` into the catalog value plus a per-credential override, so the
/// override wins here exactly as it does in [`openai_responses`](crate::api::openai_responses).
fn resolved_base_url<'a>(model: &'a Model, auth: &'a AuthResult) -> &'a str {
    auth.auth
        .base_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(model.base_url.as_str())
}

/// 1:1 port of pi `resolveCodexUrl` (`openai-codex-responses.ts:637-643`): a blank base falls back
/// to [`DEFAULT_CODEX_BASE_URL`], trailing slashes are trimmed, and the path is completed to
/// `/codex/responses` without ever doubling a segment the caller already supplied.
pub fn resolve_codex_url(base_url: &str) -> String {
    let raw = if base_url.trim().is_empty() {
        DEFAULT_CODEX_BASE_URL
    } else {
        base_url
    };
    let normalized = raw.trim_end_matches('/');
    if normalized.ends_with("/codex/responses") {
        return normalized.to_string();
    }
    if normalized.ends_with("/codex") {
        return format!("{normalized}/responses");
    }
    format!("{normalized}/codex/responses")
}

/// pi `:281-282`: the cache-scoped session id, clamped to the OpenAI prompt-cache key length.
/// `cacheRetention === "none"` drops it entirely.
fn codex_session_id(opts: &StreamOptions) -> Option<String> {
    if opts.cache_retention == Some(CacheRetention::None) {
        return None;
    }
    opts.session_id
        .as_ref()
        .map(|s| clamp_openai_prompt_cache_key(s.as_str()))
}

// ---------------------------------------------------------------------------
// Auth & headers
// ---------------------------------------------------------------------------

/// 1:1 port of pi `extractAccountId` (`openai-codex-responses.ts:1564-1575`): decode the JWT
/// payload and read `payload["https://api.openai.com/auth"].chatgpt_account_id`. Every failure —
/// wrong segment count, undecodable payload, absent or empty claim — collapses to upstream's single
/// error string, which is the whole point of its `try`/`catch`.
pub fn extract_account_id(token: &str) -> Result<String, String> {
    const FAILED: &str = "Failed to extract accountId from token";
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(FAILED.to_string());
    }
    let payload_b64 = parts.get(1).copied().unwrap_or_default();
    let decoded = ATOB.decode(payload_b64).map_err(|_| FAILED.to_string())?;
    let payload: Value = serde_json::from_slice(&decoded).map_err(|_| FAILED.to_string())?;
    let account_id = payload
        .get(JWT_CLAIM_PATH)
        .and_then(|claim| claim.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        // `if (!accountId) throw` — the empty string is falsy in JS.
        .filter(|s| !s.is_empty())
        .ok_or_else(|| FAILED.to_string())?;
    Ok(account_id.to_string())
}

/// `Headers.set` semantics on cyrup's [`HeaderMap`]: HTTP header names are case-insensitive, so a
/// `set` replaces any differently-cased entry rather than adding a second one.
fn header_set(headers: &mut HeaderMap, name: &str, value: Option<String>) {
    let lower = name.to_ascii_lowercase();
    headers.retain(|k, _| k.to_ascii_lowercase() != lower);
    headers.insert(name.to_string(), value);
}

/// 1:1 port of pi `buildBaseCodexHeaders` + `buildSSEHeaders`
/// (`openai-codex-responses.ts:1577-1617`).
///
/// Order is load-bearing: the caller's overlays are applied FIRST and the Codex identity headers
/// last, so `Authorization` / `chatgpt-account-id` / `originator` / `User-Agent` cannot be
/// overridden by `model.headers` or `options.headers` (unlike `openai-responses`, where the overlays
/// come last and do win). A `None` overlay value is pi's `headers.delete(key)`.
///
/// `originator: "pi"` and the `pi (...)` User-Agent are sent verbatim, NOT rebranded: the ChatGPT
/// backend gates on this client identity, which makes it protocol, not branding — the same reason
/// `anthropic-messages` sends `claude-cli/<version>` + `x-app: cli` unchanged. Node's
/// `os.release()` (kernel version) has no `std` equivalent and no C dependency is being added for a
/// User-Agent, so the platform triple is `(<os>; <arch>)`; upstream's own browser branch shortens it
/// further, to `pi (browser)`.
fn build_sse_headers(
    model: &Model,
    auth: &AuthResult,
    opts: &StreamOptions,
    account_id: &str,
    token: &str,
    session_id: Option<&str>,
) -> HeaderMap {
    // `new Headers(initHeaders)` where initHeaders is `model.headers` (:1583). cyrup splits pi's
    // single `model.headers` into the catalog map plus the per-credential overlay.
    let mut headers = HeaderMap::new();
    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            header_set(&mut headers, name, value.clone());
        }
    }
    if let Some(overlay) = &auth.auth.headers {
        for (name, value) in overlay {
            header_set(&mut headers, name, value.clone());
        }
    }
    // `for (const [key, value] of Object.entries(additionalHeaders || {}))` (:1584-1590).
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            header_set(&mut headers, name, value.clone());
        }
    }

    header_set(
        &mut headers,
        "Authorization",
        Some(format!("Bearer {token}")),
    );
    header_set(
        &mut headers,
        "chatgpt-account-id",
        Some(account_id.to_string()),
    );
    header_set(&mut headers, "originator", Some("pi".to_string()));
    header_set(&mut headers, "User-Agent", Some(codex_user_agent()));

    header_set(
        &mut headers,
        "OpenAI-Beta",
        Some("responses=experimental".to_string()),
    );
    header_set(
        &mut headers,
        "accept",
        Some("text/event-stream".to_string()),
    );
    header_set(
        &mut headers,
        "content-type",
        Some("application/json".to_string()),
    );

    if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
        header_set(&mut headers, "session-id", Some(sid.to_string()));
        header_set(&mut headers, "x-client-request-id", Some(sid.to_string()));
    }

    headers
}

/// pi `` `pi (${_os.platform()} ${_os.release()}; ${_os.arch()})` `` (`:1594`).
fn codex_user_agent() -> String {
    format!("pi ({}; {})", std::env::consts::OS, std::env::consts::ARCH)
}

// ---------------------------------------------------------------------------
// Request encoding
// ---------------------------------------------------------------------------

/// 1:1 port of pi `buildRequestBody` (`openai-codex-responses.ts:529-596`).
///
/// Differences from `openai-responses`' `buildParams` that are easy to "fix" wrongly:
/// * the system prompt rides in `instructions`, NOT as a leading input item
///   (`includeSystemPrompt: false`, `:545`), defaulting to `"You are a helpful assistant."`;
/// * there is **no** `max_output_tokens` — Codex never sends one;
/// * `include: ["reasoning.encrypted_content"]` is unconditional, not reasoning-gated;
/// * `tool_choice` and `parallel_tool_calls` are always present;
/// * `reasoning` is emitted purely from the requested effort, with no `model.reasoning` gate and no
///   `off`-branch `{effort}`-only body.
pub fn build_request_body(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    codex: &OpenAiCodexResponsesOptions,
    cache_session_id: Option<&str>,
) -> Value {
    let supports_tool_search = model
        .compat
        .as_ref()
        .and_then(|c| c.supports_tool_search)
        .unwrap_or(false);
    let placement = split_deferred_tools(
        &ctx.messages,
        &ctx.tools,
        supports_tool_search,
        &|name: &str| name.to_string(),
    );

    // `includeSystemPrompt: false` (:545). cyrup's shared converter always prepends
    // `ctx.system_prompt` when present, so the prompt is withheld from the context it sees and
    // placed in `instructions` below — the same bytes, in the field Codex expects.
    let body_ctx = Context {
        system_prompt: None,
        messages: ctx.messages.clone(),
        tools: ctx.tools.clone(),
    };
    // Codex's own tool options (openai-codex-responses.ts:539-540, `:575-579` @v0.83.0):
    // `supportsStrictMode ?? true` (NOT openai-responses' `?? false`), and `strict: null` as the
    // default — a JSON `null` on the wire, not an absent key.
    let tool_options = ConvertResponsesToolsOptions {
        defer_loading: false,
        supports_strict_mode: model
            .compat
            .as_ref()
            .and_then(|c| c.supports_strict_mode)
            .unwrap_or(true),
        default_strict: None,
    };
    let messages = convert_responses_messages(
        model,
        &body_ctx,
        CODEX_TOOL_CALL_PROVIDERS,
        &placement.deferred,
        tool_options,
    );

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert("store".to_string(), json!(false));
    obj.insert("stream".to_string(), json!(true));
    // `context.systemPrompt || "You are a helpful assistant."` — the empty string is falsy.
    obj.insert(
        "instructions".to_string(),
        json!(
            ctx.system_prompt
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("You are a helpful assistant.")
        ),
    );
    obj.insert("input".to_string(), Value::Array(messages));
    obj.insert(
        "text".to_string(),
        json!({ "verbosity": codex.text_verbosity.as_deref().filter(|s| !s.is_empty()).unwrap_or("low") }),
    );
    obj.insert(
        "include".to_string(),
        json!(["reasoning.encrypted_content"]),
    );
    // `prompt_cache_key: cacheSessionId` — `undefined` serializes to an absent key.
    if let Some(sid) = cache_session_id {
        obj.insert("prompt_cache_key".to_string(), json!(sid));
    }
    obj.insert(
        "tool_choice".to_string(),
        json!(codex.tool_choice.unwrap_or(CodexToolChoice::Auto).as_str()),
    );
    obj.insert("parallel_tool_calls".to_string(), json!(true));

    if let Some(temp) = opts.temperature {
        obj.insert("temperature".to_string(), json!(temp));
    }
    if let Some(tier) = codex.service_tier.as_deref() {
        obj.insert("service_tier".to_string(), json!(tier));
    }
    if !placement.immediate.is_empty() {
        obj.insert(
            "tools".to_string(),
            Value::Array(convert_responses_tools(&placement.immediate, tool_options)),
        );
    }

    // `if (options?.reasoningEffort !== undefined)` (:582). cyrup's unified level is `off` exactly
    // where pi's `streamSimple` leaves `reasoningEffort` undefined (:516-517), so `off` emits
    // nothing at all — Codex has no `openai-responses`-style `{ effort }`-only off branch.
    let clamped = clamp_thinking_level(model, opts.reasoning);
    if clamped != ModelThinkingLevel::Off {
        let key = thinking_level_key(clamped);
        // `model.thinkingLevelMap?.[level] ?? level`, then `if (effort !== null)`: a level mapped
        // explicitly to `null` suppresses the whole `reasoning` object. Ported for fidelity even
        // though `clampThinkingLevel` treats a null-mapped rung as unsupported and re-targets the
        // request before it gets here — the guard becomes live only for a caller that supplies
        // `reasoningEffort` directly, which is upstream's non-`streamSimple` entry point.
        let effort = match level_map_lookup(model.thinking_level_map.as_ref(), key) {
            Some(None) => None,
            Some(Some(mapped)) => Some(mapped.clone()),
            None => Some(key.to_string()),
        };
        if let Some(effort) = effort {
            let summary = codex
                .reasoning_summary
                .unwrap_or(CodexReasoningSummary::Auto)
                .as_str();
            obj.insert(
                "reasoning".to_string(),
                json!({ "effort": effort, "summary": summary }),
            );
        }
    }

    Value::Object(obj)
}

// ---------------------------------------------------------------------------
// Retry decisions (pi :130-183)
// ---------------------------------------------------------------------------

/// `haystack` contains `needle`, case-insensitively (pi's `/…/i` literal alternatives).
fn contains_ci(haystack_lower: &str, needle_lower: &str) -> bool {
    haystack_lower.contains(needle_lower)
}

/// `left` followed by `right` with at most one character between them — pi's `.?` in
/// `/rate.?limit/i` and friends. Case-insensitive (the caller lower-cases once).
fn contains_with_optional_gap(haystack_lower: &str, left: &str, right: &str) -> bool {
    let mut from = 0usize;
    while let Some(found) = haystack_lower.get(from..).and_then(|s| s.find(left)) {
        let at = from + found;
        let after = at + left.len();
        for gap in [0usize, 1usize] {
            // `.` never matches a newline in JS without the `s` flag.
            if gap == 1 {
                match haystack_lower.get(after..after + 1) {
                    Some("\n") | None => continue,
                    Some(_) => {}
                }
            }
            if haystack_lower
                .get(after + gap..)
                .is_some_and(|rest| rest.starts_with(right))
            {
                return true;
            }
        }
        from = at + 1;
        if from >= haystack_lower.len() {
            break;
        }
    }
    false
}

/// 1:1 port of pi `isTerminalRateLimitError` (`openai-codex-responses.ts:130-134`): a 429 that says
/// the account is out of money/quota is NOT retryable, however many attempts remain.
pub fn is_terminal_rate_limit_error(error_text: &str) -> bool {
    let lower = error_text.to_lowercase();
    [
        "gousagelimiterror",
        "freeusagelimiterror",
        "monthly usage limit reached",
        "available balance",
        "insufficient_quota",
        "out of budget",
        "quota exceeded",
        "billing",
    ]
    .iter()
    .any(|needle| contains_ci(&lower, needle))
}

/// 1:1 port of pi `isRetryableError` (`openai-codex-responses.ts:136-144`).
pub fn is_retryable_error(status: u16, error_text: &str) -> bool {
    if status == 429 && is_terminal_rate_limit_error(error_text) {
        return false;
    }
    if matches!(status, 429 | 500 | 502 | 503 | 504) {
        return true;
    }
    let lower = error_text.to_lowercase();
    contains_with_optional_gap(&lower, "rate", "limit")
        || contains_ci(&lower, "overloaded")
        || contains_with_optional_gap(&lower, "service", "unavailable")
        || contains_with_optional_gap(&lower, "upstream", "connect")
        || contains_with_optional_gap(&lower, "connection", "refused")
}

/// 1:1 port of pi `getRetryAfterDelayMs` (`openai-codex-responses.ts:146-171`): `retry-after-ms`
/// first, then `retry-after` as seconds, then `retry-after` as an HTTP-date. Every result is
/// clamped at zero (`Math.max(0, …)`), and an unparseable value yields `None`.
pub fn get_retry_after_delay_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());

    if let Some(raw) = get("retry-after-ms") {
        // `Number(raw)`: whitespace-only is 0, garbage is NaN (which `Number.isFinite` rejects).
        if let Some(millis) = js_number(raw) {
            return Some(clamp_non_negative(millis));
        }
    }

    // `if (!retryAfter) return undefined` — an empty header value is falsy.
    let raw = get("retry-after").filter(|s| !s.is_empty())?;
    if let Some(seconds) = js_number(raw) {
        return Some(clamp_non_negative(seconds * 1000.0));
    }
    let at = parse_http_date_ms(raw)?;
    Some(clamp_non_negative((at - now_millis()) as f64))
}

/// 1:1 port of pi `validateRetryDelayMs` (`openai-codex-responses.ts:175-183`). `Err` is the
/// `RetryDelayExceededError` message, which the caller must NOT retry.
pub fn validate_retry_delay_ms(
    delay_ms: u64,
    max_retry_delay_ms: Option<u64>,
) -> Result<u64, String> {
    let max = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
    if max > 0 && delay_ms > max {
        return Err(format!(
            "Server requested {}s retry delay (max: {}s)",
            ceil_seconds(delay_ms),
            ceil_seconds(max),
        ));
    }
    Ok(delay_ms)
}

/// pi `BASE_DELAY_MS * 2 ** attempt` (`:433`, `:460`) — no jitter, no ceiling.
fn backoff_delay_ms(attempt: u32) -> u64 {
    BASE_DELAY_MS.saturating_mul(1u64.checked_shl(attempt.min(32)).unwrap_or(u64::MAX))
}

/// `Math.ceil(ms / 1000)`.
fn ceil_seconds(ms: u64) -> u64 {
    ms.div_ceil(1000)
}

/// `Math.max(0, value)` on a possibly-NaN JS number.
fn clamp_non_negative(value: f64) -> u64 {
    if value.is_finite() && value > 0.0 {
        value as u64
    } else {
        0
    }
}

/// JS `Number(raw)` restricted to the finite case: leading/trailing whitespace is ignored, the
/// empty/whitespace-only string is `0`, and anything else unparseable is `NaN` ⇒ `None`.
fn js_number(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }
    trimmed.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// 1:1 port of pi `parseErrorResponse` (`openai-codex-responses.ts:1533-1558`) collapsed to the
/// single string its caller throws (`info.friendlyMessage || info.message`, `:446`).
///
/// `now_ms` is pi's `Date.now()`, taken as a parameter so the `resets_at` arithmetic is testable.
pub fn parse_error_response(status: u16, raw: &str, now_ms: i64) -> String {
    // `let message = raw || response.statusText || "Request failed";` — cyrup's transport does not
    // retain `statusText`, so a blank body goes straight to the literal fallback.
    let mut message = if raw.is_empty() {
        "Request failed".to_string()
    } else {
        raw.to_string()
    };
    let mut friendly: Option<String> = None;

    if let Ok(parsed) = serde_json::from_str::<Value>(raw)
        && let Some(err) = parsed.get("error").filter(|e| e.is_object())
    {
        let code = err
            .get("code")
            .and_then(Value::as_str)
            .or_else(|| err.get("type").and_then(Value::as_str))
            .unwrap_or("");
        let code_lower = code.to_lowercase();
        let limit_code = code_lower.contains("usage_limit_reached")
            || code_lower.contains("usage_not_included")
            || code_lower.contains("rate_limit_exceeded");
        if limit_code || status == 429 {
            let plan = err
                .get("plan_type")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(|p| format!(" ({} plan)", p.to_lowercase()))
                .unwrap_or_default();
            let when = err
                .get("resets_at")
                .and_then(Value::as_f64)
                .map(|resets_at| {
                    let mins = ((resets_at * 1000.0 - now_ms as f64) / 60_000.0).round();
                    format!(" Try again in ~{} min.", mins.max(0.0) as i64)
                })
                .unwrap_or_default();
            friendly = Some(
                format!("You have hit your ChatGPT usage limit{plan}.{when}")
                    .trim()
                    .to_string(),
            );
        }
        // `message = err.message || friendlyMessage || message`.
        if let Some(m) = err
            .get("message")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            message = m.to_string();
        } else if let Some(f) = &friendly {
            message = f.clone();
        }
    }

    friendly.unwrap_or(message)
}

// ---------------------------------------------------------------------------
// Codex → Responses event mapping (pi :721-757)
// ---------------------------------------------------------------------------

/// The outcome of mapping one Codex SSE event (pi `mapCodexEvents`, `:721-752`).
#[derive(Debug, PartialEq, Eq)]
pub enum MappedCodexEvent {
    /// `if (!type) continue` — the event is dropped.
    Skip,
    /// A `CodexApiError` was thrown; the string is upstream's exact `Error.message`.
    Fail(String),
    /// Forwarded to the shared Responses decoder unchanged.
    Pass(Value),
    /// The rewritten `response.completed` terminal; upstream `return`s right after yielding it.
    Terminal(Value),
}

/// 1:1 port of pi `mapCodexEvents`'s per-event body (`openai-codex-responses.ts:722-751`) plus
/// `normalizeCodexStatus` (`:754-757`).
///
/// `request_service_tier` is `options?.serviceTier`; it is folded into the terminal event's
/// `response.service_tier` by [`resolve_codex_service_tier`] so the shared decoder's
/// `applyServiceTierPricing` — whose multiplier table is byte-identical to Codex's
/// `getServiceTierCostMultiplier` (`:598-610` vs `openai-responses.ts:281-293`) — prices the turn
/// exactly as upstream's `resolveServiceTier` hook would.
pub fn map_codex_event(event: &Value, request_service_tier: Option<&str>) -> MappedCodexEvent {
    let Some(etype) = event.get("type").and_then(Value::as_str) else {
        return MappedCodexEvent::Skip;
    };

    if etype == "error" {
        let (code, message) = extract_codex_event_error(event);
        let detail = message
            .or(code)
            .unwrap_or_else(|| serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string()));
        return MappedCodexEvent::Fail(format!("Codex error: {detail}"));
    }

    if etype == "response.failed" {
        let message = event
            .pointer("/response/error/message")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        return MappedCodexEvent::Fail(message.unwrap_or("Codex response failed").to_string());
    }

    if matches!(
        etype,
        "response.done" | "response.completed" | "response.incomplete"
    ) {
        let mut mapped = event.clone();
        if let Some(obj) = mapped.as_object_mut() {
            obj.insert("type".to_string(), json!("response.completed"));
            if let Some(response) = obj.get_mut("response").and_then(Value::as_object_mut) {
                // `status: normalizeCodexStatus(response.status)` — an unknown status becomes
                // `undefined`, i.e. an absent key.
                let normalized = response
                    .get("status")
                    .and_then(Value::as_str)
                    .filter(|s| CODEX_RESPONSE_STATUSES.contains(s))
                    .map(str::to_string);
                match normalized {
                    Some(status) => {
                        response.insert("status".to_string(), json!(status));
                    }
                    None => {
                        response.remove("status");
                    }
                }
                let resolved = resolve_codex_service_tier(
                    response.get("service_tier").and_then(Value::as_str),
                    request_service_tier,
                );
                match resolved {
                    Some(tier) => {
                        response.insert("service_tier".to_string(), json!(tier));
                    }
                    None => {
                        response.remove("service_tier");
                    }
                }
            }
        }
        return MappedCodexEvent::Terminal(mapped);
    }

    MappedCodexEvent::Pass(event.clone())
}

/// 1:1 port of pi `extractCodexEventError` (`openai-codex-responses.ts:708-719`): the code/message
/// may sit on the event or inside a nested `error` object.
fn extract_codex_event_error(event: &Value) -> (Option<String>, Option<String>) {
    let field = |name: &str| {
        event
            .get(name)
            .and_then(Value::as_str)
            .or_else(|| {
                event
                    .pointer(&format!("/error/{name}"))
                    .and_then(Value::as_str)
            })
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    (field("code"), field("message"))
}

/// 1:1 port of pi `resolveCodexServiceTier` (`openai-codex-responses.ts:627-635`): the backend
/// reporting `"default"` does not override an explicitly requested `flex`/`priority` tier.
pub fn resolve_codex_service_tier(
    response_service_tier: Option<&str>,
    request_service_tier: Option<&str>,
) -> Option<String> {
    if response_service_tier == Some("default")
        && matches!(request_service_tier, Some("flex") | Some("priority"))
    {
        return request_service_tier.map(str::to_string);
    }
    response_service_tier
        .or(request_service_tier)
        .map(str::to_string)
}

/// Streaming state for [`map_codex_frames`].
struct MapState {
    inner: FrameStream,
    done: bool,
    request_service_tier: Option<String>,
}

/// Apply [`map_codex_event`] across an SSE frame stream (pi's `mapCodexEvents` generator wrapped
/// around `parseSSE`, `:664`).
///
/// The generator's two control-flow effects are preserved: an untyped event is dropped, and the
/// terminal event ENDS the stream (upstream `return`s from the generator), so nothing after
/// `response.done` reaches the decoder.
///
/// **Error-text delta.** Upstream's `CodexApiError`/`CodexProtocolError` reach the outer catch,
/// which writes `error.message` verbatim into `errorMessage`. Here they travel as
/// [`ProviderError::Decode`], whose `Display` prefixes `"decode error: "`, because emitting an
/// unprefixed terminal from inside the *shared* decoder would mean changing
/// `openai_responses::decode_stream`. The message body is upstream's exact text.
pub fn map_codex_frames(frames: FrameStream, request_service_tier: Option<String>) -> FrameStream {
    let state = MapState {
        inner: frames,
        done: false,
        request_service_tier,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        loop {
            let frame = match state.inner.next().await {
                // End of input: the shared decoder's own truncated-stream rule takes over.
                None => return None,
                Some(Err(e)) => {
                    state.done = true;
                    return Some((Err(e), state));
                }
                Some(Ok(frame)) => frame,
            };

            let data = frame.data.trim();
            // pi `parseSSE`: `if (data && data !== "[DONE]")`.
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let event: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(e) => {
                    state.done = true;
                    // pi `CodexProtocolError(\`Invalid Codex SSE JSON: ${formatThrownValue(cause)}\`)`
                    // (:801).
                    return Some((
                        Err(ProviderError::Decode(format!(
                            "Invalid Codex SSE JSON: {e}"
                        ))),
                        state,
                    ));
                }
            };

            let tier = state.request_service_tier.clone();
            match map_codex_event(&event, tier.as_deref()) {
                MappedCodexEvent::Skip => continue,
                MappedCodexEvent::Fail(message) => {
                    state.done = true;
                    return Some((Err(ProviderError::Decode(message)), state));
                }
                MappedCodexEvent::Pass(value) => {
                    return Some((Ok(reframe(&frame, &value)), state));
                }
                MappedCodexEvent::Terminal(value) => {
                    state.done = true;
                    return Some((Ok(reframe(&frame, &value)), state));
                }
            }
        }
    }))
}

/// Re-serialize a mapped event back into an SSE frame for the shared decoder.
fn reframe(original: &SseFrame, value: &Value) -> SseFrame {
    SseFrame {
        event: original.event.clone(),
        data: value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Terminals
// ---------------------------------------------------------------------------

/// A terminal error carrying pi's exact thrown message. Like pi's own catch block (`:489-499`) the
/// content is empty: nothing had been accumulated when the failure occurred on these paths.
fn error_event(model: &Model, api: &ApiId, message: String, aborted: bool) -> StreamEvent {
    let msg = AssistantMessage::errored(
        model.provider.clone(),
        model.id.as_str(),
        Some(api.clone()),
        if aborted {
            StopReason::Aborted
        } else {
            StopReason::Error
        },
        message,
    );
    StreamEvent::terminal(msg)
}

/// pi's abort terminal: `stopReason = "aborted"` with the `"Request was aborted"` text its
/// `throw` produced (`:397`, `:449-451`, `:495-497`).
fn aborted_event(model: &Model, api: &ApiId) -> StreamEvent {
    error_event(model, api, "Request was aborted".to_string(), true)
}

/// Interruptible `sleep(ms, signal)` (pi `:185-197`). `false` means the abort fired.
async fn sleep_or_abort(cancel: &CancelToken, delay_ms: u64) -> bool {
    if cancel.is_cancelled() {
        return false;
    }
    cancel
        .run_until_cancelled(tokio::time::sleep(std::time::Duration::from_millis(
            delay_ms,
        )))
        .await
        .is_some()
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
    use crate::api::channel;
    use crate::model::{Modality, ModelCost};
    use crate::stream::sse::decode_sse_bytes;
    use cyrup_core::SessionId;

    fn codex_model(id: &str) -> Model {
        Model {
            id: id.into(),
            name: "M".into(),
            api: API_ID.into(),
            provider: "openai-codex".into(),
            base_url: String::new(),
            reasoning: true,
            input: vec![Modality::Text],
            // NON-ZERO rates. With `ModelCost::default()` every rate is 0.0, so any assertion of
            // the form `|priority_cost - baseline_cost * N| < eps` reduces to `|0.0 - 0.0| < eps`
            // and holds no matter what the code does — which is exactly how the service-tier
            // pricing test below shipped vacuous. A reviewer proved it by deleting the whole
            // feature under test and watching every test stay green.
            cost: ModelCost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: None,
            },
            context_window: 1000,
            max_tokens: 1000,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn opts() -> OpenAiCodexResponsesOptions {
        OpenAiCodexResponsesOptions::default()
    }

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            map.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        map
    }

    /// A locally-synthesized JWT — three dot-separated base64 segments, no signature verification
    /// anywhere in this code path, and no real credential.
    fn fake_jwt(payload: &Value) -> String {
        let body = ATOB.encode(serde_json::to_vec(payload).unwrap());
        format!("eyJhbGciOiJub25lIn0.{body}.sig")
    }

    // -- URL ---------------------------------------------------------------

    #[test]
    fn codex_url_completes_without_doubling_segments() {
        // pi resolveCodexUrl (:637-643).
        assert_eq!(
            resolve_codex_url(""),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            resolve_codex_url("   "),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            resolve_codex_url("https://example.test/backend-api/"),
            "https://example.test/backend-api/codex/responses"
        );
        assert_eq!(
            resolve_codex_url("https://example.test/backend-api/codex"),
            "https://example.test/backend-api/codex/responses"
        );
        assert_eq!(
            resolve_codex_url("https://example.test/backend-api/codex/responses///"),
            "https://example.test/backend-api/codex/responses"
        );
    }

    // -- account id --------------------------------------------------------

    #[test]
    fn account_id_comes_from_the_namespaced_claim() {
        let token = fake_jwt(&json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct_abc123" },
            "sub": "user_1",
        }));
        assert_eq!(extract_account_id(&token).unwrap(), "acct_abc123");
    }

    #[test]
    fn every_account_id_failure_collapses_to_one_message() {
        const FAILED: &str = "Failed to extract accountId from token";
        // Wrong segment count (:1567).
        assert_eq!(extract_account_id("a.b").unwrap_err(), FAILED);
        assert_eq!(extract_account_id("a.b.c.d").unwrap_err(), FAILED);
        // Undecodable payload.
        assert_eq!(extract_account_id("a.!!!!.c").unwrap_err(), FAILED);
        // Claim absent (:1569-1570).
        assert_eq!(
            extract_account_id(&fake_jwt(&json!({ "sub": "user_1" }))).unwrap_err(),
            FAILED
        );
        // Claim present but empty — falsy in pi's `if (!accountId)`.
        let empty = fake_jwt(&json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "" },
        }));
        assert_eq!(extract_account_id(&empty).unwrap_err(), FAILED);
        // MIRROR: the same shape with a non-empty id still succeeds, so the assertions above are
        // testing the claim rules and not a permanently-broken decoder.
        let ok = fake_jwt(&json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct_ok" },
        }));
        assert_eq!(extract_account_id(&ok).unwrap(), "acct_ok");
    }

    // -- headers -----------------------------------------------------------

    #[test]
    fn sse_headers_match_upstream_and_cannot_be_overridden() {
        let mut model = codex_model("gpt-5.5-codex");
        // A differently-cased override of a Codex identity header must be REPLACED, not duplicated
        // (pi uses `Headers`, which is case-insensitive).
        model.headers = Some(
            [
                (
                    "authorization".to_string(),
                    Some("Bearer stale".to_string()),
                ),
                ("x-extra".to_string(), Some("kept".to_string())),
            ]
            .into_iter()
            .collect(),
        );
        let auth = AuthResult::from_key("tok", "test");
        let h = build_sse_headers(
            &model,
            &auth,
            &StreamOptions::default(),
            "acct_1",
            "tok",
            Some("sess-1"),
        );

        assert_eq!(
            h.get("Authorization"),
            Some(&Some("Bearer tok".to_string()))
        );
        assert!(!h.contains_key("authorization"), "duplicate cased header");
        assert_eq!(
            h.get("chatgpt-account-id"),
            Some(&Some("acct_1".to_string()))
        );
        // pi `headers.set("originator", "pi")` (:1593) — the backend gates on this identity.
        assert_eq!(h.get("originator"), Some(&Some("pi".to_string())));
        assert_eq!(
            h.get("OpenAI-Beta"),
            Some(&Some("responses=experimental".to_string()))
        );
        assert_eq!(
            h.get("accept"),
            Some(&Some("text/event-stream".to_string()))
        );
        assert_eq!(
            h.get("content-type"),
            Some(&Some("application/json".to_string()))
        );
        assert_eq!(h.get("session-id"), Some(&Some("sess-1".to_string())));
        assert_eq!(
            h.get("x-client-request-id"),
            Some(&Some("sess-1".to_string()))
        );
        // A non-conflicting model header survives.
        assert_eq!(h.get("x-extra"), Some(&Some("kept".to_string())));
        assert!(
            h.get("User-Agent")
                .and_then(|v| v.clone())
                .is_some_and(|v| v.starts_with("pi (")),
            "user agent: {:?}",
            h.get("User-Agent")
        );
    }

    #[test]
    fn session_headers_are_omitted_without_a_session() {
        let model = codex_model("gpt-5.5-codex");
        let auth = AuthResult::from_key("tok", "test");
        let h = build_sse_headers(
            &model,
            &auth,
            &StreamOptions::default(),
            "acct",
            "tok",
            None,
        );
        assert!(!h.contains_key("session-id"));
        assert!(!h.contains_key("x-client-request-id"));
    }

    // -- request body ------------------------------------------------------

    #[test]
    fn body_matches_upstream_shape() {
        let model = codex_model("gpt-5.5-codex");
        let ctx = Context::default();
        let so = StreamOptions {
            // Codex sends NO max_output_tokens even when the caller sets a cap.
            max_tokens: Some(4096),
            ..Default::default()
        };
        let body = build_request_body(&model, &ctx, &so, &opts(), None);

        assert_eq!(body["model"], json!("gpt-5.5-codex"));
        assert_eq!(body["store"], json!(false));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["instructions"], json!("You are a helpful assistant."));
        assert_eq!(body["text"], json!({ "verbosity": "low" }));
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert!(
            body.get("max_output_tokens").is_none(),
            "Codex never sends max_output_tokens (buildRequestBody :553-564)"
        );
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("temperature").is_none());
        assert!(body.get("service_tier").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn system_prompt_rides_in_instructions_not_input() {
        // pi passes `includeSystemPrompt: false` (:545) and puts the prompt in `instructions`
        // (:557) — the opposite of `openai-responses`, which prepends a system/developer item.
        let model = codex_model("gpt-5.5-codex");
        let ctx = Context {
            system_prompt: Some("BE TERSE".to_string()),
            messages: vec![cyrup_core::Message::User {
                content: vec![cyrup_core::Content::text("hi")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let body = build_request_body(&model, &ctx, &StreamOptions::default(), &opts(), None);
        assert_eq!(body["instructions"], json!("BE TERSE"));
        let raw = serde_json::to_string(&body["input"]).unwrap();
        assert!(
            !raw.contains("BE TERSE"),
            "system prompt leaked into input: {raw}"
        );
        // MIRROR: the user turn IS in `input`, so the assertion above is not vacuously green on an
        // empty array.
        assert!(raw.contains("hi"), "user message missing from input: {raw}");
    }

    #[test]
    fn empty_system_prompt_falls_back_to_the_default_instructions() {
        // `context.systemPrompt || "You are a helpful assistant."` — "" is falsy.
        let model = codex_model("gpt-5.5-codex");
        let ctx = Context {
            system_prompt: Some(String::new()),
            ..Default::default()
        };
        let body = build_request_body(&model, &ctx, &StreamOptions::default(), &opts(), None);
        assert_eq!(body["instructions"], json!("You are a helpful assistant."));
    }

    #[test]
    fn optional_fields_appear_only_when_set() {
        let model = codex_model("gpt-5.5-codex");
        let ctx = Context {
            tools: vec![crate::context::ToolDef {
                name: "bash".into(),
                description: "run".into(),
                parameters: json!({ "type": "object" }),
            }],
            ..Default::default()
        };
        let so = StreamOptions {
            temperature: Some(0.25),
            tool_choice: Some(ToolChoice::Required),
            ..Default::default()
        };
        let codex = OpenAiCodexResponsesOptions {
            service_tier: Some("priority".to_string()),
            text_verbosity: Some("high".to_string()),
            ..OpenAiCodexResponsesOptions::from_stream_options(&so)
        };
        let body = build_request_body(&model, &ctx, &so, &codex, Some("sess-9"));

        assert_eq!(body["temperature"], json!(0.25));
        assert_eq!(body["service_tier"], json!("priority"));
        assert_eq!(body["text"], json!({ "verbosity": "high" }));
        assert_eq!(body["prompt_cache_key"], json!("sess-9"));
        assert_eq!(body["tool_choice"], json!("required"));
        assert_eq!(body["tools"][0]["name"], json!("bash"));
    }

    #[test]
    fn named_function_tool_choice_falls_back_to_auto() {
        // `OpenAICodexResponsesOptions["toolChoice"]` is `"auto" | "none" | "required"` (:91) —
        // the named-function form has no Codex spelling.
        let so = StreamOptions {
            tool_choice: Some(ToolChoice::Function {
                name: "bash".into(),
            }),
            ..Default::default()
        };
        let codex = OpenAiCodexResponsesOptions::from_stream_options(&so);
        assert_eq!(codex.tool_choice, None);
        let body = build_request_body(
            &codex_model("gpt-5.5-codex"),
            &Context::default(),
            &so,
            &codex,
            None,
        );
        assert_eq!(body["tool_choice"], json!("auto"));
    }

    #[test]
    fn session_id_is_dropped_when_cache_retention_is_none() {
        // pi `options?.cacheRetention === "none" ? undefined : options?.sessionId` (:281).
        let none = StreamOptions {
            session_id: Some(SessionId::from("sess-1")),
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        };
        assert_eq!(codex_session_id(&none), None);
        // MIRROR: any other retention keeps it (clamped).
        let short = StreamOptions {
            session_id: Some(SessionId::from("sess-1")),
            cache_retention: Some(CacheRetention::Short),
            ..Default::default()
        };
        assert_eq!(codex_session_id(&short).as_deref(), Some("sess-1"));
    }

    #[test]
    fn reasoning_effort_maps_and_null_suppresses() {
        let mut model = codex_model("gpt-5.5-codex");
        model.thinking_level_map = Some(
            [
                ("high".to_string(), Some("xhigh".to_string())),
                ("medium".to_string(), None),
            ]
            .into_iter()
            .collect(),
        );
        // Mapped level: `model.thinkingLevelMap?.[level] ?? level` (:586).
        let so = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_request_body(&model, &Context::default(), &so, &opts(), None);
        assert_eq!(
            body["reasoning"],
            json!({ "effort": "xhigh", "summary": "auto" })
        );

        // A level mapped to `null` is *unsupported*, so `clampThinkingLevel` (:516) moves the
        // request to the nearest supported rung before `buildRequestBody` ever sees it — which is
        // why the `if (effort !== null)` guard at :587 cannot fire from this path. `medium` → the
        // next supported rung, `high`, whose mapped effort is `xhigh`.
        let so = StreamOptions {
            reasoning: ModelThinkingLevel::Medium,
            ..Default::default()
        };
        let body = build_request_body(&model, &Context::default(), &so, &opts(), None);
        assert_eq!(body["reasoning"]["effort"], json!("xhigh"));

        // `off` leaves `reasoningEffort` undefined (:516-517) — no reasoning key, and NO
        // `openai-responses`-style `{ effort: "none" }` off-branch.
        let so = StreamOptions {
            reasoning: ModelThinkingLevel::Off,
            ..Default::default()
        };
        let body = build_request_body(&model, &Context::default(), &so, &opts(), None);
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn reasoning_summary_option_overrides_auto() {
        let model = codex_model("gpt-5.5-codex");
        let codex = OpenAiCodexResponsesOptions {
            reasoning_summary: Some(CodexReasoningSummary::Detailed),
            ..Default::default()
        };
        let so = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_request_body(&model, &Context::default(), &so, &codex, None);
        assert_eq!(body["reasoning"]["summary"], json!("detailed"));
    }

    // -- retry decisions ---------------------------------------------------

    #[test]
    fn terminal_rate_limit_vectors() {
        // pi's /GoUsageLimitError|FreeUsageLimitError|Monthly usage limit reached|available
        // balance|insufficient_quota|out of budget|quota exceeded|billing/i (:131-133).
        for text in [
            "GoUsageLimitError",
            "freeusagelimiterror",
            "Monthly usage limit reached",
            "your available balance is 0",
            "insufficient_quota",
            "you are out of budget",
            "QUOTA EXCEEDED",
            "billing",
        ] {
            assert!(
                is_terminal_rate_limit_error(text),
                "expected terminal: {text}"
            );
        }
        // MIRROR: an ordinary rate-limit body is NOT terminal.
        assert!(!is_terminal_rate_limit_error(
            "Rate limit exceeded, retry soon"
        ));
    }

    #[test]
    fn retryable_status_and_text_vectors() {
        // pi isRetryableError (:136-144).
        assert!(is_retryable_error(429, "Rate limit exceeded"));
        assert!(!is_retryable_error(429, "insufficient_quota"));
        for status in [500u16, 502, 503, 504] {
            assert!(is_retryable_error(status, ""), "status {status}");
        }
        // `.?` = at most one arbitrary character.
        assert!(is_retryable_error(400, "rate limit hit"));
        assert!(is_retryable_error(400, "rate-limit hit"));
        assert!(is_retryable_error(400, "ratelimit hit"));
        assert!(is_retryable_error(400, "OVERLOADED"));
        assert!(is_retryable_error(400, "service unavailable"));
        assert!(is_retryable_error(400, "service_unavailable"));
        assert!(is_retryable_error(400, "upstream connect error"));
        assert!(is_retryable_error(400, "connection refused"));
        // MIRROR: an ordinary 4xx with none of those markers is not retried.
        assert!(!is_retryable_error(400, "invalid request: bad model"));
        assert!(!is_retryable_error(401, "unauthorized"));
        // Two characters between the halves is more than `.?` allows.
        assert!(!is_retryable_error(400, "rate  limit"));
    }

    #[test]
    fn retry_after_header_precedence() {
        // pi getRetryAfterDelayMs (:146-171): retry-after-ms wins over retry-after.
        assert_eq!(
            get_retry_after_delay_ms(&headers(&[
                ("retry-after-ms", "1500"),
                ("retry-after", "9")
            ])),
            Some(1500)
        );
        assert_eq!(
            get_retry_after_delay_ms(&headers(&[("retry-after", "2")])),
            Some(2000)
        );
        // Negative values clamp to zero (`Math.max(0, …)`).
        assert_eq!(
            get_retry_after_delay_ms(&headers(&[("retry-after-ms", "-10")])),
            Some(0)
        );
        // A past HTTP-date clamps to zero rather than going negative.
        assert_eq!(
            get_retry_after_delay_ms(&headers(&[(
                "retry-after",
                "Wed, 21 Oct 2015 07:28:00 GMT"
            )])),
            Some(0)
        );
        // Unparseable → undefined (the caller then uses the exponential ladder).
        assert_eq!(
            get_retry_after_delay_ms(&headers(&[("retry-after", "soon")])),
            None
        );
        assert_eq!(get_retry_after_delay_ms(&headers(&[])), None);
    }

    #[test]
    fn retry_delay_ceiling_message_is_upstreams() {
        // pi validateRetryDelayMs (:175-183).
        assert_eq!(validate_retry_delay_ms(30_000, None), Ok(30_000));
        assert_eq!(
            validate_retry_delay_ms(90_000, None).unwrap_err(),
            "Server requested 90s retry delay (max: 60s)"
        );
        // `maxRetryDelayMs > 0` gate: zero disables the ceiling entirely.
        assert_eq!(validate_retry_delay_ms(900_000, Some(0)), Ok(900_000));
        assert_eq!(
            validate_retry_delay_ms(11_000, Some(10_000)).unwrap_err(),
            "Server requested 11s retry delay (max: 10s)"
        );
    }

    #[test]
    fn exponential_ladder_has_no_jitter() {
        // pi `BASE_DELAY_MS * 2 ** attempt` (:433).
        assert_eq!(backoff_delay_ms(0), 1_000);
        assert_eq!(backoff_delay_ms(1), 2_000);
        assert_eq!(backoff_delay_ms(2), 4_000);
        assert_eq!(backoff_delay_ms(3), 8_000);
    }

    // -- error bodies ------------------------------------------------------

    #[test]
    fn usage_limit_bodies_get_the_friendly_message() {
        // pi parseErrorResponse (:1533-1558) + `info.friendlyMessage || info.message` (:446).
        let now = 1_700_000_000_000i64;
        let resets_at = (now / 1000) + 3600;
        let raw = json!({
            "error": {
                "code": "usage_limit_reached",
                "plan_type": "Plus",
                "resets_at": resets_at,
            }
        })
        .to_string();
        assert_eq!(
            parse_error_response(429, &raw, now),
            "You have hit your ChatGPT usage limit (plus plan). Try again in ~60 min."
        );

        // No plan and no reset time: the trimmed bare sentence.
        let raw = json!({ "error": { "code": "usage_not_included" } }).to_string();
        assert_eq!(
            parse_error_response(403, &raw, now),
            "You have hit your ChatGPT usage limit."
        );

        // Any 429 gets the friendly message even with an unrelated code.
        let raw = json!({ "error": { "code": "slow_down", "message": "chill" } }).to_string();
        assert_eq!(
            parse_error_response(429, &raw, now),
            "You have hit your ChatGPT usage limit."
        );
    }

    #[test]
    fn non_limit_bodies_surface_the_provider_message() {
        let now = 1_700_000_000_000i64;
        // MIRROR: without the limit code and without a 429, `err.message` is what surfaces.
        let raw =
            json!({ "error": { "code": "invalid_request", "message": "bad model" } }).to_string();
        assert_eq!(parse_error_response(400, &raw, now), "bad model");
        // Unparseable body: the raw text.
        assert_eq!(
            parse_error_response(500, "upstream boom", now),
            "upstream boom"
        );
        // Empty body: `raw || statusText || "Request failed"`.
        assert_eq!(parse_error_response(500, "", now), "Request failed");
    }

    // -- event mapping -----------------------------------------------------

    #[test]
    fn codex_error_events_carry_upstream_text() {
        // pi `Codex error: ${message || code || JSON.stringify(event)}` (:728).
        assert_eq!(
            map_codex_event(&json!({ "type": "error", "message": "boom" }), None),
            MappedCodexEvent::Fail("Codex error: boom".to_string())
        );
        // Nested error object (:709-718).
        assert_eq!(
            map_codex_event(
                &json!({ "type": "error", "error": { "code": "websocket_connection_limit_reached" } }),
                None
            ),
            MappedCodexEvent::Fail("Codex error: websocket_connection_limit_reached".to_string())
        );
        // Neither code nor message: the serialized event.
        let MappedCodexEvent::Fail(text) = map_codex_event(&json!({ "type": "error" }), None)
        else {
            panic!("expected Fail");
        };
        assert!(text.starts_with("Codex error: {"), "{text}");
    }

    #[test]
    fn response_failed_uses_its_error_message() {
        // pi `message || "Codex response failed"` (:738).
        assert_eq!(
            map_codex_event(
                &json!({ "type": "response.failed", "response": { "error": { "message": "nope" } } }),
                None
            ),
            MappedCodexEvent::Fail("nope".to_string())
        );
        assert_eq!(
            map_codex_event(&json!({ "type": "response.failed" }), None),
            MappedCodexEvent::Fail("Codex response failed".to_string())
        );
    }

    #[test]
    fn terminal_events_are_rewritten_to_response_completed() {
        // pi :741-748 — all three terminals collapse to `response.completed`.
        for etype in ["response.done", "response.completed", "response.incomplete"] {
            let ev = json!({ "type": etype, "response": { "id": "r1", "status": "incomplete" } });
            let MappedCodexEvent::Terminal(mapped) = map_codex_event(&ev, None) else {
                panic!("expected Terminal for {etype}");
            };
            assert_eq!(mapped["type"], json!("response.completed"));
            assert_eq!(mapped["response"]["status"], json!("incomplete"));
        }
    }

    #[test]
    fn unknown_status_is_normalized_away() {
        // pi normalizeCodexStatus (:754-757): an out-of-set status becomes `undefined`, which the
        // shared `mapStopReason(undefined)` reads as `stop`.
        let ev = json!({ "type": "response.done", "response": { "status": "weird" } });
        let MappedCodexEvent::Terminal(mapped) = map_codex_event(&ev, None) else {
            panic!("expected Terminal");
        };
        assert!(mapped["response"].get("status").is_none());
        // MIRROR: an in-set status survives.
        let ev = json!({ "type": "response.done", "response": { "status": "queued" } });
        let MappedCodexEvent::Terminal(mapped) = map_codex_event(&ev, None) else {
            panic!("expected Terminal");
        };
        assert_eq!(mapped["response"]["status"], json!("queued"));
    }

    #[test]
    fn untyped_events_are_skipped_and_others_pass_through() {
        assert_eq!(
            map_codex_event(&json!({ "no_type": true }), None),
            MappedCodexEvent::Skip
        );
        let ev = json!({ "type": "response.output_text.delta", "delta": "hi" });
        assert_eq!(
            map_codex_event(&ev, None),
            MappedCodexEvent::Pass(ev.clone())
        );
    }

    #[test]
    fn service_tier_resolution_matches_upstream() {
        // pi resolveCodexServiceTier (:627-635).
        assert_eq!(
            resolve_codex_service_tier(Some("default"), Some("priority")).as_deref(),
            Some("priority")
        );
        assert_eq!(
            resolve_codex_service_tier(Some("default"), Some("flex")).as_deref(),
            Some("flex")
        );
        // A non-`default` response tier always wins.
        assert_eq!(
            resolve_codex_service_tier(Some("flex"), Some("priority")).as_deref(),
            Some("flex")
        );
        // `default` with no requested tier stays `default`.
        assert_eq!(
            resolve_codex_service_tier(Some("default"), None).as_deref(),
            Some("default")
        );
        // Absent response tier falls back to the requested one.
        assert_eq!(
            resolve_codex_service_tier(None, Some("flex")).as_deref(),
            Some("flex")
        );
        assert_eq!(resolve_codex_service_tier(None, None), None);
    }

    // -- end-to-end decode -------------------------------------------------

    async fn drain(sse: &'static str, request_tier: Option<&str>) -> Vec<StreamEvent> {
        let model = codex_model("gpt-5.5-codex");
        let api = ApiId::from(API_ID);
        let (sink, mut rx) = channel(64);
        let frames = map_codex_frames(
            decode_sse_bytes(sse.as_bytes().to_vec()),
            request_tier.map(str::to_string),
        );
        decode_stream(frames, &model, &api, &sink).await;
        drop(sink);
        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev);
        }
        out
    }

    const CODEX_TEXT_TURN: &str = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_c1\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"m1\"}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hello\"}\n\n",
        "data: {\"type\":\"response.done\",\"response\":{\"id\":\"resp_c1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}\n\n",
    );

    #[tokio::test]
    async fn codex_response_done_completes_the_turn() {
        // `response.done` is a Codex-only spelling; without the mapping the shared decoder would
        // never see a terminal and would report the turn as truncated.
        let events = drain(CODEX_TEXT_TURN, None).await;
        let last = events.last().expect("terminal");
        let msg = last.terminal_message().expect("terminal message");
        assert_eq!(msg.stop_reason, StopReason::Stop);
        assert_eq!(msg.response_id.as_deref(), Some("resp_c1"));
        assert_eq!(msg.usage.output, 5);
        assert!(
            matches!(last, StreamEvent::Done { .. }),
            "expected a done terminal, got {last:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::TextDelta { .. })),
            "no text delta decoded"
        );
    }

    /// Codex reaches `mapStopReason` through the shared `processResponsesStream`
    /// (v0.84.1 `openai-codex-responses.ts:52,665`), so the v0.84.1 split of `incomplete` applies
    /// here too: only `incomplete_details.reason === "max_output_tokens"` is a clean `length` stop
    /// (`openai-responses-shared.ts:751-753`); a bare `incomplete` is an error terminal
    /// (`:754-759`). `mapCodexEvents` spreads the response (`openai-codex-responses.ts:745-747`),
    /// so `incomplete_details` survives the `response.done` → `response.completed` rename.
    #[tokio::test]
    async fn incomplete_status_splits_on_the_provider_reason() {
        macro_rules! head {
            () => {
                concat!(
                    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"m1\"}}\n\n",
                    "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
                )
            };
        }
        async fn terminal(sse: &'static str) -> AssistantMessage {
            drain(sse, None)
                .await
                .last()
                .and_then(StreamEvent::terminal_message)
                .cloned()
                .expect("terminal")
        }

        let capped = terminal(concat!(
            head!(),
            "data: {\"type\":\"response.done\",\"response\":{\"id\":\"r\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
        ))
        .await;
        assert_eq!(capped.stop_reason, StopReason::Length);
        assert_eq!(capped.error_message, None);
        assert_eq!(
            capped.raw_stop_reason.as_deref(),
            Some("incomplete.max_output_tokens")
        );

        let bare = terminal(concat!(
            head!(),
            "data: {\"type\":\"response.done\",\"response\":{\"id\":\"r\",\"status\":\"incomplete\"}}\n\n",
        ))
        .await;
        assert_eq!(bare.stop_reason, StopReason::Error);
        assert_eq!(
            bare.error_message.as_deref(),
            Some("Response incomplete without a provider reason")
        );
        assert_eq!(bare.raw_stop_reason.as_deref(), Some("incomplete"));
    }

    #[tokio::test]
    async fn requested_priority_tier_survives_a_default_response_tier() {
        // The pricing consequence of resolveCodexServiceTier: `priority` doubles the cost
        // (getServiceTierCostMultiplier, :598-610). The backend answering `"default"` must not
        // erase the requested tier.
        const SSE: &str = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"m1\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
            "data: {\"type\":\"response.done\",\"response\":{\"id\":\"r\",\"status\":\"completed\",\"service_tier\":\"default\",\"usage\":{\"input_tokens\":1000,\"output_tokens\":1000,\"total_tokens\":2000}}}\n\n",
        );
        let baseline = drain(SSE, None)
            .await
            .last()
            .and_then(StreamEvent::terminal_message)
            .cloned()
            .expect("terminal");
        let priority = drain(SSE, Some("priority"))
            .await
            .last()
            .and_then(StreamEvent::terminal_message)
            .cloned()
            .expect("terminal");
        // The baseline must be genuinely non-zero, or the ratio assertion below is vacuous — the
        // model now carries real rates precisely so this can be checked.
        assert!(
            baseline.usage.cost.total > 0.0,
            "the baseline must cost something for the ratio to mean anything (got {})",
            baseline.usage.cost.total
        );
        assert!(
            (priority.usage.cost.total - baseline.usage.cost.total * 2.0).abs() < 1e-9,
            "a requested `priority` tier must survive a `\"default\"` response tier and price at 2x \
             (getServiceTierCostMultiplier, :598-610) — priority {} vs baseline {}",
            priority.usage.cost.total,
            baseline.usage.cost.total
        );
    }

    /// MIRROR for the test above: the resolution is a real decision, not a blanket doubling. With
    /// NO requested tier the response's own `"default"` prices at 1x, so the 2x assertion is about
    /// `resolve_codex_service_tier` keeping the request's tier rather than about the arithmetic.
    #[tokio::test]
    async fn an_unrequested_tier_prices_at_the_responses_own_default() {
        const SSE: &str = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"m1\"}}\n\n",
            "data: {\"type\":\"response.done\",\"response\":{\"id\":\"r\",\"status\":\"completed\",\"service_tier\":\"default\",\"usage\":{\"input_tokens\":1000,\"output_tokens\":1000,\"total_tokens\":2000}}}\n\n",
        );
        let msg = drain(SSE, None)
            .await
            .last()
            .and_then(StreamEvent::terminal_message)
            .cloned()
            .expect("terminal");
        // 1000 input @ $1/1e6 + 1000 output @ $2/1e6 = 0.003, undoubled.
        assert!(
            (msg.usage.cost.total - 0.003).abs() < 1e-9,
            "an unrequested tier must price at 1x, got {}",
            msg.usage.cost.total
        );
    }

    #[tokio::test]
    async fn events_after_the_terminal_are_not_decoded() {
        // pi's generator `return`s immediately after yielding the terminal (:747).
        const SSE: &str = concat!(
            "data: {\"type\":\"response.done\",\"response\":{\"id\":\"r\",\"status\":\"completed\"}}\n\n",
            "data: {\"type\":\"error\",\"message\":\"late\"}\n\n",
        );
        let events = drain(SSE, None).await;
        let last = events.last().expect("terminal");
        assert!(
            matches!(last, StreamEvent::Done { .. }),
            "a post-terminal event leaked through: {last:?}"
        );
    }

    #[tokio::test]
    async fn a_codex_error_event_ends_the_turn_with_upstream_text() {
        const SSE: &str =
            "data: {\"type\":\"error\",\"code\":\"rate_limit\",\"message\":\"slow down\"}\n\n";
        let msg = drain(SSE, None)
            .await
            .last()
            .and_then(StreamEvent::terminal_message)
            .cloned()
            .expect("terminal");
        assert_eq!(msg.stop_reason, StopReason::Error);
        let text = msg.error_message.unwrap_or_default();
        assert!(
            text.contains("Codex error: slow down"),
            "expected pi's `Codex error: …` text, got {text}"
        );
    }

    #[tokio::test]
    async fn malformed_sse_json_reports_the_codex_protocol_error() {
        const SSE: &str = "data: {not json}\n\n";
        let msg = drain(SSE, None)
            .await
            .last()
            .and_then(StreamEvent::terminal_message)
            .cloned()
            .expect("terminal");
        let text = msg.error_message.unwrap_or_default();
        assert!(
            text.contains("Invalid Codex SSE JSON"),
            "expected pi's CodexProtocolError text, got {text}"
        );
    }

    /// PROV-051. A Codex endpoint that accepts the TCP connection and never writes a byte must
    /// terminate with pi's exact header-phase message (`openai-codex-responses.ts:412-413`
    /// @v0.83.0), not with an unattributable transport error. Red before the fix: `opts.timeout_ms`
    /// only became a reqwest `read_timeout`, so the terminal was
    /// `transport error: … operation timed out` with no mention of the configured value.
    #[tokio::test]
    async fn a_stalled_header_phase_names_the_timeout_and_its_value() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept and hold the connection open, writing nothing.
        let held = tokio::spawn(async move {
            let mut sockets = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                sockets.push(sock);
            }
        });

        let mut model = codex_model("gpt-5-codex");
        model.base_url = format!("http://{addr}");
        let (sink, mut rx) = channel(64);
        // The key MUST be a real Codex JWT carrying the namespaced account claim: pi runs
        // `const accountId = extractAccountId(apiKey)` at `openai-codex-responses.ts:276` @v0.83.0,
        // BEFORE the request is built, and it throws on anything else. A bare `sk-test` therefore
        // terminates with "Failed to extract accountId from token" and the header phase is never
        // reached — the request under test never leaves the process.
        let token = fake_jwt(&json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct_timeout" },
        }));
        let opts = StreamOptions {
            api_key: Some(token.clone()),
            timeout_ms: Some(200),
            // One attempt, so the assertion is on the FIRST failure's text.
            max_retries: Some(0),
            ..Default::default()
        };
        let task = tokio::spawn(async move {
            CodexResponsesApi::new()
                .run(
                    &model,
                    &Context {
                        system_prompt: None,
                        messages: vec![cyrup_core::Message::User {
                            content: vec![cyrup_core::Content::text("hi")],
                            timestamp: 0,
                        }],
                        tools: Vec::new(),
                    },
                    &AuthResult::from_key(&token, "test"),
                    &opts,
                    CancelToken::new(),
                    sink,
                )
                .await;
        });
        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        let _ = task.await;
        held.abort();

        let Some(StreamEvent::Error { error, .. }) = events.last() else {
            panic!("expected an error terminal, got {events:?}");
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("Codex SSE response headers timed out after 200ms")
        );
    }
}

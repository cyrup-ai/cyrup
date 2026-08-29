//! The `pi-messages` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`] speaking pi's **own** message protocol straight to a backend: a single
//! `POST {baseUrl}/messages` carrying `{ model, context, options }`, answered with an SSE stream of
//! *already-serialized assistant-message events* plus a terminal `done`/`error` event. This is the
//! protocol the Radius gateway speaks, but any backend implementing it can be selected with a
//! custom `models.json` provider whose `"api"` is `"pi-messages"` (Pi `api/pi-messages.ts:1-10`).
//!
//! 1:1 port of Pi `packages/ai/src/api/pi-messages.ts` @ **v0.83.0** — the request encoder
//! (`stream`, pi-messages.ts:345-419), the event converter (`createEventConverter`,
//! pi-messages.ts:176-264), the SSE reader (`readPiMessagesEvents` / `parsePiMessagesEvent`,
//! pi-messages.ts:266-311), the response-error formatter + diagnostic
//! (`PiMessagesResponseError` / `createPiMessagesResponseError` / `createErrorEvent`,
//! pi-messages.ts:94-152, 313-335) and the retention resolver (`resolveCacheRetention`,
//! pi-messages.ts:337-343).
//!
//! Unlike every other converter in this directory there is **no vendor shape to translate**: the
//! request body embeds cyrup's own [`Context`] verbatim and the response frames are cyrup's own
//! [`StreamEvent`] payloads. `cyrup_core`'s serde already emits Pi's exact JSON (camelCase
//! `systemPrompt`/`toolCall`/`cacheRead`, `role`-first assistant messages), so the encoder is a
//! `serde_json::to_value` of the context and the decoder reads Pi's literal event tags
//! (`text_start`, `toolcall_delta`, …) back out.
//!
//! ## Mechanism divergences, and why
//!
//! * **Framing.** Pi hand-rolls its SSE reader (`readPiMessagesEvents`): split on `\n\n`, take the
//!   FIRST line starting with `data:`. cyrup reuses [`open_sse`], the crate's shared
//!   `eventsource-stream` decoder, because there is no ambient `fetch`/`ReadableStream` to hand a
//!   bespoke reader. For a one-JSON-object-per-`data:`-line stream — which is what this protocol
//!   is — the two agree frame for frame; a multi-line `data:` field is joined with `\n` by the
//!   spec-compliant decoder where Pi would have taken only the first line.
//! * **`response.body` null check.** Pi throws ``${model.provider} response has no body``
//!   (pi-messages.ts:400-402). `reqwest` has no nullable body, so that branch is unreachable here.
//! * **Sparse content indices.** Pi writes `partial.content[event.contentIndex] = …` into a JS
//!   array, which silently grows with holes. A Rust `Vec` is dense, so [`Decoder::ensure_index`] grows it
//!   with empty text blocks — and caps growth at [`MAX_CONTENT_INDEX`], because an unbounded
//!   attacker-chosen index is an allocation DoS in Rust that it is not in JS.
//! * **Unknown event tags.** Pi's converter `switch` falls through to `{...event, partial}`, so an
//!   unrecognized `type` is forwarded verbatim. [`StreamEvent`] is a closed enum, so an
//!   unrecognized tag is skipped instead.
//! * **`JSON.parse` failure text.** A V8 `SyntaxError` message cannot be reproduced by
//!   `serde_json`; the terminal carries serde's text instead. Every *other* error string in this
//!   module is byte-identical to Pi's.
//! * **Error-body truncation.** Pi truncates the diagnostic `body` at 8192 chars
//!   ([`MAX_DIAGNOSTIC_BODY_CHARS`], pi-messages.ts:116-119). cyrup's shared transport already caps
//!   a non-2xx body before it reaches this module, so the cap here is the second of two.

use crate::HeaderMap;
use crate::api::{ApiImpl, EventSink};
use crate::auth::{AuthResult, ProviderEnv};
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::{SseFrame, SseRequest, build_client_for_target, open_sse};
use crate::stream::{CacheRetention, StreamEvent, StreamOptions};
use crate::utils::provider_plumbing::{now_millis, provider_env_value};
use crate::utils::provider_retry::ProviderRetry;
use cyrup_core::{
    ApiId, AssistantMessage, AssistantMessageDiagnostic, CancelToken, Content, Cost,
    DiagnosticCode, DiagnosticErrorInfo, LazyArgs, SharedStr, StopReason, ToolCall, ToolCallId,
    Usage,
};
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// The wire-protocol id this impl serves (Pi `KnownApi` member `"pi-messages"`, types.ts:26).
///
/// Spelled as a literal rather than a `crate::known_api::*` constant only because adding it to
/// `known_api` means editing `lib.rs`, which is outside this file's change scope; the string is the
/// contract either way.
pub const API_ID: &str = "pi-messages";

/// Pi's `truncateDiagnosticString` bound (pi-messages.ts:116-119).
const MAX_DIAGNOSTIC_BODY_CHARS: usize = 8192;

/// Ceiling on a backend-chosen `contentIndex`. See the module docs: Pi's sparse JS array tolerates
/// any index, a dense `Vec` does not.
const MAX_CONTENT_INDEX: usize = 10_000;

/// Per-API typed options for the `pi-messages` wire protocol (Pi `PiMessagesOptions`,
/// pi-messages.ts:31-36).
///
/// Pi's interface adds `reasoning`, `toolChoice` and `debug` on top of `StreamOptions`. cyrup
/// already carries `reasoning` ([`StreamOptions::reasoning`]) and `toolChoice`
/// ([`StreamOptions::tool_choice`]) on the unified options, exactly as
/// [`AnthropicOptions`](crate::api::anthropic_messages::AnthropicOptions) does, so only `debug`
/// lives here. All fields default to `None`, reproducing Pi's defaults exactly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PiMessagesOptions {
    /// Ask the backend for debug metadata, e.g. routing response headers (Pi
    /// `PiMessagesOptions.debug`, pi-messages.ts:35). `None`/`Some(false)` = Pi default: no
    /// `?debug=1` query parameter.
    pub debug: Option<bool>,
}

/// Impact summary of a server-side message rewrite, e.g. a gateway policy (Pi
/// `PiMessagesRewriteImpact`, pi-messages.ts:41-49). Delivered on the terminal `done`/`error`
/// frame and recorded as a `pi_messages_rewrite` diagnostic on the final message.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiMessagesRewriteImpact {
    #[serde(default)]
    pub policy_id: String,
    #[serde(default)]
    pub policy_version: i64,
    #[serde(default)]
    pub changed: bool,
    #[serde(default)]
    pub token_count_change: i64,
    #[serde(default)]
    pub message_count_change: i64,
    #[serde(default)]
    pub system_prompt_changed: bool,
}

/// The `ApiImpl` for `"pi-messages"`.
pub struct PiMessagesApi {
    api: ApiId,
}

impl Default for PiMessagesApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl PiMessagesApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(PiMessagesApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for PiMessagesApi {
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
        // Pi `if (!apiKey) throw new Error(...)` (pi-messages.ts:355-358). The throw lands in the
        // same catch every other failure does, so the terminal is an EMPTY-content `error` message
        // carrying this exact text.
        let api_key = match auth
            .auth
            .api_key
            .as_deref()
            .or(opts.api_key.as_deref())
            .filter(|k| !k.is_empty())
        {
            Some(k) => k.to_string(),
            None => {
                let msg = format!(
                    "No API key provided for provider \"{}\"",
                    model.provider
                );
                sink.send(error_event(model, &self.api, msg, false, None))
                    .await;
                return;
            }
        };

        let base = auth
            .auth
            .base_url
            .as_deref()
            .unwrap_or(model.base_url.as_str());
        let url = messages_url(base, resolve_debug(opts));

        // gap-08 #2: `before_provider_request` may inspect/replace the outbound body (Pi
        // `options?.onPayload?.(payload, model)`, pi-messages.ts:377-380).
        let body = crate::stream::apply_on_payload(opts, model, build_payload(model, ctx, opts))
            .await;
        let req = SseRequest {
            method: reqwest::Method::POST,
            url: url.clone(),
            headers: build_headers(opts, &api_key),
            body: Some(body),
        };

        // Honor HTTP(S)_PROXY for the live client (Pi resolveHttpProxyUrlForTarget,
        // node-http-proxy.ts:92-112). PROV-006: `StreamOptions.timeout_ms` overrides the
        // process-global idle timeout.
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
                sink.send(transport_error_event(model, &self.api, &e)).await;
                return;
            }
        };

        // gap-08 #3: capture {status, headers} at connect, then fire `after_provider_response`
        // (Pi `await options?.onResponse?.(…)`, pi-messages.ts:394).
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
            Err(ProviderError::Http { status, message }) => {
                // Pi `if (!response.ok) throw createPiMessagesResponseError(...)`
                // (pi-messages.ts:396-399): the terminal carries the formatted status line AND a
                // redacted `pi_messages_response_failure` diagnostic.
                capture.fire(opts, model).await;
                let (text, diagnostic) =
                    response_error(model, &url, status, &message);
                sink.send(error_event(model, &self.api, text, false, Some(diagnostic)))
                    .await;
                return;
            }
            Err(e) => {
                capture.fire(opts, model).await;
                sink.send(transport_error_event(model, &self.api, &e)).await;
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

/// Normalize a base URL to the `/messages` endpoint, appending `?debug=1` when requested (Pi
/// ``new URL(`${model.baseUrl.replace(/\/+$/u, "")}/messages`)`` + `url.searchParams.set("debug",
/// "1")`, pi-messages.ts:360-363).
///
/// Note the endpoint is a bare `/messages` — NOT `/v1/messages`; the version prefix, if any, is
/// part of the configured `baseUrl`.
pub fn messages_url(base: &str, debug: bool) -> String {
    let trimmed = base.trim_end_matches('/');
    let path = format!("{trimmed}/messages");
    if debug { format!("{path}?debug=1") } else { path }
}

/// The `debug` flag for this request.
///
/// Pi reads `options.debug` off the per-API `PiMessagesOptions` (pi-messages.ts:361). cyrup carries
/// per-API options through [`StreamOptions::api_options`], whose `ApiStreamOptions` enum lives in
/// `stream.rs` and has no `PiMessages` variant yet — adding one is part of registering this API
/// (see [`PiMessagesOptions`]). Until then the flag resolves to Pi's default, `false`, and
/// [`messages_url`] takes it as a parameter so the encoder itself is complete and testable.
fn resolve_debug(opts: &StreamOptions) -> bool {
    let _ = opts;
    false
}

/// Build the request headers (Pi's inline `headers` object, pi-messages.ts:383-389).
///
/// Exactly four sources, in Pi's order: `authorization`, `accept`, `content-type`, then the
/// caller's `options.headers` overlay spread last (`providerHeadersToRecord`, headers.ts:12-19 — a
/// `null` value drops the header, which cyrup's `Option<String>`-valued [`HeaderMap`] expresses
/// natively and the transport honors as suppression).
///
/// Pi does NOT merge `model.headers` here (contrast `anthropic-messages.ts:createClient`, which
/// does), so neither does this port.
pub(crate) fn build_headers(opts: &StreamOptions, api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization".to_string(),
        Some(format!("Bearer {api_key}")),
    );
    headers.insert(
        "accept".to_string(),
        Some("text/event-stream".to_string()),
    );
    headers.insert(
        "content-type".to_string(),
        Some("application/json".to_string()),
    );
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    headers
}

/// 1:1 port of Pi's `resolveCacheRetention` (pi-messages.ts:337-343).
///
/// **Deliberately unlike every other converter in this directory**: an unset retention stays unset
/// (`None`) so the *backend's* default applies — only the legacy `PI_CACHE_RETENTION=long` opt-in
/// is mapped. `anthropic-messages`/`openai-completions` default to `"short"` instead; copying them
/// here would silently override a gateway's own policy.
pub(crate) fn resolve_cache_retention(
    cache_retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> Option<CacheRetention> {
    if let Some(c) = cache_retention {
        return Some(c);
    }
    if provider_env_value("PI_CACHE_RETENTION", env).as_deref() == Some("long") {
        return Some(CacheRetention::Long);
    }
    None
}

/// The wire string for a [`CacheRetention`] (Pi `CacheRetention = "none" | "short" | "long"`,
/// types.ts:101).
fn cache_retention_wire(retention: CacheRetention) -> &'static str {
    match retention {
        CacheRetention::None => "none",
        CacheRetention::Short => "short",
        CacheRetention::Long => "long",
    }
}

/// Build the request body: `{ model, context, options }` (Pi's `payload`, pi-messages.ts:365-376).
///
/// `context` is cyrup's [`Context`] serialized as-is — its serde already emits Pi's `Context`
/// (`systemPrompt?`, `messages`, `tools`). The nested `options` object carries only Pi's six keys;
/// a key whose value is `undefined` in Pi is OMITTED here rather than emitted as `null`, matching
/// `JSON.stringify`, which drops `undefined` properties.
pub(crate) fn build_payload(model: &Model, ctx: &Context, opts: &StreamOptions) -> Value {
    let mut options = Map::new();
    if let Some(t) = opts.temperature {
        options.insert("temperature".to_string(), json!(t));
    }
    if let Some(m) = opts.max_tokens {
        options.insert("maxTokens".to_string(), json!(m));
    }
    // Pi `reasoning: options?.reasoning` — a `ThinkingLevel`, which has no `"off"` member; cyrup's
    // unified `ModelThinkingLevel::Off` is the same absence and so emits nothing (types.ts:74-75).
    if let Some(level) = opts.reasoning.level() {
        options.insert("reasoning".to_string(), json!(level));
    }
    if let Some(r) = resolve_cache_retention(opts.cache_retention, opts.env.as_ref()) {
        options.insert("cacheRetention".to_string(), json!(cache_retention_wire(r)));
    }
    if let Some(sid) = &opts.session_id {
        options.insert("sessionId".to_string(), json!(sid.to_string()));
    }
    if let Some(tc) = &opts.tool_choice {
        options.insert("toolChoice".to_string(), tc.to_wire());
    }

    json!({
        "model": model.id.as_str(),
        "context": serde_json::to_value(ctx).unwrap_or(Value::Null),
        "options": Value::Object(options),
    })
}

// ---------------------------------------------------------------------------
// Response errors (Pi PiMessagesResponseError, pi-messages.ts:94-152)
// ---------------------------------------------------------------------------

/// Pi `truncateDiagnosticString` (pi-messages.ts:116-119). Counts `char`s where Pi counts UTF-16
/// code units — the bound is a redaction guard, not a wire contract.
fn truncate_diagnostic_string(value: &str) -> String {
    if value.chars().count() > MAX_DIAGNOSTIC_BODY_CHARS {
        let head: String = value.chars().take(MAX_DIAGNOSTIC_BODY_CHARS).collect();
        format!("{head}…")
    } else {
        value.to_string()
    }
}

/// Pi `parsePiMessagesErrorBody` (pi-messages.ts:106-114): the body parses to an object with a
/// non-array object `error` property, or nothing.
fn parse_error_body(body: &str) -> Option<Value> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    let error = parsed.get("error")?;
    if error.is_object() { Some(error.clone()) } else { None }
}

/// The HTTP `statusText` for a status code.
///
/// Pi reads `response.statusText` off `fetch`. `reqwest` discards the reason phrase, so this
/// reconstructs the canonical one — which is what a spec-compliant server sends and what `fetch`
/// therefore reports. An unregistered status yields `""`, the same value `fetch` reports for a
/// missing reason phrase.
fn status_text(status: u16) -> &'static str {
    reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("")
}

/// Pi `formatPiMessagesResponseError` (pi-messages.ts:121-131) +
/// `createPiMessagesResponseError` (pi-messages.ts:133-152), fused: returns the `Error.message`
/// text and the `pi_messages_response_failure` diagnostic built from the same parse.
fn response_error(
    model: &Model,
    url: &str,
    status: u16,
    body: &str,
) -> (String, AssistantMessageDiagnostic) {
    let error_body = parse_error_body(body);
    let message = error_body
        .as_ref()
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str);
    let code = error_body
        .as_ref()
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str);
    let status_text = status_text(status);
    // `${response.status} ${response.statusText}: ${message ?? body}${code ? ` (${code})` : ""}`.
    let suffix = message.unwrap_or(body);
    let code_suffix = code.map(|c| format!(" ({c})")).unwrap_or_default();
    let text = format!("{status} {status_text}: {suffix}{code_suffix}");

    let mut details = Map::new();
    details.insert("version".to_string(), json!(1));
    details.insert("provider".to_string(), json!(model.provider.to_string()));
    details.insert("model".to_string(), json!(model.id.as_str()));
    details.insert("url".to_string(), json!(url));
    details.insert("status".to_string(), json!(status));
    details.insert("statusText".to_string(), json!(status_text));
    match &error_body {
        // Pi emits `error: errorBody?.error` and `body: undefined` — `JSON.stringify` drops the
        // `undefined` one, so exactly one of the two keys is ever present.
        Some(e) => {
            details.insert("error".to_string(), e.clone());
        }
        None => {
            details.insert(
                "body".to_string(),
                json!(truncate_diagnostic_string(body)),
            );
        }
    }
    details.insert("timestampMs".to_string(), json!(now_millis()));

    let mut info = DiagnosticErrorInfo::from_message(text.clone())
        .with_name("PiMessagesResponseError");
    if let Some(c) = code {
        info = info.with_code(DiagnosticCode::Str(c.to_string()));
    }
    let diagnostic = cyrup_core::create_assistant_message_diagnostic_from(
        "pi_messages_response_failure",
        Some(info),
        Some(Value::Object(details)),
    );
    (text, diagnostic)
}

/// Pi `createErrorEvent` (pi-messages.ts:313-335): the terminal for a *thrown* failure.
///
/// Note the content is EMPTY — Pi builds a fresh `AssistantMessage`, discarding whatever the
/// converter had accumulated. That is deliberate upstream behaviour and is reproduced here; only
/// the `done`/`error` frames delivered BY the backend carry the assembled `partial`.
fn error_event(
    model: &Model,
    api: &ApiId,
    message: String,
    aborted: bool,
    diagnostic: Option<AssistantMessageDiagnostic>,
) -> StreamEvent {
    let mut msg = AssistantMessage {
        content: Vec::new(),
        provider: model.provider.clone(),
        model: model.id.as_str().to_string(),
        api: api.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: if aborted {
            StopReason::Aborted
        } else {
            StopReason::Error
        },
        deferred: None,
        error_message: Some(message),
        raw_stop_reason: None,
        timestamp: now_millis(),
    };
    // Pi attaches the diagnostic only on the non-aborted `PiMessagesResponseError` path
    // (pi-messages.ts:327-332).
    if let Some(d) = diagnostic
        && !aborted
    {
        msg.append_diagnostic(d);
    }
    StreamEvent::terminal(msg)
}

/// A transport/decode/abort failure, routed through Pi's `createErrorEvent` shape.
///
/// `ProviderError::Aborted` is Pi's `options?.signal?.aborted` branch, which yields
/// `reason: "aborted"` (pi-messages.ts:313-314, 414).
fn transport_error_event(model: &Model, api: &ApiId, e: &ProviderError) -> StreamEvent {
    error_event(model, api, e.to_string(), e.is_aborted(), None)
}

// ---------------------------------------------------------------------------
// Response decoding (Pi createEventConverter, pi-messages.ts:176-264)
// ---------------------------------------------------------------------------

/// Streaming-decode state — Pi's `partial` closure variable plus its `toolJson` map
/// (pi-messages.ts:177-187).
struct Decoder {
    content: Vec<Content>,
    /// Accumulated tool-call JSON per content index (Pi `toolJson: Map<number, string>`).
    ///
    /// Shared with every snapshot taken from it, so attaching the arguments to a block is a
    /// refcount bump and the `Map` is recovered only if something reads it (PERF-001).
    tool_json: HashMap<usize, SharedStr>,
    usage: Usage,
    response_id: Option<String>,
    diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
    stop_reason: StopReason,
    error_message: Option<String>,
}

impl Decoder {
    fn new() -> Self {
        Self {
            content: Vec::new(),
            tool_json: HashMap::new(),
            usage: Usage::default(),
            response_id: None,
            diagnostics: None,
            // Pi seeds `stopReason: "pending"` (pi-messages.ts:184).
            stop_reason: StopReason::Pending,
            error_message: None,
        }
    }

    /// The live `partial`, as a SHARED handle (PERF-001).
    ///
    /// Every non-terminal event carries this message and it is then cloned again by the
    /// agent loop, by `MessageUpdate`, and once per live subscriber. Handing out an `Arc`
    /// turns those into refcount bumps; the wire bytes are unchanged because serde's `rc`
    /// feature serializes an `Arc<T>` transparently as `T`.
    fn snapshot(&self, model: &Model, api: &ApiId) -> Arc<AssistantMessage> {
        Arc::new(self.snapshot_owned(model, api))
    }

    /// The same message, owned, for the terminal paths that stamp a stop reason onto it
    /// before handing it to [`StreamEvent::terminal`]/[`StreamEvent::end_of_stream`].
    ///
    /// `usage` is NOT cost-adjusted here: a pi-messages backend reports its own `usage.cost`
    /// on the terminal frame (`PiMessagesUsage = AssistantMessage["usage"]`, pi-messages.ts:38),
    /// and Pi assigns it wholesale rather than recomputing from `model.cost`.
    fn snapshot_owned(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        AssistantMessage {
            content: self.content.clone(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: self.response_id.clone(),
            diagnostics: self.diagnostics.clone(),
            usage: self.usage.clone(),
            stop_reason: self.stop_reason,
            deferred: None,
            error_message: self.error_message.clone(),
            raw_stop_reason: None,
            timestamp: now_millis(),
        }
    }

    /// Grow `content` so `index` is addressable. See the module docs for why this exists and why it
    /// is bounded. Returns `false` when the index is beyond [`MAX_CONTENT_INDEX`].
    fn ensure_index(&mut self, index: usize) -> bool {
        if index > MAX_CONTENT_INDEX {
            return false;
        }
        while self.content.len() <= index {
            self.content.push(Content::text(""));
        }
        true
    }

    /// Pi `appendRewriteDiagnostic` (pi-messages.ts:165-174): a `pi_messages_rewrite` record
    /// carrying `{...rewrite}` as its details. Spread verbatim, exactly as Pi does, so a backend
    /// that adds a field to the impact summary is not silently dropped.
    fn append_rewrite(&mut self, rewrite: Option<&Value>) {
        let Some(rewrite) = rewrite.filter(|v| v.is_object()) else {
            return;
        };
        let d = cyrup_core::create_assistant_message_diagnostic_from(
            "pi_messages_rewrite",
            None,
            Some(rewrite.clone()),
        );
        cyrup_core::append_assistant_message_diagnostic(&mut self.diagnostics, d);
    }
}

/// Pi `Usage` read tolerantly. Pi assigns `event.usage` wholesale; cyrup's [`Usage`] is a typed
/// struct, so a backend that omits `cost` (or any counter) contributes zero for it instead of
/// failing the whole frame.
fn parse_usage(raw: &Value) -> Usage {
    let n = |k: &str| raw.get(k).and_then(Value::as_u64);
    let cost = raw.get("cost");
    let c = |k: &str| {
        cost.and_then(|c| c.get(k))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    Usage {
        input: n("input").unwrap_or(0),
        output: n("output").unwrap_or(0),
        cache_read: n("cacheRead").unwrap_or(0),
        cache_write: n("cacheWrite").unwrap_or(0),
        cache_write_1h: n("cacheWrite1h"),
        reasoning: n("reasoning"),
        total_tokens: n("totalTokens").unwrap_or(0),
        cost: Cost {
            input: c("input"),
            output: c("output"),
            cache_read: c("cacheRead"),
            cache_write: c("cacheWrite"),
            total: c("total"),
        },
    }
}

/// Pi's `done.reason` narrowing: `Extract<StopReason, "stop" | "length" | "toolUse">`
/// (pi-messages.ts:71). An out-of-set value is not a reason Pi's types admit; it degrades to
/// `stop`, matching the `done` terminal the backend asked for.
fn done_reason(raw: Option<&str>) -> StopReason {
    match raw {
        Some("length") => StopReason::Length,
        Some("toolUse") => StopReason::ToolUse,
        _ => StopReason::Stop,
    }
}

/// Pi's `error.reason` narrowing: `Extract<StopReason, "aborted" | "error">` (pi-messages.ts:78).
fn error_reason(raw: Option<&str>) -> StopReason {
    match raw {
        Some("aborted") => StopReason::Aborted,
        _ => StopReason::Error,
    }
}

/// Drive the pi-messages SSE frame stream into ordered [`StreamEvent`]s (Pi's `for await` loop over
/// `readPiMessagesEvents` + `convertEvent`, pi-messages.ts:404-412).
pub(crate) async fn decode_stream<S>(mut frames: S, model: &Model, api: &ApiId, sink: &EventSink)
where
    S: Stream<Item = Result<SseFrame, ProviderError>> + Unpin,
{
    let mut dec = Decoder::new();

    while let Some(frame) = frames.next().await {
        let frame = match frame {
            Ok(f) => f,
            Err(e) => {
                sink.send(transport_error_event(model, api, &e)).await;
                return;
            }
        };
        // Pi `parsePiMessagesEvent`: the frame's `data` payload, skipping the `[DONE]` sentinel
        // (pi-messages.ts:303-311). The `event:` name is never consulted.
        let data = frame.data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let event: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                // Pi's `JSON.parse` throws into the same catch every other failure uses.
                sink.send(error_event(model, api, e.to_string(), false, None))
                    .await;
                return;
            }
        };
        match process_event(&event, &mut dec, model, api, sink).await {
            Flow::Continue => {}
            Flow::Stop => return,
        }
    }

    // Pi: falling out of the loop means the backend never sent `done`/`error`
    // (pi-messages.ts:412). The throw becomes an empty-content `error` terminal.
    sink.send(error_event(
        model,
        api,
        format!("{} stream ended without a terminal event", model.provider),
        false,
        None,
    ))
    .await;
}

/// Whether the decode loop should keep reading.
enum Flow {
    Continue,
    Stop,
}

/// Convert and emit one decoded pi-messages event (Pi's converter `switch`, pi-messages.ts:189-263).
async fn process_event(
    event: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> Flow {
    let ty = event.get("type").and_then(Value::as_str).unwrap_or("");
    // `contentIndex` is absent on `start`/`done`/`error`; those arms never read it.
    let index = event
        .get("contentIndex")
        .and_then(Value::as_u64)
        .and_then(|i| usize::try_from(i).ok())
        .unwrap_or(0);
    let delta = event
        .get("delta")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // Terminals first — they are the only arms that mutate `stopReason`/`usage`.
    match ty {
        "done" => {
            dec.stop_reason = done_reason(event.get("reason").and_then(Value::as_str));
            if let Some(u) = event.get("usage") {
                dec.usage = parse_usage(u);
            }
            dec.response_id = event
                .get("responseId")
                .and_then(Value::as_str)
                .map(str::to_string);
            dec.append_rewrite(event.get("rewrite"));
            sink.send(StreamEvent::terminal(dec.snapshot_owned(model, api)))
                .await;
            return Flow::Stop;
        }
        "error" => {
            dec.stop_reason = error_reason(event.get("reason").and_then(Value::as_str));
            if let Some(u) = event.get("usage") {
                dec.usage = parse_usage(u);
            }
            dec.error_message = event
                .get("errorMessage")
                .and_then(Value::as_str)
                .map(str::to_string);
            dec.response_id = event
                .get("responseId")
                .and_then(Value::as_str)
                .map(str::to_string);
            dec.append_rewrite(event.get("rewrite"));
            sink.send(StreamEvent::terminal(dec.snapshot_owned(model, api)))
                .await;
            return Flow::Stop;
        }
        _ => {}
    }

    // Every non-terminal arm addresses `content[contentIndex]`; refuse an index a dense `Vec`
    // cannot represent (see the module docs).
    if matches!(
        ty,
        "text_start"
            | "text_delta"
            | "text_end"
            | "thinking_start"
            | "thinking_delta"
            | "thinking_end"
            | "toolcall_start"
            | "toolcall_delta"
            | "toolcall_end"
    ) && !dec.ensure_index(index)
    {
        sink.send(error_event(
            model,
            api,
            format!("pi-messages contentIndex {index} out of range"),
            false,
            None,
        ))
        .await;
        return Flow::Stop;
    }

    let sent = match ty {
        "start" => {
            sink.send(StreamEvent::Start {
                partial: dec.snapshot(model, api),
            })
            .await
        }
        "text_start" => {
            if let Some(slot) = dec.content.get_mut(index) {
                *slot = Content::text("");
            }
            sink.send(StreamEvent::TextStart {
                content_index: index,
                partial: dec.snapshot(model, api),
            })
            .await
        }
        "text_delta" => {
            if let Some(Content::Text { text, .. }) = dec.content.get_mut(index) {
                text.push_str(&delta);
            }
            sink.send(StreamEvent::TextDelta {
                content_index: index,
                delta,
                partial: dec.snapshot(model, api),
            })
            .await
        }
        "text_end" => {
            let content = event
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let signature = event
                .get("contentSignature")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(slot) = dec.content.get_mut(index) {
                *slot = Content::Text {
                    text: SharedStr::from(&content),
                    text_signature: signature,
                };
            }
            sink.send(StreamEvent::TextEnd {
                content_index: index,
                content,
                partial: dec.snapshot(model, api),
            })
            .await
        }
        "thinking_start" => {
            if let Some(slot) = dec.content.get_mut(index) {
                *slot = Content::thinking("");
            }
            sink.send(StreamEvent::ThinkingStart {
                content_index: index,
                partial: dec.snapshot(model, api),
            })
            .await
        }
        "thinking_delta" => {
            if let Some(Content::Thinking { thinking, .. }) = dec.content.get_mut(index) {
                thinking.push_str(&delta);
            }
            sink.send(StreamEvent::ThinkingDelta {
                content_index: index,
                delta,
                partial: dec.snapshot(model, api),
            })
            .await
        }
        "thinking_end" => {
            let content = event
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let signature = event
                .get("contentSignature")
                .and_then(Value::as_str)
                .map(str::to_string);
            let redacted = event
                .get("redacted")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Some(slot) = dec.content.get_mut(index) {
                *slot = Content::Thinking {
                    thinking: SharedStr::from(&content),
                    thinking_signature: signature,
                    redacted,
                };
            }
            sink.send(StreamEvent::ThinkingEnd {
                content_index: index,
                content,
                partial: dec.snapshot(model, api),
            })
            .await
        }
        "toolcall_start" => {
            let id = event.get("id").and_then(Value::as_str).unwrap_or("");
            let name = event
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(slot) = dec.content.get_mut(index) {
                *slot = Content::ToolCall(ToolCall {
                    id: ToolCallId::from(id),
                    name,
                    arguments: Map::new().into(),
                    thought_signature: None,
                });
            }
            dec.tool_json.insert(index, SharedStr::new());
            sink.send(StreamEvent::ToolCallStart {
                content_index: index,
                partial: dec.snapshot(model, api),
            })
            .await
        }
        "toolcall_delta" => {
            // Pi: `${toolJson.get(i) ?? ""}${event.delta}` re-parsed with `parseStreamingJson`
            // on EVERY delta, so `partial` always shows the best-effort arguments so far.
            //
            // PERF-001: the recovered value is identical, only the cost differs. The delta is
            // appended to a buffer every snapshot shares, and the block is handed a HANDLE on it
            // rather than a parse of it, so the whole-buffer parse Pi runs per delta happens only
            // if something actually reads the arguments.
            let acc = dec.tool_json.entry(index).or_default();
            acc.push_str(&delta);
            let arguments = LazyArgs::streaming(acc.clone());
            if let Some(Content::ToolCall(tc)) = dec.content.get_mut(index) {
                tc.arguments = arguments;
            }
            sink.send(StreamEvent::ToolCallDelta {
                content_index: index,
                delta,
                partial: dec.snapshot(model, api),
            })
            .await
        }
        "toolcall_end" => {
            // Pi `Object.assign(partial.content[i], event.toolCall)` — a MERGE, not a replace, so
            // a field the backend omits keeps whatever the deltas built.
            if let Some(Content::ToolCall(tc)) = dec.content.get_mut(index) {
                merge_tool_call(tc, event.get("toolCall"));
            }
            dec.tool_json.remove(&index);
            let Some(Content::ToolCall(tool_call)) = dec.content.get(index).cloned() else {
                // The slot is not a tool call (a malformed stream): Pi would emit a `toolcall_end`
                // carrying whatever was there. There is no representable event, so skip it.
                return Flow::Continue;
            };
            sink.send(StreamEvent::ToolCallEnd {
                content_index: index,
                tool_call,
                partial: dec.snapshot(model, api),
            })
            .await
        }
        // Unknown tag: see the module docs — Pi forwards it, a closed enum cannot.
        _ => true,
    };

    if sent { Flow::Continue } else { Flow::Stop }
}

/// `Object.assign(existing, toolCall)` for cyrup's typed [`ToolCall`] (pi-messages.ts:252).
fn merge_tool_call(tc: &mut ToolCall, raw: Option<&Value>) {
    let Some(raw) = raw else { return };
    if let Some(id) = raw.get("id").and_then(Value::as_str) {
        tc.id = ToolCallId::from(id);
    }
    if let Some(name) = raw.get("name").and_then(Value::as_str) {
        tc.name = name.to_string();
    }
    if let Some(Value::Object(args)) = raw.get("arguments") {
        tc.arguments = args.clone().into();
    }
    if let Some(sig) = raw.get("thoughtSignature").and_then(Value::as_str) {
        tc.thought_signature = Some(sig.to_string());
    }
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
    use crate::auth::types::ModelAuth;
    use crate::model::{Modality, ModelCost};
    use crate::stream::sse::decode_sse_bytes;
    use crate::stream::{DoneReason, ErrorReason, ToolChoice};
    use cyrup_core::{Message, ModelThinkingLevel};

    fn model() -> Model {
        Model {
            id: "radius-1".into(),
            name: "Radius 1".into(),
            api: API_ID.into(),
            provider: "radius".into(),
            base_url: "https://gateway.example.com/v1".to_string(),
            reasoning: true,
            input: vec![Modality::Text],
            cost: ModelCost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.1,
                cache_write: 0.2,
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

    fn auth_with(api_key: Option<&str>) -> AuthResult {
        AuthResult {
            auth: ModelAuth {
                api_key: api_key.map(String::from),
                ..Default::default()
            },
            // An EMPTY provider env is what keeps `resolve_cache_retention` — and, in the loopback
            // tests, proxy resolution — independent of the developer's shell.
            env: Some(ProviderEnv::new()),
            source: None,
        }
    }

    // ------------------------------------------------------------------ encoder --

    /// Pi ``new URL(`${model.baseUrl.replace(/\/+$/u, "")}/messages`)`` — a BARE `/messages`, and
    /// the trailing-slash strip is `/+$`, i.e. it eats a run of slashes.
    #[test]
    fn url_is_bare_messages_and_strips_trailing_slashes() {
        assert_eq!(
            messages_url("https://gateway.example.com/v1", false),
            "https://gateway.example.com/v1/messages"
        );
        assert_eq!(
            messages_url("https://gateway.example.com/v1///", false),
            "https://gateway.example.com/v1/messages"
        );
        // NOT `/v1/messages` appended to a bare host — the version prefix is the caller's.
        assert_eq!(
            messages_url("https://gateway.example.com", false),
            "https://gateway.example.com/messages"
        );
    }

    /// Pi `url.searchParams.set("debug", "1")` (pi-messages.ts:361-363).
    #[test]
    fn debug_flag_adds_query_param() {
        assert_eq!(
            messages_url("https://gateway.example.com/v1", true),
            "https://gateway.example.com/v1/messages?debug=1"
        );
    }

    /// Pi's payload is exactly `{ model, context, options }` (pi-messages.ts:365-376), with the
    /// context embedded verbatim in Pi's own `Context` JSON.
    #[test]
    fn payload_shape_matches_upstream() {
        let m = model();
        let opts = StreamOptions {
            temperature: Some(0.5),
            max_tokens: Some(1024),
            reasoning: ModelThinkingLevel::High,
            tool_choice: Some(ToolChoice::Required),
            ..Default::default()
        };
        let body = build_payload(&m, &user_ctx("hi"), &opts);

        // Exactly three top-level keys.
        let obj = body.as_object().unwrap();
        assert_eq!(obj.len(), 3);
        assert_eq!(body["model"], "radius-1");

        // `context` is Pi's Context shape: camelCase `systemPrompt`, `messages`, `tools`.
        assert_eq!(body["context"]["systemPrompt"], "be brief");
        assert_eq!(body["context"]["messages"][0]["role"], "user");
        assert_eq!(body["context"]["messages"][0]["content"][0]["text"], "hi");

        // `options` carries Pi's camelCase keys.
        assert_eq!(body["options"]["temperature"], 0.5);
        assert_eq!(body["options"]["maxTokens"], 1024);
        assert_eq!(body["options"]["reasoning"], "high");
        assert_eq!(body["options"]["toolChoice"], json!("required"));
    }

    /// MIRROR: the defaults path. `undefined` properties are dropped by `JSON.stringify`, so an
    /// options object with nothing set must serialize to `{}` — not to a bag of nulls.
    #[test]
    fn payload_omits_unset_options() {
        let m = model();
        let body = build_payload(&m, &user_ctx("hi"), &StreamOptions::default());
        assert_eq!(body["options"], json!({}));
    }

    /// Pi's `resolveCacheRetention` for pi-messages returns **undefined** when unset — the backend
    /// default applies. This is NOT `anthropic-messages`' `"short"` default; a regression to that
    /// would show up here.
    #[test]
    fn cache_retention_stays_unset_unless_asked() {
        let env = ProviderEnv::new();
        assert_eq!(resolve_cache_retention(None, Some(&env)), None);
        assert_eq!(
            resolve_cache_retention(Some(CacheRetention::Short), Some(&env)),
            Some(CacheRetention::Short)
        );
        // `PI_CACHE_RETENTION=long` is the one legacy opt-in Pi maps.
        let mut long = ProviderEnv::new();
        long.insert("PI_CACHE_RETENTION".to_string(), "long".to_string());
        assert_eq!(
            resolve_cache_retention(None, Some(&long)),
            Some(CacheRetention::Long)
        );
        // Any other value is ignored (Pi tests `=== "long"`).
        let mut other = ProviderEnv::new();
        other.insert("PI_CACHE_RETENTION".to_string(), "short".to_string());
        assert_eq!(resolve_cache_retention(None, Some(&other)), None);
    }

    /// Pi's header object, in order: authorization / accept / content-type, then the caller's
    /// overlay. A `null` overlay value suppresses (`providerHeadersToRecord` drops nulls; cyrup's
    /// transport treats `None` as suppression).
    #[test]
    fn headers_match_upstream() {
        let opts = StreamOptions::default();
        let h = build_headers(&opts, "sk-test");
        assert_eq!(h.get("authorization"), Some(&Some("Bearer sk-test".into())));
        assert_eq!(h.get("accept"), Some(&Some("text/event-stream".into())));
        assert_eq!(
            h.get("content-type"),
            Some(&Some("application/json".into()))
        );

        let mut overlay = HeaderMap::new();
        overlay.insert("x-trace".to_string(), Some("abc".to_string()));
        overlay.insert("accept".to_string(), None);
        let opts = StreamOptions {
            headers: Some(overlay),
            ..Default::default()
        };
        let h = build_headers(&opts, "sk-test");
        assert_eq!(h.get("x-trace"), Some(&Some("abc".into())));
        assert_eq!(h.get("accept"), Some(&None));
    }

    // ------------------------------------------------------------------ decoder --

    async fn collect(raw: &str, m: &Model) -> Vec<StreamEvent> {
        let (sink, mut rx) = channel(64);
        let api = ApiId::from(API_ID);
        let frames = decode_sse_bytes(raw.as_bytes().to_vec());
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

    /// A full upstream-shaped transcript: text block, thinking block, tool call, terminal `done`
    /// with usage + rewrite impact.
    #[tokio::test]
    async fn decodes_a_full_pi_messages_transcript() {
        let raw = concat!(
            "data: {\"type\":\"start\"}\n\n",
            "data: {\"type\":\"thinking_start\",\"contentIndex\":0}\n\n",
            "data: {\"type\":\"thinking_delta\",\"contentIndex\":0,\"delta\":\"hmm\"}\n\n",
            "data: {\"type\":\"thinking_end\",\"contentIndex\":0,\"content\":\"hmm\",\"contentSignature\":\"sig-1\"}\n\n",
            "data: {\"type\":\"text_start\",\"contentIndex\":1}\n\n",
            "data: {\"type\":\"text_delta\",\"contentIndex\":1,\"delta\":\"Hel\"}\n\n",
            "data: {\"type\":\"text_delta\",\"contentIndex\":1,\"delta\":\"lo\"}\n\n",
            "data: {\"type\":\"text_end\",\"contentIndex\":1,\"content\":\"Hello\"}\n\n",
            "data: {\"type\":\"toolcall_start\",\"contentIndex\":2,\"id\":\"call_1\",\"toolName\":\"read\"}\n\n",
            "data: {\"type\":\"toolcall_delta\",\"contentIndex\":2,\"delta\":\"{\\\"path\\\":\"}\n\n",
            "data: {\"type\":\"toolcall_delta\",\"contentIndex\":2,\"delta\":\"\\\"a\\\"}\"}\n\n",
            "data: {\"type\":\"toolcall_end\",\"contentIndex\":2,\"toolCall\":{\"type\":\"toolCall\",\"id\":\"call_1\",\"name\":\"read\",\"arguments\":{\"path\":\"a\"}}}\n\n",
            "data: {\"type\":\"done\",\"reason\":\"toolUse\",\"responseId\":\"resp_7\",",
            "\"usage\":{\"input\":10,\"output\":5,\"cacheRead\":1,\"cacheWrite\":2,\"totalTokens\":15,",
            "\"cost\":{\"input\":0.01,\"output\":0.02,\"cacheRead\":0.0,\"cacheWrite\":0.0,\"total\":0.03}},",
            "\"rewrite\":{\"policyId\":\"p1\",\"policyVersion\":3,\"changed\":true,\"tokenCountChange\":-4,",
            "\"messageCountChange\":0,\"systemPromptChanged\":false}}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect(raw, &model()).await;

        // Pi's tag order, 1:1.
        let tags: Vec<&str> = events
            .iter()
            .map(|e| match e {
                StreamEvent::Start { .. } => "start",
                StreamEvent::TextStart { .. } => "text_start",
                StreamEvent::TextDelta { .. } => "text_delta",
                StreamEvent::TextEnd { .. } => "text_end",
                StreamEvent::ThinkingStart { .. } => "thinking_start",
                StreamEvent::ThinkingDelta { .. } => "thinking_delta",
                StreamEvent::ThinkingEnd { .. } => "thinking_end",
                StreamEvent::ToolCallStart { .. } => "toolcall_start",
                StreamEvent::ToolCallDelta { .. } => "toolcall_delta",
                StreamEvent::ToolCallEnd { .. } => "toolcall_end",
                StreamEvent::Done { .. } => "done",
                StreamEvent::Error { .. } => "error",
            })
            .collect();
        assert_eq!(
            tags,
            vec![
                "start",
                "thinking_start",
                "thinking_delta",
                "thinking_end",
                "text_start",
                "text_delta",
                "text_delta",
                "text_end",
                "toolcall_start",
                "toolcall_delta",
                "toolcall_delta",
                "toolcall_end",
                "done",
            ]
        );

        let StreamEvent::Done { reason, message } = events.last().unwrap() else {
            panic!("expected done terminal");
        };
        assert_eq!(*reason, DoneReason::ToolUse);
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert_eq!(message.response_id.as_deref(), Some("resp_7"));

        // Usage is taken WHOLESALE from the backend, cost included — not recomputed from
        // `model.cost` (which would give input 10 * 1.0/1e6 = 0.00001, not 0.01).
        assert_eq!(message.usage.input, 10);
        assert_eq!(message.usage.total_tokens, 15);
        assert!((message.usage.cost.total - 0.03).abs() < f64::EPSILON);

        // Content, in index order, with the signatures the backend supplied.
        assert_eq!(message.content.len(), 3);
        assert_eq!(
            message.content[0],
            Content::Thinking {
                thinking: "hmm".into(),
                thinking_signature: Some("sig-1".into()),
                redacted: false,
            }
        );
        assert_eq!(message.content[1], Content::text("Hello"));
        let Content::ToolCall(tc) = &message.content[2] else {
            panic!("expected tool call");
        };
        assert_eq!(tc.id.as_str(), "call_1");
        assert_eq!(tc.name, "read");
        assert_eq!(tc.arguments.get("path"), Some(&json!("a")));

        // `appendRewriteDiagnostic` (pi-messages.ts:165-174).
        let diags = message.diagnostics.as_ref().expect("rewrite diagnostic");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].r#type, "pi_messages_rewrite");
        assert_eq!(diags[0].details.as_ref().unwrap()["policyId"], "p1");
        assert_eq!(diags[0].details.as_ref().unwrap()["tokenCountChange"], -4);
    }

    /// A backend `toolcall_delta` stream is re-parsed on EVERY delta, so `partial` shows the
    /// best-effort arguments before the JSON is complete (Pi `parseStreamingJson`).
    #[tokio::test]
    async fn toolcall_partial_arguments_parse_incrementally() {
        let raw = concat!(
            "data: {\"type\":\"start\"}\n\n",
            "data: {\"type\":\"toolcall_start\",\"contentIndex\":0,\"id\":\"c\",\"toolName\":\"t\"}\n\n",
            "data: {\"type\":\"toolcall_delta\",\"contentIndex\":0,\"delta\":\"{\\\"a\\\":1,\\\"b\\\":\"}\n\n",
            "data: {\"type\":\"done\",\"reason\":\"stop\",\"usage\":{}}\n\n",
        );
        let events = collect(raw, &model()).await;
        let StreamEvent::ToolCallDelta { partial, .. } = &events[2] else {
            panic!("expected toolcall_delta");
        };
        let Content::ToolCall(tc) = &partial.content[0] else {
            panic!("expected tool call");
        };
        // The complete key survives the truncated JSON.
        assert_eq!(tc.arguments.get("a"), Some(&json!(1)));
    }

    /// Pi's terminal `error` frame: `stopReason`/`usage`/`errorMessage` land on the ACCUMULATED
    /// partial (unlike `createErrorEvent`, which starts from an empty message).
    #[tokio::test]
    async fn backend_error_frame_keeps_accumulated_content() {
        let raw = concat!(
            "data: {\"type\":\"start\"}\n\n",
            "data: {\"type\":\"text_start\",\"contentIndex\":0}\n\n",
            "data: {\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"partial answer\"}\n\n",
            "data: {\"type\":\"error\",\"reason\":\"error\",\"usage\":{},\"errorMessage\":\"upstream exploded\"}\n\n",
        );
        let events = collect(raw, &model()).await;
        let StreamEvent::Error { reason, error } = events.last().unwrap() else {
            panic!("expected error terminal");
        };
        assert_eq!(*reason, ErrorReason::Error);
        assert_eq!(error.error_message.as_deref(), Some("upstream exploded"));
        assert_eq!(error.content, vec![Content::text("partial answer")]);
    }

    /// `reason: "aborted"` narrows to the aborted terminal (Pi's `Extract<StopReason,
    /// "aborted" | "error">`, pi-messages.ts:78).
    #[tokio::test]
    async fn backend_aborted_frame_maps_to_aborted_terminal() {
        let raw = concat!(
            "data: {\"type\":\"start\"}\n\n",
            "data: {\"type\":\"error\",\"reason\":\"aborted\",\"usage\":{}}\n\n",
        );
        let events = collect(raw, &model()).await;
        let StreamEvent::Error { reason, error } = events.last().unwrap() else {
            panic!("expected error terminal");
        };
        assert_eq!(*reason, ErrorReason::Aborted);
        assert_eq!(error.stop_reason, StopReason::Aborted);
    }

    /// Pi: falling out of the read loop throws
    /// ``${model.provider} stream ended without a terminal event`` (pi-messages.ts:412). The exact
    /// string is upstream-derived, including the leading provider id.
    #[tokio::test]
    async fn stream_without_terminal_is_an_error() {
        let raw = concat!(
            "data: {\"type\":\"start\"}\n\n",
            "data: {\"type\":\"text_start\",\"contentIndex\":0}\n\n",
            "data: [DONE]\n\n",
        );
        let events = collect(raw, &model()).await;
        let StreamEvent::Error { error, .. } = events.last().unwrap() else {
            panic!("expected error terminal");
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("radius stream ended without a terminal event")
        );
        // Pi's `createErrorEvent` builds a FRESH message: the accumulated content is dropped.
        assert!(error.content.is_empty());
    }

    /// An unrecognized tag is skipped, not fatal — the stream still reaches its terminal.
    #[tokio::test]
    async fn unknown_event_tag_is_skipped() {
        let raw = concat!(
            "data: {\"type\":\"start\"}\n\n",
            "data: {\"type\":\"some_future_event\",\"contentIndex\":0}\n\n",
            "data: {\"type\":\"done\",\"reason\":\"stop\",\"usage\":{}}\n\n",
        );
        let events = collect(raw, &model()).await;
        assert_eq!(events.len(), 2);
        assert!(matches!(events[1], StreamEvent::Done { .. }));
    }

    /// A `contentIndex` a dense `Vec` cannot represent terminates the stream instead of allocating
    /// it (module docs: mechanism divergence).
    #[tokio::test]
    async fn absurd_content_index_is_refused() {
        let raw = concat!(
            "data: {\"type\":\"start\"}\n\n",
            "data: {\"type\":\"text_start\",\"contentIndex\":4000000000}\n\n",
        );
        let events = collect(raw, &model()).await;
        let StreamEvent::Error { error, .. } = events.last().unwrap() else {
            panic!("expected error terminal");
        };
        assert!(
            error
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("out of range")
        );
    }

    // ------------------------------------------------------------- error format --

    /// Pi `formatPiMessagesResponseError` (pi-messages.ts:121-131):
    /// `${status} ${statusText}: ${message ?? body}${code ? ` (${code})` : ""}`.
    #[test]
    fn response_error_text_matches_upstream_format() {
        let m = model();
        let body = r#"{"error":{"message":"quota exceeded","code":"insufficient_quota"}}"#;
        let (text, diag) = response_error(&m, "https://g/messages", 429, body);
        assert_eq!(
            text,
            "429 Too Many Requests: quota exceeded (insufficient_quota)"
        );

        // `createPiMessagesResponseError`'s diagnosticDetails (pi-messages.ts:141-151).
        assert_eq!(diag.r#type, "pi_messages_response_failure");
        let details = diag.details.as_ref().unwrap();
        assert_eq!(details["version"], 1);
        assert_eq!(details["provider"], "radius");
        assert_eq!(details["model"], "radius-1");
        assert_eq!(details["url"], "https://g/messages");
        assert_eq!(details["status"], 429);
        assert_eq!(details["error"]["message"], "quota exceeded");
        // `body: errorBody ? undefined : ...` — dropped when the error body parsed.
        assert!(details.get("body").is_none());
        let info = diag.error.as_ref().unwrap();
        assert_eq!(info.name.as_deref(), Some("PiMessagesResponseError"));
        assert_eq!(
            info.code,
            Some(DiagnosticCode::Str("insufficient_quota".into()))
        );
    }

    /// MIRROR: an UNPARSEABLE body falls back to the raw text with no code suffix, and the
    /// diagnostic carries `body` instead of `error`.
    #[test]
    fn response_error_falls_back_to_raw_body() {
        let m = model();
        let (text, diag) = response_error(&m, "https://g/messages", 500, "gateway melted");
        assert_eq!(text, "500 Internal Server Error: gateway melted");
        let details = diag.details.as_ref().unwrap();
        assert_eq!(details["body"], "gateway melted");
        assert!(details.get("error").is_none());
        assert!(diag.error.as_ref().unwrap().code.is_none());
    }

    /// Pi truncates the diagnostic body at 8192 chars and appends `…`.
    #[test]
    fn diagnostic_body_is_truncated() {
        let long = "x".repeat(MAX_DIAGNOSTIC_BODY_CHARS + 10);
        let out = truncate_diagnostic_string(&long);
        assert_eq!(out.chars().count(), MAX_DIAGNOSTIC_BODY_CHARS + 1);
        assert!(out.ends_with('…'));
        // MIRROR: at the bound, nothing is added.
        let exact = "y".repeat(MAX_DIAGNOSTIC_BODY_CHARS);
        assert_eq!(truncate_diagnostic_string(&exact), exact);
    }

    // ------------------------------------------------------------------ run() E2E --

    /// Serve one canned HTTP/1.1 response off `127.0.0.1:0` and record the request. NOTHING in this
    /// module's tests may reach a real host (rule: no network in tests).
    async fn serve_once(
        status_line: &'static str,
        content_type: &'static str,
        body: &'static str,
    ) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 16384];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                sink.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).to_string());
                let head = format!(
                    "{status_line}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (format!("http://{addr}"), seen)
    }

    async fn drain(rx: &mut tokio::sync::mpsc::Receiver<StreamEvent>) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev);
        }
        out
    }

    /// The whole `ApiImpl::run` path over a loopback socket: the request line, headers and body
    /// are asserted against Pi's exact request shape, and the SSE reply drives the decoder.
    #[tokio::test]
    async fn run_posts_pi_shaped_request_and_streams_the_reply() {
        let (base, seen) = serve_once(
            "HTTP/1.1 200 OK",
            "text/event-stream",
            concat!(
                "data: {\"type\":\"start\"}\n\n",
                "data: {\"type\":\"text_start\",\"contentIndex\":0}\n\n",
                "data: {\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"ok\"}\n\n",
                "data: {\"type\":\"text_end\",\"contentIndex\":0,\"content\":\"ok\"}\n\n",
                "data: {\"type\":\"done\",\"reason\":\"stop\",\"usage\":{\"input\":3,\"output\":1,",
                "\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":4,",
                "\"cost\":{\"input\":0,\"output\":0,\"cacheRead\":0,\"cacheWrite\":0,\"total\":0}}}\n\n",
            ),
        )
        .await;

        let mut m = model();
        m.base_url = format!("{base}/v1");
        let (sink, mut rx) = channel(64);
        let api = PiMessagesApi::new();
        api.run(
            &m,
            &user_ctx("hi"),
            &auth_with(Some("sk-live")),
            &StreamOptions::default(),
            CancelToken::new(),
            sink,
        )
        .await;
        let events = drain(&mut rx).await;

        let req = seen.lock().unwrap().first().cloned().unwrap_or_default();
        // Pi: `POST <baseUrl>/messages`, no `/v1/` injected by the converter.
        assert!(req.starts_with("POST /v1/messages HTTP/1.1"), "{req}");
        assert!(req.to_lowercase().contains("authorization: bearer sk-live"));
        assert!(req.to_lowercase().contains("accept: text/event-stream"));
        assert!(req.to_lowercase().contains("content-type: application/json"));
        let body = req.split("\r\n\r\n").nth(1).unwrap_or_default();
        let sent: Value = serde_json::from_str(body).expect("json body");
        assert_eq!(sent["model"], "radius-1");
        assert_eq!(sent["context"]["messages"][0]["content"][0]["text"], "hi");

        let StreamEvent::Done { message, .. } = events.last().unwrap() else {
            panic!("expected done terminal, got {events:?}");
        };
        assert_eq!(message.content, vec![Content::text("ok")]);
        assert_eq!(message.usage.total_tokens, 4);
    }

    /// A non-2xx reply becomes Pi's formatted `PiMessagesResponseError` terminal, diagnostic and
    /// all — the whole point of `createPiMessagesResponseError` reaching the transcript.
    #[tokio::test]
    async fn run_maps_non_2xx_to_the_pi_messages_response_error() {
        let (base, _seen) = serve_once(
            "HTTP/1.1 401 Unauthorized",
            "application/json",
            r#"{"error":{"message":"bad token","code":"invalid_api_key"}}"#,
        )
        .await;
        let mut m = model();
        m.base_url = base;
        let (sink, mut rx) = channel(64);
        PiMessagesApi::new()
            .run(
                &m,
                &user_ctx("hi"),
                &auth_with(Some("sk-bad")),
                &StreamOptions::default(),
                CancelToken::new(),
                sink,
            )
            .await;
        let events = drain(&mut rx).await;

        let StreamEvent::Error { reason, error } = events.last().unwrap() else {
            panic!("expected error terminal, got {events:?}");
        };
        assert_eq!(*reason, ErrorReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("401 Unauthorized: bad token (invalid_api_key)")
        );
        let diags = error.diagnostics.as_ref().expect("response diagnostic");
        assert_eq!(diags[0].r#type, "pi_messages_response_failure");
        assert_eq!(diags[0].details.as_ref().unwrap()["status"], 401);
    }

    /// Pi `throw new Error(`No API key provided for provider "${model.provider}"`)`
    /// (pi-messages.ts:355-358) — asserted byte-for-byte, quotes included. No socket is opened.
    #[tokio::test]
    async fn missing_api_key_is_the_upstream_error_string() {
        let (sink, mut rx) = channel(8);
        PiMessagesApi::new()
            .run(
                &model(),
                &user_ctx("hi"),
                &auth_with(None),
                &StreamOptions::default(),
                CancelToken::new(),
                sink,
            )
            .await;
        let events = drain(&mut rx).await;
        assert_eq!(events.len(), 1);
        let StreamEvent::Error { error, .. } = &events[0] else {
            panic!("expected error terminal");
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("No API key provided for provider \"radius\"")
        );
        assert!(error.content.is_empty());
        assert_eq!(error.api.to_string(), API_ID);
    }

    /// The registry seam: `factory()` yields an impl whose `api()` is `"pi-messages"`, so the
    /// one-line `register` in `api/mod.rs` is all that is left to wire it up.
    #[test]
    fn factory_reports_the_pi_messages_api_id() {
        assert_eq!(factory().api().to_string(), API_ID);
        let reg = crate::api::ApiRegistry::new();
        reg.register_impl(factory());
        assert!(reg.contains(&ApiId::from(API_ID)));
    }
}

//! The `bedrock-converse-stream` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! 1:1 behavioural port of pi's `packages/ai/src/api/bedrock-converse-stream.ts` (v0.83.0) — the
//! Amazon Bedrock `ConverseStream` API. Covers the whole observable surface of that file: region /
//! endpoint / credential precedence, bearer-token auth, the proxy hook, caller-header injection,
//! the ConverseStream payload (messages, system, `inferenceConfig`, `toolConfig`,
//! `additionalModelRequestFields`, `requestMetadata`), prompt-cache points, extended-thinking
//! (adaptive and budget-based), the streaming event assembly, the stop-reason table and the
//! `formatBedrockError` display strings.
//!
//! # Mechanism divergence: no AWS SDK
//!
//! Upstream drives `@aws-sdk/client-bedrock-runtime`, which owns three things behaviourally
//! invisible to the caller: the REST binding (`POST {endpoint}/model/{modelId}/converse-stream`),
//! SigV4 request signing, and the `application/vnd.amazon.eventstream` binary framing of the
//! response. `cyrup-provider`'s manifest carries no AWS dependency — the workspace avoids adding a
//! dependency where a self-contained routine will do (see the justification comments in the root
//! `Cargo.toml`) — so all three are implemented here directly on `reqwest`, exactly as
//! `anthropic-messages` speaks Anthropic's HTTP+SSE protocol without the Anthropic SDK.
//!
//! What the SDK does for upstream and this module does inline:
//!
//! | SDK concern | here |
//! |---|---|
//! | `BedrockRuntimeClientConfig` `{region, endpoint, credentials, profile, token}` | `resolve_client_config` |
//! | default credential chain (env → shared config/credentials file) | `configured_bedrock_credentials` + `shared_profile_credentials` |
//! | SigV4 `build`-step signing | `sign_sigv4` |
//! | `ConverseStreamCommand` REST binding | `converse_stream_url` + `build_params` |
//! | `vnd.amazon.eventstream` decoding | `EventStreamDecoder` |
//! | `middlewareStack.add(..., {step:"build"})` header injection | `apply_custom_headers` |
//!
//! Smithy's `build` step runs after serialisation but **before** signing, which is why upstream's
//! comment says injected headers are covered by the signature; the same holds here because
//! `apply_custom_headers` mutates the header map that `sign_sigv4` then signs.
//!
//! # Scope notes
//!
//! * **`sanitizeSurrogates` is a no-op here.** A Rust `String` cannot hold a lone surrogate, so
//!   upstream's `sanitize-unicode.ts` pass (which strips them) has nothing to remove. cyrup's
//!   shared [`sanitize_surrogates`] is likewise the
//!   identity, and it is still called at each of upstream's call sites so the *shape* of the port
//!   stays diffable.
//! * **`resolveJsonSchemaStrictSampling` is unreachable.** Upstream reads
//!   `tool.constrainedSampling`; cyrup's [`ToolDef`] has no such field, so
//!   the helper's `if (!config …) return undefined` arm is the only reachable one and no `strict`
//!   key can ever be emitted. `model.compat.supports_strict_mode` is therefore not consulted. This
//!   is a gap in `ToolDef`, not in this converter; closing it means adding the field to
//!   `context.rs` (out of scope for this file).
//! * Upstream's non-Node branch (`typeof process === "undefined"`) is unreachable in Rust; only the
//!   Node/Bun branch is ported.

use crate::HeaderMap;
use crate::api::compat::sanitize_surrogates;
use crate::api::openai_completions::transform_messages_with;
use crate::api::{ApiImpl, EventSink};
use crate::auth::{AuthResult, ProviderEnv};
use crate::context::{Context, ToolDef};
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::build_client_for_target_forcing_http1;
use crate::stream::{CacheRetention, StreamEvent, StreamOptions};
use crate::usage::compute_cost;
use crate::utils::constrained_sampling::{
    ConstrainedSamplingError, resolve_json_schema_strict_sampling,
};
use crate::utils::error_body::normalize_error_body;
use crate::utils::provider_retry::{ProviderRetry, is_retryable_provider_error, retry_delay_ms};
use crate::utils::json_parse::parse_streaming_json_object;
use crate::utils::simple_options::{adjust_max_tokens_for_thinking, clamp_max_tokens_to_context};
use base64::Engine as _;
use cyrup_core::{
    ApiId, AssistantMessage, CancelToken, Content, Message, StopReason, ThinkingLevel, ToolCall,
    ToolCallId, Usage, diagnostics::create_assistant_message_diagnostic_from,
};
use futures::StreamExt;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::BEDROCK_CONVERSE_STREAM;

/// pi `EMPTY_TEXT_PLACEHOLDER` (`bedrock-converse-stream.ts:104`).
const EMPTY_TEXT_PLACEHOLDER: &str = "<empty>";

/// pi `BEDROCK_DATA_RETENTION_DOCS_URL` (`bedrock-converse-stream.ts:339`).
const BEDROCK_DATA_RETENTION_DOCS_URL: &str =
    "https://docs.aws.amazon.com/bedrock/latest/userguide/data-retention.html";

/// pi's interleaved-thinking beta token (`bedrock-converse-stream.ts:1080`).
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// The SigV4 service name for the Bedrock runtime endpoint.
const SIGV4_SERVICE: &str = "bedrock";

/// Response media type of `ConverseStream` — the AWS binary event stream the SDK decodes for
/// upstream and [`EventStreamDecoder`] decodes here.
const EVENT_STREAM_MEDIA_TYPE: &str = "application/vnd.amazon.eventstream";

/// Retries after the first attempt on the Bedrock route. The AWS SDK v3 **standard** retry mode
/// makes 3 attempts, and pi's client config (`bedrock-converse-stream.ts:150-222` @v0.83.0) never
/// overrides `maxAttempts`/`retryStrategy`, so that is what pi inherits per turn (PROV-043).
const BEDROCK_STANDARD_MODE_RETRIES: u32 = 2;

/// The dummy credential pair upstream installs when `AWS_BEDROCK_SKIP_AUTH=1`
/// (`bedrock-converse-stream.ts:186-189`).
const SKIP_AUTH_ACCESS_KEY: &str = "dummy-access-key";
const SKIP_AUTH_SECRET_KEY: &str = "dummy-secret-key";

// ---------------------------------------------------------------------------
// Typed options (pi `BedrockOptions`, bedrock-converse-stream.ts:68-100)
// ---------------------------------------------------------------------------

/// How Claude's thinking content is returned (pi `BedrockThinkingDisplay`,
/// `bedrock-converse-stream.ts:66`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BedrockThinkingDisplay {
    Summarized,
    Omitted,
}

impl BedrockThinkingDisplay {
    /// The wire string for `additionalModelRequestFields.thinking.display`.
    pub fn as_wire(self) -> &'static str {
        match self {
            BedrockThinkingDisplay::Summarized => "summarized",
            BedrockThinkingDisplay::Omitted => "omitted",
        }
    }
}

/// Bedrock's `toolChoice` union (pi `BedrockOptions.toolChoice`,
/// `bedrock-converse-stream.ts:71`). Distinct from cyrup's unified
/// [`ToolChoice`](crate::stream::ToolChoice) because Bedrock spells "required" as `any` and its
/// named form is `{tool:{name}}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BedrockToolChoice {
    Auto,
    Any,
    None,
    Tool { name: String },
}

impl BedrockToolChoice {
    /// Lower cyrup's unified tool choice onto Bedrock's union
    /// (`Required` → `any`, `Function` → `{tool:{name}}`).
    pub fn from_unified(tc: &crate::stream::ToolChoice) -> Self {
        use crate::stream::ToolChoice;
        match tc {
            ToolChoice::Auto => BedrockToolChoice::Auto,
            ToolChoice::None => BedrockToolChoice::None,
            ToolChoice::Required => BedrockToolChoice::Any,
            ToolChoice::Function { name } => BedrockToolChoice::Tool { name: name.clone() },
        }
    }

    /// The `toolConfig.toolChoice` wire JSON, or `None` for `none` (upstream returns no
    /// `toolConfig` at all for `"none"`, handled by [`convert_tool_config`]).
    fn to_wire(&self) -> Option<Value> {
        match self {
            BedrockToolChoice::Auto => Some(json!({ "auto": {} })),
            BedrockToolChoice::Any => Some(json!({ "any": {} })),
            BedrockToolChoice::Tool { name } => Some(json!({ "tool": { "name": name } })),
            BedrockToolChoice::None => None,
        }
    }
}

/// Per-API typed options for `bedrock-converse-stream` (pi `BedrockOptions`,
/// `bedrock-converse-stream.ts:68-100`).
///
/// `reasoning` and `thinkingBudgets` are NOT modelled here: cyrup carries them on the unified
/// [`StreamOptions::reasoning`] / [`StreamOptions::thinking_budgets`], and `build_params`
/// performs the same lowering upstream's `streamSimple` does (`:403-449`) — matching how
/// `anthropic-messages` already handles them. Every field defaults to `None`, reproducing pi's
/// defaults exactly. Carried via [`StreamOptions::api_options`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BedrockOptions {
    /// pi `region` (`:69`) — the highest-priority region source after an ARN-embedded region.
    pub region: Option<String>,
    /// pi `profile` (`:70`) — a shared-config profile that must beat ambient access keys.
    pub profile: Option<String>,
    /// pi `toolChoice` (`:71`).
    pub tool_choice: Option<BedrockToolChoice>,
    /// pi `interleavedThinking` (`:77`); `None` ⇒ pi default `true`.
    pub interleaved_thinking: Option<bool>,
    /// pi `thinkingDisplay` (`:88`); `None` ⇒ pi default `"summarized"`.
    pub thinking_display: Option<BedrockThinkingDisplay>,
    /// pi `requestMetadata` (`:93`) — cost-allocation tags echoed into the request body.
    pub request_metadata: Option<BTreeMap<String, String>>,
    /// pi `bearerToken` (`:99`) — Bedrock API-key auth, bypassing SigV4.
    pub bearer_token: Option<String>,
}

impl BedrockOptions {
    /// Resolve the typed options a caller can actually reach through cyrup's unified
    /// [`StreamOptions`].
    ///
    /// `toolChoice` is the one option with a unified spelling, so the unified value wins and the
    /// typed one is the fallback — the ranking every other ported api uses. The remaining six are
    /// typed-options-only and would be silently unreachable without this resolution.
    pub fn from_stream_options(opts: &StreamOptions) -> Self {
        let typed = opts
            .api_options
            .as_ref()
            .and_then(crate::stream::ApiStreamOptions::bedrock);

        Self {
            region: typed.and_then(|t| t.region.clone()),
            profile: typed.and_then(|t| t.profile.clone()),
            tool_choice: opts
                .tool_choice
                .as_ref()
                .map(BedrockToolChoice::from_unified)
                .or_else(|| typed.and_then(|t| t.tool_choice.clone())),
            interleaved_thinking: typed.and_then(|t| t.interleaved_thinking),
            thinking_display: typed.and_then(|t| t.thinking_display),
            request_metadata: typed.and_then(|t| t.request_metadata.clone()),
            bearer_token: typed.and_then(|t| t.bearer_token.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// ApiImpl
// ---------------------------------------------------------------------------

/// The `ApiImpl` for `"bedrock-converse-stream"`.
pub struct BedrockConverseStreamApi {
    api: ApiId,
}

impl Default for BedrockConverseStreamApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl BedrockConverseStreamApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(BedrockConverseStreamApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for BedrockConverseStreamApi {
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
        let api = self.api.clone();

        // The whole body of upstream's `(async () => { … })()` sits inside one `try` whose catch
        // sets `stopReason = options.signal?.aborted ? "aborted" : "error"`, folds the composed
        // `formatBedrockError` message in, and pushes ONE terminal `error` event (`:304-314`).
        // `run_inner` is that try block; this arm is that catch.
        if let Err(failure) = run_inner(model, ctx, auth, opts, &cancel, &sink, &api).await {
            let mut message = failure.partial;
            message.stop_reason = if cancel.is_cancelled() {
                StopReason::Aborted
            } else {
                failure.stop_reason
            };
            message.error_message = Some(failure.message);
            // pi `:318-320`: structured diagnostics ride along ONLY on the `error` terminal — an
            // aborted turn is not a provider failure and gets none.
            if message.stop_reason == StopReason::Error {
                append_bedrock_failure_diagnostic(
                    &mut message,
                    failure.status,
                    failure.error_code.as_deref(),
                    failure.request_id.as_deref(),
                );
            }
            sink.send(StreamEvent::terminal(message)).await;
        }
    }
}

/// A failure inside the ported `try` block: the partial snapshot to attach plus the composed
/// `errorMessage` (already run through [`format_bedrock_error`]).
///
/// `status`/`error_code` are the parts of upstream's thrown SDK exception that survive into the
/// structured diagnostic (`error.$metadata.httpStatusCode` and `error.name`); they are `None` for
/// the failure paths whose upstream counterpart throws a plain `Error` carrying neither.
struct BedrockFailure {
    partial: AssistantMessage,
    stop_reason: StopReason,
    message: String,
    status: Option<u16>,
    error_code: Option<String>,
    /// Upstream's hoisted `responseRequestId` (pi `:225`, assigned at `:254`), carried on the
    /// failure so the catch can still correlate a mid-stream throw that has no metadata of its own.
    request_id: Option<String>,
}

impl BedrockFailure {
    fn errored(partial: AssistantMessage, message: String) -> Self {
        BedrockFailure {
            partial,
            stop_reason: StopReason::Error,
            message,
            status: None,
            error_code: None,
            request_id: None,
        }
    }

    /// Attach the hoisted response request id (pi `:254`) to a failure raised after the response
    /// headers were seen.
    fn with_request_id(mut self, request_id: Option<&str>) -> Self {
        self.request_id = request_id.map(str::to_string);
        self
    }

    /// The `client.send()` rejection path: upstream's throw is a `BedrockRuntimeServiceException`,
    /// so `$metadata.httpStatusCode` and the modeled `.name` are both present.
    fn service_exception(
        partial: AssistantMessage,
        message: String,
        status: u16,
        name: &str,
    ) -> Self {
        BedrockFailure {
            partial,
            stop_reason: StopReason::Error,
            message,
            status: Some(status),
            error_code: extract_bedrock_error_code(name),
            request_id: None,
        }
    }
}

/// Over-long values are DROPPED rather than truncated (pi `MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS`,
/// v0.84.1 `ai/src/api/bedrock-converse-stream.ts:379`): a truncated request id is not a request id.
const MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS: usize = 200;

/// pi `normalizeDiagnosticValue` (v0.84.1 `ai/src/api/bedrock-converse-stream.ts:381-386`).
///
/// **Unit**: pi's guard is `trimmed.length > MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS` (`:384`), and JS
/// `String.prototype.length` counts UTF-16 CODE UNITS, not scalar values. The exact Rust analog is
/// [`str::encode_utf16`]`().count()`; `chars().count()` (scalars, what this was) and `len()`
/// (UTF-8 bytes) agree with it only for ASCII. Astral-plane characters are two UTF-16 units each,
/// so a 150-emoji request id is 300 units to pi — dropped — and 150 scalars to a `chars()`-based
/// cyrup — kept, emitting a `requestId` diagnostic pi never emits. Same reasoning, and the same
/// fix, as `cyrup-permission-system/src/wildcard.rs:21-23,81`.
fn normalize_diagnostic_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.encode_utf16().count() > MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS {
        return None;
    }
    Some(trimmed.to_string())
}

/// pi `extractBedrockErrorCode` (v0.84.1 `ai/src/api/bedrock-converse-stream.ts:388-396`): modeled
/// Bedrock errors all end in `Exception`, unlike transport names such as `TimeoutError`, so a name
/// that does not is not a code.
fn extract_bedrock_error_code(name: &str) -> Option<String> {
    if !name.ends_with("Exception") {
        return None;
    }
    normalize_diagnostic_value(name)
}

/// pi `appendBedrockFailureDiagnostic` (v0.84.1 `ai/src/api/bedrock-converse-stream.ts:398-421`),
/// called from the catch at `:318-320` whenever the terminal reason settled on `"error"`.
///
/// VERSION LAG (v0.83.0 → v0.84.1): neither `appendBedrockFailureDiagnostic`,
/// `normalizeDiagnosticValue` nor the hoisted `responseRequestId` (`:225`) exists at v0.83.0 — the
/// whole structured-diagnostic path is new in v0.84.1.
///
/// Structured metadata sits ALONGSIDE `error_message`, which stays byte-identical because the
/// turn-level retry classifier matches against it. Unknown fields are omitted, never guessed: a
/// modeled mid-stream exception reaches upstream as a bare object literal (not an `Error`, no
/// `$metadata`), leaving only the fallback request id — which is why `error_code`/`status` are
/// passed `None` on that path here too. When nothing is known the diagnostic is not appended at all.
fn append_bedrock_failure_diagnostic(
    output: &mut AssistantMessage,
    status: Option<u16>,
    error_code: Option<&str>,
    fallback_request_id: Option<&str>,
) {
    let mut details = Map::new();
    if let Some(status) = status {
        details.insert("status".to_string(), json!(status));
    }
    if let Some(code) = error_code {
        details.insert("errorCode".to_string(), json!(code));
    }
    if let Some(id) = fallback_request_id.and_then(normalize_diagnostic_value) {
        details.insert("requestId".to_string(), json!(id));
    }
    if details.is_empty() {
        return;
    }
    output.append_diagnostic(create_assistant_message_diagnostic_from(
        "bedrock_response_failure",
        None,
        Some(Value::Object(details)),
    ));
}

/// pi's `stream()` try block (`bedrock-converse-stream.ts:222-303`).
async fn run_inner(
    model: &Model,
    ctx: &Context,
    auth: &AuthResult,
    opts: &StreamOptions,
    cancel: &CancelToken,
    sink: &EventSink,
    api: &ApiId,
) -> Result<(), BedrockFailure> {
    let bedrock = BedrockOptions::from_stream_options(opts);
    let env = EnvSource::new(opts.env.as_ref().or(auth.env.as_ref()));
    let mut dec = Decoder::default();

    let config = resolve_client_config(model, opts, &bedrock, auth, &env);

    // `cacheRetention` + payload (pi `:228-241`).
    let cache_retention = resolve_cache_retention(opts.cache_retention, &env);
    let payload = build_params(model, ctx, opts, &bedrock, cache_retention, &env)
        .map_err(|e| BedrockFailure::errored(dec.snapshot(model, api), format_bedrock_error(&e)))?;

    // `onPayload` may replace the whole command input, including `modelId` (pi `:242-245`).
    let payload = crate::stream::apply_on_payload(opts, model, payload).await;
    let (request_model_id, body) = split_command_input(payload, model);

    let url = converse_stream_url(&config.endpoint, &request_model_id);
    let body_bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());

    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), EVENT_STREAM_MEDIA_TYPE.to_string());
    // pi `:224-227`: caller headers are injected at the Smithy `build` step, i.e. before signing.
    apply_custom_headers(&mut headers, opts.headers.as_ref(), model.headers.as_ref());
    authorize(&mut headers, &config, &url, &body_bytes).map_err(|e| {
        BedrockFailure::errored(dec.snapshot(model, api), format_bedrock_error(&e))
    })?;

    // pi resolves an HTTP(S) proxy per request (`:197-205`), and — only when there is no proxy —
    // honours `AWS_BEDROCK_FORCE_HTTP1=1` by dropping to a plain HTTP/1.1 handler (`:206-209`,
    // "Some custom endpoints require HTTP/1.1 instead of HTTP/2"). cyrup's client negotiates h2 by
    // ALPN, so without this a custom Bedrock endpoint or corporate gateway that requires HTTP/1.1
    // had no override at all (PROV-044).
    let force_http1 = env.get("AWS_BEDROCK_FORCE_HTTP1").as_deref() == Some("1");
    let client = build_client_for_target_forcing_http1(
        &url,
        &crate::auth::types::EnvAuthContext,
        auth.env.as_ref(),
        opts.timeout_ms,
        force_http1,
    )
    .await
    .map_err(|e| {
        BedrockFailure::errored(dec.snapshot(model, api), format_bedrock_error(&e.to_string()))
    })?;

    // PROV-043. pi builds `new BedrockRuntimeClient(config)` (`bedrock-converse-stream.ts:223`)
    // with a config (`:150-222`) that sets credentials, region, token and `requestHandler` but
    // NEVER `maxAttempts` or `retryStrategy` — so the AWS SDK v3 **standard** retry mode applies:
    // three attempts with jittered backoff on throttling and 5xx, inside a single pi turn. cyrup
    // speaks the wire directly and had no retry at all here, so a routine `ThrottlingException`
    // that pi swallows became a visible turn failure. The budget is a constant, not
    // `ProviderRetry::from_options`, because pi's is not configurable on this route either: a
    // `retry.provider.maxRetries` setting reaches the other seven impls and not this one.
    let retry = ProviderRetry {
        max_retries: BEDROCK_STANDARD_MODE_RETRIES,
        max_retry_delay_ms: opts.max_retry_delay_ms,
    };
    let max_retries = retry.max_retries;
    let mut retries_remaining = max_retries;
    let aborted = |dec: &Decoder| BedrockFailure {
        partial: dec.snapshot(model, api),
        stop_reason: StopReason::Aborted,
        message: "Request was aborted".to_string(),
        status: None,
        error_code: None,
        request_id: None,
    };
    let response = loop {
        // The SigV4 signature is over the (unchanged) body and headers, so it is reused across
        // attempts exactly as the SDK's retry middleware reuses the signed request.
        let mut request = client.post(&url).body(body_bytes.clone());
        for (name, value) in &headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let attempt = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProviderError::Aborted),
            sent = request.send() => sent.map_err(|e| ProviderError::Transport(Box::new(e))),
        };
        // An abort is terminal and is never retried (Pi `provider-retry.ts:117`).
        if cancel.is_cancelled() {
            return Err(aborted(&dec));
        }

        let (retry_headers, message) = match attempt {
            Err(ProviderError::Aborted) => return Err(aborted(&dec)),
            // A transport failure carries no status: `error.status === undefined` ⇒ retryable.
            Err(transport) => {
                if retries_remaining == 0 {
                    return Err(BedrockFailure::errored(
                        dec.snapshot(model, api),
                        format_bedrock_error(&transport.to_string()),
                    ));
                }
                (None, transport.to_string())
            }
            Ok(resp) => {
                let code = resp.status().as_u16();
                // A success — and an exhausted or non-retryable failure — leaves the loop so the
                // status/error-body path below is unchanged.
                if resp.status().is_success()
                    || retries_remaining == 0
                    || !is_retryable_provider_error(Some(code), Some(resp.headers()))
                {
                    break resp;
                }
                let retry_headers = resp.headers().clone();
                (Some(retry_headers), format!("http {code}"))
            }
        };

        let retry_index = max_retries.saturating_sub(retries_remaining);
        retries_remaining = retries_remaining.saturating_sub(1);
        let delay = retry_delay_ms(retry_headers.as_ref(), &message, retry_index, retry)
            .map_err(|e| {
                BedrockFailure::errored(
                    dec.snapshot(model, api),
                    format_bedrock_error(&e.to_string()),
                )
            })?;
        // Interruptible backoff, unlike the SDK's own retry timers.
        if cancel
            .run_until_cancelled(tokio::time::sleep(std::time::Duration::from_millis(delay)))
            .await
            .is_none()
        {
            return Err(aborted(&dec));
        }
    };

    // pi `:249-255`: fire `onResponse` with the status and the request-id header.
    let status = response.status().as_u16();
    let request_id = response
        .headers()
        .get("x-amzn-requestid")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    // pi `:254`: `responseRequestId = normalizeDiagnosticValue(response.$metadata.requestId)` —
    // hoisted out of the try so a LATER, metadata-less failure can still be correlated.
    let response_request_id: Option<String> =
        request_id.as_deref().and_then(normalize_diagnostic_value);
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if let Some(hook) = &opts.on_response {
        let mut hdrs = BTreeMap::new();
        if let Some(id) = &request_id {
            hdrs.insert("x-amzn-requestid".to_string(), id.clone());
        }
        hook(
            crate::stream::ProviderResponse {
                status,
                headers: hdrs,
            },
            model.clone(),
        )
        .await;
    }

    if !(200..300).contains(&status) {
        let body = normalize_error_body(&response.text().await.unwrap_or_default());
        let name = error_type
            .as_deref()
            .and_then(|t| t.split([':', '#']).next_back().filter(|s| !s.is_empty()))
            .unwrap_or("");
        // pi's `client.send()` rejection: a `BedrockRuntimeServiceException` whose `$metadata`
        // carries the status and whose `.name` is the modeled shape (pi `:398-421` reads both).
        return Err(BedrockFailure::service_exception(
            dec.snapshot(model, api),
            format_bedrock_service_error(name, status, &body),
            status,
            name,
        )
        .with_request_id(response_request_id.as_deref()));
    }

    // NOTE: the `start` event is NOT pushed here — pi pushes it from the `messageStart` frame
    // (`:262`), so a stream that fails before `messageStart` emits the terminal `error` alone.
    // pi `:257-289`: the `for await (const item of response.stream!)` loop.
    let mut frames = EventStreamDecoder::default();
    let mut bytes = response.bytes_stream();
    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            chunk = bytes.next() => Some(chunk),
        };
        let Some(chunk) = next else {
            return Err(BedrockFailure {
                partial: dec.snapshot(model, api),
                stop_reason: StopReason::Aborted,
                message: "Request was aborted".to_string(),
                status: None,
                error_code: None,
                request_id: response_request_id.clone(),
            });
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| {
            BedrockFailure::errored(
                dec.snapshot(model, api),
                format_bedrock_error(&format!("transport error: {e}")),
            )
            .with_request_id(response_request_id.as_deref())
        })?;
        frames.push(&chunk);
        loop {
            let frame = frames.next_frame().map_err(|e| {
                BedrockFailure::errored(dec.snapshot(model, api), format_bedrock_error(&e))
                    .with_request_id(response_request_id.as_deref())
            })?;
            let Some(frame) = frame else { break };
            match dispatch_frame(&frame, &mut dec, model, api, sink).await {
                Ok(true) => {}
                // Consumer dropped the stream.
                Ok(false) => return Ok(()),
                Err(message) => {
                    // pi's `throw item.<x>Exception`: a bare object literal, so only the hoisted
                    // request id survives into the diagnostic (`:400-402`).
                    return Err(BedrockFailure::errored(dec.snapshot(model, api), message)
                        .with_request_id(response_request_id.as_deref()));
                }
            }
        }
    }

    // pi `:291-293`: an aborted signal after the loop is still terminal.
    if cancel.is_cancelled() {
        return Err(BedrockFailure {
            partial: dec.snapshot(model, api),
            stop_reason: StopReason::Aborted,
            message: "Request was aborted".to_string(),
            status: None,
            error_code: None,
            request_id: response_request_id.clone(),
        });
    }

    // pi `:295-300`: a stream that ended still "pending" is TRUNCATED, and a settled
    // `error`/`aborted` stop reason throws with the recorded message. `end_of_stream` encodes both
    // (a `None` stop reason becomes the `error` terminal with the truncation text; a settled
    // `Error` routes to the same terminal carrying `error_message`).
    let mut message = dec.snapshot(model, api);
    if dec.stop_reason == Some(StopReason::Error) && dec.error_message.is_none() {
        message.error_message = Some("An unknown error occurred".to_string());
    }
    // pi `:301-306` throws for a still-`pending` or already-`error` reason, and the catch then runs
    // `:318-320`. Upstream's throw here is a plain `Error` with no `$metadata`, so only the hoisted
    // request id lands — `end_of_stream` settles on `error` for exactly these two cases.
    let settles_on_error = !matches!(
        dec.stop_reason,
        Some(r) if r != StopReason::Pending && r != StopReason::Error
    );
    if settles_on_error {
        append_bedrock_failure_diagnostic(
            &mut message,
            None,
            None,
            response_request_id.as_deref(),
        );
    }
    sink.send(StreamEvent::end_of_stream(
        message,
        dec.stop_reason,
        "Bedrock stream ended without a stop reason",
    ))
    .await;
    Ok(())
}

/// Split the (possibly `onPayload`-replaced) command input into the path-bound `modelId` and the
/// REST request body. Upstream hands `onPayload` the SDK's `ConverseStreamCommand` input, whose
/// `modelId` member is a **URI label**, not a body field; the REST binding puts it in the path and
/// would reject it as an unknown body member, so it is removed here after being read.
fn split_command_input(payload: Value, model: &Model) -> (String, Value) {
    let mut obj = match payload {
        Value::Object(map) => map,
        other => return (model.id.as_str().to_string(), other),
    };
    let id = obj
        .remove("modelId")
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| model.id.as_str().to_string());
    (id, Value::Object(obj))
}

// ---------------------------------------------------------------------------
// Environment resolution (pi getProviderEnvValue, provider-env.ts:44-52)
// ---------------------------------------------------------------------------

/// Env lookup for the resolution helpers.
///
/// `overlay` is pi's `options.env` (scoped, wins). `ambient` is the process environment; it is a
/// **test seam**: production constructs it with [`EnvSource::new`], which leaves it `None` and
/// falls through to [`std::env::var`], while the resolution tests inject a map so they never depend
/// on the ambient AWS configuration of whatever machine runs them.
#[derive(Clone, Copy, Default)]
struct EnvSource<'a> {
    overlay: Option<&'a ProviderEnv>,
    ambient: Option<&'a ProviderEnv>,
}

impl<'a> EnvSource<'a> {
    fn new(overlay: Option<&'a ProviderEnv>) -> Self {
        EnvSource {
            overlay,
            ambient: None,
        }
    }

    /// pi `getProviderEnvValue(name, env)`: the scoped overlay first, then the process env. Empty
    /// values are skipped (pi's `||` chain treats `""` as absent).
    fn get(&self, name: &str) -> Option<String> {
        if let Some(map) = self.overlay
            && let Some(v) = map.get(name).filter(|v| !v.is_empty())
        {
            return Some(v.clone());
        }
        self.ambient(name)
    }

    /// pi `getProviderEnvValue(name)` with **no** env argument (`bedrock-converse-stream.ts:144`):
    /// the process environment only, deliberately ignoring the scoped overlay.
    fn ambient(&self, name: &str) -> Option<String> {
        match self.ambient {
            Some(map) => map.get(name).filter(|v| !v.is_empty()).cloned(),
            None => std::env::var(name).ok().filter(|v| !v.is_empty()),
        }
    }
}

// ---------------------------------------------------------------------------
// Client configuration (pi bedrock-converse-stream.ts:136-220)
// ---------------------------------------------------------------------------

/// Static AWS credentials (pi `BedrockRuntimeClientConfig["credentials"]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct AwsCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

/// The resolved `BedrockRuntimeClientConfig` (pi `config`, `bedrock-converse-stream.ts:140-220`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BedrockClientConfig {
    /// `config.profile`.
    profile: Option<String>,
    /// `config.region`.
    region: Option<String>,
    /// `config.endpoint`, already defaulted to the standard regional runtime host when upstream
    /// leaves it unset (the SDK's endpoint resolver does that for pi).
    endpoint: String,
    /// `config.credentials`.
    credentials: Option<AwsCredentials>,
    /// `config.token.token` + `authSchemePreference: ["httpBearerAuth"]`.
    bearer_token: Option<String>,
}

/// 1:1 port of pi's client-config block (`bedrock-converse-stream.ts:136-220`).
///
/// The precedence rules that matter, in upstream's own order:
/// 1. `options.profile || options.env.AWS_PROFILE || AWS_PROFILE` becomes `config.profile`.
/// 2. A standard `bedrock-runtime.<region>.amazonaws.com[.cn]` base URL is pinned as
///    `config.endpoint` **only** when neither a region nor an ambient `AWS_PROFILE` is configured;
///    a custom (VPC/proxy) endpoint is always pinned.
/// 3. Region: ARN-embedded > explicit/env > endpoint-derived (when pinned) > `us-east-1`, and the
///    last default is skipped entirely when an ambient `AWS_PROFILE` is set.
/// 4. Ambient access keys are used only when no profile was explicitly configured.
fn resolve_client_config(
    model: &Model,
    opts: &StreamOptions,
    bedrock: &BedrockOptions,
    auth: &AuthResult,
    env: &EnvSource<'_>,
) -> BedrockClientConfig {
    let base_url = auth
        .auth
        .base_url
        .clone()
        .unwrap_or_else(|| model.base_url.clone());

    // pi `:139`: the explicit option, then the SCOPED `AWS_PROFILE` (overlay only).
    let options_profile = bedrock.profile.clone().or_else(|| {
        opts.env
            .as_ref()
            .or(auth.env.as_ref())
            .and_then(|m| m.get("AWS_PROFILE"))
            .filter(|v| !v.is_empty())
            .cloned()
    });
    let profile = options_profile
        .clone()
        .or_else(|| env.get("AWS_PROFILE"))
        .filter(|v| !v.is_empty());

    let configured_region = configured_bedrock_region(bedrock, env);
    let has_ambient_profile = env.ambient("AWS_PROFILE").is_some();
    let endpoint_region = standard_bedrock_endpoint_region(&base_url);
    let use_explicit_endpoint =
        should_use_explicit_bedrock_endpoint(&base_url, configured_region.as_deref(), has_ambient_profile);

    let skip_auth = env.get("AWS_BEDROCK_SKIP_AUTH").as_deref() == Some("1");
    let bearer_token = bedrock
        .bearer_token
        .clone()
        .or_else(|| opts.api_key.clone())
        .or_else(|| auth.auth.api_key.clone())
        .or_else(|| env.get("AWS_BEARER_TOKEN_BEDROCK"))
        .filter(|t| !t.is_empty());
    let use_bearer_token = bearer_token.is_some() && !skip_auth;

    // pi `:173-182`.
    let region = if let Some(arn_region) = arn_region(model.id.as_str()) {
        Some(arn_region)
    } else if let Some(r) = configured_region.clone() {
        Some(r)
    } else if use_explicit_endpoint && endpoint_region.is_some() {
        endpoint_region.clone()
    } else if !has_ambient_profile {
        Some("us-east-1".to_string())
    } else {
        None
    };

    // pi `:185-195`.
    let credentials = if skip_auth {
        Some(AwsCredentials {
            access_key_id: SKIP_AUTH_ACCESS_KEY.to_string(),
            secret_access_key: SKIP_AUTH_SECRET_KEY.to_string(),
            session_token: None,
        })
    } else if options_profile.is_none() {
        configured_bedrock_credentials(env)
    } else {
        None
    };

    // The SDK resolves a bare `profile` through the shared config/credentials files; without the
    // SDK that resolution happens here so an explicit/scoped profile is not silently unauthenticated.
    let credentials = credentials.or_else(|| {
        profile
            .as_deref()
            .and_then(|p| shared_profile_credentials(p, env))
    });

    // Likewise for the endpoint: upstream leaves `config.endpoint` unset and lets the SDK's
    // endpoint resolver build `https://bedrock-runtime.<region>.amazonaws.com`.
    let endpoint = if use_explicit_endpoint {
        base_url.trim_end_matches('/').to_string()
    } else {
        let r = region.clone().unwrap_or_else(|| "us-east-1".to_string());
        format!("https://bedrock-runtime.{r}.amazonaws.com")
    };

    BedrockClientConfig {
        profile,
        region,
        endpoint,
        credentials,
        bearer_token: if use_bearer_token { bearer_token } else { None },
    }
}

/// pi `getConfiguredBedrockRegion` (`bedrock-converse-stream.ts:979-986`).
fn configured_bedrock_region(bedrock: &BedrockOptions, env: &EnvSource<'_>) -> Option<String> {
    bedrock
        .region
        .clone()
        .filter(|v| !v.is_empty())
        .or_else(|| env.get("AWS_REGION"))
        .or_else(|| env.get("AWS_DEFAULT_REGION"))
}

/// pi `getConfiguredBedrockCredentials` (`bedrock-converse-stream.ts:988-1000`).
fn configured_bedrock_credentials(env: &EnvSource<'_>) -> Option<AwsCredentials> {
    let access_key_id = env.get("AWS_ACCESS_KEY_ID")?;
    let secret_access_key = env.get("AWS_SECRET_ACCESS_KEY")?;
    Some(AwsCredentials {
        access_key_id,
        secret_access_key,
        session_token: env.get("AWS_SESSION_TOKEN"),
    })
}

/// pi `getStandardBedrockEndpointRegion` (`bedrock-converse-stream.ts:1002-1014`): the region of a
/// `bedrock-runtime[-fips].<region>.amazonaws.com[.cn]` host, or `None` for any other host.
///
/// `[CYRUP-DELTA]` upstream applies the regex to `new URL(baseUrl).hostname`; cyrup has no `regex`
/// dependency, so the host is extracted by hand and the pattern is matched structurally. The
/// accepted set is identical: the `[a-z0-9-]+` region class is checked explicitly and a host with
/// extra labels (e.g. `bedrock-runtime.us-east-1.evil.amazonaws.com`) is rejected because the
/// suffix match is anchored.
fn standard_bedrock_endpoint_region(base_url: &str) -> Option<String> {
    let host = url_host(base_url)?.to_lowercase();
    let rest = host
        .strip_suffix(".amazonaws.com.cn")
        .or_else(|| host.strip_suffix(".amazonaws.com"))?;
    let region = rest
        .strip_prefix("bedrock-runtime-fips.")
        .or_else(|| rest.strip_prefix("bedrock-runtime."))?;
    if region.is_empty()
        || region.contains('.')
        || !region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    Some(region.to_string())
}

/// pi `shouldUseExplicitBedrockEndpoint` (`bedrock-converse-stream.ts:1016-1027`).
fn should_use_explicit_bedrock_endpoint(
    base_url: &str,
    configured_region: Option<&str>,
    has_ambient_profile: bool,
) -> bool {
    if standard_bedrock_endpoint_region(base_url).is_none() {
        return true;
    }
    configured_region.is_none() && !has_ambient_profile
}

/// pi's inline ARN region capture (`bedrock-converse-stream.ts:173`):
/// `/^arn:aws(?:-[a-z0-9-]+)?:bedrock:([a-z0-9-]+):/`.
///
/// `[CYRUP-DELTA]` hand-rolled for the same no-`regex` reason as above. Greedy scanning is exact
/// here: both capture classes are terminated by a literal `:`, which is not in either class.
fn arn_region(model_id: &str) -> Option<String> {
    let rest = model_id.strip_prefix("arn:aws")?;
    // `(?:-[a-z0-9-]+)?` then `:bedrock:`.
    let rest = match rest.strip_prefix(':') {
        Some(r) => r,
        None => {
            let partition = rest.strip_prefix('-')?;
            let end = partition
                .find(':')
                .filter(|i| *i > 0)
                .filter(|i| {
                    partition
                        .get(..*i)
                        .is_some_and(|p| p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
                })?;
            partition.get(end + 1..)?
        }
    };
    let rest = rest.strip_prefix("bedrock:")?;
    let end = rest.find(':').filter(|i| *i > 0)?;
    let region = rest.get(..end)?;
    if !region
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    Some(region.to_string())
}

/// Read static credentials for `profile` out of the shared credentials/config files.
///
/// This is the part of the SDK's default credential chain that a bare `profile` needs: upstream
/// sets `config.profile` and the SDK resolves it from `~/.aws/credentials` (falling back to
/// `[profile <name>]` in `~/.aws/config`). Honors `AWS_SHARED_CREDENTIALS_FILE` and
/// `AWS_CONFIG_FILE`, exactly as the SDK does. Role assumption / SSO / IMDS are **not** ported;
/// a profile that needs one resolves to `None` here and the request is sent unsigned-credentialed,
/// which surfaces as the provider's own auth error rather than a silent wrong-identity request.
fn shared_profile_credentials(profile: &str, env: &EnvSource<'_>) -> Option<AwsCredentials> {
    let home = env.get("HOME").or_else(|| env.get("USERPROFILE"));
    let credentials_path = env.get("AWS_SHARED_CREDENTIALS_FILE").or_else(|| {
        home.as_ref()
            .map(|h| format!("{}/.aws/credentials", h.trim_end_matches('/')))
    });
    let config_path = env.get("AWS_CONFIG_FILE").or_else(|| {
        home.as_ref()
            .map(|h| format!("{}/.aws/config", h.trim_end_matches('/')))
    });

    for (path, section) in [
        (credentials_path, profile.to_string()),
        (config_path, format!("profile {profile}")),
    ] {
        let Some(path) = path else { continue };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(creds) = parse_ini_profile(&text, &section) {
            return Some(creds);
        }
    }
    None
}

/// Extract `aws_access_key_id` / `aws_secret_access_key` / `aws_session_token` from one INI
/// section. Returns `None` unless both required keys are present.
fn parse_ini_profile(text: &str, section: &str) -> Option<AwsCredentials> {
    let mut in_section = false;
    let mut access_key_id: Option<String> = None;
    let mut secret_access_key: Option<String> = None;
    let mut session_token: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = name.trim() == section;
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim().to_ascii_lowercase().as_str() {
            "aws_access_key_id" => access_key_id = Some(value),
            "aws_secret_access_key" => secret_access_key = Some(value),
            "aws_session_token" => session_token = Some(value),
            _ => {}
        }
    }

    Some(AwsCredentials {
        access_key_id: access_key_id.filter(|v| !v.is_empty())?,
        secret_access_key: secret_access_key.filter(|v| !v.is_empty())?,
        session_token: session_token.filter(|v| !v.is_empty()),
    })
}

// ---------------------------------------------------------------------------
// Headers (pi addCustomHeadersMiddleware, bedrock-converse-stream.ts:373-401)
// ---------------------------------------------------------------------------

/// pi `RESERVED_HEADER_EXACT` (`bedrock-converse-stream.ts:373`).
const RESERVED_HEADER_EXACT: [&str; 2] = ["authorization", "host"];

/// pi `isReservedHeader` (`bedrock-converse-stream.ts:375-378`): case-insensitive, and every
/// `x-amz-*` key is reserved because it participates in the SigV4 canonical request.
fn is_reserved_header(key: &str) -> bool {
    let lower = key.to_lowercase();
    lower.starts_with("x-amz-") || RESERVED_HEADER_EXACT.contains(&lower.as_str())
}

/// pi `providerHeadersToRecord` + `addCustomHeadersMiddleware` (`headers.ts:10-17`,
/// `bedrock-converse-stream.ts:387-401`), collapsed: a `None` value drops the entry (pi's
/// `value !== null` filter), reserved keys are skipped, and every other caller header **overrides**
/// any same-named header already on the request. Keys are lower-cased so a mixed-case reserved key
/// cannot slip back in as a distinct header (pi's VC2 case).
///
/// `model.headers` sits below `opts.headers`, matching cyrup's documented overlay order
/// (auth < `model.headers` < `opts.headers`).
fn apply_custom_headers(
    headers: &mut BTreeMap<String, String>,
    request_headers: Option<&HeaderMap>,
    model_headers: Option<&HeaderMap>,
) {
    for source in [model_headers, request_headers].into_iter().flatten() {
        for (key, value) in source {
            if is_reserved_header(key) {
                continue;
            }
            match value {
                Some(v) => {
                    headers.insert(key.to_lowercase(), v.clone());
                }
                None => {
                    headers.remove(&key.to_lowercase());
                }
            }
        }
    }
}

/// Install the `Authorization` header: `Bearer <token>` when a bearer token is configured (pi
/// `config.token` + `authSchemePreference: ["httpBearerAuth"]`, `:217-220`), otherwise SigV4.
fn authorize(
    headers: &mut BTreeMap<String, String>,
    config: &BedrockClientConfig,
    url: &str,
    body: &[u8],
) -> Result<(), String> {
    if let Some(token) = &config.bearer_token {
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
        return Ok(());
    }
    let Some(creds) = &config.credentials else {
        // The SDK would raise `CredentialsProviderError` here. Surface the same category of
        // failure rather than sending an unsigned request that Bedrock answers with an opaque 403.
        return Err(format!(
            "Could not load credentials from any providers{}",
            config
                .profile
                .as_deref()
                .map(|p| format!(" (profile \"{p}\")"))
                .unwrap_or_default()
        ));
    };
    let region = config.region.clone().unwrap_or_else(|| "us-east-1".into());
    sign_sigv4(headers, url, body, creds, &region, now_unix_seconds())
}

// ---------------------------------------------------------------------------
// Request encoding (pi commandInput, bedrock-converse-stream.ts:230-241)
// ---------------------------------------------------------------------------

/// The `ConverseStream` REST endpoint for `model_id` (the SDK's URI binding:
/// `POST /model/{modelId}/converse-stream`, with `modelId` percent-encoded because inference-profile
/// ARNs contain `:` and `/`).
fn converse_stream_url(endpoint: &str, model_id: &str) -> String {
    format!(
        "{}/model/{}/converse-stream",
        endpoint.trim_end_matches('/'),
        uri_encode(model_id, false)
    )
}

/// Build the `ConverseStreamCommand` input (pi `commandInput`,
/// `bedrock-converse-stream.ts:230-241`), including the `modelId` URI label so `onPayload` sees the
/// same object upstream hands it. [`split_command_input`] lifts `modelId` back out afterwards.
///
/// Returns `Err(message)` for the one throwing path upstream has on this route:
/// `createImageBlock`'s `Unknown image type: <mimeType>` (`:1106`).
fn build_params(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    bedrock: &BedrockOptions,
    cache_retention: CacheRetention,
    env: &EnvSource<'_>,
) -> Result<Value, String> {
    let claude = is_anthropic_claude_model(model);
    let adaptive = supports_adaptive_thinking(model);
    let reasoning_on = opts.reasoning.is_on();

    // pi `streamSimple` (`:403-449`): only budget-based Claude models re-split `maxTokens` between
    // thinking and output; adaptive Claude and every non-Claude model pass the base cap through.
    let mut effective_max_tokens = opts.max_tokens;
    let mut budget_override: Option<u64> = None;
    if reasoning_on && claude && !adaptive {
        let level = opts.reasoning.level().unwrap_or(ThinkingLevel::High);
        let (adjusted, budget) = adjust_max_tokens_for_thinking(
            opts.max_tokens,
            model.max_tokens,
            level,
            opts.thinking_budgets.as_ref(),
        );
        let max_tokens = clamp_max_tokens_to_context(model, ctx, adjusted);
        effective_max_tokens = Some(max_tokens);
        budget_override = Some(budget.min(max_tokens.saturating_sub(1024)));
    }

    // pi `:229`: `options.maxTokens ?? (isAnthropicClaudeModel(model) ? model.maxTokens : undefined)`.
    let inference_max_tokens = effective_max_tokens.or(if claude {
        Some(model.max_tokens)
    } else {
        None
    });

    let mut obj = Map::new();
    obj.insert("modelId".to_string(), json!(model.id.as_str()));
    obj.insert(
        "messages".to_string(),
        Value::Array(convert_messages(ctx, model, cache_retention, env)?),
    );
    if let Some(system) = build_system_prompt(ctx.system_prompt.as_deref(), model, cache_retention, env) {
        obj.insert("system".to_string(), Value::Array(system));
    }

    let mut inference = Map::new();
    if let Some(max_tokens) = inference_max_tokens {
        inference.insert("maxTokens".to_string(), json!(max_tokens));
    }
    if let Some(temperature) = opts.temperature {
        inference.insert("temperature".to_string(), json!(temperature));
    }
    obj.insert("inferenceConfig".to_string(), Value::Object(inference));

    // pi `:238` reads `model.compat?.supportsStrictMode ?? false` at the call site.
    let supports_strict_mode = model
        .compat
        .as_ref()
        .and_then(|c| c.supports_strict_mode)
        .unwrap_or(false);
    if let Some(tool_config) =
        convert_tool_config(&ctx.tools, bedrock.tool_choice.as_ref(), supports_strict_mode)
            .map_err(|e| e.0)?
    {
        obj.insert("toolConfig".to_string(), tool_config);
    }
    if let Some(extra) =
        build_additional_model_request_fields(model, opts, bedrock, env, budget_override)
    {
        obj.insert("additionalModelRequestFields".to_string(), extra);
    }
    if let Some(metadata) = &bedrock.request_metadata {
        let map: Map<String, Value> = metadata
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        obj.insert("requestMetadata".to_string(), Value::Object(map));
    }

    Ok(Value::Object(obj))
}

/// pi `resolveCacheRetention` (`bedrock-converse-stream.ts:640-648`): explicit wins, else
/// `PI_CACHE_RETENTION=long` promotes, else `"short"`.
fn resolve_cache_retention(
    cache_retention: Option<CacheRetention>,
    env: &EnvSource<'_>,
) -> CacheRetention {
    if let Some(c) = cache_retention {
        return c;
    }
    if env.get("PI_CACHE_RETENTION").as_deref() == Some("long") {
        return CacheRetention::Long;
    }
    CacheRetention::Short
}

/// The `cachePoint` block for the resolved retention (pi `:724-726` / `:912-919`).
///
/// The `ttl` value is the SDK's `CacheTTL.ONE_HOUR`, whose wire form is Bedrock's `"1h"` — the same
/// spelling Anthropic's own `cache_control.ttl` uses and which cyrup's `anthropic-messages` port
/// already emits.
fn cache_point(cache_retention: CacheRetention) -> Value {
    let mut point = Map::new();
    point.insert("type".to_string(), json!("default"));
    if cache_retention == CacheRetention::Long {
        point.insert("ttl".to_string(), json!("1h"));
    }
    json!({ "cachePoint": Value::Object(point) })
}

/// pi `getModelMatchCandidates` (`bedrock-converse-stream.ts:580-586`): for the model id and (when
/// present) the model name, the lower-cased value plus the value with every run of `[\s_.:]`
/// collapsed to a single `-`.
fn model_match_candidates(model: &Model) -> Vec<String> {
    let mut values = vec![model.id.as_str().to_lowercase()];
    if !model.name.is_empty() {
        values.push(model.name.to_lowercase());
    }
    let mut out = Vec::with_capacity(values.len() * 2);
    for value in values {
        let mut dashed = String::with_capacity(value.len());
        let mut in_run = false;
        for ch in value.chars() {
            if ch.is_whitespace() || ch == '_' || ch == '.' || ch == ':' {
                if !in_run {
                    dashed.push('-');
                    in_run = true;
                }
            } else {
                dashed.push(ch);
                in_run = false;
            }
        }
        out.push(value);
        out.push(dashed);
    }
    out
}

/// pi `supportsAdaptiveThinking` (`bedrock-converse-stream.ts:588-600`).
fn supports_adaptive_thinking(model: &Model) -> bool {
    const NEEDLES: [&str; 7] = [
        "opus-4-6", "opus-4-7", "opus-4-8", "opus-5", "sonnet-4-6", "sonnet-5", "fable-5",
    ];
    let candidates = model_match_candidates(model);
    candidates
        .iter()
        .any(|s| NEEDLES.iter().any(|n| s.contains(n)))
}

/// pi `supportsNativeXhighEffort` (`bedrock-converse-stream.ts:602-612`).
fn supports_native_xhigh_effort(model: &Model) -> bool {
    const NEEDLES: [&str; 5] = ["opus-4-7", "opus-4-8", "opus-5", "sonnet-5", "fable-5"];
    let candidates = model_match_candidates(model);
    candidates
        .iter()
        .any(|s| NEEDLES.iter().any(|n| s.contains(n)))
}

/// pi `mapThinkingLevelToEffort` (`bedrock-converse-stream.ts:614-634`). Note the switch has no
/// `xhigh`/`max` arm, so both fall to `default: "high"` unless the model natively supports `xhigh`
/// or a `thinkingLevelMap` entry overrides.
fn map_thinking_level_to_effort(model: &Model, level: ThinkingLevel) -> String {
    if level == ThinkingLevel::Xhigh && supports_native_xhigh_effort(model) {
        return "xhigh".to_string();
    }
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
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => "high".to_string(),
    }
}

/// pi `isAnthropicClaudeModel` (`bedrock-converse-stream.ts:655-665`).
fn is_anthropic_claude_model(model: &Model) -> bool {
    let id = model.id.as_str().to_lowercase();
    let name = model.name.to_lowercase();
    id.contains("anthropic.claude")
        || id.contains("anthropic/claude")
        || name.contains("anthropic.claude")
        || name.contains("anthropic/claude")
        || name.contains("claude")
}

/// pi `supportsPromptCaching` (`bedrock-converse-stream.ts:679-698`).
fn supports_prompt_caching(model: &Model, env: &EnvSource<'_>) -> bool {
    let candidates = model_match_candidates(model);
    let has_claude_ref = candidates.iter().any(|s| s.contains("claude"));
    if !has_claude_ref {
        return env.get("AWS_BEDROCK_FORCE_CACHE").as_deref() == Some("1");
    }
    let any = |needles: &[&str]| {
        candidates
            .iter()
            .any(|s| needles.iter().any(|n| s.contains(n)))
    };
    // Claude 5, then Claude 4.x, then Claude 3.7 Sonnet, then Claude 3.5 Haiku.
    any(&["fable-5", "opus-5", "sonnet-5"])
        || any(&["-4-"])
        || any(&["claude-3-7-sonnet"])
        || any(&["claude-3-5-haiku"])
}

/// pi `supportsThinkingSignature` (`bedrock-converse-stream.ts:708-710`): only Anthropic Claude
/// models accept `reasoningContent.reasoningText.signature`.
fn supports_thinking_signature(model: &Model) -> bool {
    is_anthropic_claude_model(model)
}

/// pi `isGovCloudBedrockTarget` (`bedrock-converse-stream.ts:1029-1037`).
fn is_gov_cloud_bedrock_target(model: &Model, bedrock: &BedrockOptions, env: &EnvSource<'_>) -> bool {
    if let Some(region) = configured_bedrock_region(bedrock, env)
        && region.to_lowercase().starts_with("us-gov-")
    {
        return true;
    }
    let id = model.id.as_str().to_lowercase();
    id.starts_with("us-gov.") || id.starts_with("arn:aws-us-gov:")
}

/// pi `buildSystemPrompt` (`bedrock-converse-stream.ts:712-730`).
fn build_system_prompt(
    system_prompt: Option<&str>,
    model: &Model,
    cache_retention: CacheRetention,
    env: &EnvSource<'_>,
) -> Option<Vec<Value>> {
    let system_prompt = system_prompt.filter(|s| !s.is_empty())?;
    let mut blocks = vec![json!({ "text": sanitize_surrogates(system_prompt) })];
    if cache_retention != CacheRetention::None && supports_prompt_caching(model, env) {
        blocks.push(cache_point(cache_retention));
    }
    Some(blocks)
}

/// pi `normalizeToolCallId` (`bedrock-converse-stream.ts:732-735`): every character outside
/// `[a-zA-Z0-9_-]` becomes `_`, then the id is capped at 64 characters.
fn normalize_tool_call_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.chars().count() > 64 {
        sanitized.chars().take(64).collect()
    } else {
        sanitized
    }
}

/// pi `createNonBlankTextBlock` (`bedrock-converse-stream.ts:737-740`).
fn non_blank_text_block(text: &str) -> Option<Value> {
    let sanitized = sanitize_surrogates(text);
    if sanitized.trim().is_empty() {
        None
    } else {
        Some(json!({ "text": sanitized }))
    }
}

/// pi `createRequiredTextBlock` (`bedrock-converse-stream.ts:742-744`).
fn required_text_block(text: &str) -> Value {
    non_blank_text_block(text).unwrap_or_else(|| json!({ "text": EMPTY_TEXT_PLACEHOLDER }))
}

/// pi `createImageBlock` (`bedrock-converse-stream.ts:1089-1116`).
///
/// Upstream decodes the base64 with `atob` and hands the SDK a `Uint8Array`; the REST binding then
/// re-encodes it as base64, so the bytes on the wire are the same. The decode is still performed
/// here because it is the check that makes upstream's `atob` throw on a malformed payload, and the
/// canonical re-encode normalises whitespace/padding the same way the SDK's serializer does.
fn create_image_block(mime_type: &str, data: &str) -> Result<Value, String> {
    let format = match mime_type {
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        other => return Err(format!("Unknown image type: {other}")),
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|_| "The string to be decoded contains invalid characters.".to_string())?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(json!({ "image": { "format": format, "source": { "bytes": encoded } } }))
}

/// pi `convertToolResultContent` (`bedrock-converse-stream.ts:746-758`).
fn convert_tool_result_content(content: &[Content]) -> Result<Vec<Value>, String> {
    let mut result = Vec::new();
    for c in content {
        match c {
            Content::Image { data, mime_type } => {
                result.push(create_image_block(mime_type, data)?);
            }
            Content::Text { text, .. } => {
                if let Some(block) = non_blank_text_block(text) {
                    result.push(block);
                }
            }
            // `ToolResultMessage.content` is `(TextContent | ImageContent)[]` upstream, and cyrup's
            // deserializer enforces the same, so the remaining variants are unreachable — upstream's
            // `else` branch treats anything non-image as text, which cannot fire here.
            _ => {}
        }
    }
    if result.is_empty() {
        result.push(json!({ "text": EMPTY_TEXT_PLACEHOLDER }));
    }
    Ok(result)
}

/// pi `convertMessages` (`bedrock-converse-stream.ts:760-923`).
fn convert_messages(
    ctx: &Context,
    model: &Model,
    cache_retention: CacheRetention,
    env: &EnvSource<'_>,
) -> Result<Vec<Value>, String> {
    let transformed = transform_messages_with(&ctx.messages, model, normalize_tool_call_id);
    let mut result: Vec<Value> = Vec::new();

    let mut i = 0usize;
    while i < transformed.len() {
        let Some(m) = transformed.get(i) else { break };
        match m {
            Message::User { content, .. } => {
                let mut blocks: Vec<Value> = Vec::new();
                for c in content {
                    match c {
                        Content::Text { text, .. } => {
                            if let Some(block) = non_blank_text_block(text) {
                                blocks.push(block);
                            }
                        }
                        Content::Image { data, mime_type } => {
                            blocks.push(create_image_block(mime_type, data)?);
                        }
                        // pi's `default: continue` — an unknown block is skipped, never fatal.
                        _ => {}
                    }
                }
                if blocks.is_empty() {
                    blocks.push(required_text_block(""));
                }
                result.push(json!({ "role": "user", "content": blocks }));
                i += 1;
            }
            Message::Assistant(assistant) => {
                // pi `:803-805`: Bedrock rejects an empty assistant content array.
                if assistant.content.is_empty() {
                    i += 1;
                    continue;
                }
                let mut blocks: Vec<Value> = Vec::new();
                for c in &assistant.content {
                    match c {
                        Content::Text { text, .. } => {
                            if let Some(block) = non_blank_text_block(text) {
                                blocks.push(block);
                            }
                        }
                        Content::ToolCall(tc) => {
                            blocks.push(json!({
                                "toolUse": {
                                    "toolUseId": tc.id.as_str(),
                                    "name": tc.name,
                                    "input": Value::Object(tc.arguments.clone()),
                                }
                            }));
                        }
                        Content::Thinking {
                            thinking,
                            thinking_signature,
                            ..
                        } => {
                            let thinking = sanitize_surrogates(thinking);
                            if thinking.trim().is_empty() {
                                continue;
                            }
                            if supports_thinking_signature(model) {
                                // pi `:830-843`: a replayed reasoning block without a signature is
                                // rejected by Bedrock, so it degrades to plain text.
                                match thinking_signature.as_deref().filter(|s| !s.trim().is_empty())
                                {
                                    Some(sig) => blocks.push(json!({
                                        "reasoningContent": {
                                            "reasoningText": { "text": thinking, "signature": sig }
                                        }
                                    })),
                                    None => blocks.push(json!({ "text": thinking })),
                                }
                            } else {
                                blocks.push(json!({
                                    "reasoningContent": { "reasoningText": { "text": thinking } }
                                }));
                            }
                        }
                        _ => {}
                    }
                }
                if blocks.is_empty() {
                    i += 1;
                    continue;
                }
                result.push(json!({ "role": "assistant", "content": blocks }));
                i += 1;
            }
            Message::ToolResult { .. } => {
                // pi `:867-903`: every RUN of consecutive tool results collapses into ONE user
                // message, because Bedrock requires all results for a turn in a single message.
                let mut tool_results: Vec<Value> = Vec::new();
                while let Some(Message::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                    ..
                }) = transformed.get(i)
                {
                    tool_results.push(json!({
                        "toolResult": {
                            "toolUseId": tool_call_id.as_str(),
                            "content": convert_tool_result_content(content)?,
                            "status": if *is_error { "error" } else { "success" },
                        }
                    }));
                    i += 1;
                }
                result.push(json!({ "role": "user", "content": tool_results }));
            }
        }
    }

    // pi `:909-920`: the cache point goes on the LAST message, and only when it is a user message.
    if cache_retention != CacheRetention::None
        && supports_prompt_caching(model, env)
        && let Some(last) = result.last_mut()
        && last.get("role").and_then(Value::as_str) == Some("user")
        && let Some(Value::Array(content)) = last.get_mut("content")
    {
        content.push(cache_point(cache_retention));
    }

    Ok(result)
}

/// pi `convertToolConfig` (`bedrock-converse-stream.ts:925-960` @**v0.83.0**).
///
/// PROV-011 — `strict: true` is emitted only when `resolveJsonSchemaStrictSampling` resolves it
/// (`:934`, `:940`), against `model.compat?.supportsStrictMode ?? false` read at `:238`.
fn convert_tool_config(
    tools: &[ToolDef],
    tool_choice: Option<&BedrockToolChoice>,
    supports_strict_mode: bool,
) -> Result<Option<Value>, ConstrainedSamplingError> {
    if tools.is_empty() {
        return Ok(None);
    }
    if matches!(tool_choice, Some(BedrockToolChoice::None)) {
        return Ok(None);
    }
    let bedrock_tools: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode)?;
            let mut spec = Map::new();
            spec.insert("name".to_string(), json!(tool.name));
            spec.insert("description".to_string(), json!(tool.description));
            spec.insert(
                "inputSchema".to_string(),
                json!({ "json": tool.parameters }),
            );
            if strict == Some(true) {
                spec.insert("strict".to_string(), json!(true));
            }
            Ok(json!({ "toolSpec": Value::Object(spec) }))
        })
        .collect::<Result<Vec<Value>, ConstrainedSamplingError>>()?;

    let mut config = Map::new();
    config.insert("tools".to_string(), Value::Array(bedrock_tools));
    if let Some(choice) = tool_choice.and_then(BedrockToolChoice::to_wire) {
        config.insert("toolChoice".to_string(), choice);
    }
    Ok(Some(Value::Object(config)))
}

/// pi `buildAdditionalModelRequestFields` (`bedrock-converse-stream.ts:1039-1087`).
fn build_additional_model_request_fields(
    model: &Model,
    opts: &StreamOptions,
    bedrock: &BedrockOptions,
    env: &EnvSource<'_>,
    budget_override: Option<u64>,
) -> Option<Value> {
    if !opts.reasoning.is_on() || !model.reasoning {
        return None;
    }
    if !is_anthropic_claude_model(model) {
        return None;
    }
    let level = opts.reasoning.level().unwrap_or(ThinkingLevel::High);

    // pi `:1048-1050`: GovCloud's Converse schema rejects `thinking.display`.
    let display = if is_gov_cloud_bedrock_target(model, bedrock, env) {
        None
    } else {
        Some(
            bedrock
                .thinking_display
                .map(BedrockThinkingDisplay::as_wire)
                .unwrap_or("summarized"),
        )
    };

    let adaptive = supports_adaptive_thinking(model);
    let mut result = Map::new();
    if adaptive {
        let mut thinking = Map::new();
        thinking.insert("type".to_string(), json!("adaptive"));
        if let Some(display) = display {
            thinking.insert("display".to_string(), json!(display));
        }
        result.insert("thinking".to_string(), Value::Object(thinking));
        result.insert(
            "output_config".to_string(),
            json!({ "effort": map_thinking_level_to_effort(model, level) }),
        );
    } else {
        let budget = budget_override.unwrap_or_else(|| default_thinking_budget(level, opts));
        let mut thinking = Map::new();
        thinking.insert("type".to_string(), json!("enabled"));
        thinking.insert("budget_tokens".to_string(), json!(budget));
        if let Some(display) = display {
            thinking.insert("display".to_string(), json!(display));
        }
        result.insert("thinking".to_string(), Value::Object(thinking));
        // pi `:1079-1081`: the interleaved-thinking beta rides only the budget-based branch.
        if bedrock.interleaved_thinking.unwrap_or(true) {
            result.insert(
                "anthropic_beta".to_string(),
                json!([INTERLEAVED_THINKING_BETA]),
            );
        }
    }
    Some(Value::Object(result))
}

/// pi's inline `defaultBudgets` table plus the custom-budget lookup
/// (`bedrock-converse-stream.ts:1057-1068`).
///
/// The custom lookup uses the CLAMPED level (`xhigh`/`max` → `high`, because custom budgets only
/// cover the token-based rungs) while the default table is keyed by the ORIGINAL level — which is
/// why `xhigh` and `max` both default to 16384 rather than falling back to `high`'s entry.
fn default_thinking_budget(level: ThinkingLevel, opts: &StreamOptions) -> u64 {
    let budgets = opts.thinking_budgets.as_ref();
    let custom = match level {
        ThinkingLevel::Minimal => budgets.and_then(|b| b.minimal),
        ThinkingLevel::Low => budgets.and_then(|b| b.low),
        ThinkingLevel::Medium => budgets.and_then(|b| b.medium),
        // `xhigh`/`max` clamp to `high` for the custom lookup.
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => {
            budgets.and_then(|b| b.high)
        }
    };
    custom.unwrap_or(match level {
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 2048,
        ThinkingLevel::Medium => 8192,
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => 16384,
    })
}

// ---------------------------------------------------------------------------
// Errors (pi formatBedrockError, bedrock-converse-stream.ts:326-365)
// ---------------------------------------------------------------------------

/// pi `BEDROCK_ERROR_PREFIXES` (`bedrock-converse-stream.ts:326-332`). The prefixes are legacy and
/// load-bearing: the turn-level retry classifier matches `server.?error` / `service.?unavailable`
/// against this string, so the raw SDK exception name must not be used instead.
fn bedrock_error_prefix(name: &str) -> Option<&'static str> {
    match name {
        "InternalServerException" => Some("Internal server error"),
        "ModelStreamErrorException" => Some("Model stream error"),
        "ValidationException" => Some("Validation error"),
        "ThrottlingException" => Some("Throttling error"),
        "ServiceUnavailableException" => Some("Service unavailable"),
        _ => None,
    }
}

/// pi's data-retention hint (`bedrock-converse-stream.ts:357-359`), appended whenever the core
/// message mentions a data retention mode (case-insensitively).
fn data_retention_hint(core: &str) -> String {
    if core.to_lowercase().contains("data retention mode") {
        format!(" See {BEDROCK_DATA_RETENTION_DOCS_URL} for supported data retention modes.")
    } else {
        String::new()
    }
}

/// pi `formatBedrockError` for a non-SDK error (`bedrock-converse-stream.ts:364`): the message plus
/// the data-retention hint, with no prefix.
fn format_bedrock_error(message: &str) -> String {
    format!("{message}{}", data_retention_hint(message))
}

/// pi `formatBedrockError` for a `BedrockRuntimeServiceException` (`:360-363`).
///
/// The SDK folds an HTTP error into an exception whose `.name` is the modeled shape name and whose
/// `.message` is the body's `message` field. Here that arrives as the `x-amzn-errortype` header plus
/// the response body, so `core` is composed the way `normalizeProviderError` composes it when the
/// message does NOT already carry the body: `"<status>: <body>"` (`:353-356`). An unmodeled error
/// type falls back to the raw name, exactly as upstream's `?? error.name` does.
fn format_bedrock_service_error(name: &str, status: u16, body: &str) -> String {
    let core = if body.is_empty() {
        format!("{status}")
    } else {
        format!("{status}: {body}")
    };
    let hint = data_retention_hint(&core);
    match bedrock_error_prefix(name) {
        Some(prefix) => format!("{prefix}: {core}{hint}"),
        None if !name.is_empty() => format!("{name}: {core}{hint}"),
        None => format!("{core}{hint}"),
    }
}

/// pi `mapStopReason` (`bedrock-converse-stream.ts:962-977`).
///
/// Returns `(stopReason, errorMessage)` — the diagnostic is inseparable from the mapping: without
/// it a guardrail/content-filter stop would land on the generic `"An unknown error occurred"`
/// fallback and become indistinguishable from a transport failure.
fn map_stop_reason(reason: Option<&str>) -> (StopReason, Option<String>) {
    match reason {
        Some("end_turn") | Some("stop_sequence") => (StopReason::Stop, None),
        Some("max_tokens") | Some("model_context_window_exceeded") => (StopReason::Length, None),
        Some("tool_use") => (StopReason::ToolUse, None),
        Some(other) if !other.is_empty() => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
        _ => (StopReason::Error, None),
    }
}

// ---------------------------------------------------------------------------
// Response decoding (pi handleContentBlock*, bedrock-converse-stream.ts:451-573)
// ---------------------------------------------------------------------------

/// One in-progress content block, keyed by Bedrock's `contentBlockIndex` (pi's `Block` type,
/// `bedrock-converse-stream.ts:102`). `index` and `partial_json` are the streaming scratch fields
/// upstream `delete`s before the message escapes; here they are separate struct fields that the
/// snapshot never projects, so there is nothing to strip.
enum Block {
    Text {
        index: i64,
        text: String,
    },
    Thinking {
        index: i64,
        thinking: String,
        signature: String,
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

/// Streaming-decode state (pi's `output` accumulation, `bedrock-converse-stream.ts:114-132`).
#[derive(Default)]
struct Decoder {
    blocks: Vec<Block>,
    usage: Usage,
    stop_reason: Option<StopReason>,
    /// The provider's own `messageStop.stopReason`, kept verbatim beside the narrowed
    /// [`StopReason`] (pi `output.rawStopReason = item.messageStop.stopReason`,
    /// `v0.84.1 ai/src/api/bedrock-converse-stream.ts:276`). PORT BUG, not version lag: the write is
    /// present at v0.83.0 too (`v0.83.0 ai/src/api/bedrock-converse-stream.ts:270`) and cyrup never
    /// ported it. Assigned UNCONDITIONALLY at `messageStop` — pi has no truthiness guard there, so a
    /// `messageStop` with no `stopReason` writes `undefined`, i.e. `None`.
    raw_stop_reason: Option<String>,
    error_message: Option<String>,
}

impl Decoder {
    fn position_of(&self, index: i64) -> Option<usize> {
        self.blocks.iter().position(|b| b.index() == index)
    }

    /// Build the live `partial` snapshot. `calculateCost` fills only `usage.cost` upstream
    /// (`:543`), and `handleMetadata` sets `totalTokens` from the provider's own figure
    /// (`:542`), so `total_tokens` is NOT recomputed here.
    fn snapshot(&self, model: &Model, api: &ApiId) -> AssistantMessage {
        let mut usage = self.usage.clone();
        usage.cost = compute_cost(&model.cost, &usage);
        AssistantMessage {
            content: blocks_to_content(&self.blocks),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: api.clone(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage,
            // pi seeds `output.stopReason = "pending"` (`:128`) and that seed IS the `partial`
            // attached to every non-terminal event. The terminal never takes this value — it goes
            // through `StreamEvent::end_of_stream`.
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
                ..
            } => Content::Thinking {
                thinking: thinking.clone(),
                thinking_signature: if signature.is_empty() {
                    None
                } else {
                    Some(signature.clone())
                },
                redacted: false,
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

/// Dispatch one decoded event-stream frame (pi's `for await (const item of response.stream!)` body,
/// `bedrock-converse-stream.ts:257-289`).
///
/// `Ok(false)` means the consumer dropped the stream; `Err(message)` is one of upstream's five
/// `throw item.<x>Exception` arms.
async fn dispatch_frame(
    frame: &EventFrame,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> Result<bool, String> {
    // An `:message-type: exception` frame is upstream's `item.<x>Exception` throw.
    if frame.header(":message-type").as_deref() == Some("exception") {
        let name = frame
            .header(":exception-type")
            .map(|t| upper_first(&t))
            .unwrap_or_else(|| "BedrockRuntimeServiceException".to_string());
        let message = frame
            .json()
            .and_then(|v| {
                v.get("message")
                    .or_else(|| v.get("Message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let core = if message.is_empty() {
            name.clone()
        } else {
            message
        };
        let hint = data_retention_hint(&core);
        let prefix = bedrock_error_prefix(&name).unwrap_or(name.as_str());
        return Err(format!("{prefix}: {core}{hint}"));
    }

    let Some(event_type) = frame.header(":event-type") else {
        return Ok(true);
    };
    let payload = frame.json().unwrap_or(Value::Null);

    match event_type.as_str() {
        "messageStart" => {
            // pi `:258-262`: a non-assistant role is fatal.
            if payload.get("role").and_then(Value::as_str) != Some("assistant") {
                return Err(format_bedrock_error(
                    "Unexpected assistant message start but got user message start instead",
                ));
            }
            Ok(sink
                .send(StreamEvent::Start {
                    partial: dec.snapshot(model, api),
                })
                .await)
        }
        "contentBlockStart" => Ok(handle_content_block_start(&payload, dec, model, api, sink).await),
        "contentBlockDelta" => Ok(handle_content_block_delta(&payload, dec, model, api, sink).await),
        "contentBlockStop" => Ok(handle_content_block_stop(&payload, dec, model, api, sink).await),
        "messageStop" => {
            let raw = payload.get("stopReason").and_then(Value::as_str);
            // pi `output.rawStopReason = item.messageStop.stopReason` (`v0.84.1
            // ai/src/api/bedrock-converse-stream.ts:276`) — recorded before the narrowing map, so
            // `guardrail_intervened` and every future reason name themselves on the turn.
            dec.raw_stop_reason = raw.map(str::to_string);
            let (stop_reason, error_message) = map_stop_reason(raw);
            dec.stop_reason = Some(stop_reason);
            if let Some(message) = error_message {
                dec.error_message = Some(message);
            }
            Ok(true)
        }
        "metadata" => {
            handle_metadata(&payload, dec);
            Ok(true)
        }
        _ => Ok(true),
    }
}

/// pi `handleContentBlockStart` (`bedrock-converse-stream.ts:451-472`). Only `toolUse` starts a
/// block; text and reasoning blocks are created lazily by the first delta.
async fn handle_content_block_start(
    payload: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = payload
        .get("contentBlockIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let Some(tool_use) = payload.get("start").and_then(|s| s.get("toolUse")) else {
        return true;
    };
    dec.blocks.push(Block::Tool {
        index,
        id: tool_use
            .get("toolUseId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        name: tool_use
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        partial_json: String::new(),
    });
    let content_index = dec.blocks.len().saturating_sub(1);
    sink.send(StreamEvent::ToolCallStart {
        content_index,
        partial: dec.snapshot(model, api),
    })
    .await
}

/// pi `handleContentBlockDelta` (`bedrock-converse-stream.ts:474-530`).
async fn handle_content_block_delta(
    payload: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = payload
        .get("contentBlockIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let delta = payload.get("delta");
    let position = dec.position_of(index);

    if let Some(text) = delta.and_then(|d| d.get("text")).and_then(Value::as_str) {
        let position = match position {
            Some(p) => p,
            None => {
                // pi `:486-493`: no `contentBlockStart` is sent for text blocks.
                dec.blocks.push(Block::Text {
                    index,
                    text: String::new(),
                });
                let content_index = dec.blocks.len().saturating_sub(1);
                if !sink
                    .send(StreamEvent::TextStart {
                        content_index,
                        partial: dec.snapshot(model, api),
                    })
                    .await
                {
                    return false;
                }
                content_index
            }
        };
        if let Some(Block::Text { text: buf, .. }) = dec.blocks.get_mut(position) {
            buf.push_str(text);
        } else {
            return true;
        }
        return sink
            .send(StreamEvent::TextDelta {
                content_index: position,
                delta: text.to_string(),
                partial: dec.snapshot(model, api),
            })
            .await;
    }

    if let Some(tool_use) = delta.and_then(|d| d.get("toolUse")) {
        let Some(position) = position else {
            return true;
        };
        let input = tool_use
            .get("input")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match dec.blocks.get_mut(position) {
            Some(Block::Tool { partial_json, .. }) => partial_json.push_str(&input),
            // pi guards on `block?.type === "toolCall"`; any other block type is ignored.
            _ => return true,
        }
        return sink
            .send(StreamEvent::ToolCallDelta {
                content_index: position,
                delta: input,
                partial: dec.snapshot(model, api),
            })
            .await;
    }

    if let Some(reasoning) = delta.and_then(|d| d.get("reasoningContent")) {
        let position = match position {
            Some(p) => p,
            None => {
                dec.blocks.push(Block::Thinking {
                    index,
                    thinking: String::new(),
                    signature: String::new(),
                });
                let content_index = dec.blocks.len().saturating_sub(1);
                if !sink
                    .send(StreamEvent::ThinkingStart {
                        content_index,
                        partial: dec.snapshot(model, api),
                    })
                    .await
                {
                    return false;
                }
                content_index
            }
        };
        // pi `:514`: everything below is guarded on the block actually being a thinking block.
        if !matches!(dec.blocks.get(position), Some(Block::Thinking { .. })) {
            return true;
        }
        if let Some(text) = reasoning.get("text").and_then(Value::as_str)
            && !text.is_empty()
        {
            if let Some(Block::Thinking { thinking, .. }) = dec.blocks.get_mut(position) {
                thinking.push_str(text);
            }
            if !sink
                .send(StreamEvent::ThinkingDelta {
                    content_index: position,
                    delta: text.to_string(),
                    partial: dec.snapshot(model, api),
                })
                .await
            {
                return false;
            }
        }
        // pi `:524-527`: the signature accumulates silently — no event is emitted for it.
        if let Some(sig) = reasoning.get("signature").and_then(Value::as_str)
            && !sig.is_empty()
            && let Some(Block::Thinking { signature, .. }) = dec.blocks.get_mut(position)
        {
            signature.push_str(sig);
        }
    }

    true
}

/// pi `handleContentBlockStop` (`bedrock-converse-stream.ts:547-573`).
async fn handle_content_block_stop(
    payload: &Value,
    dec: &mut Decoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
) -> bool {
    let index = payload
        .get("contentBlockIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    // pi `:555`: an unknown index is a no-op, not an error.
    let Some(position) = dec.position_of(index) else {
        return true;
    };
    let event = match dec.blocks.get(position) {
        Some(Block::Text { text, .. }) => StreamEvent::TextEnd {
            content_index: position,
            content: text.clone(),
            partial: dec.snapshot(model, api),
        },
        Some(Block::Thinking { thinking, .. }) => StreamEvent::ThinkingEnd {
            content_index: position,
            content: thinking.clone(),
            partial: dec.snapshot(model, api),
        },
        Some(Block::Tool {
            id,
            name,
            partial_json,
            ..
        }) => StreamEvent::ToolCallEnd {
            content_index: position,
            tool_call: ToolCall {
                id: ToolCallId::from(id.as_str()),
                name: name.clone(),
                arguments: parse_streaming_json_object(Some(partial_json)),
                thought_signature: None,
            },
            partial: dec.snapshot(model, api),
        },
        None => return true,
    };
    sink.send(event).await
}

/// pi `handleMetadata` (`bedrock-converse-stream.ts:532-545`).
fn handle_metadata(payload: &Value, dec: &mut Decoder) {
    let Some(usage) = payload.get("usage") else {
        return;
    };
    let n = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    dec.usage.input = n("inputTokens");
    dec.usage.output = n("outputTokens");
    dec.usage.cache_read = n("cacheReadInputTokens");
    dec.usage.cache_write = n("cacheWriteInputTokens");
    let total = n("totalTokens");
    dec.usage.total_tokens = if total == 0 {
        dec.usage.input.saturating_add(dec.usage.output)
    } else {
        total
    };
}

// ---------------------------------------------------------------------------
// AWS event-stream framing (`application/vnd.amazon.eventstream`)
// ---------------------------------------------------------------------------

/// One decoded event-stream message: its headers and its (JSON) payload.
struct EventFrame {
    headers: BTreeMap<String, String>,
    payload: Vec<u8>,
}

impl EventFrame {
    fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).cloned()
    }

    fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.payload).ok()
    }
}

/// Incremental decoder for the AWS binary event-stream framing the SDK hides from upstream.
///
/// Frame layout (`vnd.amazon.eventstream`):
/// `[total_len u32][headers_len u32][prelude_crc u32][headers][payload][message_crc u32]`, all
/// big-endian. Both CRCs are CRC-32 (IEEE) and both are verified: a corrupted frame must not be
/// silently interpreted, because the SDK would have rejected it.
#[derive(Default)]
struct EventStreamDecoder {
    buffer: Vec<u8>,
}

/// The largest frame accepted, guarding a corrupt length prefix from provoking a huge allocation.
/// AWS's own limit for an event-stream message is 16 MiB.
const MAX_EVENT_FRAME_BYTES: usize = 16 * 1024 * 1024;

impl EventStreamDecoder {
    fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// Pop the next complete frame, or `Ok(None)` when more bytes are needed.
    fn next_frame(&mut self) -> Result<Option<EventFrame>, String> {
        if self.buffer.len() < 12 {
            return Ok(None);
        }
        let total_len = be_u32(&self.buffer, 0).ok_or("truncated event-stream prelude")? as usize;
        let headers_len = be_u32(&self.buffer, 4).ok_or("truncated event-stream prelude")? as usize;
        let prelude_crc = be_u32(&self.buffer, 8).ok_or("truncated event-stream prelude")?;
        if !(16..=MAX_EVENT_FRAME_BYTES).contains(&total_len) || headers_len > total_len - 16 {
            return Err(format!("invalid event-stream frame length {total_len}"));
        }
        if self.buffer.len() < total_len {
            return Ok(None);
        }
        let prelude = self.buffer.get(..8).ok_or("truncated event-stream prelude")?;
        if crc32(prelude) != prelude_crc {
            return Err("event-stream prelude checksum mismatch".to_string());
        }
        let message = self
            .buffer
            .get(..total_len - 4)
            .ok_or("truncated event-stream message")?;
        let message_crc =
            be_u32(&self.buffer, total_len - 4).ok_or("truncated event-stream message")?;
        if crc32(message) != message_crc {
            return Err("event-stream message checksum mismatch".to_string());
        }

        let headers_bytes = self
            .buffer
            .get(12..12 + headers_len)
            .ok_or("truncated event-stream headers")?
            .to_vec();
        let payload = self
            .buffer
            .get(12 + headers_len..total_len - 4)
            .ok_or("truncated event-stream payload")?
            .to_vec();
        self.buffer.drain(..total_len);

        Ok(Some(EventFrame {
            headers: parse_event_headers(&headers_bytes)?,
            payload,
        }))
    }
}

/// Read a big-endian `u32` at `offset`, or `None` when out of range (no indexing — the workspace
/// denies `clippy::indexing_slicing`).
fn be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    let mut buf = [0u8; 4];
    for (dst, src) in buf.iter_mut().zip(slice.iter()) {
        *dst = *src;
    }
    Some(u32::from_be_bytes(buf))
}

/// Decode the event-stream header block. Only string-valued headers are surfaced (the ones the
/// protocol uses for `:message-type` / `:event-type` / `:exception-type` / `:content-type`), but
/// every value type is *sized* correctly so the walk never desynchronises.
fn parse_event_headers(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let name_len = *bytes.get(i).ok_or("truncated event-stream header")? as usize;
        i += 1;
        let name = bytes.get(i..i + name_len).ok_or("truncated header name")?;
        let name = String::from_utf8_lossy(name).to_string();
        i += name_len;
        let value_type = *bytes.get(i).ok_or("truncated header type")?;
        i += 1;
        match value_type {
            // bool true / bool false — no payload.
            0 | 1 => {}
            // byte / short / integer / long.
            2 => i += 1,
            3 => i += 2,
            4 => i += 4,
            5 => i += 8,
            // byte array / string — u16 length prefix.
            6 | 7 => {
                let len = u16::from_be_bytes([
                    *bytes.get(i).ok_or("truncated header length")?,
                    *bytes.get(i + 1).ok_or("truncated header length")?,
                ]) as usize;
                i += 2;
                let value = bytes.get(i..i + len).ok_or("truncated header value")?;
                if value_type == 7 {
                    out.insert(name, String::from_utf8_lossy(value).to_string());
                }
                i += len;
            }
            // timestamp (i64 millis) / uuid (16 bytes).
            8 => i += 8,
            9 => i += 16,
            other => return Err(format!("unknown event-stream header type {other}")),
        }
    }
    Ok(out)
}

/// CRC-32 (IEEE 802.3, reflected, poly `0xEDB88320`) — the checksum the event-stream framing uses.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// SigV4 (the signing the SDK performs for upstream)
// ---------------------------------------------------------------------------

/// HMAC-SHA256 (RFC 2104) over cyrup's dependency-free SHA-256
/// ([`crate::auth::oauth::sha256`], itself written because the crate carries no hashing dependency).
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    use crate::auth::oauth::sha256::sha256;
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = sha256(key);
        for (dst, src) in padded.iter_mut().zip(digest.iter()) {
            *dst = *src;
        }
    } else {
        for (dst, src) in padded.iter_mut().zip(key.iter()) {
            *dst = *src;
        }
    }
    let mut inner = Vec::with_capacity(BLOCK + message.len());
    let mut outer = Vec::with_capacity(BLOCK + 32);
    for b in padded.iter() {
        inner.push(b ^ 0x36);
        outer.push(b ^ 0x5c);
    }
    inner.extend_from_slice(message);
    let inner_digest = sha256(&inner);
    outer.extend_from_slice(&inner_digest);
    sha256(&outer)
}

/// Lower-case hex.
fn hex(bytes: &[u8]) -> String {
    crate::auth::oauth::sha256::hex(bytes)
}

/// Percent-encode per AWS SigV4's `UriEncode`: everything outside `A-Za-z0-9-._~` is escaped, with
/// `/` optionally preserved (true for a path, false for a single path segment).
fn uri_encode(value: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~') || (keep_slash && c == '/')
        {
            out.push(c);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// The `host[:port]` of `url`, and the path (defaulting to `/`).
fn url_host(url: &str) -> Option<&str> {
    let rest = url
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let host = authority.split(':').next()?;
    if host.is_empty() { None } else { Some(host) }
}

fn url_authority(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    if authority.is_empty() {
        None
    } else {
        Some(authority)
    }
}

fn url_path(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match rest.find('/') {
        Some(i) => rest.get(i..).unwrap_or("/").split(['?', '#']).next().unwrap_or("/"),
        None => "/",
    }
}

/// Seconds since the Unix epoch (0 on a clock error — never panics).
fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Current unix time in milliseconds (0 on a clock error — never panics).
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// `(YYYYMMDD, YYYYMMDDTHHMMSSZ)` for a Unix timestamp — the two SigV4 date forms.
fn sigv4_timestamps(epoch_seconds: u64) -> (String, String) {
    let days = (epoch_seconds / 86_400) as i64;
    let secs_of_day = epoch_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    (
        format!("{year:04}{month:02}{day:02}"),
        format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z"),
    )
}

/// Howard Hinnant's `civil_from_days`, the inverse of the `days_from_civil` cyrup already uses in
/// [`crate::utils::http_date`].
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        y + i64::from(m <= 2),
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

/// Sign the request with AWS Signature Version 4 and insert the resulting headers.
///
/// This is what the SDK's signing middleware does for upstream. `x-amz-date`, `host`,
/// `x-amz-content-sha256` and (with temporary credentials) `x-amz-security-token` are added to the
/// map before the canonical request is built, so they are covered by the signature — the same
/// invariant upstream relies on when it forbids callers from overwriting `x-amz-*` / `host` /
/// `authorization` ([`is_reserved_header`]).
fn sign_sigv4(
    headers: &mut BTreeMap<String, String>,
    url: &str,
    body: &[u8],
    creds: &AwsCredentials,
    region: &str,
    epoch_seconds: u64,
) -> Result<(), String> {
    let authority = url_authority(url).ok_or_else(|| format!("invalid Bedrock endpoint: {url}"))?;
    let (date, amz_date) = sigv4_timestamps(epoch_seconds);
    let payload_hash = hex(&crate::auth::oauth::sha256::sha256(body));

    headers.insert("host".to_string(), authority.to_string());
    headers.insert("x-amz-date".to_string(), amz_date.clone());
    headers.insert("x-amz-content-sha256".to_string(), payload_hash.clone());
    if let Some(token) = &creds.session_token {
        headers.insert("x-amz-security-token".to_string(), token.clone());
    }

    // `headers` is a BTreeMap, so iteration is already the lower-cased ascending order SigV4 wants.
    let mut canonical_headers = String::new();
    let mut signed_headers = String::new();
    for (name, value) in headers.iter() {
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(value.trim());
        canonical_headers.push('\n');
        if !signed_headers.is_empty() {
            signed_headers.push(';');
        }
        signed_headers.push_str(name);
    }

    // The canonical URI is the request path URI-encoded a SECOND time (every service but S3).
    let canonical_uri = uri_encode(url_path(url), true);
    let canonical_request = format!(
        "POST\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    let scope = format!("{date}/{region}/{SIGV4_SERVICE}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&crate::auth::oauth::sha256::sha256(canonical_request.as_bytes()))
    );

    let k_date = hmac_sha256(
        format!("AWS4{}", creds.secret_access_key).as_bytes(),
        date.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, SIGV4_SERVICE.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    headers.insert(
        "authorization".to_string(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            creds.access_key_id
        ),
    );
    Ok(())
}

/// Upper-case the first character (`internalServerException` → `InternalServerException`), so an
/// event-stream `:exception-type` maps onto the SDK exception names
/// [`bedrock_error_prefix`] is keyed by.
fn upper_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
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
    use crate::model::{Modality, ModelCost};
    use cyrup_core::ModelThinkingLevel;

    fn model_with(id: &str, name: &str) -> Model {
        Model {
            id: id.into(),
            name: name.to_string(),
            api: API_ID.into(),
            provider: "amazon-bedrock".into(),
            base_url: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            reasoning: true,
            input: vec![Modality::Text, Modality::Image],
            cost: ModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
                tiers: None,
            },
            context_window: 200_000,
            max_tokens: 64_000,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn sonnet_45() -> Model {
        model_with(
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "Claude Sonnet 4.5",
        )
    }

    fn opus_48() -> Model {
        model_with("global.anthropic.claude-opus-4-8-v1", "Claude Opus 4.8 (Global)")
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

    fn env_map(pairs: &[(&str, &str)]) -> ProviderEnv {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// An `EnvSource` with an explicit (possibly empty) ambient map, so no test can be influenced
    /// by the ambient AWS configuration of the machine running it.
    fn env_source<'a>(overlay: Option<&'a ProviderEnv>, ambient: &'a ProviderEnv) -> EnvSource<'a> {
        EnvSource {
            overlay,
            ambient: Some(ambient),
        }
    }

    /// Keyless auth — `AuthResult` has no `Default`.
    fn no_auth() -> AuthResult {
        AuthResult {
            auth: crate::auth::types::ModelAuth::default(),
            env: None,
            source: Some("keyless".to_string()),
        }
    }

    fn opts_with_reasoning(level: ModelThinkingLevel) -> StreamOptions {
        StreamOptions {
            reasoning: level,
            ..Default::default()
        }
    }

    fn payload(model: &Model, ctx: &Context, opts: &StreamOptions, bedrock: &BedrockOptions) -> Value {
        let ambient = ProviderEnv::new();
        build_params(
            model,
            ctx,
            opts,
            bedrock,
            CacheRetention::None,
            &env_source(None, &ambient),
        )
        .expect("payload builds")
    }

    // -----------------------------------------------------------------------
    // Stop-reason table (pi mapStopReason, :962-977 + bedrock-raw-stop-reason.test.ts)
    // -----------------------------------------------------------------------

    #[test]
    fn stop_reason_table_matches_upstream() {
        assert_eq!(map_stop_reason(Some("end_turn")), (StopReason::Stop, None));
        assert_eq!(
            map_stop_reason(Some("stop_sequence")),
            (StopReason::Stop, None)
        );
        assert_eq!(
            map_stop_reason(Some("max_tokens")),
            (StopReason::Length, None)
        );
        assert_eq!(
            map_stop_reason(Some("model_context_window_exceeded")),
            (StopReason::Length, None)
        );
        assert_eq!(
            map_stop_reason(Some("tool_use")),
            (StopReason::ToolUse, None)
        );
        // pi `bedrock-raw-stop-reason.test.ts:78-86`: the diagnostic is part of the mapping.
        assert_eq!(
            map_stop_reason(Some("guardrail_intervened")),
            (
                StopReason::Error,
                Some("Provider stopped with: guardrail_intervened".to_string())
            )
        );
        assert_eq!(map_stop_reason(None), (StopReason::Error, None));
    }

    // -----------------------------------------------------------------------
    // Endpoint / region / credential resolution
    // (pi bedrock-endpoint-resolution.test.ts + bedrock-credentials.test.ts)
    // -----------------------------------------------------------------------

    fn resolve(
        model: &Model,
        bedrock: &BedrockOptions,
        overlay: Option<&ProviderEnv>,
        ambient: &ProviderEnv,
    ) -> BedrockClientConfig {
        let opts = StreamOptions {
            env: overlay.cloned(),
            ..Default::default()
        };
        let auth = no_auth();
        resolve_client_config(model, &opts, bedrock, &auth, &env_source(overlay, ambient))
    }

    /// pi: "does not pin standard AWS endpoints when AWS_REGION is configured".
    #[test]
    fn aws_region_wins_and_suppresses_the_pinned_standard_endpoint() {
        let mut model = opus_48();
        model.id = "us.anthropic.claude-opus-4-8".into();
        let ambient = env_map(&[("AWS_REGION", "us-east-2")]);
        let config = resolve(&model, &BedrockOptions::default(), None, &ambient);
        assert_eq!(config.region.as_deref(), Some("us-east-2"));
        // Upstream leaves `config.endpoint` unset here and lets the SDK resolve the regional host;
        // cyrup materialises that same host, which is what "not pinned to model.baseUrl" means.
        assert_eq!(
            config.endpoint,
            "https://bedrock-runtime.us-east-2.amazonaws.com"
        );
    }

    /// pi: "derives region from a built-in EU endpoint when no region or profile is configured".
    #[test]
    fn endpoint_region_is_derived_when_nothing_else_is_configured() {
        let mut model = sonnet_45();
        model.id = "eu.anthropic.claude-sonnet-4-5-20250929-v1:0".into();
        model.base_url = "https://bedrock-runtime.eu-central-1.amazonaws.com".to_string();
        let ambient = ProviderEnv::new();
        let config = resolve(&model, &BedrockOptions::default(), None, &ambient);
        assert_eq!(
            config.endpoint,
            "https://bedrock-runtime.eu-central-1.amazonaws.com"
        );
        assert_eq!(config.region.as_deref(), Some("eu-central-1"));
    }

    /// pi: "handles missing regions for explicit, scoped, and ambient profiles" — the AMBIENT
    /// profile is the one that must suppress both the pinned endpoint and the us-east-1 default.
    #[test]
    fn an_ambient_profile_suppresses_the_endpoint_pin_and_the_region_default() {
        let mut model = sonnet_45();
        model.base_url = "https://bedrock-runtime.eu-central-1.amazonaws.com".to_string();

        // Explicit profile: endpoint still pinned, region still derived.
        let ambient = ProviderEnv::new();
        let explicit = resolve(
            &model,
            &BedrockOptions {
                profile: Some("bedrock-profile".to_string()),
                ..Default::default()
            },
            None,
            &ambient,
        );
        assert_eq!(explicit.profile.as_deref(), Some("bedrock-profile"));
        assert_eq!(explicit.region.as_deref(), Some("eu-central-1"));

        // Scoped `AWS_PROFILE` (overlay only) behaves like the explicit option.
        let overlay = env_map(&[("AWS_PROFILE", "scoped-bedrock-profile")]);
        let scoped = resolve(&model, &BedrockOptions::default(), Some(&overlay), &ambient);
        assert_eq!(scoped.profile.as_deref(), Some("scoped-bedrock-profile"));
        assert_eq!(scoped.region.as_deref(), Some("eu-central-1"));

        // Ambient `AWS_PROFILE`: upstream leaves BOTH endpoint and region undefined.
        let ambient = env_map(&[("AWS_PROFILE", "ambient-bedrock-profile")]);
        let ambient_cfg = resolve(&model, &BedrockOptions::default(), None, &ambient);
        assert_eq!(
            ambient_cfg.profile.as_deref(),
            Some("ambient-bedrock-profile")
        );
        assert_eq!(ambient_cfg.region, None);
    }

    /// pi: "still passes custom Bedrock endpoints through to the SDK client".
    #[test]
    fn a_custom_endpoint_is_always_pinned() {
        let mut model = opus_48();
        model.base_url = "https://bedrock-vpc.example.com".to_string();
        let ambient = env_map(&[("AWS_REGION", "us-west-2")]);
        let config = resolve(&model, &BedrockOptions::default(), None, &ambient);
        assert_eq!(config.endpoint, "https://bedrock-vpc.example.com");
        assert_eq!(config.region.as_deref(), Some("us-west-2"));
    }

    /// pi: "extracts region from inference profile ARN regardless of AWS_REGION" (+ the GovCloud
    /// partition form).
    #[test]
    fn an_arn_region_beats_aws_region() {
        let ambient = env_map(&[("AWS_REGION", "us-east-1")]);

        let mut model = opus_48();
        model.id =
            "arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/abc123".into();
        let config = resolve(&model, &BedrockOptions::default(), None, &ambient);
        assert_eq!(config.region.as_deref(), Some("us-west-2"));

        let mut gov = opus_48();
        gov.id =
            "arn:aws-us-gov:bedrock:us-gov-west-1:123456789012:application-inference-profile/abc"
                .into();
        let config = resolve(&gov, &BedrockOptions::default(), None, &ambient);
        assert_eq!(config.region.as_deref(), Some("us-gov-west-1"));
    }

    /// pi: "uses the generic API key option as a Bedrock bearer token".
    #[test]
    fn the_api_key_option_becomes_a_bearer_token() {
        let model = opus_48();
        let ambient = ProviderEnv::new();
        let opts = StreamOptions {
            api_key: Some("bedrock-api-key".to_string()),
            ..Default::default()
        };
        let config = resolve_client_config(
            &model,
            &opts,
            &BedrockOptions::default(),
            &no_auth(),
            &env_source(None, &ambient),
        );
        assert_eq!(config.bearer_token.as_deref(), Some("bedrock-api-key"));

        let mut headers = BTreeMap::new();
        authorize(&mut headers, &config, "https://x/y", b"{}").unwrap();
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer bedrock-api-key")
        );
        // A bearer request must NOT be SigV4-signed.
        assert!(!headers.contains_key("x-amz-date"));
    }

    /// pi `bedrock-credentials.test.ts`: an explicit or scoped profile must beat ambient access
    /// keys; an ambient-only profile must not.
    #[test]
    fn a_configured_profile_beats_ambient_access_keys() {
        let model = opus_48();
        let ambient = env_map(&[
            ("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE"),
            ("AWS_SECRET_ACCESS_KEY", "secretexample"),
        ]);

        let explicit = resolve(
            &model,
            &BedrockOptions {
                profile: Some("explicit-profile".to_string()),
                ..Default::default()
            },
            None,
            &ambient,
        );
        assert_eq!(explicit.profile.as_deref(), Some("explicit-profile"));
        assert_eq!(explicit.credentials, None);

        let overlay = env_map(&[("AWS_PROFILE", "scoped-profile")]);
        let scoped = resolve(&model, &BedrockOptions::default(), Some(&overlay), &ambient);
        assert_eq!(scoped.profile.as_deref(), Some("scoped-profile"));
        assert_eq!(scoped.credentials, None);

        // No profile at all: the ambient keys are used.
        let plain = resolve(&model, &BedrockOptions::default(), None, &ambient);
        assert_eq!(plain.profile, None);
        assert_eq!(
            plain.credentials,
            Some(AwsCredentials {
                access_key_id: "AKIAEXAMPLE".to_string(),
                secret_access_key: "secretexample".to_string(),
                session_token: None,
            })
        );

        // An AMBIENT profile does not suppress the ambient keys (pi's third credentials case).
        let mut ambient_profile = ambient.clone();
        ambient_profile.insert("AWS_PROFILE".to_string(), "ambient-profile".to_string());
        let cfg = resolve(&model, &BedrockOptions::default(), None, &ambient_profile);
        assert_eq!(cfg.profile.as_deref(), Some("ambient-profile"));
        assert!(cfg.credentials.is_some());
    }

    #[test]
    fn skip_auth_installs_the_dummy_credential_pair() {
        let model = opus_48();
        let ambient = env_map(&[
            ("AWS_BEDROCK_SKIP_AUTH", "1"),
            ("AWS_BEARER_TOKEN_BEDROCK", "tok"),
        ]);
        let config = resolve(&model, &BedrockOptions::default(), None, &ambient);
        assert_eq!(
            config.credentials.as_ref().map(|c| c.access_key_id.clone()),
            Some(SKIP_AUTH_ACCESS_KEY.to_string())
        );
        // `useBearerToken` is `bearerToken !== undefined && !skipAuth`.
        assert_eq!(config.bearer_token, None);
    }

    #[test]
    fn only_standard_runtime_hosts_yield_an_endpoint_region() {
        assert_eq!(
            standard_bedrock_endpoint_region("https://bedrock-runtime.eu-central-1.amazonaws.com"),
            Some("eu-central-1".to_string())
        );
        assert_eq!(
            standard_bedrock_endpoint_region("https://bedrock-runtime-fips.us-east-1.amazonaws.com"),
            Some("us-east-1".to_string())
        );
        assert_eq!(
            standard_bedrock_endpoint_region("https://bedrock-runtime.cn-north-1.amazonaws.com.cn"),
            Some("cn-north-1".to_string())
        );
        assert_eq!(
            standard_bedrock_endpoint_region("https://bedrock-vpc.example.com"),
            None
        );
        // Anchored: an extra label must not match.
        assert_eq!(
            standard_bedrock_endpoint_region("https://bedrock-runtime.us-east-1.evil.amazonaws.com"),
            None
        );
    }

    // -----------------------------------------------------------------------
    // Thinking payload (pi bedrock-thinking-payload.test.ts)
    // -----------------------------------------------------------------------

    #[test]
    fn adaptive_models_send_adaptive_thinking_and_an_effort() {
        let opts = opts_with_reasoning(ModelThinkingLevel::High);
        for (id, name) in [
            ("global.anthropic.claude-opus-4-8-v1", "Claude Opus 4.8 (Global)"),
            ("global.anthropic.claude-fable-5", "Claude Fable 5"),
            ("global.anthropic.claude-sonnet-5", "Claude Sonnet 5"),
            ("global.anthropic.claude-opus-5", "Claude Opus 5"),
        ] {
            let model = model_with(id, name);
            let body = payload(&model, &user_ctx("Hello"), &opts, &BedrockOptions::default());
            let fields = &body["additionalModelRequestFields"];
            assert_eq!(
                fields["thinking"],
                json!({ "type": "adaptive", "display": "summarized" }),
                "{id}"
            );
            assert_eq!(fields["output_config"], json!({ "effort": "high" }), "{id}");
            assert!(fields.get("anthropic_beta").is_none(), "{id}");
        }
    }

    #[test]
    fn xhigh_reaches_the_native_effort_on_models_that_support_it() {
        let opts = opts_with_reasoning(ModelThinkingLevel::Xhigh);
        let model = opus_48();
        let body = payload(&model, &user_ctx("Hello"), &opts, &BedrockOptions::default());
        assert_eq!(
            body["additionalModelRequestFields"]["output_config"],
            json!({ "effort": "xhigh" })
        );

        // MIRROR: an adaptive model WITHOUT native xhigh support still clamps to "high" — proving
        // the branch keys off `supportsNativeXhighEffort`, not off the level alone.
        let sonnet_46 = model_with("global.anthropic.claude-sonnet-4-6", "Claude Sonnet 4.6");
        let body = payload(&sonnet_46, &user_ctx("Hello"), &opts, &BedrockOptions::default());
        assert_eq!(
            body["additionalModelRequestFields"]["output_config"],
            json!({ "effort": "high" })
        );
    }

    /// pi: "omits display for GovCloud model ids on non-adaptive Claude thinking".
    #[test]
    fn govcloud_omits_the_thinking_display_field() {
        let opts = opts_with_reasoning(ModelThinkingLevel::High);

        let model = model_with(
            "us-gov.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "Claude Sonnet 4.5 (GovCloud)",
        );
        let body = payload(&model, &user_ctx("Hello"), &opts, &BedrockOptions::default());
        assert_eq!(
            body["additionalModelRequestFields"]["thinking"],
            json!({ "type": "enabled", "budget_tokens": 16384 })
        );
        assert_eq!(
            body["additionalModelRequestFields"]["anthropic_beta"],
            json!([INTERLEAVED_THINKING_BETA])
        );

        // A GovCloud REGION does the same to an adaptive model.
        let bedrock = BedrockOptions {
            region: Some("us-gov-west-1".to_string()),
            ..Default::default()
        };
        let body = payload(&opus_48(), &user_ctx("Hello"), &opts, &bedrock);
        assert_eq!(
            body["additionalModelRequestFields"]["thinking"],
            json!({ "type": "adaptive" })
        );

        // MIRROR: the same adaptive model outside GovCloud keeps `display`.
        let body = payload(
            &opus_48(),
            &user_ctx("Hello"),
            &opts,
            &BedrockOptions::default(),
        );
        assert_eq!(
            body["additionalModelRequestFields"]["thinking"],
            json!({ "type": "adaptive", "display": "summarized" })
        );
    }

    /// pi `:1079-1081` — the beta rides only the budget-based branch, and `interleavedThinking`
    /// defaults to `true`.
    #[test]
    fn interleaved_thinking_defaults_on_and_can_be_suppressed() {
        let opts = opts_with_reasoning(ModelThinkingLevel::High);
        let model = sonnet_45();

        let body = payload(&model, &user_ctx("Hello"), &opts, &BedrockOptions::default());
        assert_eq!(
            body["additionalModelRequestFields"]["anthropic_beta"],
            json!([INTERLEAVED_THINKING_BETA])
        );

        let bedrock = BedrockOptions {
            interleaved_thinking: Some(false),
            ..Default::default()
        };
        let body = payload(&model, &user_ctx("Hello"), &opts, &bedrock);
        assert!(
            body["additionalModelRequestFields"]
                .get("anthropic_beta")
                .is_none()
        );
    }

    #[test]
    fn thinking_display_omitted_reaches_the_wire() {
        let opts = opts_with_reasoning(ModelThinkingLevel::High);
        let bedrock = BedrockOptions {
            thinking_display: Some(BedrockThinkingDisplay::Omitted),
            ..Default::default()
        };
        let body = payload(&opus_48(), &user_ctx("Hello"), &opts, &bedrock);
        assert_eq!(
            body["additionalModelRequestFields"]["thinking"]["display"],
            json!("omitted")
        );
    }

    /// The typed-options plumbing: every field a caller can only reach through
    /// `ApiStreamOptions::Bedrock` must survive `from_stream_options`.
    #[test]
    fn typed_options_are_reachable_through_api_options() {
        let typed = BedrockOptions {
            region: Some("ap-southeast-2".to_string()),
            profile: Some("p".to_string()),
            tool_choice: Some(BedrockToolChoice::Any),
            interleaved_thinking: Some(false),
            thinking_display: Some(BedrockThinkingDisplay::Omitted),
            request_metadata: Some(
                [("team".to_string(), "core".to_string())]
                    .into_iter()
                    .collect(),
            ),
            bearer_token: Some("bt".to_string()),
        };
        let opts = StreamOptions {
            api_options: Some(crate::stream::ApiStreamOptions::Bedrock(typed.clone())),
            ..Default::default()
        };
        let resolved = BedrockOptions::from_stream_options(&opts);
        assert_eq!(resolved, typed);

        // The unified tool choice wins over the typed one when both are set.
        let opts = StreamOptions {
            api_options: Some(crate::stream::ApiStreamOptions::Bedrock(typed)),
            tool_choice: Some(crate::stream::ToolChoice::Required),
            ..Default::default()
        };
        assert_eq!(
            BedrockOptions::from_stream_options(&opts).tool_choice,
            Some(BedrockToolChoice::Any)
        );
    }

    #[test]
    fn request_metadata_reaches_the_payload() {
        let bedrock = BedrockOptions {
            request_metadata: Some(
                [("team".to_string(), "core".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };
        let body = payload(
            &sonnet_45(),
            &user_ctx("Hello"),
            &StreamOptions::default(),
            &bedrock,
        );
        assert_eq!(body["requestMetadata"], json!({ "team": "core" }));

        let body = payload(
            &sonnet_45(),
            &user_ctx("Hello"),
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert!(body.get("requestMetadata").is_none());
    }

    // -----------------------------------------------------------------------
    // Message conversion (pi bedrock-convert-messages.test.ts)
    // -----------------------------------------------------------------------

    fn messages_of(body: &Value) -> &Vec<Value> {
        body["messages"].as_array().expect("messages array")
    }

    #[test]
    fn blank_user_content_becomes_the_empty_placeholder() {
        let ctx = user_ctx("   ");
        let body = payload(
            &sonnet_45(),
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert_eq!(
            messages_of(&body)[0]["content"],
            json!([{ "text": EMPTY_TEXT_PLACEHOLDER }])
        );
    }

    #[test]
    fn blank_user_text_blocks_are_filtered_when_other_content_remains() {
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text(""), Content::text("hello")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let body = payload(
            &sonnet_45(),
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert_eq!(messages_of(&body)[0]["content"], json!([{ "text": "hello" }]));
    }

    #[test]
    fn an_assistant_turn_whose_blocks_all_filter_out_is_dropped() {
        let assistant = AssistantMessage {
            content: vec![Content::text("   ")],
            provider: "amazon-bedrock".into(),
            model: "m".to_string(),
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
        let ctx = Context {
            system_prompt: None,
            messages: vec![
                Message::User {
                    content: vec![Content::text("hi")],
                    timestamp: 0,
                },
                Message::Assistant(assistant),
            ],
            tools: Vec::new(),
        };
        let body = payload(
            &sonnet_45(),
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert_eq!(messages_of(&body).len(), 1);
        assert_eq!(messages_of(&body)[0]["role"], json!("user"));
    }

    #[test]
    fn blank_tool_result_content_becomes_the_empty_placeholder() {
        let ctx = Context {
            system_prompt: None,
            messages: vec![
                Message::User {
                    content: vec![Content::text("hi")],
                    timestamp: 0,
                },
                Message::Assistant(AssistantMessage {
                    content: vec![Content::ToolCall(ToolCall {
                        id: ToolCallId::from("tool-1"),
                        name: "tool".to_string(),
                        arguments: Map::new(),
                        thought_signature: None,
                    })],
                    provider: "amazon-bedrock".into(),
                    model: "m".to_string(),
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
                    tool_call_id: ToolCallId::from("tool-1"),
                    tool_name: "tool".to_string(),
                    content: vec![Content::text("")],
                    is_error: false,
                    details: None,
                    usage: None,
                    added_tool_names: Vec::new(),
                    timestamp: 0,
                },
            ],
            tools: Vec::new(),
        };
        let body = payload(
            &sonnet_45(),
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        let last = messages_of(&body).last().unwrap();
        assert_eq!(
            last["content"][0]["toolResult"]["content"],
            json!([{ "text": EMPTY_TEXT_PLACEHOLDER }])
        );
        assert_eq!(last["content"][0]["toolResult"]["status"], json!("success"));
    }

    /// pi `:867-903`: a RUN of consecutive tool results collapses into ONE user message.
    #[test]
    fn consecutive_tool_results_collapse_into_one_user_message() {
        let calls = ["a", "b", "c"];
        let mut messages = vec![
            Message::User {
                content: vec![Content::text("hi")],
                timestamp: 0,
            },
            Message::Assistant(AssistantMessage {
                content: calls
                    .iter()
                    .map(|id| {
                        Content::ToolCall(ToolCall {
                            id: ToolCallId::from(*id),
                            name: "tool".to_string(),
                            arguments: Map::new(),
                            thought_signature: None,
                        })
                    })
                    .collect(),
                provider: "amazon-bedrock".into(),
                model: "m".to_string(),
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
        ];
        for id in calls {
            messages.push(Message::ToolResult {
                tool_call_id: ToolCallId::from(id),
                tool_name: "tool".to_string(),
                content: vec![Content::text(format!("result {id}"))],
                is_error: id == "b",
                details: None,
                usage: None,
                added_tool_names: Vec::new(),
                timestamp: 0,
            });
        }
        let ctx = Context {
            system_prompt: None,
            messages,
            tools: Vec::new(),
        };
        let body = payload(
            &sonnet_45(),
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        // user, assistant, ONE user carrying all three results.
        assert_eq!(messages_of(&body).len(), 3);
        let last = messages_of(&body).last().unwrap();
        assert_eq!(last["role"], json!("user"));
        let results = last["content"].as_array().unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[1]["toolResult"]["status"], json!("error"));
        assert_eq!(results[0]["toolResult"]["status"], json!("success"));
    }

    /// pi `:830-843`: a thinking block replayed without a signature degrades to plain text on
    /// Claude models — Bedrock rejects a signature-less reasoning block.
    #[test]
    fn signatureless_thinking_replays_as_text_on_claude_and_as_reasoning_elsewhere() {
        let thinking = |sig: Option<&str>| Content::Thinking {
            thinking: "ponder".to_string(),
            thinking_signature: sig.map(str::to_string),
            redacted: false,
        };
        // The assistant turn must claim the SAME model the request targets: `transformMessages`
        // (`transform-messages.ts`) drops cross-model thinking before the converter ever sees it,
        // which would make this test about that transform rather than about `convertMessages`.
        let ctx = |model: &Model, content: Vec<Content>| Context {
            system_prompt: None,
            messages: vec![
                Message::User {
                    content: vec![Content::text("hi")],
                    timestamp: 0,
                },
                Message::Assistant(AssistantMessage {
                    content,
                    provider: model.provider.clone(),
                    model: model.id.as_str().to_string(),
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
                }),
            ],
            tools: Vec::new(),
        };

        let claude = sonnet_45();
        let body = payload(
            &claude,
            &ctx(&claude, vec![thinking(None)]),
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert_eq!(
            messages_of(&body)[1]["content"],
            json!([{ "text": "ponder" }])
        );

        let body = payload(
            &claude,
            &ctx(&claude, vec![thinking(Some("sig"))]),
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert_eq!(
            messages_of(&body)[1]["content"],
            json!([{ "reasoningContent": { "reasoningText": { "text": "ponder", "signature": "sig" } } }])
        );

        // MIRROR: a non-Claude model never sends the signature field at all (`:844-850`).
        let nova = model_with("amazon.nova-pro-v1:0", "Nova Pro");
        let body = payload(
            &nova,
            &ctx(&nova, vec![thinking(Some("sig"))]),
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert_eq!(
            messages_of(&body)[1]["content"],
            json!([{ "reasoningContent": { "reasoningText": { "text": "ponder" } } }])
        );
    }

    #[test]
    fn an_unknown_image_type_is_the_ported_error_string() {
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::Image {
                    data: "AAAA".to_string(),
                    mime_type: "image/tiff".to_string(),
                }],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let ambient = ProviderEnv::new();
        let err = build_params(
            &sonnet_45(),
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
            CacheRetention::None,
            &env_source(None, &ambient),
        )
        .unwrap_err();
        assert_eq!(err, "Unknown image type: image/tiff");
    }

    #[test]
    fn tool_call_ids_are_sanitized_and_capped_at_64_chars() {
        assert_eq!(normalize_tool_call_id("call:abc/def"), "call_abc_def");
        assert_eq!(normalize_tool_call_id(&"x".repeat(80)).len(), 64);
        assert_eq!(normalize_tool_call_id("keep-_09AZ"), "keep-_09AZ");
    }

    // -----------------------------------------------------------------------
    // Cache points (pi :712-730 / :909-920)
    // -----------------------------------------------------------------------

    #[test]
    fn cache_points_land_on_the_system_prompt_and_the_last_user_message() {
        let ambient = ProviderEnv::new();
        let ctx = Context {
            system_prompt: Some("You are helpful.".to_string()),
            messages: vec![Message::User {
                content: vec![Content::text("Hello")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let body = build_params(
            &sonnet_45(),
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
            CacheRetention::Short,
            &env_source(None, &ambient),
        )
        .unwrap();
        let system = body["system"].as_array().unwrap();
        assert_eq!(system.len(), 2);
        assert_eq!(system[1], json!({ "cachePoint": { "type": "default" } }));
        let content = messages_of(&body)[0]["content"].as_array().unwrap();
        assert_eq!(
            content.last().unwrap(),
            &json!({ "cachePoint": { "type": "default" } })
        );

        // Long retention adds the ttl.
        let body = build_params(
            &sonnet_45(),
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
            CacheRetention::Long,
            &env_source(None, &ambient),
        )
        .unwrap();
        assert_eq!(
            body["system"][1],
            json!({ "cachePoint": { "type": "default", "ttl": "1h" } })
        );

        // MIRROR: a model with no Claude reference gets no cache points at all, unless
        // AWS_BEDROCK_FORCE_CACHE=1 says otherwise.
        let nova = model_with("amazon.nova-pro-v1:0", "Nova Pro");
        let body = build_params(
            &nova,
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
            CacheRetention::Short,
            &env_source(None, &ambient),
        )
        .unwrap();
        assert_eq!(body["system"].as_array().unwrap().len(), 1);

        let forced = env_map(&[("AWS_BEDROCK_FORCE_CACHE", "1")]);
        let body = build_params(
            &nova,
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
            CacheRetention::Short,
            &env_source(None, &forced),
        )
        .unwrap();
        assert_eq!(body["system"].as_array().unwrap().len(), 2);
    }

    /// pi's "injects cache points when model.name identifies a supported Claude model" — the ARN
    /// carries no model name, so the decision has to come from `model.name`.
    #[test]
    fn an_application_inference_profile_caches_via_the_model_name() {
        let ambient = ProviderEnv::new();
        let mut model = sonnet_45();
        model.id =
            "arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/my-profile".into();
        model.name = "Claude Sonnet 4.6".to_string();
        let ctx = Context {
            system_prompt: Some("You are helpful.".to_string()),
            messages: vec![Message::User {
                content: vec![Content::text("Hello")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let body = build_params(
            &model,
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
            CacheRetention::Short,
            &env_source(None, &ambient),
        )
        .unwrap();
        assert_eq!(body["system"].as_array().unwrap().len(), 2);

        // The same ARN with a name that identifies no Claude model gets nothing.
        model.name = "My Profile".to_string();
        let body = build_params(
            &model,
            &ctx,
            &StreamOptions::default(),
            &BedrockOptions::default(),
            CacheRetention::Short,
            &env_source(None, &ambient),
        )
        .unwrap();
        assert_eq!(body["system"].as_array().unwrap().len(), 1);
    }

    // -----------------------------------------------------------------------
    // Tool config (pi convertToolConfig, :925-960)
    // -----------------------------------------------------------------------

    fn tool_ctx() -> Context {
        Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text("Use it")],
                timestamp: 0,
            }],
            tools: vec![ToolDef {
                name: "lookup".to_string(),
                description: "Look up a value".to_string(),
                parameters: json!({ "type": "object", "properties": {} }),
                constrained_sampling: None,
            }],
        }
    }

    #[test]
    fn tool_config_shape_and_choice_mapping() {
        let base = payload(
            &sonnet_45(),
            &tool_ctx(),
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert_eq!(
            base["toolConfig"]["tools"][0]["toolSpec"]["name"],
            json!("lookup")
        );
        assert_eq!(
            base["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"]["type"],
            json!("object")
        );
        // No choice configured ⇒ no `toolChoice` key at all.
        assert!(base["toolConfig"].get("toolChoice").is_none());

        for (choice, wire) in [
            (BedrockToolChoice::Auto, json!({ "auto": {} })),
            (BedrockToolChoice::Any, json!({ "any": {} })),
            (
                BedrockToolChoice::Tool {
                    name: "lookup".to_string(),
                },
                json!({ "tool": { "name": "lookup" } }),
            ),
        ] {
            let bedrock = BedrockOptions {
                tool_choice: Some(choice),
                ..Default::default()
            };
            let body = payload(&sonnet_45(), &tool_ctx(), &StreamOptions::default(), &bedrock);
            assert_eq!(body["toolConfig"]["toolChoice"], wire);
        }

        // `"none"` drops the whole toolConfig (pi `:931`).
        let bedrock = BedrockOptions {
            tool_choice: Some(BedrockToolChoice::None),
            ..Default::default()
        };
        let body = payload(&sonnet_45(), &tool_ctx(), &StreamOptions::default(), &bedrock);
        assert!(body.get("toolConfig").is_none());

        // No tools ⇒ no toolConfig.
        let body = payload(
            &sonnet_45(),
            &user_ctx("hi"),
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert!(body.get("toolConfig").is_none());
    }

    // -----------------------------------------------------------------------
    // inferenceConfig (pi :229 / :234-237)
    // -----------------------------------------------------------------------

    #[test]
    fn claude_defaults_max_tokens_to_the_model_cap_and_non_claude_omits_it() {
        let body = payload(
            &sonnet_45(),
            &user_ctx("hi"),
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert_eq!(body["inferenceConfig"]["maxTokens"], json!(64_000));

        let nova = model_with("amazon.nova-pro-v1:0", "Nova Pro");
        let body = payload(
            &nova,
            &user_ctx("hi"),
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert!(body["inferenceConfig"].get("maxTokens").is_none());

        // An explicit cap always wins.
        let opts = StreamOptions {
            max_tokens: Some(1234),
            temperature: Some(0.5),
            ..Default::default()
        };
        let body = payload(&nova, &user_ctx("hi"), &opts, &BedrockOptions::default());
        assert_eq!(body["inferenceConfig"]["maxTokens"], json!(1234));
        assert_eq!(body["inferenceConfig"]["temperature"], json!(0.5));
    }

    /// pi `streamSimple` (`:424-441`): a budget-based Claude model re-splits the cap, and the
    /// resulting budget is `min(adjusted, maxTokens - 1024)`.
    #[test]
    fn budget_based_claude_resplits_max_tokens_between_thinking_and_output() {
        let mut model = sonnet_45();
        model.max_tokens = 8_000;
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            max_tokens: Some(2_000),
            ..Default::default()
        };
        let body = payload(&model, &user_ctx("hi"), &opts, &BedrockOptions::default());
        // adjust: min(2000 + 16384, 8000) = 8000 ⇒ budget 16384 > 8000 ⇒ budget = 8000-1024 = 6976.
        assert_eq!(body["inferenceConfig"]["maxTokens"], json!(8_000));
        assert_eq!(
            body["additionalModelRequestFields"]["thinking"]["budget_tokens"],
            json!(6_976)
        );

        // MIRROR: an ADAPTIVE model does not re-split — it keeps the caller's cap and emits no
        // budget at all.
        let mut adaptive = opus_48();
        adaptive.max_tokens = 8_000;
        let body = payload(&adaptive, &user_ctx("hi"), &opts, &BedrockOptions::default());
        assert_eq!(body["inferenceConfig"]["maxTokens"], json!(2_000));
        assert!(
            body["additionalModelRequestFields"]["thinking"]
                .get("budget_tokens")
                .is_none()
        );
    }

    #[test]
    fn a_non_reasoning_model_sends_no_additional_fields() {
        let mut model = sonnet_45();
        model.reasoning = false;
        let opts = opts_with_reasoning(ModelThinkingLevel::High);
        let body = payload(&model, &user_ctx("hi"), &opts, &BedrockOptions::default());
        assert!(body.get("additionalModelRequestFields").is_none());

        // …and neither does a reasoning model with reasoning off.
        let body = payload(
            &sonnet_45(),
            &user_ctx("hi"),
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert!(body.get("additionalModelRequestFields").is_none());
    }

    // -----------------------------------------------------------------------
    // Custom headers (pi bedrock-custom-headers.test.ts VC1/VC2/VC3)
    // -----------------------------------------------------------------------

    #[test]
    fn caller_headers_are_injected_but_reserved_ones_are_skipped_case_insensitively() {
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        headers.insert("authorization".to_string(), "real-auth".to_string());
        headers.insert("x-amz-date".to_string(), "real-date".to_string());
        headers.insert("host".to_string(), "real-host".to_string());

        let caller: HeaderMap = [
            ("authorization", Some("evil")),
            ("x-amz-date", Some("evil")),
            ("x-allowed", Some("ok")),
            ("Authorization", Some("evil2")),
            ("X-Amz-Date", Some("evil2")),
            ("HOST", Some("evil3")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
        .collect();

        apply_custom_headers(&mut headers, Some(&caller), None);

        assert_eq!(headers.get("authorization").map(String::as_str), Some("real-auth"));
        assert_eq!(headers.get("x-amz-date").map(String::as_str), Some("real-date"));
        assert_eq!(headers.get("host").map(String::as_str), Some("real-host"));
        assert_eq!(headers.get("x-allowed").map(String::as_str), Some("ok"));
        // No mixed-case leak (pi's VC2 key-set assertion).
        let keys: Vec<&str> = headers.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["authorization", "host", "x-allowed", "x-amz-date"]);
    }

    #[test]
    fn no_caller_headers_changes_nothing() {
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        let before = headers.clone();
        apply_custom_headers(&mut headers, None, None);
        apply_custom_headers(&mut headers, Some(&HeaderMap::new()), None);
        assert_eq!(headers, before);
    }

    // -----------------------------------------------------------------------
    // Error composition (pi formatBedrockError, :326-365)
    // -----------------------------------------------------------------------

    #[test]
    fn service_errors_carry_the_legacy_prefix_and_the_status_body_core() {
        assert_eq!(
            format_bedrock_service_error("ThrottlingException", 429, "{\"message\":\"slow down\"}"),
            "Throttling error: 429: {\"message\":\"slow down\"}"
        );
        assert_eq!(
            format_bedrock_service_error("ServiceUnavailableException", 503, "down"),
            "Service unavailable: 503: down"
        );
        // An unmodeled shape falls back to the raw name (pi's `?? error.name`).
        assert_eq!(
            format_bedrock_service_error("AccessDeniedException", 403, "nope"),
            "AccessDeniedException: 403: nope"
        );
    }

    #[test]
    fn a_data_retention_message_gains_the_docs_hint() {
        let out = format_bedrock_service_error(
            "ValidationException",
            400,
            "data retention mode 'default' is not available for this model",
        );
        assert!(out.starts_with("Validation error: 400: data retention mode"));
        assert!(out.ends_with(&format!(
            " See {BEDROCK_DATA_RETENTION_DOCS_URL} for supported data retention modes."
        )));

        // MIRROR: an unrelated message gains nothing.
        let plain = format_bedrock_service_error("ValidationException", 400, "bad input");
        assert_eq!(plain, "Validation error: 400: bad input");
    }

    // -----------------------------------------------------------------------
    // Event-stream framing + streaming decode
    // -----------------------------------------------------------------------

    /// Encode one AWS event-stream frame, so the decoder is tested against bytes built to the
    /// published layout rather than against its own output.
    fn frame(headers: &[(&str, &str)], payload: &str) -> Vec<u8> {
        let mut header_bytes = Vec::new();
        for (name, value) in headers {
            header_bytes.push(name.len() as u8);
            header_bytes.extend_from_slice(name.as_bytes());
            header_bytes.push(7); // string
            header_bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
            header_bytes.extend_from_slice(value.as_bytes());
        }
        let payload = payload.as_bytes();
        let total = 16 + header_bytes.len() + payload.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&(total as u32).to_be_bytes());
        out.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
        let prelude_crc = crc32(&out);
        out.extend_from_slice(&prelude_crc.to_be_bytes());
        out.extend_from_slice(&header_bytes);
        out.extend_from_slice(payload);
        let message_crc = crc32(&out);
        out.extend_from_slice(&message_crc.to_be_bytes());
        out
    }

    fn event(event_type: &str, payload: &str) -> Vec<u8> {
        frame(
            &[
                (":message-type", "event"),
                (":event-type", event_type),
                (":content-type", "application/json"),
            ],
            payload,
        )
    }

    #[test]
    fn the_event_stream_decoder_handles_split_and_coalesced_chunks() {
        let bytes = [
            event("messageStart", "{\"role\":\"assistant\"}"),
            event("contentBlockDelta", "{\"contentBlockIndex\":0,\"delta\":{\"text\":\"hi\"}}"),
        ]
        .concat();

        // One byte at a time — the decoder must never mis-frame.
        let mut dec = EventStreamDecoder::default();
        let mut seen = Vec::new();
        for byte in &bytes {
            dec.push(std::slice::from_ref(byte));
            while let Some(f) = dec.next_frame().unwrap() {
                seen.push(f.header(":event-type").unwrap());
            }
        }
        assert_eq!(seen, vec!["messageStart", "contentBlockDelta"]);

        // Both frames in one chunk.
        let mut dec = EventStreamDecoder::default();
        dec.push(&bytes);
        assert_eq!(
            dec.next_frame().unwrap().unwrap().header(":event-type"),
            Some("messageStart".to_string())
        );
        assert_eq!(
            dec.next_frame().unwrap().unwrap().header(":event-type"),
            Some("contentBlockDelta".to_string())
        );
        assert!(dec.next_frame().unwrap().is_none());
    }

    #[test]
    fn a_corrupted_frame_is_rejected_by_its_checksum() {
        let mut bytes = event("messageStart", "{\"role\":\"assistant\"}");
        let last = bytes.len() - 5;
        bytes[last] ^= 0xFF;
        let mut dec = EventStreamDecoder::default();
        dec.push(&bytes);
        assert!(dec.next_frame().is_err());
    }

    #[test]
    fn non_string_header_values_do_not_desynchronise_the_walk() {
        let mut header_bytes = Vec::new();
        // A timestamp header (type 8), then the string header we care about.
        header_bytes.push(4u8);
        header_bytes.extend_from_slice(b"when");
        header_bytes.push(8);
        header_bytes.extend_from_slice(&0i64.to_be_bytes());
        header_bytes.push(11u8);
        header_bytes.extend_from_slice(b":event-type");
        header_bytes.push(7);
        header_bytes.extend_from_slice(&(8u16).to_be_bytes());
        header_bytes.extend_from_slice(b"metadata");

        let parsed = parse_event_headers(&header_bytes).unwrap();
        assert_eq!(parsed.get(":event-type").map(String::as_str), Some("metadata"));
        assert!(!parsed.contains_key("when"));
    }

    async fn collect(chunks: Vec<Vec<u8>>, model: &Model) -> Vec<StreamEvent> {
        let api = ApiId::from(API_ID);
        let (sink, mut rx) = crate::api::channel(64);
        let mut dec = Decoder::default();
        let mut frames = EventStreamDecoder::default();
        let m = model.clone();
        let a = api.clone();
        let task = tokio::spawn(async move {
            // No `start` is pushed here: `dispatch_frame` emits it from `messageStart`, exactly as
            // pi does (`:262`), which is what this helper is exercising.
            for chunk in chunks {
                frames.push(&chunk);
                while let Some(f) = frames.next_frame().expect("frame") {
                    if let Err(message) = dispatch_frame(&f, &mut dec, &m, &a, &sink).await {
                        let mut msg = dec.snapshot(&m, &a);
                        msg.stop_reason = StopReason::Error;
                        msg.error_message = Some(message);
                        sink.send(StreamEvent::terminal(msg)).await;
                        return;
                    }
                }
            }
            let mut msg = dec.snapshot(&m, &a);
            if dec.stop_reason == Some(StopReason::Error) && dec.error_message.is_none() {
                msg.error_message = Some("An unknown error occurred".to_string());
            }
            sink.send(StreamEvent::end_of_stream(
                msg,
                dec.stop_reason,
                "Bedrock stream ended without a stop reason",
            ))
            .await;
        });
        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev);
        }
        let _ = task.await;
        out
    }

    fn kinds(events: &[StreamEvent]) -> Vec<&'static str> {
        events
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
            .collect()
    }

    #[tokio::test]
    async fn decodes_text_thinking_and_tool_use_in_upstream_order() {
        let model = sonnet_45();
        let chunks = vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event(
                "contentBlockDelta",
                "{\"contentBlockIndex\":0,\"delta\":{\"reasoningContent\":{\"text\":\"think\"}}}",
            ),
            event(
                "contentBlockDelta",
                "{\"contentBlockIndex\":0,\"delta\":{\"reasoningContent\":{\"signature\":\"sig\"}}}",
            ),
            event("contentBlockStop", "{\"contentBlockIndex\":0}"),
            event(
                "contentBlockDelta",
                "{\"contentBlockIndex\":1,\"delta\":{\"text\":\"Hel\"}}",
            ),
            event(
                "contentBlockDelta",
                "{\"contentBlockIndex\":1,\"delta\":{\"text\":\"lo\"}}",
            ),
            event("contentBlockStop", "{\"contentBlockIndex\":1}"),
            event(
                "contentBlockStart",
                "{\"contentBlockIndex\":2,\"start\":{\"toolUse\":{\"toolUseId\":\"t1\",\"name\":\"lookup\"}}}",
            ),
            event(
                "contentBlockDelta",
                "{\"contentBlockIndex\":2,\"delta\":{\"toolUse\":{\"input\":\"{\\\"q\\\":\"}}}",
            ),
            event(
                "contentBlockDelta",
                "{\"contentBlockIndex\":2,\"delta\":{\"toolUse\":{\"input\":\"1}\"}}}",
            ),
            event("contentBlockStop", "{\"contentBlockIndex\":2}"),
            event(
                "metadata",
                "{\"usage\":{\"inputTokens\":10,\"outputTokens\":5,\"cacheReadInputTokens\":2,\"cacheWriteInputTokens\":1,\"totalTokens\":18}}",
            ),
            event("messageStop", "{\"stopReason\":\"tool_use\"}"),
        ];

        let events = collect(chunks, &model).await;
        assert_eq!(
            kinds(&events),
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

        let StreamEvent::Done { message, .. } = events.last().unwrap() else {
            panic!("expected a done terminal");
        };
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert_eq!(message.usage.input, 10);
        assert_eq!(message.usage.output, 5);
        assert_eq!(message.usage.cache_read, 2);
        assert_eq!(message.usage.cache_write, 1);
        // The provider's own `totalTokens` is preserved, not recomputed (pi `:542`).
        assert_eq!(message.usage.total_tokens, 18);
        // 10 in @ $3/1e6 + 5 out @ $15/1e6 + 2 cacheRead @ $0.3/1e6 + 1 cacheWrite @ $3.75/1e6.
        let expected = 10.0 * 3.0 / 1e6 + 5.0 * 15.0 / 1e6 + 2.0 * 0.3 / 1e6 + 3.75 / 1e6;
        assert!(message.usage.cost.total > 0.0);
        assert!((message.usage.cost.total - expected).abs() < 1e-12);

        assert_eq!(message.content.len(), 3);
        match &message.content[0] {
            Content::Thinking {
                thinking,
                thinking_signature,
                ..
            } => {
                assert_eq!(thinking, "think");
                assert_eq!(thinking_signature.as_deref(), Some("sig"));
            }
            other => panic!("expected thinking, got {other:?}"),
        }
        match &message.content[2] {
            Content::ToolCall(tc) => {
                assert_eq!(tc.name, "lookup");
                assert_eq!(tc.id.as_str(), "t1");
                assert_eq!(tc.arguments.get("q"), Some(&json!(1)));
            }
            other => panic!("expected a tool call, got {other:?}"),
        }
    }

    /// Spawn a one-shot mock HTTP server that writes `raw_response` then closes. Returns its URL.
    async fn spawn_mock(raw_response: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(raw_response).await;
                let _ = sock.flush().await;
            }
        });
        format!("http://{addr}")
    }

    /// Spawn a mock server that answers each successive connection with the next entry of
    /// `responses` (the last entry repeats), and report how many connections it accepted.
    async fn spawn_mock_sequence(
        responses: &'static [&'static [u8]],
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let body = responses.get(n).copied().unwrap_or_else(|| {
                    responses.last().copied().unwrap_or(b"HTTP/1.1 500 x\r\n\r\n")
                });
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(body).await;
                let _ = sock.flush().await;
            }
        });
        (format!("http://{addr}"), hits)
    }

    /// PROV-043. pi's Bedrock client inherits the AWS SDK v3 **standard** retry mode — 3 attempts —
    /// because its config never sets `maxAttempts`/`retryStrategy`
    /// (`bedrock-converse-stream.ts:150-222` @v0.83.0). cyrup issued exactly ONE `send()`, so a
    /// routine `ThrottlingException` that pi swallows became a visible turn failure.
    ///
    /// Red before the fix: `hits == 1` and the terminal error was the 429.
    #[tokio::test]
    async fn a_throttled_bedrock_request_is_retried_to_the_sdk_attempt_count() {
        const THROTTLE: &[u8] = b"HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nx-amzn-errortype: ThrottlingException\r\nretry-after-ms: 1\r\nConnection: close\r\n\r\n{\"message\":\"slow down\"}";
        // A 400 is NOT retryable (provider-retry.ts:22-34), so it terminates the loop and proves
        // the two preceding 429s were retried rather than returned.
        const VALIDATION: &[u8] = b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nx-amzn-errortype: ValidationException\r\nConnection: close\r\n\r\n{\"message\":\"bad input\"}";
        const SEQ: &[&[u8]] = &[THROTTLE, THROTTLE, VALIDATION];

        let (url, hits) = spawn_mock_sequence(SEQ).await;
        let events = run_against(url).await;
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "standard mode is 3 attempts: the first plus BEDROCK_STANDARD_MODE_RETRIES"
        );
        let Some(StreamEvent::Error { error, .. }) = events.last() else {
            panic!("expected an error terminal, got {:?}", kinds(&events));
        };
        assert!(
            error
                .error_message
                .as_deref()
                .is_some_and(|m| m.contains("bad input")),
            "the terminal must be the third response, not the first: {:?}",
            error.error_message
        );
    }

    /// PROV-043, exhaustion half: a throttle on every attempt fails after the SDK's attempt count,
    /// not on the first response.
    #[tokio::test]
    async fn a_permanently_throttled_bedrock_request_stops_at_the_attempt_count() {
        const THROTTLE: &[u8] = b"HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nx-amzn-errortype: ThrottlingException\r\nretry-after-ms: 1\r\nConnection: close\r\n\r\n{\"message\":\"slow down\"}";
        const SEQ: &[&[u8]] = &[THROTTLE];
        let (url, hits) = spawn_mock_sequence(SEQ).await;
        let events = run_against(url).await;
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert!(matches!(events.last(), Some(StreamEvent::Error { .. })));
    }

    /// PROV-044. `AWS_BEDROCK_FORCE_HTTP1=1` must reach the client builder as `http1_only()`
    /// (pi `bedrock-converse-stream.ts:206-209` @v0.83.0), and only when no proxy was resolved —
    /// pi's `else if`. Red before the fix: `rg AWS_BEDROCK_FORCE_HTTP1 crates/` was empty and
    /// cyrup's client negotiated h2 by ALPN with no override.
    #[tokio::test]
    async fn force_http1_builds_an_http1_only_client_only_without_a_proxy() {
        use crate::stream::sse::build_client_for_target_forcing_http1;
        let ctx = crate::auth::types::EnvAuthContext;
        // No proxy in the overlay ⇒ the override applies and the client still builds.
        let no_proxy = env_map(&[]);
        assert!(
            build_client_for_target_forcing_http1(
                "https://bedrock-runtime.us-east-1.amazonaws.com",
                &ctx,
                Some(&no_proxy),
                None,
                true,
            )
            .await
            .is_ok()
        );
        // With a proxy the override is suppressed (pi's `else if`), and the proxied client builds.
        let proxied = env_map(&[("HTTPS_PROXY", "http://127.0.0.1:3128")]);
        assert!(
            build_client_for_target_forcing_http1(
                "https://bedrock-runtime.us-east-1.amazonaws.com",
                &ctx,
                Some(&proxied),
                None,
                true,
            )
            .await
            .is_ok()
        );
    }

    /// Drive the real `ApiImpl::run` (so the ported catch arm runs) against `base_url`.
    async fn run_against(base_url: String) -> Vec<StreamEvent> {
        let mut model = sonnet_45();
        model.base_url = base_url;
        let (sink, mut rx) = crate::api::channel(64);
        let auth = AuthResult {
            auth: Default::default(),
            env: Some(env_map(&[
                ("AWS_BEDROCK_SKIP_AUTH", "1"),
                ("AWS_REGION", "us-east-1"),
            ])),
            source: None,
        };
        let task = tokio::spawn(async move {
            BedrockConverseStreamApi::new()
                .run(
                    &model,
                    &user_ctx("hi"),
                    &auth,
                    &StreamOptions::default(),
                    CancelToken::new(),
                    sink,
                )
                .await;
        });
        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev);
        }
        let _ = task.await;
        out
    }

    /// VERSION LAG (v0.83.0 → v0.84.1): the whole structured-diagnostic path is new in v0.84.1 —
    /// `appendBedrockFailureDiagnostic` (`ai/src/api/bedrock-converse-stream.ts:398-421`), its
    /// `normalizeDiagnosticValue`/`extractBedrockErrorCode` helpers (`:381-396`), the hoisted
    /// `responseRequestId` (`:225`, assigned at `:254`) and the catch-side call (`:318-320`). None
    /// of those four identifiers exists anywhere in `v0.83.0 ai/src/api/bedrock-converse-stream.ts`.
    /// `errorMessage` must stay byte-identical, because the retry classifier matches against it.
    #[tokio::test]
    async fn a_bedrock_failure_carries_structured_diagnostics() {
        let url = spawn_mock(
            b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nx-amzn-errortype: com.amazon.coral.validate#ValidationException\r\nx-amzn-requestid: req-abc-123\r\nConnection: close\r\n\r\n{\"message\":\"bad input\"}",
        )
        .await;
        let events = run_against(url).await;
        let Some(StreamEvent::Error { error, .. }) = events.last() else {
            panic!("expected an error terminal, got {:?}", kinds(&events));
        };
        // The message is untouched by the diagnostic (pi `:398-402`).
        assert_eq!(
            error.error_message.as_deref(),
            Some("Validation error: 400: {\"message\":\"bad input\"}")
        );
        let diags = error.diagnostics.as_ref().expect("diagnostics");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].r#type, "bedrock_response_failure");
        // `details` only — the throw is not always an `Error` (pi `:400-402`).
        assert_eq!(diags[0].error, None);
        let details = diags[0].details.as_ref().expect("details");
        assert_eq!(details["status"], json!(400));
        assert_eq!(details["errorCode"], json!("ValidationException"));
        assert_eq!(details["requestId"], json!("req-abc-123"));
    }

    /// MIRROR: the helpers omit unknown fields rather than guessing them, and drop over-long values
    /// instead of truncating (pi `:379-396`).
    #[test]
    fn diagnostic_values_are_normalized_not_guessed() {
        assert_eq!(
            normalize_diagnostic_value("  req-1  ").as_deref(),
            Some("req-1")
        );
        assert_eq!(normalize_diagnostic_value("   "), None);
        assert_eq!(normalize_diagnostic_value(&"x".repeat(200)).as_deref().map(str::len), Some(200));
        // 201 chars is DROPPED, not truncated: a truncated request id is not a request id.
        assert_eq!(normalize_diagnostic_value(&"x".repeat(201)), None);

        // ── The cap counts UTF-16 CODE UNITS, not scalar values ──────────────────────────────
        // pi v0.84.1 `ai/src/api/bedrock-converse-stream.ts:384`:
        //     `if (trimmed.length === 0 || trimmed.length > MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS)`
        // JS `.length` is UTF-16 code units. `\u{1F600}` is ONE scalar but TWO code units, so a
        // 101-emoji value sits in the window where the units disagree: 101 scalars (<= 200, would
        // be KEPT by a `chars().count()` cap) but 202 UTF-16 units (> 200, DROPPED by pi). This
        // case is what distinguishes the two measures — an ASCII string can never separate them.
        let astral_101 = "\u{1F600}".repeat(101);
        assert_eq!(astral_101.chars().count(), 101, "under the cap in SCALARS");
        assert_eq!(astral_101.encode_utf16().count(), 202, "over the cap in UTF-16 UNITS");
        assert_eq!(
            normalize_diagnostic_value(&astral_101),
            None,
            "pi measures `.length` in UTF-16 units, so 202 > 200 drops this value"
        );

        // MIRROR: 100 emoji is exactly 200 UTF-16 units, and pi's check is `>`, so it is KEPT.
        // This pins the boundary from below — a fix that simply dropped everything non-ASCII
        // would fail here.
        let astral_100 = "\u{1F600}".repeat(100);
        assert_eq!(astral_100.encode_utf16().count(), MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS);
        assert_eq!(
            normalize_diagnostic_value(&astral_100).as_deref(),
            Some(astral_100.as_str()),
            "exactly at the cap: pi's `>` keeps it"
        );

        // Only modeled Bedrock shapes (which all end in `Exception`) are error codes.
        assert_eq!(
            extract_bedrock_error_code("ThrottlingException").as_deref(),
            Some("ThrottlingException")
        );
        assert_eq!(extract_bedrock_error_code("TimeoutError"), None);

        // Nothing known → no diagnostic at all (pi `:419`).
        let mut msg = AssistantMessage::errored(
            "amazon-bedrock".into(),
            "m",
            None,
            StopReason::Error,
            "boom",
        );
        append_bedrock_failure_diagnostic(&mut msg, None, None, None);
        assert_eq!(msg.diagnostics, None);

        // A mid-stream modeled exception reaches upstream as a bare object literal: only the
        // hoisted request id lands (pi `:400-402`).
        append_bedrock_failure_diagnostic(&mut msg, None, None, Some("req-9"));
        let diags = msg.diagnostics.as_ref().expect("diagnostics");
        let details = diags[0].details.as_ref().expect("details");
        assert_eq!(details["requestId"], json!("req-9"));
        assert_eq!(details.get("status"), None);
        assert_eq!(details.get("errorCode"), None);
    }

    /// A stream that ends without `messageStop` is TRUNCATED, never a clean `stop`
    /// (PROV-010 — the rule `StreamEvent::end_of_stream` exists to enforce).
    #[tokio::test]
    async fn a_stream_without_message_stop_is_an_error_terminal() {
        let events = collect(
            vec![
                event("messageStart", "{\"role\":\"assistant\"}"),
                event(
                    "contentBlockDelta",
                    "{\"contentBlockIndex\":0,\"delta\":{\"text\":\"hi\"}}",
                ),
            ],
            &sonnet_45(),
        )
        .await;
        let StreamEvent::Error { error, .. } = events.last().unwrap() else {
            panic!("expected an error terminal, got {:?}", kinds(&events));
        };
        assert_eq!(error.stop_reason, StopReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Bedrock stream ended without a stop reason")
        );
    }

    /// pi `:298-300`: a settled `error` stop reason throws with the mapped diagnostic.
    #[tokio::test]
    async fn a_guardrail_stop_reaches_the_terminal_with_its_diagnostic() {
        let events = collect(
            vec![
                event("messageStart", "{\"role\":\"assistant\"}"),
                event("messageStop", "{\"stopReason\":\"guardrail_intervened\"}"),
            ],
            &sonnet_45(),
        )
        .await;
        let StreamEvent::Error { error, .. } = events.last().unwrap() else {
            panic!("expected an error terminal");
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("Provider stopped with: guardrail_intervened")
        );

        // MIRROR: a `max_tokens` stop is a clean `done`, not an error.
        let events = collect(
            vec![
                event("messageStart", "{\"role\":\"assistant\"}"),
                event("messageStop", "{\"stopReason\":\"max_tokens\"}"),
            ],
            &sonnet_45(),
        )
        .await;
        let StreamEvent::Done { message, .. } = events.last().unwrap() else {
            panic!("expected a done terminal");
        };
        assert_eq!(message.stop_reason, StopReason::Length);
    }

    /// PORT BUG (present at v0.83.0, never ported): pi writes
    /// `output.rawStopReason = item.messageStop.stopReason`
    /// (`v0.84.1 ai/src/api/bedrock-converse-stream.ts:276`; `v0.83.0 …:270`). Bedrock's mapping is
    /// especially lossy — every unrecognized reason falls into the `_ => (Error, None)` arm of
    /// [`map_stop_reason`] with no message at all, so the raw string is the ONLY record of it.
    #[tokio::test]
    async fn message_stop_records_the_providers_own_stop_reason() {
        let events = collect(
            vec![
                event("messageStart", "{\"role\":\"assistant\"}"),
                event("messageStop", "{\"stopReason\":\"guardrail_intervened\"}"),
            ],
            &sonnet_45(),
        )
        .await;
        let StreamEvent::Error { error, .. } = events.last().unwrap() else {
            panic!("expected an error terminal");
        };
        assert_eq!(
            error.raw_stop_reason.as_deref(),
            Some("guardrail_intervened")
        );

        // MIRROR 1: a clean `end_turn` keeps its raw word on the `done` terminal.
        let events = collect(
            vec![
                event("messageStart", "{\"role\":\"assistant\"}"),
                event("messageStop", "{\"stopReason\":\"end_turn\"}"),
            ],
            &sonnet_45(),
        )
        .await;
        let StreamEvent::Done { message, .. } = events.last().unwrap() else {
            panic!("expected a done terminal");
        };
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("end_turn"));

        // MIRROR 2: pi's assignment at `:276` is UNCONDITIONAL, so a `messageStop` with no
        // `stopReason` writes `undefined` — `None`, not a fabricated placeholder.
        let events = collect(
            vec![
                event("messageStart", "{\"role\":\"assistant\"}"),
                event("messageStop", "{}"),
            ],
            &sonnet_45(),
        )
        .await;
        let StreamEvent::Error { error, .. } = events.last().unwrap() else {
            panic!("expected an error terminal");
        };
        assert_eq!(error.raw_stop_reason, None);
    }

    /// pi `:278-288`: an in-stream exception frame is a throw, prefixed by the legacy label.
    #[tokio::test]
    async fn an_exception_frame_is_terminal_with_the_legacy_prefix() {
        let events = collect(
            vec![
                event("messageStart", "{\"role\":\"assistant\"}"),
                frame(
                    &[
                        (":message-type", "exception"),
                        (":exception-type", "throttlingException"),
                        (":content-type", "application/json"),
                    ],
                    "{\"message\":\"Too many tokens\"}",
                ),
            ],
            &sonnet_45(),
        )
        .await;
        let StreamEvent::Error { error, .. } = events.last().unwrap() else {
            panic!("expected an error terminal");
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("Throttling error: Too many tokens")
        );
    }

    /// pi `:258-262`: a `messageStart` whose role is not `assistant` is fatal.
    #[tokio::test]
    async fn a_user_role_message_start_is_fatal() {
        let events = collect(
            vec![event("messageStart", "{\"role\":\"user\"}")],
            &sonnet_45(),
        )
        .await;
        // pi pushes `start` from `messageStart` (`:262`), so a rejected `messageStart` yields the
        // terminal ALONE — no `start` precedes it.
        assert_eq!(kinds(&events), vec!["error"]);
        let StreamEvent::Error { error, .. } = events.last().unwrap() else {
            panic!("expected an error terminal");
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("Unexpected assistant message start but got user message start instead")
        );
    }

    // -----------------------------------------------------------------------
    // SigV4
    // -----------------------------------------------------------------------

    /// RFC 4231 test case 2 — the standard HMAC-SHA256 vector.
    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Case 3: a 20-byte key of 0xaa over 50 bytes of 0xdd.
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 20], &[0xdd; 50])),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
        // A key longer than the 64-byte block must be hashed first (case 4 of the same RFC uses a
        // 131-byte key).
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    /// AWS's published signing-key derivation example (Signature Version 4 documentation):
    /// secret `wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY`, date `20150830`, region `us-east-1`,
    /// service `iam`.
    #[test]
    fn sigv4_signing_key_derivation_matches_the_aws_example() {
        let k_date = hmac_sha256(
            b"AWS4wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            b"20150830",
        );
        let k_region = hmac_sha256(&k_date, b"us-east-1");
        let k_service = hmac_sha256(&k_region, b"iam");
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        assert_eq!(
            hex(&k_signing),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn sigv4_timestamps_format_both_aws_date_forms() {
        // 2015-08-30T12:36:00Z.
        assert_eq!(
            sigv4_timestamps(1_440_938_160),
            ("20150830".to_string(), "20150830T123600Z".to_string())
        );
        assert_eq!(
            sigv4_timestamps(0),
            ("19700101".to_string(), "19700101T000000Z".to_string())
        );
    }

    #[test]
    fn sigv4_signs_deterministically_and_covers_the_caller_headers() {
        let creds = AwsCredentials {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: Some("sess".to_string()),
        };
        let url = converse_stream_url(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            "arn:aws:bedrock:us-east-1:1:application-inference-profile/x",
        );
        // The ARN's `:` and `/` must be percent-encoded in the path.
        assert!(url.ends_with(
            "/model/arn%3Aaws%3Abedrock%3Aus-east-1%3A1%3Aapplication-inference-profile%2Fx/converse-stream"
        ));

        let sign = |extra: Option<(&str, &str)>| {
            let mut headers: BTreeMap<String, String> = BTreeMap::new();
            headers.insert("content-type".to_string(), "application/json".to_string());
            if let Some((k, v)) = extra {
                headers.insert(k.to_string(), v.to_string());
            }
            sign_sigv4(&mut headers, &url, b"{\"a\":1}", &creds, "us-east-1", 1_440_938_160).unwrap();
            headers
        };

        let base = sign(None);
        assert_eq!(base, sign(None), "signing must be deterministic");
        assert_eq!(
            base.get("x-amz-date").map(String::as_str),
            Some("20150830T123600Z")
        );
        assert_eq!(base.get("x-amz-security-token").map(String::as_str), Some("sess"));
        let auth = base.get("authorization").expect("authorization");
        assert!(auth.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/bedrock/aws4_request, SignedHeaders="
        ));
        assert!(auth.contains("content-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token"));

        // A caller header changes the signature — proving injected headers are covered by it, which
        // is the whole reason upstream registers its middleware at the `build` step.
        let with_extra = sign(Some(("x-allowed", "ok")));
        assert!(with_extra.get("authorization") != base.get("authorization"));
        assert!(
            with_extra
                .get("authorization")
                .expect("authorization")
                .contains("x-allowed")
        );
    }

    #[test]
    fn missing_credentials_are_a_credential_error_not_an_unsigned_request() {
        let config = BedrockClientConfig {
            profile: Some("nope".to_string()),
            region: Some("us-east-1".to_string()),
            endpoint: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            credentials: None,
            bearer_token: None,
        };
        let mut headers = BTreeMap::new();
        let err = authorize(&mut headers, &config, "https://x/y", b"{}").unwrap_err();
        assert!(err.contains("Could not load credentials"));
        assert!(err.contains("nope"));
        assert!(!headers.contains_key("authorization"));
    }

    #[test]
    fn shared_credentials_files_are_read_for_a_configured_profile() {
        let dir = std::env::temp_dir().join(format!("cyrup-bedrock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("credentials");
        std::fs::write(
            &path,
            "[default]\naws_access_key_id = DEFAULTKEY\naws_secret_access_key = defaultsecret\n\n\
             [work]\naws_access_key_id = WORKKEY\naws_secret_access_key = worksecret\naws_session_token = worktoken\n",
        )
        .unwrap();

        let ambient = env_map(&[(
            "AWS_SHARED_CREDENTIALS_FILE",
            path.to_string_lossy().as_ref(),
        )]);
        let env = env_source(None, &ambient);
        assert_eq!(
            shared_profile_credentials("work", &env),
            Some(AwsCredentials {
                access_key_id: "WORKKEY".to_string(),
                secret_access_key: "worksecret".to_string(),
                session_token: Some("worktoken".to_string()),
            })
        );
        assert_eq!(
            shared_profile_credentials("default", &env)
                .map(|c| c.access_key_id),
            Some("DEFAULTKEY".to_string())
        );
        assert_eq!(shared_profile_credentials("absent", &env), None);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn the_command_input_carries_model_id_for_on_payload_then_leaves_the_body() {
        let model = sonnet_45();
        let body = payload(
            &model,
            &user_ctx("hi"),
            &StreamOptions::default(),
            &BedrockOptions::default(),
        );
        assert_eq!(body["modelId"], json!(model.id.as_str()));

        let (id, rest) = split_command_input(body, &model);
        assert_eq!(id, model.id.as_str());
        assert!(rest.get("modelId").is_none());
        assert!(rest.get("messages").is_some());

        // An `onPayload` replacement that rewrites `modelId` must retarget the URL.
        let replaced = json!({ "modelId": "other.model", "messages": [] });
        let (id, _) = split_command_input(replaced, &model);
        assert_eq!(id, "other.model");
    }

    #[test]
    fn the_factory_serves_the_bedrock_api_id() {
        assert_eq!(factory().api().as_str(), "bedrock-converse-stream");
    }
}

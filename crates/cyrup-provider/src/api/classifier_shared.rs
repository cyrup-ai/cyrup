//! What every HTTP classifier api shares: one JSON POST with bearer auth, the `on_payload` /
//! `on_response` hooks, a fresh timeout per attempt and the provider retry policy; the error shapes
//! that policy reads; and token usage priced from the model's catalog (1:1 port of pi
//! `packages/ai/src/api/classifier-shared.ts` @f1b2e77f5, PROV-104).
//!
//! pi keeps two copies of the transport: `llama-cpp-classify.ts` carries its own `post`
//! (`:227-271`), and `classifier-shared.ts` `postClassifierRequest` serves the System One apis and
//! `openai-decisions`. The two differ in exactly three places, which [`post_classifier_request`]
//! keeps and the llama.cpp api's own `post` does not:
//!
//! * an absent or empty api key fails the request at once, `No API key for provider: <id>`
//!   (`classifier-shared.ts`, `if (!options?.apiKey) throw`), where llama.cpp sends no
//!   `authorization` header instead;
//! * the `on_payload` / `on_response` hooks run on every request, not on one marked request;
//! * the request URL is the transport's, not `<root><path>`.
//!
//! Everything below those three — the error shapes, how a failure is rendered, the bounded body
//! read, the per-attempt timeout, cancellation and the retry loop — was the llama.cpp api's
//! private copy and is now this module, parametrized by the service label that pi's two copies
//! hard-code (`const LABEL = "llama.cpp"`; `transport.label`). The llama.cpp api's error text is
//! unchanged by the move: it passes its own label.
//!
//! `noRetryStatuses` (`postClassifierRequest`'s sixth parameter) is used only by
//! `openai-decisions`, which this build does not port; it is not carried.

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use cyrup_core::{CancelToken, Cost, Usage};
use reqwest::header::HeaderMap as ResponseHeaders;
use serde_json::Value;

use crate::HeaderMap;
use crate::classifier::{ClassifierModel, ClassifierOptions};
use crate::stream::ProviderResponse;
use crate::stream::sse::build_client_for_target;
use crate::utils::error_body::{MAX_PROVIDER_ERROR_BODY_CHARS, truncate_error_text};
use crate::utils::headers::provider_headers_to_record;
use crate::utils::provider_retry::{ProviderRetry, is_retryable_provider_error, retry_delay_ms};

// ------------------------------------------------------------------------------------- errors --

/// The `status` / `headers` / `body` pi's request errors carry (`ClassifierHttpError`,
/// `classifier-shared.ts`; `HttpError`, `llama-cpp-classify.ts:67-71`). Only errors that carry
/// these properties take part in the retry policy: pi's `isProviderError`
/// (`provider-retry.ts:15-20`) rejects every other `Error`.
#[derive(Clone, Debug)]
pub(crate) struct ProviderFields {
    status: Option<u16>,
    headers: Option<ResponseHeaders>,
    body: String,
}

/// Every failure of a classification before it is flattened into `error_message`.
#[derive(Clone, Debug)]
pub(crate) struct ClassifyError {
    message: String,
    /// `Some` for [`http_error`] and [`timeout_error`]; `None` for a plain `Error`.
    provider: Option<Box<ProviderFields>>,
}

impl ClassifyError {
    /// A plain `Error` (pi `new Error(message)`).
    pub(crate) fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            provider: None,
        }
    }
}

/// `httpError` (`classifier-shared.ts`; `llama-cpp-classify.ts:73-79`): a non-2xx response with
/// its status, headers and body, `<label> returned <status>`.
pub(crate) fn http_error(
    label: &str,
    status: u16,
    headers: ResponseHeaders,
    body: String,
) -> ClassifyError {
    ClassifyError {
        message: format!("{label} returned {status}"),
        provider: Some(Box::new(ProviderFields {
            status: Some(status),
            headers: Some(headers),
            body,
        })),
    }
}

/// `timeoutError` (`classifier-shared.ts`; `llama-cpp-classify.ts:81-88`): no status and no
/// headers, which makes it retryable.
pub(crate) fn timeout_error(timeout_ms: u64) -> ClassifyError {
    ClassifyError {
        message: format!("Request timed out after {timeout_ms}ms"),
        provider: Some(Box::new(ProviderFields {
            status: None,
            headers: None,
            body: String::new(),
        })),
    }
}

/// `createAbortError` (provider-retry.ts:68-72).
pub(crate) fn aborted_error() -> ClassifyError {
    ClassifyError::plain("Request aborted")
}

/// The display string pi composes for `output.errorMessage`:
/// `formatProviderError(normalizeProviderError(error), "<label> error")`
/// (`system-one-shared.ts` `classifySystemOne`; `llama-cpp-classify.ts:455`; error-body.ts:33-45,
/// :122-130).
///
/// An error with a status and a body that its message does not already carry renders as
/// `<label> error (400): <body>`; with a status only, as `<label> error (500): <message>`; with
/// no status, as the message alone. The body is trimmed, dropped when empty and capped at
/// [`MAX_PROVIDER_ERROR_BODY_CHARS`] (error-body.ts:76-82).
pub(crate) fn format_error(label: &str, error: &ClassifyError) -> String {
    let (status, body) = match &error.provider {
        Some(fields) => {
            let trimmed = fields.body.trim();
            let body = (!trimmed.is_empty())
                .then(|| truncate_error_text(trimmed, MAX_PROVIDER_ERROR_BODY_CHARS));
            (fields.status, body)
        }
        None => (None, None),
    };
    match (status, body) {
        (Some(status), Some(body)) if !error.message.contains(&body) => {
            format!("{label} error ({status}): {body}")
        }
        (Some(status), _) => format!("{label} error ({status}): {}", error.message),
        (None, _) => error.message.clone(),
    }
}

// ------------------------------------------------------------------------------------ requests --

/// `headersToRecord` (utils/headers.ts:3-9): a `Headers` object read as a name to value record.
pub(crate) fn headers_to_record(headers: &ResponseHeaders) -> BTreeMap<String, String> {
    let mut record: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in headers {
        let Ok(value) = value.to_str() else { continue };
        record
            .entry(name.as_str().to_string())
            .and_modify(|existing| {
                existing.push_str(", ");
                existing.push_str(value);
            })
            .or_insert_with(|| value.to_string());
    }
    record
}

/// A `reqwest` failure with its source chain, since reqwest's own `Display` carries only the
/// outermost description.
fn transport_error(error: &reqwest::Error) -> ClassifyError {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        let text = cause.to_string();
        if !text.is_empty() && !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    ClassifyError::plain(message)
}

/// The most a successful reply may carry. A llama.cpp readout of `n_probs` = 32768 is a few
/// megabytes, so this is generous; the cap keeps a misbehaving or hostile server (the base URL is
/// user configured) from growing memory for the length of the timeout. Upstream reads the body
/// unbounded (`llama-cpp-classify.ts:248-257`, `classifier-shared.ts` `next.json()`).
pub(crate) const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
/// How much of a failed reply's body is read: its text is cut to
/// [`MAX_PROVIDER_ERROR_BODY_CHARS`] before it is shown.
const MAX_ERROR_BODY_BYTES: usize = 1024 * 1024;

/// Read `response`'s body up to `limit` bytes. Past the limit the read stops: with `truncate` the
/// prefix read so far is the result, without it the reply is refused.
async fn read_body(
    label: &str,
    response: reqwest::Response,
    limit: usize,
    truncate: bool,
) -> Result<Vec<u8>, ClassifyError> {
    use futures::StreamExt as _;
    let too_large = || ClassifyError::plain(format!("{label} response exceeds {limit} bytes"));
    if !truncate
        && response
            .content_length()
            .is_some_and(|length| usize::try_from(length).map_or(true, |length| length > limit))
    {
        return Err(too_large());
    }
    let mut stream = response.bytes_stream();
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| transport_error(&error))?;
        let room = limit.saturating_sub(body.len());
        if chunk.len() > room {
            if truncate {
                body.extend_from_slice(chunk.get(..room).unwrap_or_default());
                return Ok(body);
            }
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// One attempt of a request: POST, then read the body (`llama-cpp-classify.ts:244-264`;
/// `classifier-shared.ts` `postClassifierRequest`'s attempt). The whole attempt, body read
/// included, is bound by `timeoutMs` (`AbortSignal.timeout`), and cancellation wins over a timeout
/// that fires at the same time (`llama-cpp-classify.ts:261`).
pub(crate) async fn send_once(
    label: &str,
    client: &reqwest::Client,
    options: &ClassifierOptions,
    url: &str,
    headers: &[(String, String)],
    payload: &Value,
) -> Result<(u16, ResponseHeaders, Value), ClassifyError> {
    let work = async {
        let body =
            serde_json::to_vec(payload).map_err(|error| ClassifyError::plain(error.to_string()))?;
        let mut builder = client.post(url).body(body);
        for (name, value) in headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let response = builder
            .send()
            .await
            .map_err(|error| transport_error(&error))?;
        let status = response.status();
        let response_headers = response.headers().clone();
        if !status.is_success() {
            // An error body is only ever shown truncated, so a long one is cut here, not refused.
            let bytes = read_body(label, response, MAX_ERROR_BODY_BYTES, true).await?;
            let body = String::from_utf8_lossy(&bytes).into_owned();
            return Err(http_error(label, status.as_u16(), response_headers, body));
        }
        let bytes = read_body(label, response, MAX_RESPONSE_BYTES, false).await?;
        let json: Value = serde_json::from_slice(&bytes)
            .map_err(|error| ClassifyError::plain(error.to_string()))?;
        Ok((status.as_u16(), response_headers, json))
    };
    let timed = async {
        match options.timeout_ms {
            Some(timeout_ms) => {
                match tokio::time::timeout(Duration::from_millis(timeout_ms), work).await {
                    Ok(outcome) => outcome,
                    Err(_elapsed) => Err(timeout_error(timeout_ms)),
                }
            }
            None => work.await,
        }
    };
    tokio::select! {
        biased;
        () = cancelled(options.cancel.as_ref()) => Err(aborted_error()),
        outcome = timed => outcome,
    }
}

/// Resolves when `cancel` fires; never for no token (pi's absent `signal`).
async fn cancelled(cancel: Option<&CancelToken>) {
    match cancel {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

/// `retryProviderRequest` (provider-retry.ts:104-125), over the shared decisions in
/// [`crate::utils::provider_retry`].
///
/// A failed attempt is retried only when it is a provider error (see [`ProviderFields`]), is
/// retryable by status or `x-should-retry`, and a retry is left. The delay is the server's
/// `retry-after-ms` / `retry-after` or a jittered exponential backoff; a server delay above
/// `max_retry_delay_ms` fails the request at once. Cancellation is checked first and the sleep is
/// interruptible; both end in `Request aborted`.
pub(crate) async fn retry_request<T, F, Fut>(
    mut request: F,
    retry: ProviderRetry,
    cancel: Option<&CancelToken>,
) -> Result<T, ClassifyError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ClassifyError>>,
{
    let max_retries = retry.max_retries;
    let mut retries_remaining = max_retries;
    loop {
        let error = match request().await {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(aborted_error());
        }
        let Some(fields) = &error.provider else {
            return Err(error);
        };
        if retries_remaining == 0
            || !is_retryable_provider_error(fields.status, fields.headers.as_ref())
        {
            return Err(error);
        }
        let retry_index = max_retries - retries_remaining;
        retries_remaining -= 1;
        let delay = retry_delay_ms(fields.headers.as_ref(), &error.message, retry_index, retry)
            .map_err(|failure| ClassifyError::plain(failure.to_string()))?;
        let slept = tokio::select! {
            biased;
            () = cancelled(cancel) => false,
            () = tokio::time::sleep(Duration::from_millis(delay)) => true,
        };
        if !slept {
            return Err(aborted_error());
        }
    }
}

/// `postClassifierRequest` (`classifier-shared.ts` @f1b2e77f5): posts one JSON classifier request
/// with bearer auth, the `on_payload` / `on_response` hooks, a fresh timeout per attempt and
/// provider retries, and returns the parsed response body.
///
/// In pi's order: no api key fails first (`No API key for provider: <id>`); the payload goes
/// through `on_payload` once, not per retry; the headers are
/// `{authorization: Bearer <key>, content-type: application/json}` overlaid by the model's and
/// then the request's headers, case-insensitively, a `None` value suppressing one
/// (`providerHeadersToRecord`); `on_response` sees the final response's status and headers.
pub(crate) async fn post_classifier_request(
    label: &str,
    url: &str,
    model: &ClassifierModel,
    body: Value,
    options: &ClassifierOptions,
) -> Result<Value, ClassifyError> {
    let Some(api_key) = options.api_key.as_deref().filter(|key| !key.is_empty()) else {
        return Err(ClassifyError::plain(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };
    let payload = options.apply_on_payload(model, body).await;
    let mut base = HeaderMap::new();
    base.insert(
        "authorization".to_string(),
        Some(format!("Bearer {api_key}")),
    );
    base.insert(
        "content-type".to_string(),
        Some("application/json".to_string()),
    );
    let headers = provider_headers_to_record(&[
        Some(&base),
        model.headers.as_ref(),
        options.headers.as_ref(),
    ]);
    let client = build_client_for_target(
        url,
        &crate::auth::types::EnvAuthContext,
        options.env.as_ref(),
        None,
    )
    .await
    .map_err(|error| ClassifyError::plain(error.to_string()))?;
    let retry = ProviderRetry {
        max_retries: options.max_retries,
        max_retry_delay_ms: options.max_retry_delay_ms,
    };
    let (status, response_headers, json) = retry_request(
        || send_once(label, &client, options, url, &headers, &payload),
        retry,
        options.cancel.as_ref(),
    )
    .await?;
    options
        .emit_on_response(
            model,
            ProviderResponse {
                status,
                headers: headers_to_record(&response_headers),
            },
        )
        .await;
    Ok(json)
}

// --------------------------------------------------------------------------------- body reading --

/// `requiredNumber` (`classifier-shared.ts`): a finite JSON number, or
/// `<label> returned an invalid <field>`.
pub(crate) fn required_number(
    label: &str,
    value: Option<&Value>,
    field: &str,
) -> Result<f64, ClassifyError> {
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
        .ok_or_else(|| ClassifyError::plain(format!("{label} returned an invalid {field}")))
}

/// `tokenCount` (`classifier-shared.ts`): a finite positive number, else `0`.
///
/// `[CYRUP-DELTA, type]` pi's `Usage` holds JS numbers, so a fractional count survives as a
/// fraction; [`Usage`]'s counts are `u64`, so one is truncated toward zero here. No service is
/// known to report a fractional token count.
fn token_count(value: Option<&Value>) -> u64 {
    match value.and_then(Value::as_f64) {
        Some(count) if count.is_finite() && count > 0.0 => {
            // A finite positive f64 saturates at `u64::MAX` under `as`, which is the intent.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let count = count as u64;
            count
        }
        _ => 0,
    }
}

/// `parseClassifierUsage` (`classifier-shared.ts` @f1b2e77f5): usage from an
/// `{input_tokens, output_tokens}` object, priced from the model's catalog like chat usage
/// (`calculateCost`, here [`crate::usage::apply_cost`]). A missing or malformed usage object —
/// not an object, or one carrying neither count — leaves the result without usage instead of
/// failing it; a malformed count reads as `0`.
pub(crate) fn parse_classifier_usage(
    value: Option<&Value>,
    model: &ClassifierModel,
) -> Option<Usage> {
    let object = value?.as_object()?;
    if !object.contains_key("input_tokens") && !object.contains_key("output_tokens") {
        return None;
    }
    let mut usage = Usage {
        input: token_count(object.get("input_tokens")),
        output: token_count(object.get("output_tokens")),
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 0,
        cost: Cost::default(),
    };
    crate::usage::apply_cost(&model.cost, &mut usage);
    Some(usage)
}

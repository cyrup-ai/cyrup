//! pi's `stream()` try block (`bedrock-converse-stream.ts:222-303`): request build, the retry
//! loop, the frame loop and the terminal event.

use super::blocks::Decoder;
use super::config::resolve_client_config;
use super::env::EnvSource;
use super::errors::{format_bedrock_error, format_bedrock_service_error};
use super::events::dispatch_frame;
use super::failure::{
    BedrockFailure, append_bedrock_failure_diagnostic, normalize_diagnostic_value,
};
use super::framing::EventStreamDecoder;
use super::headers::{apply_custom_headers, authorize};
use super::options::BedrockOptions;
use super::params::{build_params, resolve_cache_retention};
use super::url::converse_stream_url;
use crate::api::EventSink;
use crate::auth::AuthResult;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::build_client_for_target_forcing_http1;
use crate::stream::{StreamEvent, StreamOptions};
use crate::utils::error_body::normalize_error_body;
use crate::utils::provider_retry::{ProviderRetry, is_retryable_provider_error, retry_delay_ms};
use cyrup_core::{ApiId, CancelToken, StopReason};
use futures::StreamExt;
use serde_json::Value;
use std::collections::BTreeMap;

/// Response media type of `ConverseStream` — the AWS binary event stream the SDK decodes for
/// upstream and [`EventStreamDecoder`] decodes here.
const EVENT_STREAM_MEDIA_TYPE: &str = "application/vnd.amazon.eventstream";

/// Retries after the first attempt on the Bedrock route. The AWS SDK v3 **standard** retry mode
/// makes 3 attempts, and pi's client config (`bedrock-converse-stream.ts:150-222` @v0.83.0) never
/// overrides `maxAttempts`/`retryStrategy`, so that is what pi inherits per turn (PROV-043).
const BEDROCK_STANDARD_MODE_RETRIES: u32 = 2;

/// pi's `stream()` try block (`bedrock-converse-stream.ts:222-303`).
//
// The `Err` variant, [`BedrockFailure`], is at least 416 bytes because it carries the decoder
// snapshot the caller needs to emit a partial-turn error, so it is BOXED (`clippy::result_large_err`)
// to keep the success path's `Result` small. `BedrockFailure` is `pub(super)` and this driver's only
// caller is the catch arm in `mod.rs`, so the box is module-internal and changes no public type.
pub(super) async fn run_inner(
    model: &Model,
    ctx: &Context,
    auth: &AuthResult,
    opts: &StreamOptions,
    cancel: &CancelToken,
    sink: &EventSink,
    api: &ApiId,
) -> Result<(), Box<BedrockFailure>> {
    let bedrock = BedrockOptions::from_stream_options(opts);
    let env = EnvSource::new(opts.env.as_ref().or(auth.env.as_ref()));
    let mut dec = Decoder::default();

    let config = resolve_client_config(model, opts, &bedrock, auth, &env);

    // `cacheRetention` + payload (pi `:228-241`).
    let cache_retention = resolve_cache_retention(opts.cache_retention, &env);
    let payload = build_params(model, ctx, opts, &bedrock, cache_retention, &env).map_err(|e| {
        BedrockFailure::errored(dec.snapshot_owned(model, api), format_bedrock_error(&e))
    })?;

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
        BedrockFailure::errored(dec.snapshot_owned(model, api), format_bedrock_error(&e))
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
        BedrockFailure::errored(
            dec.snapshot_owned(model, api),
            format_bedrock_error(&e.to_string()),
        )
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
    let aborted = |dec: &mut Decoder| {
        Box::new(BedrockFailure {
            partial: dec.snapshot_owned(model, api),
            stop_reason: StopReason::Aborted,
            message: "Request was aborted".to_string(),
            status: None,
            error_code: None,
            request_id: None,
        })
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
            return Err(aborted(&mut dec));
        }

        let (retry_headers, message) = match attempt {
            Err(ProviderError::Aborted) => return Err(aborted(&mut dec)),
            // A transport failure carries no status: `error.status === undefined` ⇒ retryable.
            Err(transport) => {
                if retries_remaining == 0 {
                    return Err(Box::new(BedrockFailure::errored(
                        dec.snapshot_owned(model, api),
                        format_bedrock_error(&transport.to_string()),
                    )));
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
        let delay =
            retry_delay_ms(retry_headers.as_ref(), &message, retry_index, retry).map_err(|e| {
                BedrockFailure::errored(
                    dec.snapshot_owned(model, api),
                    format_bedrock_error(&e.to_string()),
                )
            })?;
        // Interruptible backoff, unlike the SDK's own retry timers.
        if cancel
            .run_until_cancelled(tokio::time::sleep(std::time::Duration::from_millis(delay)))
            .await
            .is_none()
        {
            return Err(aborted(&mut dec));
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
        return Err(Box::new(
            BedrockFailure::service_exception(
                dec.snapshot_owned(model, api),
                format_bedrock_service_error(name, status, &body),
                status,
                name,
            )
            .with_request_id(response_request_id.as_deref()),
        ));
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
            return Err(Box::new(BedrockFailure {
                partial: dec.snapshot_owned(model, api),
                stop_reason: StopReason::Aborted,
                message: "Request was aborted".to_string(),
                status: None,
                error_code: None,
                request_id: response_request_id.clone(),
            }));
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| {
            BedrockFailure::errored(
                dec.snapshot_owned(model, api),
                format_bedrock_error(&format!("transport error: {e}")),
            )
            .with_request_id(response_request_id.as_deref())
        })?;
        frames.push(&chunk);
        loop {
            let frame = frames.next_frame().map_err(|e| {
                BedrockFailure::errored(dec.snapshot_owned(model, api), format_bedrock_error(&e))
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
                    return Err(Box::new(
                        BedrockFailure::errored(dec.snapshot_owned(model, api), message)
                            .with_request_id(response_request_id.as_deref()),
                    ));
                }
            }
        }
    }

    // pi `:291-293`: an aborted signal after the loop is still terminal.
    if cancel.is_cancelled() {
        return Err(Box::new(BedrockFailure {
            partial: dec.snapshot_owned(model, api),
            stop_reason: StopReason::Aborted,
            message: "Request was aborted".to_string(),
            status: None,
            error_code: None,
            request_id: response_request_id.clone(),
        }));
    }

    // pi `:295-300`: a stream that ended still "pending" is TRUNCATED, and a settled
    // `error`/`aborted` stop reason throws with the recorded message. `end_of_stream` encodes both
    // (a `None` stop reason becomes the `error` terminal with the truncation text; a settled
    // `Error` routes to the same terminal carrying `error_message`).
    let mut message = dec.snapshot_owned(model, api);
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
        append_bedrock_failure_diagnostic(&mut message, None, None, response_request_id.as_deref());
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
pub(super) fn split_command_input(payload: Value, model: &Model) -> (String, Value) {
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

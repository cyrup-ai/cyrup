//! `ApiImpl`.

use super::events::map_codex_frames;
use super::headers::{build_sse_headers, extract_account_id};
use super::options::OpenAiCodexResponsesOptions;
use super::request::build_request_body;
use super::retry::{
    backoff_delay_ms, get_retry_after_delay_ms, is_retryable_error, parse_error_response,
    validate_retry_delay_ms,
};
use super::terminals::{aborted_event, error_event, sleep_or_abort};
use super::url::{codex_session_id, resolve_codex_url, resolved_base_url};
use super::{CodexResponsesApi, DEFAULT_MAX_RETRIES, FrameStream};
use crate::api::openai_responses::decode_stream;
use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::StreamOptions;
use crate::stream::sse::{SseRequest, build_client_for_target, open_sse};
use crate::utils::provider_plumbing::now_millis;
use crate::utils::provider_retry::ProviderRetry;
use cyrup_core::{ApiId, CancelToken};
use std::sync::{Arc, Mutex};

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
        // PROV-011: an unsatisfiable `constrainedSampling` fails the turn before any HTTP.
        let params =
            match build_request_body(model, ctx, opts, &codex_opts, codex_session_id.as_deref()) {
                Ok(p) => p,
                Err(e) => {
                    sink.send(error_event(model, &self.api, e.0, false)).await;
                    return;
                }
            };
        // gap-08 #2: `before_provider_request` may inspect/replace the outbound body (pi
        // `options?.onPayload?.(body, model)`, :284-287).
        let body = crate::stream::apply_on_payload(opts, model, params).await;

        // PROV-042: `transformHeaders` runs LAST over the fully-assembled set (pi
        // `models.ts:657` @v0.84.4); its return value is what goes on the wire.
        let headers = crate::stream::apply_transform_headers(
            opts,
            build_sse_headers(
                model,
                auth,
                opts,
                &account_id,
                &api_key,
                codex_session_id.as_deref(),
            ),
        )
        .await;
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

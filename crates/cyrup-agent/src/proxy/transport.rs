//! Transport (Pi `streamProxy`, proxy.ts:116-233).

use super::builder::ProxyMessageBuilder;
use super::options::{build_proxy_request_options, model_wire, ProxyStreamOptions};
use super::proxy_error_message;
use super::wire::ProxyAssistantMessageEvent;
use cyrup_core::{CancelToken, EventStream, ModelRef};
use cyrup_provider::stream::ErrorReason;
use cyrup_provider::{open_sse, Context, SseRequest, StreamEvent};
use futures::StreamExt;

/// Stream a model call through an auth-managing proxy server (1:1 port of Pi `streamProxy`,
/// proxy.ts:116-233). `POST {proxyUrl}/api/stream` with `Authorization: Bearer {authToken}` and the
/// `{ model, context, options }` body; decode the SSE `data:` frames as
/// [`ProxyAssistantMessageEvent`]s; rebuild the partial client-side via [`ProxyMessageBuilder`] and
/// re-emit the agent-facing [`StreamEvent`] stream.
///
/// Like every cyrup stream source it NEVER returns `Err`: a transport/HTTP failure, a malformed
/// frame, or a content-type mismatch arrives as a terminal `StreamEvent::Error` (Pi pushes a
/// terminal `error` event from its `catch`, proxy.ts:214-224). Abort (a cancelled `cancel` token)
/// yields the `aborted` reason, matching Pi's `signal?.aborted ? "aborted" : "error"`
/// (proxy.ts:216).
pub fn stream_proxy(
    model: ModelRef,
    context: Context,
    options: ProxyStreamOptions,
) -> EventStream<StreamEvent> {
    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(16);
    tokio::spawn(run_proxy(model, context, options, tx));
    Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    }))
}

async fn run_proxy(
    model: ModelRef,
    context: Context,
    options: ProxyStreamOptions,
    tx: tokio::sync::mpsc::Sender<StreamEvent>,
) {
    let cancel = options.cancel.clone().unwrap_or_default();
    let mut builder = ProxyMessageBuilder::new(&model);

    // Build the request: POST {proxyUrl}/api/stream, Bearer auth, JSON body (proxy.ts:152-164).
    let body = serde_json::json!({
        "model": model_wire(&model),
        "context": context,
        "options": build_proxy_request_options(&options),
    });
    let url = format!("{}/api/stream", options.proxy_url);
    let req = SseRequest::post_json(url.clone(), body)
        .header("Authorization", format!("Bearer {}", options.auth_token));

    // PROV-047 — proxy-aware, per target. Pi's `applyHttpProxySettings` writes the `httpProxy`
    // setting into `process.env` (http-dispatcher.ts:43-48 @v0.83.0) and installs an
    // `EnvHttpProxyAgent` as the GLOBAL undici dispatcher (`:79-93`), which `globalThis.fetch` then
    // runs on (`:103`) — so the bare `fetch` in `proxy.ts:165` is proxied like every other request
    // in the process. `build_client()` consulted neither the ported resolver nor
    // `configure_http_proxy` and did not call `.no_proxy()`, so this transport both ignored the
    // `httpProxy` setting entirely and let reqwest's own competing env detection decide for the
    // env-var case. `build_client_for` runs `resolveHttpProxyUrlForTarget` (node-http-proxy.ts:92-112)
    // against the proxy-stream URL, so the same authority decides here as for provider streams.
    // `build_client_for_target` rather than `build_client_for` so the provider-scoped overlay in
    // `options.env` participates in the decision, exactly as it does for provider streams. With
    // `env: None` the two are identical — same `EnvAuthContext`, same ported resolver.
    let client = match cyrup_provider::build_client_for_target(
        &url,
        &cyrup_provider::EnvAuthContext,
        options.env.as_ref(),
        None,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(error_terminal(&builder, &cancel, e.to_string())).await;
            return;
        }
    };

    // `open_sse` maps a non-2xx response / transport failure / connect-time cancel to a typed error
    // (Pi throws on `!response.ok`, proxy.ts:166-177). No request-level retry: Pi's `proxy.ts` calls
    // `fetch` directly, outside `retryProviderRequest`, and relies on the global dispatcher's idle
    // timeout — which the client above carries — to bound a stalled proxy.
    let mut frames = match open_sse(
        &client,
        req,
        cancel.clone(),
        None,
        None,
        cyrup_provider::ProviderRetry::NONE,
    )
    .await
    {
        Ok(s) => s,
        // AGENT-013 — this is pi's `if (!response.ok)` branch (proxy.ts:166-177 @v0.83.0): a non-2xx
        // response becomes `ProviderError::Http { status, message }` here, and pi's two-tier message
        // (`Proxy error: {status} {statusText}`, upgraded to `Proxy error: {errorData.error}` when
        // the body parses) is what the transcript must show. Every other failure mode reaching this
        // arm — connect failure, TLS, connect-time cancel — is one pi surfaces through its outer
        // catch as the raw thrown message, which `proxy_error_message` passes through unchanged.
        Err(e) => {
            let _ = tx.send(error_terminal(&builder, &cancel, proxy_error_message(&e))).await;
            return;
        }
    };

    while let Some(frame) = frames.next().await {
        let frame = match frame {
            Ok(f) => f,
            // A mid-stream transport error / cancellation (Pi: the read loop throws, proxy.ts:184).
            // AGENT-035 — route through `proxy_error_message` so a cancellation carries pi's own
            // `"Request aborted by user"` (proxy.ts:186-190) rather than `ProviderError::Aborted`'s
            // bare `Display`. Non-abort variants are unaffected: only `Http` and `Aborted` have
            // arms, and `Http` cannot arise once the response headers are already accepted.
            Err(e) => {
                let _ = tx.send(error_terminal(&builder, &cancel, proxy_error_message(&e))).await;
                return;
            }
        };
        // The SSE decoder already strips the `data: ` prefix (Pi does it by hand, proxy.ts:196-197).
        // Empty `data` payloads are skipped (Pi `if (data)`, proxy.ts:198).
        if frame.data.is_empty() {
            continue;
        }
        let proxy_event: ProxyAssistantMessageEvent = match serde_json::from_str(&frame.data) {
            Ok(ev) => ev,
            // A malformed frame: Pi's `JSON.parse` throws into the outer catch (proxy.ts:199,214).
            Err(e) => {
                let _ = tx.send(error_terminal(&builder, &cancel, e.to_string())).await;
                return;
            }
        };
        match builder.process(proxy_event) {
            Ok(Some(event)) => {
                if tx.send(event).await.is_err() {
                    // Consumer dropped (the agent stopped reading): nothing left to do.
                    return;
                }
            }
            Ok(None) => {}
            // A content-type mismatch: Pi `throw`s into the outer catch (proxy.ts:261 etc.).
            Err(msg) => {
                let _ = tx.send(error_terminal(&builder, &cancel, msg)).await;
                return;
            }
        }
    }
    // AGENT-035 — pi's SECOND hand-written abort check, the one after the read loop drains:
    // `if (options.signal?.aborted) { throw new Error("Request aborted by user"); }`
    // (`packages/agent/src/proxy.ts:208-211` @v0.83.0, `:210-213` @v0.84.1) — it sits between the
    // loop and `stream.end()` at `:212`, so an abort that lands after the last frame still produces
    // a terminal `error` event with `stopReason: "aborted"` instead of a silent clean close.
    // Reachable here for the same reason it is upstream: the frame stream's own cancel branch only
    // fires while a poll is outstanding, so a cancel landing after it returned `None` is seen only
    // by this check.
    if cancel.is_cancelled() {
        let _ = tx
            .send(error_terminal(&builder, &cancel, "Request aborted by user".to_string()))
            .await;
    }
    // Clean end: the `done`/`error` event already carried the terminal (proxy.ts:213). Dropping `tx`
    // ends the stream.
}

/// Build a terminal `error` event from the partial assembled so far (Pi sets
/// `partial.stopReason`/`errorMessage` then pushes `{type:"error", error: partial}`, proxy.ts:217-223).
/// The reason is `aborted` iff the request was cancelled, else `error` (Pi `signal?.aborted`,
/// proxy.ts:216).
fn error_terminal(
    builder: &ProxyMessageBuilder,
    cancel: &CancelToken,
    message: String,
) -> StreamEvent {
    let reason =
        if cancel.is_cancelled() { ErrorReason::Aborted } else { ErrorReason::Error };
    let mut error = builder.partial().clone();
    error.stop_reason = reason.into();
    error.error_message = Some(message);
    StreamEvent::Error { reason, error }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::proxy::model;
    use cyrup_core::StopReason;
    use futures::StreamExt;

    #[tokio::test]
    async fn transport_connection_failure_yields_terminal_error_event() {
        // Port 1 (tcpmux) is not listening locally → connection refused → terminal error event,
        // never an Err return (cyrup stream contract; Pi pushes a terminal `error`, proxy.ts:214).
        let opts = ProxyStreamOptions {
            proxy_url: "http://127.0.0.1:1".into(),
            auth_token: "t".into(),
            ..ProxyStreamOptions::default()
        };
        let mut stream = stream_proxy(model(), Context::default(), opts);
        let mut last = None;
        while let Some(ev) = stream.next().await {
            last = Some(ev);
        }
        match last {
            Some(StreamEvent::Error { reason: ErrorReason::Error, error }) => {
                assert!(error.error_message.is_some());
                assert_eq!(error.provider.as_str(), "anthropic");
            }
            other => panic!("expected terminal error event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_cancelled_request_yields_aborted_terminal() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let opts = ProxyStreamOptions {
            proxy_url: "http://127.0.0.1:1".into(),
            auth_token: "t".into(),
            cancel: Some(cancel),
            ..ProxyStreamOptions::default()
        };
        let mut stream = stream_proxy(model(), Context::default(), opts);
        let mut last = None;
        while let Some(ev) = stream.next().await {
            last = Some(ev);
        }
        // A cancelled token → Pi's `signal?.aborted ? "aborted" : "error"` → aborted (proxy.ts:216).
        match last {
            Some(StreamEvent::Error { reason: ErrorReason::Aborted, error }) => {
                assert_eq!(error.stop_reason, StopReason::Aborted);
            }
            other => panic!("expected aborted terminal event, got {other:?}"),
        }
    }
}

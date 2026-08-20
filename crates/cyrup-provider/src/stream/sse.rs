//! Direct-wire HTTP + SSE transport (arch-01 §7.1: `reqwest` + `rustls`, no native-tls, +
//! `eventsource-stream`).
//!
//! [`open_sse`] opens a request, exposes `on_request`/`on_response` observability hooks, retries the
//! response-head phase under the caller's [`ProviderRetry`] budget, maps a non-2xx response or a
//! transport failure to a typed [`ProviderError`] (the caller turns it into a terminal
//! `StreamEvent::Error`), and yields decoded SSE frames as an async stream that honors the
//! [`CancelToken`] (cancellation yields a single [`ProviderError::Aborted`] then ends).
//!
//! # The idle timeout
//!
//! Every client this module builds carries an idle timeout, so a provider that accepts the TCP
//! connection and then stalls — before the headers or mid-stream — can never hang a turn forever.
//! This is cyrup's stand-in for Pi's `configureHttpDispatcher` (`http-dispatcher.ts:79-104`), which
//! installs a process-global undici dispatcher with `headersTimeout` **and** `bodyTimeout` set to
//! `httpIdleTimeoutMs` (default [`DEFAULT_HTTP_IDLE_TIMEOUT_MS`] = 5 minutes) and is called
//! unconditionally at startup (`cli.ts:18`, `main.ts:538`) and again whenever the setting changes
//! (`main.ts:802`, `interactive-mode.ts:1778`).
//!
//! [`configure_http_idle_timeout`] is that call; [`http_idle_timeout_ms`] is the resulting global
//! default, and a request may override it with `StreamOptions.timeout_ms` (Pi threads the same value
//! into the SDK client's `timeout` on top of the global dispatcher, `sdk.ts:304-309`). In both
//! systems `0` means *disabled*, not *immediate*.
//!
//! `[CYRUP-DELTA]` reqwest has no dispatcher to install globally, so the timeout is applied per
//! client via [`reqwest::ClientBuilder::read_timeout`]. Its semantics are the closest faithful match
//! available and cover both of undici's knobs with one value: the timer is armed when the request is
//! dispatched and fails the request if the response head has not arrived (undici `headersTimeout`),
//! then re-arms per body frame and **resets after every successful read** (undici `bodyTimeout`), so
//! a long but productive stream is never cut off. What cyrup does *not* reproduce is undici's
//! separate 10 s `connectTimeout` default: here the connect phase is covered by the same idle
//! deadline rather than a shorter one, which is stricter than no bound and looser than undici's.
//! A total-request deadline ([`reqwest::ClientBuilder::timeout`]) would be *wrong* — it would kill a
//! healthy long generation at the 5-minute mark.

use crate::HeaderMap;
use crate::error::ProviderError;
use crate::utils::error_body::normalize_error_body;
use crate::utils::provider_retry::{
    ProviderRetry, is_retryable_provider_error, retry_delay_ms,
};
use bytes::Bytes;
use cyrup_core::CancelToken;
use eventsource_stream::{Event as EsEvent, EventStreamError, Eventsource};
use futures::{Stream, StreamExt};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Pi `DEFAULT_HTTP_IDLE_TIMEOUT_MS` (http-dispatcher.ts:4) — the value
/// `configureHttpDispatcher()` installs when called with no argument.
pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: u64 = 300_000;

/// The process-global HTTP idle timeout, in ms. `0` disables it.
static HTTP_IDLE_TIMEOUT_MS: AtomicU64 = AtomicU64::new(DEFAULT_HTTP_IDLE_TIMEOUT_MS);

/// Install the process-global HTTP idle timeout (Pi `configureHttpDispatcher`,
/// http-dispatcher.ts:79-104). `0` disables it, exactly as `httpIdleTimeoutMs: 0` /
/// `"disabled"` does upstream.
///
/// Applies to every client [`build_client`], [`build_client_with_proxy`] and
/// [`build_client_for_target`] hand out from the next call onward — provider traffic, model-catalog
/// refreshes and image requests alike — mirroring the fact that Pi's dispatcher is global to the
/// process. Already-built clients keep the timeout they were built with (Pi's `setGlobalDispatcher`
/// likewise does not retune in-flight requests).
pub fn configure_http_idle_timeout(timeout_ms: u64) {
    HTTP_IDLE_TIMEOUT_MS.store(timeout_ms, Ordering::Relaxed);
}

/// The current process-global HTTP idle timeout in ms (`0` = disabled).
pub fn http_idle_timeout_ms() -> u64 {
    HTTP_IDLE_TIMEOUT_MS.load(Ordering::Relaxed)
}

/// The process-global `httpProxy` setting (PROV-047).
static HTTP_PROXY_SETTING: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// Install the `httpProxy` setting process-wide — 1:1 with pi `applyHttpProxySettings`
/// (`coding-agent/src/core/http-dispatcher.ts:43-48` @v0.83.0, `:45-50` @v0.84.1).
///
/// pi does this by writing the **process environment**: `process.env.HTTP_PROXY ??= proxy;
/// process.env.HTTPS_PROXY ??= proxy` (`:46-47`), called at startup from `cli.ts:18` /
/// `rpc-entry.ts:10` and re-applied from `main.ts:744-745`. Because it lands in the env, EVERY
/// later proxy consultation sees it — the SDK dispatcher, `fetch`, OAuth token exchange, extension
/// HTTP — not just the streaming wire APIs.
///
/// cyrup cannot mutate its own environment safely (`std::env::set_var` is `unsafe` from edition
/// 2024 and is a data race against every concurrently-running thread's `getenv`), so the value is
/// stored here instead and consulted by
/// [`crate::utils::node_http_proxy::resolve_http_proxy_url_for_target`] at exactly the layer pi's
/// env write is observed: as the value of `HTTP_PROXY`/`HTTPS_PROXY` when the ambient environment
/// supplies neither. `??=` means an ambient variable WINS, and that precedence is preserved.
///
/// `None` clears the setting. Applies to every client built from the next call onward.
pub fn configure_http_proxy(proxy: Option<String>) {
    let normalized = proxy.filter(|p| !p.trim().is_empty());
    if let Ok(mut guard) = HTTP_PROXY_SETTING.write() {
        *guard = normalized;
    }
}

/// The configured `httpProxy`, if any. Read by the ported proxy resolver; not a public precedence
/// decision of its own.
pub fn configured_http_proxy() -> Option<String> {
    HTTP_PROXY_SETTING
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}

/// Resolve a per-request override against the global default. `None` ⇒ the global default;
/// `Some(0)` ⇒ disabled; `Some(n)` ⇒ `n` ms (Pi: `options?.timeoutMs ?? …`, with `0` meaning "no
/// timeout" at both layers).
fn resolve_idle_timeout(override_ms: Option<u64>) -> Option<Duration> {
    match override_ms.unwrap_or_else(http_idle_timeout_ms) {
        0 => None,
        ms => Some(Duration::from_millis(ms)),
    }
}

/// Apply the resolved idle timeout to a client builder.
fn with_idle_timeout(
    builder: reqwest::ClientBuilder,
    override_ms: Option<u64>,
) -> reqwest::ClientBuilder {
    match resolve_idle_timeout(override_ms) {
        Some(d) => builder.read_timeout(d),
        None => builder,
    }
}

/// Inspect (and log/route) the outbound request before send (func-01 R-01-048-adjacent).
pub type OnRequest = Arc<dyn Fn(&SseRequest) + Send + Sync>;
/// Inspect HTTP status + headers once the response opens (func-01 R-01-049).
pub type OnResponse = Arc<dyn Fn(u16, &reqwest::header::HeaderMap) + Send + Sync>;

/// An outbound SSE request description.
#[derive(Clone, Debug)]
pub struct SseRequest {
    pub method: reqwest::Method,
    pub url: String,
    /// Request headers. A `None` value suppresses a would-be default header (func-01 §4.1).
    pub headers: HeaderMap,
    pub body: Option<serde_json::Value>,
}

impl SseRequest {
    /// A `POST` with a JSON body (the common vendor case).
    pub fn post_json(url: impl Into<String>, body: serde_json::Value) -> Self {
        SseRequest {
            method: reqwest::Method::POST,
            url: url.into(),
            headers: HeaderMap::new(),
            body: Some(body),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), Some(value.into()));
        self
    }
}

/// One decoded SSE frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseFrame {
    /// The `event:` field (`"message"` when unspecified, per the SSE spec).
    pub event: String,
    /// The `data:` payload.
    pub data: String,
}

/// Build a proxy-aware HTTP client for one target URL, with the process-global idle timeout
/// (PROV-047).
///
/// This is [`build_client`]'s replacement for every **non-streaming** egress path — the OAuth
/// flows and the non-streaming provider dispatch in [`crate::wire`] — and it exists because
/// `build_client()` consults neither the ported resolver nor `configure_http_proxy`, so a user who
/// configured `httpProxy` the documented way got working model streaming and a hard failure on
/// login, on every silent token refresh, and on catalog refreshes.
///
/// Ambient `HTTP(S)_PROXY`/`ALL_PROXY`/`NO_PROXY` are read through [`EnvAuthContext`], so the same
/// ported resolver decides for these requests as for provider streams — retiring the second,
/// competing `no_proxy` implementation reqwest's own env detection was applying to them.
///
/// [`EnvAuthContext`]: crate::auth::types::EnvAuthContext
pub async fn build_client_for(target_url: &str) -> Result<reqwest::Client, ProviderError> {
    build_client_for_target(
        target_url,
        &crate::auth::types::EnvAuthContext,
        None,
        // Non-streaming requests take the process-global idle timeout, as pi's global dispatcher
        // applies its `bodyTimeout`/`headersTimeout` to every request in the process.
        None,
    )
    .await
}

/// Build the shared HTTP client (arch-01 §7.1: rustls-tls, no native-tls), carrying the
/// process-global idle timeout ([`configure_http_idle_timeout`]).
///
/// **Proxy-blind.** It applies neither the ported resolver nor [`configure_http_proxy`], and it does
/// NOT call [`reqwest::ClientBuilder::no_proxy`], so reqwest's own env detection decides — a second
/// `no_proxy`/`all_proxy` implementation inside the same process. Prefer [`build_client_for`]
/// whenever a target URL is known; this remains for clients that genuinely have no single target.
pub fn build_client() -> Result<reqwest::Client, ProviderError> {
    with_idle_timeout(reqwest::Client::builder(), None)
        .build()
        .map_err(|e| ProviderError::Transport(Box::new(e)))
}

/// Build an HTTP client whose proxy is fully determined by `proxy` — mirroring Pi, which proxies a
/// request **iff** `resolveHttpProxyUrlForTarget` returns a URL (node-http-proxy.ts:92-112).
/// `Some(url)` routes all traffic through that proxy; `None` calls [`reqwest::ClientBuilder::no_proxy`]
/// to suppress reqwest's automatic system-proxy detection, so the resolver alone decides whether a
/// proxy is used (1:1 with Pi: no resolver hit ⇒ no proxy).
///
/// `idle_timeout_ms` overrides the process-global idle timeout: `None` uses the global default,
/// `Some(0)` disables the timeout, `Some(n)` sets it to `n` ms.
pub fn build_client_with_proxy(
    proxy: Option<reqwest::Url>,
    idle_timeout_ms: Option<u64>,
) -> Result<reqwest::Client, ProviderError> {
    build_client_with_proxy_forcing_http1(proxy, idle_timeout_ms, false)
}

/// [`build_client_with_proxy`] plus Pi's HTTP/1.1 escape hatch. When `force_http1`, the client
/// offers only `http/1.1` in ALPN — the analogue of pi swapping the default `NodeHttp2Handler` for
/// a plain `NodeHttpHandler` (`bedrock-converse-stream.ts:206-209` @v0.83.0, comment: "Some custom
/// endpoints require HTTP/1.1 instead of HTTP/2"). PROV-044.
pub fn build_client_with_proxy_forcing_http1(
    proxy: Option<reqwest::Url>,
    idle_timeout_ms: Option<u64>,
    force_http1: bool,
) -> Result<reqwest::Client, ProviderError> {
    let mut builder = with_idle_timeout(reqwest::Client::builder(), idle_timeout_ms);
    if force_http1 {
        builder = builder.http1_only();
    }
    builder = match proxy {
        Some(url) => {
            let proxy = reqwest::Proxy::all(url.as_str())
                .map_err(|e| ProviderError::Transport(Box::new(e)))?;
            builder.proxy(proxy)
        }
        None => builder.no_proxy(),
    };
    builder
        .build()
        .map_err(|e| ProviderError::Transport(Box::new(e)))
}

/// Build an HTTP client honoring the standard `HTTP(S)_PROXY` / `ALL_PROXY` / `NO_PROXY` environment
/// for `target_url` (Pi `resolveHttpProxyUrlForTarget`, node-http-proxy.ts:92-112, applied at the
/// point each API builds its live client — e.g. bedrock-converse-stream.ts:187-194). `env` is the
/// provider-scoped overlay (Pi `options.env`) and wins over the ambient process env exposed by
/// `ctx`. A non-HTTP(S) (SOCKS/PAC) proxy is surfaced as a transport error, exactly as Pi throws.
///
/// `idle_timeout_ms` is the per-request override described on [`build_client_with_proxy`]; the six
/// streaming wire APIs pass `StreamOptions.timeout_ms` here, everything else passes `None` to take
/// the global default.
pub async fn build_client_for_target(
    target_url: &str,
    ctx: &dyn crate::auth::types::AuthContext,
    env: Option<&crate::auth::types::ProviderEnv>,
    idle_timeout_ms: Option<u64>,
) -> Result<reqwest::Client, ProviderError> {
    build_client_for_target_forcing_http1(target_url, ctx, env, idle_timeout_ms, false).await
}

/// [`build_client_for_target`] plus Pi's `AWS_BEDROCK_FORCE_HTTP1` escape hatch (PROV-044).
///
/// `force_http1` is applied only when NO proxy is resolved, reproducing pi's `else if`
/// (`bedrock-converse-stream.ts:197-209` @v0.83.0): a proxied request already leaves HTTP/2 behind
/// via the proxy agent, so the override is redundant there.
pub async fn build_client_for_target_forcing_http1(
    target_url: &str,
    ctx: &dyn crate::auth::types::AuthContext,
    env: Option<&crate::auth::types::ProviderEnv>,
    idle_timeout_ms: Option<u64>,
    force_http1: bool,
) -> Result<reqwest::Client, ProviderError> {
    let proxy =
        crate::utils::node_http_proxy::resolve_http_proxy_url_for_target(target_url, ctx, env)
            .await
            .map_err(|e| ProviderError::Transport(Box::new(e)))?;
    let force_http1 = force_http1 && proxy.is_none();
    build_client_with_proxy_forcing_http1(proxy, idle_timeout_ms, force_http1)
}

type FrameStream = Pin<Box<dyn Stream<Item = Result<SseFrame, ProviderError>> + Send>>;

type EsInner =
    Pin<Box<dyn Stream<Item = Result<EsEvent, EventStreamError<reqwest::Error>>> + Send>>;

struct SseState {
    es: EsInner,
    cancel: CancelToken,
    done: bool,
}

/// Flatten a [`reqwest::Error`]'s source chain into one displayable message.
///
/// reqwest's own `Display` is deliberately terse — a read timeout renders as `"error sending
/// request for url (…)"` or `"error decoding response body"` — and the actual *reason* (`operation
/// timed out`, `connection refused`, the TLS failure) lives only in [`std::error::Error::source`].
/// That reason has to reach `AssistantMessage.error_message`, because that string is the sole input
/// to the turn-level retry classifier ([`crate::utils::retry::is_retryable_assistant_error`], Pi
/// `retry.ts:26-80`), whose patterns include `timed? out`, `connection.?refused` and `socket hang
/// up`. Pi gets this for free: its SDKs surface `"Connection error."` / `"fetch failed"`, which its
/// own classifier already matches. Without flattening, a timeout would be a *silently
/// unclassifiable* error — exactly the failure mode this file is being fixed for.
fn flatten_source_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        // Skip a link that adds nothing (reqwest sometimes re-wraps the same text).
        if !text.is_empty() && !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    message
}

/// Map a [`reqwest::Error`] to [`ProviderError::Transport`], preserving the underlying reason.
fn transport_error(error: reqwest::Error) -> ProviderError {
    ProviderError::Transport(flatten_source_chain(&error).into())
}

/// Send one attempt, racing the connect against cancellation (R-01-044). The request is rebuilt per
/// attempt because `reqwest::RequestBuilder::send` consumes it — Pi does the same (`provider-retry`
/// re-invokes the request thunk, "each retry is a fresh SDK request").
async fn send_once(
    client: &reqwest::Client,
    req: &SseRequest,
    cancel: &CancelToken,
) -> Result<reqwest::Response, ProviderError> {
    let mut builder = client.request(req.method.clone(), &req.url);
    for (name, value) in &req.headers {
        // A `None` value means "suppress a default"; on a fresh request there is nothing to
        // suppress, so only present values are applied.
        if let Some(value) = value {
            builder = builder.header(name.as_str(), value.as_str());
        }
    }
    if let Some(body) = &req.body {
        builder = builder.json(body);
    }

    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProviderError::Aborted),
        sent = builder.send() => sent.map_err(transport_error),
    }
}

/// Drain a non-2xx response body into a bounded, display-ready message (PROV-008).
///
/// The raw body is trimmed and capped at
/// [`MAX_PROVIDER_ERROR_BODY_CHARS`](crate::utils::error_body::MAX_PROVIDER_ERROR_BODY_CHARS) —
/// Pi's cap on the same string (`error-body.ts:76-82`) — because it travels verbatim into
/// `AssistantMessage.error_message`, the session JSONL and the next turn's prompt. Before the cap, a
/// multi-megabyte gateway HTML error page did exactly that.
async fn read_error_body(resp: reqwest::Response) -> String {
    normalize_error_body(&resp.text().await.unwrap_or_default())
}

/// Open the request and return a cancel-aware stream of decoded SSE frames.
///
/// Errors before the stream opens (transport failure, non-2xx HTTP, cancellation during connect)
/// are returned as `Err`; errors *during* streaming arrive as `Err` items inside the stream. In
/// both cases the caller converts them to a terminal `StreamEvent::Error` (func-01 R-01-018/045).
///
/// `retry` is the response-head retry budget (Pi `retryProviderRequest`, provider-retry.ts:104-125);
/// [`ProviderRetry::NONE`] — Pi's default — makes this a single attempt. Retries fire on a transport
/// failure and on Pi's retryable status set, honor a server `Retry-After`, sleep interruptibly, and
/// never fire after cancellation. `on_response` observes only the attempt whose outcome is returned,
/// matching Pi, where intermediate SDK failures never reach `options.onResponse`.
///
/// Neither this nor Pi retries a stream that fails *after* the head: at that point tokens have
/// already been delivered to the caller.
pub async fn open_sse(
    client: &reqwest::Client,
    req: SseRequest,
    cancel: CancelToken,
    on_request: Option<OnRequest>,
    on_response: Option<OnResponse>,
    retry: ProviderRetry,
) -> Result<FrameStream, ProviderError> {
    if let Some(cb) = &on_request {
        cb(&req);
    }

    let max_retries = retry.max_retries;
    let mut retries_remaining = max_retries;
    let resp = loop {
        let attempt = send_once(client, &req, &cancel).await;

        // Pi checks the abort signal before deciding to retry (provider-retry.ts:117); an abort is
        // terminal and never retried.
        if cancel.is_cancelled() {
            return Err(ProviderError::Aborted);
        }

        // What to wait before the next attempt, and the `error.message` Pi would have composed —
        // `None` here means "return this failure now".
        let (headers, message) = match attempt {
            Err(ProviderError::Aborted) => return Err(ProviderError::Aborted),
            // A transport failure carries no status: Pi's `error.status === undefined` ⇒ retryable.
            Err(transport) => {
                if retries_remaining == 0 || !is_retryable_provider_error(None, None) {
                    return Err(transport);
                }
                (None, transport.to_string())
            }
            Ok(resp) => {
                let code = resp.status().as_u16();
                if resp.status().is_success() {
                    if let Some(cb) = &on_response {
                        cb(code, resp.headers());
                    }
                    break resp;
                }
                let retryable = is_retryable_provider_error(Some(code), Some(resp.headers()));
                if retries_remaining == 0 || !retryable {
                    // Terminal: observe the response that is actually returned, then surface its
                    // (bounded) body.
                    if let Some(cb) = &on_response {
                        cb(code, resp.headers());
                    }
                    return Err(ProviderError::Http {
                        status: code,
                        message: read_error_body(resp).await,
                    });
                }
                // Headers must be cloned before the body is consumed.
                let headers = resp.headers().clone();
                // Pi passes the SDK error's `message`, which for a non-2xx is the composed
                // status + body — the same string `ProviderError::Http` renders.
                let body = read_error_body(resp).await;
                (Some(headers), format!("http {code}: {body}"))
            }
        };

        let retry_index = max_retries.saturating_sub(retries_remaining);
        retries_remaining = retries_remaining.saturating_sub(1);
        let delay = retry_delay_ms(headers.as_ref(), &message, retry_index, retry)?;

        // Pi's `abortableSleep`: the backoff is interruptible, unlike the SDKs' own retry timers.
        if cancel
            .run_until_cancelled(tokio::time::sleep(Duration::from_millis(delay)))
            .await
            .is_none()
        {
            return Err(ProviderError::Aborted);
        }
    };

    let es: EsInner = Box::pin(resp.bytes_stream().eventsource());
    let state = SseState {
        es,
        cancel,
        done: false,
    };

    let stream = futures::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        tokio::select! {
            biased;
            _ = state.cancel.cancelled() => {
                state.done = true;
                Some((Err(ProviderError::Aborted), state))
            }
            next = state.es.next() => match next {
                None => None,
                Some(Ok(ev)) => {
                    Some((Ok(SseFrame { event: ev.event, data: ev.data }), state))
                }
                Some(Err(e)) => {
                    state.done = true;
                    // `EventStreamError` has an empty `Error::source` impl, so a wrapped transport
                    // failure has to be unwrapped by hand to reach its reason (see
                    // `flatten_source_chain`) — a mid-stream idle timeout is otherwise reported only
                    // as "error decoding response body".
                    let text = match &e {
                        EventStreamError::Transport(inner) => {
                            format!("Transport error: {}", flatten_source_chain(inner))
                        }
                        other => other.to_string(),
                    };
                    // Capped for the same reason the non-2xx body is: an `EventStreamError::Parser`
                    // embeds the offending input, which is provider-controlled and unbounded, and
                    // this string lands in `AssistantMessage.error_message` verbatim.
                    Some((Err(ProviderError::Decode(normalize_error_body(&text))), state))
                }
            },
        }
    });

    Ok(Box::pin(stream))
}

/// Decode raw SSE bytes into frames (no network) — useful for replaying recorded vendor fixtures
/// (arch-01 §11). Errors during decode arrive as `Err` items.
pub fn decode_sse_bytes(bytes: impl Into<Bytes>) -> FrameStream {
    let bytes = bytes.into();
    let byte_stream = futures::stream::once(async move { Ok::<Bytes, std::io::Error>(bytes) });
    let es = byte_stream.eventsource();
    let stream = es.map(|ev| match ev {
        Ok(ev) => Ok(SseFrame {
            event: ev.event,
            data: ev.data,
        }),
        Err(e) => Err(ProviderError::Decode(e.to_string())),
    });
    Box::pin(stream)
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
    use futures::StreamExt;

    #[tokio::test]
    async fn decodes_frames_from_fixture_bytes() {
        let raw = "event: delta\ndata: hello\n\ndata: world\n\ndata: [DONE]\n\n";
        let frames: Vec<_> = decode_sse_bytes(raw.as_bytes().to_vec())
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames[0],
            SseFrame {
                event: "delta".into(),
                data: "hello".into()
            }
        );
        assert_eq!(frames[1].data, "world");
        assert_eq!(frames[2].data, "[DONE]");
    }

    // --- proxy-aware client builder (Pi node-http-proxy.ts applied to the live client) ---

    struct MapEnv(std::collections::BTreeMap<String, String>);
    #[async_trait::async_trait]
    impl crate::auth::types::AuthContext for MapEnv {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }
    fn ctx<const N: usize>(pairs: [(&str, &str); N]) -> MapEnv {
        MapEnv(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn build_client_with_proxy_builds_for_both_arms() {
        // An explicit proxy URL and the no-proxy arm both yield a usable client.
        assert!(
            build_client_with_proxy(
                Some(reqwest::Url::parse("http://proxy.local:8080").unwrap()),
                None
            )
            .is_ok()
        );
        assert!(build_client_with_proxy(None, None).is_ok());
    }

    #[tokio::test]
    async fn build_client_for_target_applies_resolver() {
        // No proxy env -> Ok (resolver returns None, client built with no_proxy).
        let env = ctx([]);
        assert!(
            build_client_for_target("https://api.example.com/v1", &env, None, None)
                .await
                .is_ok()
        );

        // A valid http proxy env -> Ok (proxy applied to the live client).
        let env = ctx([("https_proxy", "http://proxy.local:8080")]);
        assert!(
            build_client_for_target("https://api.example.com/v1", &env, None, None)
                .await
                .is_ok()
        );

        // The provider-scoped overlay wins over the ambient env and is applied -> Ok.
        let env = ctx([("https_proxy", "http://ambient:1")]);
        let overlay: crate::auth::types::ProviderEnv =
            [("https_proxy".to_string(), "http://overlay:2".to_string())]
                .into_iter()
                .collect();
        assert!(
            build_client_for_target("https://x.example.com/", &env, Some(&overlay), None)
                .await
                .is_ok()
        );

        // A SOCKS proxy is rejected by the resolver and surfaced as a transport error, exactly as
        // Pi throws (node-http-proxy.ts:106-108) — the live client is never built with it.
        let env = ctx([("https_proxy", "socks5://proxy.local:1080")]);
        let err = build_client_for_target("https://api.example.com/", &env, None, None)
            .await
            .expect_err("socks proxy must be rejected");
        assert!(matches!(err, ProviderError::Transport(_)));
        assert!(err.to_string().contains("SOCKS and PAC"));
    }

    // ---------------------------------------------------------------------------------------
    // PROV-006 / PROV-008 — live loopback mocks.
    //
    // Every server here is a raw `TcpListener` on 127.0.0.1 with an ephemeral port, so nothing in
    // this module's tests can reach a real network host.
    // ---------------------------------------------------------------------------------------

    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Accept one connection, read the request, then do `after_accept` — and nothing else, holding
    /// the socket open forever. This is the failure PROV-006 describes: the TCP handshake succeeds,
    /// so no connect timeout fires, and the peer then goes silent.
    async fn spawn_stalling_server(head: Option<&'static str>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                if let Some(head) = head {
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.flush().await;
                }
                // Stall forever, keeping the connection open.
                std::future::pending::<()>().await;
            }
        });
        (format!("http://{addr}/v1/stream"), handle)
    }

    /// Serve a single fixed response (status line + headers + body) to one connection.
    async fn spawn_fixed_server(
        status_line: &'static str,
        body: String,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "{status_line}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (format!("http://{addr}/v1/stream"), handle)
    }

    /// Serve `attempts` scripted responses on successive connections, recording how many were made.
    async fn spawn_scripted_server(
        script: Vec<(&'static str, &'static str, String)>,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        let handle = tokio::spawn(async move {
            for (status_line, extra_headers, body) in script {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "{status_line}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (format!("http://{addr}/v1/stream"), hits, handle)
    }

    /// `Result::expect_err` needs `T: Debug`, which a boxed stream does not implement.
    fn expect_err(result: Result<FrameStream, ProviderError>, context: &str) -> ProviderError {
        match result {
            Ok(_) => panic!("{context}: expected an error, got an open stream"),
            Err(e) => e,
        }
    }

    fn get(url: &str) -> SseRequest {
        SseRequest {
            method: reqwest::Method::GET,
            url: url.to_string(),
            headers: HeaderMap::new(),
            body: None,
        }
    }

    // ------------------------------------------------------------------ PROV-006 (timeout) ----

    /// A provider that accepts the connection and never sends a byte must fail inside the
    /// configured window instead of hanging the turn forever.
    #[tokio::test]
    async fn a_stall_before_the_headers_times_out_instead_of_hanging() {
        let (url, server) = spawn_stalling_server(None).await;
        let client = build_client_with_proxy(None, Some(300)).expect("client");

        let started = Instant::now();
        let err = expect_err(
            tokio::time::timeout(
                Duration::from_secs(10),
                open_sse(
                    &client,
                    get(&url),
                    CancelToken::new(),
                    None,
                    None,
                    ProviderRetry::NONE,
                ),
            )
            .await
            .expect("open_sse must return, NOT hang"),
            "a silent peer",
        );

        assert!(matches!(err, ProviderError::Transport(_)), "got {err:?}");
        assert!(
            err.to_string().contains("timed out") || err.to_string().contains("timeout"),
            "the message must name the timeout so the turn-level retry classifier sees it: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        server.abort();
    }

    /// The same, but after the response head and a partial SSE frame — undici's `bodyTimeout`, the
    /// half that a total-request deadline would get wrong.
    #[tokio::test]
    async fn a_stall_mid_stream_ends_the_stream_with_an_error() {
        let (url, server) = spawn_stalling_server(Some(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n\
             1a\r\ndata: {\"partial\":true}\n\n\r\n",
        ))
        .await;
        let client = build_client_with_proxy(None, Some(300)).expect("client");

        let mut frames = tokio::time::timeout(
            Duration::from_secs(10),
            open_sse(
                &client,
                get(&url),
                CancelToken::new(),
                None,
                None,
                ProviderRetry::NONE,
            ),
        )
        .await
        .expect("open_sse must return")
        .expect("the head arrived, so the stream opens");

        let first = frames.next().await.expect("the partial frame").expect("ok");
        assert_eq!(first.data, "{\"partial\":true}");

        let started = Instant::now();
        let next = tokio::time::timeout(Duration::from_secs(10), frames.next())
            .await
            .expect("the stream must terminate, NOT hang")
            .expect("an error item, not a clean end");
        let err = next.expect_err("a stalled body must surface as an error");
        assert!(
            err.to_string().contains("timed out") || err.to_string().contains("timeout"),
            "got {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        server.abort();
    }

    /// Puts [`HTTP_IDLE_TIMEOUT_MS`] back to [`DEFAULT_HTTP_IDLE_TIMEOUT_MS`] on `Drop`, never only
    /// on the success path.
    ///
    /// A perturbed global that leaks past a PANICKING assertion is a far worse hazard than the
    /// transient one below: it is not a window at all but a permanent retune of every client the
    /// rest of this binary builds — `1234`, or `0` (unbounded), applied to every later
    /// [`build_client`]/[`build_client_for`] in the process, long after the test that set it has
    /// already reported its own failure.
    struct RestoreIdleTimeoutOnDrop;

    impl Drop for RestoreIdleTimeoutOnDrop {
        fn drop(&mut self) {
            configure_http_idle_timeout(DEFAULT_HTTP_IDLE_TIMEOUT_MS);
        }
    }

    /// The global default is what protects every caller that does not pass an override — this is
    /// the setting Pi installs with `configureHttpDispatcher` at startup — plus the `0 = disabled`
    /// and `None = inherit` resolution rules.
    ///
    /// One test, not three: `HTTP_IDLE_TIMEOUT_MS` is process-global, and cargo runs this module's
    /// tests concurrently in one process, so splitting it would let the cases race each other.
    ///
    /// # Keeping the perturbation invisible to the rest of the binary
    ///
    /// The hazard is not this test failing, it is this test making an UNRELATED one fail: every
    /// [`build_client`] and [`build_client_for`] in the crate reads this global, so while it holds
    /// `1_000` any bystander client is built with a 1 s `read_timeout` against its own loopback
    /// mock. The previous shape left it perturbed across the whole `open_sse` leg — seconds of real
    /// awaits, i.e. seconds during which any of this crate's other loopback tests could be built
    /// short — and reasoned that 1 s was "generous" enough not to starve them, which is a bet on
    /// machine load rather than a guarantee.
    ///
    /// It is not a bet worth taking, and it does not have to be, because the global is consulted
    /// exactly ONCE per client: `with_idle_timeout` reads it inside `build_client()` and bakes the
    /// resulting `read_timeout` into the built client (the same reason `configure_http_idle_timeout`
    /// documents that already-built clients keep the timeout they were built with). So the setting
    /// only has to survive that one SYNCHRONOUS call, and the restore moves to immediately after it
    /// — before the stalling server is ever awaited. Every window in this test is now await-free:
    /// no other task on this runtime can observe one at all, and a bystander on another thread
    /// would have to land inside a single `ClientBuilder::build()`. The seconds-long window is gone.
    #[tokio::test]
    async fn the_process_global_timeout_applies_when_no_override_is_given() {
        assert_eq!(
            DEFAULT_HTTP_IDLE_TIMEOUT_MS, 300_000,
            "the built-in default is Pi's 5 minutes (http-dispatcher.ts:4)"
        );
        assert_eq!(
            http_idle_timeout_ms(),
            DEFAULT_HTTP_IDLE_TIMEOUT_MS,
            "a process that never calls `configure_http_idle_timeout` is still bounded"
        );

        // The stalling server is set up BEFORE the global is touched, so the await it costs is not
        // spent with the global perturbed.
        let (url, server) = spawn_stalling_server(None).await;

        let client = {
            let _restore = RestoreIdleTimeoutOnDrop;

            // Resolution rules.
            configure_http_idle_timeout(1234);
            assert_eq!(resolve_idle_timeout(None), Some(Duration::from_millis(1234)));
            assert_eq!(resolve_idle_timeout(Some(0)), None, "0 disables, not 0ms");
            assert_eq!(
                resolve_idle_timeout(Some(50)),
                Some(Duration::from_millis(50))
            );
            configure_http_idle_timeout(0);
            assert_eq!(resolve_idle_timeout(None), None, "a disabled global disables");

            // ...and the global actually reaches a client built with no override. `build_client`
            // is synchronous and reads the global exactly here; `_restore` puts the default back
            // at the end of this block, so everything below runs at the default again.
            configure_http_idle_timeout(1_000);
            build_client().expect("client")
        };

        let err = expect_err(
            tokio::time::timeout(
                Duration::from_secs(20),
                open_sse(
                    &client,
                    get(&url),
                    CancelToken::new(),
                    None,
                    None,
                    ProviderRetry::NONE,
                ),
            )
            .await
            .expect("the global default must bound the request"),
            "a silent peer",
        );
        assert!(matches!(err, ProviderError::Transport(_)), "got {err:?}");
        assert_eq!(
            http_idle_timeout_ms(),
            DEFAULT_HTTP_IDLE_TIMEOUT_MS,
            "the global is back at its default for the rest of the binary, and the 1 s timeout that \
             just fired came from the client it was baked into — not from a still-perturbed global"
        );

        server.abort();
    }

    // ------------------------------------------------------------------ PROV-008 (body cap) ---

    /// A 1 MB gateway error page must not reach the transcript verbatim.
    #[tokio::test]
    async fn a_huge_error_body_is_capped_before_it_reaches_the_error_message() {
        let page = format!(
            "<html><body>{}</body></html>",
            "<p>bad gateway</p>".repeat(60_000)
        );
        assert!(page.len() > 1_000_000);
        let (url, server) = spawn_fixed_server("HTTP/1.1 502 Bad Gateway", page).await;
        let client = build_client_with_proxy(None, Some(5_000)).expect("client");

        let err = expect_err(
            open_sse(
                &client,
                get(&url),
                CancelToken::new(),
                None,
                None,
                ProviderRetry::NONE,
            )
            .await,
            "502",
        );

        let ProviderError::Http { status, message } = &err else {
            panic!("expected Http, got {err:?}");
        };
        assert_eq!(*status, 502);
        assert_eq!(
            message.chars().count(),
            crate::utils::error_body::MAX_PROVIDER_ERROR_BODY_CHARS + "... [truncated 1080020 chars]".chars().count(),
            "the body must be the 4000-char head plus Pi's marker, got {} chars",
            message.chars().count()
        );
        assert!(message.contains("... [truncated "));

        // The whole blast chain the audit describes ends here: this is what is persisted and
        // replayed.
        let assistant = err.into_error_message("openai".into(), "gpt", None);
        assert!(
            assistant
                .error_message
                .as_deref()
                .unwrap_or_default()
                .chars()
                .count()
                < crate::utils::error_body::MAX_PROVIDER_ERROR_BODY_CHARS + 128,
            "the transcript field must be bounded too"
        );
        server.abort();
    }

    #[tokio::test]
    async fn a_small_error_body_is_passed_through_trimmed() {
        let (url, server) =
            spawn_fixed_server("HTTP/1.1 400 Bad Request", "  {\"error\":\"nope\"}\n".into()).await;
        let client = build_client_with_proxy(None, Some(5_000)).expect("client");
        let err = expect_err(
            open_sse(
                &client,
                get(&url),
                CancelToken::new(),
                None,
                None,
                ProviderRetry::NONE,
            )
            .await,
            "400",
        );
        assert_eq!(err.to_string(), "http 400: {\"error\":\"nope\"}");
        server.abort();
    }

    // ------------------------------------------------------------------ PROV-006 (retry) ------

    #[tokio::test]
    async fn a_retryable_status_is_retried_within_the_budget_and_then_succeeds() {
        let sse = "data: hello\n\n".to_string();
        let (url, hits, server) = spawn_scripted_server(vec![
            ("HTTP/1.1 503 Service Unavailable", "", "down".into()),
            (
                "HTTP/1.1 200 OK",
                "Content-Type: text/event-stream\r\n",
                sse,
            ),
        ])
        .await;
        let client = build_client_with_proxy(None, Some(5_000)).expect("client");

        let mut frames = open_sse(
            &client,
            get(&url),
            CancelToken::new(),
            None,
            None,
            ProviderRetry {
                max_retries: 2,
                max_retry_delay_ms: None,
            },
        )
        .await
        .expect("the second attempt succeeds");

        assert_eq!(frames.next().await.expect("frame").expect("ok").data, "hello");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn a_non_retryable_status_fails_on_the_first_attempt() {
        let (url, hits, server) = spawn_scripted_server(vec![
            ("HTTP/1.1 401 Unauthorized", "", "bad key".into()),
            ("HTTP/1.1 200 OK", "", String::new()),
        ])
        .await;
        let client = build_client_with_proxy(None, Some(5_000)).expect("client");

        let err = expect_err(
            open_sse(
                &client,
                get(&url),
                CancelToken::new(),
                None,
                None,
                ProviderRetry { max_retries: 3, max_retry_delay_ms: None },
            )
            .await,
            "401",
        );
        assert_eq!(err.to_string(), "http 401: bad key");
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a deterministic error must not burn the budget"
        );
        server.abort();
    }

    #[tokio::test]
    async fn a_server_delay_over_the_ceiling_fails_immediately_with_pis_message() {
        let (url, hits, server) = spawn_scripted_server(vec![
            (
                "HTTP/1.1 429 Too Many Requests",
                "Retry-After: 600\r\n",
                "slow down".into(),
            ),
            ("HTTP/1.1 200 OK", "", String::new()),
        ])
        .await;
        let client = build_client_with_proxy(None, Some(5_000)).expect("client");

        let err = expect_err(
            open_sse(
                &client,
                get(&url),
                CancelToken::new(),
                None,
                None,
                ProviderRetry { max_retries: 3, max_retry_delay_ms: None },
            )
            .await,
            "a 10-minute Retry-After exceeds the 60s ceiling",
        );
        assert_eq!(
            err.to_string(),
            "Server requested 600s retry delay (max: 60s). http 429: slow down"
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn the_default_budget_makes_exactly_one_attempt() {
        let (url, hits, server) = spawn_scripted_server(vec![
            ("HTTP/1.1 503 Service Unavailable", "", "down".into()),
            ("HTTP/1.1 200 OK", "", String::new()),
        ])
        .await;
        let client = build_client_with_proxy(None, Some(5_000)).expect("client");
        let err = expect_err(
            open_sse(
                &client,
                get(&url),
                CancelToken::new(),
                None,
                None,
                ProviderRetry::NONE,
            )
            .await,
            "no budget, no retry",
        );
        assert_eq!(err.to_string(), "http 503: down");
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "Pi's default is `maxRetries ?? 0`"
        );
        server.abort();
    }

    #[tokio::test]
    async fn cancellation_during_the_backoff_is_terminal() {
        let (url, hits, server) = spawn_scripted_server(vec![
            (
                "HTTP/1.1 429 Too Many Requests",
                "Retry-After: 30\r\n",
                "slow down".into(),
            ),
            ("HTTP/1.1 200 OK", "", String::new()),
        ])
        .await;
        let client = build_client_with_proxy(None, Some(5_000)).expect("client");
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            canceller.cancel();
        });

        let err = expect_err(
            tokio::time::timeout(
                Duration::from_secs(10),
                open_sse(
                    &client,
                    get(&url),
                    cancel,
                    None,
                    None,
                    ProviderRetry { max_retries: 3, max_retry_delay_ms: None },
                ),
            )
            .await
            .expect("the 30s backoff must be interruptible"),
            "cancelled",
        );
        assert!(matches!(err, ProviderError::Aborted), "got {err:?}");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        server.abort();
    }

    /// `on_response` observes only the attempt whose outcome is returned — Pi's intermediate SDK
    /// failures never reach `options.onResponse`.
    #[tokio::test]
    async fn on_response_fires_once_for_the_returned_attempt() {
        let (url, _hits, server) = spawn_scripted_server(vec![
            ("HTTP/1.1 503 Service Unavailable", "", "down".into()),
            (
                "HTTP/1.1 200 OK",
                "Content-Type: text/event-stream\r\n",
                "data: ok\n\n".into(),
            ),
        ])
        .await;
        let client = build_client_with_proxy(None, Some(5_000)).expect("client");
        let seen = Arc::new(std::sync::Mutex::new(Vec::<u16>::new()));
        let sink = seen.clone();
        let hook: OnResponse = Arc::new(move |status, _headers| {
            sink.lock().unwrap().push(status);
        });

        let _frames = open_sse(
            &client,
            get(&url),
            CancelToken::new(),
            None,
            Some(hook),
            ProviderRetry {
                max_retries: 2,
                max_retry_delay_ms: None,
            },
        )
        .await
        .expect("succeeds on retry");
        assert_eq!(*seen.lock().unwrap(), vec![200]);
        server.abort();
    }
}

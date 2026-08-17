//! Outbound HTTP capability grant (arch-08 §3.2 draft; `pi-mcp-adapter-port.md` §3.2 — the locked
//! WIT shape this backs verbatim). A real `reqwest`-backed engine for the `http-client` WIT
//! interface: [`HttpCaps::request`] is a bounded round trip; [`HttpCaps::request_stream`] /
//! [`HttpCaps::poll_stream_chunk`] / [`HttpCaps::close_stream`] back a long-lived streaming body the
//! HOST owns (a guest cannot hold a live Rust `Stream` across the wasm boundary — the request/poll
//! bridge, arch-08 §5.2), keyed by an opaque `u32` handle the guest polls.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use tokio::io::AsyncReadExt;

/// A single outbound HTTP request (mirrors the WIT `http-request` record 1:1, world.wit).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: Option<u32>,
}

/// The response to a [`HttpRequest`] (mirrors the WIT `http-response` record 1:1, world.wit).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The initiating response's metadata for a stream opened via [`HttpCaps::request_stream`] (mirrors
/// the WIT `http-stream-response` record 1:1, world.wit): status+headers are captured off the SAME
/// round trip that opens the long-lived body, so a caller can inspect them (e.g. a 401 => re-auth,
/// `mcp-session-id`) before or independent of draining the body via [`HttpCaps::poll_stream_chunk`] —
/// closes L4 §2.3 (the real consumer, the MCP SDK's `StreamableHTTPClientTransport`, needs exactly
/// this off the response whose body it then streams).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpStreamResponse {
    pub handle: u32,
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

/// A live streaming response body, boxed for storage in the registry. The error type is
/// `std::io::Error` (not `reqwest::Error`) because a Content-Encoded stream is re-wrapped through
/// an `async-compression` decoder — see [`decode_stream`].
type ChunkStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

/// Bounds how much of a single [`HttpCaps::request`] response body gets buffered in memory before
/// the request is rejected outright. `HttpCaps` is ONE shared engine behind a single `Arc` on
/// `LiveHostServices` (`host_services.rs`), used by EVERY extension loaded into a session — an
/// unbounded `resp.bytes().await` lets any one malicious/misbehaving extension exhaust the whole
/// host process's memory just by pointing `request` at an endpoint that serves an arbitrarily
/// large (or unbounded chunked) body. Mirrors the exact bound `cyrup_ext::caps::proc`'s
/// `MAX_PIPE_BUFFER_BYTES` (`proc.rs`) already established for this identical class of
/// shared-host-resource exhaustion, ported here to close the one capability that skipped it.
/// [`Self::request_stream`]/[`Self::poll_stream_chunk`] don't need this: the host never
/// accumulates their body — the guest drains it one already-bounded network chunk at a time.
const MAX_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Bounds how many streaming HTTP bodies (`request-stream` handles) can be open at once.
/// `HttpCaps` is ONE shared engine behind a single `Arc` on `LiveHostServices` (same doc as
/// [`MAX_RESPONSE_BODY_BYTES`]) — an unbounded, ever-growing `streams` registry lets a guest that
/// keeps opening streams without ever draining/closing them exhaust host memory/fd/connection-pool
/// resources over time, even though each INDIVIDUAL stream's chunks are already bounded (the guest
/// drains one already-bounded network chunk at a time). No Pi-derived exact count to port here —
/// the real consumer, `pi-mcp-adapter/server-manager.ts:41-83`'s `connections`/`connectPromises`
/// maps, dedupes/reuses connections keyed by CONFIGURED server name, giving it an implicit bound
/// this lower-level primitive lacks by construction — so, like [`MAX_RESPONSE_BODY_BYTES`], this is
/// a deliberately generous cap (comfortably above any realistic legitimate concurrent-stream count)
/// that still guarantees bounded worst-case growth.
const MAX_OPEN_STREAMS: usize = 256;

/// Fallback ceiling for [`HttpCaps::request`]'s full round trip, and for [`HttpCaps::request_stream`]'s
/// initial connect+response-headers phase, when the guest supplied NO `req.timeout_ms` (L4 review §6
/// — `HttpCaps`'s three call sites had no fallback timeout ceiling AT ALL when `timeout_ms` was
/// absent). Every one of these host calls is bridged onto a real OS thread via `block_in_place`+
/// `block_on` (`cyrup-session-svc/src/host_services.rs`'s `http_request`/`http_request_stream`/
/// `http_poll_stream_chunk`) while the wasm guest sits suspended across it — the SEPARATE
/// `note_dialog_wait` epoch-forgiveness fix (`host/live.rs`) already stops that wait from tripping
/// the WASM epoch deadline, but does nothing to bound the REAL wall-clock block on the host's own
/// blocking-thread pool (bounded, but finite — `tokio::runtime::Builder::max_blocking_threads`,
/// default 512), which an unbounded wait against a stalled/malicious server could still exhaust one
/// thread of, indefinitely, no matter how many guests are involved. Like [`MAX_RESPONSE_BODY_BYTES`]/
/// [`MAX_OPEN_STREAMS`], the point is FINITE, not a specific magic number — deliberately generous,
/// comfortably above any realistic legitimate slow-server response time, while still guaranteeing
/// the call can never hang literally forever. An EXPLICIT `req.timeout_ms` from the guest always
/// wins; this only fills the gap when none was given.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Per-poll idle ceiling for [`HttpCaps::poll_stream_chunk`] — bounds how long a SINGLE poll may
/// wait for the next chunk (or EOF) before giving up, not the stream's total lifetime. Unlike
/// [`DEFAULT_REQUEST_TIMEOUT`], this applies UNCONDITIONALLY (the WIT `poll-stream-chunk(handle)`
/// carries no request/timeout of its own, world.wit) and is deliberately several times longer:
/// a legitimate long-lived stream — the real consumer's actual protocol need this capability exists
/// to serve, the MCP SDK's `StreamableHTTPClientTransport`'s long-lived `GET`
/// (`streamableHttp.js:75-105`, `Accept: text/event-stream`, explicitly "to listen for server
/// messages") — can go genuinely quiet between server-pushed messages for a while; firing too
/// eagerly would functionally break exactly the use case this finding says must not break. On
/// timeout the stream is put BACK in the registry (never treated as EOF/terminal — `poll_stream_chunk`'s
/// match arm below) so a guest that simply polls again keeps draining the SAME live connection; the
/// bound exists purely to guarantee the host's blocking thread is eventually released either way,
/// not to cap how long the connection itself may legitimately stay open.
const HTTP_POLL_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// The state of a single handle in [`HttpCaps`]'s stream registry. THREE distinct states — not a
/// bare `Option<ChunkStream>` — are needed to correctly account for [`MAX_OPEN_STREAMS`] while a
/// poll is genuinely in flight (L4 round-12 finding #2a): collapsing "already reached terminal EOF,
/// nothing left to free" and "currently being awaited by `poll_stream_chunk`, the real connection AND
/// a host blocking thread (`cyrup-session-svc::host_services::http_poll_stream_chunk`'s
/// `block_in_place`+`block_on`) stay pinned for up to [`HTTP_POLL_IDLE_TIMEOUT`]" into the same `None`
/// representation let `close_stream`'s unconditional `remove` free the [`MAX_OPEN_STREAMS`] accounting
/// slot the INSTANT it ran, long before the real resource was actually released — letting a guest
/// evade the cap simply by racing `close_stream` against every `poll_stream_chunk`.
enum StreamSlot {
    /// Live and not currently being awaited by a poll.
    Idle(ChunkStream),
    /// Reached terminal EOF or a stream read error; repeat polls keep degrading to `Ok(None)`.
    Eof,
    /// The `ChunkStream` has been taken out of the registry to `.await` its next chunk
    /// (`poll_stream_chunk`). `closed` records whether `close_stream` ran while this poll was in
    /// flight — actual removal (and so the real connection's drop) is deferred to that poll's own
    /// completion, so the [`MAX_OPEN_STREAMS`] slot is never freed before the resource genuinely is.
    Polling { closed: bool },
}

/// The real HTTP capability engine: one shared `reqwest::Client` plus a registry of open streaming
/// bodies keyed by an opaque `u32` handle (`request-stream` / `poll-stream-chunk` / `close-stream`).
/// See [`StreamSlot`] for what each registry entry means; repeat polls of a [`StreamSlot::Eof`] entry
/// keep returning `Ok(None)` until the guest calls `close-stream`, matching Pi's SSE/StreamableHTTP
/// "done" semantics for the eventual MCP transport consumer.
pub struct HttpCaps {
    client: reqwest::Client,
    /// PROV-047 — per-proxy clients, keyed by the resolved proxy URL.
    ///
    /// Pi's `httpProxy` setting is written into `process.env` at startup and observed by EVERY
    /// later `getProxyEnv` consultation, extension HTTP included; cyrup cannot write its own env
    /// (`set_var` is `unsafe` from edition 2024), so the setting lives in
    /// `cyrup_provider::stream::sse::configure_http_proxy` and is consulted by the ported resolver
    /// `cyrup_provider::utils::node_http_proxy::resolve_http_proxy_url_for_target`. That resolver
    /// is PER-TARGET (`no_proxy`, scheme, port all matter), so one client cannot serve every
    /// request — hence a small cache rather than a field on the struct. The un-proxied
    /// [`Self::client`] stays the fast path for the overwhelmingly common no-proxy case.
    ///
    /// Every client in here is built by [`client_builder`], so the no-auto-decompression contract
    /// [`Self::new`] documents holds for proxied requests identically.
    proxied: Mutex<HashMap<String, reqwest::Client>>,
    streams: Mutex<HashMap<u32, StreamSlot>>,
    next_handle: AtomicU32,
    /// [`MAX_OPEN_STREAMS`] in production; overridable ONLY for tests
    /// ([`Self::with_max_open_streams`]) so the cap-rejection path is exercisable without actually
    /// opening 256 real concurrent sockets (flaky under full-suite parallel test execution, which
    /// contends for the same loopback networking resources across many unrelated tests at once).
    max_open_streams: usize,
    /// [`DEFAULT_REQUEST_TIMEOUT`] in production; overridable ONLY for tests
    /// ([`Self::with_request_timeout`]) so the fallback-timeout path is exercisable without a real
    /// test waiting the full production duration.
    request_timeout: Duration,
    /// [`HTTP_POLL_IDLE_TIMEOUT`] in production; overridable ONLY for tests
    /// ([`Self::with_poll_idle_timeout`]) so the idle-timeout path is exercisable without a real
    /// test waiting the full production duration.
    poll_idle_timeout: Duration,
}

impl std::fmt::Debug for HttpCaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpCaps").finish_non_exhaustive()
    }
}

impl Default for HttpCaps {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpCaps {
    /// Build a real client (rustls, arch-01 §7.1's convention — no native-tls). A builder failure
    /// (never observed with the default rustls config actually used in this workspace) degrades to
    /// `reqwest::Client::new()` rather than panicking (no-panic policy, arch-00 §8).
    ///
    /// Disables `reqwest`'s own built-in auto-decompression (`no_gzip`/`no_brotli`/`no_deflate`/
    /// `no_zstd` — available regardless of cargo features, so this compiles either way): that
    /// machinery unconditionally strips `Content-Encoding`/`Content-Length` the instant it
    /// decompresses a body, diverging from the real consumer's `fetch()` (which preserves both —
    /// verified live against Node's real `fetch()`). [`request`]/[`request_stream`] instead
    /// advertise `fetch()`'s own real `Accept-Encoding` default (`build_request`) and decompress
    /// manually via [`decode_buffered`]/[`decode_stream`], so the caller sees the decompressed body
    /// AND the untouched original headers.
    pub fn new() -> Self {
        let client = client_builder().build().unwrap_or_else(|_| reqwest::Client::new());
        Self::with_client(client)
    }

    /// Build around a caller-supplied client (tests, or a client the host already built).
    pub fn with_client(client: reqwest::Client) -> Self {
        Self {
            client,
            proxied: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            next_handle: AtomicU32::new(1),
            max_open_streams: MAX_OPEN_STREAMS,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            poll_idle_timeout: HTTP_POLL_IDLE_TIMEOUT,
        }
    }

    /// Build with a caller-supplied open-stream cap (tests only; production always gets the real
    /// [`MAX_OPEN_STREAMS`] via [`Self::new`]/[`Self::with_client`]). Also disables connection
    /// pooling (`pool_max_idle_per_host(0)`): a test proving the cap works needs many streams to
    /// stay genuinely OPEN (their bodies deliberately undrained) at once against a short-lived local
    /// mock server whose per-connection handler task closes its socket right after writing — with
    /// pooling on, `reqwest` can hand a later call a since-closed pooled connection, an unrelated
    /// flake this test must not be sensitive to.
    #[cfg(test)]
    fn with_max_open_streams(max_open_streams: usize) -> Self {
        let client =
            client_builder().pool_max_idle_per_host(0).build().unwrap_or_else(|_| reqwest::Client::new());
        Self { max_open_streams, ..Self::with_client(client) }
    }

    /// Build with a caller-supplied fallback request timeout (tests only; production always gets the
    /// real [`DEFAULT_REQUEST_TIMEOUT`] via [`Self::new`]/[`Self::with_client`]) — L4 review §6.
    #[cfg(test)]
    fn with_request_timeout(request_timeout: Duration) -> Self {
        let client = client_builder().build().unwrap_or_else(|_| reqwest::Client::new());
        Self { request_timeout, ..Self::with_client(client) }
    }

    /// Build with a caller-supplied per-poll idle timeout (tests only; production always gets the
    /// real [`HTTP_POLL_IDLE_TIMEOUT`] via [`Self::new`]/[`Self::with_client`]) — L4 review §6.
    #[cfg(test)]
    fn with_poll_idle_timeout(poll_idle_timeout: Duration) -> Self {
        let client = client_builder().build().unwrap_or_else(|_| reqwest::Client::new());
        Self { poll_idle_timeout, ..Self::with_client(client) }
    }

    /// Shared request-building: method parse + headers + optional body + optional timeout.
    ///
    /// Sets `Accept-Encoding` when the caller didn't already supply one, mirroring the real
    /// consumer's own `fetch()` default EXACTLY — undici `dispatch`'s request-header step
    /// (`index.js:1552-1566`), not `reqwest`'s (or `tower_http`'s) own auto-decompression toggle
    /// set, which this crate disables ([`Self::new`]'s doc) precisely because it diverges from
    /// `fetch()`. In order of precedence: a `Range` header present ⇒ `"identity"` (step 18,
    /// `index.js:1552-1555` — a compressed byte-range response can't be meaningfully resumed/sliced,
    /// so real `fetch()` refuses to negotiate compression at all for a ranged request); otherwise
    /// scheme-conditional (step 19, `index.js:1561-1565`): `"br, gzip, deflate"` for `https:`,
    /// `"gzip, deflate"` for everything else. Never `zstd` in EITHER arm — undici's own outbound
    /// default doesn't advertise it, even though its decoder still accepts a `zstd`-encoded response
    /// sent unprompted (`index.js:2296-2299`; mirrored by [`decode_buffered_one`]/[`decode_stream_one`]
    /// below, which likewise keep `zstd` decoder support without ever asking for it by default).
    ///
    /// `apply_reqwest_timeout` gates whether `req.timeout_ms` (when given and non-zero) is ALSO set as
    /// `reqwest`'s own request-level `RequestBuilder::timeout()` — [`Self::request`] wants this (it
    /// wants the WHOLE round trip, including body-drain, bounded, and the outer `tokio::time::timeout`
    /// wrapper it also applies makes this redundant-but-harmless). [`Self::request_stream`] must pass
    /// `false`: verified against `reqwest` 0.13.4's source
    /// (`~/.cargo/registry/.../reqwest-0.13.4/src/async_impl/{client,body}.rs`) that
    /// `RequestBuilder::timeout()` becomes a TOTAL-request `Sleep` threaded into the returned
    /// `Response` (`client.rs`'s `Response::new(..., self.total_timeout.take(), ...)`) and wraps its
    /// body in a `TotalTimeoutBody` (`body.rs`) whose timer is NEVER reset on read progress — so it
    /// keeps running through `resp.bytes_stream()` long after the connect+headers phase this function
    /// is meant to bound has completed, silently killing an otherwise-healthy long-lived body stream
    /// at exactly `timeout_ms`, contradicting [`Self::request_stream`]'s own documented "only
    /// connect+headers is bounded" contract. `request_stream`'s outer `tokio::time::timeout` around
    /// `.send()` already bounds connect+headers on its own; the body stream is separately bounded
    /// per-poll by [`HTTP_POLL_IDLE_TIMEOUT`] in [`Self::poll_stream_chunk`], never by this timeout.
    /// Pick the client for `url`: the shared un-proxied one, or a cached per-proxy client when
    /// pi's `httpProxy` / the ambient `HTTP_PROXY`-family variables select one for this target
    /// (PROV-047).
    ///
    /// A proxy the resolver REFUSES (a SOCKS/PAC `httpProxy`, an unparseable proxy URL —
    /// `node-http-proxy.ts:89`/`:102-108`) fails this one request instead of quietly connecting
    /// direct.
    ///
    /// CYRUP-DELTA — pi has no decision to make here and cyrup does. Upstream, extension `fetch`
    /// runs on the GLOBAL `EnvHttpProxyAgent` (`coding-agent/src/core/http-dispatcher.ts:79-105`
    /// @v0.83.0), whose `ProxyAgent` handles `socks5:`/`socks:` itself
    /// (`undici/lib/dispatcher/proxy-agent.js:143,167`) — so a SOCKS `httpProxy` upstream is
    /// PROXIED, and upstream never sends this request direct under any proxy setting. cyrup's
    /// ported resolver rejects SOCKS by design (`node-http-proxy.ts:89`, ported verbatim as
    /// [`cyrup_provider::UNSUPPORTED_PROXY_PROTOCOL_MESSAGE`]), so it cannot reproduce the tunnel.
    /// Of the two remaining options, erroring is the one closer to upstream: a direct connection is
    /// egress the operator explicitly told us to route through a proxy, made without announcing
    /// itself, on a path where every other cyrup egress path (provider streams, OAuth, catalog
    /// refresh — all of which surface the resolver's typed error through
    /// `build_client_for_target`) already refuses. A warning in a log the guest never sees is not
    /// an announcement.
    async fn client_for(&self, url: &str) -> Result<reqwest::Client, String> {
        let resolved = cyrup_provider::utils::node_http_proxy::resolve_http_proxy_url_for_target(
            url,
            &cyrup_provider::auth::types::EnvAuthContext,
            None,
        )
        .await;
        let proxy_url = match resolved {
            Ok(Some(u)) => u,
            Ok(None) => return Ok(self.client.clone()),
            Err(e) => return Err(e.to_string()),
        };
        self.client_through(&proxy_url)
    }

    /// The cached client that routes through `proxy_url`, building it on first use. Split out of
    /// [`Self::client_for`] so the cache and the failure arm are testable without mutating the
    /// PROCESS-GLOBAL proxy setting — doing that from a unit test would race every other HTTP test
    /// in this crate, which talks to local mock servers a proxy would swallow.
    ///
    /// A build failure errors for the same reason [`Self::client_for`] does: the resolver said this
    /// request belongs on a proxy, so the alternative is unannounced direct egress.
    fn client_through(&self, proxy_url: &reqwest::Url) -> Result<reqwest::Client, String> {
        let key = proxy_url.to_string();
        if let Ok(guard) = self.proxied.lock()
            && let Some(c) = guard.get(&key)
        {
            return Ok(c.clone());
        }
        let built = reqwest::Proxy::all(proxy_url.clone())
            .and_then(|p| client_builder().proxy(p).build())
            .map_err(|e| format!("could not build a client for the configured proxy {key}: {e}"))?;
        if let Ok(mut guard) = self.proxied.lock() {
            guard.insert(key, built.clone());
        }
        Ok(built)
    }

    fn build_request(
        &self,
        client: &reqwest::Client,
        req: &HttpRequest,
        apply_reqwest_timeout: bool,
    ) -> Result<reqwest::RequestBuilder, String> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| format!("invalid HTTP method `{}`: {e}", req.method))?;
        let mut builder = client.request(method, req.url.as_str());
        let has_accept_encoding =
            req.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("accept-encoding"));
        if !has_accept_encoding {
            let has_range = req.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("range"));
            let default_accept_encoding = if has_range {
                // Real `fetch()` step 18 (index.js:1552-1555): a `Range` request always gets
                // `Accept-Encoding: identity` — a compressed byte-range response cannot be resumed/
                // sliced meaningfully, so the real consumer refuses to negotiate compression at all
                // for this request, regardless of scheme.
                "identity"
            } else if req.url.parse::<reqwest::Url>().map(|u| u.scheme() == "https").unwrap_or(false) {
                // Real `fetch()` step 19, HTTPS arm (index.js:1561-1563): `"br, gzip, deflate"` — no
                // `zstd` (undici's own outbound default never advertises it, even though its decoder
                // still handles a server that sends it unprompted, index.js:2296-2299 — mirrored by
                // this crate's own decoder support below).
                "br, gzip, deflate"
            } else {
                // Real `fetch()` step 19, non-HTTPS arm (index.js:1564-1565): `"gzip, deflate"`.
                "gzip, deflate"
            };
            builder = builder.header(reqwest::header::ACCEPT_ENCODING, default_accept_encoding);
        }
        for (name, value) in &req.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }
        // `0` means NO explicit timeout, not an instant one — same `> 0` guard the sibling `ui_roundtrip`
        // (`cyrup-session-svc/src/host_services.rs:236`) and `exec` (`host_services.rs:423-427`) grants
        // already apply to their own `timeout_ms`/`timeoutMs` fields, ported here for consistency: a
        // literal 0ms `reqwest` per-request timeout fires on (essentially) the very next poll, so
        // `Some(0)` was silently indistinguishable from "fail immediately" instead of falling through to
        // [`Self::request`]/[`Self::request_stream`]'s own [`DEFAULT_REQUEST_TIMEOUT`] fallback below.
        if apply_reqwest_timeout && let Some(ms) = req.timeout_ms.filter(|ms| *ms > 0) {
            builder = builder.timeout(std::time::Duration::from_millis(u64::from(ms)));
        }
        Ok(builder)
    }

    /// A bounded request/response round trip (the WIT `request`): the whole body is buffered and
    /// returned. A non-2xx status is NOT itself an `Err` (fetch/Pi semantics — only a transport
    /// failure is); the caller inspects `status` itself.
    ///
    /// `headers` are the REAL wire headers (including `Content-Encoding`/`Content-Length` when the
    /// server compressed the body) — `body` is still the DECOMPRESSED bytes, matching real `fetch()`
    /// exactly (verified live against Node's real `fetch()`, not just the spec). `read_bounded_body`
    /// bounds the WIRE (possibly compressed) transfer; [`decode_buffered`] separately bounds the
    /// DECOMPRESSED output at the same cap, so a small compressed body cannot expand into an
    /// unbounded in-memory allocation (a decompression bomb) now that decoding happens manually.
    pub async fn request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        // L4 review §6: the FULL round trip (connect through body-drain) is bounded by the guest's
        // `timeout_ms` when given (and non-zero — `0` means NO explicit timeout, [`Self::build_request`]'s
        // doc comment), else [`DEFAULT_REQUEST_TIMEOUT`] — never unbounded. `build_request`
        // ALSO applies `timeout_ms` (when given and non-zero) to `reqwest`'s own request-level timeout
        // below; that's unchanged/redundant-but-harmless in that case, and this outer bound is what
        // actually closes the gap for the (previously fully unbounded) `None`/`Some(0)` cases.
        let effective_timeout = req
            .timeout_ms
            .filter(|ms| *ms > 0)
            .map(|ms| Duration::from_millis(u64::from(ms)))
            .unwrap_or(self.request_timeout);
        tokio::time::timeout(effective_timeout, async {
            // `true`: `request` wants the WHOLE round trip (connect through body-drain) bounded, so
            // reqwest's own request-level timeout doubling up with the outer `tokio::time::timeout`
            // above is intentional, redundant-but-harmless belt-and-suspenders — see
            // [`Self::build_request`]'s doc for why [`Self::request_stream`] below must NOT do this.
            let client = self.client_for(req.url.as_str()).await?;
            let resp =
                self.build_request(&client, req, true)?.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let headers = collect_headers(resp.headers());
            let encoding = content_encoding_of(resp.headers());
            // Skip decompression entirely for a response that must never carry a coded body (a
            // null-body status, or a HEAD/CONNECT request) — see [`body_may_carry_content_coding`].
            let coding_encoding = if body_may_carry_content_coding(&req.method, status) {
                encoding.as_deref()
            } else {
                None
            };
            // Undici's `onResponseStart` rejects a response whose `Content-Encoding` chain exceeds
            // `maxContentEncodings` synchronously off HEADERS ALONE, before any body byte is read
            // (`undici/lib/web/fetch/index.js:2262-2275`) — mirror that ordering here by resolving
            // (and possibly failing on) the chain-depth cap BEFORE spending the bounded-but-possibly-
            // large `read_bounded_body` download, not after. [`Self::request_stream`] already got
            // this right via [`decode_stream`]; this brings the buffered path's ordering in line
            // with it (the actual decode below still re-resolves — cheap string work — to keep
            // [`decode_buffered`]'s self-contained signature).
            resolve_codings(coding_encoding)?;
            let raw = read_bounded_body(resp).await?;
            let body = decode_buffered(coding_encoding, raw).await?;
            Ok(HttpResponse { status, headers, body })
        })
        .await
        .unwrap_or_else(|_| Err(format!("request: timed out after {effective_timeout:?}")))
    }

    /// Start a streaming request (the WIT `request-stream`): opens the connection, captures the
    /// initiating response's status+headers (returned alongside the handle, closes L4 §2.3), then
    /// stores the decoded byte stream keyed by a fresh handle for [`Self::poll_stream_chunk`] to
    /// drain. Like [`Self::request`], a non-2xx status is not itself an error (fetch semantics) — the
    /// caller inspects [`HttpStreamResponse::status`] itself; the body is stored regardless of status.
    ///
    /// `headers` are the real wire headers (see [`Self::request`]'s doc comment); the drained chunks
    /// are the DECOMPRESSED bytes ([`decode_stream`] wraps the raw byte stream through the matching
    /// `async-compression` decoder before it ever reaches the registry).
    pub async fn request_stream(&self, req: &HttpRequest) -> Result<HttpStreamResponse, String> {
        // Reject BEFORE spending a real network round-trip if already at the cap (checked again,
        // atomically with the insert, below — this is a fast up-front rejection, not the only gate).
        {
            let g = self.streams.lock().map_err(|_| "http stream registry lock poisoned".to_string())?;
            if g.len() >= self.max_open_streams {
                return Err(format!(
                    "too many open http streams ({} already open) — close some via close-stream \
                     before opening more",
                    self.max_open_streams
                ));
            }
        }
        // L4 review §6: only the CONNECT+response-headers phase is bounded here — never the body
        // stream itself, which is meant to stay open long-lived ([`Self::poll_stream_chunk`] bounds
        // each individual drain separately via [`HTTP_POLL_IDLE_TIMEOUT`], never the whole stream's
        // lifetime). Falls back to [`DEFAULT_REQUEST_TIMEOUT`] when the guest gave no `timeout_ms`, or
        // gave `Some(0)` — `0` means NO explicit timeout, [`Self::build_request`]'s doc comment.
        let effective_timeout = req
            .timeout_ms
            .filter(|ms| *ms > 0)
            .map(|ms| Duration::from_millis(u64::from(ms)))
            .unwrap_or(self.request_timeout);
        // `false`: unlike `request`, the guest's `timeout_ms` must bound ONLY this connect+headers
        // `.send()` (via the outer `tokio::time::timeout` below) — NOT become reqwest's own
        // request-level timeout, which would keep running as a TOTAL-request timer and silently kill
        // the long-lived body stream this function hands back, contradicting this function's own
        // documented contract (see [`Self::build_request`]'s doc for the full `reqwest` internals).
        let client = self.client_for(req.url.as_str()).await?;
        let resp =
            tokio::time::timeout(effective_timeout, self.build_request(&client, req, false)?.send())
            .await
            .map_err(|_| {
                format!("request_stream: timed out after {effective_timeout:?} waiting for the initial response")
            })?
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let headers = collect_headers(resp.headers());
        let encoding = content_encoding_of(resp.headers());
        // Skip decompression entirely for a response that must never carry a coded body (a
        // null-body status, or a HEAD/CONNECT request) — see [`body_may_carry_content_coding`].
        let coding_encoding = if body_may_carry_content_coding(&req.method, status) {
            encoding.as_deref()
        } else {
            None
        };
        let stream: ChunkStream = decode_stream(coding_encoding, resp.bytes_stream())?;
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        let mut g =
            self.streams.lock().map_err(|_| "http stream registry lock poisoned".to_string())?;
        // Re-checked atomically with the insert (the up-front check above is a fast-path only — a
        // concurrent `request_stream` could have raced past it in between): dropping `stream` here
        // (never inserted) cancels the connection we just opened, same as `close_stream` would.
        if g.len() >= self.max_open_streams {
            return Err(format!(
                "too many open http streams ({} already open) — close some via close-stream \
                 before opening more",
                self.max_open_streams
            ));
        }
        g.insert(handle, StreamSlot::Idle(stream));
        Ok(HttpStreamResponse { handle, status, headers })
    }

    /// Drain the next chunk of an open stream (the WIT `poll-stream-chunk`); `Ok(None)` = EOF. A
    /// stream that already hit EOF (or errored) keeps returning `Ok(None)` on repeat polls; only an
    /// unknown handle is an `Err`. A handle already being polled by a concurrent call degrades to
    /// `Ok(None)` for THIS call too (the pre-existing, unchanged, ambiguous-taken-slot behavior —
    /// this fix touches only the `close_stream`-races-an-in-flight-poll accounting, not this).
    pub async fn poll_stream_chunk(&self, handle: u32) -> Result<Option<Vec<u8>>, String> {
        // Take the live stream out of the registry while we `.await` it (a `MutexGuard` cannot be
        // held across an await point — the compiler enforces this since the guard isn't `Send`),
        // marking the slot `Polling { closed: false }` so a racing `close_stream` (see its doc) knows
        // NOT to free the [`MAX_OPEN_STREAMS`] accounting slot until this poll actually completes.
        let stream = {
            let mut g = self
                .streams
                .lock()
                .map_err(|_| "http stream registry lock poisoned".to_string())?;
            match g.get_mut(&handle) {
                Some(slot @ StreamSlot::Idle(_)) => {
                    let StreamSlot::Idle(stream) =
                        std::mem::replace(slot, StreamSlot::Polling { closed: false })
                    else {
                        unreachable!("just matched StreamSlot::Idle above")
                    };
                    Some(stream)
                }
                Some(StreamSlot::Eof | StreamSlot::Polling { .. }) => None,
                None => return Err(format!("no open http stream for handle {handle}")),
            }
        };
        let Some(mut stream) = stream else {
            // Already EOF'd/errored on a prior poll (or a concurrent poll already has it out); the
            // handle stays registered (only `close-stream` removes it) but this call degrades to EOF.
            return Ok(None);
        };
        // Restore the registry entry once this poll concludes, UNLESS `close_stream` ran while it was
        // in flight (`Polling { closed: true }`) — in that case remove the handle HERE instead,
        // exactly when the real resource (the local `stream`, about to drop) is actually released,
        // never earlier (L4 round-12 finding #2a).
        //
        // RAII, not a closure called on each success path. `Polling { closed: false }` is a LATCH:
        // while it is installed the handle answers `Ok(None)` to every other poll and still occupies
        // a `MAX_OPEN_STREAMS` accounting slot that only this poll's completion can free. A latch
        // that is cleared solely on the paths that return normally is a permanent wedge the moment
        // one of them is skipped — this function's four outcomes each used to clear it by hand, so a
        // panic unwinding out of `stream.next()` (a decoder fault; contained upstream by the host's
        // catch_unwind, so the process survives to keep the leak) left the handle stuck `Polling`
        // for the session's lifetime: unusable, uncloseable — `close_stream` only sets
        // `closed: true` and waits for a finalizer that will never run — and permanently one slot
        // off the cap. The same hole would open the day a caller `.await`s this future in a
        // cancellable context; today's only caller bridges it with `block_in_place` + `block_on`
        // (`cyrup-session-svc::host_services::http_poll_stream_chunk`), which cannot be dropped
        // mid-flight, and that is a property of the CALLER, not of this function.
        struct PollFinalizer<'a> {
            caps: &'a HttpCaps,
            handle: u32,
            /// The state to install when the poll concluded normally and `close_stream` did not run.
            /// `None` means the poll never concluded (unwind/cancel) — the `ChunkStream` is being
            /// dropped with the frame, which cancels the connection, so the handle is terminal.
            next: Option<StreamSlot>,
        }

        impl Drop for PollFinalizer<'_> {
            fn drop(&mut self) {
                let next = self.next.take().unwrap_or(StreamSlot::Eof);
                if let Ok(mut g) = self.caps.streams.lock() {
                    match g.get_mut(&self.handle) {
                        Some(StreamSlot::Polling { closed: true }) => {
                            g.remove(&self.handle);
                        }
                        Some(slot @ StreamSlot::Polling { closed: false }) => {
                            *slot = next;
                        }
                        // Defensive only — the state machine above never lets `handle` be in any
                        // other shape (or vanish) while a poll owns it; a silent no-op is the
                        // no-panic-safe fallback if it somehow did.
                        _ => {}
                    }
                }
            }
        }

        let mut finalizer = PollFinalizer { caps: self, handle, next: None };
        // L4 review §6: bound THIS SINGLE poll's wait, never the stream's overall lifetime — a
        // legitimate long-lived SSE/StreamableHTTP connection (the real consumer's actual protocol
        // need, MCP SDK `streamableHttp.js:75-105`) can go quiet between server-pushed messages for a
        // while; see [`HTTP_POLL_IDLE_TIMEOUT`]'s doc for why this must not fire eagerly. On timeout
        // the stream is put straight BACK to `Idle` (never marked EOF/terminal) so a guest that simply
        // polls again keeps draining the SAME still-open connection.
        let poll_idle_timeout = self.poll_idle_timeout;
        match tokio::time::timeout(poll_idle_timeout, stream.next()).await {
            Err(_) => {
                finalizer.next = Some(StreamSlot::Idle(stream));
                Err(format!(
                    "poll_stream_chunk: no chunk within {poll_idle_timeout:?} — the connection \
                     may still be open, poll again"
                ))
            }
            Ok(Some(Ok(bytes))) => {
                // The chunk we already fetched is returned regardless of a racing close — it was real
                // data read off the wire before the close happened, independent of the registry's
                // bookkeeping (the finalizer above still honors the close: removes rather than
                // reinstates `Idle`).
                finalizer.next = Some(StreamSlot::Idle(stream));
                Ok(Some(bytes.to_vec()))
            }
            Ok(Some(Err(e))) => {
                finalizer.next = Some(StreamSlot::Eof); // terminal: subsequent polls degrade to EOF
                if is_network_stream_error(&e) {
                    // A genuine transport failure — always a hard error, exactly as before.
                    Err(e.to_string())
                } else {
                    // L4 round-17 finding #4: a decoder-stage error (the compressed body ended
                    // early or was malformed) degrades to a clean EOF instead of a hard failure —
                    // undici's Z_SYNC_FLUSH-style leniency, see `read_capped`'s doc for the full
                    // rationale. Whatever chunks were already yielded via earlier successful polls
                    // stay delivered; this call simply reports "no more data," matching Node's real
                    // `fetch()` pipeline, which never raises its `onError` for this exact case
                    // (zlib itself doesn't consider a Z_SYNC_FLUSH-flushed truncation an error).
                    Ok(None)
                }
            }
            Ok(None) => {
                finalizer.next = Some(StreamSlot::Eof); // natural EOF
                Ok(None)
            }
        }
    }

    /// Close (drop/cancel) a stream (the WIT `close-stream`). Never an error — mirrors the WIT
    /// signature's `()` return; closing an unknown handle is a silent no-op.
    ///
    /// An `Idle`/`Eof` entry is removed right here — dropping the underlying `reqwest::Response` body
    /// (if any) cancels the in-flight download immediately, and freeing the [`MAX_OPEN_STREAMS`]
    /// accounting slot the same instant is correct: nothing is still using it.
    ///
    /// A `Polling` entry (a [`Self::poll_stream_chunk`] call currently has the real stream taken out,
    /// `.await`-ing its next chunk) is NOT removed here — the real connection and a host blocking
    /// thread stay pinned for up to [`HTTP_POLL_IDLE_TIMEOUT`] regardless of what this function does,
    /// so removing the registry entry now would free the accounting slot long before the resource
    /// is actually released, letting a guest evade [`MAX_OPEN_STREAMS`] by racing `close_stream`
    /// against every `poll_stream_chunk` (L4 round-12 finding #2a). Instead this only flags the entry
    /// `closed`; the in-flight poll's own completion (`poll_stream_chunk`'s `PollFinalizer`, which
    /// runs on unwind and cancellation too, not only on the paths that return normally) performs the
    /// actual removal at the moment the resource genuinely frees.
    pub fn close_stream(&self, handle: u32) {
        if let Ok(mut g) = self.streams.lock() {
            match g.get_mut(&handle) {
                Some(StreamSlot::Polling { closed }) => *closed = true,
                Some(_) => {
                    g.remove(&handle);
                }
                None => {}
            }
        }
    }
}

/// Drain `resp`'s body into memory, capped at [`MAX_RESPONSE_BODY_BYTES`]. Reads chunk-by-chunk
/// (rather than the single-shot `resp.bytes()`, which buffers the WHOLE body internally before
/// this call even gets a chance to look at its length) so a body that exceeds the cap is rejected
/// — dropping the in-flight `stream` cancels the remaining download — without ever holding more
/// than the cap in memory, even transiently. A declared `Content-Length` over the cap is rejected
/// immediately, before reading a single byte; an absent/understated one (chunked transfer) is still
/// caught by the running-total check on every chunk.
async fn read_bounded_body(resp: reqwest::Response) -> Result<Vec<u8>, String> {
    if let Some(len) = resp.content_length()
        && len > MAX_RESPONSE_BODY_BYTES as u64
    {
        return Err(format!(
            "response body ({len} bytes, Content-Length) exceeds the \
             {MAX_RESPONSE_BODY_BYTES}-byte cap"
        ));
    }
    let mut stream = resp.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if body.len() + chunk.len() > MAX_RESPONSE_BODY_BYTES {
            return Err(format!(
                "response body exceeds the {MAX_RESPONSE_BODY_BYTES}-byte cap (Content-Length \
                 absent or understated)"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn collect_headers(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_string(), s.to_string())))
        .collect()
}

/// A `reqwest::ClientBuilder` with the built-in `gzip`/`brotli`/`deflate`/`zstd` auto-decompression
/// disabled (`no_gzip`/`no_brotli`/`no_deflate`/`no_zstd` — real methods regardless of which of
/// those cargo features are compiled in), so the response headers this crate hands back stay the
/// REAL wire headers (matching Pi's real `fetch()`, which preserves `Content-Encoding`/
/// `Content-Length` even while transparently decompressing — see [`HttpCaps::new`]'s doc comment).
fn client_builder() -> reqwest::ClientBuilder {
    // PROV-047 — `.no_proxy()` retires reqwest's OWN environment-proxy detection, leaving the
    // ported resolver ([`HttpCaps::client_for`] → `resolveHttpProxyUrlForTarget`,
    // node-http-proxy.ts:92-112) as the single authority for extension HTTP, exactly as
    // `build_client_with_proxy`'s negative arm already does for provider traffic
    // (`cyrup-provider/src/stream/sse.rs`). Without it, a target the ported resolver declined to
    // proxy could still be proxied by reqwest's separate `no_proxy`/`all_proxy` matching — two
    // implementations disagreeing inside one process, which is the asymmetry PROV-047 names.
    //
    // It does NOT disable the proxy [`HttpCaps::client_through`] installs: `no_proxy()` clears the
    // proxies added BEFORE it and turns off auto system-proxy detection, and that builder adds its
    // `.proxy(..)` after this call.
    reqwest::Client::builder().no_gzip().no_brotli().no_deflate().no_zstd().no_proxy()
}

/// The response's `Content-Encoding` value, verbatim (original casing preserved — this is also the
/// exact string handed back to the guest as the real wire header, which must round-trip
/// byte-for-byte; see [`HttpCaps::request`]'s doc). Matching against known codings is
/// case-INSENSITIVE (RFC 9110 §8.4.1 / RFC 7231 §3.1.2.1: "All content-coding values are
/// case-insensitive"; real consumer: `undici/lib/web/fetch/index.js:2267`'s
/// `contentEncoding.toLowerCase().split(',')`) — done separately, on a lowercase COPY, by
/// [`decode_buffered`]/[`decode_stream`], never here.
fn content_encoding_of(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers.get(reqwest::header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()).map(str::to_owned)
}

/// The status codes that NEVER carry a body (real consumer:
/// `undici/lib/web/fetch/constants.js:6`, `nullBodyStatus = [101, 204, 205, 304]`, the exact vendored
/// copy `@modelcontextprotocol/sdk`'s `fetch()` sits on top of).
const NULL_BODY_STATUS: [u16; 4] = [101, 204, 205, 304];

/// Whether a response may legitimately carry a `Content-Encoding`-coded body worth decoding — real
/// consumer citation: `undici/lib/web/fetch/index.js:2262`'s `onResponseStart`, the exact guard
/// gating decoder-chain construction: `if (request.method !== 'HEAD' && request.method !== 'CONNECT'
/// && !nullBodyStatus.includes(status) && !willFollow)`. `willFollow` (a pending redirect) has no
/// cyrup equivalent to replicate: `reqwest`'s client already follows redirects itself (default
/// policy, unchanged by [`client_builder`]) before this code ever sees a response, so `status` here
/// is always the FINAL response's status, never an intermediate 3xx.
///
/// A `false` result means: attempt NO decompression at all, regardless of what a
/// (spec-nonconforming, since none of these responses should carry one) `Content-Encoding` header
/// claims — matching undici leaving `decoders` empty and handing the raw body straight through
/// (`index.js:2312-2313`), rather than this crate's OLD behavior of decoding unconditionally, which
/// broke on e.g. a `304 Not Modified` carrying a stale `Content-Encoding: gzip` from the original
/// 200 response with a genuinely empty body (real reproduction: `decode_buffered` on `[]` bytes with
/// a `gzip` decoder returns `Err("decompression failed: unexpected end of file")` instead of the
/// empty body straight through).
///
/// Method comparison is case-insensitive: Pi's own `Request` constructor normalizes well-known
/// methods to uppercase before `fetch()` ever sees them (WHATWG Fetch §"method states"), so an
/// extension author passing a lowercase `"head"`/`"connect"` here must not silently fall through
/// undici's exact (case-sensitive) string check and attempt decompression on a body-less response.
fn body_may_carry_content_coding(method: &str, status: u16) -> bool {
    !method.eq_ignore_ascii_case("HEAD")
        && !method.eq_ignore_ascii_case("CONNECT")
        && !NULL_BODY_STATUS.contains(&status)
}

/// Real consumer citation: `undici/lib/web/fetch/index.js:2269-2275` (the exact `fetch()` engine
/// backing `@modelcontextprotocol/sdk`'s pinned version, verified against the copy vendored under
/// `pi/node_modules/undici`) rejects the WHOLE response outright once `Content-Encoding` lists more
/// than this many chained codings, rather than let one crafted response header make the host build
/// an arbitrarily long decoder chain — its own comment: "Limit the number of content-encodings to
/// prevent resource exhaustion. CVE fix similar to urllib3 (GHSA-gm62-xv2j-4w53) and curl
/// (CVE-2022-32206)." [`decode_buffered`]/[`decode_stream`] port that SAME cap, verbatim value
/// (`index.js:2271`'s `const maxContentEncodings = 5`).
const MAX_CONTENT_ENCODINGS: usize = 5;

/// One recognized `Content-Encoding` coding (RFC 9110 §8.4.1's registered set this crate supports).
/// Built by [`resolve_codings`], which owns ALL "is this token known" logic — [`decode_buffered_one`]/
/// [`decode_stream_one`] below never see an unrecognized token at all, so neither has (or needs) an
/// identity fallback arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coding {
    Gzip,
    Br,
    Deflate,
    Zstd,
}

impl Coding {
    /// `"x-gzip"` is `"gzip"`'s RFC 9112 §7.2 legacy alias — real consumer:
    /// `undici/lib/web/fetch/index.js:2280`'s `coding === 'x-gzip' || coding === 'gzip'`. `token` must
    /// already be lowercased + trimmed (`resolve_codings` does both before calling this).
    fn parse(token: &str) -> Option<Self> {
        match token {
            "gzip" | "x-gzip" => Some(Coding::Gzip),
            "br" => Some(Coding::Br),
            "deflate" => Some(Coding::Deflate),
            "zstd" => Some(Coding::Zstd),
            _ => None,
        }
    }
}

/// Resolve a `Content-Encoding` header into the ordered list of [`Coding`] stages to actually apply,
/// ALREADY reversed into application order (index 0 = outermost/last-listed coding, applied first) —
/// the exact real-consumer semantics of `undici/lib/web/fetch/index.js:2275-2303`, not merely "known
/// codings decode, unknown ones pass through": that loop walks tokens from the LAST-listed (outermost)
/// down to the FIRST-listed (innermost), pushing a decoder for each recognized one — but the MOMENT it
/// hits an unrecognized token, it runs `decoders.length = 0; break` (`index.js:2301-2302`), discarding
/// EVERY decoder identified so far (not just failing that one stage) and stopping immediately. A
/// single bad/unknown token ANYWHERE in the chain means the ENTIRE response body is later used
/// UNTOUCHED (`decoders.length ? pipeline(...) : this.body`, `index.js:2312-2313`) — not "every OTHER,
/// recognized stage still decodes normally," which is what an identity-per-unknown-stage fallback
/// (this crate's OLD behavior) would produce instead. Concretely, for `Content-Encoding: "bogus,
/// gzip"` (bogus applied first/innermost, gzip second/outermost): the loop visits `gzip` first (i=1,
/// recognized, pushed), then `bogus` (i=0, unrecognized) — which WIPES the just-pushed gzip decoder
/// and breaks, so the real consumer returns the FULLY RAW bytes, not gzip-decompressed-with-bogus-
/// left-as-is. Mirrored exactly here: `decoders.clear()` (not `continue`/skip) on the first
/// unrecognized token, then `break`.
///
/// `Err` only for the chain-DEPTH cap ([`MAX_CONTENT_ENCODINGS`]) — matching `undici`'s own
/// `reject(...)` (`index.js:2272-2275`): that failure aborts the WHOLE response before any decoder
/// resolution is even attempted, unlike an unrecognized-token discard (which is a normal `Ok(vec![])`,
/// i.e. "decode nothing," exactly like an absent `Content-Encoding` header).
///
/// Matching is case-INSENSITIVE, on a lowercased COPY of `encoding` — RFC 9110 §8.4.1 / RFC 7231
/// §3.1.2.1: "All content-coding values are case-insensitive"; real consumer:
/// `undici/lib/web/fetch/index.js:2267`'s `contentEncoding.toLowerCase().split(',')`, run BEFORE
/// the split, exactly mirrored here. The ORIGINAL `encoding` (and the untouched header this crate
/// hands back to the guest, [`content_encoding_of`]) is never itself modified.
fn resolve_codings(encoding: Option<&str>) -> Result<Vec<Coding>, String> {
    let Some(encoding) = encoding else { return Ok(Vec::new()) };
    let lower = encoding.to_lowercase();
    let tokens: Vec<&str> = lower.split(',').collect();
    if tokens.len() > MAX_CONTENT_ENCODINGS {
        return Err(format!(
            "too many content-encodings in response: {}, maximum allowed is {MAX_CONTENT_ENCODINGS}",
            tokens.len()
        ));
    }
    let mut decoders = Vec::new();
    for token in tokens.into_iter().map(str::trim).rev() {
        match Coding::parse(token) {
            Some(coding) => decoders.push(coding),
            None => {
                decoders.clear();
                break;
            }
        }
    }
    Ok(decoders)
}

/// Decompress a fully-buffered body per `encoding` (the real consumer's `fetch()` semantics — see
/// [`resolve_codings`] for the exact "unknown token discards the whole chain" behavior this ports).
/// Bounds the DECOMPRESSED output at [`MAX_RESPONSE_BODY_BYTES`] on EVERY chained stage (see below),
/// the SAME cap [`read_bounded_body`] already applies to the wire (possibly-compressed) transfer —
/// now that decompression happens manually here rather than inside `reqwest`, a small compressed body
/// must not be able to expand into an unbounded allocation (a decompression bomb).
///
/// `Content-Encoding` may list MULTIPLE codings, e.g. `"gzip, br"` (RFC 9110 §8.4.1: "the codings are
/// listed in the order in which they were applied" — `gzip` first, `br` second, on top). Decoding must
/// undo them in REVERSE: the LAST-listed coding is the OUTERMOST layer, so it comes off first. Verified
/// live against real Node `fetch()` (`zlib.gzipSync` then `brotliCompressSync`, header `gzip, br`):
/// Node decodes both layers back to the original plaintext while still exposing the untouched
/// `content-encoding: gzip, br` header — this loop reproduces that (for an all-recognized chain).
async fn decode_buffered(encoding: Option<&str>, raw: Vec<u8>) -> Result<Vec<u8>, String> {
    let codings = resolve_codings(encoding)?;
    let mut body = raw;
    for coding in codings {
        body = decode_buffered_one(coding, body).await?;
    }
    Ok(body)
}

/// Decompress `raw` per a SINGLE [`Coding`] (one stage of [`decode_buffered`]'s chain, already
/// resolved by [`resolve_codings`] — every variant here is by construction a recognized coding, so
/// there is no identity/fallback arm to reach).
async fn decode_buffered_one(coding: Coding, raw: Vec<u8>) -> Result<Vec<u8>, String> {
    let reader = tokio::io::BufReader::new(std::io::Cursor::new(raw));
    match coding {
        Coding::Gzip => {
            read_capped(async_compression::tokio::bufread::GzipDecoder::new(reader)).await
        }
        Coding::Br => read_capped(async_compression::tokio::bufread::BrotliDecoder::new(reader)).await,
        Coding::Deflate => {
            read_capped(async_compression::tokio::bufread::DeflateDecoder::new(reader)).await
        }
        Coding::Zstd => read_capped(async_compression::tokio::bufread::ZstdDecoder::new(reader)).await,
    }
}

/// Read `r` to EOF, capped at [`MAX_RESPONSE_BODY_BYTES`] of DECOMPRESSED output — rejects rather
/// than growing past the cap, mirroring [`read_bounded_body`]'s running-total check.
///
/// L4 round-17 finding #4: a decode error partway through is treated as a CLEAN EOF — returning
/// whatever was successfully decoded so far — rather than failing the whole call, mirroring the
/// real consumer's deliberate leniency: undici constructs every decoder with `flush`/`finishFlush`
/// set to each codec's non-strict flush action (`zlib.constants.Z_SYNC_FLUSH` for gzip/deflate,
/// `BROTLI_OPERATION_FLUSH` for brotli — `undici/lib/web/fetch/index.js:2277-2298`), with the
/// comment "Be less strict when decoding compressed responses, since sometimes servers send
/// slightly invalid responses that are still accepted by common browsers... Always using
/// Z_SYNC_FLUSH is what cURL does." Verified live against real Node `fetch()`: a truncated
/// (trailer-stripped) gzip/deflate stream decodes CLEANLY to the full plaintext, no error.
///
/// `async-compression` (the exact crate/version this file vendors, `0.4.33`) has no equivalent
/// flush-mode knob — it wraps flate2/brotli/zstd's own STRICT codecs directly, which instead raise
/// `UnexpectedEof`/`BufError`-class `io::Error`s on the exact same truncated bytes, with NO partial
/// output at all for the read call that fails. Returning whatever `out` already holds when that
/// happens is the closest faithful port achievable without reimplementing each codec's streaming
/// API by hand — and it recovers real, substantial (verified live: >98% for a large multi-cycle
/// body truncated right at its trailer) prefixes of the plaintext for any realistic response body
/// that spans more than one internal decode/flush cycle, because `0.4.33`'s decoders DO flush
/// what they've already produced across successive successful `read()` calls before the FINAL
/// (trailer-checking) call fails — verified live by feeding a 200 000-byte payload through this
/// exact code path truncated at its trailer: 24 successful reads totalling 196 608 bytes (98.3%)
/// landed before the terminal error. A message small enough to be decoded in a SINGLE internal
/// cycle (the member is validated as one atomic unit before anything is flushed) instead recovers
/// nothing — `out` stays empty — but that is still strictly better than today's hard failure, and
/// matches Node's own outcome for a truncated zstd stream (`decode_stream_one`'s doc): a clean,
/// empty, error-free EOF rather than a failed request. `r` here is ALWAYS reading from an
/// already-fully-downloaded in-memory buffer (`decode_buffered_one`'s `Cursor<Vec<u8>>` — no network
/// I/O happens during buffered decompression at all), so ANY error `r.read()` can produce is
/// guaranteed to be the decoder's own complaint about the bytes it was given, never a transport
/// failure — there is no ambiguity to resolve here (contrast [`decode_stream_one`]'s streaming
/// path, which reads live off the wire and so DOES need to distinguish the two, via
/// [`is_network_stream_error`]). The cap-exceeded check below is unaffected: it is still a hard
/// `Err`, never softened by this leniency (Node's leniency is about premature-EOF tolerance, not
/// about relaxing our own decompression-bomb guard).
async fn read_capped<R: tokio::io::AsyncRead + Unpin>(mut r: R) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    // `Err(_)` (a decoder-stage error, see the doc above) simply falls out of this `while let` —
    // exactly like a natural `Ok(0)` EOF — returning whatever `out` already holds.
    while let Ok(n) = r.read(&mut buf).await {
        if n == 0 {
            break;
        }
        let Some(chunk) = buf.get(..n) else { break };
        if out.len() + chunk.len() > MAX_RESPONSE_BODY_BYTES {
            return Err(format!(
                "decompressed response body exceeds the {MAX_RESPONSE_BODY_BYTES}-byte cap"
            ));
        }
        out.extend_from_slice(chunk);
    }
    Ok(out)
}

/// Wrap a raw response byte stream through the `async-compression` decoder chain matching `encoding`
/// (the real consumer's `fetch()` semantics — see [`resolve_codings`] for the exact "unknown token
/// discards the whole chain" behavior this ports), used by [`HttpCaps::request_stream`]. Unlike the
/// buffered path, there is no additional cap here: the host never accumulates a streaming body (the
/// guest drains it one already-network-bounded chunk at a time via [`HttpCaps::poll_stream_chunk`] —
/// the same reasoning [`MAX_RESPONSE_BODY_BYTES`]'s doc comment already gives for why streaming
/// doesn't need it), and a decoder that never learns the true end of a bomb-sized declared stream is
/// nothing new: an identity stream of the same declared size is already unbounded in the exact same
/// way.
///
/// `Content-Encoding` may list MULTIPLE codings — see [`decode_buffered`]'s doc for the full
/// RFC 9110 §8.4.1 rationale and the live Node `fetch()` verification. Decoded here the same way:
/// wrap the stream with one decoder per resolved [`Coding`], applied in REVERSE of the listed order
/// (the LAST-listed coding is the outermost layer, unwrapped first). `Err` here means
/// [`HttpCaps::request_stream`] fails the WHOLE call before ever registering a stream handle, matching
/// undici's `reject(...)` (`index.js:2272-2275`) for the [`MAX_CONTENT_ENCODINGS`] depth cap — not a
/// stream that silently opens and then errors on first poll.
fn decode_stream(
    encoding: Option<&str>,
    raw: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
) -> Result<ChunkStream, String> {
    // `reqwest::Error` doesn't satisfy `StreamReader`'s `Into<std::io::Error>` bound directly —
    // map it explicitly once, up front, so every decoder stage below shares one `io::Error`-typed
    // stream. Wrapped in `NetworkStreamError` (not a bare `std::io::Error::other`) so
    // [`is_network_stream_error`] can later tell a genuine transport failure apart from a decoder
    // stage's OWN complaint about the compressed bytes — see [`HttpCaps::poll_stream_chunk`]'s doc
    // for why that distinction matters (L4 round-17 finding #4).
    let mut stream: ChunkStream =
        Box::pin(raw.map(|r| r.map_err(|e| std::io::Error::other(NetworkStreamError(e)))));
    let codings = resolve_codings(encoding)?;
    for coding in codings {
        stream = decode_stream_one(coding, stream);
    }
    Ok(stream)
}

/// Marks an `io::Error` flowing through a [`ChunkStream`] as originating from the underlying
/// NETWORK transport (a `reqwest::Error`), as opposed to an `async-compression` decoder stage's own
/// complaint about the bytes it was fed. [`is_network_stream_error`] downcasts on this TYPE, not on
/// `io::ErrorKind` — `flate2` itself sometimes reports a genuine decompression error via
/// `ErrorKind::Other` (`flate2::mem::DecompressError`'s `From<io::Error>` impl), which would
/// collide with a kind-only heuristic and risk misclassifying a real decoder error as "network," or
/// vice versa. A precise type-based check has no such ambiguity.
#[derive(Debug)]
struct NetworkStreamError(reqwest::Error);

impl std::fmt::Display for NetworkStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for NetworkStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Whether `e` is a [`NetworkStreamError`] (a genuine transport failure) rather than a decoder
/// stage's own error — see [`NetworkStreamError`]'s doc and [`HttpCaps::poll_stream_chunk`]'s use of
/// this to decide whether a chunk-stream error must stay a hard failure (network) or should degrade
/// to a lenient clean EOF (decoder — undici's Z_SYNC_FLUSH-style leniency, [`read_capped`]'s doc).
fn is_network_stream_error(e: &std::io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.downcast_ref::<NetworkStreamError>().is_some())
}

/// Wrap `raw` through the decoder for a SINGLE [`Coding`] (one stage of [`decode_stream`]'s chain,
/// already resolved by [`resolve_codings`] — every variant here is by construction a recognized
/// coding, so there is no identity/fallback arm to reach).
fn decode_stream_one(coding: Coding, raw: ChunkStream) -> ChunkStream {
    match coding {
        Coding::Gzip => {
            let reader = tokio::io::BufReader::new(tokio_util::io::StreamReader::new(raw));
            let decoder = async_compression::tokio::bufread::GzipDecoder::new(reader);
            Box::pin(tokio_util::io::ReaderStream::new(decoder))
        }
        Coding::Br => {
            let reader = tokio::io::BufReader::new(tokio_util::io::StreamReader::new(raw));
            let decoder = async_compression::tokio::bufread::BrotliDecoder::new(reader);
            Box::pin(tokio_util::io::ReaderStream::new(decoder))
        }
        Coding::Deflate => {
            let reader = tokio::io::BufReader::new(tokio_util::io::StreamReader::new(raw));
            let decoder = async_compression::tokio::bufread::DeflateDecoder::new(reader);
            Box::pin(tokio_util::io::ReaderStream::new(decoder))
        }
        Coding::Zstd => {
            let reader = tokio::io::BufReader::new(tokio_util::io::StreamReader::new(raw));
            let decoder = async_compression::tokio::bufread::ZstdDecoder::new(reader);
            Box::pin(tokio_util::io::ReaderStream::new(decoder))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // ------------------------------------------------------------- process-global proxy setting
    //
    // `cyrup_provider::configure_http_proxy` writes a PROCESS-GLOBAL (`stream::sse::HTTP_PROXY_SETTING`),
    // and `HttpCaps::client_for` (unlike `client_through`, deliberately split out for this exact
    // reason — see its own doc comment) reads it on every call. `cyrup-provider`'s OWN test suite hit
    // this identical hazard first and fixed it with the same shape of guard
    // (`cyrup-provider/src/tests/proxy_setting.rs`); that guard is `pub(crate)` to `cyrup-provider`
    // and gated by ITS `#[cfg(test)]`, so it does not exist in the `cyrup-provider` artifact this
    // crate's OWN test binary links against — a separate, crate-local guard is required here.
    //
    // Scope of what this actually fixes: it serializes the ONE test that mutates the setting
    // (`a_proxy_the_resolver_refuses_fails_the_request_instead_of_connecting_directly`) against the
    // specific tests PROVEN to race it (reproduced under `cargo test -p cyrup-ext --features
    // wasm-host`, i.e. the full crate suite, where dozens of OTHER threads are runnable
    // concurrently): the two `client_for`-observing tests below, plus
    // `close_stream_racing_an_in_flight_poll_does_not_free_the_cap_slot_early` and
    // `poll_racing_close_does_not_resurrect_the_closed_handle`, which reach `client_for`
    // transitively through `request_stream`. It does not retrofit every one of this file's other
    // `request`/`request_stream` tests onto the guard — the same trade-off `proxy_setting.rs` makes
    // for the analogous reason: the writer's critical section is now as short as a single
    // `configure_http_proxy` + one resolver `.await` + its own assertions, not the whole multi-second
    // suite, so an UNGUARDED test would need to land its own `client_for` call inside that narrow
    // window to race it — empirically no longer observed (see the fix's own verification) even
    // though it is not a mathematical impossibility for a test not listed above.
    static PROXY_SETTING_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Take the serialization guard. Hold the returned value for the life of the test.
    async fn proxy_setting_guard() -> tokio::sync::MutexGuard<'static, ()> {
        PROXY_SETTING_GUARD.lock().await
    }

    /// Clears the process-global `httpProxy` setting in `Drop` — never only on the success path, so
    /// a panicking assertion cannot leave it set for whichever test (guarded or not) runs next.
    struct ClearProxySettingOnDrop;

    impl Drop for ClearProxySettingOnDrop {
        fn drop(&mut self) {
            cyrup_provider::configure_http_proxy(None);
        }
    }

    /// Spawn a raw-TCP mock HTTP/1.1 server: accepts one connection, drains the request, then writes
    /// each of `parts` as a separate flushed write with a small delay between them (so a real client
    /// observes them as distinct reads, proving genuine incremental delivery — no external network
    /// dependency). Returns the server's `http://127.0.0.1:<port>/path` URL.
    async fn spawn_mock(status_line: &'static str, headers: String, parts: Vec<Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let head = format!("{status_line}\r\n{headers}\r\n");
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.flush().await;
                for part in parts {
                    let _ = sock.write_all(&part).await;
                    let _ = sock.flush().await;
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }
            }
        });
        format!("http://{addr}/probe")
    }

    #[tokio::test]
    async fn request_returns_the_real_status_and_body() {
        let _serial = proxy_setting_guard().await;
        let body = b"hello from the mock server".to_vec();
        let headers = format!("Content-Type: text/plain\r\nContent-Length: {}\r\n", body.len());
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![body.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps.request(&req).await.expect("request succeeds");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, body);
    }

    /// Compress `input` with the REAL reference codec for `coding` — no canned bytes.
    ///
    /// Reached through the same `async-compression` 0.4.33 façade this file already vendors for
    /// decoding, which wraps the reference implementations themselves: `flate2` for gzip/deflate,
    /// Google's `brotli`, and upstream's `zstd` C library. The bytes are therefore as genuine as a
    /// system compressor's, and the round-trip still proves the decoder against real-world output
    /// rather than a fixture.
    ///
    /// This previously shelled out to system `gzip`/`brotli`/`zstd` binaries and `panic!`ed on a
    /// missing one, so three of these tests failed permanently on any machine without `brotli` and
    /// `zstd` installed — an environment dependency masquerading as an assertion, and the reason
    /// the workspace carried a standing "3 failed" baseline. The codecs were linked into this very
    /// crate the whole time.
    async fn compress(coding: Coding, input: &[u8]) -> Vec<u8> {
        compress_at(coding, async_compression::Level::Default, input).await
    }

    /// [`compress`] at an explicit level. `Level::Best` is the `-9` the decompression-bomb test
    /// needs to build a genuinely small wire form.
    async fn compress_at(
        coding: Coding,
        level: async_compression::Level,
        input: &[u8],
    ) -> Vec<u8> {
        use async_compression::tokio::write::{
            BrotliEncoder, DeflateEncoder, GzipEncoder, ZstdEncoder,
        };
        let mut out: Vec<u8> = Vec::new();
        match coding {
            Coding::Gzip => {
                let mut enc = GzipEncoder::with_quality(&mut out, level);
                enc.write_all(input).await.expect("gzip-encode the plaintext");
                enc.shutdown().await.expect("finish the gzip stream");
            }
            Coding::Br => {
                let mut enc = BrotliEncoder::with_quality(&mut out, level);
                enc.write_all(input).await.expect("brotli-encode the plaintext");
                enc.shutdown().await.expect("finish the brotli stream");
            }
            Coding::Deflate => {
                let mut enc = DeflateEncoder::with_quality(&mut out, level);
                enc.write_all(input).await.expect("deflate-encode the plaintext");
                enc.shutdown().await.expect("finish the deflate stream");
            }
            Coding::Zstd => {
                let mut enc = ZstdEncoder::with_quality(&mut out, level);
                enc.write_all(input).await.expect("zstd-encode the plaintext");
                enc.shutdown().await.expect("finish the zstd stream");
            }
        }
        assert!(!out.is_empty(), "the encoder must produce bytes");
        out
    }

    /// The real consumer's `fetch()` (`streamableHttp.js:89,306,443`, `@modelcontextprotocol/sdk
    /// @1.26.0`) auto-decodes a standard `Content-Encoding` per the Fetch spec with zero caller-
    /// visible opt-in, AND (verified live against Node's real `fetch()`, not just the spec)
    /// preserves the original wire `Content-Encoding`/`Content-Length` in `Response.headers` even
    /// though the body it hands back is decompressed. Serves a REAL gzip-compressed body (via the
    /// system `gzip` binary, no canned bytes) with a genuine `Content-Encoding: gzip` header and
    /// asserts BOTH halves: `HttpCaps::request` hands back the DECOMPRESSED plaintext body, AND the
    /// returned headers still carry the real `Content-Encoding: gzip` + the ORIGINAL (compressed)
    /// `Content-Length` — the exact divergence the L4 review found (`reqwest`'s own built-in
    /// decompression strips both).
    #[tokio::test]
    async fn request_transparently_decodes_a_real_gzip_content_encoding() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"hello decompression world, repeated for a real ratio: \
            hello decompression world, hello decompression world"
            .to_vec();
        let gzipped = compress(Coding::Gzip, &plaintext).await;
        assert_ne!(gzipped, plaintext, "sanity: the compressed wire bytes differ from the plaintext");

        let headers = format!(
            "Content-Type: text/plain\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n",
            gzipped.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![gzipped.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps.request(&req).await.expect("request succeeds");
        assert_eq!(resp.status, 200);
        assert_eq!(
            resp.body, plaintext,
            "the body must come back as the DECOMPRESSED plaintext, matching a real fetch() client"
        );
        let get = |name: &str| {
            resp.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        };
        assert_eq!(
            get("content-encoding"),
            Some("gzip"),
            "Content-Encoding must survive decompression, matching real fetch(): {:?}",
            resp.headers
        );
        assert_eq!(
            get("content-length"),
            Some(gzipped.len().to_string().as_str()),
            "Content-Length must still report the ORIGINAL (compressed, wire) size, matching real \
             fetch(), not the decompressed body's length: {:?}",
            resp.headers
        );
    }

    /// THE finding this closes, half 1: `Content-Encoding` matching must be case-INSENSITIVE (RFC
    /// 9110 §8.4.1 / RFC 7231 §3.1.2.1, `undici/lib/web/fetch/index.js:2267`'s
    /// `.toLowerCase()`) — a real server sending `Content-Encoding: GZIP` (mixed/upper case is
    /// legal per the RFC, and observed from real servers) must still decompress, not silently pass
    /// the compressed bytes through as if `gzip` were unrecognized. The ORIGINAL header casing
    /// must still round-trip byte-for-byte to the guest, matching real `fetch()`.
    #[tokio::test]
    async fn request_decodes_an_uppercase_content_encoding_and_preserves_its_original_casing() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"case-insensitive decompression world, repeated: \
            case-insensitive decompression world, case-insensitive decompression world"
            .to_vec();
        let gzipped = compress(Coding::Gzip, &plaintext).await;

        let headers = format!(
            "Content-Type: text/plain\r\nContent-Encoding: GZIP\r\nContent-Length: {}\r\n",
            gzipped.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![gzipped.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps.request(&req).await.expect("request succeeds");
        assert_eq!(
            resp.body, plaintext,
            "an uppercase `GZIP` Content-Encoding must still be decompressed, matching real fetch()"
        );
        let get = |name: &str| {
            resp.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        };
        assert_eq!(
            get("content-encoding"),
            Some("GZIP"),
            "the ORIGINAL casing must still round-trip untouched, matching real fetch(): {:?}",
            resp.headers
        );
    }

    /// THE finding this closes, half 2: `x-gzip` is `gzip`'s RFC 9112 §7.2 legacy alias —
    /// `undici/lib/web/fetch/index.js:2280`'s `coding === 'x-gzip' || coding === 'gzip'` — and must
    /// decompress identically to `gzip`, not fall through to the unrecognized-token identity path.
    #[tokio::test]
    async fn request_decodes_the_x_gzip_legacy_alias() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"x-gzip legacy alias decompression world, repeated: \
            x-gzip legacy alias decompression world, x-gzip legacy alias decompression world"
            .to_vec();
        let gzipped = compress(Coding::Gzip, &plaintext).await;

        let headers = format!(
            "Content-Type: text/plain\r\nContent-Encoding: x-gzip\r\nContent-Length: {}\r\n",
            gzipped.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![gzipped.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps.request(&req).await.expect("request succeeds");
        assert_eq!(
            resp.body, plaintext,
            "`x-gzip` must decompress exactly like `gzip`, matching real fetch()'s legacy alias"
        );
    }

    /// Same finding, the streaming path: `x-gzip` must decompress identically to `gzip` when
    /// draining chunks via `request_stream`/`poll_stream_chunk`, not just the buffered path.
    #[tokio::test]
    async fn request_stream_decodes_the_x_gzip_legacy_alias() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"streaming x-gzip legacy alias world, repeated: \
            streaming x-gzip legacy alias world, streaming x-gzip legacy alias world"
            .to_vec();
        let gzipped = compress(Coding::Gzip, &plaintext).await;

        let headers = format!(
            "Content-Type: text/plain\r\nContent-Encoding: x-gzip\r\nContent-Length: {}\r\n",
            gzipped.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![gzipped]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("stream opens");

        let mut collected = Vec::new();
        while let Some(chunk) = caps.poll_stream_chunk(opened.handle).await.expect("poll succeeds") {
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(
            collected, plaintext,
            "`x-gzip` must decompress exactly like `gzip` over the streaming path too"
        );
    }

    /// THE finding this fix closes (buffered path): a null-body status (real consumer:
    /// `undici/lib/web/fetch/constants.js:6`'s `nullBodyStatus = [101, 204, 205, 304]`) must never
    /// have decompression attempted against it, even when it carries a STALE `Content-Encoding`
    /// header — a real, observed pattern: a `304 Not Modified` reply to a conditional GET echoes the
    /// ORIGINAL cached response's `Content-Encoding` header while sending a genuinely empty body.
    /// Reproduced live against a real mock server: before this fix, `decode_buffered` ran a gzip
    /// decoder over the empty body and failed with `"decompression failed: unexpected end of file"`
    /// instead of returning the (correctly) empty body straight through, matching real `fetch()`
    /// (`index.js:2262`'s `onResponseStart` guard, which skips decoder-chain construction entirely
    /// for a `nullBodyStatus` response).
    #[tokio::test]
    async fn request_with_a_304_and_a_stale_content_encoding_returns_the_empty_body_undecoded() {
        let _serial = proxy_setting_guard().await;
        let headers = "Content-Type: text/plain\r\nContent-Encoding: gzip\r\n".to_string();
        let url = spawn_mock("HTTP/1.1 304 Not Modified", headers, vec![]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps
            .request(&req)
            .await
            .expect("a 304 with a stale Content-Encoding must not fail decompression");
        assert_eq!(resp.status, 304);
        assert_eq!(
            resp.body,
            Vec::<u8>::new(),
            "a null-body status must return the empty body untouched, not attempt to decode it"
        );
        let get = |name: &str| {
            resp.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        };
        assert_eq!(
            get("content-encoding"),
            Some("gzip"),
            "the stale header must still round-trip verbatim, exactly like real fetch(): {:?}",
            resp.headers
        );
    }

    /// Same finding, the streaming path (`request_stream`): a `304` must open (not error) and drain
    /// to immediate EOF with no decode attempted, matching the buffered path above.
    #[tokio::test]
    async fn request_stream_with_a_304_and_a_stale_content_encoding_opens_and_drains_to_eof() {
        let _serial = proxy_setting_guard().await;
        let headers = "Content-Type: text/plain\r\nContent-Encoding: gzip\r\n".to_string();
        let url = spawn_mock("HTTP/1.1 304 Not Modified", headers, vec![]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps
            .request_stream(&req)
            .await
            .expect("a 304 with a stale Content-Encoding must not fail to open the stream");
        assert_eq!(opened.status, 304);
        let chunk = caps.poll_stream_chunk(opened.handle).await.expect("poll succeeds");
        assert_eq!(chunk, None, "a null-body status drains straight to EOF with no decode attempted");
    }

    /// Same finding, the HEAD case: real servers echo the `Content-Encoding` a matching `GET` would
    /// have produced on a `HEAD` response too (RFC 9110 §9.3.2 — a `HEAD` response's header fields
    /// are "identical" to what `GET` would have sent), but a `HEAD` response body is ALWAYS empty
    /// (undici's exact guard: `request.method !== 'HEAD'`) — decoding it must never be attempted.
    #[tokio::test]
    async fn request_head_with_a_content_encoding_header_returns_the_empty_body_undecoded() {
        let _serial = proxy_setting_guard().await;
        let headers = "Content-Type: text/plain\r\nContent-Encoding: gzip\r\nContent-Length: 12345\r\n"
            .to_string();
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "HEAD".into(), url, ..Default::default() };
        let resp =
            caps.request(&req).await.expect("a HEAD response must not fail decompression either");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, Vec::<u8>::new(), "a HEAD response body must come back empty, undecoded");
    }

    /// L4 round-12 finding #2b: `Content-Encoding` may CHAIN multiple codings (RFC 9110 §8.4.1 — "the
    /// codings are listed in the order in which they were applied"). Verified live against real Node
    /// `fetch()` (`zlib.gzipSync` then `brotliCompressSync`, header `gzip, br`): Node decodes BOTH
    /// layers back to the original plaintext while still exposing `content-encoding: gzip, br` on
    /// `resp.headers`. Reproduces that exact scenario with REAL system `gzip`+`brotli` binaries (gzip
    /// applied first, brotli second, on top — the wire header lists them in THAT order) against
    /// `HttpCaps::request`, matching real `fetch()` byte-for-byte.
    #[tokio::test]
    async fn request_transparently_decodes_a_real_chained_gzip_then_brotli_content_encoding() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"chained decompression world, repeated for a real ratio: \
            chained decompression world, chained decompression world"
            .to_vec();
        let gzipped = compress(Coding::Gzip, &plaintext).await;
        let double_compressed = compress(Coding::Br, &gzipped).await;
        assert_ne!(
            double_compressed, plaintext,
            "sanity: the double-compressed wire bytes differ from the plaintext"
        );

        let headers = format!(
            "Content-Type: text/plain\r\nContent-Encoding: gzip, br\r\nContent-Length: {}\r\n",
            double_compressed.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![double_compressed.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps.request(&req).await.expect("request succeeds");
        assert_eq!(resp.status, 200);
        assert_eq!(
            resp.body, plaintext,
            "both chained layers (gzip, then br) must be undone, matching real fetch()'s chained decode"
        );
        let get = |name: &str| {
            resp.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        };
        assert_eq!(
            get("content-encoding"),
            Some("gzip, br"),
            "the untouched, full chained Content-Encoding must survive decompression: {:?}",
            resp.headers
        );
    }

    /// Same finding, the streaming path (`request_stream`/`poll_stream_chunk`): the SAME chained
    /// `gzip, br` `Content-Encoding` must decompress correctly while draining chunks, not just the
    /// buffered `request` path.
    #[tokio::test]
    async fn request_stream_transparently_decodes_a_real_chained_gzip_then_brotli_content_encoding() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"streaming chained decompression world, repeated for a real ratio: \
            streaming chained decompression world, streaming chained decompression world"
            .to_vec();
        let gzipped = compress(Coding::Gzip, &plaintext).await;
        let double_compressed = compress(Coding::Br, &gzipped).await;
        assert_ne!(
            double_compressed, plaintext,
            "sanity: the double-compressed wire bytes differ from the plaintext"
        );

        let headers = format!(
            "Content-Type: application/octet-stream\r\nContent-Encoding: gzip, br\r\nContent-Length: {}\r\n",
            double_compressed.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![double_compressed.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("stream opens");
        assert_eq!(opened.status, 200);
        let get = |name: &str| {
            opened.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        };
        assert_eq!(get("content-encoding"), Some("gzip, br"), "headers: {:?}", opened.headers);

        let mut collected = Vec::new();
        while let Some(chunk) = caps.poll_stream_chunk(opened.handle).await.expect("poll succeeds") {
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(
            collected, plaintext,
            "drained chunks concatenate back to the plaintext with BOTH chained layers undone"
        );
    }

    /// THE finding this closes (buffered path): an UNRECOGNIZED `Content-Encoding` token anywhere in
    /// the chain must discard EVERY decoder identified so far, not just fall back to identity for
    /// that one stage — matching real `fetch()`'s `decoders.length = 0; break`
    /// (`undici/lib/web/fetch/index.js:2301-2302`). `"bogus, gzip"`: real gzip-compressed bytes on
    /// the wire, but the header lists an unrecognized `bogus` coding BEFORE `gzip` — since decoding
    /// walks the header in REVERSE (last-listed = outermost, decoded first), the loop visits `gzip`
    /// first (recognized, would decode) then `bogus` (unrecognized), which wipes the just-identified
    /// gzip decoder. The response body must therefore come back FULLY RAW (still gzip-compressed),
    /// proving this is a real end-to-end behavior over a real mock server + real gzip binary, not
    /// merely a unit check of the resolver function.
    #[tokio::test]
    async fn request_with_an_unrecognized_content_encoding_token_discards_the_whole_chain_not_just_that_stage() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"this must NOT be gzip-decoded when an unknown token poisons the chain";
        let gzipped = compress(Coding::Gzip, plaintext).await;

        let headers = format!(
            "Content-Type: application/octet-stream\r\nContent-Encoding: bogus, gzip\r\nContent-Length: {}\r\n",
            gzipped.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![gzipped.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps.request(&req).await.expect("request succeeds (an unknown token is not itself an error)");
        assert_eq!(resp.status, 200);
        assert_eq!(
            resp.body, gzipped,
            "an unrecognized token anywhere in the chain must discard the WHOLE decoder chain — the \
             body must come back still gzip-compressed, not gzip-decoded with the bad token merely \
             skipped: {:?}",
            resp.body
        );
    }

    /// The order-reversed sibling: `"gzip, bogus"` — `bogus` is now the OUTERMOST (last-listed)
    /// coding, so the reverse-order walk hits it FIRST, before `gzip` is ever even inspected. Same
    /// real-server, real-gzip proof; same expected fully-raw result.
    #[tokio::test]
    async fn request_with_a_trailing_unrecognized_content_encoding_token_also_discards_the_whole_chain() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"an outermost unknown coding must poison the chain before gzip is even seen";
        let gzipped = compress(Coding::Gzip, plaintext).await;

        let headers = format!(
            "Content-Type: application/octet-stream\r\nContent-Encoding: gzip, bogus\r\nContent-Length: {}\r\n",
            gzipped.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![gzipped.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps.request(&req).await.expect("request succeeds");
        assert_eq!(
            resp.body, gzipped,
            "an outermost unrecognized token must ALSO discard the whole chain, including the \
             recognized gzip stage underneath it: {:?}",
            resp.body
        );
    }

    /// Same finding, the streaming path (`request_stream`/`poll_stream_chunk`): drained chunks must
    /// concatenate back to the still-gzip-compressed bytes, not the plaintext.
    #[tokio::test]
    async fn request_stream_with_an_unrecognized_content_encoding_token_discards_the_whole_chain() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"streaming: this must NOT be gzip-decoded when an unknown token is present";
        let gzipped = compress(Coding::Gzip, plaintext).await;

        let headers = format!(
            "Content-Type: application/octet-stream\r\nContent-Encoding: bogus, gzip\r\nContent-Length: {}\r\n",
            gzipped.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![gzipped.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("stream opens");
        assert_eq!(opened.status, 200);

        let mut collected = Vec::new();
        while let Some(chunk) = caps.poll_stream_chunk(opened.handle).await.expect("poll succeeds") {
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(
            collected, gzipped,
            "the drained stream must reconstruct the fully raw (still gzip-compressed) bytes, not the \
             gzip-decoded plaintext"
        );
    }

    /// PROV-047 — extension HTTP must go through pi's proxy resolver, and a resolved proxy must
    /// produce a DISTINCT client from the direct one, cached per proxy URL.
    ///
    /// BEFORE: `HttpCaps` built exactly one `reqwest::Client` at construction and every guest
    /// request used it, so `httpProxy` (and the ambient `HTTP_PROXY` family, which pi's
    /// `getProxyEnv` also reads) reached only the streaming wire APIs. An operator behind a
    /// corporate proxy had working model traffic and silently failing extension traffic.
    ///
    /// The absence side is asserted FIRST: with no proxy resolvable for a plain public URL,
    /// `client_for` must hand back the shared direct client untouched — otherwise the
    /// distinctness assertion below would hold for a `client_for` that proxied everything.
    #[tokio::test]
    async fn a_resolved_proxy_yields_a_distinct_cached_client_and_no_proxy_yields_the_direct_one() {
        let _serial = proxy_setting_guard().await;
        let caps = HttpCaps::new();

        // No proxy configured and none in the ambient env for this target ⇒ the direct client.
        // (`cyrup_provider::stream::sse::configure_http_proxy` is untouched by this test, so the
        // resolver falls through to the ambient environment, which carries no proxy in CI.)
        let direct = caps.client_for("https://example.invalid/x").await.expect("no proxy resolves");
        assert!(
            caps.proxied.lock().expect("cache lock").is_empty(),
            "the no-proxy path must not populate the per-proxy cache"
        );
        drop(direct);

        // A resolved proxy takes the other branch: a client built through `client_builder()` (so
        // the no-auto-decompression contract still holds) and memoized under the proxy URL.
        let proxy = reqwest::Url::parse("http://proxy.internal:3128").expect("proxy url");
        let first = caps.client_through(&proxy).expect("an http proxy builds");
        assert_eq!(
            caps.proxied.lock().expect("cache lock").len(),
            1,
            "the proxied client must be cached under its proxy URL"
        );
        let _second = caps.client_through(&proxy).expect("an http proxy builds");
        assert_eq!(
            caps.proxied.lock().expect("cache lock").len(),
            1,
            "a second request for the same proxy must reuse the cached client, not rebuild it"
        );

        // A second, different proxy gets its own entry — the cache is keyed, not a single slot.
        let other = reqwest::Url::parse("http://other.internal:8080").expect("proxy url");
        let _third = caps.client_through(&other).expect("an http proxy builds");
        assert_eq!(caps.proxied.lock().expect("cache lock").len(), 2);
        drop(first);
    }

    /// PROV-047 — a proxy setting the resolver REFUSES must fail the guest's request, never fall
    /// through to a direct connection.
    ///
    /// BEFORE: `client_for` logged `tracing::warn!("ignoring proxy setting for extension HTTP
    /// request")` and returned the DIRECT client. An operator who set `httpProxy` to a SOCKS URL
    /// (the resolver's one refusal, `node-http-proxy.ts:89`) got extension HTTP that silently
    /// egressed straight past the proxy their network policy exists to enforce, while every other
    /// egress path in the process — provider streams, OAuth, catalog refresh, all on
    /// `build_client_for_target` — refused the same setting outright. Upstream never connects
    /// direct here either: extension `fetch` runs on the global `EnvHttpProxyAgent`, whose
    /// `ProxyAgent` tunnels `socks5:` itself (`undici/lib/dispatcher/proxy-agent.js:143,167`).
    ///
    /// The setting is process-global, so this test both sets and clears it; the clear is in `Drop`
    /// so a failing assertion cannot leak it into the loopback tests around it.
    #[tokio::test]
    async fn a_proxy_the_resolver_refuses_fails_the_request_instead_of_connecting_directly() {
        let _serial = proxy_setting_guard().await;
        let caps = HttpCaps::new();
        let _restore = ClearProxySettingOnDrop;
        cyrup_provider::configure_http_proxy(Some("socks5://127.0.0.1:1080".to_string()));

        let err = caps
            .client_for("https://example.invalid/x")
            .await
            .expect_err("a SOCKS httpProxy must fail the request, not silently go direct");
        assert!(
            err.contains(cyrup_provider::UNSUPPORTED_PROXY_PROTOCOL_MESSAGE),
            "the guest must be told WHICH setting refused it, got {err:?}"
        );
        assert!(
            caps.proxied.lock().expect("cache lock").is_empty(),
            "a refused proxy must not be cached"
        );

        // The same call with the setting cleared resolves to the direct client again — so the
        // refusal above is the SETTING's doing, not this URL's.
        cyrup_provider::configure_http_proxy(None);
        caps.client_for("https://example.invalid/x")
            .await
            .expect("with no proxy configured the direct client is handed back");
    }

    /// `Accept-Encoding` defaults must match real `fetch()`'s request-header algorithm exactly
    /// (`undici/lib/web/fetch/index.js:1552-1566`), not `reqwest`'s own auto-decompression toggle set
    /// — scheme-conditional (`https:` → `"br, gzip, deflate"`, anything else → `"gzip, deflate"`,
    /// NEVER `zstd` in either arm), a `Range` header present → `"identity"` regardless of scheme, and
    /// a caller-supplied `Accept-Encoding` always left untouched. `build_request` only materializes a
    /// `reqwest::Request` (`.build()`) — no network I/O, so both the `https://` and `http://` arms are
    /// exercised directly without needing a live TLS listener.
    #[test]
    fn build_request_defaults_accept_encoding_scheme_conditionally_and_identity_for_range() {
        let caps = HttpCaps::new();
        let get_accept_encoding = |req: &HttpRequest| {
            caps.build_request(&caps.client.clone(), req, true)
                .expect("builds")
                .build()
                .expect("materializes without connecting")
                .headers()
                .get(reqwest::header::ACCEPT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };

        let req = HttpRequest { method: "GET".into(), url: "https://example.invalid/x".into(), ..Default::default() };
        assert_eq!(
            get_accept_encoding(&req).as_deref(),
            Some("br, gzip, deflate"),
            "https:// must default to br,gzip,deflate — never zstd"
        );

        let req = HttpRequest { method: "GET".into(), url: "http://example.invalid/x".into(), ..Default::default() };
        assert_eq!(
            get_accept_encoding(&req).as_deref(),
            Some("gzip, deflate"),
            "non-https must default to gzip,deflate only"
        );

        let req = HttpRequest {
            method: "GET".into(),
            url: "https://example.invalid/x".into(),
            headers: vec![("Range".to_string(), "bytes=0-99".to_string())],
            ..Default::default()
        };
        assert_eq!(
            get_accept_encoding(&req).as_deref(),
            Some("identity"),
            "a Range request must get identity, even on https:// which would otherwise get br,gzip,deflate"
        );

        let req = HttpRequest {
            method: "GET".into(),
            url: "https://example.invalid/x".into(),
            headers: vec![("Accept-Encoding".to_string(), "gzip".to_string())],
            ..Default::default()
        };
        assert_eq!(
            get_accept_encoding(&req).as_deref(),
            Some("gzip"),
            "an explicit caller-supplied Accept-Encoding must never be overridden"
        );
    }

    /// THE finding this closes (buffered path): a `Content-Encoding` chaining MORE than
    /// [`MAX_CONTENT_ENCODINGS`] tokens must be rejected OUTRIGHT — matching
    /// `undici/lib/web/fetch/index.js:2272-2275`'s `reject(...)` — rather than the host building
    /// and running an arbitrarily long decoder chain per request. Six tokens (one over the cap of
    /// five); the body content is irrelevant since the cap must fire BEFORE any decompression stage
    /// ever runs.
    #[tokio::test]
    async fn request_rejects_a_content_encoding_chain_over_the_max_depth() {
        let _serial = proxy_setting_guard().await;
        let headers = "Content-Type: application/octet-stream\r\n\
            Content-Encoding: gzip, br, deflate, zstd, gzip, br\r\n\
            Content-Length: 3\r\n"
            .to_string();
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![b"abc".to_vec()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let err = caps
            .request(&req)
            .await
            .expect_err("a 6-deep content-encoding chain must be rejected, over the 5-deep cap");
        assert!(
            err.contains("too many content-encodings") && err.contains('6') && err.contains('5'),
            "the error must name both the actual depth and the cap, matching undici's own message: {err}"
        );
    }

    /// Undici's `onResponseStart` rejects an over-depth `Content-Encoding` chain synchronously off
    /// HEADERS ALONE, before any body byte is read (`undici/lib/web/fetch/index.js:2262-2275`). Prove
    /// `HttpCaps::request` matches that ordering (not just the eventual error) by holding a mock
    /// server's connection open with headers sent but NO body bytes ever written, for far longer than
    /// this test's own bound: a pre-fix implementation that downloads the (unbounded-wait) body before
    /// checking the depth cap would hang past that bound; the fixed ordering rejects immediately.
    #[tokio::test]
    async fn request_rejects_the_content_encoding_depth_cap_before_downloading_the_body() {
        let _serial = proxy_setting_guard().await;
        let headers = "Content-Type: application/octet-stream\r\n\
            Content-Encoding: gzip, br, deflate, zstd, gzip, br\r\n"
            .to_string();
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let head = format!("HTTP/1.1 200 OK\r\n{headers}\r\n");
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.flush().await;
                // Hold the connection open with no body bytes, well past the 2s bound below — a
                // pre-fix implementation would block here inside `read_bounded_body`.
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        });
        let url = format!("http://{addr}/probe");

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let err = tokio::time::timeout(std::time::Duration::from_secs(2), caps.request(&req))
            .await
            .expect(
                "request must reject the chain-depth cap immediately off headers, \
                 never waiting on the (never-arriving) body",
            )
            .expect_err("a 6-deep content-encoding chain must be rejected, over the 5-deep cap");
        assert!(
            err.contains("too many content-encodings") && err.contains('6') && err.contains('5'),
            "the error must name both the actual depth and the cap, matching undici's own message: {err}"
        );
    }

    /// Same finding, the streaming path: `request_stream` itself must fail (no stream handle ever
    /// registered) rather than opening a stream that only errors on first poll — matching undici's
    /// `reject(...)` happening before the response promise ever resolves.
    #[tokio::test]
    async fn request_stream_rejects_a_content_encoding_chain_over_the_max_depth() {
        let _serial = proxy_setting_guard().await;
        let headers = "Content-Type: application/octet-stream\r\n\
            Content-Encoding: gzip, br, deflate, zstd, gzip, br\r\n\
            Content-Length: 3\r\n"
            .to_string();
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![b"abc".to_vec()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let err = caps
            .request_stream(&req)
            .await
            .expect_err("a 6-deep content-encoding chain must be rejected, over the 5-deep cap");
        assert!(
            err.contains("too many content-encodings") && err.contains('6') && err.contains('5'),
            "the error must name both the actual depth and the cap, matching undici's own message: {err}"
        );
    }

    /// Same finding, the streaming path (`request_stream`/`poll_stream_chunk`): a REAL zstd-
    /// compressed body (via the system `zstd` binary) must decompress the DRAINED chunks while the
    /// initiating response's headers (captured before any chunk is polled) still carry the real
    /// `Content-Encoding: zstd` + original compressed `Content-Length`.
    #[tokio::test]
    async fn request_stream_transparently_decodes_a_real_zstd_content_encoding() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"streaming zstd decompression world, repeated for a real ratio: \
            streaming zstd decompression world, streaming zstd decompression world"
            .to_vec();
        let compressed = compress(Coding::Zstd, &plaintext).await;
        assert_ne!(compressed, plaintext, "sanity: the compressed wire bytes differ from the plaintext");

        let headers = format!(
            "Content-Type: application/octet-stream\r\nContent-Encoding: zstd\r\nContent-Length: {}\r\n",
            compressed.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![compressed.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("stream opens");
        assert_eq!(opened.status, 200);
        let get = |name: &str| {
            opened.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        };
        assert_eq!(get("content-encoding"), Some("zstd"), "headers: {:?}", opened.headers);
        assert_eq!(
            get("content-length"),
            Some(compressed.len().to_string().as_str()),
            "the ORIGINAL compressed length, not the decompressed one: {:?}",
            opened.headers
        );

        let mut collected = Vec::new();
        while let Some(chunk) = caps.poll_stream_chunk(opened.handle).await.expect("poll succeeds") {
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(
            collected, plaintext,
            "drained chunks concatenate back to the DECOMPRESSED plaintext, matching real fetch()"
        );
    }

    /// L4 round-17 finding #4: a TRUNCATED (trailer-stripped) gzip body must decode LENIENTLY, not
    /// hard-fail the whole request — Node's real `fetch()` (undici) constructs its gzip decoder
    /// with `flush`/`finishFlush` set to `Z_SYNC_FLUSH` specifically for this ("Be less strict when
    /// decoding compressed responses, since sometimes servers send slightly invalid responses that
    /// are still accepted by common browsers... Always using Z_SYNC_FLUSH is what cURL does.",
    /// `undici/lib/web/fetch/index.js:2277-2288`) and recovers the plaintext with no error.
    ///
    /// Uses a LARGE (200 000-byte decompressed) body so `async-compression` 0.4.33's own internal
    /// buffering has room to flush across multiple `read()` calls before its terminal
    /// (missing-trailer) error — see `read_capped`'s doc for the full empirically-verified
    /// breakdown (98.3% recovered, 24 successful reads, for this exact scenario). This crate's fix
    /// cannot literally match Node's byte-for-byte full recovery without reimplementing the codec's
    /// streaming API by hand (`read_capped`'s doc); asserting a real, substantial recovered PREFIX
    /// — not exact full-body equality — is what's actually achievable and verified, and is still a
    /// strictly-better outcome than today's hard failure.
    #[tokio::test]
    async fn request_lenient_decodes_a_truncated_gzip_body_recovering_most_of_the_plaintext() {
        let _serial = proxy_setting_guard().await;
        let plaintext: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let gzipped = compress(Coding::Gzip, &plaintext).await;
        let truncated = gzipped[..gzipped.len() - 8].to_vec(); // strip the trailing CRC32+ISIZE

        let headers = format!(
            "Content-Type: application/octet-stream\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n",
            truncated.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![truncated]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps
            .request(&req)
            .await
            .expect("a truncated gzip body must decode leniently, not fail the whole request");
        assert_eq!(resp.status, 200);
        assert!(
            resp.body.len() > plaintext.len() * 9 / 10,
            "must recover the vast majority of the plaintext (verified live: 98.3% for this exact \
             scenario), got only {} of {} bytes",
            resp.body.len(),
            plaintext.len()
        );
        assert_eq!(
            resp.body,
            plaintext[..resp.body.len()],
            "the recovered bytes must be an exact PREFIX of the real plaintext, not garbage"
        );
    }

    /// A truncation small enough to be decoded within a SINGLE internal decode/flush cycle (see
    /// `read_capped`'s doc) recovers NOTHING via this crate's fix — but must still succeed with an
    /// empty body rather than hard-failing the request, exactly like Node's own outcome for a
    /// truncated zstd stream (`decode_stream_one`'s doc: "clean end, empty output, no error").
    #[tokio::test]
    async fn request_lenient_decodes_a_tiny_truncated_gzip_body_to_an_empty_but_non_error_result() {
        let _serial = proxy_setting_guard().await;
        let plaintext = b"hello decompression world, repeated for a real ratio: \
            hello decompression world, hello decompression world"
            .to_vec();
        let gzipped = compress(Coding::Gzip, &plaintext).await;
        let truncated = gzipped[..gzipped.len() - 8].to_vec();

        let headers = format!(
            "Content-Type: text/plain\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n",
            truncated.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![truncated]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps
            .request(&req)
            .await
            .expect("a truncated gzip body must decode leniently, not fail the whole request");
        assert_eq!(resp.status, 200);
        assert!(
            resp.body.is_empty(),
            "a single-decode-cycle truncation recovers nothing (verified live), but must still \
             succeed rather than hard-fail: got {:?}",
            resp.body
        );
    }

    /// Same finding, the streaming path (`request_stream`/`poll_stream_chunk`): a decoder-stage
    /// error must degrade `poll_stream_chunk` to a clean `Ok(None)` EOF, not a hard `Err` — see
    /// `request_lenient_decodes_a_truncated_gzip_body_recovering_most_of_the_plaintext` above for
    /// the full Node/undici rationale and why a real (not exact-byte) PREFIX is what's asserted.
    #[tokio::test]
    async fn request_stream_lenient_decodes_a_truncated_gzip_body_recovering_most_of_the_plaintext()
    {
        let _serial = proxy_setting_guard().await;
        let plaintext: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let gzipped = compress(Coding::Gzip, &plaintext).await;
        let truncated = gzipped[..gzipped.len() - 8].to_vec();

        let headers = format!(
            "Content-Type: application/octet-stream\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n",
            truncated.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![truncated]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("stream opens");
        assert_eq!(opened.status, 200);

        let mut collected = Vec::new();
        while let Some(chunk) = caps.poll_stream_chunk(opened.handle).await.expect(
            "a decoder-stage truncation error must degrade to a clean EOF, never a hard Err",
        ) {
            collected.extend_from_slice(&chunk);
        }
        assert!(
            collected.len() > plaintext.len() * 9 / 10,
            "must recover the vast majority of the plaintext over the streaming path too, got only \
             {} of {} bytes",
            collected.len(),
            plaintext.len()
        );
        assert_eq!(
            collected,
            plaintext[..collected.len()],
            "the recovered bytes must be an exact PREFIX of the real plaintext, not garbage"
        );
    }

    /// L4 round-17 finding #4 (regression guard): a GENUINE transport failure mid-stream — the
    /// server declares a `Content-Length` it never actually delivers, cutting the connection short
    /// — must STILL surface as a hard `Err` from `poll_stream_chunk`, never silently degrade to a
    /// clean EOF the way a decoder-stage truncation now does. This response carries NO
    /// `Content-Encoding` at all (no decoder stage runs), so the only way `poll_stream_chunk` could
    /// still wrongly swallow this is if `is_network_stream_error` failed to tell "network" and
    /// "decoder" errors apart.
    #[tokio::test]
    async fn request_stream_poll_still_hard_fails_on_a_genuine_mid_stream_transport_failure() {
        let _serial = proxy_setting_guard().await;
        let headers = "Content-Type: text/plain\r\nContent-Length: 100\r\n".to_string();
        // Only 10 of the promised 100 bytes are ever sent before the mock server's connection
        // (and so the underlying TCP stream) closes.
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![b"0123456789".to_vec()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("stream opens");
        let mut saw_hard_err = false;
        loop {
            match caps.poll_stream_chunk(opened.handle).await {
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(_) => {
                    saw_hard_err = true;
                    break;
                }
            }
        }
        assert!(
            saw_hard_err,
            "a genuine transport failure (body cut short of its declared Content-Length) must \
             still surface as a hard Err, not silently degrade to a clean EOF"
        );
    }

    /// A small compressed body that decompresses to something far larger than
    /// [`MAX_RESPONSE_BODY_BYTES`] (a decompression bomb) must still be rejected — now that
    /// decompression happens manually (`decode_buffered`), this cap is no longer `reqwest`'s
    /// responsibility, so it must be reasserted independently of the wire-size cap
    /// (`request_rejects_a_declared_content_length_over_the_cap` already covers the wire side).
    #[tokio::test]
    async fn request_rejects_a_decompression_bomb_over_the_cap() {
        let _serial = proxy_setting_guard().await;
        // Highly compressible: one repeated byte, decompressed size deliberately over the cap.
        let huge = vec![b'x'; MAX_RESPONSE_BODY_BYTES + 4096];
        let gzipped = compress_at(Coding::Gzip, async_compression::Level::Best, &huge).await;
        assert!(
            gzipped.len() < MAX_RESPONSE_BODY_BYTES / 4,
            "sanity: the compressed wire form must be small (a real bomb), got {} bytes",
            gzipped.len()
        );

        let headers = format!(
            "Content-Type: application/octet-stream\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n",
            gzipped.len()
        );
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![gzipped]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let err = caps.request(&req).await.expect_err("a decompression bomb must be rejected");
        assert!(
            err.contains("cap"),
            "the error must explain the decompressed-size cap was hit: {err}"
        );
    }

    #[tokio::test]
    async fn request_stream_yields_real_chunks_in_order_then_eof() {
        let _serial = proxy_setting_guard().await;
        let parts: Vec<Vec<u8>> =
            vec![b"chunk-one-".to_vec(), b"chunk-two-".to_vec(), b"chunk-three".to_vec()];
        let total: usize = parts.iter().map(Vec::len).sum();
        let headers = format!("Content-Type: application/octet-stream\r\nContent-Length: {total}\r\n");
        let url = spawn_mock("HTTP/1.1 200 OK", headers, parts.clone()).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("stream opens");
        assert_eq!(opened.status, 200, "the initiating response's real status is captured");
        let handle = opened.handle;

        let mut collected = Vec::new();
        let mut chunk_count = 0usize;
        while let Some(chunk) = caps.poll_stream_chunk(handle).await.expect("poll succeeds") {
            chunk_count += 1;
            collected.extend_from_slice(&chunk);
        }
        let expected: Vec<u8> = parts.concat();
        assert_eq!(collected, expected, "chunks concatenate back to the real body, in order");
        assert!(chunk_count >= 2, "the delayed writes arrived as multiple distinct chunks: {chunk_count}");

        // EOF is sticky: polling again still returns `Ok(None)`, never re-erroring.
        assert_eq!(caps.poll_stream_chunk(handle).await.expect("poll after EOF"), None);
    }

    /// THE HIGH finding this closes: `request_stream`'s explicit non-zero `timeout_ms` used to ALSO
    /// become `reqwest`'s own request-level (TOTAL, not just connect+headers) timeout via
    /// `build_request`, silently killing the body stream mid-read even though the doc comment (and,
    /// via the OUTER `tokio::time::timeout` around `.send()`, the actual intent) promises only the
    /// initial connect+headers phase is bounded by it. A real TCP server declares `Content-Length: 10`,
    /// writes the first 5 bytes immediately, then the remaining 5 bytes after a 400ms delay — well
    /// past the 200ms `timeout_ms` under test, but well within the stream's own (separate, much
    /// longer) per-poll idle ceiling. Pre-fix: the second `poll_stream_chunk` errors (`reqwest`'s
    /// `TotalTimeoutBody` firing mid-read). Post-fix: the full 10 bytes drain successfully.
    #[tokio::test]
    async fn request_stream_survives_a_slow_body_past_an_explicit_non_zero_timeout_ms() {
        let _serial = proxy_setting_guard().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let head =
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 10\r\n\r\n";
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(b"AAAAA").await;
                let _ = sock.flush().await;
                // Well past the 200ms `timeout_ms` under test.
                tokio::time::sleep(Duration::from_millis(400)).await;
                let _ = sock.write_all(b"BBBBB").await;
                let _ = sock.flush().await;
            }
        });

        let caps = HttpCaps::new();
        let req = HttpRequest {
            method: "GET".into(),
            url: format!("http://{addr}/probe"),
            timeout_ms: Some(200),
            ..Default::default()
        };
        let opened = caps
            .request_stream(&req)
            .await
            .expect("the connect+headers phase completes well within the 200ms timeout_ms");

        let mut collected = Vec::new();
        while let Some(chunk) = caps
            .poll_stream_chunk(opened.handle)
            .await
            .expect("the still-healthy body stream must survive past the connect-phase timeout_ms")
        {
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(
            collected, b"AAAAABBBBB",
            "the full body, including the slow tail written after timeout_ms elapsed, must drain intact"
        );
    }

    /// Closes L4 §2.3: the initiating response's status+headers must be readable off
    /// `request_stream`'s own return, from the SAME round trip that opens the body, BEFORE and
    /// INDEPENDENT of ever draining a chunk — this is exactly what the real consumer (the MCP SDK's
    /// `StreamableHTTPClientTransport`) needs (`response.status` for 401 => re-auth,
    /// `response.headers.get('mcp-session-id')`) off the same response it then streams. Uses a
    /// non-200 status and a distinguishing header to prove these are the REAL server values, not a
    /// hardcoded default.
    #[tokio::test]
    async fn request_stream_exposes_real_status_and_headers_before_draining_the_body() {
        let _serial = proxy_setting_guard().await;
        let body = b"unauthorized-body-still-streamable".to_vec();
        let headers = format!(
            "Content-Type: text/event-stream\r\nMcp-Session-Id: sess-42\r\nContent-Length: {}\r\n",
            body.len()
        );
        let url = spawn_mock("HTTP/1.1 401 Unauthorized", headers, vec![body.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("stream opens even on non-2xx status");

        // Status+headers are already available here — no chunk has been polled yet.
        assert_eq!(opened.status, 401, "the real non-2xx status is surfaced, not swallowed");
        let content_type = opened
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_str());
        assert_eq!(content_type, Some("text/event-stream"));
        let session_id = opened
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("mcp-session-id"))
            .map(|(_, v)| v.as_str());
        assert_eq!(session_id, Some("sess-42"), "the real mcp-session-id header round-trips");

        // The body is still fully drainable afterward — status/headers cost nothing extra.
        let mut collected = Vec::new();
        while let Some(chunk) = caps.poll_stream_chunk(opened.handle).await.expect("poll succeeds") {
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(collected, body);
    }

    #[tokio::test]
    async fn close_stream_invalidates_the_handle() {
        let _serial = proxy_setting_guard().await;
        let url = spawn_mock("HTTP/1.1 200 OK", "Content-Length: 0\r\n".into(), vec![]).await;
        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("stream opens");
        caps.close_stream(opened.handle);
        let err = caps.poll_stream_chunk(opened.handle).await.expect_err("closed handle is unknown");
        assert!(err.contains("no open http stream"), "got: {err}");
    }

    /// TOCTOU regression: `close_stream` racing a genuinely in-flight `poll_stream_chunk` (the
    /// `stream.next().await` window, where the live stream sits OUTSIDE the registry in
    /// `poll_stream_chunk`'s own local variable) must NOT resurrect the handle. Before the fix,
    /// `poll_stream_chunk` unconditionally re-inserted `Some(stream)`/`None` after the await,
    /// silently undoing the concurrent `close_stream`'s removal. Uses a REAL mock server with a
    /// real delay BEFORE its first (and only) chunk, and REAL concurrent tasks (`tokio::spawn`, not
    /// a single-threaded interleaving trick) — the poll is provably in-flight (awaiting the network)
    /// at the moment `close_stream` runs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn poll_racing_close_does_not_resurrect_the_closed_handle() {
        let _serial = proxy_setting_guard().await;
        let body = b"the one real chunk".to_vec();
        let server_body = body.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                // Headers land immediately (so `request_stream` returns fast and we get a handle),
                // but the body is delayed — this is the window `poll_stream_chunk` blocks in.
                let head = "HTTP/1.1 200 OK\r\nContent-Length: 19\r\n\r\n";
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let _ = sock.write_all(&server_body).await;
                let _ = sock.flush().await;
            }
        });
        let url = format!("http://{addr}/probe");

        let caps = Arc::new(HttpCaps::new());
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("stream opens");
        let handle = opened.handle;

        // Start the poll — it immediately blocks on `stream.next().await` for ~200ms.
        let poll_caps = caps.clone();
        let poll_task = tokio::spawn(async move { poll_caps.poll_stream_chunk(handle).await });

        // Give the poll a real head start so it is GENUINELY in-flight (awaiting the network), then
        // close the handle WHILE the poll is still pending.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        caps.close_stream(handle);

        // The in-flight poll still returns the REAL chunk it was already fetching — closing doesn't
        // retroactively fail an already-started read.
        let polled = poll_task.await.expect("poll task joins").expect("poll succeeds");
        assert_eq!(polled, Some(body), "the in-flight poll still returns the real chunk it fetched");

        // THE fix: the handle must NOT have been resurrected by the poll's post-await re-insert —
        // it must stay genuinely closed (`Err`, matching `close_stream_invalidates_the_handle`
        // above), never silently degrade to `Ok(None)` (which would mean the registry entry came
        // back to life).
        let err = caps.poll_stream_chunk(handle).await.expect_err("closed handle stays closed");
        assert!(
            err.contains("no open http stream"),
            "the handle must stay closed after racing with an in-flight poll, got: {err}"
        );
    }

    /// L4 round-12 finding #2a: `close_stream` racing an in-flight `poll_stream_chunk` must NOT free
    /// the [`MAX_OPEN_STREAMS`] accounting slot before the real resource (the connection + a host
    /// blocking thread, `cyrup-session-svc::host_services::http_poll_stream_chunk`'s
    /// `block_in_place`+`block_on`) is actually released. Uses a real mock server with a delayed body
    /// chunk (so the poll is genuinely in flight, awaiting the network — same proof technique as
    /// [`poll_racing_close_does_not_resurrect_the_closed_handle`]) and
    /// `HttpCaps::with_max_open_streams(1)` so a SECOND `request_stream` attempt directly observes
    /// whether the cap was (wrongly) freed early by the racing `close_stream`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn close_stream_racing_an_in_flight_poll_does_not_free_the_cap_slot_early() {
        let _serial = proxy_setting_guard().await;
        let body = b"the delayed chunk".to_vec();
        let server_body = body.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", server_body.len());
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let _ = sock.write_all(&server_body).await;
                let _ = sock.flush().await;
            }
        });
        let url = format!("http://{addr}/probe");

        let caps = Arc::new(HttpCaps::with_max_open_streams(1));
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let opened = caps.request_stream(&req).await.expect("first stream opens under the cap of 1");
        let handle = opened.handle;

        // Start the poll — it immediately blocks on `stream.next().await` for ~200ms.
        let poll_caps = caps.clone();
        let poll_task = tokio::spawn(async move { poll_caps.poll_stream_chunk(handle).await });

        // Give the poll a real head start so it is genuinely in-flight, then close the handle WHILE
        // the poll is still pending — the exact race this finding is about.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        caps.close_stream(handle);

        // THE fix: even though `close_stream` already ran, the real resource (the poll) is still in
        // flight — a second `request_stream` must still be rejected by the cap of 1, proving the
        // accounting slot was NOT freed early.
        let second_url = spawn_persistent_mock().await;
        let second_req = HttpRequest { method: "GET".into(), url: second_url, ..Default::default() };
        let err = caps
            .request_stream(&second_req)
            .await
            .expect_err("the cap slot must stay held while the closed stream's poll is still in flight");
        assert!(err.contains("too many open http streams"), "got: {err}");

        // Let the in-flight poll actually finish — it observes the close and releases the slot for
        // real at that point, not before.
        let polled = poll_task.await.expect("poll task joins").expect("poll succeeds");
        assert_eq!(polled, Some(body), "the in-flight poll still returns the real chunk it fetched");

        // NOW the slot is genuinely free: a fresh request_stream succeeds.
        caps.request_stream(&second_req)
            .await
            .expect("a stream opens once the raced-closed stream's poll has genuinely completed");
    }

    /// A mock server that accepts MANY connections (not just one, unlike [`spawn_mock`]), each
    /// answered with a small keep-nothing-open response — needed to open enough concurrent streams
    /// to actually reach [`MAX_OPEN_STREAMS`] in a test.
    async fn spawn_persistent_mock() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
                    let _ = sock.flush().await;
                });
            }
        });
        format!("http://{addr}/probe")
    }

    /// Closes the shared-host-memory-exhaustion finding: `request_stream` is rejected once the cap
    /// is reached (a guest that keeps opening streams without ever closing/draining them must NOT
    /// be able to grow the registry without bound) — and, once one is genuinely closed, a fresh
    /// `request_stream` succeeds again (the cap is a real, live gate on CURRENTLY open streams, not
    /// a one-shot lifetime limit). Uses [`HttpCaps::with_max_open_streams`]'s test-only SMALL cap
    /// rather than the real [`MAX_OPEN_STREAMS`] (256) — opening that many real concurrent sockets
    /// is flaky under full-suite parallel test execution (contends for loopback networking
    /// resources with every other test running at the same time); the cap-enforcement MECHANISM
    /// being tested is identical regardless of the configured limit's value.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn request_stream_rejects_once_the_open_stream_cap_is_reached() {
        let _serial = proxy_setting_guard().await;
        const SMALL_CAP: usize = 4;
        let url = spawn_persistent_mock().await;
        let caps = HttpCaps::with_max_open_streams(SMALL_CAP);
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };

        let mut handles = Vec::with_capacity(SMALL_CAP);
        for _ in 0..SMALL_CAP {
            let opened = caps.request_stream(&req).await.expect("stream opens under the cap");
            handles.push(opened.handle);
        }
        assert_eq!(handles.len(), SMALL_CAP);

        let err = caps
            .request_stream(&req)
            .await
            .expect_err("one more open stream must be rejected once the cap is reached");
        assert!(err.contains("too many open http streams"), "got: {err}");

        // Close exactly one, freeing a slot — a fresh stream must now succeed again.
        let freed = handles.pop().expect("at least one handle to close");
        caps.close_stream(freed);
        caps.request_stream(&req).await.expect("a stream opens again once a slot is freed by closing");
    }

    /// A mock server that answers ONE connection with headers only and then holds the socket open
    /// forever, so a `poll_stream_chunk` against it is genuinely pending — with no data ever
    /// arriving, and therefore no timing dependence in the test that uses it.
    async fn spawn_quiet_mock() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                // No Content-Length and no body: the response never completes, so the client's body
                // stream stays open and quiet.
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n").await;
                let _ = sock.flush().await;
                // Park forever holding the socket — a oneshot whose sender is dropped here would
                // close it, so keep both halves alive in this frame.
                let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
                let _ = rx.await;
                drop(sock);
            }
        });
        format!("http://{addr}/probe")
    }

    /// A `poll_stream_chunk` future that is DROPPED before it completes must not wedge its handle.
    ///
    /// `Polling { closed: false }` is a latch: while installed, the handle answers `Ok(None)` to
    /// every other poll AND still occupies a [`MAX_OPEN_STREAMS`] accounting slot that only this
    /// poll's completion can free — and `close_stream` deliberately does not free it, it only sets
    /// `closed: true` and defers to the finalizer. Clearing that latch on the four paths that return
    /// normally therefore leaks the slot permanently the moment one of them is skipped (a panic
    /// unwinding out of the decoder, contained by the host so the process survives to keep the leak;
    /// or, the day a caller stops bridging this with `block_in_place`+`block_on`, an ordinary
    /// cancellation). The RAII `PollFinalizer` runs on every exit, so the handle lands terminal and
    /// `close_stream` can genuinely release it.
    ///
    /// Deterministic by construction: `now_or_never` polls the future exactly once and drops it, and
    /// the mock never sends a body byte, so there is no sleep and no scheduling race.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_poll_does_not_wedge_the_handle_or_leak_its_cap_slot() {
        let _serial = proxy_setting_guard().await;
        use futures::FutureExt;

        let caps = HttpCaps::with_max_open_streams(1);
        let req =
            HttpRequest { method: "GET".into(), url: spawn_quiet_mock().await, ..Default::default() };
        let handle = caps.request_stream(&req).await.expect("first stream opens under the cap").handle;

        // PRESENCE first: the cap really is 1 and really is held by this open stream, so the
        // release assertion below cannot pass vacuously.
        let second_url = spawn_persistent_mock().await;
        let second_req = HttpRequest { method: "GET".into(), url: second_url, ..Default::default() };
        assert!(
            caps.request_stream(&second_req)
                .await
                .expect_err("the cap of 1 is held by the open stream")
                .contains("too many open http streams")
        );

        // Poll once and drop the future mid-await: the chunk never arrives, so this is Pending.
        assert!(
            caps.poll_stream_chunk(handle).now_or_never().is_none(),
            "the quiet mock sends no body, so the poll must still be pending when it is dropped"
        );

        // The handle is terminal rather than stuck mid-poll...
        assert_eq!(
            caps.poll_stream_chunk(handle).await.expect("a known handle is not an error"),
            None,
            "a handle whose poll was dropped reads as EOF, not as an unknown handle"
        );
        // ...and closing it genuinely frees the accounting slot. Before the RAII finalizer this
        // `close_stream` hit a `Polling { closed: false }` entry, set `closed: true`, and waited for
        // a finalizer that would never run — so this last open stayed refused forever.
        caps.close_stream(handle);
        caps.request_stream(&second_req)
            .await
            .expect("closing the dropped-poll handle releases its MAX_OPEN_STREAMS slot");
    }

    /// Closes the shared-host-memory-exhaustion finding: a response that DECLARES (via
    /// `Content-Length`) a body bigger than [`MAX_RESPONSE_BODY_BYTES`] is rejected up front, before
    /// a single byte is read — the mock server never actually has to produce that many bytes for
    /// this to be observed, proving the cap is enforced off the header alone.
    #[tokio::test]
    async fn request_rejects_a_declared_content_length_over_the_cap() {
        let _serial = proxy_setting_guard().await;
        let headers = format!("Content-Length: {}\r\n", MAX_RESPONSE_BODY_BYTES as u64 + 1);
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![b"short".to_vec()]).await;
        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let err = caps.request(&req).await.expect_err("oversized Content-Length is rejected");
        assert!(err.contains("exceeds"), "got: {err}");
        assert!(err.contains("Content-Length"), "the early, header-only path reports why: {err}");
    }

    /// Same finding, the harder path: NO `Content-Length` header at all (real chunked/streamed
    /// responses often omit it), so the cap must still be enforced off the RUNNING total as chunks
    /// arrive — a real, over-the-cap body is actually streamed from the mock server here.
    #[tokio::test]
    async fn request_rejects_a_body_that_exceeds_the_cap_with_no_content_length_header() {
        let _serial = proxy_setting_guard().await;
        let oversized = vec![b'x'; MAX_RESPONSE_BODY_BYTES + 4096];
        let url = spawn_mock("HTTP/1.1 200 OK", String::new(), vec![oversized]).await;
        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let err = caps
            .request(&req)
            .await
            .expect_err("an over-cap body with no Content-Length is still rejected");
        assert!(err.contains("exceeds"), "got: {err}");
    }

    #[tokio::test]
    async fn a_transport_failure_is_an_err_not_a_panic() {
        let _serial = proxy_setting_guard().await;
        // Bind then immediately drop to get a refused-connection address (no server listening).
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);

        let caps = HttpCaps::new();
        let req =
            HttpRequest { method: "GET".into(), url: format!("http://{addr}/nope"), ..Default::default() };
        let err = caps.request(&req).await.expect_err("connection refused surfaces as Err");
        assert!(!err.is_empty());
    }

    /// L4 review §6: a stalled server (accepts the connection, never responds) must NOT hang
    /// `request` forever when the guest gave no `timeout_ms` — the fallback [`DEFAULT_REQUEST_TIMEOUT`]
    /// (overridden here to a real, fast test duration via [`HttpCaps::with_request_timeout`]) must
    /// fire on its own.
    #[tokio::test]
    async fn request_falls_back_to_a_bounded_timeout_when_the_guest_gives_none() {
        let _serial = proxy_setting_guard().await;
        let caps = HttpCaps::with_request_timeout(Duration::from_millis(100));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        // Accept the connection but never write a response — genuinely hangs from the client's
        // perspective, exactly the stalled-server scenario the fallback ceiling exists for.
        tokio::spawn(async move {
            if let Ok((_sock, _)) = listener.accept().await {
                std::future::pending::<()>().await;
            }
        });
        let req = HttpRequest {
            method: "GET".into(),
            url: format!("http://{addr}/probe"),
            ..Default::default()
        };

        let started = tokio::time::Instant::now();
        let err = caps.request(&req).await.expect_err("a stalled server with no timeout_ms still fails");
        let elapsed = started.elapsed();
        assert!(err.contains("timed out"), "the error must identify itself as a timeout: {err}");
        assert!(
            elapsed < Duration::from_secs(2),
            "the fallback timeout must fire — this must NEVER hang forever: got {elapsed:?}"
        );
    }

    /// L4 adversarial-verification fix: `timeout_ms: Some(0)` must behave exactly like `None` — the
    /// REAL, non-instant [`DEFAULT_REQUEST_TIMEOUT`] fallback ceiling (overridden here via
    /// [`HttpCaps::with_request_timeout`]) — not degrade to a literal 0ms `reqwest` timeout that fails
    /// on (essentially) the very first poll. Mirrors the sibling `> 0` guard `ui_roundtrip`
    /// (`cyrup-session-svc/src/host_services.rs:236`) and `exec` (`host_services.rs:423-427`) already
    /// apply to their own timeout fields. Same stalled-server setup as
    /// [`request_falls_back_to_a_bounded_timeout_when_the_guest_gives_none`], but with `timeout_ms:
    /// Some(0)` supplied explicitly — must take (approximately) the SAME fallback duration to fail,
    /// not near-zero.
    #[tokio::test]
    async fn request_timeout_ms_zero_falls_back_to_the_bounded_timeout_not_an_instant_one() {
        let _serial = proxy_setting_guard().await;
        let caps = HttpCaps::with_request_timeout(Duration::from_millis(150));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((_sock, _)) = listener.accept().await {
                std::future::pending::<()>().await;
            }
        });
        let req = HttpRequest {
            method: "GET".into(),
            url: format!("http://{addr}/probe"),
            timeout_ms: Some(0),
            ..Default::default()
        };

        let started = tokio::time::Instant::now();
        let err = caps
            .request(&req)
            .await
            .expect_err("a stalled server with timeout_ms:0 still fails eventually");
        let elapsed = started.elapsed();
        assert!(err.contains("timed out"), "the error must identify itself as a timeout: {err}");
        assert!(
            elapsed >= Duration::from_millis(100),
            "timeout_ms:0 must wait for the REAL fallback ceiling, not short-circuit to an instant \
             0ms timeout: got {elapsed:?}"
        );
    }

    /// Same finding, `request_stream`'s initial connect+headers phase.
    #[tokio::test]
    async fn request_stream_timeout_ms_zero_falls_back_to_the_bounded_timeout_not_an_instant_one() {
        let _serial = proxy_setting_guard().await;
        let caps = HttpCaps::with_request_timeout(Duration::from_millis(150));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((_sock, _)) = listener.accept().await {
                std::future::pending::<()>().await;
            }
        });
        let req = HttpRequest {
            method: "GET".into(),
            url: format!("http://{addr}/probe"),
            timeout_ms: Some(0),
            ..Default::default()
        };

        let started = tokio::time::Instant::now();
        let err = caps
            .request_stream(&req)
            .await
            .expect_err("a stalled server with timeout_ms:0 still fails eventually");
        let elapsed = started.elapsed();
        assert!(err.contains("timed out"), "the error must identify itself as a timeout: {err}");
        assert!(
            elapsed >= Duration::from_millis(100),
            "timeout_ms:0 must wait for the REAL fallback ceiling, not short-circuit to an instant \
             0ms timeout: got {elapsed:?}"
        );
    }

    /// Same finding, `request_stream`'s initial connect+headers phase.
    #[tokio::test]
    async fn request_stream_falls_back_to_a_bounded_timeout_when_the_guest_gives_none() {
        let _serial = proxy_setting_guard().await;
        let caps = HttpCaps::with_request_timeout(Duration::from_millis(100));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((_sock, _)) = listener.accept().await {
                std::future::pending::<()>().await;
            }
        });
        let req = HttpRequest {
            method: "GET".into(),
            url: format!("http://{addr}/probe"),
            ..Default::default()
        };

        let started = tokio::time::Instant::now();
        let err = caps
            .request_stream(&req)
            .await
            .expect_err("a stalled server with no timeout_ms still fails");
        let elapsed = started.elapsed();
        assert!(err.contains("timed out"), "the error must identify itself as a timeout: {err}");
        assert!(
            elapsed < Duration::from_secs(2),
            "the fallback timeout must fire — this must NEVER hang forever: got {elapsed:?}"
        );
    }

    /// A mock server that sends `first`, goes SILENT for `idle_for` (no data, no close — a real
    /// still-open connection), then sends `second` and closes. No `Content-Length`/chunked framing
    /// (`Connection: close`) so the client legitimately can't distinguish "more is coming" from
    /// "idle" until either more bytes or a close arrives — simulating a real long-lived
    /// SSE/StreamableHTTP connection that goes quiet between server-pushed messages (L4 review §6).
    async fn spawn_idle_then_chunk_mock(
        first: &'static [u8],
        idle_for: Duration,
        second: &'static [u8],
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n").await;
                let _ = sock.write_all(first).await;
                let _ = sock.flush().await;
                tokio::time::sleep(idle_for).await;
                let _ = sock.write_all(second).await;
                let _ = sock.flush().await;
                // Dropping `sock` here closes it, signaling end-of-body to the client.
            }
        });
        format!("http://{addr}/probe")
    }

    /// THE core claim of L4 review §6: an idle-timeout on `poll_stream_chunk` must NOT kill the
    /// stream — a legitimate long-lived connection that merely went quiet for a while must still be
    /// drainable on a later poll once the server actually sends something, exactly the real
    /// consumer's protocol need (MCP SDK's long-lived StreamableHTTP `GET`,
    /// `streamableHttp.js:75-105`) this capability exists to serve.
    #[tokio::test]
    async fn poll_stream_chunk_idle_timeout_does_not_kill_the_stream() {
        let _serial = proxy_setting_guard().await;
        let caps = HttpCaps::with_poll_idle_timeout(Duration::from_millis(80));
        let url = spawn_idle_then_chunk_mock(b"first", Duration::from_millis(400), b"second").await;
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps.request_stream(&req).await.expect("stream opens");

        let first =
            caps.poll_stream_chunk(resp.handle).await.expect("first chunk arrives immediately");
        assert_eq!(first, Some(b"first".to_vec()));

        // The server goes silent for 400ms; the idle timeout is 80ms, so this poll must return
        // QUICKLY — bounding the real OS thread `block_in_place` would otherwise pin — rather than
        // blocking for the server's full silence.
        let started = tokio::time::Instant::now();
        let err = caps.poll_stream_chunk(resp.handle).await.expect_err("an idle poll times out");
        let elapsed = started.elapsed();
        assert!(err.contains("no chunk within"), "the error must identify itself as an idle timeout, \
                 not a terminal failure: {err}");
        assert!(
            elapsed < Duration::from_millis(300),
            "the idle timeout must fire promptly, not block for the server's full silence: {elapsed:?}"
        );

        // The stream must have survived: it is NEITHER reported as closed NOR as EOF — a later poll
        // (once the server finally sends its second chunk) still succeeds with REAL data, proving
        // the connection was never torn down by the timeout.
        let mut got_second = None;
        for _ in 0..20 {
            match caps.poll_stream_chunk(resp.handle).await {
                Ok(Some(bytes)) => {
                    got_second = Some(bytes);
                    break;
                }
                Ok(None) => panic!("must not report EOF — the server has not closed yet"),
                Err(e) if e.contains("no chunk within") => continue, // still idle, retry
                Err(e) => panic!("unexpected terminal error after a mere idle timeout: {e}"),
            }
        }
        assert_eq!(
            got_second,
            Some(b"second".to_vec()),
            "the stream must still be live and draining after surviving an idle timeout"
        );
    }
}

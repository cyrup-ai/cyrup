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
    /// advertise the SAME `Accept-Encoding` reqwest's own toggles would have (`build_request`) and
    /// decompress manually via [`decode_buffered`]/[`decode_stream`], so the caller sees the
    /// decompressed body AND the untouched original headers.
    pub fn new() -> Self {
        let client = client_builder().build().unwrap_or_else(|_| reqwest::Client::new());
        Self::with_client(client)
    }

    /// Build around a caller-supplied client (tests, or a client the host already built).
    pub fn with_client(client: reqwest::Client) -> Self {
        Self {
            client,
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
    /// Sets `Accept-Encoding` when the caller didn't already supply one — the SAME value reqwest's
    /// own `gzip`+`brotli`+`deflate`+`zstd` toggles would have set automatically
    /// (`tower_http::compression_utils::AcceptEncoding::to_header_value`'s `(true,true,true,true)`
    /// arm: `"zstd,gzip,deflate,br"`), since [`Self::new`] disables that automatic behavior to keep
    /// the ORIGINAL response headers intact (see [`Self::new`]'s doc comment).
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
    fn build_request(&self, req: &HttpRequest, apply_reqwest_timeout: bool) -> Result<reqwest::RequestBuilder, String> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| format!("invalid HTTP method `{}`: {e}", req.method))?;
        let mut builder = self.client.request(method, req.url.as_str());
        let has_accept_encoding =
            req.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("accept-encoding"));
        if !has_accept_encoding {
            builder = builder.header(reqwest::header::ACCEPT_ENCODING, "zstd,gzip,deflate,br");
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
            let resp = self.build_request(req, true)?.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let headers = collect_headers(resp.headers());
            let encoding = content_encoding_of(resp.headers());
            let raw = read_bounded_body(resp).await?;
            let body = decode_buffered(encoding.as_deref(), raw).await?;
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
        let resp = tokio::time::timeout(effective_timeout, self.build_request(req, false)?.send())
            .await
            .map_err(|_| {
                format!("request_stream: timed out after {effective_timeout:?} waiting for the initial response")
            })?
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let headers = collect_headers(resp.headers());
        let encoding = content_encoding_of(resp.headers());
        let stream: ChunkStream = decode_stream(encoding.as_deref(), resp.bytes_stream());
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
        // never earlier (L4 round-12 finding #2a). `next` is the state to install when NOT closed;
        // ignored (removal wins) when closed.
        let finalize = |next: StreamSlot| {
            if let Ok(mut g) = self.streams.lock() {
                match g.get_mut(&handle) {
                    Some(StreamSlot::Polling { closed: true }) => {
                        g.remove(&handle);
                    }
                    Some(slot @ StreamSlot::Polling { closed: false }) => {
                        *slot = next;
                    }
                    // Defensive only — the state machine above never lets `handle` be in any other
                    // shape (or vanish) while a poll owns it; a silent no-op is the no-panic-safe
                    // fallback if it somehow did.
                    _ => {}
                }
            }
        };
        // L4 review §6: bound THIS SINGLE poll's wait, never the stream's overall lifetime — a
        // legitimate long-lived SSE/StreamableHTTP connection (the real consumer's actual protocol
        // need, MCP SDK `streamableHttp.js:75-105`) can go quiet between server-pushed messages for a
        // while; see [`HTTP_POLL_IDLE_TIMEOUT`]'s doc for why this must not fire eagerly. On timeout
        // the stream is put straight BACK to `Idle` (never marked EOF/terminal) so a guest that simply
        // polls again keeps draining the SAME still-open connection.
        let poll_idle_timeout = self.poll_idle_timeout;
        match tokio::time::timeout(poll_idle_timeout, stream.next()).await {
            Err(_) => {
                finalize(StreamSlot::Idle(stream));
                Err(format!(
                    "poll_stream_chunk: no chunk within {poll_idle_timeout:?} — the connection \
                     may still be open, poll again"
                ))
            }
            Ok(Some(Ok(bytes))) => {
                // The chunk we already fetched is returned regardless of a racing close — it was real
                // data read off the wire before the close happened, independent of the registry's
                // bookkeeping (`finalize` above still honors the close: removes rather than
                // reinstates `Idle`).
                finalize(StreamSlot::Idle(stream));
                Ok(Some(bytes.to_vec()))
            }
            Ok(Some(Err(e))) => {
                finalize(StreamSlot::Eof); // terminal: subsequent polls degrade to EOF
                Err(e.to_string())
            }
            Ok(None) => {
                finalize(StreamSlot::Eof); // natural EOF
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
    /// `closed`; the in-flight poll's own completion (`poll_stream_chunk`'s `finalize`) performs the
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
    reqwest::Client::builder().no_gzip().no_brotli().no_deflate().no_zstd()
}

/// The response's `Content-Encoding` value, if any (lowercased is NOT applied — matches
/// `tower_http`'s own byte-exact, case-SENSITIVE match against `gzip`/`deflate`/`br`/`zstd`; an
/// unrecognized casing/value is treated as `identity`, exactly like the real decompression layer
/// this replaces).
fn content_encoding_of(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers.get(reqwest::header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()).map(str::to_owned)
}

/// Decompress a fully-buffered body per `encoding` (the real consumer's `fetch()` semantics — an
/// unrecognized/absent encoding passes the bytes through unchanged, matching `identity`). Bounds the
/// DECOMPRESSED output at [`MAX_RESPONSE_BODY_BYTES`], the SAME cap [`read_bounded_body`] already
/// applies to the wire (possibly-compressed) transfer — now that decompression happens manually
/// here rather than inside `reqwest`, a small compressed body must not be able to expand into an
/// unbounded allocation (a decompression bomb).
async fn decode_buffered(encoding: Option<&str>, raw: Vec<u8>) -> Result<Vec<u8>, String> {
    let Some(encoding) = encoding else { return Ok(raw) };
    let reader = tokio::io::BufReader::new(std::io::Cursor::new(raw));
    match encoding {
        "gzip" => read_capped(async_compression::tokio::bufread::GzipDecoder::new(reader)).await,
        "br" => read_capped(async_compression::tokio::bufread::BrotliDecoder::new(reader)).await,
        "deflate" => {
            read_capped(async_compression::tokio::bufread::DeflateDecoder::new(reader)).await
        }
        "zstd" => read_capped(async_compression::tokio::bufread::ZstdDecoder::new(reader)).await,
        // Unrecognized Content-Encoding ⇒ identity, matching the real decompression layer this
        // replaces (`tower_http`'s `_ => identity` fallback). `reader` (holding `raw`) is simply
        // dropped here; recover the original bytes from its inner cursor.
        _ => Ok(reader.into_inner().into_inner()),
    }
}

/// Read `r` to EOF, capped at [`MAX_RESPONSE_BODY_BYTES`] of DECOMPRESSED output — rejects rather
/// than growing past the cap, mirroring [`read_bounded_body`]'s running-total check.
async fn read_capped<R: tokio::io::AsyncRead + Unpin>(mut r: R) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = r.read(&mut buf).await.map_err(|e| format!("decompression failed: {e}"))?;
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

/// Wrap a raw response byte stream through the `async-compression` decoder matching `encoding` (the
/// real consumer's `fetch()` semantics — an unrecognized/absent encoding passes chunks through
/// unchanged, matching `identity`), used by [`HttpCaps::request_stream`]. Unlike the buffered path,
/// there is no additional cap here: the host never accumulates a streaming body (the guest drains it
/// one already-network-bounded chunk at a time via [`HttpCaps::poll_stream_chunk`] — the same
/// reasoning [`MAX_RESPONSE_BODY_BYTES`]'s doc comment already gives for why streaming doesn't need
/// it), and a decoder that never learns the true end of a bomb-sized declared stream is nothing new:
/// an identity stream of the same declared size is already unbounded in the exact same way.
fn decode_stream(
    encoding: Option<&str>,
    raw: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
) -> ChunkStream {
    // `reqwest::Error` doesn't satisfy `StreamReader`'s `Into<std::io::Error>` bound directly —
    // map it explicitly once, up front, so every decoder branch below shares one `io::Error`-typed
    // stream.
    let raw = raw.map(|r| r.map_err(std::io::Error::other));
    match encoding {
        Some("gzip") => {
            let reader = tokio::io::BufReader::new(tokio_util::io::StreamReader::new(raw));
            let decoder = async_compression::tokio::bufread::GzipDecoder::new(reader);
            Box::pin(tokio_util::io::ReaderStream::new(decoder))
        }
        Some("br") => {
            let reader = tokio::io::BufReader::new(tokio_util::io::StreamReader::new(raw));
            let decoder = async_compression::tokio::bufread::BrotliDecoder::new(reader);
            Box::pin(tokio_util::io::ReaderStream::new(decoder))
        }
        Some("deflate") => {
            let reader = tokio::io::BufReader::new(tokio_util::io::StreamReader::new(raw));
            let decoder = async_compression::tokio::bufread::DeflateDecoder::new(reader);
            Box::pin(tokio_util::io::ReaderStream::new(decoder))
        }
        Some("zstd") => {
            let reader = tokio::io::BufReader::new(tokio_util::io::StreamReader::new(raw));
            let decoder = async_compression::tokio::bufread::ZstdDecoder::new(reader);
            Box::pin(tokio_util::io::ReaderStream::new(decoder))
        }
        _ => Box::pin(raw),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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
        let body = b"hello from the mock server".to_vec();
        let headers = format!("Content-Type: text/plain\r\nContent-Length: {}\r\n", body.len());
        let url = spawn_mock("HTTP/1.1 200 OK", headers, vec![body.clone()]).await;

        let caps = HttpCaps::new();
        let req = HttpRequest { method: "GET".into(), url, ..Default::default() };
        let resp = caps.request(&req).await.expect("request succeeds");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, body);
    }

    /// Pipe `input` through a real system compressor binary (`gzip -c` / `zstd -c`), no canned
    /// bytes.
    fn compress_with(binary: &str, args: &[&str], input: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut child = std::process::Command::new(binary)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn the system `{binary}` binary: {e}"));
        child
            .stdin
            .take()
            .expect("child stdin")
            .write_all(input)
            .expect("write plaintext to the compressor");
        let out = child.wait_with_output().expect("compressor runs");
        assert!(out.status.success(), "`{binary}` must succeed");
        out.stdout
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
        let plaintext = b"hello decompression world, repeated for a real ratio: \
            hello decompression world, hello decompression world"
            .to_vec();
        let gzipped = compress_with("gzip", &["-c"], &plaintext);
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

    /// Same finding, the streaming path (`request_stream`/`poll_stream_chunk`): a REAL zstd-
    /// compressed body (via the system `zstd` binary) must decompress the DRAINED chunks while the
    /// initiating response's headers (captured before any chunk is polled) still carry the real
    /// `Content-Encoding: zstd` + original compressed `Content-Length`.
    #[tokio::test]
    async fn request_stream_transparently_decodes_a_real_zstd_content_encoding() {
        let plaintext = b"streaming zstd decompression world, repeated for a real ratio: \
            streaming zstd decompression world, streaming zstd decompression world"
            .to_vec();
        let compressed = compress_with("zstd", &["-c"], &plaintext);
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

    /// A small compressed body that decompresses to something far larger than
    /// [`MAX_RESPONSE_BODY_BYTES`] (a decompression bomb) must still be rejected — now that
    /// decompression happens manually (`decode_buffered`), this cap is no longer `reqwest`'s
    /// responsibility, so it must be reasserted independently of the wire-size cap
    /// (`request_rejects_a_declared_content_length_over_the_cap` already covers the wire side).
    #[tokio::test]
    async fn request_rejects_a_decompression_bomb_over_the_cap() {
        // Highly compressible: one repeated byte, decompressed size deliberately over the cap.
        let huge = vec![b'x'; MAX_RESPONSE_BODY_BYTES + 4096];
        let gzipped = compress_with("gzip", &["-9", "-c"], &huge);
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

    /// Closes the shared-host-memory-exhaustion finding: a response that DECLARES (via
    /// `Content-Length`) a body bigger than [`MAX_RESPONSE_BODY_BYTES`] is rejected up front, before
    /// a single byte is read — the mock server never actually has to produce that many bytes for
    /// this to be observed, proving the cap is enforced off the header alone.
    #[tokio::test]
    async fn request_rejects_a_declared_content_length_over_the_cap() {
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

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

use bytes::Bytes;
use futures::{Stream, StreamExt};

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

/// A live streaming response body, boxed for storage in the registry.
type ChunkStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

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

/// The real HTTP capability engine: one shared `reqwest::Client` plus a registry of open streaming
/// bodies keyed by an opaque `u32` handle (`request-stream` / `poll-stream-chunk` / `close-stream`).
/// A registry entry of `None` means the underlying stream already reached natural EOF (or errored)
/// but the handle stays registered — repeat polls keep returning `Ok(None)` until the guest calls
/// `close-stream`, matching Pi's SSE/StreamableHTTP "done" semantics for the eventual MCP transport
/// consumer.
pub struct HttpCaps {
    client: reqwest::Client,
    streams: Mutex<HashMap<u32, Option<ChunkStream>>>,
    next_handle: AtomicU32,
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
    pub fn new() -> Self {
        let client = reqwest::Client::builder().build().unwrap_or_else(|_| reqwest::Client::new());
        Self::with_client(client)
    }

    /// Build around a caller-supplied client (tests, or a client the host already built).
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client, streams: Mutex::new(HashMap::new()), next_handle: AtomicU32::new(1) }
    }

    /// Shared request-building: method parse + headers + optional body + optional timeout.
    fn build_request(&self, req: &HttpRequest) -> Result<reqwest::RequestBuilder, String> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| format!("invalid HTTP method `{}`: {e}", req.method))?;
        let mut builder = self.client.request(method, req.url.as_str());
        for (name, value) in &req.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }
        if let Some(ms) = req.timeout_ms {
            builder = builder.timeout(std::time::Duration::from_millis(u64::from(ms)));
        }
        Ok(builder)
    }

    /// A bounded request/response round trip (the WIT `request`): the whole body is buffered and
    /// returned. A non-2xx status is NOT itself an `Err` (fetch/Pi semantics — only a transport
    /// failure is); the caller inspects `status` itself.
    pub async fn request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        let resp = self.build_request(req)?.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let headers = collect_headers(resp.headers());
        let body = read_bounded_body(resp).await?;
        Ok(HttpResponse { status, headers, body })
    }

    /// Start a streaming request (the WIT `request-stream`): opens the connection, captures the
    /// initiating response's status+headers (returned alongside the handle, closes L4 §2.3), then
    /// stores the decoded byte stream keyed by a fresh handle for [`Self::poll_stream_chunk`] to
    /// drain. Like [`Self::request`], a non-2xx status is not itself an error (fetch semantics) — the
    /// caller inspects [`HttpStreamResponse::status`] itself; the body is stored regardless of status.
    pub async fn request_stream(&self, req: &HttpRequest) -> Result<HttpStreamResponse, String> {
        let resp = self.build_request(req)?.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let headers = collect_headers(resp.headers());
        let stream: ChunkStream = Box::pin(resp.bytes_stream());
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        let mut g =
            self.streams.lock().map_err(|_| "http stream registry lock poisoned".to_string())?;
        g.insert(handle, Some(stream));
        Ok(HttpStreamResponse { handle, status, headers })
    }

    /// Drain the next chunk of an open stream (the WIT `poll-stream-chunk`); `Ok(None)` = EOF. A
    /// stream that already hit EOF (or errored) keeps returning `Ok(None)` on repeat polls; only an
    /// unknown/already-closed handle is an `Err`.
    pub async fn poll_stream_chunk(&self, handle: u32) -> Result<Option<Vec<u8>>, String> {
        // Take the live stream out of the registry while we `.await` it (a `MutexGuard` cannot be
        // held across an await point — the compiler enforces this since the guard isn't `Send`).
        let slot = {
            let mut g = self
                .streams
                .lock()
                .map_err(|_| "http stream registry lock poisoned".to_string())?;
            match g.get_mut(&handle) {
                Some(slot) => slot.take(),
                None => return Err(format!("no open http stream for handle {handle}")),
            }
        };
        let Some(mut stream) = slot else {
            // Already EOF'd/errored on a prior poll; the handle stays open (only `close-stream`
            // removes it) but every further poll degrades to EOF, never a re-raised error.
            return Ok(None);
        };
        match stream.next().await {
            Some(Ok(bytes)) => {
                if let Ok(mut g) = self.streams.lock() {
                    g.insert(handle, Some(stream));
                }
                Ok(Some(bytes.to_vec()))
            }
            Some(Err(e)) => {
                if let Ok(mut g) = self.streams.lock() {
                    g.insert(handle, None); // terminal: subsequent polls degrade to EOF
                }
                Err(e.to_string())
            }
            None => {
                if let Ok(mut g) = self.streams.lock() {
                    g.insert(handle, None); // natural EOF
                }
                Ok(None)
            }
        }
    }

    /// Close (drop/cancel) a stream (the WIT `close-stream`). Dropping the underlying
    /// `reqwest::Response` body cancels the in-flight download. Unconditionally removes the handle
    /// (never an error — mirrors the WIT signature's `()` return); closing an unknown handle is a
    /// silent no-op.
    pub fn close_stream(&self, handle: u32) {
        if let Ok(mut g) = self.streams.lock() {
            g.remove(&handle);
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
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
}

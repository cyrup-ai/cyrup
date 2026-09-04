//! The `http-client` WIT import: a bounded request/response round trip, plus the streaming variant
//! whose live body the HOST owns and the guest drains through an opaque handle (arch-08 §5.2's
//! request/poll bridge — a guest cannot hold a Rust stream across the wasm boundary).

use super::Ctx;

impl Ctx {
    /// A bounded outbound HTTP request/response round trip (the `http-client.request` capability
    /// grant; arch-08 §3.2 draft, pi-mcp-adapter-port.md §3.2). Gated by the SAME trust check as
    /// [`Ctx::exec`] — denied unless the host granted the http-client capability. A non-2xx status is
    /// NOT itself an `Err` (fetch semantics); inspect [`HttpResponse::status`].
    pub fn http_request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let wit = req.to_wit();
            return crate::guest::bindings::cyrup::ext::http_client::request(&wit)
                .map(HttpResponse::from_wit);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = req;
            Err("http-client unavailable on host target".into())
        }
    }

    /// Start a streaming outbound HTTP request (the `http-client.request-stream` capability grant);
    /// returns the initiating response's status+headers TOGETHER with an opaque stream handle (the
    /// guest drains the body via [`Ctx::http_poll_stream_chunk`]) — the HOST owns the live Rust stream
    /// (a guest cannot hold one across the wasm boundary, arch-08 §5.2's request/poll bridge).
    /// Status/headers arrive off the SAME round trip that opens the body (closes L4 §2.3): the real
    /// consumer this backs, the MCP TS SDK's `StreamableHTTPClientTransport`/`SSEClientTransport`,
    /// reads `response.status` (401 => re-auth) and `response.headers` (`mcp-session-id`,
    /// `content-type`) off the SAME response whose body it then streams. Gated the same way as
    /// [`Ctx::http_request`].
    pub fn http_request_stream(&self, req: &HttpRequest) -> Result<HttpStreamResponse, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let wit = req.to_wit();
            return crate::guest::bindings::cyrup::ext::http_client::request_stream(&wit)
                .map(HttpStreamResponse::from_wit);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = req;
            Err("http-client unavailable on host target".into())
        }
    }

    /// Drain the next chunk of a stream opened via [`Ctx::http_request_stream`] (the
    /// `http-client.poll-stream-chunk` import); `Ok(None)` = EOF.
    pub fn http_poll_stream_chunk(&self, handle: u32) -> Result<Option<Vec<u8>>, String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::http_client::poll_stream_chunk(handle);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = handle;
            Err("http-client unavailable on host target".into())
        }
    }

    /// Close (drop/cancel) a stream opened via [`Ctx::http_request_stream`] (the
    /// `http-client.close-stream` import).
    pub fn http_close_stream(&self, handle: u32) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::http_client::close_stream(handle);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = handle;
    }
}

/// An outbound HTTP request (`Ctx::http_request`/`http_request_stream`; mirrors the WIT
/// `http-request` record 1:1, arch-08 §3.2 draft, pi-mcp-adapter-port.md §3.2).
#[derive(Clone, Debug, Default)]
pub struct HttpRequest {
    /// The HTTP method — `"GET" | "POST" | ...` (WIT `http-request.method`, `wit/world.wit:850`).
    /// Set by [`Self::get`] or [`Self::new`].
    pub method: String,
    /// The absolute request URL.
    pub url: String,
    /// The request headers, in the order [`Self::header`] appended them.
    pub headers: Vec<(String, String)>,
    /// The raw request body, or `None` for a bodyless request. Set by [`Self::body`].
    pub body: Option<Vec<u8>>,
    /// A per-request timeout in milliseconds, or `None` to leave the bound to the host. Set by
    /// [`Self::timeout_ms`].
    pub timeout_ms: Option<u32>,
}

impl HttpRequest {
    /// A bare `GET` to `url`.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET".into(),
            url: url.into(),
            ..Default::default()
        }
    }
    /// A `method` request to `url`.
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            ..Default::default()
        }
    }
    /// Append a request header (builder-style).
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    /// Set the request body (builder-style).
    #[must_use]
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }
    /// Set a request timeout in milliseconds (builder-style).
    #[must_use]
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = Some(ms);
        self
    }

    #[cfg(target_arch = "wasm32")]
    fn to_wit(&self) -> crate::guest::bindings::cyrup::ext::http_client::HttpRequest {
        crate::guest::bindings::cyrup::ext::http_client::HttpRequest {
            method: self.method.clone(),
            url: self.url.clone(),
            headers: self.headers.clone(),
            body: self.body.clone(),
            timeout_ms: self.timeout_ms,
        }
    }
}

/// The response to an [`HttpRequest`] (mirrors the WIT `http-response` record 1:1).
#[derive(Clone, Debug, Default)]
pub struct HttpResponse {
    /// The HTTP status code. A non-2xx is delivered here rather than as an `Err` — see
    /// [`Ctx::http_request`].
    pub status: u16,
    /// The response headers.
    pub headers: Vec<(String, String)>,
    /// The fully-read response body (WIT `http-response.body`, `wit/world.wit:859`) — the bounded
    /// counterpart to [`HttpStreamResponse`], which carries no body at all.
    pub body: Vec<u8>,
}

impl HttpResponse {
    #[cfg(target_arch = "wasm32")]
    fn from_wit(wit: crate::guest::bindings::cyrup::ext::http_client::HttpResponse) -> Self {
        Self {
            status: wit.status,
            headers: wit.headers,
            body: wit.body,
        }
    }
}

/// The initiating response's metadata for a stream opened via [`Ctx::http_request_stream`] (mirrors
/// the WIT `http-stream-response` record 1:1): status+headers arrive TOGETHER with the stream handle,
/// off the SAME round trip that opens the long-lived body, so callers can inspect
/// [`Self::status`]/[`Self::headers`] (e.g. 401 => re-auth, `mcp-session-id`) before or independent of
/// draining the body via [`Ctx::http_poll_stream_chunk`].
#[derive(Clone, Debug, Default)]
pub struct HttpStreamResponse {
    /// The opaque stream handle to pass to [`Ctx::http_poll_stream_chunk`] and
    /// [`Ctx::http_close_stream`] (WIT `http-stream-response.handle`, `wit/world.wit:869`).
    pub handle: u32,
    /// The initiating response's status code — readable before a single body byte is drained.
    pub status: u16,
    /// The initiating response's headers — likewise readable before draining the body.
    pub headers: Vec<(String, String)>,
}

impl HttpStreamResponse {
    #[cfg(target_arch = "wasm32")]
    fn from_wit(wit: crate::guest::bindings::cyrup::ext::http_client::HttpStreamResponse) -> Self {
        Self {
            handle: wit.handle,
            status: wit.status,
            headers: wit.headers,
        }
    }
}

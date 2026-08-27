---
stage: qa
status: completed
updated: 2026-08-27 08:00
---

# MCP-115 / F5: a 401 with a JSON-RPC body never reaches the OAuth ladder

## COMPLETED — QA 10/10

MCP-115 landed in `runtime.rs` alone; `request_headers_command.rs` needed no edit because
its client is already generic over the transport.

rmcp shortcuts any non-success status carrying a JSON-RPC error body straight to the caller,
where upstream confines that to 400 alone — so a 401 with a body bypassed the OAuth ladder
entirely. `UnauthorizedProbe` intercepts the handshake POST. The predicate covers
`DiscoverRequest` as well as `InitializeRequest`; restricting it to Initialize would have
left the bug live for `protocolVersion` "auto" and "2026-07-28". The 400 passthrough is
deliberately untouched, since Discover renegotiates off a 400 UNSUPPORTED_PROTOCOL_VERSION.

The regression test was ablated rather than assumed: with the predicate forced false it fails
out of arm 7 with the exact predicted error while the existing bare-401 test still passes.
Ablation reverted and grep-verified.

Gates: check/clippy/doc clean, 7870/7870 tests, cyrup-mcp 612 to 613.

---

## Objective

Make `ConnectionBuilder::connect_http_client` classify **every** HTTP 401 on a **handshake** POST as
unauthorized — including the one that carries `Content-Type: application/json` and a parseable
JSON-RPC error body — so the OAuth ladder runs instead of the connect dying at arm 7.

Recorded as still-open at
[13-cyrup-mcp-STATUS.md:251-274](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md), deliberately left
unfixed in wave 5 because it is a second, distinct mechanism from the one F5 addressed. The unit is
MCP-115, specified at [13c-mcp-servers.md:1316-1334](../../docs/gap-analysis/13c-mcp-servers.md) —
**not** 13g/13i.

Fails **safe** today — a hard connect error, never a wrongly-authenticated request — so this is
correctness, not a security hole.

> **Line numbers below were re-verified on 2026-08-27** against
> `crates/cyrup-mcp/src/runtime.rs` (4968 lines), `crates/cyrup-mcp/src/request_headers_command.rs`
> (1338 lines) and `rmcp-3.1.4`. The previous revision of this file was written against a
> `runtime.rs` that has since drifted by ~7 lines; every reference here is current.

---

## 1 · The exact shape that breaks

rmcp's reqwest client claims a 401 **that carries `WWW-Authenticate`** at
`…/rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:212-226`, **before** it ever
reads the body — that arm returns `Err(StreamableHttpError::AuthRequired(..))` and works today. So a
401 with a challenge header **and** a JSON body already reaches the ladder.

The broken shape is narrower than the STATUS note implies, and all four conditions are required:

1. status `401`, **and**
2. **no** `WWW-Authenticate` header (otherwise `:212-226` claims it), **and**
3. `Content-Type: application/json`, **and**
4. a body that deserialises as `ServerJsonRpcMessage::Error` — `parse_json_rpc_error`, same file
   `:42-47`, returns `None` for every other variant and the response then falls through to the
   error path.

Then `:278-299` runs:

```rust
// rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:278-299
if !status.is_success() {
    let body = response.text().await.unwrap_or_else(|_| "<failed to read response body>".to_owned());
    if content_type.as_deref().is_some_and(|ct| ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes())) {
        match parse_json_rpc_error(&body) {
            Some(message) => {
                return Ok(StreamableHttpPostResponse::Json(message, session_id)); // :289 — Ok, not Err
            }
            None => tracing::warn!("HTTP {status}: could not parse JSON body as a JSON-RPC error"),
        }
    }
    return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
        format!("HTTP {status}: {body}"),                                        // :296-298
    )));
}
```

`:289` returns **`Ok`**. The `Err(UnexpectedServerResponse("HTTP 401 …"))` at `:296` — the string
`UNEXPECTED_UNAUTHORIZED_PREFIX` ([runtime.rs:2037](../../crates/cyrup-mcp/src/runtime.rs))
prefix-matches at [runtime.rs:2070](../../crates/cyrup-mcp/src/runtime.rs) — is never constructed.
`bare_unauthorized` ([runtime.rs:2058](../../crates/cyrup-mcp/src/runtime.rs)) is therefore never
reached on this path: it is not wrong, it is not called.

**Traced to where the connect actually dies.** The `Ok(Json(Error(..)))` reaches the transport
worker at `…/rmcp-3.1.4/src/transport/streamable_http_client.rs:865-891`. `expect_initialized`
(`:256-292` of the same file) has **no error arm** — `Self::Json(message, session_id) => Ok((message,
session_id))` — so the JSON-RPC *error* is handed back as if it were the init result, and `:924`
forwards it to the handler. `expect_response` (`…/rmcp-3.1.4/src/service/client.rs:168-205`) then
turns it into `ClientInitializeError::JsonRpcError(ErrorData)` (`:194` when the body echoes the
request id, `:199` when the body omits it; a *mismatched* id gives `UncorrelatedErrorResponse` at
`:200-203`). `unauthorized_challenge` ([runtime.rs:2000](../../crates/cyrup-mcp/src/runtime.rs))
matches only `TransportError` and `LegacyFallbackFailed`
([runtime.rs:2010-2018](../../crates/cyrup-mcp/src/runtime.rs)) and returns `None` for every other
variant, so the ladder at [runtime.rs:2672](../../crates/cyrup-mcp/src/runtime.rs) takes the `else`
branch and the connect dies at arm 7 ([runtime.rs:2674](../../crates/cyrup-mcp/src/runtime.rs))
rendering `#[error("JSON-RPC error: {0}")]` (`service/client.rs:69-70`). No `needs-auth`, no
`/mcp-auth`, no OAuth ladder — ever, for that server.

### Upstream is unambiguous, and it is the strongest evidence we have

`isUnauthorizedHttpError` is status-only
([server-manager.ts:73-75](../../tmp/pi-mcp-adapter/server-manager.ts)), and the pinned SDK never
lets a 401 become a JSON-RPC error. In
`tmp/pi-mcp-adapter/node_modules/@modelcontextprotocol/client/dist/index.mjs`:

* `:5333-5334` — `if (!response.ok) { if (response.status === 401 && this._authProvider) {` — the
  **status** is read first and 401 wins. `:5335`'s `response.headers.has("www-authenticate")` only
  enriches `_resourceMetadataUrl`/`_scope`; it does **not** gate the classification. With no
  provider the 401 falls to `:5382` and becomes `SdkHttpError(..., { status: 401 })`. Either way
  `isUnauthorizedHttpError` is `true`.
* `:5374-5381` — the JSON-RPC-error-body shortcut exists upstream too, but it is gated on
  **`response.status === 400`** and on `_isModernEnvelopedRequest(message)`.

**That is the defect in one sentence: rmcp widened upstream's 400-only JSON-RPC-error shortcut to
every non-success status, and 401 got swept in.** We cannot narrow rmcp's arm, so we take the
handshake POST.

---

## 2 · Why no existing seam can see it

**The status exists in exactly one place: a function local inside rmcp.**
`let status = response.status();` at `…/reqwest/streamable_http_client.rs:243`, inside
`impl StreamableHttpClient for reqwest::Client` (`:49`). It is consumed at `:244`, `:250`, `:265`,
`:278`, formatted into a string at `:297`, and dies with the stack frame.

**Neither existing decorator can ever see it**, because both sit *above* rmcp's `reqwest::Client`
impl and delegate the send to it:

* `SessionIdProbe::post_message` — [runtime.rs:1017-1022](../../crates/cyrup-mcp/src/runtime.rs)
  `self.inner.post_message(…).await?`, then `record(&response)`
  ([runtime.rs:985-1000](../../crates/cyrup-mcp/src/runtime.rs)), which reads only the
  `Option<String>` session id off the returned `StreamableHttpPostResponse`.
* `RequestHeadersCommandClient::post_message` —
  [request_headers_command.rs:944-946](../../crates/cyrup-mcp/src/request_headers_command.rs)
  `self.inner.post_message(…).await`, returned verbatim.

What reaches them is `StreamableHttpPostResponse`
(`…/rmcp-3.1.4/src/transport/streamable_http_client.rs:239-244`), whose three variants carry a
`ServerJsonRpcMessage`, a `BoxedSseStream`, and an `Option<String>` session id. **No status, no
headers.** A 401-with-JSON-RPC-body and a 200-with-JSON-RPC-body are byte-identical at that seam, so
no decorator above the send can distinguish them without guessing at the JSON-RPC error code — which
would turn a legitimate handshake rejection into a `needs-auth`. rmcp's own `Discover` flow *depends*
on JSON-RPC errors arriving intact (`service/client.rs:980-981` retries on
`ErrorCode::UNSUPPORTED_PROTOCOL_VERSION`), so code-based classification is not merely un-upstream,
it is actively wrong.

**Consequence: the seam moves to the bottom of the chain and owns the POST.** The chain built at
[runtime.rs:2796-2827](../../crates/cyrup-mcp/src/runtime.rs) is
`SessionIdProbe<[RequestHeadersCommandClient<]reqwest::Client[>]>`; the fix replaces the innermost
`reqwest::Client` with a client this crate owns.

Three facts that bound the work:

* **Only the handshake POSTs matter.** `unauthorized_challenge` has exactly one caller — the ladder
  at [runtime.rs:2672](../../crates/cyrup-mcp/src/runtime.rs) — and it takes a
  `ClientInitializeError`. Verified by grep: `unauthorized_challenge` and `bare_unauthorized` have
  **no** consumers outside `runtime.rs`.
* **"Handshake" is two request kinds, not one.** In `ClientLifecycleMode::Initialize` the startup
  request is `ClientRequest::InitializeRequest`; in `Discover`/`Auto` it is
  `ClientRequest::DiscoverRequest` (`…/service/client.rs:943-954`, whose failure becomes
  `ClientInitializeError::transport::<T>(error, "send discover request")` at `:953`). Both are
  reachable from this crate — [`version_negotiation`](../../crates/cyrup-mcp/src/runtime.rs) maps
  `protocolVersion: "auto"` → `Auto` and `"2026-07-28"` → `Discover`. **A predicate that matches only
  `InitializeRequest` leaves the bug live for those two configurations.**
* **The GET/DELETE legs stay delegated.** rmcp's `get_stream` returns `AuthRequired` for a 401 with a
  header (`…/reqwest/streamable_http_client.rs:97-111`) and, for a bare 401, `error_for_status()` at
  `:128` yields `StreamableHttpError::Client(reqwest::Error)`. Neither reaches
  `unauthorized_challenge` regardless: the GET stream is opened from a detached `JoinSet`
  (`…/streamable_http_client.rs:685-712`), long after the handshake settled.

---

## 3 · Do the fixture first

The fixture's inability to produce this shape is exactly why the gap went unseen. Without it there
is nothing to point the fix at.

`HttpFixture` / `FixtureOptions` live in the `mod tests` of runtime.rs:
[struct at :3883-3887](../../crates/cyrup-mcp/src/runtime.rs),
[`FixtureOptions` at :3891-3905](../../crates/cyrup-mcp/src/runtime.rs),
[`Default` at :3907-3917](../../crates/cyrup-mcp/src/runtime.rs),
[destructuring at :3946-3952](../../crates/cyrup-mcp/src/runtime.rs),
[the 401 arm at :3997-4006](../../crates/cyrup-mcp/src/runtime.rs).

**One new field**, inserted after `challenge` at
[:3897](../../crates/cyrup-mcp/src/runtime.rs):

```rust
        /// Answer those 401s with `Content-Type: application/json` and a parseable JSON-RPC error
        /// body instead of `Content-Length: 0`. With `challenge: false` this is the shape rmcp
        /// collapses into `Ok(StreamableHttpPostResponse::Json(..))`
        /// (`reqwest/streamable_http_client.rs:287-290`), so no transport error is constructed and
        /// nothing `unauthorized_challenge` walks can see it. With `challenge: true` rmcp's
        /// `:212-226` arm claims it first and the ladder is reached today — which is what makes this
        /// field an ablation rather than a restatement.
        json_rpc_body: bool,
```

Add `json_rpc_body: false` to `Default` ([:3907-3917](../../crates/cyrup-mcp/src/runtime.rs)) so
every existing call site reads the same, and add `json_rpc_body,` to the
`let FixtureOptions { … } = options;` destructuring at
[:3946-3952](../../crates/cyrup-mcp/src/runtime.rs).

**Replace the body of the 401 arm** at [:3997-4006](../../crates/cyrup-mcp/src/runtime.rs):

```rust
                    } else if is_initialize && initializes <= unauthorized_initializes {
                        let challenge_header = if challenge {
                            "WWW-Authenticate: Bearer realm=\"mcp\", resource_metadata=\"https://example.invalid/.well-known\"\r\n"
                        } else {
                            // The BARE 401. rmcp builds `AuthRequiredError` only when the header is
                            // present, so this is the shape that used to fall out as a hard connect
                            // error instead of reaching the OAuth ladder.
                            ""
                        };
                        if json_rpc_body {
                            // The id is ECHOED, so rmcp's `expect_response`
                            // (`service/client.rs:191-204`) takes the `JsonRpcError` arm at `:194`
                            // rather than `UncorrelatedErrorResponse` at `:200`. Both fail today,
                            // but only the echoing shape is what a real server produces, and the
                            // ablation has to fail for the right reason.
                            let id = json_id(&recorded.body);
                            let payload = format!(
                                "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32001,\"message\":\"Unauthorized\"}}}}"
                            );
                            format!(
                                "HTTP/1.1 401 Unauthorized\r\n{challenge_header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                                payload.len()
                            )
                        } else {
                            format!("HTTP/1.1 401 Unauthorized\r\n{challenge_header}Content-Length: 0\r\nConnection: close\r\n\r\n")
                        }
                    }
```

Requirements on that body, each load-bearing and each verified:

* `Content-Type: application/json` — rmcp gates the shortcut on it
  (`reqwest/streamable_http_client.rs:283-286`).
* The body must deserialise as `ServerJsonRpcMessage::Error`. `JsonRpcMessage` is `#[serde(untagged)]`
  with variants `Request | Response | Notification | Error` (`rmcp-3.1.4/src/model.rs:673-686`) and
  `JsonRpcError { jsonrpc, id: Option<RequestId>, error: ErrorData }` (`model.rs:504-512`), so
  `{"jsonrpc","id","error":{"code","message"}}` is the only variant that can match — `Response`
  needs `result`, `Request`/`Notification` need `method`.
* `Content-Length` must equal `payload.len()` — the fixture writes raw bytes and closes.
* `json_id(&recorded.body)` already exists at
  [runtime.rs:4062-4067](../../crates/cyrup-mcp/src/runtime.rs).

**The fixture mode needs one consumer** or it is dead code. Add one ladder assertion immediately
after
[`the_401_predicate_still_refuses_every_other_status` at :4272-4288](../../crates/cyrup-mcp/src/runtime.rs),
built from the same helpers the bare-401 test uses (`CountingAuth::empty()`, `builder()`,
`http_entry`, `request`):

```rust
    /// The OTHER 401 rmcp does not type: 401 + `Content-Type: application/json` + a parseable
    /// JSON-RPC error body. rmcp applies its JSON-RPC-error shortcut to every non-success status
    /// (`reqwest/streamable_http_client.rs:278-290`), not just 400 the way the pinned TS SDK does
    /// (`index.mjs:5374-5381`), so this used to arrive at the ladder as
    /// `ClientInitializeError::JsonRpcError` — a shape `unauthorized_challenge` answers `None` for
    /// and `bare_unauthorized` is never even called on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_401_with_a_json_rpc_body_still_reaches_the_oauth_ladder() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            unauthorized_initializes: usize::MAX,
            challenge: false,
            json_rpc_body: true,
            ..FixtureOptions::default()
        })
        .await;
        let auth = CountingAuth::empty();
        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("jsonrpc401", http_entry(&fixture.url)))
            .await
            .expect("a 401 is `needs-auth` whatever body it carries");

        assert_eq!(connection.status, ConnectionStatus::NeedsAuth);
        assert_eq!(fixture.initializes(), 2, "implicit-deferred promotes and retries once");
        assert_eq!(auth.invalidations(), vec!["jsonrpc401".to_string()]);
        assert_eq!(auth.calls(), vec![Some(String::new())], "no header, so an empty challenge");
    }
```

Ablation, stated so it can be checked: **before** §4 lands this must fail with `connect_http_client`
returning `Err` out of arm 7 carrying `JSON-RPC error: Unauthorized`, not with a `NeedsAuth`.

---

## 4 · The fix — a client that owns the handshake POST

All of it goes in `runtime.rs`, immediately after
[`SessionIdProbe`'s impl block ends at :1097](../../crates/cyrup-mcp/src/runtime.rs) and before
`invalid_header` at [:1099](../../crates/cyrup-mcp/src/runtime.rs) — same file and same seam as
`SessionIdProbe`, and the two functions this feeds (`unauthorized_challenge`, `bare_unauthorized`)
are already there.

### 4.1 Imports

The import block is [runtime.rs:440-482](../../crates/cyrup-mcp/src/runtime.rs). Add:

```rust
use std::borrow::Cow;                                    // next to the other `std::` lines, :440-442
use futures::StreamExt as _;                             // next to `futures::stream::BoxStream`, :445
use sse_stream::{Error as SseError, Sse, SseStream};     // EXTEND the existing line :451
use rmcp::model::{ClientJsonRpcMessage, ClientRequest, JsonRpcMessage, ServerJsonRpcMessage}; // EXTEND :467
use rmcp::transport::common::http_header::{
    EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError,            // ADD to the existing group :468-471
    StreamableHttpClient, StreamableHttpClientTransportConfig, StreamableHttpError,
    StreamableHttpPostResponse,
};
```

`rmcp::transport::common::http_header` is reachable: `pub mod common` (`rmcp-3.1.4/src/transport.rs:125`)
→ `pub mod http_header` (`…/transport/common.rs:4`), and all four constants are `pub`
(`…/transport/common/http_header.rs:1-5`). `http`, `sse-stream`, `futures`, `reqwest` and
`serde_json` are already declared dependencies of this crate
([cyrup-mcp/Cargo.toml](../../crates/cyrup-mcp/Cargo.toml)); `reqwest`'s workspace features already
include `json` and `stream`, which `RequestBuilder::json` and `Response::bytes_stream` need.

### 4.2 Three restated privates

rmcp keeps each of these `pub(crate)`. Restating them is the identical situation
[`build_http_client` at runtime.rs:2092](../../crates/cyrup-mcp/src/runtime.rs) is already in for
rmcp's private `default_http_client`.

```rust
/// `DEFAULT_MAX_SSE_EVENT_SIZE` (`rmcp-3.1.4/src/transport/common/client_side_sse.rs:18`), which
/// rmcp keeps `pub(crate)`. Only [`UnauthorizedProbe::post_message`] needs it — the transport always
/// calls the `_with_max_sse_event_size` form
/// (`rmcp-3.1.4/src/transport/streamable_http_client.rs:773`, `:804`, `:867`, `:934`) — but
/// restating it there rather than delegating is what stops a direct caller of `post_message` from
/// bypassing the 401 classification.
const DEFAULT_MAX_SSE_EVENT_SIZE: usize = 16 * 1024 * 1024;

/// `validate_custom_header` (`rmcp-3.1.4/src/transport/common/http_header.rs:31-45`), inverted.
///
/// `MCP-Protocol-Version` is in rmcp's `RESERVED_HEADERS` but is explicitly allowed through (the
/// transport worker injects it post-init), so it is simply absent from this list. The SEP-2243
/// `Mcp-Method` / `Mcp-Name` / `Mcp-Param-*` headers are not reserved either and must keep passing:
/// `request_version_headers` puts them on the modern startup POST.
///
/// This rejection is what
/// [`request_headers_command.rs:65-67`](crate::request_headers_command) documents a derived header
/// hitting, so it must not weaken.
fn is_reserved_header(name: &HeaderName) -> bool {
    const RESERVED: [&str; 3] = ["accept", HEADER_SESSION_ID, HEADER_LAST_EVENT_ID];
    RESERVED
        .iter()
        .any(|reserved| name.as_str().eq_ignore_ascii_case(reserved))
}

/// `extract_scope_from_header` (`rmcp-3.1.4/src/transport/common/http_header.rs:50-73`), which rmcp
/// keeps `pub(crate)`. Rewritten slice-free because `clippy::indexing_slicing` is `deny` here;
/// behaviour is identical, including "an unterminated quoted value yields `None`" and "an empty
/// unquoted value yields `None`".
///
/// Byte offsets from the lowercased copy index the original safely: `to_ascii_lowercase` preserves
/// byte length, and `str::get` returns `None` rather than panicking off a char boundary.
fn scope_from_challenge(header: &str) -> Option<String> {
    const SCOPE_KEY: &str = "scope=";
    let start = header.to_ascii_lowercase().find(SCOPE_KEY)? + SCOPE_KEY.len();
    let value = header.get(start..)?;
    match value.strip_prefix('"') {
        Some(quoted) => {
            let end = quoted.find('"')?;
            quoted.get(..end).map(str::to_string)
        }
        None => {
            let end = value
                .find(|c: char| c == ',' || c == ';' || c.is_whitespace())
                .unwrap_or(value.len());
            let scope = value.get(..end)?;
            (!scope.is_empty()).then(|| scope.to_string())
        }
    }
}
```

### 4.3 The client

```rust
/// `error.status === 401` — the one bit rmcp's reqwest client throws away.
///
/// # Why this OWNS the POST instead of decorating it
///
/// [`SessionIdProbe`] and [`crate::request_headers_command::RequestHeadersCommandClient`] both sit
/// ABOVE `impl StreamableHttpClient for reqwest::Client` and delegate the send to it
/// (`runtime.rs:1017-1022`, `request_headers_command.rs:944-946`). What comes back to them is a
/// [`StreamableHttpPostResponse`] — a `ServerJsonRpcMessage`, a stream, and an `Option<String>`
/// session id, and NOTHING about the HTTP status. rmcp reads the status into a local at
/// `rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:243` and that local dies with
/// the frame, so the status cannot be decorated out of rmcp: it can only be kept by a client that
/// performs the POST itself.
///
/// # Why only the handshake
///
/// [`unauthorized_challenge`] has exactly one caller (`runtime.rs:2672`) and it takes a
/// [`ClientInitializeError`], so the startup POST is the only request whose 401 can start the OAuth
/// ladder. Every other POST is delegated to rmcp verbatim — same SSE limiter, same 202 handling,
/// same session-expiry — which keeps the blast radius at the handshake. The SSE cap in
/// [`capped_sse_stream`] is total-bytes rather than rmcp's per-event, and that is safe ONLY because
/// a handshake stream is drained to its first `Response` and dropped; a `tools/call` result stream
/// is not, which is the second reason non-handshake POSTs must stay delegated.
///
/// # Why `AuthRequired` and not a new shape
///
/// [`StreamableHttpError::AuthRequired`] is the currency the consumers already read, so nothing
/// downstream changes: [`unauthorized_challenge`] downcasts it at `runtime.rs:2021-2025` and returns
/// `Some(&required.www_authenticate_header)` — `Some("")` for a bare 401, which is exactly what
/// [`crate::oauth::on_unauthorized`] (`oauth.rs:3949-3964`) already expects. `AuthRequiredError::new`
/// is public (`rmcp-3.1.4/src/transport/streamable_http_client.rs:135-142`) and the variant carries
/// it as `#[source]` (`:203`), so the `source()` walk at `runtime.rs:2019-2030` finds it. rmcp's own
/// `ClientInitializeError::auth_challenge` (`…/service/client.rs:109-132`) reads the same type, and
/// so will `AuthClient` when section 05 lands it.
#[derive(Debug, Clone)]
pub struct UnauthorizedProbe {
    inner: reqwest::Client,
}

impl UnauthorizedProbe {
    /// Wrap the tuned client [`build_http_client`] produces.
    #[must_use]
    pub fn new(inner: reqwest::Client) -> Self {
        Self { inner }
    }
}
```

`Clone` is required by the trait (`StreamableHttpClient: Clone + Send + 'static`,
`…/streamable_http_client.rs:328`). `Debug` is not required by the trait, but
`RequestHeadersCommandClient<C>` derives it
([request_headers_command.rs:826-831](../../crates/cyrup-mcp/src/request_headers_command.rs)) and a
derived `Debug` carries a `where C: Debug` bound, so omitting it would silently delete that impl for
the production chain. `reqwest::Client` is both, so the derive is free.

`type Error` stays `reqwest::Error`: `bare_unauthorized` downcasts to
`StreamableHttpError<reqwest::Error>` at
[runtime.rs:2059-2063](../../crates/cyrup-mcp/src/runtime.rs) and that downcast must keep matching.
`?` on `send()` works because rmcp provides
`impl From<reqwest::Error> for StreamableHttpError<reqwest::Error>`
(`…/reqwest/streamable_http_client.rs:22-26`).

### 4.4 The predicate

Mirrors rmcp's own `is_legacy_startup` test
(`…/rmcp-3.1.4/src/transport/streamable_http_client.rs:848-852`), widened by the `Discover` arm for
the reason in §2.

```rust
/// The two requests that can be a client's *startup* message, and therefore the only two whose 401
/// can surface as a [`ClientInitializeError`].
///
/// `InitializeRequest` is [`ClientLifecycleMode::Initialize`]'s;
/// `DiscoverRequest` is `Discover`'s and `Auto`'s (`rmcp-3.1.4/src/service/client.rs:943-954`), both
/// of which [`version_negotiation`] reaches from `protocolVersion: "2026-07-28"` and `"auto"`.
/// Matching only `InitializeRequest` would leave this defect live for those two configurations.
fn is_handshake_request(message: &ClientJsonRpcMessage) -> bool {
    matches!(
        message,
        JsonRpcMessage::Request(request)
            if matches!(
                request.request,
                ClientRequest::InitializeRequest(_) | ClientRequest::DiscoverRequest(_)
            )
    )
}
```

### 4.5 The SSE stream

```rust
/// `bounded_sse_stream` (`rmcp-3.1.4/src/transport/common/client_side_sse.rs:144-155`), which rmcp
/// keeps `pub(crate)` along with its `SseEventSizeLimiter`.
///
/// NAMED DELTA: rmcp caps each SSE **event**; this caps the **total** bytes of the handshake
/// response, i.e. strictly stricter. That is sound only because it is reached only for a handshake
/// POST, whose stream `expect_initialized`
/// (`rmcp-3.1.4/src/transport/streamable_http_client.rs:264-283`) drains to the first `Response`
/// message and then DROPS. Every other POST keeps rmcp's per-event limiter because every other POST
/// is delegated.
///
/// `std::io::Error` rather than a bespoke enum: `SseStream::from_bytes_stream` needs only
/// `E: std::error::Error` (`sse-stream-0.2.5/src/stream.rs:36-56`), `reqwest::Error` cannot be
/// constructed, and `io::Error` carries both arms without adding a type.
fn capped_sse_stream(
    response: reqwest::Response,
    max_sse_event_size: usize,
) -> BoxStream<'static, Result<Sse, SseError>> {
    let mut seen = 0_usize;
    let capped = response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        seen = seen.saturating_add(chunk.len());
        if seen > max_sse_event_size {
            return Err(std::io::Error::other(format!(
                "handshake SSE response exceeded the maximum size of {max_sse_event_size} bytes"
            )));
        }
        Ok(chunk)
    });
    SseStream::from_bytes_stream(capped).boxed()
}
```

### 4.6 The owned send

A faithful port of `…/reqwest/streamable_http_client.rs:190-324` with **one** behavioural difference
(marked) and **two** arms dropped as unreachable-by-construction (each stated).

```rust
impl UnauthorizedProbe {
    async fn post_handshake(
        &self,
        uri: &str,
        message: &ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>> {
        // Byte-for-byte the request rmcp would have sent (`:196-211`), in rmcp's order: ACCEPT, the
        // separate bearer channel, the custom headers under the same reserved-header rejection, the
        // session header, then `serde_json` of the message.
        let mut request = self.inner.post(uri).header(
            reqwest::header::ACCEPT,
            [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "),
        );
        if let Some(auth_header) = auth_header {
            request = request.bearer_auth(auth_header);
        }
        for (name, value) in custom_headers {
            if is_reserved_header(&name) {
                return Err(StreamableHttpError::ReservedHeaderConflict(name.to_string()));
            }
            request = request.header(name, value);
        }
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request.json(message).send().await?;

        // ── THE FIX ──────────────────────────────────────────────────────────────────────────────
        // The status is read BEFORE the body, and 401 wins over the JSON-RPC shortcut whether or not
        // a challenge came with it. rmcp gates its own 401 arm on the header being present
        // (`:212-213`), which is why a bare 401 with a JSON body falls through to `:289`. Upstream
        // reads the status first and unconditionally
        // (`@modelcontextprotocol/client/dist/index.mjs:5333-5334`), and confines its own JSON-RPC
        // error passthrough to status 400 (`:5374-5381`).
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            let challenge = response
                .headers()
                .get(http::header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(challenge)));
        }
        // ── everything below mirrors rmcp `:227-324` ─────────────────────────────────────────────
        // 403 is NOT widened: `InsufficientScope` is reproduced exactly so a scope denial keeps
        // rmcp's vocabulary and stays a hard error, which is what `unauthorized_challenge`'s
        // `AuthRequiredError`-only downcast already enforces.
        if status == reqwest::StatusCode::FORBIDDEN
            && let Some(header) = response.headers().get(http::header::WWW_AUTHENTICATE)
        {
            let Ok(header) = header.to_str() else {
                return Err(StreamableHttpError::UnexpectedServerResponse(Cow::from(
                    "invalid www-authenticate header value",
                )));
            };
            return Err(StreamableHttpError::InsufficientScope(InsufficientScopeError::new(
                header.to_string(),
                scope_from_challenge(header),
            )));
        }
        if matches!(status, reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        // DROPPED ARM 1 of 2 — rmcp's `404 => SessionExpired` (`:250-252`) is kept, but it can never
        // fire here: the worker posts both startup requests with `session_id: None`
        // (`…/streamable_http_client.rs:870`, `:776`). Kept anyway, because it costs three lines and
        // an rmcp change that starts attaching one must not silently change meaning.
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let is_json = content_type
            .as_deref()
            .is_some_and(|ct| ct.starts_with(JSON_MIME_TYPE));

        // DROPPED ARM 2 of 2 — rmcp's empty-success => Accepted (`:265-277`) requires the OUTGOING
        // message to be a Notification/Response/Error. This method only runs for a Request, so the
        // arm is unreachable by construction and is omitted rather than written-and-dead.

        if !status.is_success() {
            // Unchanged from rmcp `:278-299`, and deliberately so: a 400 carrying
            // `UNSUPPORTED_PROTOCOL_VERSION` is how `Discover` renegotiates
            // (`…/service/client.rs:980-981`). Only 401 was taken above.
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_owned());
            if is_json
                && let Ok(parsed) = serde_json::from_str::<ServerJsonRpcMessage>(&body)
                && matches!(parsed, JsonRpcMessage::Error(_))
            {
                return Ok(StreamableHttpPostResponse::Json(parsed, session_id));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(format!(
                "HTTP {status}: {body}"
            ))));
        }
        if content_type
            .as_deref()
            .is_some_and(|ct| ct.starts_with(EVENT_STREAM_MIME_TYPE))
        {
            return Ok(StreamableHttpPostResponse::Sse(
                capped_sse_stream(response, max_sse_event_size),
                session_id,
            ));
        }
        if is_json {
            // Same tolerance as rmcp `:308-318`: a body that is not a `ServerJsonRpcMessage` is
            // treated as an accept rather than a failure.
            return Ok(match response.json::<ServerJsonRpcMessage>().await {
                Ok(message) => StreamableHttpPostResponse::Json(message, session_id),
                Err(_) => StreamableHttpPostResponse::Accepted,
            });
        }
        Err(StreamableHttpError::UnexpectedContentType(content_type))
    }
}
```

### 4.7 The trait impl

```rust
impl StreamableHttpClient for UnauthorizedProbe {
    type Error = reqwest::Error;

    /// Routed through [`Self::post_message_with_max_sse_event_size`] rather than delegated to
    /// `self.inner`, so a caller that reaches for the non-`_with_max` form cannot bypass the 401
    /// classification. rmcp's transport never calls this one — see [`DEFAULT_MAX_SSE_EVENT_SIZE`].
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            DEFAULT_MAX_SSE_EVENT_SIZE,
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        if is_handshake_request(&message) {
            return self
                .post_handshake(
                    &uri,
                    &message,
                    session_id,
                    auth_header,
                    custom_headers,
                    max_sse_event_size,
                )
                .await;
        }
        self.inner
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }

    // `get_stream`, `get_stream_with_max_sse_event_size` and `delete_session` delegate to
    // `self.inner` verbatim — one-line bodies, shaped exactly like `SessionIdProbe`'s at
    // `runtime.rs:1052-1096`. See §2 for why those legs need nothing.
}
```

### 4.8 Wiring — three edits in `connect_http_client` / `http_attempt`

* [runtime.rs:2606](../../crates/cyrup-mcp/src/runtime.rs) — wrap once:
  `let http_client = UnauthorizedProbe::new(build_http_client()?);`. Built **once per connect**,
  above the loop, exactly where the raw client is built today and for the reason the comment at
  [:2600-2605](../../crates/cyrup-mcp/src/runtime.rs) gives.
* [runtime.rs:2607-2614](../../crates/cyrup-mcp/src/runtime.rs) — **no textual change**;
  `RequestHeadersCommandClient::new(http_client.clone(), …)` now infers
  `RequestHeadersCommandClient<UnauthorizedProbe>`. That type is generic over
  `C: StreamableHttpClient + Sync`
  ([request_headers_command.rs:924-928](../../crates/cyrup-mcp/src/request_headers_command.rs)), so
  nothing in that file changes.
* [runtime.rs:2719-2722](../../crates/cyrup-mcp/src/runtime.rs) — `http_attempt`'s two parameter
  types: `http_client: &UnauthorizedProbe` and
  `signing_client: Option<&crate::request_headers_command::RequestHeadersCommandClient<UnauthorizedProbe>>`.
* [runtime.rs:2798](../../crates/cyrup-mcp/src/runtime.rs) and
  [:2813](../../crates/cyrup-mcp/src/runtime.rs) — `SessionIdProbe::new(…)` unchanged at both sites;
  the probe now wraps the new client. Final chain:
  `SessionIdProbe<[RequestHeadersCommandClient<]UnauthorizedProbe[>]>` → `reqwest::Client`.

### 4.9 Doc comments that become false and must not be left standing

* [runtime.rs:2047-2050](../../crates/cyrup-mcp/src/runtime.rs) (`bare_unauthorized`) says the
  `UnexpectedServerResponse` prefix arm "is the POST leg, i.e. the one `initialize` uses, so it is
  the arm that actually matters". After the fix the handshake POST is typed. **Keep the arm and
  restate what it now covers**, which is real and reachable: the `notifications/initialized` POST is
  sent inside `serve_client_with_lifecycle_and_ct` and its failure becomes
  `ClientInitializeError::transport::<T>(error, "send initialized notification")`
  (`…/service/client.rs:912`), so a bare 401 on *that* POST — still delegated to rmcp — still arrives
  here as `UnexpectedServerResponse("HTTP 401 …")`.
* Same doc block, the [`Client(reqwest::Error)` bullet at :2045-2046](../../crates/cyrup-mcp/src/runtime.rs):
  that arm is the GET/SSE leg, which is opened from a detached `JoinSet`
  (`…/streamable_http_client.rs:685-712`) and never reaches a `ClientInitializeError`. Say so — it is
  defence-in-depth, not a live path.
* [runtime.rs:1976-1981](../../crates/cyrup-mcp/src/runtime.rs) (`unauthorized_challenge`) explains
  the bare-401 widening in terms of rmcp's header gate, and **cites the wrong lines**: it says
  `streamable_http_client.rs:210-222` for POST and `:97-110` for GET; the real arms are `:212-226`
  and `:97-111`. Fix the numbers and rewrite the reasoning: on the handshake POST this crate no
  longer runs through rmcp's gate at all.
* [runtime.rs:4233-4240](../../crates/cyrup-mcp/src/runtime.rs)
  (`a_bare_401_with_no_challenge_still_reaches_the_oauth_ladder`'s doc) describes a test that now
  exercises the `AuthRequiredError` downcast rather than the `bare_unauthorized` prefix arm. The
  assertions are unchanged and must stay unchanged; the doc must say which arm it now proves.

---

## 5 · Explicitly out of scope

* No test suite, no benchmarks, no new documentation beyond the four stale doc comments in §4.9. The
  fixture mode in §3 and its single ladder assertion are a **prerequisite of the fix** — without them
  the change is unobservable — not a test-coverage exercise.
* Do not widen 403. `InsufficientScopeError` stays a hard error and is reproduced verbatim; the pin
  at [runtime.rs:4272-4288](../../crates/cyrup-mcp/src/runtime.rs) must keep passing untouched.
* Do not classify by JSON-RPC error code. The predicate is status-only, like
  [`isUnauthorizedHttpError`](../../tmp/pi-mcp-adapter/server-manager.ts).
* Do not take over non-handshake POSTs, and do not touch the GET/DELETE legs. §4.5's total-byte cap
  is only sound for a stream that is drained once and dropped.
* Do not patch or vendor rmcp — the workspace has no `[patch]` section and rmcp is a plain pin
  ([cyrup-mcp/Cargo.toml](../../crates/cyrup-mcp/Cargo.toml)).
* Do not touch [proxy/auth.rs](../../crates/cyrup-mcp/src/proxy/auth.rs). It consumes
  `ConnectionStatus::NeedsAuth`, which this fix produces more often and never differently.

---

## 6 · Definition of done

1. `FixtureOptions` has `json_rpc_body: bool` (default `false`), is destructured in `start_with`, and
   with `challenge: false, json_rpc_body: true` the fixture answers `initialize` with `401` +
   `Content-Type: application/json` + a JSON-RPC error body whose `id` echoes the request's.
2. `UnauthorizedProbe` exists in `runtime.rs`, derives `Debug + Clone`, keeps
   `type Error = reqwest::Error`, and returns
   `StreamableHttpError::AuthRequired(AuthRequiredError::new(challenge_or_empty))` for **every** 401
   on a handshake POST, regardless of body or `WWW-Authenticate`.
3. `is_handshake_request` matches **both** `ClientRequest::InitializeRequest` and
   `ClientRequest::DiscoverRequest`. Every other POST, and every GET/DELETE, is delegated to
   `reqwest::Client` unchanged.
4. A non-401 handshake response is byte-identical in outcome to rmcp's: 202/204 → `Accepted`;
   403-with-challenge → `InsufficientScope` with the scope extracted; non-success JSON-RPC error →
   `Ok(Json(..))`; success `text/event-stream` → `Sse`; success `application/json` → `Json`, or
   `Accepted` when the body will not parse; anything else → `UnexpectedContentType`. A reserved
   custom header still fails with `ReservedHeaderConflict`, and `MCP-Protocol-Version` still passes.
5. It is wired into both arms of `http_attempt` (signing and non-signing) and the two `http_attempt`
   parameter types name it.
6. `a_401_with_a_json_rpc_body_still_reaches_the_oauth_ladder` reaches `ConnectionStatus::NeedsAuth`
   with two `initialize`s, one invalidation, and one provider call carrying `Some("")` — and it fails
   without the §4 change, with `JSON-RPC error: Unauthorized` out of arm 7 (ablation).
7. `a_bare_401_with_no_challenge_still_reaches_the_oauth_ladder`
   ([runtime.rs:4242](../../crates/cyrup-mcp/src/runtime.rs)) and
   `the_401_predicate_still_refuses_every_other_status`
   ([runtime.rs:4273](../../crates/cyrup-mcp/src/runtime.rs)) still pass with their **assertions**
   unedited — F5 and the 403 exclusion are not regressed.
8. The four doc comments in §4.9 no longer state the opposite of what the code does, and the two
   wrong rmcp line citations at [runtime.rs:1978](../../crates/cyrup-mcp/src/runtime.rs) are correct.
9. `cargo clippy --workspace --all-targets` is clean under the workspace `deny`s
   (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `rustdoc::broken_intra_doc_links`),
   and `cargo nextest run --workspace` is 7863 passing (7862 + the one new assertion).

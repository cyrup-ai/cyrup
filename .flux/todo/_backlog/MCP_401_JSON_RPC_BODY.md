---
stage: aug
status: done
updated: 2026-08-22 15:14
---

# MCP-115 / F5: A bare 401 with a JSON-RPC body never reaches the OAuth ladder

## Objective

Make `ConnectionBuilder::connect_http_client` classify **every** HTTP 401 on the `initialize` POST
as unauthorized — including the one that carries `Content-Type: application/json` and a parseable
JSON-RPC error body — so the OAuth ladder runs instead of the connect failing hard.

Recorded as still-open at
[13-cyrup-mcp-STATUS.md:251](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md), deliberately left
unfixed in PR #30 because it is a second, distinct mechanism from the one F5 addressed.

Fails **safe** today — a hard connect error, never a wrongly-authenticated request — so this is
correctness, not a security hole.

---

## 1 · The exact shape that breaks (sharper than the original ticket)

rmcp's reqwest client handles a 401 **that carries `WWW-Authenticate`** at
`/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:212-226`,
**before** it ever reads the body — that arm returns `Err(StreamableHttpError::AuthRequired(..))`
and works today. So a 401 with a challenge header **and** a JSON body already reaches the ladder.

The broken shape is narrower than the ticket said, and all four conditions are required:

1. status `401`, **and**
2. **no** `WWW-Authenticate` header (otherwise `:212-226` claims it), **and**
3. `Content-Type: application/json`, **and**
4. a body that deserialises as `ServerJsonRpcMessage::Error` (rmcp's `parse_json_rpc_error`,
   same file `:42-47`, returns `None` for anything else and falls through to the error path).

Then `:278-294` runs:

```rust
if !status.is_success() {
    let body = response.text().await…;
    if content_type…starts_with(JSON_MIME_TYPE) {
        match parse_json_rpc_error(&body) {
            Some(message) => return Ok(StreamableHttpPostResponse::Json(message, session_id)), // :289
            None => tracing::warn!(…),
        }
    }
    return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(format!("HTTP {status}: {body}")))); // :296
}
```

`:289` returns **`Ok`**. The `Err(UnexpectedServerResponse("HTTP 401 …"))` at `:296` — the string
that `UNEXPECTED_UNAUTHORIZED_PREFIX` ([runtime.rs:2030](../../crates/cyrup-mcp/src/runtime.rs))
prefix-matches at [runtime.rs:2063](../../crates/cyrup-mcp/src/runtime.rs) — is never constructed.
`bare_unauthorized` ([runtime.rs:2051](../../crates/cyrup-mcp/src/runtime.rs)) is therefore never
reached on this path: it is not wrong, it is not called.

**What the connect ends as instead.** The `Ok(Json(Error(..)))` is delivered to rmcp's handshake as
an ordinary JSON-RPC error response. `expect_response` (`rmcp-3.1.4/src/service/client.rs:191-204`)
turns it into `ClientInitializeError::JsonRpcError(ErrorData)` (`:194` when the body echoes the
request id, `:199` when the body omits it; a *mismatched* id gives `UncorrelatedErrorResponse` at
`:200-203`). `unauthorized_challenge` ([runtime.rs:1993](../../crates/cyrup-mcp/src/runtime.rs))
matches only `TransportError` and `LegacyFallbackFailed` ([runtime.rs:2003-2011](../../crates/cyrup-mcp/src/runtime.rs))
and returns `None` for every other variant, so the ladder at
[runtime.rs:2665](../../crates/cyrup-mcp/src/runtime.rs) takes the `else` branch and the connect
dies at arm 7 ([runtime.rs:2667](../../crates/cyrup-mcp/src/runtime.rs)). No `needs-auth`, no
`/mcp-auth` offer, no OAuth ladder — ever, for that server.

---

## 2 · Where the status is still visible — and why no existing seam sees it

**The status exists in exactly one place: a function local inside rmcp.**
`let status = response.status();` at `…/rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:243`,
inside `impl StreamableHttpClient for reqwest::Client` (`:49`). It is consumed at `:244`, `:250`,
`:265`, `:278` and formatted into a string at `:297`, and it dies with the stack frame.

**The original ticket's fix shape is wrong as written.** It says to raise the unauthorized shape
"in the client-decorator seam this crate already occupies (`SessionIdProbe` /
`RequestHeadersCommandClient`)". Verified: **neither decorator can ever see the status**, because
both sit *above* rmcp's `reqwest::Client` impl and delegate the send to it:

* `SessionIdProbe::post_message` — [runtime.rs:1017-1020](../../crates/cyrup-mcp/src/runtime.rs)
  `self.inner.post_message(…).await?`, then `record(&response)`
  ([runtime.rs:985-1000](../../crates/cyrup-mcp/src/runtime.rs)) which reads only the
  `Option<String>` session id off the returned `StreamableHttpPostResponse`.
* `RequestHeadersCommandClient::post_message` —
  [request_headers_command.rs:944-946](../../crates/cyrup-mcp/src/request_headers_command.rs)
  `self.inner.post_message(…).await`, returned verbatim.

What reaches them is `StreamableHttpPostResponse`
(`…/rmcp-3.1.4/src/transport/streamable_http_client.rs:239-244`), whose three variants carry a
`ServerJsonRpcMessage`, a `BoxedSseStream`, and an `Option<String>` session id. **No status, and no
headers.** A 401-with-JSON-RPC-body and a 200-with-JSON-RPC-body are byte-identical at that seam, so
no decorator above the send can distinguish them without guessing at the JSON-RPC error code — which
would turn a legitimate `initialize` rejection (bad protocol version, say) into a `needs-auth`, and
upstream's predicate is status-only (`isUnauthorizedHttpError`, `server-manager.ts:73-75`).

**Consequence: the seam has to move to the bottom of the chain and own the POST send.** The chain
built at [runtime.rs:2789-2819](../../crates/cyrup-mcp/src/runtime.rs) is
`SessionIdProbe<[RequestHeadersCommandClient<]reqwest::Client[>]>`; the fix replaces the innermost
`reqwest::Client` with a client this crate owns.

Two facts that make this cheap:

* **Only the `initialize` POST matters.** `unauthorized_challenge` has exactly one caller — the
  ladder at [runtime.rs:2665](../../crates/cyrup-mcp/src/runtime.rs) — and it takes a
  `ClientInitializeError`. Every other POST (notifications, `tools/call`, resumption) can keep
  rmcp's behaviour untouched by delegating.
* **The GET/DELETE legs already work and stay delegated.** rmcp's `get_stream` returns
  `AuthRequired` for a 401 with a header (`…/streamable_http_client.rs:97-111`) and, for a bare 401,
  `error_for_status()` at `:128` yields `StreamableHttpError::Client(reqwest::Error)` whose
  `.status()` is `Some(401)` — the typed arm `bare_unauthorized` already handles at
  [runtime.rs:2058-2060](../../crates/cyrup-mcp/src/runtime.rs).

---

## 3 · Do the fixture first — concretely

The fixture's inability to produce this shape is exactly why the gap went unseen. Without it there
is nothing to point the fix at.

`HttpFixture` / `FixtureOptions` live in the `mod tests` of runtime.rs:
[struct at :3885-3898](../../crates/cyrup-mcp/src/runtime.rs),
[`Default` at :3900-3910](../../crates/cyrup-mcp/src/runtime.rs),
[destructuring at :3939-3945](../../crates/cyrup-mcp/src/runtime.rs),
[the 401 arm at :3990-3999](../../crates/cyrup-mcp/src/runtime.rs).

**One new field**, after `challenge` at [:3890](../../crates/cyrup-mcp/src/runtime.rs):

```rust
        /// Answer those 401s with `Content-Type: application/json` and a parseable JSON-RPC error
        /// body instead of `Content-Length: 0`. With `challenge: false` this is the shape rmcp
        /// collapses into `Ok(StreamableHttpPostResponse::Json(..))`
        /// (`streamable_http_client.rs:287-290`), so no transport error is constructed and nothing
        /// `unauthorized_challenge` walks can see it. With `challenge: true` rmcp's `:212-226` arm
        /// claims it first and the ladder is reached today — which is what makes this field an
        /// ablation rather than a restatement.
        json_rpc_body: bool,
```

`Default` = `false` (every existing call site keeps its current shape), and add it to the
`let FixtureOptions { … } = options;` destructuring at
[:3939-3945](../../crates/cyrup-mcp/src/runtime.rs).

**How the fixture must answer `initialize`** — replace the body of the 401 arm at
[:3990-3999](../../crates/cyrup-mcp/src/runtime.rs):

```rust
                    } else if is_initialize && initializes <= unauthorized_initializes {
                        let challenge_header = if challenge {
                            "WWW-Authenticate: Bearer realm=\"mcp\", resource_metadata=\"https://example.invalid/.well-known\"\r\n"
                        } else {
                            ""
                        };
                        if json_rpc_body {
                            // The id is ECHOED, so rmcp's `expect_response`
                            // (`service/client.rs:191-204`) takes the `JsonRpcError` arm at `:194`
                            // rather than `UncorrelatedErrorResponse` at `:200`. Both fail today,
                            // but only the echoing shape is the one a real server produces, and the
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

Requirements on that body, each load-bearing:

* `Content-Type: application/json` — rmcp gates the shortcut on it (`:283-285`).
* Must deserialise as `ServerJsonRpcMessage::Error`, i.e. `{"jsonrpc","id","error":{"code","message"}}`
  — `parse_json_rpc_error` (`:42-47`) returns `None` for any other variant and the response then
  falls through to `:296` and the *old*, already-fixed path.
* `Content-Length` must match `payload.len()` — the fixture writes raw bytes and closes.
* `json_id(&recorded.body)` already exists at
  [runtime.rs:4055-4060](../../crates/cyrup-mcp/src/runtime.rs).

**The fixture mode needs one consumer** or it is dead code. Add one ladder assertion next to
[`a_bare_401_with_no_challenge_still_reaches_the_oauth_ladder` at :4235](../../crates/cyrup-mcp/src/runtime.rs),
built from the same helpers (`CountingAuth::empty()`, `builder()`, `http_entry`, `request`):

```rust
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

Ablation, stated so it can be checked: **before** §4 lands this must fail with the connect erroring
out of arm 7 carrying rmcp's `JSON-RPC error: …` text, not with a `NeedsAuth`.

---

## 4 · The fix — a client that owns the `initialize` POST

Add next to [`SessionIdProbe` at runtime.rs:964](../../crates/cyrup-mcp/src/runtime.rs) (same file,
same seam, and `unauthorized_challenge`/`bare_unauthorized` are already there). Every type it needs
is already imported by runtime.rs at [:440-474](../../crates/cyrup-mcp/src/runtime.rs) except
`AuthRequiredError` (add to the group at
[:468-471](../../crates/cyrup-mcp/src/runtime.rs)), `rmcp::model::{ClientRequest, JsonRpcMessage,
ServerJsonRpcMessage}`, `sse_stream::SseStream`, `std::borrow::Cow` and `futures::StreamExt`.

```rust
/// `error.status === 401` — the one bit rmcp's reqwest client throws away.
///
/// # Why this OWNS the POST instead of decorating it
///
/// [`SessionIdProbe`] and [`crate::request_headers_command::RequestHeadersCommandClient`] both sit
/// ABOVE `impl StreamableHttpClient for reqwest::Client` and delegate the send to it
/// (`runtime.rs:1017-1020`, `request_headers_command.rs:944-946`). What comes back to them is a
/// `StreamableHttpPostResponse` — a `ServerJsonRpcMessage`, a stream, and an `Option<String>`
/// session id, and NOTHING about the HTTP status. rmcp reads the status into a local at
/// `rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:243` and that local dies with
/// the frame, so the status cannot be decorated out of rmcp: it can only be kept by a client that
/// performs the POST itself.
///
/// # Why only `initialize`
///
/// `unauthorized_challenge` has exactly one caller (`runtime.rs:2665`) and it takes a
/// `ClientInitializeError`, so the handshake POST is the only request whose 401 can start the OAuth
/// ladder. Every other POST is delegated to rmcp verbatim — same SSE limiter, same 202 handling,
/// same session-expiry — which keeps the blast radius at the handshake.
#[derive(Debug, Clone)]
pub struct UnauthorizedProbe {
    inner: reqwest::Client,
}
```

`Debug + Clone` are not optional: `RequestHeadersCommandClient` derives both
([request_headers_command.rs:826-831](../../crates/cyrup-mcp/src/request_headers_command.rs)) and
`StreamableHttpClient` requires `Clone + Send + 'static`
(`…/rmcp-3.1.4/src/transport/streamable_http_client.rs:328`). Keep `type Error = reqwest::Error` —
`bare_unauthorized` downcasts to `StreamableHttpError<reqwest::Error>` at
[runtime.rs:2052-2054](../../crates/cyrup-mcp/src/runtime.rs) and that downcast must keep matching.

**The dispatch.** rmcp's transport always calls the `_with_max_sse_event_size` form
(`…/streamable_http_client.rs:773`, `:804`, `:867`, `:934`), so that is where the work goes;
`post_message` forwards to it with rmcp's own default (`client_side_sse.rs:18`, `16 * 1024 * 1024`,
`pub(crate)` so it must be restated as a local const) rather than delegating, so a direct caller
cannot bypass the fix. `get_stream`, `get_stream_with_max_sse_event_size` and `delete_session`
delegate to `self.inner` unchanged — see §2 for why those legs already type their 401s.

```rust
fn is_initialize(message: &ClientJsonRpcMessage) -> bool {
    matches!(
        message,
        JsonRpcMessage::Request(request)
            if matches!(request.request, ClientRequest::InitializeRequest(_))
    )
}
```

**The owned send** — a faithful port of `…/streamable_http_client.rs:199-324` with **one**
behavioural difference, marked below:

```rust
    async fn post_initialize(
        &self,
        uri: &str,
        message: &ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>> {
        // Byte-for-byte the request rmcp would have sent (`:199-211`): same ACCEPT, same separate
        // bearer channel, same custom headers under the same reserved-header rejection, same
        // session header, same `serde_json::to_vec(&message)` body.
        let mut request = self.inner.post(uri).header(
            reqwest::header::ACCEPT,
            [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "),
        );
        if let Some(auth_header) = auth_header {
            request = request.bearer_auth(auth_header);
        }
        for (name, value) in custom_headers {
            reject_reserved_header(&name)?;
            request = request.header(name, value);
        }
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request.json(message).send().await?;

        // ── THE FIX ────────────────────────────────────────────────────────────────────────────
        // The status is read BEFORE the body, and 401 wins over the JSON-RPC shortcut whether or
        // not a challenge came with it. rmcp gates its own 401 arm on the header being present
        // (`:212-213`), which is why a bare 401 with a JSON body falls through to `:289`.
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
        // ── everything below mirrors rmcp `:244-324` ───────────────────────────────────────────
        if matches!(status, reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string());
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        // rmcp's empty-success arm (`:265-275`) is unreachable here BY CONSTRUCTION: it requires the
        // outgoing message to be a Notification/Response/Error, and this method only runs for the
        // `initialize` Request.
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_owned());
            if content_type.as_deref().is_some_and(|ct| ct.starts_with(JSON_MIME_TYPE))
                && let Ok(parsed) = serde_json::from_str::<ServerJsonRpcMessage>(&body)
                && matches!(parsed, JsonRpcMessage::Error(_))
            {
                return Ok(StreamableHttpPostResponse::Json(parsed, session_id));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(format!(
                "HTTP {status}: {body}"
            ))));
        }
        match content_type.as_deref() {
            Some(ct) if ct.starts_with(EVENT_STREAM_MIME_TYPE) => Ok(StreamableHttpPostResponse::Sse(
                bounded_handshake_stream(response, max_sse_event_size),
                session_id,
            )),
            Some(ct) if ct.starts_with(JSON_MIME_TYPE) => {
                match response.json::<ServerJsonRpcMessage>().await {
                    Ok(message) => Ok(StreamableHttpPostResponse::Json(message, session_id)),
                    // Same tolerance as rmcp `:309-317`.
                    Err(_) => Ok(StreamableHttpPostResponse::Accepted),
                }
            }
            other => Err(StreamableHttpError::UnexpectedContentType(other.map(str::to_string))),
        }
    }
```

Three helpers this needs, each because rmcp keeps its own copy `pub(crate)` — the identical reason
[`build_http_client` at runtime.rs:2085](../../crates/cyrup-mcp/src/runtime.rs) already rebuilds
rmcp's private `default_http_client`:

* `EVENT_STREAM_MIME_TYPE`, `JSON_MIME_TYPE`, `HEADER_SESSION_ID` are **public**:
  `rmcp::transport::common::http_header::{…}` (`…/rmcp-3.1.4/src/transport/common/http_header.rs:1-5`,
  module is `pub mod` at `transport.rs:125`). Import them, do not restate them.
* `reject_reserved_header` — rmcp's `validate_custom_header` is `pub(crate)`
  (`…/http_header.rs:31-45`). Restate it exactly: reject `accept`, `Mcp-Session-Id`,
  `Last-Event-Id` case-insensitively with
  `StreamableHttpError::ReservedHeaderConflict(name.to_string())`, and let
  `MCP-Protocol-Version` through (the worker injects it post-init). This rejection is what
  [request_headers_command.rs:66](../../crates/cyrup-mcp/src/request_headers_command.rs) documents a
  derived header hitting, so it must not weaken.
* `bounded_handshake_stream` — rmcp's `bounded_sse_stream` and its `SseEventSizeLimiter` are
  `pub(crate)` (`…/rmcp-3.1.4/src/transport/common/client_side_sse.rs:144`). Build the stream as
  `SseStream::from_bytes_stream(capped).boxed()` over `response.bytes_stream()` capped by a
  `futures::StreamExt::scan` that sums chunk lengths and yields an error once the total passes
  `max_sse_event_size`. Two notes for the reviewer, both deliberate:
  * the cap is **total bytes of the handshake response** rather than rmcp's **per event**, i.e.
    strictly stricter, and it applies only to the `initialize` response — which
    `expect_initialized` (`…/streamable_http_client.rs:265-288`) drains to the first `Response`
    message and then drops. Every other POST keeps rmcp's own per-event limiter because every other
    POST is delegated.
  * `SseStream::from_bytes_stream` requires `E: std::error::Error`
    (`sse-stream-0.2.5/src/stream.rs:36-56`) and `reqwest::Error` cannot be constructed, so the
    capped stream needs a small local `thiserror` enum with a `#[from] reqwest::Error` arm and a
    `TooLarge { limit }` arm.

### Wiring — four edits in `connect_http_client`/`http_attempt`

* [runtime.rs:2599](../../crates/cyrup-mcp/src/runtime.rs) — after `build_http_client()?`, wrap
  once: `let http_client = UnauthorizedProbe::new(build_http_client()?);`. Built **once per
  connect**, above `attempt`, exactly where the raw client is built today and for the reason the
  comment at [:2593-2598](../../crates/cyrup-mcp/src/runtime.rs) gives.
* [runtime.rs:2600-2607](../../crates/cyrup-mcp/src/runtime.rs) — `RequestHeadersCommandClient::new`
  takes the probe, so the signing client becomes
  `RequestHeadersCommandClient<UnauthorizedProbe>`. It is generic over `C: StreamableHttpClient +
  Sync` ([request_headers_command.rs:924-928](../../crates/cyrup-mcp/src/request_headers_command.rs)),
  so nothing in that file changes.
* [runtime.rs:2712-2715](../../crates/cyrup-mcp/src/runtime.rs) — `http_attempt`'s two parameter
  types: `http_client: &UnauthorizedProbe` and
  `signing_client: Option<&RequestHeadersCommandClient<UnauthorizedProbe>>`.
* [runtime.rs:2791](../../crates/cyrup-mcp/src/runtime.rs) and
  [:2806](../../crates/cyrup-mcp/src/runtime.rs) — `SessionIdProbe::new(…)` is unchanged at both
  sites; the probe now wraps the new client. Final chain:
  `SessionIdProbe<[RequestHeadersCommandClient<]UnauthorizedProbe[>]>` → `reqwest::Client`.

### Why `AuthRequired` and not a new shape

`StreamableHttpError::AuthRequired(AuthRequiredError)` is the currency the consumers already read,
so nothing downstream changes:

* `unauthorized_challenge` downcasts it at
  [runtime.rs:2014-2018](../../crates/cyrup-mcp/src/runtime.rs) and returns
  `Some(&required.www_authenticate_header)` — `Some("")` for a bare 401, which is exactly what the
  bare-401 path already produces ([runtime.rs:2019-2021](../../crates/cyrup-mcp/src/runtime.rs)) and
  what `on_unauthorized` ([oauth.rs:3949-3964](../../crates/cyrup-mcp/src/oauth.rs)) expects.
* `AuthRequiredError::new(String)` is public
  (`…/rmcp-3.1.4/src/transport/streamable_http_client.rs:135-142`) and the variant carries it as
  `#[source]` (`:203`), so the `source()` walk at
  [runtime.rs:2012-2022](../../crates/cyrup-mcp/src/runtime.rs) finds it.
* rmcp's own `ClientInitializeError::auth_challenge` (`…/src/service/client.rs:110-131`) reads the
  same type, and so will `AuthClient` when section 05 lands it — its
  `call_reacting_to_challenges` matches `Err(StreamableHttpError::AuthRequired(..))` to drive the
  silent refresh (`…/src/transport/common/auth/streamable_http_client.rs:44-62`). rmcp's own OAuth
  path is broken by this same collapse today; producing `AuthRequired` fixes it there too.

### Two doc comments that become false and must not be left standing

* [runtime.rs:2032-2049](../../crates/cyrup-mcp/src/runtime.rs) (`bare_unauthorized`) says the
  `UnexpectedServerResponse` prefix arm "is the POST leg, i.e. the one `initialize` uses, so it is
  the arm that actually matters". After the fix the initialize POST is typed and that arm covers
  only non-initialize POSTs, which never reach `unauthorized_challenge`. Keep the arm, restate what
  it now covers.
* [runtime.rs:1967-1974](../../crates/cyrup-mcp/src/runtime.rs) (`unauthorized_challenge`) explains
  the bare-401 widening in terms of rmcp's header gate. That gate is no longer what this crate runs
  through on the POST leg; the surviving reason for the `bare_unauthorized` arm is the GET leg's
  `error_for_status()` at `…/streamable_http_client.rs:128`.

---

## 5 · Explicitly out of scope

* No test suite, no benchmarks, no new documentation beyond the two stale doc comments above. The
  fixture mode in §3 and its single ladder assertion are a **prerequisite of the fix** — without
  them the change is unobservable — not a test-coverage exercise.
* Do not widen 403. `InsufficientScopeError` stays a hard error; the pin at
  [runtime.rs:4266](../../crates/cyrup-mcp/src/runtime.rs) must keep passing untouched.
* Do not classify by JSON-RPC error code. The predicate is status-only, like upstream's
  `isUnauthorizedHttpError`.
* Do not take over non-`initialize` POSTs, and do not touch the GET/DELETE legs.
* Do not patch or vendor rmcp — the workspace has no `[patch]` section and rmcp is a plain pin
  ([cyrup-mcp/Cargo.toml](../../crates/cyrup-mcp/Cargo.toml)).

---

## 6 · Definition of done

1. `FixtureOptions` has `json_rpc_body: bool` (default `false`), and with
   `challenge: false, json_rpc_body: true` the fixture answers `initialize` with
   `401` + `Content-Type: application/json` + a JSON-RPC error body whose `id` echoes the request's.
2. `UnauthorizedProbe` exists, owns the `initialize` POST, and returns
   `StreamableHttpError::AuthRequired(AuthRequiredError::new(challenge_or_empty))` for **every**
   401 on that POST, regardless of body or `WWW-Authenticate`. Every other POST and every
   GET/DELETE is delegated to `reqwest::Client` unchanged.
3. It is wired into both arms of `http_attempt` (signing and non-signing), and `type Error` is
   still `reqwest::Error`.
4. `a_401_with_a_json_rpc_body_still_reaches_the_oauth_ladder` reaches `ConnectionStatus::NeedsAuth`
   with two `initialize`s, one invalidation, and one provider call carrying `Some("")` — and it
   fails without the §4 change (ablation).
5. `a_bare_401_with_no_challenge_still_reaches_the_oauth_ladder`
   ([runtime.rs:4235](../../crates/cyrup-mcp/src/runtime.rs)) and
   `the_401_predicate_still_refuses_every_other_status`
   ([runtime.rs:4266](../../crates/cyrup-mcp/src/runtime.rs)) still pass, unedited — F5 and the
   403 exclusion are not regressed.
6. The two doc comments in §4 no longer state the opposite of what the code does.

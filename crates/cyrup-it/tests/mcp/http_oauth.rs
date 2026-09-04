//! **HTTP + OAuth, end to end, against a server that really demands a token.**
//!
//! [`super::live_tool_call`] proves the live path over **stdio**: a real child process, a handshake
//! observed from the server's side of the pipe, and `echoed:pong` in the transcript. It says nothing
//! about credentials, because a stdio server has none.
//!
//! This file is that proof for the transport that does. Every test here starts a **real loopback
//! HTTP server** ([`HttpMcpFixture`]) whose one and only gate is the bearer token: a request without
//! `authorization: Bearer <token>` is answered `401` with a `WWW-Authenticate` challenge, whichever
//! request it is and however many have gone before. Nothing in this crate, in `cyrup-mcp`, or in the
//! session can produce the fixture's `echoed:pong` without a round trip that carried a real
//! credential over a real socket.
//!
//! # The three journeys
//!
//! * **A — the returning user.** A credential is already in the vault when the session starts. The
//!   server must connect on the **first** attempt with no `401`, no retry, no browser and no
//!   loopback listener; its catalog must reach the model; and a model-issued `fixture_echo` call
//!   must come back with the server's own bytes.
//!   ([`a_stored_credential_connects_an_http_server_with_no_prompt`])
//! * **B — the first login.** An empty vault. The connect ends at `needs-auth`, the model drives the
//!   headless copy-paste protocol (`mcp({action:"auth-start"})` → the test plays the browser →
//!   `mcp({action:"auth-complete"})`), a token lands in the vault, `mcp({connect})` succeeds, and a
//!   **second session over the same vault** is journey A — connected with zero `401`s.
//!   ([`a_first_login_stores_a_token_and_the_next_session_connects_silently`])
//! * **C — the same first login, driven by the HUMAN's slash command.** The route the runtime's
//!   own `needs-auth` message names. `/mcp-auth fixture` is submitted through
//!   `AgentSession::prompt`, the authorization URL leaves through `HostServices::notify`, and
//!   the test — playing the browser CONCURRENTLY, because the handler is blocked inside
//!   `oauth::authenticate` until it answers — issues the callback GET on the loopback listener
//!   the flow bound. That is the leg journey B cannot reach: no string is pasted anywhere.
//!   ([`the_mcp_auth_command_logs_in_against_a_real_server_and_stores_a_usable_credential`])
//!
//! # The negative controls, and why each one is here
//!
//! An end-to-end assertion nobody has watched fail is not evidence. Each of these has been watched
//! fail: with `.with_auth_provider(...)` removed from `initialize_mcp`'s one `ConnectionBuilder`,
//! five of the eight tests this file held at the time go red, journey A's on its very first
//! assertion and journey B's at phase 5 — while the two that *should* be indifferent to the
//! provider stay green.
//!
//! * [`an_unauthenticated_http_server_ends_at_needs_auth`] — the identical fixture and config with
//!   **no** seeded credential. `needs-auth`, and the byte-exact
//!   `"OAuth authentication required. Run /mcp-auth fixture."` on the failure record.
//! * [`a_wrong_stored_token_fails_loudly_rather_than_connecting_empty`] — a credential that is
//!   present but **wrong**. This is the one that would let the whole feature ship broken: a server
//!   that answered `200` to an unauthorized handshake, or a runtime that recorded `Connected` for a
//!   `401`, would produce a connected-but-empty server and every "the tools reached the model"
//!   assertion above would read as a discovery bug rather than an auth bug.
//! * [`an_unreadable_vault_fails_the_connect_rather_than_asking_for_a_login`] — the vault itself is
//!   broken. A connect failure carrying the store's own message, never `needs-auth`: the two must
//!   not be confusable, or a user with a broken keychain is sent round the login loop forever.
//! * [`a_redirect_with_the_wrong_state_is_refused_and_no_token_is_exchanged`] — journey B's control.
//!   The paste path takes a string a human copied out of a browser; "the flow accepted it" only
//!   means something if a mismatched CSRF state is refused before the token endpoint is touched.
//! * [`an_implicit_oauth_server_retries_once_with_the_stored_token`] — `auth` omitted. The vault is
//!   not touched until the server proves it needs to be: `401`, then a retry carrying the token.
//!   Two requests, still no prompt.
//! * [`an_expired_credential_is_refreshed_at_connect_time`] — the reason the provider goes through
//!   `get_valid_token` rather than a bare store read. rmcp's streamable-HTTP transport takes a
//!   *static* bearer, so there is no SDK auth loop behind it and the provider is the only place a
//!   refresh can happen; without it an expired-but-refreshable credential would `401` forever.
//!
//! Journey C carries three of its own, in section 5: a denied authorization, an unconfigured server
//! name, and a headless session. Its own success path has been watched fail the same way — with the
//! callback `GET` suppressed and nothing else changed, `/mcp-auth fixture` never returns and the
//! test dies on its step timeout, which is what shows the loopback listener is carrying the login
//! rather than decorating it.
//!
//! # What this file does NOT prove, stated rather than implied
//!
//! * **`attempt_auto_auth`.** Journey C reaches `oauth::authenticate` — and therefore
//!   `wait_for_callback`, the loopback listener and a real callback GET — through the `/mcp-auth`
//!   command. The OTHER caller, the connect-time auto-login gated on `settings.autoAuth`, is still
//!   undriven here.
//! * **`wait_for_authorization_response`'s callback-vs-paste race.** Journey C takes the
//!   no-prompt arm: `McpExtension::authenticate_server` deliberately installs no
//!   `on_authorization_input` hook (its doc says why), so the flow simply awaits the callback and
//!   there is no second racer to observe.
//! * **The browser handoff itself.** Journey C injects [`cyrup_mcp::oauth::NoopLauncher`] through
//!   `McpExtension::with_browser_launcher`, so no `open` is attempted. Production keeps
//!   `OpenerLauncher`. The URL is surfaced by `on_authorization_url` BEFORE the handoff — which is
//!   what this file reads — so suppressing the launch costs the proof nothing. What is proven is
//!   that the login completes without a browser, not that a browser would be opened.
//!
//! # The seam this file needs, and why it is not `std::env::set_var`
//!
//! The credential backend is chosen by an **environment** switch
//! (`cyrup_mcp::credentials::TEST_AUTH_STORE_ENV`), and edition 2024 made `std::env::set_var`
//! `unsafe` with std's own conclusion that a multithreaded program must not call it at all. So these
//! tests inject the vault through `McpExtension::with_auth_store`, the same shape and the same
//! reason as `McpExtension::with_home`. Without that seam the session builds
//! `McpAuthStore::new(dirs, options)`, which on any machine with no OS credential store — this
//! container, CI, a headless Linux box — answers `keyring::Error::NoDefaultStore` on the first read;
//! `StoredCredentialAuth` correctly refuses to read that as "you have never logged in", and every
//! `auth: "oauth"` HTTP server fails its connect outright.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{Message, StopReason};
use cyrup_mcp::McpExtension;
use cyrup_mcp::credentials::{AuthStorageOptions, McpAuthStore, MemorySecretStore};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_session_svc::{
    AgentSession, AppMode, NotifyKind, SessionBuilder, SessionConfig, UiEffect,
};
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// The gateway tool `register_surface` registers.
const PROXY_TOOL: &str = "mcp";
/// The one server the fixture `mcp.json` configures.
const SERVER: &str = "fixture";
/// The tool the fixture advertises, under the name the SERVER uses.
const REMOTE_TOOL: &str = "echo";
/// The same tool under the name the MODEL sees — `format_tool_name("echo", "fixture", Server)`.
const DIRECT_TOOL: &str = "fixture_echo";
/// The fixture's answer to `tools/call`, verbatim.
const SERVER_ANSWER: &str = "echoed:pong";
/// The ONE bearer token the fixture accepts, and the one its `/token` endpoint mints.
const GOOD_TOKEN: &str = "fixture-access-token";
/// The token the fixture mints for a `grant_type=refresh_token` exchange. Distinct from
/// [`GOOD_TOKEN`] so the refresh test can tell which one authorized the connect — but the fixture
/// accepts both, because a refreshed credential must keep working.
const REFRESHED_TOKEN: &str = "fixture-refreshed-token";
/// The authorization code the test hands back as the browser.
const AUTH_CODE: &str = "fixture-auth-code";

// =================================================================================================
// 1 · The fixture: a real HTTP server, a real socket, and one gate.
// =================================================================================================

/// One request as the server saw it, plus the status it answered with.
#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
    status: u16,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Every value under `name`. A `Vec` rather than an `Option` because the assertion that matters
    /// is "exactly ONE Authorization header" — two would be the header-collision regression.
    fn all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect()
    }

    fn is_method_call(&self, method: &str) -> bool {
        self.body.contains(&format!("\"method\":\"{method}\""))
    }
}

/// A loopback MCP server over HTTP/1.1, hand-rolled, plus the authorization-server endpoints on the
/// same port.
///
/// Hand-rolled rather than a framework for the reason `crates/cyrup-mcp/src/runtime.rs`'s own
/// fixture is: the assertions are about exact bytes on the wire — which `Authorization` header
/// arrived, how many attempts were made, what status each one drew — and a framework would normalise
/// precisely the things under test. `cyrup-it` also has **no `rmcp` dependency**, by design, so the
/// fixture echoes back whatever `protocolVersion` the client asks for rather than naming one.
///
/// **The token is the only gate.** This fixture deliberately does *not* count attempts: a fixture
/// that answers `200` to the second request regardless of its headers would let journey A pass with
/// a provider that returns `None` for everything, which is exactly the state this whole change set
/// exists to leave behind.
struct HttpMcpFixture {
    /// The MCP endpoint — `http://127.0.0.1:<port>/mcp`.
    url: String,
    /// The authorization-server / protected-resource origin — `http://127.0.0.1:<port>`.
    issuer: String,
    log: Arc<Mutex<Vec<Recorded>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl HttpMcpFixture {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let issuer = format!("http://{addr}");
        let log: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        let origin = issuer.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                // One task per connection. rmcp's OAuth machinery and its transport are separate
                // clients and may have a request in flight at the same time; a serial accept loop
                // would serialise them into a deadlock the moment one waits on the other.
                let sink = Arc::clone(&sink);
                let origin = origin.clone();
                tokio::spawn(async move {
                    serve(socket, &origin, &sink).await;
                });
            }
        });
        Self {
            url: format!("http://{addr}/mcp"),
            issuer,
            log,
            _task: task,
        }
    }

    fn requests(&self) -> Vec<Recorded> {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Every request the fixture answered `401` to. The load-bearing count for journey A.
    fn unauthorized(&self) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|recorded| recorded.status == 401)
            .collect()
    }

    fn initializes(&self) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|recorded| recorded.is_method_call("initialize"))
            .collect()
    }

    fn hits(&self, path: &str) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|recorded| recorded.path == path)
            .collect()
    }
}

/// Read one request, decide one response, write it, close. `Connection: close` on every response
/// because rmcp's transport client is built with `pool_max_idle_per_host(0)` — one request is one
/// accepted socket, which is what keeps the request log a faithful transcript.
async fn serve(mut socket: tokio::net::TcpStream, issuer: &str, sink: &Arc<Mutex<Vec<Recorded>>>) {
    let Some(mut recorded) = read_request(&mut socket).await else {
        return;
    };
    let (status, response) = route(&recorded, issuer);
    recorded.status = status;
    sink.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(recorded);
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

fn ok_json(body: &str) -> (u16, String) {
    (
        200,
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn json_id(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("id").cloned())
        .map_or_else(|| "0".to_string(), |id| id.to_string())
}

fn json_str(body: &str, pointer: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .pointer(pointer)?
        .as_str()
        .map(str::to_string)
}

/// The whole routing table. Two halves on one port: the MCP resource at `/mcp`, gated on the bearer
/// token, and the OAuth authorization server at the origin, gated on nothing.
fn route(recorded: &Recorded, issuer: &str) -> (u16, String) {
    let path = recorded.path.as_str();

    // ── the authorization server ──────────────────────────────────────────────────────────────
    //
    // Both the RFC 8414 root form and the MCP path-suffixed form are served, because RFC 8414 §3.1
    // and the MCP specification disagree about placement and rmcp probes several. Every probe is
    // logged, so a discovery failure is diagnosable from the transcript rather than from a guess.
    if path.starts_with("/.well-known/oauth-protected-resource") {
        return ok_json(&format!(
            "{{\"resource\":\"{issuer}/mcp\",\"authorization_servers\":[\"{issuer}\"],\"scopes_supported\":[\"mcp\"],\"bearer_methods_supported\":[\"header\"]}}"
        ));
    }
    if path.starts_with("/.well-known/oauth-authorization-server")
        || path.starts_with("/.well-known/openid-configuration")
    {
        return ok_json(&format!(
            "{{\"issuer\":\"{issuer}\",\
              \"authorization_endpoint\":\"{issuer}/authorize\",\
              \"token_endpoint\":\"{issuer}/token\",\
              \"registration_endpoint\":\"{issuer}/register\",\
              \"scopes_supported\":[\"mcp\"],\
              \"response_types_supported\":[\"code\"],\
              \"grant_types_supported\":[\"authorization_code\",\"refresh_token\"],\
              \"code_challenge_methods_supported\":[\"S256\"],\
              \"token_endpoint_auth_methods_supported\":[\"none\"]}}"
        ));
    }
    if path.starts_with("/register") {
        // Dynamic client registration. The posted `redirect_uris` are echoed, because
        // `start_auth`'s step-9 stale check reads them back on the next login.
        let redirects = serde_json::from_str::<Value>(&recorded.body)
            .ok()
            .and_then(|value| value.get("redirect_uris").cloned())
            .unwrap_or_else(|| Value::Array(Vec::new()));
        return ok_json(&format!(
            "{{\"client_id\":\"fixture-client\",\"client_id_issued_at\":0,\"redirect_uris\":{redirects},\"token_endpoint_auth_method\":\"none\",\"grant_types\":[\"authorization_code\",\"refresh_token\"],\"response_types\":[\"code\"]}}"
        ));
    }
    if path.starts_with("/token") {
        // Form-encoded, per RFC 6749. `refresh_token` mints a DIFFERENT access token so the refresh
        // test can tell which grant authorized the connect; both are accepted at `/mcp`.
        let minted = if recorded.body.contains("grant_type=refresh_token") {
            REFRESHED_TOKEN
        } else {
            GOOD_TOKEN
        };
        return ok_json(&format!(
            "{{\"access_token\":\"{minted}\",\"token_type\":\"Bearer\",\"expires_in\":3600,\"refresh_token\":\"fixture-refresh\",\"scope\":\"mcp\"}}"
        ));
    }

    // ── the MCP resource ──────────────────────────────────────────────────────────────────────
    if recorded.method == "GET" || recorded.method == "DELETE" {
        // No server-initiated stream and no session teardown. Both are optional in streamable HTTP.
        return (
            405,
            "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string(),
        );
    }

    let authorized = recorded.all("authorization").iter().any(|value| {
        value.trim() == format!("Bearer {GOOD_TOKEN}")
            || value.trim() == format!("Bearer {REFRESHED_TOKEN}")
    });
    if !authorized {
        // THE GATE. `resource_metadata` points the ladder at this same origin, which is what makes
        // journey B's discovery walk reachable from the challenge alone.
        return (
            401,
            format!(
                "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer realm=\"mcp\", resource_metadata=\"{issuer}/.well-known/oauth-protected-resource\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        );
    }

    if recorded.is_method_call("initialize") {
        let id = json_id(&recorded.body);
        // The client's OWN protocol version, echoed rather than dictated — a real server negotiates,
        // and it keeps `rmcp` out of this crate.
        let version = json_str(&recorded.body, "/params/protocolVersion").unwrap_or_default();
        let payload = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"protocolVersion\":\"{version}\",\"capabilities\":{{\"tools\":{{}}}},\"serverInfo\":{{\"name\":\"fixture\",\"version\":\"1\"}},\"instructions\":\"the fixture server speaks\"}}}}"
        );
        return (
            200,
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nmcp-session-id: fixture-session\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            ),
        );
    }
    if recorded.is_method_call("tools/list") {
        let id = json_id(&recorded.body);
        return ok_json(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"tools\":[{{\"name\":\"{REMOTE_TOOL}\",\"description\":\"echo back\",\"inputSchema\":{{\"type\":\"object\",\"properties\":{{\"text\":{{\"type\":\"string\"}}}}}}}}]}}}}"
        ));
    }
    if recorded.is_method_call("tools/call") {
        let id = json_id(&recorded.body);
        let text = json_str(&recorded.body, "/params/arguments/text").unwrap_or_default();
        return ok_json(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"echoed:{text}\"}}],\"isError\":false}}}}"
        ));
    }
    if recorded.body.contains("\"id\":") {
        let id = json_id(&recorded.body);
        return ok_json(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{}}}}"
        ));
    }
    // A notification.
    (
        202,
        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
    )
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Recorded> {
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
    let head_end = loop {
        if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        raw.extend_from_slice(buffer.get(..read)?);
    };
    let head = String::from_utf8_lossy(raw.get(..head_end)?).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let path = target.split('?').next().unwrap_or_default().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    while raw.len() < head_end + length {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(buffer.get(..read)?);
    }
    let body = String::from_utf8_lossy(raw.get(head_end..)?).into_owned();
    Some(Recorded {
        method,
        path,
        headers,
        body,
        status: 0,
    })
}

// =================================================================================================
// 2 · The session harness.
// =================================================================================================

struct Harness {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    /// The vault the session authenticates through — memory-backed, so no keychain is touched and
    /// no D-Bus secret service is required. `McpAuthStore` is `Clone`-shares-state, so this handle
    /// and the runtime's are the same vault.
    store: McpAuthStore,
}

/// A temp `<agent_dir>` with an `mcp.json` at the USER rung naming the fixture, plus a memory vault.
///
/// * `"auth": "oauth"` ⇒ `initial_http_auth_state` is `Explicit`, so the ladder consults the vault
///   **before** the handshake and journey A is one request with no `401` at all.
/// * **No `headers` key.** A `{url, headers:{…}}` entry with no `auth` reads as `Disabled` and the
///   provider is never consulted; and a `custom_headers` `Authorization` makes the ladder drop the
///   OAuth token outright. Either would make this file's subject inert.
/// * `"directTools": true` for the reason `live_tool_call.rs` sets it — direct tools are opt-in, and
///   without it the model reaches the server only through `mcp({tool: …})`.
fn harness(fixture: &HttpMcpFixture, explicit_oauth: bool) -> Harness {
    harness_sharing(fixture, explicit_oauth, None)
}

/// [`harness`], optionally over a vault that already exists.
///
/// `Some(store)` gives a **fresh agent directory** — so a cold metadata cache and therefore a real
/// startup connect — over the **same credential vault**. That is a returning user opening a
/// different project, and it is what makes journey B's phase 7b a startup-connect assertion rather
/// than a lazy-connect one.
/// [`harness`] over a vault whose backend fails **every** operation — the broken keychain.
///
/// Not a hypothetical: `McpAuthStore::new`'s backend is `keyring::Entry`, which answers
/// `NoDefaultStore` on any host with no OS credential store, and that is exactly what the
/// production path builds when nothing is injected. The simulated fault reproduces it
/// deterministically and without depending on the host.
fn harness_with_broken_vault(fixture: &HttpMcpFixture) -> Harness {
    let mut hx = harness(fixture, true);
    hx.store = McpAuthStore::with_backends(
        Arc::new(MemorySecretStore::with_fault(
            cyrup_mcp::credentials::SimulatedFault::Unavailable,
        )),
        Arc::new(MemorySecretStore::new()),
        cyrup_mcp::dirs::McpDirs::new(hx.agent_dir.clone(), hx.cwd.clone()),
        AuthStorageOptions::with_base_dir(hx.agent_dir.join("broken-oauth")),
        Arc::new(|_| None),
    );
    hx
}

fn harness_sharing(
    fixture: &HttpMcpFixture,
    explicit_oauth: bool,
    existing: Option<McpAuthStore>,
) -> Harness {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    let mut entry = serde_json::Map::new();
    entry.insert("url".to_string(), Value::String(fixture.url.clone()));
    if explicit_oauth {
        entry.insert("auth".to_string(), Value::String("oauth".to_string()));
    }
    entry.insert("directTools".to_string(), Value::Bool(true));
    let config = serde_json::json!({ "mcpServers": { SERVER: Value::Object(entry) } });
    std::fs::write(
        agent_dir.join("mcp.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();

    // `with_backends` with a stub environment: no `$MCP_OAUTH_DIR`, no `…_TEST_AUTH_STORE`, no
    // `…_CACHE_DISABLED` — so the base dir is exactly the one named here and the entry cache is on,
    // which is what makes the store-identity question in journey B a real one.
    let store = existing.unwrap_or_else(|| {
        McpAuthStore::with_backends(
            Arc::new(MemorySecretStore::new()),
            Arc::new(MemorySecretStore::new()),
            cyrup_mcp::dirs::McpDirs::new(agent_dir.clone(), cwd.clone()),
            AuthStorageOptions::with_base_dir(tmp.path().join("mcp-oauth")),
            Arc::new(|_| None),
        )
    });

    Harness {
        _tmp: tmp,
        cwd,
        agent_dir,
        store,
    }
}

fn session_config(hx: &Harness) -> SessionConfig {
    let mut cfg = SessionConfig::new(hx.cwd.clone(), hx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// The adapter as `crates/cyrup/src/main.rs` attaches it, with the home and the vault pinned.
fn adapter(hx: &Harness) -> Arc<McpExtension> {
    let dirs = cyrup_mcp::dirs::McpDirs::new(hx.agent_dir.clone(), hx.cwd.clone());
    McpExtension::with_config(dirs, None)
        .with_home(hx.agent_dir.clone())
        .with_auth_store(hx.store.clone())
        // Journey C drives a real login, and the flow hands the authorization URL to a browser
        // after surfacing it. With the production `OpenerLauncher` that is a real `xdg-open` at the
        // fixture's `/authorize` on whoever runs this suite. The URL is surfaced by
        // `on_authorization_url` BEFORE the handoff, which is what the test reads, so a launcher
        // that does nothing costs the proof nothing.
        .with_browser_launcher(Arc::new(cyrup_mcp::oauth::NoopLauncher))
        .into_arc()
}

async fn start_session(hx: &Harness, faux: Arc<FauxProvider>) -> (AgentSession, Arc<McpExtension>) {
    let ext = adapter(hx);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, session_config(hx))
        .with_native_extension(Arc::clone(&ext) as Arc<dyn cyrup_ext::NativeExtension>)
        .build()
        .await
        .unwrap();
    session.bind_extensions().await;
    (session, ext)
}

/// Wait, bounded, for the spawned build to commit — whatever the connect's outcome was.
///
/// Unlike `live_tool_call.rs`'s helper this does **not** require `Connected`: the negative controls
/// need the settled state of a server that did not connect, and a helper that waits for success
/// would turn each of them into a 30-second timeout instead of an assertion.
async fn await_settled(ext: &Arc<McpExtension>) -> Arc<cyrup_mcp::state::McpState> {
    let poll = async {
        loop {
            if let Some(state) = ext.state()
                && ext.proxy_ctx().is_some()
                && ext.init_task().is_none()
            {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), poll)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the session start never committed a generation: either `on_session_start` did not \
             call `start_initialization`, or the memoised `init_task` was never polled"
            )
        })
}

fn status(
    state: &Arc<cyrup_mcp::state::McpState>,
) -> Option<cyrup_mcp::lifecycle::ConnectionStatus> {
    state
        .manager
        .get_connection(SERVER)
        .map(|connection| connection.status())
}

/// One tool result as the transcript holds it, plus the tool arrays the agent handed the model.
#[derive(Debug, Clone)]
struct Answer {
    text: String,
    is_error: bool,
    details: Option<Value>,
    offered: Vec<Vec<String>>,
}

impl Answer {
    fn detail(&self, key: &str) -> Option<&Value> {
        self.details.as_ref()?.get(key)
    }

    fn detail_str(&self, key: &str) -> Option<&str> {
        self.detail(key).and_then(Value::as_str)
    }

    fn was_offered(&self, name: &str) -> bool {
        self.offered
            .iter()
            .any(|turn| turn.iter().any(|tool| tool == name))
    }
}

/// The model, scripted round by round.
///
/// Journey B cannot be one static script: the redirect URL the model has to send back exists only
/// *after* `auth-start` has answered, carrying a CSRF token and a loopback port neither side knows
/// in advance. So the driver runs one `session.prompt` per round, replaces the provider's queue
/// between rounds, and reads each round's tool results out of `session.messages()` — the model's own
/// path, provider -> agent loop -> tool registry -> `ProxyTool` -> `ToolDispatch` -> `McpDispatch`,
/// with every link live or no answer appears at all.
struct Driver {
    session: AgentSession,
    faux: Arc<FauxProvider>,
    /// `ctx.tools` per turn — the MODEL-VISIBLE surface, read off the provider request itself
    /// rather than from any registry view.
    offered: Arc<Mutex<Vec<Vec<String>>>>,
    /// How far into the transcript previous rounds have already been read.
    cursor: usize,
}

impl Driver {
    /// Script one tool call per turn, drive the round, and return one [`Answer`] per call in order.
    async fn round(&mut self, calls: Vec<(&str, Value)>) -> Vec<Answer> {
        let mut steps: Vec<FauxResponseStep> = Vec::new();
        for (tool, args) in &calls {
            let offered = Arc::clone(&self.offered);
            let tool = (*tool).to_string();
            let args = args.clone();
            steps.push(FauxResponseStep::factory(
                move |ctx, _opts, _state, _model| {
                    offered
                        .lock()
                        .unwrap()
                        .push(ctx.tools.iter().map(|t| t.name.clone()).collect());
                    faux_assistant_message(
                        vec![faux_tool_call(&tool, args.clone())],
                        StopReason::ToolUse,
                    )
                },
            ));
        }
        {
            let offered = Arc::clone(&self.offered);
            steps.push(FauxResponseStep::factory(
                move |ctx, _opts, _state, _model| {
                    offered
                        .lock()
                        .unwrap()
                        .push(ctx.tools.iter().map(|t| t.name.clone()).collect());
                    faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
                },
            ));
        }
        self.faux.set_response_steps(steps);

        let _stream = self
            .session
            .prompt("use the mcp surface")
            .await
            .expect("prompt accepted");
        self.session.wait_for_idle().await;

        let captured = self.offered.lock().unwrap().clone();
        let messages = self.session.messages().await;
        let mut answers = Vec::new();
        for (tool, _) in &calls {
            let found = messages
                .iter()
                .enumerate()
                .skip(self.cursor)
                .find_map(|(index, message)| match message {
                    Message::ToolResult {
                        tool_name,
                        content,
                        is_error,
                        details,
                        ..
                    } if tool_name == tool => Some((
                        index,
                        Answer {
                            text: content
                                .iter()
                                .filter_map(|block| match block {
                                    cyrup_core::Content::Text { text, .. } => {
                                        Some(text.to_string())
                                    }
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n"),
                            is_error: *is_error,
                            details: details.clone(),
                            offered: captured.clone(),
                        },
                    )),
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!("the scripted `{tool}` call landed no tool result in the transcript")
                });
            self.cursor = found.0 + 1;
            answers.push(found.1);
        }
        answers
    }

    /// One call, one answer.
    async fn call(&mut self, tool: &str, args: Value) -> Answer {
        self.round(vec![(tool, args)]).await.remove(0)
    }
}

/// Build the session with a provider whose queue is filled per round by [`Driver::round`].
async fn driver(hx: &Harness) -> (Driver, Arc<McpExtension>) {
    let faux = Arc::new(FauxProvider::new());
    // A closing text turn, so a session awaited before the first round still settles.
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ready")],
        StopReason::Stop,
    )]);
    let (session, ext) = start_session(hx, Arc::clone(&faux)).await;
    (
        Driver {
            session,
            faux,
            offered: Arc::new(Mutex::new(Vec::new())),
            cursor: 0,
        },
        ext,
    )
}

/// The request log as one line per request — the diagnostic every count assertion prints, because
/// a bare `left: 2 right: 1` on a wire-level count says nothing about which request appeared.
fn request_summary(fixture: &HttpMcpFixture) -> Vec<String> {
    fixture
        .requests()
        .iter()
        .map(|recorded| {
            format!(
                "{} {} -> {} auth={:?} :: {}",
                recorded.method,
                recorded.path,
                recorded.status,
                recorded.all("authorization"),
                recorded.body.chars().take(140).collect::<String>()
            )
        })
        .collect()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Put a credential in the vault through the v2.25.0 plaintext entry the store imports on its first
/// read — the one route that needs no `rmcp` type to build a `StoredCredentials`.
///
/// Three details are load-bearing: `clientInfo.clientId` is mandatory (`translate_legacy_entry`
/// drops tokens with no client id rather than fabricating one); `serverUrl` must equal the entry's
/// URL byte for byte (`get_auth_for_url` is fail-closed and compares with string equality); and the
/// first migrating read MOVES the record into the backend and deletes the file.
fn seed_credential(store: &McpAuthStore, url: &str, token: &str, expires_at: i64) {
    let path = store.auth_entry_file_path(SERVER);
    std::fs::create_dir_all(path.parent().expect("a server directory")).expect("mkdir");
    let body = serde_json::json!({
        "tokens": {
            "accessToken": token,
            "refreshToken": "fixture-refresh",
            "expiresAt": expires_at,
            "scope": "mcp",
        },
        "clientInfo": { "clientId": "fixture-client" },
        "serverUrl": url,
    });
    std::fs::write(&path, serde_json::to_vec(&body).expect("json")).expect("write");
}

/// Seed a credential and then **migrate it into the backend**, which is what makes an *expired*
/// credential expressible at all.
///
/// `translate_legacy_entry` computes `expires_in = max(0, floor(expiresAt - now))` and stamps
/// `token_received_at = now` **at translation time**, and translation happens on the entry's first
/// read. So a legacy file alone can never present as expired: whatever `expiresAt` it names, the
/// record the reader materialises has `expires_at == the instant it was read`, and
/// `is_token_expired`'s `expires_at < now` is false at that instant.
///
/// Reading it here fixes `token_received_at` to *this* moment and moves the record into the backend
/// (the file is deleted — that is the migration). Every later read returns the same fixed instant,
/// so once a second of wall clock has passed the credential really is expired, by the store's own
/// arithmetic and with no clock injection anywhere.
async fn seed_expired_credential(store: &McpAuthStore, url: &str, token: &str) {
    seed_credential(store, url, token, now_secs());
    store
        .auth_entry_async(SERVER)
        .await
        .expect("the migrating read");
    // Past the second boundary the migration stamped, so `expires_at < now_secs()` holds.
    tokio::time::sleep(Duration::from_millis(1_200)).await;
}

/// The vault's own answer to "is there a usable **token** for this server and URL?".
///
/// `inspect_mcp_oauth_tokens_for_url` rather than `inspect_auth_for_url`, and the difference
/// is load-bearing: `start_auth` writes the dynamic-registration record into the vault **before** any
/// browser round trip, so an entry exists for a server that has never completed a login. The
/// entry-level accessor answers `Present` for that; the token-level one collapses a
/// registration-only entry to `Absent`, which is the question every assertion in this file is
/// actually asking.
///
/// Never refreshes, and never prints what it read.
async fn vault_token(store: &McpAuthStore, url: &str) -> cyrup_mcp::oauth::McpOAuthTokenStatus {
    let storage: Arc<dyn cyrup_mcp::oauth::McpOAuthStorage> = Arc::new(store.clone());
    cyrup_mcp::oauth::inspect_mcp_oauth_tokens_for_url(SERVER, url, &storage).await
}

/// [`vault_token`], as a yes/no.
async fn vault_holds(store: &McpAuthStore, url: &str) -> bool {
    matches!(
        vault_token(store, url).await,
        cyrup_mcp::oauth::McpOAuthTokenStatus::Present(_)
    )
}

/// No loopback listener was ever bound and no authorization is pending — the executable form of "no
/// prompt, no browser".
///
/// Both accessors are process-global (`oauth.rs`'s shared callback server), so they are asserted
/// only where this file's tests do not run an OAuth flow concurrently. `cargo nextest` runs each
/// test in its own process, which is what makes that safe.
async fn no_oauth_flow_ran() {
    assert!(
        !cyrup_mcp::oauth::is_callback_server_running().await,
        "a loopback callback listener was bound — an OAuth flow ran where none should have"
    );
    assert_eq!(
        cyrup_mcp::oauth::pending_callback_count(),
        0,
        "an authorization is pending — an OAuth flow ran where none should have"
    );
}

// =================================================================================================
// 3 · JOURNEY A — the returning user.
// =================================================================================================

/// **A credential already in the vault connects an HTTP MCP server with no prompt, and its tools
/// reach the model.**
///
/// This is the journey the change set exists for, and it is the one that was provably broken: with
/// `ConnectionBuilder::new`'s default `NoStoredCredentials` — `authorize` is literally
/// `Ok(None)` — the handshake carries no `Authorization`, the fixture answers `401`, and
/// `on_unauthorized(Explicit)` goes straight to `needs-auth` with no retry. Every assertion below
/// fails in that world, starting with assertion 2.
///
/// The chain under test is the production one end to end: `bind_extensions()` → `SessionStart` →
/// `on_session_start` → `start_initialization` → `initialize_mcp`, whose ONE `ConnectionBuilder` now
/// carries `StoredCredentialAuth` over the generation's `McpAuthStore` → `connect_http_client`'s
/// ladder reads the vault on attempt one → the handshake, `tools/list`, the metadata build, the
/// commit tail, the surface sync — and finally a model-issued `fixture_echo`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stored_credential_connects_an_http_server_with_no_prompt() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, true);
    seed_credential(&hx.store, &fixture.url, GOOD_TOKEN, now_secs() + 3600);

    let (mut model, ext) = driver(&hx).await;
    let state = await_settled(&ext).await;

    // (1) The ladder produced a LIVE connection, not `needs-auth`.
    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::Connected),
        "the stored credential connected the server; failure message: {:?}",
        state
            .failure_messages
            .lock()
            .ok()
            .and_then(|m| m.get(SERVER).cloned())
    );

    // (2) THE ASSERTION THAT FAILS WITHOUT THE PROVIDER. `auth: "oauth"` is the explicit arm, so the
    //     vault is read BEFORE the handshake and a correct implementation never draws a 401 at all.
    assert!(
        fixture.unauthorized().is_empty(),
        "a returning user must not be challenged: {:#?}",
        fixture.unauthorized()
    );
    assert_eq!(
        fixture.initializes().len(),
        1,
        "one handshake, no retry: {:#?}",
        fixture.requests()
    );

    // (3) Exactly ONE Authorization header, carrying the stored token — on every request the
    //     connection made, not merely the handshake. Two values would be the header-collision
    //     regression `http_attempt`'s custom-header clause exists to prevent.
    let mcp_requests: Vec<Recorded> = fixture
        .requests()
        .into_iter()
        .filter(|r| r.path == "/mcp")
        .collect();
    assert!(!mcp_requests.is_empty());
    for recorded in &mcp_requests {
        assert_eq!(
            recorded.all("authorization"),
            vec![format!("Bearer {GOOD_TOKEN}")],
            "every request on the connection carried exactly one stored credential: {recorded:#?}"
        );
    }
    assert!(
        mcp_requests.iter().any(|r| r.is_method_call("tools/list")),
        "discovery ran, so the token authorized more than the handshake"
    );

    // (4) NO PROMPT, NO BROWSER, NO LISTENER — and the OAuth discovery endpoints were never touched,
    //     which is the fixture's own view of "nothing went looking for a login".
    no_oauth_flow_ran().await;
    assert!(
        fixture
            .requests()
            .iter()
            .all(|r| r.path.starts_with("/mcp")),
        "a returning user's session touches only the MCP endpoint: {:#?}",
        fixture
            .requests()
            .iter()
            .map(|r| r.path.clone())
            .collect::<Vec<_>>()
    );

    // (5) The server's catalog reached the model, by name, with the server's own description and
    //     schema — none of which exists on this side of the socket.
    // Scoped, so the `std::sync::MutexGuard` is provably gone before the `await` below.
    {
        let metadata = state.tool_metadata.lock().unwrap();
        let entries = metadata
            .get(SERVER)
            .unwrap_or_else(|| panic!("`tool_metadata[\"{SERVER}\"]`"));
        assert_eq!(
            entries.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec![DIRECT_TOOL]
        );
        assert_eq!(entries[0].original_name, REMOTE_TOOL);
        assert_eq!(entries[0].description, "echo back");
    }

    // (6) THE DELIVERABLE. The model called the tool and the SERVER's own bytes came back.
    let answer = model
        .call(DIRECT_TOOL, serde_json::json!({ "text": "pong" }))
        .await;
    assert!(
        answer.was_offered(DIRECT_TOOL),
        "the tool reached the model's array: {:?}",
        answer.offered
    );
    assert!(
        answer.text.contains(SERVER_ANSWER),
        "the server's real `tools/call` result reached the model over an OAuth-authorized HTTP \
         transport: {answer:#?}"
    );
    assert!(!answer.is_error, "{answer:#?}");
    assert_eq!(answer.detail("error"), None, "{answer:#?}");
    assert_eq!(
        answer.detail_str("tool"),
        Some(REMOTE_TOOL),
        "the WIRE name reached the server"
    );

    // …and the tool call itself was authorized too — the `tools/call` POST is the last request and
    // it carries the same one header.
    let call = fixture
        .requests()
        .into_iter()
        .find(|r| r.is_method_call("tools/call"))
        .expect("the model's call reached the server");
    assert_eq!(
        call.all("authorization"),
        vec![format!("Bearer {GOOD_TOKEN}")]
    );
    assert!(
        fixture.unauthorized().is_empty(),
        "still no 401, after the call: {:#?}",
        fixture.unauthorized()
    );

    model.session.dispose("quit").await;
}

/// **The negative control: no credential ⇒ `needs-auth`, and the user is told exactly what to run.**
///
/// The identical fixture and the identical config, with the vault left empty. Without this, the test
/// above proves only that *something* connected — it could not distinguish a working provider from a
/// fixture that never checked.
///
/// `auth: "oauth"` is the explicit arm, so there is **one** `initialize` and no retry:
/// `on_unauthorized(Explicit)` answers `NeedsAuth` immediately. Getting that count backwards is the
/// likeliest source of a "flaky" HTTP OAuth test, so it is pinned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unauthenticated_http_server_ends_at_needs_auth() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, true);

    let (model, ext) = driver(&hx).await;
    let state = await_settled(&ext).await;

    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::NeedsAuth),
        "an empty vault ends at needs-auth, not at connected and not at a hard error"
    );
    // TWO handshakes, and neither is the ladder retrying: `on_unauthorized(Explicit)` answers
    // `NeedsAuth` with no retry at all. The second connect is `initialize_mcp`'s **direct-tools
    // bootstrap** (`init.ts:382`), which re-connects every server that is configured for direct
    // tools and still has no cache entry — and §11's needs-auth arm records no connection, so this
    // server is still "missing" when that pass runs. Pinned at 2 rather than waved at: a 3 would
    // mean a real retry appeared, and a 1 would mean the direct-tools bootstrap stopped running.
    assert_eq!(
        fixture.initializes().len(),
        2,
        "the startup pass and the direct-tools bootstrap, each once, neither retrying: {:#?}",
        request_summary(&fixture)
    );
    assert_eq!(
        fixture.unauthorized().len(),
        2,
        "every attempt was challenged"
    );
    for recorded in fixture.initializes() {
        assert_eq!(
            recorded.header("authorization"),
            None,
            "an empty vault attaches nothing, on either attempt"
        );
    }
    // The MCP endpoint is all that was touched: `needs-auth` must not open a browser or walk the
    // discovery endpoints from inside a session start (MCP-316's fence).
    assert!(
        fixture.requests().iter().all(|r| r.path == "/mcp"),
        "no discovery, no registration, no token exchange: {:#?}",
        request_summary(&fixture)
    );
    no_oauth_flow_ran().await;

    // The byte-exact line the startup pass records — the one the user is told to run. Reworded, it
    // is a support burden; missing, journey B has no entry point.
    assert_eq!(
        state
            .failure_messages
            .lock()
            .ok()
            .and_then(|m| m.get(SERVER).cloned()),
        Some(format!(
            "OAuth authentication required. Run /mcp-auth {SERVER}."
        )),
    );

    // No tool from an unauthenticated server reaches the model.
    assert!(
        state
            .tool_metadata
            .lock()
            .unwrap()
            .get(SERVER)
            .is_none_or(Vec::is_empty),
        "a needs-auth server contributes no catalog"
    );
    model.session.dispose("quit").await;
}

/// **A wrong token fails LOUDLY — it does not degrade to a connected-but-empty server.**
///
/// This is the control that keeps every "the tools reached the model" assertion in this file
/// honest. The failure it rules out is specific and plausible: a fixture that answered `200` to an
/// unauthorized handshake, or a runtime that recorded `Connected` for a connection whose discovery
/// never ran, would give a server that is *present* on the surface with an empty catalog — and every
/// symptom would read as a discovery bug rather than an auth bug.
///
/// The credential here is well-formed and URL-bound; only its **value** is wrong, so the provider
/// hands the ladder a token and the server rejects it. That is the shape of an expired or revoked
/// credential in the field.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_stored_token_fails_loudly_rather_than_connecting_empty() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, true);
    seed_credential(
        &hx.store,
        &fixture.url,
        "not-the-right-token",
        now_secs() + 3600,
    );

    let (mut model, ext) = driver(&hx).await;
    let state = await_settled(&ext).await;

    // The token WAS presented — this is not the empty-vault case. Two attempts, for the reason
    // [`an_unauthenticated_http_server_ends_at_needs_auth`] spells out, and BOTH carried the stored
    // value: MCP-116's cache eviction fires once and the re-read finds the same wrong credential.
    assert_eq!(
        fixture.initializes().len(),
        2,
        "{:#?}",
        request_summary(&fixture)
    );
    for recorded in fixture.initializes() {
        assert_eq!(
            recorded.all("authorization"),
            vec!["Bearer not-the-right-token"],
            "the provider read the vault and presented what it found"
        );
    }
    // …and the server rejected it, every time.
    assert_eq!(fixture.unauthorized().len(), 2);

    // NOT connected. The whole point.
    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::NeedsAuth),
        "a rejected credential is `needs-auth`, never `Connected`"
    );
    assert!(
        state
            .tool_metadata
            .lock()
            .unwrap()
            .get(SERVER)
            .is_none_or(Vec::is_empty),
        "no catalog — and, crucially, the server is not sitting on the surface pretending to have one"
    );

    // And the model is TOLD, in the status the gateway renders. A silent degradation would show the
    // server as connected with zero tools; this shows it as needing authentication.
    let answer = model.call(PROXY_TOOL, serde_json::json!({})).await;
    assert_eq!(answer.detail_str("mode"), Some("status"), "{answer:#?}");
    let servers = answer
        .detail("servers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let row = servers
        .iter()
        .find(|row| row.get("name").and_then(Value::as_str) == Some(SERVER))
        .unwrap_or_else(|| panic!("the status names the server: {answer:#?}"));
    assert_eq!(
        row.get("status").and_then(Value::as_str),
        Some("needs-auth"),
        "the model is told the server needs authentication, not that it connected: {answer:#?}"
    );
    assert_eq!(
        answer.detail("connectedCount"),
        Some(&serde_json::json!(0)),
        "…and that nothing is connected: {answer:#?}"
    );
    assert!(
        !answer.was_offered(DIRECT_TOOL),
        "no direct tool from a server that never authenticated: {:?}",
        answer.offered
    );
    model.session.dispose("quit").await;
}

/// **The implicit arm: `auth` omitted ⇒ the vault is not touched until the server proves it must
/// be.** One `401`, then one authorized retry. Two requests, still no prompt.
///
/// This is the shape that keeps a non-OAuth HTTP server from ever reaching the keychain, and it is
/// upstream's own: `initial_http_auth_state` answers `ImplicitDeferred` for `{url}` with no `auth`
/// key, so `authorize` is not called on attempt one. It is here because "journey A works" must be
/// true for both configurations, and because the request counts differ — 1 for explicit, 2 for
/// implicit — in a way that reads as a bug if you only ever saw one of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_implicit_oauth_server_retries_once_with_the_stored_token() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, false);
    seed_credential(&hx.store, &fixture.url, GOOD_TOKEN, now_secs() + 3600);

    let (model, ext) = driver(&hx).await;
    let state = await_settled(&ext).await;

    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::Connected),
        "the retry carried the stored token; failure: {:?}",
        state
            .failure_messages
            .lock()
            .ok()
            .and_then(|m| m.get(SERVER).cloned())
    );
    let initializes = fixture.initializes();
    assert_eq!(
        initializes.len(),
        2,
        "one 401, then one authorized attempt: {:#?}",
        fixture.requests()
    );
    assert_eq!(
        initializes[0].header("authorization"),
        None,
        "deferred: nothing was read from the vault before the 401"
    );
    assert_eq!(
        initializes[1].all("authorization"),
        vec![format!("Bearer {GOOD_TOKEN}")],
        "the retry carries the stored credential"
    );
    assert_eq!(
        fixture.unauthorized().len(),
        1,
        "exactly one challenge, and it was answered"
    );
    no_oauth_flow_ran().await;
    model.session.dispose("quit").await;
}

/// **An expired credential with a refresh token is refreshed at connect time — still no prompt.**
///
/// `StoredCredentialAuth` goes through `oauth::get_valid_token` rather than a bare
/// `get_auth_for_url`, and this is the difference that buys: rmcp's streamable-HTTP transport takes
/// a *static* bearer, so there is no SDK auth loop behind `auth_header` and the provider is the only
/// place a refresh can happen. Without it an expired-but-refreshable credential would `401` forever
/// and journey A would silently become journey B on every single session.
///
/// The fixture mints a **different** access token for a `grant_type=refresh_token` exchange, so the
/// header on the handshake names which grant authorized it — a fixture that minted the same string
/// for both would prove nothing here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_credential_is_refreshed_at_connect_time() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, true);
    seed_expired_credential(&hx.store, &fixture.url, GOOD_TOKEN).await;

    let (model, ext) = driver(&hx).await;
    let state = await_settled(&ext).await;

    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::Connected),
        "an expired credential with a refresh token still connects; failure: {:?}",
        state
            .failure_messages
            .lock()
            .ok()
            .and_then(|m| m.get(SERVER).cloned())
    );
    let token_hits = fixture.hits("/token");
    assert_eq!(
        token_hits.len(),
        1,
        "exactly one refresh exchange: {:#?}",
        fixture.requests()
    );
    assert!(
        token_hits[0].body.contains("grant_type=refresh_token"),
        "…and it was a refresh, not a fresh authorization: {:#?}",
        token_hits[0]
    );
    assert!(
        fixture.unauthorized().is_empty(),
        "no 401: the refreshed token was used from attempt one"
    );
    assert_eq!(
        fixture.initializes()[0].all("authorization"),
        vec![format!("Bearer {REFRESHED_TOKEN}")],
        "the handshake carried the REFRESHED token, not the expired one"
    );
    // No browser: a refresh is not an authorization.
    no_oauth_flow_ran().await;
    // The vault now holds the refreshed credential, so the NEXT session is an ordinary journey A.
    assert!(vault_holds(&hx.store, &fixture.url).await);
    model.session.dispose("quit").await;
}

// =================================================================================================
// 4 · JOURNEY B — the first login, and journey A on the next run.
// =================================================================================================

/// Percent-decode a query-string value. `cyrup-it` has no `url` dependency and this fixture is not
/// the place to add one; the authorization URL's `state` and `redirect_uri` are the only two values
/// that need decoding and both are ASCII.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The decoded query parameters of a URL, in order.
fn query_params(url: &str) -> HashMap<String, String> {
    url.split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (percent_decode(key), percent_decode(value)))
        .collect()
}

/// **The test plays the browser.** Given the authorization URL the flow produced, build the redirect
/// the authorization server would send back after the user approved.
///
/// The `redirect_uri` is read **out of the authorization URL** rather than assumed: with no
/// `oauth.clientId` and no `oauth.redirectUri` the loopback listener binds port **0** and adopts
/// whatever the OS gives it, and the URI is only fixed after that bind. Anything that hardcodes a
/// port here is testing a coincidence.
fn browser_redirect(authorization_url: &str) -> String {
    let params = query_params(authorization_url);
    let redirect_uri = params.get("redirect_uri").unwrap_or_else(|| {
        panic!("the authorization URL carries a redirect_uri: {authorization_url}")
    });
    let state = params.get("state").unwrap_or_else(|| {
        panic!("the authorization URL carries a CSRF state: {authorization_url}")
    });
    assert!(
        params.contains_key("code_challenge"),
        "the flow is PKCE — a missing challenge would mean the exchange below proves less than it \
         appears to: {authorization_url}"
    );
    let separator = if redirect_uri.contains('?') { '&' } else { '?' };
    format!("{redirect_uri}{separator}code={AUTH_CODE}&state={state}")
}

/// **The first login, driven entirely by the model, and journey A on the very next run.**
///
/// No human clicks anything and no browser is opened: the test drives the copy-paste protocol that
/// exists precisely for headless sessions (`format_manual_auth_instructions`). Everything the flow
/// needs from "the browser" — the authorization code and the CSRF state — the test reads off the
/// authorization URL the flow itself produced.
///
/// The seven phases, and what each one rules out:
///
/// 1. an empty vault ⇒ `needs-auth`, so the login below is a real first login;
/// 2. `mcp({action:"auth-start"})` ⇒ an authorization URL, which means rmcp walked the fixture's
///    `.well-known` metadata and dynamically registered a client against `/register`;
/// 3. the test plays the browser, using the port the loopback listener actually took;
/// 4. `mcp({action:"auth-complete"})` ⇒ the fixture's `/token` is hit exactly once with
///    `grant_type=authorization_code`, and the credential lands in the vault;
/// 5. `mcp({connect})` ⇒ `details.mode == "list"`, connected, with the minted token on the wire;
/// 6. the model calls `fixture_echo` and the server's own bytes come back;
/// 7. **a second session over the same vault is journey A** — connected with zero new `401`s and no
///    listener bound. That is the deliverable's own sentence, and it is the strongest assertion
///    available here because the credential under test was written by the real flow rather than
///    seeded by the test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_login_stores_a_token_and_the_next_session_connects_silently() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, true);

    // ── phase 1 — no credential, so `needs-auth` ──────────────────────────────────────────────
    let (mut model, ext) = driver(&hx).await;
    let state = await_settled(&ext).await;
    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::NeedsAuth),
        "the first session has nothing to authenticate with"
    );
    assert!(
        !vault_holds(&hx.store, &fixture.url).await,
        "the vault starts empty"
    );
    let after_startup = fixture.requests().len();

    // ── phase 2 — the model starts the manual flow ────────────────────────────────────────────
    let started = model
        .call(
            PROXY_TOOL,
            serde_json::json!({ "action": "auth-start", "server": SERVER }),
        )
        .await;
    assert_eq!(
        started.detail_str("mode"),
        Some("auth-start"),
        "{started:#?}"
    );
    assert_eq!(
        started.detail("error"),
        None,
        "the flow started: {started:#?}"
    );
    let authorization_url = started
        .detail_str("authorizationUrl")
        .unwrap_or_else(|| panic!("an authorization URL: {started:#?}"))
        .to_string();
    assert!(
        authorization_url.starts_with(&format!("{}/authorize", fixture.issuer)),
        "the URL points at the fixture's own authorization endpoint, which it learned by walking \
         the fixture's metadata: {authorization_url}"
    );
    // The discovery walk and the dynamic registration really happened, on the wire.
    assert!(
        fixture
            .requests()
            .iter()
            .skip(after_startup)
            .any(|r| r.path.starts_with("/.well-known/")),
        "the flow discovered the authorization server: {:#?}",
        request_summary(&fixture)
    );
    assert_eq!(
        fixture.hits("/register").len(),
        1,
        "one dynamic client registration: {:#?}",
        request_summary(&fixture)
    );
    // The instructions the model is shown name the paste-back call, not a browser.
    assert!(
        started.text.contains("auth-complete") && started.text.contains(&authorization_url),
        "the model is told how to finish without a browser: {started:#?}"
    );

    // ── phase 3 — the test is the browser ─────────────────────────────────────────────────────
    let redirect = browser_redirect(&authorization_url);

    // ── phase 4 — the model pastes the redirect back ──────────────────────────────────────────
    let completed = model
        .call(
            PROXY_TOOL,
            serde_json::json!({
                "action": "auth-complete",
                "server": SERVER,
                "args": { "redirectUrl": redirect },
            }),
        )
        .await;
    assert_eq!(
        completed.detail("error"),
        None,
        "the exchange succeeded: {completed:#?}"
    );
    assert_eq!(
        completed.detail("authenticated"),
        Some(&serde_json::json!(true)),
        "{completed:#?}"
    );
    let token_hits = fixture.hits("/token");
    assert_eq!(
        token_hits.len(),
        1,
        "one token exchange: {:#?}",
        request_summary(&fixture)
    );
    assert!(
        token_hits[0].body.contains("grant_type=authorization_code"),
        "…and it was the authorization-code grant: {:#?}",
        token_hits[0]
    );
    assert!(
        token_hits[0].body.contains(&format!("code={AUTH_CODE}")),
        "…carrying the code the browser handed back: {:#?}",
        token_hits[0]
    );
    assert!(
        token_hits[0].body.contains("code_verifier="),
        "…and the PKCE verifier: {:#?}",
        token_hits[0]
    );

    // THE VAULT NOW HOLDS A CREDENTIAL — written by the real flow, through the SAME store instance
    // the connect ladder reads. That store identity is what `McpServerManager::set_auth_store`
    // exists for: with a per-operation store the token would land in a vault whose cache the ladder
    // never consults.
    match vault_token(&hx.store, &fixture.url).await {
        cyrup_mcp::oauth::McpOAuthTokenStatus::Present(tokens) => {
            // Compared, never printed. This is the fixture's own minted value, so the vault holds
            // what `/token` issued rather than anything this side fabricated.
            assert!(
                tokens.access_token == GOOD_TOKEN,
                "the vault holds the token the fixture's /token endpoint minted"
            );
            assert!(
                tokens.refresh_token.is_some(),
                "…and the refresh token beside it"
            );
        }
        other => panic!("the token landed in the generation's vault, got {other:?}"),
    }

    // ── phase 5 — reconnect with the new token ────────────────────────────────────────────────
    let connected = model
        .call(PROXY_TOOL, serde_json::json!({ "connect": SERVER }))
        .await;
    assert_eq!(
        connected.detail_str("mode"),
        Some("list"),
        "a successful connect reports the server's catalog: {connected:#?}"
    );
    assert_eq!(connected.detail("error"), None, "{connected:#?}");
    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::Connected),
        "the retry with the minted token connected"
    );
    let minted = fixture
        .requests()
        .into_iter()
        .rfind(|r| r.is_method_call("initialize"))
        .expect("a handshake");
    assert_eq!(
        minted.all("authorization"),
        vec![format!("Bearer {GOOD_TOKEN}")],
        "the handshake carried the token the fixture's own /token endpoint minted"
    );

    // ── phase 6 — the model calls the server's tool ───────────────────────────────────────────
    let echoed = model
        .call(DIRECT_TOOL, serde_json::json!({ "text": "pong" }))
        .await;
    assert!(
        echoed.text.contains(SERVER_ANSWER),
        "the server's own bytes, over a transport authorized by a token this test logged in for: \
         {echoed:#?}"
    );
    assert!(!echoed.is_error, "{echoed:#?}");

    // Tear the first session down through the production path, so `shutdown_oauth` releases the
    // shared callback listener. Asserted, because a leaked listener is `oauth.rs`'s named hazard.
    model.session.dispose("quit").await;
    assert!(
        !cyrup_mcp::oauth::is_callback_server_running().await,
        "the session shutdown released the loopback listener"
    );
    assert_eq!(cyrup_mcp::oauth::pending_callback_count(), 0);

    // ── phase 7a — THE NEXT LAUNCH, literally: same agent dir, same vault ─────────────────────
    //
    // Nothing is seeded. The only credential in play is the one phase 4 wrote through the real
    // flow. The metadata cache is warm — the first session wrote it — so this run does **not**
    // startup-connect: it registers `fixture_echo` from the cache with no server contacted, and the
    // connect happens on first use. That is production's own shape for a second launch, and the
    // question this phase answers is whether the stored credential carries it.
    let baseline_401 = fixture.unauthorized().len();
    let (mut next, next_ext) = driver(&hx).await;
    let next_state = await_settled(&next_ext).await;
    assert!(
        status(&next_state).is_none(),
        "a warm metadata cache means no startup connect — the surface comes from the cache"
    );
    let again = next
        .call(DIRECT_TOOL, serde_json::json!({ "text": "pong" }))
        .await;
    assert!(
        again.was_offered(DIRECT_TOOL),
        "the cached catalog put the server's tool on the model's surface with nothing contacted: \
         {:?}",
        again.offered
    );
    assert!(
        again.text.contains(SERVER_ANSWER),
        "the lazy connect used the stored credential and the server answered: {again:#?}"
    );
    assert_eq!(
        status(&next_state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::Connected),
        "…and the connection is live"
    );
    assert_eq!(
        fixture.unauthorized().len(),
        baseline_401,
        "…with NO new 401 at all — the returning user is never challenged: {:#?}",
        request_summary(&fixture)
    );
    no_oauth_flow_ran().await;
    next.session.dispose("quit").await;

    // ── phase 7b — the same credential through a STARTUP connect ──────────────────────────────
    //
    // A fresh agent directory over the same vault: a cold metadata cache, so `bootstrap_all` is set
    // and the startup pass connects the server before the model has asked for anything. This is the
    // deliverable's sentence in its strictest form — "the session starts, the server connects
    // without any prompt" — against a credential no test seeded.
    let baseline_401 = fixture.unauthorized().len();
    let cold = harness_sharing(&fixture, true, Some(hx.store.clone()));
    let (mut cold_model, cold_ext) = driver(&cold).await;
    let cold_state = await_settled(&cold_ext).await;
    assert_eq!(
        status(&cold_state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::Connected),
        "a cold start on the stored credential connects at session start; failure: {:?}",
        cold_state
            .failure_messages
            .lock()
            .ok()
            .and_then(|m| m.get(SERVER).cloned())
    );
    assert_eq!(
        fixture.unauthorized().len(),
        baseline_401,
        "…on the first attempt, with no challenge: {:#?}",
        request_summary(&fixture)
    );
    no_oauth_flow_ran().await;
    let cold_answer = cold_model
        .call(DIRECT_TOOL, serde_json::json!({ "text": "pong" }))
        .await;
    assert!(cold_answer.text.contains(SERVER_ANSWER), "{cold_answer:#?}");

    cold_model.session.dispose("quit").await;
}

/// **The paste path is not a rubber stamp: a redirect carrying the wrong CSRF state is refused, and
/// no token is exchanged.**
///
/// Journey B's proof would be worth much less without this. `auth-complete` takes a string the model
/// was handed by a human who copied it out of a browser address bar, so "the flow accepted it" has
/// to mean something. Here the code is the one the fixture would honour and only the `state` is
/// wrong — the shape of a cross-session or cross-server paste — and the flow must refuse it
/// **before** the token endpoint is touched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redirect_with_the_wrong_state_is_refused_and_no_token_is_exchanged() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, true);

    let (mut model, ext) = driver(&hx).await;
    let state = await_settled(&ext).await;
    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::NeedsAuth)
    );

    let started = model
        .call(
            PROXY_TOOL,
            serde_json::json!({ "action": "auth-start", "server": SERVER }),
        )
        .await;
    let authorization_url = started
        .detail_str("authorizationUrl")
        .unwrap_or_else(|| panic!("an authorization URL: {started:#?}"))
        .to_string();
    let redirect_uri = query_params(&authorization_url)
        .get("redirect_uri")
        .cloned()
        .expect("a redirect_uri");
    let tampered = format!("{redirect_uri}?code={AUTH_CODE}&state=not-the-state-we-minted");

    let completed = model
        .call(
            PROXY_TOOL,
            serde_json::json!({
                "action": "auth-complete",
                "server": SERVER,
                "args": { "redirectUrl": tampered },
            }),
        )
        .await;

    assert!(
        completed.detail("error").is_some(),
        "a mismatched state must be refused: {completed:#?}"
    );
    assert_ne!(
        completed.detail("authenticated"),
        Some(&serde_json::json!(true)),
        "{completed:#?}"
    );
    assert!(
        fixture.hits("/token").is_empty(),
        "the refusal happens BEFORE the token endpoint is touched: {:#?}",
        request_summary(&fixture)
    );
    assert!(
        !vault_holds(&hx.store, &fixture.url).await,
        "and nothing was written to the vault"
    );
    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::NeedsAuth),
        "the server is still unauthenticated"
    );

    model.session.dispose("quit").await;
    assert!(
        !cyrup_mcp::oauth::is_callback_server_running().await,
        "the session shutdown released the loopback listener the failed flow bound"
    );
}

/// **An unreadable credential vault fails the connect LOUDLY — it is not reported as "you have
/// never logged in".**
///
/// The distinction is the whole design of `StoredCredentialAuth`'s error handling, and getting it
/// wrong is silent and permanent: if a broken vault answered `Ok(None)`, the runtime would record
/// `needs-auth`, the user would be told to run `/mcp-auth`, the flow would write a credential that
/// cannot be read back, and the loop would never terminate.
///
/// So the failure here must be a **connect failure carrying the store's own message**, with the
/// server absent from the connection map rather than sitting in it as `needs-auth` — and with
/// nothing sent on the wire, because the vault is consulted before the first byte.
///
/// This is also the shape production takes on a host with no OS credential store at all: the
/// keyring backend answers `NoDefaultStore` on the first read, and this is what the user sees.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_vault_fails_the_connect_rather_than_asking_for_a_login() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness_with_broken_vault(&fixture);

    let (model, ext) = driver(&hx).await;
    let state = await_settled(&ext).await;

    let message = state
        .failure_messages
        .lock()
        .ok()
        .and_then(|messages| messages.get(SERVER).cloned())
        .unwrap_or_else(|| panic!("a broken vault records a failure"));
    assert!(
        message.to_lowercase().contains("credential store"),
        "the recorded failure names the credential store, not a missing login: {message}"
    );
    assert_ne!(
        message,
        format!("OAuth authentication required. Run /mcp-auth {SERVER}."),
        "a broken vault must NOT be reported as a missing login"
    );
    assert_ne!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::NeedsAuth),
        "…and must not be recorded as `needs-auth`"
    );
    assert_ne!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::Connected)
    );
    assert!(
        fixture.requests().is_empty(),
        "nothing was sent: the vault is read before the first byte reaches the socket: {:#?}",
        request_summary(&fixture)
    );

    model.session.dispose("quit").await;
}

// =================================================================================================
// 5 · JOURNEY C — `/mcp-auth`, the HUMAN's first login, over the real loopback callback.
// =================================================================================================
//
// Journey B is the MODEL's route: `mcp({action:"auth-start"})`, a URL in a tool result, a redirect
// pasted back through `auth-complete`. That route never binds a waiter on the loopback listener —
// `start_auth` reserves the state and returns, and the code arrives as a string.
//
// `/mcp-auth` is the other half, and it is the one the runtime's own `needs-auth` message tells the
// user to run. Its shape is different in every way that matters: the URL leaves through
// `HostServices::notify` rather than through a tool result, and `crate::oauth::authenticate` then
// *blocks* on `wait_for_callback` until an HTTP GET lands on the loopback listener. So the browser
// has to be played CONCURRENTLY with the command — the callback is what lets `execute_command`
// return at all — which is why every test below joins the submission against a browser future
// instead of sequencing them.

/// The first words of `authorization_url_notice` — the only marker the test has for "this
/// notification is the one carrying the authorization URL".
const URL_NOTICE_PREFIX: &str = "Open this URL to authenticate";

/// The handler's success line, byte for byte (`McpExtension::authenticate_server`'s
/// `AuthStatus::Authenticated` arm).
const AUTH_SUCCEEDED: &str = "OAuth authentication successful for \"fixture\".";

/// The line `reconnect_after_auth` prints when the credential the flow just stored carried a
/// connect. The tool count is the fixture's own catalog — one `echo` — so a reconnect that
/// "succeeded" against an empty catalog reads as `(0 tools, …)` and fails here.
const RECONNECTED: &str = "MCP: Reconnected to fixture (1 tools, 0 resources)";

/// How long any single wait in this section may take before the test fails rather than hangs.
const STEP_TIMEOUT: Duration = Duration::from_secs(60);

/// The session's fire-and-forget UI channel, drained by the test instead of by a renderer.
///
/// This is the seam the whole section hangs on. `/mcp-auth` surfaces the authorization URL through
/// `HostServices::notify` (`McpExtension::authorization_url_hook`) and nothing else — there is no
/// tool result to read it out of — so a test that cannot see a notification cannot play the
/// browser, and the command would simply block on the loopback callback until it timed out.
///
/// No double is installed to get it: `LiveHostServices::notify` already pushes a
/// [`UiEffect::Notify`] onto the mode's effect sink, and `set_ui_effect_sink` is the seam the RPC
/// mode itself uses. The test attaches its own receiver there, so the notifications observed below
/// are the exact values a renderer would have drawn, produced by the production handle.
struct Notices(tokio::sync::mpsc::UnboundedReceiver<UiEffect>);

impl Notices {
    /// The next notification whose text contains `needle`, discarding every other UI effect.
    ///
    /// Bounded, because the alternative failure mode is a test that hangs: if the URL notice never
    /// arrives the browser is never played, the callback never lands, and `execute_command` waits
    /// on `wait_for_callback` forever.
    async fn wait_for(&mut self, needle: &str) -> String {
        let poll = async {
            loop {
                let Some(effect) = self.0.recv().await else {
                    panic!(
                        "the session's UI channel closed before a notification containing \
                         {needle:?} arrived"
                    );
                };
                if let UiEffect::Notify { message, .. } = effect
                    && message.contains(needle)
                {
                    return message;
                }
            }
        };
        tokio::time::timeout(STEP_TIMEOUT, poll)
            .await
            .unwrap_or_else(|_| {
                panic!("no notification containing {needle:?} within {STEP_TIMEOUT:?}")
            })
    }

    /// Every notification already queued, in order.
    ///
    /// Called only after the command has returned: `on_mcp_auth_command` writes each of its lines
    /// before it answers, and the sink is unbounded, so at that point the channel holds the whole
    /// transcript and a non-blocking drain cannot race it.
    fn settled(&mut self) -> Vec<(String, NotifyKind)> {
        let mut drained = Vec::new();
        while let Ok(effect) = self.0.try_recv() {
            if let UiEffect::Notify { message, kind } = effect {
                drained.push((message, kind));
            }
        }
        drained
    }
}

/// The one notification containing `needle`, with the level it was raised at.
///
/// The level is carried because it is half of what `/mcp-auth` promises: every refusal has a
/// severity upstream, and `surface` exists precisely so the message rides `notify` at that level
/// rather than the return channel's flat `Info`.
fn notice(notices: &[(String, NotifyKind)], needle: &str) -> (String, NotifyKind) {
    notices
        .iter()
        .find(|(message, _)| message.contains(needle))
        .cloned()
        .unwrap_or_else(|| panic!("no notification containing {needle:?}; saw {notices:#?}"))
}

/// Whether any notification contains `needle`.
fn any_notice(notices: &[(String, NotifyKind)], needle: &str) -> bool {
    notices.iter().any(|(message, _)| message.contains(needle))
}

/// The authorization URL out of the notice the command printed.
///
/// The notice wraps the URL in prose, and `sanitize_terminal_text` has already collapsed any
/// whitespace *inside* the URL it was given — so the URL is the one whitespace-free token that
/// names the authorization endpoint. Split on whitespace rather than on a fixed offset, because the
/// surrounding sentence is a message under test, not a format this helper should pin.
///
/// MCP-390 wraps that URL in an OSC-8 hyperlink (`ESC]8;;{url}ESC\{label}ESC]8;;ESC\`, upstream's
/// `terminalHyperlink(url, url)`), so the token no longer *starts* with `http://` — it starts with
/// the escape. The sequence carries no whitespace, so the token is still one whitespace-delimited
/// unit; trimming the escapes off each candidate keeps this helper agnostic about whether the
/// notice is hyperlinked, which is the property its doc above claims.
fn authorization_url_in(notice: &str) -> String {
    notice
        .split_whitespace()
        .filter_map(|token| {
            // Take the segment after the FIRST OSC-8 introducer — that is the link target — then
            // everything up to the ESC that terminates it. `nth(1)`, not `rsplit().next()`: the
            // sequence has a closing `ESC]8;;` too, so taking the last segment yields the trailing
            // terminator rather than the URL. A bare URL has no introducer and `nth(1)` is `None`,
            // so it passes through untouched.
            let after = token.splitn(2, "\u{1b}]8;;").nth(1).unwrap_or(token);
            let candidate = after.split('\u{1b}').next().unwrap_or(after);
            (candidate.starts_with("http://") && candidate.contains("/authorize?"))
                .then_some(candidate)
        })
        .next()
        .unwrap_or_else(|| panic!("the notice carries an authorization URL: {notice}"))
        .to_string()
}

/// [`browser_redirect`]'s refusal twin — the redirect an authorization server sends when the human
/// clicks **Deny** (RFC 6749 §4.1.2.1: `error=access_denied` on the same `redirect_uri`, with the
/// same `state`).
///
/// The `state` is real and the callback is real; only the outcome is a refusal. That is what makes
/// it a control on the success path rather than on the plumbing.
fn browser_denial(authorization_url: &str) -> String {
    let params = query_params(authorization_url);
    let redirect_uri = params.get("redirect_uri").unwrap_or_else(|| {
        panic!("the authorization URL carries a redirect_uri: {authorization_url}")
    });
    let state = params.get("state").unwrap_or_else(|| {
        panic!("the authorization URL carries a CSRF state: {authorization_url}")
    });
    let separator = if redirect_uri.contains('?') { '&' } else { '?' };
    format!("{redirect_uri}{separator}error=access_denied&state={state}")
}

/// Issue the callback GET a browser would issue, and answer with the status the listener replied.
///
/// **Connects to `127.0.0.1` rather than to the URL's own host.** `DEFAULT_OAUTH_CALLBACK_HOST` is
/// `localhost` — the *advertised* name — while the listener binds `LOOPBACK_BIND_HOST`,
/// `127.0.0.1`; on a machine whose resolver answers `::1` first, connecting to the advertised name
/// would miss the listener entirely and this test would fail for a reason that has nothing to do
/// with OAuth. The port is the one the flow itself chose (the loopback listener binds port 0 unless
/// configured), so it is read out of the URL and never assumed.
///
/// Nothing about the request or the response is printed. The authorization code is in the query
/// string, and the reply pages are HTML this test only ever inspects a status line of.
async fn loopback_get(callback_url: &str) -> u16 {
    let rest = callback_url
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("the callback URL is a loopback http:// URL"));
    let (authority, target) = match rest.split_once('/') {
        Some((authority, path)) => (authority.to_string(), format!("/{path}")),
        None => (rest.to_string(), "/".to_string()),
    };
    let port: u16 = authority
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("the callback URL names the port the listener bound"));

    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|error| {
            panic!("the loopback callback listener is accepting on 127.0.0.1:{port}: {error}")
        });
    let request =
        format!("GET {target} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    socket
        .write_all(request.as_bytes())
        .await
        .expect("write the callback request");
    let mut response = Vec::new();
    socket
        .read_to_end(&mut response)
        .await
        .expect("read the callback reply");
    let head = String::from_utf8_lossy(response.get(..response.len().min(64)).unwrap_or_default())
        .into_owned();
    head.split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("the listener answered a status line"))
}

/// [`session_config`] with a UI attached.
///
/// `AppMode::Interactive` is not decoration: `SessionBuilder`'s `ext_mode` maps it to
/// `(ExtMode::Tui, has_ui = true)`, and `has_ui` is what both halves of `/mcp-auth` are gated on —
/// `McpExtension::command_services` returns `None` without it (so every message would fall back to
/// the return channel), and `initialize_mcp` derives `McpState::ui` from the SessionStart ctx's
/// `has_ui`, which `authenticate_server` refuses without. The suite's other sessions stay on the
/// default `AppMode::Print`, which is exactly the headless shape
/// [`the_mcp_auth_command_refuses_a_headless_session_rather_than_pretending`] pins.
fn ui_session_config(hx: &Harness) -> SessionConfig {
    let mut cfg = session_config(hx);
    cfg.app_mode = AppMode::Interactive;
    cfg
}

/// [`start_session`] for a session that has a human attached, with the notifications captured.
///
/// The sink is installed BETWEEN `build()` and `bind_extensions()`: the extension's `SessionStart`
/// is dispatched by the latter, so an earlier attach would be impossible and a later one could drop
/// a notification the startup pass raised.
async fn start_ui_session(hx: &Harness) -> (AgentSession, Arc<McpExtension>, Notices) {
    let faux = Arc::new(FauxProvider::new());
    // `/mcp-auth` is serviced by `prepare` before any turn is assembled, so the provider is never
    // called on this route; the step is here so an accidental turn settles instead of hanging.
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ready")],
        StopReason::Stop,
    )]);
    let ext = adapter(hx);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, ui_session_config(hx))
        .with_native_extension(Arc::clone(&ext) as Arc<dyn cyrup_ext::NativeExtension>)
        .build()
        .await
        .unwrap();
    let (sink, notices) = tokio::sync::mpsc::unbounded_channel();
    session.services().host_services.set_ui_effect_sink(sink);
    session.bind_extensions().await;
    (session, ext, Notices(notices))
}

/// Submit `/<line>` the way a human's Enter key does.
///
/// `AgentSession::prompt` -> `prepare` -> `try_execute_extension_command` is the production route
/// and the only one that proves the command is REACHABLE: the name is parsed off the submission,
/// resolved through `ExtensionHost::command_route` (which is where `mcp-auth:2` disambiguation
/// would bite), and dispatched to `NativeExtension::execute_command` with a command-tier `HostCtx`.
/// Calling `execute_native_command` directly would skip the first two links.
async fn slash(session: &AgentSession, line: &str) {
    let _stream = session
        .prompt(line)
        .await
        .expect("the submission was accepted");
}

/// **`/mcp-auth fixture` logs in for real, over the loopback callback, and leaves a usable
/// credential behind.**
///
/// This is the route the runtime's own `needs-auth` message names — "OAuth authentication required.
/// Run /mcp-auth fixture." — and until this test it was the one route in the feature that had never
/// been executed against a server. Every other test in this file drives the model's
/// `mcp({action: …})` protocol; this one drives the human's.
///
/// The phases, and what each rules out:
///
/// 1. an empty vault ⇒ `needs-auth`, so the login below is a first login and not a re-read;
/// 2. `/mcp-auth fixture` is submitted, and — CONCURRENTLY, because the handler is blocked inside
///    `oauth::authenticate` waiting for the callback — the test reads the authorization URL off the
///    notification the handler raised and issues the callback GET the browser would have issued.
///    This is the leg journey B cannot reach: `wait_for_callback` and the loopback listener are
///    doing the work, not a pasted string;
/// 3. the wire says the flow was real — a `.well-known` walk, ONE dynamic client registration, and
///    ONE `/token` exchange carrying `grant_type=authorization_code` with the PKCE verifier;
/// 4. the handler's two lines, at their own levels, on the notify channel: the success sentence and
///    the reconnect with the fixture's own catalog counted;
/// 5. the VAULT holds the token the fixture's `/token` endpoint minted — not merely "a" token;
/// 6. the live connection is `Connected` and the server's own bytes come back through it;
/// 7. a cold second session over the same vault connects at startup with NO new `401` and no
///    browser — the credential `/mcp-auth` wrote is a credential the connect ladder can use.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mcp_auth_command_logs_in_against_a_real_server_and_stores_a_usable_credential() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, true);

    // ── phase 1 — nothing to authenticate with ────────────────────────────────────────────────
    let (session, ext, mut notices) = start_ui_session(&hx).await;
    let state = await_settled(&ext).await;
    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::NeedsAuth),
        "the session starts with an empty vault, so the startup connect ends at needs-auth"
    );
    assert!(
        !vault_holds(&hx.store, &fixture.url).await,
        "the vault starts empty"
    );
    let after_startup = fixture.requests().len();
    assert!(
        fixture.hits("/register").is_empty(),
        "no login has been attempted yet"
    );

    // ── phase 2 — the command, and the browser, at the same time ──────────────────────────────
    //
    // Joined rather than sequenced: `on_mcp_auth_command` does not return until `oauth::authenticate`
    // resolves, and `oauth::authenticate` does not resolve until the callback below lands.
    let browser = async {
        let notice = notices.wait_for(URL_NOTICE_PREFIX).await;
        let authorization_url = authorization_url_in(&notice);
        assert!(
            authorization_url.starts_with(&format!("{}/authorize", fixture.issuer)),
            "the URL points at the fixture's own authorization endpoint, which the flow learned by \
             walking the fixture's metadata: {authorization_url}"
        );
        // `browser_redirect` asserts the PKCE `code_challenge` is present and reads the redirect
        // URI and CSRF state out of the URL the flow itself produced.
        let status = loopback_get(&browser_redirect(&authorization_url)).await;
        assert_eq!(
            status, 200,
            "the loopback listener accepted the callback — a 400 is a rejected state, a 404 the \
             wrong path, a 409 an already-used flow"
        );
        authorization_url
    };
    let (_submitted, authorization_url) = tokio::time::timeout(
        STEP_TIMEOUT,
        futures::future::join(slash(&session, "/mcp-auth fixture"), browser),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "`/mcp-auth fixture` never returned within {STEP_TIMEOUT:?}; requests: {:#?}",
            request_summary(&fixture)
        )
    });

    // ── phase 3 — the wire says the flow was real ─────────────────────────────────────────────
    assert!(
        fixture
            .requests()
            .iter()
            .skip(after_startup)
            .any(|recorded| recorded.path.starts_with("/.well-known/")),
        "the command's flow discovered the authorization server: {:#?}",
        request_summary(&fixture)
    );
    assert_eq!(
        fixture.hits("/register").len(),
        1,
        "exactly one dynamic client registration: {:#?}",
        request_summary(&fixture)
    );
    let token_hits = fixture.hits("/token");
    assert_eq!(
        token_hits.len(),
        1,
        "exactly one token exchange: {:#?}",
        request_summary(&fixture)
    );
    assert!(
        token_hits[0].body.contains("grant_type=authorization_code"),
        "…and it was the authorization-code grant: {:#?}",
        token_hits[0].path
    );
    assert!(
        token_hits[0].body.contains(&format!("code={AUTH_CODE}")),
        "…carrying the code the loopback callback delivered: {:#?}",
        token_hits[0].path
    );
    assert!(
        token_hits[0].body.contains("code_verifier="),
        "…and the PKCE verifier, whose challenge `browser_redirect` already found on the \
         authorization URL {authorization_url}"
    );

    // ── phase 4 — what the human was actually told ────────────────────────────────────────────
    let said = notices.settled();
    assert_eq!(
        notice(&said, AUTH_SUCCEEDED),
        (AUTH_SUCCEEDED.to_string(), NotifyKind::Info),
        "the handler's own success sentence, at its own level: {said:#?}"
    );
    assert_eq!(
        notice(&said, RECONNECTED),
        (RECONNECTED.to_string(), NotifyKind::Info),
        "…and the reconnect line, with the fixture's own catalog counted: {said:#?}"
    );
    assert!(
        !any_notice(&said, "Failed to authenticate"),
        "nothing reported a failure: {said:#?}"
    );

    // ── phase 5 — the vault holds the fixture's OWN minted token ──────────────────────────────
    //
    // The token-level accessor, not the entry-level one: `start_auth` writes the dynamic
    // registration into the vault before any browser round trip, so an entry alone would prove
    // only that a login was attempted.
    match vault_token(&hx.store, &fixture.url).await {
        cyrup_mcp::oauth::McpOAuthTokenStatus::Present(tokens) => {
            // Compared, never printed.
            assert!(
                tokens.access_token == GOOD_TOKEN,
                "the vault holds the token the fixture's /token endpoint minted, not a value this \
                 test put there"
            );
            assert!(
                tokens.refresh_token.is_some(),
                "…and the refresh token beside it"
            );
        }
        other => panic!("`/mcp-auth` stored a token in the generation's vault, got {other:?}"),
    }

    // ── phase 6 — the connection the command's own reconnect established ──────────────────────
    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::Connected),
        "`reconnect_after_auth` retried with the stored credential and connected"
    );
    let handshake = fixture
        .requests()
        .into_iter()
        .rfind(|recorded| recorded.is_method_call("initialize"))
        .expect("a handshake");
    assert_eq!(
        handshake.all("authorization"),
        vec![format!("Bearer {GOOD_TOKEN}")],
        "the handshake carried the token the fixture's own /token endpoint minted"
    );

    session.dispose("quit").await;
    assert!(
        !cyrup_mcp::oauth::is_callback_server_running().await,
        "the session shutdown released the loopback listener the command bound"
    );
    assert_eq!(cyrup_mcp::oauth::pending_callback_count(), 0);

    // ── phase 7 — a cold second session over the credential `/mcp-auth` wrote ─────────────────
    //
    // A fresh agent directory (cold metadata cache ⇒ a real startup connect) over the SAME vault.
    // Nothing here was seeded: the only credential in play is the one the slash command's own flow
    // stored, and this is the sentence the feature exists for.
    let baseline_401 = fixture.unauthorized().len();
    let cold = harness_sharing(&fixture, true, Some(hx.store.clone()));
    let (mut cold_model, cold_ext) = driver(&cold).await;
    let cold_state = await_settled(&cold_ext).await;
    assert_eq!(
        status(&cold_state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::Connected),
        "the next session connects at startup on the stored credential; failure: {:?}",
        cold_state
            .failure_messages
            .lock()
            .ok()
            .and_then(|m| m.get(SERVER).cloned())
    );
    assert_eq!(
        fixture.unauthorized().len(),
        baseline_401,
        "…on the first attempt, with NO new 401: {:#?}",
        request_summary(&fixture)
    );
    no_oauth_flow_ran().await;
    let echoed = cold_model
        .call(DIRECT_TOOL, serde_json::json!({ "text": "pong" }))
        .await;
    assert!(
        echoed.text.contains(SERVER_ANSWER),
        "the server's own bytes, over a transport authorized by a credential `/mcp-auth` logged in \
         for: {echoed:#?}"
    );
    assert!(!echoed.is_error, "{echoed:#?}");
    cold_model.session.dispose("quit").await;
}

/// **A denied authorization is reported as a failure, and nothing is stored.**
///
/// Journey C's control, and the one that makes its success assertion mean something. Everything is
/// identical up to the browser: the same command, the same discovery, the same dynamic
/// registration, the same real loopback listener and the same real CSRF state — and then the
/// authorization server answers `error=access_denied` instead of a code, which is what a human
/// clicking **Deny** produces.
///
/// A handler that reported success from "the flow returned" rather than from
/// `AuthStatus::Authenticated`, or that reconnected regardless, passes the previous test and fails
/// here. So does one that leaves a half-written credential behind: after a refusal the vault must
/// hold no token at all, or the next session would connect on something the user declined to grant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_authorization_is_reported_as_a_failure_and_leaves_the_vault_empty() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, true);

    let (session, ext, mut notices) = start_ui_session(&hx).await;
    let state = await_settled(&ext).await;
    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::NeedsAuth),
        "the same starting point as the successful login"
    );

    let browser = async {
        let notice = notices.wait_for(URL_NOTICE_PREFIX).await;
        let authorization_url = authorization_url_in(&notice);
        // The listener answers 200 and serves the error page: the callback is well-formed and the
        // state is one it knows — it is the AUTHORIZATION that was refused, not the callback.
        let status = loopback_get(&browser_denial(&authorization_url)).await;
        assert_eq!(
            status, 200,
            "the listener recognised the state and served the refusal"
        );
    };
    tokio::time::timeout(
        STEP_TIMEOUT,
        futures::future::join(slash(&session, "/mcp-auth fixture"), browser),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "`/mcp-auth fixture` never returned after the denial within {STEP_TIMEOUT:?}; \
             requests: {:#?}",
            request_summary(&fixture)
        )
    });

    // The flow got as far as registering a client — this is a real login that was refused, not one
    // that never started.
    assert_eq!(
        fixture.hits("/register").len(),
        1,
        "the flow really ran up to the browser: {:#?}",
        request_summary(&fixture)
    );
    assert!(
        fixture.hits("/token").is_empty(),
        "NO token exchange — the refusal was honoured before the code-for-token step: {:#?}",
        request_summary(&fixture)
    );

    let said = notices.settled();
    assert_eq!(
        notice(&said, "Failed to authenticate"),
        (
            "Failed to authenticate \"fixture\": access_denied".to_string(),
            NotifyKind::Error
        ),
        "the refusal is reported with the authorization server's own reason, at Error: {said:#?}"
    );
    assert!(
        !any_notice(&said, AUTH_SUCCEEDED),
        "nothing claimed success: {said:#?}"
    );
    assert!(
        !any_notice(&said, "Reconnected"),
        "and no reconnect was attempted on a credential that does not exist: {said:#?}"
    );

    assert!(
        !vault_holds(&hx.store, &fixture.url).await,
        "the vault holds no token after a denial"
    );
    assert_eq!(
        status(&state),
        Some(cyrup_mcp::lifecycle::ConnectionStatus::NeedsAuth),
        "the server is still exactly where it was"
    );

    session.dispose("quit").await;
}

/// **`/mcp-auth <server>` for a name that is not in the config refuses it, and touches nothing.**
///
/// The cheapest thing a user can get wrong, and the one refusal that must not reach the network:
/// `authenticate_server`'s config lookup happens before the URL is resolved, before the vault is
/// read and before any listener is bound. The assertion that no request reached the fixture and no
/// callback listener was ever bound is what pins that ordering.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mcp_auth_command_refuses_a_server_that_is_not_configured() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, true);

    let (session, ext, mut notices) = start_ui_session(&hx).await;
    let _state = await_settled(&ext).await;
    let after_startup = fixture.requests().len();

    tokio::time::timeout(STEP_TIMEOUT, slash(&session, "/mcp-auth nosuchserver"))
        .await
        .expect("an unknown server is refused without waiting on anything");

    let said = notices.settled();
    assert_eq!(
        notice(&said, "nosuchserver"),
        (
            "Server \"nosuchserver\" not found in config".to_string(),
            NotifyKind::Error
        ),
        "the refusal names the server the user typed, at Error: {said:#?}"
    );
    assert!(!any_notice(&said, AUTH_SUCCEEDED), "{said:#?}");
    assert_eq!(
        fixture.requests().len(),
        after_startup,
        "nothing reached the network: {:#?}",
        request_summary(&fixture)
    );
    assert!(
        !vault_holds(&hx.store, &fixture.url).await,
        "and the vault is untouched"
    );
    no_oauth_flow_ran().await;

    session.dispose("quit").await;
}

/// **A headless session refuses `/mcp-auth` rather than pretending, and the message still reaches
/// the user.**
///
/// The mirror of [`start_ui_session`]'s reason for existing. With no UI there is no
/// `McpState::ui` for the flow's status line and no fenced handle for the authorization URL to be
/// surfaced through — so the login cannot be completed, and `authenticate_server` refuses at its
/// first guard rather than opening a browser nobody can see.
///
/// The message is asserted because this is the arm where `surface` takes its OTHER branch: with no
/// services handle the text rides `execute_command`'s return channel, which the session then prints
/// through `surface_command_outcome` at `Info`. Upstream drops the text entirely here; a port that
/// silently returned `Ok(None)` would leave a user who typed `/mcp-auth` staring at nothing, and
/// would pass every assertion above.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mcp_auth_command_refuses_a_headless_session_rather_than_pretending() {
    let fixture = HttpMcpFixture::start().await;
    let hx = harness(&fixture, true);

    // The suite's ordinary session — `AppMode::Print`, so `has_ui` is false — with the effect sink
    // attached anyway: the sink is the mode's drain and is independent of `has_ui`, which is what
    // lets the return-channel branch be observed at all.
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ready")],
        StopReason::Stop,
    )]);
    let ext = adapter(&hx);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, session_config(&hx))
        .with_native_extension(Arc::clone(&ext) as Arc<dyn cyrup_ext::NativeExtension>)
        .build()
        .await
        .unwrap();
    let (sink, notices) = tokio::sync::mpsc::unbounded_channel();
    session.services().host_services.set_ui_effect_sink(sink);
    session.bind_extensions().await;
    let mut notices = Notices(notices);
    let _state = await_settled(&ext).await;
    let after_startup = fixture.requests().len();

    tokio::time::timeout(STEP_TIMEOUT, slash(&session, "/mcp-auth fixture"))
        .await
        .expect("a headless session is refused without waiting on a browser");

    let said = notices.settled();
    assert_eq!(
        notice(&said, "interactive"),
        (
            "OAuth authentication requires an interactive session.".to_string(),
            NotifyKind::Info
        ),
        "the refusal rides the return channel, which the session surfaces at Info: {said:#?}"
    );
    assert!(!any_notice(&said, AUTH_SUCCEEDED), "{said:#?}");
    assert_eq!(
        fixture.requests().len(),
        after_startup,
        "no login was attempted: {:#?}",
        request_summary(&fixture)
    );
    assert!(!vault_holds(&hx.store, &fixture.url).await);
    no_oauth_flow_ran().await;

    session.dispose("quit").await;
}

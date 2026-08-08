//! The one-shot loopback HTTP listener that receives the authorization-server redirect.
//!
//! Every redirect-based flow in pi v0.83.0 hand-rolls the same server on top of `node:http`:
//! `openrouter.ts:135-243` (ephemeral port, claim-then-exchange, raced against a manual prompt),
//! `anthropic.ts:97-170` (fixed port 53692, `state` check), `openai-codex.ts:334-375` (fixed port
//! 1455) and `radius.ts:170-212` (fixed port 1456). This module is that server, factored out once
//! so the four flows differ only in their handler.
//!
//! ## Mechanism divergences (behaviour is upstream's)
//!
//! * pi uses `node:http`. `cyrup-provider` depends on `tokio` **without** the `net` feature and
//!   pulls in no HTTP server crate, so the listener is a `std::net::TcpListener` accept loop on a
//!   dedicated OS thread, re-entering the async world through `tokio::runtime::Handle::block_on`
//!   to run the handler. The wire behaviour — one GET, one HTML response, `connection: close` —
//!   is what a browser following a redirect needs and what upstream emits.
//! * `AbortSignal` is [`CancelToken`] (arch-00 §3.2).
//!
//! ## Semantics carried over verbatim
//!
//! * **claim** (`openrouter.ts:194`): a callback that has begun exchanging its code owns the
//!   login; [`CallbackServer::cancel_wait`] must not hand the login to manual entry after that
//!   (`openrouter.ts:235-237`).
//! * **settle-once** (`openrouter.ts:161-168`): the first outcome wins and closes the server;
//!   later requests get the "already used" page.
//! * **`cancel_wait` resolves `Ok(None)`** — "manual entry took over", distinct from a failure.
//! * on abort the wait rejects with `"Login cancelled"`, and on the flow's own deadline with the
//!   flow's timeout message (`openrouter.ts:223`).
//!
//! The listener binds a loopback address on an ephemeral port when `port` is 0, so the whole
//! module is exercisable from a test with no network access; the tests below drive it over
//! `127.0.0.1` only.

use super::page::oauth_error_html;
use super::query::parse_query;
use super::{OAuthError, interaction::AuthInteraction};
use crate::auth::types::{AuthContext, ProviderEnv};
use cyrup_core::CancelToken;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// `getCallbackHost()`'s fallback (`openrouter.ts:26`, `anthropic.ts:33`).
pub const DEFAULT_CALLBACK_HOST: &str = "127.0.0.1";

/// How long a single connection may take to send its request line + headers before it is dropped.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Accept-loop poll interval; bounds how long [`CallbackServer::close`] takes to stop the thread.
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Cap on the request head we will buffer, so a hostile client cannot grow the buffer unbounded.
const MAX_REQUEST_HEAD: usize = 8 * 1024;

/// The host the callback server binds — `getCallbackHost()` (`openrouter.ts:25-27`,
/// `openai-codex.ts:44-46`): the `PI_OAUTH_CALLBACK_HOST` provider-env value, else `127.0.0.1`.
///
/// cyrup checks `CYRUP_OAUTH_CALLBACK_HOST` first and keeps `PI_OAUTH_CALLBACK_HOST` as a
/// lower-precedence fallback, which is this workspace's standing rename convention for pi's
/// `PI_*` variables (`cyrup-config/src/env.rs:68-91`).
pub async fn callback_host(ctx: &dyn AuthContext, env: Option<&ProviderEnv>) -> String {
    for name in ["CYRUP_OAUTH_CALLBACK_HOST", "PI_OAUTH_CALLBACK_HOST"] {
        if let Some(host) = crate::env_api_keys::get_provider_env_value(name, ctx, env).await {
            return host;
        }
    }
    DEFAULT_CALLBACK_HOST.to_string()
}

/// How to bind and advertise the callback listener.
#[derive(Clone, Debug)]
pub struct CallbackServerConfig {
    /// Address to bind. Use [`callback_host`] to honour `*_OAUTH_CALLBACK_HOST`.
    pub host: String,
    /// The host name put in the advertised redirect URI. Anthropic binds `127.0.0.1` but
    /// registers `http://localhost:53692/callback` (`anthropic.ts:33-35`); `None` reuses `host`.
    pub advertise_host: Option<String>,
    /// `0` = ephemeral (`openrouter.ts:210`). Flows whose redirect URI is pre-registered pass
    /// their fixed port (53692 / 1455 / 1456).
    pub port: u16,
    /// The single path served; anything else gets the 404 page.
    pub path: String,
    /// The flow's overall login deadline (`LOGIN_TIMEOUT_MS`, `openrouter.ts:22`).
    pub timeout: Option<Duration>,
    /// The message for that deadline, e.g. `"OpenRouter OAuth login timed out"`
    /// (`openrouter.ts:223`).
    pub timeout_message: Option<String>,
    /// The login-wide abort (`interaction.signal`).
    pub cancel: Option<CancelToken>,
}

impl Default for CallbackServerConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_CALLBACK_HOST.to_string(),
            advertise_host: None,
            port: 0,
            path: "/callback".to_string(),
            timeout: None,
            timeout_message: None,
            cancel: None,
        }
    }
}

impl CallbackServerConfig {
    /// An ephemeral-port config for `path` — the `openrouter.ts:210` shape.
    pub fn ephemeral(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }

    /// A fixed-port config, for flows whose redirect URI is registered with the provider.
    pub fn fixed(port: u16, path: impl Into<String>) -> Self {
        Self {
            port,
            path: path.into(),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    #[must_use]
    pub fn advertising(mut self, host: impl Into<String>) -> Self {
        self.advertise_host = Some(host.into());
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration, message: impl Into<String>) -> Self {
        self.timeout = Some(timeout);
        self.timeout_message = Some(message.into());
        self
    }

    /// Take the login-wide abort from the interaction (`AuthInteraction.signal`).
    #[must_use]
    pub fn with_interaction(mut self, interaction: &dyn AuthInteraction) -> Self {
        self.cancel = interaction.cancel().cloned();
        self
    }

    #[must_use]
    pub fn with_cancel(mut self, cancel: Option<CancelToken>) -> Self {
        self.cancel = cancel;
        self
    }
}

/// One inbound callback request. Mirrors what upstream reads off `req`: the method, the parsed
/// `URL` pathname, and `searchParams` (`openrouter.ts:169-192`).
#[derive(Clone, Debug)]
pub struct CallbackRequest {
    pub method: String,
    /// The path, percent-decoded, without the query string.
    pub path: String,
    /// Query pairs in document order.
    pub query: Vec<(String, String)>,
    /// The raw request target, exactly as sent.
    pub target: String,
}

impl CallbackRequest {
    /// `url.searchParams.get(name)` — the **first** occurrence, or `None`.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The HTTP response a handler sends back to the browser.
#[derive(Clone, Debug)]
pub struct CallbackReply {
    pub status: u16,
    pub html: String,
    /// `cache-control: no-store`. Only `openrouter.ts:48` sets it; `radius.ts:170-173` and
    /// `anthropic.ts:118` send content-type alone, so it is opt-in rather than always-on.
    pub no_store: bool,
}

impl CallbackReply {
    pub fn new(status: u16, html: impl Into<String>) -> Self {
        Self {
            status,
            html: html.into(),
            no_store: false,
        }
    }

    /// `sendHtml(response, 200, oauthSuccessHtml(message))`.
    pub fn success(message: &str) -> Self {
        Self::new(200, super::page::oauth_success_html(message))
    }

    /// `sendHtml(response, status, oauthErrorHtml(message, details))`.
    pub fn error(status: u16, message: &str, details: Option<&str>) -> Self {
        Self::new(status, oauth_error_html(message, details))
    }

    #[must_use]
    pub fn no_store(mut self) -> Self {
        self.no_store = true;
        self
    }
}

/// What the handler decided (`finish({credential})` / `finish({error})` / plain `return` in
/// `openrouter.ts:169-207`).
pub enum CallbackOutcome<T> {
    /// Reply, then settle the wait with a value.
    Complete { reply: CallbackReply, value: T },
    /// Reply, then settle the wait with a failure.
    Failed {
        reply: CallbackReply,
        error: OAuthError,
    },
    /// Reply and keep listening: a wrong route, a missing code, a state mismatch — all of which
    /// upstream answers without settling.
    Continue { reply: CallbackReply },
}

#[derive(Default)]
struct Shared {
    stopped: AtomicBool,
    claimed: AtomicBool,
    settled: AtomicBool,
}

/// The handle a handler uses to claim the login before it starts an async token exchange
/// (`claimed = true`, `openrouter.ts:194`).
#[derive(Clone)]
pub struct CallbackControl {
    shared: Arc<Shared>,
}

impl CallbackControl {
    /// Claim the login. Returns `false` if it was already claimed — the "This OAuth callback has
    /// already been used" case (`openrouter.ts:177`).
    pub fn claim(&self) -> bool {
        !self.shared.claimed.swap(true, Ordering::SeqCst)
    }

    pub fn is_claimed(&self) -> bool {
        self.shared.claimed.load(Ordering::SeqCst)
    }

    /// Whether the wait has already been settled (`settled`, `openrouter.ts:150`).
    pub fn is_settled(&self) -> bool {
        self.shared.settled.load(Ordering::SeqCst)
    }
}

/// The flow-specific half of the callback server: validate the request, decide the page, and say
/// whether the login is finished.
#[async_trait::async_trait]
pub trait CallbackHandler: Send + Sync + 'static {
    /// What a successful callback yields — an authorization code, or a fully exchanged
    /// credential when the handler performs the exchange itself (`openrouter.ts:196-206`).
    type Value: Send + 'static;

    async fn handle(
        &self,
        request: CallbackRequest,
        control: CallbackControl,
    ) -> CallbackOutcome<Self::Value>;
}

/// What the wait resolves to: a value, `None` for "manual entry took over", or a failure.
type Settled<T> = Result<Option<T>, OAuthError>;
type SettleSlot<T> = std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Settled<T>>>>;
type ReceiverSlot<T> = std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<Settled<T>>>>;

/// A running one-shot callback listener.
///
/// Dropping it stops the accept thread, mirroring the `finally { callback.close() }` every flow
/// wraps its login in (`openrouter.ts:298-301`).
pub struct CallbackServer<T> {
    redirect_uri: String,
    local_addr: SocketAddr,
    path: String,
    shared: Arc<Shared>,
    settle: Arc<SettleSlot<T>>,
    receiver: ReceiverSlot<T>,
    timeout: Option<Duration>,
    timeout_message: Option<String>,
    cancel: Option<CancelToken>,
}

impl<T: Send + 'static> CallbackServer<T> {
    /// Bind the listener and start accepting (`server.listen(port, host, ...)`).
    ///
    /// Errors as [`OAuthError::Cancelled`] if the login was already aborted
    /// (`openrouter.ts:136`), and as [`OAuthError::Listen`] if the port cannot be bound — the
    /// `server.once("error", reject)` path (`openrouter.ts:209`), which for the fixed-port flows
    /// means "another login is already listening".
    pub async fn start<H>(config: CallbackServerConfig, handler: H) -> Result<Self, OAuthError>
    where
        H: CallbackHandler<Value = T>,
    {
        if config
            .cancel
            .as_ref()
            .is_some_and(CancelToken::is_cancelled)
        {
            return Err(OAuthError::Cancelled);
        }

        let bind_addr = format!("{}:{}", bracket_host(&config.host), config.port);
        let listener = TcpListener::bind(&bind_addr).map_err(|source| OAuthError::Listen {
            address: bind_addr.clone(),
            source,
        })?;
        let local_addr = listener.local_addr().map_err(|source| OAuthError::Listen {
            address: bind_addr.clone(),
            source,
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|source| OAuthError::Listen {
                address: bind_addr.clone(),
                source,
            })?;

        // The handler runs on the async runtime this call was made from; there is no `net`
        // feature available for a tokio listener, hence the thread + `block_on` bridge.
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            OAuthError::Failed(
                "OAuth callback server must be started from a tokio runtime".to_string(),
            )
        })?;

        let (tx, rx) = tokio::sync::oneshot::channel();
        let shared = Arc::new(Shared::default());
        let settle: Arc<SettleSlot<T>> = Arc::new(std::sync::Mutex::new(Some(tx)));

        let advertise_host = config
            .advertise_host
            .clone()
            .unwrap_or_else(|| config.host.clone());
        let redirect_uri = format!(
            "http://{}:{}{}",
            bracket_host(&advertise_host),
            local_addr.port(),
            config.path
        );

        {
            let shared = Arc::clone(&shared);
            let settle = Arc::clone(&settle);
            let handler = Arc::new(handler);
            let path = config.path.clone();
            std::thread::Builder::new()
                .name("cyrup-oauth-callback".to_string())
                .spawn(move || {
                    accept_loop(listener, handler, shared, settle, runtime, path);
                })
                .map_err(|source| OAuthError::Listen {
                    address: bind_addr,
                    source,
                })?;
        }

        // An abort that landed while we were binding still cancels (`openrouter.ts:219-222`).
        if config
            .cancel
            .as_ref()
            .is_some_and(CancelToken::is_cancelled)
        {
            shared.stopped.store(true, Ordering::SeqCst);
            return Err(OAuthError::Cancelled);
        }

        Ok(Self {
            redirect_uri,
            local_addr,
            path: config.path,
            shared,
            settle,
            receiver: std::sync::Mutex::new(Some(rx)),
            timeout: config.timeout,
            timeout_message: config.timeout_message,
            cancel: config.cancel,
        })
    }

    /// The URI handed to the authorization server (`callbackUrl`, `openrouter.ts:232`).
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// The bound port — ephemeral binds only know it after `listen` (`openrouter.ts:226-231`).
    pub fn port(&self) -> u16 {
        self.local_addr.port()
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Stop listening without settling the wait (`close`, `openrouter.ts:155-159`).
    pub fn close(&self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
    }

    /// Hand the login over to manual code entry — resolves the wait with `Ok(None)` **unless** a
    /// callback already claimed the exchange (`cancelWait`, `openrouter.ts:235-237`).
    pub fn cancel_wait(&self) {
        if !self.shared.claimed.load(Ordering::SeqCst) {
            finish(&self.shared, &self.settle, Ok(None));
        }
    }

    /// Wait for the browser callback (`waitForCredential`, `openrouter.ts:238`).
    ///
    /// * `Ok(Some(value))` — a callback completed the login.
    /// * `Ok(None)` — [`Self::cancel_wait`] handed the login to manual entry.
    /// * `Err` — the handler failed, the login was aborted (`"Login cancelled"`) or the flow's
    ///   timeout elapsed (the flow's own message).
    pub async fn wait(&self) -> Result<Option<T>, OAuthError> {
        let receiver = self.receiver.lock().ok().and_then(|mut slot| slot.take());
        let Some(receiver) = receiver else {
            return Err(OAuthError::Failed(
                "OAuth callback result was already taken".to_string(),
            ));
        };

        let timeout_error = || match &self.timeout_message {
            Some(message) => OAuthError::Timeout {
                message: message.clone(),
            },
            None => OAuthError::Timeout {
                message: "OAuth login timed out".to_string(),
            },
        };

        let settled = async {
            match receiver.await {
                Ok(result) => result,
                // The sender was dropped without settling: the server is gone.
                Err(_) => Err(OAuthError::Failed(
                    "OAuth callback server stopped before the login completed".to_string(),
                )),
            }
        };
        let timed = async {
            match self.timeout {
                Some(duration) => match tokio::time::timeout(duration, settled).await {
                    Ok(result) => result,
                    Err(_) => Err(timeout_error()),
                },
                None => settled.await,
            }
        };

        let result = match &self.cancel {
            Some(token) => tokio::select! {
                biased;
                () = token.cancelled() => Err(OAuthError::Cancelled),
                result = timed => result,
            },
            None => timed.await,
        };

        if result.is_err() {
            self.close();
        }
        result
    }
}

impl<T> Drop for CallbackServer<T> {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
    }
}

/// `finish(...)` (`openrouter.ts:161-168`): the first outcome wins, and settling closes the
/// server.
fn finish<T>(shared: &Arc<Shared>, settle: &Arc<SettleSlot<T>>, result: Settled<T>) {
    if shared.settled.swap(true, Ordering::SeqCst) {
        return;
    }
    shared.stopped.store(true, Ordering::SeqCst);
    if let Ok(mut slot) = settle.lock()
        && let Some(sender) = slot.take()
    {
        let _ = sender.send(result);
    }
}

/// `[::1]` for a bare IPv6 literal, so both `TcpListener::bind` and the advertised URL parse.
fn bracket_host(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn accept_loop<H: CallbackHandler>(
    listener: TcpListener,
    handler: Arc<H>,
    shared: Arc<Shared>,
    settle: Arc<SettleSlot<H::Value>>,
    runtime: tokio::runtime::Handle,
    path: String,
) {
    while !shared.stopped.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                // One thread per connection: `node:http` serves requests concurrently, and
                // `openrouter.ts:176-179` depends on that — a second redirect arriving *while*
                // the first is exchanging its code must be answered with the 409 page rather than
                // queued behind it.
                let handler = Arc::clone(&handler);
                let shared = Arc::clone(&shared);
                let settle = Arc::clone(&settle);
                let runtime = runtime.clone();
                let path = path.clone();
                if std::thread::Builder::new()
                    .name("cyrup-oauth-callback-conn".to_string())
                    .spawn(move || {
                        serve_connection(stream, &handler, &shared, &settle, &runtime, &path);
                    })
                    .is_err()
                {
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(_) => break,
        }
    }
}

fn serve_connection<H: CallbackHandler>(
    stream: TcpStream,
    handler: &Arc<H>,
    shared: &Arc<Shared>,
    settle: &Arc<SettleSlot<H::Value>>,
    runtime: &tokio::runtime::Handle,
    path: &str,
) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(Some(REQUEST_READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(REQUEST_READ_TIMEOUT));

    let mut stream = stream;
    let Some(request) = read_request(&mut stream) else {
        // A malformed or truncated request is answered like an unknown route rather than left
        // hanging, so a stray probe cannot wedge a browser tab.
        write_reply(
            &mut stream,
            &CallbackReply::error(400, "Malformed OAuth callback request.", None),
        );
        return;
    };

    // Route check lives here, not in every handler: all four upstream servers answer a foreign
    // path with the same 404 page, and they check it first (`openrouter.ts:171-174`,
    // `radius.ts:178-181`).
    if request.path != path {
        write_reply(
            &mut stream,
            &CallbackReply::error(404, "OAuth callback route not found.", None),
        );
        return;
    }

    // `if (claimed || settled)` (`openrouter.ts:176-179`) — hoisted out of the handler because it
    // is the same check for every flow. Flows that never call `claim()` only ever hit the
    // `settled` half.
    if shared.claimed.load(Ordering::SeqCst) || shared.settled.load(Ordering::SeqCst) {
        write_reply(
            &mut stream,
            &CallbackReply::error(409, "This OAuth callback has already been used.", None),
        );
        return;
    }

    let control = CallbackControl {
        shared: Arc::clone(shared),
    };
    let outcome = runtime.block_on(handler.handle(request, control));
    match outcome {
        CallbackOutcome::Complete { reply, value } => {
            write_reply(&mut stream, &reply);
            finish(shared, settle, Ok(Some(value)));
        }
        CallbackOutcome::Failed { reply, error } => {
            write_reply(&mut stream, &reply);
            finish(shared, settle, Err(error));
        }
        CallbackOutcome::Continue { reply } => write_reply(&mut stream, &reply),
    }
}

/// Read and parse the request line, then drain headers up to the blank line.
fn read_request(stream: &mut TcpStream) -> Option<CallbackRequest> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    let mut consumed = 0usize;
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    consumed += line.len();

    let request_line = line.trim_end_matches(['\r', '\n']).to_string();
    let mut parts = request_line.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    if method.is_empty() || target.is_empty() {
        return None;
    }

    // Drain the header block so the client's write completes before we answer.
    loop {
        let mut header = String::new();
        let read = reader.read_line(&mut header).ok()?;
        consumed += read;
        if read == 0 || header == "\r\n" || header == "\n" || consumed > MAX_REQUEST_HEAD {
            break;
        }
    }

    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target.as_str(), ""),
    };
    Some(CallbackRequest {
        method,
        // `new URL(req.url ?? "/", base).pathname` percent-decodes but never treats `+` as a
        // space (`openrouter.ts:169`).
        path: super::query::percent_decode(raw_path),
        query: parse_query(raw_query),
        target,
    })
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "OK",
    }
}

fn write_reply(stream: &mut TcpStream, reply: &CallbackReply) {
    let body = reply.html.as_bytes();
    let mut head = format!(
        "HTTP/1.1 {} {}\r\ncontent-type: text/html; charset=utf-8\r\n",
        reply.status,
        reason_phrase(reply.status)
    );
    if reply.no_store {
        head.push_str("cache-control: no-store\r\n");
    }
    head.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    ));
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
    // Let the client read the response before the socket is dropped.
    let mut sink = [0u8; 64];
    let _ = stream.read(&mut sink);
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use std::collections::BTreeMap;

    /// Codes-only handler, the `radius.ts:176-201` shape: validate `state`, answer, hand the code
    /// back.
    struct CodeHandler {
        expected_state: String,
    }

    #[async_trait::async_trait]
    impl CallbackHandler for CodeHandler {
        type Value = String;

        async fn handle(
            &self,
            request: CallbackRequest,
            _control: CallbackControl,
        ) -> CallbackOutcome<String> {
            if request.param("state") != Some(self.expected_state.as_str()) {
                return CallbackOutcome::Continue {
                    reply: CallbackReply::error(400, "OAuth state mismatch.", None),
                };
            }
            if let Some(error) = request.param("error") {
                let description = request.param("error_description").unwrap_or(error);
                return CallbackOutcome::Failed {
                    reply: CallbackReply::error(400, description, None),
                    error: OAuthError::Failed(format!("authorization failed: {description}")),
                };
            }
            match request.param("code") {
                Some(code) => CallbackOutcome::Complete {
                    reply: CallbackReply::success("Signed in. You may now close this page."),
                    value: code.to_string(),
                },
                None => CallbackOutcome::Continue {
                    reply: CallbackReply::error(400, "Missing authorization code.", None),
                },
            }
        }
    }

    /// Drive the loopback listener the way a browser would. Blocking I/O, so it runs on a
    /// blocking thread.
    async fn get(port: u16, target: &str) -> String {
        let target = target.to_string();
        tokio::task::spawn_blocking(move || {
            let mut stream =
                TcpStream::connect(("127.0.0.1", port)).expect("connect to loopback listener");
            stream
                .write_all(
                    format!(
                        "GET {target} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .expect("write request");
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response);
            response
        })
        .await
        .expect("client task")
    }

    fn config() -> CallbackServerConfig {
        CallbackServerConfig::ephemeral("/oauth/callback")
    }

    /// `unwrap_err` needs `T: Debug`, and a live server is not one.
    fn start_err<T: Send + 'static>(result: Result<CallbackServer<T>, OAuthError>) -> OAuthError {
        match result {
            Ok(server) => panic!(
                "expected start to fail, but it bound {}",
                server.redirect_uri()
            ),
            Err(error) => error,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn binds_loopback_and_completes_on_the_redirect() {
        let server = CallbackServer::start(
            config(),
            CodeHandler {
                expected_state: "st4te".into(),
            },
        )
        .await
        .unwrap();

        assert_eq!(
            server.redirect_uri(),
            format!("http://127.0.0.1:{}/oauth/callback", server.port())
        );
        assert_ne!(server.port(), 0, "an ephemeral port must be resolved");

        let response = get(server.port(), "/oauth/callback?code=abc123&state=st4te").await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("content-type: text/html; charset=utf-8"));
        assert!(response.contains("<title>Authentication successful</title>"));

        assert_eq!(server.wait().await.unwrap(), Some("abc123".to_string()));
    }

    /// A foreign path gets upstream's 404 page and does **not** settle the login.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_route_serves_the_404_page_without_settling() {
        let server = CallbackServer::start(
            config(),
            CodeHandler {
                expected_state: "st4te".into(),
            },
        )
        .await
        .unwrap();

        let response = get(server.port(), "/favicon.ico").await;
        assert!(
            response.starts_with("HTTP/1.1 404 Not Found\r\n"),
            "{response}"
        );
        assert!(response.contains("<title>Authentication failed</title>"));

        // Still listening: the real redirect that follows still wins.
        let ok = get(server.port(), "/oauth/callback?code=late&state=st4te").await;
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        assert_eq!(server.wait().await.unwrap(), Some("late".to_string()));
    }

    /// A state mismatch answers 400 and keeps listening (`radius.ts:182-185`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn state_mismatch_replies_400_and_keeps_listening() {
        let server = CallbackServer::start(
            config(),
            CodeHandler {
                expected_state: "st4te".into(),
            },
        )
        .await
        .unwrap();

        let bad = get(server.port(), "/oauth/callback?code=abc&state=forged").await;
        assert!(bad.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{bad}");
        assert!(bad.contains("OAuth state mismatch."));

        let good = get(server.port(), "/oauth/callback?code=abc&state=st4te").await;
        assert!(good.starts_with("HTTP/1.1 200 OK\r\n"), "{good}");
        assert_eq!(server.wait().await.unwrap(), Some("abc".to_string()));
    }

    /// `?error=access_denied` settles the wait as a failure carrying the server's description.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn authorization_error_settles_as_failure() {
        let server = CallbackServer::start(
            config(),
            CodeHandler {
                expected_state: "st4te".into(),
            },
        )
        .await
        .unwrap();

        let response = get(
            server.port(),
            "/oauth/callback?state=st4te&error=access_denied&error_description=User+said+no",
        )
        .await;
        assert!(
            response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
            "{response}"
        );
        assert!(response.contains("User said no"));

        let err = server.wait().await.unwrap_err();
        assert_eq!(err.to_string(), "authorization failed: User said no");
    }

    /// Settle-once: while one claimed callback is exchanging its code, a second redirect gets the
    /// 409 "already used" page instead of a second exchange (`openrouter.ts:176-179`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_second_callback_gets_the_already_used_page() {
        let server = CallbackServer::start(config(), SlowExchange).await.unwrap();
        let port = server.port();

        let first = tokio::spawn(async move { get(port, "/oauth/callback?code=one").await });
        tokio::time::sleep(Duration::from_millis(150)).await;
        let second = get(port, "/oauth/callback?code=two").await;

        assert!(second.starts_with("HTTP/1.1 409 Conflict\r\n"), "{second}");
        assert!(second.contains("This OAuth callback has already been used."));
        assert!(first.await.unwrap().starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(
            server.wait().await.unwrap(),
            Some("exchanged:one".to_string()),
            "the first, claimed callback wins"
        );
    }

    /// The same, for a handler that claims but does **not** re-check the claim itself: the
    /// hoisted `claimed || settled` gate must still answer the concurrent request with 409, so a
    /// flow cannot accidentally exchange one code twice.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn hoisted_claim_gate_answers_before_the_handler() {
        struct ClaimsOnly;
        #[async_trait::async_trait]
        impl CallbackHandler for ClaimsOnly {
            type Value = String;
            async fn handle(
                &self,
                request: CallbackRequest,
                control: CallbackControl,
            ) -> CallbackOutcome<String> {
                control.claim();
                tokio::time::sleep(Duration::from_millis(600)).await;
                CallbackOutcome::Complete {
                    reply: CallbackReply::success("done"),
                    value: request.param("code").unwrap_or_default().to_string(),
                }
            }
        }

        let server = CallbackServer::start(config(), ClaimsOnly).await.unwrap();
        let port = server.port();
        let first = tokio::spawn(async move { get(port, "/oauth/callback?code=one").await });
        tokio::time::sleep(Duration::from_millis(150)).await;
        let second = get(port, "/oauth/callback?code=two").await;

        assert!(second.starts_with("HTTP/1.1 409 Conflict\r\n"), "{second}");
        assert!(first.await.unwrap().starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(server.wait().await.unwrap(), Some("one".to_string()));
    }

    /// A handler that claims the login before its (slow) token exchange —
    /// `openrouter.ts:194-206`.
    struct SlowExchange;

    #[async_trait::async_trait]
    impl CallbackHandler for SlowExchange {
        type Value = String;
        async fn handle(
            &self,
            request: CallbackRequest,
            control: CallbackControl,
        ) -> CallbackOutcome<String> {
            let Some(code) = request.param("code").map(str::to_string) else {
                return CallbackOutcome::Continue {
                    reply: CallbackReply::error(400, "Missing authorization code.", None),
                };
            };
            if !control.claim() {
                return CallbackOutcome::Continue {
                    reply: CallbackReply::error(
                        409,
                        "This OAuth callback has already been used.",
                        None,
                    ),
                };
            }
            // The token exchange, which `cancelWait` must not race.
            tokio::time::sleep(Duration::from_millis(600)).await;
            CallbackOutcome::Complete {
                reply: CallbackReply::success("done").no_store(),
                value: format!("exchanged:{code}"),
            }
        }
    }

    /// `cancelWait()` resolves the wait with `None` — "manual entry took over" — and is distinct
    /// from an error (`openrouter.ts:235-237`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_wait_hands_over_to_manual_entry() {
        let server = CallbackServer::start(
            config(),
            CodeHandler {
                expected_state: "st4te".into(),
            },
        )
        .await
        .unwrap();
        server.cancel_wait();
        assert_eq!(server.wait().await.unwrap(), None);
    }

    /// A handler that claims before doing async work keeps `cancel_wait` from stealing the login.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn claimed_callback_survives_cancel_wait() {
        let server = CallbackServer::start(config(), SlowExchange).await.unwrap();
        let port = server.port();
        let client = tokio::spawn(async move { get(port, "/oauth/callback?code=xyz").await });

        // Give the handler time to claim, then simulate the manual prompt resolving.
        tokio::time::sleep(Duration::from_millis(150)).await;
        server.cancel_wait();

        let response = client.await.unwrap();
        assert!(response.contains("cache-control: no-store"), "{response}");
        assert_eq!(
            server.wait().await.unwrap(),
            Some("exchanged:xyz".to_string()),
            "a claimed callback must win over cancel_wait"
        );
    }

    /// An aborted login rejects with upstream's `"Login cancelled"`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abort_rejects_with_login_cancelled() {
        let token = CancelToken::new();
        let server = CallbackServer::start(
            config().with_cancel(Some(token.clone())),
            CodeHandler {
                expected_state: "st4te".into(),
            },
        )
        .await
        .unwrap();
        token.cancel();
        assert_eq!(
            server.wait().await.unwrap_err().to_string(),
            "Login cancelled"
        );
    }

    /// Starting with an already-aborted signal never binds (`openrouter.ts:136`).
    #[tokio::test]
    async fn start_with_an_aborted_signal_fails_immediately() {
        let token = CancelToken::new();
        token.cancel();
        let err = start_err(
            CallbackServer::start(
                config().with_cancel(Some(token)),
                CodeHandler {
                    expected_state: "st4te".into(),
                },
            )
            .await,
        );
        assert_eq!(err.to_string(), "Login cancelled");
    }

    /// The flow's own deadline surfaces with the flow's message (`openrouter.ts:223`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn login_timeout_uses_the_flow_message() {
        let server = CallbackServer::start(
            config().with_timeout(
                Duration::from_millis(150),
                "OpenRouter OAuth login timed out",
            ),
            CodeHandler {
                expected_state: "st4te".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            server.wait().await.unwrap_err().to_string(),
            "OpenRouter OAuth login timed out"
        );
    }

    /// Anthropic binds `127.0.0.1` yet advertises `localhost` (`anthropic.ts:33-35`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn advertised_host_may_differ_from_the_bound_host() {
        let server = CallbackServer::start(
            CallbackServerConfig::ephemeral("/callback").advertising("localhost"),
            CodeHandler {
                expected_state: "s".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            server.redirect_uri(),
            format!("http://localhost:{}/callback", server.port())
        );
    }

    /// A fixed port that is already taken fails as `Listen`, the "another login is running" case.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fixed_port_conflict_reports_a_listen_error() {
        let squatter = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = squatter.local_addr().unwrap().port();
        let err = start_err(
            CallbackServer::start(
                CallbackServerConfig::fixed(port, "/callback"),
                CodeHandler {
                    expected_state: "s".into(),
                },
            )
            .await,
        );
        drop(squatter);
        assert!(
            matches!(err, OAuthError::Listen { .. }),
            "expected a listen error, got {err}"
        );
        assert!(err.to_string().contains(&format!("127.0.0.1:{port}")));
    }

    /// `close()` stops listening **without** settling the wait — upstream's contract for it
    /// ("Stop listening and release timers without settling `waitForCredential`",
    /// `openrouter.ts:33-34`). A flow that only closes must rely on its own timeout/abort.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_without_settling_does_not_resolve_the_wait() {
        let server = CallbackServer::start(
            config(),
            CodeHandler {
                expected_state: "s".into(),
            },
        )
        .await
        .unwrap();
        server.close();
        let waited = tokio::time::timeout(Duration::from_millis(200), server.wait()).await;
        assert!(waited.is_err(), "close() alone must not settle the login");
    }

    struct MapCtx(BTreeMap<String, String>);

    #[async_trait::async_trait]
    impl AuthContext for MapCtx {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn callback_host_precedence() {
        let empty = MapCtx(BTreeMap::new());
        assert_eq!(callback_host(&empty, None).await, "127.0.0.1");

        let pi_only = MapCtx(BTreeMap::from([(
            "PI_OAUTH_CALLBACK_HOST".to_string(),
            "0.0.0.0".to_string(),
        )]));
        assert_eq!(callback_host(&pi_only, None).await, "0.0.0.0");

        let both = MapCtx(BTreeMap::from([
            ("PI_OAUTH_CALLBACK_HOST".to_string(), "0.0.0.0".to_string()),
            ("CYRUP_OAUTH_CALLBACK_HOST".to_string(), "::1".to_string()),
        ]));
        assert_eq!(callback_host(&both, None).await, "::1");
    }

    /// Query parsing off the wire: percent escapes, `+`, and a `code` that contains `=`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn parses_percent_encoded_callback_parameters() {
        struct Echo;
        #[async_trait::async_trait]
        impl CallbackHandler for Echo {
            type Value = String;
            async fn handle(
                &self,
                request: CallbackRequest,
                _control: CallbackControl,
            ) -> CallbackOutcome<String> {
                CallbackOutcome::Complete {
                    reply: CallbackReply::success("ok"),
                    value: format!(
                        "{}|{}|{}",
                        request.method,
                        request.path,
                        request.param("code").unwrap_or_default()
                    ),
                }
            }
        }

        let server = CallbackServer::start(CallbackServerConfig::ephemeral("/oauth/cb"), Echo)
            .await
            .unwrap();
        let _ = get(server.port(), "/oauth/cb?code=a%2Bb%3Dc&state=s").await;
        assert_eq!(
            server.wait().await.unwrap(),
            Some("GET|/oauth/cb|a+b=c".to_string())
        );
    }
}

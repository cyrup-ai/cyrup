//! A loopback fake of `llama-server` in router mode, shared by the `cyrup-llama` tests.
//!
//! One [`FakeLlamaServer`] binds `127.0.0.1:0` (the workspace's loopback-server technique:
//! `cyrup-provider/src/tests/remote_catalog.rs`, `transform_headers_on_the_wire.rs`) and speaks the
//! router management API that pi's `client.ts` drives:
//!
//! | request                | answer                                                              |
//! |------------------------|---------------------------------------------------------------------|
//! | `GET /models`          | [`wire::models_envelope`] over the catalog (also `?reload=1`)       |
//! | `GET /props`           | the configured props (default [`wire::router_props`] with autoload) |
//! | `POST /models/load`    | [`wire::success`], then runs the model's [`on_load`] script         |
//! | `POST /models/unload`  | [`wire::success`], then runs the model's [`on_unload`] script       |
//! | `POST /models`         | [`wire::success`], then runs the model's [`on_download`] script     |
//! | `GET /models/sse`      | an open `text/event-stream` fed by [`Step`]s and [`broadcast`]      |
//! | anything else          | [`wire::file_not_found`], llama.cpp's own unknown-route 404         |
//!
//! Every one of those answers, and the SSE framing, comes from [`wire`] — ONE definition of the
//! llama.cpp router wire, each shape carrying a `file:line` citation into llama.cpp's own server
//! source at a recorded pin (EXT-100), shared since EXT-108 with the workspace's other
//! llama-server fakes through the test-only `cyrup-llama-cpp-wire` crate. This file holds no
//! transcription of its own, and `llama_cpp_wire.rs`'s drift guard fails if what it writes on the
//! socket stops being that crate's pinned bytes.
//!
//! State is whatever the test makes it: the catalog is a list of JSON objects ([`model`] builds
//! one), and the lifecycle transitions are *scripted*, not simulated. A script is a list of
//! [`Step`]s (wait, change the catalog, emit an SSE frame, drop the SSE streams) that runs on its
//! own task after the triggering POST has been answered, so a test spells out exactly the
//! sequence of states and events the client must cope with. A trigger with no script answers
//! `{"success":true}` and changes nothing.
//!
//! Beyond the happy path:
//!   * [`respond`] / [`respond_times`] replace the answer for a `(method, path)` pair with a
//!     status + body ([`Reply`]), or [`Reply::Hang`] to never answer (timeouts);
//!   * [`require_bearer`] makes the server reject requests without `Authorization: Bearer <key>`
//!     with a `401`;
//!   * [`requests`] / [`requests_to`] return everything received (method, target, lower-cased
//!     headers, body), [`sse_connections`] / [`wait_for_sse`] observe the event stream.
//!
//! Every connection is `connection: close`; the SSE stream is delimited by the close.
//!
//! [`wire`]: cyrup_llama_cpp_wire::router
//! [`wire::models_envelope`]: cyrup_llama_cpp_wire::router::models_envelope
//! [`wire::router_props`]: cyrup_llama_cpp_wire::router::router_props
//! [`wire::success`]: cyrup_llama_cpp_wire::router::success
//! [`wire::file_not_found`]: cyrup_llama_cpp_wire::router::file_not_found
//! [`on_load`]: FakeLlamaServer::on_load
//! [`on_unload`]: FakeLlamaServer::on_unload
//! [`on_download`]: FakeLlamaServer::on_download
//! [`broadcast`]: FakeLlamaServer::broadcast
//! [`respond`]: FakeLlamaServer::respond
//! [`respond_times`]: FakeLlamaServer::respond_times
//! [`require_bearer`]: FakeLlamaServer::require_bearer
//! [`requests`]: FakeLlamaServer::requests
//! [`requests_to`]: FakeLlamaServer::requests_to
//! [`sse_connections`]: FakeLlamaServer::sse_connections
//! [`wait_for_sse`]: FakeLlamaServer::wait_for_sse
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use cyrup_llama_cpp_wire::router as wire;

// ------------------------------------------------------------------------------------- builders --

/// A catalog entry `{"id": id, "status": {"value": status}}`.
pub fn model(id: &str, status: &str) -> Value {
    json!({ "id": id, "status": { "value": status } })
}

/// A catalog entry with extra top-level fields merged in (`source`, `meta`, `architecture`, ...).
pub fn model_with(id: &str, status: &str, extra: Value) -> Value {
    let mut entry = model(id, status);
    if let (Some(entry), Value::Object(extra)) = (entry.as_object_mut(), extra) {
        entry.extend(extra);
    }
    entry
}

/// A catalog entry whose `status` object carries extra fields (`failed`, `exit_code`, `progress`,
/// `args`).
pub fn model_with_status(id: &str, status: &str, extra: Value) -> Value {
    let mut entry = model(id, status);
    if let (Some(status), Value::Object(extra)) = (entry.get_mut("status"), extra)
        && let Some(status) = status.as_object_mut()
    {
        status.extend(extra);
    }
    entry
}

/// An SSE event body `{"model": model, "event": event, "data": data}`.
pub fn event(model: &str, event: &str, data: Value) -> Value {
    json!({ "model": model, "event": event, "data": data })
}

// ---------------------------------------------------------------------------------- scripting --

/// One step of a scripted state transition.
#[derive(Debug, Clone)]
pub enum Step {
    /// Sleep before the next step.
    Delay(Duration),
    /// Insert or replace the catalog entry with the same `id`.
    SetModel(Value),
    /// Drop the catalog entry with this id.
    RemoveModel(String),
    /// Send `data: <json>\n\n` to every open SSE stream.
    Sse(Value),
    /// Send these exact bytes (as text) to every open SSE stream: split frames, CRLF, comments,
    /// malformed JSON.
    SseRaw(String),
    /// Send these exact bytes to every open SSE stream (a multi-byte character split in two).
    SseBytes(Vec<u8>),
    /// Wait until at least this many SSE streams are open (up to 5 s), so a following
    /// [`Step::Sse`] cannot be sent before the client subscribed.
    WaitForSse(usize),
    /// Close every open SSE stream (the client sees a clean end of stream).
    CloseSse,
    /// Wait until the server has received at least `count` requests of this method and path (up to
    /// 5 s): a transition ordered by what the client has asked, not by the clock.
    WaitForRequests {
        /// `GET`, `POST`, ...
        method: &'static str,
        /// The request path, query excluded.
        path: &'static str,
        /// How many such requests must have arrived.
        count: usize,
    },
}

impl Step {
    /// `Step::Delay` in milliseconds.
    pub fn delay_ms(ms: u64) -> Self {
        Self::Delay(Duration::from_millis(ms))
    }
}

/// What the server answers for an overridden `(method, path)`.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A JSON body with this status.
    Json(u16, Value),
    /// A raw text body with this status (not JSON, empty, truncated JSON).
    Raw(u16, String),
    /// Accept the request and never answer it.
    Hang,
    /// Wait, then answer with the inner reply (a slow poll).
    After(Duration, Box<Reply>),
    /// Hold the request until the test calls `notify_one`, then answer with the inner reply: a
    /// reply ordered by something the client has done, not by the clock.
    Gated(Arc<Notify>, Box<Reply>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    Load,
    Unload,
    Download,
}

struct Override {
    method: String,
    path: String,
    reply: Reply,
    remaining: Option<usize>,
}

/// One request the server received.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// `GET`, `POST`, ...
    pub method: String,
    /// The request target as sent, query included: `/models?reload=1`.
    pub target: String,
    /// Header `(name, value)` pairs, names lower-cased.
    pub headers: Vec<(String, String)>,
    /// The request body, empty when none.
    pub body: String,
}

impl RecordedRequest {
    /// The target without its query string.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or(&self.target)
    }

    /// The query string without the `?`.
    pub fn query(&self) -> Option<&str> {
        self.target.split_once('?').map(|(_, query)| query)
    }

    /// A header value by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body parsed as JSON (`Value::Null` when it is not).
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

struct State {
    models: Vec<Value>,
    props: Value,
    bearer: Option<String>,
    requests: Vec<RecordedRequest>,
    subscribers: Vec<mpsc::UnboundedSender<Vec<u8>>>,
    sse_connections: usize,
    scripts: Vec<(Trigger, String, Vec<Step>)>,
    overrides: Vec<Override>,
    tasks: Vec<JoinHandle<()>>,
}

struct Inner {
    state: Mutex<State>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn broadcast(&self, frame: Vec<u8>) {
        self.state()
            .subscribers
            .retain(|subscriber| subscriber.send(frame.clone()).is_ok());
    }

    async fn run(self: &Arc<Self>, steps: Vec<Step>) {
        for step in steps {
            match step {
                Step::Delay(duration) => tokio::time::sleep(duration).await,
                Step::SetModel(entry) => {
                    let id = entry.get("id").cloned();
                    let mut state = self.state();
                    match state
                        .models
                        .iter_mut()
                        .find(|existing| existing.get("id") == id.as_ref())
                    {
                        Some(existing) => *existing = entry,
                        None => state.models.push(entry),
                    }
                }
                Step::RemoveModel(id) => self
                    .state()
                    .models
                    .retain(|existing| existing.get("id").and_then(Value::as_str) != Some(&id)),
                Step::Sse(value) => self.broadcast(wire::sse_frame(&value).into_bytes()),
                Step::SseRaw(raw) => self.broadcast(raw.into_bytes()),
                Step::SseBytes(bytes) => self.broadcast(bytes),
                Step::WaitForSse(count) => self.wait_for_sse(count).await,
                Step::CloseSse => self.state().subscribers.clear(),
                Step::WaitForRequests {
                    method,
                    path,
                    count,
                } => self.wait_for_requests(method, path, count).await,
            }
        }
    }

    async fn wait_for_sse(&self, count: usize) {
        for _ in 0..1000 {
            if self.state().subscribers.len() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("no SSE subscriber after 5s");
    }

    async fn wait_for_requests(&self, method: &str, path: &str, count: usize) {
        for _ in 0..1000 {
            let seen = self
                .state()
                .requests
                .iter()
                .filter(|request| request.method == method && request.path() == path)
                .count();
            if seen >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("fewer than {count} {method} {path} requests after 5s");
    }

    fn spawn_script(self: &Arc<Self>, trigger: Trigger, model: &str) {
        let steps: Vec<Step> = self
            .state()
            .scripts
            .iter()
            .filter(|(candidate, id, _)| *candidate == trigger && id == model)
            .flat_map(|(_, _, steps)| steps.clone())
            .collect();
        if steps.is_empty() {
            return;
        }
        let inner = self.clone();
        let handle = tokio::spawn(async move { inner.run(steps).await });
        self.state().tasks.push(handle);
    }
}

// ---------------------------------------------------------------------------------------- server --

/// The fake. Dropping it stops the listener, the scripts and every SSE stream.
pub struct FakeLlamaServer {
    url: String,
    inner: Arc<Inner>,
    accept: JoinHandle<()>,
}

impl Drop for FakeLlamaServer {
    fn drop(&mut self) {
        self.accept.abort();
        let mut state = self.inner.state();
        for task in state.tasks.drain(..) {
            task.abort();
        }
        state.subscribers.clear();
    }
}

impl FakeLlamaServer {
    /// Bind `127.0.0.1:0` and start serving an empty router-mode catalog.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                models: Vec::new(),
                props: wire::router_props(true),
                bearer: None,
                requests: Vec::new(),
                subscribers: Vec::new(),
                sse_connections: 0,
                scripts: Vec::new(),
                overrides: Vec::new(),
                tasks: Vec::new(),
            }),
        });
        let served = inner.clone();
        let accept = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(handle_connection(served.clone(), socket));
            }
        });
        Self {
            url: format!("http://{addr}"),
            inner,
            accept,
        }
    }

    /// [`Self::start`] with an initial catalog.
    pub async fn with_models(models: Vec<Value>) -> Self {
        let server = Self::start().await;
        server.set_models(models);
        server
    }

    /// `http://127.0.0.1:<port>`: the server URL a `LlamaClient` is built with.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Replace the whole catalog.
    pub fn set_models(&self, models: Vec<Value>) {
        self.inner.state().models = models;
    }

    /// Insert or replace one catalog entry, matched on `id`.
    pub fn set_model(&self, entry: Value) {
        let id = entry.get("id").cloned();
        let mut state = self.inner.state();
        match state
            .models
            .iter_mut()
            .find(|existing| existing.get("id") == id.as_ref())
        {
            Some(existing) => *existing = entry,
            None => state.models.push(entry),
        }
    }

    /// The current catalog.
    pub fn models(&self) -> Vec<Value> {
        self.inner.state().models.clone()
    }

    /// Replace the `GET /props` body.
    pub fn set_props(&self, props: Value) {
        self.inner.state().props = props;
    }

    /// Reject every request lacking `Authorization: Bearer <key>` with `401`.
    pub fn require_bearer(&self, key: &str) {
        self.inner.state().bearer = Some(key.to_string());
    }

    /// Script run after each `POST /models/load` for `model`.
    pub fn on_load(&self, model: &str, steps: Vec<Step>) {
        self.add_script(Trigger::Load, model, steps);
    }

    /// Script run after each `POST /models/unload` for `model`.
    pub fn on_unload(&self, model: &str, steps: Vec<Step>) {
        self.add_script(Trigger::Unload, model, steps);
    }

    /// Script run after each `POST /models` (download) for `model`.
    pub fn on_download(&self, model: &str, steps: Vec<Step>) {
        self.add_script(Trigger::Download, model, steps);
    }

    fn add_script(&self, trigger: Trigger, model: &str, steps: Vec<Step>) {
        self.inner
            .state()
            .scripts
            .push((trigger, model.to_string(), steps));
    }

    /// Run `steps` now, on a background task.
    pub fn run(&self, steps: Vec<Step>) {
        let inner = self.inner.clone();
        let handle = tokio::spawn(async move { inner.run(steps).await });
        self.inner.state().tasks.push(handle);
    }

    /// Send one SSE event to every open stream, framed as llama.cpp frames it
    /// ([`wire::sse_frame`]).
    ///
    /// [`wire::sse_frame`]: cyrup_llama_cpp_wire::router::sse_frame
    pub fn broadcast(&self, event: Value) {
        self.inner.broadcast(wire::sse_frame(&event).into_bytes());
    }

    /// Close every open SSE stream.
    pub fn close_sse(&self) {
        self.inner.state().subscribers.clear();
    }

    /// Answer every `(method, path)` request with `reply` instead of the normal handler. `path`
    /// has no query string.
    pub fn respond(&self, method: &str, path: &str, reply: Reply) {
        self.add_override(method, path, reply, None);
    }

    /// [`Self::respond`] for the next `times` matching requests only.
    pub fn respond_times(&self, method: &str, path: &str, reply: Reply, times: usize) {
        self.add_override(method, path, reply, Some(times));
    }

    fn add_override(&self, method: &str, path: &str, reply: Reply, remaining: Option<usize>) {
        self.inner.state().overrides.push(Override {
            method: method.to_string(),
            path: path.to_string(),
            reply,
            remaining,
        });
    }

    /// Drop every [`Self::respond`] override.
    pub fn clear_overrides(&self) {
        self.inner.state().overrides.clear();
    }

    /// Every request received so far, in arrival order.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.inner.state().requests.clone()
    }

    /// The requests for `method` + `path` (query ignored).
    pub fn requests_to(&self, method: &str, path: &str) -> Vec<RecordedRequest> {
        self.requests()
            .into_iter()
            .filter(|request| request.method == method && request.path() == path)
            .collect()
    }

    /// Wait (up to 5 s) until at least `count` requests for `method` + `path` have arrived.
    pub async fn wait_for_requests(&self, method: &str, path: &str, count: usize) {
        self.inner.wait_for_requests(method, path, count).await;
    }

    /// How many `GET /models/sse` streams have ever been opened.
    pub fn sse_connections(&self) -> usize {
        self.inner.state().sse_connections
    }

    /// Wait (up to 5 s) until at least `count` SSE streams are open, so a scripted event is not
    /// sent before the client subscribed.
    ///
    /// Streams are counted until a broadcast finds them closed, so use it for "the client has
    /// subscribed", not for "the client has left".
    pub async fn wait_for_sse(&self, count: usize) {
        self.inner.wait_for_sse(count).await;
    }
}

// ------------------------------------------------------------------------------------ wire level --

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

async fn write_response(socket: &mut TcpStream, status: u16, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(body.as_bytes()).await;
    let _ = socket.flush().await;
}

/// Read one HTTP/1.1 request: head up to the blank line, then `content-length` body bytes.
async fn read_request(socket: &mut TcpStream) -> Option<RecordedRequest> {
    let mut data: Vec<u8> = Vec::new();
    let head_end = loop {
        if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break position;
        }
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < length {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Some(RecordedRequest {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

async fn handle_connection(inner: Arc<Inner>, mut socket: TcpStream) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    let method = request.method.clone();
    let path = request.path().to_string();
    let model = request
        .json()
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let (bearer, reply) = {
        let mut state = inner.state();
        state.requests.push(request.clone());
        let reply = state
            .overrides
            .iter_mut()
            .find(|candidate| {
                candidate.method == method
                    && candidate.path == path
                    && candidate.remaining != Some(0)
            })
            .map(|candidate| {
                if let Some(remaining) = &mut candidate.remaining {
                    *remaining -= 1;
                }
                candidate.reply.clone()
            });
        (state.bearer.clone(), reply)
    };

    if let Some(key) = bearer
        && request.header("authorization") != Some(format!("Bearer {key}").as_str())
    {
        let (status, body) = wire::invalid_api_key();
        write_response(&mut socket, status, &body.to_string()).await;
        return;
    }
    let mut reply = reply;
    loop {
        match reply {
            Some(Reply::After(delay, inner_reply)) => {
                tokio::time::sleep(delay).await;
                reply = Some(*inner_reply);
            }
            Some(Reply::Gated(gate, inner_reply)) => {
                gate.notified().await;
                reply = Some(*inner_reply);
            }
            _ => break,
        }
    }
    match reply {
        Some(Reply::After(..) | Reply::Gated(..)) => {}
        Some(Reply::Json(status, body)) => {
            write_response(&mut socket, status, &body.to_string()).await;
            return;
        }
        Some(Reply::Raw(status, body)) => {
            write_response(&mut socket, status, &body).await;
            return;
        }
        Some(Reply::Hang) => {
            // Hold the socket open, and read until the client hangs up.
            let mut sink = [0_u8; 64];
            while socket.read(&mut sink).await.is_ok_and(|read| read > 0) {}
            return;
        }
        None => {}
    }

    match (method.as_str(), path.as_str()) {
        ("GET", "/models") => {
            let body = wire::models_envelope(inner.state().models.clone()).to_string();
            write_response(&mut socket, 200, &body).await;
        }
        ("GET", "/props") => {
            let body = inner.state().props.to_string();
            write_response(&mut socket, 200, &body).await;
        }
        ("POST", "/models/load") => {
            write_response(&mut socket, 200, &wire::success().to_string()).await;
            inner.spawn_script(Trigger::Load, &model);
        }
        ("POST", "/models/unload") => {
            write_response(&mut socket, 200, &wire::success().to_string()).await;
            inner.spawn_script(Trigger::Unload, &model);
        }
        ("POST", "/models") => {
            write_response(&mut socket, 200, &wire::success().to_string()).await;
            inner.spawn_script(Trigger::Download, &model);
        }
        ("GET", "/models/sse") => serve_sse(inner, socket).await,
        _ => {
            let (status, body) = wire::file_not_found();
            write_response(&mut socket, status, &body.to_string()).await;
        }
    }
}

async fn serve_sse(inner: Arc<Inner>, mut socket: TcpStream) {
    let (sender, mut receiver) = mpsc::unbounded_channel::<Vec<u8>>();
    {
        let mut state = inner.state();
        state.subscribers.push(sender);
        state.sse_connections += 1;
    }
    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n";
    if socket.write_all(head.as_bytes()).await.is_err() || socket.flush().await.is_err() {
        return;
    }
    while let Some(frame) = receiver.recv().await {
        if socket.write_all(frame.as_slice()).await.is_err() || socket.flush().await.is_err() {
            return;
        }
    }
    let _ = socket.shutdown().await;
}

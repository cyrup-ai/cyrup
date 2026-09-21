//! A fake herdr API server: a real `UnixListener` in a tempdir speaking the real framing.
//!
//! It reproduces `handle_connection_with_stop` (`tmp/herdr/src/api/server.rs:156-317`) exactly
//! where it matters and nowhere else: accept, read **one** `\n`-terminated line, write **one**
//! `\n`-terminated line, close. No read loop, because herdr has none — a fake that answered twice
//! on one connection would let a client that pipelines pass here and hang against a real herdr.
//!
//! Precedent for the shape: `crates/cyrup-intercom/src/transport/client.rs:1467-1510` (bind a
//! `UnixListener` in a tempdir, spawn a task that reads a line and writes a line).
//!
//! **No test in this crate requires the `herdr` binary.** It is not installed in this container and
//! neither is ghostty; everything here drives a socket this workspace created.
//!
//! Unix-only: `tokio::net::UnixListener` has no Windows equivalent and
//! `tokio::net::windows::named_pipe::ServerOptions` is a different API. The Windows arm of
//! [`crate::transport`] is compiled and type-checked on Windows targets but is not exercised here.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// What the fake does with a connection once it has read the request line.
#[derive(Clone)]
pub(crate) enum Reply {
    /// Write this line (a `\n` is appended) and close.
    Line(String),
    /// Read the request, then hold the connection open forever without writing. This is the
    /// "herdr accepted but the UI never answered" case, which has **no** server-side deadline
    /// (`tmp/herdr/src/api/server.rs:911-913` is a `recv()` with `None` timeout).
    Silence,
    /// Close without writing anything.
    Close,
    /// Hold the request for this long, then write the line and close.
    ///
    /// herdr's four in-band waits behave exactly like this: `pane.wait_for_output` reads the
    /// request, polls the pane on a 100 ms tick, and writes its single answer whenever the match
    /// lands (`tmp/herdr/src/api/wait.rs:52-110`). It is the only way to observe that a client's
    /// deadline is the one it derived rather than its default.
    After(std::time::Duration, String),
    /// Hand the whole connection to `script`, which owns it from here: it may write any number of
    /// `\n`-terminated lines, at any time, and close whenever it likes.
    ///
    /// This is the only shape that can reproduce `events.subscribe`
    /// (`tmp/herdr/src/api/server.rs:715-779`), where the first line is an acknowledgement and
    /// every later line is a pushed event on the same connection. `Line`/`After` cannot: they
    /// write once and close, which is every *other* method.
    Script(Arc<dyn Fn(StreamHandle) -> BoxFuture + Send + Sync>),
}

/// The return type of a [`Reply::Script`] body.
pub(crate) type BoxFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// The write half of a connection a [`Reply::Script`] has taken over.
pub(crate) struct StreamHandle {
    stream: UnixStream,
}

impl StreamHandle {
    /// Write one `\n`-terminated line, exactly as herdr's `write_text_line` does
    /// (`tmp/herdr/src/api/server.rs:781-785`).
    ///
    /// Errors are swallowed: a client that has closed its end mid-script is a case several tests
    /// create on purpose, and herdr tolerates it too (`write_json_line` →
    /// `is_connection_closed_error`, `:760-770`).
    pub(crate) async fn write_line(&mut self, line: &str) {
        let _ = self.stream.write_all(line.as_bytes()).await;
        let _ = self.stream.write_all(b"\n").await;
        let _ = self.stream.flush().await;
    }

    /// Hold the connection open, writing nothing further, until the fake is dropped.
    pub(crate) async fn hold(self) {
        std::future::pending::<()>().await;
    }
}

/// Box a `Reply::Script` body.
pub(crate) fn script<F, Fut>(body: F) -> Reply
where
    F: Fn(StreamHandle) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    Reply::Script(Arc::new(move |handle| Box::pin(body(handle))))
}

/// A running fake herdr server.
///
/// Dropping it drops the accept task and the tempdir holding the socket.
pub(crate) struct FakeHerdr {
    path: PathBuf,
    received: Arc<Mutex<Vec<String>>>,
    accept: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl FakeHerdr {
    /// Start a fake that answers every connection with `reply`.
    pub(crate) fn start(reply: Reply) -> Self {
        Self::start_with(move |_request| reply.clone())
    }

    /// Start a fake whose answer is computed from the request line it received.
    pub(crate) fn start_with<F>(responder: F) -> Self
    where
        F: Fn(&str) -> Reply + Send + Sync + 'static,
    {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).expect("bind the fake herdr socket");
        let received = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&received);
        let responder = Arc::new(responder);

        let accept = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let recorder = Arc::clone(&recorder);
                let responder = Arc::clone(&responder);
                tokio::spawn(async move {
                    serve_one(stream, &recorder, responder.as_ref()).await;
                });
            }
        });

        Self {
            path,
            received,
            accept,
            _dir: dir,
        }
    }

    /// The socket path to hand a client.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the same tempdir that nothing is listening on — the "herdr crashed, the socket
    /// is gone" case.
    pub(crate) fn dead_path(&self) -> PathBuf {
        self.path.with_file_name("herdr-dead.sock")
    }

    /// Every request line the fake has read so far, in arrival order.
    pub(crate) fn received(&self) -> Vec<String> {
        self.received.lock().expect("recorder mutex").clone()
    }
}

/// One connection: read one line, record it, write at most one line, close.
async fn serve_one<F>(stream: UnixStream, recorder: &Mutex<Vec<String>>, responder: &F)
where
    F: Fn(&str) -> Reply,
{
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    let request = line.trim_end_matches(['\r', '\n']).to_owned();
    recorder
        .lock()
        .expect("recorder mutex")
        .push(request.clone());

    match responder(&request) {
        Reply::Line(answer) => {
            let stream = reader.get_mut();
            let _ = stream.write_all(answer.as_bytes()).await;
            let _ = stream.write_all(b"\n").await;
            let _ = stream.flush().await;
        }
        Reply::Silence => std::future::pending::<()>().await,
        Reply::Close => {}
        Reply::After(delay, answer) => {
            tokio::time::sleep(delay).await;
            let stream = reader.get_mut();
            let _ = stream.write_all(answer.as_bytes()).await;
            let _ = stream.write_all(b"\n").await;
            let _ = stream.flush().await;
        }
        Reply::Script(body) => {
            body(StreamHandle {
                stream: reader.into_inner(),
            })
            .await;
        }
    }
}

/// A well-formed `pong`, echoing the request's `id` — what a real herdr writes
/// (`tmp/herdr/src/api/server.rs:356-363`).
pub(crate) fn pong_for(request: &str, capabilities: Option<&str>) -> Reply {
    let id = request_id(request);
    let capabilities =
        capabilities.map_or_else(|| "null".to_owned(), std::borrow::ToOwned::to_owned);
    Reply::Line(format!(
        r#"{{"id":"{id}","result":{{"type":"pong","version":"0.9.1","protocol":22,"capabilities":{capabilities}}}}}"#
    ))
}

/// A success envelope echoing the request's `id`, carrying `result` verbatim.
///
/// `result` is written as raw JSON rather than built from this crate's own types on purpose: a
/// fake that serialised [`crate::schema::ResponseResult`] would agree with the client by
/// construction and could never catch a field this crate spells differently from herdr.
pub(crate) fn result_for(request: &str, result: &str) -> Reply {
    Reply::Line(compact(&format!(
        r#"{{"id":"{}","result":{result}}}"#,
        request_id(request)
    )))
}

/// [`result_for`], held for `delay` first.
pub(crate) fn result_after(request: &str, delay: std::time::Duration, result: &str) -> Reply {
    Reply::After(
        delay,
        compact(&format!(
            r#"{{"id":"{}","result":{result}}}"#,
            request_id(request)
        )),
    )
}

/// Re-emit `json` as **one line**, and fail loudly if it is not valid JSON.
///
/// Both halves matter. The framing is one JSON value per `\n`-terminated line
/// (`tmp/herdr/src/api/server.rs:781-785`), so a fixture written across several source lines would
/// be delivered to the client as a truncated first line — a real herdr never does that, and a test
/// that accidentally did would be measuring the wrong failure. And a typo inside a fixture must
/// surface here, in the fake, rather than as a client-side `Malformed` that reads exactly like the
/// bug the test is hunting.
pub(crate) fn compact(json: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(json)
        .unwrap_or_else(|err| panic!("a test fixture must be valid JSON: {err}\n{json}"));
    serde_json::to_string(&value).expect("a decoded Value re-serialises")
}

/// The `params` object of a recorded request line, as raw JSON.
///
/// Every assertion about what this client *sends* goes through here or through the whole line,
/// because the request bytes are the half a client cannot see from its own return value.
pub(crate) fn request_params(request: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(request)
        .ok()
        .and_then(|value| value.get("params").cloned())
        .unwrap_or(serde_json::Value::Null)
}

/// The `method` field of a recorded request line.
pub(crate) fn request_method(request: &str) -> String {
    serde_json::from_str::<serde_json::Value>(request)
        .ok()
        .and_then(|value| {
            value
                .get("method")
                .and_then(|method| method.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// An error envelope echoing the request's `id`.
pub(crate) fn error_for(request: &str, code: &str, message: &str) -> Reply {
    let id = request_id(request);
    Reply::Line(format!(
        r#"{{"id":"{id}","error":{{"code":"{code}","message":"{message}"}}}}"#
    ))
}

/// The `id` the client sent, pulled back out of the recorded line.
pub(crate) fn request_id(request: &str) -> String {
    serde_json::from_str::<serde_json::Value>(request)
        .ok()
        .and_then(|value| {
            value
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// The acknowledgement herdr writes as the **first** line of an `events.subscribe` stream
/// (`tmp/herdr/src/api/server.rs:749-760`), echoing the request's `id`.
pub(crate) fn subscription_ack_for(request: &str) -> String {
    compact(&format!(
        r#"{{"id":"{}","result":{{"type":"subscription_started"}}}}"#,
        request_id(request)
    ))
}

/// A minimal but complete `session.snapshot` result, as raw JSON.
///
/// Every required field of `SessionSnapshot` and nothing more, so a test that cares about the
/// ordering of a bootstrap is not also testing record decoding — `read_half.rs` already owns that.
pub(crate) fn empty_snapshot_result() -> String {
    compact(
        r#"{"type":"session_snapshot","snapshot":{"version":"0.9.1","protocol":22,
           "focused_workspace_id":"w1","focused_tab_id":"w1:t1","focused_pane_id":"w1:p1",
           "workspaces":[],"tabs":[],"panes":[],"layouts":[],"agents":[]}}"#,
    )
}

/// A lifecycle `pane.created` event line — **`pane_created` on the wire**, snake_case, with a
/// complete `PaneInfo` payload (`tmp/herdr/src/api/schema/events.rs:493-495`).
pub(crate) fn pane_created_event(pane_id: &str) -> String {
    compact(&format!(
        r#"{{"event":"pane_created","data":{{"type":"pane_created","pane":{{
             "pane_id":"{pane_id}","terminal_id":"t-{pane_id}","workspace_id":"w1","tab_id":"w1:t1",
             "focused":false,"agent_status":"idle","revision":7}}}}}}"#
    ))
}

/// A `pane.agent_status_changed` subscription event line — **dotted on the wire**
/// (`tmp/herdr/src/api/schema/events.rs:367-375`).
pub(crate) fn agent_status_changed_event(pane_id: &str, status: &str) -> String {
    compact(&format!(
        r#"{{"event":"pane.agent_status_changed","data":{{
             "pane_id":"{pane_id}","workspace_id":"w1","agent_status":"{status}"}}}}"#
    ))
}

//! R4 — the three **context-usage fields** of `SessionInfo` must be validated, proven over REAL
//! sockets on BOTH sides of the wire.
//!
//! # The hole
//!
//! `isSessionInfo` guards SEVEN optional fields, not four. The last three are checked by a loop
//! rather than by the per-field ladder above them, which is exactly why they were missed:
//!
//! ```text
//! for (const key of ["contextPct", "contextTokens", "contextWindow"] as const) {
//!   if (session[key] !== undefined && typeof session[key] !== "number") {
//!     return false;
//!   }
//! }
//! ```
//! (`v0.9.2 broker/client.ts:182-186`, 5 lines.) A `false` there is a `throw` in the client's
//! switch and `framing.ts:44-51` turns that into `socket.destroy()`.
//!
//! cyrup modelled none of the three, so they fell into `SessionInfo`'s `#[serde(flatten)] extra`
//! capture — a `serde_json::Value` map, which accepts a string, an object, an array, a bool and an
//! explicit `null` alike. That is looser than pi at four broker tags (`session_joined`,
//! `presence_update`, `sessions[]`, `message.from`), and the intercom socket is reachable by every
//! process on the box.
//!
//! # The other direction — the `presence` CLIENT tag obeys a DIFFERENT rule
//!
//! Porting `isSessionInfo`'s rule everywhere would have been the opposite bug. In
//! `case "presence"` the broker runs its own ladder (`v0.9.2 broker/broker.ts:921-950`) in which an
//! explicit `null` is **legal and meaningful**: it CLEARS the field, because the value is genuinely
//! unknown right after a compaction and carrying the stale-high one forward would be a lie. Only a
//! value that is neither `null` nor a number throws (`:924`, `:934`, `:944`). A cyrup broker that
//! rejected `null` there would disconnect a peer pi serves — a denial of service against a session
//! doing nothing wrong. Both rules are probed below, against each other.
//!
//! Every rejection case here is paired with a positive control: the well-formed frames a real pi
//! peer sends must still be served, and the numbers must come out the far side intact.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use crate::common::Broker;
use cyrup_intercom::transport::client::{InboundEvent, IntercomClient};
use cyrup_intercom::transport::framing::{FrameReader, encode_json};
use cyrup_intercom::transport::protocol::{SessionRegistration, now_ms};

/// The three keys, as one list, so no test can silently cover two of them.
const CONTEXT_KEYS: [&str; 3] = ["contextPct", "contextTokens", "contextWindow"];

/// Everything `typeof x !== "number"` is true for, minus `undefined`.
fn non_numbers() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!("42"),
        serde_json::json!(""),
        serde_json::json!({}),
        serde_json::json!({ "pct": 42 }),
        serde_json::json!([]),
        serde_json::json!([42]),
        serde_json::json!(true),
        serde_json::json!(false),
    ]
}

// ---------------------------------------------------------------------------------------------
// Side A — the real broker subprocess, driven by raw framed clients.
// ---------------------------------------------------------------------------------------------

/// A raw length-prefixed-JSON client: it can put ANY frame on the wire, including payload shapes
/// the Rust types cannot express.
struct RawClient {
    stream: UnixStream,
    reader: FrameReader,
    queued: VecDeque<serde_json::Value>,
    buf: Vec<u8>,
}

impl RawClient {
    async fn connect(socket: &Path) -> Self {
        Self {
            stream: UnixStream::connect(socket)
                .await
                .expect("connect to the broker socket"),
            reader: FrameReader::new(),
            queued: VecDeque::new(),
            buf: vec![0u8; 16 * 1024],
        }
    }

    async fn send(&mut self, frame: &serde_json::Value) {
        let bytes = encode_json(frame).expect("encodes");
        self.stream.write_all(&bytes).await.expect("write frame");
    }

    async fn next_frame_within(&mut self, within: Duration) -> Option<serde_json::Value> {
        loop {
            if let Some(v) = self.queued.pop_front() {
                return Some(v);
            }
            let n = match tokio::time::timeout(within, self.stream.read(&mut self.buf)).await {
                Err(_) => return None,
                Ok(Ok(0) | Err(_)) => return None,
                Ok(Ok(n)) => n,
            };
            let frames = self
                .reader
                .push(&self.buf[..n])
                .expect("broker frames are well-formed");
            for payload in frames {
                self.queued
                    .push_back(serde_json::from_slice(&payload).expect("broker frames are JSON"));
            }
        }
    }

    async fn expect_frame(&mut self, ty: &str) -> serde_json::Value {
        loop {
            let Some(v) = self.next_frame_within(Duration::from_secs(5)).await else {
                panic!("no `{ty}` frame: the connection closed or went quiet");
            };
            if v["type"] == ty {
                return v;
            }
        }
    }

    async fn register(&mut self, session_id: &str) {
        self.send(&serde_json::json!({
            "type": "register",
            "sessionId": session_id,
            "session": {
                "name": session_id,
                "cwd": "/tmp/work",
                "model": "test-model",
                "pid": std::process::id(),
                "startedAt": 0,
                "lastActivity": 0,
            },
        }))
        .await;
        assert_eq!(
            self.expect_frame("registered").await["sessionId"],
            session_id
        );
    }

    /// Assert the broker destroyed this connection. A `list` is queued first so a broker that
    /// merely *ignored* the hostile frame would answer and fail the assertion. The probe write is
    /// fallible on purpose: a destroy is exactly the case where the peer may already be gone, so an
    /// `EPIPE` here is a pass, not a test error.
    async fn assert_destroyed(&mut self, what: &str) {
        let probe = encode_json(&serde_json::json!({ "type": "list", "requestId": "probe" }))
            .expect("encodes");
        if self.stream.write_all(&probe).await.is_err() {
            return;
        }
        let frame = self.next_frame_within(Duration::from_secs(5)).await;
        assert!(
            frame.is_none(),
            "the broker must destroy the connection for {what}, but it answered with {frame:?}"
        );
    }

    /// Assert the broker did NOT destroy this connection — the half a blanket "reject everything"
    /// fix would fail.
    async fn assert_alive(&mut self, what: &str) {
        self.send(&serde_json::json!({ "type": "list", "requestId": "alive" }))
            .await;
        let frame = self.expect_frame("sessions").await;
        assert_eq!(
            frame["requestId"], "alive",
            "the broker must keep serving after {what}"
        );
    }

    /// The `sessions[]` entry for `session_id` from a fresh `list` — the third of the four broker
    /// tags that carry a `SessionInfo`.
    async fn list_entry(&mut self, session_id: &str) -> serde_json::Value {
        self.send(&serde_json::json!({ "type": "list", "requestId": "entry" }))
            .await;
        let frame = self.expect_frame("sessions").await;
        frame["sessions"]
            .as_array()
            .expect("`sessions` is an array")
            .iter()
            .find(|s| s["id"] == session_id)
            .unwrap_or_else(|| panic!("no `sessions[]` entry for {session_id}"))
            .clone()
    }
}

/// **The hole, live.** A `presence` frame whose context field is neither `null` nor a number is a
/// `throw` out of `case "presence"` (`v0.9.2 broker/broker.ts:924,934,944`), i.e. `socket.destroy`.
/// Before this fix the broker read only `name`/`status`/`model` and ignored these three outright,
/// so every frame below was quietly accepted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_with_a_non_numeric_context_field_destroys_the_connection() {
    let broker = Broker::start().await;
    for key in CONTEXT_KEYS {
        for bad in non_numbers() {
            let mut c = RawClient::connect(&broker.socket).await;
            c.register(&format!("s-{key}")).await;
            let mut frame = serde_json::json!({ "type": "presence" });
            frame[key] = bad.clone();
            c.send(&frame).await;
            c.assert_destroyed(&format!("`presence.{key}` = {bad}"))
                .await;
        }
    }
}

/// **Positive control, and the behaviour half.** A number SETS, an explicit `null` CLEARS, and an
/// absent key leaves the field untouched (`v0.9.2 broker/broker.ts:921-950`). All three outcomes
/// are observed on a *peer's* `presence_update` and in a `list` — i.e. on the wire, not in a
/// struct — because the broker only reaches a pi peer through those.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_context_fields_are_set_cleared_and_relayed_to_peers() {
    let broker = Broker::start().await;
    let mut beta = RawClient::connect(&broker.socket).await;
    beta.register("beta-session").await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha-session").await;
    let _ = beta.expect_frame("session_joined").await;

    // SET all three.
    alpha
        .send(&serde_json::json!({
            "type": "presence",
            "contextPct": 42, "contextTokens": 128000, "contextWindow": 200000,
        }))
        .await;
    let update = beta.expect_frame("presence_update").await;
    assert_eq!(update["session"]["id"], "alpha-session");
    assert_eq!(
        update["session"]["contextPct"], 42,
        "a number must SET the field"
    );
    assert_eq!(
        update["session"]["contextTokens"], 128000,
        "an integer must survive the relay as an integer, not as 128000.0"
    );
    assert_eq!(update["session"]["contextWindow"], 200000);

    // The same values must also reach the `sessions[]` tag (`list`), the third `SessionInfo` site.
    let entry = beta.list_entry("alpha-session").await;
    assert_eq!(entry["contextPct"], 42);
    assert_eq!(entry["contextTokens"], 128000);
    assert_eq!(entry["contextWindow"], 200000);

    // An ABSENT key must leave the field untouched — pi's `!== undefined` arm. A status change
    // gives the broadcast something to carry.
    alpha
        .send(&serde_json::json!({ "type": "presence", "status": "thinking" }))
        .await;
    let update = beta.expect_frame("presence_update").await;
    assert_eq!(update["session"]["status"], "thinking");
    assert_eq!(
        update["session"]["contextPct"], 42,
        "an absent key must leave the field untouched, not clear it"
    );

    // An explicit `null` must CLEAR — the key is gone, not present-and-null
    // (`delete session.info.contextPct`, `v0.9.2 broker/broker.ts:923`).
    alpha
        .send(&serde_json::json!({ "type": "presence", "contextPct": null }))
        .await;
    let update = beta.expect_frame("presence_update").await;
    assert!(
        update["session"].get("contextPct").is_none(),
        "an explicit null must DELETE the key, but the frame was {update}"
    );
    assert_eq!(
        update["session"]["contextTokens"], 128000,
        "clearing one context field must not disturb its siblings"
    );
    alpha
        .assert_alive("a `presence` carrying an explicit null context field")
        .await;
}

/// The two rules must not be collapsed into one. `null` is FATAL inside `isSessionInfo`
/// (`v0.9.2 broker/client.ts:182-186`) but LEGAL inside `case "presence"`
/// (`v0.9.2 broker/broker.ts:922-923`), so a broker that reused the stricter rule would disconnect
/// a peer pi serves. Asserted for all three keys, on a live connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_with_an_explicit_null_context_field_is_never_fatal() {
    let broker = Broker::start().await;
    let mut c = RawClient::connect(&broker.socket).await;
    c.register("null-presence-session").await;
    for key in CONTEXT_KEYS {
        // …both when the field was set beforehand and when it was never set at all.
        let mut set = serde_json::json!({ "type": "presence" });
        set[key] = serde_json::json!(7);
        c.send(&set).await;
        for _ in 0..2 {
            let mut clear = serde_json::json!({ "type": "presence" });
            clear[key] = serde_json::Value::Null;
            c.send(&clear).await;
            c.assert_alive(&format!("`presence.{key}` = null")).await;
        }
    }
}

/// `register` must keep IGNORING the context fields, whatever they contain. pi's
/// `isSessionRegistration` does not guard them (`v0.9.2 broker/broker.ts:190-212` checks only
/// `cwd`/`model`/`pid`/`startedAt`/`lastActivity`/`name`/`status`) and the `SessionInfo` it builds
/// is a whitelist that never copies them (`v0.9.2 broker/broker.ts:472-483`). Tightening this to
/// match `isSessionInfo` would be a divergence in the stricter direction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn register_ignores_context_fields_instead_of_rejecting_them() {
    let broker = Broker::start().await;
    let mut observer = RawClient::connect(&broker.socket).await;
    observer.register("observer-session").await;

    let mut c = RawClient::connect(&broker.socket).await;
    c.send(&serde_json::json!({
        "type": "register",
        "sessionId": "junk-context-session",
        "session": {
            "cwd": "/tmp/work", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0,
            // Garbage pi accepts here because its registration guard never looks.
            "contextPct": "not-a-number", "contextTokens": {}, "contextWindow": null,
        },
    }))
    .await;
    assert_eq!(
        c.expect_frame("registered").await["sessionId"],
        "junk-context-session"
    );

    // …and none of it may leak into the broadcast SessionInfo, or every pi peer on this broker
    // would destroy its own socket on the resulting `session_joined`.
    let joined = observer.expect_frame("session_joined").await;
    assert_eq!(joined["session"]["id"], "junk-context-session");
    for key in CONTEXT_KEYS {
        assert!(
            joined["session"].get(key).is_none(),
            "`register` must drop `{key}`, but `session_joined` carried {joined}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Side B — the real `IntercomClient` against a hostile broker listener.
//
// `session_joined`/`presence_update`/`sessions[]`/`message.from` only ever travel broker -> client,
// so proving the guard needs a listener that speaks the framing protocol and then lies. pi's client
// throws out of its own switch on an `isSessionInfo` failure
// (`v0.9.2 broker/client.ts:433-435,476-478,485-487,494-496,516-518`) and `framing.ts:44-51`
// destroys the socket; cyrup's equivalent is a decode failure in `read_task`, observable as an
// `InboundEvent::Disconnected`.
// ---------------------------------------------------------------------------------------------

/// A listener that accepts one connection, answers `register` with `registered`, then — once the
/// test releases it — writes whatever frames the test handed it.
struct HostileBroker {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    release: std::sync::Arc<tokio::sync::Notify>,
}

impl HostileBroker {
    fn start(frames: Vec<serde_json::Value>) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("hostile.sock");
        let listener = UnixListener::bind(&socket).expect("bind hostile listener");
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let gate = release.clone();
        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut reader = FrameReader::new();
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                let Ok(got) = reader.push(&buf[..n]) else {
                    return;
                };
                if !got.is_empty() {
                    break;
                }
            }
            let ack = encode_json(&serde_json::json!({ "type": "registered", "sessionId": "s1" }))
                .expect("encodes");
            if stream.write_all(&ack).await.is_err() {
                return;
            }
            gate.notified().await;
            for frame in frames {
                let bytes = encode_json(&frame).expect("encodes");
                if stream.write_all(&bytes).await.is_err() {
                    return;
                }
            }
            // Hold the socket open so a client disconnect can only come from the client itself.
            std::future::pending::<()>().await;
        });
        Self {
            _dir: dir,
            socket,
            release,
        }
    }
}

fn registration() -> SessionRegistration {
    SessionRegistration {
        // ICOM-041: `runtimeFallbackAlias` (v0.10.1 types.ts:6-7) — these fixtures
        // register under a REAL name, not a synthesized unnamed-runtime alias.
        runtime_fallback_alias: None,
        name: Some("probe".to_string()),
        cwd: "/tmp/work".to_string(),
        model: "test-model".to_string(),
        pid: std::process::id().into(),
        started_at: now_ms().into(),
        last_activity: now_ms().into(),
        status: None,
        tmux_pane: None,
        extra: Default::default(),
    }
}

/// Connect a real client to a hostile broker that emits `frame`; return the events it surfaced,
/// stopping at the first `Disconnected`.
async fn client_events_on(frame: serde_json::Value) -> Vec<InboundEvent> {
    let broker = HostileBroker::start(vec![frame]);
    let client = IntercomClient::connect(&broker.socket, registration(), None)
        .await
        .expect("the hostile broker's `registered` ack still completes the handshake");
    let mut events = client.subscribe();
    broker.release.notify_one();
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(event)) => {
                let done = matches!(event, InboundEvent::Disconnected(_));
                seen.push(event);
                if done {
                    return seen;
                }
            }
            Ok(Err(_)) | Err(_) => return seen,
        }
    }
}

async fn client_disconnects_on(frame: serde_json::Value) -> bool {
    client_events_on(frame)
        .await
        .iter()
        .any(|e| matches!(e, InboundEvent::Disconnected(_)))
}

fn good_session() -> serde_json::Value {
    serde_json::json!({
        "id": "s-peer", "cwd": "/tmp/work", "model": "m", "pid": 1, "startedAt": 2, "lastActivity": 3,
    })
}

/// **The hole, live, in the broker -> client direction.** Every one of the four tags that carries a
/// `SessionInfo` must destroy the connection when a context field is not a number — including an
/// explicit `null`, which is where `isSessionInfo`'s rule and `case "presence"`'s part company.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_destroys_the_connection_on_a_non_numeric_context_field() {
    let good_msg = serde_json::json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } });

    let mut bad_values = non_numbers();
    // `null` is fatal HERE (`v0.9.2 broker/client.ts:183`, `typeof null === "object"`) even though
    // it is legal in a `presence` frame. This single case is the whole reason the two ladders are
    // ported separately.
    bad_values.push(serde_json::Value::Null);

    let mut cases: Vec<(String, serde_json::Value)> = Vec::new();
    for key in CONTEXT_KEYS {
        for bad in &bad_values {
            let mut session = good_session();
            session[key] = bad.clone();
            cases.push((
                format!("`session_joined.session.{key}` = {bad}"),
                serde_json::json!({ "type": "session_joined", "session": session.clone() }),
            ));
            cases.push((
                format!("`presence_update.session.{key}` = {bad}"),
                serde_json::json!({ "type": "presence_update", "session": session.clone() }),
            ));
            cases.push((
                format!("`sessions[0].{key}` = {bad}"),
                serde_json::json!({
                    "type": "sessions", "requestId": "r1", "sessions": [session.clone()],
                }),
            ));
            cases.push((
                format!("`message.from.{key}` = {bad}"),
                serde_json::json!({ "type": "message", "from": session, "message": good_msg }),
            ));
        }
    }

    for (what, frame) in cases {
        assert!(
            client_disconnects_on(frame).await,
            "the client must destroy the connection on {what}"
        );
    }
}

/// **Positive control for side B.** Well-typed numbers are exactly what a real pi peer sends, so
/// they must keep the client connected AND arrive intact — modelled, not dropped. A fix that
/// restored parity by rejecting more, or by decoding the fields into nothing, fails here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_accepts_and_surfaces_well_typed_context_fields() {
    let mut session = good_session();
    session["contextPct"] = serde_json::json!(42);
    session["contextTokens"] = serde_json::json!(128_000);
    session["contextWindow"] = serde_json::json!(200_000);
    // A newer pi broker's genuinely unknown additive key must still pass through untouched.
    session["piFutureKey"] = serde_json::json!({ "nested": [1, 2, 3] });

    let events = client_events_on(
        serde_json::json!({ "type": "session_joined", "session": session.clone() }),
    )
    .await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, InboundEvent::Disconnected(_))),
        "well-typed context fields must not disconnect the client; got {events:?}"
    );
    let Some(InboundEvent::SessionJoined(info)) = events
        .iter()
        .find(|e| matches!(e, InboundEvent::SessionJoined(_)))
    else {
        panic!("no `session_joined` event surfaced; got {events:?}");
    };
    assert_eq!(info.context_pct, Some(serde_json::Number::from(42)));
    assert_eq!(info.context_tokens, Some(serde_json::Number::from(128_000)));
    assert_eq!(info.context_window, Some(serde_json::Number::from(200_000)));
    assert!(
        !info.extra.contains_key("contextPct"),
        "a guarded field must be modelled, not parked in the `extra` catch-all"
    );
    assert_eq!(info.extra["piFutureKey"]["nested"][2], 3);
    // …and the whole thing must re-serialize byte-identically, integers still integers.
    assert_eq!(serde_json::to_value(info).unwrap(), session);

    // Absent is also what pi accepts (all three are optional, `v0.9.2 types.ts:19-21`).
    let events = client_events_on(
        serde_json::json!({ "type": "session_joined", "session": good_session() }),
    )
    .await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, InboundEvent::Disconnected(_))),
        "absent context fields must not disconnect the client; got {events:?}"
    );
}

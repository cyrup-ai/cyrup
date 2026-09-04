//! R5 — cyrup's numeric wire fields must accept **every JSON number**, because that is all pi
//! checks. Proven over REAL sockets on BOTH sides of the wire.
//!
//! # The hole, and which way it points
//!
//! Every numeric field in pi's hand-written guards is guarded as `typeof x === "number"` and
//! nothing more — an IEEE-754 double:
//!
//! ```text
//! || typeof session.pid !== "number"
//! || typeof session.startedAt !== "number"
//! || typeof session.lastActivity !== "number"
//! ```
//! (`v0.9.2 broker/broker.ts:200-202`, mirrored for `SessionInfo` at
//! `v0.9.2 broker/client.ts:163-165`.) So `-1`, `1.5`, `2**32` and `1e300` are all values a
//! conforming pi peer may put on the wire and a pi broker relays without comment.
//!
//! cyrup typed them `u32`/`u64`. That made cyrup **stricter** than pi, and on this socket stricter
//! is the dangerous direction, not the safe one: a required field that fails to decode is a fatal
//! frame (`FrameResult::protocol_error()`), so a value pi handles normally destroyed the
//! connection. That is a denial of service — one a peer that has done nothing wrong suffers.
//!
//! And it **amplifies**. `SessionInfo` is broadcast: a single `register` carrying `pid: -1` is
//! accepted by a pi broker and then relayed to every attached client at four tags
//! (`session_joined`, `presence_update`, `sessions[]`, `message.from`), so one hostile peer would
//! knock over every OTHER cyrup client on a shared broker, none of which ever spoke to it. Side B
//! below is that scenario, live.
//!
//! A quieter consequence rode along: because `send` decodes the message with `.ok()` and falls back
//! to `messageId = isMessage(message) ? message.id : "unknown"`
//! (`v0.9.2 broker/broker.ts:604-613`), a `timestamp: 1.5` that pi *delivers* answered
//! `delivery_failed { messageId: "unknown" }` here — which no pi peer can correlate, so its
//! `pendingSends` entry never settles. A hang, not a rejection. Covered below.
//!
//! Every rejection this file asserts is paired with a positive control, and the acceptance side is
//! asserted for *lossless relay*, not merely for "did not disconnect": an integer must come out the
//! far side an integer and a `-1` must come out `-1`, because pi's broker object-spreads what it
//! was handed (`v0.9.2 broker/broker.ts:672-676`) and a lossy widening would be its own divergence.

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

/// JSON numbers a `u32`/`u64` field cannot hold, every one of which passes `typeof x === "number"`.
///
/// `-1` and `1.5` are the two the audit found live; `4294967296` is `2**32`, which a `u64` field
/// accepted and a `u32` `pid` did not, so it is what separates the two widths; `1e300` is the far
/// end of the double range and `-0.5` pins the negative-fractional corner.
fn js_numbers() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!(4_294_967_296i64),
        serde_json::json!(-0.5),
        serde_json::json!(1e300),
    ]
}

/// Values `typeof x === "number"` is FALSE for. These must STAY fatal — widening the numeric domain
/// must not become "accept anything", or the parity fix would have re-broken the batch it belongs
/// to.
fn non_numbers() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!("1"),
        serde_json::json!(""),
        serde_json::json!({}),
        serde_json::json!([]),
        serde_json::json!([1]),
        serde_json::json!(true),
        serde_json::Value::Null,
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

    /// Register with a `session` payload built from `overrides` on top of a well-formed base.
    async fn register_with(&mut self, session_id: &str, overrides: &serde_json::Value) {
        let mut session = serde_json::json!({
            "name": session_id,
            "cwd": "/tmp/work",
            "model": "test-model",
            "pid": std::process::id(),
            "startedAt": 0,
            "lastActivity": 0,
        });
        for (k, v) in overrides.as_object().expect("overrides is an object") {
            session[k.as_str()] = v.clone();
        }
        self.send(&serde_json::json!({
            "type": "register", "sessionId": session_id, "session": session,
        }))
        .await;
    }

    async fn register(&mut self, session_id: &str) {
        self.register_with(session_id, &serde_json::json!({})).await;
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

    /// Assert the broker did NOT destroy this connection.
    async fn assert_alive(&mut self, what: &str) {
        self.send(&serde_json::json!({ "type": "list", "requestId": "alive" }))
            .await;
        let frame = self.expect_frame("sessions").await;
        assert_eq!(
            frame["requestId"], "alive",
            "the broker must keep serving after {what}"
        );
    }

    /// The `sessions[]` entry for `session_id` from a fresh `list`.
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

/// **The hole, live, in the client -> broker direction.** `isSessionRegistration` accepts any
/// double for `pid`/`startedAt`/`lastActivity` (`v0.9.2 broker/broker.ts:200-202`); a `u32`/`u64`
/// here answered with a destroyed socket instead of a `registered`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn register_accepts_every_json_number_for_pid_started_at_and_last_activity() {
    let broker = Broker::start().await;
    for key in ["pid", "startedAt", "lastActivity"] {
        for value in js_numbers() {
            let id = format!("reg-{key}-{value}");
            let mut c = RawClient::connect(&broker.socket).await;
            c.register_with(&id, &serde_json::json!({ key: value.clone() }))
                .await;
            let ack = c.next_frame_within(Duration::from_secs(5)).await;
            assert_eq!(
                ack.as_ref().map(|f| f["type"].clone()),
                Some(serde_json::json!("registered")),
                "`register` with `{key}` = {value} must be accepted, as pi accepts it; got {ack:?}"
            );
            c.assert_alive(&format!("`register.session.{key}` = {value}"))
                .await;
        }
    }
}

/// The relay half, and the reason the fields are `serde_json::Number` rather than `f64`. The value
/// a `register` carried must reach every peer **byte-identical**, at both `SessionInfo` tags the
/// broker emits it on: pi's broker copies these three across verbatim
/// (`v0.9.2 broker/broker.ts:472-483`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_js_number_registration_relays_losslessly_to_peers() {
    let broker = Broker::start().await;
    let mut observer = RawClient::connect(&broker.socket).await;
    observer.register("observer-session").await;

    let mut c = RawClient::connect(&broker.socket).await;
    c.register_with(
        "odd-numbers-session",
        &serde_json::json!({ "pid": -1, "startedAt": 1.5, "lastActivity": 4_294_967_296i64 }),
    )
    .await;
    assert_eq!(
        c.expect_frame("registered").await["sessionId"],
        "odd-numbers-session"
    );

    let joined = observer.expect_frame("session_joined").await;
    assert_eq!(joined["session"]["id"], "odd-numbers-session");
    assert_eq!(
        joined["session"]["pid"],
        serde_json::json!(-1),
        "a -1 pid must relay as -1"
    );
    assert_eq!(
        joined["session"]["startedAt"],
        serde_json::json!(1.5),
        "a fractional startedAt must relay unrounded"
    );
    assert_eq!(
        joined["session"]["lastActivity"],
        serde_json::json!(4_294_967_296i64)
    );

    let entry = observer.list_entry("odd-numbers-session").await;
    assert_eq!(entry["pid"], serde_json::json!(-1));
    assert_eq!(entry["startedAt"], serde_json::json!(1.5));
}

/// **Positive control for `register`.** The frames a real pi peer sends must still be served, and
/// an integer must still land as an integer — `1700000000000`, never `1700000000000.0`, which is
/// what an `f64` field would have produced and what a `JSON.stringify` diff would catch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn register_still_serves_ordinary_integer_registrations() {
    let broker = Broker::start().await;
    let mut observer = RawClient::connect(&broker.socket).await;
    observer.register("obs").await;

    let mut c = RawClient::connect(&broker.socket).await;
    c.register_with(
        "ordinary-session",
        &serde_json::json!({ "pid": 4321, "startedAt": 1_700_000_000_000i64, "lastActivity": 1_700_000_000_001i64 }),
    )
    .await;
    assert_eq!(
        c.expect_frame("registered").await["sessionId"],
        "ordinary-session"
    );

    let joined = observer.expect_frame("session_joined").await;
    let raw = serde_json::to_string(&joined["session"]).expect("re-serializes");
    assert!(
        raw.contains("\"pid\":4321") && raw.contains("\"startedAt\":1700000000000"),
        "integers must survive the relay as integers, not as floats; got {raw}"
    );
}

/// **Negative control for `register`.** Widening to "any JSON number" must not become "anything".
/// `typeof session.pid !== "number"` is still a `throw` out of `case "register"`
/// (`v0.9.2 broker/broker.ts:429-432`), i.e. `socket.destroy`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn register_with_a_non_number_pid_still_destroys_the_connection() {
    let broker = Broker::start().await;
    for value in non_numbers() {
        let mut c = RawClient::connect(&broker.socket).await;
        c.register_with(
            "bad-pid-session",
            &serde_json::json!({ "pid": value.clone() }),
        )
        .await;
        c.assert_destroyed(&format!("`register.session.pid` = {value}"))
            .await;
    }
}

/// **The hang, live.** `isMessage` guards `timestamp` and the five counters with
/// `typeof === "number"` only (`v0.9.2 broker/broker.ts:147,151-155`), so pi DELIVERS a message
/// whose `timestamp` is `1.5`. cyrup answered `delivery_failed { messageId: "unknown" }` — a reply
/// no pi peer can correlate to its `pendingSends` entry, so the send never settles.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_accepts_every_json_number_and_acks_with_the_real_message_id() {
    let broker = Broker::start().await;
    let mut beta = RawClient::connect(&broker.socket).await;
    beta.register("beta").await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha").await;
    let _ = beta.expect_frame("session_joined").await;

    for key in [
        "timestamp",
        "senderSequence",
        "receiverReceivedAt",
        "injectedAt",
    ] {
        for value in js_numbers() {
            let id = format!("m-{key}-{value}");
            let mut message =
                serde_json::json!({ "id": id, "timestamp": 1, "content": { "text": "hi" } });
            message[key] = value.clone();
            alpha
                .send(&serde_json::json!({ "type": "send", "to": "beta", "message": message }))
                .await;

            let ack = alpha.expect_frame("delivered").await;
            assert_eq!(
                ack["messageId"], id,
                "`send` with `{key}` = {value} must be delivered and acked with the REAL message id"
            );
            let relayed = beta.expect_frame("message").await;
            assert_eq!(
                relayed["message"][key], value,
                "the relayed envelope must carry `{key}` = {value} verbatim"
            );
        }
    }
}

/// **Negative control for `send`.** A non-number is still `isMessage`-fatal, and pi's answer to it
/// is the `"unknown"` fallback, on a connection that SURVIVES
/// (`v0.9.2 broker/broker.ts:604-613`) — so this is the one place `"unknown"` is correct.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_with_a_non_number_timestamp_still_fails_with_unknown_and_keeps_the_connection() {
    let broker = Broker::start().await;
    let mut beta = RawClient::connect(&broker.socket).await;
    beta.register("beta").await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha").await;

    for value in non_numbers() {
        alpha
            .send(&serde_json::json!({
                "type": "send", "to": "beta",
                "message": { "id": "m-bad", "timestamp": value.clone(), "content": { "text": "hi" } },
            }))
            .await;
        let failure = alpha.expect_frame("delivery_failed").await;
        assert_eq!(
            failure["messageId"], "unknown",
            "pi's fallback for an unparseable message"
        );
        assert_eq!(failure["reason"], "Invalid message format");
    }
    alpha
        .assert_alive("a `send` whose message failed `isMessage`")
        .await;
}

/// **Batch 2's newly-extended defect, live.** `MessageReceipt` gained a modelled `timestamp` when
/// the v0.9.2 tags landed, typed `u64`, so a receipt pi validates and forwards
/// (`isMessageReceipt`, `v0.9.2 broker/broker.ts:107-116`) tore the connection down here instead —
/// a `throw` is what pi reserves for a receipt that FAILS the guard (`:805-807`), not one that
/// passes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_receipt_accepts_every_json_number_for_timestamp() {
    let broker = Broker::start().await;
    let mut c = RawClient::connect(&broker.socket).await;
    c.register("receipt-session").await;
    for value in js_numbers() {
        c.send(&serde_json::json!({
            "type": "message_receipt",
            "receipt": { "messageId": "m1", "status": "queued", "timestamp": value.clone() },
        }))
        .await;
        c.assert_alive(&format!("`message_receipt.receipt.timestamp` = {value}"))
            .await;
    }
}

/// **Negative control for the receipt.** A non-number `timestamp` fails `isMessageReceipt` and is
/// still fatal (`v0.9.2 broker/broker.ts:805-807`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_receipt_with_a_non_number_timestamp_still_destroys_the_connection() {
    let broker = Broker::start().await;
    for value in non_numbers() {
        let mut c = RawClient::connect(&broker.socket).await;
        c.register("receipt-session").await;
        c.send(&serde_json::json!({
            "type": "message_receipt",
            "receipt": { "messageId": "m1", "status": "queued", "timestamp": value.clone() },
        }))
        .await;
        c.assert_destroyed(&format!("`message_receipt.receipt.timestamp` = {value}"))
            .await;
    }
}

// ---------------------------------------------------------------------------------------------
// Side B — the real `IntercomClient` against a hostile broker listener.
//
// This is the AMPLIFYING direction: a `SessionInfo` a broker relays reaches clients that never
// spoke to whoever originated it. pi's client throws out of its own switch on an `isSessionInfo`
// failure (`v0.9.2 broker/client.ts:433-435,476-478,485-487,494-496,516-518`) and
// `framing.ts:44-51` destroys the socket; cyrup's equivalent is a decode failure in `read_task`,
// observable as an `InboundEvent::Disconnected`. A number must produce NEITHER.
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

/// Assert a real client survives EVERY frame in `frames`, delivered back-to-back on one connection.
///
/// Batched rather than one-connection-per-frame purely for wall-clock: proving a NON-event costs a
/// full quiet window each time, and a single fatal frame anywhere in the batch still disconnects
/// the client — a decode failure in `read_task` tears down the whole connection, so nothing after
/// it could mask it. Callers keep one batch per broker tag so a failure still says where.
async fn assert_client_survives_all(frames: Vec<serde_json::Value>, what: &str) {
    let broker = HostileBroker::start(frames);
    let client = IntercomClient::connect(&broker.socket, registration(), None)
        .await
        .expect("the hostile broker's `registered` ack still completes the handshake");
    let mut events = client.subscribe();
    broker.release.notify_one();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(InboundEvent::Disconnected(why))) => {
                panic!("the client destroyed the connection on {what}, which pi accepts: {why}")
            }
            Ok(Ok(_)) => {}
            // Lagged: keep draining. Closed cannot happen while `client` is alive.
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => return,
            // A quiet window with no disconnect is the pass.
            Err(_) => return,
        }
    }
}

fn good_session() -> serde_json::Value {
    serde_json::json!({
        "id": "s-peer", "cwd": "/tmp/work", "model": "m", "pid": 1, "startedAt": 2, "lastActivity": 3,
    })
}

fn good_msg() -> serde_json::Value {
    serde_json::json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } })
}

/// The four broker tags that carry a `SessionInfo`, each built from the given session object.
fn session_info_tags(session: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    vec![
        (
            "session_joined".to_string(),
            serde_json::json!({ "type": "session_joined", "session": session }),
        ),
        (
            "presence_update".to_string(),
            serde_json::json!({ "type": "presence_update", "session": session }),
        ),
        (
            "sessions[0]".to_string(),
            serde_json::json!({ "type": "sessions", "requestId": "r1", "sessions": [session] }),
        ),
        (
            "message.from".to_string(),
            serde_json::json!({ "type": "message", "from": session, "message": good_msg() }),
        ),
    ]
}

/// **The amplifying hole, live.** One hostile `register` on a shared broker becomes a relayed
/// `SessionInfo` at four tags, and a `u32` `pid` / `u64` `startedAt` turned each of them into a
/// disconnect for every cyrup client attached — clients that never spoke to the originator. pi
/// accepts all of these (`v0.9.2 broker/client.ts:163-165,178-180`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_survives_a_relayed_session_info_carrying_any_json_number() {
    let mut batches: std::collections::BTreeMap<String, Vec<serde_json::Value>> =
        std::collections::BTreeMap::new();
    for key in ["pid", "startedAt", "lastActivity", "peerUid"] {
        for value in js_numbers() {
            let mut session = good_session();
            session[key] = value.clone();
            for (tag, frame) in session_info_tags(&session) {
                batches.entry(tag).or_default().push(frame);
            }
        }
    }
    for (tag, frames) in batches {
        assert_client_survives_all(
            frames,
            &format!("a `{tag}` whose pid/startedAt/lastActivity/peerUid is any JSON number"),
        )
        .await;
    }
}

/// The same, for the message envelope's own numeric fields — reachable the moment a cyrup client
/// receives a relayed message, and for `brokerReceivedAt`/`brokerDeliveredAt` the value is stamped
/// by the BROKER (`v0.9.2 broker/broker.ts:674-675`), so a cyrup client trusts it sight unseen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_survives_a_relayed_message_carrying_any_json_number() {
    for key in [
        "timestamp",
        "senderSequence",
        "brokerReceivedAt",
        "brokerDeliveredAt",
        "injectedAt",
    ] {
        let frames = js_numbers()
            .into_iter()
            .map(|value| {
                let mut message = good_msg();
                message[key] = value;
                serde_json::json!({ "type": "message", "from": good_session(), "message": message })
            })
            .collect();
        assert_client_survives_all(
            frames,
            &format!("a `message` whose `{key}` is any JSON number"),
        )
        .await;
    }
}

/// `message_receipt` and `message_control` travel broker -> client too, and both carry a
/// `timestamp` pi guards with nothing but `typeof === "number"`
/// (`v0.9.2 broker/client.ts:61,72`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_survives_a_relayed_receipt_or_control_carrying_any_json_number() {
    let receipts = js_numbers()
        .into_iter()
        .map(|value| {
            serde_json::json!({
                "type": "message_receipt", "from": good_session(),
                "receipt": { "messageId": "m1", "status": "queued", "timestamp": value },
            })
        })
        .collect();
    assert_client_survives_all(
        receipts,
        "a `message_receipt` whose timestamp is any JSON number",
    )
    .await;

    let controls = js_numbers()
        .into_iter()
        .map(|value| {
            serde_json::json!({
                "type": "message_control", "from": good_session(),
                "control": { "messageId": "m1", "action": "cancel", "timestamp": value },
            })
        })
        .collect();
    assert_client_survives_all(
        controls,
        "a `message_control` whose timestamp is any JSON number",
    )
    .await;
}

/// **Positive control for side B, and the fidelity assertion.** The frames a real pi peer sends
/// must keep working, the values must be *surfaced* rather than swallowed, and the struct must
/// re-serialize byte-identically — integers still integers, `-1` still `-1`. A fix that widened by
/// decoding into `f64` would turn `startedAt: 2` into `2.0` and fail here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_surfaces_wire_numbers_intact() {
    // Ordinary integers, exactly as a pi broker emits them.
    let session = good_session();
    let events = client_events_on(
        serde_json::json!({ "type": "session_joined", "session": session.clone() }),
    )
    .await;
    let Some(InboundEvent::SessionJoined(info)) = events
        .iter()
        .find(|e| matches!(e, InboundEvent::SessionJoined(_)))
    else {
        panic!("no `session_joined` event surfaced; got {events:?}");
    };
    // Compared through `to_value` rather than against a Rust type on purpose: this file asserts
    // WIRE behaviour, so it must stay compilable against the pre-fix `u32`/`u64` fields — that is
    // what makes the revert proof a genuine test failure rather than a compile error.
    assert_eq!(
        serde_json::to_value(&info.pid).unwrap(),
        serde_json::json!(1)
    );
    assert_eq!(
        serde_json::to_value(&info.started_at).unwrap(),
        serde_json::json!(2)
    );
    assert_eq!(
        serde_json::to_value(info).unwrap(),
        session,
        "integers must round-trip as integers"
    );

    // …and the out-of-domain values too: surfaced, not silently normalised.
    let mut odd = good_session();
    odd["pid"] = serde_json::json!(-1);
    odd["startedAt"] = serde_json::json!(1.5);
    odd["peerUid"] = serde_json::json!(-1);
    let events =
        client_events_on(serde_json::json!({ "type": "session_joined", "session": odd.clone() }))
            .await;
    let Some(InboundEvent::SessionJoined(info)) = events
        .iter()
        .find(|e| matches!(e, InboundEvent::SessionJoined(_)))
    else {
        panic!("no `session_joined` event surfaced for the odd session; got {events:?}");
    };
    assert_eq!(
        serde_json::to_value(info).unwrap(),
        odd,
        "a -1 pid must round-trip as -1"
    );
    assert!(
        !info.extra.contains_key("pid"),
        "a guarded field must be modelled, not parked in the `extra` catch-all"
    );

    // The point-of-use narrowing is where `-1` stops being an OS pid — not at the wire boundary.
    // (Unit-level coverage of the accessors themselves lives in
    // `transport::protocol::tests::the_point_of_use_accessors_refuse_what_is_not_an_integer`.)
    let wire_pid: serde_json::Number = serde_json::from_value(
        serde_json::to_value(&info.pid).expect("the decoded pid re-serializes"),
    )
    .expect("`-1` is a JSON number");
    assert_eq!(
        cyrup_intercom::transport::protocol::as_os_pid(&wire_pid),
        None,
        "`-1` decodes fine but must never narrow to a signallable pid"
    );
}

/// **Negative control for side B.** A non-number is still `isSessionInfo`-fatal
/// (`v0.9.2 broker/client.ts:163-165`), including an explicit `null` — `typeof null === "object"`.
/// The widening must not have turned these four fields into a `serde_json::Value` catch-all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_still_destroys_the_connection_on_a_non_number_session_info_field() {
    for key in ["pid", "startedAt", "lastActivity", "peerUid"] {
        for value in non_numbers() {
            // A required field is fatal even when absent; `peerUid` is optional, so `null` is the
            // only way its absence is expressible — and `null` is fatal there too.
            let mut session = good_session();
            session[key] = value.clone();
            assert!(
                client_disconnects_on(
                    serde_json::json!({ "type": "session_joined", "session": session })
                )
                .await,
                "`session_joined.session.{key}` = {value} is not a number; pi returns false and \
                 destroys the socket"
            );
        }
    }
}

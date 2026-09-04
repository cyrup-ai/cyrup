//! R1 — **array-shaped payloads must be rejected**, proven over REAL sockets on BOTH sides of the
//! wire: a real broker subprocess for the frames a client can send, and a real
//! [`IntercomClient`] against a hostile listener for the frames a broker can send.
//!
//! Batch 2 modelled the v0.9.2 message tags so a pi >= 0.9.0 peer stops tearing the connection
//! down. That widened the set of payload structs serde will decode — and serde's derived
//! `Deserialize` for a plain struct implements `visit_seq`, so a JSON **array** deserializes
//! *positionally*. pi does the opposite: `isMessageReceipt` and `isSessionRegistration` bail on
//! `Array.isArray(value)` outright (`v0.9.2 broker/client.ts:57-59`,
//! `v0.9.2 broker/broker.ts:108-110,191-193`) and the remaining guards reject an array because
//! `[]["id"]` is `undefined` (`v0.9.2 broker/client.ts:106-150,152-189`,
//! `v0.9.2 broker/broker.ts:1159-1168`). Every one of those rejections is a `throw` out of the
//! message switch, which `framing.ts:44-51` turns into `socket.destroy()` — so upstream KILLS the
//! connection on exactly these frames.
//!
//! Confirmed live before the fix: after `register`,
//! `{"type":"message_receipt","receipt":["m1","queued",1,null]}` left the connection alive and
//! serving. This socket is reachable by any process on the box, so a decoder that accepts what pi
//! destroys the connection over is an input-validation hole, not a compatibility win.
//!
//! The fix is the `[MAP-ONLY]` invariant in `crate::transport::protocol`: every payload struct
//! carries a `#[serde(flatten)] extra` capture, which both reproduces pi's object-spread
//! pass-through of unmodelled keys and makes serde derive a map-only visitor. Each test below has a
//! **positive control** in the same file, because "reject everything" would pass a rejection test
//! while being a worse bug than the one it fixes — pi does not disconnect over a well-formed frame,
//! and neither may cyrup.

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

// ---------------------------------------------------------------------------------------------
// Side A — the real broker subprocess, driven by a raw framed client.
// ---------------------------------------------------------------------------------------------

/// A raw length-prefixed-JSON client: it can put ANY frame on the wire, including payload shapes
/// `ClientMessage` cannot express in Rust.
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

    /// The next frame, or `None` if the broker closed the connection (pi's `socket.destroy`).
    async fn next_frame(&mut self) -> Option<serde_json::Value> {
        loop {
            if let Some(v) = self.queued.pop_front() {
                return Some(v);
            }
            let n =
                match tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut self.buf))
                    .await
                    .expect("broker responds or closes within 5s")
                {
                    Ok(0) | Err(_) => return None,
                    Ok(n) => n,
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
            let Some(v) = self.next_frame().await else {
                panic!("connection closed while waiting for a `{ty}` frame");
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
    /// merely *ignored* the hostile frame would answer and fail the assertion — without it, a
    /// broker that accepted the frame and simply had nothing to say would look identical to one
    /// that closed.
    ///
    /// The probe write is *fallible on purpose*. A destroy is exactly the case where the peer may
    /// already be gone, so an `EPIPE` here is a pass, not a test error — `send`'s
    /// `.expect("write frame")` made this assertion fail under CPU contention while passing on an
    /// idle box.
    async fn assert_destroyed(&mut self, what: &str) {
        let probe = encode_json(&serde_json::json!({ "type": "list", "requestId": "probe" }))
            .expect("encodes");
        if self.stream.write_all(&probe).await.is_err() {
            return;
        }
        let frame = self.next_frame().await;
        assert!(
            frame.is_none(),
            "the broker must destroy the connection for {what}, but it answered with {frame:?}"
        );
    }
}

/// A positional `SessionRegistration`: `name, cwd, model, pid, startedAt, lastActivity, status`.
fn array_shaped_registration() -> serde_json::Value {
    serde_json::json!([null, "/tmp/work", "test-model", 4242, 0, 0, null])
}

/// A positional `MessageReceipt`: `messageId, status, timestamp, detail`. This is the exact frame
/// that was confirmed live to leave the connection alive before the fix.
fn array_shaped_receipt() -> serde_json::Value {
    serde_json::json!(["m1", "queued", 1, null])
}

/// A positional `MessageControl`: `messageId, action, timestamp, supersededBy, detail`.
fn array_shaped_control() -> serde_json::Value {
    serde_json::json!(["m1", "cancel", 1, null, null])
}

/// A positional `SessionInfo`: `id, name, cwd, model, pid, startedAt, lastActivity, status,
/// peerUid, trustedLocal`.
fn array_shaped_session_info() -> serde_json::Value {
    serde_json::json!(["s-evil", null, "/tmp/work", "m", 1, 2, 3, null, null, null])
}

/// A positional `ExtensionCapability`: `namespace, ownerEligible`.
fn array_shaped_capability() -> serde_json::Value {
    serde_json::json!(["ns", true])
}

/// `SessionRegistration` — `isSessionRegistration` bails on `Array.isArray` explicitly
/// (`v0.9.2 broker/broker.ts:191-193`) and the failure throws out of `case "register"`
/// (`v0.9.2 broker/broker.ts:429-432`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn register_with_an_array_shaped_session_is_fatal() {
    let broker = Broker::start().await;
    let mut c = RawClient::connect(&broker.socket).await;
    c.send(&serde_json::json!({
        "type": "register", "sessionId": "s1", "session": array_shaped_registration(),
    }))
    .await;
    c.assert_destroyed("an array-shaped `register.session`")
        .await;
}

/// `MessageReceipt` — the confirmed hole. `isMessageReceipt` bails on `Array.isArray`
/// (`v0.9.2 broker/broker.ts:108-110`) and `case "message_receipt"` throws
/// (`v0.9.2 broker/broker.ts:805-807`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_receipt_with_an_array_shaped_receipt_is_fatal() {
    let broker = Broker::start().await;
    let mut c = RawClient::connect(&broker.socket).await;
    c.register("alpha-session").await;
    c.send(&serde_json::json!({ "type": "message_receipt", "receipt": array_shaped_receipt() }))
        .await;
    c.assert_destroyed("an array-shaped `message_receipt.receipt`")
        .await;
}

/// `ExtensionCapability` via `register` — `validateExtensionCapability` rejects an array because
/// `[]["namespace"]` is `undefined` (`v0.9.2 broker/broker.ts:1159-1168`), and `case "register"`
/// throws (`v0.9.2 broker/broker.ts:451-455`). cyrup's `SessionRegistration` does not model
/// `extensions`, so the value is validated out of its `[MAP-ONLY]` flatten capture.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn register_with_an_array_shaped_extension_capability_is_fatal() {
    let broker = Broker::start().await;
    let mut c = RawClient::connect(&broker.socket).await;
    c.send(&serde_json::json!({
        "type": "register",
        "sessionId": "s1",
        "session": {
            "cwd": "/tmp/work", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0,
            "extensions": [array_shaped_capability()],
        },
    }))
    .await;
    c.assert_destroyed("an array-shaped `register.session.extensions[0]`")
        .await;
}

/// `ExtensionCapability` via `extension_capabilities_update`
/// (`v0.9.2 broker/broker.ts:559-567`). cyrup ignores the frame's *effects* (the bus is unported)
/// but must not ignore its validation, which pi runs first and fails with a `throw`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extension_capabilities_update_with_an_array_shaped_capability_is_fatal() {
    let broker = Broker::start().await;
    for bad in [
        // The array-shaped capability entry — R1 proper.
        serde_json::json!({ "type": "extension_capabilities_update", "extensions": [array_shaped_capability()] }),
        // `!Array.isArray(extensions)` (`v0.9.2 broker/broker.ts:560`).
        serde_json::json!({ "type": "extension_capabilities_update", "extensions": { "ns": true } }),
        // The same guard, field absent entirely.
        serde_json::json!({ "type": "extension_capabilities_update" }),
        // `length > MAX_EXTENSIONS_PER_SESSION` (`v0.9.2 broker/broker.ts:35,560`).
        serde_json::json!({
            "type": "extension_capabilities_update",
            "extensions": (0..33).map(|i| serde_json::json!({ "namespace": format!("ns{i}"), "ownerEligible": false })).collect::<Vec<_>>(),
        }),
        // `validateNamespace` — an uppercase leading char fails `^[a-z0-9]`
        // (`v0.9.2 broker/broker.ts:1170-1182`).
        serde_json::json!({
            "type": "extension_capabilities_update",
            "extensions": [{ "namespace": "Bad", "ownerEligible": true }],
        }),
        // `typeof c.ownerEligible !== "boolean"` (`v0.9.2 broker/broker.ts:1164`).
        serde_json::json!({
            "type": "extension_capabilities_update",
            "extensions": [{ "namespace": "ns", "ownerEligible": "yes" }],
        }),
    ] {
        let mut c = RawClient::connect(&broker.socket).await;
        c.register(&format!("probe-{}", uuid::Uuid::new_v4())).await;
        c.send(&bad).await;
        c.assert_destroyed(&format!("{bad}")).await;
    }
}

/// **Positive control for side A.** Rejecting arrays must not turn into rejecting everything: pi
/// serves all of these, so a disconnect here would be a regression in the opposite direction.
/// This also covers the `extensions` shapes pi explicitly permits — absent, and a well-formed list
/// (`extensions !== undefined` at `v0.9.2 broker/broker.ts:447`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn well_formed_map_payloads_are_still_served() {
    let broker = Broker::start().await;
    let mut c = RawClient::connect(&broker.socket).await;
    c.send(&serde_json::json!({
        "type": "register",
        "sessionId": "alpha-session",
        "session": {
            "name": "alpha", "cwd": "/tmp/work", "model": "m", "pid": 1,
            "startedAt": 0, "lastActivity": 0,
            "extensions": [{ "namespace": "ns.a/b-c_1", "ownerEligible": true }],
        },
    }))
    .await;
    assert_eq!(
        c.expect_frame("registered").await["sessionId"],
        "alpha-session"
    );

    c.send(&serde_json::json!({
        "type": "message_receipt",
        "receipt": { "messageId": "m1", "status": "queued", "timestamp": 1, "piFutureKey": 7 },
    }))
    .await;
    c.send(&serde_json::json!({
        "type": "extension_capabilities_update",
        "extensions": [{ "namespace": "ns", "ownerEligible": false }],
    }))
    .await;

    c.send(&serde_json::json!({ "type": "list", "requestId": "r1" }))
        .await;
    let sessions = c.expect_frame("sessions").await;
    assert_eq!(sessions["requestId"], "r1");
    assert_eq!(sessions["sessions"][0]["id"], "alpha-session");
}

// ---------------------------------------------------------------------------------------------
// Side B — the real `IntercomClient` against a hostile broker listener.
//
// `SessionInfo` and `MessageControl` only ever arrive broker -> client, so proving them needs a
// listener that speaks the framing protocol and then lies. pi's client throws out of its own
// switch on these (`v0.9.2 broker/client.ts:433-435,476-478,485-487,494-496`), and
// `framing.ts:44-51` destroys the socket; cyrup's equivalent is a decode failure in `read_task`,
// observable as an `InboundEvent::Disconnected`.
// ---------------------------------------------------------------------------------------------

/// A listener that accepts one connection, answers `register` with `registered`, then — once the
/// test releases it — writes whatever frames the test handed it.
struct HostileBroker {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    /// Gates the hostile frames until the test has subscribed. Without it the frame can be decoded,
    /// and `Disconnected` broadcast to zero receivers, before `subscribe()` runs — which made this
    /// test flaky per-frame rather than wrong.
    release: std::sync::Arc<tokio::sync::Notify>,
}

impl HostileBroker {
    /// Serve one client: consume its `register`, ack it, wait for [`Self::release`], emit `frames`,
    /// then park.
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
            // Consume the client's `register` frame before answering, so the ack cannot race it.
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

/// Connect a real client to a hostile broker that emits `frame`, and report whether the client
/// tore the connection down.
async fn client_disconnects_on(frame: serde_json::Value) -> bool {
    let broker = HostileBroker::start(vec![frame]);
    let client = IntercomClient::connect(&broker.socket, registration(), None)
        .await
        .expect("the hostile broker's `registered` ack still completes the handshake");
    let mut events = client.subscribe();
    // Only now let the hostile frame onto the wire, so `Disconnected` cannot be broadcast before
    // there is a receiver for it.
    broker.release.notify_one();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(InboundEvent::Disconnected(_))) => return true,
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => return false,
        }
    }
}

/// `SessionInfo` on the client side (`isSessionInfo`, `v0.9.2 broker/client.ts:152-189`; the
/// throws it feeds are at `:433-435`, `:476-478`, `:485-487`, `:494-496`, `:516-518`).
/// `MessageControl` likewise (`isMessageControl`, `v0.9.2 broker/client.ts:67-82`, throw at
/// `:485-487`), and `MessageReceipt` in the broker -> client direction (`:476-478`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_destroys_the_connection_on_array_shaped_broker_payloads() {
    let good_from = serde_json::json!({
        "id": "s-peer", "cwd": "/tmp/work", "model": "m", "pid": 1, "startedAt": 2, "lastActivity": 3,
    });
    for (what, frame) in [
        // SessionInfo, in each of the four places a broker can put one.
        (
            "`session_joined.session`",
            serde_json::json!({ "type": "session_joined", "session": array_shaped_session_info() }),
        ),
        (
            "`presence_update.session`",
            serde_json::json!({ "type": "presence_update", "session": array_shaped_session_info() }),
        ),
        (
            "`sessions[0]`",
            serde_json::json!({ "type": "sessions", "requestId": "r1", "sessions": [array_shaped_session_info()] }),
        ),
        (
            "`message.from`",
            serde_json::json!({
                "type": "message",
                "from": array_shaped_session_info(),
                "message": { "id": "m1", "timestamp": 1, "content": { "text": "hi" } },
            }),
        ),
        // MessageControl.
        (
            "`message_control.control`",
            serde_json::json!({
                "type": "message_control", "from": good_from.clone(), "control": array_shaped_control(),
            }),
        ),
        // MessageReceipt, broker -> client.
        (
            "`message_receipt.receipt`",
            serde_json::json!({
                "type": "message_receipt", "from": good_from.clone(), "receipt": array_shaped_receipt(),
            }),
        ),
        // Message / Attachment, which were map-only only by accident before the invariant existed.
        (
            "`message.message`",
            serde_json::json!({
                "type": "message", "from": good_from.clone(),
                "message": ["m1", 1, null, null, null, null, null, null, null, null, null, { "text": "hi" }],
            }),
        ),
        (
            "`message.content.attachments[0]`",
            serde_json::json!({
                "type": "message", "from": good_from.clone(),
                "message": { "id": "m1", "timestamp": 1, "content": { "text": "hi", "attachments": [["snippet", "n", "c", null]] } },
            }),
        ),
    ] {
        assert!(
            client_disconnects_on(frame).await,
            "the client must destroy the connection on an array-shaped {what}"
        );
    }
}

/// **Positive control for side B.** The same frames in map shape — carrying unmodelled keys a
/// newer pi broker would add — must keep the client connected, because pi's guards pass them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_survives_well_formed_broker_payloads_with_unmodelled_keys() {
    let good_from = serde_json::json!({
        "id": "s-peer", "cwd": "/tmp/work", "model": "m", "pid": 1, "startedAt": 2, "lastActivity": 3,
        // v0.9.x `SessionInfo` fields cyrup does not model (`v0.9.2 types.ts:19-21`).
        "contextPct": 42, "contextTokens": 100, "contextWindow": 200,
    });
    for (what, frame) in [
        (
            "session_joined",
            serde_json::json!({ "type": "session_joined", "session": good_from.clone() }),
        ),
        (
            "message_control",
            serde_json::json!({
                "type": "message_control",
                "from": good_from.clone(),
                "control": { "messageId": "m1", "action": "cancel", "timestamp": 1, "piFutureKey": 7 },
            }),
        ),
        (
            "message_receipt",
            serde_json::json!({
                "type": "message_receipt",
                "from": good_from.clone(),
                "receipt": { "messageId": "m1", "status": "queued", "timestamp": 1, "piFutureKey": 7 },
            }),
        ),
        (
            "message",
            serde_json::json!({
                "type": "message",
                "from": good_from.clone(),
                "message": {
                    "id": "m1", "timestamp": 1, "piFutureKey": 7,
                    "content": { "text": "hi", "attachments": [{ "type": "snippet", "name": "n", "content": "c", "piFutureKey": 7 }] },
                },
            }),
        ),
    ] {
        assert!(
            !client_disconnects_on(frame).await,
            "the client must stay connected for a well-formed `{what}`"
        );
    }
}

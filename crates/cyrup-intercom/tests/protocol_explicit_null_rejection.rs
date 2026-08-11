//! R2 — **an explicit JSON `null` on an optional payload field must be rejected**, proven over REAL
//! sockets on BOTH sides of the wire: a real broker subprocess for the frames a client can send,
//! and a real [`IntercomClient`] against a hostile listener for the frames a broker can send.
//!
//! # The hole
//!
//! Every optional field in pi's guards is checked as
//! `x.field !== undefined && typeof x.field !== "<t>"` — the canonical site being `isMessage`'s
//! numeric sweep at `v0.9.2 broker/broker.ts:151-155`, with the string/boolean fields at `:157-171`
//! and `content.attachments` at `:182-183`. In JavaScript `null !== undefined` and
//! `typeof null === "object"`, so **an explicit `null` fails every one of them**: pi rejects it
//! exactly as it rejects `"nope"`. Only an *absent* key is accepted.
//!
//! serde's `Option<T>` does the opposite. It maps `null` to `None`, and because every one of these
//! fields is `skip_serializing_if = "Option::is_none"`, the key is then *deleted* from anything
//! cyrup re-emits. Confirmed live before this fix: alpha sent
//! `senderSequence: null, replyTo: null`, the broker answered `delivered`, and beta received an
//! envelope with both keys gone — so the same batch that claimed "cyrup now matches pi's reject
//! behaviour" and "the relay is lossless" broke both claims at once. pi answers
//! `delivery_failed` / `"Invalid message format"` (`v0.9.2 broker/broker.ts:607-613`).
//!
//! # What "reject" means per frame — not the same thing everywhere
//!
//! The decoder's job is only to fail; the *consequence* is the call site's, and pi's differs by
//! case. This file asserts the real consequence in each direction, because asserting the wrong one
//! would be a divergence in the opposite direction:
//!
//! | frame | pi | cyrup |
//! |---|---|---|
//! | `send` | `delivery_failed` / `"Invalid message format"`, socket SURVIVES (`v0.9.2 broker/broker.ts:607-613`) | `handle_send` decodes with `.ok()` → same |
//! | `register` | `throw` → `socket.destroy` (`v0.9.2 broker/broker.ts:429-432`) | `FrameResult::protocol_error()` |
//! | `message_receipt` | `throw` → `socket.destroy` (`v0.9.2 broker/broker.ts:805-807`) | `FrameResult::protocol_error()` |
//! | broker → client `message`/`session_joined`/… | `throw` → `socket.destroy` (`v0.9.2 broker/client.ts:433-435,494-496`) | decode failure in `read_task` → `InboundEvent::Disconnected` |
//!
//! Every rejection test below is paired with a **positive control** in the same file: the identical
//! frame with the key ABSENT rather than null. `undefined` is precisely what those guards permit, so
//! a fix that rejected both would disconnect peers pi serves — worse than the bug. The `send`
//! control additionally asserts the *relayed* envelope is byte-identical to what was sent (modulo
//! the two broker-owned stamps), which is what the lossless-relay claim actually means.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use cyrup_intercom::transport::client::{IntercomClient, InboundEvent};
use cyrup_intercom::transport::framing::{FrameReader, encode_json};
use cyrup_intercom::transport::protocol::{SessionRegistration, now_ms};
use cyrup_intercom::transport::spawn::wait_for_broker;

// ---------------------------------------------------------------------------------------------
// Side A — the real broker subprocess, driven by raw framed clients.
// ---------------------------------------------------------------------------------------------

/// A live broker child process + its socket path. Killed on drop.
struct Broker {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    child: tokio::process::Child,
}

impl Broker {
    async fn start() -> Self {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"));
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("intercom").join("broker.sock");
        let child = tokio::process::Command::new(&bin)
            .env("CYRUP_CODING_AGENT_DIR", dir.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn the real intercom broker subprocess");
        wait_for_broker(&socket, Duration::from_secs(5)).await.expect("broker is health-connectable");
        Self { _dir: dir, socket, child }
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// A raw length-prefixed-JSON client: it can put ANY frame on the wire, including payload shapes
/// `ClientMessage` cannot express in Rust — such as an explicit `null` on an `Option` field.
struct RawClient {
    stream: UnixStream,
    reader: FrameReader,
    queued: VecDeque<serde_json::Value>,
    buf: Vec<u8>,
}

impl RawClient {
    async fn connect(socket: &Path) -> Self {
        Self {
            stream: UnixStream::connect(socket).await.expect("connect to the broker socket"),
            reader: FrameReader::new(),
            queued: VecDeque::new(),
            buf: vec![0u8; 16 * 1024],
        }
    }

    async fn send(&mut self, frame: &serde_json::Value) {
        let bytes = encode_json(frame).expect("encodes");
        self.stream.write_all(&bytes).await.expect("write frame");
    }

    /// The next frame within `within`, or `None` on close **or** on quiet. Callers that need to
    /// distinguish those two use [`Self::assert_destroyed`] / [`Self::assert_alive`].
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
            let frames = self.reader.push(&self.buf[..n]).expect("broker frames are well-formed");
            for payload in frames {
                self.queued.push_back(serde_json::from_slice(&payload).expect("broker frames are JSON"));
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
        assert_eq!(self.expect_frame("registered").await["sessionId"], session_id);
    }

    /// Assert the broker destroyed this connection. A `list` is queued first so a broker that
    /// merely *ignored* the hostile frame would answer and fail the assertion — without it, a
    /// broker that accepted the frame and simply had nothing to say would look identical to one
    /// that closed.
    ///
    /// The probe write is *fallible on purpose*. A destroy is exactly the case where the peer may
    /// already be gone, so an `EPIPE` here is a pass, not a test error — panicking on it made this
    /// assertion fail under CPU contention while passing on an idle box.
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

    /// Assert the broker did NOT destroy this connection — the other half of matching pi, and the
    /// half a blanket "reject everything" fix would fail.
    async fn assert_alive(&mut self, what: &str) {
        self.send(&serde_json::json!({ "type": "list", "requestId": "alive" })).await;
        let frame = self.expect_frame("sessions").await;
        assert_eq!(frame["requestId"], "alive", "the broker must keep serving after {what}");
    }

    /// Assert nothing at all arrives for a beat — used to prove a rejected `send` was not silently
    /// relayed to the peer with the offending keys stripped, which is what happened before the fix.
    async fn assert_quiet(&mut self, what: &str) {
        let frame = self.next_frame_within(Duration::from_millis(750)).await;
        assert!(frame.is_none(), "{what}: the peer must receive nothing, but got {frame:?}");
    }
}

/// A well-formed `send` body, with `patch` applied on top of the `message` object.
fn send_frame(to: &str, message_patch: serde_json::Value) -> serde_json::Value {
    let mut message = serde_json::json!({
        "id": "m-null-probe",
        "timestamp": 1_700_000_000_000_u64,
        "content": { "text": "hi" },
    });
    if let (Some(dst), Some(src)) = (message.as_object_mut(), message_patch.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    serde_json::json!({ "type": "send", "to": to, "message": message })
}

/// The confirmed-live hole, end to end. `senderSequence: null` / `replyTo: null` used to be
/// answered `delivered` and relayed to the peer with both keys erased. pi answers
/// `delivery_failed` / `"Invalid message format"` with `messageId: "unknown"` — because
/// `messageId = isMessage(message) ? message.id : "unknown"` (`v0.9.2 broker/broker.ts:605`) — and
/// the connection SURVIVES (`:607-613`). Nothing reaches the peer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_with_a_null_optional_field_is_delivery_failed_and_never_relayed() {
    let broker = Broker::start().await;
    let mut beta = RawClient::connect(&broker.socket).await;
    beta.register("beta-session").await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha-session").await;
    // `beta` observed `alpha` joining; drain that so `assert_quiet` only sees message traffic.
    let _ = beta.expect_frame("session_joined").await;

    for patch in [
        // The exact frame confirmed live.
        serde_json::json!({ "senderSequence": null, "replyTo": null }),
        // Each of the nine optional fields on its own (`v0.9.2 broker/broker.ts:151-171`).
        serde_json::json!({ "senderSequence": null }),
        serde_json::json!({ "brokerReceivedAt": null }),
        serde_json::json!({ "brokerDeliveredAt": null }),
        serde_json::json!({ "receiverReceivedAt": null }),
        serde_json::json!({ "injectedAt": null }),
        serde_json::json!({ "supersedes": null }),
        serde_json::json!({ "retryOf": null }),
        serde_json::json!({ "replyTo": null }),
        serde_json::json!({ "expectsReply": null }),
        // `content.attachments` (`v0.9.2 broker/broker.ts:182-183`).
        serde_json::json!({ "content": { "text": "hi", "attachments": null } }),
        // `attachment.language` (`v0.9.2 broker/broker.ts:137`), reached through `.every(isAttachment)`.
        serde_json::json!({ "content": { "text": "hi", "attachments": [
            { "type": "snippet", "name": "n", "content": "c", "language": null },
        ] } }),
    ] {
        alpha.send(&send_frame("beta-session", patch.clone())).await;
        let failed = alpha.expect_frame("delivery_failed").await;
        assert_eq!(failed["reason"], "Invalid message format", "for {patch}");
        assert_eq!(
            failed["messageId"], "unknown",
            "pi reports `unknown` because isMessage() failed (`v0.9.2 broker/broker.ts:605`), for {patch}"
        );
        alpha.assert_alive(&format!("a rejected send carrying {patch}")).await;
        beta.assert_quiet(&format!("send carrying {patch}")).await;
    }
}

/// `register` — `isSessionRegistration` rejects a null `name`/`status`
/// (`v0.9.2 broker/broker.ts:207-211`) and the failure throws out of `case "register"`
/// (`v0.9.2 broker/broker.ts:429-432`), i.e. `socket.destroy`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn register_with_a_null_optional_field_is_fatal() {
    let broker = Broker::start().await;
    for key in ["name", "status"] {
        let mut session = serde_json::json!({
            "cwd": "/tmp/work", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0,
        });
        session[key] = serde_json::Value::Null;
        let mut c = RawClient::connect(&broker.socket).await;
        c.send(&serde_json::json!({ "type": "register", "sessionId": "s1", "session": session })).await;
        c.assert_destroyed(&format!("`register.session.{key}` = null")).await;
    }
}

/// `message_receipt` — `isMessageReceipt` rejects a null `detail`
/// (`v0.9.2 broker/broker.ts:115`) and `case "message_receipt"` throws (`:805-807`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_receipt_with_a_null_detail_is_fatal() {
    let broker = Broker::start().await;
    let mut c = RawClient::connect(&broker.socket).await;
    c.register("receipt-session").await;
    c.send(&serde_json::json!({
        "type": "message_receipt",
        "receipt": { "messageId": "m1", "status": "queued", "timestamp": 1, "detail": null },
    }))
    .await;
    c.assert_destroyed("`message_receipt.receipt.detail` = null").await;
}

/// **Positive control for side A.** `undefined` is what pi's guards permit, so the same frames with
/// the key ABSENT must be served — and the relayed envelope must come out the far side byte-equal
/// to what went in, plus only the two broker-owned stamps pi adds
/// (`v0.9.2 broker/broker.ts:672-676`). This is the "lossless relay" claim, asserted rather than
/// declared.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn absent_optional_fields_are_served_and_relayed_verbatim() {
    let broker = Broker::start().await;
    let mut beta = RawClient::connect(&broker.socket).await;
    beta.register("beta-session").await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha-session").await;
    let _ = beta.expect_frame("session_joined").await;

    // A `register` with `name`/`status` absent, and one with them present-and-well-typed.
    let mut plain = RawClient::connect(&broker.socket).await;
    plain
        .send(&serde_json::json!({
            "type": "register",
            "sessionId": "plain-session",
            "session": { "cwd": "/tmp/work", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
        }))
        .await;
    assert_eq!(plain.expect_frame("registered").await["sessionId"], "plain-session");
    plain
        .send(&serde_json::json!({
            "type": "message_receipt",
            "receipt": { "messageId": "m1", "status": "queued", "timestamp": 1 },
        }))
        .await;
    plain.assert_alive("a `message_receipt` with `detail` absent").await;

    // The relay half: every optional field set to a well-typed value, plus an unmodelled key.
    let sent = serde_json::json!({
        "id": "m-good",
        "timestamp": 1_700_000_000_000_u64,
        "senderSequence": 7,
        "receiverReceivedAt": 1_700_000_000_003_u64,
        "injectedAt": 1_700_000_000_004_u64,
        "supersedes": "m0",
        "retryOf": "m-retry",
        "piFutureKey": { "nested": [1, 2, 3] },
        "content": { "text": "hi", "attachments": [
            { "type": "snippet", "name": "n", "content": "c", "language": "rust" },
        ] },
    });
    alpha.send(&serde_json::json!({ "type": "send", "to": "beta-session", "message": sent })).await;
    assert_eq!(alpha.expect_frame("delivered").await["messageId"], "m-good");

    let relayed = beta.expect_frame("message").await;
    let mut got = relayed["message"].clone();
    // The two stamps pi's broker adds on top of the spread (`v0.9.2 broker/broker.ts:674-675`).
    let obj = got.as_object_mut().expect("the relayed message is a map");
    assert!(obj.remove("brokerReceivedAt").is_some(), "the broker must stamp `brokerReceivedAt`");
    assert!(obj.remove("brokerDeliveredAt").is_some(), "the broker must stamp `brokerDeliveredAt`");
    assert_eq!(got, sent, "the relayed envelope must be byte-equal to what was sent");
}

// ---------------------------------------------------------------------------------------------
// Side B — the real `IntercomClient` against a hostile broker listener.
//
// `SessionInfo` and `MessageControl` only ever arrive broker -> client, so proving them needs a
// listener that speaks the framing protocol and then lies. pi's client throws out of its own switch
// on these (`v0.9.2 broker/client.ts:433-435,476-478,485-487,494-496,516-518`), and
// `framing.ts:44-51` destroys the socket; cyrup's equivalent is a decode failure in `read_task`,
// observable as an `InboundEvent::Disconnected`.
// ---------------------------------------------------------------------------------------------

/// A listener that accepts one connection, answers `register` with `registered`, then — once the
/// test releases it — writes whatever frames the test handed it.
struct HostileBroker {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    /// Gates the hostile frames until the test has subscribed, so `Disconnected` cannot be
    /// broadcast to zero receivers before `subscribe()` runs.
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
            // Consume the client's `register` frame before answering, so the ack cannot race it.
            let mut reader = FrameReader::new();
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let Ok(n) = stream.read(&mut buf).await else { return };
                if n == 0 {
                    return;
                }
                let Ok(got) = reader.push(&buf[..n]) else { return };
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
        Self { _dir: dir, socket, release }
    }
}

fn registration() -> SessionRegistration {
    SessionRegistration {
        name: Some("probe".to_string()),
        cwd: "/tmp/work".to_string(),
        model: "test-model".to_string(),
        pid: std::process::id().into(),
        started_at: now_ms().into(),
        last_activity: now_ms().into(),
        status: None,
        extra: Default::default(),
    }
}

/// Connect a real client to a hostile broker that emits `frame`, and report whether the client tore
/// the connection down.
async fn client_disconnects_on(frame: serde_json::Value) -> bool {
    let broker = HostileBroker::start(vec![frame]);
    let client = IntercomClient::connect(&broker.socket, registration(), None)
        .await
        .expect("the hostile broker's `registered` ack still completes the handshake");
    let mut events = client.subscribe();
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

fn good_from() -> serde_json::Value {
    serde_json::json!({
        "id": "s-peer", "cwd": "/tmp/work", "model": "m", "pid": 1, "startedAt": 2, "lastActivity": 3,
    })
}

fn with_null(base: &serde_json::Value, key: &str) -> serde_json::Value {
    let mut v = base.clone();
    v[key] = serde_json::Value::Null;
    v
}

/// `SessionInfo` (`isSessionInfo`, `v0.9.2 broker/client.ts:170-188`), `Message`
/// (`isMessage`, `v0.9.2 broker/client.ts:117-135`), `MessageControl`
/// (`isMessageControl`, `v0.9.2 broker/client.ts:78-81`) and `MessageReceipt`
/// (`isMessageReceipt`, `v0.9.2 broker/client.ts:64`) — each guard rejects an explicit null, and
/// each rejection is a `throw` in the client's switch, i.e. `socket.destroy`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_destroys_the_connection_on_null_optional_fields() {
    let good_msg =
        serde_json::json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } });
    let good_receipt = serde_json::json!({ "messageId": "m1", "status": "queued", "timestamp": 1 });
    let good_control = serde_json::json!({ "messageId": "m1", "action": "cancel", "timestamp": 1 });

    let mut cases: Vec<(String, serde_json::Value)> = Vec::new();
    for key in ["name", "status", "peerUid", "trustedLocal"] {
        cases.push((
            format!("`session_joined.session.{key}` = null"),
            serde_json::json!({ "type": "session_joined", "session": with_null(&good_from(), key) }),
        ));
    }
    for key in [
        "senderSequence",
        "brokerReceivedAt",
        "brokerDeliveredAt",
        "receiverReceivedAt",
        "injectedAt",
        "supersedes",
        "retryOf",
        "replyTo",
        "expectsReply",
    ] {
        cases.push((
            format!("`message.message.{key}` = null"),
            serde_json::json!({
                "type": "message", "from": good_from(), "message": with_null(&good_msg, key),
            }),
        ));
    }
    cases.push((
        "`message.message.content.attachments` = null".to_string(),
        serde_json::json!({
            "type": "message", "from": good_from(),
            "message": { "id": "m1", "timestamp": 1, "content": { "text": "hi", "attachments": null } },
        }),
    ));
    cases.push((
        "`message.message.content.attachments[0].language` = null".to_string(),
        serde_json::json!({
            "type": "message", "from": good_from(),
            "message": { "id": "m1", "timestamp": 1, "content": { "text": "hi", "attachments": [
                { "type": "snippet", "name": "n", "content": "c", "language": null },
            ] } },
        }),
    ));
    for key in ["supersededBy", "detail"] {
        cases.push((
            format!("`message_control.control.{key}` = null"),
            serde_json::json!({
                "type": "message_control", "from": good_from(),
                "control": with_null(&good_control, key),
            }),
        ));
    }
    cases.push((
        "`message_receipt.receipt.detail` = null".to_string(),
        serde_json::json!({
            "type": "message_receipt", "from": good_from(),
            "receipt": with_null(&good_receipt, "detail"),
        }),
    ));

    for (what, frame) in cases {
        assert!(
            client_disconnects_on(frame).await,
            "the client must destroy the connection on {what}"
        );
    }
}

/// **Positive control for side B.** The same frames with those keys ABSENT — and carrying
/// unmodelled keys a newer pi broker would add — must keep the client connected, because pi's
/// guards pass them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_survives_absent_optional_fields() {
    let from = serde_json::json!({
        "id": "s-peer", "cwd": "/tmp/work", "model": "m", "pid": 1, "startedAt": 2, "lastActivity": 3,
        // v0.9.x `SessionInfo` fields cyrup does not model (`v0.9.2 types.ts:19-21`).
        "contextPct": 42, "contextTokens": 100, "contextWindow": 200,
    });
    for (what, frame) in [
        ("session_joined", serde_json::json!({ "type": "session_joined", "session": from.clone() })),
        ("message_control", serde_json::json!({
            "type": "message_control", "from": from.clone(),
            "control": { "messageId": "m1", "action": "cancel", "timestamp": 1 },
        })),
        ("message_receipt", serde_json::json!({
            "type": "message_receipt", "from": from.clone(),
            "receipt": { "messageId": "m1", "status": "queued", "timestamp": 1 },
        })),
        ("message", serde_json::json!({
            "type": "message", "from": from.clone(),
            "message": {
                "id": "m1", "timestamp": 1, "senderSequence": 7, "expectsReply": false,
                "content": { "text": "hi", "attachments": [
                    { "type": "snippet", "name": "n", "content": "c" },
                ] },
            },
        })),
    ] {
        assert!(
            !client_disconnects_on(frame).await,
            "the client must stay connected for a well-formed `{what}`"
        );
    }
}

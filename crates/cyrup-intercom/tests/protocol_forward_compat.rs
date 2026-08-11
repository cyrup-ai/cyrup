//! G136 — interop survivability against a pi >= 0.9.0 peer, proven against the **real** broker
//! subprocess over the **real** Unix socket, driven by a raw framed client so arbitrary wire frames
//! can be sent (an `IntercomClient` can only emit tags it models).
//!
//! Two properties, and their mirrors:
//!
//! * **(a) the v0.9.2 tag set must not tear the connection down.** A pi >= 0.9.0 client emits
//!   `message_receipt` on its FIRST inbound message — `emitMessageReceipt(id, "receiver_received")`
//!   fires unconditionally at `v0.9.2 index.ts:954`, and `sendMessageReceipt` is deliberately NOT
//!   feature-gated (`v0.9.2 broker/client.ts:773-784`, in contrast to `:817-819`). Against a
//!   v0.7.0-era tag set that frame hit the broker's catch-all `default` arm and killed the socket.
//!   MIRROR: a tag from a *later* protocol version, and a known tag with a wrong-typed payload,
//!   must STILL kill it — that is pi's own behaviour (`v0.9.2 broker/broker.ts:971-972` →
//!   `framing.ts:44-51` → `broker.ts:321-323` `socket.destroy(error)`), and this socket is
//!   reachable by every other session on the box.
//! * **(b) a relayed message must survive verbatim.** pi's broker re-forwards by object spread
//!   (`v0.9.2 broker/broker.ts:672-676`), so a cyrup broker sitting between two pi sessions must
//!   not delete the half of their envelope it does not model.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use cyrup_intercom::transport::framing::{FrameReader, encode_json};
use cyrup_intercom::transport::spawn::wait_for_broker;

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

/// A raw length-prefixed-JSON client: it can put ANY frame on the wire, including tags cyrup's
/// `ClientMessage` does not model.
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

    /// The next frame, or `None` if the broker closed the connection (pi's `socket.destroy`).
    async fn next_frame(&mut self) -> Option<serde_json::Value> {
        loop {
            if let Some(v) = self.queued.pop_front() {
                return Some(v);
            }
            let n = match tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut self.buf))
                .await
                .expect("broker responds or closes within 5s")
            {
                Ok(0) | Err(_) => return None,
                Ok(n) => n,
            };
            let frames = self.reader.push(&self.buf[..n]).expect("broker frames are well-formed");
            for payload in frames {
                self.queued.push_back(serde_json::from_slice(&payload).expect("broker frames are JSON"));
            }
        }
    }

    /// Read until a frame with `type == ty` arrives (skipping broadcasts), or the socket closes.
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

    /// Register under a stable session id and consume the `registered` ack.
    async fn register(&mut self, name: &str, session_id: &str) {
        self.send(&serde_json::json!({
            "type": "register",
            "sessionId": session_id,
            "session": {
                "name": name,
                "cwd": "/tmp/work",
                "model": "test-model",
                "pid": std::process::id(),
                "startedAt": 0,
                "lastActivity": 0,
            },
        }))
        .await;
        let ack = self.expect_frame("registered").await;
        assert_eq!(ack["sessionId"], session_id);
        // cyrup must NOT advertise `extension-bus-v1`: not advertising is the whole reason a
        // conforming pi client never sends the extension-bus frames this crate cannot service
        // (`supportsFeature` gate, `v0.9.2 broker/client.ts:648,817-819`).
        assert!(ack.get("features").is_none(), "cyrup must not advertise extension-bus features");
    }
}

// G136(a). Regression proof: with the v0.7.0-era tag set, `message_receipt` fell through the
// broker's `match ty` to `_ => FrameResult::protocol_error()`, whose `FrameOutcome::ProtocolError`
// becomes `keep_going: false` and tears the connection down — so the `list` below would find a dead
// socket and this test would panic in `expect_frame("sessions")` with "connection closed".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broker_survives_the_v0_9_2_client_tags_a_pi_peer_actually_sends() {
    let broker = Broker::start().await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha", "alpha-session").await;

    // The frame a pi >= 0.9.0 client emits on its very first inbound message.
    alpha
        .send(&serde_json::json!({
            "type": "message_receipt",
            "receipt": { "messageId": "m-unknown", "status": "receiver_received", "timestamp": 1 },
        }))
        .await;
    // The extension-bus frames, which cannot arrive from a conforming peer but must not be fatal.
    alpha
        .send(&serde_json::json!({
            "type": "extension_capabilities_update",
            "extensions": [{ "namespace": "ns", "ownerEligible": true }],
        }))
        .await;
    alpha
        .send(&serde_json::json!({
            "type": "extension_publish", "namespace": "ns", "audience": "capable", "payload": { "k": 1 },
        }))
        .await;

    // The connection must still be serving: a `list` round-trips.
    alpha.send(&serde_json::json!({ "type": "list", "requestId": "r1" })).await;
    let sessions = alpha.expect_frame("sessions").await;
    assert_eq!(sessions["requestId"], "r1");
    assert_eq!(sessions["sessions"][0]["id"], "alpha-session");
}

// G136(a), the `cancel_message` half. pi's `cancelMessage()` returns a promise settled only by a
// `delivered`/`delivery_failed` frame (`v0.9.2 broker/client.ts:738`), so accept-and-ignore would
// hang the caller. cyrup has no `messageReceiptRoutes` table yet (pi populates it at
// `v0.9.2 broker/broker.ts:698`), so every lookup misses and pi's own miss branch applies —
// `delivery_failed` with pi's exact reason string (`v0.9.2 broker/broker.ts:842-848`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_message_is_answered_with_pis_delivery_failed_reason() {
    let broker = Broker::start().await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha", "alpha-session").await;

    alpha.send(&serde_json::json!({ "type": "cancel_message", "messageId": "m-nope" })).await;
    let failed = alpha.expect_frame("delivery_failed").await;
    assert_eq!(failed["messageId"], "m-nope");
    assert_eq!(failed["reason"], "Message cannot be cancelled by this session");
}

// MIRROR for the two tests above. Accepting the v0.9.2 tag set must not make the broker credulous.
// This socket is reachable by every session on the box, so "tolerant" turning into "accepts
// anything" would be an input-validation hole, not a compatibility fix. All three of these are
// fatal upstream too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broker_still_destroys_the_connection_on_unknown_tags_and_malformed_payloads() {
    let broker = Broker::start().await;
    for bad in [
        // A tag from some later protocol version — `default: throw` at
        // `v0.9.2 broker/broker.ts:971-972`.
        serde_json::json!({ "type": "pi_quantum_v2", "whatever": 1 }),
        // A known tag whose payload fails pi's `isMessageReceipt` guard — closed status vocabulary
        // (`v0.9.2 broker/client.ts:45-54`); `throw new Error("Invalid message_receipt message")`
        // at `v0.9.2 broker/broker.ts:806-808`.
        serde_json::json!({
            "type": "message_receipt",
            "receipt": { "messageId": "m1", "status": "teleported", "timestamp": 1 },
        }),
        // Same tag, required field missing entirely.
        serde_json::json!({ "type": "message_receipt", "receipt": { "messageId": "m1" } }),
        // `cancel_message` with a non-string id — `throw` at `v0.9.2 broker/broker.ts:825-827`.
        serde_json::json!({ "type": "cancel_message", "messageId": 42 }),
        // A frame with no `type` at all.
        serde_json::json!({ "nope": true }),
    ] {
        let mut c = RawClient::connect(&broker.socket).await;
        c.register("probe", &format!("probe-{}", uuid::Uuid::new_v4())).await;
        c.send(&bad).await;
        // Give the broker a chance to answer before it closes; a `list` that never gets its reply
        // is exactly the "socket destroyed" observation. The probe write is fallible on purpose —
        // an `EPIPE` means the broker had already closed, which is the same observation, and
        // `send`'s `.expect("write frame")` made this fail under CPU contention while passing on an
        // idle box.
        let probe = encode_json(&serde_json::json!({ "type": "list", "requestId": "r1" }))
            .expect("encodes");
        if c.stream.write_all(&probe).await.is_ok() {
            assert!(
                c.next_frame().await.is_none(),
                "the broker must still destroy the connection for {bad}"
            );
        }
    }
}

// G136(b). Regression proof: `broker/mod.rs` re-parses the `send` payload into the typed `Message`
// and re-serializes THAT. Before this change the struct held 5 fields, so `senderSequence`,
// `retryOf`, `supersedes` and every unmodelled key were deleted on the hop and the two broker-owned
// timestamps were never stamped — a cyrup broker between two pi sessions silently corrupted their
// conversation. Each assertion below fails against that behaviour.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relayed_message_keeps_every_field_a_pi_sender_set_and_gains_the_broker_stamps() {
    let broker = Broker::start().await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha", "alpha-session").await;
    let mut beta = RawClient::connect(&broker.socket).await;
    beta.register("beta", "beta-session").await;

    alpha
        .send(&serde_json::json!({
            "type": "send",
            "to": "beta-session",
            "message": {
                "id": "m1",
                "timestamp": 1_700_000_000_000_u64,
                "senderSequence": 7,
                "retryOf": "m0",
                "receiverReceivedAt": 1_700_000_000_005_u64,
                "injectedAt": 1_700_000_000_006_u64,
                // A key from a protocol version cyrup does not know at all.
                "piFutureField": { "nested": [1, 2, 3] },
                "content": { "text": "hello", "piFutureContentKey": "kept" },
            },
        }))
        .await;

    let delivered = beta.expect_frame("message").await;
    let m = &delivered["message"];
    assert_eq!(m["id"], "m1");
    assert_eq!(m["content"]["text"], "hello");
    // The v0.9.x fields the sender set.
    assert_eq!(m["senderSequence"], 7, "senderSequence must survive the hop: {m}");
    assert_eq!(m["retryOf"], "m0", "retryOf must survive the hop: {m}");
    assert_eq!(m["receiverReceivedAt"], 1_700_000_000_005_u64);
    assert_eq!(m["injectedAt"], 1_700_000_000_006_u64);
    // The keys cyrup models nowhere — pi's object spread carries these, so the flatten capture must.
    assert_eq!(m["piFutureField"]["nested"][2], 3, "unmodelled top-level key must survive: {m}");
    assert_eq!(m["content"]["piFutureContentKey"], "kept", "unmodelled content key must survive: {m}");
    // And the two timestamps the broker itself owns (`v0.9.2 broker/broker.ts:674-675`).
    assert!(m["brokerReceivedAt"].is_u64(), "broker must stamp brokerReceivedAt: {m}");
    assert!(m["brokerDeliveredAt"].is_u64(), "broker must stamp brokerDeliveredAt: {m}");
    assert!(m["brokerDeliveredAt"].as_u64() >= m["brokerReceivedAt"].as_u64());

    // The sender still gets its ack, i.e. the relay path is otherwise unchanged.
    assert_eq!(alpha.expect_frame("delivered").await["messageId"], "m1");
}

// MIRROR for the test above: preserving unknown fields must not weaken the validation of KNOWN
// ones. pi's `isMessage()` type-checks the v0.9.x fields (`v0.9.2 broker/broker.ts:151-163`) and a
// failure yields `delivery_failed: "Invalid message format"` (`:607-613`). Before these fields were
// modelled, cyrup silently IGNORED `{"senderSequence": "nope"}` and delivered the message anyway —
// i.e. cyrup was looser than pi here, and this test fails against that.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_typed_v0_9_x_field_still_fails_delivery_like_pi() {
    let broker = Broker::start().await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha", "alpha-session").await;
    let mut beta = RawClient::connect(&broker.socket).await;
    beta.register("beta", "beta-session").await;

    alpha
        .send(&serde_json::json!({
            "type": "send",
            "to": "beta-session",
            "message": {
                "id": "m1",
                "timestamp": 1,
                "senderSequence": "nope",
                "content": { "text": "hello" },
            },
        }))
        .await;

    let failed = alpha.expect_frame("delivery_failed").await;
    assert_eq!(failed["reason"], "Invalid message format");
    // `messageId` falls back to "unknown" because `isMessage()` failed (`v0.9.2 broker/broker.ts:605`).
    assert_eq!(failed["messageId"], "unknown");

    // And the connection survives — an invalid message is a delivery failure, not a protocol error.
    alpha.send(&serde_json::json!({ "type": "list", "requestId": "r1" })).await;
    assert_eq!(alpha.expect_frame("sessions").await["requestId"], "r1");
}

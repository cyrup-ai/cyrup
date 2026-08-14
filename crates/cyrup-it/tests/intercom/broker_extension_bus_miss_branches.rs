//! The extension-bus client tags must answer the way pi answers, proven against the **real**
//! broker subprocess over a **real** Unix socket with a raw framed client (an `IntercomClient`
//! cannot emit these tags at all, so a unit-level mirror cannot observe what the broker puts on the
//! wire).
//!
//! cyrup does not implement the extension bus and deliberately does not advertise
//! `extension-bus-v1`, so a *conforming* pi client never sends these frames
//! (`supportsFeature` gate, `v0.9.2 broker/client.ts:648,817-819`). A non-conforming one still
//! can — this socket is openable by every process on the box — and upstream is neither silent nor
//! tolerant when it does:
//!
//! * `extension_capabilities_update` validates and **throws** on failure, i.e. `socket.destroy`
//!   (`v0.9.2 broker/broker.ts:551-567` → `framing.ts:44-51`).
//! * `extension_publish` **answers** `error` and keeps the socket
//!   (`v0.9.2 broker/broker.ts:1271-1280`).
//! * `extension_state_commit` **always** answers `extension_state_result`
//!   (`v0.9.2 broker/broker.ts:1367-1388`; every other exit from that handler writes one too).
//!
//! Because the `session.extensions` assignment at `v0.9.2 broker/broker.ts:568` is exactly the
//! effect this crate leaves unported, `!session.extensions?.length` is unconditionally true here
//! and pi's not-advertised branch is the only reachable one. That is the branch under test; the
//! bus itself is later-batch work.
//!
//! Each test carries a MIRROR so "answer everything" cannot pass: an unknown tag must still tear
//! the connection down, and a well-formed capabilities update must still be served.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use cyrup_intercom::transport::framing::{FrameReader, encode_json};
use crate::common::Broker;

/// A raw length-prefixed-JSON client: it can put ANY frame on the wire.
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

    /// Write without asserting success. A destroy is precisely the case where the peer may already
    /// be gone, so the `EPIPE` that PROVES the assertion must not be a panic that fails it.
    async fn try_send(&mut self, frame: &serde_json::Value) -> bool {
        let bytes = encode_json(frame).expect("encodes");
        self.stream.write_all(&bytes).await.is_ok()
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
    }

    /// The connection is still serving: a `list` round-trips.
    async fn assert_alive(&mut self, request_id: &str) {
        self.send(&serde_json::json!({ "type": "list", "requestId": request_id })).await;
        assert_eq!(self.expect_frame("sessions").await["requestId"], request_id);
    }
}

// R3(a). `extension_publish` from a session that never advertised an extension capability gets
// pi's `error` frame (`v0.9.2 broker/broker.ts:1277-1280`), not silence. Regression proof: the
// previous arm was `tracing::debug!(...); FrameResult::cont()`, so no frame was ever written and
// `expect_frame("error")` below hangs until the 5 s read timeout and panics.
//
// Both probes below are the exact frames confirmed live to be swallowed: a bare `extension_publish`
// with no payload at all, and a fully-formed one. Neither reaches the namespace/audience checks
// upstream, because the not-advertised branch precedes them at `:1277`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extension_publish_from_an_unadvertised_session_is_answered_with_pis_error() {
    let broker = Broker::start().await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha", "alpha-session").await;

    for frame in [
        // The frame confirmed live to leave the connection alive with no answer at all.
        serde_json::json!({ "type": "extension_publish" }),
        // A well-formed publish. Still refused — the capability gate precedes every field check.
        serde_json::json!({
            "type": "extension_publish",
            "namespace": "ns",
            "audience": "capable",
            "payload": { "k": 1 },
        }),
        // A publish whose namespace/audience are garbage. Same answer, and specifically NOT
        // "Invalid namespace" (`:1289`) — pi never gets that far.
        serde_json::json!({ "type": "extension_publish", "namespace": 42, "audience": "nope" }),
    ] {
        alpha.send(&frame).await;
        let err = alpha.expect_frame("error").await;
        assert_eq!(
            err["error"], "Session has not advertised extension capability",
            "pi's `:1278` error text, for {frame}"
        );
    }

    // And an `error` is not a protocol error: pi `return`s, it does not throw.
    alpha.assert_alive("r-after-publish").await;
}

// R3(b). `extension_state_commit` ALWAYS produces an `extension_state_result`
// (`v0.9.2 broker/broker.ts:1379-1388`). This is the contradiction the previous batch left in
// place: it answered `cancel_message` on the reasoning that a silent drop hangs the caller, then
// silently dropped a commit, whose promise hangs in exactly the same way. Regression proof: the
// old arm wrote nothing, so `expect_frame("extension_state_result")` panics on the read timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extension_state_commit_is_always_answered_with_an_extension_state_result() {
    let broker = Broker::start().await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha", "alpha-session").await;

    // `String(msg.namespace || "")` (`v0.9.2 broker/broker.ts:1382`) echoes the RAW value — the
    // namespace type-check is at `:1395`, well past this branch — so each of these is what node
    // prints for that expression, not what a validated namespace would be.
    for (frame, echoed) in [
        // The frame confirmed live to be swallowed: a non-string namespace.
        (serde_json::json!({ "type": "extension_state_commit", "namespace": 42 }), "42"),
        // Absent, null and the other falsy values all short-circuit the `||` to "".
        (serde_json::json!({ "type": "extension_state_commit" }), ""),
        (serde_json::json!({ "type": "extension_state_commit", "namespace": null }), ""),
        (serde_json::json!({ "type": "extension_state_commit", "namespace": "" }), ""),
        (serde_json::json!({ "type": "extension_state_commit", "namespace": false }), ""),
        (serde_json::json!({ "type": "extension_state_commit", "namespace": 0 }), ""),
        // A valid namespace is echoed verbatim, with a well-formed rest-of-frame.
        (
            serde_json::json!({
                "type": "extension_state_commit",
                "namespace": "ns",
                "ownerEpoch": "e1",
                "expectedRevision": 0,
                "payload": { "k": 1 },
            }),
            "ns",
        ),
    ] {
        alpha.send(&frame).await;
        let res = alpha.expect_frame("extension_state_result").await;
        assert_eq!(res["namespace"], echoed, "String(namespace || \"\") for {frame}");
        assert_eq!(res["committed"], false, "for {frame}");
        assert_eq!(res["revision"], 0, "for {frame}");
        assert_eq!(
            res["reason"], "Session has not advertised extension capability",
            "pi's `:1385` reason, for {frame}"
        );
    }

    // pi `return`s from every one of those branches; it never throws.
    alpha.assert_alive("r-after-commit").await;
}

// MIRROR for both tests above. Answering these two tags must not make the broker answer-happy in
// general, and must not weaken `extension_capabilities_update`, whose failures are all `throw` →
// `socket.destroy` (`v0.9.2 broker/broker.ts:551-567`). Each frame here must still kill the
// connection, including the non-array `extensions` payload confirmed live to be accepted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_capabilities_updates_and_unknown_tags_still_destroy_the_connection() {
    let broker = Broker::start().await;
    for bad in [
        // `!Array.isArray(extensions)` → throw (`:560-562`).
        serde_json::json!({ "type": "extension_capabilities_update", "extensions": "not-an-array" }),
        // Missing entirely: `!Array.isArray(undefined)` is true upstream too.
        serde_json::json!({ "type": "extension_capabilities_update" }),
        // An entry failing `validateExtensionCapability` (`:563-567`): non-boolean `ownerEligible`.
        serde_json::json!({
            "type": "extension_capabilities_update",
            "extensions": [{ "namespace": "ns", "ownerEligible": "yes" }],
        }),
        // An entry whose namespace fails `validateNamespace` (`:1170-1182`).
        serde_json::json!({
            "type": "extension_capabilities_update",
            "extensions": [{ "namespace": "NotLowercase", "ownerEligible": true }],
        }),
        // Over `MAX_EXTENSIONS_PER_SESSION = 32` (`v0.9.2 broker/broker.ts:35,560`).
        serde_json::json!({
            "type": "extension_capabilities_update",
            "extensions": (0..33)
                .map(|i| serde_json::json!({ "namespace": format!("ns{i}"), "ownerEligible": true }))
                .collect::<Vec<_>>(),
        }),
        // And a genuinely unknown tag remains fatal (`default: throw`, `:971-972`).
        serde_json::json!({ "type": "extension_teleport", "namespace": "ns" }),
    ] {
        let mut c = RawClient::connect(&broker.socket).await;
        c.register("probe", &format!("probe-{}", uuid::Uuid::new_v4())).await;
        c.send(&bad).await;
        // A `list` that never gets its reply is the "socket destroyed" observation; an `EPIPE` on
        // the probe write is the same observation arriving sooner.
        if c.try_send(&serde_json::json!({ "type": "list", "requestId": "r1" })).await {
            assert!(
                c.next_frame().await.is_none(),
                "the broker must still destroy the connection for {bad}"
            );
        }
    }
}

// MIRROR, the other direction: the new refusals must not be stricter than pi either. A WELL-FORMED
// `extension_capabilities_update` is accepted upstream and must not disconnect here — pi's
// validation prefix passes and only the (unported) bus effects follow. It is also the one
// extension-bus frame that legitimately draws no reply from cyrup, so "answer everything" is not
// the rule being applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_well_formed_capabilities_update_is_accepted_and_draws_no_refusal() {
    let broker = Broker::start().await;
    let mut alpha = RawClient::connect(&broker.socket).await;
    alpha.register("alpha", "alpha-session").await;

    alpha
        .send(&serde_json::json!({
            "type": "extension_capabilities_update",
            "extensions": [
                { "namespace": "ns", "ownerEligible": true },
                // Unmodelled keys pass through upstream's object spread, so they must not be fatal.
                { "namespace": "a.b/c-d_e", "ownerEligible": false, "piFutureKey": 1 },
            ],
        }))
        .await;

    // The very next frame must be the `list` reply: no `error`, no `extension_state_result`, and
    // above all not a closed socket.
    alpha.send(&serde_json::json!({ "type": "list", "requestId": "r1" })).await;
    let next = alpha.next_frame().await.expect("the connection must survive a valid capabilities update");
    assert_eq!(next["type"], "sessions", "a valid capabilities update must draw no reply: {next}");
    assert_eq!(next["requestId"], "r1");
}

//! The frame dispatch switch and the endpoint-credential gate (`broker.ts:284-334,971-972`).
//!
//! One method: [`BrokerState::handle_frame`] resolves the `type` tag, applies the TCP endpoint
//! `stateId` gate and the before-register ordering rule, then hands off to the per-concern handler
//! in `session`, `send`, `receipts`, `presence` or `extensions`. `health` is answered inline
//! because `HealthOk` is not part of the `BrokerMessage` union.
//!
//! Split out of `broker/mod.rs` as the one place that names every handler, so no handler module
//! ever dispatches to another. They still share types and validators across module lines —
//! `session` imports `extensions::extensions_field_is_valid`, `send` and `mailbox` name
//! `receipts::MessageReceiptRoute` — but the frame-type dispatch happens only here.

use tokio::sync::mpsc::UnboundedSender;

use crate::transport::framing::encode_json;
use crate::transport::protocol::{HealthMessage, PROTOCOL_NAME, PROTOCOL_VERSION};

use super::frame::FrameResult;
use super::routing::SessionKey;
use super::state::BrokerState;

impl BrokerState {
    /// Handle one already-JSON-parsed frame (`handleMessage`, `broker.ts:298-563`). `session_key`
    /// is this connection's current session key — the `(scope, id)` pair, upstream's
    /// `sessionKey`/`currentKey` (`v0.13.0 broker/broker.ts:268,384`) — mutated on
    /// register/unregister. `self_tx` writes to this connection's own socket.
    pub(super) fn handle_frame(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_key: &mut Option<SessionKey>,
        now: u64,
    ) -> FrameResult {
        let Some(ty) = value.get("type").and_then(|v| v.as_str()) else {
            return FrameResult::protocol_error();
        };

        // `const requiresEndpointAuth = typeof LISTEN_TARGET !== "string";`
        // `const hasEndpointAuth = clientMessage.stateId === BROKER_STATE_ID;` (`broker.ts:284-285`).
        //
        // ICOM-015 — computed for EVERY frame, ahead of the health branch, exactly where upstream
        // computes it. `endpoint_state_id` is `Some` only on the TCP endpoint, so on a socket/pipe
        // both halves collapse to "no gate" and a client's `stateId` is ignored rather than
        // rejected — pi's `requiresEndpointAuth &&` short-circuit, which is what lets the SAME
        // client code send `stateId` on TCP and omit it on a socket.
        let has_endpoint_auth = self
            .endpoint_state_id
            .as_deref()
            .is_some_and(|id| value.get("stateId").and_then(|v| v.as_str()) == Some(id));
        let requires_endpoint_auth = self.endpoint_state_id.is_some();

        // health — legal before register, no TCP endpoint auth on a Unix socket (broker.ts:312-326).
        // HealthOk is not part of the BrokerMessage union, so it is encoded directly (framing only).
        if ty == "health" {
            let Some(rid) = value.get("requestId").and_then(|v| v.as_str()) else {
                return FrameResult::protocol_error();
            };
            // `if (requiresEndpointAuth && !hasEndpointAuth) throw new Error("Invalid intercom TCP
            // endpoint credentials")` (`broker.ts:290-292`) — AFTER the requestId shape check, so a
            // malformed health frame is malformed on every transport, and BEFORE the reply, so an
            // uncredentialled prober learns nothing about the broker (not even that it speaks the
            // protocol). A `throw` here is `socket.destroy(error)` (`:204-206`), i.e. cyrup's
            // `ProtocolError`.
            if requires_endpoint_auth && !has_endpoint_auth {
                return FrameResult::protocol_error();
            }
            if let Ok(frame) = encode_json(&HealthMessage::HealthOk {
                request_id: rid.to_string(),
                protocol: PROTOCOL_NAME.to_string(),
                version: PROTOCOL_VERSION,
            }) {
                let _ = self_tx.send(frame);
            }
            return FrameResult::cont();
        }

        // `if (requiresEndpointAuth && clientMessage.type === "register" && !hasEndpointAuth) throw`
        // (`broker.ts:303-305`) — register is gated on the credential BEFORE the
        // before-register ordering check below, and only register: every other frame type is
        // already unreachable pre-registration, and post-registration the connection itself is the
        // credential (a client that registered proved the state id once).
        if requires_endpoint_auth && ty == "register" && !has_endpoint_auth {
            return FrameResult::protocol_error();
        }

        // Only health/register are legal before registration (broker.ts:332-334).
        if session_key.is_none() && ty != "register" {
            return FrameResult::protocol_error();
        }

        match ty {
            "register" => self.handle_register(conn_id, self_tx, value, session_key, now),
            "unregister" => self.handle_unregister(conn_id, self_tx, session_key, now),
            "list" => self.handle_list(conn_id, self_tx, value, session_key),
            "send" => self.handle_send(conn_id, self_tx, value, session_key, now),
            "cancel_ask" => self.handle_cancel_ask(conn_id, value, session_key),
            "presence" => self.handle_presence(conn_id, value, session_key, now),
            "message_receipt" => self.handle_message_receipt(conn_id, value, session_key, now),
            "cancel_message" => {
                self.handle_cancel_message(conn_id, self_tx, value, session_key, now)
            }
            // Extension-bus frames (`v0.9.2 broker/broker.ts:551-585,961-969`). ICOM-016 landed the
            // effects, so the broker now advertises `EXTENSION_BUS_FEATURE` on `registered` and a
            // conforming pi client sends these as a matter of course (`supportsFeature` gate,
            // `v0.9.2 broker/client.ts:648,817-819`). `handle_extension_capabilities_update` records
            // the advertised namespaces and re-elects, `handle_extension_publish` fans out to the
            // capable set, and `handle_extension_state_commit` drives the revision-checked store in
            // `super::extension_state`. All three still port pi's validation prefix verbatim,
            // because a non-conforming peer can reach this socket regardless of what was advertised.
            "extension_publish" => {
                self.handle_extension_publish(conn_id, self_tx, value, session_key)
            }
            "extension_state_commit" => {
                self.handle_extension_state_commit(conn_id, self_tx, value, session_key)
            }
            "extension_capabilities_update" => {
                self.handle_extension_capabilities_update(conn_id, self_tx, value, session_key)
            }
            // Genuinely unknown tags stay fatal — that is pi's own behaviour
            // (`default: throw new Error(\`Unknown client message type\`)`,
            // `v0.9.2 broker/broker.ts:971-972`, routed to `socket.destroy(error)` by
            // `framing.ts:44-51` + `broker.ts:321-323`). Forward compatibility upstream comes from
            // additive FIELDS and feature negotiation, never from accepting unknown tags.
            _ => FrameResult::protocol_error(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::super::frame::FrameOutcome;
    use super::super::routing::SessionKey;
    use super::super::state::BrokerState;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Notify;

    /// ICOM-015 — the `requiresEndpointAuth` gate on the loopback-TCP endpoint
    /// (`v0.10.1 broker/broker.ts:284-305`). Pre-fix the broker had no `endpoint_state_id` at all
    /// and answered `health` and `register` from ANY connection on ANY endpoint, so every TCP
    /// assertion below failed: an uncredentialled prober got a `health_ok` and could register.
    ///
    /// All three groups matter, because the gate is asymmetric by design: it must reject on TCP
    /// without a credential, admit on TCP with the right one, and stay entirely out of the way on
    /// the socket endpoint — where pi's `requiresEndpointAuth &&` short-circuits and a client's
    /// `stateId` is simply not read. That last case is what lets ONE client implementation send
    /// `stateId` on TCP and omit it on a socket (`broker/client.ts:287`).
    #[test]
    fn the_tcp_endpoint_credential_gates_health_and_register_and_the_socket_endpoint_does_not() {
        fn drive(state_id: Option<&str>, frame: serde_json::Value) -> FrameOutcome {
            let mut state = BrokerState::new(
                30_000,
                Arc::new(Notify::new()),
                super::super::test_support::test_extension_state_dir(),
            )
            .with_listen_endpoint(state_id.is_none(), state_id.map(str::to_string));
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let mut sid = None;
            state.handle_frame(1, &tx, &frame, &mut sid, 1_000).outcome
        }
        let with_cred = |mut f: serde_json::Value, cred: Option<&str>| {
            if let (Some(o), Some(c)) = (f.as_object_mut(), cred) {
                o.insert("stateId".to_string(), json!(c));
            }
            f
        };
        let health =
            |cred: Option<&str>| with_cred(json!({ "type": "health", "requestId": "r1" }), cred);
        let register = |cred: Option<&str>| {
            with_cred(
                json!({
                    "type": "register", "sessionId": "s1",
                    "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
                }),
                cred,
            )
        };

        // TCP endpoint (`endpoint_state_id = Some`), no/wrong credential => `throw` => destroy.
        for frame in [
            health(None),
            health(Some("wrong")),
            register(None),
            register(Some("wrong")),
        ] {
            assert!(
                matches!(
                    drive(Some("run-state-id"), frame.clone()),
                    FrameOutcome::ProtocolError
                ),
                "an uncredentialled {} on the TCP endpoint must destroy the connection",
                frame["type"]
            );
        }
        // TCP endpoint, matching credential => served.
        for frame in [health(Some("run-state-id")), register(Some("run-state-id"))] {
            assert!(
                matches!(
                    drive(Some("run-state-id"), frame.clone()),
                    FrameOutcome::Continue
                ),
                "the credentialled {} must be served",
                frame["type"]
            );
        }
        // Socket endpoint: the gate does not exist, with or without a `stateId`.
        for frame in [
            health(None),
            health(Some("anything")),
            register(None),
            register(Some("x")),
        ] {
            assert!(
                matches!(drive(None, frame.clone()), FrameOutcome::Continue),
                "the socket endpoint must not gate {}",
                frame["type"]
            );
        }
    }
    /// ICOM-015 — `trustedLocal = typeof LISTEN_TARGET === "string" && platform !== "win32"`
    /// (`v0.10.1 broker/broker.ts:365`), stamped at register (`:374`). Pre-fix this was the
    /// compile-time `cfg!(unix)`, so a broker bound to the loopback-TCP endpoint on a unix host
    /// stamped `trustedLocal: true` on every session — the inverse of upstream, and the one field a
    /// peer reads to decide whether a connection came from a process that could open the socket.
    #[test]
    fn trusted_local_follows_the_bound_endpoint_not_the_platform() {
        for (trusted, label) in [(true, "socket"), (false, "tcp")] {
            let mut state = BrokerState::new(
                30_000,
                Arc::new(Notify::new()),
                super::super::test_support::test_extension_state_dir(),
            )
            .with_listen_endpoint(trusted, None);
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let mut sid = None;
            state.handle_frame(
                1,
                &tx,
                &json!({
                    "type": "register", "sessionId": "s1",
                    "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
                }),
                &mut sid,
                1_000,
            );
            assert_eq!(
                state
                    .sessions
                    .get(&SessionKey::unscoped("s1".to_string()))
                    .and_then(|s| s.info.trusted_local),
                Some(trusted),
                "the {label} endpoint's registration must carry trustedLocal = {trusted}"
            );
        }
    }
}

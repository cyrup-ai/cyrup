//! Message routing: `send` and `cancel_ask` (`broker.ts:424-596` plus the `v0.10.1` mailbox
//! fallback).
//!
//! [`BrokerState::handle_send`] resolves the target by session identity, enforces the mutual-ask
//! refusal, records the receipt route and the ask edge, and falls through to
//! `handle_send_to_disconnected` when the named peer has left — the offline path that parks the
//! message in `super::mailbox`. That fallback is the only handler-to-handler call in the broker,
//! which is why the two stay in one file and it stays private.

use tokio::sync::mpsc::UnboundedSender;

use crate::transport::protocol::{
    BrokerMessage, Message, MessageControl, MessageControlAction, now_ms,
};

use super::frame::{FrameResult, send_msg};
use super::receipts::MessageReceiptRoute;
use super::routing::{AskEdge, find_session_ids};
use super::state::BrokerState;

impl BrokerState {
    pub(super) fn handle_send(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_id: &Option<String>,
        now: u64,
    ) -> FrameResult {
        let Some(current_id) = session_id.clone() else {
            return FrameResult::protocol_error();
        };
        let message_val = value.get("message");
        let parsed_message =
            message_val.and_then(|m| serde_json::from_value::<Message>(m.clone()).ok());
        // messageId = isMessage(message) ? message.id : "unknown" (broker.ts:418).
        let message_id = parsed_message
            .as_ref()
            .map(|m| m.id.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let to = value.get("to").and_then(|v| v.as_str());
        let (to, message) = match (to, parsed_message) {
            (Some(to), Some(msg)) => (to.to_string(), msg),
            _ => {
                send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                    message_id,
                    reason: "Invalid message format".to_string(),
                });
                return FrameResult::cont();
            }
        };

        self.prune_ask_edges(now);
        // `this.pruneMessageReceiptRoutes(brokerReceivedAt)` (`v0.10.1 broker/broker.ts:502`).
        self.prune_message_receipt_routes(now);
        let reply_edge = message.reply_to.as_ref().and_then(|rt| self.ask_edges.get(rt).cloned());

        // Join-ordered, matching `findSessions`' `Array.from(this.sessions.values()/.entries())`
        // (`broker.ts:586-594`).
        let entries: Vec<(String, Option<String>)> = self
            .sessions_in_order()
            .map(|(_, s)| (s.info.id.clone(), s.info.name.clone()))
            .collect();
        let targets = find_session_ids(&entries, &to);

        if targets.len() > 1 {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: format!(
                    "Multiple sessions named \"{to}\" are connected. Use the session ID instead."
                ),
            });
            return FrameResult::cont();
        }
        let Some(target_id) = targets.first().cloned() else {
            // No LIVE target — fall through to the mailbox ladder
            // (`v0.10.1 broker/broker.ts:596-673`).
            return self.handle_send_to_disconnected(
                conn_id,
                self_tx,
                &to,
                &message,
                &current_id,
                reply_edge.as_ref(),
                now,
            );
        };

        // A reply must match a pending edge (broker.ts:434-441).
        if message.reply_to.is_some() && reply_edge.is_none() {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Reply target does not match a pending ask".to_string(),
            });
            return FrameResult::cont();
        }
        // The sender's own session must still own this socket (broker.ts:442-450).
        let Some(from_info) = self
            .sessions
            .get(&current_id)
            .filter(|s| s.conn_id == conn_id)
            .map(|s| s.info.clone())
        else {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Sender session not found".to_string(),
            });
            return FrameResult::cont();
        };
        // `if (message.supersedes)` (`v0.10.1 broker/broker.ts:522-533`): a supersede is only legal
        // against a message THIS sender previously got delivered to THIS receiver, which is exactly
        // what `messageReceiptRoutes` records. Without the table every supersede was accepted and
        // silently dropped its `message_control`, so the receiver never learned the earlier message
        // had been replaced.
        if let Some(superseded) = &message.supersedes {
            let route_ok = self
                .message_receipt_routes
                .get(superseded)
                .is_some_and(|route| route.from == current_id && route.to == target_id);
            if !route_ok {
                send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                    message_id: message.id.clone(),
                    reason:
                        "Supersede target does not match a previous message from this sender to this receiver"
                            .to_string(),
                });
                return FrameResult::cont();
            }
        }
        // A reply edge must point exactly current←target (broker.ts:452-459).
        if let Some(edge) = &reply_edge
            && (edge.to != current_id || edge.from != target_id)
        {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Reply target does not match the pending ask".to_string(),
            });
            return FrameResult::cont();
        }

        if message.expects_reply == Some(true) {
            // Mutual-ask refusal (broker.ts:460-469): reject if the target already has an open ask
            // back toward the sender (ignoring the edge this reply, if any, targets).
            let reply_to = message.reply_to.clone();
            let reverse = self.ask_edges.iter().any(|(mid, edge)| {
                Some(mid) != reply_to.as_ref() && edge.from == target_id && edge.to == current_id
            });
            if reverse {
                send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                    message_id: message.id.clone(),
                    reason: "Mutual ask refused: target session is already waiting for a reply from this session.".to_string(),
                });
                return FrameResult::cont();
            }
            self.ask_edges.insert(message.id.clone(), AskEdge {
                from: current_id.clone(),
                to: target_id.clone(),
                created_at: now,
            });
        }

        // Deliver to the target, then (on a reply) delete the satisfied edge, then ack the sender.
        //
        // The delivered envelope is pi's `deliveredMessage` (`v0.9.2 broker/broker.ts:672-676`):
        // the sender's message **spread verbatim**, with the two broker-owned timestamps stamped
        // on top. `Message`'s `#[serde(flatten)] extra` is what makes the "verbatim" half true in
        // Rust; without the stamps pi's latency instrumentation reads `undefined` on every
        // cyrup-brokered hop.
        let mut delivered = message.clone();
        delivered.broker_received_at = Some(now.into());
        delivered.broker_delivered_at = Some(now_ms().into());
        if let Some(target) = self.sessions.get(&target_id) {
            // The `message_control{action:"supersede"}` notice precedes the replacement message
            // (`v0.10.1 broker/broker.ts:558-571`), so a receiver that has not yet surfaced the
            // superseded message can drop it before the new one lands.
            if let Some(superseded) = &message.supersedes {
                send_msg(&target.tx, &BrokerMessage::MessageControl {
                    from: from_info.clone(),
                    control: MessageControl {
                        message_id: superseded.clone(),
                        action: MessageControlAction::Supersede,
                        timestamp: now_ms().into(),
                        superseded_by: Some(message.id.clone()),
                        detail: None,
                        extra: Default::default(),
                    },
                });
            }
            send_msg(&target.tx, &BrokerMessage::Message { from: from_info, message: delivered });
        }
        if let Some(rt) = &message.reply_to {
            self.ask_edges.remove(rt);
        }
        // `this.messageReceiptRoutes.set(...)` (`v0.10.1 broker/broker.ts:580`), dated from
        // `brokerReceivedAt` — NOT from the delivery — so the 1 h retention measures how long ago
        // the broker accepted the message.
        self.message_receipt_routes.insert(message.id.clone(), MessageReceiptRoute {
            from: current_id.clone(),
            to: target_id.clone(),
            created_at: now,
        });
        send_msg(self_tx, &BrokerMessage::Delivered { message_id: message.id.clone() });
        FrameResult::cont()
    }

    /// The mailbox ladder for a `send` whose target is not connected
    /// (`v0.10.1 broker/broker.ts:596-673`), reached only when `findSessions` returned nothing.
    ///
    /// Every refusal below is upstream's, in upstream's order, and the two shapes it does NOT queue
    /// are load-bearing: a `supersedes` (the earlier message cannot be reached to be replaced) and
    /// an `expectsReply` (a blocking ask parked for 24 h would hang the asker past its own
    /// timeout). The mail itself either goes to the ONE live session that has taken over the
    /// target's mailbox identity — name + cwd, never name alone — or is parked for it.
    #[allow(clippy::too_many_arguments)]
    fn handle_send_to_disconnected(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        to: &str,
        message: &Message,
        current_id: &str,
        reply_edge: Option<&AskEdge>,
        now: u64,
    ) -> FrameResult {
        let disconnected = self.find_disconnected_session_ids(to, now);
        let target_info = match disconnected.as_slice() {
            [only] => self.disconnected_sessions.get(only).map(|s| s.info.clone()),
            [] => {
                send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                    message_id: message.id.clone(),
                    reason: "Session not found".to_string(),
                });
                return FrameResult::cont();
            }
            _ => {
                send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                    message_id: message.id.clone(),
                    reason: format!(
                        "Multiple disconnected sessions named \"{to}\" can receive queued mail. Use the session ID instead."
                    ),
                });
                return FrameResult::cont();
            }
        };
        let Some(target) = target_info else {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Session not found".to_string(),
            });
            return FrameResult::cont();
        };

        // `:598-604`
        if message.reply_to.is_some() && reply_edge.is_none() {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Reply target does not match a pending ask".to_string(),
            });
            return FrameResult::cont();
        }
        // `:605-613`
        let Some(from_info) = self
            .sessions
            .get(current_id)
            .filter(|s| s.conn_id == conn_id)
            .map(|s| s.info.clone())
        else {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Sender session not found".to_string(),
            });
            return FrameResult::cont();
        };
        // `:615-622`
        if message.supersedes.is_some() {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Supersede target is not connected".to_string(),
            });
            return FrameResult::cont();
        }
        // `:623-630`
        if let Some(edge) = reply_edge
            && (edge.to != current_id || edge.from != target.id)
        {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Reply target does not match the pending ask".to_string(),
            });
            return FrameResult::cont();
        }
        // `:631-638` — ICOM-045's reason, in pi's own position: it belongs to a target the broker
        // KNOWS but cannot reach, not to a name it has never seen (that is `Session not found`).
        if message.expects_reply == Some(true) {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Target session is not currently connected; blocking asks are not queued"
                    .to_string(),
            });
            return FrameResult::cont();
        }

        // `:640-655`
        match self.find_unique_live_session_for_disconnected_session(&target, current_id) {
            Some(live_id) => {
                let mut delivered = message.clone();
                delivered.broker_received_at = Some(now.into());
                delivered.broker_delivered_at = Some(now_ms().into());
                if let Some(live) = self.sessions.get(&live_id) {
                    send_msg(&live.tx, &BrokerMessage::Message {
                        from: from_info,
                        message: delivered,
                    });
                }
                self.message_receipt_routes.insert(message.id.clone(), MessageReceiptRoute {
                    from: current_id.to_string(),
                    to: live_id,
                    created_at: now,
                });
            }
            None => self.queue_mailbox_message(from_info, target, message, now),
        }
        // `:656-658`
        if let Some(rt) = &message.reply_to {
            self.ask_edges.remove(rt);
        }
        send_msg(self_tx, &BrokerMessage::Delivered { message_id: message.id.clone() });
        FrameResult::cont()
    }

    pub(super) fn handle_cancel_ask(
        &mut self,
        conn_id: u64,
        value: &serde_json::Value,
        session_id: &Option<String>,
    ) -> FrameResult {
        let Some(current_id) = session_id.clone() else {
            return FrameResult::protocol_error();
        };
        let Some(message_id) = value.get("messageId").and_then(|v| v.as_str()) else {
            return FrameResult::protocol_error();
        };
        let owns_socket = self.sessions.get(&current_id).map(|s| s.conn_id) == Some(conn_id);
        let owns_edge = self.ask_edges.get(message_id).map(|e| e.from.as_str()) == Some(current_id.as_str());
        if owns_socket && owns_edge {
            self.ask_edges.remove(message_id);
        }
        FrameResult::cont()
    }

}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use serde_json::json;
    use super::super::test_support::{make_state, payloads, register, register_named, send_frame};

    /// ICOM-045's broker half (`v0.10.1 broker/broker.ts:631-638`, v0.10.0): a blocking ask against
    /// a target the broker KNOWS but cannot reach gets a reason that says blocking asks are not
    /// queued, so the model switches to `send` or retries instead of re-issuing the same ask.
    ///
    /// **The pre-ICOM-010 version of this test asserted that reason for a target the broker had
    /// NEVER seen (`to: "ghost"`), which is not pi's behaviour**: upstream reaches `:631-638` only
    /// inside `if (disconnectedTargets.length === 1)`, so an unknown name is `Session not found`
    /// whether or not it expects a reply. That over-broad approximation was the only way to express
    /// the message before the disconnected-session map existed; it is now placed where pi places
    /// it, and the never-seen case is pinned separately below.
    #[test]
    fn a_blocking_ask_to_a_disconnected_peer_is_refused_with_the_not_queued_reason() {
        fn drive(expects_reply: bool) -> String {
            let mut state = make_state();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let mut sid = None;
            let mut peer_sid = None;
            register(&mut state, 1, &mut sid, "s1");
            register(&mut state, 2, &mut peer_sid, "s2");
            // s2 leaves; the broker remembers it for its mailbox.
            state.on_connection_closed(2, &peer_sid, 1_500);
            while rx.try_recv().is_ok() {}
            state.handle_frame(
                1,
                &tx,
                &json!({
                    "type": "send", "to": "s2",
                    "message": { "id": "m1", "timestamp": 1, "expectsReply": expects_reply,
                                 "content": { "text": "hi" } },
                }),
                &mut sid,
                2_000,
            );
            let frame = rx.try_recv().expect("a reply frame");
            let payload: serde_json::Value =
                serde_json::from_slice(frame.get(4..).unwrap_or_default()).expect("json");
            payload["type"].as_str().unwrap_or_default().to_string()
                + " "
                + payload["reason"].as_str().unwrap_or_default()
        }
        assert_eq!(
            drive(true),
            "delivery_failed Target session is not currently connected; blocking asks are not queued"
        );
        // POSITIVE CONTROL: the same non-blocking send is ACCEPTED and parked, so the assertion
        // above is about `expectsReply` and not about the target being unreachable.
        assert_eq!(drive(false), "delivered ");
    }
    /// The never-registered target, which has no mailbox to queue into
    /// (`v0.10.1 broker/broker.ts:669-673`): `Session not found` for both shapes.
    #[test]
    fn a_send_to_a_name_the_broker_has_never_seen_is_session_not_found() {
        fn drive(expects_reply: bool) -> String {
            let mut state = make_state();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let mut sid = None;
            register(&mut state, 1, &mut sid, "s1");
            while rx.try_recv().is_ok() {}
            state.handle_frame(
                1,
                &tx,
                &json!({
                    "type": "send", "to": "ghost",
                    "message": { "id": "m1", "timestamp": 1, "expectsReply": expects_reply,
                                 "content": { "text": "hi" } },
                }),
                &mut sid,
                2_000,
            );
            let frame = rx.try_recv().expect("a delivery_failed frame");
            let payload: serde_json::Value =
                serde_json::from_slice(frame.get(4..).unwrap_or_default()).expect("json");
            payload["reason"].as_str().unwrap_or_default().to_string()
        }
        assert_eq!(drive(true), "Session not found");
        assert_eq!(drive(false), "Session not found");
    }
    /// **A disconnect must NOT clear the departing session's ask edges.** `clearAskEdgesForSession`
    /// has exactly one call site in `broker.ts` at every tag v0.9.2…v0.10.1 — the register-time
    /// identity takeover (`:350`) — and cyrup was additionally calling it from BOTH leave paths.
    ///
    /// Red before the fix: `a` asks `b`, `b`'s socket drops and reconnects (the routine state
    /// ICOM-003's reconnect ladder creates), and `b`'s reply is then refused with "Reply target
    /// does not match a pending ask" — the ask is unanswerable and `a` blocks until its own
    /// timeout.
    #[test]
    fn a_disconnect_preserves_the_ask_edge_so_the_reply_still_lands_after_reconnect() {
        let mut state = make_state();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, _b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);
        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "ask1", "timestamp": 1, "expectsReply": true, "content": { "text": "?" } }),
            1_100,
        );
        assert!(state.ask_edges.contains_key("ask1"), "the ask edge exists before the drop");

        state.on_connection_closed(2, &b_sid, 1_500);
        assert!(state.ask_edges.contains_key("ask1"), "a disconnect must not drop it");

        let (b2_tx, _b2_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut b2_sid = None;
        register_named(&mut state, 3, &mut b2_sid, &b2_tx, "b", "bob", "/w", 2_000);
        let _ = payloads(&mut a_rx);
        send_frame(
            &mut state,
            3,
            &b2_tx,
            &mut b2_sid,
            "a",
            json!({ "id": "r1", "timestamp": 1, "replyTo": "ask1", "content": { "text": "yes" } }),
            2_100,
        );
        let got = payloads(&mut a_rx);
        assert!(
            got.iter().any(|p| p["type"] == "message" && p["message"]["id"] == "r1"),
            "the reply reaches the asker after the reconnect: {got:?}"
        );
        assert!(!state.ask_edges.contains_key("ask1"), "and the satisfied edge is dropped");
    }
}

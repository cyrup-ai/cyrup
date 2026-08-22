//! Message receipt routes and the sender-side control frames that ride them
//! (`v0.10.1 broker/broker.ts:80-84,676-696` and the `cancel_message` case).
//!
//! A delivered message records where it went, so a receipt from the receiver can be forwarded back
//! to the original sender and so that sender can later `cancel` or `supersede` it.
//! [`BrokerState::handle_message_receipt`] answers every miss with a silent `break`, exactly as
//! upstream does; [`BrokerState::handle_cancel_message`] answers a well-formed cancel instead —
//! `delivered` when it lands, `delivery_failed` with upstream's reason when the route does not
//! authorise it — and, like every handler, destroys the connection on a malformed frame.
//!
//! Split out of `broker/mod.rs`; the route table itself lives on `BrokerState` in `state`.

use tokio::sync::mpsc::UnboundedSender;

use crate::transport::protocol::{
    BrokerMessage, MessageControl, MessageControlAction, MessageReceipt, now_ms,
};

use super::frame::{FrameResult, send_msg};
use super::state::BrokerState;

/// `interface MessageReceiptRoute` (`v0.10.1 broker/broker.ts:80-84`) — where a delivered message
/// went, so a receipt from the receiver can be forwarded back to its original sender and so the
/// sender can `cancel`/`supersede` it.
pub(super) struct MessageReceiptRoute {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) created_at: u64,
}

impl BrokerState {
    /// `case "message_receipt"` (`v0.10.1 broker/broker.ts:676-696`).
    ///
    /// pi validates the receipt with `isMessageReceipt()` — a bad one THROWS, i.e. destroys the
    /// connection — then looks the message up in `messageReceiptRoutes` and forwards the receipt to
    /// the original sender only if the route says this session was the receiver AND still owns this
    /// socket. A miss on any of the three is a silent `break`, not an error frame.
    pub(super) fn handle_message_receipt(
        &mut self,
        conn_id: u64,
        value: &serde_json::Value,
        session_id: &Option<String>,
        now: u64,
    ) -> FrameResult {
        let Some(current_id) = session_id.as_deref() else {
            return FrameResult::protocol_error();
        };
        let Some(receipt_val) = value.get("receipt") else {
            return FrameResult::protocol_error();
        };
        let Ok(receipt) = serde_json::from_value::<MessageReceipt>(receipt_val.clone()) else {
            // `throw new Error("Invalid message_receipt message")` (`v0.10.1 broker/broker.ts:681`).
            return FrameResult::protocol_error();
        };
        self.prune_message_receipt_routes(now);
        let route_from = self
            .message_receipt_routes
            .get(&receipt.message_id)
            .filter(|route| route.to == current_id)
            .map(|route| route.from.clone());
        let receiver_info = self
            .sessions
            .get(current_id)
            .filter(|s| s.conn_id == conn_id)
            .map(|s| s.info.clone());
        if let Some(from_id) = route_from
            && let Some(from) = receiver_info
            && let Some(sender) = self.sessions.get(&from_id)
        {
            send_msg(&sender.tx, &BrokerMessage::MessageReceipt { from, receipt });
        }
        FrameResult::cont()
    }

    /// `case "cancel_message"` (`v0.10.1 broker/broker.ts:698-745`).
    ///
    /// A non-string `messageId` throws upstream (`:701-703`) and is fatal here for the same reason.
    /// Past that there are two cancellable states, in pi's order: a message still PARKED in the
    /// mailbox (dropped in place, `:711-720`), and a message already DELIVERED whose receipt route
    /// still names this sender (a `message_control{action:"cancel"}` to the receiver, `:731-743`).
    /// Anything else is `delivery_failed` with pi's exact reason (`:735-741`).
    ///
    /// Answering matters: pi's `cancelMessage()` returns a promise settled only by
    /// `delivered`/`delivery_failed` (`v0.9.2 broker/client.ts:738`), so a silent drop would hang
    /// the caller instead.
    pub(super) fn handle_cancel_message(
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
        let Some(message_id) = value.get("messageId").and_then(|v| v.as_str()).map(str::to_string)
        else {
            return FrameResult::protocol_error();
        };
        self.prune_message_receipt_routes(now);
        self.prune_mailbox_messages(now);
        let sender_info = self
            .sessions
            .get(&current_id)
            .filter(|s| s.conn_id == conn_id)
            .map(|s| s.info.clone());

        // The parked-mail arm (`:711-720`): the sender may withdraw its own queued message before
        // the target ever comes back.
        let queued_index = self
            .mailbox_messages
            .iter()
            .position(|entry| entry.message.id == message_id && entry.from.id == current_id);
        if let Some(index) = queued_index
            && sender_info.is_some()
        {
            self.mailbox_messages.remove(index);
            if self.ask_edges.get(&message_id).is_some_and(|edge| edge.from == current_id) {
                self.ask_edges.remove(&message_id);
            }
            send_msg(self_tx, &BrokerMessage::Delivered { message_id });
            return FrameResult::cont();
        }

        let route = self.message_receipt_routes.get(&message_id);
        let receiver_tx = route
            .filter(|route| route.from == current_id)
            .and_then(|route| self.sessions.get(&route.to))
            .map(|receiver| receiver.tx.clone());
        let (Some(from), Some(receiver_tx)) = (sender_info, receiver_tx) else {
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id,
                reason: "Message cannot be cancelled by this session".to_string(),
            });
            return FrameResult::cont();
        };
        send_msg(&receiver_tx, &BrokerMessage::MessageControl {
            from,
            control: MessageControl {
                message_id: message_id.clone(),
                action: MessageControlAction::Cancel,
                timestamp: now_ms().into(),
                superseded_by: None,
                detail: None,
                extra: Default::default(),
            },
        });
        if self.ask_edges.get(&message_id).is_some_and(|edge| edge.from == current_id) {
            self.ask_edges.remove(&message_id);
        }
        send_msg(self_tx, &BrokerMessage::Delivered { message_id });
        FrameResult::cont()
    }

}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use serde_json::json;
    use super::super::test_support::{make_state, payloads, register_named, send_frame};

    /// `case "cancel_message"`'s two live arms (`v0.10.1 broker/broker.ts:711-743`), both of which
    /// were unreachable while `messageReceiptRoutes` and the mailbox did not exist: the sender
    /// could only ever be told "Message cannot be cancelled by this session".
    #[test]
    fn a_sender_can_cancel_both_parked_and_delivered_mail() {
        let mut state = make_state();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);

        // (1) delivered mail → a `message_control{cancel}` reaches the receiver.
        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "oops" } }),
            1_100,
        );
        let _ = payloads(&mut a_rx);
        let _ = payloads(&mut b_rx);
        state.handle_frame(
            1,
            &a_tx,
            &json!({ "type": "cancel_message", "messageId": "m1" }),
            &mut a_sid,
            1_200,
        );
        let control = payloads(&mut b_rx);
        assert!(
            control.iter().any(|p| p["type"] == "message_control"
                && p["control"]["action"] == "cancel"
                && p["control"]["messageId"] == "m1"),
            "the receiver is told the message was withdrawn: {control:?}"
        );
        assert_eq!(payloads(&mut a_rx).first().map(|p| p["type"].clone()), Some(json!("delivered")));

        // (2) parked mail → dropped in place, with no control frame to send anywhere.
        state.on_connection_closed(2, &b_sid, 1_500);
        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m2", "timestamp": 1, "content": { "text": "recall me" } }),
            1_600,
        );
        assert_eq!(state.mailbox_messages.len(), 1);
        let _ = payloads(&mut a_rx);
        state.handle_frame(
            1,
            &a_tx,
            &json!({ "type": "cancel_message", "messageId": "m2" }),
            &mut a_sid,
            1_700,
        );
        assert!(state.mailbox_messages.is_empty(), "the parked entry is withdrawn");
        assert_eq!(payloads(&mut a_rx).first().map(|p| p["type"].clone()), Some(json!("delivered")));

        // NEGATIVE CONTROL: a message this session never sent is still refused.
        state.handle_frame(
            1,
            &a_tx,
            &json!({ "type": "cancel_message", "messageId": "never" }),
            &mut a_sid,
            1_800,
        );
        let refused = payloads(&mut a_rx);
        assert_eq!(refused.first().map(|p| p["type"].clone()), Some(json!("delivery_failed")));
        assert_eq!(refused[0]["reason"], "Message cannot be cancelled by this session");
    }
    /// `case "message_receipt"` (`v0.10.1 broker/broker.ts:676-696`) forwards a receipt back to the
    /// ORIGINAL sender, which needs the `messageReceiptRoutes` entry the delivery wrote. Every pi
    /// >= 0.9.0 client emits `receiver_received` unconditionally on its first inbound message
    /// (`broker/client.ts:773-784`), so before the table existed every one of those receipts was
    /// dropped on the floor.
    #[test]
    fn a_receipt_is_forwarded_to_the_original_sender() {
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
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } }),
            1_100,
        );
        let _ = payloads(&mut a_rx);

        state.handle_frame(
            2,
            &b_tx,
            &json!({
                "type": "message_receipt",
                "receipt": { "messageId": "m1", "status": "receiver_received", "timestamp": 2 },
            }),
            &mut b_sid,
            1_200,
        );
        let got = payloads(&mut a_rx);
        assert!(
            got.iter().any(|p| p["type"] == "message_receipt"
                && p["receipt"]["messageId"] == "m1"
                && p["from"]["id"] == "b"),
            "the sender learns its message arrived: {got:?}"
        );

        // NEGATIVE CONTROL: a receipt for a message this session did not receive routes nowhere.
        state.handle_frame(
            2,
            &b_tx,
            &json!({
                "type": "message_receipt",
                "receipt": { "messageId": "other", "status": "receiver_received", "timestamp": 3 },
            }),
            &mut b_sid,
            1_300,
        );
        assert!(payloads(&mut a_rx).is_empty());
    }
    /// `if (message.supersedes)` (`v0.10.1 broker/broker.ts:522-533,558-571`): a supersede is legal
    /// only against a message this sender previously had delivered to this receiver, and when it is
    /// legal the receiver gets a `message_control{supersede}` BEFORE the replacement.
    #[test]
    fn a_supersede_is_validated_against_the_receipt_route_and_announced_before_the_replacement() {
        let mut state = make_state();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);

        // NEGATIVE FIRST: nothing to supersede.
        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m2", "timestamp": 1, "supersedes": "nope", "content": { "text": "v2" } }),
            1_100,
        );
        let refused = payloads(&mut a_rx);
        assert_eq!(refused.last().map(|p| p["type"].clone()), Some(json!("delivery_failed")));
        assert_eq!(
            refused[refused.len() - 1]["reason"],
            "Supersede target does not match a previous message from this sender to this receiver"
        );

        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "v1" } }),
            1_200,
        );
        let _ = payloads(&mut b_rx);
        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m3", "timestamp": 1, "supersedes": "m1", "content": { "text": "v2" } }),
            1_300,
        );
        let got = payloads(&mut b_rx);
        let control_at = got.iter().position(|p| p["type"] == "message_control");
        let message_at = got.iter().position(|p| p["type"] == "message");
        assert!(control_at.is_some() && message_at.is_some(), "both frames arrive: {got:?}");
        assert!(control_at < message_at, "the supersede notice precedes the replacement");
        let control = &got[control_at.unwrap_or(0)];
        assert_eq!(control["control"]["action"], "supersede");
        assert_eq!(control["control"]["messageId"], "m1");
        assert_eq!(control["control"]["supersededBy"], "m3");
    }
}

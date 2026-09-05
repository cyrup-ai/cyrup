//! Message routing: `send` and `cancel_ask` (`broker.ts:424-596` plus the `v0.10.1` mailbox
//! fallback).
//!
//! [`BrokerState::handle_send`] resolves the target by session identity, enforces the mutual-ask
//! refusal, records the receipt route and the ask edge, and falls through to
//! `handle_send_to_disconnected` when the named peer has left — the offline path that parks the
//! message in `super::mailbox`. That fallback is the only handler-to-handler call in the broker,
//! which is why the two stay in one file and it stays private.
//!
//! ICOM-054 adds the exact-target block ahead of name resolution, the
//! [`BrokerState::replay_or_reject`] gate on all three send arms, and a machine-readable `code` on
//! every refusal (`v0.13.0 broker/broker.ts:602-786`).

use tokio::sync::mpsc::UnboundedSender;

use crate::transport::protocol::{
    BrokerMessage, DeliveredState, Message, MessageControl, MessageControlAction, now_ms,
};

use super::delivery::{DeliveryFingerprint, RecordedOutcome};
use super::frame::{FrameResult, send_msg};
use super::receipts::MessageReceiptRoute;
use super::routing::{AskEdge, SessionKey, find_session_keys};
use super::state::BrokerState;

/// What [`BrokerState::replay_or_reject`] decided about a `send` frame.
///
/// ICOM-054 design note: upstream returns a bare `boolean` whose `true` means "I already answered
/// the sender, stop". A two-variant enum names both halves, so the gate cannot be read backwards at
/// any of its three call sites, and `#[must_use]` makes ignoring it a compile error rather than a
/// double delivery.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplayVerdict {
    /// A recorded outcome was replayed (or the id was refused); the caller must not deliver.
    Answered,
    /// Nothing recorded, or the one retryable rebound record; carry on with this send.
    Proceed,
}

impl BrokerState {
    pub(super) fn handle_send(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_key: &Option<SessionKey>,
        now: u64,
    ) -> FrameResult {
        let Some(current_key) = session_key.clone() else {
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
                send_msg(
                    self_tx,
                    &BrokerMessage::delivery_failed(
                        message_id,
                        "Invalid message format",
                        "E_INVALID_MESSAGE",
                        false,
                    ),
                );
                return FrameResult::cont();
            }
        };

        // ICOM-055 — `const fromSession = this.sessions.get(currentKey); if (!fromSession || …)`
        // (`v0.13.0 broker/broker.ts:614-618`), MOVED AHEAD of target resolution because the
        // sender's SCOPE is the input to every lookup below. Upstream moved it for the same reason
        // and deleted its two later copies; this port's two copies were `handle_send`'s (after
        // target resolution) and `handle_send_to_disconnected`'s, byte-identical apart from the
        // borrow. The only observable reordering is that a send from a superseded socket is now
        // refused `Sender session not found` before the multi-target and no-target arms can answer
        // — which is upstream's own order.
        let Some(from) = self
            .sessions
            .get(&current_key)
            .filter(|s| s.conn_id == conn_id)
        else {
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message.id.clone(),
                    "Sender session not found",
                    "E_SENDER_NOT_FOUND",
                    false,
                ),
            );
            return FrameResult::cont();
        };
        let from_info = from.info.clone();
        let scope = from.key.scope.clone();

        self.prune_ask_edges(now);
        // `this.pruneMessageReceiptRoutes(brokerReceivedAt)` (`v0.10.1 broker/broker.ts:502`).
        self.prune_message_receipt_routes(now);
        let reply_edge = message
            .reply_to
            .as_ref()
            .and_then(|rt| self.ask_edges.get(rt).cloned());

        // ICOM-054 — the exact-target block (`v0.13.0 broker/broker.ts:624-654`), placed BEFORE
        // name resolution because a VALID exact target REPLACES `to` rather than filtering it.
        //
        // Read off the raw frame rather than off a decoded [`ExactTarget`], because upstream's
        // first guard distinguishes "present but wrong" from "absent" — `hasTargetId !==
        // hasTargetEpoch` — and a decoded pair cannot tell the two apart. This whole handler works
        // on `serde_json::Value` for the same reason (see `broker/js.rs`).
        let exact_id = value.get("targetId");
        let exact_epoch = value.get("targetEpoch");
        let mut to = to;
        if exact_id.is_some() != exact_epoch.is_some()
            || exact_id.is_some_and(|v| v.as_str().is_none_or(str::is_empty))
            || exact_epoch.is_some_and(|v| v.as_str().is_none_or(str::is_empty))
        {
            // Half-supplied, non-string, or empty. NOT a connection kill, and NOT a silent
            // fallback to name routing: the sender asked for an exact endpoint and must be told it
            // did not get one, or a stale binding would be laundered into a name-routed delivery.
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message.id.clone(),
                    "Exact target requires an id and endpoint epoch",
                    "E_INVALID_TARGET",
                    false,
                ),
            );
            return FrameResult::cont();
        }
        if let (Some(target_id), Some(target_epoch)) = (
            exact_id.and_then(serde_json::Value::as_str),
            exact_epoch.and_then(serde_json::Value::as_str),
        ) {
            let fingerprint = DeliveryFingerprint::of(&message, target_id);
            if self.replay_or_reject(self_tx, &current_key, &message.id, &fingerprint, now)
                == ReplayVerdict::Answered
            {
                return FrameResult::cont();
            }
            // `this.sessions.get(scopedSessionKey(fromSession.scopeId, targetId))` (`:641`) — an
            // exact id is still resolved INSIDE the sender's scope (ICOM-055), so a cross-scope id
            // is `Session not found` exactly as an unknown name is.
            let exact_key = SessionKey::new(scope.clone(), target_id.to_string());
            let Some(exact) = self.sessions.get(&exact_key) else {
                self.record_delivery(
                    &current_key,
                    &message.id,
                    fingerprint,
                    RecordedOutcome::Failed {
                        reason: "Session not found".to_string(),
                        code: "E_TARGET_NOT_FOUND".to_string(),
                        retryable: false,
                    },
                    now,
                );
                send_msg(
                    self_tx,
                    &BrokerMessage::delivery_failed(
                        message.id.clone(),
                        "Session not found",
                        "E_TARGET_NOT_FOUND",
                        false,
                    ),
                );
                return FrameResult::cont();
            };
            if exact.info.endpoint_epoch.as_deref() != Some(target_epoch) {
                // The refusal this whole item exists for (`:648-652`). Recorded RETRYABLE, which is
                // the only record `replay_or_reject` lets a resend past — that is how the client's
                // one retry reaches the replacement endpoint under the SAME message id.
                self.record_delivery(
                    &current_key,
                    &message.id,
                    fingerprint,
                    RecordedOutcome::Failed {
                        reason: "Target endpoint changed before delivery".to_string(),
                        code: "E_TARGET_REBOUND".to_string(),
                        retryable: true,
                    },
                    now,
                );
                send_msg(
                    self_tx,
                    &BrokerMessage::delivery_failed(
                        message.id.clone(),
                        "Target endpoint changed before delivery",
                        "E_TARGET_REBOUND",
                        true,
                    ),
                );
                return FrameResult::cont();
            }
            // `clientMessage.to = targetId` (`:653`).
            to = target_id.to_string();
        }

        // Join-ordered, matching `findSessions`' `Array.from(this.sessions.values()/.entries())`
        // (`v0.13.0 broker/broker.ts:1247-1262`) — and SCOPE-FILTERED in all three tiers, so a
        // peer in another scope falls out of this ladder, falls out of the disconnected ladder
        // below, and lands on the same `Session not found` a never-seen name gets.
        let entries: Vec<(SessionKey, Option<String>)> = self
            .sessions_in_order()
            .map(|(key, s)| (key.clone(), s.info.name.clone()))
            .collect();
        let targets = find_session_keys(&entries, &to, scope.as_ref());

        if targets.len() > 1 {
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message.id.clone(),
                    format!(
                        "Multiple sessions named \"{to}\" are connected. Use the session ID instead."
                    ),
                    "E_AMBIGUOUS_TARGET",
                    false,
                ),
            );
            return FrameResult::cont();
        }
        let Some(target_key) = targets.first().cloned() else {
            // No LIVE target — fall through to the mailbox ladder
            // (`v0.10.1 broker/broker.ts:596-673`).
            return self.handle_send_to_disconnected(
                self_tx,
                &to,
                &message,
                &current_key,
                from_info,
                reply_edge.as_ref(),
                now,
            );
        };

        // A reply must match a pending edge (broker.ts:434-441).
        if message.reply_to.is_some() && reply_edge.is_none() {
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message.id.clone(),
                    "Reply target does not match a pending ask",
                    "E_REPLY_TARGET",
                    false,
                ),
            );
            return FrameResult::cont();
        }
        // ICOM-054 — the replay gate on the LIVE arm (`v0.13.0 broker/broker.ts:663-666`), in
        // upstream's position: after the pending-ask check, before `supersedes`. The guarantee is
        // not scoped to exact sends, so a plain re-send of an id lands here too.
        let fingerprint = DeliveryFingerprint::of(&message, &target_key.id);
        if self.replay_or_reject(self_tx, &current_key, &message.id, &fingerprint, now)
            == ReplayVerdict::Answered
        {
            return FrameResult::cont();
        }
        // (The sender's own session was proved to still own this socket above,
        // `v0.13.0 broker/broker.ts:614-618`.)
        // `if (message.supersedes)` (`v0.10.1 broker/broker.ts:522-533`): a supersede is only legal
        // against a message THIS sender previously got delivered to THIS receiver, which is exactly
        // what `messageReceiptRoutes` records. Without the table every supersede was accepted and
        // silently dropped its `message_control`, so the receiver never learned the earlier message
        // had been replaced.
        if let Some(superseded) = &message.supersedes {
            let route_ok = self
                .message_receipt_routes
                .get(superseded)
                .is_some_and(|route| route.from == current_key && route.to == target_key);
            if !route_ok {
                send_msg(
                    self_tx,
                    &BrokerMessage::delivery_failed(
                        message.id.clone(),
                        "Supersede target does not match a previous message from this sender to \
                         this receiver",
                        "E_SUPERSEDE_TARGET",
                        false,
                    ),
                );
                return FrameResult::cont();
            }
        }
        // A reply edge must point exactly current←target (broker.ts:452-459).
        if let Some(edge) = &reply_edge
            && (edge.to != current_key || edge.from != target_key)
        {
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message.id.clone(),
                    "Reply target does not match the pending ask",
                    "E_REPLY_TARGET",
                    false,
                ),
            );
            return FrameResult::cont();
        }

        if message.expects_reply == Some(true) {
            // Mutual-ask refusal (broker.ts:460-469): reject if the target already has an open ask
            // back toward the sender (ignoring the edge this reply, if any, targets).
            let reply_to = message.reply_to.clone();
            let reverse = self.ask_edges.iter().any(|(mid, edge)| {
                Some(mid) != reply_to.as_ref() && edge.from == target_key && edge.to == current_key
            });
            if reverse {
                send_msg(
                    self_tx,
                    &BrokerMessage::delivery_failed(
                        message.id.clone(),
                        "Mutual ask refused: target session is already waiting for a reply from \
                         this session.",
                        "E_MUTUAL_ASK",
                        false,
                    ),
                );
                return FrameResult::cont();
            }
            self.ask_edges.insert(
                message.id.clone(),
                AskEdge {
                    from: current_key.clone(),
                    to: target_key.clone(),
                    created_at: now,
                },
            );
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
        if let Some(target) = self.sessions.get(&target_key) {
            // The `message_control{action:"supersede"}` notice precedes the replacement message
            // (`v0.10.1 broker/broker.ts:558-571`), so a receiver that has not yet surfaced the
            // superseded message can drop it before the new one lands.
            if let Some(superseded) = &message.supersedes {
                send_msg(
                    &target.tx,
                    &BrokerMessage::MessageControl {
                        from: from_info.clone(),
                        control: MessageControl {
                            message_id: superseded.clone(),
                            action: MessageControlAction::Supersede,
                            timestamp: now_ms().into(),
                            superseded_by: Some(message.id.clone()),
                            detail: None,
                            extra: Default::default(),
                        },
                    },
                );
            }
            send_msg(
                &target.tx,
                &BrokerMessage::Message {
                    from: from_info,
                    message: delivered,
                },
            );
        }
        // ICOM-054 — `updateDeliveryRecord(currentKey, message.supersedes, "failed", …)`
        // (`v0.13.0 broker/broker.ts:709`), so a replayed re-send of the SUPERSEDED id reports the
        // supersede rather than re-asserting its original success.
        if let Some(superseded) = &message.supersedes {
            self.update_delivery_record(
                &current_key,
                superseded,
                RecordedOutcome::Failed {
                    reason: format!("Superseded by {}", message.id),
                    code: "E_DELIVERY_SUPERSEDED".to_string(),
                    retryable: false,
                },
            );
        }
        if let Some(rt) = &message.reply_to {
            self.ask_edges.remove(rt);
        }
        // `this.messageReceiptRoutes.set(...)` (`v0.10.1 broker/broker.ts:580`), dated from
        // `brokerReceivedAt` — NOT from the delivery — so the 1 h retention measures how long ago
        // the broker accepted the message.
        self.message_receipt_routes.insert(
            message.id.clone(),
            MessageReceiptRoute {
                from: current_key.clone(),
                to: target_key.clone(),
                created_at: now,
            },
        );
        self.record_delivery(
            &current_key,
            &message.id,
            fingerprint,
            RecordedOutcome::Delivered(DeliveredState::SocketDelivered),
            now,
        );
        send_msg(
            self_tx,
            &BrokerMessage::delivered(message.id.clone(), DeliveredState::SocketDelivered),
        );
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
        self_tx: &UnboundedSender<Vec<u8>>,
        to: &str,
        message: &Message,
        current_key: &SessionKey,
        from_info: crate::transport::protocol::SessionInfo,
        reply_edge: Option<&AskEdge>,
        now: u64,
    ) -> FrameResult {
        // `this.findDisconnectedSessions(clientMessage.to, fromSession.scopeId)`
        // (`v0.13.0 broker/broker.ts:731`) — the mailbox ladder is scoped exactly as the live one
        // is, so a scoped peer cannot even reach another scope's PARKED identity.
        let disconnected = self.find_disconnected_session_keys(to, current_key.scope.as_ref(), now);
        let target_info = match disconnected.as_slice() {
            [only] => self.disconnected_sessions.get(only).map(|s| s.info.clone()),
            [] => {
                send_msg(
                    self_tx,
                    &BrokerMessage::delivery_failed(
                        message.id.clone(),
                        "Session not found",
                        "E_TARGET_NOT_FOUND",
                        false,
                    ),
                );
                return FrameResult::cont();
            }
            _ => {
                send_msg(
                    self_tx,
                    &BrokerMessage::delivery_failed(
                        message.id.clone(),
                        format!(
                            "Multiple disconnected sessions named \"{to}\" can receive queued mail. Use the session ID instead."
                        ),
                        "E_AMBIGUOUS_TARGET",
                        false,
                    ),
                );
                return FrameResult::cont();
            }
        };
        let Some(target) = target_info else {
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message.id.clone(),
                    "Session not found",
                    "E_TARGET_NOT_FOUND",
                    false,
                ),
            );
            return FrameResult::cont();
        };

        // `:598-604`
        if message.reply_to.is_some() && reply_edge.is_none() {
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message.id.clone(),
                    "Reply target does not match a pending ask",
                    "E_REPLY_TARGET",
                    false,
                ),
            );
            return FrameResult::cont();
        }
        // ICOM-054 — the replay gate on the MAILBOX arm (`v0.13.0 broker/broker.ts:739-742`),
        // fingerprinted against the DISCONNECTED target's id, so a resend that is parked and a
        // resend that is delivered live share one record.
        let fingerprint = DeliveryFingerprint::of(message, &target.id);
        if self.replay_or_reject(self_tx, current_key, &message.id, &fingerprint, now)
            == ReplayVerdict::Answered
        {
            return FrameResult::cont();
        }
        // (`:605-613`'s sender lookup is hoisted into `handle_send`, as upstream hoisted its own
        // at `v0.13.0 broker/broker.ts:614-618`.)
        // `:615-622`
        if message.supersedes.is_some() {
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message.id.clone(),
                    "Supersede target is not connected",
                    "E_SUPERSEDE_TARGET",
                    false,
                ),
            );
            return FrameResult::cont();
        }
        // `:623-630`
        if let Some(edge) = reply_edge
            && (edge.to != *current_key || edge.from.id != target.id)
        {
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message.id.clone(),
                    "Reply target does not match the pending ask",
                    "E_REPLY_TARGET",
                    false,
                ),
            );
            return FrameResult::cont();
        }
        // `:631-638` — ICOM-045's reason, in pi's own position: it belongs to a target the broker
        // KNOWS but cannot reach, not to a name it has never seen (that is `Session not found`).
        if message.expects_reply == Some(true) {
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message.id.clone(),
                    "Target session is not currently connected; blocking asks are not queued",
                    "E_TARGET_DISCONNECTED",
                    false,
                ),
            );
            return FrameResult::cont();
        }

        // `:640-655`
        let live_delivered = match self.find_unique_live_session_for_disconnected_session(
            current_key.scope.as_ref(),
            &target,
            current_key,
        ) {
            Some(live_key) => {
                let mut delivered = message.clone();
                delivered.broker_received_at = Some(now.into());
                delivered.broker_delivered_at = Some(now_ms().into());
                if let Some(live) = self.sessions.get(&live_key) {
                    send_msg(
                        &live.tx,
                        &BrokerMessage::Message {
                            from: from_info,
                            message: delivered,
                        },
                    );
                }
                self.message_receipt_routes.insert(
                    message.id.clone(),
                    MessageReceiptRoute {
                        from: current_key.clone(),
                        to: live_key,
                        created_at: now,
                    },
                );
                DeliveredState::SocketDelivered
            }
            None => {
                self.queue_mailbox_message(
                    current_key.clone(),
                    from_info,
                    SessionKey::new(current_key.scope.clone(), target.id.clone()),
                    target,
                    message,
                    now,
                );
                DeliveredState::Queued
            }
        };
        // `:656-658`
        if let Some(rt) = &message.reply_to {
            self.ask_edges.remove(rt);
        }
        // `recordDelivery(currentKey, message.id, fingerprint, liveMailboxTarget ?
        // "socket_delivered" : "queued")` (`v0.13.0 broker/broker.ts:775-776`) — a PARKED message
        // records `queued`, which `flush_mailbox_for_session` later flips to `socket_delivered`.
        self.record_delivery(
            current_key,
            &message.id,
            fingerprint,
            RecordedOutcome::Delivered(live_delivered),
            now,
        );
        send_msg(
            self_tx,
            &BrokerMessage::delivered(message.id.clone(), live_delivered),
        );
        FrameResult::cont()
    }

    /// `replayOrReject` (`v0.13.0 broker/broker.ts:1060-1077`). [`ReplayVerdict::Answered`] ⇒ this
    /// frame is fully answered; the caller must return without delivering.
    ///
    /// The `E_TARGET_REBOUND`-and-retryable arm is the ONE hole in the replay rule, and it is what
    /// makes the client's single retry work: the rebound refusal is recorded so a *changed* resend
    /// under that id is still caught by the fingerprint check above it, but an identical resend is
    /// allowed through to the target's new epoch.
    fn replay_or_reject(
        &mut self,
        self_tx: &UnboundedSender<Vec<u8>>,
        from: &SessionKey,
        message_id: &str,
        fingerprint: &DeliveryFingerprint,
        now: u64,
    ) -> ReplayVerdict {
        self.prune_delivery_records(now);
        let Some(record) = self
            .delivery_records
            .get(&(from.clone(), message_id.to_string()))
        else {
            return ReplayVerdict::Proceed;
        };
        if &record.fingerprint != fingerprint {
            send_msg(
                self_tx,
                &BrokerMessage::delivery_failed(
                    message_id,
                    "Message id was reused with different authored content",
                    "E_MESSAGE_ID_REUSE",
                    false,
                ),
            );
            return ReplayVerdict::Answered;
        }
        match &record.outcome {
            RecordedOutcome::Failed {
                code, retryable, ..
            } if code == "E_TARGET_REBOUND" && *retryable => ReplayVerdict::Proceed,
            RecordedOutcome::Delivered(state) => {
                send_msg(self_tx, &BrokerMessage::delivered(message_id, *state));
                ReplayVerdict::Answered
            }
            RecordedOutcome::Failed {
                reason,
                code,
                retryable,
            } => {
                // Upstream's `record.reason ?? "Previous delivery failed"` /
                // `record.code ?? "E_DELIVERY_FAILED"` fallbacks (`:1074`) are unreachable here by
                // construction: `RecordedOutcome::Failed` cannot exist without both.
                send_msg(
                    self_tx,
                    &BrokerMessage::delivery_failed(
                        message_id,
                        reason.clone(),
                        code.clone(),
                        *retryable,
                    ),
                );
                ReplayVerdict::Answered
            }
        }
    }

    pub(super) fn handle_cancel_ask(
        &mut self,
        conn_id: u64,
        value: &serde_json::Value,
        session_key: &Option<SessionKey>,
    ) -> FrameResult {
        let Some(current_key) = session_key.clone() else {
            return FrameResult::protocol_error();
        };
        let Some(message_id) = value.get("messageId").and_then(|v| v.as_str()) else {
            return FrameResult::protocol_error();
        };
        let owns_socket = self.sessions.get(&current_key).map(|s| s.conn_id) == Some(conn_id);
        let owns_edge = self.ask_edges.get(message_id).map(|e| &e.from) == Some(&current_key);
        if owns_socket && owns_edge {
            self.ask_edges.remove(message_id);
        }
        FrameResult::cont()
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::super::test_support::{make_state, payloads, register, register_named, send_frame};
    use serde_json::json;

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
            register(&mut state, 1, &mut sid, "s1", None);
            register(&mut state, 2, &mut peer_sid, "s2", None);
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
            register(&mut state, 1, &mut sid, "s1", None);
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
    /// ICOM-054 DoD 8 — **replies keep their existing behavior**, which `636f61e` guaranteed by
    /// DELETING `clearAskEdgesForSession(id)` from the register-time takeover (it has zero call
    /// sites at `v0.13.0 broker/broker.ts:1176`).
    ///
    /// The sibling test below already pins the DISCONNECT case; this is the LIVE stable-id
    /// replacement, which the takeover branch used to wipe. Red before ICOM-054: `b`'s reply was
    /// refused `Reply target does not match a pending ask`, so `a`'s ask was unanswerable.
    #[test]
    fn a_live_identity_takeover_preserves_the_ask_edge() {
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
        assert!(state.ask_edges.contains_key("ask1"));

        // b takes its own identity over on a NEW connection while the old one is still live.
        let (b2_tx, _b2_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut b2_sid = None;
        register_named(&mut state, 3, &mut b2_sid, &b2_tx, "b", "bob", "/w", 1_500);
        assert!(
            state.ask_edges.contains_key("ask1"),
            "the takeover must not wipe the replaced endpoint's ask edges"
        );
        let _ = payloads(&mut a_rx);

        send_frame(
            &mut state,
            3,
            &b2_tx,
            &mut b2_sid,
            "a",
            json!({ "id": "r1", "timestamp": 1, "replyTo": "ask1", "content": { "text": "yes" } }),
            1_600,
        );
        let got = payloads(&mut a_rx);
        assert!(
            got.iter()
                .any(|p| p["type"] == "message" && p["message"]["id"] == "r1"),
            "the reply must still reach the asker after a live replacement: {got:?}"
        );
    }

    // ---------------------------------------------------------------- ICOM-054

    /// The epoch the broker minted for `id`, read off the live roster.
    fn epoch_of(state: &super::BrokerState, id: &str) -> String {
        state
            .sessions
            .get(&super::SessionKey::unscoped(id.to_string()))
            .and_then(|s| s.info.endpoint_epoch.clone())
            .expect("every registered session carries an endpointEpoch")
    }

    /// A `send` frame carrying the optional exact-target pair verbatim, so a test can supply a
    /// half-set / empty / stale pair the typed [`crate::transport::protocol::ExactTarget`] cannot
    /// construct.
    #[allow(clippy::too_many_arguments)]
    fn send_exact(
        state: &mut super::BrokerState,
        conn_id: u64,
        tx: &tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        sid: &mut Option<super::SessionKey>,
        to: &str,
        message: serde_json::Value,
        extra: serde_json::Value,
        now: u64,
    ) {
        let mut frame = json!({ "type": "send", "to": to, "message": message });
        if let (Some(frame), Some(extra)) = (frame.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                frame.insert(k.clone(), v.clone());
            }
        }
        state.handle_frame(conn_id, tx, &frame, sid, now);
    }

    /// ICOM-054 DoD 1 + 10 — `endpointEpoch: randomUUID()` (`v0.13.0 broker/broker.ts:466`) is
    /// minted on EVERY register, a stable-id takeover included, and reaches peers on the wire; and
    /// `registered` advertises `exact-send-v1` beside the bus feature (`:498-502`).
    ///
    /// Red before ICOM-054: `SessionInfo` had no `endpoint_epoch` at all, so `session_joined`
    /// carried no `endpointEpoch` key and `features` listed only `extension-bus-v1`.
    #[test]
    fn every_register_mints_a_fresh_endpoint_epoch_and_advertises_exact_send() {
        let mut state = make_state();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);

        let registered = payloads(&mut b_rx)
            .into_iter()
            .find(|p| p["type"] == "registered")
            .expect("b is told it registered");
        assert_eq!(
            registered["features"],
            json!(["extension-bus-v1", "exact-send-v1"]),
            "`features: [EXTENSION_BUS_FEATURE, EXACT_SEND_FEATURE]` (`v0.13.0 broker/broker.ts:498-502`)"
        );

        let first = payloads(&mut a_rx)
            .into_iter()
            .find(|p| p["type"] == "session_joined" && p["session"]["id"] == "b")
            .expect("a sees b join")["session"]["endpointEpoch"]
            .as_str()
            .expect("the joined roster row carries a string endpointEpoch")
            .to_string();
        assert_eq!(first, epoch_of(&state, "b"));

        // b takes its own identity over on a new connection.
        let (b2_tx, _b2_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut b2_sid = None;
        register_named(&mut state, 3, &mut b2_sid, &b2_tx, "b", "bob", "/w", 2_000);
        let second = payloads(&mut a_rx)
            .into_iter()
            .find(|p| p["type"] == "session_joined" && p["session"]["id"] == "b")
            .expect("a sees b re-join")["session"]["endpointEpoch"]
            .as_str()
            .expect("string")
            .to_string();
        assert_ne!(
            first, second,
            "the id names the identity, the epoch names THIS socket binding of it"
        );
        assert_eq!(b2_sid.as_ref().map(|k| k.id.as_str()), Some("b"));
    }

    /// ICOM-054 DoD 2 — a send bound to a superseded endpoint is REFUSED, not silently routed to
    /// whatever the name resolves to now (`v0.13.0 broker/broker.ts:648-652`).
    ///
    /// Red before ICOM-054: the broker ignored `targetId`/`targetEpoch` entirely, so this send was
    /// name-routed to the replacement endpoint and acked `delivered`.
    #[test]
    fn an_exact_send_against_a_superseded_endpoint_is_refused_and_delivers_nothing() {
        let mut state = make_state();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, _b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);
        let stale = epoch_of(&state, "b");

        let (b2_tx, mut b2_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut b2_sid = None;
        register_named(&mut state, 3, &mut b2_sid, &b2_tx, "b", "bob", "/w", 2_000);
        let _ = payloads(&mut a_rx);
        let _ = payloads(&mut b2_rx);

        send_exact(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } }),
            json!({ "targetId": "b", "targetEpoch": stale }),
            2_100,
        );
        let ack = payloads(&mut a_rx);
        let failed = ack
            .iter()
            .find(|p| p["type"] == "delivery_failed")
            .unwrap_or_else(|| panic!("expected a refusal, got {ack:?}"));
        assert_eq!(failed["reason"], "Target endpoint changed before delivery");
        assert_eq!(failed["code"], "E_TARGET_REBOUND");
        assert_eq!(failed["retryable"], json!(true));
        assert_eq!(failed["delivery"], "failed");
        assert_eq!(failed["outcomeKnown"], json!(true));
        assert!(
            !payloads(&mut b2_rx).iter().any(|p| p["type"] == "message"),
            "the REPLACEMENT endpoint must receive nothing"
        );

        // DoD 4's broker half: the recorded rebound refusal is the one record a resend may pass,
        // so the client's single retry against the CURRENT epoch lands under the same message id.
        let current = epoch_of(&state, "b");
        send_exact(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } }),
            json!({ "targetId": "b", "targetEpoch": current }),
            2_200,
        );
        let retry = payloads(&mut a_rx);
        assert!(
            retry
                .iter()
                .any(|p| p["type"] == "delivered" && p["delivery"] == "socket_delivered"),
            "the retry must be accepted under the same id: {retry:?}"
        );
        assert!(
            payloads(&mut b2_rx)
                .iter()
                .any(|p| p["type"] == "message" && p["message"]["id"] == "m1"),
            "and the replacement endpoint receives it exactly once"
        );
    }

    /// ICOM-054 DoD 3 — a malformed exact target is a `delivery_failed`, never a connection kill
    /// and never a silent degrade to name routing (`v0.13.0 broker/broker.ts:624-633`).
    #[test]
    fn a_malformed_exact_target_is_refused_and_never_falls_back_to_name_routing() {
        for extra in [
            json!({ "targetId": "b" }),
            json!({ "targetEpoch": "e" }),
            json!({ "targetId": "", "targetEpoch": "e" }),
            json!({ "targetId": "b", "targetEpoch": "" }),
            json!({ "targetId": 7, "targetEpoch": "e" }),
        ] {
            let mut state = make_state();
            let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
            let (b_tx, mut b_rx) = tokio::sync::mpsc::unbounded_channel();
            let mut a_sid = None;
            let mut b_sid = None;
            register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
            register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);
            let _ = payloads(&mut a_rx);
            let _ = payloads(&mut b_rx);

            send_exact(
                &mut state,
                1,
                &a_tx,
                &mut a_sid,
                "b",
                json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } }),
                extra.clone(),
                1_100,
            );
            let ack = payloads(&mut a_rx);
            let failed = ack
                .iter()
                .find(|p| p["type"] == "delivery_failed")
                .unwrap_or_else(|| panic!("{extra} must be refused, got {ack:?}"));
            assert_eq!(
                failed["reason"],
                "Exact target requires an id and endpoint epoch"
            );
            assert_eq!(failed["code"], "E_INVALID_TARGET");
            assert!(
                !payloads(&mut b_rx).iter().any(|p| p["type"] == "message"),
                "{extra} must not be name-routed to the live peer"
            );
            // The connection survives: a plain send after it still works.
            send_frame(
                &mut state,
                1,
                &a_tx,
                &mut a_sid,
                "b",
                json!({ "id": "m2", "timestamp": 1, "content": { "text": "ok" } }),
                1_200,
            );
            assert!(
                payloads(&mut b_rx)
                    .iter()
                    .any(|p| p["message"]["id"] == "m2"),
                "the socket must stay open after {extra}"
            );
        }
    }

    /// ICOM-054 DoD 5 — bounded delivery records make a resend exactly-once per AUTHORED content
    /// (`v0.13.0 broker/broker.ts:1060-1077`), and that guarantee is not scoped to exact sends.
    ///
    /// Red before ICOM-054: there were no records, so the identical resend was injected a SECOND
    /// time and the changed resend was injected as a third message under the same id.
    #[test]
    fn a_resent_message_id_replays_its_ack_and_a_changed_one_is_refused() {
        let mut state = make_state();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);
        let _ = payloads(&mut a_rx);
        let _ = payloads(&mut b_rx);

        let original = json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } });
        for now in [1_100, 1_200] {
            send_frame(&mut state, 1, &a_tx, &mut a_sid, "b", original.clone(), now);
        }
        let acks = payloads(&mut a_rx);
        assert_eq!(
            acks.iter()
                .filter(|p| p["type"] == "delivered" && p["delivery"] == "socket_delivered")
                .count(),
            2,
            "both sends are acked, the second from the record: {acks:?}"
        );
        assert_eq!(
            payloads(&mut b_rx)
                .iter()
                .filter(|p| p["type"] == "message" && p["message"]["id"] == "m1")
                .count(),
            1,
            "but the receiver gets the message exactly once"
        );

        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "DIFFERENT" } }),
            1_300,
        );
        let reuse = payloads(&mut a_rx);
        let failed = reuse
            .iter()
            .find(|p| p["type"] == "delivery_failed")
            .unwrap_or_else(|| panic!("expected the reuse refusal, got {reuse:?}"));
        assert_eq!(
            failed["reason"],
            "Message id was reused with different authored content"
        );
        assert_eq!(failed["code"], "E_MESSAGE_ID_REUSE");
        assert!(
            !payloads(&mut b_rx).iter().any(|p| p["type"] == "message"),
            "and the changed content is not delivered"
        );
    }

    /// ICOM-054 — two senders whose keys and ids both contain `:` must not collide, which is what
    /// upstream's `JSON.stringify([fromKey, messageId])` buys and a naive `from + ":" + id` would
    /// lose (`v0.13.0 broker/broker.ts:1055-1057`).
    #[test]
    fn delivery_record_keys_do_not_collide_across_senders() {
        let mut state = make_state();
        let (x_tx, mut x_rx) = tokio::sync::mpsc::unbounded_channel();
        let (y_tx, mut y_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut x_sid, mut y_sid, mut b_sid) = (None, None, None);
        register_named(&mut state, 1, &mut x_sid, &x_tx, "a:b", "x", "/w", 1_000);
        register_named(&mut state, 2, &mut y_sid, &y_tx, "a", "y", "/w", 1_000);
        register_named(&mut state, 3, &mut b_sid, &b_tx, "peer", "bob", "/w", 1_000);
        let _ = (
            payloads(&mut x_rx),
            payloads(&mut y_rx),
            payloads(&mut b_rx),
        );

        // sender "a:b" + id "c" and sender "a" + id "b:c" are different records.
        send_frame(
            &mut state,
            1,
            &x_tx,
            &mut x_sid,
            "peer",
            json!({ "id": "c", "timestamp": 1, "content": { "text": "one" } }),
            1_100,
        );
        send_frame(
            &mut state,
            2,
            &y_tx,
            &mut y_sid,
            "peer",
            json!({ "id": "b:c", "timestamp": 1, "content": { "text": "two" } }),
            1_200,
        );
        let got = payloads(&mut b_rx);
        assert_eq!(
            got.iter().filter(|p| p["type"] == "message").count(),
            2,
            "neither send may be mistaken for a replay of the other: {got:?}"
        );
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
        assert!(
            state.ask_edges.contains_key("ask1"),
            "the ask edge exists before the drop"
        );

        state.on_connection_closed(2, &b_sid, 1_500);
        assert!(
            state.ask_edges.contains_key("ask1"),
            "a disconnect must not drop it"
        );

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
            got.iter()
                .any(|p| p["type"] == "message" && p["message"]["id"] == "r1"),
            "the reply reaches the asker after the reconnect: {got:?}"
        );
        assert!(
            !state.ask_edges.contains_key("ask1"),
            "and the satisfied edge is dropped"
        );
    }
}

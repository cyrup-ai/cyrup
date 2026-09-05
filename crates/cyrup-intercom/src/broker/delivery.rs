//! Bounded per-sender delivery records (`v0.13.0 broker/broker.ts:44-46,65-74,1043-1110`), the
//! store ICOM-054's exact-send refusal is built on.
//!
//! One record per `(sender session key, message id)`, so a re-sent id either REPLAYS its recorded
//! outcome — when the authored content is identical — or is refused, instead of being delivered
//! twice. Bounded two ways, both upstream's: a 1 h TTL and a 4096-entry FIFO cap.
//!
//! The guarantee is NOT scoped to exact sends: [`BrokerState::replay_or_reject`] runs on all three
//! send arms, so any resend of a message id is caught. The single hole is the
//! `E_TARGET_REBOUND`-and-retryable record, which is what lets the client retry a rebound target
//! under the same message id (`v0.13.0 broker/broker.ts:1068-1070`).

use crate::transport::protocol::{Attachment, DeliveredState, Message};

use super::limits::{DELIVERY_RECORD_RETENTION_MS, MAX_DELIVERY_RECORDS};
use super::routing::SessionKey;
use super::state::BrokerState;

/// `deliveryFingerprint` (`v0.13.0 broker/broker.ts:1043-1053`) — the AUTHORED content of a send,
/// the part a resend must not change.
///
/// Upstream `JSON.stringify`s a fixed-key object; this is a struct compared with `==`, because the
/// fingerprint never crosses the wire — byte-identity with JS's serialisation is not a requirement,
/// and structural equality is stronger (no separator ambiguity, no key-order hazard).
///
/// The FIELD SET is upstream's exactly. Note it takes `content.text` and `content.attachments`
/// individually rather than the whole `MessageContent`, so a differing `#[serde(flatten)] extra` on
/// the content object is NOT a fingerprint change — matching pi, and deliberately: the broker-owned
/// timestamps and the receipt bookkeeping are not authored content.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DeliveryFingerprint {
    target_id: String,
    text: String,
    attachments: Option<Vec<Attachment>>,
    reply_to: Option<String>,
    expects_reply: Option<bool>,
    supersedes: Option<String>,
    retry_of: Option<String>,
}

impl DeliveryFingerprint {
    /// The fingerprint of `message` sent to `target_id`.
    pub(super) fn of(message: &Message, target_id: &str) -> Self {
        Self {
            target_id: target_id.to_string(),
            text: message.content.text.clone(),
            attachments: message.content.attachments.clone(),
            reply_to: message.reply_to.clone(),
            expects_reply: message.expects_reply,
            supersedes: message.supersedes.clone(),
            retry_of: message.retry_of.clone(),
        }
    }
}

/// What a recorded delivery ended up as (`DeliveryRecord`'s
/// `state`/`reason`/`code`/`retryable` quartet, `v0.13.0 broker/broker.ts:65-74`), as ONE sum type.
///
/// ICOM-054 design note. Upstream carries four independent fields and then defends itself with
/// `record.reason ?? "Previous delivery failed"` / `record.code ?? "E_DELIVERY_FAILED"` (`:1074`),
/// because in TypeScript nothing stops a `"failed"` state from being stored with no reason — or a
/// `"socket_delivered"` state from being stored WITH a failure code. Splitting the quartet makes
/// both states unrepresentable: a success has no code and no reason, and a failure always has both.
/// The two `??` defaults are consequently unreachable here rather than merely unused, and the
/// broker never emits `unknown` (the client-only state) into a record at all.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum RecordedOutcome {
    /// `"socket_delivered"` or `"queued"` — `recordDelivery(…, state)` with no reason or code
    /// (`v0.13.0 broker/broker.ts:721,775`), and `updateDeliveryRecord(…, "socket_delivered")` on a
    /// mailbox flush (`:1162`).
    Delivered(DeliveredState),
    /// `"failed"` with the reason and code that always accompany it (`:644,649,709,823,856,1005,
    /// :1021`). `retryable` is `true` for exactly one of them, `E_TARGET_REBOUND`.
    Failed {
        /// The human-facing reason, byte-identical to upstream's.
        reason: String,
        /// The machine-readable code.
        code: String,
        /// Whether the sender may retry under the same message id.
        retryable: bool,
    },
}

/// `interface DeliveryRecord` (`v0.13.0 broker/broker.ts:65-74`).
pub(super) struct DeliveryRecord {
    /// The authored content this id was first used for.
    pub(super) fingerprint: DeliveryFingerprint,
    /// The outcome to replay.
    pub(super) outcome: RecordedOutcome,
    /// Insertion time, for the TTL prune. Never moved by an in-place update (`:1103-1110`).
    pub(super) created_at: u64,
}

/// `deliveryRecordKey` (`v0.13.0 broker/broker.ts:1055-1057`).
///
/// Upstream needs `JSON.stringify([fromKey, messageId])` because a JS `Map` key must be a primitive
/// and a naive `from + ":" + id` collides (sender `a:b` + id `c` against sender `a` + id `b:c`). A
/// Rust tuple key is structurally unambiguous, so the escaping problem does not arise.
pub(super) type DeliveryRecordKey = (SessionKey, String);

impl BrokerState {
    /// `pruneDeliveryRecords` (`v0.13.0 broker/broker.ts:1097-1101`).
    pub(super) fn prune_delivery_records(&mut self, now: u64) {
        let records = &mut self.delivery_records;
        records.retain(|_, r| now.saturating_sub(r.created_at) <= DELIVERY_RECORD_RETENTION_MS);
        self.delivery_record_order
            .retain(|k| records.contains_key(k));
    }

    /// `recordDelivery` (`v0.13.0 broker/broker.ts:1079-1095`) — prune, evict oldest-first down to
    /// the cap, then insert.
    pub(super) fn record_delivery(
        &mut self,
        from: &SessionKey,
        message_id: &str,
        fingerprint: DeliveryFingerprint,
        outcome: RecordedOutcome,
        now: u64,
    ) {
        self.prune_delivery_records(now);
        while self.delivery_records.len() >= MAX_DELIVERY_RECORDS {
            // `this.deliveryRecords.keys().next().value` (`:1082-1085`) — a JS `Map` iterates in
            // insertion order; a `HashMap` does not, which is why `delivery_record_order` exists.
            let Some(oldest) = self.delivery_record_order.first().cloned() else {
                break;
            };
            self.delivery_record_order.remove(0);
            self.delivery_records.remove(&oldest);
        }
        let key = (from.clone(), message_id.to_string());
        let record = DeliveryRecord {
            fingerprint,
            outcome,
            created_at: now,
        };
        // `Map.set` on an EXISTING key keeps its original position, so only a new key is pushed.
        if self.delivery_records.insert(key.clone(), record).is_none() {
            self.delivery_record_order.push(key);
        }
    }

    /// `updateDeliveryRecord` (`v0.13.0 broker/broker.ts:1103-1110`) — a later lifecycle event
    /// overwrites the outcome in place.
    ///
    /// A miss is a silent no-op, exactly as upstream's `if (!record) return`. Never touches the
    /// fingerprint, the insertion order, or `created_at`.
    pub(super) fn update_delivery_record(
        &mut self,
        from: &SessionKey,
        message_id: &str,
        outcome: RecordedOutcome,
    ) {
        if let Some(record) = self
            .delivery_records
            .get_mut(&(from.clone(), message_id.to_string()))
        {
            record.outcome = outcome;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::super::test_support::make_state;
    use super::*;

    fn message(id: &str, text: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "timestamp": 1, "content": { "text": text },
        }))
        .expect("a well-formed message")
    }

    fn key(id: &str) -> SessionKey {
        SessionKey::unscoped(id.to_string())
    }

    /// ICOM-054 DoD 6 — `MAX_DELIVERY_RECORDS` evicts the OLDEST INSERTED entry
    /// (`v0.13.0 broker/broker.ts:1081-1085`), and an in-place update does not move a key in that
    /// order (`Map.set` on an existing key keeps its position).
    #[test]
    fn the_record_store_is_capped_and_evicts_oldest_inserted_first() {
        let mut state = make_state();
        let sender = key("a");
        for n in 0..MAX_DELIVERY_RECORDS + 3 {
            let id = format!("m{n}");
            let msg = message(&id, "hi");
            state.record_delivery(
                &sender,
                &id,
                DeliveryFingerprint::of(&msg, "b"),
                RecordedOutcome::Delivered(DeliveredState::SocketDelivered),
                1_000,
            );
            // Re-stating the FIRST surviving record's outcome must not rescue it from eviction.
            state.update_delivery_record(
                &sender,
                "m3",
                RecordedOutcome::Delivered(DeliveredState::Queued),
            );
        }
        assert_eq!(state.delivery_records.len(), MAX_DELIVERY_RECORDS);
        assert_eq!(state.delivery_record_order.len(), MAX_DELIVERY_RECORDS);
        assert!(
            !state
                .delivery_records
                .contains_key(&(sender.clone(), "m0".to_string())),
            "the oldest inserted record is the one evicted"
        );
        for evicted in ["m1", "m2"] {
            assert!(
                !state
                    .delivery_records
                    .contains_key(&(sender.clone(), evicted.to_string())),
                "{evicted} was inserted before the cap was reached"
            );
        }
        assert_eq!(
            state.delivery_record_order.first(),
            Some(&(sender.clone(), "m3".to_string())),
            "m3 was RE-STATED on every iteration; an in-place update must not move a key to the \
             back of the eviction order, or the cap would evict the wrong entry"
        );
        assert!(
            state
                .delivery_records
                .contains_key(&(sender, format!("m{}", MAX_DELIVERY_RECORDS + 2))),
            "the newest is retained"
        );
    }

    /// ICOM-054 DoD 6 — the 1 h TTL (`v0.13.0 broker/broker.ts:1097-1101`), and the insertion-order
    /// vector must be pruned with it or it would leak keys forever.
    #[test]
    fn records_older_than_the_retention_window_are_pruned_with_their_order_entry() {
        let mut state = make_state();
        let sender = key("a");
        let msg = message("m1", "hi");
        state.record_delivery(
            &sender,
            "m1",
            DeliveryFingerprint::of(&msg, "b"),
            RecordedOutcome::Delivered(DeliveredState::SocketDelivered),
            1_000,
        );
        state.prune_delivery_records(1_000 + DELIVERY_RECORD_RETENTION_MS);
        assert_eq!(
            state.delivery_records.len(),
            1,
            "exactly at the TTL survives"
        );

        state.prune_delivery_records(1_001 + DELIVERY_RECORD_RETENTION_MS);
        assert!(state.delivery_records.is_empty());
        assert!(
            state.delivery_record_order.is_empty(),
            "the order vector must not outlive the map it indexes"
        );
    }

    /// ICOM-054 — the fingerprint covers exactly upstream's AUTHORED field set
    /// (`v0.13.0 broker/broker.ts:1043-1053`): the broker-owned timestamps are NOT part of it, so a
    /// re-send that only differs in `brokerReceivedAt` still replays rather than being refused.
    #[test]
    fn the_fingerprint_covers_authored_content_only() {
        let base = message("m1", "hi");
        let mut restamped = base.clone();
        restamped.broker_received_at = Some(99u64.into());
        restamped.timestamp = 42u64.into();
        assert_eq!(
            DeliveryFingerprint::of(&base, "b"),
            DeliveryFingerprint::of(&restamped, "b"),
            "broker timestamps are not authored content"
        );

        let mut retargeted = base.clone();
        retargeted.reply_to = Some("earlier".to_string());
        assert_ne!(
            DeliveryFingerprint::of(&base, "b"),
            DeliveryFingerprint::of(&retargeted, "b"),
        );
        assert_ne!(
            DeliveryFingerprint::of(&base, "b"),
            DeliveryFingerprint::of(&base, "c"),
            "the target id is part of the fingerprint"
        );
    }
}

//! Offline delivery — disconnected-session retention and the message mailbox
//! (`v0.10.1 broker/broker.ts:85-95,880-925,1010-1024`).
//!
//! A session that leaves is remembered for [`super::limits::DISCONNECTED_SESSION_RETENTION_MS`] so a
//! `send` naming it still routes; the message is parked in a FIFO mailbox and redelivered by
//! [`BrokerState::flush_mailbox_for_session`] when that identity registers again.
//!
//! Split out of `broker/mod.rs`, where these eight methods sat interleaved with the connection
//! bookkeeping they do not depend on. Their only outward calls are to `state` primitives.

use crate::transport::protocol::{BrokerMessage, Message, SessionInfo, now_ms};

use super::frame::send_msg;
use super::js::js_truthy_alias;
use super::limits::{
    DISCONNECTED_SESSION_RETENTION_MS, MAILBOX_MESSAGE_RETENTION_MS, MAX_MAILBOX_MESSAGES,
};
use super::receipts::MessageReceiptRoute;
use super::routing::find_session_ids;
use super::state::BrokerState;

/// `interface DisconnectedSession` (`v0.10.1 broker/broker.ts:85-88`) — the last-known
/// [`SessionInfo`] of a session that has left, kept for
/// [`DISCONNECTED_SESSION_RETENTION_MS`] so a `send` naming it can still be routed to its mailbox.
pub(super) struct DisconnectedSession {
    pub(super) info: SessionInfo,
    pub(super) disconnected_at: u64,
}

/// `interface MailboxMessage` (`v0.10.1 broker/broker.ts:90-95`) — one message parked for a
/// disconnected target, redelivered by [`BrokerState::flush_mailbox_for_session`] when that
/// identity registers again.
pub(super) struct MailboxMessage {
    pub(super) from: SessionInfo,
    pub(super) target: SessionInfo,
    pub(super) message: Message,
    pub(super) queued_at: u64,
}

impl BrokerState {
    /// `rememberDisconnectedSession` (`v0.10.1 broker/broker.ts:864-867`).
    ///
    /// pi stores a COPY (`{ ...info }`) because the live `ConnectedSession.info` it was read from
    /// keeps being mutated by presence frames; the Rust `SessionInfo` is moved/cloned in, so the
    /// same isolation is structural here.
    pub(super) fn remember_disconnected_session(&mut self, info: SessionInfo, now: u64) {
        self.disconnected_sessions
            .insert(info.id.clone(), DisconnectedSession { info, disconnected_at: now });
        self.prune_disconnected_sessions(now);
    }

    /// `pruneDisconnectedSessions` (`v0.10.1 broker/broker.ts:869-875`).
    pub(super) fn prune_disconnected_sessions(&mut self, now: u64) {
        self.disconnected_sessions.retain(|_, session| {
            now.saturating_sub(session.disconnected_at) <= DISCONNECTED_SESSION_RETENTION_MS
        });
    }

    /// `pruneMailboxMessages` (`v0.10.1 broker/broker.ts:877-888`).
    ///
    /// Dropping a parked ask must drop its ask edge too, or the sender's reply window stays open
    /// against mail that no longer exists.
    pub(super) fn prune_mailbox_messages(&mut self, now: u64) {
        let mut expired: Vec<(String, bool)> = Vec::new();
        self.mailbox_messages.retain(|entry| {
            if now.saturating_sub(entry.queued_at) > MAILBOX_MESSAGE_RETENTION_MS {
                expired.push((entry.message.id.clone(), entry.message.expects_reply == Some(true)));
                return false;
            }
            true
        });
        for (message_id, expects_reply) in expired {
            if expects_reply {
                self.ask_edges.remove(&message_id);
            }
            self.message_receipt_routes.remove(&message_id);
        }
    }

    /// `queueMailboxMessage` (`v0.10.1 broker/broker.ts:890-906`).
    ///
    /// The `while (length >= MAX)` head-eviction is pi's, including that it drops the OLDEST entry
    /// rather than refusing the new one, and that each eviction takes the same ask-edge and
    /// receipt-route cleanup an expiry does.
    pub(super) fn queue_mailbox_message(
        &mut self,
        from: SessionInfo,
        target: SessionInfo,
        message: &Message,
        broker_received_at: u64,
    ) {
        self.prune_mailbox_messages(broker_received_at);
        while self.mailbox_messages.len() >= MAX_MAILBOX_MESSAGES {
            let evicted = self.mailbox_messages.remove(0);
            if evicted.message.expects_reply == Some(true) {
                self.ask_edges.remove(&evicted.message.id);
            }
            self.message_receipt_routes.remove(&evicted.message.id);
        }
        // `{ ...message, brokerReceivedAt }` (`:903`): the parked envelope carries the time the
        // BROKER accepted it, so a later flush can date the receipt route from it (`:948`).
        let mut parked = message.clone();
        parked.broker_received_at = Some(broker_received_at.into());
        self.mailbox_messages.push(MailboxMessage {
            from,
            target,
            message: parked,
            queued_at: broker_received_at,
        });
    }

    /// `findLiveSessionsSharingMailboxIdentity` (`v0.10.1 broker/broker.ts:1039-1048`), returning
    /// ids because the caller needs `&mut self` afterwards. Upstream's own rationale, verbatim
    /// (`:1029-1037`):
    ///
    /// ```text
    /// Mailbox identity is an explicit name plus directory, never name alone. A
    /// runtime fallback alias is derived from the session id rather than chosen as
    /// a durable identity, so it must not transfer mail to another process. This
    /// also prevents two unnamed UUIDv7 sessions started close together from
    /// inheriting each other's mailbox through a shared short alias.
    /// ```
    ///
    /// Both guards are JS **truthiness** tests, not presence tests: `!lowerName` rejects `""` as
    /// well as `undefined`, and `info.runtimeFallbackAlias` is falsy when the flag is `false`.
    /// [`js_truthy_alias`] and the `is_empty` filter reproduce that, so an empty name can never
    /// become a mailbox identity every unnamed session shares.
    pub(super) fn find_live_sessions_sharing_mailbox_identity(&self, info: &SessionInfo) -> Vec<String> {
        let Some(lower_name) =
            info.name.as_deref().map(str::to_lowercase).filter(|n| !n.is_empty())
        else {
            return Vec::new();
        };
        if js_truthy_alias(info.runtime_fallback_alias) {
            return Vec::new();
        }
        self.sessions_in_order()
            .filter(|(_, s)| {
                !js_truthy_alias(s.info.runtime_fallback_alias)
                    && s.info.name.as_deref().map(str::to_lowercase).as_deref()
                        == Some(lower_name.as_str())
                    && crate::cwd::same_cwd(&s.info.cwd, &info.cwd)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// `findUniqueLiveSessionForDisconnectedSession` (`v0.10.1 broker/broker.ts:1022-1026`): the
    /// sole live session sharing the disconnected target's mailbox identity, excluding the sender
    /// (so a session that renamed itself onto a peer's identity cannot receive its own mail).
    pub(super) fn find_unique_live_session_for_disconnected_session(
        &self,
        info: &SessionInfo,
        sender_id: &str,
    ) -> Option<String> {
        let matches: Vec<String> = self
            .find_live_sessions_sharing_mailbox_identity(info)
            .into_iter()
            .filter(|id| id != sender_id)
            .collect();
        match matches.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        }
    }

    /// `findDisconnectedSessions` (`v0.10.1 broker/broker.ts:1010-1024`) — the same exact-id →
    /// exact-name → id-prefix ladder `findSessions` uses, over the disconnected map.
    pub(super) fn find_disconnected_session_ids(&mut self, name_or_id: &str, now: u64) -> Vec<String> {
        self.prune_disconnected_sessions(now);
        let entries: Vec<(String, Option<String>)> = self
            .disconnected_sessions
            .values()
            .map(|s| (s.info.id.clone(), s.info.name.clone()))
            .collect();
        find_session_ids(&entries, name_or_id)
    }

    /// `flushMailboxForSession` (`v0.10.1 broker/broker.ts:908-953`), called from `register` once
    /// the joining session is in `this.sessions`.
    ///
    /// The three-way match is upstream's: by stored target **id**; or — only when this session is
    /// the UNIQUE live holder of its mailbox identity — by target name+cwd; and never by an entry
    /// this very identity SENT (`matchesSenderIdentity`), which is what stops a relaunched sender
    /// from swallowing the mail it queued for a peer of the same name in the same directory.
    pub(super) fn flush_mailbox_for_session(&mut self, session_id: &str, now: u64) {
        self.prune_mailbox_messages(now);
        let Some(session) = self.sessions.get(session_id) else {
            return;
        };
        let info = session.info.clone();
        let tx = session.tx.clone();
        let session_name = info.name.as_deref().map(str::to_lowercase).filter(|n| !n.is_empty());
        let unique_mailbox_identity =
            self.find_live_sessions_sharing_mailbox_identity(&info).len() == 1;

        let mut index = 0;
        while index < self.mailbox_messages.len() {
            let (matches_id, matches_unique_name) = {
                let Some(entry) = self.mailbox_messages.get(index) else { break };
                let matches_id = entry.target.id == info.id;
                let matches_sender_identity = session_name.as_deref().is_some_and(|name| {
                    entry.from.name.as_deref().map(str::to_lowercase).as_deref() == Some(name)
                        && crate::cwd::same_cwd(&entry.from.cwd, &info.cwd)
                });
                let matches_unique_name = unique_mailbox_identity
                    && session_name.as_deref().is_some_and(|name| {
                        !matches_sender_identity
                            && entry.target.name.as_deref().map(str::to_lowercase).as_deref()
                                == Some(name)
                            && crate::cwd::same_cwd(&entry.target.cwd, &info.cwd)
                    });
                (matches_id, matches_unique_name)
            };
            if !matches_id && !matches_unique_name {
                index += 1;
                continue;
            }

            let entry = self.mailbox_messages.remove(index);
            // `edge.to = session.info.id` (`:936-939`): a parked ASK is re-pointed at the session
            // that actually received it, so the reply the peer eventually sends still matches its
            // edge. This is the one place an ask edge is MUTATED rather than created or dropped,
            // and it is why a disconnect must NOT clear the edges (see `on_connection_closed`).
            if let Some(edge) = self.ask_edges.get_mut(&entry.message.id)
                && edge.to == entry.target.id
            {
                edge.to.clone_from(&info.id);
            }
            let mut delivered = entry.message.clone();
            delivered.broker_delivered_at = Some(now_ms().into());
            send_msg(&tx, &BrokerMessage::Message { from: entry.from.clone(), message: delivered });
            self.message_receipt_routes.insert(entry.message.id.clone(), MessageReceiptRoute {
                from: entry.from.id.clone(),
                to: info.id.clone(),
                created_at: entry
                    .message
                    .broker_received_at
                    .as_ref()
                    .and_then(serde_json::Number::as_u64)
                    .unwrap_or(entry.queued_at),
            });
        }
    }

}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use serde_json::json;
    use super::*;
    use super::super::test_support::{make_state, payloads, register_named, send_frame};

    /// ICOM-010 — the broker mailbox (`v0.10.1 broker/broker.ts:890-953`). Before this landed, a
    /// message sent during a peer's reconnect gap was answered `Session not found` and DROPPED;
    /// `connect.rs:44-51` even documented "there is no mailbox, no queue, no redelivery" as an
    /// invariant. Now it is parked and redelivered on the peer's next `register`.
    #[test]
    fn mail_for_a_disconnected_peer_is_parked_and_flushed_on_re_register() {
        let mut state = make_state();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);

        // b leaves.
        assert!(state.on_connection_closed(2, &b_sid, 1_500));
        let _ = payloads(&mut a_rx);
        let _ = payloads(&mut b_rx);

        // a sends to b anyway.
        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "while you were out" } }),
            2_000,
        );
        let acks = payloads(&mut a_rx);
        assert_eq!(acks.len(), 1, "the sender is acked exactly once");
        assert_eq!(acks[0]["type"], "delivered", "parked mail is ACKED, not refused: {acks:?}");
        assert_eq!(state.mailbox_messages.len(), 1);

        // b comes back on a new connection with the same id.
        let (b2_tx, mut b2_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut b2_sid = None;
        register_named(&mut state, 3, &mut b2_sid, &b2_tx, "b", "bob", "/w", 3_000);
        let delivered = payloads(&mut b2_rx);
        let message = delivered
            .iter()
            .find(|p| p["type"] == "message")
            .expect("the parked message is redelivered on register");
        assert_eq!(message["message"]["id"], "m1");
        assert_eq!(message["message"]["content"]["text"], "while you were out");
        assert_eq!(message["from"]["id"], "a");
        // `registered` and `session_joined` precede it (`:383-392`).
        assert_eq!(delivered.first().map(|p| p["type"].clone()), Some(json!("registered")));
        assert!(state.mailbox_messages.is_empty(), "the entry is consumed, not copied");
        // The flush records where the message went, so a receipt can be routed home (`:945-952`).
        assert_eq!(state.message_receipt_routes.get("m1").map(|r| r.to.clone()), Some("b".into()));
    }
    /// `flushMailboxForSession`'s `matchesUniqueName` arm (`:919-931`) and the identity guard it
    /// rests on (`:1039-1048`): mail parked for a disconnected `bob` in `/w` is inherited by a
    /// RELAUNCHED `bob` in `/w` under a brand-new session id — but not by a `bob` in another
    /// directory, whose registration must leave the entry parked.
    #[test]
    fn a_relaunched_peer_inherits_mail_by_name_and_cwd_but_not_by_name_alone() {
        let mut state = make_state();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, _b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b-old", "bob", "/w", 1_000);
        state.on_connection_closed(2, &b_sid, 1_500);
        let _ = payloads(&mut a_rx);
        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "bob",
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } }),
            2_000,
        );
        assert_eq!(state.mailbox_messages.len(), 1);

        // NEGATIVE FIRST: same name, different directory — no inheritance.
        let (other_tx, mut other_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut other_sid = None;
        register_named(&mut state, 3, &mut other_sid, &other_tx, "b-elsewhere", "bob", "/elsewhere", 3_000);
        assert!(
            !payloads(&mut other_rx).iter().any(|p| p["type"] == "message"),
            "a same-named peer in another directory must not inherit the mailbox"
        );
        assert_eq!(state.mailbox_messages.len(), 1, "the entry stays parked");

        // Now the genuine relaunch: same name, same cwd, new id.
        let (b2_tx, mut b2_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut b2_sid = None;
        register_named(&mut state, 4, &mut b2_sid, &b2_tx, "b-new", "bob", "/w", 4_000);
        assert!(
            payloads(&mut b2_rx).iter().any(|p| p["type"] == "message"),
            "a relaunch with the same name in the same directory inherits its mail"
        );
        assert!(state.mailbox_messages.is_empty());
    }
    /// `findUniqueLiveSessionForDisconnectedSession` (`:640-653`): when a peer has ALREADY
    /// relaunched under a new id, mail addressed to the old id is delivered live rather than
    /// parked — and the sender is excluded from that match, so a session cannot inherit the
    /// mailbox of a peer it is itself writing to.
    #[test]
    fn mail_for_an_old_session_id_is_delivered_live_to_its_relaunched_identity() {
        let mut state = make_state();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, _b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b-old", "bob", "/w", 1_000);
        state.on_connection_closed(2, &b_sid, 1_500);
        let (b2_tx, mut b2_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut b2_sid = None;
        register_named(&mut state, 3, &mut b2_sid, &b2_tx, "b-new", "bob", "/w", 2_000);
        let _ = payloads(&mut a_rx);
        let _ = payloads(&mut b2_rx);

        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b-old",
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } }),
            3_000,
        );
        assert!(state.mailbox_messages.is_empty(), "a live identity holder takes it immediately");
        let got = payloads(&mut b2_rx);
        assert!(
            got.iter().any(|p| p["type"] == "message" && p["message"]["id"] == "m1"),
            "the relaunched session receives it: {got:?}"
        );
        assert_eq!(state.message_receipt_routes.get("m1").map(|r| r.to.clone()), Some("b-new".into()));
    }
    /// `MAX_MAILBOX_MESSAGES` head-eviction (`:892-898`): the cap drops the OLDEST parked entry, so
    /// the newest 256 survive.
    #[test]
    fn the_mailbox_cap_evicts_the_oldest_entry() {
        let mut state = make_state();
        let (a_tx, _a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, _b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);
        state.on_connection_closed(2, &b_sid, 1_500);
        for n in 0..MAX_MAILBOX_MESSAGES + 3 {
            send_frame(
                &mut state,
                1,
                &a_tx,
                &mut a_sid,
                "b",
                json!({ "id": format!("m{n}"), "timestamp": 1, "content": { "text": "x" } }),
                2_000,
            );
        }
        assert_eq!(state.mailbox_messages.len(), MAX_MAILBOX_MESSAGES);
        assert_eq!(state.mailbox_messages.first().map(|e| e.message.id.clone()), Some("m3".into()));
        assert_eq!(
            state.mailbox_messages.last().map(|e| e.message.id.clone()),
            Some(format!("m{}", MAX_MAILBOX_MESSAGES + 2))
        );
    }
    /// Parked mail expires on `MAILBOX_MESSAGE_RETENTION_MS`, and a disconnected identity on
    /// `DISCONNECTED_SESSION_RETENTION_MS` (`v0.10.1 broker/broker.ts:869-888`) — after which the
    /// same `send` is `Session not found` again rather than queueing forever.
    ///
    /// **Mailbox expiry is LAZY, and `case "send"` is not one of its call sites.**
    /// `pruneMailboxMessages` has exactly four callers upstream — `register` (`:342`),
    /// `cancel_message` (`:709`), `queueMailboxMessage` (`:891`) and `flushMailboxForSession`
    /// (`:909`) — while `case "send"` (`:484-680`) prunes only the ask edges and the receipt
    /// routes (`:501-502`). A `send` refused `Session not found` therefore leaves the stale entry
    /// sitting in `mailboxMessages`; it is the target's next `register` that drops it.
    ///
    /// So the guarantee worth pinning is not "the failing send emptied the mailbox" (pi never does
    /// that) but "an expired entry is never REDELIVERED" — which is only observable by actually
    /// re-registering the target. `park_then_rejoin` does that on both sides of pi's strict `>`
    /// boundary, so the expiry assertion cannot pass vacuously: at exactly the retention the entry
    /// must still flush, and one millisecond later it must not.
    #[test]
    fn parked_mail_and_the_disconnected_identity_both_expire_after_their_retention() {
        /// Park `m1` at t=2_000 for a `b` that dropped at t=1_500, then re-register `b` at
        /// `rejoin_at`, returning (message ids flushed to the rejoining session, entries left).
        fn park_then_rejoin(rejoin_at: u64) -> (Vec<String>, usize) {
            let mut state = make_state();
            let (a_tx, _a_rx) = tokio::sync::mpsc::unbounded_channel();
            let (b_tx, _b_rx) = tokio::sync::mpsc::unbounded_channel();
            let mut a_sid = None;
            let mut b_sid = None;
            register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
            register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);
            state.on_connection_closed(2, &b_sid, 1_500);
            send_frame(
                &mut state,
                1,
                &a_tx,
                &mut a_sid,
                "b",
                json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } }),
                2_000,
            );
            assert_eq!(state.mailbox_messages.len(), 1, "m1 is parked for the disconnected b");

            let (b2_tx, mut b2_rx) = tokio::sync::mpsc::unbounded_channel();
            let mut b2_sid = None;
            register_named(&mut state, 3, &mut b2_sid, &b2_tx, "b", "bob", "/w", rejoin_at);
            let flushed = payloads(&mut b2_rx)
                .into_iter()
                .filter(|p| p["type"] == "message")
                .map(|p| p["message"]["id"].as_str().unwrap_or_default().to_string())
                .collect();
            (flushed, state.mailbox_messages.len())
        }

        // `now - entry.queuedAt > MAILBOX_MESSAGE_RETENTION_MS` (`:880`) is STRICT: at exactly the
        // retention the entry is still live, and the rejoin flushes it.
        let (flushed, left) = park_then_rejoin(2_000 + MAILBOX_MESSAGE_RETENTION_MS);
        assert_eq!(flushed, vec!["m1".to_string()], "at exactly the retention it still flushes");
        assert_eq!(left, 0, "and the flush consumed it");

        // One millisecond past it, `register`'s prune (`:342`) drops it before the flush loop ever
        // sees it, so the rejoining session receives nothing at all.
        let (flushed, left) = park_then_rejoin(2_000 + MAILBOX_MESSAGE_RETENTION_MS + 1);
        assert!(flushed.is_empty(), "one ms later it is pruned, not redelivered: {flushed:?}");
        assert_eq!(left, 0, "and it is gone from the mailbox");

        // The disconnected identity's own retention, on the `send` path.
        let mut state = make_state();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, _b_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a_sid = None;
        let mut b_sid = None;
        register_named(&mut state, 1, &mut a_sid, &a_tx, "a", "alice", "/w", 1_000);
        register_named(&mut state, 2, &mut b_sid, &b_tx, "b", "bob", "/w", 1_000);
        state.on_connection_closed(2, &b_sid, 1_500);
        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m1", "timestamp": 1, "content": { "text": "hi" } }),
            2_000,
        );
        assert_eq!(state.mailbox_messages.len(), 1);
        let _ = payloads(&mut a_rx);

        let later = 2_000 + MAILBOX_MESSAGE_RETENTION_MS + 1;
        send_frame(
            &mut state,
            1,
            &a_tx,
            &mut a_sid,
            "b",
            json!({ "id": "m2", "timestamp": 1, "content": { "text": "hi" } }),
            later,
        );
        let reason = payloads(&mut a_rx)
            .first()
            .map(|p| p["reason"].as_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        // `findDisconnectedSessions` prunes first (`:1005`), so the retained identity is gone and
        // the ladder falls through to the empty-targets arm rather than queueing forever.
        assert_eq!(reason, "Session not found", "the retained identity expired too");
        assert_eq!(
            state.mailbox_messages.len(),
            1,
            "`case \"send\"` prunes only ask edges and receipt routes (`:501-502`), so the stale \
             entry is still parked here — the next `register` is what drops it"
        );
        assert_eq!(
            state.mailbox_messages[0].message.id, "m1",
            "and m2 was refused outright, never queued behind it"
        );
    }
}

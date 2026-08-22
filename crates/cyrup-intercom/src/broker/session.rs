//! Session lifecycle frames: `register`, `unregister`, `list`
//! (`broker.ts:336-420` and the `v0.10.1` mailbox-flush additions).
//!
//! [`BrokerState::handle_register`] is the crate's registration handshake — caps, takeover, the
//! join-order insert, the mailbox flush for a returning identity, and the `registered` +
//! `presence_update` fan-out. Split out of `broker/mod.rs` as the one concern that owns a session's
//! arrival and departure.

use tokio::sync::mpsc::UnboundedSender;

use crate::transport::protocol::{BrokerMessage, SessionInfo, SessionRegistration};

use super::extensions::extensions_field_is_valid;
use super::frame::{FrameOutcome, FrameResult, send_msg};
use super::limits::MAX_SESSIONS;
use super::state::{BrokerState, ConnectedSession};

impl BrokerState {
    pub(super) fn handle_register(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_id: &mut Option<String>,
        now: u64,
    ) -> FrameResult {
        let Some(session_val) = value.get("session") else {
            return FrameResult::protocol_error();
        };
        let Ok(registration) = serde_json::from_value::<SessionRegistration>(session_val.clone())
        else {
            return FrameResult::protocol_error();
        };
        if session_id.is_some() {
            // Duplicate register (broker.ts:342-344).
            return FrameResult::protocol_error();
        }
        // sessionId: absent → randomUUID; present must be a non-blank string (broker.ts:346-352).
        let id = match value.get("sessionId") {
            None => uuid::Uuid::new_v4().to_string(),
            Some(v) => match v.as_str() {
                Some(s) if !s.trim().is_empty() => s.to_string(),
                _ => return FrameResult::protocol_error(),
            },
        };
        // `case "register"`'s extensions guard (`v0.9.2 broker/broker.ts:446-456`), in pi's own
        // position — after the `sessionId` check, before `pruneDisconnectedSessions()`. Absent is
        // legal (`extensions !== undefined`); present-but-invalid throws, i.e. `socket.destroy`.
        // `SessionRegistration` does not model `extensions` (cyrup never advertises the bus), so the
        // raw value comes out of its `#[serde(flatten)]` capture — the same place pi's
        // `session.extensions` comes from, since pi does not model it on `SessionInfo` either
        // (`v0.9.2 broker/broker.ts:472-490` puts it on `ConnectedSession`, not on `info`).
        if let Some(extensions) = registration.extra.get("extensions")
            && !extensions_field_is_valid(extensions)
        {
            return FrameResult::protocol_error();
        }

        // `pruneDisconnectedSessions(); pruneMailboxMessages();` in pi's own position — after the
        // `extensions` guard, before the `MAX_SESSIONS` check (`v0.10.1 broker/broker.ts:340-341`).
        self.prune_disconnected_sessions(now);
        self.prune_mailbox_messages(now);

        let previous_conn = self.sessions.get(&id).map(|s| s.conn_id);
        if previous_conn.is_none() && self.sessions.len() >= MAX_SESSIONS {
            send_msg(self_tx, &BrokerMessage::Error {
                error: "Too many registered intercom sessions".to_string(),
            });
            return FrameResult::close_self();
        }
        if previous_conn.is_some() {
            // Identity takeover (`v0.10.1 broker/broker.ts:348-352`): clear the old edges AND the
            // old receipt routes, then end the previous socket. This is `clearAskEdgesForSession`'s
            // ONLY call site upstream — see `on_connection_closed`.
            self.clear_ask_edges_for_session(&id);
            self.clear_message_receipt_routes_for_session(&id);
            if let Some(prev_id) = previous_conn
                && let Some(h) = self.connections.get(&prev_id)
            {
                h.close.notify_one();
            }
        }

        *session_id = Some(id.clone());
        self.remove_unregistered(conn_id);

        let info = SessionInfo {
            id: id.clone(),
            name: registration.name,
            // `v0.10.1 broker/broker.ts:358` copies the registration's `runtimeFallbackAlias` onto
            // the stored `SessionInfo`, so every peer's roster can tell a chosen name from a
            // synthesized alias.
            runtime_fallback_alias: registration.runtime_fallback_alias,
            cwd: registration.cwd,
            model: registration.model,
            pid: registration.pid,
            started_at: registration.started_at,
            last_activity: registration.last_activity,
            status: registration.status,
            peer_uid: None,
            // `trustedLocal` — broker-owned, never from the payload (`broker.ts:374`), and a
            // property of the BOUND ENDPOINT rather than of the platform (ICOM-015): false for the
            // loopback-TCP endpoint even on unix, because a TCP peer carries no uid.
            trusted_local: Some(self.trusted_local),
            context_pct: None,
            context_tokens: None,
            context_window: None,
            extra: Default::default(),
        };
        self.insert_session(id.clone(), ConnectedSession {
            conn_id,
            info: info.clone(),
            tx: self_tx.clone(),
            last_presence_broadcast_at: now,
        });
        // `this.disconnectedSessions.delete(id)` (`v0.10.1 broker/broker.ts:377`): this identity is
        // live again, so it must no longer be a mailbox TARGET — only a mailbox recipient.
        self.disconnected_sessions.remove(&id);
        // A register cancels any pending auto-shutdown (`v0.10.1 broker/broker.ts:378-381`):
        //   if (this.shutdownTimer) { clearTimeout(this.shutdownTimer); this.shutdownTimer = null; }
        // NULLING THE HANDLE is the load-bearing half — it is what lets a LATER disconnect arm a
        // fresh check. The generation bump alone only makes the pending check stale; it is kept as
        // belt-and-braces for a check already past its sleep.
        self.shutdown_gen = self.shutdown_gen.wrapping_add(1);
        self.shutdown_scheduled = false;
        if let Some(task) = self.shutdown_task.take() {
            task.abort();
        }

        send_msg(self_tx, &BrokerMessage::Registered { session_id: id.clone(), features: None });
        self.broadcast(&BrokerMessage::SessionJoined { session: info }, Some(&id));
        // `this.flushMailboxForSession(connectedSession)` (`v0.10.1 broker/broker.ts:392`), in pi's
        // own position: AFTER `registered` and `session_joined`, so the client has already
        // transitioned to connected and installed its message handler before its parked mail
        // arrives on the same socket, in order.
        self.flush_mailbox_for_session(&id, now);
        FrameResult::cont()
    }

    pub(super) fn handle_unregister(
        &mut self,
        conn_id: u64,
        _self_tx: &UnboundedSender<Vec<u8>>,
        session_id: &mut Option<String>,
        now: u64,
    ) -> FrameResult {
        let Some(sid) = session_id.clone() else {
            return FrameResult::protocol_error();
        };
        let mut schedule = false;
        if self.sessions.get(&sid).map(|s| s.conn_id) == Some(conn_id) {
            // `case "unregister"` (`v0.10.1 broker/broker.ts:418-432`) is the socket-close body
            // verbatim: remember the departing identity for its mailbox, drop its receipt routes,
            // and — as at `on_connection_closed` — do NOT clear its ask edges.
            if let Some(existing) = self.sessions.get(&sid) {
                let info = existing.info.clone();
                self.remember_disconnected_session(info, now);
            }
            self.remove_session(&sid);
            self.clear_message_receipt_routes_for_session(&sid);
            self.broadcast(&BrokerMessage::SessionLeft { session_id: sid.clone() }, Some(&sid));
            schedule = true;
        }
        *session_id = None;
        // Re-arm the registration timeout for the now-unregistered-but-open socket (broker.ts:228):
        // the reader re-arms its 1 s deadline; track the connection as unregistered again and run
        // the same oldest-eviction pass pi's `armRegistrationTimeout` runs on this transition
        // (`broker.ts:189-195,223-230,399`).
        self.mark_unregistered(conn_id);
        FrameResult { outcome: FrameOutcome::Continue, schedule_shutdown: schedule, rearmed_registration: true }
    }

    pub(super) fn handle_list(
        &mut self,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
    ) -> FrameResult {
        let Some(request_id) = value.get("requestId").and_then(|v| v.as_str()) else {
            return FrameResult::protocol_error();
        };
        send_msg(self_tx, &BrokerMessage::Sessions {
            request_id: request_id.to_string(),
            sessions: self.session_infos(),
        });
        FrameResult::cont()
    }

}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::super::limits::MAX_UNREGISTERED_CONNECTIONS;
    use std::sync::Arc;
    use super::super::frame::FrameOutcome;
    use super::super::test_support::{make_state, make_tx, register};
    use tokio::sync::Notify;

    /// Regression test: pi's `armRegistrationTimeout` re-runs `evictOldestUnregisteredConnections`
    /// on **every** transition into the unregistered state — both a brand-new connection
    /// (`broker.ts:210`) and an explicit `unregister` (`setId(null)` → `armRegistrationTimeout`,
    /// `broker.ts:223-230,399`). Before the fix, `handle_unregister` pushed the connection id onto
    /// `self.unregistered` with no eviction call, so churn of register/unregister on already-live
    /// connections could grow the unregistered set past `MAX_UNREGISTERED_CONNECTIONS` and it would
    /// stay oversized until a brand-new connection happened to arrive and trigger `add_connection`'s
    /// eviction. This test fails against that pre-fix behavior (final len would be 42, not 32).
    #[test]
    fn handle_unregister_evicts_oldest_unregistered_past_cap() {
        let mut state = make_state();

        // Fill the unregistered set to exactly the cap.
        for conn_id in 0..MAX_UNREGISTERED_CONNECTIONS as u64 {
            state.add_connection(conn_id, Arc::new(Notify::new()));
        }
        assert_eq!(state.unregistered.len(), MAX_UNREGISTERED_CONNECTIONS);

        // Register 10 of those connections, removing them from the unregistered set.
        let mut session_ids: Vec<Option<String>> = Vec::new();
        for conn_id in 0..10u64 {
            let mut sid = None;
            register(&mut state, conn_id, &mut sid, &format!("session-{conn_id}"));
            session_ids.push(sid);
        }
        assert_eq!(state.unregistered.len(), MAX_UNREGISTERED_CONNECTIONS - 10);

        // 10 brand-new connections arrive, filling the unregistered set back up to the cap (no
        // eviction needed yet: 22 + 10 == 32).
        for conn_id in 100..110u64 {
            state.add_connection(conn_id, Arc::new(Notify::new()));
        }
        assert_eq!(state.unregistered.len(), MAX_UNREGISTERED_CONNECTIONS);

        // Now the 10 registered sessions unregister. Each must re-arm + evict, exactly like pi's
        // `armRegistrationTimeout`; the unregistered set must never exceed the cap.
        for (conn_id, sid) in session_ids.iter_mut().enumerate() {
            let tx = make_tx();
            let result = state.handle_unregister(conn_id as u64, &tx, sid, 0);
            assert!(matches!(result.outcome, FrameOutcome::Continue));
            assert!(
                state.unregistered.len() <= MAX_UNREGISTERED_CONNECTIONS,
                "unregistered set exceeded the cap after unregister #{conn_id}: {}",
                state.unregistered.len()
            );
        }
        assert_eq!(state.unregistered.len(), MAX_UNREGISTERED_CONNECTIONS);
    }
}

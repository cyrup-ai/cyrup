//! The broker's in-memory routing state and the primitives that maintain it
//! (`broker.ts:32-36,132-139`).
//!
//! [`BrokerState`] is upstream's `IntercomBroker` field block; the methods here are the shared
//! bookkeeping the frame handlers build on — connection tracking, the join-ordered session map, the
//! broadcast fan-out, and the ask-edge/receipt-route pruning. The frame handlers themselves live in
//! the sibling modules (`dispatch`, `session`, `send`, `receipts`, `presence`, `extensions`) and the
//! offline-delivery machinery in `mailbox`; all of them are `impl BrokerState` blocks over this
//! type, which is why its fields are `pub(super)` rather than private.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;

use crate::transport::framing::encode_json;
use crate::transport::protocol::{BrokerMessage, SessionInfo};

use super::extension_state::ExtensionStateManager;
use super::extensions::NamespaceOwner;
use super::limits::{MAX_UNREGISTERED_CONNECTIONS, MESSAGE_RECEIPT_ROUTE_RETENTION_MS};
use super::mailbox::{DisconnectedSession, MailboxMessage};
use super::receipts::MessageReceiptRoute;
use super::routing::AskEdge;

/// One registered session + a handle to write to its socket (`broker.ts:32-36`).
pub(super) struct ConnectedSession {
    pub(super) conn_id: u64,
    pub(super) info: SessionInfo,
    pub(super) tx: UnboundedSender<Vec<u8>>,
    pub(super) last_presence_broadcast_at: u64,
    /// `ownerOrder` (`v0.9.2 broker/broker.ts:56`) — the broker-owned registration order the
    /// namespace-owner election sorts on, assigned from [`BrokerState::next_owner_order`] and
    /// PRESERVED across an identity takeover (`:488`), so a client cannot seize a namespace by
    /// reconnecting or by backdating its advertised `startedAt`.
    pub(super) owner_order: u64,
    /// `extensions` (`v0.9.2 broker/broker.ts:57`), as advertised on `register` or by a later
    /// `extension_capabilities_update`.
    ///
    /// Upstream's is `ExtensionCapability[] | undefined`; an EMPTY vec is the faithful stand-in for
    /// `undefined` because every reader is either `!session.extensions?.length` (`:1277`) or
    /// `session.extensions ?? []` (`:1188`) — no branch upstream can tell the two apart.
    pub(super) extensions: Vec<crate::transport::protocol::ExtensionCapability>,
}

/// A live connection's close handle, tracked so any handler can destroy it (takeover, eviction,
/// global shutdown) regardless of which task owns its read loop. The per-session writer is on
/// [`ConnectedSession`]; an unregistered connection is tracked in [`BrokerState::unregistered`].
pub(super) struct ConnHandle {
    pub(super) close: Arc<Notify>,
}

/// The broker's in-memory routing state (`broker.ts:132-139`). Held behind a `std::sync::Mutex`;
/// every handler is synchronous and never holds the guard across an `.await`.
pub(super) struct BrokerState {
    pub(super) sessions: HashMap<String, ConnectedSession>,
    /// Registered session ids in **join order**. `broker.ts:133` holds the sessions in a JS `Map`,
    /// which iterates in insertion order, so every consumer of the map — the `list` reply
    /// (`broker.ts:408`), presence broadcasts and name resolution — observes a stable join order.
    /// A `std::collections::HashMap` has no such guarantee, so the order is tracked alongside it,
    /// the way `unregistered` already tracks connection insertion order below.
    pub(super) session_order: Vec<String>,
    pub(super) ask_edges: HashMap<String, AskEdge>,
    /// `messageReceiptRoutes` (`v0.10.1 broker/broker.ts:100`), keyed by message id.
    pub(super) message_receipt_routes: HashMap<String, MessageReceiptRoute>,
    /// `disconnectedSessions` (`v0.10.1 broker/broker.ts:101`), keyed by session id.
    ///
    /// pi holds this in a JS `Map`, whose iteration order is insertion order; a `HashMap`'s is
    /// arbitrary. That is immaterial HERE, unlike [`BrokerState::session_order`]: the only consumer
    /// is `findDisconnectedSessions` (`:1010-1024`) and every one of ITS consumers is gated on
    /// `length === 1` or `length > 1` (`:596`, `:660`), so no branch can observe which element
    /// came first. Same argument as `resolve_reply_target`'s (ICOM-001).
    pub(super) disconnected_sessions: HashMap<String, DisconnectedSession>,
    /// `mailboxMessages` (`v0.10.1 broker/broker.ts:102`) — an ARRAY upstream, and the order is
    /// load-bearing: [`BrokerState::queue_mailbox_message`] evicts from the FRONT at the cap
    /// (`:892-898`, FIFO) and [`BrokerState::flush_mailbox_for_session`] redelivers front-to-back
    /// (`:913`), so a peer receives its parked mail in the order it was sent.
    pub(super) mailbox_messages: Vec<MailboxMessage>,
    pub(super) connections: HashMap<u64, ConnHandle>,
    /// Unregistered connection ids in insertion order (for oldest-eviction, `broker.ts:256-268`).
    pub(super) unregistered: Vec<u64>,
    pub(super) ask_timeout_ms: u64,
    /// Bumped on every `register` so a pending auto-shutdown check becomes stale (`broker.ts:378-381`).
    pub(super) shutdown_gen: u64,
    pub(super) shutdown_scheduled: bool,
    /// The pending auto-shutdown task, i.e. pi's `shutdownTimer` HANDLE
    /// (`v0.10.1 broker/broker.ts:106`). Holding it is what makes `register`'s
    /// `clearTimeout(this.shutdownTimer); this.shutdownTimer = null` (`:378-381`) portable: without
    /// it, a register inside the 5 s window left `shutdown_scheduled` set, so the next disconnect's
    /// `schedule_shutdown_check` early-returned and the re-arm was LOST — the broker then idled
    /// forever with zero sessions until an unrelated connect/disconnect cycle re-armed it.
    pub(super) shutdown_task: Option<tokio::task::JoinHandle<()>>,
    /// Global shutdown signal awaited by [`super::run`].
    pub(super) shutdown: Arc<Notify>,
    /// `trustedLocal = typeof LISTEN_TARGET === "string" && process.platform !== "win32"`
    /// (`broker.ts:365`), stamped onto every registered [`SessionInfo`] (`:374`).
    ///
    /// ICOM-015 — a PROPERTY OF THE BOUND ENDPOINT, not of the platform: this was `cfg!(unix)`,
    /// which is upstream's answer for a unix socket but the wrong one for the loopback-TCP endpoint,
    /// where pi's `typeof LISTEN_TARGET === "string"` is false on every platform. A TCP peer carries
    /// no uid the broker can read, so claiming `trustedLocal` for it would let a remote-shaped
    /// connection inherit a local peer's trust. Supplied by
    /// [`super::listener::BrokerListener::is_trusted_local`].
    pub(super) trusted_local: bool,
    /// `BROKER_STATE_ID` (`broker.ts:29`, `randomUUID()` once per broker process) when — and only
    /// when — the bound endpoint demands it: `requiresEndpointAuth = typeof LISTEN_TARGET !==
    /// "string"` (`:284`). `None` collapses upstream's two constants into "no gate", which is
    /// exactly what a socket/pipe endpoint gets.
    pub(super) endpoint_state_id: Option<String>,
    /// `namespaceOwners` (`v0.9.2 broker/broker.ts:225`), keyed by namespace.
    ///
    /// [CYRUP-DELTA] A `BTreeMap` where pi has an insertion-ordered `Map`, so
    /// `recompute_namespace_owners` walks namespaces in lexicographic rather than first-seen order.
    /// The only thing that order can reach is the relative order of `extension_owner` frames for two
    /// DIFFERENT namespaces on one socket; every consumer of that frame is per-namespace and
    /// idempotent (`v0.9.2 broker/client.ts:538-552`), so no peer can observe the difference — and a
    /// `HashMap` here WOULD be observable as nondeterminism across runs, which is the failure
    /// `session_order` exists to prevent.
    pub(super) namespace_owners: BTreeMap<String, NamespaceOwner>,
    /// `nextOwnerOrder = 1` (`v0.9.2 broker/broker.ts:226`).
    pub(super) next_owner_order: u64,
    /// `extensionStateManager` (`v0.9.2 broker/broker.ts:227,232`).
    pub(super) extension_state: ExtensionStateManager,
}

pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl BrokerState {
    pub(super) fn new(
        ask_timeout_ms: u64,
        shutdown: Arc<Notify>,
        extension_state_dir: PathBuf,
    ) -> Self {
        Self {
            sessions: HashMap::new(),
            session_order: Vec::new(),
            namespace_owners: BTreeMap::new(),
            next_owner_order: 1,
            extension_state: ExtensionStateManager::new(extension_state_dir),
            ask_edges: HashMap::new(),
            message_receipt_routes: HashMap::new(),
            disconnected_sessions: HashMap::new(),
            mailbox_messages: Vec::new(),
            connections: HashMap::new(),
            unregistered: Vec::new(),
            ask_timeout_ms,
            shutdown_gen: 0,
            shutdown_scheduled: false,
            shutdown_task: None,
            shutdown,
            // The socket/pipe answer, which is what every caller but [`run`] binds.
            trusted_local: cfg!(unix),
            endpoint_state_id: None,
        }
    }

    /// Adopt the properties of the endpoint actually bound (ICOM-015): whether a peer on it is
    /// `trustedLocal` (`broker.ts:365`) and, for the TCP endpoint only, the per-run credential
    /// clients must echo (`BROKER_STATE_ID`, `:29`/`:284-305`).
    ///
    /// Separate from [`Self::new`] rather than folded into it because upstream's two facts come
    /// from the LISTEN TARGET, which only the real [`super::run`] has; every unit test drives a state whose
    /// endpoint is the default socket, and pi's own default is that same string arm.
    pub(super) fn with_listen_endpoint(
        mut self,
        trusted_local: bool,
        endpoint_state_id: Option<String>,
    ) -> Self {
        self.trusted_local = trusted_local;
        self.endpoint_state_id = endpoint_state_id;
        self
    }

    /// Register a fresh connection and evict the oldest unregistered ones past the cap
    /// (`armRegistrationTimeout` → `evictOldestUnregisteredConnections`, `broker.ts:189-268`).
    pub(super) fn add_connection(&mut self, conn_id: u64, close: Arc<Notify>) {
        self.connections.insert(conn_id, ConnHandle { close });
        self.mark_unregistered(conn_id);
    }

    /// Insert (or move to newest) `conn_id` into the unregistered set and evict the oldest
    /// unregistered connections past the cap. Mirrors pi's `armRegistrationTimeout` — which does
    /// `this.unregisteredConnections.delete(socket); .add(socket); this.evictOldestUnregisteredConnections(socket)`
    /// (`broker.ts:193-195`) — and which pi runs on **every** transition into the unregistered
    /// state: both a fresh connection (`broker.ts:210`) and an explicit `unregister`
    /// (`setId(null)` → `armRegistrationTimeout`, `broker.ts:223-230,399`).
    pub(super) fn mark_unregistered(&mut self, conn_id: u64) {
        self.unregistered.retain(|&c| c != conn_id);
        self.unregistered.push(conn_id);
        while self.unregistered.len() > MAX_UNREGISTERED_CONNECTIONS {
            // Oldest is at the front; never evict the just-added current if it is the only one.
            let Some(&oldest) = self.unregistered.first() else {
                break;
            };
            if oldest == conn_id && self.unregistered.len() == 1 {
                break;
            }
            self.unregistered.remove(0);
            if let Some(h) = self.connections.remove(&oldest) {
                h.close.notify_one();
            }
        }
    }

    pub(super) fn remove_unregistered(&mut self, conn_id: u64) {
        self.unregistered.retain(|&c| c != conn_id);
    }

    /// The registered sessions in join order — the Rust equivalent of iterating pi's
    /// `this.sessions` JS `Map` (`broker.ts:133`).
    pub(super) fn sessions_in_order(&self) -> impl Iterator<Item = (&String, &ConnectedSession)> {
        self.session_order
            .iter()
            .filter_map(|id| self.sessions.get_key_value(id))
    }

    /// `this.sessions.set(id, …)` (`broker.ts:376`). JS `Map.set` on an **existing** key keeps
    /// that key's original position, so an identity takeover must not move the session to the back
    /// of the join order.
    pub(super) fn insert_session(&mut self, id: String, session: ConnectedSession) {
        if self.sessions.insert(id.clone(), session).is_none() {
            self.session_order.push(id);
        }
    }

    /// `this.sessions.delete(id)` (`broker.ts:243,394`).
    pub(super) fn remove_session(&mut self, id: &str) {
        if self.sessions.remove(id).is_some() {
            self.session_order.retain(|s| s != id);
        }
    }

    pub(super) fn broadcast(&self, msg: &BrokerMessage, exclude: Option<&str>) {
        let frame = match encode_json(msg) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "failed to encode broadcast");
                return;
            }
        };
        for (id, session) in self.sessions_in_order() {
            if Some(id.as_str()) != exclude {
                let _ = session.tx.send(frame.clone());
            }
        }
    }

    pub(super) fn clear_ask_edges_for_session(&mut self, session_id: &str) {
        self.ask_edges
            .retain(|_, edge| edge.from != session_id && edge.to != session_id);
    }

    pub(super) fn prune_ask_edges(&mut self, now: u64) {
        let timeout = self.ask_timeout_ms;
        self.ask_edges
            .retain(|_, edge| now.saturating_sub(edge.created_at) <= timeout);
    }

    /// `clearMessageReceiptRoutesForSession` (`v0.10.1 broker/broker.ts:979-985`).
    pub(super) fn clear_message_receipt_routes_for_session(&mut self, session_id: &str) {
        self.message_receipt_routes
            .retain(|_, route| route.from != session_id && route.to != session_id);
    }

    /// `pruneMessageReceiptRoutes` (`v0.10.1 broker/broker.ts:971-977`).
    pub(super) fn prune_message_receipt_routes(&mut self, now: u64) {
        self.message_receipt_routes.retain(|_, route| {
            now.saturating_sub(route.created_at) <= MESSAGE_RECEIPT_ROUTE_RETENTION_MS
        });
    }

    /// `Array.from(this.sessions.values()).map(s => s.info)` (`broker.ts:408`) — join-ordered,
    /// because pi's `Map` iterates in insertion order and neither `index.ts`'s `list` handler nor
    /// `ui/session-list.ts` re-sorts the reply.
    pub(super) fn session_infos(&self) -> Vec<SessionInfo> {
        self.sessions_in_order()
            .map(|(_, s)| s.info.clone())
            .collect()
    }

    /// Socket-close handler (`v0.10.1 broker/broker.ts:210-224`). Returns `true` if this owned
    /// session actually left (so the caller schedules the auto-shutdown check). Guarded by
    /// `conn_id` equality so a superseded socket cannot delete the replacement
    /// (pi `existing?.socket === socket`).
    ///
    /// **The departing session's ask edges are deliberately NOT cleared**, and that is upstream's
    /// mechanism, not an omission: `clearAskEdgesForSession` has exactly one call site in
    /// `broker.ts` — the register-time identity takeover at `:350` — at every tag from v0.9.2 to
    /// v0.10.1. An edge toward a departed session is what `flushMailboxForSession` re-points at
    /// `:936-939` when that identity comes back, so clearing it here would make every parked ask
    /// undeliverable-as-a-reply ("Reply target does not match a pending ask") the moment the peer
    /// reconnected. The edges instead expire on `pruneAskEdges`' `askTimeoutMs`, exactly as they do
    /// for a live-but-silent peer.
    pub(super) fn on_connection_closed(
        &mut self,
        conn_id: u64,
        session_id: &Option<String>,
        now: u64,
    ) -> bool {
        self.connections.remove(&conn_id);
        self.remove_unregistered(conn_id);
        if let Some(sid) = session_id
            && self.sessions.get(sid).map(|s| s.conn_id) == Some(conn_id)
        {
            if let Some(existing) = self.sessions.get(sid) {
                let info = existing.info.clone();
                self.remember_disconnected_session(info, now);
            }
            self.remove_session(sid);
            self.clear_message_receipt_routes_for_session(sid);
            self.broadcast(
                &BrokerMessage::SessionLeft {
                    session_id: sid.clone(),
                },
                Some(sid),
            );
            // `this.recomputeNamespaceOwners()` (`v0.9.2 broker/broker.ts:337`) — this is what
            // re-elects a namespace whose owner just died.
            self.recompute_namespace_owners();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::super::test_support::{make_state, make_tx, register};

    /// Regression test for "the broker session list is backed by a `HashMap`, so `intercom list`
    /// returns sessions in an arbitrary order". `broker.ts:133` holds the sessions in a JS `Map`
    /// and `broker.ts:408` answers `list` with `Array.from(this.sessions.values()).map(s => s.info)`
    /// — a `Map` iterates in **insertion order**, and neither `index.ts`'s `list` handler nor
    /// `ui/session-list.ts` re-sorts the reply, so pi's session list is deterministically ordered
    /// by join time. Before the fix `session_infos()` iterated `self.sessions.values()` directly;
    /// `std::collections::HashMap`'s iteration order is arbitrary (and randomly seeded per
    /// process), so with 16 sessions this assertion would hold by luck at a rate of 1/16!.
    #[test]
    fn session_infos_are_returned_in_join_order() {
        let mut state = make_state();

        let joined: Vec<String> = (0..16u64).map(|i| format!("session-{i}")).collect();
        for (conn_id, id) in joined.iter().enumerate() {
            let mut sid = None;
            register(&mut state, conn_id as u64, &mut sid, id);
        }
        let listed: Vec<String> = state.session_infos().into_iter().map(|s| s.id).collect();
        assert_eq!(
            listed, joined,
            "`list` must report sessions in join order, like pi's Map"
        );

        // An identity takeover is `this.sessions.set(id, …)` on an EXISTING key, which in JS keeps
        // that key's original position (`broker.ts:376`) — it must not jump to the back.
        let mut sid = None;
        register(&mut state, 900, &mut sid, "session-3");
        let after_takeover: Vec<String> = state.session_infos().into_iter().map(|s| s.id).collect();
        assert_eq!(
            after_takeover, joined,
            "a re-register must keep the session's original position"
        );

        // `this.sessions.delete(id)` drops it from the order and leaves the rest intact.
        let mut sid = Some("session-7".to_string());
        state.handle_unregister(7, &make_tx(), &mut sid, 0);
        let expected: Vec<String> = joined
            .iter()
            .filter(|id| *id != "session-7")
            .cloned()
            .collect();
        let after_leave: Vec<String> = state.session_infos().into_iter().map(|s| s.id).collect();
        assert_eq!(
            after_leave, expected,
            "a departure must not disturb the surviving join order"
        );
    }
}

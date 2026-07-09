//! The standalone broker **process** — a 1:1 port of `pi-intercom/broker/broker.ts`.
//!
//! Dispatched as the hidden `cyrup __intercom-broker` subcommand (re-exec of `current_exe()`,
//! mirroring `cyrup-ext-subagents`' `__subagent-runner`). It binds a `tokio::net::UnixListener` at
//! `<intercomDir>/broker.sock`, speaks length-prefixed JSON ([`crate::transport::framing`]), routes
//! `send` frames child→broker→target by session identity, enforces the registration handshake +
//! caps + per-connection token bucket, tracks ask edges (mutual-ask refusal + prune), coalesces
//! presence, answers the health probe byte-identically, and auto-shuts-down 5 s after its last
//! client leaves (`broker.ts:286-296`).
//!
//! First cyrup milestone: **Unix domain socket only**. The Windows named-pipe / opt-in TCP-loopback
//! transports (`broker.ts:143-180,307-330`, TCP `stateId` auth) are deferred behind the same env
//! gates (the port doc §10-Q2); on a Unix socket `requiresEndpointAuth` is always `false`.

pub mod ratelimit;
pub mod routing;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Notify;
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::config;
use crate::paths;
use crate::transport::framing::{FrameReader, encode_json};
use crate::transport::protocol::{
    BrokerMessage, HealthMessage, Message, PROTOCOL_NAME, PROTOCOL_VERSION, SessionInfo,
    SessionRegistration, now_ms,
};
use ratelimit::TokenBucket;
use routing::{AskEdge, find_session_ids};

/// `MAX_SESSIONS = 128` (`broker.ts:25`).
const MAX_SESSIONS: usize = 128;
/// `MAX_UNREGISTERED_CONNECTIONS = 32` (`broker.ts:26`).
const MAX_UNREGISTERED_CONNECTIONS: usize = 32;
/// `REGISTRATION_TIMEOUT_MS = 1000` (`broker.ts:27`).
const REGISTRATION_TIMEOUT_MS: u64 = 1000;
/// `PRESENCE_HEARTBEAT_MS = 1000` (`broker.ts:30`).
const PRESENCE_HEARTBEAT_MS: u64 = 1000;
/// Auto-shutdown delay after the last session leaves (`broker.ts:295`, 5000ms).
const SHUTDOWN_DELAY_MS: u64 = 5000;
/// Reader read-buffer size (implementation detail; framing reassembles across chunk boundaries).
const READ_BUF: usize = 16 * 1024;

/// One registered session + a handle to write to its socket (`broker.ts:32-36`).
struct ConnectedSession {
    conn_id: u64,
    info: SessionInfo,
    tx: UnboundedSender<Vec<u8>>,
    last_presence_broadcast_at: u64,
}

/// A live connection's close handle, tracked so any handler can destroy it (takeover, eviction,
/// global shutdown) regardless of which task owns its read loop. The per-session writer is on
/// [`ConnectedSession`]; an unregistered connection is tracked in [`BrokerState::unregistered`].
struct ConnHandle {
    close: Arc<Notify>,
}

/// The broker's in-memory routing state (`broker.ts:132-139`). Held behind a `std::sync::Mutex`;
/// every handler is synchronous and never holds the guard across an `.await`.
struct BrokerState {
    sessions: HashMap<String, ConnectedSession>,
    ask_edges: HashMap<String, AskEdge>,
    connections: HashMap<u64, ConnHandle>,
    /// Unregistered connection ids in insertion order (for oldest-eviction, `broker.ts:256-268`).
    unregistered: Vec<u64>,
    ask_timeout_ms: u64,
    /// Bumped on every `register` so a pending auto-shutdown check becomes stale (`broker.ts:378-381`).
    shutdown_gen: u64,
    shutdown_scheduled: bool,
    /// Global shutdown signal awaited by [`run`].
    shutdown: Arc<Notify>,
}

/// What the reader task should do after one frame.
enum FrameOutcome {
    /// Keep reading.
    Continue,
    /// Reply already queued; destroy this connection (cap/rate-limit, `broker.ts:220,355`).
    CloseSelf,
    /// A malformed/illegal frame — destroy this connection (pi `throw` → `socket.destroy`).
    ProtocolError,
}

/// The result of handling one frame: what to do next + whether a session left (so the reader can
/// schedule the auto-shutdown check outside the state lock).
struct FrameResult {
    outcome: FrameOutcome,
    schedule_shutdown: bool,
    /// Whether this frame transitioned the connection back to unregistered (re-arm reg timeout).
    rearmed_registration: bool,
}

impl FrameResult {
    fn cont() -> Self {
        Self { outcome: FrameOutcome::Continue, schedule_shutdown: false, rearmed_registration: false }
    }
    fn close_self() -> Self {
        Self { outcome: FrameOutcome::CloseSelf, schedule_shutdown: false, rearmed_registration: false }
    }
    fn protocol_error() -> Self {
        Self { outcome: FrameOutcome::ProtocolError, schedule_shutdown: false, rearmed_registration: false }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Encode `msg` and queue it on `tx` (best-effort; a full/closed channel drops, as pi's
/// `socket.write` is fire-and-forget).
fn send_msg(tx: &UnboundedSender<Vec<u8>>, msg: &BrokerMessage) {
    match encode_json(msg) {
        Ok(frame) => {
            let _ = tx.send(frame);
        }
        Err(e) => tracing::warn!(error = %e, "failed to encode broker message"),
    }
}

impl BrokerState {
    fn new(ask_timeout_ms: u64, shutdown: Arc<Notify>) -> Self {
        Self {
            sessions: HashMap::new(),
            ask_edges: HashMap::new(),
            connections: HashMap::new(),
            unregistered: Vec::new(),
            ask_timeout_ms,
            shutdown_gen: 0,
            shutdown_scheduled: false,
            shutdown,
        }
    }

    /// Register a fresh connection and evict the oldest unregistered ones past the cap
    /// (`armRegistrationTimeout` → `evictOldestUnregisteredConnections`, `broker.ts:189-268`).
    fn add_connection(&mut self, conn_id: u64, close: Arc<Notify>) {
        self.connections.insert(conn_id, ConnHandle { close });
        self.mark_unregistered(conn_id);
    }

    /// Insert (or move to newest) `conn_id` into the unregistered set and evict the oldest
    /// unregistered connections past the cap. Mirrors pi's `armRegistrationTimeout` — which does
    /// `this.unregisteredConnections.delete(socket); .add(socket); this.evictOldestUnregisteredConnections(socket)`
    /// (`broker.ts:193-195`) — and which pi runs on **every** transition into the unregistered
    /// state: both a fresh connection (`broker.ts:210`) and an explicit `unregister`
    /// (`setId(null)` → `armRegistrationTimeout`, `broker.ts:223-230,399`).
    fn mark_unregistered(&mut self, conn_id: u64) {
        self.unregistered.retain(|&c| c != conn_id);
        self.unregistered.push(conn_id);
        while self.unregistered.len() > MAX_UNREGISTERED_CONNECTIONS {
            // Oldest is at the front; never evict the just-added current if it is the only one.
            let Some(&oldest) = self.unregistered.first() else { break };
            if oldest == conn_id && self.unregistered.len() == 1 {
                break;
            }
            self.unregistered.remove(0);
            if let Some(h) = self.connections.remove(&oldest) {
                h.close.notify_one();
            }
        }
    }

    fn remove_unregistered(&mut self, conn_id: u64) {
        self.unregistered.retain(|&c| c != conn_id);
    }

    fn broadcast(&self, msg: &BrokerMessage, exclude: Option<&str>) {
        let frame = match encode_json(msg) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "failed to encode broadcast");
                return;
            }
        };
        for (id, session) in &self.sessions {
            if Some(id.as_str()) != exclude {
                let _ = session.tx.send(frame.clone());
            }
        }
    }

    fn clear_ask_edges_for_session(&mut self, session_id: &str) {
        self.ask_edges.retain(|_, edge| edge.from != session_id && edge.to != session_id);
    }

    fn prune_ask_edges(&mut self, now: u64) {
        let timeout = self.ask_timeout_ms;
        self.ask_edges.retain(|_, edge| now.saturating_sub(edge.created_at) <= timeout);
    }

    fn session_infos(&self) -> Vec<SessionInfo> {
        self.sessions.values().map(|s| s.info.clone()).collect()
    }

    /// Socket-close handler (`broker.ts:237-249`). Returns `true` if this owned session actually left
    /// (so the caller schedules the auto-shutdown check). Guarded by `conn_id` equality so a
    /// superseded socket cannot delete the replacement (pi `existing?.socket === socket`).
    fn on_connection_closed(&mut self, conn_id: u64, session_id: &Option<String>) -> bool {
        self.connections.remove(&conn_id);
        self.remove_unregistered(conn_id);
        if let Some(sid) = session_id
            && self.sessions.get(sid).map(|s| s.conn_id) == Some(conn_id)
        {
            self.sessions.remove(sid);
            self.clear_ask_edges_for_session(sid);
            self.broadcast(&BrokerMessage::SessionLeft { session_id: sid.clone() }, Some(sid));
            return true;
        }
        false
    }

    /// Handle one already-JSON-parsed frame (`handleMessage`, `broker.ts:298-563`). `session_id` is
    /// this connection's current id (mutated on register/unregister). `self_tx` writes to this
    /// connection's own socket.
    fn handle_frame(
        &mut self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_id: &mut Option<String>,
        now: u64,
    ) -> FrameResult {
        let Some(ty) = value.get("type").and_then(|v| v.as_str()) else {
            return FrameResult::protocol_error();
        };

        // health — legal before register, no TCP endpoint auth on a Unix socket (broker.ts:312-326).
        // HealthOk is not part of the BrokerMessage union, so it is encoded directly (framing only).
        if ty == "health" {
            let Some(rid) = value.get("requestId").and_then(|v| v.as_str()) else {
                return FrameResult::protocol_error();
            };
            if let Ok(frame) = encode_json(&HealthMessage::HealthOk {
                request_id: rid.to_string(),
                protocol: PROTOCOL_NAME.to_string(),
                version: PROTOCOL_VERSION,
            }) {
                let _ = self_tx.send(frame);
            }
            return FrameResult::cont();
        }

        // Only health/register are legal before registration (broker.ts:332-334).
        if session_id.is_none() && ty != "register" {
            return FrameResult::protocol_error();
        }

        match ty {
            "register" => self.handle_register(conn_id, self_tx, value, session_id, now),
            "unregister" => self.handle_unregister(conn_id, self_tx, session_id),
            "list" => self.handle_list(self_tx, value),
            "send" => self.handle_send(conn_id, self_tx, value, session_id, now),
            "cancel_ask" => self.handle_cancel_ask(conn_id, value, session_id),
            "presence" => self.handle_presence(conn_id, value, session_id, now),
            _ => FrameResult::protocol_error(),
        }
    }

    fn handle_register(
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

        let previous_conn = self.sessions.get(&id).map(|s| s.conn_id);
        if previous_conn.is_none() && self.sessions.len() >= MAX_SESSIONS {
            send_msg(self_tx, &BrokerMessage::Error {
                error: "Too many registered intercom sessions".to_string(),
            });
            return FrameResult::close_self();
        }
        if previous_conn.is_some() {
            // Identity takeover (broker.ts:359-362): clear the old edges + end the previous socket.
            self.clear_ask_edges_for_session(&id);
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
            cwd: registration.cwd,
            model: registration.model,
            pid: registration.pid,
            started_at: registration.started_at,
            last_activity: registration.last_activity,
            status: registration.status,
            peer_uid: None,
            // trustedLocal = unix && !win — broker-owned, never from the payload (broker.ts:374).
            trusted_local: Some(cfg!(unix)),
        };
        self.sessions.insert(id.clone(), ConnectedSession {
            conn_id,
            info: info.clone(),
            tx: self_tx.clone(),
            last_presence_broadcast_at: now,
        });
        // A register cancels any pending auto-shutdown (broker.ts:378-381).
        self.shutdown_gen = self.shutdown_gen.wrapping_add(1);

        send_msg(self_tx, &BrokerMessage::Registered { session_id: id.clone() });
        self.broadcast(&BrokerMessage::SessionJoined { session: info }, Some(&id));
        FrameResult::cont()
    }

    fn handle_unregister(
        &mut self,
        conn_id: u64,
        _self_tx: &UnboundedSender<Vec<u8>>,
        session_id: &mut Option<String>,
    ) -> FrameResult {
        let Some(sid) = session_id.clone() else {
            return FrameResult::protocol_error();
        };
        let mut schedule = false;
        if self.sessions.get(&sid).map(|s| s.conn_id) == Some(conn_id) {
            self.sessions.remove(&sid);
            self.clear_ask_edges_for_session(&sid);
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

    fn handle_list(
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

    fn handle_send(
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
        let reply_edge = message.reply_to.as_ref().and_then(|rt| self.ask_edges.get(rt).cloned());

        let entries: Vec<(String, Option<String>)> = self
            .sessions
            .values()
            .map(|s| (s.info.id.clone(), s.info.name.clone()))
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
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: "Session not found".to_string(),
            });
            return FrameResult::cont();
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
        if let Some(target) = self.sessions.get(&target_id) {
            send_msg(&target.tx, &BrokerMessage::Message { from: from_info, message: message.clone() });
        }
        if let Some(rt) = &message.reply_to {
            self.ask_edges.remove(rt);
        }
        send_msg(self_tx, &BrokerMessage::Delivered { message_id: message.id.clone() });
        FrameResult::cont()
    }

    fn handle_cancel_ask(
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

    fn handle_presence(
        &mut self,
        conn_id: u64,
        value: &serde_json::Value,
        session_id: &Option<String>,
        now: u64,
    ) -> FrameResult {
        let Some(current_id) = session_id.clone() else {
            return FrameResult::protocol_error();
        };
        // Validate field types first (a wrong type is fatal — broker.ts:524,533,542).
        for key in ["name", "status", "model"] {
            if let Some(v) = value.get(key)
                && !v.is_string()
            {
                return FrameResult::protocol_error();
            }
        }
        let Some(session) = self.sessions.get_mut(&current_id).filter(|s| s.conn_id == conn_id) else {
            return FrameResult::cont();
        };
        let mut changed = false;
        if let Some(name) = value.get("name").and_then(|v| v.as_str())
            && session.info.name.as_deref() != Some(name)
        {
            session.info.name = Some(name.to_string());
            changed = true;
        }
        if let Some(status) = value.get("status").and_then(|v| v.as_str())
            && session.info.status.as_deref() != Some(status)
        {
            session.info.status = Some(status.to_string());
            changed = true;
        }
        if let Some(model) = value.get("model").and_then(|v| v.as_str())
            && session.info.model != model
        {
            session.info.model = model.to_string();
            changed = true;
        }
        session.info.last_activity = now;
        let should_broadcast =
            changed || now.saturating_sub(session.last_presence_broadcast_at) >= PRESENCE_HEARTBEAT_MS;
        if should_broadcast {
            session.last_presence_broadcast_at = now;
            let info = session.info.clone();
            self.broadcast(&BrokerMessage::PresenceUpdate { session: info }, Some(&current_id));
        }
        FrameResult::cont()
    }
}

/// Schedule the 5 s auto-shutdown check (`scheduleShutdownCheck`, `broker.ts:286-296`). Only one is
/// ever pending; a `register` in the window bumps `shutdown_gen`, making the pending check stale.
fn schedule_shutdown_check(state: &Arc<Mutex<BrokerState>>) {
    let (generation, shutdown) = {
        let mut g = lock(state);
        if g.shutdown_scheduled {
            return;
        }
        g.shutdown_scheduled = true;
        (g.shutdown_gen, g.shutdown.clone())
    };
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(SHUTDOWN_DELAY_MS)).await;
        let empty_and_current = {
            let mut g = lock(&state);
            g.shutdown_scheduled = false;
            g.shutdown_gen == generation && g.sessions.is_empty()
        };
        if empty_and_current {
            tracing::info!("no sessions connected, shutting down");
            shutdown.notify_one();
        }
    });
}

/// The per-connection writer task: drain queued frames to the socket, then half-close on EOF.
async fn writer_task(mut write_half: OwnedWriteHalf, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(frame) = rx.recv().await {
        if write_half.write_all(&frame).await.is_err() {
            break;
        }
    }
    let _ = write_half.shutdown().await;
}

/// The outcome of [`process_frame_payload`]: whether the connection should keep reading, and
/// whether the registration timeout must be re-armed (`was_registered && session_id.is_none()` on a
/// frame `handle_frame` flagged `rearmed_registration` for — an `unregister`, `broker.ts:223-230`).
struct PayloadOutcome {
    keep_going: bool,
    rearm_registration: bool,
}

/// Process one fully-reassembled frame payload: rate-limit, JSON-decode, dispatch to
/// [`BrokerState::handle_frame`], and apply its result — pi's per-message `onMessage` callback
/// (`framing.ts:29-47`, `broker.ts:217-230`). `keep_going = false` means tear the connection down,
/// mirroring `onError`'s `socket.destroy(error)` / a fatal [`FrameOutcome`].
fn process_frame_payload(
    payload: &[u8],
    conn_id: u64,
    self_tx: &UnboundedSender<Vec<u8>>,
    state: &Arc<Mutex<BrokerState>>,
    bucket: &mut TokenBucket,
    session_id: &mut Option<String>,
) -> PayloadOutcome {
    // Rate limit BEFORE handling (broker.ts:218-222).
    if !bucket.consume(now_ms()) {
        send_msg(self_tx, &BrokerMessage::Error {
            error: "Intercom broker rate limit exceeded".to_string(),
        });
        return PayloadOutcome { keep_going: false, rearm_registration: false };
    }
    let value: serde_json::Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => {
            // `reportMessage`'s `JSON.parse` catch (`framing.ts:29-37`): a descriptive diagnostic,
            // then destroy the connection (`onError` -> `socket.destroy(error)`, `broker.ts:231-233`).
            tracing::warn!(
                error = %crate::transport::framing::FrameError::Parse { message: e.to_string() },
                "intercom broker: dropping connection"
            );
            return PayloadOutcome { keep_going: false, rearm_registration: false };
        }
    };
    let was_registered = session_id.is_some();
    let now = now_ms();
    let result = {
        let mut g = lock(state);
        g.handle_frame(conn_id, self_tx, &value, session_id, now)
    };
    if result.schedule_shutdown {
        schedule_shutdown_check(state);
    }
    let rearm = result.rearmed_registration && was_registered && session_id.is_none();
    PayloadOutcome { keep_going: matches!(result.outcome, FrameOutcome::Continue), rearm_registration: rearm }
}

/// The per-connection reader task: read chunks, reassemble frames, rate-limit, and dispatch each to
/// [`BrokerState::handle_frame`], honoring the 1 s registration timeout.
async fn reader_task(
    conn_id: u64,
    mut read_half: OwnedReadHalf,
    self_tx: UnboundedSender<Vec<u8>>,
    close: Arc<Notify>,
    state: Arc<Mutex<BrokerState>>,
) {
    let mut session_id: Option<String> = None;
    let mut bucket = TokenBucket::new(now_ms());
    let mut reader = FrameReader::new();
    let mut buf = vec![0u8; READ_BUF];

    let reg_deadline = tokio::time::sleep(Duration::from_millis(REGISTRATION_TIMEOUT_MS));
    tokio::pin!(reg_deadline);

    'outer: loop {
        tokio::select! {
            biased;
            () = close.notified() => break,
            () = &mut reg_deadline, if session_id.is_none() => {
                // No register within the timeout → destroy (broker.ts:196-201).
                break;
            }
            read = read_half.read(&mut buf) => {
                let n = match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let chunk = buf.get(..n).unwrap_or(&[]);
                let frames = match reader.push(chunk) {
                    Ok(frames) => frames,
                    Err(e) => {
                        // pi's reader delivers every frame reassembled earlier in this SAME chunk to
                        // `onMessage` synchronously, in order, and only afterward discovers/reports the
                        // oversize length (`framing.ts:52-84`) — dispatch `e.frames` before tearing the
                        // connection down, rather than discarding them.
                        for payload in &e.frames {
                            let outcome = process_frame_payload(payload, conn_id, &self_tx, &state, &mut bucket, &mut session_id);
                            if outcome.rearm_registration {
                                reg_deadline
                                    .as_mut()
                                    .reset(tokio::time::Instant::now() + Duration::from_millis(REGISTRATION_TIMEOUT_MS));
                            }
                            if !outcome.keep_going {
                                break;
                            }
                        }
                        tracing::warn!(error = %e.error, "intercom broker: dropping connection");
                        break; // oversize → drop the connection (framing.ts:63-66)
                    }
                };
                for payload in &frames {
                    let outcome = process_frame_payload(payload, conn_id, &self_tx, &state, &mut bucket, &mut session_id);
                    if outcome.rearm_registration {
                        reg_deadline
                            .as_mut()
                            .reset(tokio::time::Instant::now() + Duration::from_millis(REGISTRATION_TIMEOUT_MS));
                    }
                    if !outcome.keep_going {
                        break 'outer;
                    }
                }
            }
        }
    }

    // Teardown (socket 'close', broker.ts:237-249). Dropping `self_tx` after this lets the writer
    // task's channel close so it half-closes the socket.
    let did_leave = {
        let mut g = lock(&state);
        g.on_connection_closed(conn_id, &session_id)
    };
    if did_leave {
        schedule_shutdown_check(&state);
    }
    drop(self_tx);
}

/// Wire one accepted connection: split it, spawn its writer + reader, and register it.
fn spawn_connection(
    conn_id: u64,
    stream: tokio::net::UnixStream,
    state: Arc<Mutex<BrokerState>>,
) {
    let (read_half, write_half) = stream.into_split();
    let (tx, rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let close = Arc::new(Notify::new());
    tokio::spawn(writer_task(write_half, rx));
    {
        let mut g = lock(&state);
        g.add_connection(conn_id, close.clone());
    }
    tokio::spawn(reader_task(conn_id, read_half, tx, close, state));
}

/// Tear down the whole broker on shutdown (`shutdown`, `broker.ts:606-633`): end every session,
/// clear the maps, unlink the runtime files.
fn shutdown_broker(state: &Arc<Mutex<BrokerState>>, socket_path: &std::path::Path, pid_path: &std::path::Path) {
    {
        let mut g = lock(state);
        for (_id, h) in g.connections.drain() {
            h.close.notify_one();
        }
        g.sessions.clear();
        g.ask_edges.clear();
        g.unregistered.clear();
    }
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_file(pid_path);
}

/// The `cyrup __intercom-broker` entrypoint (`new IntercomBroker().start()`, `broker.ts:636`).
/// Binds the Unix socket, writes the pid file, runs the accept loop, and shuts down on SIGTERM/
/// SIGINT or the 5 s idle auto-shutdown. Returns once the socket + runtime files are cleaned up.
///
/// # Errors
/// Returns an I/O error if the intercom dir cannot be created or the socket cannot be bound.
pub async fn run() -> std::io::Result<()> {
    // `ask_timeout_ms` hard-errors on an invalid env value, matching pi's uncaught throw
    // (`config.ts:14-16`) that crashes `new IntercomBroker()` — a class-field initializer that runs
    // INSIDE the constructor, i.e. before `.start()` ever binds the listener or writes any file
    // (`broker.ts:139`). Resolved here FIRST, before any startup side effect (dir/socket/pid), so an
    // invalid env value fails the whole process before anything is created — never a socket/pid file
    // left behind for an external caller to observe as a falsely "started" broker.
    let ask_timeout = config::ask_timeout_ms()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let agent_dir = paths::agent_dir_path();
    let intercom_dir = paths::intercom_dir_path(&agent_dir);
    paths::ensure_intercom_runtime_dir(&intercom_dir)?;
    let socket_path = paths::broker_socket_path(&intercom_dir);
    let pid_path = paths::broker_pid_path(&intercom_dir);

    // Unlink a stale socket left by a crashed broker (broker.ts:143-148).
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    let _ = paths::restrict_intercom_runtime_file(&socket_path);
    std::fs::write(&pid_path, std::process::id().to_string())?;
    let _ = paths::restrict_intercom_runtime_file(&pid_path);
    tracing::info!(pid = std::process::id(), socket = %socket_path.display(), "intercom broker started");

    let shutdown = Arc::new(Notify::new());
    let state = Arc::new(Mutex::new(BrokerState::new(ask_timeout, shutdown.clone())));
    let mut next_conn_id: u64 = 0;

    // SIGTERM/SIGINT → graceful shutdown (broker.ts:181-182). The broker is a Unix-socket process
    // (`UnixListener` above), so this whole entrypoint is inherently unix-only for this milestone.
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    loop {
        tokio::select! {
            () = shutdown.notified() => break,
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        let conn_id = next_conn_id;
                        next_conn_id = next_conn_id.wrapping_add(1);
                        spawn_connection(conn_id, stream, state.clone());
                    }
                    Err(e) => tracing::warn!(error = %e, "intercom broker accept failed"),
                }
            }
        }
    }

    tracing::info!("intercom broker shutting down");
    shutdown_broker(&state, &socket_path, &pid_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use serde_json::json;

    fn make_state() -> BrokerState {
        BrokerState::new(30_000, Arc::new(Notify::new()))
    }

    fn make_tx() -> UnboundedSender<Vec<u8>> {
        let (tx, _rx) = mpsc::unbounded_channel::<Vec<u8>>();
        tx
    }

    fn register(state: &mut BrokerState, conn_id: u64, session_id: &mut Option<String>, id: &str) {
        let tx = make_tx();
        let value = json!({
            "type": "register",
            "sessionId": id,
            "session": {
                "cwd": "/tmp",
                "model": "test-model",
                "pid": 1,
                "startedAt": 0,
                "lastActivity": 0,
            }
        });
        let result = state.handle_register(conn_id, &tx, &value, session_id, 0);
        assert!(matches!(result.outcome, FrameOutcome::Continue));
    }

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
            let result = state.handle_unregister(conn_id as u64, &tx, sid);
            assert!(matches!(result.outcome, FrameOutcome::Continue));
            assert!(
                state.unregistered.len() <= MAX_UNREGISTERED_CONNECTIONS,
                "unregistered set exceeded the cap after unregister #{conn_id}: {}",
                state.unregistered.len()
            );
        }
        assert_eq!(state.unregistered.len(), MAX_UNREGISTERED_CONNECTIONS);
    }

    /// Regression test for the framing.rs dossier item ("frames already reassembled before an
    /// oversize frame in the same `push()` call are discarded"): pi's reader delivers every complete
    /// frame found earlier in the same `data` chunk to `onMessage` synchronously, in order, BEFORE it
    /// discovers a later oversize length (`framing.ts:52-84`). Before this fix, `reader_task`'s
    /// `Err(_) => break` on `reader.push` discarded `FrameReadError::frames` entirely — a `register`
    /// frame reassembled earlier in the very same chunk as a trailing oversize header would never
    /// reach `handle_frame`, silently dropping a connection's registration. This test fails against
    /// that pre-fix behavior: `session_id` would stay `None` instead of becoming `Some("s1")`.
    #[test]
    fn oversize_chunk_still_dispatches_frames_reassembled_earlier_in_the_same_chunk() {
        let state: Arc<Mutex<BrokerState>> = Arc::new(Mutex::new(make_state()));
        let mut session_id: Option<String> = None;
        let mut bucket = TokenBucket::new(now_ms());
        let self_tx = make_tx();

        let register_payload = json!({
            "type": "register",
            "sessionId": "s1",
            "session": {"cwd": "/tmp", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0}
        });
        let register_bytes = serde_json::to_vec(&register_payload).unwrap();
        let mut chunk = crate::transport::framing::encode_frame(&register_bytes);
        // Append a bogus trailing frame header declaring an over-cap length, in the SAME chunk.
        let bad_len = (crate::transport::framing::MAX_FRAME_BYTES as u32) + 1;
        chunk.extend_from_slice(&bad_len.to_be_bytes());

        let mut reader = FrameReader::new();
        let err = reader.push(&chunk).expect_err("oversize declared length must error");
        assert_eq!(
            err.frames.len(),
            1,
            "the register frame reassembled before the oversize header must be preserved, not discarded"
        );

        for payload in &err.frames {
            let outcome = process_frame_payload(payload, 1, &self_tx, &state, &mut bucket, &mut session_id);
            assert!(outcome.keep_going, "a valid register frame must not itself trip a teardown");
        }
        assert_eq!(
            session_id.as_deref(),
            Some("s1"),
            "the preserved register frame must actually be dispatched to handle_frame, not discarded"
        );
    }
}

//! The standalone broker **process** — a 1:1 port of `pi-intercom/broker/broker.ts`.
//!
//! Dispatched as the hidden `cyrup __intercom-broker` subcommand (re-exec of `current_exe()`,
//! mirroring `cyrup-ext-subagents`' `__subagent-runner`). It binds the listen target resolved by
//! [`crate::transport::target::broker_listen_target`] — `<intercomDir>/broker.sock` on POSIX,
//! `\\.\pipe\cyrup-intercom-<agent dir>` on Windows — speaks length-prefixed JSON
//! ([`crate::transport::framing`]), routes
//! `send` frames child→broker→target by session identity, enforces the registration handshake +
//! caps + per-connection token bucket, tracks ask edges (mutual-ask refusal + prune), coalesces
//! presence, answers the health probe byte-identically, and auto-shuts-down 5 s after its last
//! client leaves (`broker.ts:286-296`).
//!
//! Transports: the Unix domain socket (POSIX) and the Windows named pipe are both bound through
//! [`listener::BrokerListener`], which is this port's stand-in for upstream's single polymorphic
//! `net.createServer().listen(LISTEN_TARGET)` (`broker.ts:123,149-152`). The **opt-in** loopback-TCP
//! transport (`CYRUP_INTERCOM_TRANSPORT=tcp`; `broker.ts:134-141,284-305`, `stateId` auth) is the
//! one piece still unported — see the port doc §10-Q2 and [`listener::BrokerListener::bind`], which
//! refuses that listen target loudly rather than downgrading to another endpoint.

pub mod listener;
pub mod ratelimit;
pub mod routing;
pub mod runtime_claim;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::config;
use crate::paths;
use crate::transport::framing::{FrameReader, encode_json};
use crate::transport::protocol::{
    BrokerMessage, ExtensionCapability, HealthMessage, Message, MessageControl,
    MessageControlAction, MessageReceipt, PROTOCOL_NAME, PROTOCOL_VERSION, SessionInfo,
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
/// `MESSAGE_RECEIPT_ROUTE_RETENTION_MS = 60 * 60 * 1000` (`v0.10.1 broker/broker.ts:39`).
const MESSAGE_RECEIPT_ROUTE_RETENTION_MS: u64 = 60 * 60 * 1000;
/// `DISCONNECTED_SESSION_RETENTION_MS = 24 * 60 * 60 * 1000` (`v0.10.1 broker/broker.ts:40`).
const DISCONNECTED_SESSION_RETENTION_MS: u64 = 24 * 60 * 60 * 1000;
/// `MAILBOX_MESSAGE_RETENTION_MS = 24 * 60 * 60 * 1000` (`v0.10.1 broker/broker.ts:41`).
const MAILBOX_MESSAGE_RETENTION_MS: u64 = 24 * 60 * 60 * 1000;
/// `MAX_MAILBOX_MESSAGES = 256` (`v0.10.1 broker/broker.ts:42`).
const MAX_MAILBOX_MESSAGES: usize = 256;
/// Reader read-buffer size (implementation detail; framing reassembles across chunk boundaries).
const READ_BUF: usize = 16 * 1024;
/// `MAX_EXTENSIONS_PER_SESSION = 32` (`v0.9.2 broker/broker.ts:35`).
const MAX_EXTENSIONS_PER_SESSION: usize = 32;

/// pi's `extensions` field guard, shared verbatim by `case "register"`
/// (`v0.9.2 broker/broker.ts:446-456`) and `case "extension_capabilities_update"`
/// (`v0.9.2 broker/broker.ts:559-567`): the value must be an ARRAY of at most
/// [`MAX_EXTENSIONS_PER_SESSION`] entries, each passing `validateExtensionCapability`
/// (`v0.9.2 broker/broker.ts:1159-1168`). Anything else `throw`s, i.e. destroys the socket.
///
/// The per-entry decode into [`ExtensionCapability`] reproduces upstream's
/// `typeof c.namespace !== "string" || typeof c.ownerEligible !== "boolean"` check *and* its
/// rejection of an array-shaped entry (`[]["namespace"]` is `undefined`), the latter because that
/// struct is `[MAP-ONLY]` — see `crate::transport::protocol`.
fn extensions_field_is_valid(extensions: &serde_json::Value) -> bool {
    let Some(items) = extensions.as_array() else {
        return false;
    };
    if items.len() > MAX_EXTENSIONS_PER_SESSION {
        return false;
    }
    items.iter().all(|item| {
        serde_json::from_value::<ExtensionCapability>(item.clone())
            .is_ok_and(|cap| namespace_is_valid(&cap.namespace))
    })
}

/// `validateNamespace` (`v0.9.2 broker/broker.ts:1170-1182`): `^[a-z0-9][a-z0-9._/-]{0,63}$`, with
/// the length bound checked first.
///
/// [CYRUP-DELTA] pi's `ns.length` counts UTF-16 code units and this counts `char`s; the two can
/// disagree only for non-ASCII input, which the character test rejects on both sides anyway, so the
/// accepted set is identical.
fn namespace_is_valid(ns: &str) -> bool {
    if ns.is_empty() || ns.chars().count() > 64 {
        return false;
    }
    let mut chars = ns.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/' | '-'))
}

/// `String(msg.namespace || "")` — the expression pi echoes into the two `extension_state_result`
/// frames it emits *before* `namespace` has been type-checked
/// (`v0.9.2 broker/broker.ts:1371` and `:1382`). Those are the only two places in the protocol
/// where an arbitrary untyped JSON value is coerced to a string, so the JS coercion is reproduced
/// here rather than approximated: the field is echoed back to a peer that may be matching on it.
///
/// `||` short-circuits on every JS falsy value, so `undefined`/`null`/`false`/`0`/`""` all yield
/// `""`; anything else goes through `ToString`.
///
/// [CYRUP-DELTA] Number formatting agrees with JS for every integral value under `1e21` and for
/// the shortest-round-trip decimals both runtimes emit, but not for JS's exponent notation
/// (`String(1e21)` is `"1e+21"` upstream and `1e21` here). JSON cannot carry `NaN`/`Infinity` at
/// all, so those cases are unreachable rather than divergent.
fn js_string_or_empty(v: Option<&serde_json::Value>) -> String {
    match v {
        None => String::new(),
        Some(v) if js_is_falsy(v) => String::new(),
        Some(v) => js_to_string(v),
    }
}

/// JS falsiness for the JSON value subset (`undefined` is the `None` arm of the caller).
fn js_is_falsy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => true,
        serde_json::Value::Bool(b) => !b,
        serde_json::Value::Number(n) => n.as_f64() == Some(0.0),
        serde_json::Value::String(s) => s.is_empty(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => false,
    }
}

/// JS `ToString` for the JSON value subset. Arrays go through `Array.prototype.join(",")`, which
/// renders `null` elements as the empty string and recurses into nested arrays; every plain object
/// stringifies to `"[object Object]"`.
fn js_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => js_number_to_string(n),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|it| match it {
                serde_json::Value::Null => String::new(),
                other => js_to_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        serde_json::Value::Object(_) => "[object Object]".to_string(),
    }
}

/// JS `Number::toString`. `1.0` is the integer `1` upstream, so an integral `f64` is printed
/// without its fractional part; everything else falls back to serde's shortest round-trip form,
/// which matches JS across the ordinary decimal range.
fn js_number_to_string(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    match n.as_f64() {
        Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e21 => format!("{f:.0}"),
        _ => n.to_string(),
    }
}

/// `info.runtimeFallbackAlias` read as a JS **truthiness** test, the way the mailbox identity guard
/// reads it (`v0.10.1 broker/broker.ts:1041`, `:1045`).
///
/// `undefined` and `false` are both falsy upstream, so an explicit `runtimeFallbackAlias: false` —
/// which cyrup's own presence path can send (`transport/protocol.rs:727`) — must NOT disqualify a
/// session from owning its mailbox identity. `Option::is_some` would.
const fn js_truthy_alias(alias: Option<bool>) -> bool {
    matches!(alias, Some(true))
}

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

/// `interface DisconnectedSession` (`v0.10.1 broker/broker.ts:85-88`) — the last-known
/// [`SessionInfo`] of a session that has left, kept for
/// [`DISCONNECTED_SESSION_RETENTION_MS`] so a `send` naming it can still be routed to its mailbox.
struct DisconnectedSession {
    info: SessionInfo,
    disconnected_at: u64,
}

/// `interface MailboxMessage` (`v0.10.1 broker/broker.ts:90-95`) — one message parked for a
/// disconnected target, redelivered by [`BrokerState::flush_mailbox_for_session`] when that
/// identity registers again.
struct MailboxMessage {
    from: SessionInfo,
    target: SessionInfo,
    message: Message,
    queued_at: u64,
}

/// `interface MessageReceiptRoute` (`v0.10.1 broker/broker.ts:80-84`) — where a delivered message
/// went, so a receipt from the receiver can be forwarded back to its original sender and so the
/// sender can `cancel`/`supersede` it.
struct MessageReceiptRoute {
    from: String,
    to: String,
    created_at: u64,
}

/// The broker's in-memory routing state (`broker.ts:132-139`). Held behind a `std::sync::Mutex`;
/// every handler is synchronous and never holds the guard across an `.await`.
struct BrokerState {
    sessions: HashMap<String, ConnectedSession>,
    /// Registered session ids in **join order**. `broker.ts:133` holds the sessions in a JS `Map`,
    /// which iterates in insertion order, so every consumer of the map — the `list` reply
    /// (`broker.ts:408`), presence broadcasts and name resolution — observes a stable join order.
    /// A `std::collections::HashMap` has no such guarantee, so the order is tracked alongside it,
    /// the way `unregistered` already tracks connection insertion order below.
    session_order: Vec<String>,
    ask_edges: HashMap<String, AskEdge>,
    /// `messageReceiptRoutes` (`v0.10.1 broker/broker.ts:100`), keyed by message id.
    message_receipt_routes: HashMap<String, MessageReceiptRoute>,
    /// `disconnectedSessions` (`v0.10.1 broker/broker.ts:101`), keyed by session id.
    ///
    /// pi holds this in a JS `Map`, whose iteration order is insertion order; a `HashMap`'s is
    /// arbitrary. That is immaterial HERE, unlike [`BrokerState::session_order`]: the only consumer
    /// is `findDisconnectedSessions` (`:1010-1024`) and every one of ITS consumers is gated on
    /// `length === 1` or `length > 1` (`:596`, `:660`), so no branch can observe which element
    /// came first. Same argument as `resolve_reply_target`'s (ICOM-001).
    disconnected_sessions: HashMap<String, DisconnectedSession>,
    /// `mailboxMessages` (`v0.10.1 broker/broker.ts:102`) — an ARRAY upstream, and the order is
    /// load-bearing: [`BrokerState::queue_mailbox_message`] evicts from the FRONT at the cap
    /// (`:892-898`, FIFO) and [`BrokerState::flush_mailbox_for_session`] redelivers front-to-back
    /// (`:913`), so a peer receives its parked mail in the order it was sent.
    mailbox_messages: Vec<MailboxMessage>,
    connections: HashMap<u64, ConnHandle>,
    /// Unregistered connection ids in insertion order (for oldest-eviction, `broker.ts:256-268`).
    unregistered: Vec<u64>,
    ask_timeout_ms: u64,
    /// Bumped on every `register` so a pending auto-shutdown check becomes stale (`broker.ts:378-381`).
    shutdown_gen: u64,
    shutdown_scheduled: bool,
    /// The pending auto-shutdown task, i.e. pi's `shutdownTimer` HANDLE
    /// (`v0.10.1 broker/broker.ts:106`). Holding it is what makes `register`'s
    /// `clearTimeout(this.shutdownTimer); this.shutdownTimer = null` (`:378-381`) portable: without
    /// it, a register inside the 5 s window left `shutdown_scheduled` set, so the next disconnect's
    /// `schedule_shutdown_check` early-returned and the re-arm was LOST — the broker then idled
    /// forever with zero sessions until an unrelated connect/disconnect cycle re-armed it.
    shutdown_task: Option<tokio::task::JoinHandle<()>>,
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
            session_order: Vec::new(),
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

    /// The registered sessions in join order — the Rust equivalent of iterating pi's
    /// `this.sessions` JS `Map` (`broker.ts:133`).
    fn sessions_in_order(&self) -> impl Iterator<Item = (&String, &ConnectedSession)> {
        self.session_order.iter().filter_map(|id| self.sessions.get_key_value(id))
    }

    /// `this.sessions.set(id, …)` (`broker.ts:376`). JS `Map.set` on an **existing** key keeps
    /// that key's original position, so an identity takeover must not move the session to the back
    /// of the join order.
    fn insert_session(&mut self, id: String, session: ConnectedSession) {
        if self.sessions.insert(id.clone(), session).is_none() {
            self.session_order.push(id);
        }
    }

    /// `this.sessions.delete(id)` (`broker.ts:243,394`).
    fn remove_session(&mut self, id: &str) {
        if self.sessions.remove(id).is_some() {
            self.session_order.retain(|s| s != id);
        }
    }

    fn broadcast(&self, msg: &BrokerMessage, exclude: Option<&str>) {
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

    fn clear_ask_edges_for_session(&mut self, session_id: &str) {
        self.ask_edges.retain(|_, edge| edge.from != session_id && edge.to != session_id);
    }

    fn prune_ask_edges(&mut self, now: u64) {
        let timeout = self.ask_timeout_ms;
        self.ask_edges.retain(|_, edge| now.saturating_sub(edge.created_at) <= timeout);
    }

    /// `clearMessageReceiptRoutesForSession` (`v0.10.1 broker/broker.ts:979-985`).
    fn clear_message_receipt_routes_for_session(&mut self, session_id: &str) {
        self.message_receipt_routes
            .retain(|_, route| route.from != session_id && route.to != session_id);
    }

    /// `pruneMessageReceiptRoutes` (`v0.10.1 broker/broker.ts:971-977`).
    fn prune_message_receipt_routes(&mut self, now: u64) {
        self.message_receipt_routes.retain(|_, route| {
            now.saturating_sub(route.created_at) <= MESSAGE_RECEIPT_ROUTE_RETENTION_MS
        });
    }

    /// `rememberDisconnectedSession` (`v0.10.1 broker/broker.ts:864-867`).
    ///
    /// pi stores a COPY (`{ ...info }`) because the live `ConnectedSession.info` it was read from
    /// keeps being mutated by presence frames; the Rust `SessionInfo` is moved/cloned in, so the
    /// same isolation is structural here.
    fn remember_disconnected_session(&mut self, info: SessionInfo, now: u64) {
        self.disconnected_sessions
            .insert(info.id.clone(), DisconnectedSession { info, disconnected_at: now });
        self.prune_disconnected_sessions(now);
    }

    /// `pruneDisconnectedSessions` (`v0.10.1 broker/broker.ts:869-875`).
    fn prune_disconnected_sessions(&mut self, now: u64) {
        self.disconnected_sessions.retain(|_, session| {
            now.saturating_sub(session.disconnected_at) <= DISCONNECTED_SESSION_RETENTION_MS
        });
    }

    /// `pruneMailboxMessages` (`v0.10.1 broker/broker.ts:877-888`).
    ///
    /// Dropping a parked ask must drop its ask edge too, or the sender's reply window stays open
    /// against mail that no longer exists.
    fn prune_mailbox_messages(&mut self, now: u64) {
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
    fn queue_mailbox_message(
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
    fn find_live_sessions_sharing_mailbox_identity(&self, info: &SessionInfo) -> Vec<String> {
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
    fn find_unique_live_session_for_disconnected_session(
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
    fn find_disconnected_session_ids(&mut self, name_or_id: &str, now: u64) -> Vec<String> {
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
    fn flush_mailbox_for_session(&mut self, session_id: &str, now: u64) {
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

    /// `Array.from(this.sessions.values()).map(s => s.info)` (`broker.ts:408`) — join-ordered,
    /// because pi's `Map` iterates in insertion order and neither `index.ts`'s `list` handler nor
    /// `ui/session-list.ts` re-sorts the reply.
    fn session_infos(&self) -> Vec<SessionInfo> {
        self.sessions_in_order().map(|(_, s)| s.info.clone()).collect()
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
    fn on_connection_closed(&mut self, conn_id: u64, session_id: &Option<String>, now: u64) -> bool {
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
            "unregister" => self.handle_unregister(conn_id, self_tx, session_id, now),
            "list" => self.handle_list(self_tx, value),
            "send" => self.handle_send(conn_id, self_tx, value, session_id, now),
            "cancel_ask" => self.handle_cancel_ask(conn_id, value, session_id),
            "presence" => self.handle_presence(conn_id, value, session_id, now),
            "message_receipt" => self.handle_message_receipt(conn_id, value, session_id, now),
            "cancel_message" => self.handle_cancel_message(conn_id, self_tx, value, session_id, now),
            // Extension-bus frames (`v0.9.2 broker/broker.ts:551-585,961-969`). cyrup does not
            // implement the bus, so it never advertises `EXTENSION_BUS_FEATURE` on `registered` —
            // which is exactly what stops a conforming pi client from sending these
            // (`supportsFeature` gate, `v0.9.2 broker/client.ts:648,817-819`). A NON-conforming
            // peer can still send them over a socket every process on the box can reach, so each
            // handler ports pi's own validation prefix and pi's own miss branch. The bus EFFECTS
            // stay unported; nothing below needs them.
            "extension_publish" => self.handle_extension_publish(conn_id, self_tx, session_id),
            "extension_state_commit" => {
                self.handle_extension_state_commit(conn_id, self_tx, value, session_id)
            }
            "extension_capabilities_update" => {
                self.handle_extension_capabilities_update(conn_id, value, session_id)
            }
            // Genuinely unknown tags stay fatal — that is pi's own behaviour
            // (`default: throw new Error(\`Unknown client message type\`)`,
            // `v0.9.2 broker/broker.ts:971-972`, routed to `socket.destroy(error)` by
            // `framing.ts:44-51` + `broker.ts:321-323`). Forward compatibility upstream comes from
            // additive FIELDS and feature negotiation, never from accepting unknown tags.
            _ => FrameResult::protocol_error(),
        }
    }

    /// `case "message_receipt"` (`v0.10.1 broker/broker.ts:676-696`).
    ///
    /// pi validates the receipt with `isMessageReceipt()` — a bad one THROWS, i.e. destroys the
    /// connection — then looks the message up in `messageReceiptRoutes` and forwards the receipt to
    /// the original sender only if the route says this session was the receiver AND still owns this
    /// socket. A miss on any of the three is a silent `break`, not an error frame.
    fn handle_message_receipt(
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

    /// `case "extension_capabilities_update"` (`v0.9.2 broker/broker.ts:551-585`).
    ///
    /// cyrup does not implement the extension bus, so pi's *effects* — `session.extensions = …`,
    /// `recomputeNamespaceOwners()`, and the `extension_owner`/`extension_state` replies
    /// (`v0.9.2 broker/broker.ts:568-585`) — stay unported and the frame is ignored, exactly like
    /// `extension_publish`/`extension_state_commit` above.
    ///
    /// pi's **validation prefix** (`v0.9.2 broker/broker.ts:559-567`) is ported, though, because it
    /// runs before any of that and every one of its failures is a `throw` → `socket.destroy`
    /// (`framing.ts:44-51`). Without it cyrup accepts an `extensions` payload pi kills the
    /// connection over — including the array-shaped `[["ns", true]]`, which serde would otherwise
    /// fill into [`ExtensionCapability`] positionally — on a socket every process on the box can
    /// reach. Ignoring a *well-formed* frame is a survivability choice; ignoring a malformed one is
    /// an input-validation hole.
    ///
    /// The "before register" throw at `:552-554` is covered by the shared pre-registration guard in
    /// [`Self::handle_frame`]. The "session not found" throw at `:555-558` is ported below: pi's
    /// `session.socket !== socket` is [`Self::session_owns_connection`] here, and it IS reachable —
    /// an identity takeover (`handle_register`'s `close.notify_one()`) reassigns the id to the new
    /// connection before the superseded reader task observes the notify, so a frame already in the
    /// old socket's buffer arrives with a stale `conn_id`. Upstream destroys that connection; so
    /// does this.
    fn handle_extension_capabilities_update(
        &self,
        conn_id: u64,
        value: &serde_json::Value,
        session_id: &Option<String>,
    ) -> FrameResult {
        let Some(current_id) = session_id.as_deref() else {
            return FrameResult::protocol_error();
        };
        // `throw new Error("Extension capability session not found")`
        // (`v0.9.2 broker/broker.ts:556-558`) — fatal, unlike the two handlers below, which answer.
        if !self.session_owns_connection(current_id, conn_id) {
            return FrameResult::protocol_error();
        }
        // `!Array.isArray(undefined)` is true, so a missing field throws upstream too
        // (`v0.9.2 broker/broker.ts:559-562`).
        let extensions = value.get("extensions").unwrap_or(&serde_json::Value::Null);
        if !extensions_field_is_valid(extensions) {
            return FrameResult::protocol_error();
        }
        tracing::debug!(
            "intercom broker: extension_capabilities_update ignored (bus not implemented)"
        );
        FrameResult::cont()
    }

    /// pi's `session.socket !== socket` guard (`v0.9.2 broker/broker.ts:556,1272,1368`), expressed
    /// against cyrup's connection ids: the session must exist AND still be owned by the connection
    /// the frame arrived on.
    fn session_owns_connection(&self, session_id: &str, conn_id: u64) -> bool {
        self.sessions.get(session_id).map(|s| s.conn_id) == Some(conn_id)
    }

    /// `case "extension_publish"` (`v0.9.2 broker/broker.ts:961-964`) → `handleExtensionPublish`
    /// (`v0.9.2 broker/broker.ts:1262-1356`).
    ///
    /// cyrup does not implement the extension bus, which means it never records
    /// `session.extensions` — pi's `case "extension_capabilities_update"` assignment at
    /// `v0.9.2 broker/broker.ts:568` is exactly the effect this crate leaves unported. So
    /// `!session.extensions?.length` (`:1277`) is **unconditionally true** here and pi's own
    /// not-advertised miss branch is the whole handler: `error`
    /// `"Session has not advertised extension capability"` (`:1278`). Everything past `:1281` —
    /// the namespace / audience / payload-size checks and the fan-out — is unreachable while the
    /// bus is unported, so porting it would be dead code guessing at state that cannot exist.
    ///
    /// Answering matters for the same reason it did for `cancel_message`: pi's client resolves an
    /// extension publish only on a broker frame, so a silent drop strands the caller. Ignoring the
    /// frame outright was also LOOSER than upstream, on a socket every process on the box can open.
    fn handle_extension_publish(
        &self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        session_id: &Option<String>,
    ) -> FrameResult {
        let Some(current_id) = session_id.as_deref() else {
            return FrameResult::protocol_error();
        };
        // `!session || session.socket !== socket` → `error: "Session not found"`, and NOT fatal
        // (`v0.9.2 broker/broker.ts:1271-1275`).
        let error = if self.session_owns_connection(current_id, conn_id) {
            "Session has not advertised extension capability"
        } else {
            "Session not found"
        };
        send_msg(self_tx, &BrokerMessage::Error { error: error.to_string() });
        FrameResult::cont()
    }

    /// `case "extension_state_commit"` (`v0.9.2 broker/broker.ts:966-969`) →
    /// `handleExtensionStateCommit` (`v0.9.2 broker/broker.ts:1358-1495`).
    ///
    /// Same shape as [`Self::handle_extension_publish`], with pi's different refusal frame: every
    /// exit from this handler writes an `extension_state_result`, so the two miss branches at
    /// `v0.9.2 broker/broker.ts:1367-1388` are `committed: false`, `revision: 0` and a reason —
    /// `"Session not found"` (`:1374`) or `"Session has not advertised extension capability"`
    /// (`:1385`). With the bus unported the second is unconditional, exactly as above.
    ///
    /// Both branches echo `String(msg.namespace || "")` (`:1371`, `:1382`) — the raw, not-yet-
    /// type-checked value — so [`js_string_or_empty`] reproduces that coercion rather than
    /// requiring a string.
    ///
    /// A commit is a promise upstream; dropping it silently hangs the committer forever, which is
    /// the same defect the `cancel_message` port already fixed.
    fn handle_extension_state_commit(
        &self,
        conn_id: u64,
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
        session_id: &Option<String>,
    ) -> FrameResult {
        let Some(current_id) = session_id.as_deref() else {
            return FrameResult::protocol_error();
        };
        let reason = if self.session_owns_connection(current_id, conn_id) {
            "Session has not advertised extension capability"
        } else {
            "Session not found"
        };
        send_msg(self_tx, &BrokerMessage::ExtensionStateResult {
            namespace: js_string_or_empty(value.get("namespace")),
            committed: false,
            revision: 0,
            reason: Some(reason.to_string()),
        });
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
    fn handle_cancel_message(
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
            // trustedLocal = unix && !win — broker-owned, never from the payload (broker.ts:374).
            trusted_local: Some(cfg!(unix)),
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

    fn handle_unregister(
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
        // OWNERSHIP FIRST. Every `throw new Error("Invalid presence …")` upstream is nested INSIDE
        // `if (session?.socket === socket) { … }` (`v0.10.1 broker/broker.ts:763-805`, guard at
        // `:765`), so a NON-OWNING socket's malformed presence is ignored, not fatal. Running the
        // type checks first killed a superseded socket's late malformed frame as a protocol error;
        // the reconnect ladder deliberately re-offers the previous session id, so takeover races are
        // a live path, not a theoretical one.
        let Some(session) = self.sessions.get_mut(&current_id).filter(|s| s.conn_id == conn_id) else {
            return FrameResult::cont();
        };
        // A wrong type IS fatal for the owner (`v0.9.2 broker/broker.ts:892-894`, `:901-903`,
        // `:910-912`). `value.get` yields `Some(Value::Null)` for an explicit `null`, which is not a
        // string, so `null` is fatal here exactly as `typeof null === "object"` makes it upstream.
        for key in ["name", "status", "model"] {
            if let Some(v) = value.get(key)
                && !v.is_string()
            {
                return FrameResult::protocol_error();
            }
        }
        // `runtimeFallbackAlias` (`v0.10.1 broker/broker.ts:779-787`) is a BOOLEAN, and its check
        // sits inside the same ownership block.
        if let Some(v) = value.get("runtimeFallbackAlias")
            && !v.is_boolean()
        {
            return FrameResult::protocol_error();
        }
        // The context-usage trio obeys a DIFFERENT rule from the string trio above, and from
        // `isSessionInfo`'s (`v0.9.2 broker/client.ts:182-186`, where `null` is fatal). Here an
        // explicit `null` is a legal CLEAR — the value is genuinely unknown right after a
        // compaction, and carrying the stale-high one forward would be a lie — so only a value that
        // is neither `null` nor a number is fatal (`v0.9.2 broker/broker.ts:921-950`, the
        // `else if (typeof … !== "number") throw` arm at `:924`, `:934`, `:944`).
        for key in ["contextPct", "contextTokens", "contextWindow"] {
            if let Some(v) = value.get(key)
                && !v.is_null()
                && !v.is_number()
            {
                return FrameResult::protocol_error();
            }
        }
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
        // `v0.10.1 broker/broker.ts:779-787` — additive at v0.10.0 (`126875e`). The flag tells a
        // peer a chosen name from a synthesized alias, and the mailbox identity guard reads it
        // (`:1039-1047`).
        if let Some(alias) = value.get("runtimeFallbackAlias").and_then(serde_json::Value::as_bool)
            && session.info.runtime_fallback_alias != Some(alias)
        {
            session.info.runtime_fallback_alias = Some(alias);
            changed = true;
        }
        // `v0.9.2 broker/broker.ts:921-950`, one arm per field. Kept as three explicit calls (not a
        // loop) because Rust cannot index a struct by name; the helper carries the whole tri-state.
        changed |= apply_presence_context(&mut session.info.context_pct, value.get("contextPct"));
        changed |= apply_presence_context(&mut session.info.context_tokens, value.get("contextTokens"));
        changed |= apply_presence_context(&mut session.info.context_window, value.get("contextWindow"));
        session.info.last_activity = now.into();
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

/// Apply one `presence` context-usage field to the session's stored [`SessionInfo`], returning
/// whether it changed (`v0.9.2 broker/broker.ts:921-930` for `contextPct`, `:931-940` for
/// `contextTokens`, `:941-950` for `contextWindow` — three copies of one 10-line shape).
///
/// The tri-state is upstream's, verbatim, and it is NOT the rule the same three fields obey inside
/// `isSessionInfo` (`v0.9.2 broker/client.ts:182-186`), where `null` is a rejection:
///
/// * key absent (`undefined`) — leave the field untouched, no change.
/// * key present and `null` — **CLEAR** the field (`delete session.info[key]`,
///   `v0.9.2 broker/broker.ts:923`), and count that as a change only if it was set. pi's guard is
///   `if (session.info.contextPct !== undefined)`, so clearing an already-absent field is a no-op
///   and must NOT trigger a `presence_update` broadcast.
/// * key present and a number — set it, counting a change only if it differs (`:926-929`).
///
/// Anything else is unreachable: `handle_presence` has already returned
/// [`FrameResult::protocol_error`] for it, matching pi's `throw` at `:924`/`:934`/`:944`. The arm
/// is written as a no-op rather than a `panic!`/`unreachable!` so a future refactor that drops the
/// pre-validation degrades to "ignored" instead of taking the whole broker down.
fn apply_presence_context(
    slot: &mut Option<serde_json::Number>,
    incoming: Option<&serde_json::Value>,
) -> bool {
    match incoming {
        None => false,
        Some(serde_json::Value::Null) => slot.take().is_some(),
        Some(serde_json::Value::Number(n)) => {
            if slot.as_ref() == Some(n) {
                false
            } else {
                *slot = Some(n.clone());
                true
            }
        }
        Some(_) => false,
    }
}

/// Schedule the 5 s auto-shutdown check (`scheduleShutdownCheck`, `broker.ts:286-296`). Only one is
/// ever pending; a `register` in the window bumps `shutdown_gen`, making the pending check stale.
fn schedule_shutdown_check(state: &Arc<Mutex<BrokerState>>) {
    let mut g = lock(state);
    if g.shutdown_scheduled {
        return;
    }
    g.shutdown_scheduled = true;
    let generation = g.shutdown_gen;
    let shutdown = g.shutdown.clone();
    let task_state = state.clone();
    // The handle is installed under the SAME lock the flag was set under, so `handle_register`
    // never observes `shutdown_scheduled == true` with an empty `shutdown_task` slot. The task's
    // first action is a 5 s sleep, so it cannot contend for this guard.
    g.shutdown_task = Some(tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(SHUTDOWN_DELAY_MS)).await;
        let empty_and_current = {
            let mut g = lock(&task_state);
            g.shutdown_scheduled = false;
            g.shutdown_task = None;
            g.shutdown_gen == generation && g.sessions.is_empty()
        };
        if empty_and_current {
            tracing::info!("no sessions connected, shutting down");
            shutdown.notify_one();
        }
    }));
}

/// The per-connection writer task: drain queued frames to the socket, then half-close on EOF.
async fn writer_task(
    mut write_half: crate::transport::stream::BrokerWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
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
    // JS-lenient: an overflowing numeric literal must not kill the whole frame — see
    // `framing::from_frame_slice`.
    let value: serde_json::Value = match crate::transport::framing::from_frame_slice(payload) {
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
    mut read_half: crate::transport::stream::BrokerReadHalf,
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
        g.on_connection_closed(conn_id, &session_id, now_ms())
    };
    if did_leave {
        schedule_shutdown_check(&state);
    }
    drop(self_tx);
}

/// Wire one accepted connection: split it, spawn its writer + reader, and register it.
fn spawn_connection(
    conn_id: u64,
    stream: crate::transport::stream::BrokerStream,
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
fn shutdown_broker(
    state: &Arc<Mutex<BrokerState>>,
    listen_target: &crate::transport::target::BrokerConnectTarget,
    pid_path: &std::path::Path,
) {
    {
        let mut g = lock(state);
        for (_id, h) in g.connections.drain() {
            h.close.notify_one();
        }
        g.sessions.clear();
        g.session_order.clear();
        g.ask_edges.clear();
        // `shutdown()` clears every routing table, not just the sessions
        // (`v0.10.1 broker/broker.ts:1411-1415`). Parked mail is in-memory only upstream too — a
        // broker restart loses it by design, which is why `MAILBOX_MESSAGE_RETENTION_MS` is a
        // liveness bound rather than a durability promise.
        g.message_receipt_routes.clear();
        g.disconnected_sessions.clear();
        g.mailbox_messages.clear();
        g.unregistered.clear();
    }
    // `unlinkSync(LISTEN_TARGET)` guarded by
    // `typeof LISTEN_TARGET === "string" && process.platform !== "win32"`
    // (`v0.10.1 broker/broker.ts:1416-1418`) — a named pipe has no filesystem entry to remove.
    listener::unlink_stale_endpoint(listen_target);
    let _ = std::fs::remove_file(pid_path);
}

/// The `cyrup __intercom-broker` entrypoint (`new IntercomBroker().start()`, `broker.ts:636`).
/// Binds the listen target (Unix socket / Windows named pipe, [`listener::BrokerListener`]), writes
/// the pid file, runs the accept loop, and shuts down on SIGTERM/SIGINT or the 5 s idle
/// auto-shutdown. Returns once the endpoint + runtime files are cleaned up.
///
/// # Errors
/// Returns an I/O error if the intercom dir cannot be created or the listen target cannot be bound.
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
    // `const LISTEN_TARGET = getBrokerListenTarget();` (`v0.9.2 broker/broker.ts:26`) — the socket
    // path on POSIX, the `\\.\pipe\cyrup-intercom-<agent dir>` name on Windows, or the loopback-TCP
    // endpoint under the Windows-only opt-in (`broker/paths.ts:107-116`). This replaces a direct
    // `paths::broker_socket_path(...)` read, which hard-coded the POSIX arm.
    let listen_target = crate::transport::target::broker_listen_target(&agent_dir);
    let pid_path = paths::broker_pid_path(&intercom_dir);

    // Claim the runtime BEFORE touching anything in it (`assertNoLiveBroker(PID_PATH)`,
    // `v0.9.2 broker/broker.ts:231`, sitting between `ensureIntercomRuntimeDir` at `:230` and the
    // stale-socket unlink at `:233-238`). A second broker must DECLINE while an incumbent is alive
    // rather than unlink its socket and bind its own: the incumbent keeps every connection it has
    // already accepted (the unlinked inode outlives its name) but is unreachable to new clients, so
    // the theft silently partitions the session graph instead of failing. Only a *live* pid refuses
    // — a stale `broker.pid` left by a SIGKILLed broker is still reclaimable, or a crash would wedge
    // intercom until a human deleted the file. See `broker::runtime_claim`.
    runtime_claim::assert_no_live_broker(&pid_path)?;

    // Unlink a stale socket left by a crashed broker (`v0.9.2 broker/broker.ts:233-238`;
    // `v0.7.0 broker/broker.ts:143-148`), under upstream's own
    // `typeof LISTEN_TARGET === "string" && platform !== "win32"` guard (`:116`).
    listener::unlink_stale_endpoint(&listen_target);
    // CYRUP-DELTA (`v0.9.2 broker/broker.ts:239` `net.createServer().listen(LISTEN_TARGET)`, and
    // `broker/paths.ts:65-74`, which has no length guard either): upstream loses the reason the same
    // way, so this is a shared robustness gap, not a parity divergence. What is added here is only
    // the DIAGNOSTIC — `sockaddr_un.sun_path` is 104 bytes on macOS and 108 on Linux, so a deep
    // `HOME`/`CYRUP_AGENT_DIR` makes this bind fail with a bare "path must be shorter than SUN_LEN"
    // that names neither the limit's cause nor the path. Naming both here is what makes the parent's
    // captured-stderr message (`transport::spawn::BrokerStderrTail`) actionable.
    let endpoint = describe_listen_target(&listen_target);
    let mut listener = listener::BrokerListener::bind(&listen_target).await.map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("failed to bind the intercom broker endpoint at {endpoint} ({} bytes): {e}", endpoint.len()),
        )
    })?;
    if let crate::transport::target::BrokerConnectTarget::Socket(path) = &listen_target {
        // `restrictIntercomRuntimeFile(LISTEN_TARGET)` for the string arm (`broker.ts:128-130`);
        // itself a no-op off POSIX (`paths.ts:128-135`).
        let _ = paths::restrict_intercom_runtime_file(path);
    }
    std::fs::write(&pid_path, std::process::id().to_string())?;
    let _ = paths::restrict_intercom_runtime_file(&pid_path);
    tracing::info!(pid = std::process::id(), endpoint = %endpoint, "intercom broker started");

    let shutdown = Arc::new(Notify::new());
    let state = Arc::new(Mutex::new(BrokerState::new(ask_timeout, shutdown.clone())));
    let mut next_conn_id: u64 = 0;

    // `process.on("SIGTERM"|"SIGINT", () => this.shutdown())` (`broker.ts:181-182`).
    //
    // # [CYRUP-DELTA] — the terminate signal is per-platform because the OS is
    //
    // Upstream symbol: `broker.ts:181-182`. Node synthesises a `SIGTERM`/`SIGINT` listener on every
    // platform, but on Windows there are no POSIX signals underneath: libuv raises `SIGINT` from a
    // console Ctrl-C and simply never raises `SIGTERM` (`taskkill` without `/F` delivers a console
    // CTRL_CLOSE/CTRL_SHUTDOWN event instead). `tokio::signal::unix` does not exist off POSIX at
    // all, so the same intent is expressed with the platform's own events: Ctrl-C everywhere, plus
    // SIGTERM on POSIX and the console close/shutdown controls on Windows. The observable behaviour
    // — a polite terminate reaches `shutdown_broker`, so the pid file and (on POSIX) the socket are
    // removed rather than orphaned — is the same one upstream gets.
    let mut terminate = TerminateSignal::install()?;

    loop {
        tokio::select! {
            () = shutdown.notified() => break,
            _ = tokio::signal::ctrl_c() => break,
            () = terminate.recv() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok(stream) => {
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
    shutdown_broker(&state, &listen_target, &pid_path);
    Ok(())
}

/// The listen target rendered for diagnostics — the socket path / pipe name, or `host:port`.
fn describe_listen_target(target: &crate::transport::target::BrokerConnectTarget) -> String {
    match target {
        crate::transport::target::BrokerConnectTarget::Socket(path) => path.display().to_string(),
        crate::transport::target::BrokerConnectTarget::Tcp(e) => format!("{}:{}", e.host, e.port),
    }
}

/// The platform's "please terminate" event, standing in for upstream's `process.on("SIGTERM")`
/// (`broker.ts:181`). See the CYRUP-DELTA at its installation site in [`run`].
struct TerminateSignal {
    #[cfg(unix)]
    sigterm: tokio::signal::unix::Signal,
    #[cfg(windows)]
    close: tokio::signal::windows::CtrlClose,
    #[cfg(windows)]
    shutdown: tokio::signal::windows::CtrlShutdown,
}

impl TerminateSignal {
    /// Register the handler(s). Errors propagate exactly as the old bare
    /// `tokio::signal::unix::signal(...)?` did.
    fn install() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                sigterm: tokio::signal::unix::signal(
                    tokio::signal::unix::SignalKind::terminate(),
                )?,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                close: tokio::signal::windows::ctrl_close()?,
                shutdown: tokio::signal::windows::ctrl_shutdown()?,
            })
        }
    }

    /// Resolve on the first terminate-shaped event.
    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            self.sigterm.recv().await;
        }
        #[cfg(windows)]
        {
            tokio::select! {
                _ = self.close.recv() => {}
                _ = self.shutdown.recv() => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use serde_json::json;

    fn make_state() -> BrokerState {
        BrokerState::new(30_000, Arc::new(Notify::new()))
    }

    /// `String(msg.namespace || "")`, the coercion pi applies to the raw `namespace` before it has
    /// been type-checked (`v0.9.2 broker/broker.ts:1371,1382`). Every case below is what node
    /// prints for the same expression.
    #[test]
    fn js_string_or_empty_matches_the_js_coercion() {
        // `||` short-circuits on every falsy value.
        assert_eq!(js_string_or_empty(None), "");
        assert_eq!(js_string_or_empty(Some(&json!(null))), "");
        assert_eq!(js_string_or_empty(Some(&json!(false))), "");
        assert_eq!(js_string_or_empty(Some(&json!(0))), "");
        assert_eq!(js_string_or_empty(Some(&json!(0.0))), "");
        assert_eq!(js_string_or_empty(Some(&json!(""))), "");
        // Truthy values go through `ToString`.
        assert_eq!(js_string_or_empty(Some(&json!("ns"))), "ns");
        assert_eq!(js_string_or_empty(Some(&json!(42))), "42");
        assert_eq!(js_string_or_empty(Some(&json!(-7))), "-7");
        assert_eq!(js_string_or_empty(Some(&json!(42.5))), "42.5");
        // `1.0` is the integer `1` in JS; serde would otherwise print "1.0".
        assert_eq!(js_string_or_empty(Some(&json!(1.0_f64))), "1");
        assert_eq!(js_string_or_empty(Some(&json!(true))), "true");
        assert_eq!(js_string_or_empty(Some(&json!({"a": 1}))), "[object Object]");
        // `Array.prototype.join(",")`: null elements render empty and nesting flattens.
        assert_eq!(js_string_or_empty(Some(&json!([1, 2]))), "1,2");
        assert_eq!(js_string_or_empty(Some(&json!([null]))), "");
        assert_eq!(js_string_or_empty(Some(&json!([[1, 2], 3]))), "1,2,3");
        assert_eq!(js_string_or_empty(Some(&json!([{}]))), "[object Object]");
        // A non-empty array is truthy even when it joins to "" — `String([null] || "")` is `""`
        // via `join`, not via the `||`, and both paths agree here.
        assert_eq!(js_string_or_empty(Some(&json!([]))), "");
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
        assert_eq!(listed, joined, "`list` must report sessions in join order, like pi's Map");

        // An identity takeover is `this.sessions.set(id, …)` on an EXISTING key, which in JS keeps
        // that key's original position (`broker.ts:376`) — it must not jump to the back.
        let mut sid = None;
        register(&mut state, 900, &mut sid, "session-3");
        let after_takeover: Vec<String> = state.session_infos().into_iter().map(|s| s.id).collect();
        assert_eq!(after_takeover, joined, "a re-register must keep the session's original position");

        // `this.sessions.delete(id)` drops it from the order and leaves the rest intact.
        let mut sid = Some("session-7".to_string());
        state.handle_unregister(7, &make_tx(), &mut sid, 0);
        let expected: Vec<String> = joined.iter().filter(|id| *id != "session-7").cloned().collect();
        let after_leave: Vec<String> = state.session_infos().into_iter().map(|s| s.id).collect();
        assert_eq!(after_leave, expected, "a departure must not disturb the surviving join order");
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

#[cfg(test)]
mod presence_context_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use serde_json::json;

    /// R4 — the `presence` tri-state, arm by arm (`v0.9.2 broker/broker.ts:921-950`). The
    /// no-op-clear arm (`:923`'s `if (session.info.contextPct !== undefined)`) is asserted here
    /// rather than over the socket because `PRESENCE_HEARTBEAT_MS` is 1 s, so "no broadcast" is not
    /// a claim a live probe can make without racing the heartbeat.
    #[test]
    fn apply_presence_context_matches_pis_tristate() {
        // undefined — untouched, no change.
        let mut slot = Some(serde_json::Number::from(42));
        assert!(!apply_presence_context(&mut slot, None));
        assert_eq!(slot, Some(serde_json::Number::from(42)));

        // null on a SET field — cleared, and that IS a change (`v0.9.2 broker/broker.ts:923`).
        assert!(apply_presence_context(&mut slot, Some(&json!(null))));
        assert_eq!(slot, None);

        // null on an ALREADY-ABSENT field — still cleared, but NOT a change, so it must not
        // trigger a `presence_update` broadcast.
        assert!(!apply_presence_context(&mut slot, Some(&json!(null))));
        assert_eq!(slot, None);

        // a number on an absent field — set, and a change.
        assert!(apply_presence_context(&mut slot, Some(&json!(7))));
        assert_eq!(slot, Some(serde_json::Number::from(7)));

        // the SAME number again — set, but not a change (`v0.9.2 broker/broker.ts:926`).
        assert!(!apply_presence_context(&mut slot, Some(&json!(7))));

        // a different number — a change.
        assert!(apply_presence_context(&mut slot, Some(&json!(8))));
        assert_eq!(slot, Some(serde_json::Number::from(8)));

        // a fractional number is a `number` upstream too, so it is accepted, not coerced.
        assert!(apply_presence_context(&mut slot, Some(&json!(8.5))));
        assert_eq!(slot, Some(serde_json::Number::from_f64(8.5).unwrap()));
    }

    /// The whole handler, driven through `handle_frame`: a wrong-typed context field destroys the
    /// connection (`v0.9.2 broker/broker.ts:924,934,944`) while `null` and a number do not
    /// (`:922-923,926-929`) — the two halves that must not be collapsed into one rule.
    #[test]
    fn handle_presence_rejects_non_number_context_but_accepts_null_and_numbers() {
        fn drive(patch: serde_json::Value) -> FrameOutcome {
            let mut state = BrokerState::new(30_000, Arc::new(Notify::new()));
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
            let mut frame = json!({ "type": "presence" });
            if let (Some(dst), Some(src)) = (frame.as_object_mut(), patch.as_object()) {
                for (k, v) in src {
                    dst.insert(k.clone(), v.clone());
                }
            }
            state.handle_frame(1, &tx, &frame, &mut sid, 2_000).outcome
        }

        for key in ["contextPct", "contextTokens", "contextWindow"] {
            for bad in [json!("42"), json!({}), json!([]), json!(true)] {
                assert!(
                    matches!(drive(json!({ key: bad })), FrameOutcome::ProtocolError),
                    "`presence.{key} = {bad}` must destroy the connection"
                );
            }
            // POSITIVE CONTROL: null (clear) and a number (set) are both legal here.
            for good in [json!(null), json!(0), json!(42), json!(99.5)] {
                assert!(
                    matches!(drive(json!({ key: good })), FrameOutcome::Continue),
                    "`presence.{key} = {good}` must be served, not disconnected"
                );
            }
        }
    }

    /// ICOM-014 — every `throw new Error("Invalid presence …")` upstream is nested INSIDE
    /// `if (session?.socket === socket)` (`v0.10.1 broker/broker.ts:763-805`, guard at `:765`), so a
    /// NON-OWNING socket's malformed presence is IGNORED, not fatal.
    ///
    /// Red against the pre-fix ordering: the type-check loops ran before the ownership filter, so a
    /// superseded socket sending a late `{"name": 5}` had its connection destroyed as a protocol
    /// error. The reconnect ladder deliberately re-offers the previous session id, so a takeover
    /// race is a live path.
    #[test]
    fn a_non_owning_socket_s_malformed_presence_is_ignored_not_fatal() {
        let mut state = BrokerState::new(30_000, Arc::new(Notify::new()));
        let (tx_owner, _rx_owner) = tokio::sync::mpsc::unbounded_channel();
        let (tx_loser, _rx_loser) = tokio::sync::mpsc::unbounded_channel();

        // conn 1 registers as `s1`, then conn 2 takes the id over — conn 1 is now the LOSER.
        let mut sid_loser = None;
        let reg = json!({
            "type": "register", "sessionId": "s1",
            "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
        });
        state.handle_frame(1, &tx_loser, &reg, &mut sid_loser, 1_000);
        let mut sid_owner = None;
        state.handle_frame(2, &tx_owner, &reg, &mut sid_owner, 1_001);

        // The loser still believes it is `s1`, and sends a malformed presence.
        let bad = json!({ "type": "presence", "name": 5 });
        assert!(
            matches!(state.handle_frame(1, &tx_loser, &bad, &mut sid_loser, 2_000).outcome, FrameOutcome::Continue),
            "a non-owning socket's malformed presence must be ignored, not a protocol error"
        );

        // POSITIVE CONTROL: the OWNER sending the same frame is still fatal
        // (`v0.10.1 broker/broker.ts:766-768`).
        assert!(
            matches!(state.handle_frame(2, &tx_owner, &bad, &mut sid_owner, 2_001).outcome, FrameOutcome::ProtocolError),
            "the owner's malformed presence is still fatal"
        );
    }

    /// ICOM-041 — `runtimeFallbackAlias` is a BOOLEAN checked inside the ownership block
    /// (`v0.10.1 broker/broker.ts:779-787`) and applied to the stored `SessionInfo`.
    #[test]
    fn presence_carries_runtime_fallback_alias() {
        let mut state = BrokerState::new(30_000, Arc::new(Notify::new()));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut sid = None;
        state.handle_frame(
            1,
            &tx,
            &json!({
                "type": "register", "sessionId": "s1",
                "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0,
                             "name": "subagent-chat-0192", "runtimeFallbackAlias": true },
            }),
            &mut sid,
            1_000,
        );
        // `v0.10.1 broker/broker.ts:358` copies it off the registration.
        assert_eq!(
            state.sessions.get("s1").and_then(|s| s.info.runtime_fallback_alias),
            Some(true),
            "register must carry the flag onto the stored SessionInfo"
        );

        // A presence frame flips it (`:779-787`).
        assert!(matches!(
            state
                .handle_frame(1, &tx, &json!({ "type": "presence", "runtimeFallbackAlias": false }), &mut sid, 2_000)
                .outcome,
            FrameOutcome::Continue
        ));
        assert_eq!(state.sessions.get("s1").and_then(|s| s.info.runtime_fallback_alias), Some(false));

        // A non-boolean is fatal, like every other presence type check.
        assert!(matches!(
            state
                .handle_frame(1, &tx, &json!({ "type": "presence", "runtimeFallbackAlias": "yes" }), &mut sid, 3_000)
                .outcome,
            FrameOutcome::ProtocolError
        ));
    }

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

    /// Local copies of `mod tests`' fixtures: a sibling test module cannot reach that module's
    /// private items.
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
                "cwd": "/tmp", "model": "test-model", "pid": 1, "startedAt": 0, "lastActivity": 0,
            }
        });
        let result = state.handle_register(conn_id, &tx, &value, session_id, 0);
        assert!(matches!(result.outcome, FrameOutcome::Continue));
    }

    /// Decode every queued frame on `rx` as JSON, dropping the 4-byte length prefix.
    fn payloads(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            out.push(
                serde_json::from_slice(frame.get(4..).unwrap_or_default())
                    .expect("a broker frame is JSON"),
            );
        }
        out
    }

    /// Register `id` on `conn_id` with an explicit name + cwd, so the mailbox identity rules
    /// (`v0.10.1 broker/broker.ts:1039-1048`) have something to match on.
    fn register_named(
        state: &mut BrokerState,
        conn_id: u64,
        session_id: &mut Option<String>,
        tx: &UnboundedSender<Vec<u8>>,
        id: &str,
        name: &str,
        cwd: &str,
        now: u64,
    ) {
        let value = json!({
            "type": "register",
            "sessionId": id,
            "session": {
                "name": name, "cwd": cwd, "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0,
            }
        });
        let result = state.handle_register(conn_id, tx, &value, session_id, now);
        assert!(matches!(result.outcome, FrameOutcome::Continue));
    }

    fn send_frame(
        state: &mut BrokerState,
        conn_id: u64,
        tx: &UnboundedSender<Vec<u8>>,
        sid: &mut Option<String>,
        to: &str,
        message: serde_json::Value,
        now: u64,
    ) {
        state.handle_frame(conn_id, tx, &json!({ "type": "send", "to": to, "message": message }), sid, now);
    }

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

    /// ICOM-005 — `register` must NULL the pending auto-shutdown handle
    /// (`v0.10.1 broker/broker.ts:378-381`), not merely bump the generation. Red against the pre-fix
    /// code: `shutdown_scheduled` stayed `true`, so the NEXT disconnect's `schedule_shutdown_check`
    /// early-returned and the re-arm was lost, leaving an idle broker alive forever.
    #[tokio::test]
    async fn a_register_clears_the_pending_shutdown_so_a_later_disconnect_can_re_arm() {
        let state: Arc<Mutex<BrokerState>> =
            Arc::new(Mutex::new(BrokerState::new(30_000, Arc::new(Notify::new()))));
        // t=0: the last session left → a check is armed.
        schedule_shutdown_check(&state);
        assert!(lock(&state).shutdown_scheduled, "armed");

        // t=1: a register lands inside the 5 s window.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut sid = None;
        lock(&state).handle_frame(
            1,
            &tx,
            &json!({
                "type": "register", "sessionId": "s1",
                "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
            }),
            &mut sid,
            1_000,
        );
        assert!(!lock(&state).shutdown_scheduled, "a register cancels the pending check");
        assert!(lock(&state).shutdown_task.is_none(), "and drops its handle");

        // t=2: that session disconnects → the check must arm AGAIN.
        lock(&state).sessions.clear();
        schedule_shutdown_check(&state);
        assert!(
            lock(&state).shutdown_scheduled,
            "the re-arm must not be swallowed by a stale `shutdown_scheduled`"
        );
    }
}

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
pub mod runtime_claim;

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
    BrokerMessage, ExtensionCapability, HealthMessage, Message, MessageReceipt, PROTOCOL_NAME,
    PROTOCOL_VERSION, SessionInfo, SessionRegistration, now_ms,
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
    /// Registered session ids in **join order**. `broker.ts:133` holds the sessions in a JS `Map`,
    /// which iterates in insertion order, so every consumer of the map — the `list` reply
    /// (`broker.ts:408`), presence broadcasts and name resolution — observes a stable join order.
    /// A `std::collections::HashMap` has no such guarantee, so the order is tracked alongside it,
    /// the way `unregistered` already tracks connection insertion order below.
    session_order: Vec<String>,
    ask_edges: HashMap<String, AskEdge>,
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

    /// `Array.from(this.sessions.values()).map(s => s.info)` (`broker.ts:408`) — join-ordered,
    /// because pi's `Map` iterates in insertion order and neither `index.ts`'s `list` handler nor
    /// `ui/session-list.ts` re-sorts the reply.
    fn session_infos(&self) -> Vec<SessionInfo> {
        self.sessions_in_order().map(|(_, s)| s.info.clone()).collect()
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
            self.remove_session(sid);
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
            "message_receipt" => Self::handle_message_receipt(value),
            "cancel_message" => Self::handle_cancel_message(self_tx, value),
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

    /// `case "message_receipt"` (`v0.9.2 broker/broker.ts:801-820`).
    ///
    /// pi validates the receipt with `isMessageReceipt()` — a bad one THROWS, i.e. destroys the
    /// connection — then looks the message up in `messageReceiptRoutes` and forwards the receipt to
    /// the original sender only if the route says this session was the receiver.
    ///
    /// cyrup has no `messageReceiptRoutes` table yet (it is populated at
    /// `v0.9.2 broker/broker.ts:698`, alongside the message-lifecycle work this crate has not
    /// ported), so every lookup misses and pi's own `route === undefined` branch applies: validate,
    /// then fall through the `if` and `break` without writing anything. Shape validation is kept
    /// because dropping it would make cyrup LOOSER than pi on a frame arriving over a socket other
    /// sessions can reach.
    fn handle_message_receipt(value: &serde_json::Value) -> FrameResult {
        let Some(receipt) = value.get("receipt") else {
            return FrameResult::protocol_error();
        };
        if serde_json::from_value::<MessageReceipt>(receipt.clone()).is_err() {
            // `throw new Error("Invalid message_receipt message")` (`v0.9.2 broker/broker.ts:807`).
            return FrameResult::protocol_error();
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

    /// `case "cancel_message"` (`v0.9.2 broker/broker.ts:822-869`).
    ///
    /// A non-string `messageId` throws upstream (`:825-827`) and is fatal here for the same reason.
    /// Past that, pi searches its mailbox and then `messageReceiptRoutes`; cyrup has neither table,
    /// so both misses land on pi's `route?.from !== currentId` branch — `delivery_failed` with
    /// pi's exact reason (`v0.9.2 broker/broker.ts:842-848`). Answering matters: pi's
    /// `cancelMessage()` returns a promise settled only by `delivered`/`delivery_failed`
    /// (`v0.9.2 broker/client.ts:738`), so a silent drop would hang the caller instead.
    fn handle_cancel_message(
        self_tx: &UnboundedSender<Vec<u8>>,
        value: &serde_json::Value,
    ) -> FrameResult {
        let Some(message_id) = value.get("messageId").and_then(|v| v.as_str()) else {
            return FrameResult::protocol_error();
        };
        send_msg(self_tx, &BrokerMessage::DeliveryFailed {
            message_id: message_id.to_string(),
            reason: "Message cannot be cancelled by this session".to_string(),
        });
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
            self.remove_session(&sid);
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
            // `v0.10.1 broker/broker.ts:631-638` (v0.10.0): a BLOCKING ask against a target the
            // broker cannot route gets a reason that says so, because nothing queues it — the
            // caller must switch to `send` or retry after the peer reconnects. A bare
            // `Session not found` told the model nothing actionable, so it retried the same
            // blocking ask.
            let reason = if message.expects_reply == Some(true) {
                "Target session is not currently connected; blocking asks are not queued"
            } else {
                "Session not found"
            };
            send_msg(self_tx, &BrokerMessage::DeliveryFailed {
                message_id: message.id.clone(),
                reason: reason.to_string(),
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
            send_msg(&target.tx, &BrokerMessage::Message { from: from_info, message: delivered });
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
        g.session_order.clear();
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
    // `v0.7.0 broker/broker.ts:143-148`).
    let _ = std::fs::remove_file(&socket_path);
    // CYRUP-DELTA (`v0.9.2 broker/broker.ts:239` `net.createServer().listen(LISTEN_TARGET)`, and
    // `broker/paths.ts:65-74`, which has no length guard either): upstream loses the reason the same
    // way, so this is a shared robustness gap, not a parity divergence. What is added here is only
    // the DIAGNOSTIC — `sockaddr_un.sun_path` is 104 bytes on macOS and 108 on Linux, so a deep
    // `HOME`/`CYRUP_AGENT_DIR` makes this bind fail with a bare "path must be shorter than SUN_LEN"
    // that names neither the limit's cause nor the path. Naming both here is what makes the parent's
    // captured-stderr message (`transport::spawn::BrokerStderrTail`) actionable.
    let listener = UnixListener::bind(&socket_path).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!(
                "failed to bind the intercom broker socket at {} ({} bytes): {e}",
                socket_path.display(),
                socket_path.as_os_str().len()
            ),
        )
    })?;
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
        state.handle_unregister(7, &make_tx(), &mut sid);
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

    /// ICOM-045's broker half (`v0.10.1 broker/broker.ts:631-638`, v0.10.0): an unroutable target
    /// gets a reason that says blocking asks are not queued, so the model switches to `send` or
    /// retries instead of re-issuing the same blocking ask.
    #[test]
    fn an_unroutable_blocking_ask_is_refused_with_the_not_queued_reason() {
        fn drive(expects_reply: bool) -> String {
            let mut state = BrokerState::new(30_000, Arc::new(Notify::new()));
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
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
        assert_eq!(drive(true), "Target session is not currently connected; blocking asks are not queued");
        // POSITIVE CONTROL: a non-blocking send keeps pi's original wording.
        assert_eq!(drive(false), "Session not found");
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

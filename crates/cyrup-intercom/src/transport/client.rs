//! The per-session [`IntercomClient`] — a port of `pi-intercom/broker/client.ts:119-580`.
//!
//! Connect → immediately `register` → resolve on the `registered` frame (10 s timeout,
//! `client.ts:182-233`). `send` correlates a `delivered`/`delivery_failed` ack by `message.id`
//! (10 s, `client.ts:504-549`); `list_sessions` correlates a `sessions` reply by `requestId` (5 s,
//! `client.ts:469-502`). Inbound `message`/`session_joined`/`session_left`/`presence_update`/`error`
//! frames fan out on a `broadcast` channel ([`IntercomClient::subscribe`]) — the Rust analog of pi's
//! `EventEmitter`. There is **no automatic reconnect** (`client.ts:214-251`); a stable identity
//! across reconnect is achieved by re-`register`ing with the same `session_id` (broker takeover).
//!
//! [`IntercomClient::connect_target`] speaks all three of pi-intercom's transports (the connection
//! itself lives in [`crate::transport::stream`]); [`IntercomClient::connect`] is the socket/pipe
//! shorthand. Over the opt-in TCP transport the `register` frame additionally carries the endpoint's
//! `stateId` credential (`client.ts:280-285`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, broadcast, mpsc, oneshot};
use tokio::task::AbortHandle;

use crate::error::{IntercomError, Result};
use crate::transport::framing::{FrameReader, encode_json};
use crate::transport::protocol::{
    Attachment, BrokerMessage, ClientMessage, Message, MessageContent, SessionInfo,
    SessionRegistration, now_ms,
};
use crate::transport::stream::{BrokerReadHalf, BrokerStream, BrokerWriteHalf};
use crate::transport::target::BrokerConnectTarget;

/// The `registered`-frame + `list`/`send` correlation timeouts (`client.ts:182,492,538`).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const LIST_TIMEOUT: Duration = Duration::from_secs(5);
/// The `disconnect()` forced-destroy watchdog (`client.ts:452-454`).
const DISCONNECT_TIMEOUT: Duration = Duration::from_millis(2000);
const READ_BUF: usize = 16 * 1024;

/// Options for [`IntercomClient::send`] (`SendOptions`, `client.ts:8-14`).
#[derive(Clone, Debug, Default)]
pub struct SendOptions {
    /// The message text.
    pub text: String,
    /// Optional structured attachments.
    pub attachments: Option<Vec<Attachment>>,
    /// The message id this is a reply to, if any.
    pub reply_to: Option<String>,
    /// Whether a reply is expected (records an ask edge on the broker).
    pub expects_reply: Option<bool>,
    /// A caller-supplied message id (the ask `questionId`); a fresh UUID is minted when absent.
    pub message_id: Option<String>,
}

/// The result of a [`IntercomClient::send`] (`SendResult`, `client.ts:16-20`).
#[derive(Clone, Debug)]
pub struct SendResult {
    /// The message id.
    pub id: String,
    /// Whether the broker confirmed delivery.
    pub delivered: bool,
    /// The `delivery_failed` reason, when `!delivered`.
    pub reason: Option<String>,
}

/// A fanned-out inbound broker event (the Rust analog of pi's `IntercomClient` `EventEmitter`
/// events, `client.ts:344,387,396,405,417`).
#[derive(Clone, Debug)]
pub enum InboundEvent {
    /// A message routed from another session (`client.ts:338-345`).
    Message {
        /// The sender's session info.
        from: SessionInfo,
        /// The delivered message. Boxed because `Message` carries the full v0.9.2 envelope
        /// (`v0.9.2 types.ts:24-40`) plus its `#[serde(flatten)]` spread capture, and this enum is
        /// fanned out over a `broadcast` channel that clones it once per subscriber — an unboxed
        /// `Message` would make every `SessionLeft(String)` cost the same as a full message.
        message: Box<Message>,
    },
    /// A session joined (`client.ts:382-388`).
    SessionJoined(SessionInfo),
    /// A session left (`client.ts:391-397`).
    SessionLeft(String),
    /// A presence change (`client.ts:400-406`).
    PresenceUpdate(SessionInfo),
    /// A broker-level error for this connection (`client.ts:409-418`).
    Error(String),
    /// The connection closed (`client.ts:227-229`).
    Disconnected(String),
}

type PendingSend = oneshot::Sender<SendResult>;
type PendingList = oneshot::Sender<std::result::Result<Vec<SessionInfo>, String>>;

/// A command to the per-connection writer task: emit a frame, or half-close the socket after the
/// frames queued ahead of it (`disconnect` writes `unregister` then `socket.end()`, `client.ts:459-461`).
enum WriterCmd {
    Frame(Vec<u8>),
    Close,
}

struct ClientInner {
    session_id: Mutex<Option<String>>,
    writer: mpsc::UnboundedSender<WriterCmd>,
    pending_sends: Mutex<HashMap<String, PendingSend>>,
    pending_lists: Mutex<HashMap<String, PendingList>>,
    events: broadcast::Sender<InboundEvent>,
    connected: AtomicBool,
    /// Set the moment `disconnect()` is called (`this.disconnecting`, `client.ts:124,432`); gates
    /// `is_connected()`/`cancel_ask`/`update_presence` exactly like pi's socket-liveness guards.
    disconnecting: AtomicBool,
    /// Sticky "has this connection ever completed registration" flag (pi's `connectionEstablished`
    /// closure var, `client.ts:194,236,244`) — gates whether a post-connect failure gets a distinct
    /// `error` event before the eventual `disconnected`.
    ever_registered: AtomicBool,
    /// Runs the teardown tail (fail pending + emit `Disconnected`) at most once, whichever of
    /// `read_task`/`writer_task` notices the failure first (mirrors the single `close` event pi's
    /// one socket object delivers to every listener).
    teardown_started: AtomicBool,
    /// The spawned `read_task`'s abort handle, used to force it down when a connect times out
    /// (`socket.destroy()`, `client.ts:189`) or a write fails (a write-side socket error tears down
    /// the whole duplex in pi, `client.ts:235-240`).
    read_abort: Mutex<Option<AbortHandle>>,
    /// Signalled once teardown has actually run, so `disconnect()` can await the real close
    /// (`client.ts:436-466`) instead of returning immediately.
    closed_notify: Notify,
}

impl ClientInner {
    /// Queue one frame to the writer; returns `false` if the writer channel is closed.
    fn send_frame(&self, frame: Vec<u8>) -> bool {
        self.writer.send(WriterCmd::Frame(frame)).is_ok()
    }
}

/// The teardown tail: fail every pending send/list, flip `connected`/`session_id`, and broadcast
/// `Disconnected` — run at most once per connection (`onClose`, `client.ts:214-233`).
fn teardown(inner: &Arc<ClientInner>, reason: String) {
    if inner.teardown_started.swap(true, Ordering::SeqCst) {
        return;
    }
    inner.connected.store(false, Ordering::SeqCst);
    *guard(&inner.session_id) = None;
    for (_, tx) in guard(&inner.pending_sends).drain() {
        let _ = tx.send(SendResult { id: String::new(), delivered: false, reason: Some(reason.clone()) });
    }
    for (_, tx) in guard(&inner.pending_lists).drain() {
        let _ = tx.send(Err(reason.clone()));
    }
    let _ = inner.events.send(InboundEvent::Disconnected(reason));
    inner.closed_notify.notify_one();
}

fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A per-session broker client.
pub struct IntercomClient {
    inner: Arc<ClientInner>,
}

impl IntercomClient {
    /// Connect to the broker over the Unix socket / named pipe at `socket_path` — the
    /// `typeof target === "string"` arm of [`Self::connect_target`], kept as the ergonomic form
    /// every existing call site already uses.
    ///
    /// # Errors
    /// [`IntercomError::Io`] if the socket cannot be connected; [`IntercomError::Client`] on a
    /// registration timeout / a pre-registration error / a connection closed before registration.
    pub async fn connect(
        socket_path: &Path,
        registration: SessionRegistration,
        session_id: Option<String>,
    ) -> Result<Self> {
        Self::connect_target(
            &BrokerConnectTarget::Socket(socket_path.to_path_buf()),
            registration,
            session_id,
        )
        .await
    }

    /// Connect to the broker over `target` — a Unix socket, a Windows named pipe, or the opt-in
    /// loopback TCP endpoint (`connectToBrokerTarget`, `client.ts:26-30,169-176`) — register
    /// `registration` (re-adopting `session_id` if `Some`), and resolve once the `registered` frame
    /// arrives (`connect`, `client.ts:164-293`).
    ///
    /// Over a TCP target the `register` frame additionally carries the endpoint's `stateId`
    /// (`client.ts:280-285`: `...(typeof target === "string" ? {} : { stateId: target.stateId })`),
    /// which the broker requires or it closes the connection with
    /// `Invalid intercom TCP endpoint credentials` (`broker.ts:263-266`). Over a socket/pipe target
    /// the field is **omitted**, not sent as null.
    ///
    /// # Errors
    /// [`IntercomError::Io`] if the target cannot be connected; [`IntercomError::Client`] on a
    /// registration timeout / a pre-registration error / a connection closed before registration.
    pub async fn connect_target(
        target: &BrokerConnectTarget,
        registration: SessionRegistration,
        session_id: Option<String>,
    ) -> Result<Self> {
        let state_id = target.state_id().map(str::to_string);
        let stream = BrokerStream::connect(target).await?;
        let (read_half, write_half) = stream.into_split();
        let (wtx, wrx) = mpsc::unbounded_channel::<WriterCmd>();
        let (events, _) = broadcast::channel::<InboundEvent>(256);

        let inner = Arc::new(ClientInner {
            session_id: Mutex::new(None),
            writer: wtx,
            pending_sends: Mutex::new(HashMap::new()),
            pending_lists: Mutex::new(HashMap::new()),
            events,
            connected: AtomicBool::new(false),
            disconnecting: AtomicBool::new(false),
            ever_registered: AtomicBool::new(false),
            teardown_started: AtomicBool::new(false),
            read_abort: Mutex::new(None),
            closed_notify: Notify::new(),
        });

        tokio::spawn(writer_task(write_half, wrx, inner.clone()));

        let (reg_tx, reg_rx) = oneshot::channel::<std::result::Result<String, String>>();
        let read_handle = tokio::spawn(read_task(read_half, inner.clone(), reg_tx));
        *guard(&inner.read_abort) = Some(read_handle.abort_handle());

        // Register immediately; the OS/tokio buffers the write until connected (client.ts:276-282).
        let register = ClientMessage::Register {
            session: registration,
            session_id,
            state_id,
        };
        if !inner.send_frame(encode_json(&register)?) {
            return Err(IntercomError::Client("writer closed before register".to_string()));
        }

        match tokio::time::timeout(CONNECT_TIMEOUT, reg_rx).await {
            Ok(Ok(Ok(sid))) => {
                *guard(&inner.session_id) = Some(sid);
                inner.connected.store(true, Ordering::SeqCst);
                inner.ever_registered.store(true, Ordering::SeqCst);
                Ok(Self { inner })
            }
            Ok(Ok(Err(msg))) => Err(IntercomError::Client(msg)),
            Ok(Err(_)) => Err(IntercomError::Client("connection closed before registration".to_string())),
            Err(_) => {
                // Destroy the socket + background tasks on timeout (client.ts:184-191) rather than
                // leaving them running past the reported error.
                if let Some(h) = guard(&inner.read_abort).take() {
                    h.abort();
                }
                let _ = inner.writer.send(WriterCmd::Close);
                Err(IntercomError::Client("connection timeout".to_string()))
            }
        }
    }

    /// This client's broker-assigned session id (`sessionId`, `client.ts:138-140`).
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        guard(&self.inner.session_id).clone()
    }

    /// Whether the client is connected + registered (`isConnected`, `client.ts:142-145`).
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.inner.disconnecting.load(Ordering::SeqCst)
            && self.inner.connected.load(Ordering::SeqCst)
            && guard(&self.inner.session_id).is_some()
    }

    /// Subscribe to inbound broker events (pi's `EventEmitter` `.on(...)`).
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<InboundEvent> {
        self.inner.events.subscribe()
    }

    /// Send a message to `to` and await the broker's `delivered`/`delivery_failed` ack, correlated by
    /// `message.id` (`send`, `client.ts:504-549`).
    ///
    /// # Errors
    /// [`IntercomError::Client`] on a send timeout or if the client disconnected mid-send.
    pub async fn send(&self, to: &str, options: SendOptions) -> Result<SendResult> {
        if !self.is_connected() {
            return Err(IntercomError::Client("not connected".to_string()));
        }
        let message_id = options
            .message_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let message = Message {
            id: message_id.clone(),
            timestamp: now_ms().into(),
            reply_to: options.reply_to,
            expects_reply: options.expects_reply,
            content: MessageContent { text: options.text, attachments: options.attachments, ..Default::default() },
            ..Default::default()
        };
        let (tx, rx) = oneshot::channel();
        guard(&self.inner.pending_sends).insert(message_id.clone(), tx);
        let frame = encode_json(&ClientMessage::Send { to: to.to_string(), message })?;
        if !self.inner.send_frame(frame) {
            guard(&self.inner.pending_sends).remove(&message_id);
            return Err(IntercomError::Client("client disconnected".to_string()));
        }
        match tokio::time::timeout(SEND_TIMEOUT, rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => {
                guard(&self.inner.pending_sends).remove(&message_id);
                Err(IntercomError::Client("client disconnected".to_string()))
            }
            Err(_) => {
                guard(&self.inner.pending_sends).remove(&message_id);
                Err(IntercomError::Client("send timeout".to_string()))
            }
        }
    }

    /// List all connected sessions, correlated by `requestId` (`listSessions`, `client.ts:469-502`).
    ///
    /// # Errors
    /// [`IntercomError::Client`] on a list timeout or if the client disconnected mid-list.
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        if !self.is_connected() {
            return Err(IntercomError::Client("not connected".to_string()));
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        guard(&self.inner.pending_lists).insert(request_id.clone(), tx);
        let frame = encode_json(&ClientMessage::List { request_id: request_id.clone() })?;
        if !self.inner.send_frame(frame) {
            guard(&self.inner.pending_lists).remove(&request_id);
            return Err(IntercomError::Client("client disconnected".to_string()));
        }
        match tokio::time::timeout(LIST_TIMEOUT, rx).await {
            Ok(Ok(Ok(sessions))) => Ok(sessions),
            Ok(Ok(Err(msg))) => Err(IntercomError::Client(msg)),
            Ok(Err(_)) => {
                guard(&self.inner.pending_lists).remove(&request_id);
                Err(IntercomError::Client("client disconnected".to_string()))
            }
            Err(_) => {
                guard(&self.inner.pending_lists).remove(&request_id);
                Err(IntercomError::Client("list sessions timeout".to_string()))
            }
        }
    }

    /// Best-effort cancel of an outstanding ask edge this session owns (`cancelAsk`,
    /// `client.ts:551-566`). A no-op once disconnecting has begun, or the connection is otherwise
    /// dead — mirrors pi's `disconnecting`/socket-liveness guards; no frame is ever attempted.
    pub fn cancel_ask(&self, message_id: &str) {
        if self.inner.disconnecting.load(Ordering::SeqCst) || !self.is_connected() {
            return;
        }
        if let Ok(frame) = encode_json(&ClientMessage::CancelAsk { message_id: message_id.to_string() }) {
            let _ = self.inner.send_frame(frame);
        }
    }

    /// Best-effort presence update (`updatePresence`, `client.ts:568-579`). Same guards as
    /// [`Self::cancel_ask`] (`client.ts:569-576`).
    pub fn update_presence(&self, name: Option<String>, status: Option<String>, model: Option<String>) {
        if self.inner.disconnecting.load(Ordering::SeqCst) || !self.is_connected() {
            return;
        }
        self.update_presence_with_context(name, status, model, None, None, None);
    }

    /// [`Self::update_presence`] plus the three context-usage fields
    /// (`v0.9.2 types.ts:86`, applied by the broker at `v0.9.2 broker/broker.ts:918-950`).
    ///
    /// Each context argument is a tri-state, exactly as the wire is: `None` omits the key (the
    /// broker leaves the field untouched), `Some(None)` sends an explicit `null` (the broker
    /// CLEARS the field — the right thing right after a compaction, when the value is unknown and
    /// carrying the stale-high one forward would be a lie), and `Some(Some(n))` sets it.
    ///
    /// WIRED: the production presence heartbeat calls this with a populated context on every
    /// agent/tool lifecycle edge — `IntercomExtension::sync_presence` reads
    /// `HostServices::context_usage()` through `IntercomExtension::current_context_usage`, pi's
    /// `client.updatePresence({ status: currentStatus(), ...currentContextUsage() })`
    /// (`v0.9.2 index.ts:842-848`). A peer's `/intercom` picker and `intercom({action:"list"})`
    /// therefore show this session's live `NN% ctx (used/window)`.
    pub fn update_presence_with_context(
        &self,
        name: Option<String>,
        status: Option<String>,
        model: Option<String>,
        context_pct: Option<Option<serde_json::Number>>,
        context_tokens: Option<Option<serde_json::Number>>,
        context_window: Option<Option<serde_json::Number>>,
    ) {
        if self.inner.disconnecting.load(Ordering::SeqCst) || !self.is_connected() {
            return;
        }
        if let Ok(frame) = encode_json(&ClientMessage::Presence {
            name,
            status,
            model,
            context_pct,
            context_tokens,
            context_window,
        }) {
            let _ = self.inner.send_frame(frame);
        }
    }

    /// Send `unregister` then half-close the socket (`disconnect`, `client.ts:426-467`). Eagerly
    /// fails every pending `send`/`list_sessions` the moment it is called
    /// (`this.failPending(new Error("Client disconnected"))`, `client.ts:434`) rather than waiting
    /// for the peer to notice — callers blocked in-flight see an immediate rejection instead of
    /// riding out their full 10s/5s timeout. A background watchdog then forces the socket down
    /// within 2s if the peer never closes it (`client.ts:452-454`).
    ///
    /// Kept synchronous (not `async`, unlike `client.ts`'s awaitable `disconnect()`) so every
    /// existing call site keeps compiling/behaving unchanged; the bounded forced-teardown still
    /// happens, just on a detached background task rather than being awaited by the caller.
    pub fn disconnect(&self) {
        if self.inner.disconnecting.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.connected.store(false, Ordering::SeqCst);

        let reason = "client disconnected".to_string();
        for (_, tx) in guard(&self.inner.pending_sends).drain() {
            let _ = tx.send(SendResult { id: String::new(), delivered: false, reason: Some(reason.clone()) });
        }
        for (_, tx) in guard(&self.inner.pending_lists).drain() {
            let _ = tx.send(Err(reason.clone()));
        }

        if let Ok(frame) = encode_json(&ClientMessage::Unregister) {
            let _ = self.inner.send_frame(frame);
        }
        let _ = self.inner.writer.send(WriterCmd::Close);

        let inner = self.inner.clone();
        tokio::spawn(async move {
            let notified = inner.closed_notify.notified();
            tokio::pin!(notified);
            tokio::select! {
                () = &mut notified => {}
                () = tokio::time::sleep(DISCONNECT_TIMEOUT) => {
                    if let Some(h) = guard(&inner.read_abort).take() {
                        h.abort();
                    }
                    // The abort short-circuits read_task's own teardown tail, so run it here
                    // (idempotent via `teardown_started`) exactly as `socket.destroy()` forcing
                    // `close` would (`client.ts:452-454`).
                    teardown(&inner, reason);
                }
            }
        });
    }
}

async fn writer_task(
    mut write_half: BrokerWriteHalf,
    mut rx: mpsc::UnboundedReceiver<WriterCmd>,
    inner: Arc<ClientInner>,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            WriterCmd::Frame(frame) => {
                if let Err(e) = write_half.write_all(&frame).await {
                    // Node's single socket object surfaces read- and write-side failures through
                    // the same `error` event (`client.ts:235-240`); a split write half can't make
                    // the read half notice on its own, so propagate the real error and force the
                    // whole connection down here instead of silently discarding it.
                    let reason = e.to_string();
                    if inner.ever_registered.load(Ordering::SeqCst) {
                        let _ = inner.events.send(InboundEvent::Error(reason.clone()));
                    }
                    if let Some(h) = guard(&inner.read_abort).take() {
                        h.abort();
                    }
                    teardown(&inner, reason);
                    break;
                }
            }
            WriterCmd::Close => break,
        }
    }
    let _ = write_half.shutdown().await;
}

/// The wire `type` tag for a [`BrokerMessage`], used only for the pre-registration
/// message-ordering error text (`Received ${type} before registered`, `client.ts:303`).
fn broker_message_kind(msg: &BrokerMessage) -> &'static str {
    match msg {
        BrokerMessage::Registered { .. } => "registered",
        BrokerMessage::Sessions { .. } => "sessions",
        BrokerMessage::Message { .. } => "message",
        BrokerMessage::PresenceUpdate { .. } => "presence_update",
        BrokerMessage::SessionJoined { .. } => "session_joined",
        BrokerMessage::SessionLeft { .. } => "session_left",
        BrokerMessage::Error { .. } => "error",
        BrokerMessage::Delivered { .. } => "delivered",
        BrokerMessage::DeliveryFailed { .. } => "delivery_failed",
        BrokerMessage::MessageReceipt { .. } => "message_receipt",
        BrokerMessage::MessageControl { .. } => "message_control",
        BrokerMessage::ExtensionOwner { .. } => "extension_owner",
        BrokerMessage::ExtensionMessage { .. } => "extension_message",
        BrokerMessage::ExtensionState { .. } => "extension_state",
        BrokerMessage::ExtensionStateResult { .. } => "extension_state_result",
    }
}

async fn read_task(
    mut read_half: BrokerReadHalf,
    inner: Arc<ClientInner>,
    reg_tx: oneshot::Sender<std::result::Result<String, String>>,
) {
    let mut reader = FrameReader::new();
    let mut buf = vec![0u8; READ_BUF];
    // Resolved exactly once — on the `registered` frame, a pre-registration error, or teardown.
    let mut reg_tx: Option<oneshot::Sender<std::result::Result<String, String>>> = Some(reg_tx);
    let mut close_reason = "client disconnected".to_string();

    'outer: loop {
        let n = match read_half.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                close_reason = e.to_string();
                // Post-registration socket errors get a distinct `error` event before the
                // eventual `disconnected` (`onSocketError`, `client.ts:235-240`).
                if inner.ever_registered.load(Ordering::SeqCst) {
                    let _ = inner.events.send(InboundEvent::Error(close_reason.clone()));
                }
                break;
            }
        };
        let chunk = buf.get(..n).unwrap_or(&[]);
        // pi's reader delivers every frame reassembled earlier in this SAME chunk to `onMessage`
        // synchronously, in order, and only afterward discovers/reports an oversize length
        // (`framing.ts:52-84`) — so an oversize `Err` still carries (and this dispatches) any frames
        // already reassembled, rather than discarding them; `oversize_close_reason` is applied only
        // AFTER the loop, so a fatal condition raised by one of those preserved frames themselves
        // (e.g. a duplicate `registered`) still takes precedence, exactly as it would have in pi.
        let (frames, oversize_close_reason) = match reader.push(chunk) {
            Ok(frames) => (frames, None),
            Err(e) => (e.frames, Some(e.error.to_string())),
        };
        for payload in frames {
            // JS-lenient: an overflowing numeric literal must not kill the whole frame — see
            // `framing::from_frame_slice`.
            let msg: BrokerMessage = match crate::transport::framing::from_frame_slice(&payload) {
                Ok(m) => m,
                Err(e) => {
                    close_reason = format!("intercom protocol error: {e}");
                    // A post-registration reader/protocol error also gets a distinct `error`
                    // event first (`onReaderError`, `client.ts:242-251`); pre-registration it
                    // just rejects the pending connect (handled via `reg_tx` below).
                    if inner.ever_registered.load(Ordering::SeqCst) {
                        let _ = inner.events.send(InboundEvent::Error(close_reason.clone()));
                    }
                    break 'outer;
                }
            };
            let registered = guard(&inner.session_id).is_some();
            let kind = broker_message_kind(&msg);
            // Any message other than `registered`/`error` arriving before registration is fatal
            // (`client.ts:302-304`) — no frame type is meaningful without a session id yet.
            if !registered && !matches!(msg, BrokerMessage::Registered { .. } | BrokerMessage::Error { .. }) {
                close_reason = format!("received {kind} before registered");
                break 'outer;
            }
            match msg {
                BrokerMessage::Registered { session_id, features: _ } => {
                    if registered {
                        // A second `registered` frame is fatal post-connect (`client.ts:312-314`);
                        // connectionEstablished is already true, so this surfaces as a distinct
                        // `error` event before the eventual `disconnected`.
                        close_reason =
                            "intercom protocol error: received duplicate registered message".to_string();
                        let _ = inner.events.send(InboundEvent::Error(close_reason.clone()));
                        break 'outer;
                    }
                    // Set the id BEFORE resolving connect (so `is_connected` holds immediately).
                    *guard(&inner.session_id) = Some(session_id.clone());
                    inner.ever_registered.store(true, Ordering::SeqCst);
                    if let Some(tx) = reg_tx.take() {
                        let _ = tx.send(Ok(session_id));
                    }
                }
                BrokerMessage::Sessions { request_id, sessions } => {
                    if let Some(tx) = guard(&inner.pending_lists).remove(&request_id) {
                        let _ = tx.send(Ok(sessions));
                    }
                }
                BrokerMessage::Message { from, message } => {
                    let _ = inner.events.send(InboundEvent::Message { from, message: Box::new(message) });
                }
                BrokerMessage::Delivered { message_id } => {
                    if let Some(tx) = guard(&inner.pending_sends).remove(&message_id) {
                        let _ = tx.send(SendResult { id: message_id, delivered: true, reason: None });
                    }
                }
                BrokerMessage::DeliveryFailed { message_id, reason } => {
                    if let Some(tx) = guard(&inner.pending_sends).remove(&message_id) {
                        let _ = tx.send(SendResult {
                            id: message_id,
                            delivered: false,
                            reason: Some(reason),
                        });
                    }
                }
                BrokerMessage::SessionJoined { session } => {
                    let _ = inner.events.send(InboundEvent::SessionJoined(session));
                }
                BrokerMessage::SessionLeft { session_id } => {
                    let _ = inner.events.send(InboundEvent::SessionLeft(session_id));
                }
                BrokerMessage::PresenceUpdate { session } => {
                    let _ = inner.events.send(InboundEvent::PresenceUpdate(session));
                }
                // v0.9.x frames this client decodes but does not yet act on. Decoding them is what
                // keeps the connection ALIVE: before these variants existed, a `message_receipt`
                // — which pi's broker forwards to the original sender the moment a route exists
                // (`v0.9.2 broker/broker.ts:812-818`) — was an `unknown variant` serde error, i.e.
                // `close_reason` + `break 'outer`, so the first message a cyrup client successfully
                // sent to a pi peer killed its own connection.
                //
                // Reacting to them is deliberately NOT done here: pi's client re-emits them as
                // `message_receipt`/`message_control` events (`v0.9.2 broker/client.ts:475-491`)
                // and the session-side handling of those is a separate, larger port. Dropping them
                // is a strict improvement over disconnecting and loses nothing that was previously
                // delivered.
                BrokerMessage::MessageReceipt { .. } | BrokerMessage::MessageControl { .. } => {
                    tracing::debug!(
                        kind,
                        "intercom client: v0.9.x frame decoded but not yet surfaced"
                    );
                }
                // Extension-bus frames (`v0.9.2 types.ts:115-136`). Unreachable in practice: pi's
                // broker only routes these to a session that advertised `extensions` in its
                // `register` (`isCapable`, `v0.9.2 broker/broker.ts:1209`), and cyrup's
                // `SessionRegistration` has no such field. Modelled anyway so a future/misbehaving
                // broker cannot tear the connection down.
                BrokerMessage::ExtensionOwner { .. }
                | BrokerMessage::ExtensionMessage { .. }
                | BrokerMessage::ExtensionState { .. }
                | BrokerMessage::ExtensionStateResult { .. } => {
                    tracing::debug!(
                        kind,
                        "intercom client: extension-bus frame ignored (bus not implemented)"
                    );
                }
                BrokerMessage::Error { error } => {
                    if !registered {
                        // A pre-registration error rejects the connect (client.ts:414-415).
                        close_reason = error;
                        break 'outer;
                    }
                    let _ = inner.events.send(InboundEvent::Error(error));
                }
            }
        }
        if let Some(reason) = oversize_close_reason {
            close_reason = reason;
            if inner.ever_registered.load(Ordering::SeqCst) {
                let _ = inner.events.send(InboundEvent::Error(close_reason.clone()));
            }
            break;
        }
    }

    // Reject a still-pending connect (unique to read_task), then run the shared teardown tail:
    // fail every pending send/list and broadcast `disconnected` (`onClose`, `client.ts:214-233`).
    if let Some(tx) = reg_tx.take() {
        let _ = tx.send(Err(close_reason.clone()));
    }
    teardown(&inner, close_reason);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;
    use crate::transport::protocol::now_ms;
    use crate::transport::target::{BrokerTcpEndpoint, INTERCOM_TCP_HOST};
    use tokio::net::UnixStream;

    /// A bare `ClientInner` with a throwaway writer channel — enough to exercise
    /// `IntercomClient` methods without a real socket.
    fn bare_inner() -> (Arc<ClientInner>, mpsc::UnboundedReceiver<WriterCmd>) {
        let (wtx, wrx) = mpsc::unbounded_channel::<WriterCmd>();
        let (events, _) = broadcast::channel::<InboundEvent>(16);
        let inner = Arc::new(ClientInner {
            session_id: Mutex::new(None),
            writer: wtx,
            pending_sends: Mutex::new(HashMap::new()),
            pending_lists: Mutex::new(HashMap::new()),
            events,
            connected: AtomicBool::new(false),
            disconnecting: AtomicBool::new(false),
            ever_registered: AtomicBool::new(false),
            teardown_started: AtomicBool::new(false),
            read_abort: Mutex::new(None),
            closed_notify: Notify::new(),
        });
        (inner, wrx)
    }

    fn registration() -> SessionRegistration {
        SessionRegistration {
            name: None,
            cwd: "/tmp".to_string(),
            model: "m".to_string(),
            pid: 1u32.into(),
            started_at: now_ms().into(),
            last_activity: now_ms().into(),
            status: None,
            extra: Default::default(),
        }
    }

    /// Accept one connection, read the client's first frame, reply `registered`, and hand the
    /// captured frame back — narrow enough to assert the exact `register` bytes
    /// `IntercomClient::connect_target` puts on the wire (`client.ts:280-285`).
    async fn capture_register_frame<S>(mut stream: S) -> serde_json::Value
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let mut reader = FrameReader::new();
        let mut buf = vec![0u8; 4096];
        loop {
            let n = stream.read(&mut buf).await.expect("read");
            assert_ne!(n, 0, "the client closed before registering");
            if let Some(payload) = reader.push(&buf[..n]).expect("frames").into_iter().next() {
                let frame: serde_json::Value = serde_json::from_slice(&payload).expect("json");
                let registered =
                    encode_json(&BrokerMessage::Registered { session_id: "s1".to_string(), features: None })
                        .expect("encodes");
                stream.write_all(&registered).await.expect("write");
                return frame;
            }
        }
    }

    /// `client.ts:26-30,280-285` — the opt-in TCP transport end to end at the client layer: a real
    /// loopback `TcpStream` (no network), and the `register` frame carries the endpoint's `stateId`,
    /// which the broker requires (`broker.ts:263-266`). Registration then completes normally.
    #[tokio::test]
    async fn connect_target_registers_over_tcp_with_the_endpoint_state_id() {
        let listener = tokio::net::TcpListener::bind((INTERCOM_TCP_HOST, 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let broker = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            capture_register_frame(stream).await
        });

        let target = BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
            host: INTERCOM_TCP_HOST.to_string(),
            port,
            state_id: Some("state-1".to_string()),
        });
        let client = IntercomClient::connect_target(&target, registration(), Some("sess-1".into()))
            .await
            .expect("registers over TCP");
        assert_eq!(client.session_id().as_deref(), Some("s1"));
        assert!(client.is_connected());

        let frame = broker.await.expect("broker task");
        assert_eq!(frame["type"], "register");
        assert_eq!(frame["sessionId"], "sess-1");
        assert_eq!(frame["stateId"], "state-1", "client.ts:284 spreads the TCP endpoint stateId");
    }

    /// MIRROR (stays green): over a socket target pi spreads `{}` (`client.ts:284`), so `stateId`
    /// must be **absent** from the register frame — not present-and-null, which the broker's
    /// `clientMessage.stateId === BROKER_STATE_ID` comparison would treat identically but which
    /// would differ byte-for-byte from pi's frame.
    #[tokio::test]
    async fn connect_over_a_socket_omits_the_state_id_from_register() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("broker.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let broker = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            capture_register_frame(stream).await
        });

        let client = IntercomClient::connect(&socket_path, registration(), Some("sess-1".into()))
            .await
            .expect("registers over the socket");
        assert!(client.is_connected());

        let frame = broker.await.expect("broker task");
        assert_eq!(frame["type"], "register");
        assert!(frame.get("stateId").is_none(), "socket registers carry no credential: {frame}");
    }

    // dossier item 1 (client.ts:426-467): `disconnect()` must synchronously fail every pending
    // send/list the moment it's called, not leave them to time out on their own. Regression proof:
    // against the pre-fix `disconnect()` (which never touched `pending_sends`/`pending_lists` at
    // all) this `try_recv()` would still be `Empty` after the call returns.
    #[tokio::test]
    async fn disconnect_synchronously_fails_pending_sends_and_lists() {
        let (inner, _wrx) = bare_inner();
        *guard(&inner.session_id) = Some("s1".to_string());
        inner.connected.store(true, Ordering::SeqCst);

        let (send_tx, mut send_rx) = oneshot::channel::<SendResult>();
        guard(&inner.pending_sends).insert("m1".to_string(), send_tx);
        let (list_tx, mut list_rx) = oneshot::channel::<std::result::Result<Vec<SessionInfo>, String>>();
        guard(&inner.pending_lists).insert("r1".to_string(), list_tx);

        let client = IntercomClient { inner };
        client.disconnect();

        // No `.await` yield occurred between `disconnect()` returning and these `try_recv()`s —
        // proving the fail-pending happened synchronously inside `disconnect()` itself.
        let sent = send_rx.try_recv().expect("pending send was already failed synchronously");
        assert!(!sent.delivered);
        assert_eq!(sent.reason.as_deref(), Some("client disconnected"));
        let listed = list_rx.try_recv().expect("pending list was already failed synchronously");
        assert_eq!(listed.unwrap_err(), "client disconnected");
        assert!(!client.is_connected());
    }

    // dossier item 2 (client.ts:551-579): `cancel_ask`/`update_presence` must no-op once
    // `disconnect()` has begun, never attempting a frame. Regression proof: against the pre-fix
    // (unguarded) methods, the writer channel would additionally observe `WriterCmd::Frame`s for
    // the cancel_ask/presence payloads below.
    #[tokio::test]
    async fn cancel_ask_and_update_presence_are_noop_once_disconnecting() {
        let (inner, mut wrx) = bare_inner();
        *guard(&inner.session_id) = Some("s1".to_string());
        inner.connected.store(true, Ordering::SeqCst);
        let client = IntercomClient { inner };

        client.disconnect(); // queues Unregister + Close, sets disconnecting=true synchronously.
        client.cancel_ask("some-message");
        client.update_presence(Some("name".to_string()), None, None);

        // Only the two frames `disconnect()` itself queued may appear; cancel_ask/update_presence
        // must not have queued anything after them.
        let first = wrx.try_recv().expect("disconnect queued the unregister frame");
        assert!(matches!(first, WriterCmd::Frame(_)));
        let second = wrx.try_recv().expect("disconnect queued the close command");
        assert!(matches!(second, WriterCmd::Close));
        assert!(wrx.try_recv().is_err(), "cancel_ask/update_presence must not queue any frame");
    }

    // dossier item 4 (client.ts:302-304): any message other than `registered`/`error` arriving
    // before the session is registered is a fatal, connection-ending protocol violation.
    // Regression proof: pre-fix, `read_task` processed every message type unconditionally
    // regardless of registration state, so `reg_rx` would never resolve here (it would instead
    // hang until the unrelated 10s CONNECT_TIMEOUT elsewhere).
    #[tokio::test]
    async fn read_task_rejects_message_before_registered() {
        let (a, mut b) = UnixStream::pair().expect("socketpair");
        let (read_half, _write_half) = BrokerStream::new(a).into_split();
        let (inner, _wrx) = bare_inner();
        let (reg_tx, reg_rx) = oneshot::channel();
        tokio::spawn(read_task(read_half, inner, reg_tx));

        // A `sessions` frame is well-formed but illegal before `registered`.
        let frame = encode_json(&BrokerMessage::Sessions { request_id: "r1".to_string(), sessions: vec![] })
            .expect("encodes");
        b.write_all(&frame).await.expect("write");

        let result = tokio::time::timeout(Duration::from_secs(2), reg_rx)
            .await
            .expect("reg_rx resolves promptly, not after the 10s connect timeout")
            .expect("oneshot not dropped");
        let err = result.expect_err("must reject the connect");
        assert!(err.contains("sessions"), "error names the offending message type: {err}");
        assert!(err.contains("before registered"), "error text: {err}");
    }

    // dossier item 4 (client.ts:312-314): a second `registered` frame after the session is already
    // registered is fatal. Regression proof: pre-fix, the second `Registered` arm just overwrote
    // `session_id` again silently (reg_tx was already consumed) with no error surfaced at all.
    #[tokio::test]
    async fn read_task_rejects_duplicate_registered_message() {
        let (a, mut b) = UnixStream::pair().expect("socketpair");
        let (read_half, _write_half) = BrokerStream::new(a).into_split();
        let (inner, _wrx) = bare_inner();
        let (reg_tx, reg_rx) = oneshot::channel();
        let mut events = inner.events.subscribe();
        tokio::spawn(read_task(read_half, inner, reg_tx));

        let registered = encode_json(&BrokerMessage::Registered { session_id: "s1".to_string(), features: None }).expect("encodes");
        b.write_all(&registered).await.expect("write");
        let first = tokio::time::timeout(Duration::from_secs(2), reg_rx)
            .await
            .expect("registers")
            .expect("oneshot not dropped")
            .expect("first registration succeeds");
        assert_eq!(first, "s1");

        // A second `registered` frame must be rejected as a distinct `error` event, then a
        // `disconnected` event — never silently accepted.
        b.write_all(&registered).await.expect("write");
        let error_evt = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("an error event arrives")
            .expect("channel open");
        match error_evt {
            InboundEvent::Error(msg) => assert!(msg.contains("duplicate registered"), "{msg}"),
            other => panic!("expected InboundEvent::Error, got {other:?}"),
        }
        let disc_evt = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("a disconnected event follows")
            .expect("channel open");
        assert!(matches!(disc_evt, InboundEvent::Disconnected(_)));
    }

    // dossier item 5 (client.ts:242-251): a post-registration reader/protocol error emits a
    // distinct `InboundEvent::Error` BEFORE the eventual `InboundEvent::Disconnected`. Regression
    // proof: pre-fix, `read_task` only ever set a local `close_reason` and broke, so a caller
    // listening for `InboundEvent::Error` on a post-registration frame error never saw one.
    #[tokio::test]
    async fn read_task_emits_error_before_disconnected_on_post_registration_frame_error() {
        let (a, mut b) = UnixStream::pair().expect("socketpair");
        let (read_half, _write_half) = BrokerStream::new(a).into_split();
        let (inner, _wrx) = bare_inner();
        let (reg_tx, reg_rx) = oneshot::channel();
        let mut events = inner.events.subscribe();
        tokio::spawn(read_task(read_half, inner, reg_tx));

        let registered = encode_json(&BrokerMessage::Registered { session_id: "s1".to_string(), features: None }).expect("encodes");
        b.write_all(&registered).await.expect("write");
        tokio::time::timeout(Duration::from_secs(2), reg_rx)
            .await
            .expect("registers")
            .expect("oneshot not dropped")
            .expect("registration succeeds");

        // An oversize length header is a hard framing error (`FrameError::Oversize`).
        let bad_len = (crate::transport::framing::MAX_FRAME_BYTES as u32) + 1;
        b.write_all(&bad_len.to_be_bytes()).await.expect("write");

        let first_evt = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("an error event arrives")
            .expect("channel open");
        assert!(matches!(first_evt, InboundEvent::Error(_)), "{first_evt:?}");
        let second_evt = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("a disconnected event follows")
            .expect("channel open");
        assert!(matches!(second_evt, InboundEvent::Disconnected(_)), "{second_evt:?}");
    }

    // framing.rs dossier item ("frames already reassembled before an oversize frame in the same
    // push() call are discarded"): pi's reader delivers every complete frame found earlier in the
    // same `data` chunk to `onMessage` synchronously, in order, BEFORE it discovers a later oversize
    // length (`framing.ts:52-84`). Regression proof: before this fix, `read_task`'s
    // `Err(e) => { close_reason = e.to_string(); ...; break; }` on `reader.push` discarded
    // `FrameReadError::frames` entirely, so a valid `message` frame arriving in the SAME chunk as a
    // trailing oversize header would never surface as an `InboundEvent::Message` — this test fails
    // against that pre-fix behavior (no `Message` event would ever arrive before `Disconnected`).
    #[tokio::test]
    async fn message_reassembled_before_an_oversize_frame_in_the_same_chunk_still_surfaces() {
        let (a, mut b) = UnixStream::pair().expect("socketpair");
        let (read_half, _write_half) = BrokerStream::new(a).into_split();
        let (inner, _wrx) = bare_inner();
        let (reg_tx, reg_rx) = oneshot::channel();
        let mut events = inner.events.subscribe();
        tokio::spawn(read_task(read_half, inner, reg_tx));

        let registered = encode_json(&BrokerMessage::Registered { session_id: "s1".to_string(), features: None }).expect("encodes");
        b.write_all(&registered).await.expect("write");
        tokio::time::timeout(Duration::from_secs(2), reg_rx)
            .await
            .expect("registers")
            .expect("oneshot not dropped")
            .expect("registration succeeds");

        // A valid `message` frame, immediately followed (in the SAME write, i.e. the SAME chunk the
        // reader's `push()` sees) by a bogus oversize length header.
        let from = SessionInfo {
            id: "sender".to_string(),
            name: Some("sender".to_string()),
            cwd: "/w".to_string(),
            model: "m".to_string(),
            pid: 1u32.into(),
            started_at: 0u64.into(),
            last_activity: 0u64.into(),
            status: None,
            peer_uid: None,
            trusted_local: None,
            context_pct: None,
            context_tokens: None,
            context_window: None,
            extra: Default::default(),
        };
        let message = Message {
            id: "q1".to_string(),
            timestamp: 0u64.into(),
            reply_to: None,
            expects_reply: Some(true),
            content: MessageContent { text: "hi".to_string(), attachments: None, ..Default::default() },
            ..Default::default()
        };
        let mut chunk = encode_json(&BrokerMessage::Message { from, message }).expect("encodes");
        let bad_len = (crate::transport::framing::MAX_FRAME_BYTES as u32) + 1;
        chunk.extend_from_slice(&bad_len.to_be_bytes());
        b.write_all(&chunk).await.expect("single combined write");

        let first_evt = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("the preserved message frame must still surface")
            .expect("channel open");
        assert!(
            matches!(first_evt, InboundEvent::Message { .. }),
            "expected the message reassembled before the oversize header, got {first_evt:?}"
        );
        let second_evt = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("an error event follows")
            .expect("channel open");
        assert!(matches!(second_evt, InboundEvent::Error(_)), "{second_evt:?}");
        let third_evt = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("a disconnected event follows")
            .expect("channel open");
        assert!(matches!(third_evt, InboundEvent::Disconnected(_)), "{third_evt:?}");
    }

    fn test_session_info(id: &str) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            name: Some(id.to_string()),
            cwd: "/w".to_string(),
            model: "m".to_string(),
            pid: 1u32.into(),
            started_at: 0u64.into(),
            last_activity: 0u64.into(),
            status: None,
            peer_uid: None,
            trusted_local: None,
            context_pct: None,
            context_tokens: None,
            context_window: None,
            extra: Default::default(),
        }
    }

    /// Drive `read_task` to a registered state over a socketpair; returns the peer end, an event
    /// receiver, and the two handles the caller must keep alive for the socket to stay open.
    #[allow(clippy::type_complexity)]
    async fn registered_read_task() -> (
        UnixStream,
        broadcast::Receiver<InboundEvent>,
        (BrokerWriteHalf, mpsc::UnboundedReceiver<WriterCmd>),
    ) {
        let (a, mut b) = UnixStream::pair().expect("socketpair");
        let (read_half, write_half) = BrokerStream::new(a).into_split();
        let (inner, wrx) = bare_inner();
        let (reg_tx, reg_rx) = oneshot::channel();
        let events = inner.events.subscribe();
        tokio::spawn(read_task(read_half, inner, reg_tx));
        let registered =
            encode_json(&BrokerMessage::Registered { session_id: "s1".to_string(), features: None })
                .expect("encodes");
        b.write_all(&registered).await.expect("write");
        tokio::time::timeout(Duration::from_secs(2), reg_rx)
            .await
            .expect("registers")
            .expect("oneshot not dropped")
            .expect("registration succeeds");
        (b, events, (write_half, wrx))
    }

    // G136(a), client side. pi >= 0.9.0 brokers forward a `message_receipt` to the ORIGINAL SENDER
    // as soon as a route exists (`v0.9.2 broker/broker.ts:812-818`) and send a `message_control` to
    // the RECEIVER on any peer cancel (`:852-860`) or supersede (`:684-688`). Regression proof:
    // before `BrokerMessage` carried these variants, `serde_json::from_slice` returned an
    // `unknown variant` error, which this reader turns into `close_reason` + `break 'outer` — so
    // the FIRST message a cyrup client successfully sent to a pi peer killed its own connection.
    // Against that pre-fix behavior the `Message` assertion below never fires: the reader is gone.
    #[tokio::test]
    async fn read_task_survives_v0_9_x_receipt_and_control_frames() {
        let (mut b, mut events, _keepalive) = registered_read_task().await;

        // Written as RAW wire frames on purpose: these are bytes a pi >= 0.9.0 broker puts on the
        // socket, and encoding them through `BrokerMessage` would only prove the enum round-trips
        // to itself.
        let peer = serde_json::to_value(test_session_info("peer")).expect("encodes");
        for frame in [
            serde_json::json!({
                "type": "message_receipt", "from": peer,
                "receipt": { "messageId": "m1", "status": "receiver_received", "timestamp": 1 },
            }),
            serde_json::json!({
                "type": "message_control", "from": peer,
                "control": { "messageId": "m1", "action": "cancel", "timestamp": 1 },
            }),
            serde_json::json!({
                "type": "extension_owner", "namespace": "ns", "ownerId": "s2", "ownerEpoch": "e1",
            }),
        ] {
            b.write_all(&encode_json(&frame).expect("encodes")).await.expect("write");
        }

        // The connection must still be reading: a normal `message` after all of the above surfaces.
        let follow_up = encode_json(&BrokerMessage::Message {
            from: test_session_info("peer"),
            message: Message {
                id: "m2".to_string(),
                timestamp: 0u64.into(),
                content: MessageContent { text: "still alive".to_string(), ..Default::default() },
                ..Default::default()
            },
        })
        .expect("encodes");
        b.write_all(&follow_up).await.expect("write");

        let evt = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("the connection must still be reading after the v0.9.x frames")
            .expect("channel open");
        match evt {
            InboundEvent::Message { message, .. } => assert_eq!(message.content.text, "still alive"),
            other => panic!("expected the follow-up message, got {other:?}"),
        }
    }

    // MIRROR for the test above. Modelling the v0.9.2 tag set must not make the reader credulous:
    // a tag from some *later* protocol version is still fatal, exactly as it is upstream
    // (`default: throw new Error(\`Unknown broker message type\`)`, `v0.9.2 broker/client.ts:599-600`,
    // routed to `socket.destroy()` by `framing.ts:44-51`), and so is a KNOWN tag whose payload does
    // not type-check (pi's `isMessageReceipt` guard, `v0.9.2 broker/client.ts:56-65`).
    #[tokio::test]
    async fn read_task_still_closes_on_unknown_tag_and_on_malformed_known_tag() {
        for bad in [
            serde_json::json!({ "type": "pi_quantum_v2", "whatever": 1 }),
            serde_json::json!({
                "type": "message_receipt",
                "from": { "id": "p", "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
                "receipt": { "messageId": "m1", "status": "teleported", "timestamp": 1 },
            }),
            serde_json::json!({
                "type": "message_control",
                "from": { "id": "p", "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
                "control": { "messageId": "m1", "timestamp": 1 },
            }),
        ] {
            let (mut b, mut events, _keepalive) = registered_read_task().await;
            b.write_all(&encode_json(&bad).expect("encodes")).await.expect("write");

            let mut saw_error = false;
            let mut saw_disconnected = false;
            for _ in 0..3 {
                match tokio::time::timeout(Duration::from_secs(2), events.recv()).await {
                    Ok(Ok(InboundEvent::Error(_))) => saw_error = true,
                    Ok(Ok(InboundEvent::Disconnected(_))) => {
                        saw_disconnected = true;
                        break;
                    }
                    Ok(Ok(other)) => panic!("unexpected event for {bad}: {other:?}"),
                    Ok(Err(e)) => panic!("event channel error: {e}"),
                    Err(_) => break,
                }
            }
            assert!(saw_error, "a protocol error event must precede teardown for {bad}");
            assert!(saw_disconnected, "the connection must still be torn down for {bad}");
        }
    }

    // dossier item 6 (client.ts:235-240): a write-path failure must propagate the real error and
    // tear the connection down, not silently discard it. Regression proof: pre-fix, `writer_task`
    // only ever did `if write_half.write_all(...).is_err() { break; }` with no event emitted and no
    // teardown triggered — `events.recv()` here would never resolve and `pending_sends` would
    // never be failed.
    #[tokio::test]
    async fn writer_task_propagates_write_failure_and_tears_down() {
        let (a, b) = UnixStream::pair().expect("socketpair");
        let (_read_half, write_half) = BrokerStream::new(a).into_split();
        drop(b); // Fully close the peer so the next write(s) fail with a real io::Error.

        let (wtx, wrx) = mpsc::unbounded_channel::<WriterCmd>();
        let (events, _) = broadcast::channel::<InboundEvent>(16);
        let inner = Arc::new(ClientInner {
            session_id: Mutex::new(Some("s1".to_string())),
            writer: wtx.clone(),
            pending_sends: Mutex::new(HashMap::new()),
            pending_lists: Mutex::new(HashMap::new()),
            events,
            connected: AtomicBool::new(true),
            disconnecting: AtomicBool::new(false),
            ever_registered: AtomicBool::new(true),
            teardown_started: AtomicBool::new(false),
            read_abort: Mutex::new(None),
            closed_notify: Notify::new(),
        });
        let (send_tx, send_rx) = oneshot::channel::<SendResult>();
        guard(&inner.pending_sends).insert("m1".to_string(), send_tx);
        let mut events = inner.events.subscribe();

        tokio::spawn(writer_task(write_half, wrx, inner.clone()));

        // The peer is gone; keep writing until the broken-pipe error is observed (bounded).
        for _ in 0..64 {
            if wtx.send(WriterCmd::Frame(vec![0u8; 4096])).is_err() {
                break;
            }
        }

        let evt = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("an error event arrives from the write failure")
            .expect("channel open");
        assert!(matches!(evt, InboundEvent::Error(_)), "{evt:?}");

        let result = tokio::time::timeout(Duration::from_secs(5), send_rx)
            .await
            .expect("pending send is failed by the write-failure teardown")
            .expect("oneshot not dropped");
        assert!(!result.delivered);
        assert!(inner.teardown_started.load(Ordering::SeqCst));
    }

    // dossier item 3 (client.ts:184-191): a registration timeout must destroy the socket + its
    // background tasks, not leave them running. Regression proof: pre-fix, `connect()` just
    // returned `Err` on timeout without aborting `read_task` or closing the writer, so the broker's
    // accepted stream would never observe EOF (`read()` would hang indefinitely instead of the
    // bounded wait asserted below).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connect_timeout_tears_down_the_socket_and_background_tasks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("broker.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");

        let broker = tokio::spawn(async move {
            let (mut stream, _addr) = listener.accept().await.expect("accept");
            // Read (and discard) the register frame the client sends, then never reply — forcing
            // the client's CONNECT_TIMEOUT to fire.
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            // Now prove the client side was actually torn down: our read() must observe EOF
            // within a bounded window. This window must extend past the client's own 10s
            // CONNECT_TIMEOUT (it only aborts the socket once ITS timer fires), plus slack for
            // the abort/shutdown to actually land.
            tokio::time::timeout(Duration::from_secs(12), async {
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) => return true,
                        Ok(_) => continue,
                        Err(_) => return true,
                    }
                }
            })
            .await
            .unwrap_or(false)
        });

        let result = IntercomClient::connect(&socket_path, registration(), None).await;
        assert!(result.is_err(), "the broker never registers, so connect() must time out");

        let saw_eof = tokio::time::timeout(Duration::from_secs(15), broker)
            .await
            .expect("broker task completes")
            .expect("broker task did not panic");
        assert!(saw_eof, "the client's socket/tasks must be torn down on a connect timeout");
    }
}

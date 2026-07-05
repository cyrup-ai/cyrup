//! The per-session [`IntercomClient`] — a port of `pi-intercom/broker/client.ts:119-580`.
//!
//! Connect → immediately `register` → resolve on the `registered` frame (10 s timeout,
//! `client.ts:182-233`). `send` correlates a `delivered`/`delivery_failed` ack by `message.id`
//! (10 s, `client.ts:504-549`); `list_sessions` correlates a `sessions` reply by `requestId` (5 s,
//! `client.ts:469-502`). Inbound `message`/`session_joined`/`session_left`/`presence_update`/`error`
//! frames fan out on a `broadcast` channel ([`IntercomClient::subscribe`]) — the Rust analog of pi's
//! `EventEmitter`. There is **no automatic reconnect** (`client.ts:214-251`); a stable identity
//! across reconnect is achieved by re-`register`ing with the same `session_id` (broker takeover).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::error::{IntercomError, Result};
use crate::transport::framing::{FrameReader, encode_json};
use crate::transport::protocol::{
    Attachment, BrokerMessage, ClientMessage, Message, MessageContent, SessionInfo,
    SessionRegistration, now_ms,
};

/// The `registered`-frame + `list`/`send` correlation timeouts (`client.ts:182,492,538`).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const LIST_TIMEOUT: Duration = Duration::from_secs(5);
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
        /// The delivered message.
        message: Message,
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
}

impl ClientInner {
    /// Queue one frame to the writer; returns `false` if the writer channel is closed.
    fn send_frame(&self, frame: Vec<u8>) -> bool {
        self.writer.send(WriterCmd::Frame(frame)).is_ok()
    }
}

fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A per-session broker client.
pub struct IntercomClient {
    inner: Arc<ClientInner>,
}

impl IntercomClient {
    /// Connect to the broker at `socket_path`, register `registration` (re-adopting `session_id` if
    /// `Some`), and resolve once the `registered` frame arrives (`connect`, `client.ts:164-293`).
    ///
    /// # Errors
    /// [`IntercomError::Io`] if the socket cannot be connected; [`IntercomError::Client`] on a
    /// registration timeout / a pre-registration error / a connection closed before registration.
    pub async fn connect(
        socket_path: &Path,
        registration: SessionRegistration,
        session_id: Option<String>,
    ) -> Result<Self> {
        let stream = UnixStream::connect(socket_path).await?;
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
        });

        tokio::spawn(writer_task(write_half, wrx));

        let (reg_tx, reg_rx) = oneshot::channel::<std::result::Result<String, String>>();
        tokio::spawn(read_task(read_half, inner.clone(), reg_tx));

        // Register immediately; the OS/tokio buffers the write until connected (client.ts:276-282).
        let register = ClientMessage::Register {
            session: registration,
            session_id,
            state_id: None,
        };
        if !inner.send_frame(encode_json(&register)?) {
            return Err(IntercomError::Client("writer closed before register".to_string()));
        }

        match tokio::time::timeout(CONNECT_TIMEOUT, reg_rx).await {
            Ok(Ok(Ok(sid))) => {
                *guard(&inner.session_id) = Some(sid);
                inner.connected.store(true, Ordering::SeqCst);
                Ok(Self { inner })
            }
            Ok(Ok(Err(msg))) => Err(IntercomError::Client(msg)),
            Ok(Err(_)) => Err(IntercomError::Client("connection closed before registration".to_string())),
            Err(_) => Err(IntercomError::Client("connection timeout".to_string())),
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
        self.inner.connected.load(Ordering::SeqCst) && guard(&self.inner.session_id).is_some()
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
            timestamp: now_ms(),
            reply_to: options.reply_to,
            expects_reply: options.expects_reply,
            content: MessageContent { text: options.text, attachments: options.attachments },
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
    /// `client.ts:551-566`). A closed socket is a silent no-op.
    pub fn cancel_ask(&self, message_id: &str) {
        if let Ok(frame) = encode_json(&ClientMessage::CancelAsk { message_id: message_id.to_string() }) {
            let _ = self.inner.send_frame(frame);
        }
    }

    /// Best-effort presence update (`updatePresence`, `client.ts:568-579`).
    pub fn update_presence(&self, name: Option<String>, status: Option<String>, model: Option<String>) {
        if let Ok(frame) = encode_json(&ClientMessage::Presence { name, status, model }) {
            let _ = self.inner.send_frame(frame);
        }
    }

    /// Send `unregister` then half-close the socket (`disconnect`, `client.ts:426-467`). The
    /// [`WriterCmd::Close`] is queued AFTER the `unregister` frame, so the broker sees the graceful
    /// unregister before the EOF.
    pub fn disconnect(&self) {
        self.inner.connected.store(false, Ordering::SeqCst);
        if let Ok(frame) = encode_json(&ClientMessage::Unregister) {
            let _ = self.inner.send_frame(frame);
        }
        let _ = self.inner.writer.send(WriterCmd::Close);
    }
}

async fn writer_task(mut write_half: OwnedWriteHalf, mut rx: mpsc::UnboundedReceiver<WriterCmd>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            WriterCmd::Frame(frame) => {
                if write_half.write_all(&frame).await.is_err() {
                    break;
                }
            }
            WriterCmd::Close => break,
        }
    }
    let _ = write_half.shutdown().await;
}

async fn read_task(
    mut read_half: OwnedReadHalf,
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
                break;
            }
        };
        let chunk = buf.get(..n).unwrap_or(&[]);
        let frames = match reader.push(chunk) {
            Ok(frames) => frames,
            Err(e) => {
                close_reason = e.to_string();
                break;
            }
        };
        for payload in frames {
            let msg: BrokerMessage = match serde_json::from_slice(&payload) {
                Ok(m) => m,
                Err(e) => {
                    close_reason = format!("intercom protocol error: {e}");
                    break 'outer;
                }
            };
            let registered = guard(&inner.session_id).is_some();
            match msg {
                BrokerMessage::Registered { session_id } => {
                    // Set the id BEFORE resolving connect (so `is_connected` holds immediately).
                    *guard(&inner.session_id) = Some(session_id.clone());
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
                    let _ = inner.events.send(InboundEvent::Message { from, message });
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
    }

    // Teardown: reject a still-pending connect, fail every pending send/list, broadcast disconnect.
    inner.connected.store(false, Ordering::SeqCst);
    *guard(&inner.session_id) = None;
    if let Some(tx) = reg_tx.take() {
        let _ = tx.send(Err(close_reason.clone()));
    }
    for (_, tx) in guard(&inner.pending_sends).drain() {
        let _ = tx.send(SendResult { id: String::new(), delivered: false, reason: Some(close_reason.clone()) });
    }
    for (_, tx) in guard(&inner.pending_lists).drain() {
        let _ = tx.send(Err(close_reason.clone()));
    }
    let _ = inner.events.send(InboundEvent::Disconnected(close_reason));
}

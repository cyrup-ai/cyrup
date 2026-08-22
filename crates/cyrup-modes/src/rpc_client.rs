//! The RPC **client** — the other end of [`crate::run_rpc`] (SEAM-017).
//!
//! 1:1 port of `pi/packages/coding-agent/src/modes/rpc/rpc-client.ts` @v0.83.0 (600 lines,
//! re-exported from `modes/index.ts:7`), read in full at that tag. pi's own summary: *"Spawns the
//! agent in RPC mode and provides a typed API for all operations."* Everything below keeps pi's
//! method names, its `req_N` id scheme, its two default timeouts (30 000 ms per request, 60 000 ms
//! per idle wait), and its **error strings verbatim** — `Timeout waiting for response to ${type}.
//! Stderr: ${stderr}` (`rpc-client.ts:565`), `Agent process exited (code=${code} signal=${signal}).
//! Stderr: ${stderr}` (`:529`), `Client already started` (`:75`), `Client not started` (`:543`) —
//! because a client's error text is part of what an embedder matches on.
//!
//! # Why it matters that this exists
//!
//! Without it every embedder — and every one of this crate's own RPC tests — hand-rolls NDJSON
//! framing and request correlation, which is precisely how the wire-shape divergences SEAM-011
//! (a cyrup-invented `{widget}` blob) and SEAM-053 (explicit `null` where pi omits the key) survived
//! three audits: nothing in-tree ever *read* the wire the way a real client does.
//!
//! # Transport: `attach` vs `spawn`
//!
//! pi has one constructor because Node's `spawn` always yields a child with pipes. Here the
//! transport is a parameter, exactly as it already is for [`crate::run_rpc`]/[`crate::run_print`]
//! ("Both endpoints are parameters so tests drive an in-memory reader/writer pair and the binary
//! wires real stdio", `lib.rs`):
//!
//! * [`RpcClient::attach`] takes any reader + writer — an in-process `tokio::io::duplex` pair driven
//!   straight against `run_rpc`, or a child's real stdout/stdin.
//! * [`RpcClient::spawn`] is pi's `start()`: build the argv, spawn the child, pump its stderr, and
//!   `attach` to its stdio.
//!
//! [`RpcClient::spawn`] is the literal port; `attach` is the seam it is written on top of, so the
//! correlation/dispatch machinery is exercised by the crate's own tests without a build artifact on
//! disk (the gate does not build the `cyrup` binary for this crate).
//!
//! # Three JS→Rust mechanism gaps, and how each is closed
//!
//! These are the reasons this file is not a transliteration.
//!
//! 1. **A JS `async` function always settles; a Rust future can be dropped at any `.await`.**
//!    pi removes a pending request from `pendingRequests` on the resolve path, the reject path and
//!    the timeout path (`rpc-client.ts:514`, `:564`, `:584`) — three success-shaped paths, and in JS
//!    that is exhaustive because `send()` cannot be *abandoned* mid-flight. In Rust, dropping the
//!    `send()` future (a cancelled task, a `select!` losing arm, a timeout wrapped by the caller)
//!    would leave the entry in the map forever. Cleanup is therefore [`PendingGuard::drop`], not a
//!    statement on any path. The same argument applies to `onEvent`'s unsubscribe closure, which pi
//!    calls from the resolve path of `waitForIdle`/`collectEvents` (`:465`, `:487`): here it is
//!    [`EventSubscription::drop`], so a dropped `wait_for_idle` future cannot leak a listener that
//!    then fires for the rest of the process's life.
//! 2. **JS has no locks, so a re-entered handler is an ordinary nested call.** pi's `handleLine`
//!    iterates `this.eventListeners` directly (`:520-522`); a listener that subscribes or
//!    unsubscribes during that loop is legal. Holding the listener `Mutex` across the callback would
//!    let exactly that call re-take a held lock and hang with no deadlock detection, so
//!    [`ClientInner::dispatch_event`] **snapshots the listener list, releases the lock, then calls**.
//! 3. **A JS promise can be forgotten; a Rust `oneshot` cannot.** pi's `stop()` does
//!    `this.pendingRequests.clear()` (`:165`) without rejecting, so any in-flight `send()` promise
//!    is never settled — a permanent hang the caller cannot observe. Dropping the sender here wakes
//!    the receiver with an error instead. **[CYRUP-DELTA]** (`rpc-client.ts:165`): a stopped client
//!    fails its in-flight requests with the process-exit error rather than hanging forever. The
//!    divergence is forced — `tokio::sync::oneshot` has no "leak the receiver" state — and it is
//!    strictly the safer half.
//!
//! # [CYRUP-DELTA] — the listener payload is the wire object, not a typed event
//!
//! pi types its listener `(event: AgentSessionEvent) => void` (`rpc-client.ts:49`; at v0.84.1
//! `JsonAgentSessionEvent`, `:50`). Neither Rust type can stand in: [`crate::JsonAgentSessionEvent`]
//! is a *borrowing serialization view* over an owned [`cyrup_session_svc::AgentSessionEvent`]
//! (`json_event.rs`), and `AgentSessionEvent` itself derives `Serialize` only
//! (`cyrup-session-svc/src/event.rs:116`) — there is no deserialize direction to project back
//! through. Listeners therefore receive the parsed [`serde_json::Value`] line, which is what a real
//! client has in hand anyway, plus [`event_type`] for the `type` tag every dispatch needs. The
//! in-tree precedent is `cyrup-ext-subagents`' `SubagentEvent` (`exec/ndjson.rs`), which re-types
//! the identical bytes rather than reusing the producer's enum.
//!
//! # [CYRUP-DELTA] — no interpreter in the argv
//!
//! pi spawns `node <cliPath> --mode rpc …` with `cliPath` defaulting to the build artifact
//! `"dist/cli.js"` (`rpc-client.ts:80`, `:93`). A cyrup build artifact is an executable, so the argv
//! is `<cli_path> --mode rpc …` with `cli_path` defaulting to [`DEFAULT_CLI_PATH`] — the installed
//! binary name, resolved through `PATH`. Every other spawn detail is pi's: `--provider`/`--model`
//! appended only when set, then the caller's extra `args`, the child's environment inherited and
//! overlaid, and all three stdio handles piped (`:93-97`).

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::oneshot;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use crate::rpc::types::RpcResponse;

// ---------------------------------------------------------------------------------------------
// Constants — pi's literals
// ---------------------------------------------------------------------------------------------

/// Per-request response timeout — pi's `setTimeout(…, 30000)` (`rpc-client.ts:566`).
pub const REQUEST_TIMEOUT_MS: u64 = 30_000;

/// Default idle/collect timeout — pi's `timeout = 60000` default parameter (`rpc-client.ts:455`,
/// `:475`, `:497`).
pub const DEFAULT_IDLE_TIMEOUT_MS: u64 = 60_000;

/// How long [`RpcClient::spawn`] lets the child initialize before checking whether it already died —
/// pi's `await new Promise((resolve) => setTimeout(resolve, 100))` (`rpc-client.ts:132`).
const START_SETTLE_MS: u64 = 100;

/// How long [`RpcClient::stop`] waits after `SIGTERM` before escalating to `SIGKILL` — pi's
/// `setTimeout(… kill("SIGKILL") …, 1000)` (`rpc-client.ts:153-156`).
const STOP_GRACE_MS: u64 = 1_000;

/// The executable [`RpcClientOptions::cli_path`] defaults to — the cyrup analogue of pi's
/// `"dist/cli.js"` (`rpc-client.ts:80`). Resolved through `PATH` by the OS, as pi's relative path is
/// resolved against the child's cwd.
pub const DEFAULT_CLI_PATH: &str = "cyrup";

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// Everything [`RpcClient`] can fail with. Each `Display` form is pi's thrown `Error` message
/// character-for-character, so an embedder that matched on pi's text keeps matching.
#[derive(Debug, thiserror::Error)]
pub enum RpcClientError {
    /// pi `throw new Error("Client already started")` (`rpc-client.ts:75`).
    #[error("Client already started")]
    AlreadyStarted,

    /// pi `throw new Error("Client not started")` (`rpc-client.ts:543`).
    #[error("Client not started")]
    NotStarted,

    /// pi `createProcessExitError` (`rpc-client.ts:528-530`). `code`/`signal` render as `null` when
    /// absent, matching JS template interpolation of `null`.
    #[error("Agent process exited (code={code} signal={signal}). Stderr: {stderr}")]
    ProcessExited {
        code: String,
        signal: String,
        stderr: String,
    },

    /// pi's `"error"` handler on the child (`rpc-client.ts:114`).
    #[error("Agent process error: {message}. Stderr: {stderr}")]
    ProcessError { message: String, stderr: String },

    /// pi's stdin-not-writable guard (`rpc-client.ts:554`).
    #[error("Agent process stdin is not writable. Stderr: {stderr}")]
    StdinNotWritable { stderr: String },

    /// pi's per-request timeout (`rpc-client.ts:565`).
    #[error("Timeout waiting for response to {command}. Stderr: {stderr}")]
    RequestTimeout { command: String, stderr: String },

    /// pi's `waitForIdle` timeout (`rpc-client.ts:459`).
    #[error("Timeout waiting for agent to become idle. Stderr: {stderr}")]
    IdleTimeout { stderr: String },

    /// pi's `collectEvents` timeout (`rpc-client.ts:480`).
    #[error("Timeout collecting events. Stderr: {stderr}")]
    CollectTimeout { stderr: String },

    /// pi's `getData` rejection — the `error` string off a `success:false` response, rethrown
    /// verbatim (`rpc-client.ts:591-594`).
    #[error("{0}")]
    Command(String),

    /// Decoding a `data` payload into the caller's requested shape failed. pi asserts the type
    /// instead (`rpc-client.ts:595-598` — "Type assertion: we trust response.data matches T"), which
    /// Rust cannot do, so the assertion becomes a checked failure.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// Spawning the child or writing to its stdin failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------------------------
// Options and small wire shapes
// ---------------------------------------------------------------------------------------------

/// Spawn options — pi's `RpcClientOptions` (`rpc-client.ts:27-40`), field for field.
#[derive(Clone, Debug, Default)]
pub struct RpcClientOptions {
    /// Path to the CLI entry point (pi: `cliPath`, default `dist/cli.js`; here
    /// [`DEFAULT_CLI_PATH`]).
    pub cli_path: Option<String>,
    /// Working directory for the agent (pi: `cwd`).
    pub cwd: Option<std::path::PathBuf>,
    /// Environment overlay, applied on top of the inherited environment (pi:
    /// `env: { ...process.env, ...this.options.env }`, `rpc-client.ts:95`).
    pub env: Vec<(String, String)>,
    /// Provider to use — appended as `--provider <p>` when set (pi: `:83-85`).
    pub provider: Option<String>,
    /// Model id to use — appended as `--model <m>` when set (pi: `:86-88`).
    pub model: Option<String>,
    /// Additional CLI arguments, appended last (pi: `:89-91`).
    pub args: Vec<String>,
}

/// The narrowed model view pi's client hands back from `getAvailableModels` (`rpc-client.ts:42-47`).
///
/// pi's RPC host answers with the *whole* model objects (`rpc-mode.ts:485-487` →
/// `{ models }` off `modelRuntime.getAvailable()`), and cyrup's does the same
/// (`rpc.rs` `get_available_models` → `session.available_model_catalog()`); the client type is a
/// structural subset on both sides, so the extra keys are ignored rather than rejected.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// Provider id the model is served by (wire key `provider`) — the `provider` argument of
    /// [`RpcClient::set_model`].
    pub provider: String,
    /// Model id (wire key `id`) — the `model_id` argument of [`RpcClient::set_model`].
    pub id: String,
    /// Context window in tokens (wire key `contextWindow` — camelCase via the `rename_all`).
    pub context_window: u64,
    /// Whether this is a reasoning model (wire key `reasoning`).
    pub reasoning: bool,
}

/// One row of `get_fork_messages` (pi `Array<{ entryId, text }>`, `rpc-client.ts:395-398`).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkMessage {
    /// Transcript entry id of the fork point (wire key `entryId` — camelCase via the `rename_all`)
    /// — the `entry_id` argument of [`RpcClient::fork`].
    pub entry_id: String,
    /// The entry's message text, for presenting the fork point to a user (wire key `text`).
    pub text: String,
}

/// The `type` tag of a wire line, if it has one — the discriminant every client dispatch needs, and
/// the only thing this module itself reads out of an event (`handleLine`'s `data.type === "response"`
/// test, `rpc-client.ts:512`, and `waitForIdle`'s `event.type === "agent_settled"`, `:463`).
#[must_use]
pub fn event_type(event: &Value) -> Option<&str> {
    event.get("type").and_then(Value::as_str)
}

/// The event that ends a turn — pi's `waitForIdle`/`collectEvents` terminal (`rpc-client.ts:463`,
/// `:485`).
const AGENT_SETTLED: &str = "agent_settled";

// ---------------------------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------------------------

type Listener = Arc<dyn Fn(&Value) + Send + Sync>;

/// The half of the client the reader task shares with the handle — pi's private fields
/// (`rpc-client.ts:56-64`).
struct ClientInner {
    /// pi `pendingRequests: Map<string, {resolve, reject}>` (`:59-60`). A dropped receiver simply
    /// makes the eventual `send` fail; entries are removed by [`PendingGuard`], never by a caller.
    pending: Mutex<HashMap<String, oneshot::Sender<RpcResponse>>>,
    /// pi `eventListeners: RpcEventListener[]` (`:58`), keyed so an unsubscribe is O(n) by identity
    /// rather than by closure equality (which Rust does not have).
    listeners: Mutex<Vec<(u64, Listener)>>,
    next_listener_id: AtomicU64,
    /// pi `requestId = 0`, incremented pre-increment into `req_${++this.requestId}` (`:61`, `:559`).
    request_id: AtomicU64,
    /// pi `stderr = ""` (`:62`) — everything the child wrote to stderr, for the error messages.
    stderr: Mutex<String>,
    /// pi `exitError: Error | null` (`:63`) — latched on exit/error/stdin-error, and re-thrown by
    /// every later `send` (`:545-547`).
    exit_error: Mutex<Option<RpcClientError>>,
    /// The write half. `None` once [`RpcClient::stop`] has run — pi's `this.process = null` (`:164`).
    stdin: AsyncMutex<Option<Box<dyn AsyncWrite + Send + Unpin>>>,
}

impl ClientInner {
    fn stderr_snapshot(&self) -> String {
        self.stderr
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    fn set_exit_error(&self, error: RpcClientError) {
        let mut slot = lock_ignoring_poison(&self.exit_error);
        // pi keeps the FIRST error it latched: `this.exitError ?? new Error(...)` on the stdin path
        // (`rpc-client.ts:121`). The exit/error handlers overwrite, but they fire once each.
        if slot.is_none() {
            *slot = Some(error);
        }
    }

    /// The latched exit error, re-rendered (pi rethrows the same `Error` object; `RpcClientError` is
    /// not `Clone`, so the stored variant is reconstructed).
    fn exit_error(&self) -> Option<RpcClientError> {
        let slot = lock_ignoring_poison(&self.exit_error);
        slot.as_ref().map(clone_error)
    }

    /// pi `rejectPendingRequests` (`rpc-client.ts:532-537`): settle every in-flight request and empty
    /// the map. Dropping the senders is the settle — see mechanism gap 3 in the module docs.
    fn reject_pending_requests(&self) {
        let mut map = lock_ignoring_poison(&self.pending);
        map.clear();
    }

    /// pi's `for (const listener of this.eventListeners) listener(data)` (`rpc-client.ts:520-522`),
    /// with the lock released before the first callback — mechanism gap 2 in the module docs.
    fn dispatch_event(&self, event: &Value) {
        let snapshot: Vec<Listener> = {
            let guard = lock_ignoring_poison(&self.listeners);
            guard.iter().map(|(_, l)| Arc::clone(l)).collect()
        };
        for listener in snapshot {
            listener(event);
        }
    }

    /// pi `handleLine` (`rpc-client.ts:507-526`): a `response` line whose `id` is pending resolves
    /// that request; anything else — including a `response` for an id nobody is waiting on — is an
    /// event; a non-JSON line is ignored.
    fn handle_line(&self, line: &str) {
        let Ok(data) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if event_type(&data) == Some("response") {
            // pi correlates on the RAW id (`data.id`), which the host echoes as-sent — string OR
            // number (R-11-015). `id_key` renders both the way `send` registered them.
            if let Some(key) = data.get("id").and_then(id_key) {
                let waiting = {
                    let mut map = lock_ignoring_poison(&self.pending);
                    map.remove(&key)
                };
                if let Some(tx) = waiting {
                    // The receiver may already be gone (the caller's future was dropped); pi's
                    // resolve on a settled promise is likewise a no-op. A malformed `response` line
                    // is not a response — fall through to nothing, exactly as pi's `JSON.parse`
                    // catch swallows a bad line.
                    if let Ok(response) = serde_json::from_value::<RpcResponse>(data) {
                        let _ = tx.send(response);
                    }
                    return;
                }
            }
        }
        self.dispatch_event(&data);
    }
}

/// Reconstruct a latched error so it can be handed to more than one caller (pi rethrows one shared
/// `Error` object; Rust needs a copy per throw).
fn clone_error(e: &RpcClientError) -> RpcClientError {
    match e {
        RpcClientError::AlreadyStarted => RpcClientError::AlreadyStarted,
        RpcClientError::NotStarted => RpcClientError::NotStarted,
        RpcClientError::ProcessExited {
            code,
            signal,
            stderr,
        } => RpcClientError::ProcessExited {
            code: code.clone(),
            signal: signal.clone(),
            stderr: stderr.clone(),
        },
        RpcClientError::ProcessError { message, stderr } => RpcClientError::ProcessError {
            message: message.clone(),
            stderr: stderr.clone(),
        },
        RpcClientError::StdinNotWritable { stderr } => RpcClientError::StdinNotWritable {
            stderr: stderr.clone(),
        },
        RpcClientError::RequestTimeout { command, stderr } => RpcClientError::RequestTimeout {
            command: command.clone(),
            stderr: stderr.clone(),
        },
        RpcClientError::IdleTimeout { stderr } => RpcClientError::IdleTimeout {
            stderr: stderr.clone(),
        },
        RpcClientError::CollectTimeout { stderr } => RpcClientError::CollectTimeout {
            stderr: stderr.clone(),
        },
        RpcClientError::Command(m) => RpcClientError::Command(m.clone()),
        RpcClientError::Json(e) => RpcClientError::Command(e.to_string()),
        RpcClientError::Io(e) => RpcClientError::Command(e.to_string()),
    }
}

/// The correlation key for a wire `id`. pi uses the JSON value directly as a `Map` key, which for a
/// string id is the string and for a number id is that number's identity; both render uniquely here.
fn id_key(id: &Value) -> Option<String> {
    match id {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Removes its request from `pending` when the `send` future ends — however it ends. This is
/// mechanism gap 1 from the module docs: pi's three `pendingRequests.delete(id)` statements
/// (`rpc-client.ts:514`, `:564`, `:584`) are exhaustive in JS and would not be in Rust.
struct PendingGuard {
    inner: Arc<ClientInner>,
    id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let mut map = lock_ignoring_poison(&self.inner.pending);
        map.remove(&self.id);
    }
}

/// A live `onEvent` registration. pi returns an unsubscribe **closure** (`rpc-client.ts:171-179`)
/// and calls it from the success path of `waitForIdle`/`collectEvents`; dropping this guard is that
/// call, and it also runs when the awaiting future is cancelled — mechanism gap 1 again.
#[must_use = "dropping the subscription immediately unsubscribes the listener"]
pub struct EventSubscription {
    inner: Arc<ClientInner>,
    id: u64,
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        let mut list = lock_ignoring_poison(&self.inner.listeners);
        list.retain(|(id, _)| *id != self.id);
    }
}

// ---------------------------------------------------------------------------------------------
// RpcClient
// ---------------------------------------------------------------------------------------------

/// A connected RPC client — pi's `RpcClient` class (`rpc-client.ts:55`).
pub struct RpcClient {
    inner: Arc<ClientInner>,
    /// The spawned child, when this client owns one. `None` for [`RpcClient::attach`].
    child: AsyncMutex<Option<Child>>,
    /// pi `stopReadingStdout` (`rpc-client.ts:57`): aborting the task detaches the reader.
    reader_task: Mutex<Option<JoinHandle<()>>>,
    stderr_task: Mutex<Option<JoinHandle<()>>>,
}

impl RpcClient {
    // -----------------------------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------------------------

    /// Attach to an already-open RPC transport: `reader` is the host's `RpcOut` stream, `writer` its
    /// command sink.
    ///
    /// **[CYRUP-DELTA] (SEAM-082) — this constructor has NO upstream counterpart.** pi's `RpcClient`
    /// exposes exactly one way to come into existence, `start()`
    /// (`packages/coding-agent/src/modes/rpc/rpc-client.ts:73-139` @v0.83.0), and it always spawns
    /// `node <cliPath> --mode rpc`; there is no `attach`, and no route by which a caller supplies its
    /// own transport. cyrup adds one so an in-process `tokio::io::duplex` pair can drive the protocol
    /// without a child process. What it decomposes out of `start()` is pi's
    /// `attachJsonlLineReader(childProcess.stdout, …)` + `stdin.write` pair (`:127-129`, `:580`);
    /// [`RpcClient::spawn`] below is the faithful `start()` port and keeps the process half.
    ///
    /// The delta adds **no wire surface** — the 33 protocol verbs, the three helpers, the four
    /// lifecycle methods, the `req_${n}` id format and every timeout constant are pi's — so a client
    /// built through `attach` speaks exactly the protocol a client built through `spawn` speaks. It is
    /// recorded here rather than left implicit so an auditor comparing the two method surfaces finds
    /// the one difference already accounted for.
    ///
    /// Framing is pi's strict LF-only JSONL (`jsonl.ts:7-8`, `:26`): records split on `\n` only, a
    /// trailing `\r` is stripped, and a final unterminated line is delivered at EOF —
    /// `tokio::io::Lines` does all three.
    pub fn attach<R, W>(reader: R, writer: W) -> Self
    where
        R: AsyncBufRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let inner = Arc::new(ClientInner {
            pending: Mutex::new(HashMap::new()),
            listeners: Mutex::new(Vec::new()),
            next_listener_id: AtomicU64::new(0),
            request_id: AtomicU64::new(0),
            stderr: Mutex::new(String::new()),
            exit_error: Mutex::new(None),
            stdin: AsyncMutex::new(Some(Box::new(writer))),
        });
        let reader_task = tokio::spawn(read_lines(Arc::clone(&inner), reader));
        Self {
            inner,
            child: AsyncMutex::new(None),
            reader_task: Mutex::new(Some(reader_task)),
            stderr_task: Mutex::new(None),
        }
    }

    /// Spawn the agent in RPC mode and attach to it — pi's `start()` (`rpc-client.ts:73-139`).
    ///
    /// The argv is pi's, minus the interpreter: `<cli_path> --mode rpc [--provider P] [--model M]
    /// [args…]`. stderr is collected into the buffer every error message quotes **and** forwarded to
    /// this process's stderr, exactly as pi does (`:101-104`). After spawning, pi waits 100 ms and
    /// fails if the child has already exited (`:131-138`); so does this.
    ///
    /// pi's `if (this.process) throw new Error("Client already started")` guard (`:74-76`) is
    /// structural here — a fresh `RpcClient` is returned, so there is no started client to re-start.
    /// [`RpcClientError::AlreadyStarted`] is kept for embedders that wrap this in their own
    /// start/stop lifecycle.
    pub async fn spawn(options: RpcClientOptions) -> Result<Self, RpcClientError> {
        let cli_path = options
            .cli_path
            .clone()
            .unwrap_or_else(|| DEFAULT_CLI_PATH.to_string());

        let mut args: Vec<String> = vec!["--mode".to_string(), "rpc".to_string()];
        if let Some(provider) = &options.provider {
            args.push("--provider".to_string());
            args.push(provider.clone());
        }
        if let Some(model) = &options.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        args.extend(options.args.iter().cloned());

        let mut command = tokio::process::Command::new(&cli_path);
        command
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &options.cwd {
            command.current_dir(cwd);
        }
        for (k, v) in &options.env {
            command.env(k, v);
        }
        // pi does not detach; the child dies with the parent's process group as Node's does.
        let mut child = command.spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("child stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("child stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("child stderr was not piped"))?;

        let client = Self::attach(BufReader::new(stdout), stdin);

        // pi: `childProcess.stderr?.on("data", …)` — accumulate AND passthrough (`:101-104`).
        let inner = Arc::clone(&client.inner);
        let stderr_task = tokio::spawn(async move {
            // Read BYTES and decode lossily, the way the host-side reader does
            // (`rpc.rs`'s `String::from_utf8_lossy`). A line-strict `lines()` pump ends
            // permanently on the first non-UTF-8 byte, and this buffer is the only diagnostic
            // payload `ProcessExited`/`ProcessError`/`RequestTimeout`/… carry — so the failure
            // it would truncate is exactly the one whose explanation the embedder needs. pi's
            // `on("data")` accumulates raw bytes and has no decode failure mode at all.
            let mut reader = BufReader::new(stderr);
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                match reader.read_until(b'\n', &mut bytes).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let decoded = String::from_utf8_lossy(&bytes);
                        let line = decoded.strip_suffix('\n').unwrap_or(&decoded);
                        let line = line.strip_suffix('\r').unwrap_or(line);
                        {
                            let mut buf = lock_ignoring_poison(&inner.stderr);
                            buf.push_str(line);
                            buf.push('\n');
                        }
                        eprintln!("{line}");
                    }
                    Err(_) => break, // the pipe is gone; nothing left to collect
                }
            }
        });
        if let Ok(mut slot) = client.stderr_task.lock() {
            *slot = Some(stderr_task);
        }

        *client.child.lock().await = Some(child);

        // pi `await new Promise((resolve) => setTimeout(resolve, 100))` then the exit check
        // (`rpc-client.ts:131-138`).
        tokio::time::sleep(Duration::from_millis(START_SETTLE_MS)).await;
        let early_exit = {
            let mut guard = client.child.lock().await;
            match guard.as_mut() {
                Some(c) => c.try_wait()?,
                None => None,
            }
        };
        if let Some(status) = early_exit {
            let error = process_exit_error(&client.inner, &status);
            client.inner.set_exit_error(clone_error(&error));
            client.inner.reject_pending_requests();
            return Err(error);
        }
        Ok(client)
    }

    // -----------------------------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------------------------

    /// Stop the agent process — pi's `stop()` (`rpc-client.ts:144-166`): detach the stdout reader,
    /// `SIGTERM`, wait up to 1 s, then `SIGKILL`, then drop the handle and clear the pending map.
    ///
    /// For an [`RpcClient::attach`]ed client there is no child to signal; the reader is detached and
    /// the writer dropped, which is the whole of "stop" for that transport.
    pub async fn stop(&self) {
        abort_task(&self.reader_task);
        // Closing the write half is what makes an in-process host observe EOF and return.
        *self.inner.stdin.lock().await = None;

        let mut guard = self.child.lock().await;
        if let Some(child) = guard.as_mut() {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                // pi `this.process.kill("SIGTERM")` (`:149`). `Child::kill` is SIGKILL in Rust, so
                // the graceful signal goes through `nix`, matching the crate-wide pattern
                // (`cyrup-ext-subagents/src/background/control.rs:433-434`).
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
            }
            // pi's 1 s escalation timer (`:152-162`).
            if tokio::time::timeout(Duration::from_millis(STOP_GRACE_MS), child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
            }
        }
        *guard = None;
        drop(guard);

        abort_task(&self.stderr_task);
        // pi `this.pendingRequests.clear()` (`:165`) — see mechanism gap 3.
        self.inner.reject_pending_requests();
    }

    /// Subscribe to agent events — pi's `onEvent` (`rpc-client.ts:171-179`). The returned
    /// [`EventSubscription`] unsubscribes on drop; pi returns a closure that does the same thing when
    /// called.
    pub fn on_event<F>(&self, listener: F) -> EventSubscription
    where
        F: Fn(&Value) + Send + Sync + 'static,
    {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::SeqCst);
        {
            let mut list = lock_ignoring_poison(&self.inner.listeners);
            list.push((id, Arc::new(listener)));
        }
        EventSubscription {
            inner: Arc::clone(&self.inner),
            id,
        }
    }

    /// Everything the child has written to stderr — pi's `getStderr()` (`rpc-client.ts:184-186`).
    #[must_use]
    pub fn stderr(&self) -> String {
        self.inner.stderr_snapshot()
    }

    /// How many `onEvent` listeners are registered. Test-only: the two RAII cleanups this module
    /// exists to guarantee (mechanism gap 1) are *absences*, and an absence assertion is vacuous
    /// unless the corresponding presence is asserted first.
    #[cfg(test)]
    pub(crate) fn listener_count(&self) -> usize {
        lock_ignoring_poison(&self.inner.listeners).len()
    }

    /// How many requests are awaiting a correlated response. Test-only, for the same reason.
    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        lock_ignoring_poison(&self.inner.pending).len()
    }

    // -----------------------------------------------------------------------------------------
    // Command methods — pi `rpc-client.ts:192-445`, in pi's order
    // -----------------------------------------------------------------------------------------

    /// pi `prompt` (`:197`). Returns as soon as the host accepts; use [`Self::on_event`] for the
    /// stream and [`Self::wait_for_idle`] for completion.
    pub async fn prompt(&self, message: &str, images: Option<Value>) -> Result<(), RpcClientError> {
        self.send(command("prompt", [("message", json!(message))], images))
            .await
            .map(|_| ())
    }

    /// pi `steer` (`:204`).
    pub async fn steer(&self, message: &str, images: Option<Value>) -> Result<(), RpcClientError> {
        self.send(command("steer", [("message", json!(message))], images))
            .await
            .map(|_| ())
    }

    /// pi `followUp` (`:211`).
    pub async fn follow_up(
        &self,
        message: &str,
        images: Option<Value>,
    ) -> Result<(), RpcClientError> {
        self.send(command("follow_up", [("message", json!(message))], images))
            .await
            .map(|_| ())
    }

    /// pi `abort` (`:218`).
    pub async fn abort(&self) -> Result<(), RpcClientError> {
        self.send(command("abort", [], None)).await.map(|_| ())
    }

    /// pi `newSession` (`:227`) — `{ cancelled }` when an extension vetoed the new session.
    pub async fn new_session(&self, parent_session: Option<&str>) -> Result<bool, RpcClientError> {
        // pi spreads `parentSession` in unconditionally; `undefined` is dropped by JSON.stringify,
        // so an absent parent means an absent KEY, never `null` (SEAM-053's rule).
        let mut fields: Vec<(&str, Value)> = Vec::new();
        if let Some(parent) = parent_session {
            fields.push(("parentSession", json!(parent)));
        }
        let data = self.data(command("new_session", fields, None)).await?;
        Ok(cancelled(&data))
    }

    /// pi `getState` (`:235`). The host builds this object with `json!` (`rpc.rs` `get_state`), and
    /// there is no Rust `RpcSessionState` type to deserialize into, so the snapshot is returned raw.
    pub async fn get_state(&self) -> Result<Value, RpcClientError> {
        self.data(command("get_state", [], None)).await
    }

    /// pi `setModel` (`:243`).
    pub async fn set_model(
        &self,
        provider: &str,
        model_id: &str,
    ) -> Result<Value, RpcClientError> {
        self.data(command(
            "set_model",
            [("provider", json!(provider)), ("modelId", json!(model_id))],
            None,
        ))
        .await
    }

    /// pi `cycleModel` (`:251`) — `None` when there is nothing to cycle to (the host answers
    /// `data: null`).
    pub async fn cycle_model(&self) -> Result<Option<Value>, RpcClientError> {
        let data = self.data(command("cycle_model", [], None)).await?;
        Ok(nullable(data))
    }

    /// pi `getAvailableModels` (`:263`) — unwraps `{ models }`.
    pub async fn get_available_models(&self) -> Result<Vec<ModelInfo>, RpcClientError> {
        let data = self
            .data(command("get_available_models", [], None))
            .await?;
        Ok(serde_json::from_value(field(data, "models"))?)
    }

    /// pi `setThinkingLevel` (`:271`).
    pub async fn set_thinking_level(&self, level: &str) -> Result<(), RpcClientError> {
        self.send(command("set_thinking_level", [("level", json!(level))], None))
            .await
            .map(|_| ())
    }

    /// pi `cycleThinkingLevel` (`:278`).
    pub async fn cycle_thinking_level(&self) -> Result<Option<Value>, RpcClientError> {
        let data = self.data(command("cycle_thinking_level", [], None)).await?;
        Ok(nullable(data))
    }

    /// pi `getAvailableThinkingLevels` (`:286`) — unwraps `{ levels }`. SEAM-014 landed the host arm.
    pub async fn get_available_thinking_levels(&self) -> Result<Vec<String>, RpcClientError> {
        let data = self
            .data(command("get_available_thinking_levels", [], None))
            .await?;
        Ok(serde_json::from_value(field(data, "levels"))?)
    }

    /// pi `setSteeringMode` (`:294`) — `"all"` | `"one-at-a-time"`.
    pub async fn set_steering_mode(&self, mode: &str) -> Result<(), RpcClientError> {
        self.send(command("set_steering_mode", [("mode", json!(mode))], None))
            .await
            .map(|_| ())
    }

    /// pi `setFollowUpMode` (`:301`).
    pub async fn set_follow_up_mode(&self, mode: &str) -> Result<(), RpcClientError> {
        self.send(command("set_follow_up_mode", [("mode", json!(mode))], None))
            .await
            .map(|_| ())
    }

    /// pi `compact` (`:308`). `custom_instructions` is omitted from the wire when absent, never sent
    /// as `null`.
    pub async fn compact(
        &self,
        custom_instructions: Option<&str>,
    ) -> Result<Value, RpcClientError> {
        let mut fields: Vec<(&str, Value)> = Vec::new();
        if let Some(instructions) = custom_instructions {
            fields.push(("customInstructions", json!(instructions)));
        }
        self.data(command("compact", fields, None)).await
    }

    /// pi `setAutoCompaction` (`:316`).
    pub async fn set_auto_compaction(&self, enabled: bool) -> Result<(), RpcClientError> {
        self.send(command(
            "set_auto_compaction",
            [("enabled", json!(enabled))],
            None,
        ))
        .await
        .map(|_| ())
    }

    /// pi `setAutoRetry` (`:323`).
    pub async fn set_auto_retry(&self, enabled: bool) -> Result<(), RpcClientError> {
        self.send(command(
            "set_auto_retry",
            [("enabled", json!(enabled))],
            None,
        ))
        .await
        .map(|_| ())
    }

    /// pi `abortRetry` (`:330`).
    pub async fn abort_retry(&self) -> Result<(), RpcClientError> {
        self.send(command("abort_retry", [], None)).await.map(|_| ())
    }

    /// pi `bash` (`:337`).
    pub async fn bash(&self, cmd: &str) -> Result<Value, RpcClientError> {
        self.data(command("bash", [("command", json!(cmd))], None))
            .await
    }

    /// pi `abortBash` (`:345`).
    pub async fn abort_bash(&self) -> Result<(), RpcClientError> {
        self.send(command("abort_bash", [], None)).await.map(|_| ())
    }

    /// pi `getSessionStats` (`:352`).
    pub async fn get_session_stats(&self) -> Result<Value, RpcClientError> {
        self.data(command("get_session_stats", [], None)).await
    }

    /// pi `exportHtml` (`:360`) — `{ path }`.
    pub async fn export_html(&self, output_path: Option<&str>) -> Result<String, RpcClientError> {
        let mut fields: Vec<(&str, Value)> = Vec::new();
        if let Some(path) = output_path {
            fields.push(("outputPath", json!(path)));
        }
        let data = self.data(command("export_html", fields, None)).await?;
        Ok(serde_json::from_value(field(data, "path"))?)
    }

    /// pi `switchSession` (`:369`) — `{ cancelled }`.
    pub async fn switch_session(&self, session_path: &str) -> Result<bool, RpcClientError> {
        let data = self
            .data(command(
                "switch_session",
                [("sessionPath", json!(session_path))],
                None,
            ))
            .await?;
        Ok(cancelled(&data))
    }

    /// pi `fork` (`:378`) — `{ text, cancelled }`.
    pub async fn fork(&self, entry_id: &str) -> Result<(String, bool), RpcClientError> {
        let data = self
            .data(command("fork", [("entryId", json!(entry_id))], None))
            .await?;
        let text = data
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok((text, cancelled(&data)))
    }

    /// pi `clone` (`:387`) — `{ cancelled }`.
    pub async fn clone_session(&self) -> Result<bool, RpcClientError> {
        let data = self.data(command("clone", [], None)).await?;
        Ok(cancelled(&data))
    }

    /// pi `getForkMessages` (`:395`) — unwraps `{ messages }`.
    pub async fn get_fork_messages(&self) -> Result<Vec<ForkMessage>, RpcClientError> {
        let data = self.data(command("get_fork_messages", [], None)).await?;
        Ok(serde_json::from_value(field(data, "messages"))?)
    }

    /// pi `getEntries` (`:403`) — the whole `{ entries, leafId }` object, since `SessionEntry` has no
    /// deserializable Rust counterpart in this crate.
    pub async fn get_entries(&self, since: Option<&str>) -> Result<Value, RpcClientError> {
        let mut fields: Vec<(&str, Value)> = Vec::new();
        if let Some(since) = since {
            fields.push(("since", json!(since)));
        }
        self.data(command("get_entries", fields, None)).await
    }

    /// pi `getTree` (`:411`) — the whole `{ tree, leafId }` object.
    pub async fn get_tree(&self) -> Result<Value, RpcClientError> {
        self.data(command("get_tree", [], None)).await
    }

    /// pi `getLastAssistantText` (`:419`) — unwraps `{ text }`, `None` for a `null`.
    pub async fn get_last_assistant_text(&self) -> Result<Option<String>, RpcClientError> {
        let data = self
            .data(command("get_last_assistant_text", [], None))
            .await?;
        Ok(data
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// pi `setSessionName` (`:427`).
    pub async fn set_session_name(&self, name: &str) -> Result<(), RpcClientError> {
        self.send(command("set_session_name", [("name", json!(name))], None))
            .await
            .map(|_| ())
    }

    /// pi `getMessages` (`:434`) — unwraps `{ messages }`.
    pub async fn get_messages(&self) -> Result<Vec<Value>, RpcClientError> {
        let data = self.data(command("get_messages", [], None)).await?;
        Ok(serde_json::from_value(field(data, "messages"))?)
    }

    /// pi `getCommands` (`:442`) — unwraps `{ commands }`.
    pub async fn get_commands(&self) -> Result<Vec<Value>, RpcClientError> {
        let data = self.data(command("get_commands", [], None)).await?;
        Ok(serde_json::from_value(field(data, "commands"))?)
    }

    // -----------------------------------------------------------------------------------------
    // Helpers — pi `rpc-client.ts:451-501`
    // -----------------------------------------------------------------------------------------

    /// Wait for `agent_settled` — pi's `waitForIdle` (`rpc-client.ts:455-470`).
    ///
    /// The listener is registered synchronously before the first `.await`, and unsubscribed by the
    /// [`EventSubscription`]'s `Drop` whether this future completes, times out, **or is cancelled**.
    pub async fn wait_for_idle(&self, timeout: Duration) -> Result<(), RpcClientError> {
        let (subscription, settled) = self.subscribe_settled();
        let outcome = tokio::time::timeout(timeout, settled).await;
        drop(subscription);
        match outcome {
            Ok(_) => Ok(()),
            Err(_) => Err(RpcClientError::IdleTimeout {
                stderr: self.inner.stderr_snapshot(),
            }),
        }
    }

    /// Collect every event up to and including `agent_settled` — pi's `collectEvents`
    /// (`rpc-client.ts:475-492`).
    pub async fn collect_events(&self, timeout: Duration) -> Result<Vec<Value>, RpcClientError> {
        let (subscription, events) = self.subscribe_collect();
        let outcome = tokio::time::timeout(timeout, events).await;
        drop(subscription);
        match outcome {
            Ok(Ok(events)) => Ok(events),
            _ => Err(RpcClientError::CollectTimeout {
                stderr: self.inner.stderr_snapshot(),
            }),
        }
    }

    /// pi `promptAndWait` (`rpc-client.ts:497-501`): arm the collector **before** sending, so the
    /// first `agent_start` cannot be missed, then send and await.
    pub async fn prompt_and_wait(
        &self,
        message: &str,
        images: Option<Value>,
        timeout: Duration,
    ) -> Result<Vec<Value>, RpcClientError> {
        let (subscription, events) = self.subscribe_collect();
        self.prompt(message, images).await?;
        let outcome = tokio::time::timeout(timeout, events).await;
        drop(subscription);
        match outcome {
            Ok(Ok(events)) => Ok(events),
            _ => Err(RpcClientError::CollectTimeout {
                stderr: self.inner.stderr_snapshot(),
            }),
        }
    }

    /// The synchronous half of [`Self::wait_for_idle`]: register the listener and hand back the
    /// receiver, so a caller can arm the wait before performing the action that triggers it.
    fn subscribe_settled(&self) -> (EventSubscription, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        let slot = Mutex::new(Some(tx));
        let subscription = self.on_event(move |event| {
            if event_type(event) == Some(AGENT_SETTLED) {
                let mut guard = lock_ignoring_poison(&slot);
                if let Some(tx) = guard.take() {
                    let _ = tx.send(());
                }
            }
        });
        (subscription, rx)
    }

    /// The synchronous half of [`Self::collect_events`].
    fn subscribe_collect(&self) -> (EventSubscription, oneshot::Receiver<Vec<Value>>) {
        let (tx, rx) = oneshot::channel();
        let state = Mutex::new((Vec::<Value>::new(), Some(tx)));
        let subscription = self.on_event(move |event| {
            let mut guard = lock_ignoring_poison(&state);
            let (events, tx) = &mut *guard;
            if tx.is_none() {
                return;
            }
            events.push(event.clone());
            if event_type(event) == Some(AGENT_SETTLED)
                && let Some(tx) = tx.take()
            {
                let _ = tx.send(std::mem::take(events));
            }
        });
        (subscription, rx)
    }

    // -----------------------------------------------------------------------------------------
    // Internal — pi `rpc-client.ts:507-599`
    // -----------------------------------------------------------------------------------------

    /// pi `getData<T>` (`rpc-client.ts:590-599`): reject with the response's own `error` string when
    /// `success` is false, otherwise hand back `data`.
    async fn data(&self, body: Map<String, Value>) -> Result<Value, RpcClientError> {
        let response = self.send(body).await?;
        if !response.success {
            return Err(RpcClientError::Command(
                response.error.unwrap_or_default(),
            ));
        }
        Ok(response.data.unwrap_or(Value::Null))
    }

    /// pi `send` (`rpc-client.ts:539-588`) — the whole preflight, in pi's order.
    async fn send(&self, body: Map<String, Value>) -> Result<RpcResponse, RpcClientError> {
        // pi reads `command.type` for the timeout message (`:565`).
        let command_name = body
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        // `:545-547` — a latched exit error pre-empts everything.
        if let Some(error) = self.inner.exit_error() {
            return Err(error);
        }

        // `:559-560` — `req_${++this.requestId}`, so the first id is `req_1`.
        let n = self.inner.request_id.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("req_{n}");
        let mut full = body;
        full.insert("id".to_string(), json!(id));
        let line = format!("{}\n", Value::Object(full));

        let (tx, rx) = oneshot::channel();
        {
            let mut map = lock_ignoring_poison(&self.inner.pending);
            map.insert(id.clone(), tx);
        }
        // Registered before the first `.await` ⇒ the removal must be RAII. See mechanism gap 1.
        let _guard = PendingGuard {
            inner: Arc::clone(&self.inner),
            id,
        };

        {
            let mut stdin = self.inner.stdin.lock().await;
            // `:542-544` / `:553-557` — no writer at all is "not started"; a writer that refuses the
            // bytes is pi's `stdin.destroyed || !stdin.writable`.
            let Some(writer) = stdin.as_mut() else {
                return Err(RpcClientError::NotStarted);
            };
            if let Err(e) = writer.write_all(line.as_bytes()).await {
                let error = RpcClientError::StdinNotWritable {
                    stderr: format!("{}{e}", self.inner.stderr_snapshot()),
                };
                self.inner.set_exit_error(clone_error(&error));
                return Err(error);
            }
            if let Err(e) = writer.flush().await {
                let error = RpcClientError::StdinNotWritable {
                    stderr: format!("{}{e}", self.inner.stderr_snapshot()),
                };
                self.inner.set_exit_error(clone_error(&error));
                return Err(error);
            }
        }

        match tokio::time::timeout(Duration::from_millis(REQUEST_TIMEOUT_MS), rx).await {
            Ok(Ok(response)) => Ok(response),
            // The sender was dropped: either `reject_pending_requests` ran (child exited, client
            // stopped) or the reader task is gone. pi rejects with the latched exit error (`:534`).
            Ok(Err(_)) => Err(self
                .inner
                .exit_error()
                .unwrap_or(RpcClientError::NotStarted)),
            Err(_) => Err(RpcClientError::RequestTimeout {
                command: command_name,
                stderr: self.inner.stderr_snapshot(),
            }),
        }
    }
}

impl Drop for RpcClient {
    /// Detach the background tasks when the handle goes away. pi has no destructor — a garbage
    /// collected `RpcClient` leaves its child running and its listeners attached — but a Rust task
    /// holding an `Arc<ClientInner>` would outlive the handle and keep reading forever, so the
    /// reader/stderr pumps are aborted here. Any spawned child is left alone unless [`Self::stop`]
    /// was called, matching pi.
    fn drop(&mut self) {
        abort_task(&self.reader_task);
        abort_task(&self.stderr_task);
    }
}

// ---------------------------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------------------------

/// Take a `std::sync::Mutex` the way every call site in this file wants it: a poisoned lock still
/// yields its guard. A panic in one holder does not invalidate any of the state guarded here — a
/// pending map, a listener list, a stderr buffer — so recovering the inner value is what every one
/// of these locks did individually before this helper collapsed them.
fn lock_ignoring_poison<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Abort a background pump and forget its handle. A poisoned slot is left alone, exactly as each of
/// the four hand-written copies of this block did.
fn abort_task(slot: &Mutex<Option<JoinHandle<()>>>) {
    if let Ok(mut guard) = slot.lock()
        && let Some(handle) = guard.take()
    {
        handle.abort();
    }
}

/// Build a command body without its `id` — pi's `RpcCommandBody` (`rpc-client.ts:25`). `images` is
/// spread in only when present, because pi passes `images` as an optional property and
/// `JSON.stringify` drops an `undefined` value: an absent image list is an absent KEY, never `null`
/// (SEAM-053).
fn command<'a, I>(kind: &str, fields: I, images: Option<Value>) -> Map<String, Value>
where
    I: IntoIterator<Item = (&'a str, Value)>,
{
    let mut map = Map::new();
    map.insert("type".to_string(), json!(kind));
    for (k, v) in fields {
        map.insert(k.to_string(), v);
    }
    if let Some(images) = images {
        map.insert("images".to_string(), images);
    }
    map
}

/// `data.cancelled === true` — the shape pi's `newSession`/`switchSession`/`fork`/`clone` return
/// (`rpc-client.ts:227`, `:369`, `:378`, `:387`) and the host emits (`rpc.rs`,
/// `json!({ "cancelled": … })`).
fn cancelled(data: &Value) -> bool {
    data.get("cancelled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Pull one key out of a `data` object, yielding `null` when it is absent so the caller's
/// `from_value` produces the type error rather than a panic.
fn field(data: Value, key: &str) -> Value {
    match data {
        Value::Object(mut map) => map.remove(key).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// pi's `T | null` returns (`cycleModel`, `cycleThinkingLevel`): the host answers `data: null` when
/// there is nothing to report.
fn nullable(data: Value) -> Option<Value> {
    if data.is_null() {
        None
    } else {
        Some(data)
    }
}

/// pi `createProcessExitError` (`rpc-client.ts:528-530`), with JS's `null` rendering for an absent
/// code or signal.
fn process_exit_error(inner: &ClientInner, status: &std::process::ExitStatus) -> RpcClientError {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|s| s.to_string())
    };
    #[cfg(not(unix))]
    let signal: Option<String> = None;
    RpcClientError::ProcessExited {
        code: status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "null".to_string()),
        signal: signal.unwrap_or_else(|| "null".to_string()),
        stderr: inner.stderr_snapshot(),
    }
}

/// The stdout pump — pi's `attachJsonlLineReader(childProcess.stdout, this.handleLine)`
/// (`rpc-client.ts:127-129`).
///
/// At EOF the host is gone, so every in-flight request is settled — pi reaches the same state
/// through its `exit` handler's `rejectPendingRequests` (`:106-111`). Without this an embedder whose
/// child died mid-request would wait the full 30 s timeout instead of failing at once.
///
/// **[CYRUP-DELTA] — the EOF error renders `code=null signal=null`.** pi latches from the child's
/// `exit` event, which carries the real `(code, signal)` pair (`rpc-client.ts:106-111`). Here EOF on
/// the read half is the only signal available to this task: the `Child` lives behind
/// [`RpcClient::child`], which [`RpcClient::stop`] takes `&mut` on, so waiting for the status here
/// would hold that lock for the child's whole life and make `stop` unable to run. An
/// [`RpcClient::attach`]ed client has no status to report at all, and for a spawned one the status
/// is reported by [`RpcClient::spawn`]'s own start check (`:134-138`) — which is the path where a
/// non-zero code actually tells the embedder something. Everything else about the message, including
/// the accumulated stderr that usually carries the real cause, is pi's.
///
/// A read or decode FAILURE is a different termination and is reported as one: `next_line` yields
/// `Err(InvalidData)` for a non-UTF-8 byte and `Err` for any read failure, so collapsing it into the
/// EOF arm would hand the embedder `code=null signal=null` — "the agent process exited" — for a
/// child that is alive and healthy, and drop the real cause. The [`RpcClientError::Io`] latched here
/// wins because [`ClientInner::set_exit_error`] is first-error-wins, so the `ProcessExited` tail
/// below is a no-op on this path while still settling the in-flight requests.
async fn read_lines<R>(inner: Arc<ClientInner>, reader: R)
where
    R: AsyncBufRead + Send + Unpin + 'static,
{
    let mut lines = reader.lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => inner.handle_line(&line),
            Ok(None) => break, // clean EOF — the host is gone
            Err(e) => {
                inner.set_exit_error(RpcClientError::Io(e));
                break;
            }
        }
    }
    inner.set_exit_error(RpcClientError::ProcessExited {
        code: "null".to_string(),
        signal: "null".to_string(),
        stderr: inner.stderr_snapshot(),
    });
    inner.reject_pending_requests();
}

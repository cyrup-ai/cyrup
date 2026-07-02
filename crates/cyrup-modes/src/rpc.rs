//! RPC mode — the headless bidirectional stdio protocol (func-11 R-11-011…016; arch-11 §2.2/§3.5).
//!
//! A persistent line protocol other processes embed: incoming [`SessionCommand`] requests arrive as
//! strict-LF JSONL on a reader; the adapter drives the [`AgentSessionRuntime`] host (Pi
//! `rpc-mode.ts` `runtimeHost`) and emits [`RpcOut`] lines (a `response` per command + the full
//! agent/session event stream) on a writer. Both endpoints are parameters so tests drive an
//! in-memory reader/writer pair and the binary wires real stdio.
//!
//! ## Runtime host (R-11-019…023)
//! The session-replacing commands (`new_session`/`switch_session`/`fork`/`clone`) drive the
//! [`AgentSessionRuntime`] and then **rebind** — re-acquire the now-active session and re-subscribe
//! its event stream — exactly as Pi's `rebindSession()` (rpc-mode.ts:312-360). Every other command
//! operates on the active session (`runtime.session()`), the single integration seam.
//!
//! ## Framing (R-11-011)
//! Records are split on `\n` only (CRLF-tolerant: a trailing `\r` is stripped). We never rely on a
//! generic line reader that also splits on other Unicode separators inside JSON payloads.
//!
//! ## Streaming behaviour (R-11-016)
//! A `prompt` issued while the agent is already streaming MUST carry a `streamingBehavior`
//! (`steer` → queued after the current tool batch; `followUp` → after the agent goes idle); without
//! one it is rejected. While not streaming, `prompt` starts a fresh run. The active session's
//! `prompt_with` performs this preflight (the `input` ext event + steer/follow-up routing).

use std::collections::HashMap;

use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionRuntime, BashOptions, Content, EntryId,
    ForkPosition, InputSource, ModelThinkingLevel, PromptAccepted, PromptOptions, QueueMode,
    StreamingBehavior, UiKind, UiReply, UiRequest, UserInput,
};
use futures::{FutureExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::error::ModesError;

// ---------------------------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------------------------

/// The queue-drain mode argument (`all` | `one-at-a-time`; Pi `set_steering_mode`/`set_follow_up_mode`,
/// rpc-types.ts:41-42).
#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueModeArg {
    All,
    OneAtATime,
}

impl From<QueueModeArg> for QueueMode {
    fn from(m: QueueModeArg) -> Self {
        match m {
            QueueModeArg::All => QueueMode::All,
            QueueModeArg::OneAtATime => QueueMode::OneAtATime,
        }
    }
}

/// Render a [`QueueMode`] back to its Pi wire string.
fn queue_mode_str(mode: QueueMode) -> &'static str {
    match mode {
        QueueMode::All => "all",
        QueueMode::OneAtATime => "one-at-a-time",
    }
}

/// An incoming RPC request (`type`-tagged snake_case to match Pi clients; camelCase fields per the
/// wire; R-11-014, rpc-types.ts:20-72).
///
/// The request `id` is **not** a variant field: exactly as Pi reads `const id = command.id` once at
/// the top of `handleCommand` (rpc-mode.ts:383), cyrup recovers it from the raw parsed line in
/// [`dispatch`] (`raw_id`), preserved as-sent (string **or** number — Pi types `id?: string` but an
/// opaque number passes through untouched, R-11-015; #10). Keeping `id` off the variant means a
/// numeric-`id` command still deserializes and **executes** rather than tripping payload
/// validation. Unknown command types deserialize to [`SessionCommand::Unknown`] via `#[serde(other)]`
/// (detected in [`dispatch`], never reaching [`handle`]); a required field that is missing/wrong-typed
/// yields a serde error — both produce a `success:false` response, never a panic (R-00-009).
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionCommand {
    // ---- Prompting ----
    /// Submit a prompt. While streaming, `streamingBehavior` is required (R-11-016).
    Prompt {
        message: String,
        #[serde(default)]
        images: Vec<Content>,
        #[serde(default, rename = "streamingBehavior")]
        streaming_behavior: Option<StreamingBehavior>,
    },
    /// Enqueue a steering message (delivered after the current tool batch).
    Steer {
        message: String,
        #[serde(default)]
        images: Vec<Content>,
    },
    /// Enqueue a follow-up message (delivered after the agent goes idle).
    FollowUp {
        message: String,
        #[serde(default)]
        images: Vec<Content>,
    },
    /// Interrupt the active run (idempotent).
    Abort,
    /// Start a fresh session in the same cwd, optionally recording a `parentSession`.
    NewSession {
        #[serde(default, rename = "parentSession")]
        parent_session: Option<String>,
    },

    // ---- State ----
    /// Query the full snapshot of session state (rpc-types.ts:94-107).
    GetState,

    // ---- Model ----
    /// Switch the active model by `provider` + `modelId`.
    SetModel {
        provider: String,
        #[serde(rename = "modelId")]
        model_id: String,
    },
    /// Cycle to the next model in the scoped/available set.
    CycleModel,
    /// List the available models.
    GetAvailableModels,

    // ---- Thinking ----
    /// Set the thinking level (`off`|`minimal`|`low`|`medium`|`high`|`xhigh`).
    SetThinkingLevel { level: ModelThinkingLevel },
    /// Cycle to the next thinking level.
    CycleThinkingLevel,

    // ---- Queue modes ----
    /// Set the steering drain mode.
    SetSteeringMode { mode: QueueModeArg },
    /// Set the follow-up drain mode.
    SetFollowUpMode { mode: QueueModeArg },

    // ---- Compaction ----
    /// Compact the current branch.
    Compact {
        #[serde(default, rename = "customInstructions")]
        custom_instructions: Option<String>,
    },
    /// Toggle auto-compaction.
    SetAutoCompaction { enabled: bool },

    // ---- Retry ----
    /// Toggle auto-retry.
    SetAutoRetry { enabled: bool },
    /// Abort the pending auto-retry.
    AbortRetry,

    // ---- Bash ----
    /// Run an immediate bash command out of the agent loop.
    Bash {
        command: String,
        #[serde(default, rename = "excludeFromContext")]
        exclude_from_context: bool,
    },
    /// Cancel a running bash command.
    AbortBash,

    // ---- Session ----
    /// Aggregate transcript statistics for the current branch.
    GetSessionStats,
    /// Export the current branch to a standalone HTML document.
    ExportHtml {
        #[serde(default, rename = "outputPath")]
        output_path: Option<String>,
    },
    /// Resume a session file, rebuilding cwd-bound services.
    SwitchSession {
        #[serde(rename = "sessionPath")]
        session_path: String,
    },
    /// Fork at an entry into a new branched session (`position:"before"` returns the anchor text).
    Fork {
        #[serde(rename = "entryId")]
        entry_id: String,
    },
    /// Clone the current leaf at-position into a new session.
    Clone,
    /// The user-message fork anchors on the current branch.
    GetForkMessages,
    /// The persisted entries on the current branch (optionally `since` an entry id).
    GetEntries {
        #[serde(default)]
        since: Option<String>,
    },
    /// The full session tree.
    GetTree,
    /// The text of the last assistant message.
    GetLastAssistantText,
    /// Set the session display name (trimmed; empty rejected).
    SetSessionName { name: String },

    // ---- Messages ----
    /// Query the persisted transcript on the current branch.
    GetMessages,

    // ---- Commands ----
    /// List the slash commands available for invocation via a prompt.
    GetCommands,

    /// Any unrecognized `type` (R-00-009). Detected in [`dispatch`]; never reaches [`handle`].
    #[serde(other)]
    Unknown,
}

/// A correlated reply to a [`SessionCommand`] (arch-11 §3.5).
///
/// Field order is the exact byte layout Pi's `success`/`error` helpers emit
/// (`{ id, type, command, success, data|error }`, rpc-mode.ts:63-76): `id` **first** (omitted when
/// absent, so an id-less response byte-matches Pi's `{ type, command, ... }`), then the `type` tag,
/// the echoed `command`, the `success` flag, and finally the mutually-exclusive `data`/`error`. The
/// `command` is an owned `String` because the unknown-command / malformed-payload error paths echo
/// the caller's **real** `type` string, not one of the fixed verb literals (#7/#8).
#[derive(Debug, serde::Serialize)]
pub struct RpcResponse {
    /// Echoed request `id` for correlation, preserved as-is (string or number; R-11-015). Emitted
    /// first to match Pi's object literal (`{ id, type: "response", … }`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Always `"response"`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// Echoed command name (a fixed verb, or the caller's real `type` on the error paths).
    pub command: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RpcResponse {
    fn ok(command: impl Into<String>, id: Option<Value>, data: Option<Value>) -> Self {
        Self { id, kind: "response", command: command.into(), success: true, data, error: None }
    }

    fn err(command: impl Into<String>, id: Option<Value>, error: impl Into<String>) -> Self {
        Self {
            id,
            kind: "response",
            command: command.into(),
            success: false,
            data: None,
            error: Some(error.into()),
        }
    }
}

/// The two top-level shapes written on the protocol stream (arch-11 §3.5).
///
/// Serialized untagged: a `response` carries `"type":"response"`; an event carries its own
/// `AgentSessionEvent` `type` tag (`agent_start`, `tool_execution_end`, …) — distinct, so a client
/// dispatches on `type`. The event is boxed (it is the larger variant).
#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum RpcOut {
    Response(RpcResponse),
    Event(Box<AgentSessionEvent>),
    /// A synchronous extension dialog request (`ui.{confirm,input,select,editor}`) emitted on stdout
    /// for the RPC client to render + answer via an `extension_ui_response` (Pi
    /// `createExtensionUIContext` → `output({type:"extension_ui_request", …})`, rpc-mode.ts:128-160,
    /// 253-268). Carries the pre-shaped Pi wire object so field names/order match byte-for-byte.
    ExtensionUiRequest(Value),
}

/// A pending extension dialog awaiting its `extension_ui_response` (mirrors Pi's
/// `pendingExtensionRequests` map, rpc-mode.ts:79-82). `kind` is retained so a `{value}`/`{confirmed}`/
/// `{cancelled}` response can be mapped back to the guest's expected reply shape. `select`'s WIT
/// return is now the chosen option STRING (world.wit:259), byte-for-byte the Pi wire `value`
/// (rpc-types.ts:273) — no index translation, so no options bag needs to be retained here.
struct PendingUi {
    kind: UiKind,
    reply: oneshot::Sender<UiReply>,
}

/// Shape a guest [`UiRequest`] into the exact Pi `extension_ui_request` wire object
/// (rpc-types.ts:230-265). `id` correlates the later `extension_ui_response`.
fn extension_ui_request_json(id: &str, req: &UiRequest) -> Value {
    // Serialize a `{timeout}` field only when the guest supplied one (Pi omits it otherwise).
    let with_timeout = |mut v: Value| -> Value {
        if let (Some(ms), Some(obj)) = (req.opts.timeout_ms, v.as_object_mut()) {
            obj.insert("timeout".to_string(), json!(ms));
        }
        v
    };
    match req.kind {
        // Pi `select(title, options, opts)` → `{method:"select", title, options, timeout?}`.
        UiKind::Select => with_timeout(json!({
            "type": "extension_ui_request",
            "id": id,
            "method": "select",
            "title": req.prompt,
            "options": req.options,
        })),
        // Pi `confirm(title, message, opts)` → `{method:"confirm", title, message, timeout?}`. The
        // cyrup guest `confirm(prompt)` carries a single string → `title`; `message` is empty.
        UiKind::Confirm => with_timeout(json!({
            "type": "extension_ui_request",
            "id": id,
            "method": "confirm",
            "title": req.prompt,
            "message": "",
        })),
        // Pi `input(title, placeholder, opts)` → `{method:"input", title, timeout?}` (the cyrup WIT
        // `input(prompt)` has no placeholder).
        UiKind::Input => with_timeout(json!({
            "type": "extension_ui_request",
            "id": id,
            "method": "input",
            "title": req.prompt,
        })),
        // Pi `editor(title, prefill)` → `{method:"editor", prefill}`. The cyrup WIT `editor(initial)`
        // carries only the seed text → `prefill`.
        UiKind::Editor => json!({
            "type": "extension_ui_request",
            "id": id,
            "method": "editor",
            "title": "",
            "prefill": req.prompt,
        }),
    }
}

/// Map an `extension_ui_response` body onto the guest's expected [`UiReply`] for `pending` (Pi
/// `parseResponse`, rpc-mode.ts:137-149,257-264). A `{cancelled:true}` yields the per-kind default; a
/// `{confirmed}` a confirm; a `{value}` maps straight to text (input/editor/select) — Pi's
/// `select(...): Promise<string|undefined>` (types.ts:127) passes the chosen STRING straight through
/// to the guest, with NO index translation.
fn map_ui_response(pending: &PendingUi, body: &Value) -> UiReply {
    let cancelled = body.get("cancelled").and_then(Value::as_bool) == Some(true);
    match pending.kind {
        UiKind::Confirm => {
            if cancelled {
                return UiReply::Confirm(false);
            }
            UiReply::Confirm(body.get("confirmed").and_then(Value::as_bool).unwrap_or(false))
        }
        UiKind::Input | UiKind::Editor | UiKind::Select => {
            if cancelled {
                return UiReply::Text(None);
            }
            UiReply::Text(body.get("value").and_then(Value::as_str).map(str::to_owned))
        }
    }
}

/// The disposition of a dispatched command: the correlated [`RpcResponse`] plus whether the active
/// session was replaced (the loop must rebind: re-acquire the session + re-subscribe; R-11-021).
struct Dispatched {
    response: RpcResponse,
    rebind: bool,
}

// ---------------------------------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------------------------------

/// Serve the RPC protocol over `reader` (commands in) and `writer` (responses + events out), driving
/// the [`AgentSessionRuntime`] host.
///
/// Reads strict-LF JSONL requests, drives the active session, and streams every
/// [`AgentSessionEvent`] (agent + session-level) back as it occurs. A session-replacing command
/// rebinds: the active session + its event subscription are re-acquired from the runtime (Pi
/// `rebindSession`). Returns once the reader reaches EOF *and* no run is in flight. A dedicated
/// reader task keeps line parsing cancel-safe against the concurrent event stream; the writer is
/// owned by the loop so its writes never interleave.
pub async fn run_rpc<R, W>(
    runtime: &AgentSessionRuntime,
    reader: R,
    writer: &mut W,
) -> Result<(), ModesError>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    // The active session + its event subscription (re-acquired on every replacement).
    let mut session = runtime.session().await;
    let mut events = session.subscribe();

    // The synchronous extension-dialog sink (mode #4): a loaded guest's `ui.{confirm,input,select,
    // editor}` capability blocks on a one-shot while this loop emits an `extension_ui_request` and
    // awaits the client's `extension_ui_response` (Pi `createExtensionUIContext`, rpc-mode.ts:135-160).
    // Installed on the active session's `LiveHostServices` (re-installed on every rebind, since a
    // replacement brings a fresh backend). `pending` mirrors Pi's `pendingExtensionRequests`.
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiRequest>();
    session.services().host_services.set_ui_sink(ui_tx.clone());
    let mut pending: HashMap<String, PendingUi> = HashMap::new();

    // Dedicated reader task → mpsc of raw JSONL lines (strict LF framing; cancel-safe vs. events).
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
    let reader_task = tokio::spawn(read_lines(reader, cmd_tx));

    let mut reader_open = true;
    // True from the moment a run is accepted until its `agent_end` is observed.
    let mut in_flight = false;

    loop {
        tokio::select! {
            maybe_line = cmd_rx.recv(), if reader_open => {
                match maybe_line {
                    Some(line) => {
                        // Intercept an `extension_ui_response` BEFORE command dispatch (Pi
                        // `handleInputLine`, rpc-mode.ts:739-753): look up the pending dialog by `id`,
                        // resolve its one-shot, and never route it to the command switch.
                        if let Some(id) = extension_ui_response_id(&line) {
                            if let Some(p) = pending.remove(&id) {
                                let body: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                                let reply = map_ui_response(&p, &body);
                                let _ = p.reply.send(reply);
                            }
                            continue;
                        }
                        let dispatched = dispatch(runtime, &session, &line, &mut in_flight).await;
                        write_out(writer, &RpcOut::Response(dispatched.response)).await?;
                        if dispatched.rebind {
                            // Pi `rebindSession`: the active session was replaced — re-acquire it
                            // and re-subscribe (the prior subscription was terminated, R-11-021).
                            session = runtime.session().await;
                            events = session.subscribe();
                            // The replacement brought a fresh `LiveHostServices`; re-install the ui
                            // sink so a post-swap guest dialog still reaches this loop.
                            session.services().host_services.set_ui_sink(ui_tx.clone());
                            in_flight = false;
                        }
                    }
                    None => reader_open = false,
                }
            }
            Some(req) = ui_rx.recv() => {
                // A guest opened a dialog: allocate a correlation id, emit the Pi `extension_ui_request`
                // on stdout, and stash the one-shot until the client's `extension_ui_response` arrives.
                let id = new_request_id();
                let wire = extension_ui_request_json(&id, &req);
                pending.insert(id, PendingUi { kind: req.kind, reply: req.reply });
                write_out(writer, &RpcOut::ExtensionUiRequest(wire)).await?;
            }
            maybe_ev = events.next() => {
                if let Some(ev) = maybe_ev {
                    if matches!(ev, AgentSessionEvent::AgentEnd { .. }) {
                        in_flight = false;
                    }
                    // The internal `SessionReplaced` terminal is a rebind signal, not a Pi event.
                    if !matches!(ev, AgentSessionEvent::SessionReplaced { .. }) {
                        write_out(writer, &RpcOut::Event(Box::new(ev))).await?;
                    }
                }
            }
        }

        if !reader_open && !in_flight {
            // Flush any events already buffered on the channel, then shut down cleanly.
            while let Some(Some(ev)) = events.next().now_or_never() {
                if !matches!(ev, AgentSessionEvent::SessionReplaced { .. }) {
                    write_out(writer, &RpcOut::Event(Box::new(ev))).await?;
                }
            }
            break;
        }
    }

    // The reader task ends on its own at EOF; this just reaps it.
    reader_task.abort();
    Ok(())
}

/// Decode one request line and apply it, in the same **staged** order Pi's `handleInputLine` +
/// `handleCommand` use (rpc-mode.ts:723-773, 382-689). Side effect: a freshly-started run sets
/// `in_flight`.
///
/// 1. **Parse** the line as JSON (`JSON.parse`, rpc-mode.ts:726). A syntax error is *not* a command:
///    Pi emits `error(undefined, "parse", "Failed to parse command: …")` with **no** id — `JSON.parse`
///    itself failed, so there is no object to recover an id from (rpc-mode.ts:728-734). #6.
/// 2. Recover the `id` from the parsed object (`const id = command.id`, rpc-mode.ts:383), preserved
///    exactly as sent — string **or** number (#10); `null`/absent → no id.
/// 3. **Deserialize** the command. An unknown `type` tag hits Pi's `switch` default:
///    `error(id, command.type, "Unknown command: <type>")` echoing the **real** type (rpc-mode.ts:686-689).
///    #7. A recognized type whose payload is missing/wrong-typed a required field surfaces as a runtime
///    error under `handleCommand`, caught as `error(id, command.type, <message>)` — again the **real**
///    command name, not `"unknown"` (rpc-mode.ts:755-772). #8.
async fn dispatch(
    runtime: &AgentSessionRuntime,
    session: &AgentSession,
    line: &str,
    in_flight: &mut bool,
) -> Dispatched {
    // (1) Parse the raw line. A malformed line is Pi's `"parse"` error with NO id.
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Dispatched {
                response: RpcResponse::err(
                    "parse",
                    None,
                    format!("Failed to parse command: {e}"),
                ),
                rebind: false,
            }
        }
    };

    // (2) Recover the id and the `type` discriminant before consuming `value`. The id is preserved
    // as-sent (string or number); the type string is what Pi echoes on the error paths.
    let raw_id = value.get("id").filter(|id| !id.is_null()).cloned();
    let type_str = value.get("type").and_then(Value::as_str).map(str::to_owned);

    // (3) Deserialize the recognized command's payload.
    match serde_json::from_value::<SessionCommand>(value) {
        // Recognized-shape line with an unknown `type` tag (`#[serde(other)]`): echo the real type.
        Ok(SessionCommand::Unknown) => {
            let name = type_str.unwrap_or_default();
            let message = format!("Unknown command: {name}");
            Dispatched { response: RpcResponse::err(name, raw_id, message), rebind: false }
        }
        Ok(cmd) => {
            let response = handle(runtime, session, cmd, raw_id, in_flight).await;
            // The session-replacing commands rebind on success (non-cancelled).
            let rebind = response.success
                && matches!(
                    response.command.as_str(),
                    "new_session" | "switch_session" | "fork" | "clone"
                )
                && response
                    .data
                    .as_ref()
                    .and_then(|d| d.get("cancelled"))
                    .and_then(Value::as_bool)
                    != Some(true);
            Dispatched { response, rebind }
        }
        // A known `type` whose payload failed validation (missing/wrong-typed required field): echo
        // the real command name + the runtime error, NOT `"unknown"`. A missing/`null` `type` tag
        // (serde: "missing field `type`") has no command name to echo — fall back to Pi's default
        // `Unknown command` shaping so it still correlates.
        Err(e) => match type_str {
            Some(name) => Dispatched {
                response: RpcResponse::err(name, raw_id, e.to_string()),
                rebind: false,
            },
            None => Dispatched {
                response: RpcResponse::err(
                    String::new(),
                    raw_id,
                    "Unknown command: undefined",
                ),
                rebind: false,
            },
        },
    }
}

#[allow(clippy::too_many_lines)] // a faithful 1:1 of Pi's `handleCommand` switch (rpc-mode.ts:385).
async fn handle(
    runtime: &AgentSessionRuntime,
    session: &AgentSession,
    cmd: SessionCommand,
    raw_id: Option<Value>,
    in_flight: &mut bool,
) -> RpcResponse {
    // Pi reads the id once at the top of `handleCommand` (`const id = command.id`, rpc-mode.ts:383);
    // cyrup recovered it in `dispatch` and threads it in as `raw_id`. Each arm clones it into the
    // reply (string or number, preserved as-sent).
    match cmd {
        // -------------------------------------------------------------- Prompting ----
        SessionCommand::Prompt { message, images, streaming_behavior } => {
            let id = raw_id.clone();
            let input = user_input(message, images);
            match session.prompt_with(input, PromptOptions { streaming_behavior }).await {
                Ok(accepted) => {
                    if !matches!(accepted, PromptAccepted::Handled) {
                        *in_flight = true;
                    }
                    RpcResponse::ok("prompt", id, None)
                }
                Err(e) => RpcResponse::err("prompt", id, e.to_string()),
            }
        }
        SessionCommand::Steer { message, images } => {
            let id = raw_id.clone();
            *in_flight = true;
            match session.steer(user_input(message, images)).await {
                Ok(_) => RpcResponse::ok("steer", id, None),
                Err(e) => RpcResponse::err("steer", id, e.to_string()),
            }
        }
        SessionCommand::FollowUp { message, images } => {
            let id = raw_id.clone();
            *in_flight = true;
            match session.follow_up(user_input(message, images)).await {
                Ok(_) => RpcResponse::ok("follow_up", id, None),
                Err(e) => RpcResponse::err("follow_up", id, e.to_string()),
            }
        }
        SessionCommand::Abort => {
            session.abort();
            RpcResponse::ok("abort", raw_id.clone(), None)
        }
        SessionCommand::NewSession { parent_session } => {
            let id = raw_id.clone();
            let options = cyrup_session_svc::NewSessionOptions { parent_session };
            match runtime.new_session_with(options).await {
                Ok(result) => {
                    RpcResponse::ok("new_session", id, Some(json!({ "cancelled": result.cancelled })))
                }
                Err(e) => RpcResponse::err("new_session", id, e.to_string()),
            }
        }

        // ------------------------------------------------------------------ State ----
        SessionCommand::GetState => {
            RpcResponse::ok("get_state", raw_id.clone(), Some(state_view(session).await))
        }

        // ------------------------------------------------------------------ Model ----
        SessionCommand::SetModel { provider, model_id } => {
            let id = raw_id.clone();
            let found = session
                .model_catalog()
                .iter()
                .find(|m| m.provider.as_str() == provider && m.id.as_str() == model_id)
                .cloned();
            match found {
                Some(model) => {
                    let model_json = serde_json::to_value(&model).unwrap_or(Value::Null);
                    match session.set_model_resolved(model).await {
                        Ok(_) => RpcResponse::ok("set_model", id, Some(model_json)),
                        Err(e) => RpcResponse::err("set_model", id, e.to_string()),
                    }
                }
                None => RpcResponse::err(
                    "set_model",
                    id,
                    format!("Model not found: {provider}/{model_id}"),
                ),
            }
        }
        SessionCommand::CycleModel => {
            let id = raw_id.clone();
            match session.cycle_model(true).await {
                Ok(Some(result)) => RpcResponse::ok(
                    "cycle_model",
                    id,
                    Some(json!({
                        "model": serde_json::to_value(&result.model).unwrap_or(Value::Null),
                        "thinkingLevel": result.thinking_level,
                        "isScoped": result.is_scoped,
                    })),
                ),
                Ok(None) => RpcResponse::ok("cycle_model", id, Some(Value::Null)),
                Err(e) => RpcResponse::err("cycle_model", id, e.to_string()),
            }
        }
        SessionCommand::GetAvailableModels => {
            let models = serde_json::to_value(session.model_catalog()).unwrap_or(json!([]));
            RpcResponse::ok(
                "get_available_models",
                raw_id.clone(),
                Some(json!({ "models": models })),
            )
        }

        // --------------------------------------------------------------- Thinking ----
        SessionCommand::SetThinkingLevel { level } => {
            let id = raw_id.clone();
            match session.set_thinking_level(level).await {
                Ok(_) => RpcResponse::ok("set_thinking_level", id, None),
                Err(e) => RpcResponse::err("set_thinking_level", id, e.to_string()),
            }
        }
        SessionCommand::CycleThinkingLevel => {
            let id = raw_id.clone();
            match session.cycle_thinking_level().await {
                Ok(Some(level)) => {
                    RpcResponse::ok("cycle_thinking_level", id, Some(json!({ "level": level })))
                }
                Ok(None) => RpcResponse::ok("cycle_thinking_level", id, Some(Value::Null)),
                Err(e) => RpcResponse::err("cycle_thinking_level", id, e.to_string()),
            }
        }

        // ------------------------------------------------------------ Queue modes ----
        SessionCommand::SetSteeringMode { mode } => {
            session.set_steering_mode(mode.into());
            RpcResponse::ok("set_steering_mode", raw_id.clone(), None)
        }
        SessionCommand::SetFollowUpMode { mode } => {
            session.set_follow_up_mode(mode.into());
            RpcResponse::ok("set_follow_up_mode", raw_id.clone(), None)
        }

        // ------------------------------------------------------------- Compaction ----
        SessionCommand::Compact { custom_instructions } => {
            let id = raw_id.clone();
            match session.compact(custom_instructions).await {
                Ok(result) => RpcResponse::ok(
                    "compact",
                    id,
                    Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                ),
                Err(e) => RpcResponse::err("compact", id, e.to_string()),
            }
        }
        SessionCommand::SetAutoCompaction { enabled } => {
            session.set_auto_compaction_enabled(enabled);
            RpcResponse::ok("set_auto_compaction", raw_id.clone(), None)
        }

        // ------------------------------------------------------------------ Retry ----
        SessionCommand::SetAutoRetry { enabled } => {
            session.set_auto_retry_enabled(enabled);
            RpcResponse::ok("set_auto_retry", raw_id.clone(), None)
        }
        SessionCommand::AbortRetry => {
            session.abort_retry();
            RpcResponse::ok("abort_retry", raw_id.clone(), None)
        }

        // ------------------------------------------------------------------- Bash ----
        SessionCommand::Bash { command, exclude_from_context } => {
            let id = raw_id.clone();
            let result = session
                .execute_bash(&command, BashOptions { exclude_from_context }, None)
                .await;
            RpcResponse::ok("bash", id, Some(serde_json::to_value(result).unwrap_or(Value::Null)))
        }
        SessionCommand::AbortBash => {
            session.abort_bash();
            RpcResponse::ok("abort_bash", raw_id.clone(), None)
        }

        // ---------------------------------------------------------------- Session ----
        SessionCommand::GetSessionStats => {
            let stats = serde_json::to_value(session.session_stats().await).unwrap_or(Value::Null);
            RpcResponse::ok("get_session_stats", raw_id.clone(), Some(stats))
        }
        SessionCommand::ExportHtml { output_path } => {
            let id = raw_id.clone();
            let path = output_path.map(std::path::PathBuf::from);
            match session.export_to_html(path.as_deref()).await {
                Ok(out) => {
                    RpcResponse::ok("export_html", id, Some(json!({ "path": out.display().to_string() })))
                }
                Err(e) => RpcResponse::err("export_html", id, e.to_string()),
            }
        }
        SessionCommand::SwitchSession { session_path } => {
            let id = raw_id.clone();
            match runtime.switch_session(session_path).await {
                Ok(result) => RpcResponse::ok(
                    "switch_session",
                    id,
                    Some(json!({ "cancelled": result.cancelled })),
                ),
                Err(e) => RpcResponse::err("switch_session", id, e.to_string()),
            }
        }
        SessionCommand::Fork { entry_id } => {
            let id = raw_id.clone();
            match runtime.fork(EntryId::from(entry_id.as_str()), ForkPosition::Before).await {
                Ok(result) => RpcResponse::ok(
                    "fork",
                    id,
                    Some(json!({ "text": result.selected_text, "cancelled": result.cancelled })),
                ),
                Err(e) => RpcResponse::err("fork", id, e.to_string()),
            }
        }
        SessionCommand::Clone => {
            let id = raw_id.clone();
            let leaf = session.leaf_id().await;
            match leaf {
                None => RpcResponse::err(
                    "clone",
                    id,
                    "Cannot clone session: no current entry selected",
                ),
                Some(leaf) => match runtime.fork(leaf, ForkPosition::At).await {
                    Ok(result) => RpcResponse::ok(
                        "clone",
                        id,
                        Some(json!({ "cancelled": result.cancelled })),
                    ),
                    Err(e) => RpcResponse::err("clone", id, e.to_string()),
                },
            }
        }
        SessionCommand::GetForkMessages => {
            let messages: Vec<Value> = session
                .user_messages_for_forking()
                .await
                .into_iter()
                .map(|a| json!({ "entryId": a.entry_id.as_str(), "text": a.text }))
                .collect();
            RpcResponse::ok(
                "get_fork_messages",
                raw_id.clone(),
                Some(json!({ "messages": messages })),
            )
        }
        SessionCommand::GetEntries { since } => {
            let id = raw_id.clone();
            let mut entries = session.entries_json().await;
            if let Some(since) = since {
                match entries.iter().position(|e| e.get("id").and_then(Value::as_str) == Some(since.as_str())) {
                    Some(idx) => entries = entries.split_off(idx + 1),
                    None => return RpcResponse::err("get_entries", id, format!("Entry not found: {since}")),
                }
            }
            let leaf = session.leaf_id().await.map(|l| l.as_str().to_string());
            RpcResponse::ok("get_entries", id, Some(json!({ "entries": entries, "leafId": leaf })))
        }
        SessionCommand::GetTree => {
            let tree = session.tree_json().await;
            let leaf = session.leaf_id().await.map(|l| l.as_str().to_string());
            RpcResponse::ok(
                "get_tree",
                raw_id.clone(),
                Some(json!({ "tree": tree, "leafId": leaf })),
            )
        }
        SessionCommand::GetLastAssistantText => {
            let text = session.last_assistant_text().await;
            RpcResponse::ok(
                "get_last_assistant_text",
                raw_id.clone(),
                Some(json!({ "text": text })),
            )
        }
        SessionCommand::SetSessionName { name } => {
            let id = raw_id.clone();
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return RpcResponse::err("set_session_name", id, "Session name cannot be empty");
            }
            match session.set_session_name(trimmed).await {
                Ok(_) => RpcResponse::ok("set_session_name", id, None),
                Err(e) => RpcResponse::err("set_session_name", id, e.to_string()),
            }
        }

        // --------------------------------------------------------------- Messages ----
        SessionCommand::GetMessages => {
            let id = raw_id.clone();
            match serde_json::to_value(session.messages().await) {
                Ok(v) => RpcResponse::ok("get_messages", id, Some(json!({ "messages": v }))),
                Err(e) => RpcResponse::err("get_messages", id, e.to_string()),
            }
        }

        // --------------------------------------------------------------- Commands ----
        SessionCommand::GetCommands => RpcResponse::ok(
            "get_commands",
            raw_id.clone(),
            Some(json!({ "commands": session.slash_command_catalog() })),
        ),

        // Unreachable: `dispatch` intercepts the `#[serde(other)]` unknown-type variant before it
        // reaches `handle` (Pi's `switch` default, rpc-mode.ts:686-689). Kept for exhaustiveness —
        // defensively echoes the id rather than panicking (R-00-009).
        SessionCommand::Unknown => {
            RpcResponse::err(String::new(), raw_id.clone(), "Unknown command: undefined")
        }
    }
}

/// A fresh correlation id for an `extension_ui_request` (Pi `crypto.randomUUID`, rpc-mode.ts:98). A
/// process-monotonic counter suffices: the id is opaque and only has to be unique among the dialogs
/// in flight on this loop, and the client echoes it back verbatim on the `extension_ui_response`.
fn new_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("ext-ui-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// If `line` is an `extension_ui_response` envelope, return its correlation `id` (Pi intercepts these
/// before command dispatch, rpc-mode.ts:739-753). Returns `None` for any other line so it falls
/// through to the normal command path.
fn extension_ui_response_id(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("extension_ui_response") {
        return None;
    }
    value.get("id").and_then(Value::as_str).map(str::to_owned)
}

/// Build an RPC-sourced [`UserInput`] from text + optional image content blocks.
fn user_input(text: String, images: Vec<Content>) -> UserInput {
    UserInput { text, images, source: InputSource::Rpc, expand_templates: true }
}

/// The full `get_state` snapshot (Pi `RpcSessionState`, rpc-types.ts:94-107).
async fn state_view(session: &AgentSession) -> Value {
    let model_ref = session.model();
    // The full `Model` from the catalog (Pi `session.model`), else a minimal `{provider, id}`.
    let model = session
        .model_catalog()
        .iter()
        .find(|m| m.provider == model_ref.provider && m.id == model_ref.model)
        .and_then(|m| serde_json::to_value(m).ok())
        .unwrap_or_else(|| json!({
            "provider": model_ref.provider.as_str(),
            "id": model_ref.model.as_str(),
        }));
    json!({
        "model": model,
        "thinkingLevel": session.thinking_level().await,
        "isStreaming": session.is_streaming().await,
        "isCompacting": session.is_compacting(),
        "steeringMode": queue_mode_str(session.steering_mode()),
        "followUpMode": queue_mode_str(session.follow_up_mode()),
        "sessionFile": session.session_file().await.map(|p| p.display().to_string()),
        "sessionId": session.session_id().as_str(),
        "sessionName": session.session_name().await,
        "autoCompactionEnabled": session.auto_compaction_enabled(),
        "messageCount": session.messages().await.len(),
        "pendingMessageCount": session.pending_message_count(),
    })
}

/// Serialize one protocol record and write it as a single LF-terminated line, flushed immediately so
/// the peer never waits on buffering (R-11-013).
async fn write_out<W: AsyncWrite + Unpin>(writer: &mut W, out: &RpcOut) -> Result<(), ModesError> {
    let mut line = serde_json::to_string(out)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Read strict-LF JSONL lines from `reader` and forward each non-empty record over `tx`. Splits on
/// `\n` only; a trailing `\r` is stripped (CRLF tolerance). Ends at EOF or when the receiver drops.
async fn read_lines<R: AsyncBufRead + Unpin>(mut reader: R, tx: mpsc::Sender<String>) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                }
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                if buf.is_empty() {
                    continue;
                }
                let line = String::from_utf8_lossy(&buf).into_owned();
                if tx.send(line).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

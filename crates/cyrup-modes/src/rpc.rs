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

use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionRuntime, BashOptions, Content, EntryId,
    ForkPosition, InputSource, ModelThinkingLevel, PromptAccepted, PromptOptions, QueueMode,
    StreamingBehavior, UserInput,
};
use futures::{FutureExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

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
/// Every variant carries an optional `id` echoed back on its [`RpcResponse`] for correlation
/// (R-11-015). Unknown command types deserialize to [`SessionCommand::Unknown`] and yield a
/// `success:false` response — never a panic (R-00-009).
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionCommand {
    // ---- Prompting ----
    /// Submit a prompt. While streaming, `streamingBehavior` is required (R-11-016).
    Prompt {
        #[serde(default)]
        id: Option<String>,
        message: String,
        #[serde(default)]
        images: Vec<Content>,
        #[serde(default, rename = "streamingBehavior")]
        streaming_behavior: Option<StreamingBehavior>,
    },
    /// Enqueue a steering message (delivered after the current tool batch).
    Steer {
        #[serde(default)]
        id: Option<String>,
        message: String,
        #[serde(default)]
        images: Vec<Content>,
    },
    /// Enqueue a follow-up message (delivered after the agent goes idle).
    FollowUp {
        #[serde(default)]
        id: Option<String>,
        message: String,
        #[serde(default)]
        images: Vec<Content>,
    },
    /// Interrupt the active run (idempotent).
    Abort {
        #[serde(default)]
        id: Option<String>,
    },
    /// Start a fresh session in the same cwd, optionally recording a `parentSession`.
    NewSession {
        #[serde(default)]
        id: Option<String>,
        #[serde(default, rename = "parentSession")]
        parent_session: Option<String>,
    },

    // ---- State ----
    /// Query the full snapshot of session state (rpc-types.ts:94-107).
    GetState {
        #[serde(default)]
        id: Option<String>,
    },

    // ---- Model ----
    /// Switch the active model by `provider` + `modelId`.
    SetModel {
        #[serde(default)]
        id: Option<String>,
        provider: String,
        #[serde(rename = "modelId")]
        model_id: String,
    },
    /// Cycle to the next model in the scoped/available set.
    CycleModel {
        #[serde(default)]
        id: Option<String>,
    },
    /// List the available models.
    GetAvailableModels {
        #[serde(default)]
        id: Option<String>,
    },

    // ---- Thinking ----
    /// Set the thinking level (`off`|`minimal`|`low`|`medium`|`high`|`xhigh`).
    SetThinkingLevel {
        #[serde(default)]
        id: Option<String>,
        level: ModelThinkingLevel,
    },
    /// Cycle to the next thinking level.
    CycleThinkingLevel {
        #[serde(default)]
        id: Option<String>,
    },

    // ---- Queue modes ----
    /// Set the steering drain mode.
    SetSteeringMode {
        #[serde(default)]
        id: Option<String>,
        mode: QueueModeArg,
    },
    /// Set the follow-up drain mode.
    SetFollowUpMode {
        #[serde(default)]
        id: Option<String>,
        mode: QueueModeArg,
    },

    // ---- Compaction ----
    /// Compact the current branch.
    Compact {
        #[serde(default)]
        id: Option<String>,
        #[serde(default, rename = "customInstructions")]
        custom_instructions: Option<String>,
    },
    /// Toggle auto-compaction.
    SetAutoCompaction {
        #[serde(default)]
        id: Option<String>,
        enabled: bool,
    },

    // ---- Retry ----
    /// Toggle auto-retry.
    SetAutoRetry {
        #[serde(default)]
        id: Option<String>,
        enabled: bool,
    },
    /// Abort the pending auto-retry.
    AbortRetry {
        #[serde(default)]
        id: Option<String>,
    },

    // ---- Bash ----
    /// Run an immediate bash command out of the agent loop.
    Bash {
        #[serde(default)]
        id: Option<String>,
        command: String,
        #[serde(default, rename = "excludeFromContext")]
        exclude_from_context: bool,
    },
    /// Cancel a running bash command.
    AbortBash {
        #[serde(default)]
        id: Option<String>,
    },

    // ---- Session ----
    /// Aggregate transcript statistics for the current branch.
    GetSessionStats {
        #[serde(default)]
        id: Option<String>,
    },
    /// Export the current branch to a standalone HTML document.
    ExportHtml {
        #[serde(default)]
        id: Option<String>,
        #[serde(default, rename = "outputPath")]
        output_path: Option<String>,
    },
    /// Resume a session file, rebuilding cwd-bound services.
    SwitchSession {
        #[serde(default)]
        id: Option<String>,
        #[serde(rename = "sessionPath")]
        session_path: String,
    },
    /// Fork at an entry into a new branched session (`position:"before"` returns the anchor text).
    Fork {
        #[serde(default)]
        id: Option<String>,
        #[serde(rename = "entryId")]
        entry_id: String,
    },
    /// Clone the current leaf at-position into a new session.
    Clone {
        #[serde(default)]
        id: Option<String>,
    },
    /// The user-message fork anchors on the current branch.
    GetForkMessages {
        #[serde(default)]
        id: Option<String>,
    },
    /// The persisted entries on the current branch (optionally `since` an entry id).
    GetEntries {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        since: Option<String>,
    },
    /// The full session tree.
    GetTree {
        #[serde(default)]
        id: Option<String>,
    },
    /// The text of the last assistant message.
    GetLastAssistantText {
        #[serde(default)]
        id: Option<String>,
    },
    /// Set the session display name (trimmed; empty rejected).
    SetSessionName {
        #[serde(default)]
        id: Option<String>,
        name: String,
    },

    // ---- Messages ----
    /// Query the persisted transcript on the current branch.
    GetMessages {
        #[serde(default)]
        id: Option<String>,
    },

    // ---- Commands ----
    /// List the slash commands available for invocation via a prompt.
    GetCommands {
        #[serde(default)]
        id: Option<String>,
    },

    /// Any unrecognized `type` (R-00-009).
    #[serde(other)]
    Unknown,
}

/// A correlated reply to a [`SessionCommand`] (arch-11 §3.5).
#[derive(Debug, serde::Serialize)]
pub struct RpcResponse {
    /// Always `"response"`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// Echoed command name.
    pub command: &'static str,
    pub success: bool,
    /// Echoed request `id` for correlation, preserved as-is (string or number; R-11-015).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RpcResponse {
    fn ok(command: &'static str, id: Option<Value>, data: Option<Value>) -> Self {
        Self { kind: "response", command, success: true, id, data, error: None }
    }

    fn err(command: &'static str, id: Option<Value>, error: impl Into<String>) -> Self {
        Self { kind: "response", command, success: false, id, data: None, error: Some(error.into()) }
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
                        let dispatched = dispatch(runtime, &session, &line, &mut in_flight).await;
                        write_out(writer, &RpcOut::Response(dispatched.response)).await?;
                        if dispatched.rebind {
                            // Pi `rebindSession`: the active session was replaced — re-acquire it
                            // and re-subscribe (the prior subscription was terminated, R-11-021).
                            session = runtime.session().await;
                            events = session.subscribe();
                            in_flight = false;
                        }
                    }
                    None => reader_open = false,
                }
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

/// Decode one request line and apply it. Side effect: a freshly-started run sets `in_flight`.
async fn dispatch(
    runtime: &AgentSessionRuntime,
    session: &AgentSession,
    line: &str,
    in_flight: &mut bool,
) -> Dispatched {
    // Recover the request `id` from the raw JSON FIRST so even an unknown/unmappable command can be
    // correlated (R-11-015). Preserved exactly as sent (string or number); `null`/absent → no id.
    let raw_id = extract_id(line);

    let cmd: SessionCommand = match serde_json::from_str(line) {
        Ok(c) => c,
        // A JSON object carrying an id MUST echo it even when it fails to map to a known command.
        Err(e) => {
            return Dispatched {
                response: RpcResponse::err("unknown", raw_id, format!("invalid command json: {e}")),
                rebind: false,
            }
        }
    };

    let response = handle(runtime, session, cmd, raw_id, in_flight).await;
    // The session-replacing commands rebind on success (non-cancelled).
    let rebind = response.success
        && matches!(response.command, "new_session" | "switch_session" | "fork" | "clone")
        && response
            .data
            .as_ref()
            .and_then(|d| d.get("cancelled"))
            .and_then(Value::as_bool)
            != Some(true);
    Dispatched { response, rebind }
}

#[allow(clippy::too_many_lines)] // a faithful 1:1 of Pi's `handleCommand` switch (rpc-mode.ts:385).
async fn handle(
    runtime: &AgentSessionRuntime,
    session: &AgentSession,
    cmd: SessionCommand,
    raw_id: Option<Value>,
    in_flight: &mut bool,
) -> RpcResponse {
    match cmd {
        // -------------------------------------------------------------- Prompting ----
        SessionCommand::Prompt { id, message, images, streaming_behavior } => {
            let id = id.map(Value::String);
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
        SessionCommand::Steer { id, message, images } => {
            let id = id.map(Value::String);
            *in_flight = true;
            match session.steer(user_input(message, images)).await {
                Ok(_) => RpcResponse::ok("steer", id, None),
                Err(e) => RpcResponse::err("steer", id, e.to_string()),
            }
        }
        SessionCommand::FollowUp { id, message, images } => {
            let id = id.map(Value::String);
            *in_flight = true;
            match session.follow_up(user_input(message, images)).await {
                Ok(_) => RpcResponse::ok("follow_up", id, None),
                Err(e) => RpcResponse::err("follow_up", id, e.to_string()),
            }
        }
        SessionCommand::Abort { id } => {
            session.abort();
            RpcResponse::ok("abort", id.map(Value::String), None)
        }
        SessionCommand::NewSession { id, parent_session } => {
            let id = id.map(Value::String);
            let options = cyrup_session_svc::NewSessionOptions { parent_session };
            match runtime.new_session_with(options).await {
                Ok(result) => {
                    RpcResponse::ok("new_session", id, Some(json!({ "cancelled": result.cancelled })))
                }
                Err(e) => RpcResponse::err("new_session", id, e.to_string()),
            }
        }

        // ------------------------------------------------------------------ State ----
        SessionCommand::GetState { id } => {
            RpcResponse::ok("get_state", id.map(Value::String), Some(state_view(session).await))
        }

        // ------------------------------------------------------------------ Model ----
        SessionCommand::SetModel { id, provider, model_id } => {
            let id = id.map(Value::String);
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
        SessionCommand::CycleModel { id } => {
            let id = id.map(Value::String);
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
        SessionCommand::GetAvailableModels { id } => {
            let models = serde_json::to_value(session.model_catalog()).unwrap_or(json!([]));
            RpcResponse::ok(
                "get_available_models",
                id.map(Value::String),
                Some(json!({ "models": models })),
            )
        }

        // --------------------------------------------------------------- Thinking ----
        SessionCommand::SetThinkingLevel { id, level } => {
            let id = id.map(Value::String);
            match session.set_thinking_level(level).await {
                Ok(_) => RpcResponse::ok("set_thinking_level", id, None),
                Err(e) => RpcResponse::err("set_thinking_level", id, e.to_string()),
            }
        }
        SessionCommand::CycleThinkingLevel { id } => {
            let id = id.map(Value::String);
            match session.cycle_thinking_level().await {
                Ok(Some(level)) => {
                    RpcResponse::ok("cycle_thinking_level", id, Some(json!({ "level": level })))
                }
                Ok(None) => RpcResponse::ok("cycle_thinking_level", id, Some(Value::Null)),
                Err(e) => RpcResponse::err("cycle_thinking_level", id, e.to_string()),
            }
        }

        // ------------------------------------------------------------ Queue modes ----
        SessionCommand::SetSteeringMode { id, mode } => {
            session.set_steering_mode(mode.into());
            RpcResponse::ok("set_steering_mode", id.map(Value::String), None)
        }
        SessionCommand::SetFollowUpMode { id, mode } => {
            session.set_follow_up_mode(mode.into());
            RpcResponse::ok("set_follow_up_mode", id.map(Value::String), None)
        }

        // ------------------------------------------------------------- Compaction ----
        SessionCommand::Compact { id, custom_instructions } => {
            let id = id.map(Value::String);
            match session.compact(custom_instructions).await {
                Ok(result) => RpcResponse::ok(
                    "compact",
                    id,
                    Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                ),
                Err(e) => RpcResponse::err("compact", id, e.to_string()),
            }
        }
        SessionCommand::SetAutoCompaction { id, enabled } => {
            session.set_auto_compaction_enabled(enabled);
            RpcResponse::ok("set_auto_compaction", id.map(Value::String), None)
        }

        // ------------------------------------------------------------------ Retry ----
        SessionCommand::SetAutoRetry { id, enabled } => {
            session.set_auto_retry_enabled(enabled);
            RpcResponse::ok("set_auto_retry", id.map(Value::String), None)
        }
        SessionCommand::AbortRetry { id } => {
            session.abort_retry();
            RpcResponse::ok("abort_retry", id.map(Value::String), None)
        }

        // ------------------------------------------------------------------- Bash ----
        SessionCommand::Bash { id, command, exclude_from_context } => {
            let id = id.map(Value::String);
            let result = session
                .execute_bash(&command, BashOptions { exclude_from_context }, None)
                .await;
            RpcResponse::ok("bash", id, Some(serde_json::to_value(result).unwrap_or(Value::Null)))
        }
        SessionCommand::AbortBash { id } => {
            session.abort_bash();
            RpcResponse::ok("abort_bash", id.map(Value::String), None)
        }

        // ---------------------------------------------------------------- Session ----
        SessionCommand::GetSessionStats { id } => {
            let stats = serde_json::to_value(session.session_stats().await).unwrap_or(Value::Null);
            RpcResponse::ok("get_session_stats", id.map(Value::String), Some(stats))
        }
        SessionCommand::ExportHtml { id, output_path } => {
            let id = id.map(Value::String);
            let path = output_path.map(std::path::PathBuf::from);
            match session.export_to_html(path.as_deref()).await {
                Ok(out) => {
                    RpcResponse::ok("export_html", id, Some(json!({ "path": out.display().to_string() })))
                }
                Err(e) => RpcResponse::err("export_html", id, e.to_string()),
            }
        }
        SessionCommand::SwitchSession { id, session_path } => {
            let id = id.map(Value::String);
            match runtime.switch_session(session_path).await {
                Ok(result) => RpcResponse::ok(
                    "switch_session",
                    id,
                    Some(json!({ "cancelled": result.cancelled })),
                ),
                Err(e) => RpcResponse::err("switch_session", id, e.to_string()),
            }
        }
        SessionCommand::Fork { id, entry_id } => {
            let id = id.map(Value::String);
            match runtime.fork(EntryId::from(entry_id.as_str()), ForkPosition::Before).await {
                Ok(result) => RpcResponse::ok(
                    "fork",
                    id,
                    Some(json!({ "text": result.selected_text, "cancelled": result.cancelled })),
                ),
                Err(e) => RpcResponse::err("fork", id, e.to_string()),
            }
        }
        SessionCommand::Clone { id } => {
            let id = id.map(Value::String);
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
        SessionCommand::GetForkMessages { id } => {
            let messages: Vec<Value> = session
                .user_messages_for_forking()
                .await
                .into_iter()
                .map(|a| json!({ "entryId": a.entry_id.as_str(), "text": a.text }))
                .collect();
            RpcResponse::ok(
                "get_fork_messages",
                id.map(Value::String),
                Some(json!({ "messages": messages })),
            )
        }
        SessionCommand::GetEntries { id, since } => {
            let id = id.map(Value::String);
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
        SessionCommand::GetTree { id } => {
            let tree = session.tree_json().await;
            let leaf = session.leaf_id().await.map(|l| l.as_str().to_string());
            RpcResponse::ok(
                "get_tree",
                id.map(Value::String),
                Some(json!({ "tree": tree, "leafId": leaf })),
            )
        }
        SessionCommand::GetLastAssistantText { id } => {
            let text = session.last_assistant_text().await;
            RpcResponse::ok(
                "get_last_assistant_text",
                id.map(Value::String),
                Some(json!({ "text": text })),
            )
        }
        SessionCommand::SetSessionName { id, name } => {
            let id = id.map(Value::String);
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
        SessionCommand::GetMessages { id } => {
            let id = id.map(Value::String);
            match serde_json::to_value(session.messages().await) {
                Ok(v) => RpcResponse::ok("get_messages", id, Some(json!({ "messages": v }))),
                Err(e) => RpcResponse::err("get_messages", id, e.to_string()),
            }
        }

        // --------------------------------------------------------------- Commands ----
        SessionCommand::GetCommands { id } => RpcResponse::ok(
            "get_commands",
            id.map(Value::String),
            Some(json!({ "commands": session.slash_command_catalog() })),
        ),

        // A well-formed-but-unknown `type`: echo the recovered id so the client can correlate.
        SessionCommand::Unknown => RpcResponse::err("unknown", raw_id, "unknown command type"),
    }
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

/// Recover the top-level `id` from a raw request line, preserved as-is (string or number) for
/// correlation; returns `None` when the line is not a JSON object, has no `id`, or `id` is null.
fn extract_id(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("id").filter(|id| !id.is_null()).cloned())
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

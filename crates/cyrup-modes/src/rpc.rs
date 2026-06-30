//! RPC mode — the headless bidirectional stdio protocol (func-11 R-11-011…016; arch-11 §2.2/§3.5).
//!
//! A persistent line protocol other processes embed: incoming [`SessionCommand`] requests arrive as
//! strict-LF JSONL on a reader; the adapter drives the [`AgentSession`] and emits [`RpcOut`] lines
//! (a `response` per command + the full agent/session event stream) on a writer. Both endpoints are
//! parameters so tests drive an in-memory reader/writer pair and the binary wires real stdio.
//!
//! ## Framing (R-11-011)
//! Records are split on `\n` only (CRLF-tolerant: a trailing `\r` is stripped). We never rely on a
//! generic line reader that also splits on other Unicode separators inside JSON payloads.
//!
//! ## Streaming behaviour (R-11-016)
//! A `prompt` issued while the agent is already streaming MUST carry a `streaming_behavior`
//! (`steer` → queued after the current tool batch; `followUp` → after the agent goes idle); without
//! one it is rejected. While not streaming, `prompt` starts a fresh run.

use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, InputSource, StreamingBehavior, UserInput,
};
use futures::{FutureExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::error::ModesError;

// ---------------------------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------------------------

/// An incoming RPC request (`type`-tagged, snake_case to match Pi clients; R-11-014).
///
/// Every variant carries an optional `id` echoed back on its [`RpcResponse`] for correlation
/// (R-11-015). Unknown command types deserialize to [`SessionCommand::Unknown`] and yield a
/// `success:false` response — never a panic (R-00-009).
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionCommand {
    /// Submit a prompt. While streaming, `streaming_behavior` is required (R-11-016).
    Prompt {
        #[serde(default)]
        id: Option<String>,
        message: String,
        #[serde(default)]
        streaming_behavior: Option<StreamingBehavior>,
    },
    /// Enqueue a steering message (delivered after the current tool batch).
    Steer {
        #[serde(default)]
        id: Option<String>,
        message: String,
    },
    /// Enqueue a follow-up message (delivered after the agent goes idle).
    FollowUp {
        #[serde(default)]
        id: Option<String>,
        message: String,
    },
    /// Interrupt the active run (idempotent).
    Abort {
        #[serde(default)]
        id: Option<String>,
    },
    /// Compact the current branch.
    Compact {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        custom_instructions: Option<String>,
    },
    /// Fork the current session into a new file, returning the new session id (R-11-014).
    Fork {
        #[serde(default)]
        id: Option<String>,
    },
    /// Switch the active model by `provider/id[:level]` pattern.
    SetModel {
        #[serde(default)]
        id: Option<String>,
        model: String,
    },
    /// Query a snapshot of session state (id / model / streaming).
    GetState {
        #[serde(default)]
        id: Option<String>,
    },
    /// Query the persisted transcript on the current branch.
    GetMessages {
        #[serde(default)]
        id: Option<String>,
    },
    /// List the RPC command surface this server understands.
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

/// The advertised command surface returned by `get_commands` (R-11-014).
pub fn command_catalog() -> Value {
    json!([
        { "name": "prompt", "description": "submit a prompt (streaming_behavior required while streaming)" },
        { "name": "steer", "description": "enqueue a steering message" },
        { "name": "follow_up", "description": "enqueue a follow-up message" },
        { "name": "abort", "description": "interrupt the active run" },
        { "name": "compact", "description": "compact the current branch" },
        { "name": "fork", "description": "fork the current session into a new file" },
        { "name": "set_model", "description": "switch the active model by pattern" },
        { "name": "get_state", "description": "snapshot session id / model / streaming" },
        { "name": "get_messages", "description": "the persisted transcript on the current branch" },
        { "name": "get_commands", "description": "list the supported commands" },
    ])
}

// ---------------------------------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------------------------------

/// Serve the RPC protocol over `reader` (commands in) and `writer` (responses + events out).
///
/// Reads strict-LF JSONL requests, drives the `session`, and streams every [`AgentSessionEvent`]
/// (agent + session-level) back as it occurs. Returns once the reader reaches EOF *and* no run is
/// in flight. A dedicated reader task keeps line parsing cancel-safe against the concurrent event
/// stream; the writer is owned by the loop so its writes never interleave.
pub async fn run_rpc<R, W>(
    session: &AgentSession,
    reader: R,
    writer: &mut W,
) -> Result<(), ModesError>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    // One persistent subscription carries ALL events (agent + facade-level) for the whole session.
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
                        let resp = dispatch(session, &line, &mut in_flight).await;
                        write_out(writer, &RpcOut::Response(resp)).await?;
                    }
                    None => reader_open = false,
                }
            }
            maybe_ev = events.next() => {
                if let Some(ev) = maybe_ev {
                    if matches!(ev, AgentSessionEvent::AgentEnd { .. }) {
                        in_flight = false;
                    }
                    write_out(writer, &RpcOut::Event(Box::new(ev))).await?;
                }
            }
        }

        if !reader_open && !in_flight {
            // Flush any events already buffered on the channel, then shut down cleanly.
            while let Some(Some(ev)) = events.next().now_or_never() {
                write_out(writer, &RpcOut::Event(Box::new(ev))).await?;
            }
            break;
        }
    }

    // The reader task ends on its own at EOF; this just reaps it.
    reader_task.abort();
    Ok(())
}

/// Decode one request line and apply it, returning the correlated [`RpcResponse`]. Side effect: a
/// freshly-started run sets `in_flight`.
async fn dispatch(session: &AgentSession, line: &str, in_flight: &mut bool) -> RpcResponse {
    // Recover the request `id` from the raw JSON FIRST so even an unknown/unmappable command can be
    // correlated (R-11-015). Preserved exactly as sent (string or number); `null`/absent → no id.
    let raw_id = extract_id(line);

    let cmd: SessionCommand = match serde_json::from_str(line) {
        Ok(c) => c,
        // A JSON object carrying an id MUST echo it even when it fails to map to a known command.
        Err(e) => return RpcResponse::err("unknown", raw_id, format!("invalid command json: {e}")),
    };

    match cmd {
        SessionCommand::Prompt { id, message, streaming_behavior } => {
            let id = id.map(Value::String);
            let input = UserInput::text(message, InputSource::Rpc);
            if session.is_streaming().await {
                match streaming_behavior {
                    Some(StreamingBehavior::Steer) => match session.steer(input).await {
                        Ok(_) => RpcResponse::ok("prompt", id, Some(json!({ "queued": "steer" }))),
                        Err(e) => RpcResponse::err("prompt", id, e.to_string()),
                    },
                    Some(StreamingBehavior::FollowUp) => match session.follow_up(input).await {
                        Ok(_) => {
                            RpcResponse::ok("prompt", id, Some(json!({ "queued": "followUp" })))
                        }
                        Err(e) => RpcResponse::err("prompt", id, e.to_string()),
                    },
                    None => RpcResponse::err(
                        "prompt",
                        id,
                        "agent is streaming; specify streaming_behavior (steer|followUp)",
                    ),
                }
            } else {
                match session.prompt_accepted(input).await {
                    Ok(_) => {
                        *in_flight = true;
                        RpcResponse::ok("prompt", id, Some(json!({ "started": true })))
                    }
                    Err(e) => RpcResponse::err("prompt", id, e.to_string()),
                }
            }
        }
        SessionCommand::Steer { id, message } => {
            let id = id.map(Value::String);
            let input = UserInput::text(message, InputSource::Rpc);
            *in_flight = true;
            match session.steer(input).await {
                Ok(_) => RpcResponse::ok("steer", id, None),
                Err(e) => RpcResponse::err("steer", id, e.to_string()),
            }
        }
        SessionCommand::FollowUp { id, message } => {
            let id = id.map(Value::String);
            let input = UserInput::text(message, InputSource::Rpc);
            *in_flight = true;
            match session.follow_up(input).await {
                Ok(_) => RpcResponse::ok("follow_up", id, None),
                Err(e) => RpcResponse::err("follow_up", id, e.to_string()),
            }
        }
        SessionCommand::Abort { id } => {
            session.abort();
            RpcResponse::ok("abort", id.map(Value::String), None)
        }
        SessionCommand::Compact { id, custom_instructions } => {
            let id = id.map(Value::String);
            match session.compact(custom_instructions).await {
                // The facade now returns the full `CompactionResult`; the RPC payload reports both
                // the boolean and (when produced) the result detail (Pi `compaction_end.result`).
                Ok(result) => RpcResponse::ok(
                    "compact",
                    id,
                    Some(json!({
                        "compacted": result.is_some(),
                        "result": result,
                    })),
                ),
                Err(e) => RpcResponse::err("compact", id, e.to_string()),
            }
        }
        SessionCommand::Fork { id } => {
            let id = id.map(Value::String);
            match session.fork().await {
                Ok(new_id) => {
                    RpcResponse::ok("fork", id, Some(json!({ "sessionId": new_id.as_str() })))
                }
                Err(e) => RpcResponse::err("fork", id, e.to_string()),
            }
        }
        SessionCommand::SetModel { id, model } => {
            let id = id.map(Value::String);
            match session.set_model(&model).await {
                Ok(m) => RpcResponse::ok(
                    "set_model",
                    id,
                    Some(
                        json!({ "provider": m.provider.to_string(), "model": m.model.to_string() }),
                    ),
                ),
                Err(e) => RpcResponse::err("set_model", id, e.to_string()),
            }
        }
        SessionCommand::GetState { id } => {
            let m = session.model();
            let data = json!({
                "sessionId": session.session_id().as_str(),
                "isStreaming": session.is_streaming().await,
                "model": { "provider": m.provider.to_string(), "model": m.model.to_string() },
            });
            RpcResponse::ok("get_state", id.map(Value::String), Some(data))
        }
        SessionCommand::GetMessages { id } => {
            let id = id.map(Value::String);
            match serde_json::to_value(session.messages().await) {
                Ok(v) => RpcResponse::ok("get_messages", id, Some(v)),
                Err(e) => RpcResponse::err("get_messages", id, e.to_string()),
            }
        }
        SessionCommand::GetCommands { id } => {
            RpcResponse::ok("get_commands", id.map(Value::String), Some(command_catalog()))
        }
        // A well-formed-but-unknown `type`: echo the recovered id so the client can correlate.
        SessionCommand::Unknown => RpcResponse::err("unknown", raw_id, "unknown command type"),
    }
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

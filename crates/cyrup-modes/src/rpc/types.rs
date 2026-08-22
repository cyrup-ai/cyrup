//! RPC wire types — the protocol vocabulary the mode host and [`crate::RpcClient`] share (Pi
//! `modes/rpc/rpc-types.ts`, imported by both `rpc-mode.ts` and `rpc-client.ts`).
//!
//! Moved out of the mode implementation unchanged: the declarations, their field order and their
//! upstream citations are exactly as they were when they lived in `rpc.rs`.

use cyrup_session_svc::{
    AgentSessionEvent, Content, ModelThinkingLevel, QueueMode, StreamingBehavior,
};
use serde_json::Value;

/// The queue-drain mode argument (`all` | `one-at-a-time`; Pi `set_steering_mode`/`set_follow_up_mode`,
/// rpc-types.ts:41-42).
#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueModeArg {
    /// Wire string `"all"` — each drain takes every queued message.
    All,
    /// Wire string `"one-at-a-time"` — each drain takes the single oldest queued message.
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
pub(super) fn queue_mode_str(mode: QueueMode) -> &'static str {
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
/// [`dispatch`](super::dispatch) (`raw_id`), preserved as-sent (string **or** number — Pi types `id?: string` but an
/// opaque number passes through untouched, R-11-015; #10). Keeping `id` off the variant means a
/// numeric-`id` command still deserializes and **executes** rather than tripping payload
/// validation. Unknown command types deserialize to [`SessionCommand::Unknown`] via `#[serde(other)]`
/// (detected in [`dispatch`](super::dispatch), never reaching [`handle`](super::handle)); a required field that is missing/wrong-typed
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
    /// Set the thinking level (`off`|`minimal`|`low`|`medium`|`high`|`xhigh`|`max`).
    SetThinkingLevel { level: ModelThinkingLevel },
    /// Cycle to the next thinking level.
    CycleThinkingLevel,
    /// The thinking levels the ACTIVE model supports (`rpc-types.ts:39`, handler
    /// `rpc-mode.ts:507-510`, response `{levels}` at `rpc-types.ts:158-164`). SEAM-014.
    GetAvailableThinkingLevels,

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

    /// Any unrecognized `type` (R-00-009). Detected in [`dispatch`](super::dispatch); never reaches [`handle`](super::handle).
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
    /// Whether the command succeeded — `true` selects the `data` side, `false` the `error` side
    /// (Pi's `success`/`error` helpers, rpc-mode.ts:63-76). Always emitted.
    pub success: bool,
    /// The success payload — set only on the `success:true` path (a payload-less success leaves it
    /// `None`), never alongside `error`. Omitted from the object entirely when `None` rather than
    /// serialized as `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// The failure message — set exactly on the `success:false` path, never alongside `data`.
    /// Omitted from the object entirely when `None` rather than serialized as `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The read direction of the same wire object, for [`crate::RpcClient`] (SEAM-017). Pi shares ONE
/// `RpcResponse` type between its host and its client (`rpc-types.ts`, imported by both
/// `rpc-mode.ts` and `rpc-client.ts:15`), so cyrup shares one too rather than growing a second,
/// drift-prone client-side mirror.
///
/// Hand-written because `kind` is a `&'static str` — the tag is a constant of the type, not data —
/// so the derive cannot produce a `Deserialize`. The `type` key is read and discarded (a
/// `type` other than `"response"` never reaches here: the client tests it before deserializing,
/// exactly as Pi's `handleLine` does at `rpc-client.ts:512`), and every field is defaulted so a
/// truncated or partial response object degrades to `success:false` rather than failing the parse
/// and silently dropping a correlated reply.
impl<'de> serde::Deserialize<'de> for RpcResponse {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Wire {
            #[serde(default)]
            id: Option<Value>,
            #[serde(default)]
            command: String,
            #[serde(default)]
            success: bool,
            #[serde(default)]
            data: Option<Value>,
            #[serde(default)]
            error: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            id: wire.id,
            kind: "response",
            command: wire.command,
            success: wire.success,
            data: wire.data,
            error: wire.error,
        })
    }
}

impl RpcResponse {
    pub(super) fn ok(command: impl Into<String>, id: Option<Value>, data: Option<Value>) -> Self {
        Self { id, kind: "response", command: command.into(), success: true, data, error: None }
    }

    pub(super) fn err(command: impl Into<String>, id: Option<Value>, error: impl Into<String>) -> Self {
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
///
/// [`RpcOut::Event`] serializes through [`crate::to_json_event`], NOT through
/// [`AgentSessionEvent`]'s own `Serialize` — Pi's `output(toJsonEvent(event))`
/// (`rpc-mode.ts:356` **@v0.84.1**; at the ported v0.83.0 baseline that line is
/// `if (event.type === "agent_settled")` and `:355` is a bare `output(event)` — `toJsonEvent` does
/// not exist at the tag. See [`crate::json_event`] for the version-lag rationale, SEAM-085).
/// The `Serialize` impl is hand-written for exactly that reason (see below); the variant keeps its
/// `Box<AgentSessionEvent>` payload so the projection stays a wire concern and no public signature
/// changes.
#[derive(Debug)]
pub enum RpcOut {
    /// A correlated reply to a request; serialized untagged, so its discriminant on the wire is the
    /// [`RpcResponse::kind`] key — `"type":"response"`.
    Response(RpcResponse),
    /// A session event pushed without a request; serialized untagged through
    /// [`crate::to_json_event`], so its discriminant is the event's own `type` tag (`agent_start`,
    /// `tool_execution_end`, …) rather than `"response"`.
    Event(Box<AgentSessionEvent>),
    /// A synchronous extension dialog request (`ui.{confirm,input,select,editor}`) emitted on stdout
    /// for the RPC client to render + answer via an `extension_ui_response` (Pi
    /// `createExtensionUIContext` → `output({type:"extension_ui_request", …})`, rpc-mode.ts:128-160,
    /// 253-268). Carries the pre-shaped Pi wire object so field names/order match byte-for-byte.
    ExtensionUiRequest(Value),
    /// A contained extension fault surfaced to the client (Pi `bindExtensions({onError})`,
    /// rpc-mode.ts:347-349: `output({type:"extension_error", extensionPath, event, error})`). Carries
    /// the pre-shaped `{type:"extension_error", extensionPath, event, error}` wire object; emitted on
    /// stdout each time the dispatcher contains + skips a guest handler fault (R-08-036). Untagged, so
    /// the embedded `"type":"extension_error"` is the discriminant a client dispatches on.
    ExtensionError(Value),
}

/// Untagged serialization — each variant serializes as its inner value and nothing else, exactly as
/// `#[serde(untagged)]` did — with ONE difference: [`RpcOut::Event`] routes through
/// [`crate::to_json_event`], so the rpc stdout stream carries the delta-only `message_update` Pi
/// emits (`rpc-mode.ts:356` **@v0.84.1** — see above) rather than the cumulative snapshots.
///
/// Hand-written rather than derived because the projection has to happen at the point of writing:
/// `RpcOut::Event` is constructed from an [`AgentSessionEvent`] the driver has in hand (`:807`,
/// `:830`), which is the analog of Pi's `output(event)` call site, and the projection borrows rather
/// than owning. Keeping the payload type and moving the projection into the serializer means the two
/// wire writers ([`crate::run_json`] and [`write_out`](super::jsonl::write_out)) share ONE projection with no copy to drift.
/// The match is exhaustive so a new variant cannot silently skip it.
impl serde::Serialize for RpcOut {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            RpcOut::Response(response) => response.serialize(serializer),
            RpcOut::Event(event) => crate::to_json_event(event).serialize(serializer),
            RpcOut::ExtensionUiRequest(value) | RpcOut::ExtensionError(value) => {
                value.serialize(serializer)
            }
        }
    }
}

//! The backend's two FIRE-AND-FORGET sinks and the payloads they carry: [`ControlSink`] (a guest's
//! `control` op, handed to the runtime that owns the session) and [`InjectSink`] (an
//! [`InjectMessage`] a background task pushed at the live session's turn loop), plus
//! [`ActiveToolsPush`], the rebuilt tool array + prompt a guest `setActiveTools` queues for the
//! same async drain.
//!
//! The ui seam's sinks live in [`super::ui`] instead; what these three share is the sync→async
//! bridge described below — the guest's call returns the moment the payload is queued, so nothing
//! blocks the wasm-suspended caller for the whole turn.

use std::sync::Arc;

use cyrup_core::Tool;
use cyrup_ext::host::ControlOp;

// Doc-only: the docs below name the backend that forwards onto these sinks and the async drain the
// queued push is applied by; neither is named in code here.
#[cfg(doc)]
use super::LiveHostServices;

/// A command-tier control sink: a loaded extension's `control` import (new/switch/fork/…) is routed
/// here so the runtime can act on it (Pi `createCommandContext`, agent-session.ts:1158). Set by the
/// runtime once it owns the session; until then control ops are reported as unavailable.
pub type ControlSink = Arc<dyn Fn(ControlOp) -> Result<(), String> + Send + Sync>;

/// A rebuilt active-tool push: the new tool array + the rebuilt system prompt a guest `setActiveTools`
/// produced (Pi `setActiveToolsByName` output, agent-session.ts:850-854), queued for the async agent
/// push in [`crate::AgentSession::apply_pending_control`].
pub(super) type ActiveToolsPush = (Vec<Arc<dyn Tool>>, String);

/// A host-originated message injection routed from a background task's `inject_message` to the live
/// session's turn loop (Pi `pi.sendMessage(message, {triggerTurn})` → `sendCustomMessage`,
/// agent-session.ts:1337-1370). The REQUEST payload of the late-bound [`InjectSink`]; carries the
/// fields the trait's [`HostServices::inject_message`] takes so the sink can drive the async
/// append/turn on the live session. Closes R-SA-101 (cyrup-ext-subagents background completion).
///
/// [`HostServices::inject_message`]: cyrup_ext::host::HostServices::inject_message
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InjectMessage {
    /// The message body (Pi `content`).
    pub content: String,
    /// A custom (non-LLM) message tag when `Some` (Pi `customType`, e.g. `"subagent-notify"`); a
    /// plain user message when `None`.
    pub custom_type: Option<String>,
    /// Whether the message is surfaced to the user (Pi `display`).
    pub display: bool,
    /// Whether to re-enter the agent turn loop over the injected message (Pi `{ triggerTurn: true }`).
    pub trigger_turn: bool,
}

/// A fire-and-forget message-injection sink: [`LiveHostServices::inject_message`] forwards an
/// [`InjectMessage`] here; the installed sink (bound by `AgentSession::into_shared`) spawns the async
/// inject/turn on the live session and returns immediately, so the sync caller never blocks for the
/// whole turn (the same sync→async bridge the [`ControlSink`] uses). `None` until bound (the default
/// host, a headless-by-value session): `inject_message` then reports the seam unavailable, matching
/// the trait's deny default.
pub type InjectSink = Arc<dyn Fn(InjectMessage) -> Result<(), String> + Send + Sync>;

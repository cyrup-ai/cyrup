//! The `host-tool` WIT import: the per-call cancellation [`Signal`] a guest tool polls, and the
//! [`ToolCall`] its `execute` receives.

use serde::Serialize;
use serde_json::Value;

use super::Ctx;

/// The tool `execute` cancellation signal (Pi `ToolDefinition.execute` `signal: AbortSignal`,
/// types.ts:483; sdk gap #1). A long-running tool polls [`Self::is_aborted`] to cooperatively stop;
/// it reads the host's live cancellation state for this `call_id` (the run `CancelToken`, the epoch
/// deadline, or a named `ui.abort-signal` matching the call id). The host epoch is the hard backstop.
#[derive(Clone, Debug, Default)]
pub struct Signal {
    // Read only by the wasm32 `is_aborted` import call; inert on the host target.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    call_id: String,
}

impl Signal {
    pub fn new(call_id: impl Into<String>) -> Self {
        Self { call_id: call_id.into() }
    }
    /// Whether cancellation has been requested for this tool call (Pi `signal.aborted`).
    pub fn is_aborted(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::host_tool::is_cancelled(&self.call_id);
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }
}

/// The call passed to a guest tool's `execute` (Pi `ToolDefinition.execute` args, types.ts:480).
/// Carries the `toolCallId`, parsed `params`, the cancellation [`Signal`], and a [`Ctx`];
/// `emit_update` streams partial output back to the runtime (Pi `onUpdate`).
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub call_id: String,
    pub params: Value,
    pub ctx: Ctx,
    /// The cancellation signal (Pi `signal`): poll [`Signal::is_aborted`] inside a long `execute`.
    pub signal: Signal,
}

impl ToolCall {
    pub fn new(call_id: impl Into<String>, params: Value) -> Self {
        let call_id = call_id.into();
        Self { signal: Signal::new(call_id.clone()), call_id, params, ctx: Ctx }
    }
    /// The cancellation signal for this call (Pi `signal` param).
    pub fn signal(&self) -> &Signal {
        &self.signal
    }
    /// Stream a partial-output chunk (Pi `onUpdate`).
    pub fn emit_update(&self, chunk: impl Serialize) {
        let chunk_json = serde_json::to_string(&chunk).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::host_tool::emit_update(&self.call_id, &chunk_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = chunk_json;
    }
}

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
    /// Bind a signal to a tool `call_id` — the id [`Self::is_aborted`] asks the host about.
    /// [`ToolCall::new`] already does this for the call it builds, so a tool `execute` body reads
    /// [`ToolCall::signal`] rather than constructing one.
    pub fn new(call_id: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
        }
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
    /// The host's id for this call (Pi `toolCallId`) — the key [`Self::emit_update`] streams
    /// against and the one [`Self::signal`] polls.
    pub call_id: String,
    /// The call's arguments, already parsed out of the host's JSON (Pi `params`).
    pub params: Value,
    /// The capability context for this call — the same [`Ctx`] an event handler receives.
    pub ctx: Ctx,
    /// The cancellation signal (Pi `signal`): poll [`Signal::is_aborted`] inside a long `execute`.
    pub signal: Signal,
}

impl ToolCall {
    /// Build a call from the host's id and its parsed `params`, binding a [`Signal`] to that same
    /// id.
    pub fn new(call_id: impl Into<String>, params: Value) -> Self {
        let call_id = call_id.into();
        Self {
            signal: Signal::new(call_id.clone()),
            call_id,
            params,
            ctx: Ctx,
        }
    }
    /// The cancellation signal for this call (Pi `signal` param).
    pub fn signal(&self) -> &Signal {
        &self.signal
    }
    /// Stream a partial-output chunk (Pi `onUpdate`).
    ///
    /// **On an encode failure NO chunk is streamed.** `chunk` is author-supplied and its
    /// `serde_json` encoding is fallible; rather than streaming a `null` chunk into the runtime's
    /// partial-output channel, the update is skipped and the error is surfaced as an error-severity
    /// [`Ui::notify_with`] notification. The signature stays `()` — Pi's `onUpdate` has no return
    /// value to fold an `Err` into.
    ///
    /// [`Ui::notify_with`]: crate::Ui::notify_with
    pub fn emit_update(&self, chunk: impl Serialize) {
        let chunk_json = match serde_json::to_string(&chunk) {
            Ok(c) => c,
            Err(e) => {
                self.ctx.ui().notify_with(
                    &format!(
                        "emit_update({}): chunk dropped, failed to encode: {e}",
                        self.call_id
                    ),
                    crate::ctx::NotifyKind::Error,
                );
                return;
            }
        };
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::host_tool::emit_update(&self.call_id, &chunk_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = chunk_json;
    }
}

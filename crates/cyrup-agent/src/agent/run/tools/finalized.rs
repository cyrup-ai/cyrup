//! The one finalized tool-call record. It lives in its own leaf module so that [`Finalized::new`]
//! is the ONLY way to build one: Rust field privacy is module-tree scoped, and a struct declared
//! in `tools/mod.rs` with private fields is still literal-constructible from `exec.rs` and
//! `preflight.rs` (its children) — which is exactly how a `source_index: 0` placeholder got
//! written by one producer and patched by two of its three consumers.

use crate::agent::message::result_value_of;
use crate::event::{AgentEvent, ToolResultMessage};
use cyrup_core::TerminateHint;
use serde_json::Value;

/// A tool call's settled result: the transcript message the batch will return, the index of the
/// call it answers, and the `tool_execution_end.result` payload derived from both.
pub(super) struct Finalized {
    source_index: usize,
    /// `AgentToolResult.terminate?` (`packages/agent/src/types.ts:354-368`) —
    /// [`TerminateHint::Unspecified`] is pi's `undefined`, i.e. the key is absent from the emitted
    /// `result` and the call does not contribute a vote to `shouldTerminateToolBatch`
    /// (`agent-loop.ts:582-584`). AGENT-009. Runtime-only: not a field of the persisted message.
    terminate: TerminateHint,
    result_value: Value,
    message: ToolResultMessage,
}

impl Finalized {
    /// The only constructor. `source_index` is the position of the answered call in the assistant
    /// message's tool-call list; `result_value` is derived here so it can never disagree with
    /// `message` (Pi emits `result: finalized.result` verbatim, `emitToolExecutionEnd`,
    /// `agent-loop.ts:763-771`).
    pub(super) fn new(
        source_index: usize,
        message: ToolResultMessage,
        terminate: TerminateHint,
    ) -> Self {
        let result_value = result_value_of(
            &message.content,
            &message.details,
            message.usage.as_ref(),
            &message.added_tool_names,
            terminate,
        );
        Self {
            source_index,
            terminate,
            result_value,
            message,
        }
    }

    pub(super) fn source_index(&self) -> usize {
        self.source_index
    }

    pub(super) fn terminate(&self) -> TerminateHint {
        self.terminate
    }

    /// The `tool_execution_end` event for this result — the one place the literal is written.
    pub(super) fn end_event(&self) -> AgentEvent {
        AgentEvent::ToolExecutionEnd {
            tool_call_id: self.message.tool_call_id.clone(),
            tool_name: self.message.tool_name.clone(),
            result: self.result_value.clone(),
            is_error: self.message.is_error,
        }
    }

    pub(super) fn into_message(self) -> ToolResultMessage {
        self.message
    }
}

//! The one finalized tool-call record. It lives in its own leaf module so that [`Finalized::new`]
//! is the ONLY way to build one: Rust field privacy is module-tree scoped, and a struct declared
//! in `tools/mod.rs` with private fields is still literal-constructible from `exec.rs` and
//! `preflight.rs` (its children) — which is exactly how a `source_index: 0` placeholder got
//! written by one producer and patched by two of its three consumers.

use crate::agent::message::result_value_of;
use crate::event::{AgentEvent, ToolResultMessage};
use cyrup_core::{TerminateHint, ToolCall, ToolResult};
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
    /// `AgentToolResult.structuredContent?` (`agent/src/types.ts:433` @v1.0.1, AGENT-045). Kept
    /// BESIDE `message` rather than in it, because pi's persisted `ToolResultMessage` has no such
    /// field — it reaches only `result_value` (the `tool_execution_end.result` payload) and
    /// [`Self::into_outcome`]'s programmatic caller. AGENT-047 is the reader the constructor's
    /// doc anticipated.
    structured_content: Option<Value>,
    /// The TOOL's own `is_error` (`AgentToolResult.isError?`, `types.ts:440` @v1.0.1, AGENT-046),
    /// kept beside `message` for the same reason as `structured_content`: `message.is_error` is
    /// the NORMALISED verdict (which a hook can flip and a throw forces), and pi keeps the two
    /// apart — its `{...result}` spread never assigns `isError` (`agent-loop.ts:881-890`).
    tool_is_error: bool,
    result_value: Value,
    message: ToolResultMessage,
}

/// One tool call's settled outcome, for a caller that invoked it programmatically rather than
/// through a model-issued batch — pi `AgentToolCallOutcome` (`packages/agent/src/types.ts:449-453`
/// @v1.0.1), promoted there from the private `FinalizedToolCallOutcome` alias for exactly this
/// purpose.
///
/// `is_error` is a field of the OUTCOME, not of [`ToolResult`], which is why AGENT-047 needs no
/// widening of the public result struct: upstream's `AgentToolCallOutcome.isError` is likewise
/// separate from `AgentToolResult.isError` (`types.ts:440`, AGENT-046's subject) and is the
/// normalised verdict after `after_tool_call` ran.
pub struct ToolCallOutcome {
    /// The call as it was issued, echoed back (pi `toolCall`, `types.ts:450`).
    pub tool_call: ToolCall,
    /// The settled result, `structured_content` included (pi `result`, `types.ts:451`).
    pub result: ToolResult,
    /// `true` for every failure class: unknown tool, validation failure, blocked call, thrown
    /// tool, failing `after_tool_call` (pi `isError`, `types.ts:452`).
    pub is_error: bool,
    /// Milliseconds `execute()` took, measured with a monotonic clock; `None` when the tool did not
    /// run (pi `durationMs?`, `packages/agent/src/types.ts` @v1.1.0).
    pub duration_ms: Option<u64>,
}

impl ToolCallOutcome {
    /// The outcome as pi's `AgentToolCallOutcome` object, `{ toolCall, result, isError }`
    /// (`packages/agent/src/types.ts:449-453` @v1.0.1) — the value `ctx.executeTool()` resolves to
    /// for an extension that is not Rust. `result` is the same payload a `tool_execution_end` event
    /// carries (absent keys omitted, `structuredContent` and the tool's own `isError` included),
    /// and the top-level `isError` is the normalised verdict ([`Self::is_error`]).
    pub fn to_wire(&self) -> Value {
        let result = result_value_of(
            &self.result.content,
            &self.result.details,
            self.result.usage.as_ref(),
            &self.result.added_tool_names,
            self.result.terminate,
            self.result.structured_content.as_ref(),
            self.result.is_error,
        );
        let mut wire = serde_json::json!({
            "toolCall": serde_json::to_value(&self.tool_call).unwrap_or(Value::Null),
            "result": result,
            "isError": self.is_error,
        });
        // pi's `finalizeExecutedToolCall` adds the key last, and only for a call that ran.
        if let (Some(ms), Some(object)) = (self.duration_ms, wire.as_object_mut()) {
            object.insert("durationMs".to_string(), Value::from(ms));
        }
        wire
    }
}

impl Finalized {
    /// The only constructor. `source_index` is the position of the answered call in the assistant
    /// message's tool-call list; `result_value` is derived here so it can never disagree with
    /// `message` (Pi emits `result: finalized.result` verbatim, `emitToolExecutionEnd`,
    /// `agent-loop.ts:763-771`).
    ///
    /// `structured_content` (`AgentToolResult.structuredContent?`, `agent/src/types.ts:429-433`
    /// @v1.0.1, AGENT-045) goes into the emitted `result` payload but NOT into `message`, because
    /// pi's persisted `ToolResultMessage` has no such field. AGENT-047 added the field beside
    /// `message` and [`Self::into_outcome`] as its reader, which is the programmatic caller this
    /// doc anticipated.
    pub(super) fn new(
        source_index: usize,
        message: ToolResultMessage,
        terminate: TerminateHint,
        structured_content: Option<Value>,
        tool_is_error: bool,
    ) -> Self {
        let result_value = result_value_of(
            &message.content,
            &message.details,
            message.usage.as_ref(),
            &message.added_tool_names,
            terminate,
            structured_content.as_ref(),
            tool_is_error,
        );
        Self {
            source_index,
            terminate,
            structured_content,
            tool_is_error,
            result_value,
            message,
        }
    }

    /// Reassemble the `ToolResult` this record settled on and pair it with the call that produced
    /// it — pi `runToolCall`'s return (`agent-loop.ts:817` @v1.0.1, which returns
    /// `finalizeExecutedToolCall`'s `{ toolCall, result, isError }` verbatim).
    ///
    /// The two runtime-only halves live here rather than on `message`, so this is the one place
    /// that can put them back: `terminate` and `structured_content` come off `self`, everything
    /// else off the transcript message the batch would have appended.
    pub(super) fn into_outcome(self, tool_call: ToolCall) -> ToolCallOutcome {
        let Self {
            terminate,
            structured_content,
            tool_is_error,
            message,
            ..
        } = self;
        ToolCallOutcome {
            // The NORMALISED verdict — pi's `isError` local, not `result.isError` (`:898-902`).
            is_error: message.is_error,
            duration_ms: message.duration_ms,
            result: ToolResult {
                content: message.content,
                details: message.details,
                usage: message.usage,
                added_tool_names: message.added_tool_names,
                structured_content,
                // The TOOL's own flag, carried through the fold untouched. AGENT-046.
                is_error: tool_is_error,
                terminate,
            },
            tool_call,
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
            duration_ms: self.message.duration_ms,
        }
    }

    pub(super) fn into_message(self) -> ToolResultMessage {
        self.message
    }
}

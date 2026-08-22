//! Tool-batch dispatch: pick the parallel or sequential runtime for a turn's calls, and the
//! all-calls-failed path for an assistant message truncated by the output token limit.

mod exec;
mod finalize;
mod preflight;

use super::{RunCtx, RunFailure};
use crate::event::{AgentEvent, AgentMessage, ToolResultMessage};
use crate::queue::ToolExecution;
use cyrup_core::{
    AssistantMessage, ExecMode, Tool, ToolCall, ToolCallId, ToolError, ToolResult, ToolUpdate,
};
use serde_json::Value;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Internal run-loop types
// ---------------------------------------------------------------------------

enum ToolRuntimeMsg {
    Update { call_id: ToolCallId, partial: ToolUpdate },
    Finished { call_id: ToolCallId, source_index: usize, tool_name: String, outcome: Result<ToolResult, ToolError> },
}

struct Finalized {
    source_index: usize,
    tool_call_id: ToolCallId,
    tool_name: String,
    result_value: Value,
    is_error: bool,
    /// `AgentToolResult.terminate?` (`packages/agent/src/types.ts:354-368`) — `None` is pi's
    /// `undefined`, i.e. the key is absent from the emitted `result` and the call does not
    /// contribute a `true` to `shouldTerminateToolBatch` (`agent-loop.ts:582-584`). AGENT-009.
    terminate: Option<bool>,
    message: ToolResultMessage,
}

enum Prep {
    /// Boxed: `Finalized` embeds a whole `ToolResultMessage` and dwarfs the `Ready` arm, so an
    /// unboxed variant makes every `Prep` (including the common prepared-call case) pay for it.
    Immediate(Box<Finalized>),
    Ready { tool: Arc<dyn Tool>, args: Value },
}

pub(super) struct Batch {
    pub(super) messages: Vec<ToolResultMessage>,
    pub(super) terminate: bool,
}

impl RunCtx {
    pub(super) async fn execute_tool_calls(
        &self,
        assistant: &AssistantMessage,
        calls: &[ToolCall],
    ) -> Result<Batch, RunFailure> {
        let any_seq = calls.iter().any(|c| {
            self.find_tool(&c.name).map(|t| t.execution_mode() == ExecMode::Sequential).unwrap_or(false)
        });
        let sequential = any_seq || matches!(self.tool_execution, ToolExecution::Sequential);
        // Snapshot the loop's working transcript once for the per-call hook context view (Pi
        // `currentContext.messages`, agent-loop.ts:691).
        let ctx_messages = self.messages.clone();
        if sequential {
            self.execute_sequential(assistant, &ctx_messages, calls).await
        } else {
            self.execute_parallel(assistant, &ctx_messages, calls).await
        }
    }

    /// Fail every tool call from an assistant message that was truncated by the output token limit
    /// (Pi `failToolCallsFromTruncatedMessage`, agent-loop.ts:374-405).
    ///
    /// Streamed tool-call arguments are finalized with a best-effort JSON salvage parser
    /// (`cyrup-provider` `parse_streaming_json_object`), so a truncated message can yield tool calls
    /// whose arguments parse and validate but are silently incomplete. None of them are safe to
    /// execute; report each as an error so the model can re-issue them. No tool is located, no
    /// `before_tool_call`/`after_tool_call` hook runs, and the batch never terminates the loop —
    /// Pi returns `{ messages, terminate: false }` so the model gets its turn to re-issue.
    ///
    /// Per call, in source order, the emitted sequence mirrors Pi exactly:
    /// `tool_execution_start` → `tool_execution_end` (`isError`) → `message_start` / `message_end`.
    pub(super) async fn fail_truncated_tool_calls(&self, calls: &[ToolCall]) -> Result<Batch, RunFailure> {
        let mut tool_results = Vec::new();
        for call in calls {
            self.emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: Value::Object(call.arguments.clone()),
            })
            .await?;
            let fin = self.immediate_error(
                call,
                format!(
                    "Tool call \"{}\" was not executed: the response hit the output token limit, \
                     so its arguments may be truncated. Re-issue the tool call with complete \
                     arguments.",
                    call.name
                ),
                false,
            );
            self.emit(AgentEvent::ToolExecutionEnd {
                tool_call_id: fin.tool_call_id.clone(),
                tool_name: fin.tool_name.clone(),
                result: fin.result_value.clone(),
                is_error: fin.is_error,
            })
            .await?;
            let msg = AgentMessage::ToolResult(fin.message.clone());
            self.emit(AgentEvent::MessageStart { message: msg.clone() }).await?;
            self.emit(AgentEvent::MessageEnd { message: msg }).await?;
            tool_results.push(fin.message);
        }
        // Pi `{ messages, terminate: false }` (agent-loop.ts:404).
        Ok(Batch { messages: tool_results, terminate: false })
    }
}

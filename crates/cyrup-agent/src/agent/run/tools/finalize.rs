//! Tool result finalization: apply `after_tool_call` (replace-not-merge per field) and build the
//! finalized result + transcript message the batch returns.

use super::Finalized;
use crate::agent::message::{empty_details, result_value_of};
use crate::agent::run::RunCtx;
use crate::agent::util::now_millis;
use crate::event::{AgentMessage, ToolResultMessage};
use crate::hooks::{AfterToolCall, AgentContextView};
use cyrup_core::{AssistantMessage, Content, ToolCall, ToolError, ToolResult};
use serde_json::Value;

impl RunCtx {
    /// Apply `after_tool_call` (replace-not-merge per field, R-02-025) and build the finalized
    /// result. On hook `Err`: error result, `terminate` ignored (R-02-025/050).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn finalize(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[AgentMessage],
        call: &ToolCall,
        source_index: usize,
        args: Value,
        outcome: Result<ToolResult, ToolError>,
    ) -> Finalized {
        let call_id = call.id.clone();
        let tool_name = call.name.clone();
        // `added_tool_names` rides through untouched: Pi's `finalizeExecutedToolCall` spreads
        // `{...result}` before applying the hook's explicit fields (agent-loop.ts:736-742) and
        // `addedToolNames` is not one of them, so no hook can set or clear it.
        let (
            mut content,
            mut details,
            mut usage,
            mut added_tool_names,
            mut terminate,
            mut is_error,
        ) = match outcome {
            // AGENT-009 — `terminate` is optional upstream (`AgentToolResult.terminate?`,
            // types.ts:354-368). `cyrup_core::ToolResult.terminate` is a plain `bool` whose default
            // IS "absent", so `false` maps to `None` (no key) and `true` to `Some(true)`.
            // [CYRUP-DELTA] a tool that wants pi's explicit `terminate: false` cannot express it
            // until `cyrup_core::ToolResult.terminate` becomes `Option<bool>` like
            // `ToolUpdate.terminate` already is (cyrup-core/src/tool.rs:41).
            Ok(r) => (
                r.content,
                r.details,
                r.usage,
                r.added_tool_names,
                if r.terminate { Some(true) } else { None },
                false,
            ),
            // A throwing TOOL yields `createErrorToolResult(...)` (`agent-loop.ts:700-703`
            // @v0.83.0), i.e. `details: {}` and no `terminate`.
            Err(e) => (
                vec![Content::text(e.to_string())],
                Some(empty_details()),
                None,
                Vec::new(),
                None,
                true,
            ),
        };

        let hook_result = {
            let ctx = AfterToolCall {
                tool_name: &tool_name,
                tool_call_id: &call_id,
                args: &args,
                content: &content,
                details: details.as_ref(),
                usage: usage.as_ref(),
                is_error,
                terminate,
                assistant_message: assistant,
                tool_call: call,
                context: AgentContextView {
                    system_prompt: &self.system_prompt,
                    messages: ctx_messages,
                    tools: &self.tools,
                },
            };
            self.hooks.after_tool_call(ctx, self.cancel.child()).await
        };
        match hook_result {
            Ok(Some(ov)) => {
                if let Some(c) = ov.content {
                    content = c;
                }
                if let Some(d) = ov.details {
                    details = Some(d);
                }
                // Replace-not-merge, the same rule as `content`/`details` (Pi
                // `usage: afterResult.usage ?? result.usage`, agent-loop.ts:738; types.ts:70-78:
                // "There is no deep merge for `content`, `details`, or `usage`").
                if let Some(u) = ov.usage {
                    usage = Some(u);
                }
                if let Some(e) = ov.is_error {
                    is_error = e;
                }
                // Pi `terminate: afterResult.terminate ?? result.terminate` (agent-loop.ts:739
                // @v0.83.0): an absent hook value keeps whatever the tool set.
                if let Some(t) = ov.terminate {
                    terminate = Some(t);
                }
            }
            Ok(None) => {}
            Err(e) => {
                // Pi discards the whole result for `createErrorToolResult(…)` when the hook throws
                // (agent-loop.ts:743-745), and that carries neither usage nor added tool names. The
                // replacement content is the thrown value's own text
                // (`error instanceof Error ? error.message : String(error)`, agent-loop.ts:744), so
                // the failing hook's reason — not a fixed label — is what the model reads back.
                content = vec![Content::text(e.to_string())];
                // AGENT-009 — `createErrorToolResult` sets `details: {}` (agent-loop.ts:756-761).
                details = Some(empty_details());
                usage = None;
                added_tool_names = Vec::new();
                is_error = true;
                terminate = None;
            }
        }

        let message = ToolResultMessage {
            tool_call_id: call_id.clone(),
            tool_name: tool_name.clone(),
            content,
            details,
            usage,
            added_tool_names,
            is_error,
            // Pi `createToolResultMessage` stamps every tool result with `Date.now()`
            // (agent-loop.ts:741); this reaches the wire payload via `convert_to_llm`.
            timestamp: now_millis(),
        };
        Finalized {
            source_index,
            tool_call_id: call_id,
            tool_name,
            result_value: result_value_of(
                &message.content,
                &message.details,
                message.usage.as_ref(),
                &message.added_tool_names,
                terminate,
            ),
            is_error,
            terminate,
            message,
        }
    }
}

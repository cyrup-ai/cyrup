//! Tool preflight: locate the tool, normalize its arguments, validate/coerce them against the
//! schema, run `before_tool_call` — yielding either a prepared executor or an immediate error
//! result the model can retry from.

use super::{Finalized, Prep};
use crate::agent::message::{empty_details, result_value_of};
use crate::agent::run::RunCtx;
use crate::agent::util::now_millis;
use crate::event::{AgentMessage, ToolResultMessage};
use crate::hooks::{AgentContextView, BeforeOutcome, BeforeToolCall};
use cyrup_core::{AssistantMessage, Content, SharedStr, ToolCall};
use cyrup_provider::validate_tool_call;
use serde_json::Value;

impl RunCtx {
    /// Preflight: locate tool → normalize args (`prepare_arguments`) → validate/coerce → `before_tool_call`.
    /// Returns an immediate (finalized) error result or a prepared executor (func-02 R-02-019/020/021/022).
    pub(super) async fn prepare(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[AgentMessage],
        call: &ToolCall,
    ) -> Prep {
        let tool = match self.find_tool(&call.name) {
            Some(t) => t,
            None => {
                // AGENT-010 — byte-for-byte pi: `` createErrorToolResult(`Tool ${toolCall.name} not
                // found`) `` (`packages/agent/src/agent-loop.ts:611` @v0.83.0, identical offset at
                // v0.84.1). NO quotes around the name; this string reaches the model.
                return Prep::Immediate(Box::new(
                    self.immediate_error(call, format!("Tool {} not found", call.name), false),
                ))
            }
        };
        // Normalize the raw model-emitted arguments via the tool's `prepare_arguments` compat shim
        // BEFORE schema validation (Pi `prepareToolCallArguments` → `validateToolArguments`,
        // agent-loop.ts:548-560,578-579). Default impl is identity.
        let prepared = tool.prepare_arguments(Value::Object((*call.arguments).clone())).await;
        // Validate AND coerce against the tool's JSON-Schema `parameters` (R-02-020 / func-01
        // R-01-034). On failure surface an immediate isError tool-result so the model can retry on
        // the next turn; the tool is NOT executed.
        let mut args = match validate_tool_call(tool.parameters(), prepared) {
            Ok(coerced) => coerced,
            Err(e) => {
                return Prep::Immediate(Box::new(self.immediate_error(call, e.to_string(), false)))
            }
        };
        // AGENT-012 — pi's `prepareToolCall` has NO pre-hook abort check
        // (`packages/agent/src/agent-loop.ts:616-656` @v0.83.0): the only two checks are `if
        // (signal?.aborted)` at `:629`, immediately AFTER `beforeToolCall` returns and BEFORE the
        // block branch at `:636`, and a second at `:644`. So pi always invokes `beforeToolCall` —
        // audit logs, permission bookkeeping and ref-counted resources in an extension see every
        // call even on an aborted run. The check that used to sit here skipped the hook entirely.
        let before = {
            let ctx = BeforeToolCall {
                tool_name: &call.name,
                tool_call_id: &call.id,
                args: &mut args,
                messages: &self.new_messages,
                assistant_message: assistant,
                tool_call: call,
                context: AgentContextView {
                    system_prompt: &self.system_prompt,
                    messages: ctx_messages,
                    tools: &self.tools,
                },
            };
            self.hooks.before_tool_call(ctx, self.cancel.child()).await
        };
        match before {
            // Pi's `prepareToolCall` wraps the `beforeToolCall` await in the same try that guards
            // argument preparation/validation, and its catch returns
            // `createErrorToolResult(error instanceof Error ? error.message : String(error))`
            // (agent-loop.ts:657-662) — the hook's OWN text reaches the model, exactly as the
            // validation failure two arms up already does.
            Err(e) => Prep::Immediate(Box::new(self.immediate_error(call, e.to_string(), false))),
            Ok(outcome) => {
                // AGENT-012 — pi checks the signal the instant the hook returns and BEFORE it looks
                // at `beforeResult.block` (`agent-loop.ts:629-635` @v0.83.0), so an abort landing
                // during the hook OUT-VOTES a block and the transcript attributes the stop to the
                // user rather than to policy.
                if self.cancel.is_cancelled() {
                    return Prep::Immediate(Box::new(
                        self.immediate_error(call, "Operation aborted", false),
                    ));
                }
                match outcome {
                    BeforeOutcome::Block { reason, terminate } => {
                        Prep::Immediate(Box::new(self.immediate_error(
                            call,
                            // AGENT-010 + AGENT-032(a) — pi is
                            // `createErrorToolResult(beforeResult.reason || "Tool execution was
                            // blocked")` (`agent-loop.ts:639` @v0.83.0, `:637` @v0.84.1). `||` is
                            // JS-FALSY, so an empty-string reason yields the DEFAULT text; an
                            // `Option`-only fallback let `Some("")` through as an empty text content
                            // block, which Anthropic's Messages API rejects with a 400. The
                            // extension seam can produce exactly that (`block(some(""))`).
                            reason
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| "Tool execution was blocked".to_string()),
                            // AGENT-022 — `if (beforeResult.terminate === true) { result.terminate =
                            // true; }` (`agent-loop.ts:637-645` @v0.84.1).
                            terminate,
                        )))
                    }
                    // Args mutated in place are executed as-is, WITHOUT re-validation (R-02-022).
                    BeforeOutcome::Proceed => {
                        // pi's SECOND abort check, outside the `if (config.beforeToolCall)` block
                        // (`agent-loop.ts:644-650` @v0.83.0, `:648` @v0.84.1).
                        if self.cancel.is_cancelled() {
                            Prep::Immediate(Box::new(
                                self.immediate_error(call, "Operation aborted", false),
                            ))
                        } else {
                            Prep::Ready { tool, args }
                        }
                    }
                }
            }
        }
    }

    /// Pi `createErrorToolResult(message)` (`packages/agent/src/agent-loop.ts:756-761` @v0.83.0):
    /// `{ content: [{type:"text", text: message}], details: {} }` — an object literal for `details`
    /// and NO `terminate` key. `terminate` is stamped onto it only by the v0.84.1 blocked-call arm
    /// (`agent-loop.ts:637-645`, AGENT-022), which is what the `terminate` parameter carries.
    pub(super) fn immediate_error(
        &self,
        call: &ToolCall,
        msg: impl Into<SharedStr>,
        terminate: bool,
    ) -> Finalized {
        // AGENT-009 — `details: {}`, not absent: pi writes the empty object literal, so the JSONL
        // transcript records `"details":{}` and `tool_execution_end.result` carries the key.
        let message = ToolResultMessage {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            content: vec![Content::text(msg)],
            details: Some(empty_details()),
            // Pi's `createErrorToolResult` builds `{content, details:{}}` and nothing else
            // (agent-loop.ts:756-761): a call that did not run reports no usage and cannot have
            // introduced a tool, so an error result never anchors deferred tool loading.
            usage: None,
            added_tool_names: Vec::new(),
            is_error: true,
            // Pi `createToolResultMessage` stamps every tool result with `Date.now()`
            // (agent-loop.ts:741); this reaches the wire payload via `convert_to_llm`.
            timestamp: now_millis(),
        };
        // pi's blocked-with-terminate arm assigns `result.terminate = true` only when the hook asked
        // for it; every other error result leaves the key absent.
        let terminate = if terminate { Some(true) } else { None };
        Finalized {
            source_index: 0,
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            result_value: result_value_of(&message.content, &message.details, None, &[], terminate),
            is_error: true,
            terminate,
            message,
        }
    }
}

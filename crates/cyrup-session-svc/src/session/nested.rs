//! Tool calls that a tool makes while it runs — the session half of pi's nested tool calls.
//!
//! Pi `AgentSession._executeNestedToolCall` (`core/agent-session.ts:699-740` @v1.0.1) backs
//! `ctx.executeTool()`: *"Run a call that the tool call `parentToolCallId` made through
//! `ctx.executeTool()`. It goes through the agent's tool pipeline with the session's hooks, against
//! the callable tools."* The bookkeeping — ids, the exclusive queue, the record — is
//! [`cyrup_agent::NestedToolCallRunner`]; what it needs from a session is the
//! [`cyrup_agent::NestedToolCallHost`] seam implemented here.
//!
//! Where the pieces land:
//!
//! * the call resolves against the CALLABLE tools ([`crate::tools::DynamicToolState::callable_tools`]),
//!   never the declared set;
//! * it runs through [`cyrup_agent::run_tool_call`] with the session's hooks
//!   (`PolicyHooks`), whose nested entry points hand the extension chain the parent id;
//! * its `tool_execution_*` events go to the extensions and to the session's subscribers
//!   ([`AgentSessionEvent::NestedToolExecutionStart`] and siblings);
//! * the model-issued call's tool result is stamped with the record and the summed usage at
//!   `message_end` (`SvcSubscriber::stamp_nested_calls`).

use std::sync::Arc;

use cyrup_agent::{
    AgentContextView, AgentMessage, NestedToolCallHost, NestedToolCallOptions,
    NestedToolExecutionEvent, RunToolCallOptions, ToolCallOutcome, ToolExecution, run_tool_call,
};
use cyrup_core::{CancelToken, Content, Tool, ToolCall, ToolCallId, ToolResult, ToolUpdateSink};
use serde_json::Value;

use crate::event::AgentSessionEvent;

use super::AgentSession;

/// The result text pi returns when a nested call finds no assistant message to attribute the call
/// to (`agent-session.ts:708-712`).
pub const NO_ASSISTANT_MESSAGE: &str = "No assistant message issued this call";

impl AgentSession {
    /// Run a tool call on behalf of the tool call `caller_call_id` — pi `_executeNestedToolCall`
    /// (`agent-session.ts:699-740` @v1.0.1), the session API behind an extension tool's
    /// `ctx.executeTool(name, args, options)` (`core/extensions/types.ts:376-394`).
    ///
    /// The call gets the id `<caller_call_id>/<n>`; `tool_call`, `tool_result` and
    /// `tool_execution_*` events carry `caller_call_id` as their parent; it never appears in the
    /// transcript; and its record (and usage) is stamped on `caller_call_id`'s tool result when
    /// that result is appended — which is why `caller_call_id` must be the id of the tool call the
    /// calling tool is executing, or of another nested call (`<id>/<n>`), whose calls are recorded
    /// on the same model-issued result.
    ///
    /// **Never fails for tool failures.** An unknown tool, a tool that is not callable (hidden,
    /// `model-only`, an inactive `direct` one), a validation error, a call a hook blocked, and a
    /// tool that failed all come back as an outcome with `is_error` set; so does a call made when no
    /// assistant message exists to attribute it to ([`NO_ASSISTANT_MESSAGE`]).
    ///
    /// `args` is the arguments object; `Null` is an empty object. `options.cancel` is the call's
    /// abort signal; pi's `ExecuteToolOptions.signal` defaults to the calling tool's, which is the
    /// calling surface's to supply (the tool's own [`CancelToken`]).
    ///
    /// This is the signature an extension-facing `ctx.executeTool` binds.
    pub async fn execute_nested_tool(
        &self,
        caller_call_id: &ToolCallId,
        name: &str,
        args: Value,
        options: NestedToolCallOptions,
    ) -> ToolCallOutcome {
        self.nested_calls
            .execute(
                &SessionNestedHost { session: self },
                caller_call_id,
                name,
                args,
                options,
            )
            .await
    }

    /// The tools a nested call resolves against — pi `_getCallableTools()`
    /// (`agent-session.ts:1515-1520`): the active `direct` tools and every registered `codemode`
    /// or `deferred` one.
    fn callable_tools(&self) -> Vec<Arc<dyn Tool>> {
        Self::lock(&self.dynamic_tools).callable_tools()
    }
}

/// [`NestedToolCallHost`] over a session — pi's inline `new NestedToolCallRunner({ getTools,
/// isSequential, runToolCall, emit })` (`agent-session.ts:699-740`).
struct SessionNestedHost<'a> {
    session: &'a AgentSession,
}

fn refusal(call: ToolCall, text: &str) -> ToolCallOutcome {
    ToolCallOutcome {
        tool_call: call,
        result: ToolResult {
            content: vec![Content::text(text)],
            // pi's `{ content: [...], details: {} }`.
            details: Some(Value::Object(serde_json::Map::new())),
            ..ToolResult::default()
        },
        is_error: true,
    }
}

#[async_trait::async_trait]
impl NestedToolCallHost for SessionNestedHost<'_> {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.session.callable_tools()
    }

    fn is_sequential(&self) -> bool {
        self.session.agent.tool_execution() == ToolExecution::Sequential
    }

    async fn run_tool_call(
        &self,
        call: ToolCall,
        parent: &ToolCallId,
        cancel: Option<CancelToken>,
        on_update: ToolUpdateSink,
    ) -> ToolCallOutcome {
        let state = self.session.agent.snapshot().await;
        // pi `_findLastAssistantMessage()`: the most recent assistant message in the transcript,
        // which the `message_end` barrier has put there before any tool of its calls runs.
        let Some(assistant) = state.messages.iter().rev().find_map(|m| match m {
            AgentMessage::Assistant(a) => Some(Arc::clone(a)),
            _ => None,
        }) else {
            return refusal(call, NO_ASSISTANT_MESSAGE);
        };
        let tools = self.tools();
        let messages: Vec<Arc<AgentMessage>> = state.messages.into_iter().map(Arc::new).collect();
        let agent_tools = self.session.agent.tools().await;
        run_tool_call(
            call,
            RunToolCallOptions {
                tools: &tools,
                assistant_message: &assistant,
                context: AgentContextView {
                    system_prompt: &state.system_prompt,
                    messages: &messages,
                    // `context.tools` is the agent's own set, not the callable one: it is what the
                    // hooks are shown (pi `{ messages, tools: this.agent.state.tools }`).
                    tools: &agent_tools,
                },
                hooks: &*self.session.nested_hooks,
                cancel,
                on_update: Some(on_update),
                parent_tool_call_id: Some(parent),
            },
        )
        .await
    }

    /// Pi `await this._extensionRunner.emit(event); this._emit(event);`
    /// (`agent-session.ts:719-733`): the extensions first, then the session's subscribers.
    async fn emit(&self, event: NestedToolExecutionEvent) {
        let cancel = self.session.session_cancel.child_token();
        self.session
            .services
            .ext_host
            .emit_nested_tool_execution(&event, &cancel)
            .await;
        self.session
            .fanout_emit(AgentSessionEvent::from(event))
            .await;
    }
}

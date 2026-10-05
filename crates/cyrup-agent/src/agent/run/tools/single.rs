//! One tool call, run on behalf of a caller rather than a model-issued batch — AGENT-047.
//!
//! The public entry point is [`run_tool_call`], cyrup's counterpart to pi's exported
//! `runToolCall` (`packages/agent/src/agent-loop.ts:810-818` @v1.0.1). It composes the SAME three
//! stages the batch runtimes compose — [`prepare_tool_call`], [`execute_prepared`],
//! [`finalize_tool_call`] — so argument preparation, schema validation, `before_tool_call`,
//! execution and `after_tool_call` all apply to a nested call exactly as they do to a model-issued
//! one. That is the point: a permission hook cannot be routed around by a tool that calls a tool.

use super::exec::execute_prepared;
use super::finalize::finalize_tool_call;
use super::preflight::prepare_tool_call;
use super::{Prep, PreparedCall, ToolCallEnv};
use crate::event::AgentMessage;
use crate::hooks::{AgentContextView, Hooks};
use cyrup_core::{AssistantMessage, CancelToken, Tool, ToolCall, ToolCallId, ToolUpdateSink};
use std::sync::Arc;

pub use super::finalized::ToolCallOutcome;

/// Options for [`run_tool_call`] — pi `RunToolCallOptions` (`agent-loop.ts:790-799` @v1.0.1,
/// `extends ToolCallHooks`).
///
/// `tools` and `context.tools` are separate on purpose, as they are upstream: `tools` is the slice
/// the NAME resolves against (pi's `tools` parameter, `agent-loop.ts:713`/`:812`), `context` is what
/// the hooks are shown. A nested call may therefore run against a tool set the turn never had —
/// which is how a codemode sandbox exposes a filtered list without changing the model's.
pub struct RunToolCallOptions<'a> {
    /// Tools the call resolves against (pi `tools`, `:792`).
    pub tools: &'a [Arc<dyn Tool>],
    /// Passed to the hooks as the message that issued the call (pi `assistantMessage`, `:794`).
    pub assistant_message: &'a AssistantMessage,
    /// Passed to the hooks as the current agent context (pi `context`, `:796`).
    pub context: AgentContextView<'a>,
    /// The `before_tool_call` / `after_tool_call` seam (pi `ToolCallHooks`, `:683`). cyrup bundles
    /// its hooks behind one trait object, so this is the bundle; only those two are consulted.
    pub hooks: &'a dyn Hooks,
    /// Abort signal (pi `signal?`, `:797`). `None` is an uncancellable call.
    pub cancel: Option<CancelToken>,
    /// Streamed partial results (pi `onUpdate?`, `:798`). `None` is pi's `options.onUpdate ?? (() =>
    /// {})` (`:816`) — the updates are dropped on the floor, because emitting them is the batch
    /// runtime's job and this path emits nothing.
    pub on_update: Option<ToolUpdateSink>,
    /// The tool call that made this call, when a tool is calling a tool (CODE-006). Pi has no such
    /// field on `RunToolCallOptions`: its session passes hooks that close over the parent id
    /// (`beforeToolCall: (context) => this._beforeToolCall(context, parentId)`,
    /// `agent-session.ts:719-733` @v1.0.1). cyrup bundles its hooks behind one trait object, so the
    /// parent travels beside it and selects the hooks' nested entry points
    /// ([`Hooks::before_nested_tool_call`], [`Hooks::after_nested_tool_call`]) instead.
    /// `None` runs the ordinary entry points.
    pub parent_tool_call_id: Option<&'a ToolCallId>,
}

/// Run one tool call through the same steps as a model-issued call: argument preparation, schema
/// validation, `before_tool_call`, execution, and `after_tool_call`. **Emits no events and adds no
/// messages.** Tools that call other tools use this so the hooks (for example permission checks)
/// apply to those calls too.
///
/// **Never returns `Err`** — the signature has no `Result` at all. Unknown tools, validation
/// errors, blocked calls, a failing hook and a tool that returns `Err(ToolError)` or panics all
/// come back as [`ToolCallOutcome::is_error`] `== true` with the failure text in
/// `result.content`. Pi states the same contract in prose (*"Never rejects for tool failures"*,
/// `agent-loop.ts:807` @v1.0.1); in Rust it is in the type.
///
/// "Emits no events" is likewise structural rather than a rule to remember: this function has no
/// subscriber list and no transcript to append to. The three stages it composes are exactly the
/// ones the batch runtimes compose, and every `AgentEvent` the batch publishes is emitted by the
/// runtime AROUND them, never from inside — which is what pi achieved by narrowing
/// `executePreparedToolCall`'s sink to a plain callback and moving event construction out into
/// `emitToolExecutionUpdate` (`:778`, `:823`).
///
/// The `source_index` the stages carry is the position of the answered call in the assistant
/// message's tool-call list. A programmatic call answers no list, so it is `0` and discarded with
/// the rest of the batch bookkeeping: [`ToolCallOutcome`] has no such field, and neither does pi's
/// `AgentToolCallOutcome`.
pub async fn run_tool_call(call: ToolCall, options: RunToolCallOptions<'_>) -> ToolCallOutcome {
    let RunToolCallOptions {
        tools,
        assistant_message,
        context,
        hooks,
        cancel,
        on_update,
        parent_tool_call_id,
    } = options;
    // pi has no `newMessages` on the `beforeToolCall` context; cyrup's field is a backward-compat
    // extra documented on `BeforeToolCall::messages`, and a programmatic call produced none.
    const NO_NEW_MESSAGES: &[Arc<AgentMessage>] = &[];
    let env = ToolCallEnv {
        hooks,
        cancel: cancel.unwrap_or_default(),
        resolve_tools: tools,
        context,
        new_messages: NO_NEW_MESSAGES,
        assistant: assistant_message,
        parent: parent_tool_call_id,
    };

    // pi: `if (preparation.kind === "immediate") return { toolCall, result, isError }`
    // (`agent-loop.ts:813-815` @v1.0.1) — an unknown tool, a validation failure, a block and an
    // abort all return HERE, so `after_tool_call` never sees a call that did not execute.
    let prepared = match prepare_tool_call(&env, &call, 0).await {
        Prep::Immediate(fin) => return fin.into_outcome(call),
        Prep::Ready(prepared) => prepared,
    };
    let PreparedCall { tool, args, .. } = prepared;

    // pi: `executePreparedToolCall(preparation, signal, options.onUpdate ?? (() => {}))` (`:816`).
    let outcome = execute_prepared(
        tool,
        call.id.clone(),
        args.clone(),
        env.cancel.child_token(),
        on_update.unwrap_or_else(|| Box::new(|_| {})),
    )
    .await;

    // pi: `return finalizeExecutedToolCall(context, assistantMessage, preparation, executed,
    // options, signal)` (`:817`).
    finalize_tool_call(&env, &call, 0, args, outcome)
        .await
        .into_outcome(call)
}

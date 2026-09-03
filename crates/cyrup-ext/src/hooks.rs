//! `ExtHooks` — the mutating agent seam (arch-08 §5.4). Implements `cyrup_agent::Hooks`: the agent
//! awaits these and applies their block/mutate results. `before_tool_call` blocks/mutates the tool
//! input (R-08-010, the permission seam), `after_tool_call` patches the result (R-08-011),
//! `transform_context` filters/replaces the message list (R-08-028 subset). Mutating results flow
//! back through the `Hooks` return values, never through `on_event`.

use crate::contract::Reduced;
use crate::dispatch::Dispatcher;
use crate::event::HostEvent;
use cyrup_agent::{
    AfterOutcome, AfterOverride, AfterToolCall, AgentMessage, BeforeOutcome, BeforeToolCall, Hooks,
};
use cyrup_agent::HookError;
use cyrup_core::{CancelToken, TerminateHint};
use std::sync::Arc;

/// The mutating hooks adapter handed to the agent (arch-08 §3.1). Shares the dispatcher with
/// [`crate::subscriber::ExtSubscriber`].
pub struct ExtHooks {
    dispatcher: Arc<Dispatcher>,
}

impl ExtHooks {
    pub fn new(dispatcher: Arc<Dispatcher>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait::async_trait]
impl Hooks for ExtHooks {
    /// The permission seam (R-08-010): first `Block` wins (agent produces an `isError` result and
    /// does NOT execute); otherwise the folded input is written back and the call proceeds.
    async fn before_tool_call(
        &self,
        ctx: BeforeToolCall<'_>,
        cancel: CancelToken,
    ) -> BeforeOutcome {
        let ev = HostEvent::ToolCall {
            call_id: ctx.tool_call_id.clone(),
            name: ctx.tool_name.to_string(),
            input: ctx.args.clone(),
        };
        match self.dispatcher.dispatch_block_mutate(ev, &cancel).await {
            // EXT-029. `cancel` is a FRESH child of the run token, minted at the call site and
            // handed to nobody else (`self.cancel.child()`, cyrup-agent/src/agent.rs:1009), so it
            // can only be cancelled by the run root — a cancelled token here means the USER
            // aborted, not that the extension declined. Returning `Proceed` does NOT run the tool:
            // the agent re-checks the root immediately (`BeforeOutcome::Block { .. } |
            // BeforeOutcome::Proceed if self.cancel.is_cancelled()`, tools/preflight.rs) and produces pi's own
            // "Operation aborted" error result (packages/agent/src/agent-loop.ts:629-635
            // @v0.84.1), which is the text the transcript should show.
            _ if cancel.is_cancelled() => BeforeOutcome::Proceed,
            // EXT-049: `terminate` rides the block through to the finalized error result, where
            // the agent's every()-rule (`shouldTerminateToolBatch`,
            // packages/agent/src/agent-loop.ts:583 @v0.84.1) decides whether the BATCH ends.
            Reduced::Blocked { reason, terminate, .. } => {
                BeforeOutcome::Block { reason, terminate }
            }
            Reduced::Pass(ev) => {
                if let HostEvent::ToolCall { input, .. } = *ev {
                    *ctx.args = input; // mutated args execute as-is, WITHOUT re-validation (R-02-022)
                }
                BeforeOutcome::Proceed
            }
            // Handled (shouldn't happen) => proceed unmodified.
            _ => BeforeOutcome::Proceed,
        }
    }

    /// Patch the tool result (R-08-011). Replace-not-merge: only fields a handler changed are
    /// returned as `Some(_)` (func-02 R-02-025).
    async fn after_tool_call(
        &self,
        ctx: AfterToolCall<'_>,
        cancel: CancelToken,
    ) -> AfterOutcome {
        let orig_content = ctx.content.to_vec();
        let orig_is_error = ctx.is_error;
        let orig_details = ctx.details.cloned();
        // The tool's own reported usage (Pi `ToolResultEventBase.usage`, types.ts:919-921). Passed
        // in so a handler can OBSERVE it, and diffed below so a handler can PATCH it — Pi wires both
        // directions (`runner.emitToolResult({..., usage: result.usage})` then
        // `usage: hookResult.usage`, agent-session.ts:490-516).
        let orig_usage = ctx.usage.cloned();
        let orig_terminate: TerminateHint = ctx.terminate;
        let ev = HostEvent::ToolResult {
            call_id: ctx.tool_call_id.clone(),
            name: ctx.tool_name.to_string(),
            // The executed tool's arguments (Pi `ToolResultEventBase.input`, types.ts:886).
            input: ctx.args.clone(),
            content: orig_content.clone(),
            details: orig_details.clone(),
            is_error: orig_is_error,
            usage: orig_usage.clone(),
            terminate: orig_terminate,
        };
        match self.dispatcher.dispatch_block_mutate(ev, &cancel).await {
            Reduced::Pass(ev) => {
                let HostEvent::ToolResult { content, details, is_error, usage, terminate, .. } = *ev
                else {
                    return AfterOutcome::Keep;
                };
                let mut over = AfterOverride::default();
                let mut changed = false;
                if content != orig_content {
                    over.content = Some(content);
                    changed = true;
                }
                if details != orig_details {
                    over.details = details;
                    changed = true;
                }
                if is_error != orig_is_error {
                    over.is_error = Some(is_error);
                    changed = true;
                }
                // `AfterOverride::usage` is replace-not-merge and `None` means "keep", so only a
                // handler-supplied CHANGE is forwarded. A handler that clears the usage cannot be
                // expressed — neither can it upstream (`usage: afterResult.usage ?? result.usage`,
                // agent-loop.ts:738).
                if usage != orig_usage && usage.is_some() {
                    over.usage = usage;
                    changed = true;
                }
                // A patched hint — including a patch to `Unspecified`, which clears it — is
                // forwarded as `Some`; `AfterOverride::terminate == None` means "hook had no opinion".
                if terminate != orig_terminate {
                    over.terminate = Some(terminate);
                    changed = true;
                }
                if changed { AfterOutcome::Override(over) } else { AfterOutcome::Keep }
            }
            // A block on a result is not meaningful; keep the original.
            _ => AfterOutcome::Keep,
        }
    }

    /// Filter/replace the LLM context (R-08-028 subset). Runs before `convert_to_llm`.
    async fn transform_context(
        &self,
        msgs: Vec<Arc<AgentMessage>>,
        cancel: CancelToken,
    ) -> Result<Vec<Arc<AgentMessage>>, HookError> {
        // `msgs.clone()` is a POINTER clone now (PERF-002). It runs before the `no_subscribers`
        // gate inside `dispatch_block_mutate`, so every turn paid a whole-transcript deep copy
        // here even with no `context`-subscribing extension wired. Do not "fix" a type mismatch
        // at this line by unwrapping the handles — that restores exactly that copy.
        let ev = HostEvent::Context { messages: msgs.clone() };
        match self.dispatcher.dispatch_block_mutate(ev, &cancel).await {
            Reduced::Pass(ev) => match *ev {
                HostEvent::Context { messages } => Ok(messages),
                _ => Ok(msgs),
            },
            _ => Ok(msgs),
        }
    }
}

//! `ExtHooks` — the mutating agent seam (arch-08 §5.4). Implements `cyrup_agent::Hooks`: the agent
//! awaits these and applies their block/mutate results. `before_tool_call` blocks/mutates the tool
//! input (R-08-010, the permission seam), `after_tool_call` patches the result (R-08-011),
//! `transform_context` filters/replaces the message list (R-08-028 subset). Mutating results flow
//! back through the `Hooks` return values, never through `on_event`.

use crate::contract::Reduced;
use crate::dispatch::Dispatcher;
use crate::event::HostEvent;
use cyrup_agent::{
    AfterOverride, AfterToolCall, AgentMessage, BeforeOutcome, BeforeToolCall, Hooks,
};
use cyrup_agent::HookError;
use cyrup_core::CancelToken;
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
    ) -> Result<BeforeOutcome, HookError> {
        let ev = HostEvent::ToolCall {
            call_id: ctx.tool_call_id.clone(),
            name: ctx.tool_name.to_string(),
            input: ctx.args.clone(),
        };
        match self.dispatcher.dispatch_block_mutate(ev, &cancel).await {
            Reduced::Blocked { reason, .. } => Ok(BeforeOutcome::Block { reason }),
            Reduced::Pass(HostEvent::ToolCall { input, .. }) => {
                *ctx.args = input; // mutated args execute as-is, WITHOUT re-validation (R-02-022)
                Ok(BeforeOutcome::Proceed)
            }
            // Handled / shape-shift (shouldn't happen) => proceed unmodified.
            _ => Ok(BeforeOutcome::Proceed),
        }
    }

    /// Patch the tool result (R-08-011). Replace-not-merge: only fields a handler changed are
    /// returned as `Some(_)` (func-02 R-02-025).
    async fn after_tool_call(
        &self,
        ctx: AfterToolCall<'_>,
        cancel: CancelToken,
    ) -> Result<Option<AfterOverride>, HookError> {
        let orig_content = ctx.content.to_vec();
        let orig_is_error = ctx.is_error;
        let orig_details = ctx.details.cloned();
        let ev = HostEvent::ToolResult {
            call_id: ctx.tool_call_id.clone(),
            name: ctx.tool_name.to_string(),
            content: orig_content.clone(),
            details: orig_details.clone(),
            is_error: orig_is_error,
        };
        match self.dispatcher.dispatch_block_mutate(ev, &cancel).await {
            Reduced::Pass(HostEvent::ToolResult { content, details, is_error, .. }) => {
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
                Ok(if changed { Some(over) } else { None })
            }
            // A block on a result is not meaningful; keep the original.
            _ => Ok(None),
        }
    }

    /// Filter/replace the LLM context (R-08-028 subset). Runs before `convert_to_llm`.
    async fn transform_context(
        &self,
        msgs: Vec<AgentMessage>,
        cancel: CancelToken,
    ) -> Result<Vec<AgentMessage>, HookError> {
        let ev = HostEvent::Context { messages: msgs.clone() };
        match self.dispatcher.dispatch_block_mutate(ev, &cancel).await {
            Reduced::Pass(HostEvent::Context { messages }) => Ok(messages),
            _ => Ok(msgs),
        }
    }
}

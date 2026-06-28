//! `PolicyHooks` — composes the opt-in [`PermissionPolicy`] gate (arch-12) in front of the
//! extension `Hooks` seam (arch-08). The agent owns a single `Hooks` slot, so the facade folds the
//! permission decision and the extension mutating seam into one: the policy decides first
//! (proceed/mutate/block/confirm), then — unless blocked — the extensions' `before_tool_call`
//! chain runs and may further block/mutate. All other hook methods delegate straight to the
//! extension seam.

use std::sync::Arc;

use cyrup_agent::{
    AfterOverride, AfterToolCall, AgentMessage, BeforeOutcome, BeforeToolCall, HookError, Hooks,
    PostTurn, TurnUpdate,
};
use cyrup_core::{CancelToken, Message};
use cyrup_tools::{PermissionPolicy, PolicyDecision};

/// The composed hooks handed to the agent (permission gate → extension hooks).
pub(crate) struct PolicyHooks {
    policy: PermissionPolicy,
    inner: Arc<dyn Hooks>,
    /// Whether a `Confirm` decision may be auto-resolved (interactive UI). Non-interactive modes
    /// block-by-default on `Confirm` (arch-12 R-12-009).
    has_ui: bool,
}

impl PolicyHooks {
    pub(crate) fn new(policy: PermissionPolicy, inner: Arc<dyn Hooks>, has_ui: bool) -> Self {
        Self { policy, inner, has_ui }
    }
}

#[async_trait::async_trait]
impl Hooks for PolicyHooks {
    async fn convert_to_llm(&self, msgs: &[AgentMessage]) -> Result<Vec<Message>, HookError> {
        self.inner.convert_to_llm(msgs).await
    }

    async fn transform_context(
        &self,
        msgs: Vec<AgentMessage>,
        cancel: CancelToken,
    ) -> Result<Vec<AgentMessage>, HookError> {
        self.inner.transform_context(msgs, cancel).await
    }

    async fn before_tool_call(
        &self,
        ctx: BeforeToolCall<'_>,
        cancel: CancelToken,
    ) -> Result<BeforeOutcome, HookError> {
        // 1. Opt-in permission policy (empty policy ⇒ always Proceed, the YOLO default R-12-001).
        match self.policy.evaluate(ctx.tool_name, ctx.args) {
            PolicyDecision::Proceed => {}
            PolicyDecision::Mutate { input } => *ctx.args = input,
            PolicyDecision::Block { reason } => return Ok(BeforeOutcome::Block { reason: Some(reason) }),
            PolicyDecision::Confirm { reason } => {
                if !self.has_ui {
                    // No UI to prompt: block-by-default (R-12-009).
                    return Ok(BeforeOutcome::Block { reason: Some(reason) });
                }
                // With UI the front-end resolves confirmation; absent a wired confirm hook we
                // proceed (the interactive front-end owns the prompt — arch-10/12).
            }
        }
        // 2. Extension mutating seam (may further block / rewrite the — possibly mutated — args).
        self.inner.before_tool_call(ctx, cancel).await
    }

    async fn after_tool_call(
        &self,
        ctx: AfterToolCall<'_>,
        cancel: CancelToken,
    ) -> Result<Option<AfterOverride>, HookError> {
        self.inner.after_tool_call(ctx, cancel).await
    }

    async fn prepare_next_turn(
        &self,
        ctx: PostTurn<'_>,
    ) -> Result<Option<TurnUpdate>, HookError> {
        self.inner.prepare_next_turn(ctx).await
    }

    async fn should_stop_after_turn(&self, ctx: PostTurn<'_>) -> Result<bool, HookError> {
        self.inner.should_stop_after_turn(ctx).await
    }
}

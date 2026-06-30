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

/// The placeholder text Pi substitutes for a blocked image (sdk.ts:270).
const BLOCKED_IMAGE_TEXT: &str = "Image reading is disabled.";

/// The composed hooks handed to the agent (permission gate → extension hooks).
pub(crate) struct PolicyHooks {
    policy: PermissionPolicy,
    inner: Arc<dyn Hooks>,
    /// Whether a `Confirm` decision may be auto-resolved (interactive UI). Non-interactive modes
    /// block-by-default on `Confirm` (arch-12 R-12-009).
    has_ui: bool,
    /// `blockImages` defense-in-depth: strip image content from converted LLM messages (Pi
    /// sdk.ts:254-289). Resolved once at build time from settings.
    block_images: bool,
}

impl PolicyHooks {
    pub(crate) fn new(
        policy: PermissionPolicy,
        inner: Arc<dyn Hooks>,
        has_ui: bool,
        block_images: bool,
    ) -> Self {
        Self { policy, inner, has_ui, block_images }
    }
}

/// Replace every [`Content::Image`] with the placeholder text, deduping consecutive placeholders
/// (Pi sdk.ts:262-288). Applied to `user`/`toolResult` message content only.
fn filter_images(content: &[cyrup_core::Content]) -> Vec<cyrup_core::Content> {
    use cyrup_core::Content;
    let mut out: Vec<Content> = Vec::with_capacity(content.len());
    for c in content {
        let replaced = match c {
            Content::Image { .. } => Content::text(BLOCKED_IMAGE_TEXT),
            other => other.clone(),
        };
        // Dedupe consecutive "Image reading is disabled." placeholders.
        let is_placeholder = matches!(&replaced, Content::Text { text, .. } if text == BLOCKED_IMAGE_TEXT);
        let prev_placeholder = matches!(
            out.last(),
            Some(Content::Text { text, .. }) if text == BLOCKED_IMAGE_TEXT
        );
        if is_placeholder && prev_placeholder {
            continue;
        }
        out.push(replaced);
    }
    out
}

#[async_trait::async_trait]
impl Hooks for PolicyHooks {
    async fn convert_to_llm(&self, msgs: &[AgentMessage]) -> Result<Vec<Message>, HookError> {
        let converted = self.inner.convert_to_llm(msgs).await?;
        if !self.block_images {
            return Ok(converted);
        }
        Ok(converted
            .into_iter()
            .map(|m| match m {
                Message::User { content, timestamp } if content.iter().any(is_image) => {
                    Message::User { content: filter_images(&content), timestamp }
                }
                Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    is_error,
                    details,
                    timestamp,
                } if content.iter().any(is_image) => Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content: filter_images(&content),
                    is_error,
                    details,
                    timestamp,
                },
                other => other,
            })
            .collect())
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

/// Whether a content block is an image (the trigger for the `blockImages` rewrite).
fn is_image(c: &cyrup_core::Content) -> bool {
    matches!(c, cyrup_core::Content::Image { .. })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use cyrup_core::Content;

    #[test]
    fn filter_images_replaces_and_dedupes_placeholders() {
        let img = || Content::Image { data: "AAAA".into(), mime_type: "image/png".into() };
        // Two adjacent images collapse to a single placeholder; surrounding text is preserved.
        let content = vec![Content::text("before"), img(), img(), Content::text("after")];
        let out = filter_images(&content);
        assert_eq!(out.len(), 3, "two adjacent images dedupe to one placeholder: {out:?}");
        assert!(matches!(&out[0], Content::Text { text, .. } if text == "before"));
        assert!(matches!(&out[1], Content::Text { text, .. } if text == BLOCKED_IMAGE_TEXT));
        assert!(matches!(&out[2], Content::Text { text, .. } if text == "after"));
    }

    #[test]
    fn filter_images_keeps_image_free_content_intact() {
        let content = vec![Content::text("a"), Content::text("b")];
        let out = filter_images(&content);
        assert_eq!(out.len(), 2);
    }
}

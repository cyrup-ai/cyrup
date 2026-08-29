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

/// SESS-043 — cyrup's `convertToLlm` (`coding-agent/src/core/messages.ts:148-195` @v0.83.0), the
/// function pi hands the Agent as `convertToLlm` (`coding-agent/src/core/sdk.ts:256-301`).
///
/// pi has TWO of these: the base `defaultConvertToLlm` in the agent package, which keeps only
/// `user`/`assistant`/`toolResult`, and this one in the coding agent, which additionally renders the
/// four declaration-merged roles. cyrup only had the base one
/// ([`cyrup_agent::default_convert_to_llm`]), which is why the coding-agent roles could not live in
/// the transcript at all: anything that entered would silently vanish from the request.
///
/// The three [`AgentMessage::App`] roles are rendered by handing the stored pi wire object back to
/// `cyrup-session`, whose `push_llm` IS this crate's other copy of the same upstream switch — so the
/// transcript-seeded path and the `build_context()` path cannot drift. `custom` is rendered by
/// [`cyrup_session::agent_message::custom_to_message`], pi's `case "custom"` (`:162-168`): before
/// this, a `custom` message was dropped here while the SAME message rendered into the request after
/// a compaction re-seed, because `build_context()` had already flattened it to a `user` turn.
pub(crate) fn coding_agent_convert_to_llm(msgs: &[AgentMessage]) -> Vec<Message> {
    use cyrup_session::agent_message::AgentMessage as Raw;

    let mut out = Vec::with_capacity(msgs.len());
    for m in msgs {
        match m {
            AgentMessage::User { content, timestamp } => {
                out.push(Message::User { content: content.clone(), timestamp: timestamp.unwrap_or(0) });
            }
            AgentMessage::Assistant(a) => out.push(Message::Assistant((**a).clone())),
            AgentMessage::ToolResult(t) => out.push(Message::ToolResult {
                tool_call_id: t.tool_call_id.clone(),
                tool_name: t.tool_name.clone(),
                content: t.content.clone(),
                is_error: t.is_error,
                details: t.details.clone(),
                // Both must cross the agent→LLM boundary — see `cyrup_agent::default_convert_to_llm`.
                usage: t.usage.clone(),
                added_tool_names: t.added_tool_names.clone(),
                timestamp: t.timestamp,
            }),
            // `kind` is OVERLOADED on this arm and both meanings have to be honoured here.
            //
            // For an extension message it is pi's `customType` and `payload` is pi's `content`, so
            // `case "custom"` (`messages.ts:162-168` @v0.83.0) applies. But `record_bash_result`
            // (`session.rs`) also appends a LIVE `!` execution as `Custom { kind: "bashExecution",
            // payload: <the whole BashExecutionMessage object> }` — pi has a first-class
            // `bashExecution` ROLE there, and the session file already treats that `customType` as
            // the role (`append_custom_message("bashExecution", …)` reloads as
            // `Raw::BashExecution`). Rendering such a message through `custom_to_message` would hit
            // its stringify catch-all and inject the raw JSON object as a user turn — and, worse,
            // would ignore `excludeFromContext`, so a `!!` command's output would reach the model
            // on the live turn. pi's `case "bashExecution"` returns `undefined` for exactly that
            // message (`:152-156`).
            //
            // So a `kind` naming one of pi's declaration-merged roles is reconstituted into its pi
            // wire object and rendered by the SAME `push_llm` the `App` arm below uses — the two
            // paths cannot disagree, which they did until this was added: after a compaction
            // re-seed the same execution arrives as `App { role: "bashExecution" }` and IS dropped.
            AgentMessage::Custom { kind, payload, timestamp, .. } => {
                match app_role_payload(kind, payload, *timestamp)
                    .and_then(|v| serde_json::from_value::<Raw>(v).ok())
                {
                    Some(raw) => raw.push_llm(&mut out),
                    None => out.push(cyrup_session::agent_message::custom_to_message(
                        payload,
                        timestamp.unwrap_or(0),
                    )),
                }
            }
            // A payload this crate wrote and cannot read back would be a bug, not user data, so
            // the `Err` arm skips exactly this message — pi's `default:` case
            // (`messages.ts:187-190`), which likewise returns `undefined` and is filtered out.
            AgentMessage::App { payload, .. } => {
                if let Ok(raw) =
                    serde_json::from_value::<Raw>(serde_json::Value::Object(payload.clone()))
                {
                    raw.push_llm(&mut out);
                }
            }
        }
    }
    out
}

/// Rebuild the pi wire object for an [`AgentMessage::Custom`] whose `kind` is in fact one of pi's
/// declaration-merged ROLES rather than a `customType` — see the `Custom` arm above.
///
/// Returns `None` for a genuine `custom` message, which is every `kind` outside
/// [`cyrup_agent::APP_MESSAGE_ROLES`]. The `role` key is injected because cyrup's producer stores
/// only the body, and `timestamp` is filled from the transcript entry when the body has none (all
/// three target structs carry `#[serde(default)] timestamp`).
fn app_role_payload(
    kind: &str,
    payload: &serde_json::Value,
    timestamp: Option<i64>,
) -> Option<serde_json::Value> {
    if !cyrup_agent::APP_MESSAGE_ROLES.contains(&kind) {
        return None;
    }
    let serde_json::Value::Object(mut obj) = payload.clone() else {
        return None;
    };
    obj.insert("role".to_string(), serde_json::Value::String(kind.to_string()));
    if let (None, Some(ts)) = (obj.get("timestamp"), timestamp) {
        obj.insert("timestamp".to_string(), serde_json::Value::from(ts));
    }
    Some(serde_json::Value::Object(obj))
}

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
    /// The session's weak self-handle, shared with the persist+fan-out subscriber and the post-run
    /// driver. The hooks are built BEFORE the session that owns them exists, so this is empty until
    /// `AgentSession::into_shared` binds it — see [`Self::prepare_next_turn`].
    session: Arc<crate::session::SessionHandle>,
}

impl PolicyHooks {
    pub(crate) fn new(
        policy: PermissionPolicy,
        inner: Arc<dyn Hooks>,
        has_ui: bool,
        block_images: bool,
        session: Arc<crate::session::SessionHandle>,
    ) -> Self {
        Self { policy, inner, has_ui, block_images, session }
    }
}

/// Replace every [`cyrup_core::Content::Image`] with the placeholder text, deduping consecutive placeholders
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
        // SESS-043 — pi's `convertToLlmWithBlockImages` is `blockImages(convertToLlm(messages))`
        // (`coding-agent/src/core/sdk.ts:256-289` @v0.83.0) over the CODING-AGENT `convertToLlm`,
        // not the agent package's base one. This previously delegated to `inner` (the extension
        // seam), which does not override `convert_to_llm` and so resolved to
        // `cyrup_agent::default_convert_to_llm` — pi's BASE function. Upstream has no extension
        // seam on `convertToLlm` at all (extensions hook `transformContext`, which still runs
        // ahead of this), so the delegation was inventing a seam AND losing the merged roles.
        let converted = coding_agent_convert_to_llm(msgs);
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
                    usage,
                    added_tool_names,
                    timestamp,
                } if content.iter().any(is_image) => Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content: filter_images(&content),
                    is_error,
                    details,
                    // Image stripping must not disturb anything else on the message; carrying
                    // `added_tool_names` through keeps the deferred-tool anchor intact.
                    usage,
                    added_tool_names,
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
            // AGENT-022 `terminate: false` — a POLICY block is not pi's "stop after this batch"
            // hint; that flag belongs to an extension's `BeforeToolCallResult.terminate`
            // (`packages/agent/src/types.ts:61-69` @v0.84.1) and the permission gate never sets it.
            PolicyDecision::Block { reason } => {
                return Ok(BeforeOutcome::Block { reason: Some(reason), terminate: false });
            }
            PolicyDecision::Confirm { reason } => {
                if !self.has_ui {
                    // No UI to prompt: block-by-default (R-12-009).
                    return Ok(BeforeOutcome::Block { reason: Some(reason), terminate: false });
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

    /// Pi `_installAgentNextTurnRefresh` (agent-session.ts:519-540): run whatever
    /// `prepareNextTurnWithContext` was already installed, then OVERWRITE the tool set with the
    /// session's live tool set, model and thinking level — every turn, unconditionally.
    ///
    /// Ordering matches Pi: `previousSnapshot` is awaited first and its fields are spread, then
    /// `context.tools` (`:534`), `model` (`:537`) and `thinkingLevel` (`:538`) are assigned over the
    /// top. So an extension may still replace the transcript; it may not out-vote the session on
    /// which tools exist, which model runs, or which reasoning tier is in force. The previous
    /// revision of this comment claimed an extension's `model`/`thinkingLevel` survived and that
    /// this "matches Pi exactly" — the first half described cyrup's own behaviour accurately, the
    /// second half was false, because pi stamps the session's values OVER the extension's
    /// (AGENT-017).
    ///
    /// Pi also re-pushes `context.systemPrompt` here — `systemPrompt: this._systemPromptOverride ??
    /// this._baseSystemPrompt` (agent-session.ts:534 @v0.83.0) — and so does cyrup now that the
    /// session models both of pi's slots (DRIFT-033). Re-pushing the RESOLVED value is what makes a
    /// mid-run tool addition describable to the model in the same run, without undoing a
    /// `before_agent_start` handler's sanitization: the override slot is exactly what survives the
    /// rebuild.
    ///
    /// This is the seam that makes a MID-RUN tool addition real. Without it the loop runs the whole
    /// prompt on the tool array it snapshotted at run start, so a `ToolResult::added_tool_names`
    /// anchor (DRIFT-001) named a tool the model could not call until the next prompt, and EXT-004's
    /// late registration only landed after `handle.finished()`.
    ///
    /// On an UNBOUND session (a by-value `AgentSession` that never went through `into_shared`) there
    /// is no self-handle to upgrade, so this degrades to the plain delegate — the same graceful
    /// no-op the post-run driver and the inject sink take on an unbound session.
    async fn prepare_next_turn(
        &self,
        ctx: PostTurn<'_>,
        cancel: CancelToken,
    ) -> Result<Option<TurnUpdate>, HookError> {
        let previous = self.inner.prepare_next_turn(ctx, cancel).await?;
        let Some(session) = self.session.get() else {
            return Ok(previous);
        };
        let mut update = previous.unwrap_or_default();
        update.tools = Some(session.next_turn_tools().await);
        // DRIFT-033 — pi's refresh assigns `context.systemPrompt` in the SAME object literal as
        // `context.tools` (agent-session.ts:534 vs `:535` @v0.83.0), so the prompt the model is sent
        // always describes the tool array it is sent with. Read AFTER `next_turn_tools`, because the
        // EXT-004 refresh that call performs is what rewrites the base slot for a late tool.
        update.system_prompt = Some(session.effective_system_prompt());
        // AGENT-017 — pi's refresh returns THREE session-owned fields after the spread, not one:
        // `context.tools` (agent-session.ts:534 @v0.83.0), `model` (`:537`) and `thinkingLevel`
        // (`:538`). Only `tools` was re-pushed here, so `TurnUpdate::model` /
        // `TurnUpdate::thinking_level` — which the loop folds stickily at `agent.rs:582-587` —
        // never carried a value and a mid-run `/model` or thinking-level change did not reach the
        // running loop until the next prompt, while the session still persisted the change and
        // emitted its events. Stamped AFTER the inner hook for the same reason pi puts them after
        // the spread: the session out-votes an extension override on all three.
        let (model, thinking_level) = session.next_turn_model_baseline().await;
        update.model = Some(model);
        update.thinking_level = Some(thinking_level);
        Ok(Some(update))
    }

    async fn should_stop_after_turn(
        &self,
        ctx: PostTurn<'_>,
        cancel: CancelToken,
    ) -> Result<bool, HookError> {
        self.inner.should_stop_after_turn(ctx, cancel).await
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

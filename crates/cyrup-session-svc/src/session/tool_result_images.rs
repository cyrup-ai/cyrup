//! Tool-result image normalization — Pi `normalizeToolResultImages`
//! (`packages/coding-agent/src/utils/tool-result-images.ts` @v1.0.4), called from
//! `AgentSession._afterToolCall` (`core/agent-session.ts:693-698`).
//!
//! # The gap this closes
//!
//! `inputLimits.images.resize` has THREE consumers upstream, not two. The `read` tool reads its own
//! (`core/tools/read.ts:138`) and the prompt path reads one in `_normalizePromptImages`
//! ([`super::prompt_images`]) — and every OTHER tool that hands back an image block gets one here.
//! Upstream's own doc comment states the reason:
//!
//! > The `read` tool and `@file` CLI attachments run their images through `processImage`, but tools
//! > that produce images themselves (extensions, MCP bridges, screenshot tools) hand back arbitrary
//! > base64 payloads that go straight into session history and every subsequent provider request.
//! > Oversized images make the provider reject the whole conversation, not just the offending turn,
//! > so normalize them once as they enter history.
//!
//! "Not just the offending turn" is the part that makes this worse than the prompt case it mirrors:
//! a tool result is PERSISTED, so an MCP screenshot tool's 6000px PNG is re-sent at 6000px on every
//! later request in the session.
//!
//! # The failure arm is the OPPOSITE of the prompt path's
//!
//! A prompt image that cannot be processed is DROPPED and replaced by a hint, because the user can
//! see the hint and re-attach. A tool-result image that cannot be processed is KEPT VERBATIM
//! (upstream `tool-result-images.ts`: *"the tool already produced this image and the failure may
//! just be an unavailable image backend, so passing it through preserves the behavior tools have
//! today instead of silently deleting their output"*). Deleting a tool's output would make an
//! unrelated backend failure look like the tool returning nothing.
//!
//! # Unchanged means UNCHANGED
//!
//! Upstream returns the original array identically (`return changed ? normalized : content`) so the
//! caller can skip rewriting the result — and in cyrup that identity is load-bearing in a way it is
//! not upstream: [`cyrup_agent::AfterOutcome::Keep`] versus an
//! [`cyrup_agent::AfterOverride`] is the difference between the tool's own result standing and a
//! replace-not-merge fold running over it. So this returns `Option`: `None` IS pi's
//! `normalizedContent === content`.

use cyrup_core::Content;

use super::prompt_images::decode_wire_base64;
use crate::AgentSession;

impl AgentSession {
    /// Normalize the image blocks of one tool result, or `None` when nothing moved.
    ///
    /// The profile and the setting are resolved exactly as the prompt path resolves them — pi reads
    /// `this._limitsModel()?.inputLimits?.images?.resize` and
    /// `this.settingsManager.getImageAutoResize()` in BOTH places
    /// (`agent-session.ts:694-696` and `:1931-1932`) — so a tool-produced image and a prompt image
    /// in the same turn get the same clamp.
    pub(crate) async fn normalize_tool_result_images(
        &self,
        content: &[Content],
    ) -> Option<Vec<Content>> {
        // Pi's `if (!content.some((block) => block.type === "image")) return content` — the early
        // exit that keeps the overwhelming majority of tool results (all-text) off `limits_model()`,
        // which takes a transcript snapshot, and off `spawn_blocking` entirely.
        if !content.iter().any(|c| matches!(c, Content::Image { .. })) {
            return None;
        }

        let profile = self
            .limits_model()
            .await
            .and_then(|m| m.image_resize_profile());
        let auto_resize_images = self.services.settings.effective().image_auto_resize();

        let owned = content.to_vec();
        // CPU-bound, same as the prompt path and the `read` tool: the decode + EXIF bake + clamp +
        // JPEG ladder must not run inline on the async worker.
        // `unwrap_or_default()` rather than a match arm, at clippy's insistence: a panic in the
        // resizer settles as `None`, i.e. keep the tool's own content — which is this path's
        // failure arm for every other reason too, so the default IS the behaviour wanted here.
        tokio::task::spawn_blocking(move || {
            normalize_blocking(owned, auto_resize_images, profile.as_ref())
        })
        .await
        .unwrap_or_default()
    }
}

/// The blocking half — pi's per-block loop, `tool-result-images.ts:33-64`.
fn normalize_blocking(
    content: Vec<Content>,
    auto_resize_images: bool,
    profile: Option<&cyrup_core::ModelImageResizeOptions>,
) -> Option<Vec<Content>> {
    use cyrup_tools::image_proc::{ProcessImageOptions, Processed, process_image};

    let mut out: Vec<Content> = Vec::with_capacity(content.len());
    let mut changed = false;
    for block in content {
        let Content::Image { data, mime_type } = &block else {
            out.push(block);
            continue;
        };
        let raw = decode_wire_base64(data);
        match process_image(
            &raw,
            mime_type,
            ProcessImageOptions {
                auto_resize_images,
                resize: profile,
            },
        ) {
            // `if (!processed.ok) { normalized.push(block); continue; }` (`:45-51`) — KEEP the
            // original, unlike the prompt path, and emit no hint.
            Processed::Failed { .. } => out.push(block),
            Processed::Ok {
                data: new,
                mime,
                hints,
            } => {
                // `if (processed.data === block.data && processed.mimeType === block.mimeType &&
                //     processed.hints.length === 0)` (`:53-56`): an image already inside the
                // profile re-emits its ORIGINAL bytes, so this is the common no-op.
                if new == *data && mime == *mime_type && hints.is_empty() {
                    out.push(block);
                    continue;
                }
                out.push(Content::Image {
                    data: new,
                    mime_type: mime,
                });
                // The hints ride a text block appended AFTER the image (`:58-61`), not folded into
                // a pre-existing one: a tool result has no `expandedText` to fold them into, and the
                // dimension note has to sit beside the image it describes.
                if !hints.is_empty() {
                    out.push(Content::text(hints.join("\n")));
                }
                changed = true;
            }
        }
    }
    changed.then_some(out)
}

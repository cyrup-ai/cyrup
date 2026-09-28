//! Request encoding — the reasoning lowering onto Mistral's `promptMode` / `reasoningEffort`
//! pair (Pi `streamSimple` + `usesPromptModeReasoning` / `usesReasoningEffort` /
//! `mapReasoningEffort`, mistral-conversations.ts:120-130,621-634).

use crate::collection::clamp_thinking_level;
use crate::model::Model;
use cyrup_core::ModelThinkingLevel;

/// Lower the unified reasoning level to Mistral's `promptMode`/`reasoningEffort` pair (Pi
/// `streamSimple` + `usesPromptModeReasoning`/`usesReasoningEffort`/`mapReasoningEffort`,
/// mistral-conversations.ts:120-130,621-634).
pub(super) fn lower_reasoning(
    model: &Model,
    reasoning: ModelThinkingLevel,
) -> (Option<&'static str>, Option<String>) {
    if !reasoning.is_on() {
        return (None, None);
    }
    let clamped = clamp_thinking_level(model, reasoning);
    if !clamped.is_on() {
        return (None, None);
    }
    let should_use = model.reasoning;
    if !should_use {
        return (None, None);
    }

    if uses_reasoning_effort(model) {
        (None, Some(map_reasoning_effort(model, clamped)))
    } else if uses_prompt_mode_reasoning(model) {
        (Some("reasoning"), None)
    } else {
        (None, None)
    }
}

/// `model.id` ∈ the explicit reasoning-effort set (Pi `usesReasoningEffort`,
/// mistral-conversations.ts:898-905 @v0.87.1). The whole `mistral-medium-` family matches by
/// prefix, so no exact `mistral-medium-*` arm is needed.
fn uses_reasoning_effort(model: &Model) -> bool {
    matches!(
        model.id.as_str(),
        "mistral-small-2603" | "mistral-small-latest" | "zai-glm-5-2"
    ) || model.id.as_str().starts_with("mistral-medium-")
}

/// `model.reasoning && !usesReasoningEffort` (Pi `usesPromptModeReasoning`,
/// mistral-conversations.ts:907-908 @v0.87.1).
fn uses_prompt_mode_reasoning(model: &Model) -> bool {
    model.reasoning && !uses_reasoning_effort(model)
}

/// `model.thinkingLevelMap?.[level] ?? "high"` (Pi `mapReasoningEffort`,
/// mistral-conversations.ts:629-634). The result is a Mistral `reasoningEffort` (`"none"`/`"high"`).
fn map_reasoning_effort(model: &Model, level: ModelThinkingLevel) -> String {
    let key = crate::api::compat::thinking_level_key(level);
    if let Some(Some(mapped)) = model.thinking_level_map.as_ref().and_then(|m| m.get(key)) {
        return mapped.clone();
    }
    "high".to_string()
}

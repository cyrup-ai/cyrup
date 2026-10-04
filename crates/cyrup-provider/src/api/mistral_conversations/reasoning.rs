//! Request encoding — the reasoning lowering onto Mistral's `promptMode` / `reasoningEffort`
//! pair (Pi `streamSimple`, mistral-conversations.ts:199-215 @v1.0.0).

use crate::api::compat::{level_map_lookup, mapped_effort_or};
use crate::collection::clamp_thinking_level;
use crate::model::Model;
use cyrup_core::ModelThinkingLevel;

/// Lower the unified reasoning level to Mistral's `promptMode`/`reasoningEffort` pair (Pi
/// `streamSimple`, mistral-conversations.ts:199-215 @v1.0.0).
///
/// `dc84c1ac0` ("send requested thinking level to Mistral reasoning models") deleted
/// `usesReasoningEffort` / `usesPromptModeReasoning` / `mapReasoningEffort` and replaced the
/// hardcoded model-id set with a **presence test on the catalog's `thinkingLevelMap`**:
///
/// ```ts
/// // Models with a thinking level map use `reasoning_effort`; other reasoning models use `prompt_mode`.
/// const effortMap = model.reasoning ? model.thinkingLevelMap : undefined;
/// const reasoningEffort = effortMap ? (reasoning ? (effortMap[reasoning] ?? "high") : (effortMap.off ?? undefined)) : undefined;
/// …
/// promptMode: model.reasoning && !effortMap && reasoning ? "reasoning" : undefined,
/// ```
///
/// Two consequences the id set got wrong, both reachable from cyrup's own
/// `providers/catalog/mistral.json`: `zai-glm-5-3` carries a map and so takes `reasoning_effort`
/// (the id set sent it `prompt_mode`, which pi's commit message says is "ignored or unsupported"
/// on these models), and with reasoning **off** a mapped model still sends
/// `reasoning_effort: effortMap.off` rather than nothing. Note the asymmetry in the `??` chain:
/// an on-level falls back to `"high"`, the `off` key does not fall back at all.
pub(super) fn lower_reasoning(
    model: &Model,
    reasoning: ModelThinkingLevel,
) -> (Option<&'static str>, Option<String>) {
    // `clampedReasoning = options?.reasoning ? clampThinkingLevel(…) : undefined`, then
    // `reasoning = clampedReasoning === "off" ? undefined : clampedReasoning` (`:200-201`).
    let effective = if reasoning.is_on() {
        let clamped = clamp_thinking_level(model, reasoning);
        clamped.is_on().then_some(clamped)
    } else {
        None
    };

    // `model.reasoning ? model.thinkingLevelMap : undefined` (`:203`).
    let effort_map = if model.reasoning {
        model.thinking_level_map.as_ref()
    } else {
        None
    };

    let reasoning_effort = match effort_map {
        // `effortMap[reasoning] ?? "high"` for an on-level; `effortMap.off ?? undefined` for off —
        // a missing/null `off` key yields nothing, with no `"high"` fallback (`:204-208`).
        Some(map) => match effective {
            Some(level) => Some(mapped_effort_or(Some(map), level, "high")),
            None => level_map_lookup(Some(map), "off").cloned().flatten(),
        },
        None => None,
    };

    // `model.reasoning && !effortMap && reasoning ? "reasoning" : undefined` (`:212`).
    let prompt_mode =
        (model.reasoning && effort_map.is_none() && effective.is_some()).then_some("reasoning");

    (prompt_mode, reasoning_effort)
}

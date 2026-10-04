//! The reasoning lowering and the direct `MistralOptions` overrides.

use super::*;

/// A row from cyrup's own shipped `providers/catalog/mistral.json`, so the lowering is exercised
/// against the `thinkingLevelMap` data pi v1.0.0's presence test actually reads rather than a
/// hand-written stand-in.
fn catalog_model(id: &str) -> Model {
    crate::providers::mistral::mistral_models()
        .into_iter()
        .find(|m| m.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} missing from providers/catalog/mistral.json"))
}

/// PROV-114 — pi v1.0.0 picks `reasoning_effort` vs `prompt_mode` from the **presence of a
/// `thinkingLevelMap`**, not from a hardcoded id set (`dc84c1ac0`, mistral-conversations.ts:203-212):
/// `const effortMap = model.reasoning ? model.thinkingLevelMap : undefined`.
///
/// `zai-glm-5-3` is the row that proves the id set was wrong: it ships in cyrup's own catalog with
/// `reasoning: true` and a map of `{low, high, max}`, and it is **not** in the deleted
/// `usesReasoningEffort` set — so the old code sent it `prompt_mode: "reasoning"`, the field pi's
/// commit message says is "ignored or unsupported" on these models.
#[test]
fn a_mapped_model_outside_the_old_id_set_uses_reasoning_effort() {
    let m = catalog_model("zai-glm-5-3");
    assert!(m.thinking_level_map.is_some(), "catalog row carries a map");
    for (level, expected) in [
        (ModelThinkingLevel::Low, "low"),
        (ModelThinkingLevel::High, "high"),
        (ModelThinkingLevel::Max, "max"),
    ] {
        let opts = StreamOptions {
            reasoning: level,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["reasoningEffort"], expected, "{level:?}");
        assert!(body.get("promptMode").is_none(), "{level:?}");
    }
}

/// PROV-114 — with reasoning **off**, a mapped model now sends `effortMap.off` rather than nothing
/// (`reasoningEffort = effortMap ? (reasoning ? … : (effortMap.off ?? undefined)) : undefined`,
/// mistral-conversations.ts:204-208). Note the asymmetry: the `off` branch has no `?? "high"`
/// fallback, so a map whose `off` key is absent or `null` still sends nothing.
#[test]
fn reasoning_off_sends_the_maps_off_value_and_nothing_without_one() {
    // `mistral-medium-latest` maps `off -> "none"`: pi sends it, the pinned-id code sent nothing.
    let m = catalog_model("mistral-medium-latest");
    let body = build_body(&m, &user_ctx("x"), &Default::default());
    assert_eq!(body["reasoningEffort"], "none");
    assert!(body.get("promptMode").is_none());

    // `zai-glm-5-3` maps `off -> null`, which is `?? undefined` with no fallback: nothing at all.
    let m = catalog_model("zai-glm-5-3");
    let body = build_body(&m, &user_ctx("x"), &Default::default());
    assert!(body.get("reasoningEffort").is_none());
    assert!(body.get("promptMode").is_none());

    // A reasoning model with no map has no `off` value to send either.
    let m = catalog_model("magistral-medium-latest");
    assert!(m.thinking_level_map.is_none());
    let body = build_body(&m, &user_ctx("x"), &Default::default());
    assert!(body.get("reasoningEffort").is_none());
    assert!(body.get("promptMode").is_none());

    // A non-reasoning model is `effortMap = undefined` by the `model.reasoning ? …` guard even if a
    // map were present, so it stays bare.
    let mut m = catalog_model("codestral-latest");
    assert!(!m.reasoning);
    m.thinking_level_map = Some(crate::model::ThinkingLevelMap::from([(
        "off".to_string(),
        Some("none".to_string()),
    )]));
    let body = build_body(&m, &user_ctx("x"), &Default::default());
    assert!(body.get("reasoningEffort").is_none());
    assert!(body.get("promptMode").is_none());
}

/// PROV-114 — `MistralReasoningEffort` widened from `"none" | "high"` to
/// `"none" | "low" | "medium" | "high" | "max"` (`dc84c1ac0`, mistral-conversations.ts:34). The
/// three added values must reach the wire verbatim through the per-request override, which is the
/// only surface a caller can set them from (`buildChatPayload` reads `options.reasoningEffort`
/// unchanged, `:257`). Red at HEAD: `MistralReasoningEffort::{Low, Medium, Max}` did not exist.
#[test]
fn the_reasoning_effort_override_spans_all_five_upstream_values() {
    let m = catalog_model("zai-glm-5-3");
    for (value, wire) in [
        (MistralReasoningEffort::None, "none"),
        (MistralReasoningEffort::Low, "low"),
        (MistralReasoningEffort::Medium, "medium"),
        (MistralReasoningEffort::High, "high"),
        (MistralReasoningEffort::Max, "max"),
    ] {
        assert_eq!(value.as_wire(), wire);
        let opts = StreamOptions {
            api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
                reasoning_effort: Some(value),
                ..Default::default()
            })),
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["reasoningEffort"], wire, "{value:?}");
    }
}

#[test]
fn reasoning_effort_models_emit_effort() {
    let m = catalog_model("mistral-small-latest");
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert_eq!(body["reasoningEffort"], "high");
    assert!(body.get("promptMode").is_none());
}

#[test]
fn prompt_mode_reasoning_for_other_reasoning_models() {
    // `model.reasoning && !effortMap && reasoning` — the catalog row carries no map.
    let m = catalog_model("magistral-medium-latest");
    assert!(m.thinking_level_map.is_none());
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::Medium,
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert_eq!(body["promptMode"], "reasoning");
    assert!(body.get("reasoningEffort").is_none());
}

/// Byte-diff vs Pi `buildChatPayload` (mistral-conversations.ts:256): a direct
/// `MistralOptions.promptMode` override is written verbatim, overriding the unified-`reasoning`
/// lowering. Proven two ways: (a) it ADDS `promptMode` on a non-reasoning request that the
/// lowering leaves bare, and (b) it overrides on a model whose lowering would otherwise emit
/// `reasoningEffort` only.
#[test]
fn mistral_prompt_mode_override_threads_to_payload() {
    // (a) Non-reasoning model + no unified reasoning ⇒ lowering yields no promptMode; the direct
    //     override supplies it. Pi: `if (options?.promptMode) payload.promptMode = options.promptMode`.
    let m = model_with("codestral-latest", false);
    let opts = StreamOptions {
        api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
            prompt_mode: Some(MistralPromptMode::Reasoning),
            ..Default::default()
        })),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert_eq!(body["promptMode"], "reasoning");
    // The lowering contributed nothing here, so reasoningEffort stays absent.
    assert!(body.get("reasoningEffort").is_none());

    // (b) A reasoning-effort model at High would lower to `reasoningEffort:"high"` with no
    //     promptMode; the override adds `promptMode:"reasoning"` on top (Pi reads both fields
    //     independently from `options`).
    let m = catalog_model("mistral-small-latest");
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
            prompt_mode: Some(MistralPromptMode::Reasoning),
            ..Default::default()
        })),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert_eq!(body["promptMode"], "reasoning");
    assert_eq!(body["reasoningEffort"], "high");

    // Control: without the override the same request omits promptMode (proving the override drove
    // the bytes above, not the lowering).
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert!(body.get("promptMode").is_none());
    assert_eq!(body["reasoningEffort"], "high");
}

/// Byte-diff vs Pi `buildChatPayload` (mistral-conversations.ts:257): a direct
/// `MistralOptions.reasoningEffort` override is written verbatim, independent of `promptMode` and
/// of the unified-`reasoning` lowering. Pi: `if (options?.reasoningEffort) payload.reasoningEffort
/// = options.reasoningEffort`. Proven three ways.
#[test]
fn mistral_reasoning_effort_override_threads_to_payload() {
    // (a) Non-reasoning model + no unified reasoning ⇒ lowering yields no reasoningEffort; the
    //     direct override supplies `"high"` with no promptMode (independent of the promptMode field).
    let m = model_with("codestral-latest", false);
    let opts = StreamOptions {
        api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
            reasoning_effort: Some(MistralReasoningEffort::High),
            ..Default::default()
        })),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert_eq!(body["reasoningEffort"], "high");
    assert!(body.get("promptMode").is_none());

    // (b) `"none"` is also written verbatim (Pi's `if (options?.reasoningEffort)` is truthy for
    //     the non-empty string `"none"`).
    let opts = StreamOptions {
        api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
            reasoning_effort: Some(MistralReasoningEffort::None),
            ..Default::default()
        })),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert_eq!(body["reasoningEffort"], "none");

    // (c) The override REPLACES the value the lowering would otherwise compute. A reasoning-effort
    //     model at High lowers to `reasoningEffort:"high"`; overriding with `"none"` wins.
    let m = catalog_model("mistral-small-latest");
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        api_options: Some(crate::stream::ApiStreamOptions::Mistral(MistralOptions {
            reasoning_effort: Some(MistralReasoningEffort::None),
            ..Default::default()
        })),
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert_eq!(body["reasoningEffort"], "none");
    // Control: without the override the same request lowers to "high" (proving the override drove
    // the bytes above).
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("x"), &opts);
    assert_eq!(body["reasoningEffort"], "high");
}

/// `PROV-088`'s rows still take `reasoningEffort` rather than `promptMode` under `PROV-114`'s new
/// rule, but now *because they carry a `thinkingLevelMap`* rather than because their ids were
/// listed (`effortMap = model.reasoning ? model.thinkingLevelMap : undefined`,
/// mistral-conversations.ts:203). `mistral-medium-3.6` is not a catalog row, so it stands in for the
/// deleted `mistral-medium-` prefix arm: under the new rule a mapless id matching that prefix takes
/// `promptMode`, so it is checked here with the catalog's own map attached.
#[test]
fn mapped_models_use_reasoning_effort_not_prompt_mode() {
    let map = catalog_model("mistral-medium-latest").thinking_level_map;
    for id in ["mistral-medium-3.6", "mistral-medium-latest", "zai-glm-5-2"] {
        let mut m = catalog_model_or_mapped(id, map.clone());
        m.reasoning = true;
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_body(&m, &user_ctx("x"), &opts);
        assert_eq!(body["reasoningEffort"], "high", "{id}");
        assert!(body.get("promptMode").is_none(), "{id}");
    }
}

/// `mistral-medium-3.6` is not in cyrup's shipped catalog; give it the family's map so the row
/// above exercises the same shape the live catalog would deliver.
fn catalog_model_or_mapped(id: &str, map: Option<crate::model::ThinkingLevelMap>) -> Model {
    crate::providers::mistral::mistral_models()
        .into_iter()
        .find(|m| m.id.as_str() == id)
        .unwrap_or_else(|| {
            let mut m = model_with(id, true);
            m.thinking_level_map = map;
            m
        })
}

//! The reasoning lowering and the direct `MistralOptions` overrides.

use super::*;

#[test]
fn reasoning_effort_models_emit_effort() {
    let m = model_with("mistral-small-latest", true);
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
    let m = model_with("magistral-medium-latest", true);
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
    let m = model_with("mistral-small-latest", true);
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
    let m = model_with("mistral-small-latest", true);
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

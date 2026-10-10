//! Model-capability predicates (pi `getModelMatchCandidates` and the `supports*` helpers,
//! `bedrock-converse-stream.ts:758-912` @f1b2e77f5).

use super::config::configured_bedrock_region;
use super::options::BedrockOptions;
use crate::model::Model;
use crate::utils::provider_plumbing::EnvSource;
use cyrup_core::ThinkingLevel;

/// pi `getModelMatchCandidates` (`bedrock-converse-stream.ts:758-764` @f1b2e77f5): for the model id and (when
/// present) the model name, the lower-cased value plus the value with every run of `[\s_.:]`
/// collapsed to a single `-`.
pub(super) fn model_match_candidates(model: &Model) -> Vec<String> {
    let mut values = vec![model.id.as_str().to_lowercase()];
    if !model.name.is_empty() {
        values.push(model.name.to_lowercase());
    }
    let mut out = Vec::with_capacity(values.len() * 2);
    for value in values {
        let mut dashed = String::with_capacity(value.len());
        let mut in_run = false;
        for ch in value.chars() {
            if ch.is_whitespace() || ch == '_' || ch == '.' || ch == ':' {
                if !in_run {
                    dashed.push('-');
                    in_run = true;
                }
            } else {
                dashed.push(ch);
                in_run = false;
            }
        }
        out.push(value);
        out.push(dashed);
    }
    out
}

/// pi `supportsAdaptiveThinking` (`bedrock-converse-stream.ts:766-779` @f1b2e77f5). `haiku-5`
/// arrived with Claude Haiku 5.5 (pi `f76c1db66`, v1.1.0; PROV-139).
pub(super) fn supports_adaptive_thinking(model: &Model) -> bool {
    const NEEDLES: [&str; 8] = [
        "opus-4-6",
        "opus-4-7",
        "opus-4-8",
        "opus-5",
        "sonnet-4-6",
        "sonnet-5",
        "haiku-5",
        "fable-5",
    ];
    let candidates = model_match_candidates(model);
    candidates
        .iter()
        .any(|s| NEEDLES.iter().any(|n| s.contains(n)))
}

/// pi `supportsThinkingBlockBinding` (`bedrock-converse-stream.ts:794-809` @f1b2e77f5): the models
/// that accept `thinking.block_binding`. Opus 4.6 and Sonnet 4.6 support adaptive thinking but
/// REJECT the field with "thinking.adaptive.block_binding: Extra inputs are not permitted", which
/// is why this needle list is narrower than [`supports_adaptive_thinking`]'s (PROV-130).
pub(super) fn supports_thinking_block_binding(model: &Model) -> bool {
    const NEEDLES: [&str; 6] = [
        "opus-4-7", "opus-4-8", "opus-5", "sonnet-5", "haiku-5", "fable-5",
    ];
    let candidates = model_match_candidates(model);
    candidates
        .iter()
        .any(|s| NEEDLES.iter().any(|n| s.contains(n)))
}

/// pi `supportsNativeXhighEffort` (`bedrock-converse-stream.ts:781-792` @f1b2e77f5).
fn supports_native_xhigh_effort(model: &Model) -> bool {
    const NEEDLES: [&str; 6] = [
        "opus-4-7", "opus-4-8", "opus-5", "sonnet-5", "haiku-5", "fable-5",
    ];
    let candidates = model_match_candidates(model);
    candidates
        .iter()
        .any(|s| NEEDLES.iter().any(|n| s.contains(n)))
}

/// pi `mapThinkingLevelToEffort` (`bedrock-converse-stream.ts:811-831` @f1b2e77f5). Note the switch has no
/// `xhigh`/`max` arm, so both fall to `default: "high"` unless the model natively supports `xhigh`
/// or a `thinkingLevelMap` entry overrides.
pub(super) fn map_thinking_level_to_effort(model: &Model, level: ThinkingLevel) -> String {
    if level == ThinkingLevel::Xhigh && supports_native_xhigh_effort(model) {
        return "xhigh".to_string();
    }
    let key = match level {
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    };
    if let Some(Some(mapped)) = model.thinking_level_map.as_ref().and_then(|m| m.get(key)) {
        return mapped.clone();
    }
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => "low".to_string(),
        ThinkingLevel::Medium => "medium".to_string(),
        ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => "high".to_string(),
    }
}

/// pi `isAnthropicClaudeModel` (`bedrock-converse-stream.ts:852-862` @f1b2e77f5).
pub(super) fn is_anthropic_claude_model(model: &Model) -> bool {
    let id = model.id.as_str().to_lowercase();
    let name = model.name.to_lowercase();
    id.contains("anthropic.claude")
        || id.contains("anthropic/claude")
        || name.contains("anthropic.claude")
        || name.contains("anthropic/claude")
        || name.contains("claude")
}

/// pi `supportsPromptCaching` (`bedrock-converse-stream.ts:876-900` @f1b2e77f5).
pub(super) fn supports_prompt_caching(model: &Model, env: &EnvSource<'_>) -> bool {
    let candidates = model_match_candidates(model);
    let has_claude_ref = candidates.iter().any(|s| s.contains("claude"));
    if !has_claude_ref {
        return env.get("AWS_BEDROCK_FORCE_CACHE").as_deref() == Some("1");
    }
    let any = |needles: &[&str]| {
        candidates
            .iter()
            .any(|s| needles.iter().any(|n| s.contains(n)))
    };
    // Claude 5, then Claude 4.x, then Claude 3.7 Sonnet, then Claude 3.5 Haiku.
    any(&["fable-5", "opus-5", "sonnet-5", "haiku-5"])
        || any(&["-4-"])
        || any(&["claude-3-7-sonnet"])
        || any(&["claude-3-5-haiku"])
}

/// pi `supportsThinkingSignature` (`bedrock-converse-stream.ts:910-912` @f1b2e77f5): only Anthropic Claude
/// models accept `reasoningContent.reasoningText.signature`.
pub(super) fn supports_thinking_signature(model: &Model) -> bool {
    is_anthropic_claude_model(model)
}

/// pi `isGovCloudBedrockTarget` (`bedrock-converse-stream.ts:1252-1260` @f1b2e77f5).
pub(super) fn is_gov_cloud_bedrock_target(
    model: &Model,
    bedrock: &BedrockOptions,
    env: &EnvSource<'_>,
) -> bool {
    if let Some(region) = configured_bedrock_region(bedrock, env)
        && region.to_lowercase().starts_with("us-gov-")
    {
        return true;
    }
    let id = model.id.as_str().to_lowercase();
    id.starts_with("us-gov.") || id.starts_with("arn:aws-us-gov:")
}

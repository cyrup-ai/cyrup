//! OpenAI-completions compatibility matrix (1:1 port of Pi's `OpenAICompletionsCompat`,
//! `detectCompat`, and `getCompat` from `packages/ai/src/api/openai-completions.ts` +
//! `packages/ai/src/types.ts`).
//!
//! OpenAI-"compatible" providers diverge in dozens of small ways (which max-tokens field,
//! how reasoning is encoded, whether `store`/`strict`/`developer` role are accepted, prompt
//! caching conventions, …). Pi captures these as a per-provider/per-model flag matrix that is
//! auto-detected from `provider` + `baseUrl` and then overridden by an explicit `model.compat`.
//! This module reproduces that matrix verbatim (it is data, ported faithfully).

use crate::model::{Model, ThinkingLevelMap};
use cyrup_core::ModelThinkingLevel;
use serde_json::{Map, Value};

/// Which request field carries the max-tokens cap.
/// Pi: `maxTokensField?: "max_completion_tokens" | "max_tokens"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxTokensField {
    MaxCompletionTokens,
    MaxTokens,
}

/// How reasoning/thinking is encoded in the request body.
/// Pi: `thinkingFormat?: "openai" | "openrouter" | ... | "ant-ling"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingFormat {
    Openai,
    Openrouter,
    Deepseek,
    Together,
    Zai,
    Qwen,
    ChatTemplate,
    QwenChatTemplate,
    StringThinking,
    AntLing,
}

/// Prompt-cache marker convention.
/// Pi: `cacheControlFormat?: "anthropic"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheControlFormat {
    Anthropic,
}

/// Vercel AI Gateway routing preferences (Pi `VercelGatewayRouting`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VercelGatewayRouting {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub only: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub order: Option<Vec<String>>,
}

/// Per-model compatibility overrides (Pi `OpenAICompletionsCompat`). Every field is optional; a
/// `None` field falls back to the URL/provider auto-detected value (see [`get_compat`]).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiCompletionsCompat {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_store: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_developer_role: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_reasoning_effort: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_usage_in_streaming: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_tokens_field: Option<MaxTokensField>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub requires_tool_result_name: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub requires_assistant_after_tool_result: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub requires_thinking_as_text: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub requires_reasoning_content_on_assistant_messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thinking_format: Option<ThinkingFormat>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub chat_template_kwargs: Option<Map<String, Value>>,
    /// OpenRouter routing, passed through verbatim as the `provider` request field.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub open_router_routing: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub vercel_gateway_routing: Option<VercelGatewayRouting>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub zai_tool_stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_strict_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_control_format: Option<CacheControlFormat>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub send_session_affinity_headers: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_long_cache_retention: Option<bool>,
}

/// Fully-resolved compatibility settings (Pi `ResolvedOpenAICompletionsCompat`). Routing fields are
/// read straight off `model.compat` at build time (matching Pi's `buildParams`), so they are not
/// duplicated here.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedCompat {
    pub supports_store: bool,
    pub supports_developer_role: bool,
    pub supports_reasoning_effort: bool,
    pub supports_usage_in_streaming: bool,
    pub max_tokens_field: MaxTokensField,
    pub requires_tool_result_name: bool,
    pub requires_assistant_after_tool_result: bool,
    pub requires_thinking_as_text: bool,
    pub requires_reasoning_content_on_assistant_messages: bool,
    pub thinking_format: ThinkingFormat,
    pub chat_template_kwargs: Map<String, Value>,
    pub zai_tool_stream: bool,
    pub supports_strict_mode: bool,
    pub cache_control_format: Option<CacheControlFormat>,
    pub send_session_affinity_headers: bool,
    pub supports_long_cache_retention: bool,
}

/// The `thinkingLevelMap` key for a [`ModelThinkingLevel`] (Pi `ModelThinkingLevel`).
pub fn thinking_level_key(level: ModelThinkingLevel) -> &'static str {
    match level {
        ModelThinkingLevel::Off => "off",
        ModelThinkingLevel::Minimal => "minimal",
        ModelThinkingLevel::Low => "low",
        ModelThinkingLevel::Medium => "medium",
        ModelThinkingLevel::High => "high",
        ModelThinkingLevel::Xhigh => "xhigh",
    }
}

/// Look up `map?.[key]` returning the three-state JS result: `None` = key absent (`undefined`),
/// `Some(None)` = key present and `null`, `Some(Some(s))` = key present with a string.
pub fn level_map_lookup<'a>(
    map: Option<&'a ThinkingLevelMap>,
    key: &str,
) -> Option<&'a Option<String>> {
    map.and_then(|m| m.get(key))
}

/// `model.thinkingLevelMap?.[level] ?? <fallback>` — both `undefined` and `null` fall back.
pub fn mapped_effort_or(map: Option<&ThinkingLevelMap>, level: ModelThinkingLevel, fallback: &str) -> String {
    match level_map_lookup(map, thinking_level_key(level)) {
        Some(Some(s)) => s.clone(),
        _ => fallback.to_string(),
    }
}

/// `model.thinkingLevelMap?.off !== null` (a missing key is `undefined`, which `!== null`).
pub fn off_is_not_null(map: Option<&ThinkingLevelMap>) -> bool {
    !matches!(level_map_lookup(map, "off"), Some(None))
}

/// `model.thinkingLevelMap?.off ?? <fallback>`.
pub fn off_value_or(map: Option<&ThinkingLevelMap>, fallback: &str) -> String {
    match level_map_lookup(map, "off") {
        Some(Some(s)) => s.clone(),
        _ => fallback.to_string(),
    }
}

/// Auto-detect compatibility from `provider` + `baseUrl` + model id.
/// 1:1 port of Pi `detectCompat` (openai-completions.ts L1173-1254).
pub fn detect_compat(model: &Model) -> ResolvedCompat {
    let provider = model.provider.as_str();
    let base_url = model.base_url.as_deref().unwrap_or("");
    let id = model.id.as_str();

    let is_zai = provider == "zai"
        || provider == "zai-coding-cn"
        || base_url.contains("api.z.ai")
        || base_url.contains("open.bigmodel.cn");
    let is_together = provider == "together"
        || base_url.contains("api.together.ai")
        || base_url.contains("api.together.xyz");
    let is_moonshot =
        provider == "moonshotai" || provider == "moonshotai-cn" || base_url.contains("api.moonshot.");
    let is_openrouter = provider == "openrouter" || base_url.contains("openrouter.ai");
    let is_cloudflare_workers_ai =
        provider == "cloudflare-workers-ai" || base_url.contains("api.cloudflare.com");
    let is_cloudflare_ai_gateway =
        provider == "cloudflare-ai-gateway" || base_url.contains("gateway.ai.cloudflare.com");
    let is_nvidia = provider == "nvidia" || base_url.contains("integrate.api.nvidia.com");
    let is_ant_ling = provider == "ant-ling" || base_url.contains("api.ant-ling.com");

    let is_non_standard = is_nvidia
        || provider == "cerebras"
        || base_url.contains("cerebras.ai")
        || provider == "xai"
        || base_url.contains("api.x.ai")
        || is_together
        || base_url.contains("chutes.ai")
        || base_url.contains("deepseek.com")
        || is_zai
        || is_moonshot
        || provider == "opencode"
        || base_url.contains("opencode.ai")
        || is_cloudflare_workers_ai
        || is_cloudflare_ai_gateway
        || is_ant_ling;

    let use_max_tokens = base_url.contains("chutes.ai")
        || is_moonshot
        || is_cloudflare_ai_gateway
        || is_together
        || is_nvidia
        || is_ant_ling;

    let is_grok = provider == "xai" || base_url.contains("api.x.ai");
    let is_deepseek = provider == "deepseek" || base_url.contains("deepseek.com");
    let is_openrouter_developer_role_model =
        is_openrouter && (id.starts_with("anthropic/") || id.starts_with("openai/"));
    let cache_control_format = if provider == "openrouter" && id.starts_with("anthropic/") {
        Some(CacheControlFormat::Anthropic)
    } else {
        None
    };

    let thinking_format = if is_deepseek {
        ThinkingFormat::Deepseek
    } else if is_zai {
        ThinkingFormat::Zai
    } else if is_together {
        ThinkingFormat::Together
    } else if is_ant_ling {
        ThinkingFormat::AntLing
    } else if is_openrouter {
        ThinkingFormat::Openrouter
    } else {
        ThinkingFormat::Openai
    };

    ResolvedCompat {
        supports_store: !is_non_standard,
        supports_developer_role: is_openrouter_developer_role_model
            || (!is_non_standard && !is_openrouter),
        supports_reasoning_effort: !is_grok
            && !is_zai
            && !is_moonshot
            && !is_together
            && !is_cloudflare_ai_gateway
            && !is_nvidia
            && !is_ant_ling,
        supports_usage_in_streaming: true,
        max_tokens_field: if use_max_tokens {
            MaxTokensField::MaxTokens
        } else {
            MaxTokensField::MaxCompletionTokens
        },
        requires_tool_result_name: false,
        requires_assistant_after_tool_result: false,
        requires_thinking_as_text: false,
        requires_reasoning_content_on_assistant_messages: is_deepseek,
        thinking_format,
        chat_template_kwargs: Map::new(),
        zai_tool_stream: false,
        supports_strict_mode: !is_moonshot
            && !is_together
            && !is_cloudflare_ai_gateway
            && !is_nvidia,
        cache_control_format,
        send_session_affinity_headers: false,
        supports_long_cache_retention: !(is_together
            || is_cloudflare_workers_ai
            || is_cloudflare_ai_gateway
            || is_nvidia
            || is_ant_ling),
    }
}

/// Resolve compatibility for a model: auto-detect, then apply explicit `model.compat` overrides.
/// 1:1 port of Pi `getCompat` (openai-completions.ts L1260-1287).
pub fn get_compat(model: &Model) -> ResolvedCompat {
    let detected = detect_compat(model);
    let Some(c) = &model.compat else {
        return detected;
    };
    ResolvedCompat {
        supports_store: c.supports_store.unwrap_or(detected.supports_store),
        supports_developer_role: c
            .supports_developer_role
            .unwrap_or(detected.supports_developer_role),
        supports_reasoning_effort: c
            .supports_reasoning_effort
            .unwrap_or(detected.supports_reasoning_effort),
        supports_usage_in_streaming: c
            .supports_usage_in_streaming
            .unwrap_or(detected.supports_usage_in_streaming),
        max_tokens_field: c.max_tokens_field.unwrap_or(detected.max_tokens_field),
        requires_tool_result_name: c
            .requires_tool_result_name
            .unwrap_or(detected.requires_tool_result_name),
        requires_assistant_after_tool_result: c
            .requires_assistant_after_tool_result
            .unwrap_or(detected.requires_assistant_after_tool_result),
        requires_thinking_as_text: c
            .requires_thinking_as_text
            .unwrap_or(detected.requires_thinking_as_text),
        requires_reasoning_content_on_assistant_messages: c
            .requires_reasoning_content_on_assistant_messages
            .unwrap_or(detected.requires_reasoning_content_on_assistant_messages),
        thinking_format: c.thinking_format.unwrap_or(detected.thinking_format),
        chat_template_kwargs: c
            .chat_template_kwargs
            .clone()
            .unwrap_or(detected.chat_template_kwargs),
        zai_tool_stream: c.zai_tool_stream.unwrap_or(detected.zai_tool_stream),
        supports_strict_mode: c.supports_strict_mode.unwrap_or(detected.supports_strict_mode),
        cache_control_format: c.cache_control_format.or(detected.cache_control_format),
        send_session_affinity_headers: c
            .send_session_affinity_headers
            .unwrap_or(detected.send_session_affinity_headers),
        supports_long_cache_retention: c
            .supports_long_cache_retention
            .unwrap_or(detected.supports_long_cache_retention),
    }
}

/// Clamp a prompt-cache key to OpenAI's 64-char limit (Pi `clampOpenAIPromptCacheKey`).
pub const OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH: usize = 64;
pub fn clamp_openai_prompt_cache_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH {
        return key.to_string();
    }
    chars.into_iter().take(OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH).collect()
}

/// Remove unpaired UTF-16 surrogate code units (Pi `sanitizeSurrogates`).
///
/// Rust `String` is always well-formed UTF-8, so it can never *contain* an unpaired surrogate;
/// this is a no-op kept for 1:1 structural parity with Pi's encoder (the JS runtime operates on
/// UTF-16 where lone surrogates are representable).
pub fn sanitize_surrogates(text: &str) -> String {
    text.to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::{Modality, ModelCost};

    fn base_model(provider: &str, base_url: &str, id: &str) -> Model {
        Model {
            id: id.into(),
            name: "M".into(),
            api: crate::known_api::OPENAI_COMPLETIONS.into(),
            provider: provider.into(),
            base_url: Some(base_url.to_string()),
            reasoning: true,
            input: vec![Modality::Text],
            output: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 1000,
            max_tokens: 1000,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    #[test]
    fn detect_together_matrix() {
        let c = detect_compat(&base_model("together", "https://api.together.ai/v1", "x/y"));
        assert!(!c.supports_store);
        assert!(!c.supports_developer_role);
        assert!(!c.supports_reasoning_effort);
        assert_eq!(c.max_tokens_field, MaxTokensField::MaxTokens);
        assert_eq!(c.thinking_format, ThinkingFormat::Together);
        assert!(!c.supports_strict_mode);
        assert!(!c.supports_long_cache_retention);
    }

    #[test]
    fn detect_openai_matrix() {
        let c = detect_compat(&base_model("openai", "https://api.openai.com/v1", "gpt-5"));
        assert!(c.supports_store);
        assert!(c.supports_developer_role);
        assert!(c.supports_reasoning_effort);
        assert_eq!(c.max_tokens_field, MaxTokensField::MaxCompletionTokens);
        assert_eq!(c.thinking_format, ThinkingFormat::Openai);
        assert!(c.supports_strict_mode);
        assert!(c.supports_long_cache_retention);
    }

    #[test]
    fn model_compat_overrides_detection() {
        let mut m = base_model("together", "https://api.together.ai/v1", "openai/gpt-oss-120b");
        m.compat = Some(OpenAiCompletionsCompat {
            supports_reasoning_effort: Some(true),
            thinking_format: Some(ThinkingFormat::Openai),
            ..Default::default()
        });
        let c = get_compat(&m);
        assert!(c.supports_reasoning_effort); // overridden
        assert_eq!(c.thinking_format, ThinkingFormat::Openai); // overridden
        assert!(!c.supports_store); // still detected
        assert_eq!(c.max_tokens_field, MaxTokensField::MaxTokens); // still detected
    }

    #[test]
    fn level_map_three_state() {
        let mut map = ThinkingLevelMap::new();
        map.insert("high".to_string(), Some("high".to_string()));
        map.insert("low".to_string(), None);
        // present string
        assert_eq!(mapped_effort_or(Some(&map), ModelThinkingLevel::High, "fb"), "high");
        // present null -> fallback
        assert_eq!(mapped_effort_or(Some(&map), ModelThinkingLevel::Low, "fb"), "fb");
        // absent -> fallback
        assert_eq!(mapped_effort_or(Some(&map), ModelThinkingLevel::Medium, "fb"), "fb");
    }

    #[test]
    fn clamp_cache_key_truncates() {
        let long = "a".repeat(100);
        assert_eq!(clamp_openai_prompt_cache_key(&long).chars().count(), 64);
        assert_eq!(clamp_openai_prompt_cache_key("short"), "short");
    }
}

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

/// Which session-affinity headers a provider reads.
/// Pi: `sessionAffinityFormat?: SessionAffinityFormat` — `"openai" | "openai-nosession" |
/// "openrouter"` — declared on `OpenAICompletionsCompat` (`types.ts:569` @v0.83.0) and on
/// `OpenAIResponsesCompat` (`:579`). The doc block above the declaration spells out all three
/// header sets; the branches are `openai-completions.ts:647-656` and `openai-responses.ts:233-241`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionAffinityFormat {
    /// `session_id` + `x-client-request-id` (+ `x-session-affinity` on the completions route).
    Openai,
    /// As `openai` but WITHOUT `session_id` — pi's documented migration target for the flag it
    /// deleted (`packages/ai/CHANGELOG.md:168`, #6496).
    OpenaiNosession,
    /// `x-session-id` only — the header OpenRouter actually reads for sticky routing.
    Openrouter,
}

/// `detectSessionAffinityFormat(model)` — `openai-responses.ts:49` @v0.83.0, and the identical
/// `isOpenRouter ? "openrouter" : "openai"` used by the completions detector
/// (`openai-completions.ts:1473`, with `isOpenRouter` defined `:1404`).
pub fn detect_session_affinity_format(model: &Model) -> SessionAffinityFormat {
    if model.provider.as_str() == "openrouter" || model.base_url.as_str().contains("openrouter.ai")
    {
        SessionAffinityFormat::Openrouter
    } else {
        SessionAffinityFormat::Openai
    }
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

/// Per-model compatibility overrides — cyrup's **flat union** of every per-API compat shape.
///
/// Pi types `Model<API>["compat"]` per wire API via the `Model<API>` generic, so the single `compat`
/// JSON key carries `OpenAICompletionsCompat` for `openai-completions` models and
/// `AnthropicMessagesCompat` for `anthropic-messages` models (Pi `types.ts:531` / `:688`). cyrup's
/// `Model` is **not** generic, so the honest non-generic representation is one struct holding the
/// union of all per-API fields (every field optional, so a given model only sets the keys its api
/// understands; the unused ones stay `None` and are skipped on serialize → byte-1:1 with Pi). The
/// `openai-completions` resolver ([`get_compat`]) reads the openai subset; the `anthropic-messages`
/// resolver (`api::anthropic_messages::get_anthropic_compat`) reads the anthropic subset. The
/// [`OpenAiCompletionsCompat`] and [`AnthropicMessagesCompat`] aliases name this union at the use
/// sites where one api's view is intended (closing gap #13's compat-union generalization).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCompat {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_store: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_developer_role: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_reasoning_effort: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_usage_in_streaming: Option<bool>,
    /// Pi `supportsFinishReason` (v0.84.1 `ai/src/types.ts:547-548`, default `true`): whether
    /// streamed responses include `finish_reason`. When `false`, pi INFERS `stop` vs `toolUse` at
    /// end of stream instead of treating the missing reason as a truncated turn.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_finish_reason: Option<bool>,
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
    /// Pi `sessionAffinityFormat` (`types.ts:569` @v0.83.0 on the completions compat, `:579` on the
    /// responses compat). Shared by both openai routes exactly as pi shares it. Unset falls back to
    /// [`detect_session_affinity_format`] (PROV-024/PROV-033).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_affinity_format: Option<SessionAffinityFormat>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_long_cache_retention: Option<bool>,
    /// Pi `supportsOpenAIGrammarTools` (`openai-responses.ts:73` @v0.83.0, default **false**):
    /// provider accepts OpenAI's grammar-constrained custom-tool encoding.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_openai_grammar_tools: Option<bool>,

    // --- `anthropic-messages` subset (Pi `AnthropicMessagesCompat`, types.ts:531). These are read
    // only by the anthropic-messages resolver; `supports_long_cache_retention` and
    // `send_session_affinity_headers` above are SHARED with the anthropic subset (same field/default
    // semantics on both apis), so they are not duplicated here. ---
    /// Pi `supportsEagerToolInputStreaming` (default true): provider accepts per-tool
    /// `eager_input_streaming`; when false, the legacy `fine-grained-tool-streaming` beta is sent
    /// for tool-enabled requests instead.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_eager_tool_input_streaming: Option<bool>,
    /// Pi `supportsCacheControlOnTools` (default true): provider accepts `cache_control` markers on
    /// tool definitions.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_cache_control_on_tools: Option<bool>,
    /// Pi `supportsStrictTools` (`types.ts:639` @v0.83.0, default **false**): model accepts
    /// `tools[].strict: true` and the full JSON schema in `input_schema`, i.e. JSON-schema
    /// constrained sampling on the Anthropic route (PROV-011).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_strict_tools: Option<bool>,
    /// Pi `supportsTemperature` (default true): model accepts the Anthropic `temperature` field
    /// (Claude Opus 4.7+ rejects non-default temperatures).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_temperature: Option<bool>,
    /// Pi `forceAdaptiveThinking` (default false): force `thinking.type:"adaptive"` +
    /// `output_config.effort` regardless of model id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub force_adaptive_thinking: Option<bool>,
    /// Pi `allowEmptySignature` (default false): replay empty thinking signatures as `signature:""`
    /// instead of converting the thinking block to text.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub allow_empty_signature: Option<bool>,
    /// Pi `supportsToolReferences` (types.ts:643-647): provider accepts client-side
    /// `tool_reference` content blocks inside `tool_result`, so tools introduced mid-transcript can
    /// be sent `defer_loading: true` and anchored at their marker instead of sitting in the
    /// cache-stable prefix. Unset falls back to `defaultSupportsToolReferences` (first-party
    /// Anthropic Opus/Sonnet/Fable >= 4.5, non-Haiku) — see
    /// `crate::api::anthropic_messages::default_supports_tool_references`. DRIFT-001.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_tool_references: Option<bool>,

    // --- `openai-responses` subset (Pi `OpenAIResponsesCompat`, openai-responses.ts:57-63). Read
    // only by the openai-responses resolver. `supports_developer_role` and
    // `supports_long_cache_retention` above are SHARED with this subset. ---
    /// Pi `supportsExplicitPromptCacheMode` (`openai-responses.ts:75` @v0.83.0, default
    /// **false** — older OpenAI models reject the parameter): when a one-shot request runs with
    /// `cacheRetention: "none"`, send `prompt_cache_options: {mode:"explicit"}` so the endpoint does
    /// not implicitly cache-WRITE (and bill the premium for) a prompt that will never be re-read
    /// (PROV-023).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_explicit_prompt_cache_mode: Option<bool>,
    /// Pi `supportsToolSearch` (types.ts:588-589, default **false**): provider accepts the
    /// client-side `tool_search_call`/`tool_search_output` pair, the Responses rendering of the
    /// same DRIFT-001 anchor `supports_tool_references` renders for Anthropic.
    ///
    /// Unlike `supports_tool_references` this has **no runtime predicate** — Pi's gate is flatly
    /// `model.compat?.supportsToolSearch ?? false` (openai-responses.ts:74) and enablement is baked
    /// into the generated catalog by `ai/scripts/generate-models.ts:731-738` against a hardcoded id
    /// set. No cyrup catalog sets it today, so it resolves `false` everywhere.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_tool_search: Option<bool>,
}

/// The `openai-responses` view of [`ModelCompat`] (Pi `OpenAIResponsesCompat`,
/// openai-responses.ts:10).
pub type OpenAiResponsesCompat = ModelCompat;

/// Fully-resolved openai-responses compat (Pi `Required<OpenAIResponsesCompat>`, `getCompat`
/// `openai-responses.ts:68-76` @v0.83.0). `supportsDeveloperRole` and `supportsLongCacheRetention`
/// default `true`; `supportsStrictMode`, `supportsOpenAIGrammarTools`, `supportsToolSearch` and
/// `supportsExplicitPromptCacheMode` default **false**; `sessionAffinityFormat` falls back to
/// [`detect_session_affinity_format`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedResponsesCompat {
    pub supports_developer_role: bool,
    /// Pi `sessionAffinityFormat: model.compat?.sessionAffinityFormat ??
    /// detectSessionAffinityFormat(model)` (`openai-responses.ts:70`). Replaces the
    /// `sendSessionIdHeader` flag pi DELETED in #6496 (`packages/ai/CHANGELOG.md:168`, whose
    /// documented migration is `sendSessionIdHeader: false` ⇒ `"openai-nosession"`) — PROV-033.
    pub session_affinity_format: SessionAffinityFormat,
    pub supports_long_cache_retention: bool,
    /// Pi `supportsStrictMode: model.compat?.supportsStrictMode ?? false`
    /// (`openai-responses.ts:72`).
    pub supports_strict_mode: bool,
    /// Pi `supportsOpenAIGrammarTools: … ?? false` (`openai-responses.ts:73`).
    pub supports_openai_grammar_tools: bool,
    /// Pi `supportsToolSearch: model.compat?.supportsToolSearch ?? false`
    /// (openai-responses.ts:74). Catalog-driven; no predicate.
    pub supports_tool_search: bool,
    /// Pi `supportsExplicitPromptCacheMode: … ?? false` (`openai-responses.ts:75`).
    pub supports_explicit_prompt_cache_mode: bool,
}

/// Resolve the openai-responses compat for a model (1:1 port of Pi `getCompat`,
/// `openai-responses.ts:68-76` @v0.83.0).
pub fn get_responses_compat(model: &Model) -> ResolvedResponsesCompat {
    let c = model.compat.as_ref();
    ResolvedResponsesCompat {
        supports_developer_role: c.and_then(|c| c.supports_developer_role).unwrap_or(true),
        session_affinity_format: c
            .and_then(|c| c.session_affinity_format)
            .unwrap_or_else(|| detect_session_affinity_format(model)),
        supports_long_cache_retention: c
            .and_then(|c| c.supports_long_cache_retention)
            .unwrap_or(true),
        supports_strict_mode: c.and_then(|c| c.supports_strict_mode).unwrap_or(false),
        supports_openai_grammar_tools: c
            .and_then(|c| c.supports_openai_grammar_tools)
            .unwrap_or(false),
        supports_tool_search: c.and_then(|c| c.supports_tool_search).unwrap_or(false),
        supports_explicit_prompt_cache_mode: c
            .and_then(|c| c.supports_explicit_prompt_cache_mode)
            .unwrap_or(false),
    }
}

/// The `openai-completions` view of [`ModelCompat`] (Pi `OpenAICompletionsCompat`, types.ts).
pub type OpenAiCompletionsCompat = ModelCompat;
/// The `anthropic-messages` view of [`ModelCompat`] (Pi `AnthropicMessagesCompat`, types.ts:531).
pub type AnthropicMessagesCompat = ModelCompat;

/// Fully-resolved compatibility settings (Pi `ResolvedOpenAICompletionsCompat`). Routing fields are
/// read straight off `model.compat` at build time (matching Pi's `buildParams`), so they are not
/// duplicated here.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedCompat {
    pub supports_store: bool,
    pub supports_developer_role: bool,
    pub supports_reasoning_effort: bool,
    pub supports_usage_in_streaming: bool,
    /// Pi `supportsFinishReason` (v0.84.1 `ai/src/api/openai-completions.ts:1499`, detected `true`).
    pub supports_finish_reason: bool,
    pub max_tokens_field: MaxTokensField,
    pub requires_tool_result_name: bool,
    pub requires_assistant_after_tool_result: bool,
    pub requires_thinking_as_text: bool,
    pub requires_reasoning_content_on_assistant_messages: bool,
    pub thinking_format: ThinkingFormat,
    pub chat_template_kwargs: Map<String, Value>,
    pub zai_tool_stream: bool,
    pub supports_strict_mode: bool,
    /// Pi `supportsOpenAIGrammarTools` — detected **false** (`openai-completions.ts:1469`
    /// @v0.83.0), catalog override resolved at `:1511`. Read by
    /// [`crate::utils::constrained_sampling::resolve_grammar_constrained_sampling`] (PROV-011).
    pub supports_openai_grammar_tools: bool,
    pub cache_control_format: Option<CacheControlFormat>,
    pub send_session_affinity_headers: bool,
    /// Pi `sessionAffinityFormat` — detected `isOpenRouter ? "openrouter" : "openai"`
    /// (`openai-completions.ts:1473` @v0.83.0), catalog override resolved at `:1515` (PROV-024).
    pub session_affinity_format: SessionAffinityFormat,
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
        ModelThinkingLevel::Max => "max",
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
pub fn mapped_effort_or(
    map: Option<&ThinkingLevelMap>,
    level: ModelThinkingLevel,
    fallback: &str,
) -> String {
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
    let base_url = model.base_url.as_str();
    let id = model.id.as_str();

    let is_zai = provider == "zai"
        || provider == "zai-coding-cn"
        || base_url.contains("api.z.ai")
        || base_url.contains("open.bigmodel.cn");
    let is_together = provider == "together"
        || base_url.contains("api.together.ai")
        || base_url.contains("api.together.xyz");
    let is_moonshot = provider == "moonshotai"
        || provider == "moonshotai-cn"
        || base_url.contains("api.moonshot.");
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

    // pi `useMaxTokens` — `openai-completions.ts:1427-1435` @**v0.83.0**, byte-identical at
    // v0.84.1 apart from the line offset (`:1478-1485`).
    //
    // DRIFT-013: the trailing `|| isZai` was dropped in the port, so every Z.AI request carried
    // `max_completion_tokens`, which Z.AI ignores — an effectively uncapped completion. The item
    // classified this `upstream-drift`; it is **not-ported**. `git show
    // v0.83.0:packages/ai/src/api/openai-completions.ts` has `isZai` at `:1435`, inside the
    // ported baseline, so a rebase would never have swept it up.
    let use_max_tokens = base_url.contains("chutes.ai")
        || is_moonshot
        || is_cloudflare_ai_gateway
        || is_together
        || is_nvidia
        || is_ant_ling
        || is_zai;

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
        supports_finish_reason: true,
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
        // `supportsOpenAIGrammarTools: false` (openai-completions.ts:1469 @v0.83.0) — never
        // detected, only enabled by the generated catalog.
        supports_openai_grammar_tools: false,
        cache_control_format,
        send_session_affinity_headers: false,
        // `sessionAffinityFormat: isOpenRouter ? "openrouter" : "openai"`
        // (openai-completions.ts:1473 @v0.83.0).
        session_affinity_format: if is_openrouter {
            SessionAffinityFormat::Openrouter
        } else {
            SessionAffinityFormat::Openai
        },
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
        supports_finish_reason: c
            .supports_finish_reason
            .unwrap_or(detected.supports_finish_reason),
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
        supports_strict_mode: c
            .supports_strict_mode
            .unwrap_or(detected.supports_strict_mode),
        // `supportsOpenAIGrammarTools: model.compat.supportsOpenAIGrammarTools ??
        // detected.supportsOpenAIGrammarTools` (openai-completions.ts:1511 @v0.83.0).
        supports_openai_grammar_tools: c
            .supports_openai_grammar_tools
            .unwrap_or(detected.supports_openai_grammar_tools),
        cache_control_format: c.cache_control_format.or(detected.cache_control_format),
        send_session_affinity_headers: c
            .send_session_affinity_headers
            .unwrap_or(detected.send_session_affinity_headers),
        // `sessionAffinityFormat: model.compat.sessionAffinityFormat ??
        // detected.sessionAffinityFormat` (openai-completions.ts:1515 @v0.83.0).
        session_affinity_format: c
            .session_affinity_format
            .unwrap_or(detected.session_affinity_format),
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
    chars
        .into_iter()
        .take(OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH)
        .collect()
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::model::{Modality, ModelCost};

    fn base_model(provider: &str, base_url: &str, id: &str) -> Model {
        Model {
            id: id.into(),
            name: "M".into(),
            api: crate::known_api::OPENAI_COMPLETIONS.into(),
            provider: provider.into(),
            base_url: base_url.to_string(),
            reasoning: true,
            input: vec![Modality::Text],
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

    /// DRIFT-013. pi's `useMaxTokens` disjunction ends `|| isZai`
    /// (`openai-completions.ts:1427-1435` @v0.83.0). Without it every Z.AI request carried
    /// `max_completion_tokens`, which Z.AI ignores, so the cap silently did nothing.
    ///
    /// All four `isZai` inputs are asserted (`:1400-1404`): both provider ids and both base-URL
    /// hosts, because the two self-hosted-gateway cases are exactly the ones a provider-id check
    /// would miss.
    #[test]
    fn drift013_zai_uses_max_tokens_through_every_detection_route() {
        for (provider, base_url) in [
            ("zai", "https://api.z.ai/api/paas/v4"),
            ("zai-coding-cn", "https://open.bigmodel.cn/api/paas/v4"),
            ("custom", "https://api.z.ai/api/paas/v4"),
            ("custom", "https://open.bigmodel.cn/api/paas/v4"),
        ] {
            let c = detect_compat(&base_model(provider, base_url, "glm-4.6"));
            assert_eq!(
                c.max_tokens_field,
                MaxTokensField::MaxTokens,
                "{provider} @ {base_url} must send max_tokens"
            );
        }

        // The negative half: a plain OpenAI model is untouched by the new term.
        let c = detect_compat(&base_model("openai", "https://api.openai.com/v1", "gpt-5"));
        assert_eq!(c.max_tokens_field, MaxTokensField::MaxCompletionTokens);
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
        let mut m = base_model(
            "together",
            "https://api.together.ai/v1",
            "openai/gpt-oss-120b",
        );
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
        assert_eq!(
            mapped_effort_or(Some(&map), ModelThinkingLevel::High, "fb"),
            "high"
        );
        // present null -> fallback
        assert_eq!(
            mapped_effort_or(Some(&map), ModelThinkingLevel::Low, "fb"),
            "fb"
        );
        // absent -> fallback
        assert_eq!(
            mapped_effort_or(Some(&map), ModelThinkingLevel::Medium, "fb"),
            "fb"
        );
    }

    #[test]
    fn clamp_cache_key_truncates() {
        let long = "a".repeat(100);
        assert_eq!(clamp_openai_prompt_cache_key(&long).chars().count(), 64);
        assert_eq!(clamp_openai_prompt_cache_key("short"), "short");
    }
}

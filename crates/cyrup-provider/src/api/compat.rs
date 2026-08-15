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

/// How deferred (mid-transcript-introduced) tools are rendered on the wire.
/// Pi: `deferredToolsMode?: "kimi"` on `OpenAICompletionsCompat` (`types.ts:567` @v0.83.0).
///
/// PROV-025. The one upstream mode is Kimi's: instead of repeating a tool's schema in the
/// top-level `tools` array on every turn, the schema is emitted ONCE, inline, in a
/// `{role: "system", tools: [...]}` message placed immediately after the tool-result run that
/// introduced it (`openai-completions.ts:1266-1276` @v0.83.0), and the tool is then EXCLUDED from
/// `params.tools` (`:719-721`). The anchor is `ToolResultMessage.addedToolNames`, which cyrup
/// already produces (PROV-009).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeferredToolsMode {
    Kimi,
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

/// A price that may be written as a number or as a string (Pi `number | string`,
/// `types.ts:691-699` @v0.83.0). Passed through verbatim in whichever form the user wrote.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum OpenRouterPrice {
    Number(f64),
    Text(String),
}

/// Per-percentile cutoffs for `preferred_min_throughput` / `preferred_max_latency`
/// (Pi `types.ts:704-713` / `:717-726` @v0.83.0).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRouterPercentiles {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub p50: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub p75: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub p90: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub p99: Option<f64>,
}

/// `number | { p50?, p75?, p90?, p99? }` — "can be a number (applies to p50) or an object with
/// percentile-specific cutoffs" (Pi `types.ts:702-713` @v0.83.0).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum OpenRouterPercentileCutoff {
    /// The bare number form, which OpenRouter applies to p50.
    P50(f64),
    ByPercentile(OpenRouterPercentiles),
}

/// The object form of `sort` (Pi `types.ts:682-687` @v0.83.0).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRouterSortSpec {
    /// The sorting metric: `"price"`, `"throughput"`, `"latency"` (Pi `:684`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub by: Option<String>,
    /// Partitioning strategy: `"model"` (default) or `"none"` (Pi `partition?: string | null`,
    /// `types.ts:686` @v0.83.0).
    ///
    /// The double `Option` and its custom deserializer are load-bearing, because upstream's type is
    /// a THREE-state one: absent, an explicit `null`, or a string. With a plain `Option<String>`
    /// serde maps a JSON `null` to `None`, which `skip_serializing_if` then DELETES from the
    /// outgoing request — so a user's explicit `"partition": null` would silently become "key
    /// absent", a different instruction to OpenRouter. `de_present_partition` runs only for a
    /// PRESENT key, so absent stays `None` while `null` becomes `Some(None)` and serializes back
    /// out as `null`.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "de_present_partition"
    )]
    pub partition: Option<Option<String>>,
}

/// See [`OpenRouterSortSpec::partition`]. Serde calls this only for a PRESENT key, so every value
/// it sees — `null` included — is a value the user actually wrote.
fn de_present_partition<'de, D>(de: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    Option::<String>::deserialize(de).map(Some)
}

/// `sort?: string | { by?, partition? }` (Pi `types.ts:680-687` @v0.83.0).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum OpenRouterSort {
    Named(String),
    Spec(OpenRouterSortSpec),
}

/// Maximum price per million tokens, USD (Pi `types.ts:689-700` @v0.83.0).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRouterMaxPrice {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt: Option<OpenRouterPrice>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub completion: Option<OpenRouterPrice>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<OpenRouterPrice>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio: Option<OpenRouterPrice>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request: Option<OpenRouterPrice>,
}

/// OpenRouter provider-routing preferences — a 1:1 port of Pi `OpenRouterRouting`
/// (`packages/ai/src/types.ts:660-727` @v0.83.0), sent verbatim as the `provider` field of the
/// OpenRouter request body. PROV-066.
///
/// **Why this is a struct and not a `serde_json::Value`.** The value is wire-identical either way —
/// both sides forward it untouched — so what a typed shape buys is the one thing the `Value` form
/// could not do: **reject a misspelled key**. `deny_unknown_fields` here and on every nested object
/// turns `"allow_fallback"` (singular) from a preference OpenRouter silently ignores, presenting to
/// the user as "my `order` never takes effect" with nothing anywhere saying why, into a config
/// error at load. pi gets the same protection from its TypeScript declaration.
///
/// **Field names are the wire names verbatim** — snake_case, NOT the enclosing [`ModelCompat`]'s
/// `rename_all = "camelCase"`, because OpenRouter's API spells them this way and pi's interface
/// declares them this way. That is why this type is declared outside `ModelCompat`'s rename scope
/// and carries no `rename_all` of its own.
///
/// **Key ORDER note.** cyrup's `serde_json` has no `preserve_order` feature, so the previous
/// `Value` representation emitted these keys alphabetically, while pi emits them in the order the
/// user's config file wrote them (JS objects preserve insertion order). This struct emits them in
/// pi's DECLARATION order, which matches neither exactly — but JSON object key order carries no
/// meaning to the OpenRouter API, and no shipped catalog sets `openRouterRouting` at all
/// (`grep openRouterRouting crates/**/catalog/*.json` is empty), so the only producer is a user's
/// `models.json`.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRouterRouting {
    /// Whether to allow backup providers to serve requests. Default: true (Pi `:662`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub allow_fallbacks: Option<bool>,
    /// Filter providers to only those that support all parameters in the request (Pi `:664`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub require_parameters: Option<bool>,
    /// `"allow"` (default) or `"deny"` (Pi `:666`). Typed as a string rather than a two-variant
    /// enum because pi types it `"deny" | "allow"` and an unknown value must reach OpenRouter for
    /// OpenRouter to reject, exactly as it does upstream.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data_collection: Option<String>,
    /// Restrict routing to Zero-Data-Retention endpoints only (Pi `:668`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub zdr: Option<bool>,
    /// Restrict routing to models that allow text distillation (Pi `:670`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub enforce_distillable_text: Option<bool>,
    /// Ordered list of provider names/slugs to try in sequence (Pi `:672`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub order: Option<Vec<String>>,
    /// Providers to exclusively allow for this request (Pi `:674`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub only: Option<Vec<String>>,
    /// Providers to skip for this request (Pi `:676`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ignore: Option<Vec<String>>,
    /// Quantization levels to filter providers by (Pi `:678`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantizations: Option<Vec<String>>,
    /// Sorting strategy (Pi `:680-687`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sort: Option<OpenRouterSort>,
    /// Maximum price per million tokens, USD (Pi `:689-700`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_price: Option<OpenRouterMaxPrice>,
    /// Preferred minimum throughput, tokens/second (Pi `:702-713`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub preferred_min_throughput: Option<OpenRouterPercentileCutoff>,
    /// Preferred maximum latency, seconds (Pi `:715-726`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub preferred_max_latency: Option<OpenRouterPercentileCutoff>,
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
    ///
    /// `[CYRUP-DELTA]` **Forward-port from v0.84.1; no v0.83.0 warrant** (PROV-063). The ported
    /// baseline is v0.83.0 and `git grep supportsFinishReason v0.83.0 -- packages/ai` is EMPTY —
    /// the flag first exists at v0.84.1 (`ai/src/types.ts:548`, read at
    /// `ai/src/api/openai-completions.ts:578`, `:584`, detected `:1499`, resolved `:1551`). It is
    /// the only member of [`ModelCompat`] without a baseline counterpart, kept rather than deleted
    /// because it is real upstream behaviour that the v0.84.1 rebase will need.
    ///
    /// **Inert in every shipped configuration**, and that inertness is pinned rather than assumed:
    /// [`detect_compat`] hard-codes `true` for every provider, no embedded catalog sets the key,
    /// and the sole consumer (`api/openai_completions.rs`'s stop-reason inference) is therefore
    /// unreachable — so behaviour is byte-identical to v0.83.0. See
    /// `supports_finish_reason_is_a_v0841_forward_port_that_stays_inert` and
    /// `crate::tests::catalog_data`'s roster guard.
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
    ///
    /// PROV-066: typed as [`OpenRouterRouting`] rather than a bare `serde_json::Value`, so a
    /// misspelled routing key is a config error instead of a preference OpenRouter silently drops.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub open_router_routing: Option<OpenRouterRouting>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub vercel_gateway_routing: Option<VercelGatewayRouting>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub zai_tool_stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_strict_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_control_format: Option<CacheControlFormat>,
    /// Pi `deferredToolsMode?: "kimi"` (`types.ts:567` @v0.83.0 on `OpenAICompletionsCompat`).
    /// Never detected — only the generated catalog turns it on (PROV-025).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deferred_tools_mode: Option<DeferredToolsMode>,
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
    /// Pi `deferredToolsMode` — detected `undefined` (`openai-completions.ts:1472` @v0.83.0),
    /// catalog override resolved at `:1514` (PROV-025).
    pub deferred_tools_mode: Option<DeferredToolsMode>,
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
        // `deferredToolsMode: undefined` (openai-completions.ts:1472 @v0.83.0) — never detected.
        deferred_tools_mode: None,
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
        // `deferredToolsMode: model.compat.deferredToolsMode ?? detected.deferredToolsMode`
        // (openai-completions.ts:1514 @v0.83.0).
        deferred_tools_mode: c.deferred_tools_mode.or(detected.deferred_tools_mode),
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

// --- model-option compatibility: temperature (PERM-012) ---------------------------------------
//
// A 1:1 port of `pi-permission-system/src/model-option-compatibility.ts` @v0.8.0 (`9affcc9`) —
// upstream's `TEMPERATURE_UNSUPPORTED_APIS` / `TEMPERATURE_UNSUPPORTED_PROVIDERS` /
// `OPENAI_RESPONSES_APIS` (`:11-25`), `normalizeIdentifier` (`:54-56`), `hasModelToken`
// (`:58-60`) and `getUnsupportedTemperatureReason` (`:62-83`).
//
// **[CYRUP-DELTA] — the seam, not the rule.** Upstream cannot edit the request body from inside an
// extension, so it wraps the api provider: `registerModelOptionCompatibilityGuard` (`:164-168`)
// re-registers each guarded api with a `streamSimple` that blanks `options.temperature` and
// installs an `onPayload` hook running `stripUnsupportedTemperatureFromPayload` (`:89-96`,
// `:98-124`), lazily from `index.ts:1485-1497` and awaited first thing in `session_start`
// (`:1829`). That indirection exists only because pi's permission-system is a JS package with no
// access to the request builder; it is a HOST-SPECIFIC FACILITY, not the mechanism. cyrup owns the
// builder, so the same rule is applied where the key is written — the three `*_responses.rs` body
// builders gate their `temperature` insert on [`unsupported_temperature_reason`]. The wire bodies
// are identical; what differs is that cyrup applies it whether or not the permission-system
// extension is loaded, where pi applies it only once that extension has run `session_start`. A
// user cannot notice the difference except by unloading the permission system and getting pi's
// provider error back, which is not a behaviour worth reproducing.

/// `TEMPERATURE_UNSUPPORTED_APIS` (`model-option-compatibility.ts:20-22` @v0.8.0) — apis that never
/// accept `temperature`, whatever the model.
const TEMPERATURE_UNSUPPORTED_APIS: &[&str] = &[crate::known_api::OPENAI_CODEX_RESPONSES];

/// `TEMPERATURE_UNSUPPORTED_PROVIDERS` (`model-option-compatibility.ts:23-25` @v0.8.0).
const TEMPERATURE_UNSUPPORTED_PROVIDERS: &[&str] = &["openai-codex"];

/// `OPENAI_RESPONSES_APIS` (`model-option-compatibility.ts:16-19` @v0.8.0) — the two Responses apis
/// on which the per-MODEL rules (a `codex` id token, or a reasoning model) apply.
const OPENAI_RESPONSES_APIS: &[&str] = &[
    crate::known_api::OPENAI_RESPONSES,
    crate::known_api::AZURE_OPENAI_RESPONSES,
];

/// `normalizeIdentifier` (`model-option-compatibility.ts:54-56` @v0.8.0).
fn normalize_identifier(value: &str) -> String {
    value.trim().to_lowercase()
}

/// `hasModelToken` (`model-option-compatibility.ts:58-60` @v0.8.0):
/// `normalizeIdentifier(modelId).split(/[^a-z0-9]+/).includes(token)`.
///
/// The split is on runs of non-`[a-z0-9]`, so the match is on a whole TOKEN — `gpt-5.5-codex`
/// matches `codex`, `codexify` does not. Rust's `split` on a char predicate yields the same pieces
/// (including the empty leading/trailing ones a JS regex split produces, which cannot equal a
/// non-empty token either way).
fn has_model_token(model_id: &str, token: &str) -> bool {
    normalize_identifier(model_id)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|piece| piece == token)
}

/// `getUnsupportedTemperatureReason(model)` (`model-option-compatibility.ts:62-83` @v0.8.0).
///
/// `Some(reason)` means the outgoing request body must omit `temperature` entirely; the string is
/// upstream's verbatim (it is user-facing nowhere in cyrup today, but it is the reason of record
/// and diverging from it would make the two implementations impossible to diff).
pub fn unsupported_temperature_reason(model: &Model) -> Option<String> {
    let api = model.api.as_str();
    if TEMPERATURE_UNSUPPORTED_APIS.contains(&api) {
        return Some(format!("api '{api}' does not support temperature"));
    }

    let provider = normalize_identifier(model.provider.as_str());
    if TEMPERATURE_UNSUPPORTED_PROVIDERS.contains(&provider.as_str()) {
        // pi reports the RAW provider here (`model.provider`), not the normalized one (`:71`).
        return Some(format!(
            "provider '{}' does not support temperature",
            model.provider.as_str()
        ));
    }

    if OPENAI_RESPONSES_APIS.contains(&api) && has_model_token(model.id.as_str(), "codex") {
        return Some(format!(
            "model '{}' does not support temperature",
            model.id.as_str()
        ));
    }

    if OPENAI_RESPONSES_APIS.contains(&api) && model.reasoning {
        return Some(format!(
            "reasoning model '{}' accepts only the provider default temperature",
            model.id.as_str()
        ));
    }

    None
}

/// `stripUnsupportedTemperatureFromPayload` composed with the guard
/// (`model-option-compatibility.ts:89-124` @v0.8.0), expressed as the predicate a body builder
/// wants: may this request carry `temperature` at all?
pub fn temperature_is_supported(model: &Model) -> bool {
    unsupported_temperature_reason(model).is_none()
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
            sampling_params: None,
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

    /// PROV-063 — pins the INERTNESS of the one `ModelCompat` member with no v0.83.0 warrant.
    ///
    /// `supports_finish_reason` is a knowing forward-port from v0.84.1 (`ai/src/types.ts:548`;
    /// `git grep supportsFinishReason v0.83.0 -- packages/ai` is empty). Its `[CYRUP-DELTA]` tag
    /// says behaviour is byte-identical to the baseline; that claim rests on two facts, and this
    /// test makes both of them properties rather than observations:
    ///
    /// 1. `detect_compat` answers `true` for EVERY provider, so the flag is on unless a catalog
    ///    turns it off; and
    /// 2. no embedded catalog turns it off — so `get_compat` can never yield `false` and the sole
    ///    consumer (`openai_completions.rs`'s stop-reason inference) is unreachable.
    ///
    /// **This test does not go red before any change in this pass** — PROV-063 proposed no code
    /// change and the fix is the `[CYRUP-DELTA]` tag. It goes red the moment somebody makes the
    /// delta live: shipping a catalog row with `"supportsFinishReason": false`, or flipping the
    /// detection default, without carrying the item forward.
    #[test]
    fn supports_finish_reason_is_a_v0841_forward_port_that_stays_inert() {
        // (1) Detection is unconditionally `true`, across every shape `detect_compat` branches on.
        for (provider, base_url, id) in [
            ("openai", "https://api.openai.com/v1", "gpt-5"),
            ("together", "https://api.together.ai/v1", "x/y"),
            ("zai", "https://api.z.ai/api/paas/v4", "glm-4.6"),
            ("deepseek", "https://api.deepseek.com", "deepseek-chat"),
            ("openrouter", "https://openrouter.ai/api/v1", "a/b"),
            ("moonshotai", "https://api.moonshot.ai/v1", "kimi-k2"),
            ("nvidia", "https://integrate.api.nvidia.com/v1", "n/m"),
            ("ant-ling", "https://api.ling.ai/v1", "ling-1"),
            ("custom", "https://example.invalid/v1", "whatever"),
        ] {
            assert!(
                detect_compat(&base_model(provider, base_url, id)).supports_finish_reason,
                "{provider} must detect supports_finish_reason = true (v0.83.0 has no flag at all)"
            );
        }

        // (2) No embedded catalog row sets the key, so the resolver can never see `Some(false)`.
        for model in crate::catalog::builtin_catalog() {
            assert_eq!(
                model
                    .compat
                    .as_ref()
                    .and_then(|c| c.supports_finish_reason),
                None,
                "{}/{} sets supportsFinishReason — a v0.84.1-only flag with no v0.83.0 warrant \
                 (PROV-063). Carry the item forward before shipping it.",
                model.provider.as_str(),
                model.id.as_str()
            );
            assert!(
                get_compat(model).supports_finish_reason,
                "{}/{} resolves supports_finish_reason = false, so the v0.84.1 inference branch \
                 is LIVE and cyrup no longer matches v0.83.0 (PROV-063)",
                model.provider.as_str(),
                model.id.as_str()
            );
        }
    }

    /// PROV-066 (a) — a fully populated routing object round-trips through the typed shape with
    /// zero semantic change, so nothing on the wire moved.
    ///
    /// **Red before the fix:** `open_router_routing` was `Option<serde_json::Value>`; every field
    /// access below (`routing.sort`, `routing.max_price.prompt`, …) is a compile error against that
    /// type, so this test could not exist. The `Value` form accepted this JSON and any other.
    #[test]
    fn prov066_open_router_routing_round_trips_every_upstream_field() {
        // Every member of pi `OpenRouterRouting` (`types.ts:660-727` @v0.83.0), including all four
        // union arms: `sort` as an object, `max_price` mixing number and string, and both
        // percentile cutoffs in their two forms.
        let json = serde_json::json!({
            "allow_fallbacks": false,
            "require_parameters": true,
            "data_collection": "deny",
            "zdr": true,
            "enforce_distillable_text": false,
            "order": ["alpha", "beta"],
            "only": ["alpha"],
            "ignore": ["gamma"],
            "quantizations": ["fp16", "bf16"],
            "sort": { "by": "throughput", "partition": "model" },
            "max_price": { "prompt": 1.5, "completion": "2.0", "image": 0.1, "audio": 0.2, "request": 0.01 },
            "preferred_min_throughput": 42.0,
            "preferred_max_latency": { "p50": 1.0, "p99": 9.0 },
        });

        let routing: OpenRouterRouting =
            serde_json::from_value(json.clone()).expect("every upstream field must parse");

        assert_eq!(routing.allow_fallbacks, Some(false));
        assert_eq!(routing.require_parameters, Some(true));
        assert_eq!(routing.data_collection.as_deref(), Some("deny"));
        assert_eq!(routing.zdr, Some(true));
        assert_eq!(routing.enforce_distillable_text, Some(false));
        assert_eq!(routing.order.as_deref().map(<[String]>::len), Some(2));
        assert!(matches!(
            routing.sort,
            Some(OpenRouterSort::Spec(OpenRouterSortSpec {
                by: Some(ref by),
                partition: Some(Some(ref p)),
            })) if by == "throughput" && p == "model"
        ));
        assert!(matches!(
            routing.max_price.as_ref().and_then(|m| m.prompt.clone()),
            Some(OpenRouterPrice::Number(n)) if (n - 1.5).abs() < f64::EPSILON
        ));
        assert!(matches!(
            routing.max_price.as_ref().and_then(|m| m.completion.clone()),
            Some(OpenRouterPrice::Text(ref s)) if s == "2.0"
        ));
        assert!(matches!(
            routing.preferred_min_throughput,
            Some(OpenRouterPercentileCutoff::P50(_))
        ));
        assert!(matches!(
            routing.preferred_max_latency,
            Some(OpenRouterPercentileCutoff::ByPercentile(_))
        ));

        // The wire payload is unchanged: re-serializing yields the same object, key for key and
        // value for value. (Key ORDER differs from the old `Value` path — see the type's doc — so
        // this compares parsed values, which is what the OpenRouter API actually consumes.)
        let back = serde_json::to_value(&routing).expect("serializes");
        assert_eq!(back, json, "the typed shape must be wire-identical to the JSON it came from");
    }

    /// PROV-066 (b) — the whole point: a misspelled key is a config error, not a silent
    /// pass-through.
    ///
    /// **Red before the fix:** with `Option<serde_json::Value>` this JSON deserialized cleanly and
    /// was forwarded verbatim; OpenRouter ignores unknown members, so the user's `order` preference
    /// vanished with nothing anywhere saying why. `assert!(… .is_err())` failed.
    #[test]
    fn prov066_a_misspelled_routing_key_is_rejected() {
        // `allow_fallback` — singular; the real key is `allow_fallbacks`.
        let typo = serde_json::json!({ "order": ["alpha"], "allow_fallback": false });
        let err = serde_json::from_value::<OpenRouterRouting>(typo)
            .expect_err("a misspelled routing key must not pass silently");
        assert!(
            err.to_string().contains("allow_fallback"),
            "the error must name the offending key so the user can fix it: {err}"
        );

        // Nested objects are guarded too — the same failure mode one level down.
        let nested_typo = serde_json::json!({ "max_price": { "prmopt": 1.0 } });
        assert!(
            serde_json::from_value::<OpenRouterRouting>(nested_typo).is_err(),
            "deny_unknown_fields must reach the nested max_price object"
        );
        let sort_typo = serde_json::json!({ "sort": { "bye": "price" } });
        assert!(
            serde_json::from_value::<OpenRouterRouting>(sort_typo).is_err(),
            "deny_unknown_fields must reach the nested sort object"
        );

        // And the valid spellings still parse, so the guard is not simply rejecting everything.
        assert!(
            serde_json::from_value::<OpenRouterRouting>(
                serde_json::json!({ "order": ["alpha"], "allow_fallbacks": false })
            )
            .is_ok()
        );
    }

    /// PROV-066 (c) — `sort.partition: null` is preserved rather than dropped. pi types it
    /// `string | null` (`types.ts:686`), so an explicit null is a value, and collapsing it into
    /// `Option::None` would silently remove the key from the request.
    #[test]
    fn prov066_an_explicit_null_partition_survives_the_round_trip() {
        let json = serde_json::json!({ "sort": { "by": "price", "partition": null } });
        let routing: OpenRouterRouting = serde_json::from_value(json.clone()).expect("parses");
        assert!(
            matches!(
                routing.sort,
                Some(OpenRouterSort::Spec(OpenRouterSortSpec {
                    partition: Some(None),
                    ..
                }))
            ),
            "an explicit null must survive as Some(None), not collapse to None"
        );
        assert_eq!(serde_json::to_value(&routing).expect("serializes"), json);
    }

    #[test]
    fn clamp_cache_key_truncates() {
        let long = "a".repeat(100);
        assert_eq!(clamp_openai_prompt_cache_key(&long).chars().count(), 64);
        assert_eq!(clamp_openai_prompt_cache_key("short"), "short");
    }

    // --- PERM-012: `getUnsupportedTemperatureReason` (`model-option-compatibility.ts:62-83`
    // @v0.8.0). Every arm, in upstream's order, with its verbatim reason string — the strings are
    // what make the two implementations diffable, so they are asserted rather than just the
    // Some/None. Red at HEAD~: the whole function did not exist.

    fn temp_model(api: &str, provider: &str, id: &str, reasoning: bool) -> Model {
        Model {
            api: api.into(),
            provider: provider.into(),
            id: id.into(),
            reasoning,
            ..base_model(provider, "", id)
        }
    }

    #[test]
    fn the_codex_responses_api_never_supports_temperature() {
        let m = temp_model(
            crate::known_api::OPENAI_CODEX_RESPONSES,
            "openai-codex",
            "gpt-5.5-codex",
            false,
        );
        assert_eq!(
            unsupported_temperature_reason(&m).as_deref(),
            Some("api 'openai-codex-responses' does not support temperature")
        );
        assert!(!temperature_is_supported(&m));
    }

    /// The provider arm fires even on an api that is NOT in either api set — upstream checks the
    /// provider unconditionally (`:69-72`), after the api check and before the two Responses-only
    /// rules.
    #[test]
    fn the_openai_codex_provider_never_supports_temperature() {
        let m = temp_model(
            crate::known_api::OPENAI_COMPLETIONS,
            "openai-codex",
            "some-model",
            false,
        );
        assert_eq!(
            unsupported_temperature_reason(&m).as_deref(),
            Some("provider 'openai-codex' does not support temperature")
        );
    }

    /// `normalizeIdentifier` trims + lowercases the provider for the LOOKUP, while the reported
    /// string is the RAW `model.provider` (`:70-72`).
    #[test]
    fn the_provider_lookup_normalizes_but_the_reason_quotes_the_raw_id() {
        let m = temp_model(
            crate::known_api::OPENAI_COMPLETIONS,
            "  OpenAI-Codex ",
            "some-model",
            false,
        );
        assert_eq!(
            unsupported_temperature_reason(&m).as_deref(),
            Some("provider '  OpenAI-Codex ' does not support temperature")
        );
    }

    #[test]
    fn a_codex_token_in_the_id_strips_on_both_responses_apis() {
        for api in [
            crate::known_api::OPENAI_RESPONSES,
            crate::known_api::AZURE_OPENAI_RESPONSES,
        ] {
            let m = temp_model(api, "openai", "gpt-5.5-codex", false);
            assert_eq!(
                unsupported_temperature_reason(&m).as_deref(),
                Some("model 'gpt-5.5-codex' does not support temperature"),
                "api {api}"
            );
        }
    }

    /// `hasModelToken` splits on runs of non-`[a-z0-9]` and compares WHOLE tokens (`:58-60`), so a
    /// model whose id merely CONTAINS the substring is untouched. A `contains()` implementation
    /// would silently strip temperature from it.
    #[test]
    fn a_codex_substring_that_is_not_a_token_does_not_strip() {
        let m = temp_model(
            crate::known_api::OPENAI_RESPONSES,
            "openai",
            "codexify-4",
            false,
        );
        assert_eq!(unsupported_temperature_reason(&m), None);
        assert!(temperature_is_supported(&m));
    }

    #[test]
    fn a_reasoning_model_on_a_responses_api_strips() {
        let m = temp_model(crate::known_api::OPENAI_RESPONSES, "openai", "gpt-5", true);
        assert_eq!(
            unsupported_temperature_reason(&m).as_deref(),
            Some("reasoning model 'gpt-5' accepts only the provider default temperature")
        );
    }

    /// The rule is scoped to the two Responses apis: the SAME reasoning model on
    /// `openai-completions` keeps its temperature, because upstream's last two arms are both
    /// guarded by `OPENAI_RESPONSES_APIS.has(model.api)` (`:74`, `:78`).
    #[test]
    fn a_reasoning_model_on_another_api_is_untouched() {
        let m = temp_model(crate::known_api::OPENAI_COMPLETIONS, "openai", "gpt-5", true);
        assert_eq!(unsupported_temperature_reason(&m), None);
    }

    #[test]
    fn an_ordinary_responses_model_is_untouched() {
        let m = temp_model(crate::known_api::OPENAI_RESPONSES, "openai", "gpt-4o", false);
        assert_eq!(unsupported_temperature_reason(&m), None);
        assert!(temperature_is_supported(&m));
    }
}

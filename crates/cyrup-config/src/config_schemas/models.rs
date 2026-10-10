//! `models.schema.json` — port of pi's `ModelsConfigSchema` (`packages/coding-agent/src/core/
//! model-config.ts:19-87` @f1b2e77f5) and the pi-ai schemas it is built from:
//! `packages/ai/src/providers/model-schema.ts` (cost, input limits, prompt cache, thinking-level
//! map) and `packages/ai/src/providers/compat-schema.ts` (`ProviderCompatSchema`).
//!
//! The runtime half of the same contract is [`crate::model::validate_models_config`] (plus the
//! serde pass over [`crate::ModelFile`]); this document is what an editor sees. The two rules
//! CFG-104 and CFG-105 added there are encoded here too: `samplingParamsByThinkingLevel`
//! (`model-config.ts:20-28`) and `contextWindow` / `maxTokens` as `exclusiveMinimum: 0` on both a
//! model definition and a `modelOverrides` entry (`:30`, `:43-44`, `:59-60`), plus the optional
//! top-level `$schema` string (`:84-87`).
//!
//! **[CYRUP-DELTA]s in `ProviderCompat`**, each matching what cyrup's
//! `cyrup_provider::api::compat::ModelCompat` deserializes:
//!
//! - `deferredToolsMode` and `supportsToolReferences` are cyrup compat keys that pi's
//!   `compat-schema.ts` no longer lists; cyrup still reads both (PROV-025, DRIFT-001), so they
//!   are published with descriptions taken from `compat.rs`.
//! - `openRouterRouting` (and its nested `sort`, `max_price` and percentile objects) and
//!   `allowedFallbackModels[]` are `additionalProperties: false`: cyrup's types for them are
//!   `deny_unknown_fields`, so a misspelled routing key is a load error (PROV-066), and the
//!   schema says so rather than accepting what cyrup will refuse.
//!
//! A description that names pi as the actor ("Pi auto-detects", "pi-controlled values") names
//! cyrup instead.

use serde_json::{Value, json};

use super::settings::MODEL_THINKING_LEVELS;
use super::typebox::{
    array, boolean, described, enumeration, integer, literal, non_empty_string, null, number,
    object, opt, partial, record, req, string, string_literals, union, unknown, with,
};

/// The models document's `$defs`, in pi's order (`generate-schemas.ts:35-41`).
pub(super) fn definitions() -> Vec<(&'static str, Value)> {
    vec![
        ("ModelCost", model_cost()),
        ("ModelInputLimits", model_input_limits()),
        ("ModelPromptCache", model_prompt_cache()),
        ("ProviderCompat", provider_compat()),
        ("ThinkingLevelMap", thinking_level_map()),
    ]
}

// ---------------------------------------------------------------------------------------------
// `packages/ai/src/providers/model-schema.ts` @f1b2e77f5
// ---------------------------------------------------------------------------------------------

/// `ThinkingLevelMapSchema = Type.Partial(Type.Record(ModelThinkingLevelSchema,
/// Type.Union([Type.String(), Type.Null()])))` (`model-schema.ts:16-17`). A record keyed by a
/// closed enum is an object with one optional property per member.
fn thinking_level_map() -> Value {
    object(
        MODEL_THINKING_LEVELS
            .iter()
            .map(|level| opt(level, union(vec![string(), null()])))
            .collect(),
    )
}

/// `ModelPromptCacheSchema` (`model-schema.ts:19-25`).
fn model_prompt_cache() -> Value {
    described(
        object(vec![
            opt("short", with(number(), json!({ "exclusiveMinimum": 0 }))),
            opt("long", with(number(), json!({ "exclusiveMinimum": 0 }))),
        ]),
        "Best-effort prompt cache lifetime in seconds for each retention tier. A missing tier \
         means the lifetime is unknown, so cyrup does not warm it.",
    )
}

/// `ModelCostRatesProperties` (`model-schema.ts:27-32`).
fn cost_rates(required: bool) -> Vec<super::typebox::Prop> {
    let rate = |name: &'static str, description: &str| {
        let schema = described(number(), description);
        if required {
            req(name, schema)
        } else {
            opt(name, schema)
        }
    };
    vec![
        rate("input", "Input cost in USD per million tokens."),
        rate("output", "Output cost in USD per million tokens."),
        rate("cacheRead", "Cache-read cost in USD per million tokens."),
        rate("cacheWrite", "Cache-write cost in USD per million tokens."),
    ]
}

/// `ModelCostTierSchema` (`model-schema.ts:35-40`).
fn model_cost_tier() -> Value {
    let mut props = vec![req(
        "inputTokensAbove",
        described(
            number(),
            "Use this tier when total request input exceeds this token count.",
        ),
    )];
    props.extend(cost_rates(true));
    object(props)
}

/// `ModelCostSchema` (`model-schema.ts:41-48`).
fn model_cost() -> Value {
    let mut props = cost_rates(true);
    props.push(opt(
        "tiers",
        described(
            array(model_cost_tier()),
            "Request-wide pricing tiers. The highest matching input threshold applies to the \
             full request.",
        ),
    ));
    object(props)
}

fn positive_integer(description: Option<&str>) -> Value {
    let schema = with(integer(), json!({ "minimum": 1 }));
    match description {
        Some(description) => described(schema, description),
        None => schema,
    }
}

/// `modelImageResizeOptions(options)` (`model-schema.ts:50-62`).
fn model_image_resize_options() -> Value {
    object(vec![
        opt("maxWidth", positive_integer(None)),
        opt("maxHeight", positive_integer(None)),
        opt(
            "maxBytes",
            positive_integer(Some("Maximum base64-encoded payload size in bytes.")),
        ),
        opt(
            "jpegQuality",
            with(integer(), json!({ "minimum": 1, "maximum": 100 })),
        ),
    ])
}

/// `ModelInputLimitsSchema` with its nested `ModelImageInputLimitsSchema`
/// (`model-schema.ts:66-86`).
fn model_input_limits() -> Value {
    let images = object(vec![
        opt(
            "resize",
            described(
                model_image_resize_options(),
                "Cache-safe resize profile applied before a new image enters conversation \
                 history.",
            ),
        ),
        opt(
            "maxPerMessage",
            positive_integer(Some("Maximum images accepted in one provider message.")),
        ),
        opt(
            "maxPerRequest",
            positive_integer(Some("Maximum images accepted across one provider request.")),
        ),
    ]);
    object(vec![
        opt(
            "maxRequestBytes",
            positive_integer(Some("Maximum serialized provider request size in bytes.")),
        ),
        opt("images", images),
    ])
}

// ---------------------------------------------------------------------------------------------
// `packages/ai/src/providers/compat-schema.ts` @f1b2e77f5
// ---------------------------------------------------------------------------------------------

/// `sessionAffinityFormat(options)` (`compat-schema.ts:4-6`).
fn session_affinity_format(description: &str) -> Value {
    described(
        string_literals(&["openai", "openai-nosession", "openrouter"]),
        description,
    )
}

/// `thinkingTokenBudgetField(options)` (`compat-schema.ts:12-17`).
fn thinking_token_budget_field(description: &str) -> Value {
    described(
        string_literals(&[
            "thinking_token_budget",
            "thinking_budget",
            "thinking_budget_tokens",
        ]),
        description,
    )
}

/// `ChatTemplateKwargValueSchema` (`compat-schema.ts:24-37`).
fn chat_template_kwarg_value() -> Value {
    union(vec![
        string(),
        number(),
        boolean(),
        null(),
        object(vec![
            req(
                "$var",
                string_literals(&["thinking.enabled", "thinking.effort", "thinking.budget"]),
            ),
            opt("omitWhenOff", boolean()),
        ]),
    ])
}

/// `additionalProperties: false` — cyrup's `deny_unknown_fields` (see the module doc).
fn closed(schema: Value) -> Value {
    with(schema, json!({ "additionalProperties": false }))
}

/// `percentileCutoffs(metric)` (`compat-schema.ts:39-46`). Closed: `OpenRouterPercentiles` is
/// `deny_unknown_fields`.
fn percentile_cutoffs(metric: &str) -> Value {
    let p = |name: &'static str, nth: &str| {
        opt(
            name,
            described(number(), &format!("{metric} at the {nth} percentile.")),
        )
    };
    closed(object(vec![
        p("p50", "50th"),
        p("p75", "75th"),
        p("p90", "90th"),
        p("p99", "99th"),
    ]))
}

fn number_or_string(description: &str) -> Value {
    described(union(vec![number(), string()]), description)
}

/// `OpenRouterRoutingSchema` (`compat-schema.ts:48-148`), closed at every object level.
fn open_router_routing() -> Value {
    let sort = described(
        union(vec![
            string(),
            closed(object(vec![
                opt(
                    "by",
                    described(
                        string(),
                        "The sorting metric, such as \"price\", \"throughput\", or \"latency\".",
                    ),
                ),
                opt(
                    "partition",
                    with(
                        union(vec![string(), null()]),
                        json!({
                            "description": "Partitioning strategy: \"model\" or \"none\".",
                            "default": "model",
                        }),
                    ),
                ),
            ])),
        ]),
        "Sorting strategy. Can be a string such as \"price\", \"throughput\", or \"latency\", or \
         an object.",
    );
    let max_price = described(
        closed(object(vec![
            opt(
                "prompt",
                number_or_string("Price per million prompt tokens."),
            ),
            opt(
                "completion",
                number_or_string("Price per million completion tokens."),
            ),
            opt("image", number_or_string("Price per image.")),
            opt("audio", number_or_string("Price per audio unit.")),
            opt("request", number_or_string("Price per request.")),
        ])),
        "Maximum price per million tokens in USD.",
    );
    described(
        closed(object(vec![
            opt(
                "allow_fallbacks",
                with(
                    boolean(),
                    json!({
                        "description": "Whether to allow backup providers to serve requests.",
                        "default": true,
                    }),
                ),
            ),
            opt(
                "require_parameters",
                with(
                    boolean(),
                    json!({
                        "description": "Whether to filter providers to only those that support all parameters in the request.",
                        "default": false,
                    }),
                ),
            ),
            opt(
                "data_collection",
                with(
                    string_literals(&["deny", "allow"]),
                    json!({
                        "description": "Data collection setting. \"allow\": allow providers that may store or train on data. \"deny\": only use providers that do not collect user data.",
                        "default": "allow",
                    }),
                ),
            ),
            opt(
                "zdr",
                described(
                    boolean(),
                    "Whether to restrict routing to only ZDR (Zero Data Retention) endpoints.",
                ),
            ),
            opt(
                "enforce_distillable_text",
                described(
                    boolean(),
                    "Whether to restrict routing to only models that allow text distillation.",
                ),
            ),
            opt(
                "order",
                described(
                    array(string()),
                    "An ordered list of provider names or slugs to try in sequence, falling back \
                     to the next if unavailable.",
                ),
            ),
            opt(
                "only",
                described(
                    array(string()),
                    "List of provider names or slugs to exclusively allow for this request.",
                ),
            ),
            opt(
                "ignore",
                described(
                    array(string()),
                    "List of provider names or slugs to skip for this request.",
                ),
            ),
            opt(
                "quantizations",
                described(
                    array(string()),
                    "A list of quantization levels to filter providers by, for example \
                     [\"fp16\", \"bf16\", \"fp8\", \"fp6\", \"int8\", \"int4\", \"fp4\", \
                     \"fp32\"].",
                ),
            ),
            opt("sort", sort),
            opt("max_price", max_price),
            opt(
                "preferred_min_throughput",
                described(
                    union(vec![
                        number(),
                        percentile_cutoffs("Minimum tokens per second"),
                    ]),
                    "Preferred minimum throughput in tokens per second. A number applies to p50.",
                ),
            ),
            opt(
                "preferred_max_latency",
                described(
                    union(vec![
                        number(),
                        percentile_cutoffs("Maximum latency in seconds"),
                    ]),
                    "Preferred maximum latency in seconds. A number applies to p50.",
                ),
            ),
        ])),
        "OpenRouter provider routing preferences. Controls which upstream providers OpenRouter \
         routes requests to. Sent as the provider field in the OpenRouter API request body. See \
         https://openrouter.ai/docs/guides/routing/provider-selection.",
    )
}

/// `VercelGatewayRoutingSchema` (`compat-schema.ts:150-167`).
fn vercel_gateway_routing() -> Value {
    described(
        object(vec![
            opt(
                "only",
                described(
                    array(string()),
                    "List of provider slugs to exclusively use for this request, for example \
                     [\"bedrock\", \"anthropic\"].",
                ),
            ),
            opt(
                "order",
                described(
                    array(string()),
                    "List of provider slugs to try in order, for example [\"anthropic\", \
                     \"openai\"].",
                ),
            ),
        ]),
        "Vercel AI Gateway routing preferences. Controls which upstream providers the gateway \
         routes requests to. See \
         https://vercel.com/docs/ai-gateway/models-and-providers/provider-options.",
    )
}

/// `AnthropicAllowedFallbackModelSchema` (`compat-schema.ts:169-176`), closed: cyrup's
/// `AnthropicAllowedFallbackModel` is `deny_unknown_fields`.
fn anthropic_allowed_fallback_model() -> Value {
    with(
        object(vec![
            req("provider", non_empty_string()),
            req("model", non_empty_string()),
            req("cost", model_cost()),
        ]),
        json!({
            "description": "An Anthropic server-side refusal fallback model with local pricing metadata.",
            "additionalProperties": false,
        }),
    )
}

fn bool_described(description: &str) -> Value {
    described(boolean(), description)
}

fn bool_default(description: &str, default: bool) -> Value {
    with(
        boolean(),
        json!({ "description": description, "default": default }),
    )
}

type Props = Vec<(&'static str, Value)>;

/// `OpenAICompletionsCompatSchema.properties` (`compat-schema.ts:186-361`).
fn openai_completions_compat() -> Props {
    vec![
        (
            "supportsStore",
            bool_described(
                "Whether the provider supports the store field. Default: auto-detected from URL.",
            ),
        ),
        (
            "supportsDeveloperRole",
            bool_described(
                "Whether the provider supports the developer role instead of system. Default: \
                 auto-detected from URL.",
            ),
        ),
        (
            "supportsReasoningEffort",
            bool_described(
                "Whether the provider supports reasoning_effort. Default: auto-detected from URL.",
            ),
        ),
        (
            "supportsUsageInStreaming",
            bool_default(
                "Whether the provider supports stream_options.include_usage for token usage in \
                 streaming responses.",
                true,
            ),
        ),
        (
            "supportsFinishReason",
            bool_default(
                "Whether streamed responses include finish_reason. When false, cyrup infers stop \
                 or toolUse when the stream ends.",
                true,
            ),
        ),
        (
            "maxTokensField",
            described(
                string_literals(&["max_completion_tokens", "max_tokens"]),
                "Which field to use for max tokens. Default: auto-detected from URL.",
            ),
        ),
        (
            "requiresToolResultName",
            bool_described(
                "Whether tool results require the name field. Default: auto-detected from URL.",
            ),
        ),
        (
            "requiresAssistantAfterToolResult",
            bool_described(
                "Whether a user message after tool results requires an assistant message in \
                 between. Default: auto-detected from URL.",
            ),
        ),
        (
            "requiresThinkingAsText",
            bool_described(
                "Whether thinking blocks must be converted to text blocks with <thinking> \
                 delimiters. Default: auto-detected from URL.",
            ),
        ),
        (
            "requiresReasoningContentOnAssistantMessages",
            bool_described(
                "Whether all replayed assistant messages must include an empty reasoning_content \
                 field when reasoning is enabled. Default: auto-detected from URL.",
            ),
        ),
        (
            "thinkingFormat",
            described(
                string_literals(&[
                    "openai",
                    "openrouter",
                    "deepseek",
                    "together",
                    "baseten",
                    "zai",
                    "qwen",
                    "chat-template",
                    "qwen-chat-template",
                    "string-thinking",
                    "ant-ling",
                ]),
                "Format for reasoning or thinking parameters. When omitted, cyrup auto-detects the \
                 format from the provider URL. \"openai\" uses reasoning_effort, \"openrouter\" \
                 uses reasoning.effort, \"deepseek\" uses thinking.type plus reasoning_effort when \
                 supported, \"together\" uses reasoning.enabled plus reasoning_effort when \
                 supported, \"baseten\" uses configurable chat_template_args plus \
                 reasoning_effort when supported, \"zai\" uses thinking.type, \"qwen\" uses \
                 top-level enable_thinking, \"qwen-chat-template\" uses \
                 chat_template_kwargs.enable_thinking and preserve_thinking, \"chat-template\" \
                 uses configurable chat_template_kwargs, \"string-thinking\" uses top-level \
                 thinking, and \"ant-ling\" uses reasoning.effort only when the mapped effort is \
                 non-null.",
            ),
        ),
        (
            "chatTemplateKwargs",
            described(
                record(chat_template_kwarg_value()),
                "Kwargs sent as chat_template_kwargs when thinkingFormat is \"chat-template\". \
                 Use $var with \"thinking.enabled\", \"thinking.effort\", or \"thinking.budget\" \
                 for cyrup-controlled values.",
            ),
        ),
        (
            "chatTemplateArgs",
            described(
                record(chat_template_kwarg_value()),
                "Arguments sent as chat_template_args when thinkingFormat is \"baseten\". Use \
                 $var with \"thinking.enabled\", \"thinking.effort\", or \"thinking.budget\" for \
                 cyrup-controlled values.",
            ),
        ),
        ("openRouterRouting", open_router_routing()),
        ("vercelGatewayRouting", vercel_gateway_routing()),
        (
            "zaiToolStream",
            bool_default(
                "Whether z.ai supports top-level tool_stream for streaming tool call deltas.",
                false,
            ),
        ),
        (
            "thinkingTokenBudgetField",
            thinking_token_budget_field(
                "Top-level request field used to cap reasoning tokens from thinkingBudgets. \
                 Reasoning and the answer share max_tokens on these endpoints. \
                 \"thinking_token_budget\" is vLLM, \"thinking_budget\" is Qwen, DashScope, or \
                 SGLang, and \"thinking_budget_tokens\" is llama.cpp. Off by default and not set \
                 on the generated catalog.",
            ),
        ),
        (
            "supportsThinkingTokenBudget",
            bool_default(
                "Alias for thinkingTokenBudgetField: \"thinking_token_budget\" (vLLM). Prefer \
                 thinkingTokenBudgetField.",
                false,
            ),
        ),
        (
            "supportsOpenAIGrammarTools",
            bool_default(
                "Whether the provider supports OpenAI custom tools with Lark or regex grammar \
                 formats. When false, grammar-constrained tools fall back to normal function \
                 tools. The generated catalog enables this for capable models.",
                false,
            ),
        ),
        (
            "supportsMidConvoSystemMessages",
            bool_default(
                "Whether the exact model accepts system or developer messages after the \
                 conversation has started. When false, later system messages are folded into the \
                 leading system message. The generated catalog enables this for verified models.",
                false,
            ),
        ),
        (
            "supportsMidConvoToolAdditions",
            bool_default(
                "Whether system messages can introduce additional tools mid-conversation. \
                 Requires supportsMidConvoSystemMessages. The generated catalog enables this for \
                 capable models.",
                false,
            ),
        ),
        (
            "supportsStrictMode",
            bool_default(
                "Whether the provider supports the strict field in tool definitions. Generated \
                 capable models enable it explicitly.",
                false,
            ),
        ),
        (
            "cacheControlFormat",
            described(
                literal("anthropic"),
                "Cache control convention for prompt caching. Anthropic applies cache_control \
                 markers to the system prompt, last tool definition, and last user, assistant, \
                 or tool-result text content.",
            ),
        ),
        (
            "sendSessionAffinityHeaders",
            bool_described(
                "Whether to send session-affinity data from options.sessionId. Default: true for \
                 OpenRouter endpoints, false otherwise.",
            ),
        ),
        (
            "sessionAffinityFormat",
            session_affinity_format(
                "Session-affinity header format. openai sends session_id, x-client-request-id, \
                 and x-session-affinity; openai-nosession sends x-client-request-id and \
                 x-session-affinity; openrouter sends x-session-id. Does not affect \
                 prompt_cache_key. Default: auto-detected.",
            ),
        ),
        (
            "supportsLongCacheRetention",
            bool_described(
                "Whether the provider supports long prompt cache retention \
                 (prompt_cache_retention: \"24h\" or Anthropic-style cache_control.ttl: \"1h\", \
                 depending on format). Default: auto-detected from provider and URL.",
            ),
        ),
        (
            "vllmPriority",
            described(
                number(),
                "vLLM scheduler priority sent as the top-level priority request field. Lower \
                 values are handled earlier and the server default is 0. Only meaningful with \
                 --scheduling-policy priority. Off by default and not set on the generated \
                 catalog.",
            ),
        ),
    ]
}

/// `OpenAIResponsesCompatSchema.properties` (`compat-schema.ts:363-430`).
fn openai_responses_compat() -> Props {
    vec![
        (
            "supportsDeveloperRole",
            bool_default(
                "Whether the provider supports the developer role instead of system.",
                true,
            ),
        ),
        (
            "supportsMidConvoSystemMessages",
            bool_default(
                "Whether the exact model accepts developer or system messages after the \
                 conversation has started. When false, later system messages are folded into the \
                 leading system message. The generated catalog enables this for verified models.",
                false,
            ),
        ),
        (
            "sessionAffinityFormat",
            session_affinity_format(
                "Session-affinity header format. openai sends session_id and \
                 x-client-request-id; openai-nosession sends x-client-request-id; openrouter \
                 sends x-session-id. Does not affect prompt_cache_key. Default: auto-detected.",
            ),
        ),
        (
            "supportsLongCacheRetention",
            bool_default(
                "Whether the provider supports long prompt cache retention. This uses \
                 prompt_cache_options.ttl: \"30m\" on GPT-5.6+ and prompt_cache_retention: \
                 \"24h\" on earlier models.",
                true,
            ),
        ),
        (
            "supportsStrictMode",
            bool_described(
                "Whether the provider supports strict JSON-schema function tools. Defaults are \
                 API-specific; generated OpenAI models enable it explicitly.",
            ),
        ),
        (
            "supportsOpenAIGrammarTools",
            bool_default(
                "Whether to emit OpenAI custom tools with Lark or regex grammar formats. When \
                 false, grammar-constrained tools fall back to normal function tools. The \
                 generated catalog enables this for capable models.",
                false,
            ),
        ),
        (
            "supportsAdditionalTools",
            bool_default(
                "Whether the model supports message-anchored additional_tools input items.",
                false,
            ),
        ),
        (
            "supportsToolSearch",
            bool_default(
                "Whether the model supports client-executed tool search for transcript-anchored \
                 additions.",
                false,
            ),
        ),
        (
            "supportsExplicitPromptCacheMode",
            bool_default(
                "Whether the model accepts prompt_cache_options. Older OpenAI models reject the \
                 parameter.",
                false,
            ),
        ),
        (
            "supportsMaxOutputTokens",
            bool_default(
                "Whether the provider accepts max_output_tokens. Some Codex-protocol gateways \
                 reject it.",
                true,
            ),
        ),
    ]
}

/// `AnthropicMessagesCompatSchema.properties` (`compat-schema.ts:432-525`).
fn anthropic_messages_compat() -> Props {
    vec![
        (
            "supportsEagerToolInputStreaming",
            bool_default(
                "Whether the provider accepts per-tool eager_input_streaming. When false, the \
                 Anthropic provider omits tools[].eager_input_streaming and sends the legacy \
                 fine-grained-tool-streaming-2025-05-14 beta header for tool-enabled requests.",
                true,
            ),
        ),
        (
            "supportsLongCacheRetention",
            bool_default(
                "Whether the provider supports Anthropic long cache retention through \
                 cache_control.ttl.",
                true,
            ),
        ),
        (
            "sendSessionAffinityHeaders",
            bool_described(
                "Whether to send x-session-affinity from options.sessionId when caching is \
                 enabled. Required for providers like Fireworks that use session affinity for \
                 prompt cache routing; requests to the same replica maximize cache hits. \
                 Default: true for OpenRouter endpoints, false otherwise.",
            ),
        ),
        (
            "sessionAffinityFormat",
            described(
                literal("openrouter"),
                "Session-affinity format. openrouter sends x-session-id; when unset, sends \
                 x-session-affinity.",
            ),
        ),
        (
            "supportsCacheControlOnTools",
            bool_default(
                "Whether the provider supports Anthropic-style cache_control markers on tool \
                 definitions. When false, cache_control is omitted from tool parameters. Some \
                 Anthropic-compatible providers, such as Fireworks, do not support this field on \
                 tools and may reject or ignore it.",
                true,
            ),
        ),
        (
            "supportsTemperature",
            bool_default(
                "Whether the model accepts the Anthropic temperature request field. Claude Opus \
                 4.7+ rejects non-default values.",
                true,
            ),
        ),
        (
            "forceAdaptiveThinking",
            bool_default(
                "Whether to force adaptive thinking (thinking.type: adaptive plus \
                 output_config.effort) regardless of model ID. Built-in models that require \
                 adaptive thinking set this in generated metadata. Custom Anthropic-compatible \
                 providers can set this to true for any model whose upstream requires the \
                 adaptive format. Set false to opt out on overridden built-in models.",
                false,
            ),
        ),
        (
            "allowEmptySignature",
            bool_default(
                "Whether to replay empty thinking signatures instead of converting thinking to \
                 text.",
                false,
            ),
        ),
        (
            "supportsStrictTools",
            bool_default(
                "Whether the provider supports Anthropic strict tool schemas. Generated \
                 Anthropic models enable it explicitly.",
                false,
            ),
        ),
        (
            "supportsMidConvoEffort",
            bool_default(
                "Whether the exact model transport supports effort-only system messages and \
                 thinking binding controls.",
                false,
            ),
        ),
        (
            "supportsMidConvoSystemMessages",
            bool_default(
                "Whether the exact model accepts system-role messages inside the conversation. \
                 When false, later system messages are folded into the top-level system prompt.",
                false,
            ),
        ),
        (
            "supportsMidConvoToolChanges",
            bool_default(
                "Whether the exact model accepts mid-conversation tool_addition and tool_removal \
                 blocks. Requires supportsMidConvoSystemMessages.",
                false,
            ),
        ),
        (
            "allowedFallbackModels",
            with(
                array(anthropic_allowed_fallback_model()),
                json!({
                    "maxItems": 3,
                    "description": "Models Anthropic accepts for server-side refusal fallback, with local pricing metadata for returned fallback responses. When absent or empty, callers must omit fallbacks; Anthropic rejects the field for models with no permitted fallback targets.",
                }),
            ),
        ),
    ]
}

/// `BedrockCompatSchema.properties` (`compat-schema.ts:527-535`).
fn bedrock_compat() -> Props {
    vec![(
        "supportsStrictMode",
        bool_default(
            "Whether the model supports Bedrock strict tool schemas.",
            false,
        ),
    )]
}

/// `MistralConversationsCompatSchema.properties` (`compat-schema.ts:537-546`).
fn mistral_conversations_compat() -> Props {
    vec![(
        "supportsMidConvoSystemMessages",
        bool_default(
            "Whether the exact model accepts system messages after the conversation has started. \
             When false, later system messages are folded into the leading system message.",
            false,
        ),
    )]
}

/// `ProviderCompatPropertyOverrides` (`compat-schema.ts:548-575`): the API-agnostic description
/// every property declared by more than one API schema takes in the merged provider schema.
fn provider_compat_property_overrides() -> Props {
    vec![
        (
            "supportsDeveloperRole",
            bool_described(
                "Whether the provider supports the developer role instead of system. Defaults \
                 are API-specific.",
            ),
        ),
        (
            "supportsMidConvoSystemMessages",
            bool_default(
                "Whether the exact model accepts system or developer messages after the \
                 conversation has started. When false, later system messages are folded into the \
                 leading system message.",
                false,
            ),
        ),
        (
            "sessionAffinityFormat",
            session_affinity_format(
                "Session-affinity header format. Defaults are API-specific or auto-detected.",
            ),
        ),
        (
            "supportsLongCacheRetention",
            bool_described(
                "Whether the provider supports long prompt cache retention. Defaults are \
                 API-specific or auto-detected.",
            ),
        ),
        (
            "supportsStrictMode",
            bool_described(
                "Whether the provider supports strict tool schemas. Defaults are API-specific.",
            ),
        ),
        (
            "supportsOpenAIGrammarTools",
            bool_default(
                "Whether the provider supports OpenAI custom tools with Lark or regex grammar \
                 formats. When false, grammar-constrained tools fall back to normal function \
                 tools.",
                false,
            ),
        ),
        (
            "sendSessionAffinityHeaders",
            bool_described(
                "Whether to send session-affinity data from options.sessionId. Defaults are \
                 API-specific.",
            ),
        ),
    ]
}

/// **[CYRUP-DELTA]** compat keys cyrup's `ModelCompat` reads that pi's `compat-schema.ts` @f1b2e77f5
/// does not list (see the module doc). Appended after pi's merged set.
fn cyrup_only_compat() -> Props {
    vec![
        (
            "deferredToolsMode",
            described(
                literal("kimi"),
                "How tools introduced mid-transcript are rendered. \"kimi\" emits each new \
                 tool's schema once, inline, in a system message after the tool-result run that \
                 introduced it, instead of repeating it in the top-level tools array.",
            ),
        ),
        (
            "supportsToolReferences",
            bool_described(
                "Whether the provider accepts client-side tool_reference content blocks inside \
                 tool_result, so tools introduced mid-transcript can be sent with \
                 defer_loading. Default: detected from the Anthropic model id.",
            ),
        ),
    ]
}

/// The result of pi's `mergeCompatProperties` (`compat-schema.ts:599-628`).
struct MergedCompat {
    properties: Props,
    /// A property declared by two API schemas with no override — pi throws `Duplicate
    /// compatibility schema property requires an override`.
    duplicates_without_override: Vec<&'static str>,
    /// An override naming a property no two schemas share — pi throws `Compatibility schema
    /// override does not resolve a duplicate property`.
    overrides_without_duplicate: Vec<&'static str>,
}

/// pi's `mergeCompatProperties`: the union of every API schema's properties in first-declaration
/// order, where a property two schemas share takes its override.
fn merge_compat_properties() -> MergedCompat {
    let groups = [
        openai_completions_compat(),
        openai_responses_compat(),
        anthropic_messages_compat(),
        bedrock_compat(),
        mistral_conversations_compat(),
    ];
    let overrides = provider_compat_property_overrides();
    let mut merged: Props = Vec::new();
    let mut duplicates: Vec<&'static str> = Vec::new();
    for group in groups {
        for (name, schema) in group {
            match merged.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => {
                    if !duplicates.contains(&name) {
                        duplicates.push(name);
                    }
                    slot.1 = schema;
                }
                None => merged.push((name, schema)),
            }
        }
    }
    let duplicates_without_override = duplicates
        .iter()
        .copied()
        .filter(|name| !overrides.iter().any(|(o, _)| o == name))
        .collect();
    let overrides_without_duplicate = overrides
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !duplicates.contains(name))
        .collect();
    for (name, schema) in overrides {
        if let Some(slot) = merged.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = schema;
        }
    }
    MergedCompat {
        properties: merged,
        duplicates_without_override,
        overrides_without_duplicate,
    }
}

/// pi throws from `mergeCompatProperties` while the module loads; the generator refuses to render
/// instead.
pub(super) fn check_compat_merge() -> Result<(), super::ConfigSchemaError> {
    let merged = merge_compat_properties();
    if let Some(name) = merged.duplicates_without_override.first() {
        return Err(super::ConfigSchemaError::CompatMerge(format!(
            "Duplicate compatibility schema property requires an override: {name}"
        )));
    }
    if let Some(name) = merged.overrides_without_duplicate.first() {
        return Err(super::ConfigSchemaError::CompatMerge(format!(
            "Compatibility schema override does not resolve a duplicate property: {name}"
        )));
    }
    Ok(())
}

/// `ProviderCompatSchema` (`compat-schema.ts:636-639`), plus [`cyrup_only_compat`].
fn provider_compat() -> Value {
    let mut properties = merge_compat_properties().properties;
    properties.extend(cyrup_only_compat());
    with(
        object(properties.into_iter().map(|(n, s)| opt(n, s)).collect()),
        json!({
            "description": "Provider and model compatibility overrides.",
            "additionalProperties": true,
        }),
    )
}

// ---------------------------------------------------------------------------------------------
// `packages/coding-agent/src/core/model-config.ts` @f1b2e77f5
// ---------------------------------------------------------------------------------------------

/// `SamplingParamsSchema = Type.Record(Type.String(), Type.Unknown())` (`model-config.ts:19`).
fn sampling_params() -> Value {
    record(unknown())
}

/// `SamplingParamsByThinkingLevelSchema` (`model-config.ts:20-28`). CFG-104.
fn sampling_params_by_thinking_level() -> Value {
    object(
        MODEL_THINKING_LEVELS
            .iter()
            .map(|level| opt(level, sampling_params()))
            .collect(),
    )
}

/// `PositiveTokenCountSchema = Type.Number({ exclusiveMinimum: 0 })` (`model-config.ts:30`).
/// CFG-105.
fn positive_token_count() -> Value {
    with(number(), json!({ "exclusiveMinimum": 0 }))
}

fn model_input_modalities() -> Value {
    // `Type.Array(ModelInputModalitySchema)`, `ModelInputModalitySchema =
    // Type.Enum(["text", "image"])` (`model-schema.ts:5`, `:11`).
    array(enumeration(&["text", "image"]))
}

fn string_record() -> Value {
    record(string())
}

/// `ModelDefinitionSchema` (`model-config.ts:32-49`).
fn model_definition() -> Value {
    object(vec![
        req("id", non_empty_string()),
        opt("name", non_empty_string()),
        opt("api", non_empty_string()),
        opt("baseUrl", non_empty_string()),
        opt("reasoning", boolean()),
        opt("thinkingLevelMap", thinking_level_map()),
        opt("input", model_input_modalities()),
        opt("inputLimits", model_input_limits()),
        opt("cost", model_cost()),
        opt("promptCache", model_prompt_cache()),
        opt("contextWindow", positive_token_count()),
        opt("maxTokens", positive_token_count()),
        opt("samplingParams", sampling_params()),
        opt(
            "samplingParamsByThinkingLevel",
            sampling_params_by_thinking_level(),
        ),
        opt("headers", string_record()),
        opt("compat", provider_compat()),
    ])
}

/// `ModelOverrideSchema` (`model-config.ts:51-65`). Its `cost` is `Type.Partial(ModelCostSchema)`:
/// every rate is optional, so it is not the `ModelCost` definition.
fn model_override() -> Value {
    object(vec![
        opt("name", non_empty_string()),
        opt("reasoning", boolean()),
        opt("thinkingLevelMap", thinking_level_map()),
        opt("input", model_input_modalities()),
        opt("inputLimits", model_input_limits()),
        opt("cost", partial(model_cost())),
        opt("promptCache", model_prompt_cache()),
        opt("contextWindow", positive_token_count()),
        opt("maxTokens", positive_token_count()),
        opt("samplingParams", sampling_params()),
        opt(
            "samplingParamsByThinkingLevel",
            sampling_params_by_thinking_level(),
        ),
        opt("headers", string_record()),
        opt("compat", provider_compat()),
    ])
}

/// `ProviderConfigSchema` (`model-config.ts:67-78`).
fn provider_config() -> Value {
    object(vec![
        opt("name", non_empty_string()),
        opt("baseUrl", non_empty_string()),
        opt("apiKey", non_empty_string()),
        opt("api", non_empty_string()),
        opt("oauth", literal("radius")),
        opt("headers", string_record()),
        opt("compat", provider_compat()),
        opt("authHeader", boolean()),
        opt("models", array(model_definition())),
        opt("modelOverrides", record(model_override())),
    ])
}

/// `ModelsConfigSchema` (`model-config.ts:80-87`).
pub(super) fn models_config_schema() -> Value {
    object(vec![
        opt("$schema", super::schema_reference_property(None)),
        req("providers", record(provider_config())),
    ])
}

//! The streaming event model + per-request options (arch-01 §8 / func-01 §8).

use std::sync::Arc;
use cyrup_core::{
    AssistantMessage, CancelToken, EventStream, ModelThinkingLevel, ProviderId, SessionId,
    StopReason, ToolCall,
};
use futures::StreamExt;

pub mod sse;

/// Prompt-cache retention preference (func-01 §11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheRetention {
    None,
    #[default]
    Short,
    Long,
}

/// Preferred transport for providers that support multiple transports (Pi `Transport`,
/// types.ts:98). Providers that do not support the option ignore it. `kebab-case` makes the wire
/// bytes byte-1:1 with Pi: `"sse"`, `"websocket"`, `"websocket-cached"`, `"auto"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    Sse,
    Websocket,
    WebsocketCached,
    Auto,
}

/// The HTTP response metadata handed to [`StreamOptions::on_response`] before the body is consumed
/// (Pi `ProviderResponse`, types.ts:104-107).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderResponse {
    pub status: u16,
    pub headers: std::collections::BTreeMap<String, String>,
}

/// Inspect or replace a provider payload before sending (Pi `StreamOptions.onPayload`,
/// types.ts:130-134). Returning `None` keeps the payload unchanged.
///
/// ASYNC (gap-08 #2): the extension `before_provider_request` producer dispatches into wasm, which
/// is async; the hook is invoked with `.await` in the (already async) wire `run` fns. It is NOT
/// bridged sync→async via `block_on` (that panics on a current-thread runtime — no-panic DENY).
pub type OnPayload = std::sync::Arc<
    dyn Fn(serde_json::Value, crate::model::Model) -> futures::future::BoxFuture<'static, Option<serde_json::Value>>
        + Send
        + Sync,
>;

/// Invoked after an HTTP response is received and before its body stream is consumed (Pi
/// `StreamOptions.onResponse`, types.ts:135-139).
///
/// ASYNC (gap-08 #3): the extension `after_provider_response` producer dispatches into wasm; see
/// [`OnPayload`] for the rationale.
pub type OnResponseHook = std::sync::Arc<
    dyn Fn(ProviderResponse, crate::model::Model) -> futures::future::BoxFuture<'static, ()>
        + Send
        + Sync,
>;

/// Apply the async `on_payload` hook (gap-08 #2) to an outbound request body: if a hook is set,
/// `.await` it and adopt any replacement wholesale (Pi `emitBeforeProviderRequest` REPLACES the
/// payload, sdk.ts:332-338). Called by each wire `run` fn just before constructing the request.
pub async fn apply_on_payload(
    opts: &StreamOptions,
    model: &crate::model::Model,
    body: serde_json::Value,
) -> serde_json::Value {
    if let Some(h) = &opts.on_payload
        && let Some(replaced) = h(body.clone(), model.clone()).await
    {
        return replaced;
    }
    body
}

/// Bridges the sync `open_sse` response-observation point (func-01 R-01-049) to the async
/// `on_response` hook (gap-08 #3): the sync shim records `{status, headers}` into a shared cell
/// during connect; after `open_sse` returns, [`ResponseCapture::fire`] `.await`s the async hook.
#[derive(Clone, Default)]
pub struct ResponseCapture(std::sync::Arc<std::sync::Mutex<Option<ProviderResponse>>>);

impl ResponseCapture {
    /// The sync `open_sse` callback that records the response metadata (`None` when no hook is set,
    /// so `open_sse` skips it entirely).
    pub fn sse_hook(&self, opts: &StreamOptions) -> Option<self::sse::OnResponse> {
        opts.on_response.as_ref()?;
        let cell = self.0.clone();
        Some(std::sync::Arc::new(move |status: u16, headers: &reqwest::header::HeaderMap| {
            let map = headers
                .iter()
                .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_string(), s.to_string())))
                .collect();
            *cell.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(ProviderResponse { status, headers: map });
        }))
    }

    /// Fire the async `on_response` hook with the captured metadata (no-op when unset).
    pub async fn fire(&self, opts: &StreamOptions, model: &crate::model::Model) {
        if let Some(h) = &opts.on_response {
            let resp = self.0.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
            h(resp, model.clone()).await;
        }
    }
}

/// Provider-scoped environment overrides (Pi `ProviderEnv`, types.ts:100-101). Values take
/// precedence over the process environment for provider configuration.
pub type ProviderEnv = std::collections::BTreeMap<String, String>;

/// Caller-specified tool-choice constraint (Pi `OpenAICompletionsOptions.toolChoice`:
/// `"auto" | "none" | "required" | { type: "function"; function: { name } }`). When `None`, the
/// wire impl omits `tool_choice` entirely (matching Pi's default — it never auto-injects `"auto"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Function { name: String },
}

impl ToolChoice {
    /// The OpenAI `tool_choice` wire JSON for this choice.
    pub fn to_wire(&self) -> serde_json::Value {
        match self {
            ToolChoice::Auto => serde_json::Value::String("auto".to_string()),
            ToolChoice::None => serde_json::Value::String("none".to_string()),
            ToolChoice::Required => serde_json::Value::String("required".to_string()),
            ToolChoice::Function { name } => serde_json::json!({
                "type": "function",
                "function": { "name": name },
            }),
        }
    }
}

/// A `Models`-level outbound-header transform (Pi `ModelsStreamTransforms.transformHeaders`,
/// `models.ts:60` @v0.83.0). Async because pi's signature is
/// `(headers) => ProviderHeaders | Promise<ProviderHeaders>` and its production consumer awaits an
/// extension dispatch.
pub type TransformHeadersFn = std::sync::Arc<
    dyn Fn(crate::HeaderMap) -> futures::future::BoxFuture<'static, crate::HeaderMap>
        + Send
        + Sync,
>;

/// Per-request options (func-01 §13). Errors never throw; cancellation is delivered as a terminal
/// `StreamEvent::Error` with `stop_reason: Aborted` (func-01 R-01-044).
#[derive(Clone, Default)]
pub struct StreamOptions {
    pub cancel: Option<CancelToken>,
    pub api_key: Option<String>,
    /// Forwarded for cache routing / session affinity (func-01 R-01-039).
    pub session_id: Option<SessionId>,
    /// Caller-specified prompt-cache retention. `None` = unset: the encoder then consults the
    /// `PI_CACHE_RETENTION` env var (Pi `resolveCacheRetention`, openai-completions.ts:141-149).
    /// An explicit `Some(_)` always wins over the env. Additive, backward-compatible (defaults to
    /// `None`, which resolves to `Short` unless the env promotes it to `Long`).
    pub cache_retention: Option<CacheRetention>,
    pub temperature: Option<f32>,
    /// Arbitrary sampling parameters merged into the request body **as-is, after the named request
    /// fields, so keys here override them** (Pi `StreamOptions.samplingParams`, types.ts:183-189
    /// @v0.84.1, introduced by `25a2c8dcf`; declared between `temperature` and `maxTokens`, which is
    /// why it sits here). Lets custom OpenAI-compatible servers (llama.cpp, vLLM, SGLang, …) receive
    /// parameters cyrup does not model — `top_p`, `top_k`, `min_p`, `repetition_penalty`.
    ///
    /// Merged over [`crate::Model::sampling_params`] per key by
    /// [`crate::utils::simple_options::build_base_options`] (`simple-options.ts:27-33`). **Only
    /// applied by the OpenAI-compatible adapters** — completions, responses, Azure responses
    /// (`openai-completions.ts:885-887`, `openai-responses.ts:331-333`,
    /// `azure-openai-responses.ts:325-327` @v0.84.1) — every other api ignores it, including
    /// `openai-codex-responses`, which upstream does NOT apply it in. AGENT-026.
    pub sampling_params: Option<serde_json::Map<String, serde_json::Value>>,
    pub max_tokens: Option<u64>,
    /// Unified reasoning level (func-01 R-01-040). Additive, backward-compatible (defaulted to
    /// `Off`); a non-reasoning model silently ignores it (R-01-041).
    pub reasoning: ModelThinkingLevel,
    /// Per-level custom thinking token budgets for token-budget providers (Pi
    /// `SimpleStreamOptions.thinkingBudgets`, types.ts:293). `build_base_options` threads the unified
    /// `SimpleStreamOptions.thinking_budgets` here so the API wire (e.g. anthropic-messages'
    /// `adjustMaxTokensForThinking`, anthropic-messages.ts:792-797) can honor it. A non-budget
    /// provider ignores it. Additive, backward-compatible (defaults to `None`).
    pub thinking_budgets: Option<crate::utils::simple_options::ThinkingBudgets>,
    /// Per-request header overlay; a `None` value suppresses a default header (func-01 §4.1).
    pub headers: Option<crate::HeaderMap>,
    /// `ModelsStreamTransforms.transformHeaders` — "Transform fully assembled model/auth/request
    /// headers before provider dispatch" (Pi `models.ts:58-64` @v0.83.0, mixed into
    /// `ModelsApiStreamOptions` / `ModelsSimpleStreamOptions` at `:65-66`).
    ///
    /// Applied by [`crate::collection::Models`] at pi's exact position — after auth headers and
    /// request headers are merged, *last* (`models.ts:480`) — and then **stripped** from what the
    /// provider and its wire impl see (`:483`, the `const { transformHeaders: _t, ...providerOptions }`
    /// rest-spread), so it is a `Models`-level seam and not part of the api option surface.
    ///
    /// This is where `mergeProviderAttributionHeaders` and the `before_provider_headers` extension
    /// hook belong; pi's production consumer is `coding-agent/src/core/sdk.ts:318-327`. PROV-042.
    pub transform_headers: Option<TransformHeadersFn>,
    /// Optional tool-choice constraint (Pi `OpenAICompletionsOptions.toolChoice`). Additive,
    /// backward-compatible (defaults to `None`, which omits the `tool_choice` field).
    pub tool_choice: Option<ToolChoice>,
    /// Preferred transport for providers that support multiple transports (Pi
    /// `StreamOptions.transport`, types.ts:118). Providers that do not support it ignore it.
    pub transport: Option<Transport>,
    /// HTTP request timeout in milliseconds for providers/SDKs that support it (Pi
    /// `StreamOptions.timeoutMs`, types.ts:153).
    pub timeout_ms: Option<u64>,
    /// WebSocket connect (handshake) timeout in milliseconds for WebSocket transports (Pi
    /// `StreamOptions.websocketConnectTimeoutMs`, types.ts:159).
    ///
    /// # `None` must stay `None` here — the 15 s default belongs at the connect site (CFG-058)
    ///
    /// pi keeps this `undefined` all the way down and defaults it **at the socket**: the getter
    /// returns `number | undefined` when the key is unset (`settings-manager.ts:842-844`
    /// @v0.83.0), `sdk.ts:309-315` threads that `undefined` through verbatim
    /// (`options?.websocketConnectTimeoutMs ?? settingsManager.getWebSocketConnectTimeoutMs()`),
    /// and only `connectWebSocket` supplies the number, as a *parameter default* —
    /// `const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS = 15_000;`
    /// (`packages/ai/src/api/openai-codex-responses.ts:64` @v0.83.0) applied at `:1039`, with
    /// `if (connectTimeoutMs > 0)` at `:1102` making an explicit `0` mean *disabled* rather than
    /// *immediate*. Documented as the user-visible default at
    /// `packages/coding-agent/docs/settings.md:172`.
    ///
    /// So **defaulting this field to `Some(15_000)` — in `Default`, in `build_base_options`, or in
    /// the settings thread at `cyrup-session-svc/src/builder.rs` — would be the divergence, not the
    /// fix.** It would erase the distinction between "unset" and "explicitly 15 000" that pi's
    /// `??` chain relies on, and it would apply a WebSocket handshake bound to a code path that
    /// never opens one.
    ///
    /// **Why there is no connect site to default it at:** cyrup's `openai-codex-responses` port has
    /// no WebSocket client at all — see that module's "Mechanism deltas" header — so every
    /// transport resolves to SSE, which is pi's own documented behaviour in a runtime that exposes
    /// no WebSocket constructor (`connectWebSocket` throws at `:1043-1045`, `stream` records the
    /// failure and breaks to the SSE path at `:358-377`). The field is therefore carried faithfully
    /// (settings → [`crate::utils::simple_options::build_base_options`] → here) and consumed by
    /// nothing, which is correct: **the constant lands WITH the WebSocket transport, in the same
    /// change, or it lands nowhere.** CFG-058 stands refuted as filed — an unset key cannot produce
    /// an unbounded handshake in a port that performs no handshake.
    pub websocket_connect_timeout_ms: Option<u64>,
    /// Maximum retry attempts for providers/SDKs that support client-side retries (Pi
    /// `StreamOptions.maxRetries`, types.ts:164).
    pub max_retries: Option<u32>,
    /// Maximum delay (ms) to wait for a server-requested retry before failing immediately (Pi
    /// `StreamOptions.maxRetryDelayMs`, types.ts:172). `Some(0)` disables the cap.
    pub max_retry_delay_ms: Option<u64>,
    /// Provider-extracted request metadata; providers take the fields they understand and ignore
    /// the rest (Pi `StreamOptions.metadata`, types.ts:178 — e.g. Anthropic `user_id`).
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    /// Provider-scoped environment overrides, taking precedence over the process environment (Pi
    /// `StreamOptions.env`, types.ts:184).
    pub env: Option<ProviderEnv>,
    /// Inspect or replace the provider payload before sending (Pi `StreamOptions.onPayload`,
    /// types.ts:130). Additive; defaults to `None`.
    pub on_payload: Option<OnPayload>,
    /// Invoked after an HTTP response is received, before its body is consumed (Pi
    /// `StreamOptions.onResponse`, types.ts:135). Additive; defaults to `None`.
    pub on_response: Option<OnResponseHook>,
    /// Per-API typed options (Pi `ApiStreamOptions<TApi>` / `ApiOptionsMap`, types.ts:189-214). A
    /// wire impl extracts its own variant and ignores the rest; an absent or mismatched variant
    /// leaves every default unchanged. Additive; defaults to `None`.
    pub api_options: Option<ApiStreamOptions>,
}

/// Per-API typed stream options, mirroring Pi's `ApiStreamOptions<TApi>` resolution against
/// `ApiOptionsMap` (types.ts:189-214): each known API resolves to its own concrete option struct.
/// A wire impl downcasts to its own variant via [`StreamOptions`]'s typed accessors; any other
/// variant is ignored (the impl keeps Pi's defaults). Only the variants whose fields cyrup did not
/// already carry on [`StreamOptions`] are modeled here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiStreamOptions {
    /// Options for the `anthropic-messages` wire protocol.
    Anthropic(crate::api::anthropic_messages::AnthropicOptions),
    /// Options for the `openai-responses` wire protocol.
    OpenAiResponses(crate::api::openai_responses::OpenAiResponsesOptions),
    /// Options for the `azure-openai-responses` wire protocol.
    AzureOpenAiResponses(crate::api::azure_openai_responses::AzureOpenAiResponsesOptions),
    /// Options for the `openai-codex-responses` wire protocol.
    OpenAiCodexResponses(crate::api::openai_codex_responses::OpenAiCodexResponsesOptions),
    /// Options for the `bedrock-converse-stream` wire protocol.
    Bedrock(crate::api::bedrock_converse_stream::BedrockOptions),
    /// Options for the `google-generative-ai` wire protocol.
    Google(crate::api::google_generative_ai::GoogleOptions),
    /// Options for the `mistral-conversations` wire protocol.
    Mistral(crate::api::mistral_conversations::MistralOptions),
    // There is deliberately NO `OpenAiCompletions` variant, and PROV-015 should not add an empty
    // one. `OpenAICompletionsOptions extends StreamOptions` (`api/openai-completions.ts:141-144`
    // @v0.83.0) declares exactly two own members — `toolChoice` and `reasoningEffort` — and
    // v0.84.1 adds one more, `thinkingBudgets` (`:142-147`). All three are already carried on
    // cyrup's [`StreamOptions`] in the same role: `tool_choice`, `reasoning` and
    // `thinking_budgets`. A variant here would therefore be an empty struct modelling nothing, and
    // would create two sources of truth for `tool_choice`. Re-check this note if upstream adds a
    // completions-only member that is NOT already on `StreamOptions`.
}

impl ApiStreamOptions {
    /// The `anthropic-messages` options, if this is that variant.
    pub fn anthropic(&self) -> Option<&crate::api::anthropic_messages::AnthropicOptions> {
        match self {
            ApiStreamOptions::Anthropic(o) => Some(o),
            _ => None,
        }
    }

    /// The `openai-responses` options, if this is that variant.
    pub fn openai_responses(
        &self,
    ) -> Option<&crate::api::openai_responses::OpenAiResponsesOptions> {
        match self {
            ApiStreamOptions::OpenAiResponses(o) => Some(o),
            _ => None,
        }
    }

    /// The `azure-openai-responses` options, if this is that variant.
    pub fn azure_openai_responses(
        &self,
    ) -> Option<&crate::api::azure_openai_responses::AzureOpenAiResponsesOptions> {
        match self {
            ApiStreamOptions::AzureOpenAiResponses(o) => Some(o),
            _ => None,
        }
    }

    /// The `openai-codex-responses` options, if this is that variant.
    pub fn openai_codex_responses(
        &self,
    ) -> Option<&crate::api::openai_codex_responses::OpenAiCodexResponsesOptions> {
        match self {
            ApiStreamOptions::OpenAiCodexResponses(o) => Some(o),
            _ => None,
        }
    }

    /// The `bedrock-converse-stream` options, if this is that variant.
    pub fn bedrock(&self) -> Option<&crate::api::bedrock_converse_stream::BedrockOptions> {
        match self {
            ApiStreamOptions::Bedrock(o) => Some(o),
            _ => None,
        }
    }

    /// The `google-generative-ai` options, if this is that variant.
    pub fn google(&self) -> Option<&crate::api::google_generative_ai::GoogleOptions> {
        match self {
            ApiStreamOptions::Google(o) => Some(o),
            _ => None,
        }
    }

    /// The `mistral-conversations` options, if this is that variant.
    pub fn mistral(&self) -> Option<&crate::api::mistral_conversations::MistralOptions> {
        match self {
            ApiStreamOptions::Mistral(o) => Some(o),
            _ => None,
        }
    }
}

impl StreamOptions {
    /// The carried `anthropic-messages` per-API options, if any.
    pub fn anthropic_options(&self) -> Option<&crate::api::anthropic_messages::AnthropicOptions> {
        self.api_options
            .as_ref()
            .and_then(ApiStreamOptions::anthropic)
    }

    /// The carried `openai-responses` per-API options, if any.
    pub fn openai_responses_options(
        &self,
    ) -> Option<&crate::api::openai_responses::OpenAiResponsesOptions> {
        self.api_options
            .as_ref()
            .and_then(ApiStreamOptions::openai_responses)
    }

    /// The carried `azure-openai-responses` per-API options, if any.
    pub fn azure_openai_responses_options(
        &self,
    ) -> Option<&crate::api::azure_openai_responses::AzureOpenAiResponsesOptions> {
        self.api_options
            .as_ref()
            .and_then(ApiStreamOptions::azure_openai_responses)
    }

    /// The carried `google-generative-ai` per-API options, if any.
    pub fn google_options(&self) -> Option<&crate::api::google_generative_ai::GoogleOptions> {
        self.api_options.as_ref().and_then(ApiStreamOptions::google)
    }

    /// The carried `mistral-conversations` per-API options, if any.
    pub fn mistral_options(&self) -> Option<&crate::api::mistral_conversations::MistralOptions> {
        self.api_options.as_ref().and_then(ApiStreamOptions::mistral)
    }
}

/// Fallback diagnostic stamped by [`StreamEvent::terminal`] when a caller routes a still-`pending`
/// message to a terminal without going through [`StreamEvent::end_of_stream`] (which supplies the
/// exact per-api text Pi throws). Reaching this string means a code path built a terminal from an
/// unfinished message — a bug in that path, surfaced rather than swallowed.
pub const PENDING_AT_TERMINAL: &str = "stream ended without a stop reason";

/// Terminal-`done` reason. Pi narrows the `done` event's `reason` to
/// `Extract<StopReason, "stop" | "length" | "toolUse" | "deferred">`
/// (`v0.84.1 ai/src/types.ts:527-531`; the same union without `"deferred"` at
/// `v0.83.0 ai/src/types.ts:464`), so cyrup mirrors that with a dedicated enum rather than the full
/// [`StopReason`] (arch-01 §3.3). `rename_all="camelCase"` makes the wire bytes byte-1:1 with the
/// matching [`StopReason`] values: `"stop"`, `"length"`, `"toolUse"`, `"deferred"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DoneReason {
    Stop,
    Length,
    ToolUse,
    /// The provider returned a durable handle instead of a completed turn. A **success** terminal
    /// in Pi's union — see [`cyrup_core::StopReason::Deferred`]. No cyrup wire api produces it
    /// today; it exists so the narrowing stays total against Pi's.
    Deferred,
}

/// Terminal-`error` reason. Pi narrows the `error` event's `reason` to
/// `Extract<StopReason, "aborted" | "error">` (Pi types.ts:465), mirrored here (arch-01 §3.3).
/// `rename_all="camelCase"` makes the wire bytes byte-1:1 with the matching [`StopReason`] values:
/// `"error"`, `"aborted"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorReason {
    Error,
    Aborted,
}

impl From<DoneReason> for StopReason {
    fn from(reason: DoneReason) -> Self {
        match reason {
            DoneReason::Stop => StopReason::Stop,
            DoneReason::Length => StopReason::Length,
            DoneReason::ToolUse => StopReason::ToolUse,
            DoneReason::Deferred => StopReason::Deferred,
        }
    }
}

impl From<ErrorReason> for StopReason {
    fn from(reason: ErrorReason) -> Self {
        match reason {
            ErrorReason::Error => StopReason::Error,
            ErrorReason::Aborted => StopReason::Aborted,
        }
    }
}

impl TryFrom<StopReason> for DoneReason {
    /// A non-`done` [`StopReason`] (`error`/`aborted`) carries the [`ErrorReason`] it maps to, so the
    /// caller can route it straight to the `error` terminal without a separate lookup (and without
    /// ever panicking).
    type Error = ErrorReason;

    fn try_from(reason: StopReason) -> Result<Self, ErrorReason> {
        match reason {
            StopReason::Stop => Ok(DoneReason::Stop),
            StopReason::Length => Ok(DoneReason::Length),
            StopReason::ToolUse => Ok(DoneReason::ToolUse),
            // `deferred` is in Pi's `done` extract (`v0.84.1 ai/src/types.ts:529`), NOT its `error`
            // one — a deferred turn is an accepted request, not a failure.
            StopReason::Deferred => Ok(DoneReason::Deferred),
            StopReason::Error => Err(ErrorReason::Error),
            StopReason::Aborted => Err(ErrorReason::Aborted),
            // `pending` is Pi's in-flight sentinel and is NOT in its `done` extract (types.ts:464).
            // Pi turns a surviving `"pending"` into a `throw`, and the catch pushes
            // `{type:"error", reason:"error"}` (anthropic-messages.ts:751-768) — so `error`, never
            // `aborted`. This arm is what makes `Pending` unable to reach a `done` event by
            // construction, not merely by convention.
            StopReason::Pending => Err(ErrorReason::Error),
        }
    }
}

/// One streaming event (func-01 §8.1; 1:1 with Pi `AssistantMessageEvent`, types.ts:453-465).
///
/// Ordering (func-01 §8.2): first event is `Start`; each content block at `content_index` follows
/// `*Start → (*Delta)* → *End`; exactly one terminal (`Done` or `Error`) closes the stream.
///
/// Every NON-terminal variant carries a `partial: AssistantMessage` — the live snapshot of the
/// message assembled so far (Pi `partial: AssistantMessage` on each event, types.ts:454-463;
/// func-01 R-01-022) — so consumers render the growing message without reconstructing it from
/// deltas. The terminals carry the `reason` discriminant plus the full message (Pi
/// `{type:"done", reason, message}` / `{type:"error", reason, error}`, types.ts:464-465).
// Serde so `cyrup-agent`'s `AgentEvent::MessageUpdate` (which carries a `StreamEvent` delta as
// `assistantMessageEvent`) can derive Serialize/Deserialize for the json/rpc wire (func-02
// R-02-009 / arch-02 §3.1). The `type` discriminant is byte-1:1 with Pi's `AssistantMessageEvent`
// literal tags (types.ts:453-465): `start`, `text_start`/`text_delta`/`text_end`,
// `thinking_start`/`thinking_delta`/`thinking_end`, `toolcall_start`/`toolcall_delta`/`toolcall_end`,
// `done`, `error`. These are lowercase-with-underscore (note `toolcall_*` has NO boundary between
// `tool` and `call`), so neither serde's `camelCase` NOR `snake_case` reproduces them — each
// non-`start`/`done`/`error` variant carries an explicit `#[serde(rename = "…")]`. Payload FIELDS
// stay camelCase (Pi `contentIndex`/`partial`/`toolCall`/…), via `rename_all_fields`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum StreamEvent {
    #[serde(rename = "start")]
    Start { partial: Arc<AssistantMessage> },
    #[serde(rename = "text_start")]
    TextStart {
        content_index: usize,
        partial: Arc<AssistantMessage>,
    },
    #[serde(rename = "text_delta")]
    TextDelta {
        content_index: usize,
        delta: String,
        partial: Arc<AssistantMessage>,
    },
    #[serde(rename = "text_end")]
    TextEnd {
        content_index: usize,
        content: String,
        partial: Arc<AssistantMessage>,
    },
    #[serde(rename = "thinking_start")]
    ThinkingStart {
        content_index: usize,
        partial: Arc<AssistantMessage>,
    },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta {
        content_index: usize,
        delta: String,
        partial: Arc<AssistantMessage>,
    },
    #[serde(rename = "thinking_end")]
    ThinkingEnd {
        content_index: usize,
        content: String,
        partial: Arc<AssistantMessage>,
    },
    #[serde(rename = "toolcall_start")]
    ToolCallStart {
        content_index: usize,
        partial: Arc<AssistantMessage>,
    },
    #[serde(rename = "toolcall_delta")]
    ToolCallDelta {
        content_index: usize,
        delta: String,
        partial: Arc<AssistantMessage>,
    },
    /// Pi `toolcall_end` (types.ts:463). The `tool_call` field carries Pi's `type:"toolCall"`
    /// discriminant first (Pi `ToolCall.type`, types.ts:345) because [`ToolCall`] now self-tags via
    /// its own [`serde::Serialize`] impl — the single source of the discriminant. Deserialize uses
    /// `ToolCall`'s derived impl, which tolerates the extra `type` key.
    #[serde(rename = "toolcall_end")]
    ToolCallEnd {
        content_index: usize,
        tool_call: ToolCall,
        partial: Arc<AssistantMessage>,
    },
    /// Terminal: normal completion. `reason` ∈ {stop, length, toolUse} (Pi narrows the `done` reason
    /// to `Extract<StopReason,"stop"|"length"|"toolUse">`, types.ts:464); `message.stop_reason`
    /// matches.
    #[serde(rename = "done")]
    Done {
        reason: DoneReason,
        message: Arc<AssistantMessage>,
    },
    /// Terminal: error/abort. `reason` ∈ {error, aborted} (Pi narrows the `error` reason to
    /// `Extract<StopReason,"aborted"|"error">`, types.ts:465); the final message is keyed `error`.
    #[serde(rename = "error")]
    Error {
        reason: ErrorReason,
        error: Arc<AssistantMessage>,
    },
}

impl StreamEvent {
    /// Build the correct terminal event for a final `message`, narrowing `message.stop_reason` into a
    /// [`DoneReason`] (`done` terminal) or an [`ErrorReason`] (`error` terminal). The mapping is total
    /// and never panics: `error`/`aborted` route to the `error` terminal, every other settled reason
    /// to the `done` terminal — matching Pi's `done`/`error` split (types.ts:464-465).
    ///
    /// A still-[`StopReason::Pending`] message is **normalized in place** to
    /// [`StopReason::Error`] before routing, exactly as Pi's catch does
    /// (`output.stopReason = signal.aborted ? "aborted" : "error"`, anthropic-messages.ts:765-768;
    /// the abort case never reaches here because every decoder emits its own aborted terminal).
    /// This is the second half of the structural guarantee that `pending` never escapes a
    /// non-terminal `partial`: no caller can hand a `Pending` message to a terminal and have it
    /// survive into `message_end`, the settled transcript, or a session file. `error_message` is
    /// filled only if the caller left it empty, so a decoder that already recorded the per-api
    /// diagnostic (see [`Self::end_of_stream`]) keeps its exact Pi-matching text.
    pub fn terminal(mut message: AssistantMessage) -> Self {
        if message.stop_reason == StopReason::Pending {
            message.stop_reason = StopReason::Error;
            if message.error_message.as_deref().unwrap_or("").is_empty() {
                message.error_message = Some(PENDING_AT_TERMINAL.to_string());
            }
        }
        match DoneReason::try_from(message.stop_reason) {
            Ok(reason) => StreamEvent::Done { reason, message: Arc::new(message) },
            Err(reason) => StreamEvent::Error {
                reason,
                error: Arc::new(message),
            },
        }
    }

    /// Build the terminal event for a stream that reached **end of input**, given the settled stop
    /// reason the provider actually delivered.
    ///
    /// This is the single seam that encodes Pi's truncated-stream rule, and every wire-API decoder
    /// funnels its end-of-stream path through it so the rule cannot be forgotten in one converter
    /// and honoured in another (which is exactly how PROV-010 arose: `openai-completions` guarded,
    /// `anthropic-messages` and `google-generative-ai` defaulted a stop-reason-less stream to a
    /// clean `stop`, transcribing a truncated turn as a completed one with no diagnostic).
    ///
    /// `delivered` is `None` when the stream ended without the provider ever sending a terminal
    /// stop reason — cyrup's spelling of Pi's still-`"pending"` output, which every Pi stream
    /// function turns into a `throw` and therefore an `{type:"error", reason:"error"}` terminal:
    ///
    /// | Pi source | throw text |
    /// |---|---|
    /// | `anthropic-messages.ts:751-753` | `Anthropic stream ended without a stop reason` |
    /// | `google-generative-ai.ts:266-268` | `Google stream ended without a finish reason` |
    /// | `mistral-conversations.ts:88-90` | `Mistral stream ended without a finish reason` |
    /// | `openai-responses.ts:170-172` | `OpenAI Responses stream ended without a stop reason` |
    /// | `openai-completions.ts:580-582` | `Stream ended without finish_reason` |
    ///
    /// `truncated` carries that per-api text; it lands in `error_message`, matching Pi's catch block
    /// (`output.errorMessage = error.message`). A `Some(_)` settled reason is used verbatim, so an
    /// already-settled `error`/`aborted` keeps the `error_message` the decoder recorded.
    ///
    /// `Some(StopReason::Pending)` is treated identically to `None` — Pi's guard is a value test on
    /// the sentinel (`output.stopReason === "pending"`), not a "was anything assigned" test, so a
    /// decoder that tracks its reason as a plain [`StopReason`] rather than an `Option` gets the
    /// same answer.
    pub fn end_of_stream(
        mut message: AssistantMessage,
        delivered: Option<StopReason>,
        truncated: &str,
    ) -> Self {
        match delivered {
            Some(reason) if reason != StopReason::Pending => {
                message.stop_reason = reason;
            }
            _ => {
                message.stop_reason = StopReason::Error;
                message.error_message = Some(truncated.to_string());
            }
        }
        StreamEvent::terminal(message)
    }

    /// The final message iff this is a terminal event (func-01 R-01-023).
    pub fn terminal_message(&self) -> Option<&Arc<AssistantMessage>> {
        match self {
            StreamEvent::Done { message, .. } => Some(message),
            StreamEvent::Error { error, .. } => Some(error),
            _ => None,
        }
    }

    /// The per-event `partial` snapshot for a non-terminal event (Pi `event.partial`); `None` for
    /// the terminals (which carry the full `message`/`error` instead).
    pub fn partial(&self) -> Option<&Arc<AssistantMessage>> {
        match self {
            StreamEvent::Start { partial }
            | StreamEvent::TextStart { partial, .. }
            | StreamEvent::TextDelta { partial, .. }
            | StreamEvent::TextEnd { partial, .. }
            | StreamEvent::ThinkingStart { partial, .. }
            | StreamEvent::ThinkingDelta { partial, .. }
            | StreamEvent::ThinkingEnd { partial, .. }
            | StreamEvent::ToolCallStart { partial, .. }
            | StreamEvent::ToolCallDelta { partial, .. }
            | StreamEvent::ToolCallEnd { partial, .. } => Some(partial),
            StreamEvent::Done { .. } | StreamEvent::Error { .. } => None,
        }
    }
}

/// Drain a stream to its terminal event and return the final message (func-01 R-01-005/023).
/// Never panics: a stream that ends without a terminal event yields a synthesized error message.
pub async fn collect_message(mut stream: EventStream<StreamEvent>) -> AssistantMessage {
    let mut last: Option<AssistantMessage> = None;
    while let Some(ev) = stream.next().await {
        if let Some(msg) = ev.terminal_message() {
            last = Some((**msg).clone());
        }
    }
    last.unwrap_or_else(|| {
        AssistantMessage::errored(
            ProviderId::from("unknown"),
            "unknown",
            None,
            StopReason::Error,
            "stream ended without a terminal event",
        )
    })
}

/// A push-driven [`StreamEvent`] stream that resolves to the final [`AssistantMessage`] — the
/// extension-facing authoring path (1:1 with Pi `AssistantMessageEventStream` +
/// `createAssistantMessageEventStream`, event-stream.ts:69-88). Specializes cyrup-core's generic
/// [`cyrup_core::FinalizingStream`] over `StreamEvent`/`AssistantMessage`, keying completion on the
/// `Done`/`Error` terminals and extracting their message (Pi `isComplete`/`extractResult`).
pub type AssistantMessageEventStream = cyrup_core::FinalizingStream<StreamEvent, AssistantMessage>;

/// The producer half an extension drives to author an [`AssistantMessageEventStream`].
pub type AssistantMessageEventSink = cyrup_core::FinalizingSink<StreamEvent, AssistantMessage>;

/// Create an [`AssistantMessageEventStream`] for extensions to drive (Pi
/// `createAssistantMessageEventStream()`, event-stream.ts:85-88). The sink's `push`/`end` feed the
/// stream; `result()` resolves to the terminal message (or a synthesized error if it ends without
/// a terminal, matching [`collect_message`]'s no-panic policy).
pub fn create_assistant_message_event_stream()
-> (AssistantMessageEventSink, AssistantMessageEventStream) {
    cyrup_core::finalizing_channel(
        |e: &StreamEvent| matches!(e, StreamEvent::Done { .. } | StreamEvent::Error { .. }),
        |e: &StreamEvent| {
            e.terminal_message()
                .map(|m| (**m).clone())
                .unwrap_or_else(synth_terminal_less_message)
        },
        synth_terminal_less_message,
    )
}

/// The synthesized final message for a stream that ended without a terminal (shared by
/// [`collect_message`] and [`create_assistant_message_event_stream`]).
fn synth_terminal_less_message() -> AssistantMessage {
    AssistantMessage::errored(
        ProviderId::from("unknown"),
        "unknown",
        None,
        StopReason::Error,
        "stream ended without a terminal event",
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use cyrup_core::{ToolCallId, Usage};

    fn empty_partial() -> Arc<AssistantMessage> {
        Arc::new(AssistantMessage {
            content: Vec::new(),
            provider: ProviderId::from("faux"),
            model: "faux-1".into(),
            api: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        })
    }

    /// Gap 1: every `type` discriminant is byte-1:1 with Pi's `AssistantMessageEvent` literals
    /// (types.ts:453-465) — in particular the underscored `text_*`/`thinking_*`/`toolcall_*` tags,
    /// not serde's camelCase.
    #[test]
    fn stream_event_type_tags_are_pi_literals() {
        let p = empty_partial();
        let cases = [
            (StreamEvent::Start { partial: p.clone() }, "start"),
            (
                StreamEvent::TextStart {
                    content_index: 0,
                    partial: p.clone(),
                },
                "text_start",
            ),
            (
                StreamEvent::TextDelta {
                    content_index: 0,
                    delta: "d".into(),
                    partial: p.clone(),
                },
                "text_delta",
            ),
            (
                StreamEvent::TextEnd {
                    content_index: 0,
                    content: "c".into(),
                    partial: p.clone(),
                },
                "text_end",
            ),
            (
                StreamEvent::ThinkingStart {
                    content_index: 0,
                    partial: p.clone(),
                },
                "thinking_start",
            ),
            (
                StreamEvent::ThinkingDelta {
                    content_index: 0,
                    delta: "d".into(),
                    partial: p.clone(),
                },
                "thinking_delta",
            ),
            (
                StreamEvent::ThinkingEnd {
                    content_index: 0,
                    content: "c".into(),
                    partial: p.clone(),
                },
                "thinking_end",
            ),
            (
                StreamEvent::ToolCallStart {
                    content_index: 0,
                    partial: p.clone(),
                },
                "toolcall_start",
            ),
            (
                StreamEvent::ToolCallDelta {
                    content_index: 0,
                    delta: "d".into(),
                    partial: p.clone(),
                },
                "toolcall_delta",
            ),
            (
                StreamEvent::Done {
                    reason: DoneReason::Stop,
                    message: p.clone(),
                },
                "done",
            ),
            (
                StreamEvent::Error {
                    reason: ErrorReason::Error,
                    error: p.clone(),
                },
                "error",
            ),
        ];
        for (ev, tag) in cases {
            let v = serde_json::to_value(&ev).expect("serialize");
            assert_eq!(v["type"], tag, "wrong tag for {ev:?}");
            // Payload fields stay camelCase (Pi `contentIndex`/`partial`).
            let back: StreamEvent = serde_json::from_value(v).expect("roundtrip");
            assert_eq!(back, ev);
        }
    }

    /// Gap 2: `toolcall_end.toolCall` carries Pi's `type:"toolCall"` discriminant first, then
    /// `id`/`name`/`arguments`/`thoughtSignature?` in Pi declaration order (types.ts:344-350,463) —
    /// with no duplicate `type` key — and round-trips.
    #[test]
    fn toolcall_end_tool_call_carries_type_discriminant() {
        let ev = StreamEvent::ToolCallEnd {
            content_index: 0,
            tool_call: ToolCall {
                id: ToolCallId::from("tc1"),
                name: "read".into(),
                arguments: serde_json::Map::new().into(),
                thought_signature: None,
            },
            partial: empty_partial(),
        };
        let s = serde_json::to_string(&ev).expect("serialize");
        assert_eq!(
            s.matches("\"type\"").count(),
            2,
            "event tag + toolCall tag, no dup: {s}"
        );
        let v: serde_json::Value = serde_json::from_str(&s).expect("json");
        assert_eq!(v["type"], "toolcall_end");
        assert_eq!(v["toolCall"]["type"], "toolCall");
        assert_eq!(v["toolCall"]["id"], "tc1");
        assert_eq!(v["toolCall"]["name"], "read");
        assert!(v["toolCall"]["arguments"].is_object());
        // `type` is emitted first, byte-1:1 with Pi's `ToolCall` field order.
        let tc = &s[s.find("\"toolCall\":{").expect("toolCall obj")..];
        assert!(
            tc.starts_with("\"toolCall\":{\"type\":\"toolCall\""),
            "{tc}"
        );
        let back: StreamEvent = serde_json::from_str(&s).expect("roundtrip");
        assert_eq!(back, ev);
    }

    /// Gap 3: the terminal `reason` is narrowed to Pi's `Extract<StopReason,…>` subsets
    /// (types.ts:464-465: `done.reason ∈ {"stop","length","toolUse"}`,
    /// `error.reason ∈ {"error","aborted"}`) yet the emitted bytes stay byte-1:1 with the old full
    /// [`StopReason`] strings — and every value round-trips.
    #[test]
    fn terminal_reasons_are_pi_narrowed_subsets_and_byte_stable() {
        let p = empty_partial();
        // `done` reasons serialize EXACTLY as the matching `StopReason` did before the narrowing.
        let done_cases = [
            (DoneReason::Stop, StopReason::Stop, "stop"),
            (DoneReason::Length, StopReason::Length, "length"),
            (DoneReason::ToolUse, StopReason::ToolUse, "toolUse"),
        ];
        for (reason, stop, wire) in done_cases {
            let ev = StreamEvent::Done {
                reason,
                message: p.clone(),
            };
            let v = serde_json::to_value(&ev).expect("serialize");
            assert_eq!(v["type"], "done");
            assert_eq!(v["reason"], wire, "done reason wire byte for {reason:?}");
            // Byte-identical to the full-`StopReason` encoding it replaced.
            assert_eq!(v["reason"], serde_json::to_value(stop).expect("stop"));
            let back: StreamEvent = serde_json::from_value(v).expect("roundtrip");
            assert_eq!(back, ev);
        }
        // `error` reasons likewise.
        let err_cases = [
            (ErrorReason::Error, StopReason::Error, "error"),
            (ErrorReason::Aborted, StopReason::Aborted, "aborted"),
        ];
        for (reason, stop, wire) in err_cases {
            let ev = StreamEvent::Error {
                reason,
                error: p.clone(),
            };
            let v = serde_json::to_value(&ev).expect("serialize");
            assert_eq!(v["type"], "error");
            assert_eq!(v["reason"], wire, "error reason wire byte for {reason:?}");
            assert_eq!(v["reason"], serde_json::to_value(stop).expect("stop"));
            let back: StreamEvent = serde_json::from_value(v).expect("roundtrip");
            assert_eq!(back, ev);
        }
    }

    /// `StreamEvent::terminal` routes by `stop_reason`: stop/length/toolUse → `done` with the
    /// matching [`DoneReason`]; error/aborted → `error` with the matching [`ErrorReason`]. Total and
    /// never panics.
    #[test]
    fn terminal_routes_stop_reason_without_panic() {
        let mk = |stop: StopReason| {
            let mut m = (*empty_partial()).clone();
            m.stop_reason = stop;
            m
        };
        match StreamEvent::terminal(mk(StopReason::Stop)) {
            StreamEvent::Done {
                reason: DoneReason::Stop,
                ..
            } => {}
            other => panic!("expected done/stop, got {other:?}"),
        }
        match StreamEvent::terminal(mk(StopReason::ToolUse)) {
            StreamEvent::Done {
                reason: DoneReason::ToolUse,
                ..
            } => {}
            other => panic!("expected done/toolUse, got {other:?}"),
        }
        match StreamEvent::terminal(mk(StopReason::Error)) {
            StreamEvent::Error {
                reason: ErrorReason::Error,
                ..
            } => {}
            other => panic!("expected error/error, got {other:?}"),
        }
        match StreamEvent::terminal(mk(StopReason::Aborted)) {
            StreamEvent::Error {
                reason: ErrorReason::Aborted,
                ..
            } => {}
            other => panic!("expected error/aborted, got {other:?}"),
        }
    }
}

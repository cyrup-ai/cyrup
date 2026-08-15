//! cyrup-provider — vendor-neutral LLM layer (arch-01; conformance: func-01).
//!
//! Providers/models/collection, auth resolution + credential store, the streaming event model,
//! tool validation, cross-provider handoff, usage/cost, the faux provider, the compat matrix.
//!
//! This slice implements the core data model (`Model`/`Context`/`StreamEvent`/`StreamOptions`),
//! the `Provider` trait, the `collect_message` helper, and the **faux provider**, plus the base
//! infrastructure real `ApiImpl`s sit on (arch-01 §7.1 direct-wire): the [`error`] taxonomy, the
//! [`auth`] credential/resolution engine, the [`stream::sse`] HTTP+SSE transport, the [`api`]
//! `ApiImpl` trait + lazy `ApiRegistry`, the reusable [`wire::WireProvider`], [`usage`] cost, and a
//! static [`catalog`] seam. Concrete vendor `ApiImpl`s (anthropic-messages / openai-completions)
//! and cross-provider handoff land next.
#![forbid(unsafe_code)]

pub mod api;
pub mod auth;
pub mod cache_stats;
pub mod catalog;
pub mod collection;
pub mod config_provider;
pub mod context;
pub mod env_api_keys;
pub mod error;
pub mod images;
pub mod legacy_api_aliases;
pub mod model;
pub mod models_store;
pub mod provider;
pub mod providers;
pub mod remote_catalog;
pub mod session_resources;
pub mod stream;
/// The zero-model stand-in installed when nothing is authenticated (pi `main.ts:852-855`
/// @v0.83.0). **Always compiled** — unlike [`faux`], this is production code.
pub mod unconfigured;
pub mod usage;
pub mod utils;
pub mod validate;
pub mod wire;

// The scripted test double. `feature = "faux"` must reach this crate ONLY through a
// `[dev-dependencies]` edge — see `Cargo.toml`'s `[features]` comment and PROV-052. A normal
// (`cargo tree --edges normal`) build of the shipped binary compiles neither this module nor any
// path that can select it; the no-credential state resolves to [`unconfigured`] instead.
#[cfg(any(test, feature = "faux"))]
pub mod faux;

pub use api::anthropic_messages::AnthropicMessagesApi;
pub use api::azure_openai_responses::AzureOpenAiResponsesApi;
pub use api::google_generative_ai::GoogleGenerativeAiApi;
pub use api::google_vertex::GoogleVertexApi;
pub use api::mistral_conversations::MistralConversationsApi;
pub use api::openai_completions::OpenAiCompletionsApi;
pub use api::openai_responses::OpenAiResponsesApi;
pub use api::{
    ApiFactory, ApiImpl, ApiRegistry, EventSink, builtin_registry, channel, register_builtins,
};
pub use auth::{
    ApiKeyAuth, AuthContext, AuthOverrides, AuthResult, Credential, CredentialInfo,
    CredentialStore, CredentialType, EnvAuthContext, InMemoryCredentialStore, ModelAuth, ModifyFn,
    OAuthAuth, ProviderAuth, ProviderEnv, env_key, keyless_local, resolve_provider_auth,
};
pub use cache_stats::{
    CACHE_TTL_MS, CacheMiss, CacheScan, CacheScanEntry, CacheWasteTotals, ModelPriceSource,
    NOISE_FLOOR_TOKENS, NoPrices, collect_cache_misses, compute_cache_waste, detect_cache_miss,
};
pub use catalog::{builtin_catalog, load_catalog};
pub use collection::{
    CreateModelsOptions, EXTENDED_THINKING_LEVELS, Models, ModelsRefreshOptions,
    ModelsRefreshResult, clamp_thinking_level, create_models, get_supported_thinking_levels,
    has_api, models_are_equal,
};
pub use config_provider::ConfigProvider;
pub use context::{
    ConstrainedSampling, ConstrainedSamplingConfig, Context, GrammarVariants, StrictSampling,
    ToolDef,
};
pub use cyrup_core::ApiId;
pub use env_api_keys::{
    AUTHENTICATED_SENTINEL, api_key_env_vars, find_env_keys, get_env_api_key,
    get_provider_env_value,
};
pub use error::{AuthError, BoxErr, ProviderError};
pub use images::{
    AssistantImages, CreateImagesProviderOptions, ImagesApiImpl, ImagesApiRegistry, ImagesContext,
    ImagesModel, ImagesModels, ImagesOptions, ImagesProvider, ImagesStopReason, OPENROUTER_IMAGES,
    create_images_models, create_images_provider, generate_images, get_image_model,
    get_image_models, get_image_providers, image_models, images_builtin_registry,
    openrouter_image_models, register_images_builtins,
};
pub use model::{Modality, Model, ModelCost, ModelCostTier};
pub use models_store::{
    InMemoryModelsStore, ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions,
    ProviderModelsStore,
};
pub use remote_catalog::{
    CatalogOverlay, DEFAULT_CATALOG_BASE_URL, REMOTE_CATALOG_REFRESH_INTERVAL_MS, RefreshOptions,
    RemoteCatalog, RemoteCatalogProvider, merge_models, parse_catalog, remote_models,
};
pub use provider::{Provider, RefreshModelsContext};
pub use providers::all::{
    BUILTIN_CATALOG_MANIFEST_JSON, all_images_providers, all_providers, all_providers_with,
    all_providers_with_overlay, builtin_model_data_generated_at, default_images_models,
    default_models,
};
pub use providers::fleet::{FLEET, FleetSpec, fleet_providers_with, fleet_spec};
pub use providers::{
    ANTHROPIC_BASE_URL, ANTHROPIC_FLEET, ANTHROPIC_PROVIDER_ID, AnthropicFleetSpec, anthropic_auth,
    anthropic_fleet_providers_with, anthropic_fleet_spec, anthropic_models, anthropic_provider,
    anthropic_provider_with,
};
pub use providers::{
    AZURE_OPENAI_API_KEY, AZURE_OPENAI_RESPONSES_PROVIDER_ID, azure_openai_responses_auth,
    azure_openai_responses_models, azure_openai_responses_provider,
    azure_openai_responses_provider_with,
};
pub use providers::{
    CLOUDFLARE_AI_GATEWAY_PROVIDER_ID, CLOUDFLARE_WORKERS_AI_PROVIDER_ID,
    cloudflare_ai_gateway_auth, cloudflare_ai_gateway_models, cloudflare_ai_gateway_provider,
    cloudflare_ai_gateway_provider_with, cloudflare_workers_ai_auth, cloudflare_workers_ai_models,
    cloudflare_workers_ai_provider, cloudflare_workers_ai_provider_with,
};
pub use providers::{
    FIREWORKS_BASE_URL, FIREWORKS_PROVIDER_ID, fireworks_auth, fireworks_models,
    fireworks_provider, fireworks_provider_with,
};
pub use providers::{
    GOOGLE_BASE_URL, GOOGLE_PROVIDER_ID, google_auth, google_models, google_provider,
    google_provider_with,
};
pub use providers::{
    MISTRAL_BASE_URL, MISTRAL_PROVIDER_ID, mistral_auth, mistral_models, mistral_provider,
    mistral_provider_with,
};
pub use providers::{
    OPENAI_BASE_URL, OPENAI_PROVIDER_ID, openai_auth, openai_models, openai_provider,
    openai_provider_with,
};
pub use providers::{
    OPENCODE_GO_PROVIDER_ID, opencode_go_auth, opencode_go_models, opencode_go_provider,
    opencode_go_provider_with,
};
pub use providers::{
    OPENCODE_PROVIDER_ID, opencode_auth, opencode_models, opencode_provider, opencode_provider_with,
};
pub use providers::{
    TOGETHER_BASE_URL, together_models, together_provider, together_provider_with,
};
pub use providers::builtin_provider_oauth;
pub use providers::{openrouter_images_auth, openrouter_images_provider};
pub use stream::sse::{
    DEFAULT_HTTP_IDLE_TIMEOUT_MS, OnRequest, OnResponse, SseFrame, SseRequest, build_client,
    build_client_for, build_client_for_target, build_client_with_proxy,
    configure_http_idle_timeout, configure_http_proxy, configured_http_proxy, decode_sse_bytes,
    http_idle_timeout_ms, open_sse,
};
pub use utils::error_body::{MAX_PROVIDER_ERROR_BODY_CHARS, truncate_error_text};
pub use utils::provider_retry::{DEFAULT_MAX_RETRY_DELAY_MS, ProviderRetry};
pub use stream::{
    AssistantMessageEventSink, AssistantMessageEventStream, CacheRetention, OnPayload,
    OnResponseHook, ProviderResponse, StreamEvent, StreamOptions, ToolChoice, Transport,
    collect_message, create_assistant_message_event_stream,
};
pub use usage::{apply_cost, compute_cost};
pub use utils::estimate::{
    ContextUsageEstimate, calculate_context_tokens, estimate_context_tokens,
    estimate_message_tokens, estimate_text_tokens,
};
pub use utils::json_parse::{parse_streaming_json, parse_streaming_json_object, repair_json};
pub use utils::node_http_proxy::{
    ProxyError, UNSUPPORTED_PROXY_PROTOCOL_MESSAGE, resolve_http_proxy_url_for_target,
};
pub use utils::overflow::{is_context_overflow, overflow_patterns};
pub use utils::refresh::RefreshDedup;
pub use utils::retry::{
    RetryObserver, RetryPolicy, is_retryable_assistant_error, retry_assistant_call,
};
pub use utils::simple_options::{
    SimpleStreamOptions, ThinkingBudgets, adjust_max_tokens_for_thinking, build_base_options,
    clamp_max_tokens_to_context, clamp_reasoning,
};
pub use validate::{ToolValidationError, validate_named_tool_call, validate_tool_call};
pub use wire::WireProvider;

/// Header overlay: a `None` value suppresses a would-be default header (arch-01 §3.1).
pub type HeaderMap = std::collections::BTreeMap<String, Option<String>>;

/// Known wire-protocol ids (arch-01 §3.1). `ApiId` accepts custom strings too.
pub mod known_api {
    pub const ANTHROPIC_MESSAGES: &str = "anthropic-messages";
    pub const OPENAI_COMPLETIONS: &str = "openai-completions";
    pub const OPENAI_RESPONSES: &str = "openai-responses";
    pub const AZURE_OPENAI_RESPONSES: &str = "azure-openai-responses";
    pub const GOOGLE_GENERATIVE_AI: &str = "google-generative-ai";
    /// pi `KnownApi` (`ai/src/types.ts:25` @v0.83.0, `:26` @v0.84.1). Ported by PROV-030;
    /// implemented in [`crate::api::google_vertex`].
    pub const GOOGLE_VERTEX: &str = "google-vertex";
    pub const MISTRAL_CONVERSATIONS: &str = "mistral-conversations";
    pub const BEDROCK_CONVERSE_STREAM: &str = "bedrock-converse-stream";
    /// pi `KnownApi` (`ai/src/types.ts:16-26`). Ported in the unported-work sweep.
    pub const PI_MESSAGES: &str = "pi-messages";
    /// pi `KnownApi` (`ai/src/types.ts:16-26`). Ported in the unported-work sweep.
    pub const OPENAI_CODEX_RESPONSES: &str = "openai-codex-responses";
}

#[cfg(test)]
mod tests;

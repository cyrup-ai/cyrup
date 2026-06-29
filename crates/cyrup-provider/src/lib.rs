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
pub mod catalog;
pub mod context;
pub mod error;
pub mod model;
pub mod provider;
pub mod providers;
pub mod stream;
pub mod usage;
pub mod validate;
pub mod wire;

#[cfg(any(test, feature = "faux"))]
pub mod faux;

pub use api::openai_completions::OpenAiCompletionsApi;
pub use api::{
    builtin_registry, channel, register_builtins, ApiFactory, ApiImpl, ApiRegistry, EventSink,
};
pub use providers::{together_models, together_provider, together_provider_with, TOGETHER_BASE_URL};
pub use auth::{
    env_key, keyless_local, resolve_provider_auth, ApiKeyAuth, AuthContext, AuthOverrides,
    AuthResult, Credential, CredentialStore, EnvAuthContext, InMemoryCredentialStore, ModelAuth,
    ModifyFn, OAuthAuth, ProviderAuth, ProviderEnv,
};
pub use catalog::{load_catalog, seed_catalog};
pub use context::{Context, ToolDef};
pub use cyrup_core::ApiId;
pub use error::{AuthError, BoxErr, ProviderError};
pub use model::{Modality, Model, ModelCost};
pub use provider::Provider;
pub use stream::sse::{build_client, decode_sse_bytes, open_sse, OnRequest, OnResponse, SseFrame, SseRequest};
pub use stream::{
    collect_message, create_assistant_message_event_stream, AssistantMessageEventSink,
    AssistantMessageEventStream, CacheRetention, StreamEvent, StreamOptions, ToolChoice,
};
pub use usage::{apply_cost, compute_cost};
pub use validate::{validate_named_tool_call, validate_tool_call, ToolValidationError};
pub use wire::WireProvider;

/// Header overlay: a `None` value suppresses a would-be default header (arch-01 §3.1).
pub type HeaderMap = std::collections::BTreeMap<String, Option<String>>;

/// Known wire-protocol ids (arch-01 §3.1). `ApiId` accepts custom strings too.
pub mod known_api {
    pub const ANTHROPIC_MESSAGES: &str = "anthropic-messages";
    pub const OPENAI_COMPLETIONS: &str = "openai-completions";
    pub const OPENAI_RESPONSES: &str = "openai-responses";
    pub const GOOGLE_GENERATIVE_AI: &str = "google-generative-ai";
    pub const BEDROCK_CONVERSE_STREAM: &str = "bedrock-converse-stream";
}

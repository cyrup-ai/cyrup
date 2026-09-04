//! Concrete vendor providers (arch-01 §5). Each is a [`crate::wire::WireProvider`] = a catalog + an
//! auth strategy + an api mapping over the shared [`crate::api::ApiRegistry`].

pub mod all;
pub mod amazon_bedrock;
pub mod anthropic;
pub mod azure_openai_responses;
pub mod builtin_oauth;
pub mod cloudflare;
pub mod fireworks;
pub mod fleet;
pub mod github_copilot;
pub mod google;
pub mod google_vertex;
pub mod mistral;
pub mod openai;
pub mod openai_codex;
pub mod opencode;
pub mod opencode_go;
pub mod openrouter_images;
pub mod together;

pub use all::{
    BUILTIN_CATALOG_MANIFEST_JSON, all_images_providers, all_providers, all_providers_with,
    all_providers_with_overlay, builtin_model_data_generated_at, default_images_models,
    default_models,
};
pub use amazon_bedrock::*;
pub use anthropic::{
    ANTHROPIC_BASE_URL, ANTHROPIC_FLEET, ANTHROPIC_PROVIDER_ID, AnthropicFleetSpec, anthropic_auth,
    anthropic_fleet_providers_with, anthropic_fleet_spec, anthropic_models, anthropic_provider,
    anthropic_provider_with,
};
pub use azure_openai_responses::{
    AZURE_OPENAI_API_KEY, AZURE_OPENAI_RESPONSES_PROVIDER_ID, azure_openai_responses_auth,
    azure_openai_responses_models, azure_openai_responses_provider,
    azure_openai_responses_provider_with,
};
pub use builtin_oauth::builtin_provider_oauth;
pub use cloudflare::{
    CLOUDFLARE_AI_GATEWAY_PROVIDER_ID, CLOUDFLARE_WORKERS_AI_PROVIDER_ID,
    cloudflare_ai_gateway_auth, cloudflare_ai_gateway_models, cloudflare_ai_gateway_provider,
    cloudflare_ai_gateway_provider_with, cloudflare_workers_ai_auth, cloudflare_workers_ai_models,
    cloudflare_workers_ai_provider, cloudflare_workers_ai_provider_with,
};
pub use fireworks::{
    FIREWORKS_BASE_URL, FIREWORKS_PROVIDER_ID, fireworks_auth, fireworks_models,
    fireworks_provider, fireworks_provider_with,
};
pub use github_copilot::{
    GITHUB_COPILOT_BASE_URL, GITHUB_COPILOT_PROVIDER_ID, github_copilot_auth,
    github_copilot_models, github_copilot_provider, github_copilot_provider_with,
};
pub use google::{
    GOOGLE_BASE_URL, GOOGLE_PROVIDER_ID, google_auth, google_models, google_provider,
    google_provider_with,
};
pub use google_vertex::*;
pub use mistral::{
    MISTRAL_BASE_URL, MISTRAL_PROVIDER_ID, mistral_auth, mistral_models, mistral_provider,
    mistral_provider_with,
};
pub use openai::{
    OPENAI_BASE_URL, OPENAI_PROVIDER_ID, openai_auth, openai_models, openai_provider,
    openai_provider_with,
};
pub use openai_codex::*;
pub use opencode::{
    OPENCODE_PROVIDER_ID, opencode_auth, opencode_models, opencode_provider, opencode_provider_with,
};
pub use opencode_go::{
    OPENCODE_GO_PROVIDER_ID, opencode_go_auth, opencode_go_models, opencode_go_provider,
    opencode_go_provider_with,
};
pub use openrouter_images::{openrouter_images_auth, openrouter_images_provider};
pub use together::{
    TOGETHER_BASE_URL, together_auth, together_models, together_provider, together_provider_with,
};

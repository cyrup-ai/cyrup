//! Concrete vendor providers (arch-01 §5). Each is a [`crate::wire::WireProvider`] = a catalog + an
//! auth strategy + an api mapping over the shared [`crate::api::ApiRegistry`].

pub mod anthropic;
pub mod cloudflare;
pub mod fireworks;
pub mod fleet;
pub mod opencode_go;
pub mod openai;
pub mod together;

pub use anthropic::{
    anthropic_auth, anthropic_fleet_providers_with, anthropic_fleet_spec, anthropic_models,
    anthropic_provider, anthropic_provider_with, AnthropicFleetSpec, ANTHROPIC_BASE_URL,
    ANTHROPIC_FLEET, ANTHROPIC_PROVIDER_ID,
};
pub use fireworks::{
    fireworks_auth, fireworks_models, fireworks_provider, fireworks_provider_with,
    FIREWORKS_BASE_URL, FIREWORKS_PROVIDER_ID,
};
pub use opencode_go::{
    opencode_go_auth, opencode_go_models, opencode_go_provider, opencode_go_provider_with,
    OPENCODE_GO_PROVIDER_ID,
};
pub use openai::{
    openai_auth, openai_models, openai_provider, openai_provider_with, OPENAI_BASE_URL,
    OPENAI_PROVIDER_ID,
};
pub use cloudflare::{
    cloudflare_workers_ai_auth, cloudflare_workers_ai_models, cloudflare_workers_ai_provider,
    cloudflare_workers_ai_provider_with, CLOUDFLARE_WORKERS_AI_PROVIDER_ID,
};
pub use together::{
    together_auth, together_models, together_provider, together_provider_with, TOGETHER_BASE_URL,
};

//! cyrup-provider — vendor-neutral LLM layer (arch-01; conformance: func-01).
//!
//! Providers/models/collection, auth resolution + credential store, the streaming event model,
//! tool validation, cross-provider handoff, usage/cost, the faux provider, the compat matrix.
//!
//! This slice implements the core data model (`Model`/`Context`/`StreamEvent`/`StreamOptions`),
//! the `Provider` trait, the `collect_message` helper, and the **faux provider** so the agent
//! runtime (arch-02) and sessions (arch-04) can be built and tested with no network. The concrete
//! vendor API implementations, auth, and handoff transforms land next.

pub mod context;
pub mod model;
pub mod provider;
pub mod stream;

#[cfg(any(test, feature = "faux"))]
pub mod faux;

pub use context::{Context, ToolDef};
pub use cyrup_core::ApiId;
pub use model::{Modality, Model, ModelCost};
pub use provider::Provider;
pub use stream::{collect_message, CacheRetention, StreamEvent, StreamOptions};

/// Known wire-protocol ids (arch-01 §3.1). `ApiId` accepts custom strings too.
pub mod known_api {
    pub const ANTHROPIC_MESSAGES: &str = "anthropic-messages";
    pub const OPENAI_COMPLETIONS: &str = "openai-completions";
    pub const OPENAI_RESPONSES: &str = "openai-responses";
    pub const GOOGLE_GENERATIVE_AI: &str = "google-generative-ai";
    pub const BEDROCK_CONVERSE_STREAM: &str = "bedrock-converse-stream";
}

/// Auth/stream error taxonomy (arch-01 §3.7/§8). Scaffold placeholder; full taxonomy
/// (`oauth`/`auth`/`provider`/`stream`/`model_source`) lands with the concrete providers.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider not found: {0}")]
    UnknownProvider(String),
    #[error("no API implementation for {0}")]
    NoApiImpl(String),
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}

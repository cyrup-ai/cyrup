//! Deprecated per-API stream aliases (1:1 surface port of Pi `legacy-api-aliases.ts`).
//!
//! Pi re-exports each API module's `stream` / `streamSimple` under flat, provider-named aliases
//! (`streamAnthropic`, `streamSimpleAnthropic`, …) marked `@deprecated` in favor of importing the
//! API module (`anthropicMessagesApi().stream`). cyrup folds both the full-options (`stream`) and
//! simple-options (`streamSimple`) entry points into a single [`ApiImpl`] whose `run` serves both
//! via [`StreamOptions`](crate::StreamOptions), so each alias resolves to that API's `factory()`
//! — the cyrup analog of Pi's bound module object. These exist only for source-compatibility; new
//! code should use the API module's `factory()` or the [`ApiRegistry`](crate::api::ApiRegistry).

use std::sync::Arc;

use crate::api::ApiImpl;

/// The `ApiImpl` for the `anthropic-messages` wire protocol.
#[deprecated(note = "Use `api::anthropic_messages::factory()` or the `ApiRegistry`.")]
pub fn stream_anthropic() -> Arc<dyn ApiImpl> {
    crate::api::anthropic_messages::factory()
}

/// The `ApiImpl` for the `anthropic-messages` wire protocol (simple-options entry point).
#[deprecated(note = "Use `api::anthropic_messages::factory()` or the `ApiRegistry`.")]
pub fn stream_simple_anthropic() -> Arc<dyn ApiImpl> {
    crate::api::anthropic_messages::factory()
}

/// The `ApiImpl` for the `azure-openai-responses` wire protocol.
#[deprecated(note = "Use `api::azure_openai_responses::factory()` or the `ApiRegistry`.")]
pub fn stream_azure_openai_responses() -> Arc<dyn ApiImpl> {
    crate::api::azure_openai_responses::factory()
}

/// The `ApiImpl` for the `azure-openai-responses` wire protocol (simple-options entry point).
#[deprecated(note = "Use `api::azure_openai_responses::factory()` or the `ApiRegistry`.")]
pub fn stream_simple_azure_openai_responses() -> Arc<dyn ApiImpl> {
    crate::api::azure_openai_responses::factory()
}

/// The `ApiImpl` for the `google-generative-ai` wire protocol.
#[deprecated(note = "Use `api::google_generative_ai::factory()` or the `ApiRegistry`.")]
pub fn stream_google() -> Arc<dyn ApiImpl> {
    crate::api::google_generative_ai::factory()
}

/// The `ApiImpl` for the `google-generative-ai` wire protocol (simple-options entry point).
#[deprecated(note = "Use `api::google_generative_ai::factory()` or the `ApiRegistry`.")]
pub fn stream_simple_google() -> Arc<dyn ApiImpl> {
    crate::api::google_generative_ai::factory()
}

/// The `ApiImpl` for the `mistral-conversations` wire protocol.
#[deprecated(note = "Use `api::mistral_conversations::factory()` or the `ApiRegistry`.")]
pub fn stream_mistral() -> Arc<dyn ApiImpl> {
    crate::api::mistral_conversations::factory()
}

/// The `ApiImpl` for the `mistral-conversations` wire protocol (simple-options entry point).
#[deprecated(note = "Use `api::mistral_conversations::factory()` or the `ApiRegistry`.")]
pub fn stream_simple_mistral() -> Arc<dyn ApiImpl> {
    crate::api::mistral_conversations::factory()
}

/// The `ApiImpl` for the `openai-completions` wire protocol.
#[deprecated(note = "Use `api::openai_completions::factory()` or the `ApiRegistry`.")]
pub fn stream_openai_completions() -> Arc<dyn ApiImpl> {
    crate::api::openai_completions::factory()
}

/// The `ApiImpl` for the `openai-completions` wire protocol (simple-options entry point).
#[deprecated(note = "Use `api::openai_completions::factory()` or the `ApiRegistry`.")]
pub fn stream_simple_openai_completions() -> Arc<dyn ApiImpl> {
    crate::api::openai_completions::factory()
}

/// The `ApiImpl` for the `openai-responses` wire protocol.
#[deprecated(note = "Use `api::openai_responses::factory()` or the `ApiRegistry`.")]
pub fn stream_openai_responses() -> Arc<dyn ApiImpl> {
    crate::api::openai_responses::factory()
}

/// The `ApiImpl` for the `openai-responses` wire protocol (simple-options entry point).
#[deprecated(note = "Use `api::openai_responses::factory()` or the `ApiRegistry`.")]
pub fn stream_simple_openai_responses() -> Arc<dyn ApiImpl> {
    crate::api::openai_responses::factory()
}

#[cfg(test)]
#[allow(deprecated, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_to_their_api_impl() {
        // Each deprecated alias resolves to the same wire-protocol id as the API module it shims.
        assert_eq!(
            stream_anthropic().api().as_str(),
            crate::known_api::ANTHROPIC_MESSAGES
        );
        assert_eq!(
            stream_simple_anthropic().api().as_str(),
            crate::known_api::ANTHROPIC_MESSAGES
        );
        assert_eq!(
            stream_azure_openai_responses().api().as_str(),
            crate::known_api::AZURE_OPENAI_RESPONSES
        );
        assert_eq!(
            stream_simple_azure_openai_responses().api().as_str(),
            crate::known_api::AZURE_OPENAI_RESPONSES
        );
        assert_eq!(
            stream_google().api().as_str(),
            crate::known_api::GOOGLE_GENERATIVE_AI
        );
        assert_eq!(
            stream_simple_google().api().as_str(),
            crate::known_api::GOOGLE_GENERATIVE_AI
        );
        assert_eq!(
            stream_mistral().api().as_str(),
            crate::known_api::MISTRAL_CONVERSATIONS
        );
        assert_eq!(
            stream_simple_mistral().api().as_str(),
            crate::known_api::MISTRAL_CONVERSATIONS
        );
        assert_eq!(
            stream_openai_completions().api().as_str(),
            crate::known_api::OPENAI_COMPLETIONS
        );
        assert_eq!(
            stream_simple_openai_completions().api().as_str(),
            crate::known_api::OPENAI_COMPLETIONS
        );
        assert_eq!(
            stream_openai_responses().api().as_str(),
            crate::known_api::OPENAI_RESPONSES
        );
        assert_eq!(
            stream_simple_openai_responses().api().as_str(),
            crate::known_api::OPENAI_RESPONSES
        );
    }
}

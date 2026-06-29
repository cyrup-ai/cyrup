//! The OpenAI provider (arch-01 §5). Speaks the
//! [`openai-responses`](crate::api::openai_responses) wire protocol. Mirrors Pi's
//! `providers/openai.ts` + the generated `openai.models.ts` catalog.

use crate::api::{builtin_registry, ApiRegistry};
use crate::auth::{env_key, CredentialStore, InMemoryCredentialStore, ProviderAuth};
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// OpenAI's API base URL (the `/responses` path is appended by the wire impl).
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// The provider id.
pub const OPENAI_PROVIDER_ID: &str = "openai";

/// The env var carrying the OpenAI API key (Pi `envApiKeyAuth("OpenAI API key",
/// ["OPENAI_API_KEY"])`, openai.ts:11).
pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

/// The verbatim catalog extracted from Pi's generated `openai.models.ts`.
const OPENAI_CATALOG_JSON: &str = include_str!("catalog/openai.json");

/// The full OpenAI catalog (1:1 with Pi `OPENAI_MODELS`). A parse failure yields an empty catalog
/// (surfaced loudly by the count test) rather than a panic (NO-PANIC policy).
pub fn openai_models() -> Vec<Model> {
    serde_json::from_str(OPENAI_CATALOG_JSON).unwrap_or_default()
}

/// The OpenAI [`ProviderAuth`]: an API key from `$OPENAI_API_KEY` (Pi `envApiKeyAuth`).
pub fn openai_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key([OPENAI_API_KEY_ENV]))
}

/// Construct the OpenAI provider over the given credential store + shared api registry. The
/// registry MUST provide the `openai-responses` impl (use [`builtin_registry`]).
pub fn openai_provider_with(store: Arc<dyn CredentialStore>, registry: Arc<ApiRegistry>) -> WireProvider {
    WireProvider::new(OPENAI_PROVIDER_ID, "OpenAI", openai_models(), openai_auth(), store, registry)
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn openai_provider() -> WireProvider {
    openai_provider_with(Arc::new(InMemoryCredentialStore::new()), Arc::new(builtin_registry()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::auth::types::AuthContext;
    use crate::context::Context;
    use crate::known_api::OPENAI_RESPONSES;
    use crate::provider::Provider;
    use crate::stream::{collect_message, StreamOptions};
    use cyrup_core::StopReason;
    use std::collections::BTreeMap;

    struct MapEnv(BTreeMap<String, String>);
    #[async_trait::async_trait]
    impl AuthContext for MapEnv {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    #[test]
    fn catalog_parses_verbatim_with_expected_count() {
        let models = openai_models();
        // Every entry in Pi's `openai.models.ts` (42 models).
        assert_eq!(models.len(), 42);
        assert!(models.iter().all(|m| m.api.as_str() == OPENAI_RESPONSES));
        assert!(models.iter().all(|m| m.provider.as_str() == "openai"));
        assert!(models.iter().all(|m| m.base_url.as_deref() == Some(OPENAI_BASE_URL)));
    }

    #[test]
    fn provider_identity_and_env_mapping() {
        let p = openai_provider();
        assert_eq!(p.id().as_str(), "openai");
        assert!(p.get_model("gpt-4").is_some());
        let vars = crate::env_api_keys::api_key_env_vars("openai").expect("env mapping");
        assert!(vars.contains(&OPENAI_API_KEY_ENV));
    }

    #[tokio::test]
    async fn unconfigured_without_env_yields_error_terminal() {
        let provider = openai_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider.get_model("gpt-4").unwrap().clone();
        let msg = collect_message(provider.stream(&model, &Context::default(), &StreamOptions::default())).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("not configured"));
    }

    #[tokio::test]
    async fn resolves_auth_then_fails_at_transport() {
        let env = MapEnv(BTreeMap::from([(OPENAI_API_KEY_ENV.to_string(), "sk-openai-test".to_string())]));
        let provider = openai_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(env));
        let mut model = provider.get_model("gpt-4").unwrap().clone();
        model.base_url = Some("http://127.0.0.1:1/v1".to_string());
        let msg = collect_message(provider.stream(&model, &Context::default(), &StreamOptions::default())).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(!err.contains("not configured"), "auth should have resolved, got: {err}");
        assert!(err.contains("transport"), "expected transport error, got: {err}");
    }

    /// Live smoke test against the real OpenAI Responses API. Ignored by default; run with
    /// `OPENAI_API_KEY` set: `cargo test -p cyrup-provider -- --ignored live_openai`.
    #[tokio::test]
    #[ignore = "hits the real OpenAI API; requires OPENAI_API_KEY"]
    async fn live_openai_returns_non_empty_done() {
        use cyrup_core::{Content, Message};
        if std::env::var("OPENAI_API_KEY").is_err() {
            eprintln!("skipping: OPENAI_API_KEY not set");
            return;
        }
        let provider = openai_provider();
        let model = provider.get_model("gpt-5-mini").or_else(|| provider.get_model("gpt-5")).unwrap().clone();
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text("Reply with exactly: pong")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let opts = StreamOptions { max_tokens: Some(256), ..Default::default() };
        let msg = collect_message(provider.stream(&model, &ctx, &opts)).await;
        assert_ne!(msg.stop_reason, StopReason::Error, "got error: {:?}", msg.error_message);
        let has_content = msg.content.iter().any(|c| match c {
            Content::Text { text, .. } => !text.trim().is_empty(),
            Content::Thinking { thinking, .. } => !thinking.trim().is_empty(),
            _ => false,
        });
        assert!(has_content, "expected non-empty content, got: {:?}", msg.content);
    }
}

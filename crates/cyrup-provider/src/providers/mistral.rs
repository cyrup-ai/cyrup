//! The Mistral provider (arch-01 §5). Speaks the
//! [`mistral-conversations`](crate::api::mistral_conversations) wire protocol. Mirrors Pi's
//! `providers/mistral.ts` + the generated `mistral.models.ts` catalog.
//!
//! Auth: `MISTRAL_API_KEY` (Pi `envApiKeyAuth("Mistral API key", ["MISTRAL_API_KEY"])`). The Mistral
//! API authenticates with `Authorization: Bearer` (set by the wire impl). Base URL
//! `https://api.mistral.ai` (the `/v1/chat/completions` path is appended by the wire impl).

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::{CredentialStore, InMemoryCredentialStore, ProviderAuth, env_key};
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// Mistral's API base URL (the chat-completions path is appended by the wire impl).
pub const MISTRAL_BASE_URL: &str = "https://api.mistral.ai";

/// The provider id.
pub const MISTRAL_PROVIDER_ID: &str = "mistral";

/// The verbatim catalog extracted from Pi's generated `mistral.models.ts`.
const MISTRAL_CATALOG_JSON: &str = include_str!("catalog/mistral.json");

/// The full Mistral catalog (1:1 with Pi `MISTRAL_MODELS`). A parse failure yields an empty catalog
/// (surfaced loudly by the catalog-count test) rather than a panic (NO-PANIC policy).
pub fn mistral_models() -> Vec<Model> {
    serde_json::from_str(MISTRAL_CATALOG_JSON).unwrap_or_default()
}

/// The Mistral [`ProviderAuth`]: `MISTRAL_API_KEY` (Pi `envApiKeyAuth`, env-api-keys.ts).
pub fn mistral_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key(["MISTRAL_API_KEY"]))
}

/// Construct the Mistral provider over the given credential store + shared api registry. The
/// registry MUST provide the `mistral-conversations` impl (use [`builtin_registry`]).
pub fn mistral_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        MISTRAL_PROVIDER_ID,
        "Mistral",
        mistral_models(),
        mistral_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn mistral_provider() -> WireProvider {
    mistral_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
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
    use crate::auth::types::AuthContext;
    use crate::context::Context;
    use crate::known_api::MISTRAL_CONVERSATIONS;
    use crate::provider::Provider;
    use crate::stream::{StreamOptions, collect_message};
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
        let models = mistral_models();
        assert_eq!(models.len(), 30);
        assert!(
            models
                .iter()
                .all(|m| m.api.as_str() == MISTRAL_CONVERSATIONS)
        );
        assert!(models.iter().all(|m| m.provider.as_str() == "mistral"));
        assert!(models.iter().all(|m| m.base_url == MISTRAL_BASE_URL));
    }

    #[test]
    fn reasoning_and_non_reasoning_present() {
        let models = mistral_models();
        let find = |id: &str| models.iter().find(|m| m.id.as_str() == id);
        assert!(
            find("magistral-medium-latest")
                .expect("magistral-medium-latest")
                .reasoning
        );
        assert!(
            !find("codestral-latest")
                .expect("codestral-latest")
                .reasoning
        );
    }

    #[test]
    fn provider_identity() {
        let p = mistral_provider();
        assert_eq!(p.id().as_str(), "mistral");
        assert!(p.get_model("codestral-latest").is_some());
        assert!(p.models().len() >= 20);
    }

    #[tokio::test]
    async fn unconfigured_without_env_yields_error_terminal() {
        let provider = mistral_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider.get_model("codestral-latest").unwrap().clone();
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("not configured"));
    }

    #[tokio::test]
    async fn resolves_auth_then_fails_at_transport() {
        let env = MapEnv(BTreeMap::from([(
            "MISTRAL_API_KEY".to_string(),
            "sk-mistral".to_string(),
        )]));
        let provider = mistral_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(env));
        let mut model = provider.get_model("codestral-latest").unwrap().clone();
        model.base_url = "http://127.0.0.1:1".to_string();
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(
            !err.contains("not configured"),
            "auth should have resolved, got: {err}"
        );
        assert!(
            err.contains("transport"),
            "expected transport error, got: {err}"
        );
    }

    /// Live smoke test against the real Mistral API. Ignored by default; run with `MISTRAL_API_KEY`
    /// set: `cargo test -p cyrup-provider -- --ignored live_mistral`.
    #[tokio::test]
    #[ignore = "hits the real Mistral API; requires MISTRAL_API_KEY"]
    async fn live_mistral_returns_non_empty_done() {
        use cyrup_core::{Content, Message};
        if std::env::var("MISTRAL_API_KEY").is_err() {
            eprintln!("skipping: MISTRAL_API_KEY not set");
            return;
        }
        let provider = mistral_provider();
        let model = provider.get_model("mistral-small-latest").unwrap().clone();
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text("Reply with exactly: pong")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let opts = StreamOptions {
            max_tokens: Some(64),
            ..Default::default()
        };
        let msg = collect_message(provider.stream(&model, &ctx, &opts)).await;
        assert_ne!(
            msg.stop_reason,
            StopReason::Error,
            "got error: {:?}",
            msg.error_message
        );
        let has_text = msg
            .content
            .iter()
            .any(|c| matches!(c, Content::Text { text, .. } if !text.trim().is_empty()));
        assert!(has_text, "expected non-empty text, got: {:?}", msg.content);
    }
}

//! The Google (Gemini) provider (arch-01 §5). Speaks the
//! [`google-generative-ai`](crate::api::google_generative_ai) wire protocol. Mirrors Pi's
//! `providers/google.ts` + the generated `google.models.ts` catalog.
//!
//! Auth: `GEMINI_API_KEY` (Pi `envApiKeyAuth("Gemini API key", ["GEMINI_API_KEY"])`). The Gemini
//! REST API authenticates with the `x-goog-api-key` header (set by the wire impl). Base URL
//! `https://generativelanguage.googleapis.com/v1beta` (the `/models/{id}:streamGenerateContent`
//! path is appended by the wire impl).

use crate::api::{builtin_registry, ApiRegistry};
use crate::auth::{env_key, CredentialStore, InMemoryCredentialStore, ProviderAuth};
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// Google's Generative Language API base URL (the streaming-generate path is appended by the wire
/// impl).
pub const GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// The provider id.
pub const GOOGLE_PROVIDER_ID: &str = "google";

/// The verbatim catalog extracted from Pi's generated `google.models.ts`.
const GOOGLE_CATALOG_JSON: &str = include_str!("catalog/google.json");

/// The full Google catalog (1:1 with Pi `GOOGLE_MODELS`). A parse failure yields an empty catalog
/// (surfaced loudly by the catalog-count test) rather than a panic (NO-PANIC policy).
pub fn google_models() -> Vec<Model> {
    serde_json::from_str(GOOGLE_CATALOG_JSON).unwrap_or_default()
}

/// The Google [`ProviderAuth`]: `GEMINI_API_KEY` (Pi `envApiKeyAuth`, env-api-keys.ts:80).
pub fn google_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key(["GEMINI_API_KEY"]))
}

/// Construct the Google provider over the given credential store + shared api registry. The registry
/// MUST provide the `google-generative-ai` impl (use [`builtin_registry`]).
pub fn google_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(GOOGLE_PROVIDER_ID, "Google", google_models(), google_auth(), store, registry)
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn google_provider() -> WireProvider {
    google_provider_with(Arc::new(InMemoryCredentialStore::new()), Arc::new(builtin_registry()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::auth::types::AuthContext;
    use crate::context::Context;
    use crate::known_api::GOOGLE_GENERATIVE_AI;
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
        let models = google_models();
        assert_eq!(models.len(), 16);
        assert!(models.iter().all(|m| m.api.as_str() == GOOGLE_GENERATIVE_AI));
        assert!(models.iter().all(|m| m.provider.as_str() == "google"));
        assert!(models.iter().all(|m| m.base_url.as_deref() == Some(GOOGLE_BASE_URL)));
        // Every Gemini model accepts image input.
        assert!(models.iter().all(|m| m.supports_image_input()));
    }

    #[test]
    fn flagship_models_present() {
        let models = google_models();
        let find = |id: &str| models.iter().find(|m| m.id.as_str() == id);
        let pro = find("gemini-2.5-pro").expect("gemini-2.5-pro");
        assert!(pro.reasoning);
        assert_eq!(pro.context_window, 1_048_576);
        // A non-reasoning flash model.
        assert!(!find("gemini-2.0-flash").expect("2.0-flash").reasoning);
    }

    #[test]
    fn provider_identity() {
        let p = google_provider();
        assert_eq!(p.id().as_str(), "google");
        assert!(p.get_model("gemini-2.5-pro").is_some());
        assert!(p.models().len() >= 10);
    }

    #[tokio::test]
    async fn unconfigured_without_env_yields_error_terminal() {
        let provider = google_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider.get_model("gemini-2.5-pro").unwrap().clone();
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
        let env = MapEnv(BTreeMap::from([("GEMINI_API_KEY".to_string(), "test-key".to_string())]));
        let provider = google_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(env));
        let mut model = provider.get_model("gemini-2.5-pro").unwrap().clone();
        model.base_url = Some("http://127.0.0.1:1".to_string());
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(!err.contains("not configured"), "auth should have resolved, got: {err}");
        assert!(err.contains("transport"), "expected transport error, got: {err}");
    }

    /// Live smoke test against the real Gemini API. Ignored by default; run with `GEMINI_API_KEY`
    /// set: `cargo test -p cyrup-provider -- --ignored live_google`.
    #[tokio::test]
    #[ignore = "hits the real Gemini API; requires GEMINI_API_KEY"]
    async fn live_google_returns_non_empty_done() {
        use cyrup_core::{Content, Message};
        if std::env::var("GEMINI_API_KEY").is_err() {
            eprintln!("skipping: GEMINI_API_KEY not set");
            return;
        }
        let provider = google_provider();
        let model = provider.get_model("gemini-2.0-flash").unwrap().clone();
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text("Reply with exactly: pong")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let opts = StreamOptions { max_tokens: Some(64), ..Default::default() };
        let msg = collect_message(provider.stream(&model, &ctx, &opts)).await;
        assert_ne!(msg.stop_reason, StopReason::Error, "got error: {:?}", msg.error_message);
        let has_text = msg
            .content
            .iter()
            .any(|c| matches!(c, Content::Text { text, .. } if !text.trim().is_empty()));
        assert!(has_text, "expected non-empty text, got: {:?}", msg.content);
    }
}

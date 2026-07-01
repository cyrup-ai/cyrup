//! The OpenCode Zen provider (arch-01 §5). A **mixed-api** provider: its catalog spans four wire
//! protocols — `anthropic-messages`, `google-generative-ai`, `openai-completions`, and
//! `openai-responses` — each model carrying its own `api` + `baseUrl`. Mirrors Pi's
//! `providers/opencode.ts` + the generated `opencode.models.ts` catalog.
//!
//! Auth: `OPENCODE_API_KEY` (Pi `envApiKeyAuth("OpenCode API key", ["OPENCODE_API_KEY"])`). The
//! provider has no provider-level base URL; the per-model `baseUrl` drives each request. The shared
//! [`builtin_registry`] already provides all four wire impls, so this provider is purely additive.

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::{CredentialStore, InMemoryCredentialStore, ProviderAuth, env_key};
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// The provider id.
pub const OPENCODE_PROVIDER_ID: &str = "opencode";

/// The verbatim catalog extracted from Pi's generated `opencode.models.ts`.
const OPENCODE_CATALOG_JSON: &str = include_str!("catalog/opencode.json");

/// The full OpenCode Zen catalog (1:1 with Pi `OPENCODE_MODELS`). A parse failure yields an empty
/// catalog (surfaced loudly by the catalog-count test) rather than a panic (NO-PANIC policy).
pub fn opencode_models() -> Vec<Model> {
    serde_json::from_str(OPENCODE_CATALOG_JSON).unwrap_or_default()
}

/// The OpenCode [`ProviderAuth`]: `OPENCODE_API_KEY` (Pi `envApiKeyAuth`, env-api-keys.ts:97).
pub fn opencode_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key(["OPENCODE_API_KEY"]))
}

/// Construct the OpenCode provider over the given credential store + shared api registry. The
/// registry MUST provide every api the catalog references (use [`builtin_registry`]).
pub fn opencode_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        OPENCODE_PROVIDER_ID,
        "OpenCode Zen",
        opencode_models(),
        opencode_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn opencode_provider() -> WireProvider {
    opencode_provider_with(
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
    use crate::known_api::{
        ANTHROPIC_MESSAGES, GOOGLE_GENERATIVE_AI, OPENAI_COMPLETIONS, OPENAI_RESPONSES,
    };
    use crate::provider::Provider;
    use crate::stream::{StreamOptions, collect_message};
    use cyrup_core::StopReason;
    use std::collections::{BTreeMap, BTreeSet};

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
        let models = opencode_models();
        assert_eq!(models.len(), 48);
        assert!(models.iter().all(|m| m.provider.as_str() == "opencode"));
        assert!(models.iter().all(|m| !m.base_url.is_empty()));
        // The catalog spans all four wire protocols this provider declares (Pi opencode.ts:8-23).
        let apis: BTreeSet<&str> = models.iter().map(|m| m.api.as_str()).collect();
        assert!(apis.contains(ANTHROPIC_MESSAGES));
        assert!(apis.contains(GOOGLE_GENERATIVE_AI));
        assert!(apis.contains(OPENAI_COMPLETIONS));
        assert!(apis.contains(OPENAI_RESPONSES));
        // Every api the catalog uses must be backed by a builtin impl.
        let reg = builtin_registry();
        for api in apis {
            assert!(reg.contains(&api.into()), "no builtin impl for {api}");
        }
    }

    #[test]
    fn provider_identity() {
        let p = opencode_provider();
        assert_eq!(p.id().as_str(), "opencode");
        assert!(p.models().len() >= 40);
    }

    #[tokio::test]
    async fn unconfigured_without_env_yields_error_terminal() {
        let provider = opencode_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider.models().first().unwrap().clone();
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("not configured"));
    }

    /// A google-tagged opencode model reaches the `google-generative-ai` transport once auth
    /// resolves — proving the mixed-api registry routes the new impl end-to-end.
    #[tokio::test]
    async fn google_tagged_model_reaches_transport() {
        let env = MapEnv(BTreeMap::from([(
            "OPENCODE_API_KEY".to_string(),
            "test-key".to_string(),
        )]));
        let provider = opencode_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(env));
        let mut model = provider
            .models()
            .iter()
            .find(|m| m.api.as_str() == GOOGLE_GENERATIVE_AI)
            .expect("a google-tagged opencode model")
            .clone();
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
}

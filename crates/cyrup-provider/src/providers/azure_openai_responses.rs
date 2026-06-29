//! The Azure OpenAI provider (arch-01 §5). Speaks the
//! [`azure-openai-responses`](crate::api::azure_openai_responses) wire protocol with a single
//! env-key auth (`AZURE_OPENAI_API_KEY`) and the verbatim generated catalog. 1:1 port of Pi's
//! `providers/azure-openai-responses.ts` + `azure-openai-responses.models.ts`.
//!
//! Azure endpoint configuration (base URL / resource name / api version / deployment-name map) is
//! resolved from the provider env by the wire impl; see [`crate::api::azure_openai_responses`].

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::{CredentialStore, InMemoryCredentialStore, ProviderAuth, env_key};
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// The provider id.
pub const AZURE_OPENAI_RESPONSES_PROVIDER_ID: &str = "azure-openai-responses";

/// The Azure OpenAI API-key env var (`env-api-keys.ts:77`).
pub const AZURE_OPENAI_API_KEY: &str = "AZURE_OPENAI_API_KEY";

/// The verbatim catalog extracted from Pi's generated `azure-openai-responses.models.ts`.
const CATALOG_JSON: &str = include_str!("catalog/azure-openai-responses.json");

/// The full Azure OpenAI catalog (1:1 with Pi `AZURE_OPENAI_RESPONSES_MODELS`). A parse failure
/// yields an empty catalog (surfaced loudly by the catalog-count test) rather than a panic.
pub fn azure_openai_responses_models() -> Vec<Model> {
    serde_json::from_str(CATALOG_JSON).unwrap_or_default()
}

/// The Azure OpenAI [`ProviderAuth`]: an API key from `AZURE_OPENAI_API_KEY` (Pi
/// `envApiKeyAuth("Azure OpenAI API key", ["AZURE_OPENAI_API_KEY"])`).
pub fn azure_openai_responses_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key([AZURE_OPENAI_API_KEY]))
}

/// Construct the Azure OpenAI provider over the given credential store + shared api registry. The
/// registry MUST provide the `azure-openai-responses` impl (use [`builtin_registry`]).
pub fn azure_openai_responses_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        AZURE_OPENAI_RESPONSES_PROVIDER_ID,
        "Azure OpenAI",
        azure_openai_responses_models(),
        azure_openai_responses_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn azure_openai_responses_provider() -> WireProvider {
    azure_openai_responses_provider_with(
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
    use crate::known_api::AZURE_OPENAI_RESPONSES;
    use crate::provider::Provider;
    use crate::stream::{StreamOptions, collect_message};
    use cyrup_core::{ProviderId, StopReason};
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
    fn catalog_parses_with_expected_count_and_tags() {
        let models = azure_openai_responses_models();
        assert_eq!(models.len(), 42);
        assert!(
            models
                .iter()
                .all(|m| m.api.as_str() == AZURE_OPENAI_RESPONSES)
        );
        assert!(
            models
                .iter()
                .all(|m| m.provider.as_str() == "azure-openai-responses")
        );
        // At least one reasoning model carries a thinkingLevelMap (24 in Pi's catalog).
        assert!(models.iter().any(|m| m.thinking_level_map.is_some()));
    }

    #[test]
    fn env_mapping_present() {
        let vars =
            crate::env_api_keys::api_key_env_vars("azure-openai-responses").expect("mapping");
        assert!(vars.contains(&AZURE_OPENAI_API_KEY));
    }

    #[test]
    fn provider_identity() {
        let p = azure_openai_responses_provider();
        assert_eq!(p.id(), &ProviderId::from("azure-openai-responses"));
        assert_eq!(p.name(), "Azure OpenAI");
        assert_eq!(p.models().len(), 42);
    }

    #[tokio::test]
    async fn unconfigured_without_env_yields_error_terminal() {
        let provider = azure_openai_responses_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider.models().first().expect("model").clone();
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
    async fn configured_with_resource_reaches_transport() {
        // API key present → auth resolves. With an Azure resource name set to an unroutable host
        // label, the request builds a valid Azure URL and fails fast at connect (proving the wire
        // impl resolved the Azure endpoint rather than short-circuiting as "not configured").
        let provider = azure_openai_responses_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::from([(
            AZURE_OPENAI_API_KEY.to_string(),
            "az-key".to_string(),
        )]))));
        let mut model = provider.models().first().expect("model").clone();
        // A concrete (unroutable) base URL so normalization + the /responses route are exercised.
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
            "auth should have resolved: {err}"
        );
        assert!(err.contains("transport"), "expected transport error: {err}");
    }

    #[tokio::test]
    async fn configured_without_base_url_errors_on_missing_azure_config() {
        // API key present but NO Azure base URL/resource configured and an empty model.baseUrl →
        // the wire impl errors with the "base URL is required" message (Pi `resolveAzureConfig`).
        let provider = azure_openai_responses_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::from([(
            AZURE_OPENAI_API_KEY.to_string(),
            "az-key".to_string(),
        )]))));
        // Catalog models carry an empty baseUrl, so no Azure endpoint can be resolved.
        let model = provider.models().first().expect("model").clone();
        assert_eq!(model.base_url, "");
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
            "auth should have resolved: {err}"
        );
        assert!(
            err.contains("Azure OpenAI base URL is required"),
            "expected config error: {err}"
        );
    }
}

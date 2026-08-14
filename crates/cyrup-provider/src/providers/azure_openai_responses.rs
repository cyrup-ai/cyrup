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
    ProviderAuth::with_api_key(env_key("Azure OpenAI API key", [AZURE_OPENAI_API_KEY]))
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
        // pi `azure-openai-responses.models.ts` @91585d9a (45 — GPT-5.6 trio, `7df2a94e`).
        assert_eq!(models.len(), 45);
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

    /// VERSION LAG (v0.83.0 → v0.84.1): the Azure catalog is a DERIVED clone of the `openai` rows
    /// (v0.84.1 `ai/scripts/generate-models.ts:2711-2726`) taken AFTER the temporary-override loop
    /// at `:2273`, so the 2026-07-30 GPT-5.6 price cut (`OPENAI_GPT_56_STANDARD_COSTS`,
    /// `:387-393` — absent at v0.83.0) propagates here. The clone copies the four scalar rates and
    /// DROPS `tiers` (`:2718-2723` lists exactly input/output/cacheRead/cacheWrite), which is why
    /// the tier assertions live in the `openai` provider test and not this one.
    #[test]
    fn the_gpt_5_6_clone_carries_the_post_cut_prices_and_no_tiers() {
        let models = azure_openai_responses_models();
        let find = |id: &str| {
            models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("{id} missing"))
                .clone()
        };

        let luna = find("gpt-5.6-luna");
        assert_eq!(
            (
                luna.cost.input,
                luna.cost.output,
                luna.cost.cache_read,
                luna.cost.cache_write
            ),
            (0.2, 1.2, 0.02, 0.25)
        );
        assert!(luna.cost.tiers.is_none(), "the Azure clone drops tiers");

        let terra = find("gpt-5.6-terra");
        assert_eq!(
            (
                terra.cost.input,
                terra.cost.output,
                terra.cost.cache_read,
                terra.cost.cache_write
            ),
            (2.0, 12.0, 0.2, 2.5)
        );
        assert!(terra.cost.tiers.is_none());

        // MIRROR: Sol is absent from the price table, so its clone is unchanged, and the Azure
        // context-window override still applies to all three (`…:2704-2710`).
        let sol = find("gpt-5.6-sol");
        assert_eq!(
            (
                sol.cost.input,
                sol.cost.output,
                sol.cost.cache_read,
                sol.cost.cache_write
            ),
            (5.0, 30.0, 0.5, 6.25)
        );
        for id in ["gpt-5.6-luna", "gpt-5.6-sol", "gpt-5.6-terra"] {
            assert_eq!(find(id).context_window, 1_050_000, "{id}");
        }
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
        assert_eq!(p.models().len(), 45);
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

//! A `models.json` block composes a provider's catalog and auth, and it must not cost the provider
//! its credential-scoped availability policies. pi's `composeModelProvider` carries both through
//! from the base:
//!
//! ```ts
//! filterModels: base?.filterModels
//!     ? (models, credential: Credential | undefined) => base.filterModels!(models, credential)
//!     : undefined,
//! filterAllModels: base?.filterAllModels
//!     ? (models, credential: Credential | undefined) => base.filterAllModels!(models, credential)
//!     : undefined,
//! ```
//!
//! (`packages/coding-agent/src/core/provider-composer.ts:636-641` @f1b2e77f5). cyrup's composed
//! `WireProvider` carried neither: `github-copilot`'s credential-scoped chat filter was lost under
//! any `github-copilot` block (CFG-112), and PROV-147's new `openai` `filterAllModels` — which hides
//! the Decisions classifier from a Sign in with ChatGPT credential the Decisions API rejects — would
//! have been lost under any `openai` block. Both tests were red before the composed provider
//! delegated the two filters to its base.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use crate::{ModelFile, compose_provider_registry, load_models_file};
use cyrup_provider::{
    CreateModelsOptions, Credential, CredentialStore, InMemoryCredentialStore, ModelType,
};

fn model_file(json: &str) -> ModelFile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, json).unwrap();
    load_models_file(&path).expect("models.json parses")
}

fn registry(file: &ModelFile, provider: &str, credential: Credential) -> cyrup_provider::Models {
    let store: Arc<dyn CredentialStore> =
        Arc::new(InMemoryCredentialStore::new().with_credential(provider.into(), credential));
    let (models, errors) = compose_provider_registry(
        file,
        CreateModelsOptions {
            credentials: Some(store),
            auth_context: None,
            catalog_overlay: None,
        },
    );
    assert!(errors.is_empty(), "the block composes cleanly: {errors:?}");
    models
}

fn oauth(ext: serde_json::Value) -> Credential {
    Credential::Oauth {
        access: "access".to_string(),
        refresh: "refresh".to_string(),
        expires: i64::MAX,
        ext: ext.as_object().cloned().unwrap_or_default(),
    }
}

/// PROV-147 under a `models.json` `openai` block: an OAuth credential still does not list the
/// Decisions classifier, and an API key still does (`providers/openai.ts:27-29`).
#[tokio::test]
async fn a_composed_openai_keeps_its_filter_all_models() {
    let file =
        model_file(r#"{ "providers": { "openai": { "baseUrl": "https://gateway.example/v1" } } }"#);
    let classifiers = |models: Vec<cyrup_provider::AnyModel>| -> Vec<String> {
        models.iter().map(|m| m.id().to_string()).collect()
    };

    let with_oauth = registry(&file, "openai", oauth(serde_json::json!({})));
    assert!(
        classifiers(
            with_oauth
                .get_available_of_type(ModelType::Classifier, Some("openai"))
                .await
                .unwrap()
        )
        .is_empty(),
        "the composed openai dropped its base's filterAllModels"
    );

    let with_key = registry(&file, "openai", Credential::api_key("sk-test"));
    assert_eq!(
        classifiers(
            with_key
                .get_available_of_type(ModelType::Classifier, Some("openai"))
                .await
                .unwrap()
        ),
        ["gpt-6-luna"]
    );
}

/// CFG-112 under a `models.json` `github-copilot` block: the credential's `availableModelIds`
/// still narrow the available chat models (`filterModels`, `providers/github-copilot.ts`).
#[tokio::test]
async fn a_composed_github_copilot_keeps_its_filter_models() {
    let file = model_file(
        r#"{ "providers": { "github-copilot": { "baseUrl": "https://gateway.example/v1" } } }"#,
    );
    let models = registry(
        &file,
        "github-copilot",
        oauth(serde_json::json!({ "availableModelIds": ["gpt-5.4"] })),
    );
    let available: Vec<String> = models
        .get_available(Some("github-copilot"))
        .await
        .unwrap()
        .iter()
        .map(|m| m.id.as_str().to_string())
        .collect();
    assert_eq!(
        available,
        ["gpt-5.4"],
        "the composed github-copilot dropped its base's filterModels"
    );
}

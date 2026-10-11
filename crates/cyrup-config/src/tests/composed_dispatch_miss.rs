//! PROV-157 — what a composed provider says when nothing can stream a model's api.
//!
//! pi's `composeModelProvider` streams through `streamWith` (`provider-composer.ts:582-603`
//! @f1b2e77f5):
//!
//! ```ts
//! const supportsBaseApi = (model) => base?.getModels().some((entry) => entry.api === model.api) ?? false;
//! …
//! if (base && supportsBaseApi(model)) return base.stream(model, context, options);
//! const api = getApiProvider(model.api);
//! if (!api) throw new Error(`No API provider registered for api: ${model.api}`);
//! ```
//!
//! So a model on an api its base's own chat catalog lists goes to the base, whose `createProvider`
//! answers a miss with `Provider <id> has no API implementation for "<api>"` (`models.ts:1072-1083`);
//! any other model falls through to the process-wide registry and its text. Both tests stream
//! through `Models::complete`, pi's production path (`Models.stream` → `applyAuth` →
//! `provider.stream`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use crate::{ModelFile, load_models_file};
use cyrup_core::StopReason;
use cyrup_provider::model::{Modality, ModelCost};
use cyrup_provider::wire::WireProvider;
use cyrup_provider::{
    ApiRegistry, AuthContext, Context, CreateModelsOptions, CredentialStore,
    InMemoryCredentialStore, Model, Models, ProviderAuth, StreamOptions, create_models, env_key,
};

/// An auth context with an empty environment, so a literal `apiKey` stays literal.
struct NoEnv;

#[async_trait::async_trait]
impl AuthContext for NoEnv {
    async fn env(&self, _name: &str) -> Option<String> {
        None
    }
    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

fn model_file(json: &str) -> ModelFile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, json).unwrap();
    load_models_file(&path).expect("models.json parses")
}

/// A collection with `base` registered (if any), then `json` composed over it, every provider on
/// an EMPTY api registry — so every stream ends at a dispatch miss.
fn composed(json: &str, base: Option<WireProvider>) -> Models {
    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::new());
    let ctx: Arc<dyn AuthContext> = Arc::new(NoEnv);
    let mut models = create_models(CreateModelsOptions {
        credentials: Some(store.clone()),
        auth_context: Some(ctx.clone()),
        catalog_overlay: None,
    });
    if let Some(base) = base {
        models.set_provider(Arc::new(base));
    }
    let errors = model_file(json).compose_providers(
        &mut models,
        store,
        Arc::new(ApiRegistry::new()),
        Some(ctx),
    );
    assert!(errors.is_empty(), "the block composes cleanly: {errors:?}");
    models
}

async fn error_of(models: &Models, provider: &str, id: &str) -> Option<String> {
    let model = models
        .get_model(provider, id)
        .unwrap_or_else(|| panic!("{provider}/{id} resolves"));
    let message = models
        .complete(&model, &Context::default(), &StreamOptions::default())
        .await;
    assert_eq!(message.stop_reason, StopReason::Error, "{message:?}");
    message.error_message
}

/// A base-less block (`base` undefined): `supportsBaseApi` is false, so the registry's text.
#[tokio::test]
async fn a_models_json_provider_on_an_unregistered_api_reports_the_registry_miss() {
    let models = composed(
        r#"{ "providers": { "mycorp": {
            "baseUrl": "https://gateway.mycorp.example/v1",
            "api": "no-such-api",
            "apiKey": "sk-literal",
            "models": [{ "id": "m" }]
        } } }"#,
        None,
    );
    assert_eq!(
        error_of(&models, "mycorp", "m").await.as_deref(),
        Some("No API provider registered for api: no-such-api")
    );
}

/// A block over a base: the base's row is on an api the base's own catalog lists, so pi hands it
/// to `base.stream` and the miss is the base's `createProvider` text; the row the block adds is on
/// an api the base does not list, so it falls through to the registry's text.
#[tokio::test]
async fn a_composed_providers_miss_is_its_bases_text_exactly_when_the_base_lists_the_api() {
    let based = Model {
        id: "based".into(),
        name: "Based".into(),
        api: "ghost-api".into(),
        provider: "acme".into(),
        base_url: "https://acme.example/v1".to_string(),
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        input_limits: None,
        prompt_cache: None,
        context_window: 1000,
        max_tokens: 100,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    };
    let base = WireProvider::new(
        "acme",
        "Acme",
        vec![based],
        ProviderAuth::with_api_key(env_key("Acme", ["ACME_API_KEY"])),
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(ApiRegistry::new()),
    )
    .with_auth_context(Arc::new(NoEnv));
    let models = composed(
        r#"{ "providers": { "acme": {
            "apiKey": "sk-literal",
            "baseUrl": "https://acme.example/v1",
            "api": "other-ghost",
            "models": [{ "id": "added" }]
        } } }"#,
        Some(base),
    );
    assert_eq!(
        error_of(&models, "acme", "based").await.as_deref(),
        Some(r#"Provider acme has no API implementation for "ghost-api""#)
    );
    assert_eq!(
        error_of(&models, "acme", "added").await.as_deref(),
        Some("No API provider registered for api: other-ghost")
    );
}

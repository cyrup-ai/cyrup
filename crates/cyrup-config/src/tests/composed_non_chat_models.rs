//! A `models.json` block composes a provider's catalog and auth. It must not cost the provider the
//! models it has that are not chat models, nor what it can do with them.
//!
//! Pi's `composeModelProvider` builds the composed provider over `getAllProviderModels(base)`, which
//! is `provider.getAllModels?.() ?? provider.getModels()` (provider-composer.ts:538, :558 @v1.0.4):
//! the base's catalog of EVERY type. `applyModelsJson` rewrites the `baseUrl` of each of them,
//! chat or not (:326), and the composed provider's `generateImages` and `classify` call the base's
//! (:657, :669). cyrup built the composed `WireProvider` from `Provider::models()` alone, so a block
//! as small as `{"providers":{"openrouter":{"apiKey":"…"}}}` left `openrouter` with no image models
//! and no way to generate an image.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{ModelFile, compose_provider_registry, load_models_file};
use cyrup_core::{Content, EventStream};
use cyrup_provider::{
    AnyModel, ApiRegistry, AssistantImages, ClassifierContext, ClassifierModel, ClassifierOptions,
    ClassifierResult, Context, CreateModelsOptions, CredentialStore, ImageModel, ImagesContext,
    ImagesOptions, ImagesStopReason, InMemoryCredentialStore, Model, Models, Provider,
    ProviderAuth, StreamEvent, StreamOptions, WireProvider, create_models, env_key,
};

fn model_file(json: &str) -> ModelFile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, json).unwrap();
    load_models_file(&path).expect("models.json parses")
}

fn builtin_registry_with(file: &ModelFile) -> (Models, Vec<String>) {
    compose_provider_registry(
        file,
        CreateModelsOptions {
            credentials: Some(Arc::new(InMemoryCredentialStore::new())),
            auth_context: None,
            catalog_overlay: None,
        },
    )
}

fn image_ids(models: &Models, provider: &str) -> BTreeSet<String> {
    models
        .get_image_models(Some(provider))
        .into_iter()
        .map(|m| m.id.as_str().to_string())
        .collect()
}

const GATEWAY: &str = "https://gateway.example/v1";

/// The reported defect, on the real built-ins: the smallest block that composes `openrouter` used to
/// leave it with zero image models (and `getModelsOfType("image")`, `getAvailableOfType("image")` and
/// `generateImages` with nothing to work on).
#[test]
fn a_block_that_only_sets_an_api_key_keeps_the_builtin_image_models() {
    let (baseline, _) = builtin_registry_with(&ModelFile::default());
    let expected = image_ids(&baseline, "openrouter");
    assert!(
        !expected.is_empty(),
        "the built-in openrouter lists image models, or this test proves nothing"
    );

    let (models, errors) = builtin_registry_with(&model_file(
        r#"{ "providers": { "openrouter": { "apiKey": "sk-test" } } }"#,
    ));
    assert!(errors.is_empty(), "the block composes cleanly: {errors:?}");

    assert_eq!(image_ids(&models, "openrouter"), expected);
    assert!(
        models
            .get_provider("openrouter")
            .expect("openrouter is registered")
            .supports_image_generation(),
        "the composed provider still generates images"
    );
}

/// Pi evaluates `config.baseUrl ?? model.baseUrl` for every model the base lists (:326), so a block
/// that redirects `openrouter` redirects its image models too.
#[test]
fn the_blocks_base_url_reaches_the_image_models() {
    let (baseline, _) = builtin_registry_with(&ModelFile::default());
    let expected = image_ids(&baseline, "openrouter");

    let (models, errors) = builtin_registry_with(&model_file(&format!(
        r#"{{ "providers": {{ "openrouter": {{ "baseUrl": "{GATEWAY}" }} }} }}"#
    )));
    assert!(errors.is_empty(), "the block composes cleanly: {errors:?}");

    let images = models.get_image_models(Some("openrouter"));
    assert_eq!(
        images
            .iter()
            .map(|m| m.id.as_str().to_string())
            .collect::<BTreeSet<_>>(),
        expected
    );
    for image in &images {
        assert_eq!(image.base_url, GATEWAY, "{}", image.id);
    }
}

/// `baseUrl` under an oauth mode is the auth gateway, not where requests go (:326
/// `config.oauth === "radius" ? model.baseUrl : …`), for image rows as for chat rows.
#[test]
fn an_oauth_gateway_base_url_leaves_the_image_endpoints_alone() {
    let (baseline, _) = builtin_registry_with(&ModelFile::default());
    let own_endpoints: BTreeSet<String> = baseline
        .get_image_models(Some("openrouter"))
        .into_iter()
        .map(|m| m.base_url)
        .collect();

    let (models, errors) = builtin_registry_with(&model_file(&format!(
        r#"{{ "providers": {{ "openrouter": {{ "baseUrl": "{GATEWAY}", "oauth": "radius" }} }} }}"#
    )));
    assert!(errors.is_empty(), "the block composes cleanly: {errors:?}");

    let endpoints: BTreeSet<String> = models
        .get_image_models(Some("openrouter"))
        .into_iter()
        .map(|m| m.base_url)
        .collect();
    assert_eq!(endpoints, own_endpoints);
}

/// A base with a chat model, an image model and a classifier model, whose `generate_images` and
/// `classify` say which endpoint they were handed. The rows and the stream are a real
/// [`WireProvider`]'s; only the two operations are scripted.
struct OperatingBase {
    inner: WireProvider,
}

impl OperatingBase {
    fn new() -> Self {
        let inner = WireProvider::new(
            "acme",
            "Acme",
            Vec::new(),
            ProviderAuth::with_api_key(env_key("Acme key", ["ACME_API_KEY"])),
            Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>,
            Arc::new(ApiRegistry::new()),
        )
        .with_image_models(vec![image_row()])
        .with_classifier_models(vec![classifier_row()]);
        Self { inner }
    }
}

fn image_row() -> ImageModel {
    cyrup_provider::openrouter_image_model("google/gemini-2.5-flash-image")
        .map(|mut m| {
            m.provider = "acme".into();
            m.id = "acme-img".into();
            m.base_url = "https://acme.example/img".into();
            m
        })
        .expect("the embedded catalog has the example image model")
}

fn classifier_row() -> ClassifierModel {
    ClassifierModel {
        id: "acme-cls".into(),
        name: "Acme classifier".into(),
        api: "llama-cpp-classify".into(),
        provider: "acme".into(),
        base_url: "https://acme.example/cls".into(),
        input: vec![cyrup_provider::Modality::Text],
        cost: cyrup_provider::ModelCost::default(),
        headers: None,
        input_limits: None,
        context_window: 4096,
    }
}

#[async_trait::async_trait]
impl Provider for OperatingBase {
    fn id(&self) -> &cyrup_core::ProviderId {
        self.inner.id()
    }
    fn models(&self) -> &[Model] {
        self.inner.models()
    }
    fn get_all_models(&self) -> Vec<AnyModel> {
        self.inner.get_all_models()
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        self.inner.provider_auth()
    }
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream(model, context, options)
    }
    async fn generate_images(
        &self,
        model: &ImageModel,
        _context: &ImagesContext,
        _options: &ImagesOptions,
    ) -> AssistantImages {
        let mut out = AssistantImages::new(model);
        out.output = vec![Content::text(format!("generated at {}", model.base_url))];
        out
    }
    fn supports_image_generation(&self) -> bool {
        true
    }
    async fn classify(
        &self,
        model: &ClassifierModel,
        _context: &ClassifierContext,
        _options: &ClassifierOptions,
    ) -> ClassifierResult {
        ClassifierResult::errored(model, format!("classified at {}", model.base_url), false)
    }
    fn supports_classification(&self) -> bool {
        true
    }
}

/// `Models` holding `base` under its own id, then composed with `json`.
fn composed_over(base: Arc<dyn Provider>, json: &str) -> Models {
    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::new());
    let mut models = create_models(CreateModelsOptions {
        credentials: Some(store.clone()),
        auth_context: None,
        catalog_overlay: None,
    });
    models.set_provider(base);
    let errors =
        model_file(json).compose_providers(&mut models, store, Arc::new(ApiRegistry::new()), None);
    assert!(errors.is_empty(), "the block composes cleanly: {errors:?}");
    models
}

const ACME_BLOCK: &str = r#"{ "providers": { "acme": { "baseUrl": "https://gateway.example/v1", "apiKey": "sk-test" } } }"#;

/// Pi's composed `provider.generateImages` is `base.generateImages` (:657), handed the composed
/// model: the call reaches the base's implementation, at the block's endpoint.
#[tokio::test]
async fn image_generation_is_carried_over_to_the_composed_provider() {
    let models = composed_over(Arc::new(OperatingBase::new()), ACME_BLOCK);

    let image = models
        .get_image_model("acme", "acme-img")
        .expect("the image row survives the composition");
    assert_eq!(image.base_url, GATEWAY);

    let out = models
        .generate_images(&image, &ImagesContext::default(), &ImagesOptions::default())
        .await;
    assert_eq!(out.error_message, None, "{out:?}");
    assert_eq!(out.stop_reason, ImagesStopReason::Stop);
    assert_eq!(
        out.output,
        vec![Content::text(format!("generated at {GATEWAY}"))],
        "the base's own implementation ran, against the composed endpoint"
    );
}

/// Pi's composed `provider.classify` is `base.classify` (:669).
#[tokio::test]
async fn classification_is_carried_over_to_the_composed_provider() {
    let models = composed_over(Arc::new(OperatingBase::new()), ACME_BLOCK);

    let classifier = models
        .get_classifier_model("acme", "acme-cls")
        .expect("the classifier row survives the composition");
    assert_eq!(classifier.base_url, GATEWAY);

    let out = models
        .classify(
            &classifier,
            &ClassifierContext::default(),
            &ClassifierOptions::default(),
        )
        .await;
    assert_eq!(
        out.error_message.as_deref(),
        Some(format!("classified at {GATEWAY}").as_str()),
        "the base's own implementation ran, against the composed endpoint"
    );
}

/// Pi attaches the members only `if (base?.generateImages)` / `if (base?.classify)`: composing over
/// a base that cannot do either does not conjure the ability.
#[test]
fn a_base_without_the_operations_gives_a_composed_provider_without_them() {
    let chat_only = WireProvider::new(
        "acme",
        "Acme",
        Vec::new(),
        ProviderAuth::with_api_key(env_key("Acme key", ["ACME_API_KEY"])),
        Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>,
        Arc::new(ApiRegistry::new()),
    );
    let models = composed_over(Arc::new(chat_only), ACME_BLOCK);

    let provider = models.get_provider("acme").expect("acme is registered");
    assert!(!provider.supports_image_generation());
    assert!(!provider.supports_classification());
}

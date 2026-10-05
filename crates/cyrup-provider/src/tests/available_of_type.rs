//! `Models::get_all_available` / `get_available_of_type` (pi `Models.getAllAvailable` /
//! `getAvailableOfType`, `packages/ai/src/models.ts:708-732` @v1.0.1): the auth gate of
//! `get_available`, applied across every model type, with the provider's own `filterAllModels`
//! policy when it has one. PROV-105.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use super::classifier_support::{
    ClassifyingProvider, FixedAuth, LLAMA_API, UnconfiguredAuth, chat_model, classifier_model,
    image_model,
};
use crate::auth::ProviderAuth;
use crate::classifier::{AnyModel, ClassifierApiRegistry, ModelType};
use crate::context::Context;
use crate::provider::Provider;
use crate::stream::{StreamEvent, StreamOptions};
use crate::{CreateModelsOptions, Credential, HeaderMap, Model, Models, create_models};
use cyrup_core::{EventStream, ProviderId};

fn configured() -> Option<ProviderAuth> {
    Some(ProviderAuth::with_api_key(Arc::new(FixedAuth {
        key: Some("k"),
        base_url: None,
        headers: HeaderMap::default(),
        env: None,
    })))
}

fn collection(providers: Vec<Arc<dyn Provider>>) -> Models {
    let mut models = create_models(CreateModelsOptions::default());
    for provider in providers {
        models.set_provider(provider);
    }
    models
}

fn ids(models: &[AnyModel]) -> Vec<String> {
    models
        .iter()
        .map(|m| format!("{}/{}", m.provider(), m.id()))
        .collect()
}

fn mixed(id: &str, auth: Option<ProviderAuth>) -> ClassifyingProvider {
    ClassifyingProvider::new(
        id,
        vec![chat_model(id, "chat-a"), chat_model(id, "chat-b")],
        vec![classifier_model(id, "judge", LLAMA_API)],
        auth,
        ClassifierApiRegistry::new(),
    )
}

/// A provider that also lists an image model and drops `chat-b` from the chat catalog through
/// `filter_models`, the way a credential-specific entitlement does.
struct Filtering {
    inner: ClassifyingProvider,
    all_models_policy: bool,
}

impl Provider for Filtering {
    fn id(&self) -> &ProviderId {
        self.inner.id()
    }
    fn models(&self) -> &[Model] {
        self.inner.models()
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        self.inner.provider_auth()
    }
    fn get_all_models(&self) -> Vec<AnyModel> {
        let mut all = self.inner.get_all_models();
        all.push(AnyModel::Image(image_model(
            self.inner.id().as_str(),
            "painter",
            "openrouter-images",
        )));
        all
    }
    fn filter_models(&self, models: &[Model], _credential: Option<&Credential>) -> Vec<Model> {
        models
            .iter()
            .filter(|m| m.id.as_str() != "chat-b")
            .cloned()
            .collect()
    }
    fn filter_all_models(
        &self,
        models: &[AnyModel],
        _credential: Option<&Credential>,
    ) -> Option<Vec<AnyModel>> {
        // A policy of its own: keep only the classifier, whatever `filter_models` says.
        self.all_models_policy.then(|| {
            models
                .iter()
                .filter(|m| m.model_type() == ModelType::Classifier)
                .cloned()
                .collect()
        })
    }
    fn stream(&self, m: &Model, c: &Context, o: &StreamOptions) -> EventStream<StreamEvent> {
        self.inner.stream(m, c, o)
    }
}

/// `getAuthenticatedProviders` drops a provider whose auth is not configured, for every type: a
/// classifier row of an unconfigured provider is listed by `get_models_of_type` and absent here.
#[tokio::test]
async fn an_unconfigured_provider_contributes_no_model_of_any_type() {
    let models = collection(vec![
        Arc::new(mixed("ready", configured())),
        Arc::new(mixed(
            "idle",
            Some(ProviderAuth::with_api_key(Arc::new(UnconfiguredAuth))),
        )),
        Arc::new(mixed("bare", None)),
    ]);

    let listed = models.get_models_of_type(ModelType::Classifier, None);
    assert_eq!(listed.len(), 3, "the catalog lists every provider's rows");

    let available = models
        .get_available_of_type(ModelType::Classifier, None)
        .await
        .unwrap();
    assert_eq!(ids(&available), ["ready/judge"]);
}

/// `getAvailableOfType(type, providerId)` restricts to one provider, and an unknown provider is an
/// empty list (`getAuthenticatedProviders` over no entry).
#[tokio::test]
async fn the_provider_argument_restricts_the_listing() {
    let models = collection(vec![
        Arc::new(mixed("one", configured())),
        Arc::new(mixed("two", configured())),
    ]);
    let only_two = models
        .get_available_of_type(ModelType::Classifier, Some("two"))
        .await
        .unwrap();
    assert_eq!(ids(&only_two), ["two/judge"]);
    assert!(
        models
            .get_available_of_type(ModelType::Classifier, Some("ghost"))
            .await
            .unwrap()
            .is_empty()
    );
}

/// Without `filterAllModels`, chat models go through `filterModels` and every other model is kept
/// (`models.ts:725-728`).
#[tokio::test]
async fn without_a_policy_chat_is_filtered_and_other_types_are_kept() {
    let models = collection(vec![Arc::new(Filtering {
        inner: mixed("p", configured()),
        all_models_policy: false,
    })]);

    let chat = models
        .get_available_of_type(ModelType::Chat, None)
        .await
        .unwrap();
    assert_eq!(ids(&chat), ["p/chat-a"], "`filter_models` dropped chat-b");
    let image = models
        .get_available_of_type(ModelType::Image, None)
        .await
        .unwrap();
    assert_eq!(ids(&image), ["p/painter"]);
    let classifier = models
        .get_available_of_type(ModelType::Classifier, None)
        .await
        .unwrap();
    assert_eq!(ids(&classifier), ["p/judge"]);
}

/// With `filterAllModels` the provider's policy replaces the default outright (`models.ts:724`).
#[tokio::test]
async fn a_filter_all_models_policy_replaces_the_default() {
    let models = collection(vec![Arc::new(Filtering {
        inner: mixed("p", configured()),
        all_models_policy: true,
    })]);

    let all = models.get_all_available(None).await.unwrap();
    assert_eq!(ids(&all), ["p/judge"]);
    assert!(
        models
            .get_available_of_type(ModelType::Chat, None)
            .await
            .unwrap()
            .is_empty()
    );
}

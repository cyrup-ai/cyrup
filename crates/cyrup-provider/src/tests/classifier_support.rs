//! Fixtures shared by the `classifier_*` tests: models, a recording classifier api, and providers
//! that do and do not support `classify`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use crate::auth::types::ModelAuth;
use crate::auth::{ApiKeyAuth, AuthContext, ProviderAuth};
use crate::classifier::{
    AnyModel, ClassifierAnswer, ClassifierApiRegistry, ClassifierContext, ClassifierModel,
    ClassifierOptions, ClassifierResult, ImageModel, OrderedMap, ProviderClassifier,
};
use crate::context::Context;
use crate::error::AuthError;
use crate::provider::Provider;
use crate::stream::{StreamEvent, StreamOptions};
use crate::{AuthResult, HeaderMap, Modality, Model, ModelCost, ProviderEnv};
use cyrup_core::{EventStream, ProviderId};
use tokio_stream::wrappers::ReceiverStream;

pub(super) const LLAMA_API: &str = "llama-cpp-classify";

pub(super) fn chat_model(provider: &str, id: &str) -> Model {
    Model {
        id: id.into(),
        name: id.into(),
        api: "openai-completions".into(),
        provider: provider.into(),
        base_url: "http://chat.test/v1".into(),
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        prompt_cache: None,
        context_window: 1000,
        max_tokens: 100,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

pub(super) fn classifier_model(provider: &str, id: &str, api: &str) -> ClassifierModel {
    ClassifierModel {
        id: id.into(),
        name: format!("{id} (classifier)"),
        api: api.into(),
        provider: provider.into(),
        base_url: "http://model.test/v1".into(),
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        headers: None,
        context_window: 4096,
    }
}

/// PROV-128 — an image row in v1.0.0's unified shape (`ImageModel`, types.ts:1144-1149):
/// `BaseModel` plus `type: "image"` and a required `output` list. `output` includes `text` here so
/// [`ImageModel::outputs_text`] has something to report.
pub(super) fn image_model(provider: &str, id: &str, api: &str) -> ImageModel {
    ImageModel {
        id: id.into(),
        name: format!("{id} (image)"),
        api: api.into(),
        provider: provider.into(),
        base_url: "http://image.test/api/v1".into(),
        input: vec![Modality::Text, Modality::Image],
        output: vec![Modality::Text, Modality::Image],
        cost: ModelCost::default(),
        headers: None,
    }
}

/// A one-question context; enough for dispatch tests, which never read the answers back.
pub(super) fn empty_context() -> ClassifierContext {
    ClassifierContext::default()
}

/// What a [`RecordingClassifier`] saw in its last call.
#[derive(Clone, Debug, Default)]
pub(super) struct Seen {
    pub calls: usize,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub headers: Option<HeaderMap>,
    pub env: Option<ProviderEnv>,
    pub had_transform: bool,
}

/// A classifier api that records its inputs and answers one bool question named after `tag`, so a
/// test can tell WHICH implementation produced a result.
pub(super) struct RecordingClassifier {
    pub tag: &'static str,
    pub seen: Mutex<Seen>,
}

impl RecordingClassifier {
    pub(super) fn new(tag: &'static str) -> Arc<Self> {
        Arc::new(Self {
            tag,
            seen: Mutex::new(Seen::default()),
        })
    }

    pub(super) fn seen(&self) -> Seen {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ProviderClassifier for RecordingClassifier {
    async fn classify(
        &self,
        model: &ClassifierModel,
        _context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult {
        {
            let mut seen = self.seen.lock().unwrap();
            seen.calls += 1;
            seen.base_url = Some(model.base_url.clone());
            seen.api_key = options.api_key.clone();
            seen.headers = options.headers.clone();
            seen.env = options.env.clone();
            seen.had_transform = options.transform_headers.is_some();
        }
        let mut result = ClassifierResult::new(model);
        let mut answers = OrderedMap::new();
        answers.insert(self.tag, ClassifierAnswer::Bool { probability: 0.5 });
        result.answers = answers;
        result
    }
}

/// An api-key strategy resolving to a fixed key, base URL, header and env overlay.
pub(super) struct FixedAuth {
    pub key: Option<&'static str>,
    pub base_url: Option<&'static str>,
    pub headers: HeaderMap,
    pub env: Option<ProviderEnv>,
}

#[async_trait::async_trait]
impl ApiKeyAuth for FixedAuth {
    fn name(&self) -> &str {
        "fixed"
    }
    async fn resolve(
        &self,
        _model: &Model,
        _ctx: &dyn AuthContext,
        _cred: Option<&crate::auth::Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        Ok(Some(AuthResult {
            auth: ModelAuth {
                api_key: self.key.map(str::to_string),
                headers: Some(self.headers.clone()),
                base_url: self.base_url.map(str::to_string),
            },
            env: self.env.clone(),
            source: Some("test".into()),
        }))
    }
}

/// An api-key strategy that is never configured (`resolve` answers `None`).
pub(super) struct UnconfiguredAuth;

#[async_trait::async_trait]
impl ApiKeyAuth for UnconfiguredAuth {
    fn name(&self) -> &str {
        "unconfigured"
    }
    async fn resolve(
        &self,
        _model: &Model,
        _ctx: &dyn AuthContext,
        _cred: Option<&crate::auth::Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        Ok(None)
    }
}

fn empty_stream() -> EventStream<StreamEvent> {
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    Box::pin(ReceiverStream::new(rx))
}

/// A provider with chat models only: it inherits [`Provider::classify`] and
/// [`Provider::get_all_models`].
pub(super) struct PlainProvider {
    pub id: ProviderId,
    pub models: Vec<Model>,
}

impl PlainProvider {
    pub(super) fn new(id: &str, models: Vec<Model>) -> Self {
        Self {
            id: id.into(),
            models,
        }
    }
}

impl Provider for PlainProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &self.models
    }
    fn stream(&self, _: &Model, _: &Context, _: &StreamOptions) -> EventStream<StreamEvent> {
        empty_stream()
    }
}

/// A provider that lists classifier models and classifies through a [`ClassifierApiRegistry`], the
/// way pi's `createProvider({ classifiers })` does.
pub(super) struct ClassifyingProvider {
    pub id: ProviderId,
    pub models: Vec<Model>,
    pub classifier_models: Vec<ClassifierModel>,
    pub auth: Option<ProviderAuth>,
    pub registry: ClassifierApiRegistry,
}

impl ClassifyingProvider {
    pub(super) fn new(
        id: &str,
        models: Vec<Model>,
        classifier_models: Vec<ClassifierModel>,
        auth: Option<ProviderAuth>,
        registry: ClassifierApiRegistry,
    ) -> Self {
        Self {
            id: id.into(),
            models,
            classifier_models,
            auth,
            registry,
        }
    }
}

#[async_trait::async_trait]
impl Provider for ClassifyingProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &self.models
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        self.auth.as_ref()
    }
    fn get_all_models(&self) -> Vec<AnyModel> {
        self.models
            .iter()
            .cloned()
            .map(AnyModel::Chat)
            .chain(
                self.classifier_models
                    .iter()
                    .cloned()
                    .map(AnyModel::Classifier),
            )
            .collect()
    }
    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult {
        self.registry.classify(model, context, options).await
    }
    fn stream(&self, _: &Model, _: &Context, _: &StreamOptions) -> EventStream<StreamEvent> {
        empty_stream()
    }
}

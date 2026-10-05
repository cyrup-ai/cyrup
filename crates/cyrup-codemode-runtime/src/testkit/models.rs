//! A [`CodemodeModels`] over a fixed catalog and scripted provider calls.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use cyrup_core::CancelToken;
use cyrup_provider::{
    AnyModel, AssistantImages, ClassifierContext, ClassifierModel, ClassifierResult, ImageModel,
    ImagesContext, ModelType,
};
use futures::future::BoxFuture;

use crate::tool::models::CodemodeModels;

/// How a [`FakeModels`] answers a classification.
pub type ClassifyFn = Arc<
    dyn Fn(ClassifierModel, ClassifierContext) -> BoxFuture<'static, ClassifierResult>
        + Send
        + Sync,
>;

/// How a [`FakeModels`] answers an image generation.
pub type ImagesFn =
    Arc<dyn Fn(ImageModel, ImagesContext) -> BoxFuture<'static, AssistantImages> + Send + Sync>;

/// See the module docs.
pub struct FakeModels {
    /// Every known model.
    pub catalog: Vec<AnyModel>,
    /// The models whose provider has working credentials.
    pub available: Vec<AnyModel>,
    pub classify: ClassifyFn,
    pub images: ImagesFn,
    /// Every provider call: `(function, provider/id)`.
    pub calls: Mutex<Vec<(&'static str, String)>>,
}

impl FakeModels {
    #[must_use]
    pub fn new(catalog: Vec<AnyModel>, classify: ClassifyFn, images: ImagesFn) -> Self {
        Self {
            available: catalog.clone(),
            catalog,
            classify,
            images,
            calls: Mutex::new(Vec::new()),
        }
    }
}

fn of_type(models: &[AnyModel], model_type: ModelType, provider: Option<&str>) -> Vec<AnyModel> {
    models
        .iter()
        .filter(|m| {
            m.model_type() == model_type && provider.is_none_or(|p| m.provider().as_str() == p)
        })
        .cloned()
        .collect()
}

#[async_trait::async_trait]
impl CodemodeModels for FakeModels {
    fn get_models_of_type(&self, model_type: ModelType, provider: Option<&str>) -> Vec<AnyModel> {
        of_type(&self.catalog, model_type, provider)
    }

    async fn get_available_of_type(
        &self,
        model_type: ModelType,
        provider: Option<&str>,
        _cancel: &CancelToken,
    ) -> Result<Vec<AnyModel>, String> {
        Ok(of_type(&self.available, model_type, provider))
    }

    fn get_model_of_type(
        &self,
        model_type: ModelType,
        provider: &str,
        id: &str,
    ) -> Option<AnyModel> {
        of_type(&self.catalog, model_type, Some(provider))
            .into_iter()
            .find(|m| m.id() == id)
    }

    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        _cancel: &CancelToken,
    ) -> ClassifierResult {
        self.calls
            .lock()
            .unwrap()
            .push(("classify", format!("{}/{}", model.provider, model.id)));
        (self.classify)(model.clone(), context.clone()).await
    }

    async fn generate_images(
        &self,
        model: &ImageModel,
        context: &ImagesContext,
        _cancel: &CancelToken,
    ) -> AssistantImages {
        self.calls
            .lock()
            .unwrap()
            .push(("generateImages", format!("{}/{}", model.provider, model.id)));
        (self.images)(model.clone(), context.clone()).await
    }
}

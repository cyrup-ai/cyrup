//! The image-generation subsystem (1:1 port of Pi `packages/ai/src/images*.ts` +
//! `image-models*.ts`).
//!
//! The image-side counterpart of the text [`crate::collection::Models`] /
//! [`crate::provider::Provider`] / [`crate::api`] stack:
//!
//! - [`ImagesModel`] — the model shape (Pi `types.ts:698-703`: a `Model` minus
//!   `reasoning`/`contextWindow`/`maxTokens`/`compat`, plus a required `output` modality list).
//! - [`ImagesContext`] / [`AssistantImages`] — request/response (Pi `types.ts:413-429`).
//! - [`ImagesApiImpl`] + [`ImagesApiRegistry`] — the per-wire-protocol image API + its registry
//!   (Pi `images-api-registry.ts`).
//! - [`ImagesProvider`] + [`create_images_provider`] — an image-generation provider (Pi
//!   `images-models.ts:243`).
//! - [`ImagesModels`] + [`create_images_models`] — the runtime collection with auth application +
//!   generation convenience (Pi `images-models.ts:96-221`).
//! - [`generate_images`] — the registry-routed entrypoint (Pi `images.ts:14`).
//! - [`image_models`]/[`get_image_model`]/[`get_image_models`]/[`get_image_providers`] — the static
//!   generated catalog accessors (Pi `image-models.ts`).

pub mod openrouter;

use crate::auth::{
    resolve_provider_auth, AuthContext, AuthOverrides, AuthResult, CredentialStore,
    EnvAuthContext, InMemoryCredentialStore, ProviderAuth, ProviderEnv,
};
use crate::collection::CreateModelsOptions;
use crate::error::ProviderError;
use crate::model::{Modality, Model, ModelCost, ThinkingLevelMap};
use crate::stream::ProviderResponse;
use crate::HeaderMap;
use cyrup_core::{ApiId, CancelToken, Content, ProviderId, Usage};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

/// The known image wire-protocol id (Pi `KnownImagesApi = "openrouter-images"`, types.ts:28).
pub const OPENROUTER_IMAGES: &str = "openrouter-images";

/// The known image provider id (Pi `KnownImagesProvider = "openrouter"`, types.ts:70).
pub const OPENROUTER_PROVIDER_ID: &str = "openrouter";

/// An image-generation model (Pi `ImagesModel<TApi>`, types.ts:698-703). Structurally a [`Model`]
/// minus `reasoning`/`contextWindow`/`maxTokens`/`compat`, with a REQUIRED `output` modality list
/// and a REQUIRED `base_url` (Pi `baseUrl: string`, inherited from `Model`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesModel {
    pub id: String,
    pub name: String,
    pub api: ApiId,
    pub provider: ProviderId,
    pub base_url: String,
    pub input: Vec<Modality>,
    pub output: Vec<Modality>,
    pub cost: ModelCost,
    /// Per-level reasoning value overrides (Pi `Model.thinkingLevelMap`, inherited). Additive.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    /// Top-level per-provider request headers (Pi `Model.headers`, inherited). Additive.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub headers: Option<HeaderMap>,
}

impl ImagesModel {
    /// `true` if this model emits text alongside images (Pi `model.output.includes("text")`,
    /// openrouter-images.ts:149).
    pub fn outputs_text(&self) -> bool {
        self.output.contains(&Modality::Text)
    }

    /// Build the text-[`Model`] shim auth resolution operates on. Pi's `resolveProviderAuth` is typed
    /// for both `Model` and `ImagesModel` (auth/resolve.ts); cyrup's [`resolve_provider_auth`] takes a
    /// [`Model`], and the standard env-key strategy ignores the model entirely, so this faithful
    /// adapter is auth-equivalent. The image-only fields (`reasoning`/`contextWindow`/`maxTokens`)
    /// default to non-reasoning/zero.
    fn to_auth_model(&self) -> Model {
        Model {
            id: self.id.clone().into(),
            name: self.name.clone(),
            api: self.api.clone(),
            provider: self.provider.clone(),
            base_url: self.base_url.clone(),
            reasoning: false,
            input: self.input.clone(),
            cost: self.cost,
            context_window: 0,
            max_tokens: 0,
            thinking_level_map: self.thinking_level_map.clone(),
            compat: None,
            headers: self.headers.clone(),
        }
    }
}

/// The terminal reason of an image generation (Pi `ImagesStopReason`, types.ts:417).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImagesStopReason {
    Stop,
    Error,
    Aborted,
}

/// Input to a single image-generation call (Pi `ImagesContext`, types.ts:413-415). `input` carries
/// text + image content (Pi `ImagesInputContent = TextContent | ImageContent`, types.ts:410).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesContext {
    pub input: Vec<Content>,
}

/// The result of an image generation (Pi `AssistantImages`, types.ts:419-429).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantImages {
    pub api: ApiId,
    pub provider: ProviderId,
    pub model: String,
    /// Text + image output (Pi `ImagesOutputContent[]`, types.ts:411/423).
    pub output: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage: Option<Usage>,
    pub stop_reason: ImagesStopReason,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error_message: Option<String>,
    /// Unix timestamp in milliseconds (Pi `timestamp`).
    pub timestamp: u64,
}

impl AssistantImages {
    /// A fresh successful-by-default envelope (Pi `output` initializer, openrouter-images.ts:43-50).
    fn new(model: &ImagesModel) -> Self {
        AssistantImages {
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            output: Vec::new(),
            response_id: None,
            usage: None,
            stop_reason: ImagesStopReason::Stop,
            error_message: None,
            timestamp: now_ms(),
        }
    }

    /// The terminal error envelope (Pi `ImagesModels.generateImages` catch, images-models.ts:205).
    fn errored(model: &ImagesModel, message: impl Into<String>, aborted: bool) -> Self {
        AssistantImages {
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            output: Vec::new(),
            response_id: None,
            usage: None,
            stop_reason: if aborted { ImagesStopReason::Aborted } else { ImagesStopReason::Error },
            error_message: Some(message.into()),
            timestamp: now_ms(),
        }
    }
}

/// Inspect or replace an image payload before sending (Pi `ImagesOptions.onPayload`, types.ts:253).
pub type ImagesOnPayload =
    Arc<dyn Fn(&serde_json::Value, &ImagesModel) -> Option<serde_json::Value> + Send + Sync>;

/// Invoked after an HTTP response is received (Pi `ImagesOptions.onResponse`, types.ts:257).
pub type ImagesOnResponse = Arc<dyn Fn(&ProviderResponse, &ImagesModel) + Send + Sync>;

/// Per-request options for image generation (Pi `ImagesOptions`, types.ts:241-285).
#[derive(Clone, Default)]
pub struct ImagesOptions {
    /// Cancellation token (Pi `signal?: AbortSignal`, types.ts:242).
    pub cancel: Option<CancelToken>,
    pub api_key: Option<String>,
    /// Provider-scoped env overlay (Pi `env`, types.ts:248).
    pub env: Option<ProviderEnv>,
    /// Per-request header overlay; a `None` value suppresses a default header (Pi `headers`,
    /// types.ts:263).
    pub headers: Option<HeaderMap>,
    /// HTTP request timeout in milliseconds (Pi `timeoutMs`, types.ts:267).
    pub timeout_ms: Option<u64>,
    /// Max client-side retry attempts (Pi `maxRetries`, types.ts:271).
    pub max_retries: Option<u32>,
    /// Cap on a server-requested retry delay (Pi `maxRetryDelayMs`, types.ts:279).
    pub max_retry_delay_ms: Option<u64>,
    /// Provider-extracted request metadata (Pi `metadata`, types.ts:284).
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    /// Inspect/replace the payload before sending (Pi `onPayload`, types.ts:253).
    pub on_payload: Option<ImagesOnPayload>,
    /// Invoked after a response is received (Pi `onResponse`, types.ts:257).
    pub on_response: Option<ImagesOnResponse>,
}

/// One image wire protocol (Pi `ImagesApiProvider`, images-api-registry.ts:9). Builds the payload,
/// performs the request, and returns the assembled [`AssistantImages`]. Like the text [`crate::api::ApiImpl`],
/// it never returns `Err`: every failure is encoded into the returned envelope with `stop_reason ∈
/// {error, aborted}` (Pi `openrouter-images.ts:100-104`).
#[async_trait::async_trait]
pub trait ImagesApiImpl: Send + Sync {
    fn api(&self) -> &ApiId;

    async fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages;
}

/// Lazily-constructed factory for an [`ImagesApiImpl`] (mirrors [`crate::api::ApiFactory`]).
pub type ImagesApiFactory = fn() -> Arc<dyn ImagesApiImpl>;

/// Maps `ApiId → Arc<dyn ImagesApiImpl>` with lazy get-or-init (Pi `imagesApiProviderRegistry`,
/// images-api-registry.ts:24).
#[derive(Default)]
pub struct ImagesApiRegistry {
    factories: std::collections::HashMap<ApiId, ImagesApiFactory>,
    live: std::collections::HashMap<ApiId, Arc<dyn ImagesApiImpl>>,
}

impl ImagesApiRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a lazy factory (Pi `registerImagesApiProvider`, images-api-registry.ts:38).
    pub fn register(&mut self, api: ApiId, factory: ImagesApiFactory) {
        self.factories.insert(api, factory);
    }

    /// Get-or-init the impl for `api` (Pi `getImagesApiProvider`, images-api-registry.ts:51).
    pub fn get(&mut self, api: &ApiId) -> Option<Arc<dyn ImagesApiImpl>> {
        if let Some(found) = self.live.get(api) {
            return Some(found.clone());
        }
        let factory = self.factories.get(api)?;
        let imp = factory();
        self.live.insert(api.clone(), imp.clone());
        Some(imp)
    }

    pub fn contains(&self, api: &ApiId) -> bool {
        self.live.contains_key(api) || self.factories.contains_key(api)
    }
}

/// A registry pre-seeded with every built-in image wire-protocol factory (Pi
/// `providers/images/register-builtins.ts`).
pub fn images_builtin_registry() -> ImagesApiRegistry {
    let mut reg = ImagesApiRegistry::new();
    register_images_builtins(&mut reg);
    reg
}

/// Register the built-in image wire-protocol factories into `reg`.
pub fn register_images_builtins(reg: &mut ImagesApiRegistry) {
    reg.register(ApiId::from(OPENROUTER_IMAGES), openrouter::factory);
}

/// Registry-routed entrypoint (Pi `generateImages`, images.ts:14-21). Resolves the api impl for
/// `model.api` and delegates; `Err(NoApiImpl)` when no impl is registered (Pi throws
/// `No API provider registered for api: …`).
pub async fn generate_images(
    registry: &mut ImagesApiRegistry,
    model: &ImagesModel,
    context: &ImagesContext,
    options: &ImagesOptions,
) -> Result<AssistantImages, ProviderError> {
    let Some(provider) = registry.get(&model.api) else {
        return Err(ProviderError::NoApiImpl(model.api.clone()));
    };
    Ok(provider.generate_images(model, context, options).await)
}

/// A dynamic image-model refresh source (Pi `CreateImagesProviderOptions.refreshModels?`,
/// images-models.ts:238).
pub type ImagesRefreshFn = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<Vec<ImagesModel>, ProviderError>> + Send>>
        + Send
        + Sync,
>;

/// An image-generation provider: the image-side counterpart of [`crate::provider::Provider`] (Pi
/// `ImagesProvider`, images-models.ts:12).
#[async_trait::async_trait]
pub trait ImagesProvider: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;

    /// The provider's auth strategy (Pi `ImagesProvider.auth`, images-models.ts:21).
    fn provider_auth(&self) -> Option<&ProviderAuth>;

    /// Current known models (Pi `getModels`, images-models.ts:29). Best-effort snapshot clone.
    fn get_models(&self) -> Vec<ImagesModel>;

    /// Dynamic providers only: re-fetch the model list (Pi `refreshModels?`, images-models.ts:36).
    /// `None` for a static provider.
    async fn refresh_models(&self) -> Option<Result<(), ProviderError>> {
        None
    }

    async fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages;
}

/// Inputs to [`create_images_provider`] (Pi `CreateImagesProviderOptions`, images-models.ts:223).
pub struct CreateImagesProviderOptions {
    pub id: String,
    /// Display name; defaults to `id` (Pi `name?`, images-models.ts:226).
    pub name: Option<String>,
    pub auth: ProviderAuth,
    pub models: Vec<ImagesModel>,
    pub refresh_models: Option<ImagesRefreshFn>,
    pub api: Arc<dyn ImagesApiImpl>,
}

/// Build an image-generation provider from parts (Pi `createImagesProvider`, images-models.ts:243).
pub fn create_images_provider(input: CreateImagesProviderOptions) -> BuiltImagesProvider {
    let name = input.name.unwrap_or_else(|| input.id.clone());
    BuiltImagesProvider {
        id: input.id,
        name,
        auth: input.auth,
        models: Arc::new(RwLock::new(input.models)),
        refresh: input.refresh_models,
        dedup: crate::utils::refresh::RefreshDedup::new(),
        api: input.api,
    }
}

/// The provider produced by [`create_images_provider`].
pub struct BuiltImagesProvider {
    id: String,
    name: String,
    auth: ProviderAuth,
    models: Arc<RwLock<Vec<ImagesModel>>>,
    refresh: Option<ImagesRefreshFn>,
    dedup: crate::utils::refresh::RefreshDedup,
    api: Arc<dyn ImagesApiImpl>,
}

#[async_trait::async_trait]
impl ImagesProvider for BuiltImagesProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        Some(&self.auth)
    }
    fn get_models(&self) -> Vec<ImagesModel> {
        self.models.read().map(|m| m.clone()).unwrap_or_default()
    }
    async fn refresh_models(&self) -> Option<Result<(), ProviderError>> {
        let refresh = self.refresh.clone()?;
        let models = Arc::clone(&self.models);
        // Share one in-flight fetch (Pi `inflightRefresh ??=`, images-models.ts:255). On success the
        // catalog is replaced; on failure the list stays at its last-known state and a later call
        // retries (Pi `finally`).
        let result = self
            .dedup
            .run(move || async move {
                let new_models = refresh().await?;
                if let Ok(mut guard) = models.write() {
                    *guard = new_models;
                }
                Ok(())
            })
            .await;
        Some(result)
    }
    async fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages {
        self.api.generate_images(model, context, options).await
    }
}

/// Runtime collection of image-generation providers (Pi `ImagesModels`, images-models.ts:49) with
/// auth application + generation convenience: the image-side [`crate::collection::Models`].
pub struct ImagesModels {
    providers: BTreeMap<String, Arc<dyn ImagesProvider>>,
    credentials: Arc<dyn CredentialStore>,
    auth_context: Arc<dyn AuthContext>,
}

/// Build an empty image collection (Pi `createImagesModels`, images-models.ts:219).
pub fn create_images_models(options: CreateModelsOptions) -> ImagesModels {
    ImagesModels {
        providers: BTreeMap::new(),
        credentials: options
            .credentials
            .unwrap_or_else(|| Arc::new(InMemoryCredentialStore::new())),
        auth_context: options.auth_context.unwrap_or_else(|| Arc::new(EnvAuthContext)),
    }
}

impl ImagesModels {
    /// Upsert/replace by `provider.id` (Pi `setProvider`, images-models.ts:106).
    pub fn set_provider(&mut self, provider: Arc<dyn ImagesProvider>) {
        self.providers.insert(provider.id().to_string(), provider);
    }

    /// Remove a provider by id (Pi `deleteProvider`).
    pub fn delete_provider(&mut self, id: &str) {
        self.providers.remove(id);
    }

    /// Remove every provider (Pi `clearProviders`).
    pub fn clear_providers(&mut self) {
        self.providers.clear();
    }

    /// All registered providers (Pi `getProviders`).
    pub fn get_providers(&self) -> Vec<Arc<dyn ImagesProvider>> {
        self.providers.values().cloned().collect()
    }

    /// One provider by id (Pi `getProvider`).
    pub fn get_provider(&self, id: &str) -> Option<Arc<dyn ImagesProvider>> {
        self.providers.get(id).cloned()
    }

    /// Last-known models from one provider, or all providers (Pi `getModels`, images-models.ts:126).
    pub fn get_models(&self, provider: Option<&str>) -> Vec<ImagesModel> {
        match provider {
            Some(id) => self.providers.get(id).map(|p| p.get_models()).unwrap_or_default(),
            None => self.providers.values().flat_map(|p| p.get_models()).collect(),
        }
    }

    /// Runtime model lookup against last-known lists (Pi `getModel`, images-models.ts:148).
    pub fn get_model(&self, provider: &str, id: &str) -> Option<ImagesModel> {
        self.get_models(Some(provider)).into_iter().find(|m| m.id == id)
    }

    /// Ask dynamic providers to re-fetch their model lists (Pi `refresh`, images-models.ts:152).
    pub async fn refresh(&self, provider: Option<&str>) -> Result<(), ProviderError> {
        if let Some(id) = provider {
            let Some(entry) = self.providers.get(id) else {
                return Ok(());
            };
            return match entry.refresh_models().await {
                None => Ok(()),
                Some(Ok(())) => Ok(()),
                Some(Err(e @ ProviderError::ModelSource(_))) => Err(e),
                Some(Err(e)) => Err(ProviderError::ModelSource(
                    format!("Model refresh failed for {id}: {e}").into(),
                )),
            };
        }
        let refreshes = self.providers.values().map(|p| p.refresh_models());
        futures::future::join_all(refreshes).await;
        Ok(())
    }

    /// Resolve request auth for an image model (Pi `getAuth`, images-models.ts:170).
    pub async fn get_auth(
        &self,
        model: &ImagesModel,
    ) -> Result<Option<AuthResult>, ProviderError> {
        let Some(provider) = self.providers.get(model.provider.as_str()) else {
            return Ok(None);
        };
        let Some(auth) = provider.provider_auth() else {
            return Ok(None);
        };
        Ok(resolve_provider_auth(
            &model.provider,
            auth,
            &model.to_auth_model(),
            self.credentials.as_ref(),
            self.auth_context.as_ref(),
            AuthOverrides::default(),
        )
        .await?)
    }

    /// Generate images through the owning provider with auth resolved + merged (Pi `generateImages`,
    /// images-models.ts:176). Never returns `Err`: failures are encoded into an [`AssistantImages`]
    /// with `stop_reason: error` (Pi catch, images-models.ts:205-215).
    pub async fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages {
        match self.try_generate(model, context, options).await {
            Ok(out) => out,
            Err(message) => AssistantImages::errored(model, message, false),
        }
    }

    /// The fallible inner path (Pi try-body, images-models.ts:181-204); its `Err(String)` becomes the
    /// `errorMessage` of the terminal error envelope.
    async fn try_generate(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> Result<AssistantImages, String> {
        let Some(provider) = self.providers.get(model.provider.as_str()) else {
            return Err(format!("Unknown provider: {}", model.provider));
        };

        // Resolve auth with the per-request key/env overlay (Pi `resolveProviderAuth(…, {apiKey, env})`).
        let resolution = match provider.provider_auth() {
            Some(auth) => resolve_provider_auth(
                &model.provider,
                auth,
                &model.to_auth_model(),
                self.credentials.as_ref(),
                self.auth_context.as_ref(),
                AuthOverrides { api_key: options.api_key.as_deref(), env: options.env.as_ref() },
            )
            .await
            .map_err(|e| e.to_string())?,
            None => None,
        };

        let Some(resolution) = resolution else {
            // Unconfigured: delegate as-is (Pi `if (!auth) return provider.generateImages(...)`).
            return Ok(provider.generate_images(model, context, options).await);
        };
        let auth = &resolution.auth;

        // `auth.baseUrl ? { ...model, baseUrl: auth.baseUrl } : model` (images-models.ts:196).
        let mut request_model = model.clone();
        if let Some(base_url) = &auth.base_url {
            request_model.base_url = base_url.clone();
        }

        // Explicit request options win per-field; headers/env merge per key (images-models.ts:199-202).
        let api_key = options.api_key.clone().or_else(|| auth.api_key.clone());
        let headers = merge_headers(auth.headers.as_ref(), options.headers.as_ref());
        let env = merge_env(resolution.env.as_ref(), options.env.as_ref());

        let request_options = ImagesOptions {
            api_key,
            headers,
            env,
            ..options.clone()
        };
        Ok(provider.generate_images(&request_model, context, &request_options).await)
    }
}

/// Merge auth headers with request headers; request headers win per key (Pi spread
/// `{ ...auth.headers, ...options.headers }`, images-models.ts:200). `None` when neither side has any.
fn merge_headers(
    auth_headers: Option<&HeaderMap>,
    option_headers: Option<&HeaderMap>,
) -> Option<HeaderMap> {
    if auth_headers.is_none() && option_headers.is_none() {
        return None;
    }
    let mut merged = HeaderMap::new();
    if let Some(h) = auth_headers {
        merged.extend(h.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    if let Some(h) = option_headers {
        merged.extend(h.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    Some(merged)
}

/// Merge resolution env with request env; request env wins per key (Pi spread
/// `{ ...(resolution.env ?? {}), ...(options.env ?? {}) }`, images-models.ts:201-202).
fn merge_env(
    resolution_env: Option<&ProviderEnv>,
    option_env: Option<&ProviderEnv>,
) -> Option<ProviderEnv> {
    if resolution_env.is_none() && option_env.is_none() {
        return None;
    }
    let mut merged = ProviderEnv::new();
    if let Some(e) = resolution_env {
        merged.extend(e.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    if let Some(e) = option_env {
        merged.extend(e.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    Some(merged)
}

/// Unix-epoch milliseconds (Pi `Date.now()`); never panics (a pre-epoch clock yields 0).
pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---- static generated catalog (Pi `image-models.ts` + `image-models.generated.ts`) ----

/// The verbatim image catalog extracted from Pi's generated `image-models.generated.ts`.
const OPENROUTER_IMAGES_CATALOG_JSON: &str =
    include_str!("../providers/catalog/openrouter-images.json");

/// The full openrouter image catalog (1:1 with Pi `IMAGE_MODELS.openrouter`). A parse failure yields
/// an empty catalog (surfaced by the catalog-count test) rather than a panic.
pub fn openrouter_image_models() -> Vec<ImagesModel> {
    serde_json::from_str(OPENROUTER_IMAGES_CATALOG_JSON).unwrap_or_default()
}

/// All known image models, grouped by provider (Pi `IMAGE_MODELS`, image-models.generated.ts:6).
pub fn image_models() -> BTreeMap<String, Vec<ImagesModel>> {
    let mut out = BTreeMap::new();
    out.insert(OPENROUTER_PROVIDER_ID.to_string(), openrouter_image_models());
    out
}

/// All providers in the static catalog (Pi `getImageProviders`, image-models.ts:31).
pub fn get_image_providers() -> Vec<String> {
    image_models().into_keys().collect()
}

/// All static catalog models for a provider (Pi `getImageModels`, image-models.ts:35).
pub fn get_image_models(provider: &str) -> Vec<ImagesModel> {
    image_models().remove(provider).unwrap_or_default()
}

/// One static catalog model by provider + id (Pi `getImageModel`, image-models.ts:23).
pub fn get_image_model(provider: &str, model_id: &str) -> Option<ImagesModel> {
    get_image_models(provider).into_iter().find(|m| m.id == model_id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::auth::env_key;
    use std::collections::BTreeMap as Map;

    struct MapEnv(Map<String, String>);
    #[async_trait::async_trait]
    impl AuthContext for MapEnv {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    fn sample_model() -> ImagesModel {
        get_image_model("openrouter", "google/gemini-2.5-flash-image").expect("nano banana")
    }

    #[test]
    fn catalog_parses_verbatim_with_expected_count() {
        let models = openrouter_image_models();
        assert_eq!(models.len(), 37);
        assert!(models.iter().all(|m| m.api.as_str() == OPENROUTER_IMAGES));
        assert!(models.iter().all(|m| m.provider.as_str() == "openrouter"));
        assert!(models.iter().all(|m| m.base_url == "https://openrouter.ai/api/v1"));
        // Every image model emits image output.
        assert!(models.iter().all(|m| m.output.contains(&Modality::Image)));
    }

    #[test]
    fn catalog_accessors_match_pi() {
        assert_eq!(get_image_providers(), vec!["openrouter".to_string()]);
        assert_eq!(get_image_models("openrouter").len(), 37);
        assert!(get_image_model("openrouter", "openai/gpt-image-1").is_some());
        assert!(get_image_model("openrouter", "nope").is_none());
        assert!(get_image_model("nope", "x").is_none());
        // Nano Banana emits both text + image; FLUX is image-only.
        assert!(sample_model().outputs_text());
        assert!(!get_image_model("openrouter", "black-forest-labs/flux.2-flex")
            .unwrap()
            .outputs_text());
    }

    #[test]
    fn registry_lazily_resolves_openrouter_images() {
        let mut reg = images_builtin_registry();
        assert!(reg.contains(&ApiId::from(OPENROUTER_IMAGES)));
        assert!(reg.get(&ApiId::from(OPENROUTER_IMAGES)).is_some());
        assert!(reg.get(&ApiId::from("nope")).is_none());
    }

    #[test]
    fn assistant_images_roundtrips_camelcase() {
        let mut a = AssistantImages::new(&sample_model());
        a.response_id = Some("gen-1".into());
        a.output.push(Content::text("hi"));
        let v = serde_json::to_value(&a).expect("serialize");
        assert_eq!(v["responseId"], "gen-1");
        assert_eq!(v["stopReason"], "stop");
        assert_eq!(v["output"][0]["type"], "text");
        let back: AssistantImages = serde_json::from_value(v).expect("roundtrip");
        assert_eq!(back, a);
    }

    fn openrouter_provider_for_tests() -> Arc<dyn ImagesProvider> {
        Arc::new(create_images_provider(CreateImagesProviderOptions {
            id: "openrouter".into(),
            name: Some("OpenRouter".into()),
            auth: ProviderAuth::with_api_key(env_key(["OPENROUTER_API_KEY"])),
            models: openrouter_image_models(),
            refresh_models: None,
            api: openrouter::factory(),
        }))
    }

    #[tokio::test]
    async fn collection_set_get_and_auth() {
        let mut models = create_images_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(MapEnv(Map::from([(
                "OPENROUTER_API_KEY".to_string(),
                "sk-or".to_string(),
            )])))),
        });
        models.set_provider(openrouter_provider_for_tests());
        assert_eq!(models.get_providers().len(), 1);
        assert!(models.get_provider("openrouter").is_some());
        assert_eq!(models.get_models(Some("openrouter")).len(), 37);

        let m = sample_model();
        let auth = models.get_auth(&m).await.expect("ok").expect("configured");
        assert_eq!(auth.auth.api_key.as_deref(), Some("sk-or"));
        assert_eq!(auth.source.as_deref(), Some("env"));

        // Unknown provider → Ok(None).
        let mut unknown = m.clone();
        unknown.provider = "nope".into();
        assert!(models.get_auth(&unknown).await.expect("ok").is_none());
    }

    #[tokio::test]
    async fn generate_unknown_provider_yields_error_envelope() {
        let models = create_images_models(CreateModelsOptions::default());
        let m = sample_model();
        let out = models.generate_images(&m, &ImagesContext::default(), &ImagesOptions::default()).await;
        assert_eq!(out.stop_reason, ImagesStopReason::Error);
        assert!(out.error_message.unwrap().contains("Unknown provider"));
    }

    #[tokio::test]
    async fn generate_applies_auth_then_reaches_transport() {
        // Configured env key resolves; an unroutable base URL proves the wire impl ran (an
        // unconfigured provider would short-circuit with "No API key").
        let mut models = create_images_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(MapEnv(Map::from([(
                "OPENROUTER_API_KEY".to_string(),
                "sk-or".to_string(),
            )])))),
        });
        models.set_provider(openrouter_provider_for_tests());
        let mut m = sample_model();
        m.base_url = "http://127.0.0.1:1/api/v1".into();
        let ctx = ImagesContext { input: vec![Content::text("a red square")] };
        let out = models.generate_images(&m, &ctx, &ImagesOptions::default()).await;
        assert_eq!(out.stop_reason, ImagesStopReason::Error);
        let err = out.error_message.unwrap();
        assert!(!err.contains("No API key"), "auth should have resolved: {err}");
    }
}

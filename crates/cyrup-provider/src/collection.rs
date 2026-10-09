//! The `Models` runtime collection (1:1 port of Pi `packages/ai/src/models.ts`).
//!
//! A registry of [`Provider`]s plus auth application and stream convenience. Providers own stream
//! behavior; `Models` resolves request auth against its own credential store + auth context and
//! delegates each request to the provider that owns the model (`createModels`, `setProvider`,
//! `getProvider(s)`, `getModel(s)`, `getAuth`, `applyAuth`, `stream`/`complete`, `refresh`). Also
//! ports the model-capability helpers (`getSupportedThinkingLevels`, `clampThinkingLevel`,
//! `modelsAreEqual`, `hasApi`).

use crate::api::channel;
use crate::auth::{
    AuthContext, AuthOverrides, AuthResult, CredentialStore, EnvAuthContext,
    InMemoryCredentialStore, ProviderEnv, resolve_provider_auth,
};
use crate::classifier::{
    AnyModel, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, ImageModel,
    ModelType,
};
use crate::context::Context;
use crate::error::ProviderError;
use crate::images::{AssistantImages, ImagesContext, ImagesOptions};
use crate::model::Model;
use crate::provider::Provider;
use crate::stream::{StreamEvent, StreamOptions, collect_message};
use crate::utils::simple_options::SimpleStreamOptions;
use cyrup_core::{AssistantMessage, EventStream, ModelThinkingLevel};
use futures::StreamExt;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

/// Channel buffer for the collection's lazy auth-applying stream bridge.
const STREAM_BUFFER: usize = 64;

/// Options for [`create_models`] (Pi `CreateModelsOptions`). All default: an empty in-memory
/// credential store, the real-env auth context, and no remote catalog overlay.
#[derive(Clone, Default)]
pub struct CreateModelsOptions {
    pub credentials: Option<Arc<dyn CredentialStore>>,
    pub auth_context: Option<Arc<dyn AuthContext>>,
    /// The persisted pi.dev model-catalog overlay to merge over the compiled-in catalogs
    /// (DRIFT-007; Pi threads its equivalent through `ModelRuntime`'s `modelsStore` +
    /// `withRemoteCatalog`, `model-runtime.ts:139-151`).
    ///
    /// `None` — the default and what every pre-DRIFT-007 caller gets — means "embedded catalogs
    /// only", which is exactly today's behavior. The overlay can only ADD or REPLACE models by id
    /// (see [`crate::remote_catalog::merge_models`]); it can never remove one, so a failed, disabled
    /// or offline refresh is indistinguishable from `None`.
    pub catalog_overlay: Option<Arc<crate::remote_catalog::CatalogOverlay>>,
}

/// Options for [`Models::refresh_with`] — pi `ModelsRefreshOptions` (`models.ts:46-51` @v0.83.0).
/// PROV-S05.
///
/// [`Default`] reproduces pi's defaults for a bare `refresh()`: `allowNetwork = options.allowNetwork
/// ?? true` (`:277`), `force` undefined ⇒ falsy, and a fresh, never-cancelled token standing in for
/// an absent `signal`.
#[derive(Clone, Debug)]
pub struct ModelsRefreshOptions {
    /// pi `allowNetwork?: boolean` (`:47`). `false` is the cache-only restore.
    pub allow_network: bool,
    /// pi `force?: boolean` (`:49`) — "bypass provider freshness checks and fetch immediately when
    /// network access is allowed". Forwarded to every provider; NOT forwarded to the post-failure
    /// cache restore, which pi builds without it (`:314-319`).
    pub force: bool,
    /// pi `signal?: AbortSignal` (`:50`).
    pub cancel: cyrup_core::CancelToken,
}

impl Default for ModelsRefreshOptions {
    fn default() -> Self {
        Self {
            allow_network: true,
            force: false,
            cancel: cyrup_core::CancelToken::new(),
        }
    }
}

impl ModelsRefreshOptions {
    /// Network allowed, freshness window respected — the background posture.
    pub fn network() -> Self {
        Self::default()
    }

    /// The offline restore (pi `refresh({ allowNetwork: false })`,
    /// `agent-session-services.ts:180`).
    pub fn cache_only() -> Self {
        Self {
            allow_network: false,
            ..Self::default()
        }
    }

    /// Bypass the freshness window (pi `force: true`, `package-manager-cli.ts:409`).
    pub fn forced() -> Self {
        Self {
            force: true,
            ..Self::default()
        }
    }

    /// Attach the caller's abort token (pi's `signal`).
    #[must_use]
    pub fn with_cancel(mut self, cancel: cyrup_core::CancelToken) -> Self {
        self.cancel = cancel;
        self
    }
}

/// The outcome of [`Models::refresh_with`] — pi `ModelsRefreshResult` (`models.ts:53-56` @v0.83.0),
/// returned rather than thrown so a per-provider failure never rejects the whole refresh
/// (`:304-312`). PROV-S05.
#[derive(Debug, Default)]
pub struct ModelsRefreshResult {
    /// pi `aborted: boolean` (`:54`) — `options.signal?.aborted ?? false`, read AFTER every
    /// provider has settled (`:327`), so it reports whether the caller cancelled at any point, not
    /// whether cancellation was observed by a particular provider.
    pub aborted: bool,
    /// pi `errors: ReadonlyMap<string, Error>` (`:55`) — one entry per provider whose refresh threw,
    /// keyed by provider id, exactly as upstream keys it (`errors.set(provider.id, …)`, `:306`). A
    /// provider aborted by `cancel` contributes NO entry, because pi guards the `errors.set` with
    /// `if (!options.signal?.aborted)` (`:305`).
    pub errors: BTreeMap<String, ProviderError>,
}

impl ModelsRefreshResult {
    /// No provider failed and nobody cancelled.
    pub fn is_clean(&self) -> bool {
        !self.aborted && self.errors.is_empty()
    }

    /// The error recorded for one provider, if any.
    pub fn error_for(&self, provider: &str) -> Option<&ProviderError> {
        self.errors.get(provider)
    }
}

/// Runtime collection of providers plus auth application + stream convenience (Pi `MutableModels`).
/// Providers are held by id (ids are unique; `set_provider` upserts).
pub struct Models {
    providers: BTreeMap<String, Arc<dyn Provider>>,
    credentials: Arc<dyn CredentialStore>,
    auth_context: Arc<dyn AuthContext>,
}

/// Build an empty collection (Pi `createModels`).
pub fn create_models(options: CreateModelsOptions) -> Models {
    Models {
        providers: BTreeMap::new(),
        credentials: options
            .credentials
            .unwrap_or_else(|| Arc::new(InMemoryCredentialStore::new())),
        auth_context: options
            .auth_context
            .unwrap_or_else(|| Arc::new(EnvAuthContext)),
    }
}

impl Models {
    // ---- MutableModels (Pi `models.ts:189-194` @v0.83.0; PROV-041 corrected `:130`, blank) ----

    /// Upsert/replace by `provider.id` (Pi `setProvider`). Ids are unique.
    pub fn set_provider(&mut self, provider: Arc<dyn Provider>) {
        self.providers
            .insert(provider.id().as_str().to_string(), provider);
    }

    /// Remove a provider by id (Pi `deleteProvider`).
    pub fn delete_provider(&mut self, id: &str) {
        self.providers.remove(id);
    }

    /// Remove every provider (Pi `clearProviders`).
    pub fn clear_providers(&mut self) {
        self.providers.clear();
    }

    // ---- reads (Pi `getModels` `models.ts:135` / `getModel` `:141` @v0.83.0; PROV-041 corrected
    // `:164`, which is `getAuth`) ----

    /// All registered providers (Pi `getProviders`).
    pub fn get_providers(&self) -> Vec<Arc<dyn Provider>> {
        self.providers.values().cloned().collect()
    }

    /// One provider by id (Pi `getProvider`).
    pub fn get_provider(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(id).cloned()
    }

    /// Last-known models from one provider, or all providers (Pi `getModels`). Best-effort: our
    /// `Provider::models()` is infallible (it cannot throw), matching Pi's catch-and-skip contract.
    pub fn get_models(&self, provider: Option<&str>) -> Vec<Model> {
        match provider {
            Some(id) => self
                .providers
                .get(id)
                .map(|p| p.models().to_vec())
                .unwrap_or_default(),
            None => self
                .providers
                .values()
                .flat_map(|p| p.models().to_vec())
                .collect(),
        }
    }

    /// Runtime model lookup against last-known lists (Pi `getModel`).
    pub fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
        self.get_models(Some(provider))
            .into_iter()
            .find(|m| m.id.as_str() == id)
    }

    // ---- multi-type reads (Pi `getAllModels` `models.ts:446-463`, `getModelsOfType` `:468-470`,
    // `getModelOfType` `:476-478` @v0.99.2-17). All three types are reachable here as of PROV-128:
    // `openrouter` lists its image rows through [`Provider::get_all_models`]
    // ([`crate::providers::openrouter`]), and the pi.dev overlay contributes image and classifier
    // rows for any provider the live catalog serves them for
    // ([`crate::remote_catalog::RemoteCatalogProvider`]). ----

    /// Last-known models of every type from one provider, or all providers (Pi `getAllModels`).
    /// Each provider contributes [`Provider::get_all_models`], whose default is its chat catalog
    /// (`entry.getAllModels?.() ?? entry.getModels()`, `:451`, `:460`).
    pub fn get_all_models(&self, provider: Option<&str>) -> Vec<AnyModel> {
        match provider {
            Some(id) => self
                .providers
                .get(id)
                .map(|p| p.get_all_models())
                .unwrap_or_default(),
            None => self
                .providers
                .values()
                .flat_map(|p| p.get_all_models())
                .collect(),
        }
    }

    /// The models of one type (Pi `getModelsOfType`, `:468-470`).
    pub fn get_models_of_type(
        &self,
        model_type: ModelType,
        provider: Option<&str>,
    ) -> Vec<AnyModel> {
        self.get_all_models(provider)
            .into_iter()
            .filter(|m| m.model_type() == model_type)
            .collect()
    }

    /// One model of a type by provider and id (Pi `getModelOfType`, `:476-478`). Ids are unique
    /// within a type, not across types.
    pub fn get_model_of_type(
        &self,
        model_type: ModelType,
        provider: &str,
        id: &str,
    ) -> Option<AnyModel> {
        self.get_models_of_type(model_type, Some(provider))
            .into_iter()
            .find(|m| m.id() == id)
    }

    /// The image models of one provider, or all providers (Pi `getModelsOfType("image")`).
    /// PROV-128.
    pub fn get_image_models(&self, provider: Option<&str>) -> Vec<ImageModel> {
        self.get_all_models(provider)
            .into_iter()
            .filter_map(AnyModel::into_image)
            .collect()
    }

    /// One image model by provider and id (Pi `getModelOfType("image", ...)`). PROV-128.
    pub fn get_image_model(&self, provider: &str, id: &str) -> Option<ImageModel> {
        self.get_image_models(Some(provider))
            .into_iter()
            .find(|m| m.id.as_str() == id)
    }

    /// The classifier models of one provider, or all providers (Pi `getModelsOfType("classifier")`).
    pub fn get_classifier_models(&self, provider: Option<&str>) -> Vec<ClassifierModel> {
        self.get_all_models(provider)
            .into_iter()
            .filter_map(AnyModel::into_classifier)
            .collect()
    }

    /// One classifier model by provider and id (Pi `getModelOfType("classifier", ...)`).
    pub fn get_classifier_model(&self, provider: &str, id: &str) -> Option<ClassifierModel> {
        self.get_classifier_models(Some(provider))
            .into_iter()
            .find(|m| m.id.as_str() == id)
    }

    // ---- auth (Pi `getAuth` declared `models.ts:164-165`, implemented `:411-429` @v0.83.0;
    // PROV-041 corrected `:216`, the closing brace of `mergeHeaders`) ----

    /// Resolve request auth for a model (Pi `getAuth(model)` with no overrides). `Ok(None)` when the
    /// provider is unknown or unconfigured; `Err` carries the R-01-017 taxonomy (e.g. `oauth` on a
    /// failed token refresh — the stored credential is preserved for re-login).
    pub async fn get_auth(&self, model: &Model) -> Result<Option<AuthResult>, ProviderError> {
        self.get_auth_with(model, AuthOverrides::default()).await
    }

    /// [`Self::get_auth`] with per-request overrides (Pi `getAuth(model, overrides)`, `models.ts:165`
    /// declared / `:413-429` implemented @v0.83.0
    /// / `ModelRuntimeAuthOverrides`, model-runtime.ts:72-77). `min_oauth_validity_ms` is how a
    /// caller that needs a token to survive past the request — Pi's bearer-token export,
    /// credential-print.ts:122-125 — widens the OAuth refresh window and gets the post-refresh
    /// contract enforced.
    pub async fn get_auth_with(
        &self,
        model: &Model,
        overrides: AuthOverrides<'_>,
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
            model,
            self.credentials.as_ref(),
            self.auth_context.as_ref(),
            overrides,
        )
        .await?)
    }

    /// Resolve request auth for a model of ANY type — pi's `getAuth(model: AnyModel, overrides?)`
    /// overload (`models.ts:734-753` @v1.0.1), which is typed for every model shape.
    ///
    /// PROV-128 — [`Models::get_auth`] takes a chat [`Model`], and the deleted parallel images tree
    /// carried its own `ImagesModels::get_auth(&ImagesModel)`. Retiring that tree without this
    /// would have been a lost ability: there would be no way to ask the collection for an image
    /// model's credential at all. Routing every type through one method is upstream's shape, and it
    /// closes the same hole for [`ClassifierModel`], which never had a public accessor either.
    ///
    /// `Ok(None)` when the provider is unknown or unconfigured, exactly as [`Models::get_auth`].
    pub async fn get_any_auth(
        &self,
        model: &AnyModel,
    ) -> Result<Option<AuthResult>, ProviderError> {
        self.get_any_auth_with(model, AuthOverrides::default())
            .await
    }

    /// [`Models::get_any_auth`] with per-request overrides.
    ///
    /// Non-chat models resolve through their `to_auth_model()` shim, which is what
    /// [`Models::generate_images`] and [`Models::classify`] already pass to `resolve_provider_auth`
    /// — pi types `resolveProviderAuth` for every model shape and cyrup's takes a [`Model`], so the
    /// shim is the port of that typing, not an approximation of it (strategies read only identity,
    /// `base_url` and `headers`).
    pub async fn get_any_auth_with(
        &self,
        model: &AnyModel,
        overrides: AuthOverrides<'_>,
    ) -> Result<Option<AuthResult>, ProviderError> {
        match model {
            AnyModel::Chat(m) => self.get_auth_with(m, overrides).await,
            AnyModel::Image(m) => self.get_auth_with(&m.to_auth_model(), overrides).await,
            AnyModel::Classifier(m) => self.get_auth_with(&m.to_auth_model(), overrides).await,
        }
    }

    // ---- stream / complete (Pi `stream` declared `models.ts:173-177` / implemented `:489-502`,
    // `complete` `:179-183` / `:504-510` @v0.83.0; PROV-041 corrected `:258`, which is inside
    // `getModels`). applyAuth lives on [`AuthHelper`] so the spawned stream task can own its
    // inputs (Pi `applyAuth`, `models.ts:463-487`; PROV-041 corrected `:230`, `setProvider`). ----

    /// Stream a model through the owning provider, applying auth first (Pi `stream`). Returns
    /// immediately; auth application happens behind the stream and any failure arrives as a terminal
    /// [`StreamEvent::Error`] (never thrown).
    pub fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let (sink, rx) = channel(STREAM_BUFFER);

        let provider = self.providers.get(model.provider.as_str()).cloned();
        let credentials = self.credentials.clone();
        let auth_context = self.auth_context.clone();
        let provider_id = model.provider.clone();
        let model_owned = model.clone();
        let context = context.clone();
        let options = options.clone();

        tokio::spawn(async move {
            let model_id = model_owned.id.as_str().to_string();
            let api = Some(model_owned.api.clone());

            let Some(provider) = provider else {
                let err = ProviderError::UnknownProvider(provider_id.clone());
                sink.send(err.into_error_event(provider_id, &model_id, api))
                    .await;
                return;
            };

            // applyAuth (re-resolves against the collection store/ctx).
            let helper = AuthHelper {
                provider: provider.clone(),
                credentials,
                auth_context,
            };
            let (request_model, request_options) =
                match helper.apply_auth(&model_owned, &options).await {
                    Ok(v) => v,
                    Err(e) => {
                        sink.send(e.into_error_event(provider_id, &model_id, api))
                            .await;
                        return;
                    }
                };

            // Delegate to the provider and forward every event.
            let mut stream = provider.stream(&request_model, &context, &request_options);
            while let Some(event) = stream.next().await {
                if !sink.send(event).await {
                    break;
                }
            }
        });

        Box::pin(ReceiverStream::new(rx))
    }

    /// Stream to completion (Pi `complete`): drive the stream and fold into the terminal message.
    pub async fn complete(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> AssistantMessage {
        collect_message(self.stream(model, context, options)).await
    }

    /// Stream a model through the owning provider with the unified "simple" option surface,
    /// applying auth first (Pi `streamSimple`, `models.ts:185` declared / `:512-518`
    /// implemented @v0.83.0; PROV-041 corrected `:278`). Like [`Models::stream`], returns
    /// immediately and surfaces every failure (unknown provider, auth) as a terminal
    /// [`StreamEvent::Error`]. Auth is applied to the wrapped [`StreamOptions`] (`base`) exactly as
    /// for `stream`; the unified `reasoning`/`thinking_budgets` ride through untouched to the
    /// provider's [`Provider::stream_simple`].
    pub fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> EventStream<StreamEvent> {
        let (sink, rx) = channel(STREAM_BUFFER);

        let provider = self.providers.get(model.provider.as_str()).cloned();
        let credentials = self.credentials.clone();
        let auth_context = self.auth_context.clone();
        let provider_id = model.provider.clone();
        let model_owned = model.clone();
        let context = context.clone();
        let options = options.clone();

        tokio::spawn(async move {
            let model_id = model_owned.id.as_str().to_string();
            let api = Some(model_owned.api.clone());

            let Some(provider) = provider else {
                let err = ProviderError::UnknownProvider(provider_id.clone());
                sink.send(err.into_error_event(provider_id, &model_id, api))
                    .await;
                return;
            };

            // applyAuth on the wrapped StreamOptions (Pi `applyAuth<SimpleStreamOptions>`).
            let helper = AuthHelper {
                provider: provider.clone(),
                credentials,
                auth_context,
            };
            let (request_model, request_base) =
                match helper.apply_auth(&model_owned, &options.base).await {
                    Ok(v) => v,
                    Err(e) => {
                        sink.send(e.into_error_event(provider_id, &model_id, api))
                            .await;
                        return;
                    }
                };
            let request_options = SimpleStreamOptions {
                base: request_base,
                reasoning: options.reasoning,
                thinking_budgets: options.thinking_budgets,
            };

            let mut stream = provider.stream_simple(&request_model, &context, &request_options);
            while let Some(event) = stream.next().await {
                if !sink.send(event).await {
                    break;
                }
            }
        });

        Box::pin(ReceiverStream::new(rx))
    }

    /// Stream to completion with the unified "simple" options (Pi `completeSimple`, `models.ts:186` declared /
    /// `:520-526` implemented @v0.83.0; PROV-041 corrected `:286`):
    /// drive [`Models::stream_simple`] and fold into the terminal message.
    pub async fn complete_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> AssistantMessage {
        collect_message(self.stream_simple(model, context, options)).await
    }

    /// Generate images through the owning provider, applying request auth first (pi
    /// `Models.generateImages`, `models.ts:948-963` @v1.0.1). PROV-128.
    ///
    /// **Never fails as a call**: an unknown provider (`Unknown provider: ${model.provider}`), a
    /// provider that is not configured (`Provider is not configured: ...`), a failed auth
    /// resolution, and a provider without image generation
    /// ([`Provider::generate_images`]'s default) all come back as an error [`AssistantImages`],
    /// marked `aborted` when the request was cancelled (`imageErrorResult(model, error,
    /// options?.signal?.aborted)`, `:962`).
    ///
    /// Support is checked BEFORE auth, as pi does (`:956-959`): a provider that cannot generate
    /// images ([`Provider::supports_image_generation`]) is answered without a credential read.
    ///
    /// Auth is applied exactly as [`Models::classify`] applies it — pi's one `applyAuth` serves
    /// every operation (`:837-869`).
    pub async fn generate_images(
        &self,
        model: &ImageModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages {
        let Some(provider) = self.providers.get(model.provider.as_str()) else {
            return AssistantImages::errored(
                model,
                format!("Unknown provider: {}", model.provider),
                options.is_aborted(),
            );
        };
        // `if (!provider.generateImages) throw ...` precedes `applyAuth` (`models.ts:956-959`).
        if !provider.supports_image_generation() {
            return AssistantImages::errored(
                model,
                format!(
                    "Provider {} does not support image generation",
                    model.provider
                ),
                options.is_aborted(),
            );
        }
        let (request_model, request_options) = match self
            .apply_images_auth(provider.as_ref(), model, options)
            .await
        {
            Ok(request) => request,
            Err(message) => {
                return AssistantImages::errored(model, message, options.is_aborted());
            }
        };
        provider
            .generate_images(&request_model, context, &request_options)
            .await
    }

    /// Pi `applyAuth` (`models.ts:837-869`) for an image request; the `Err` string is the error
    /// envelope's message. Identical in every clause to [`Models::apply_classifier_auth`] —
    /// upstream has ONE `applyAuth` for every operation, so the two must not drift.
    async fn apply_images_auth(
        &self,
        provider: &dyn Provider,
        model: &ImageModel,
        options: &ImagesOptions,
    ) -> Result<(ImageModel, ImagesOptions), String> {
        let resolution = match provider.provider_auth() {
            Some(auth_strategy) => {
                let resolved = resolve_provider_auth(
                    &model.provider,
                    auth_strategy,
                    &model.to_auth_model(),
                    self.credentials.as_ref(),
                    self.auth_context.as_ref(),
                    AuthOverrides {
                        api_key: options.api_key.as_deref(),
                        env: options.env.as_ref(),
                        min_oauth_validity_ms: None,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
                Some(
                    resolved
                        .ok_or_else(|| format!("Provider is not configured: {}", model.provider))?,
                )
            }
            None => None,
        };
        let (auth, resolution_env) = match &resolution {
            Some(r) => (Some(&r.auth), r.env.as_ref()),
            None => (None, None),
        };

        let mut request_model = model.clone();
        if let Some(base_url) = auth.and_then(|a| a.base_url.as_ref()) {
            request_model.base_url = base_url.clone();
        }
        let mut request_options = options.clone();
        request_options.api_key = options
            .api_key
            .clone()
            .or_else(|| auth.and_then(|a| a.api_key.clone()));
        let auth_headers = merge_headers(
            auth.and_then(|a| a.headers.as_ref()),
            model.headers.as_ref(),
        );
        let mut headers = merge_headers(auth_headers.as_ref(), options.headers.as_ref());
        if let Some(transform) = &options.transform_headers {
            headers = Some(transform(headers.unwrap_or_default()).await);
        }
        request_options.headers = headers;
        request_options.transform_headers = None;
        request_options.env = merge_env(resolution_env, options.env.as_ref());
        Ok((request_model, request_options))
    }

    /// Classify structured state through the owning provider, applying request auth first (Pi
    /// `Models.classify`, `models.ts:966-982` @v0.99.2-17; the registry's pass-through,
    /// `core/model-registry.ts:168-173`).
    ///
    /// **Never fails as a call**: an unknown provider (`Unknown provider: ${model.provider}`,
    /// `:824-829`), a provider that is not configured (`Provider is not configured: ...`,
    /// `:853-855`), a failed auth resolution, and a provider without classification
    /// ([`Provider::classify`]'s default) all come back as an error [`ClassifierResult`], marked
    /// `aborted` when the request was cancelled (`classifierErrorResult(model, error,
    /// options?.signal?.aborted)`, `:980`).
    ///
    /// Support is checked BEFORE auth, as pi does (`:974-977`): a provider that cannot classify
    /// ([`Provider::supports_classification`]) is answered without a credential read.
    ///
    /// Auth is applied the way pi's `applyAuth` does for any operation (`:837-869`): the explicit
    /// `api_key` wins over the resolved one, the model's own headers fold over the resolved ones
    /// (`getAuth`, `:746-752`), `headers` and `env` merge per key with the request
    /// side winning, a resolved base URL replaces the model's, `transform_headers` runs last over
    /// the merged headers and is then stripped from what the provider receives. A provider that
    /// returns no [`Provider::provider_auth`] encapsulates its own auth: nothing is resolved or
    /// required, and the request options pass through (still with the transform applied and
    /// stripped).
    pub async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult {
        let Some(provider) = self.providers.get(model.provider.as_str()) else {
            return ClassifierResult::errored(
                model,
                format!("Unknown provider: {}", model.provider),
                options.is_aborted(),
            );
        };
        // `if (!provider.classify) throw ...` precedes `applyAuth` (`models.ts:974-977`), so a
        // provider that cannot classify answers this without a credential read or OAuth refresh.
        if !provider.supports_classification() {
            return ClassifierResult::errored(
                model,
                format!(
                    "Provider {} does not support classification",
                    model.provider
                ),
                options.is_aborted(),
            );
        }
        let (request_model, request_options) = match self
            .apply_classifier_auth(provider.as_ref(), model, options)
            .await
        {
            Ok(request) => request,
            Err(message) => {
                return ClassifierResult::errored(model, message, options.is_aborted());
            }
        };
        provider
            .classify(&request_model, context, &request_options)
            .await
    }

    /// Pi `applyAuth` (`models.ts:837-869`) for a classifier request; the `Err` string is the
    /// error result's message.
    async fn apply_classifier_auth(
        &self,
        provider: &dyn Provider,
        model: &ClassifierModel,
        options: &ClassifierOptions,
    ) -> Result<(ClassifierModel, ClassifierOptions), String> {
        let resolution = match provider.provider_auth() {
            Some(auth_strategy) => {
                let resolved = resolve_provider_auth(
                    &model.provider,
                    auth_strategy,
                    &model.to_auth_model(),
                    self.credentials.as_ref(),
                    self.auth_context.as_ref(),
                    AuthOverrides {
                        api_key: options.api_key.as_deref(),
                        env: options.env.as_ref(),
                        min_oauth_validity_ms: None,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
                // `if (!resolution) throw new ModelsError("auth", "Provider is not configured: ...")`
                // (`:853-855`).
                Some(
                    resolved
                        .ok_or_else(|| format!("Provider is not configured: {}", model.provider))?,
                )
            }
            None => None,
        };
        let (auth, resolution_env) = match &resolution {
            Some(r) => (Some(&r.auth), r.env.as_ref()),
            None => (None, None),
        };

        let mut request_model = model.clone();
        if let Some(base_url) = auth.and_then(|a| a.base_url.as_ref()) {
            request_model.base_url = base_url.clone();
        }
        let mut request_options = options.clone();
        request_options.api_key = options
            .api_key
            .clone()
            .or_else(|| auth.and_then(|a| a.api_key.clone()));
        // `getAuth(model, ...)` folds the model's own headers over the resolved ones
        // (`auth.headers = mergeHeaders(result.auth.headers, model.headers)`, `models.ts:746-752`)
        // BEFORE `applyAuth` merges the request's and runs `transformHeaders`, so the transform
        // sees (and can rewrite) the model's headers. The no-strategy arm gets the same base.
        let auth_headers = merge_headers(
            auth.and_then(|a| a.headers.as_ref()),
            model.headers.as_ref(),
        );
        let mut headers = merge_headers(auth_headers.as_ref(), options.headers.as_ref());
        if let Some(transform) = &options.transform_headers {
            headers = Some(transform(headers.unwrap_or_default()).await);
        }
        request_options.headers = headers;
        request_options.transform_headers = None;
        request_options.env = merge_env(resolution_env, options.env.as_ref());
        Ok((request_model, request_options))
    }

    /// Ask dynamic providers to re-fetch their model lists — **the full port** of Pi
    /// `Models.refresh` (declared `models.ts:147`, implemented `:276-328` @v0.83.0). PROV-S05.
    ///
    /// Every clause of upstream's body is reproduced here, in upstream's order:
    ///
    /// | pi | here |
    /// |---|---|
    /// | `allowNetwork = options.allowNetwork ?? true` (`:277`) | [`ModelsRefreshOptions::default`] |
    /// | `refreshable = providers.filter(p => p.refreshModels !== undefined)` (`:279-282`) | a provider whose [`Provider::refresh_models`] answers `None` |
    /// | `if (options.signal?.aborted) return;` (`:286`) | the per-provider pre-check below |
    /// | `provider.refreshModels({…, allowNetwork, force, signal})` (`:297-303`) | [`crate::provider::RefreshModelsContext`] |
    /// | `if (!signal?.aborted) errors.set(id, error)` (`:305-311`) | an abort records NO error |
    /// | the `allowNetwork:false` re-invocation, its own failure swallowed (`:313-322`) | the restore arm |
    /// | `return { aborted: signal?.aborted ?? false, errors }` (`:327`) | [`ModelsRefreshResult`] |
    ///
    /// Two deliberate deltas, both narrower than upstream rather than wider:
    ///
    /// * **`provider: Option<&str>` has no upstream counterpart.** pi's `refresh` always fans out;
    ///   the single-provider form exists here because [`Models::refresh`] has always had it and
    ///   `crates/cyrup/src/provider.rs` restricts the fetch set by id. `Some(id)` refreshes exactly
    ///   that provider and is a clean no-op for an unknown id (pi's
    ///   `if (!entry?.refreshModels) return`).
    /// * **No `credential` / `stored` / `publisher` are threaded.** See
    ///   [`crate::provider::RefreshModelsContext`]'s `[CYRUP-DELTA]`: the persisting fetcher owns
    ///   all three, so pi's `resolveRefreshCredential` bail (`:296`) is reproduced at the trigger
    ///   site rather than here.
    ///
    /// **The abort is real, not advisory.** A provider that has not started when `cancel` fires is
    /// never called; a provider that is mid-flight is cut off only if it honours the token it was
    /// handed, which is why [`crate::provider::RefreshModelsContext::cancel`] documents that as a requirement rather
    /// than a courtesy. Like pi, this waits for every started provider to settle before returning —
    /// it does not drop in-flight futures at this layer.
    pub async fn refresh_with(
        &self,
        provider: Option<&str>,
        options: ModelsRefreshOptions,
    ) -> ModelsRefreshResult {
        // `refreshable` (`:279-282`) — pi filters on the optional member being present; here a
        // static provider is the one that answers `None`, which it can only do by being called, so
        // the filter is expressed inside the per-provider body instead.
        let targets: Vec<&Arc<dyn Provider>> = match provider {
            Some(id) => self.providers.get(id).into_iter().collect(),
            None => self.providers.values().collect(),
        };

        let ctx = crate::provider::RefreshModelsContext {
            allow_network: options.allow_network,
            force: options.force,
            cancel: options.cancel.clone(),
            ..crate::provider::RefreshModelsContext::default()
        };

        let refreshes = targets.into_iter().map(|entry| {
            let ctx = &ctx;
            async move {
                // `if (options.signal?.aborted) return;` (`:286`). Checked per provider, INSIDE the
                // concurrent body, so a cancellation that lands while an earlier provider is in
                // flight still stops the ones that have not begun.
                if ctx.is_aborted() {
                    return None;
                }
                let failure = match entry.refresh_models(ctx).await {
                    // Static provider (`refreshModels === undefined`) or a clean refresh.
                    None | Some(Ok(())) => return None,
                    Some(Err(e)) => e,
                };

                // `catch (error) { if (!options.signal?.aborted) errors.set(…) }` (`:304-312`) — an
                // abort is a cancellation, not a provider failure, so it records nothing.
                let recorded =
                    (!ctx.is_aborted()).then(|| (entry.id().as_str().to_string(), failure));

                // `try { await provider.refreshModels({credential: stored, store, allowNetwork:
                // false, signal}) } catch { /* best-effort */ }` (`:313-322`) — restore the
                // persisted catalog after ANY failure. Note upstream does NOT carry `force` onto
                // this call, and its own failure is deliberately swallowed so the original
                // auth/network error is what the caller sees.
                let restore = crate::provider::RefreshModelsContext {
                    allow_network: false,
                    force: false,
                    cancel: ctx.cancel.clone(),
                    ..crate::provider::RefreshModelsContext::default()
                };
                let _ = entry.refresh_models(&restore).await;
                recorded
            }
        });

        let collected = futures::future::join_all(refreshes).await;
        ModelsRefreshResult {
            // `aborted: options.signal?.aborted ?? false` (`:327`) — read after the join, exactly
            // as upstream reads it after `Promise.all`.
            aborted: options.cancel.is_cancelled(),
            errors: collected.into_iter().flatten().collect(),
        }
    }

    /// [`Models::refresh_with`] with pi's default options, keeping the pre-PROV-S05 return shape for
    /// existing callers.
    ///
    /// With a provider id: a static provider (no [`Provider::refresh_models`] source → `None`) is a
    /// no-op (`Ok(())`); a dynamic provider's fetch failure is surfaced as a `model_source`
    /// [`ProviderError`] (Pi wraps the cause in `ModelsError("model_source", …)`; an error that is
    /// already a `model_source` error is re-raised unchanged, mirroring Pi's
    /// `if (error instanceof ModelsError) throw error`).
    ///
    /// Without a provider id: every provider is refreshed concurrently, best-effort — failures are
    /// swallowed (Pi `Promise.all` + the collected error map, which this form discards). **A caller
    /// that needs to know WHICH provider failed, to cancel, or to force past the freshness window
    /// must use [`Models::refresh_with`]** — `refresh(None)` reporting `Ok(())` for a wholly failed
    /// refresh is the exact hole PROV-S05 was filed for, and it survives here only as the
    /// compatibility shape.
    pub async fn refresh(&self, provider: Option<&str>) -> Result<(), ProviderError> {
        let result = self
            .refresh_with(provider, ModelsRefreshOptions::default())
            .await;
        let Some(id) = provider else {
            return Ok(());
        };
        match result.errors.into_iter().next() {
            None => Ok(()),
            Some((_, e @ ProviderError::ModelSource(_))) => Err(e),
            Some((_, e)) => Err(ProviderError::ModelSource(
                format!("Model refresh failed for {id}: {e}").into(),
            )),
        }
    }

    // ---- auth status / availability (Pi models.ts:150-153, :364-409 @v0.83.0) ----

    /// Whether a provider has complete auth configuration, **without** refreshing OAuth
    /// (1:1 port of Pi `Models.checkAuth` `models.ts:150`/`:388-392` and the private
    /// `checkProviderAuth` `:364-386` @v0.83.0). PROV-031.
    ///
    /// - A stored OAuth credential counts only when the provider declares an `oauth` strategy
    ///   (`:368-370`).
    /// - Otherwise the api-key strategy resolves (`:384-385`); `None` means unconfigured.
    ///
    /// **CYRUP-DELTA** (pi `models.ts:384`): pi's `resolveProviderAuth` is provider-scoped, while
    /// cyrup's [`crate::auth::ApiKeyAuth::resolve`] takes a `&Model` — `providers/cloudflare.rs`
    /// needs `model.base_url` for its `{CLOUDFLARE_ACCOUNT_ID}` substitution. The provider's first
    /// catalog row stands in as the resolution subject; a provider with an empty catalog has
    /// nothing to make available and reports `None`.
    ///
    /// A strategy's optional [`crate::auth::ApiKeyAuth::check`] hook is consulted at pi's exact
    /// position (`models.ts:507-517` @v0.87.1) — after the stored-OAuth branch, after the "no
    /// api-key strategy" guard, and INSTEAD OF the resolution path. No built-in provider implements
    /// `check` on either side, so every shipped provider still resolves; the seam exists for a
    /// strategy whose `resolve` executes commands.
    ///
    /// Failures PROPAGATE rather than reading as "unconfigured", as upstream's
    /// `ModelsError("auth", ...)` rethrows do: the credential-store read (`:490-494`), the `check`
    /// hook (`:514-516`) and the resolution (`:518`) each surface their error to the caller.
    pub async fn check_auth(&self, provider: &str) -> Result<Option<AuthCheck>, ProviderError> {
        // `const provider = this.providers.get(providerId); if (!provider) return undefined` (:527-528).
        let Some(entry) = self.providers.get(provider) else {
            return Ok(None);
        };
        let Some(auth) = entry.provider_auth() else {
            return Ok(None);
        };
        let id = entry.id().clone();
        let stored = self.credentials.read(&id).await?;

        // `if (credential?.type === "oauth") return provider.auth.oauth ? {source:"OAuth",
        // type:"oauth"} : undefined` (models.ts:502-503).
        if matches!(stored, Some(crate::auth::Credential::Oauth { .. })) {
            return Ok(auth.oauth.as_ref().map(|_| AuthCheck {
                auth_type: AuthType::Oauth,
                source: Some("OAuth".to_string()),
            }));
        }
        // `const apiKey = provider.auth.apiKey; if (!apiKey) return undefined` (:504-505).
        let Some(api_key) = auth.api_key.as_ref() else {
            return Ok(None);
        };
        // `if (apiKey.check) { try { return await apiKey.check({ctx, credential: credential?.type
        // === "api_key" ? credential : undefined, signal}); } catch (error) { throw new
        // ModelsError("auth", `API key auth check failed for provider ${provider.id}`, {cause:
        // error}); } }` (:506-517).
        if api_key.supports_check() {
            let cred = stored
                .as_ref()
                .filter(|cred| matches!(cred, crate::auth::Credential::ApiKey { .. }));
            return api_key
                .check(self.auth_context.as_ref(), cred)
                .await
                .map_err(|cause| crate::error::AuthError::api_key(id.clone(), cause).into());
        }
        let Some(model) = entry.models().first() else {
            return Ok(None);
        };
        let resolved = resolve_provider_auth(
            &id,
            auth,
            model,
            self.credentials.as_ref(),
            self.auth_context.as_ref(),
            AuthOverrides::default(),
        )
        .await?;
        Ok(resolved.map(|resolved| AuthCheck {
            auth_type: AuthType::ApiKey,
            source: resolved.source,
        }))
    }

    /// Models whose providers have complete auth configuration (1:1 port of Pi
    /// `Models.getAvailable` `models.ts:153`/`:394-409` @v0.83.0). PROV-031.
    ///
    /// Per provider: read the stored credential, run [`Models::check_auth`], skip the provider
    /// entirely when it is unconfigured, and otherwise pass its complete catalog through
    /// [`Provider::filter_models`] — pi's exact position, `models.ts:407` (PROV-032).
    /// [`Models::get_models`] still returns everything, as pi's `getModels()` does.
    ///
    /// An auth-check failure PROPAGATES, as upstream's does — `getAvailable` awaits
    /// `checkProviderAuth` without a catch (`models.ts:544`), so a rejecting
    /// [`crate::auth::ApiKeyAuth::check`] or credential-store read surfaces here instead of quietly
    /// removing a provider's whole catalog.
    pub async fn get_available(&self, provider: Option<&str>) -> Result<Vec<Model>, ProviderError> {
        let entries: Vec<&Arc<dyn Provider>> = match provider {
            Some(id) => self.providers.get(id).into_iter().collect(),
            None => self.providers.values().collect(),
        };
        let mut out = Vec::new();
        for entry in entries {
            if self.check_auth(entry.id().as_str()).await?.is_none() {
                continue;
            }
            let credential = self.credentials.read(entry.id()).await?;
            out.extend(entry.filter_models(entry.models(), credential.as_ref()));
        }
        Ok(out)
    }

    /// Models of every type whose providers have complete auth configuration (Pi
    /// `Models.getAllAvailable`, `models.ts:716-732` @v1.0.1). PROV-105.
    ///
    /// Per provider: the same auth gate as [`Models::get_available`] (an unconfigured provider is
    /// skipped; an auth-check failure propagates), then [`Provider::get_all_models`] through the
    /// provider's own [`Provider::filter_all_models`] when it has one. Without it, chat models go
    /// through [`Provider::filter_models`] and every other model is kept — pi's
    /// `!isModelType(model, "chat") || availableChatIds.has(model.id)`.
    pub async fn get_all_available(
        &self,
        provider: Option<&str>,
    ) -> Result<Vec<AnyModel>, ProviderError> {
        let entries: Vec<&Arc<dyn Provider>> = match provider {
            Some(id) => self.providers.get(id).into_iter().collect(),
            None => self.providers.values().collect(),
        };
        let mut out = Vec::new();
        for entry in entries {
            if self.check_auth(entry.id().as_str()).await?.is_none() {
                continue;
            }
            let credential = self.credentials.read(entry.id()).await?;
            let all = entry.get_all_models();
            if let Some(filtered) = entry.filter_all_models(&all, credential.as_ref()) {
                out.extend(filtered);
                continue;
            }
            let available_chat: std::collections::HashSet<String> = entry
                .filter_models(entry.models(), credential.as_ref())
                .into_iter()
                .map(|model| model.id.as_str().to_string())
                .collect();
            out.extend(all.into_iter().filter(|model| match model.as_chat() {
                Some(chat) => available_chat.contains(chat.id.as_str()),
                None => true,
            }));
        }
        Ok(out)
    }

    /// The available models of one type (Pi `Models.getAvailableOfType`, `models.ts:708-714`
    /// @v1.0.1). PROV-105.
    pub async fn get_available_of_type(
        &self,
        model_type: ModelType,
        provider: Option<&str>,
    ) -> Result<Vec<AnyModel>, ProviderError> {
        Ok(self
            .get_all_available(provider)
            .await?
            .into_iter()
            .filter(|model| model.model_type() == model_type)
            .collect())
    }

    /// Run a provider-owned login flow and persist the credential it returns (Pi `Models.login`,
    /// `models.ts:168` @v0.83.0). PROV-031.
    pub async fn login(
        &self,
        provider: &str,
        auth_type: AuthType,
        interaction: &dyn crate::auth::oauth::AuthInteraction,
    ) -> Result<crate::auth::Credential, ProviderError> {
        self.login_with_options(
            provider,
            auth_type,
            interaction,
            &crate::auth::LoginOptions::default(),
        )
        .await
    }

    /// The same login with pi's optional `LoginOptions` supplied
    /// (`Models.login(providerId, type, interaction, options?)`, `models.ts:756-770`).
    ///
    /// PROV-118 — this is where an app hands a flow its stable installation id. The two entry
    /// points are upstream's one call with and without the trailing argument: [`Models::login`]
    /// delegates here with [`crate::auth::LoginOptions::default`], which is the `options ===
    /// undefined` case, so there is one implementation and no behavioural fork.
    pub async fn login_with_options(
        &self,
        provider: &str,
        auth_type: AuthType,
        interaction: &dyn crate::auth::oauth::AuthInteraction,
        options: &crate::auth::LoginOptions,
    ) -> Result<crate::auth::Credential, ProviderError> {
        let id: cyrup_core::ProviderId = provider.into();
        let entry = self
            .providers
            .get(provider)
            .ok_or_else(|| auth_err(&id, "unknown provider"))?;
        let auth = entry
            .provider_auth()
            .ok_or_else(|| auth_err(&id, "provider has no auth strategy"))?;
        let cred = match auth_type {
            AuthType::Oauth => {
                let flow = auth
                    .oauth
                    .as_ref()
                    .ok_or_else(|| auth_err(&id, "provider has no OAuth flow"))?;
                flow.login(interaction, options)
                    .await
                    .map_err(|e| auth_err(&id, &format!("login failed: {e}")))?
            }
            AuthType::ApiKey => {
                let strategy = auth
                    .api_key
                    .as_ref()
                    .ok_or_else(|| auth_err(&id, "provider has no api-key strategy"))?;
                strategy
                    .login(interaction)
                    .await
                    .map_err(|e| auth_err(&id, &format!("login failed: {e}")))?
            }
        };
        let id = entry.id().clone();
        let stored = cred.clone();
        self.credentials
            .modify(
                &id,
                Box::new(move |_| Box::pin(async move { Ok(Some(stored)) })),
            )
            .await?;
        Ok(cred)
    }

    /// Remove the stored credential for a provider (Pi `Models.logout`, `models.ts:171` @v0.83.0).
    /// PROV-031.
    pub async fn logout(&self, provider: &str) -> Result<(), ProviderError> {
        let id = match self.providers.get(provider) {
            Some(entry) => entry.id().clone(),
            None => provider.into(),
        };
        self.credentials.delete(&id).await?;
        Ok(())
    }
}

/// A `ProviderError::Auth` naming the provider — the shape [`crate::error::AuthError`] requires.
fn auth_err(provider: &cyrup_core::ProviderId, detail: &str) -> ProviderError {
    ProviderError::Auth(crate::error::AuthError::ApiKey {
        provider: provider.clone(),
        cause: detail.to_string().into(),
    })
}

/// Whether a provider's configured auth is an api key or an OAuth credential
/// (Pi `AuthType = "api_key" | "oauth"`, `auth/types.ts:111` @v0.83.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthType {
    ApiKey,
    Oauth,
}

/// The result of [`Models::check_auth`] (Pi `AuthCheck`, `auth/types.ts:106-109` @v0.83.0:
/// `{ source?: string; type: "api_key" | "oauth" }`).
///
/// `cyrup-config` declares its own `AuthCheck` with the same two members
/// (`cyrup-config/src/login.rs:116-119`); collapsing the two onto this one is cross-crate work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthCheck {
    pub auth_type: AuthType,
    pub source: Option<String>,
}

/// A snapshot of the data `apply_auth` needs, captured for the spawned stream task.
struct AuthHelper {
    provider: Arc<dyn Provider>,
    credentials: Arc<dyn CredentialStore>,
    auth_context: Arc<dyn AuthContext>,
}

impl AuthHelper {
    async fn apply_auth(
        &self,
        model: &Model,
        options: &StreamOptions,
    ) -> Result<(Model, StreamOptions), ProviderError> {
        let Some(auth_strategy) = self.provider.provider_auth() else {
            return Ok((model.clone(), options.clone()));
        };
        let resolution = resolve_provider_auth(
            &model.provider,
            auth_strategy,
            model,
            self.credentials.as_ref(),
            self.auth_context.as_ref(),
            AuthOverrides {
                api_key: options.api_key.as_deref(),
                // Pi `getAuth(model, { apiKey, env: options?.env, ... })` (`models.ts:850-851`):
                // the request's env participates in resolution, as in `apply_classifier_auth`.
                env: options.env.as_ref(),
                min_oauth_validity_ms: None,
            },
        )
        .await?;
        let Some(resolution) = resolution else {
            return Ok((model.clone(), options.clone()));
        };
        let auth = &resolution.auth;
        let mut request_model = model.clone();
        if let Some(base_url) = &auth.base_url {
            request_model.base_url = base_url.clone();
        }
        let mut request_options = options.clone();
        request_options.api_key = options.api_key.clone().or_else(|| auth.api_key.clone());
        let mut headers = merge_headers(auth.headers.as_ref(), options.headers.as_ref());
        // PROV-042. `if (options?.transformHeaders) headers = await options.transformHeaders(headers
        // ?? {})` (Pi `models.ts:480` @v0.83.0) — LAST, after auth headers and request headers are
        // merged, so the transform observes the final set and its return value wins. Then
        // `const { transformHeaders: _t, ...providerOptions } = options ?? {}` (`:483`) strips it,
        // so no provider or wire impl ever sees it as an option field.
        if let Some(transform) = &options.transform_headers {
            headers = Some(transform(headers.unwrap_or_default()).await);
        }
        request_options.headers = headers;
        request_options.transform_headers = None;
        // PROV-109. `env = { ...(resolution.env ?? {}), ...(options?.env ?? {}) }` (Pi
        // `models.ts:862`): the resolution's env first, the request's env wins per key.
        request_options.env = merge_env(resolution.env.as_ref(), options.env.as_ref());
        Ok((request_model, request_options))
    }
}

/// Merge auth headers with request headers; request headers win per key (Pi `applyAuth` header
/// spread `{ ...auth.headers, ...options.headers }`). `None` when neither side has any.
fn merge_headers(
    auth_headers: Option<&crate::HeaderMap>,
    option_headers: Option<&crate::HeaderMap>,
) -> Option<crate::HeaderMap> {
    if auth_headers.is_none() && option_headers.is_none() {
        return None;
    }
    let mut merged = crate::HeaderMap::new();
    if let Some(h) = auth_headers {
        for (k, v) in h {
            merged.insert(k.clone(), v.clone());
        }
    }
    if let Some(h) = option_headers {
        for (k, v) in h {
            merged.insert(k.clone(), v.clone());
        }
    }
    Some(merged)
}

/// Merge a resolution's env with the request's; the request wins per key (Pi
/// `{ ...(resolution.env ?? {}), ...(options?.env ?? {}) }`, `models.ts:862`). `None` when
/// neither side has any.
fn merge_env(
    resolution_env: Option<&ProviderEnv>,
    option_env: Option<&ProviderEnv>,
) -> Option<ProviderEnv> {
    if resolution_env.is_none() && option_env.is_none() {
        return None;
    }
    let mut merged = ProviderEnv::new();
    for env in [resolution_env, option_env].into_iter().flatten() {
        merged.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    Some(merged)
}

// ---- model capability helpers (Pi `hasApi` `models.ts:635-637`, `getSupportedThinkingLevels`
// `:663-672`, `clampThinkingLevel` `:674-693`, `calculateCost` `:639-659` @v0.83.0; PROV-041
// corrected `:397`, a line inside `getAvailable`) ----

/// The full ordered extended-thinking ladder (Pi `EXTENDED_THINKING_LEVELS`).
pub const EXTENDED_THINKING_LEVELS: [ModelThinkingLevel; 7] = [
    ModelThinkingLevel::Off,
    ModelThinkingLevel::Minimal,
    ModelThinkingLevel::Low,
    ModelThinkingLevel::Medium,
    ModelThinkingLevel::High,
    ModelThinkingLevel::Xhigh,
    ModelThinkingLevel::Max,
];

fn level_key(level: ModelThinkingLevel) -> &'static str {
    crate::api::compat::thinking_level_key(level)
}

/// The thinking levels a model supports (Pi `getSupportedThinkingLevels`). A non-reasoning model
/// supports only `off`. A `thinkingLevelMap` value of `null` marks a level unsupported; `xhigh` and
/// `max` additionally require an explicit (non-`undefined`) map entry (Pi `models.ts:669`
/// @v0.83.0; PROV-041 corrected `:670`, the `return true` fall-through beneath it).
///
/// PROV-068 asked whether an explicit `null` instead means "supported, but sent with no
/// provider-specific value". It does NOT — that meaning belongs to ABSENCE. `thinkingLevelMap` is
/// three-way, and Pi keeps all three cases distinct at BOTH ends, filter and wire:
///   - absent    → supported; wire sends the generic level name, because the lookup is defaulted
///     (`model.thinkingLevelMap?.[level] ?? options.reasoningEffort`,
///     `api/openai-completions.ts:875` @v0.84.2). THIS is "no mapped value".
///   - `Some(v)` → supported; wire sends `v` in place of the level name (`:882`).
///   - `null`    → the rung does not exist on this model; the wire emits NOTHING for it, gated
///     BEFORE any default: `else if (model.thinkingLevelMap?.off !== null)`
///     (`api/openai-completions.ts:870`, `:884`, `:905`; `openai-responses.ts:333`;
///     `anthropic-messages.ts:1087`) — the off-switch is suppressed, not defaulted.
///
/// `if (mapped === null) return false` (`models.ts:668` @v0.83.0; unchanged at `:907` @v0.84.2) is
/// simply the filter half of that same three-way, so `Some(None) => false` below is correct as
/// written. Upstream pins it by test: `{off,minimal,low,medium: null, xhigh: "max"}` yields exactly
/// `["high", "xhigh"]` (`coding-agent/test/model-registry.test.ts:1064-1071`), and
/// `{xhigh: null, max: "max"}` yields `[off,minimal,low,medium,high,max]`
/// (`ai/test/max-thinking.test.ts:59-66`). Reading `null` as "supported" would offer `off` on every
/// `{"off": null}` row — gpt-5, gemini-3-flash, claude-fable-5 — models that cannot stop reasoning,
/// and the wire would still send no off-parameter, so the rung would silently do nothing.
pub fn get_supported_thinking_levels(model: &Model) -> Vec<ModelThinkingLevel> {
    if !model.reasoning {
        return vec![ModelThinkingLevel::Off];
    }
    EXTENDED_THINKING_LEVELS
        .iter()
        .copied()
        .filter(|level| {
            let mapped = model
                .thinking_level_map
                .as_ref()
                .and_then(|m| m.get(level_key(*level)));
            match mapped {
                Some(None) => false, // explicit null → unsupported
                Some(Some(_)) => true,
                // undefined: `xhigh`/`max` require an entry, every other rung is implicit.
                None => !matches!(*level, ModelThinkingLevel::Xhigh | ModelThinkingLevel::Max),
            }
        })
        .collect()
}

/// Clamp a requested thinking level to one the model supports (Pi `clampThinkingLevel`): prefer the
/// requested level, else the nearest higher supported level, else the nearest lower, else the first
/// supported (or `off`).
pub fn clamp_thinking_level(model: &Model, level: ModelThinkingLevel) -> ModelThinkingLevel {
    let available = get_supported_thinking_levels(model);
    if available.contains(&level) {
        return level;
    }
    let Some(requested_index) = EXTENDED_THINKING_LEVELS.iter().position(|l| *l == level) else {
        return available
            .first()
            .copied()
            .unwrap_or(ModelThinkingLevel::Off);
    };
    for candidate in EXTENDED_THINKING_LEVELS.iter().skip(requested_index) {
        if available.contains(candidate) {
            return *candidate;
        }
    }
    for candidate in EXTENDED_THINKING_LEVELS.iter().take(requested_index).rev() {
        if available.contains(candidate) {
            return *candidate;
        }
    }
    available
        .first()
        .copied()
        .unwrap_or(ModelThinkingLevel::Off)
}

/// Two models are equal iff their id AND provider match (Pi `modelsAreEqual`).
pub fn models_are_equal(a: Option<&Model>, b: Option<&Model>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.id == b.id && a.provider == b.provider,
        _ => false,
    }
}

/// Runtime-checked narrowing for a dynamically looked-up model's wire api (Pi `hasApi`).
pub fn has_api(model: &Model, api: &str) -> bool {
    model.api.as_str() == api
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
    use crate::auth::types::Credential;
    use crate::known_api::OPENAI_COMPLETIONS;
    use crate::model::{Modality, ModelCost, ThinkingLevelMap};
    use crate::providers::fleet;
    use cyrup_core::{ProviderId, StopReason};
    use std::collections::BTreeMap;

    struct MapCtx(BTreeMap<String, String>);
    #[async_trait::async_trait]
    impl AuthContext for MapCtx {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    fn model(provider: &str, id: &str, reasoning: bool, map: Option<ThinkingLevelMap>) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            api: OPENAI_COMPLETIONS.into(),
            provider: provider.into(),
            base_url: "https://example.test/v1".into(),
            reasoning,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            input_limits: None,
            prompt_cache: None,
            context_window: 1000,
            max_tokens: 1000,
            sampling_params: None,
            thinking_level_map: map,
            compat: None,
            headers: None,
        }
    }

    #[test]
    fn set_get_delete_clear_providers() {
        let mut models = create_models(CreateModelsOptions::default());
        assert!(models.get_providers().is_empty());

        models.set_provider(Arc::new(fleet::GROQ.provider()));
        models.set_provider(Arc::new(fleet::XAI.provider()));
        assert_eq!(models.get_providers().len(), 2);
        assert!(models.get_provider("groq").is_some());

        // Upsert by id is idempotent (replace, not append).
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        assert_eq!(models.get_providers().len(), 2);

        // Cross-provider getModels aggregates; provider-scoped narrows.
        let all = models.get_models(None);
        let groq_only = models.get_models(Some("groq"));
        assert!(all.len() > groq_only.len());
        assert!(groq_only.iter().all(|m| m.provider.as_str() == "groq"));
        assert!(
            models
                .get_model("xai", &groq_only[0].id.clone().to_string())
                .is_none()
        );

        models.delete_provider("groq");
        assert!(models.get_provider("groq").is_none());
        assert_eq!(models.get_providers().len(), 1);

        models.clear_providers();
        assert!(models.get_providers().is_empty());
    }

    #[tokio::test]
    async fn get_auth_resolves_env_key_via_collection_store() {
        let mut models = create_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(MapCtx(BTreeMap::from([(
                "GROQ_API_KEY".to_string(),
                "sk-groq".to_string(),
            )])))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        let m = models
            .get_models(Some("groq"))
            .into_iter()
            .next()
            .expect("a groq model");
        let auth = models.get_auth(&m).await.expect("ok").expect("configured");
        assert_eq!(auth.auth.api_key.as_deref(), Some("sk-groq"));
        assert_eq!(auth.source.as_deref(), Some("env"));

        // Unknown provider → Ok(None).
        let unknown = model("nope", "x", false, None);
        assert!(models.get_auth(&unknown).await.expect("ok").is_none());
    }

    /// PROV-128 — pi folds auth resolution for every model type into ONE `getAuth(model: AnyModel)`
    /// (`models.ts:734-753` @v1.0.1). The deleted parallel images tree had its own
    /// `ImagesModels::get_auth(&ImagesModel)`; without [`Models::get_any_auth`] retiring that tree
    /// would have left no way to resolve an image model's credential from the collection at all.
    ///
    /// `openrouter` is the one built-in that lists chat AND image rows, so one provider covers two
    /// of the three arms; the classifier arm is built by hand because cyrup ships no built-in
    /// classifier catalog.
    #[tokio::test]
    async fn the_collection_resolves_auth_for_a_model_of_every_type() {
        let mut models = create_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(MapCtx(BTreeMap::from([(
                "OPENROUTER_API_KEY".to_string(),
                "sk-or".to_string(),
            )])))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(crate::providers::fleet::OPENROUTER.provider()));

        let key = |auth: Option<crate::auth::AuthResult>| {
            auth.expect("configured").auth.api_key.expect("an api key")
        };

        let chat = models
            .get_all_models(Some("openrouter"))
            .into_iter()
            .find(|m| m.model_type() == ModelType::Chat)
            .expect("openrouter lists chat rows");
        assert_eq!(key(models.get_any_auth(&chat).await.expect("ok")), "sk-or");

        let image = models
            .get_all_models(Some("openrouter"))
            .into_iter()
            .find(|m| m.model_type() == ModelType::Image)
            .expect("openrouter lists image rows");
        assert_eq!(key(models.get_any_auth(&image).await.expect("ok")), "sk-or");

        let classifier = AnyModel::Classifier(ClassifierModel {
            id: "~typesafe/jev-latest".into(),
            name: "TypeSafe: Jev Latest".into(),
            api: "typesafe-system-one".into(),
            provider: "openrouter".into(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            input: vec![crate::model::Modality::Text],
            input_limits: None,
            cost: crate::model::ModelCost::default(),
            headers: None,
            context_window: 32_000,
        });
        assert_eq!(
            key(models.get_any_auth(&classifier).await.expect("ok")),
            "sk-or"
        );

        // An unknown provider is `Ok(None)` for every type, exactly as `get_auth` is.
        let mut orphan = image.clone().into_image().expect("an image model");
        orphan.provider = "nope".into();
        assert!(
            models
                .get_any_auth(&AnyModel::Image(orphan))
                .await
                .expect("ok")
                .is_none()
        );
    }

    #[tokio::test]
    async fn get_auth_prefers_stored_credential() {
        let store = crate::auth::InMemoryCredentialStore::new()
            .with_credential(ProviderId::from("groq"), Credential::api_key("stored-key"));
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(Arc::new(store)),
            auth_context: Some(Arc::new(MapCtx(BTreeMap::from([(
                "GROQ_API_KEY".to_string(),
                "env-key".to_string(),
            )])))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        let m = models
            .get_models(Some("groq"))
            .into_iter()
            .next()
            .expect("groq model");
        let auth = models.get_auth(&m).await.expect("ok").expect("configured");
        // Stored credential owns the provider (env not consulted, R-01-012).
        assert_eq!(auth.auth.api_key.as_deref(), Some("stored-key"));
        assert_eq!(auth.source.as_deref(), Some("stored"));
    }

    /// `get_auth_with` really forwards its overrides (Pi `getAuth(model, overrides)`,
    /// `models.ts:165`/`:413-429` @v0.83.0)
    /// — `get_auth` used to hard-code `AuthOverrides::default()`, so the whole override tier,
    /// `min_oauth_validity_ms` included, was unreachable through this seam.
    #[tokio::test]
    async fn get_auth_with_forwards_per_request_overrides() {
        let store = crate::auth::InMemoryCredentialStore::new()
            .with_credential(ProviderId::from("groq"), Credential::api_key("stored-key"));
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(Arc::new(store)),
            auth_context: Some(Arc::new(MapCtx(BTreeMap::new()))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        let m = models
            .get_models(Some("groq"))
            .into_iter()
            .next()
            .expect("groq model");

        let auth = models
            .get_auth_with(
                &m,
                AuthOverrides {
                    api_key: Some("explicit-key"),
                    env: None,
                    min_oauth_validity_ms: Some(30 * 60_000),
                },
            )
            .await
            .expect("ok")
            .expect("configured");
        assert_eq!(
            auth.auth.api_key.as_deref(),
            Some("explicit-key"),
            "the per-request override must reach resolve_provider_auth, not be dropped"
        );

        // The no-override entry point is unchanged: the stored credential still owns the provider.
        let auth = models.get_auth(&m).await.expect("ok").expect("configured");
        assert_eq!(auth.auth.api_key.as_deref(), Some("stored-key"));
    }

    #[tokio::test]
    async fn stream_unknown_provider_yields_error_terminal() {
        let models = create_models(CreateModelsOptions::default());
        let m = model("ghost", "m1", false, None);
        let msg =
            collect_message(models.stream(&m, &Context::default(), &StreamOptions::default()))
                .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("unknown provider"));
    }

    #[tokio::test]
    async fn stream_applies_auth_then_delegates() {
        // Configured env key → auth applies; an unroutable base URL proves we reached transport
        // (an unconfigured provider would short-circuit before the request).
        let mut models = create_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(MapCtx(BTreeMap::from([(
                "GROQ_API_KEY".to_string(),
                "sk-groq".to_string(),
            )])))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        let mut m = models
            .get_models(Some("groq"))
            .into_iter()
            .next()
            .expect("groq model");
        m.base_url = "http://127.0.0.1:1/v1".to_string();
        let msg =
            collect_message(models.stream(&m, &Context::default(), &StreamOptions::default()))
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
    async fn stream_simple_unknown_provider_yields_error_terminal() {
        let models = create_models(CreateModelsOptions::default());
        let m = model("ghost", "m1", false, None);
        let opts = SimpleStreamOptions::default();
        let msg = collect_message(models.stream_simple(&m, &Context::default(), &opts)).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("unknown provider"));
    }

    #[tokio::test]
    async fn stream_simple_applies_auth_then_delegates() {
        // Mirrors `stream_applies_auth_then_delegates` for the simple path: a configured env key
        // resolves, and the unroutable base URL proves we reached transport (Pi `streamSimple`
        // applies auth identically to `stream`).
        let mut models = create_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(MapCtx(BTreeMap::from([(
                "GROQ_API_KEY".to_string(),
                "sk-groq".to_string(),
            )])))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        let mut m = models
            .get_models(Some("groq"))
            .into_iter()
            .next()
            .expect("groq model");
        m.base_url = "http://127.0.0.1:1/v1".to_string();
        let opts = SimpleStreamOptions {
            reasoning: Some(cyrup_core::ThinkingLevel::Low),
            ..Default::default()
        };
        let msg = collect_message(models.stream_simple(&m, &Context::default(), &opts)).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(
            !err.contains("not configured"),
            "auth should have resolved: {err}"
        );
        assert!(err.contains("transport"), "expected transport error: {err}");
    }

    #[tokio::test]
    async fn complete_simple_folds_to_terminal_message() {
        let models = create_models(CreateModelsOptions::default());
        let m = model("ghost", "m1", false, None);
        let opts = SimpleStreamOptions::default();
        let msg = models.complete_simple(&m, &Context::default(), &opts).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("unknown provider"));
    }

    #[test]
    fn supported_thinking_levels_match_pi() {
        // Non-reasoning → only off.
        let plain = model("p", "a", false, None);
        assert_eq!(
            get_supported_thinking_levels(&plain),
            vec![ModelThinkingLevel::Off]
        );

        // Reasoning, no map → off,minimal,low,medium,high (xhigh AND max each require an explicit
        // entry — Pi `models.ts:669` @v0.83.0 `if (level === "xhigh" || level === "max") return
        // mapped !== undefined;`).
        let r = model("p", "b", true, None);
        assert_eq!(
            get_supported_thinking_levels(&r),
            vec![
                ModelThinkingLevel::Off,
                ModelThinkingLevel::Minimal,
                ModelThinkingLevel::Low,
                ModelThinkingLevel::Medium,
                ModelThinkingLevel::High,
            ]
        );

        // null entry removes a level; explicit xhigh/max entries add them.
        let mut map = ThinkingLevelMap::new();
        map.insert("low".to_string(), None); // unsupported
        map.insert("xhigh".to_string(), Some("xhigh".to_string())); // enabled
        map.insert("max".to_string(), Some("max".to_string())); // enabled
        let r2 = model("p", "c", true, Some(map));
        let levels = get_supported_thinking_levels(&r2);
        assert!(!levels.contains(&ModelThinkingLevel::Low));
        assert!(levels.contains(&ModelThinkingLevel::Xhigh));
        assert!(levels.contains(&ModelThinkingLevel::Max));
        // `max` is opt-in exactly like `xhigh`: an explicit null keeps it off the ladder even
        // though every lower rung is implicitly on.
        let mut null_max = ThinkingLevelMap::new();
        null_max.insert("max".to_string(), None);
        let r3 = model("p", "d", true, Some(null_max));
        assert!(!get_supported_thinking_levels(&r3).contains(&ModelThinkingLevel::Max));
    }

    #[test]
    fn clamp_thinking_level_walks_up_then_down() {
        // Map removes medium+high; request medium → clamps up past the gap.
        let mut map = ThinkingLevelMap::new();
        map.insert("medium".to_string(), None);
        map.insert("high".to_string(), None);
        map.insert("xhigh".to_string(), Some("xhigh".to_string()));
        let m = model("p", "c", true, Some(map));
        // medium unsupported → nearest higher supported is xhigh.
        assert_eq!(
            clamp_thinking_level(&m, ModelThinkingLevel::Medium),
            ModelThinkingLevel::Xhigh
        );
        // `max` is above every supported rung → the upward walk finds nothing and the downward
        // walk lands on xhigh (Pi `models.ts:688-691` @v0.83.0).
        assert_eq!(
            clamp_thinking_level(&m, ModelThinkingLevel::Max),
            ModelThinkingLevel::Xhigh
        );

        // The mirror case, and the one the corrected `claude-opus-4-6` catalog produces: `max` is
        // the ONLY top rung, so a request for `xhigh` must promote UP to it rather than fall to
        // `high`. This only works because `Max` is declared after `Xhigh` in the ladder.
        let mut only_max = ThinkingLevelMap::new();
        only_max.insert("max".to_string(), Some("max".to_string()));
        let m_max = model("p", "e", true, Some(only_max));
        assert_eq!(
            clamp_thinking_level(&m_max, ModelThinkingLevel::Xhigh),
            ModelThinkingLevel::Max
        );

        // Non-reasoning model clamps everything to off.
        let plain = model("p", "a", false, None);
        assert_eq!(
            clamp_thinking_level(&plain, ModelThinkingLevel::High),
            ModelThinkingLevel::Off
        );
    }

    #[test]
    fn models_equal_and_has_api() {
        let a = model("p", "m", false, None);
        let b = model("p", "m", true, None); // same id+provider, differing fields
        let c = model("q", "m", false, None);
        assert!(models_are_equal(Some(&a), Some(&b)));
        assert!(!models_are_equal(Some(&a), Some(&c)));
        assert!(!models_are_equal(Some(&a), None));
        assert!(has_api(&a, OPENAI_COMPLETIONS));
        assert!(!has_api(&a, "anthropic-messages"));
    }

    // ---- dynamic-refresh dispatch (Pi `Models.refresh`, declared `models.ts:147`, implemented
    // `:276-328` @v0.83.0; PROV-041 corrected `:198-214`, which is `CreateModelsOptions` +
    // `mergeHeaders`) ----

    use crate::utils::refresh::RefreshDedup;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A dynamic provider whose `refresh_models` counts fetches (deduplicated via [`RefreshDedup`])
    /// and optionally fails. `stream()` is unused here and yields a terminal error immediately.
    struct DynProvider {
        id: cyrup_core::ProviderId,
        models: Vec<Model>,
        fetches: Arc<AtomicUsize>,
        /// Calls that arrived with `allow_network: false` — pi's cache restore (`models.ts:314-319`).
        restores: Arc<AtomicUsize>,
        fail: bool,
        /// When set, the fetch parks on this until notified or aborted (PROV-S05's abort test).
        hold: Option<Arc<tokio::sync::Notify>>,
        dedup: RefreshDedup,
    }

    #[async_trait::async_trait]
    impl Provider for DynProvider {
        fn id(&self) -> &cyrup_core::ProviderId {
            &self.id
        }
        fn models(&self) -> &[Model] {
            &self.models
        }
        fn stream(
            &self,
            model: &Model,
            _context: &Context,
            _options: &StreamOptions,
        ) -> EventStream<StreamEvent> {
            let msg = cyrup_core::AssistantMessage::errored(
                model.provider.clone(),
                model.id.as_str(),
                Some(model.api.clone()),
                cyrup_core::StopReason::Error,
                "unused",
            );
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::terminal(msg)).await;
            });
            Box::pin(ReceiverStream::new(rx))
        }
        fn has_refresh_models(&self) -> bool {
            true
        }
        /// PROV-S05: shaped like pi's `createProvider` `refreshModels` (`models.ts:566-616`
        /// @v0.83.0) — it honours `allowNetwork` by returning without fetching, which is what makes
        /// the post-failure cache restore (`:313-322`) a restore rather than a second attempt, and
        /// it honours the abort token so a cancel actually cuts the fetch off.
        async fn refresh_models(
            &self,
            ctx: &crate::provider::RefreshModelsContext,
        ) -> Option<Result<(), ProviderError>> {
            // `if (!allowNetwork) { restore from store; return; }` — no fetch, no error.
            if !ctx.allow_network {
                self.restores.fetch_add(1, Ordering::SeqCst);
                return Some(Ok(()));
            }
            let fetches = self.fetches.clone();
            let fail = self.fail;
            let hold = self.hold.clone();
            let cancel = ctx.cancel.clone();
            Some(
                self.dedup
                    .run(move || async move {
                        fetches.fetch_add(1, Ordering::SeqCst);
                        if let Some(hold) = hold {
                            // Block until either the caller aborts or the test releases us. `biased`
                            // so the abort arm is deterministic when both are ready.
                            tokio::select! {
                                biased;
                                () = cancel.cancelled() => {
                                    return Err(ProviderError::Transport("aborted".into()));
                                }
                                _ = hold.notified() => {}
                            }
                        }
                        if fail {
                            Err(ProviderError::Transport("network down".into()))
                        } else {
                            Ok(())
                        }
                    })
                    .await,
            )
        }
    }

    fn dyn_provider(id: &str, fail: bool) -> (Arc<DynProvider>, Arc<AtomicUsize>) {
        let (p, fetches, _) = dyn_provider_full(id, fail, None);
        (p, fetches)
    }

    /// [`dyn_provider`] plus the restore counter and an optional park handle (PROV-S05).
    fn dyn_provider_full(
        id: &str,
        fail: bool,
        hold: Option<Arc<tokio::sync::Notify>>,
    ) -> (Arc<DynProvider>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let fetches = Arc::new(AtomicUsize::new(0));
        let restores = Arc::new(AtomicUsize::new(0));
        let p = Arc::new(DynProvider {
            id: cyrup_core::ProviderId::from(id),
            models: vec![model(id, "m1", false, None)],
            fetches: fetches.clone(),
            restores: restores.clone(),
            fail,
            hold,
            dedup: RefreshDedup::new(),
        });
        (p, fetches, restores)
    }

    #[tokio::test]
    async fn refresh_static_provider_is_noop() {
        // A fleet provider has no dynamic source (`refresh_models` → None): refresh is a clean Ok.
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::new(fleet::GROQ.provider()));
        models
            .refresh(Some("groq"))
            .await
            .expect("static refresh is a no-op");
        // Unknown provider id is also a no-op (Pi `if (!entry?.refreshModels) return`).
        models
            .refresh(Some("does-not-exist"))
            .await
            .expect("unknown is a no-op");
    }

    #[tokio::test]
    async fn refresh_dynamic_provider_calls_fetch() {
        let mut models = create_models(CreateModelsOptions::default());
        let (p, fetches) = dyn_provider("dyn-ok", false);
        models.set_provider(p);
        models.refresh(Some("dyn-ok")).await.expect("ok");
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_dynamic_failure_is_model_source() {
        let mut models = create_models(CreateModelsOptions::default());
        let (p, fetches) = dyn_provider("dyn-bad", true);
        models.set_provider(p);
        let err = models
            .refresh(Some("dyn-bad"))
            .await
            .expect_err("should fail");
        assert_eq!(err.code(), "model_source");
        assert!(err.to_string().contains("Model refresh failed for dyn-bad"));
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_all_is_best_effort_and_swallows_failures() {
        // A mix of a static provider, a failing dynamic provider, and a healthy dynamic provider:
        // refresh(None) must call every dynamic source and never error (Pi `allSettled`).
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::new(fleet::GROQ.provider())); // static
        let (bad, bad_fetches) = dyn_provider("dyn-bad", true);
        let (ok, ok_fetches) = dyn_provider("dyn-ok", false);
        models.set_provider(bad);
        models.set_provider(ok);
        models
            .refresh(None)
            .await
            .expect("best-effort never errors");
        assert_eq!(
            bad_fetches.load(Ordering::SeqCst),
            1,
            "failing source still fetched"
        );
        assert_eq!(
            ok_fetches.load(Ordering::SeqCst),
            1,
            "healthy source fetched"
        );
    }

    // ---- PROV-S05: `Models::refresh` options, result and abort (Pi `models.ts:276-328` @v0.83.0) ----

    /// PROV-S05 (a) — `refresh_with(None, …)` names the provider that failed.
    ///
    /// **Red before the fix:** `refresh` returned `Result<(), ProviderError>` and the all-provider
    /// path was `join_all(refreshes).await; Ok(())` with EVERY result discarded, so a wholly failed
    /// refresh was indistinguishable from a clean one and there was no `errors` map to assert on —
    /// this test did not compile, and its `refresh(None)` predecessor asserted `Ok(())` for exactly
    /// the state this asserts an error for.
    #[tokio::test]
    async fn prov_s05_refresh_all_returns_a_per_provider_error_map() {
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::new(fleet::GROQ.provider())); // static: contributes nothing
        let (bad, bad_fetches, bad_restores) = dyn_provider_full("dyn-bad", true, None);
        let (ok, ok_fetches, ok_restores) = dyn_provider_full("dyn-ok", false, None);
        models.set_provider(bad);
        models.set_provider(ok);

        let result = models
            .refresh_with(None, ModelsRefreshOptions::default())
            .await;

        assert!(!result.aborted, "nobody cancelled");
        assert_eq!(
            result.errors.keys().collect::<Vec<_>>(),
            vec!["dyn-bad"],
            "exactly the failing provider is named, and the static one is absent"
        );
        // pi stores the ORIGINAL error (`errors.set(provider.id, error)`, models.ts:306), not a
        // re-wrapped one — `error instanceof Error ? error : …` always takes the first arm here.
        //
        // That claim is about the error's IDENTITY, so it is asserted on the variant: a re-wrap
        // would arrive as `ProviderError::ModelSource` (which is what the legacy `refresh(Some(id))`
        // shape below deliberately produces). Asserting the rendered string instead would have been
        // asserting `ProviderError::Transport`'s own `Display` prefix, which is not what upstream
        // is saying anything about.
        let recorded = result
            .error_for("dyn-bad")
            .expect("the failing provider is named");
        assert!(
            matches!(recorded, ProviderError::Transport(_)),
            "the provider's own error variant must survive the fan-out unwrapped, got {recorded:?}"
        );
        assert_eq!(
            recorded.to_string(),
            "transport error: network down",
            "and it must still carry the provider's message verbatim"
        );
        assert!(result.error_for("dyn-ok").is_none());
        assert!(!result.is_clean());

        // The healthy provider's catalog is still updated — one failure does not abandon the fan-out.
        assert_eq!(ok_fetches.load(Ordering::SeqCst), 1);
        assert_eq!(bad_fetches.load(Ordering::SeqCst), 1);

        // pi `:313-322`: after ANY failure, re-invoke with `allowNetwork: false` so the persisted
        // catalog is restored. Only the failing provider gets it.
        assert_eq!(
            bad_restores.load(Ordering::SeqCst),
            1,
            "the failed provider must get the allowNetwork:false cache restore"
        );
        assert_eq!(
            ok_restores.load(Ordering::SeqCst),
            0,
            "a clean refresh must NOT be followed by a restore"
        );
    }

    /// PROV-S05 (b) — `force` and `allow_network` actually reach the provider.
    ///
    /// **Red before the fix:** `Provider::refresh_models` took no argument at all, so neither flag
    /// had anywhere to arrive; this test could not be written.
    #[tokio::test]
    async fn prov_s05_force_and_allow_network_reach_the_provider() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct Recorder {
            seen: Mutex<Vec<(bool, bool)>>,
        }
        struct RecordingProvider {
            id: cyrup_core::ProviderId,
            models: Vec<Model>,
            rec: Arc<Recorder>,
        }
        #[async_trait::async_trait]
        impl Provider for RecordingProvider {
            fn id(&self) -> &cyrup_core::ProviderId {
                &self.id
            }
            fn models(&self) -> &[Model] {
                &self.models
            }
            fn stream(
                &self,
                _model: &Model,
                _context: &Context,
                _options: &StreamOptions,
            ) -> EventStream<StreamEvent> {
                let (_tx, rx) = tokio::sync::mpsc::channel(1);
                Box::pin(ReceiverStream::new(rx))
            }
            fn has_refresh_models(&self) -> bool {
                true
            }
            async fn refresh_models(
                &self,
                ctx: &crate::provider::RefreshModelsContext,
            ) -> Option<Result<(), ProviderError>> {
                if let Ok(mut seen) = self.rec.seen.lock() {
                    seen.push((ctx.allow_network, ctx.force));
                }
                Some(Ok(()))
            }
        }

        let rec = Arc::new(Recorder::default());
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::new(RecordingProvider {
            id: cyrup_core::ProviderId::from("rec"),
            models: vec![model("rec", "m1", false, None)],
            rec: rec.clone(),
        }));

        models
            .refresh_with(None, ModelsRefreshOptions::forced())
            .await;
        models
            .refresh_with(None, ModelsRefreshOptions::cache_only())
            .await;
        models
            .refresh_with(None, ModelsRefreshOptions::default())
            .await;

        let seen = rec.seen.lock().expect("not poisoned").clone();
        assert_eq!(
            seen,
            vec![(true, true), (false, false), (true, false)],
            "forced ⇒ (network, force); cache_only ⇒ (offline, no force); default ⇒ pi's \
             `allowNetwork ?? true` with force falsy (models.ts:277)"
        );
    }

    /// PROV-S05 (c) — **the abort actually aborts.** This is the guarantee the item was filed on:
    /// a signal that is accepted and then ignored is worse than none.
    ///
    /// Two distinct properties, because only the pair makes the signal real:
    ///
    /// 1. a provider parked in its fetch is CUT OFF when the token fires (it returns because it
    ///    selected on `ctx.cancel`, which is only possible because the token reaches it); and
    /// 2. a provider whose turn had not started is never called at all — pi's
    ///    `if (options.signal?.aborted) return;` (`models.ts:286`).
    ///
    /// Also pins pi's `:305` guard: an aborted provider records NO error, even though its fetch
    /// returned `Err`. Cancellation is not a provider failure.
    ///
    /// **Red before the fix:** there was no token to pass, no `aborted` to read, and no `errors` to
    /// find empty — `refresh(None)` would have parked forever on the held provider with no way for
    /// any caller to interrupt it.
    #[tokio::test]
    async fn prov_s05_cancel_actually_aborts_an_in_flight_refresh() {
        let hold = Arc::new(tokio::sync::Notify::new());
        let (held, held_fetches, _) = dyn_provider_full("dyn-held", false, Some(hold.clone()));

        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(held);

        let cancel = cyrup_core::CancelToken::new();
        let options = ModelsRefreshOptions::default().with_cancel(cancel.clone());

        // Nothing ever notifies `hold`, so the ONLY way this resolves is the abort arm.
        let refresh = models.refresh_with(None, options);
        tokio::pin!(refresh);

        // Let the fetch reach its park before cancelling, so this is a genuine mid-flight abort
        // rather than the pre-check.
        tokio::select! {
            biased;
            _ = &mut refresh => panic!("refresh settled before the fetch was even entered"),
            () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }
        assert_eq!(
            held_fetches.load(Ordering::SeqCst),
            1,
            "the fetch must be in flight for this to test a mid-flight abort"
        );

        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), refresh)
            .await
            .expect("cancel must cut the in-flight refresh off, not merely be recorded");

        assert!(
            result.aborted,
            "models.ts:327 — `aborted: signal?.aborted ?? false`"
        );
        assert!(
            result.errors.is_empty(),
            "models.ts:305 — an aborted provider records no error, even though its fetch returned Err"
        );
    }

    /// PROV-S05 (c2) — the pre-check half of the abort: a provider whose turn has not begun when
    /// the token is already cancelled is never called (pi `models.ts:286`).
    #[tokio::test]
    async fn prov_s05_a_pre_cancelled_refresh_calls_no_provider() {
        let mut models = create_models(CreateModelsOptions::default());
        let (p, fetches, restores) = dyn_provider_full("dyn-ok", false, None);
        models.set_provider(p);

        let cancel = cyrup_core::CancelToken::new();
        cancel.cancel();
        let result = models
            .refresh_with(None, ModelsRefreshOptions::default().with_cancel(cancel))
            .await;

        assert!(result.aborted);
        assert!(result.errors.is_empty());
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            0,
            "models.ts:286 — an already-aborted refresh must not start any provider"
        );
        assert_eq!(restores.load(Ordering::SeqCst), 0);
    }

    /// PROV-042. `transformHeaders` runs after auth + request headers are merged (Pi
    /// `models.ts:480` @v0.83.0), sees the FINAL set, its return value wins, and it is stripped
    /// from the options the provider receives (`:483`).
    ///
    /// Red before the fix: `rg transform_headers crates/` was empty workspace-wide, so an
    /// extension could not observe or modify outbound provider headers at all while its two sibling
    /// hooks (`before_provider_request` / `after_provider_response`) worked.
    #[tokio::test]
    async fn transform_headers_runs_last_and_is_stripped_from_provider_options() {
        use std::sync::{Arc, Mutex};

        let seen: Arc<Mutex<Option<crate::HeaderMap>>> = Arc::new(Mutex::new(None));
        let recorder = seen.clone();
        let transform: crate::stream::TransformHeadersFn =
            Arc::new(move |mut headers: crate::HeaderMap| {
                let recorder = recorder.clone();
                Box::pin(async move {
                    *recorder.lock().unwrap_or_else(|e| e.into_inner()) = Some(headers.clone());
                    // Removing an auth header inside the transform must actually suppress it.
                    headers.remove("x-api-key");
                    headers.insert("x-test".to_string(), Some("1".to_string()));
                    headers
                })
            });

        let mut request_headers = crate::HeaderMap::new();
        request_headers.insert("x-request".to_string(), Some("r".to_string()));
        let mut auth_headers = crate::HeaderMap::new();
        auth_headers.insert("x-api-key".to_string(), Some("k".to_string()));

        let helper = AuthHelper {
            provider: Arc::new(HeaderAuthProvider {
                id: "p".into(),
                models: vec![header_model()],
                auth: crate::auth::ProviderAuth::with_api_key(Arc::new(FixedHeaderAuth(
                    auth_headers,
                ))),
            }),
            credentials: Arc::new(InMemoryCredentialStore::new()),
            auth_context: Arc::new(EnvAuthContext),
        };
        let options = StreamOptions {
            headers: Some(request_headers),
            transform_headers: Some(transform),
            ..Default::default()
        };
        let (_model, out) = helper
            .apply_auth(&header_model(), &options)
            .await
            .expect("apply_auth");

        // The transform observed the ALREADY-MERGED auth + request headers.
        let observed = seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("transform ran");
        assert_eq!(observed.get("x-api-key"), Some(&Some("k".to_string())));
        assert_eq!(observed.get("x-request"), Some(&Some("r".to_string())));

        // Its return value wins, in both directions.
        let final_headers = out.headers.expect("headers");
        assert_eq!(final_headers.get("x-test"), Some(&Some("1".to_string())));
        assert!(
            !final_headers.contains_key("x-api-key"),
            "removing a header inside the transform must suppress it"
        );
        // And it is not visible to the api impl as an option field (models.ts:483).
        assert!(out.transform_headers.is_none());
    }

    fn header_model() -> Model {
        Model {
            id: "m".into(),
            name: "M".into(),
            api: "openai-completions".into(),
            provider: "p".into(),
            base_url: "https://example.invalid".to_string(),
            reasoning: false,
            input: vec![crate::model::Modality::Text],
            cost: crate::model::ModelCost::default(),
            input_limits: None,
            prompt_cache: None,
            context_window: 1000,
            max_tokens: 100,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    /// An api-key strategy that resolves to a fixed header overlay and no key.
    struct FixedHeaderAuth(crate::HeaderMap);

    #[async_trait::async_trait]
    impl crate::auth::ApiKeyAuth for FixedHeaderAuth {
        fn name(&self) -> &str {
            "fixed-headers"
        }
        async fn resolve(
            &self,
            _model: &Model,
            _ctx: &dyn AuthContext,
            _cred: Option<&crate::auth::Credential>,
        ) -> Result<Option<AuthResult>, crate::error::AuthError> {
            Ok(Some(AuthResult {
                auth: crate::auth::types::ModelAuth {
                    api_key: None,
                    headers: Some(self.0.clone()),
                    base_url: None,
                },
                env: None,
                source: Some("test".to_string()),
            }))
        }
    }

    struct HeaderAuthProvider {
        id: cyrup_core::ProviderId,
        models: Vec<Model>,
        auth: crate::auth::ProviderAuth,
    }

    impl Provider for HeaderAuthProvider {
        fn id(&self) -> &cyrup_core::ProviderId {
            &self.id
        }
        fn models(&self) -> &[Model] {
            &self.models
        }
        fn provider_auth(&self) -> Option<&crate::auth::ProviderAuth> {
            Some(&self.auth)
        }
        fn stream(
            &self,
            _model: &Model,
            _context: &Context,
            _options: &StreamOptions,
        ) -> EventStream<StreamEvent> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Box::pin(ReceiverStream::new(rx))
        }
    }

    /// An api-key strategy whose resolution yields a provider-scoped `env` (and no key), the
    /// shape llama.cpp's re-resolution has.
    struct FixedEnvAuth(crate::stream::ProviderEnv);

    #[async_trait::async_trait]
    impl crate::auth::ApiKeyAuth for FixedEnvAuth {
        fn name(&self) -> &str {
            "fixed-env"
        }
        async fn resolve(
            &self,
            _model: &Model,
            _ctx: &dyn AuthContext,
            _cred: Option<&crate::auth::Credential>,
        ) -> Result<Option<AuthResult>, crate::error::AuthError> {
            Ok(Some(AuthResult {
                auth: crate::auth::types::ModelAuth {
                    api_key: None,
                    headers: None,
                    base_url: None,
                },
                env: Some(self.0.clone()),
                source: Some("test".to_string()),
            }))
        }
    }

    fn env_helper(resolution_env: crate::stream::ProviderEnv) -> AuthHelper {
        AuthHelper {
            provider: Arc::new(HeaderAuthProvider {
                id: "p".into(),
                models: vec![header_model()],
                auth: crate::auth::ProviderAuth::with_api_key(Arc::new(FixedEnvAuth(
                    resolution_env,
                ))),
            }),
            credentials: Arc::new(InMemoryCredentialStore::new()),
            auth_context: Arc::new(EnvAuthContext),
        }
    }

    /// PROV-109. `applyAuth` folds the resolution's `env` into the request options:
    /// `env = { ...(resolution.env ?? {}), ...(options?.env ?? {}) }` (pi `models.ts:862`
    /// @v0.99.2-17), the same merge `Models::classify` does. `Models::stream` used to drop it.
    #[tokio::test]
    async fn apply_auth_merges_the_resolution_env_into_the_request_options() {
        let helper = env_helper(BTreeMap::from([
            ("LLAMA_BASE_URL".to_string(), "http://resolved".to_string()),
            ("SHARED".to_string(), "from-resolution".to_string()),
        ]));

        // No request env: the resolution's env reaches the provider unchanged.
        let (_model, out) = helper
            .apply_auth(&header_model(), &StreamOptions::default())
            .await
            .expect("apply_auth");
        assert_eq!(
            out.env,
            Some(BTreeMap::from([
                ("LLAMA_BASE_URL".to_string(), "http://resolved".to_string()),
                ("SHARED".to_string(), "from-resolution".to_string()),
            ]))
        );

        // An explicit request env wins per key; keys only the resolution has survive.
        let options = StreamOptions {
            env: Some(BTreeMap::from([
                ("SHARED".to_string(), "from-options".to_string()),
                ("ONLY_OPTION".to_string(), "o".to_string()),
            ])),
            ..Default::default()
        };
        let (_model, out) = helper
            .apply_auth(&header_model(), &options)
            .await
            .expect("apply_auth");
        assert_eq!(
            out.env,
            Some(BTreeMap::from([
                ("LLAMA_BASE_URL".to_string(), "http://resolved".to_string()),
                ("SHARED".to_string(), "from-options".to_string()),
                ("ONLY_OPTION".to_string(), "o".to_string()),
            ]))
        );
    }

    // ---- CFG-020c: the optional `ApiKeyAuth.check` hook ----
    // pi `ai/src/auth/types.ts:180-186` (the `check?` member) consulted by
    // `Models.checkProviderAuth` at `ai/src/models.ts:506-517` @v0.87.1.

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum CheckOutcome {
        /// `check?` absent upstream: `supports_check()` is false and `Models` falls through to the
        /// resolution path. Every built-in provider is this, on both sides.
        Absent,
        /// `check` answers "configured", with its own `source` string.
        Configured,
        /// `check` answers `undefined` even though `resolve` would report configured.
        Unconfigured,
        /// `check` rejects — upstream rethrows it as `ModelsError("auth", ...)` (`models.ts:514-516`).
        Fails,
    }

    /// An api-key strategy that answers `check` and records whether `resolve` was reached.
    /// `resolve` always reports CONFIGURED with `source = "resolve-path"`, so "which path ran" is
    /// readable straight off the returned `AuthCheck`.
    struct CheckingAuth {
        outcome: CheckOutcome,
        checks: Arc<std::sync::atomic::AtomicUsize>,
        resolves: Arc<std::sync::atomic::AtomicUsize>,
        saw_api_key_credential: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl crate::auth::ApiKeyAuth for CheckingAuth {
        fn name(&self) -> &str {
            "checking"
        }

        fn supports_check(&self) -> bool {
            self.outcome != CheckOutcome::Absent
        }

        async fn check(
            &self,
            _ctx: &dyn AuthContext,
            cred: Option<&Credential>,
        ) -> Result<Option<AuthCheck>, crate::error::AuthError> {
            self.checks
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.saw_api_key_credential.store(
                matches!(cred, Some(Credential::ApiKey { .. })),
                std::sync::atomic::Ordering::SeqCst,
            );
            match self.outcome {
                CheckOutcome::Configured => Ok(Some(AuthCheck {
                    auth_type: AuthType::ApiKey,
                    source: Some("check-hook".to_string()),
                })),
                CheckOutcome::Unconfigured | CheckOutcome::Absent => Ok(None),
                CheckOutcome::Fails => Err(crate::error::AuthError::api_key(
                    ProviderId::from("checked"),
                    "check hook exploded",
                )),
            }
        }

        async fn resolve(
            &self,
            _model: &Model,
            _ctx: &dyn AuthContext,
            _cred: Option<&Credential>,
        ) -> Result<Option<AuthResult>, crate::error::AuthError> {
            self.resolves
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(AuthResult {
                auth: crate::auth::types::ModelAuth {
                    api_key: Some("k".to_string()),
                    headers: None,
                    base_url: None,
                },
                env: None,
                source: Some("resolve-path".to_string()),
            }))
        }
    }

    struct CheckProbe {
        models: Models,
        checks: Arc<std::sync::atomic::AtomicUsize>,
        resolves: Arc<std::sync::atomic::AtomicUsize>,
        saw_api_key_credential: Arc<std::sync::atomic::AtomicBool>,
        store: Arc<InMemoryCredentialStore>,
    }

    impl CheckProbe {
        fn checks(&self) -> usize {
            self.checks.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn resolves(&self) -> usize {
            self.resolves.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn saw_api_key_credential(&self) -> bool {
            self.saw_api_key_credential
                .load(std::sync::atomic::Ordering::SeqCst)
        }
        async fn store_credential(&self, cred: Credential) {
            self.store
                .modify(
                    &ProviderId::from("checked"),
                    Box::new(move |_| Box::pin(async move { Ok(Some(cred)) })),
                )
                .await
                .expect("store credential");
        }
    }

    fn check_probe(outcome: CheckOutcome) -> CheckProbe {
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resolves = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let saw_api_key_credential = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let store = Arc::new(InMemoryCredentialStore::new());
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(store.clone()),
            ..Default::default()
        });
        let mut model = header_model();
        model.provider = "checked".into();
        models.set_provider(Arc::new(HeaderAuthProvider {
            id: ProviderId::from("checked"),
            models: vec![model],
            auth: crate::auth::ProviderAuth::with_api_key(Arc::new(CheckingAuth {
                outcome,
                checks: checks.clone(),
                resolves: resolves.clone(),
                saw_api_key_credential: saw_api_key_credential.clone(),
            })),
        }));
        CheckProbe {
            models,
            checks,
            resolves,
            saw_api_key_credential,
            store,
        }
    }

    /// CFG-020c. `if (apiKey.check) { return await apiKey.check(...) }` (`models.ts:506-517`)
    /// returns from `checkProviderAuth` — the `resolveProviderAuth` call at `:518` is NOT reached.
    ///
    /// Red before the fix: `ApiKeyAuth` had no `check`, so `check_auth` always resolved and the
    /// reported source was `"resolve-path"`, not the hook's `"check-hook"`.
    #[tokio::test]
    async fn check_auth_answers_from_the_check_hook_instead_of_resolving() {
        let probe = check_probe(CheckOutcome::Configured);
        let checked = probe
            .models
            .check_auth("checked")
            .await
            .expect("check_auth")
            .expect("configured");
        assert_eq!(checked.auth_type, AuthType::ApiKey);
        assert_eq!(
            checked.source.as_deref(),
            Some("check-hook"),
            "the hook's own AuthCheck is returned verbatim (models.ts:508-512)"
        );
        assert_eq!(probe.checks(), 1, "the hook is consulted exactly once");
        assert_eq!(
            probe.resolves(),
            0,
            "consulting `check` REPLACES the resolution path — that is the point of the hook: \
             upstream documents it for strategies whose `resolve` executes commands"
        );
    }

    /// CFG-020c. A hook answering `undefined` means "not configured" and stops there; it does not
    /// fall through to resolution. Absent-vs-present-and-undefined is the distinction
    /// `supports_check` exists to carry.
    #[tokio::test]
    async fn check_hook_reporting_unconfigured_beats_a_resolvable_strategy() {
        let probe = check_probe(CheckOutcome::Unconfigured);
        assert!(
            probe
                .models
                .check_auth("checked")
                .await
                .expect("check_auth")
                .is_none(),
            "check() -> undefined is the answer, even though resolve() would report configured"
        );
        assert_eq!(probe.checks(), 1);
        assert_eq!(probe.resolves(), 0);
        assert!(
            probe
                .models
                .get_available(None)
                .await
                .expect("get_available")
                .is_empty(),
            "getAvailable drops a provider whose checkProviderAuth answered undefined \
             (models.ts:546-547)"
        );
    }

    /// CFG-020c. The credential handed to the hook is the stored one and only when it is an
    /// api-key credential — `credential?.type === "api_key" ? credential : undefined`
    /// (`models.ts:510`).
    #[tokio::test]
    async fn check_hook_receives_the_stored_api_key_credential() {
        let probe = check_probe(CheckOutcome::Configured);
        assert!(
            probe
                .models
                .check_auth("checked")
                .await
                .expect("check_auth")
                .is_some()
        );
        assert!(
            !probe.saw_api_key_credential(),
            "nothing stored yet, so the hook is passed None"
        );

        probe.store_credential(Credential::api_key("stored")).await;
        assert!(
            probe
                .models
                .check_auth("checked")
                .await
                .expect("check_auth")
                .is_some()
        );
        assert!(
            probe.saw_api_key_credential(),
            "the stored api-key credential reaches the hook"
        );
    }

    /// CFG-020c. `catch (error) { throw new ModelsError("auth", ...) }` (`models.ts:514-516`) —
    /// a failing hook is an ERROR, never "this provider is unconfigured".
    ///
    /// Red before the fix: `check_auth` returned a bare `Option` with no error channel at all, and
    /// answered `Some(resolve-path)` here.
    #[tokio::test]
    async fn check_hook_failure_propagates_rather_than_reading_as_unconfigured() {
        let probe = check_probe(CheckOutcome::Fails);
        let error = probe
            .models
            .check_auth("checked")
            .await
            .expect_err("a failing check hook must surface");
        assert!(
            matches!(error, ProviderError::Auth(ref e) if e.code() == "auth"),
            "taxonomy code `auth`, as `new ModelsError(\"auth\", ...)` (models.ts:515): {error:?}"
        );
        assert_eq!(probe.resolves(), 0);
        probe
            .models
            .get_available(None)
            .await
            .expect_err("getAvailable inherits the rethrow (models.ts:544)");
    }

    /// CFG-020c. `check?` is OPTIONAL: every built-in provider omits it on both sides, so the
    /// default must leave today's behaviour untouched — `Missing means Models checks availability
    /// by resolving auth` (`auth/types.ts:182-184`). Green before and after the fix.
    #[tokio::test]
    async fn a_strategy_without_check_still_takes_the_resolution_path() {
        let probe = check_probe(CheckOutcome::Absent);
        let checked = probe
            .models
            .check_auth("checked")
            .await
            .expect("check_auth")
            .expect("configured");
        assert_eq!(checked.source.as_deref(), Some("resolve-path"));
        assert_eq!(probe.checks(), 0, "the hook is not consulted when absent");
        assert_eq!(probe.resolves(), 1);
        assert_eq!(
            probe
                .models
                .get_available(None)
                .await
                .expect("get_available")
                .len(),
            1
        );
    }

    /// CFG-020c. The hook sits AFTER the stored-OAuth branch (`models.ts:502-503`), so a stored
    /// OAuth credential on an api-key-only provider never reaches it.
    #[tokio::test]
    async fn a_stored_oauth_credential_short_circuits_before_the_check_hook() {
        let probe = check_probe(CheckOutcome::Configured);
        probe
            .store_credential(Credential::Oauth {
                refresh: "r".to_string(),
                access: "a".to_string(),
                expires: 0,
                ext: serde_json::Map::new(),
            })
            .await;
        assert!(
            probe
                .models
                .check_auth("checked")
                .await
                .expect("check_auth")
                .is_none(),
            "no oauth strategy on this provider, so the oauth branch answers undefined"
        );
        assert_eq!(probe.checks(), 0);
        assert_eq!(probe.resolves(), 0);
    }

    /// PROV-118 (a) — `Models.login(providerId, type, interaction, options?)` is where the app
    /// hands a flow its installation id (`models.ts:756-770`, `method.login({…}, options)` at
    /// `:770`). [`Models::login_with_options`] is the call WITH the trailing argument and
    /// [`Models::login`] the one without; this pins that both reach the same flow and that only
    /// the former carries a device id.
    #[tokio::test]
    async fn login_with_options_threads_the_device_id_to_the_oauth_flow() {
        use crate::auth::oauth::{AuthInteraction, OAuthError, ScriptedInteraction};
        use crate::auth::{LoginOptions, ModelAuth, OAuthAuth, ProviderAuth};
        use crate::error::AuthError;

        /// Mints a credential whose access token is the device id it was given, so the value is
        /// read back out of the stored credential rather than a side channel.
        struct DeviceIdFlow;

        #[async_trait::async_trait]
        impl OAuthAuth for DeviceIdFlow {
            fn name(&self) -> &str {
                "Device Id Flow"
            }
            async fn login(
                &self,
                _interaction: &dyn AuthInteraction,
                options: &LoginOptions,
            ) -> Result<Credential, OAuthError> {
                Ok(crate::auth::oauth_credential(
                    options.device_id().unwrap_or_else(|| "none".to_string()),
                    "r",
                    0,
                ))
            }
            async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
                Ok(cred.clone())
            }
            async fn to_auth(&self, _cred: &Credential) -> Result<ModelAuth, AuthError> {
                Ok(ModelAuth::default())
            }
        }

        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::new(crate::wire::WireProvider::new(
            "deviced",
            "Deviced",
            vec![model("deviced", "m", false, None)],
            ProviderAuth::with_oauth(Arc::new(DeviceIdFlow)),
            Arc::new(crate::auth::InMemoryCredentialStore::new()),
            Arc::new(crate::api::builtin_registry()),
        )));

        let interaction = ScriptedInteraction::new(Vec::new());
        let cred = models
            .login_with_options(
                "deviced",
                AuthType::Oauth,
                &interaction,
                &LoginOptions::with_device_id(|| "install-7f3a".to_string()),
            )
            .await
            .unwrap();
        match &cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "install-7f3a"),
            other => panic!("expected an oauth credential, got {other:?}"),
        }

        // The no-options entry point is `options === undefined`, not a second code path.
        let cred = models
            .login("deviced", AuthType::Oauth, &interaction)
            .await
            .unwrap();
        match &cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "none"),
            other => panic!("expected an oauth credential, got {other:?}"),
        }
    }
}

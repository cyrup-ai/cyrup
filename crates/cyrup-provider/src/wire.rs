//! `WireProvider` — a reusable `Provider` implementation backing real (and test) wire protocols
//! (arch-01 §3.4/§5). It holds a static model catalog + a `ProviderAuth` + a `CredentialStore` + an
//! `ApiRegistry`, and in `stream()` spawns a task that resolves auth, looks up the `ApiImpl` for
//! `model.api`, and drives `run(...)` into an `EventStream<StreamEvent>`.
//!
//! `stream()` returns IMMEDIATELY (func-01 R-01-009); the network lives behind the returned stream.
//! Auth/registry failures arrive as a terminal `StreamEvent::Error` (R-01-018/045) — never as a
//! thrown error.
//!
//! Concrete providers (anthropic/together/…) are `WireProvider` + a catalog + an api mapping; those
//! land in the next steps.

use crate::api::{ApiRegistry, channel};
use crate::auth::{
    AuthContext, AuthOverrides, CredentialStore, EnvAuthContext, ProviderAuth,
    resolve_provider_auth,
};
use crate::classifier::{
    AnyModel, ClassifierApiRegistry, ClassifierContext, ClassifierModel, ClassifierOptions,
    ClassifierResult, ImageModel, ProviderClassifier,
};
use crate::collection::clamp_thinking_level;
use crate::context::Context;
use crate::error::ProviderError;
use crate::images::{
    AssistantImages, ImageApiRegistry, ImagesContext, ImagesOptions, ProviderImages,
};
use crate::model::Model;
use crate::provider::{ModelsPublication, Provider, RefreshModelsContext};
use crate::stream::{StreamEvent, StreamOptions};
use crate::utils::simple_options::{SimpleStreamOptions, build_base_options};
use cyrup_core::{ApiId, EventStream, ProviderId};
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

/// Channel buffer for the producer→consumer stream (bounded back-pressure, arch-01 §10).
const STREAM_BUFFER: usize = 64;

/// A provider that speaks one or more wire protocols via the `ApiRegistry` (arch-01 §3.4).
pub struct WireProvider {
    id: ProviderId,
    name: String,
    models: Vec<Model>,
    auth: ProviderAuth,
    store: Arc<dyn CredentialStore>,
    auth_ctx: Arc<dyn AuthContext>,
    registry: Arc<ApiRegistry>,
    /// Pi `createProvider({ baseUrl })` (`CreateProviderOptions.baseUrl`, `models.ts:1000`, carried
    /// onto `Provider.baseUrl` at `:1088` @f1b2e77f5) — PROV-017, PROV-154.
    base_url: Option<String>,
    /// Pi `createProvider({ headers })` (`CreateProviderOptions.headers`, `models.ts:538` @v0.83.0;
    /// the `Provider.headers` it feeds is `:80`) — PROV-017.
    headers: Option<crate::HeaderMap>,
    /// Pi `createProvider({ filterModels })` — the option `models.ts:545`/`:618` transports onto
    /// the constructed provider, applied by `Models.getAvailable()` at `:407`. PROV-032.
    filter_models: Option<FilterModelsFn>,
    /// Pi `createProvider({ filterAllModels })` (`CreateProviderOptions.filterAllModels`,
    /// `models.ts:1016-1020`, carried onto the provider at `:1122` @f1b2e77f5), applied by
    /// `Models.getAllAvailable()` (`:730`) in place of the chat-only default. PROV-147: the
    /// `openai` provider installs one so an OAuth credential does not list its Decisions rows.
    filter_all_models: Option<FilterAllModelsFn>,
    /// The provider's `type: "image"` catalog rows (pi's ONE `models` array holds every type;
    /// `CreateProviderOptions.models: readonly ProviderModel<TApi>[]`, `models.ts:997`). Held
    /// apart from [`WireProvider::models`] rather than in one `Vec<AnyModel>` so that
    /// [`Provider::models`] — whose every caller streams what it is handed — CANNOT return an
    /// image row: that is a type-level fact here, not a filter a caller has to remember. pi needs
    /// the runtime `isModelType` filter at `models.ts:1084` precisely because its one array is
    /// structurally typed. PROV-128.
    image_models: Vec<ImageModel>,
    /// Pi `createProvider({ images })` (`CreateProviderOptions.images`, `models.ts:1021`) — the
    /// per-api image-generation map, dispatched on `model.api` at `:1146-1160`. `None` is pi's
    /// absent option, which is what makes `provider.generateImages` absent. PROV-128.
    images: Option<Arc<ImageApiRegistry>>,
    /// The provider's `type: "classifier"` catalog rows, held apart from [`WireProvider::models`]
    /// for the reason [`WireProvider::image_models`] is (`CreateProviderOptions.models`,
    /// `models.ts:997`). PROV-104.
    classifier_models: Vec<ClassifierModel>,
    /// Pi `createProvider({ classifiers })` (`CreateProviderOptions.classifiers`) — the per-api
    /// classifier map, dispatched on `model.api` (`models.ts:1161-1171`). `None` is pi's absent
    /// option, which is what makes `provider.classify` absent. PROV-104.
    classifiers: Option<Arc<ClassifierApiRegistry>>,
    /// The provider this one was composed over — pi `composeModelProvider`'s `base`
    /// (`provider-composer.ts:657`, `:669` @v1.0.4), whose `generateImages` and `classify` the
    /// composed provider calls through (`provider.generateImages = … base.generateImages`,
    /// `provider.classify = … base.classify`). `None` for every provider that owns its own
    /// operations. When set, it answers the image and classifier operations INSTEAD of
    /// `images` and `classifiers`.
    operations_base: Option<Arc<dyn Provider>>,
    /// What this provider publishes as its `update` after [`WireProvider::operations_base`]
    /// refreshed ([`WireProvider::with_refresh_update`], PROV-164).
    refresh_update: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Which of pi's two api lookups [`WireProvider::registry`] stands for, and so what a miss on
    /// `model.api` reports (PROV-157). [`ApiDispatch::Own`] unless the constructing site declares
    /// otherwise.
    api_dispatch: ApiDispatch,
    /// SEAM-173 — [`WireProvider::operations_base`] encapsulates its own auth (it has no
    /// [`Provider::provider_auth`]) and the layer composed over it configures none, so a model the
    /// base answers streams through it UNRESOLVED and this provider has no auth strategy either
    /// ([`WireProvider::with_auth_from_base`]).
    auth_from_base: bool,
}

/// Which of pi's two api lookups a [`WireProvider`]'s [`ApiRegistry`] stands for — and so which of
/// pi's two texts a stream reports when the registry has no entry for `model.api` (PROV-157).
///
/// cyrup dispatches every provider through an [`ApiRegistry`] keyed on `model.api`; pi has two
/// different maps behind that one lookup, and they fail differently. Both texts are
/// [`ProviderError`] variants; this type decides between them, so no construction site spells
/// either string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApiDispatch {
    /// The provider's OWN api map — pi `createProvider({ api })` (`models.ts:1070-1083`
    /// @f1b2e77f5), which every built-in is. A miss is [`ProviderError::NoApiImpl`]: `Provider
    /// <id> has no API implementation for "<api>"`.
    #[default]
    Own,
    /// The PROCESS-WIDE api registry — reached by pi's composed provider (`composeModelProvider`'s
    /// `streamWith` falls through to `getApiProvider(model.api)`, `provider-composer.ts:598-599`),
    /// which is what a `models.json` block and a guest registration under a fresh id are
    /// (`model-runtime.ts:921-933` → `composeModelProvider`), and by a provider that streams through
    /// `compat` `stream` (llama.cpp, `extensions/llama/provider.ts:288-289`). A miss is
    /// [`ProviderError::UnregisteredApi`]: `No API provider registered for api: <api>`.
    ///
    /// A provider composed over a base ([`WireProvider::with_operations_from`]) first asks the base:
    /// pi hands `base.stream` every model whose api the base's own chat catalog lists
    /// (`supportsBaseApi`, `provider-composer.ts:582`, `:593-596`), so such a model streams through
    /// the base — and a miss there is the base's own text. [`WireProvider::stream`] resolves that
    /// per request (PROV-162).
    Global,
}

impl ApiDispatch {
    /// The error a miss on `api` reports. `provider` is the dispatching provider's own id (pi
    /// `input.id`); [`ApiDispatch::Global`]'s text names no provider.
    #[must_use]
    pub fn miss(self, provider: &ProviderId, api: &ApiId) -> ProviderError {
        match self {
            ApiDispatch::Own => ProviderError::NoApiImpl {
                provider: provider.clone(),
                api: api.clone(),
            },
            ApiDispatch::Global => ProviderError::UnregisteredApi(api.clone()),
        }
    }
}

/// A provider's credential-scoped availability policy (Pi `Provider.filterModels?`,
/// `models.ts:111` @v0.83.0). A plain fn pointer, because every upstream implementation is a
/// stateless pure function of `(models, credential)`.
pub type FilterModelsFn = fn(&[Model], Option<&crate::auth::Credential>) -> Vec<Model>;

/// A provider's credential-scoped availability policy across every model type (Pi
/// `Provider.filterAllModels?`, `models.ts:196-206` @f1b2e77f5). A plain fn pointer for the reason
/// [`FilterModelsFn`] is one. PROV-147.
pub type FilterAllModelsFn = fn(&[AnyModel], Option<&crate::auth::Credential>) -> Vec<AnyModel>;

impl WireProvider {
    /// Construct a provider. `auth_ctx` defaults to the real-env [`EnvAuthContext`] via
    /// [`WireProvider::new`]; use [`WireProvider::with_auth_context`] to inject a test context.
    pub fn new(
        id: impl Into<ProviderId>,
        name: impl Into<String>,
        models: Vec<Model>,
        auth: ProviderAuth,
        store: Arc<dyn CredentialStore>,
        registry: Arc<ApiRegistry>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            models,
            auth,
            store,
            auth_ctx: Arc::new(EnvAuthContext),
            registry,
            base_url: None,
            headers: None,
            filter_models: None,
            filter_all_models: None,
            image_models: Vec::new(),
            images: None,
            classifier_models: Vec::new(),
            classifiers: None,
            operations_base: None,
            refresh_update: None,
            api_dispatch: ApiDispatch::Own,
            auth_from_base: false,
        }
    }

    /// Set the provider-level default base URL (Pi `createProvider({ baseUrl })`) — PROV-017.
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Set the provider-level default headers (Pi `createProvider({ headers })`) — PROV-017.
    #[must_use]
    pub fn with_headers(mut self, headers: crate::HeaderMap) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Install the credential-scoped availability policy (Pi `createProvider({ filterModels })`) —
    /// PROV-032. Applied only by `Models::get_available`, never by `models()`.
    #[must_use]
    pub fn with_filter_models(mut self, filter: FilterModelsFn) -> Self {
        self.filter_models = Some(filter);
        self
    }

    /// Install the credential-scoped availability policy across every model type (Pi
    /// `createProvider({ filterAllModels })`) — PROV-147. Applied only by
    /// `Models::get_all_available` / `get_available_of_type`, where it REPLACES the default
    /// (`filter_models` over the chat rows, every other row kept), as pi's
    /// `if (provider.filterAllModels) return provider.filterAllModels(models, credential)` does.
    #[must_use]
    pub fn with_filter_all_models(mut self, filter: FilterAllModelsFn) -> Self {
        self.filter_all_models = Some(filter);
        self
    }

    /// Add the provider's image catalog rows (pi's `...Object.values(OPENROUTER_IMAGE_MODELS)`
    /// inside the one `models` array, `providers/openrouter.ts:25`). They appear in
    /// [`Provider::get_all_models`] and NOT in [`Provider::models`]. PROV-128.
    #[must_use]
    pub fn with_image_models(mut self, models: Vec<ImageModel>) -> Self {
        self.image_models = models;
        self
    }

    /// Install the per-api image-generation map (pi `createProvider({ images })`,
    /// `providers/openrouter.ts:33`). Installing it is what makes
    /// [`Provider::supports_image_generation`] true, exactly as pi attaches `generateImages` only
    /// when `images` carries at least one implementation (`models.ts:1146`); an EMPTY registry is
    /// therefore ignored, which is pi's `imageImplementations.length === 0`. PROV-128.
    #[must_use]
    pub fn with_images(mut self, images: Arc<ImageApiRegistry>) -> Self {
        self.images = (!images.is_empty()).then_some(images);
        self
    }

    /// Add the provider's classifier catalog rows (pi's
    /// `...Object.values(<PROVIDER>_CLASSIFIER_MODELS)` inside the one `models` array,
    /// `providers/openrouter.ts:26`). They appear in [`Provider::get_all_models`] and NOT in
    /// [`Provider::models`]. PROV-104.
    #[must_use]
    pub fn with_classifier_models(mut self, models: Vec<ClassifierModel>) -> Self {
        self.classifier_models = models;
        self
    }

    /// Install the per-api classifier map (pi `createProvider({ classifiers })`,
    /// `providers/openrouter.ts:34`, `providers/cloudflare-workers-ai.ts:21-23`). Installing it is
    /// what makes [`Provider::supports_classification`] true, exactly as pi attaches `classify`
    /// only when `classifiers` carries an implementation; an EMPTY registry is ignored, as
    /// [`WireProvider::with_images`] ignores an empty image map. PROV-104.
    #[must_use]
    pub fn with_classifiers(mut self, classifiers: Arc<ClassifierApiRegistry>) -> Self {
        self.classifiers = (!classifiers.is_empty()).then_some(classifiers);
        self
    }

    /// Make `base` the owner of this provider's image generation and classification — pi
    /// `composeModelProvider` (`provider-composer.ts` @v1.0.4), whose composed provider is built
    /// `generateImages: base.generateImages` and `classify: base.classify` over `base`'s
    /// `getAllModels()`: the composition changes the catalog and the auth, never what the provider
    /// can do. Whether the operations exist at all is `base`'s answer too
    /// ([`Provider::supports_image_generation`], [`Provider::supports_classification`]), as pi
    /// attaches the members only `if (base?.generateImages)` / `if (base?.classify)`.
    ///
    /// The same composition carries `base`'s availability filters through: `filterModels:
    /// base?.filterModels ? (models, credential) => base.filterModels!(models, credential) :
    /// undefined` and the same for `filterAllModels` (`provider-composer.ts:636-641` @f1b2e77f5),
    /// so [`Provider::filter_models`] and [`Provider::filter_all_models`] answer with `base`'s
    /// (PROV-147; `CFG-112` for the chat filter, which the composer dropped before).
    ///
    /// And its catalog refresh: the composed provider refreshes exactly when `base` does (pi
    /// `refreshModels: base?.refreshModels || …`, `provider-composer.ts:613` @f1b2e77f5), by running
    /// `base`'s refresh (PROV-164; see [`Provider::refresh_models`] below).
    #[must_use]
    pub fn with_operations_from(mut self, base: Arc<dyn Provider>) -> Self {
        self.operations_base = Some(base);
        self
    }

    /// SEAM-173, `[CYRUP-DELTA]` in mechanism — the base this provider was composed over
    /// ([`Self::with_operations_from`]) carries its auth inside itself, and the layer adds none.
    ///
    /// cyrup's [`Provider::provider_auth`] is optional: `None` is a provider that "fully
    /// encapsulates its own auth", which a [`crate::Models`] collection streams without
    /// re-resolution — an embedder's own provider, the offline faux. pi has no such provider (every
    /// `Provider` carries `auth`), so `composeApiKeyAuth` always has a base strategy to inherit
    /// (`provider-composer.ts:417-455` @f1b2e77f5). Composing a configured-key strategy over a
    /// base that has none would refuse every request the base used to stream (`Provider is not
    /// configured`) — the layer would change what the provider can do, which a composition never
    /// does (`compose.rs`). So: a model the base answers streams through the base as is, and this
    /// provider answers `provider_auth() == None`, the same encapsulation its base declares. A
    /// model the base does not answer still resolves this provider's own auth.
    #[must_use]
    pub fn with_auth_from_base(mut self) -> Self {
        self.auth_from_base = true;
        self
    }

    /// What this provider publishes as its own `update` after the base it was composed over
    /// ([`Self::with_operations_from`]) refreshed — pi's composed `refreshModels` runs
    /// `await base?.refreshModels?.(context)` and then `context.publish({ update })`
    /// (`provider-composer.ts:613-635` @f1b2e77f5), after which its `getAllModels` reads the
    /// refreshed base. A cyrup provider's rows are an immutable borrow, so the update that makes a
    /// refreshed base show is a re-composition over it, which only the composition's owner can make:
    /// it hands that in here. Without one, a refresh still runs the base's. PROV-164.
    #[must_use]
    pub fn with_refresh_update(mut self, update: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.refresh_update = Some(update);
        self
    }

    /// Declare which of pi's api lookups this provider's registry stands for (PROV-157). A
    /// construction site that is pi's composed provider, or a `compat`-streaming one, declares
    /// [`ApiDispatch::Global`]; leaving the default [`ApiDispatch::Own`] there reports
    /// `createProvider`'s text where pi reports the registry's.
    #[must_use]
    pub fn with_api_dispatch(mut self, api_dispatch: ApiDispatch) -> Self {
        self.api_dispatch = api_dispatch;
        self
    }

    /// Override the ambient auth context (for tests / custom env sources).
    #[must_use]
    pub fn with_auth_context(mut self, ctx: Arc<dyn AuthContext>) -> Self {
        self.auth_ctx = ctx;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait::async_trait]
impl Provider for WireProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    fn headers(&self) -> Option<&crate::HeaderMap> {
        self.headers.as_ref()
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    /// Pi `getAllModels: currentModels` (`models.ts:1085`): the ONE catalog, every type. Chat rows
    /// first, then image rows, then classifier rows, in the order `createProvider({ models })` was
    /// given them (`providers/openrouter.ts:23-27`). PROV-128, PROV-104.
    fn get_all_models(&self) -> Vec<AnyModel> {
        let mut all: Vec<AnyModel> = Vec::with_capacity(
            self.models.len() + self.image_models.len() + self.classifier_models.len(),
        );
        all.extend(self.models.iter().cloned().map(AnyModel::Chat));
        all.extend(self.image_models.iter().cloned().map(AnyModel::Image));
        all.extend(
            self.classifier_models
                .iter()
                .cloned()
                .map(AnyModel::Classifier),
        );
        all
    }

    /// Pi dispatches `images[model.api]` and answers `imageErrorResult` when there is no entry
    /// (`models.ts:1146-1160`); [`ImageApiRegistry`] is that map and that message. With no `images`
    /// option at all the trait default runs, which is pi's absent member. PROV-128.
    async fn generate_images(
        &self,
        model: &ImageModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages {
        if let Some(base) = &self.operations_base {
            return base.generate_images(model, context, options).await;
        }
        match &self.images {
            Some(images) => images.generate_images(model, context, options).await,
            None => AssistantImages::errored(
                model,
                format!(
                    "Provider {} does not support image generation",
                    model.provider
                ),
                options.is_aborted(),
            ),
        }
    }

    /// Pi attaches `generateImages` exactly when `createProvider` was given a non-empty `images`
    /// map (`models.ts:1146`) — NOT when the catalog happens to hold an image row. So this answers
    /// for the map, not the catalog: a provider given image rows but no implementation reports
    /// "does not support image generation" before any credential is read, rather than resolving
    /// auth and then failing on dispatch. PROV-128.
    fn supports_image_generation(&self) -> bool {
        match &self.operations_base {
            Some(base) => base.supports_image_generation(),
            None => self.images.is_some(),
        }
    }

    /// Pi dispatches `classifiers[model.api]` and answers `classifierErrorResult` when there is no
    /// entry (`models.ts:1161-1171`); [`ClassifierApiRegistry`] is that map and that message. With
    /// no `classifiers` option at all, the absent member's message. A provider composed over a
    /// base ([`WireProvider::with_operations_from`]) calls through to it instead, as pi's
    /// `composeModelProvider` assigns `provider.classify = … base.classify`. PROV-104, CFG-111.
    async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult {
        if let Some(base) = &self.operations_base {
            return base.classify(model, context, options).await;
        }
        match &self.classifiers {
            Some(classifiers) => classifiers.classify(model, context, options).await,
            None => ClassifierResult::errored(
                model,
                format!(
                    "Provider {} does not support classification",
                    model.provider
                ),
                options.is_aborted(),
            ),
        }
    }

    /// Pi attaches `classify` exactly when `createProvider` was given a non-empty `classifiers`
    /// map, not when the catalog holds a classifier row — the twin of
    /// [`WireProvider::supports_image_generation`]. A composed provider answers with its base's
    /// answer (pi attaches the member only `if (base?.classify)`). PROV-104, CFG-111.
    fn supports_classification(&self) -> bool {
        match &self.operations_base {
            Some(base) => base.supports_classification(),
            None => self.classifiers.is_some(),
        }
    }

    fn filter_models(
        &self,
        models: &[Model],
        credential: Option<&crate::auth::Credential>,
    ) -> Vec<Model> {
        // A composed provider filters with its base's policy (provider-composer.ts:636-638).
        if let Some(base) = &self.operations_base {
            return base.filter_models(models, credential);
        }
        match self.filter_models {
            Some(f) => f(models, credential),
            None => models.to_vec(),
        }
    }

    /// Pi `provider.filterAllModels` (`models.ts:202`, applied at `:730` @f1b2e77f5): `None` is the
    /// member being absent. A composed provider answers with its base's (provider-composer.ts:
    /// 639-641). PROV-147.
    fn filter_all_models(
        &self,
        models: &[AnyModel],
        credential: Option<&crate::auth::Credential>,
    ) -> Option<Vec<AnyModel>> {
        if let Some(base) = &self.operations_base {
            return base.filter_all_models(models, credential);
        }
        self.filter_all_models.map(|f| f(models, credential))
    }

    fn provider_auth(&self) -> Option<&ProviderAuth> {
        (!self.auth_from_base).then_some(&self.auth)
    }

    /// pi's composed `refreshModels` (`provider-composer.ts:613-635` @f1b2e77f5): the base's own
    /// refresh with THIS refresh's context (so its publications are this id's and are gated as
    /// such), then — unless the refresh was aborted (`if (context.signal.aborted) return;`,
    /// `:619`) — this provider's own publication of [`Self::with_refresh_update`]'s update. `None`:
    /// no base, or a base that does not refresh. PROV-164.
    async fn refresh_models(
        &self,
        ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        let base = self.operations_base.as_ref()?;
        if let Err(error) = base.refresh_models(ctx).await? {
            return Some(Err(error));
        }
        if ctx.is_aborted() {
            return Some(Ok(()));
        }
        if let Some(update) = &self.refresh_update {
            let update = Arc::clone(update);
            let published = ctx
                .publish(ModelsPublication {
                    persist: None,
                    update: Some(Box::new(move || update())),
                })
                .await;
            if let Err(error) = published {
                return Some(Err(error));
            }
        }
        Some(Ok(()))
    }

    /// Forwarded with [`Self::refresh_models`]: refreshable exactly when the base is (pi
    /// `base?.refreshModels || …`, `provider-composer.ts:613`). PROV-164.
    fn has_refresh_models(&self) -> bool {
        self.operations_base
            .as_ref()
            .is_some_and(|base| base.has_refresh_models())
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let (sink, rx) = channel(STREAM_BUFFER);

        // Snapshot everything the task needs (the returned stream owns the work).
        let id = self.id.clone();
        let auth = self.auth.clone();
        let store = self.store.clone();
        let auth_ctx = self.auth_ctx.clone();
        let registry = self.registry.clone();
        let api_dispatch = self.api_dispatch;
        // pi's composer hands `base.stream` every model whose api the base's own chat catalog lists
        // (`supportsBaseApi`, `provider-composer.ts:582`, `:593-596` @f1b2e77f5), reading the base's
        // live catalog per request; only the rest fall through to the registry. PROV-162.
        let through_base = self.base_for(model);
        if self.auth_from_base
            && let Some(base) = &through_base
        {
            return base.stream(model, context, options);
        }
        let model = model.clone();
        let context = context.clone();
        let options = options.clone();
        let cancel = options.cancel.clone().unwrap_or_default();

        tokio::spawn(async move {
            let model_id = model.id.as_str().to_string();
            // pi's setup failures end in `lazyStream`'s `createSetupErrorMessage`, which carries
            // `provider: model.provider` and `api: model.api` (`api/lazy.ts:4-12` @f1b2e77f5).
            let setup_error = |error: ProviderError| {
                error.into_error_event(model.provider.clone(), &model_id, Some(model.api.clone()))
            };

            // 1. Resolve auth (failures → terminal Error, never thrown — R-01-018); see
            // `resolve_request_auth` for the order this fuses with dispatch.
            let mut auth_result = match resolve_request_auth(
                &id,
                &auth,
                &model,
                store.as_ref(),
                auth_ctx.as_ref(),
                &options,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    sink.send(setup_error(error)).await;
                    return;
                }
            };

            // A model the base answers streams THROUGH the base (`base.stream(model, context,
            // options)`, provider-composer.ts:593-596), carrying this provider's resolved auth the
            // way `Models.stream`'s `applyAuth` hands it to a provider (`models.ts:863-874`). The
            // base's own dispatch then reports its own miss (`createProvider`'s text, PROV-157).
            if let Some(base) = through_base {
                let (request_model, request_options) =
                    crate::collection::apply_resolution(&model, &options, &auth_result).await;
                let mut events = base.stream(&request_model, &context, &request_options);
                forward(&mut events, &sink).await;
                return;
            }

            // Merge the per-request env overlay into the resolved env so the request path
            // (cache-retention / base-url resolution) sees it, with `options.env` winning per key
            // (Pi `applyAuth`, `models.ts:481` @v0.83.0:
            // `{ ...(resolution.env ?? {}), ...(options.env ?? {}) }`).
            if let Some(req_env) = &options.env {
                let merged = auth_result.env.get_or_insert_with(Default::default);
                for (k, v) in req_env {
                    merged.insert(k.clone(), v.clone());
                }
            }

            // 2. Look up the ApiImpl for model.api (missing → terminal Error — R-01-008), in
            // whichever of pi's maps the registry stands for (PROV-157).
            let Some(api_impl) = registry.get(&model.api) else {
                sink.send(setup_error(api_dispatch.miss(&id, &model.api)))
                    .await;
                return;
            };

            // 3. Drive the wire protocol. `run` emits Start..deltas..terminal into the sink.
            api_impl
                .run(&model, &context, &auth_result, &options, cancel, sink)
                .await;
        });

        Box::pin(ReceiverStream::new(rx))
    }

    /// The trait's lowering for every model this provider dispatches itself; a model its base
    /// answers goes to the BASE's `stream_simple` — pi `streamWith(…, simple: true)` calls
    /// `base.streamSimple(model, context, options)` (`provider-composer.ts:594-595` @f1b2e77f5), so
    /// a base that overrides it (radius does) keeps its override. PROV-162.
    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> EventStream<StreamEvent> {
        let Some(base) = self.base_for(model) else {
            // `Provider::stream_simple`'s default, verbatim: lower, clamp, `self.stream`.
            let mut lowered =
                build_base_options(model, context, options, options.base.api_key.as_deref());
            if let Some(level) = options.reasoning {
                lowered.reasoning = clamp_thinking_level(model, level.into());
            }
            return self.stream(model, context, &lowered);
        };
        if self.auth_from_base {
            return base.stream_simple(model, context, options);
        }
        let (sink, rx) = channel(STREAM_BUFFER);
        let id = self.id.clone();
        let auth = self.auth.clone();
        let store = self.store.clone();
        let auth_ctx = self.auth_ctx.clone();
        let model = model.clone();
        let context = context.clone();
        let options = options.clone();
        tokio::spawn(async move {
            let auth_result = match resolve_request_auth(
                &id,
                &auth,
                &model,
                store.as_ref(),
                auth_ctx.as_ref(),
                &options.base,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    let model_id = model.id.as_str().to_string();
                    sink.send(error.into_error_event(
                        model.provider.clone(),
                        &model_id,
                        Some(model.api.clone()),
                    ))
                    .await;
                    return;
                }
            };
            let (request_model, request_base) =
                crate::collection::apply_resolution(&model, &options.base, &auth_result).await;
            let mut request_options = options;
            request_options.base = request_base;
            let mut events = base.stream_simple(&request_model, &context, &request_options);
            forward(&mut events, &sink).await;
        });
        Box::pin(ReceiverStream::new(rx))
    }
}

impl WireProvider {
    /// The base that answers `model`, if any: pi `base && supportsBaseApi(model)`, where
    /// `supportsBaseApi = (model) => base?.getModels().some((entry) => entry.api === model.api)`
    /// (`provider-composer.ts:582`, `:593` @f1b2e77f5) — the base's LIVE chat catalog, read per
    /// request. PROV-162.
    fn base_for(&self, model: &Model) -> Option<Arc<dyn Provider>> {
        self.operations_base
            .as_ref()
            .filter(|base| base.models().iter().any(|row| row.api == model.api))
            .cloned()
    }
}

/// Resolve a request's auth — the stream task's step 1 (pi `applyAuth`'s resolution,
/// `models.ts:850-861`). `Ok(None)` from the strategy is pi's `Provider is not configured:
/// ${model.provider}` (`:859-861`).
async fn resolve_request_auth(
    id: &ProviderId,
    auth: &ProviderAuth,
    model: &Model,
    store: &dyn CredentialStore,
    auth_ctx: &dyn AuthContext,
    options: &StreamOptions,
) -> Result<crate::auth::AuthResult, ProviderError> {
    // The provider-scoped env overlay (Pi `options.env`) participates in env-key resolution /
    // base-url (Pi `applyAuth`, `models.ts:482` @v0.83.0).
    //
    // [CYRUP-DELTA, mechanism] The stream task is pi's `Models.stream` (`applyAuth`,
    // `models.ts:843-875`) and `createProvider`'s `dispatch` (`:1072-1083`) in one, because the
    // agent streams through `Provider::stream` directly (`cyrup-agent` `stream_fn.rs`): so a
    // direct call resolves auth BEFORE the dispatch lookup, where pi's bare `provider.stream`
    // dispatches unauthenticated. Through `Models::stream` the order is pi's: `Models` applies auth
    // first either way.
    let overrides = AuthOverrides {
        api_key: options.api_key.as_deref(),
        env: options.env.as_ref(),
        min_oauth_validity_ms: None,
    };
    match resolve_provider_auth(id, auth, model, store, auth_ctx, overrides).await {
        Ok(Some(result)) => Ok(result),
        Ok(None) => Err(ProviderError::NotConfigured(model.provider.clone())),
        Err(e) => Err(ProviderError::from(e)),
    }
}

/// Forward every event of `events` into `sink` until either ends.
async fn forward(events: &mut EventStream<StreamEvent>, sink: &crate::api::EventSink) {
    use futures::StreamExt;
    while let Some(event) = events.next().await {
        if !sink.send(event).await {
            break;
        }
    }
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
    use crate::api::{ApiImpl, EventSink};
    use crate::auth::types::{AuthContext, AuthResult, Credential};
    use crate::auth::{InMemoryCredentialStore, env_key};
    use crate::model::{Modality, ModelCost};
    use crate::stream::collect_message;
    use crate::stream::sse::{SseRequest, open_sse};
    use cyrup_core::{
        AssistantMessage, CancelToken, Content, StopReason, ToolCall, ToolCallId, Usage,
    };
    use futures::StreamExt;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A neutral non-terminal `partial` snapshot for scripted test events.
    fn test_partial() -> Arc<AssistantMessage> {
        Arc::new(AssistantMessage {
            content: Vec::new(),
            provider: ProviderId::from("p"),
            model: "m1".into(),
            api: ApiId::from("scripted"),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            thinking_level: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
            duration_ms: None,
        })
    }

    fn test_model(api: &str) -> Model {
        Model {
            id: "m1".into(),
            name: "M1".into(),
            api: api.into(),
            provider: "p".into(),
            base_url: String::new(),
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
        }
    }

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

    fn store_with_key() -> Arc<dyn CredentialStore> {
        Arc::new(
            InMemoryCredentialStore::new()
                .with_credential(ProviderId::from("p"), Credential::api_key("sk-test")),
        )
    }

    fn provider_with(registry: ApiRegistry, model: &Model) -> WireProvider {
        WireProvider::new(
            "p",
            "Test Provider",
            vec![model.clone()],
            ProviderAuth::with_api_key(env_key("P API key", ["P_API_KEY"])),
            store_with_key(),
            Arc::new(registry),
        )
        .with_auth_context(Arc::new(MapCtx(BTreeMap::new())))
    }

    // ---- A fake in-process ApiImpl producing a scripted StreamEvent sequence ----

    struct ScriptedApi {
        api: ApiId,
        seen_key: Arc<Mutex<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl ApiImpl for ScriptedApi {
        fn api(&self) -> &ApiId {
            &self.api
        }
        async fn run(
            &self,
            _model: &Model,
            _ctx: &Context,
            auth: &AuthResult,
            _opts: &StreamOptions,
            _cancel: CancelToken,
            sink: EventSink,
        ) {
            if let Ok(mut g) = self.seen_key.lock() {
                *g = auth.auth.api_key.clone();
            }
            sink.send(StreamEvent::Start {
                partial: test_partial(),
            })
            .await;
            sink.send(StreamEvent::TextStart {
                content_index: 0,
                partial: test_partial(),
            })
            .await;
            sink.send(StreamEvent::TextDelta {
                content_index: 0,
                delta: "hi".into(),
                partial: test_partial(),
            })
            .await;
            sink.send(StreamEvent::TextEnd {
                content_index: 0,
                content: "hi".into(),
                partial: test_partial(),
            })
            .await;
            sink.send(StreamEvent::ToolCallStart {
                content_index: 1,
                partial: test_partial(),
            })
            .await;
            sink.send(StreamEvent::ToolCallDelta {
                content_index: 1,
                delta: "{\"x\":1}".into(),
                partial: test_partial(),
            })
            .await;
            let tc = ToolCall {
                id: ToolCallId::from("c1"),
                name: "echo".into(),
                arguments: serde_json::json!({"x": 1})
                    .as_object()
                    .cloned()
                    .expect("object")
                    .into(),
                thought_signature: None,
                namespace: None,
            };
            sink.send(StreamEvent::ToolCallEnd {
                content_index: 1,
                tool_call: tc,
                partial: test_partial(),
            })
            .await;
            let msg = AssistantMessage {
                content: vec![
                    Content::text("hi"),
                    Content::ToolCall(ToolCall {
                        id: ToolCallId::from("c1"),
                        name: "echo".into(),
                        arguments: serde_json::json!({"x": 1})
                            .as_object()
                            .cloned()
                            .expect("object")
                            .into(),
                        thought_signature: None,
                        namespace: None,
                    }),
                ],
                provider: ProviderId::from("p"),
                model: "m1".into(),
                api: ApiId::from("scripted"),
                response_model: None,
                response_id: None,
                provider_thinking_level: None,
                thinking_level: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 0,
                duration_ms: None,
            };
            sink.send(StreamEvent::terminal(msg)).await;
        }
    }

    #[tokio::test]
    async fn wire_provider_drives_fake_api_end_to_end() {
        let model = test_model("scripted");
        let seen_key = Arc::new(Mutex::new(None));
        let registry = ApiRegistry::new();
        registry.register_impl(Arc::new(ScriptedApi {
            api: ApiId::from("scripted"),
            seen_key: seen_key.clone(),
        }));
        let provider = provider_with(registry, &model);

        let mut stream = provider.stream(&model, &Context::default(), &StreamOptions::default());
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }

        assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
        assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::TextDelta {
                content_index: 0,
                ..
            }
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolCallEnd {
                content_index: 1,
                ..
            }
        )));
        // Stored credential flowed through to the ApiImpl (auth resolution wired end-to-end).
        assert_eq!(seen_key.lock().unwrap().as_deref(), Some("sk-test"));
    }

    /// PROV-157 — pi `createProvider`'s dispatch miss (`models.ts:1072-1083` @f1b2e77f5):
    /// ``throw new ModelsError("stream", `Provider ${input.id} has no API implementation for
    /// "${model.api}"`)`` inside `lazyStream`, whose setup-error message carries `provider:
    /// model.provider` and `api: model.api` (`api/lazy.ts:4-12`). The model names ANOTHER provider
    /// on purpose: the text takes the provider's own id, the message the model's.
    #[tokio::test]
    async fn missing_api_impl_yields_error_terminal() {
        let mut model = test_model("no-such-api");
        model.provider = "elsewhere".into();
        let registry = ApiRegistry::new(); // nothing registered
        let provider = provider_with(registry, &model);
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(
            msg.error_message.as_deref(),
            Some(r#"Provider p has no API implementation for "no-such-api""#)
        );
        assert_eq!(msg.provider.as_str(), "elsewhere");
        assert_eq!(msg.api.as_str(), "no-such-api");
    }

    /// PROV-157 — a provider whose registry stands for pi's process-wide one reports the
    /// registry's text, which names no provider (`compat.ts:244-250`, `provider-composer.ts:
    /// 598-599` @f1b2e77f5); the default is `createProvider`'s own map.
    ///
    /// `ApiDispatch` is new, so this cannot be red at the base; it is mutation-proved (swap
    /// [`ApiDispatch::miss`]'s arms and it fails, with `missing_api_impl_yields_error_terminal`).
    #[tokio::test]
    async fn a_global_dispatch_miss_is_the_registry_text() {
        assert_eq!(ApiDispatch::default(), ApiDispatch::Own);
        let model = test_model("no-such-api");
        let provider =
            provider_with(ApiRegistry::new(), &model).with_api_dispatch(ApiDispatch::Global);
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(
            msg.error_message.as_deref(),
            Some("No API provider registered for api: no-such-api")
        );
    }

    /// pi `providers.test.ts:695-705` @f1b2e77f5, "produces a stream error for a model whose api
    /// has no implementation": a provider with `api: { "api-a": … }` handed a model on
    /// `api-ghost`. pi asserts `toContain("no API implementation")`; this asserts the whole text.
    /// The credential is there only because a `WireProvider` resolves auth before it dispatches
    /// (the `[CYRUP-DELTA, mechanism]` in [`WireProvider::stream`]); pi's bare
    /// `provider.streamSimple` dispatches unauthenticated.
    #[tokio::test]
    async fn a_model_whose_api_has_no_implementation_errors_the_stream() {
        let registry = ApiRegistry::new();
        registry.register_impl(Arc::new(ScriptedApi {
            api: ApiId::from("api-a"),
            seen_key: Arc::new(Mutex::new(None)),
        }));
        let mut known = test_model("api-a");
        known.id = "model-a".into();
        known.provider = "mixed".into();
        let provider = WireProvider::new(
            "mixed",
            "Mixed",
            vec![known],
            ProviderAuth::with_api_key(env_key("Test", Vec::<String>::new())),
            Arc::new(
                InMemoryCredentialStore::new()
                    .with_credential(ProviderId::from("mixed"), Credential::api_key("k")),
            ),
            Arc::new(registry),
        )
        .with_auth_context(Arc::new(MapCtx(BTreeMap::new())));
        let mut ghost = test_model("api-ghost");
        ghost.id = "model-x".into();
        ghost.provider = "mixed".into();
        let msg = collect_message(provider.stream_simple(
            &ghost,
            &Context::default(),
            &crate::utils::simple_options::SimpleStreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(
            msg.error_message.as_deref(),
            Some(r#"Provider mixed has no API implementation for "api-ghost""#)
        );
    }

    #[tokio::test]
    async fn unconfigured_provider_yields_error_terminal() {
        let model = test_model("scripted");
        let registry = ApiRegistry::new();
        registry.register_impl(Arc::new(ScriptedApi {
            api: ApiId::from("scripted"),
            seen_key: Arc::new(Mutex::new(None)),
        }));
        // No stored credential, empty env → unconfigured.
        let provider = WireProvider::new(
            "p",
            "P",
            vec![model.clone()],
            ProviderAuth::with_api_key(env_key("P API key", ["P_API_KEY"])),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(registry),
        )
        .with_auth_context(Arc::new(MapCtx(BTreeMap::new())));
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        // pi `applyAuth` (`models.ts:859-861` @f1b2e77f5), the half of pi's `Models.stream` a
        // `WireProvider` stands in for: ``Provider is not configured: ${model.provider}``.
        assert_eq!(
            msg.error_message.as_deref(),
            Some("Provider is not configured: p")
        );
        // The text names `model.provider`, and the message carries it (`api/lazy.ts:4-12`), even
        // when a direct call hands the provider another provider's model.
        let mut stray = model.clone();
        stray.provider = "elsewhere".into();
        let msg = collect_message(provider.stream(
            &stray,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(
            msg.error_message.as_deref(),
            Some("Provider is not configured: elsewhere")
        );
        assert_eq!(msg.provider.as_str(), "elsewhere");
    }

    // ---- A TEST ApiImpl that drives the real SSE transport against a local mock server ----

    struct SseEchoApi {
        api: ApiId,
    }

    #[async_trait::async_trait]
    impl ApiImpl for SseEchoApi {
        fn api(&self) -> &ApiId {
            &self.api
        }
        async fn run(
            &self,
            model: &Model,
            _ctx: &Context,
            _auth: &AuthResult,
            _opts: &StreamOptions,
            cancel: CancelToken,
            sink: EventSink,
        ) {
            let provider = model.provider.clone();
            let model_id = model.id.as_str().to_string();
            let url = model.base_url.clone();

            let client = match crate::stream::sse::build_client() {
                Ok(c) => c,
                Err(e) => {
                    sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                        .await;
                    return;
                }
            };
            let req = SseRequest::post_json(url, serde_json::json!({"stream": true}));
            let mut frames = match open_sse(
                &client,
                req,
                cancel,
                None,
                None,
                crate::utils::provider_retry::ProviderRetry::NONE,
            )
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    // transport / non-2xx / abort-during-connect → terminal Error (R-01-018/045)
                    sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                        .await;
                    return;
                }
            };

            sink.send(StreamEvent::Start {
                partial: test_partial(),
            })
            .await;
            let mut idx = 0usize;
            let mut text = String::new();
            while let Some(frame) = frames.next().await {
                match frame {
                    Ok(f) if f.data == "[DONE]" => break,
                    Ok(f) => {
                        sink.send(StreamEvent::TextStart {
                            content_index: idx,
                            partial: test_partial(),
                        })
                        .await;
                        sink.send(StreamEvent::TextDelta {
                            content_index: idx,
                            delta: f.data.clone(),
                            partial: test_partial(),
                        })
                        .await;
                        sink.send(StreamEvent::TextEnd {
                            content_index: idx,
                            content: f.data.clone(),
                            partial: test_partial(),
                        })
                        .await;
                        text.push_str(&f.data);
                        idx += 1;
                    }
                    Err(e) => {
                        sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                            .await;
                        return;
                    }
                }
            }
            let msg = AssistantMessage {
                content: vec![Content::text(text)],
                provider,
                model: model_id,
                api: self.api.clone(),
                response_model: None,
                response_id: None,
                provider_thinking_level: None,
                thinking_level: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 0,
                duration_ms: None,
            };
            sink.send(StreamEvent::terminal(msg)).await;
        }
    }

    /// Spawn a one-shot mock HTTP server that writes `raw_response` then closes. Returns its URL.
    async fn spawn_mock(raw_response: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                // Drain the request (best-effort) then write the canned response.
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(raw_response).await;
                let _ = sock.flush().await;
            }
        });
        format!("http://{addr}/v1/stream")
    }

    fn sse_provider(base_url: String) -> (WireProvider, Model) {
        let mut model = test_model("sse-echo");
        model.base_url = base_url;
        let registry = ApiRegistry::new();
        registry.register_impl(Arc::new(SseEchoApi {
            api: ApiId::from("sse-echo"),
        }));
        (provider_with(registry, &model), model)
    }

    #[tokio::test]
    async fn sse_frames_decode_to_stream_events_via_test_api() {
        let body = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: alpha\n\ndata: beta\n\ndata: [DONE]\n\n";
        let url = spawn_mock(body).await;
        let (provider, model) = sse_provider(url);
        let mut stream = provider.stream(&model, &Context::default(), &StreamOptions::default());
        let mut deltas = Vec::new();
        let mut terminal = None;
        while let Some(ev) = stream.next().await {
            match ev {
                StreamEvent::TextDelta { delta, .. } => deltas.push(delta),
                StreamEvent::Done { message, .. } => terminal = Some(message),
                _ => {}
            }
        }
        assert_eq!(deltas, vec!["alpha".to_string(), "beta".to_string()]);
        let msg = terminal.expect("done terminal");
        assert_eq!(msg.stop_reason, StopReason::Stop);
        assert_eq!(msg.content, vec![Content::text("alphabeta")]);
    }

    #[tokio::test]
    async fn http_non_2xx_yields_error_terminal() {
        let body = b"HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: 9\r\n\r\noh no!!!!";
        let url = spawn_mock(body).await;
        let (provider, model) = sse_provider(url);
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("http 500"));
    }

    #[tokio::test]
    async fn transport_error_yields_error_terminal() {
        // Bind then immediately drop the listener to get a refused-connection address.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let (provider, model) = sse_provider(format!("http://{addr}/v1/stream"));
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(
            msg.error_message
                .unwrap()
                .chars()
                .take(9)
                .collect::<String>(),
            "transport".to_string()
        );
    }

    #[tokio::test]
    async fn cancellation_yields_aborted_terminal() {
        // A server that accepts but never sends a response body → the stream blocks until cancel.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((sock, _)) = listener.accept().await {
                // Hold the connection open without responding.
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                drop(sock);
            }
        });

        let cancel = CancelToken::new();
        let mut model = test_model("sse-echo");
        model.base_url = format!("http://{addr}/v1/stream");
        let registry = ApiRegistry::new();
        registry.register_impl(Arc::new(SseEchoApi {
            api: ApiId::from("sse-echo"),
        }));
        let provider = provider_with(registry, &model);

        let opts = StreamOptions {
            cancel: Some(cancel.clone()),
            ..Default::default()
        };
        let stream = provider.stream(&model, &Context::default(), &opts);

        // Cancel shortly after starting.
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cancel.cancel();
        });

        let msg = collect_message(stream).await;
        canceller.await.unwrap();
        assert_eq!(msg.stop_reason, StopReason::Aborted);
        assert_eq!(msg.error_message.as_deref(), Some("aborted"));
    }
}

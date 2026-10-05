//! The image-generation operation on the ONE provider surface (pi `packages/ai/src/types.ts` +
//! `models.ts`'s `createProvider({ images })` @v1.0.1).
//!
//! # What this module is, and what it replaced
//!
//! PROV-128. Until v1.0.0 pi carried a second, parallel registry for image models —
//! `images-models.ts` (`ImagesProvider`, `ImagesModels`, `createImagesProvider`,
//! `createImagesModels`), `image-models.generated.ts`, `providers/openrouter-images.ts` and
//! `builtinImagesProviders`/`builtinImagesModels` — and cyrup ported it 1:1 as `crate::images`'s
//! `ImagesProvider`/`ImagesModels` tree. `a328aa89a` ("unify image and classifier models") DELETED
//! that whole layer: `git cat-file -e v1.0.1:packages/ai/src/images-models.ts`,
//! `…/image-models.generated.ts` and `…/providers/openrouter-images.ts` all fail. An image model is
//! now an ordinary catalog entry of an ordinary provider, distinguished only by
//! `type: "image"` ([`crate::ImageModel`], pi `ImageModel`, `types.ts:1145-1149`), and image
//! generation is an operation on that provider: `images: { "openrouter-images": openrouterImagesApi() }`
//! (`providers/openrouter.ts:33`), dispatched on `model.api` (`models.ts:1146-1160`).
//!
//! So what survives here is exactly what survives upstream — checked file by file at the pin, not
//! assumed:
//!
//! - [`ImagesContext`] / [`AssistantImages`] / [`ImagesStopReason`] — the request and response
//!   (pi `types.ts`, unchanged by the unification).
//! - [`ImagesOptions`] — per-request options.
//! - [`ProviderImages`] — the contract an image api implements (pi `ProviderImages`,
//!   `types.ts:307-313`).
//! - [`ImageApiRegistry`] — pi's per-provider `images` map (`CreateProviderOptions.images`,
//!   `models.ts:1021`), dispatched at `:1146-1160`.
//! - [`ImagesApiImpl`] / [`ImagesApiProviderRegistry`] / [`images_builtin_registry`] /
//!   [`register_images_builtins`] / [`generate_images`] — the **global, `api`-keyed images
//!   registry**, which `a328aa89a` did NOT delete. `git cat-file -e v1.0.1:packages/ai/src/images.ts`
//!   and `…/images-api-registry.ts` both succeed, `providers/images/register-builtins.ts` still
//!   calls `registerBuiltInImagesApiProviders()` at module scope, and all three are re-exported from
//!   pi's public `compat.ts` (`:25-29`). It sits BESIDE a provider's `images` map, not instead of
//!   it, and its own doc comment says which to prefer: "Auth must be passed explicitly via
//!   `options.apiKey`; prefer `Models.generateImages()`, which resolves provider auth"
//!   (`images.ts:14-18`).
//!
//! The model shape itself lives beside its siblings in [`crate::classifier`]
//! ([`crate::ImageModel`]), because pi declares all three in one file over one `BaseModel`. The
//! `openrouter-images` wire protocol lives where pi keeps it, under `api`
//! ([`crate::api::openrouter_images`], pi `api/openrouter-images.ts`). Dispatch lives where pi
//! keeps it: [`crate::provider::Provider::generate_images`] and
//! [`crate::collection::Models::generate_images`].

use crate::HeaderMap;
use crate::auth::ProviderEnv;
use crate::classifier::ImageModel;
use crate::stream::{ProviderResponse, TransformHeadersFn};
use cyrup_core::{ApiId, CancelToken, Content, ProviderId, Usage};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The known image wire-protocol ids this build implements (pi `KnownImageApi`,
/// `types.ts:31-33` — renamed from v0.87.1's `KnownImagesApi`).
///
/// Like pi's `ImageApi = KnownImageApi | (string & {})`, an [`ImageModel::api`] may carry any
/// string; this enum names the ones with an implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnownImageApi {
    /// OpenRouter's image generation over the OpenAI chat-completions shape
    /// (`api/openrouter-images.ts`).
    OpenrouterImages,
}

impl KnownImageApi {
    pub const ALL: [KnownImageApi; 1] = [KnownImageApi::OpenrouterImages];

    /// The api id as it appears in [`ImageModel::api`].
    pub const fn as_str(self) -> &'static str {
        match self {
            KnownImageApi::OpenrouterImages => OPENROUTER_IMAGES,
        }
    }

    /// The known api an id names, if any.
    pub fn from_api(api: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|known| known.as_str() == api)
    }
}

impl std::fmt::Display for KnownImageApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<KnownImageApi> for ApiId {
    fn from(api: KnownImageApi) -> Self {
        ApiId::from(api.as_str())
    }
}

/// The `openrouter-images` api id (pi `KnownImageApi`, `types.ts:31`).
pub const OPENROUTER_IMAGES: &str = "openrouter-images";

/// The terminal reason of an image generation (pi `ImagesStopReason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImagesStopReason {
    Stop,
    Error,
    Aborted,
}

/// Input to a single image-generation call (pi `ImagesContext`). `input` carries text + image
/// content (pi `ImagesInputContent = TextContent | ImageContent`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesContext {
    pub input: Vec<Content>,
}

/// The result of an image generation (pi `AssistantImages`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantImages {
    pub api: ApiId,
    pub provider: ProviderId,
    pub model: String,
    /// Text + image output (pi `ImagesOutputContent[]`).
    pub output: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage: Option<Usage>,
    pub stop_reason: ImagesStopReason,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error_message: Option<String>,
    /// Unix timestamp in milliseconds (pi `timestamp`).
    pub timestamp: u64,
}

impl AssistantImages {
    /// A fresh successful-by-default envelope: the `output` an api fills in
    /// (`api/openrouter-images.ts:43-50`).
    pub fn new(model: &ImageModel) -> Self {
        AssistantImages {
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            output: Vec::new(),
            response_id: None,
            usage: None,
            stop_reason: ImagesStopReason::Stop,
            error_message: None,
            timestamp: now_ms(),
        }
    }

    /// The terminal error envelope (pi `imageErrorResult`, `utils/model-operations.ts:44-54`):
    /// no output, `error` or `aborted`, and the message. Every failure of image generation is
    /// delivered this way; it never rejects.
    pub fn errored(model: &ImageModel, message: impl Into<String>, aborted: bool) -> Self {
        AssistantImages {
            stop_reason: if aborted {
                ImagesStopReason::Aborted
            } else {
                ImagesStopReason::Error
            },
            error_message: Some(message.into()),
            ..Self::new(model)
        }
    }
}

/// Inspect or replace an image payload before sending (pi `ImagesOptions.onPayload`).
pub type ImagesOnPayload =
    Arc<dyn Fn(&serde_json::Value, &ImageModel) -> Option<serde_json::Value> + Send + Sync>;

/// Invoked after an HTTP response is received (pi `ImagesOptions.onResponse`).
pub type ImagesOnResponse = Arc<dyn Fn(&ProviderResponse, &ImageModel) + Send + Sync>;

/// Per-request options for image generation (pi `ImagesOptions` over `ProviderRequestOptions`).
#[derive(Clone, Default)]
pub struct ImagesOptions {
    /// Cancellation token (pi `signal?: AbortSignal`).
    pub cancel: Option<CancelToken>,
    pub api_key: Option<String>,
    /// Provider-scoped env overlay (pi `env`).
    pub env: Option<ProviderEnv>,
    /// Per-request header overlay; a `None` value suppresses a default header (pi `headers`).
    pub headers: Option<HeaderMap>,
    /// HTTP request timeout in milliseconds (pi `timeoutMs`).
    pub timeout_ms: Option<u64>,
    /// Max client-side retry attempts (pi `maxRetries`).
    pub max_retries: Option<u32>,
    /// Cap on a server-requested retry delay (pi `maxRetryDelayMs`).
    pub max_retry_delay_ms: Option<u64>,
    /// Provider-extracted request metadata (pi `metadata`).
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    /// Transform the fully assembled headers before provider dispatch (pi
    /// `ModelsRequestTransforms.transformHeaders`, `models.ts:102-105`). Applied and then STRIPPED
    /// by [`crate::collection::Models::generate_images`], exactly as it is for `classify`, so an
    /// api never sees it.
    pub transform_headers: Option<TransformHeadersFn>,
    /// Inspect/replace the payload before sending (pi `onPayload`).
    pub on_payload: Option<ImagesOnPayload>,
    /// Invoked after a response is received (pi `onResponse`).
    pub on_response: Option<ImagesOnResponse>,
}

impl ImagesOptions {
    /// Whether the caller has already aborted (pi `options?.signal?.aborted`), which decides
    /// whether a terminal envelope is `aborted` rather than `error`.
    pub fn is_aborted(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
    }
}

/// The uniform contract implemented by image api modules (pi `ProviderImages`, `types.ts:307-313`).
///
/// **Never returns an error**: every failure, including an unsupported `model.api`, bad options,
/// transport errors and cancellation, is encoded in the returned [`AssistantImages`]
/// (`stop_reason` `error` / `aborted` plus `error_message`, `output` empty), exactly as pi's
/// `generateImages` resolves rather than rejects (`api/openrouter-images.ts:100-104`).
#[async_trait::async_trait]
pub trait ProviderImages: Send + Sync {
    async fn generate_images(
        &self,
        model: &ImageModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages;
}

/// Image-generation implementations keyed by `model.api` (pi `CreateProviderOptions.images`,
/// `models.ts:1021`, dispatched at `:1146-1160`), optionally layered over a fallback.
///
/// It is itself a [`ProviderImages`], so a provider's `generate_images` is one delegating call. A
/// model whose api has no entry and no fallback answers with an error envelope naming the provider
/// and the api (`Provider ${id} has no image generation implementation for "${model.api}"`,
/// `models.ts:1154`).
///
/// The twin of [`crate::classifier::ClassifierApiRegistry`], deliberately: upstream builds the two
/// maps the same way in the same function, and the one difference that mattered in the old tree —
/// a LAZY `fn()` factory per api, so an unused wire protocol was never constructed — is kept by
/// registering an `Arc` that the provider factory itself builds once per provider, not per request.
#[derive(Clone, Default)]
pub struct ImageApiRegistry {
    apis: BTreeMap<String, Arc<dyn ProviderImages>>,
    fallback: Option<Arc<dyn ProviderImages>>,
}

impl ImageApiRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) the implementation of `api`. A [`KnownImageApi`], a `&str` and a
    /// `String` all convert.
    pub fn register(
        &mut self,
        api: impl Into<ApiId>,
        images: Arc<dyn ProviderImages>,
    ) -> &mut Self {
        self.apis.insert(api.into().as_str().to_string(), images);
        self
    }

    /// Consulted when no registered api matches.
    #[must_use]
    pub fn with_fallback(mut self, fallback: Arc<dyn ProviderImages>) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// The implementation registered for exactly `api`.
    pub fn get(&self, api: &str) -> Option<Arc<dyn ProviderImages>> {
        self.apis.get(api).cloned()
    }

    pub fn contains(&self, api: &str) -> bool {
        self.apis.contains_key(api)
    }

    /// The registered api ids, sorted.
    pub fn apis(&self) -> Vec<&str> {
        self.apis.keys().map(String::as_str).collect()
    }

    /// `true` when nothing is registered and there is no fallback (pi's `createProvider` rejects
    /// an empty `images` map, `models.ts:1045-1047`).
    pub fn is_empty(&self) -> bool {
        self.apis.is_empty() && self.fallback.is_none()
    }
}

#[async_trait::async_trait]
impl ProviderImages for ImageApiRegistry {
    async fn generate_images(
        &self,
        model: &ImageModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages {
        if let Some(implementation) = self.apis.get(model.api.as_str()) {
            return implementation
                .generate_images(model, context, options)
                .await;
        }
        if let Some(fallback) = &self.fallback {
            return fallback.generate_images(model, context, options).await;
        }
        AssistantImages::errored(
            model,
            format!(
                "Provider {} has no image generation implementation for \"{}\"",
                model.provider, model.api
            ),
            false,
        )
    }
}

// ----------------------------------------------------- the global images api registry (pi) ------

/// One image wire protocol as the GLOBAL registry holds it (pi `ImagesApiProvider`,
/// `images-api-registry.ts:9-12` @v1.0.1). Builds the payload, performs the request, and returns
/// the assembled [`AssistantImages`]; like [`ProviderImages`] it never returns `Err` — every
/// failure is encoded into the envelope with `stop_reason ∈ {error, aborted}`
/// (`api/openrouter-images.ts:100-104`).
///
/// **This is not the parallel tree PROV-128 retired.** `a328aa89a` deleted `images-models.ts`
/// (`ImagesProvider` / `ImagesModels` / `createImagesProvider` / `createImagesModels`),
/// `image-models.generated.ts` and `providers/openrouter-images.ts` — but `images.ts`,
/// `images-api-registry.ts` and `providers/images/register-builtins.ts` are all present at v1.0.1
/// and re-exported from `compat.ts`. Retiring cyrup's port of them would have been a lost
/// guarantee, not a cleanup.
#[async_trait::async_trait]
pub trait ImagesApiImpl: Send + Sync {
    /// pi `ImagesApiProvider.api` (`images-api-registry.ts:10`) — the protocol this impl speaks.
    /// Upstream uses it to reject a mismatched model (`wrapGenerateImages`, `:26-35`); here the
    /// registry is keyed by the same id and [`generate_images`] looks up `model.api`, so a mismatch
    /// cannot be constructed through the registry. A caller holding an impl directly can still
    /// compare it — which is what this accessor is for.
    fn api(&self) -> &ApiId;

    async fn generate_images(
        &self,
        model: &ImageModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages;
}

/// Lazily-constructed factory for an [`ImagesApiImpl`] (mirrors [`crate::api::ApiFactory`]).
///
/// This is the port of upstream's module-load memo: `register-builtins.ts` registers a
/// `generateImages` that `await`s `import("../../api/openrouter-images.ts")` behind an
/// `||=`-memoised promise, so the module is loaded at most once and only on first use. Rust has no
/// dynamic import, so the deferral point is construction of the impl value — the same substitution
/// [`crate::api`]'s header note signs off for the ten streaming apis.
pub type ImagesApiFactory = fn() -> Arc<dyn ImagesApiImpl>;

/// Maps `ApiId → Arc<dyn ImagesApiImpl>` with lazy get-or-init (pi `imagesApiProviderRegistry`,
/// `images-api-registry.ts:24`, written by `registerImagesApiProvider` `:37-48` and read by
/// `getImagesApiProvider` `:50-52`).
///
/// Named for upstream's own identifier rather than contracted to `ImagesApiRegistry`, which is one
/// letter from the per-provider [`ImageApiRegistry`] and was read as the same thing more than once.
/// The two are different registries in upstream too: this one is global and keyed by api id; that
/// one is a provider's `images` option.
#[derive(Default)]
pub struct ImagesApiProviderRegistry {
    factories: std::collections::HashMap<ApiId, ImagesApiFactory>,
    live: std::collections::HashMap<ApiId, Arc<dyn ImagesApiImpl>>,
}

impl ImagesApiProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a lazy factory (pi `registerImagesApiProvider`, `images-api-registry.ts:37`).
    pub fn register(&mut self, api: ApiId, factory: ImagesApiFactory) {
        self.factories.insert(api, factory);
    }

    /// Get-or-init the impl for `api` (pi `getImagesApiProvider`, `images-api-registry.ts:50`).
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

/// A registry pre-seeded with every built-in image wire-protocol factory (pi
/// `registerBuiltInImagesApiProviders()`, `providers/images/register-builtins.ts`, which upstream
/// calls at module scope).
pub fn images_builtin_registry() -> ImagesApiProviderRegistry {
    let mut reg = ImagesApiProviderRegistry::new();
    register_images_builtins(&mut reg);
    reg
}

/// Register the built-in image wire-protocol factories into `reg`.
pub fn register_images_builtins(reg: &mut ImagesApiProviderRegistry) {
    reg.register(
        ApiId::from(OPENROUTER_IMAGES),
        crate::api::openrouter_images::factory,
    );
}

/// Registry-routed entrypoint (pi `generateImages`, `images.ts:19-25`). Resolves the api impl for
/// `model.api` and delegates; `Err(NoApiImpl)` is pi's
/// `throw new Error("No API provider registered for api: …")` (`:9`, inside
/// `resolveImagesApiProvider` `:6-12`).
///
/// Auth is the caller's problem here, exactly as upstream documents: prefer
/// [`crate::collection::Models::generate_images`], which resolves provider auth first.
///
/// # Errors
///
/// [`crate::error::ProviderError::NoApiImpl`] when no impl is registered for `model.api`.
pub async fn generate_images(
    registry: &mut ImagesApiProviderRegistry,
    model: &ImageModel,
    context: &ImagesContext,
    options: &ImagesOptions,
) -> Result<AssistantImages, crate::error::ProviderError> {
    let Some(provider) = registry.get(&model.api) else {
        return Err(crate::error::ProviderError::NoApiImpl(model.api.clone()));
    };
    Ok(provider.generate_images(model, context, options).await)
}

/// Unix-epoch milliseconds (pi `Date.now()`); never panics (a pre-epoch clock yields 0).
pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    use crate::providers::openrouter::{openrouter_image_model, openrouter_images_registry};

    fn sample_model() -> ImageModel {
        openrouter_image_model("google/gemini-2.5-flash-image").expect("nano banana")
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

    #[test]
    fn known_image_api_names_openrouter_images() {
        assert_eq!(KnownImageApi::OpenrouterImages.as_str(), OPENROUTER_IMAGES);
        assert_eq!(
            KnownImageApi::from_api("openrouter-images"),
            Some(KnownImageApi::OpenrouterImages)
        );
        assert_eq!(KnownImageApi::from_api("nope"), None);
    }

    /// pi `models.ts:1146-1160`: the `images` map dispatches on `model.api`, and a model whose api
    /// has no entry comes back as an error envelope rather than a rejection.
    #[tokio::test]
    async fn registry_dispatches_on_api_and_errors_for_an_unknown_one() {
        let registry = openrouter_images_registry();
        assert_eq!(registry.apis(), vec![OPENROUTER_IMAGES]);
        assert!(!registry.is_empty());

        let mut unknown = sample_model();
        unknown.api = "no-such-image-api".into();
        let out = registry
            .generate_images(
                &unknown,
                &ImagesContext::default(),
                &ImagesOptions::default(),
            )
            .await;
        assert_eq!(out.stop_reason, ImagesStopReason::Error);
        assert_eq!(
            out.error_message.as_deref(),
            Some(
                "Provider openrouter has no image generation implementation for \"no-such-image-api\""
            )
        );
    }

    #[test]
    fn an_empty_registry_is_empty() {
        assert!(ImageApiRegistry::new().is_empty());
    }

    /// The GLOBAL registry (pi `images-api-registry.ts`, alive at v1.0.1) resolves
    /// `openrouter-images` lazily and nothing else.
    #[test]
    fn the_global_registry_lazily_resolves_openrouter_images() {
        let mut reg = images_builtin_registry();
        assert!(reg.contains(&ApiId::from(OPENROUTER_IMAGES)));
        let imp = reg
            .get(&ApiId::from(OPENROUTER_IMAGES))
            .expect("openrouter-images is a built-in image api");
        assert_eq!(imp.api().as_str(), OPENROUTER_IMAGES);
        assert!(reg.get(&ApiId::from("nope")).is_none());
        assert!(!reg.contains(&ApiId::from("nope")));
    }

    /// The registry-routed entrypoint (pi `images.ts:19-25`): an unregistered api is pi's throw,
    /// which is `Err(NoApiImpl)` here rather than an error envelope — the one place in the image
    /// surface that reports a missing api as a call failure, because upstream's does too.
    #[tokio::test]
    async fn the_global_entrypoint_errors_for_an_unregistered_api() {
        let mut reg = ImagesApiProviderRegistry::new();
        let model = sample_model();
        let err = generate_images(
            &mut reg,
            &model,
            &ImagesContext::default(),
            &ImagesOptions::default(),
        )
        .await
        .expect_err("nothing is registered");
        assert!(
            matches!(&err, crate::error::ProviderError::NoApiImpl(api) if api.as_str() == OPENROUTER_IMAGES),
            "expected NoApiImpl, got {err:?}"
        );

        // With the built-ins registered it resolves and reaches the impl (no api key configured, so
        // the envelope is the api's own "No API key" error rather than a dispatch failure).
        let mut reg = images_builtin_registry();
        let out = generate_images(
            &mut reg,
            &model,
            &ImagesContext::default(),
            &ImagesOptions::default(),
        )
        .await
        .expect("the built-in api is registered");
        assert_eq!(out.stop_reason, ImagesStopReason::Error);
        assert!(
            out.error_message
                .as_deref()
                .is_some_and(|m| m.contains("No API key")),
            "expected the impl to have run, got {:?}",
            out.error_message
        );
    }
}

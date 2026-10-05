//! The `Provider` abstraction (arch-01 §6 / func-01 §6).

use crate::auth::{Credential, ProviderAuth};
use crate::classifier::{
    AnyModel, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, ImageModel,
};
use crate::collection::clamp_thinking_level;
use crate::context::Context;
use crate::error::ProviderError;
use crate::images::{AssistantImages, ImagesContext, ImagesOptions};
use crate::model::Model;
use crate::stream::{StreamEvent, StreamOptions};
use crate::utils::simple_options::{SimpleStreamOptions, build_base_options};
use cyrup_core::{CancelToken, EventStream, ProviderId};

/// What a provider asks the store to do with its persisted catalog — pi's
/// `ModelsPublication.persist?: ModelsStoreEntry | null` (`packages/ai/src/models.ts:67-72`
/// @v0.99.2-17), minus the "omit" arm, which is an absent [`ModelsPublication::persist`].
#[derive(Clone, Debug)]
pub enum ModelsPersist {
    /// `persist: entry` — replace the provider's stored entry (`modelsStore.write`, `models.ts:512`).
    ///
    /// `classifiers` is the `type: "classifier"` half of pi's ONE `models` array
    /// (`ModelsStoreEntry.models: readonly AnyModel[]`, `models-store.ts:3-5`); cyrup's entry holds
    /// chat models only and the classifier models travel through
    /// [`crate::ModelsStore::write_classifier_models`] (see that method for why).
    Write {
        entry: crate::ModelsStoreEntry,
        classifiers: Vec<ClassifierModel>,
    },
    /// `persist: null` — delete the provider's stored entry (`modelsStore.delete`, `models.ts:510`).
    Delete,
}

/// pi `ModelsPublication` (`packages/ai/src/models.ts:67-72` @v0.99.2-17): the provider-chosen
/// persistence plus an optional synchronous update of its own in-memory catalog.
///
/// The `update` runs only after the persistence step and only while the refresh is still the
/// provider's current one, so a superseded refresh can neither write the store nor change the
/// catalog. A cyrup provider's `models()` is a borrow of an immutable catalog, so "update" is
/// typically a re-registration of a new provider value carrying the new catalog: the registry's
/// replace path, which does not supersede the publishing refresh itself.
#[derive(Default)]
pub struct ModelsPublication {
    /// pi `persist?`. `None` leaves storage unchanged.
    pub persist: Option<ModelsPersist>,
    /// pi `update?: () => void`.
    pub update: Option<Box<dyn FnOnce() + Send + 'static>>,
}

/// The registry's side of [`RefreshModelsContext::publish`].
#[async_trait::async_trait]
pub trait ModelsPublisher: Send + Sync {
    /// pi `publish(publication): Promise<boolean>` (`models.ts:83`): `Ok(true)` when the publication
    /// was applied, `Ok(false)` when this refresh was superseded or aborted first (pi's
    /// generation/`signal.aborted` checks, `models.ts:507`, `:515`). A store failure is the error pi
    /// would reject with.
    async fn publish(&self, publication: ModelsPublication) -> Result<bool, ProviderError>;
}

/// The per-refresh context a dynamic provider's [`Provider::refresh_models`] receives — pi
/// `RefreshModelsContext` (`packages/ai/src/models.ts:74-90` @v0.99.2-17), threaded from the
/// refresh engine exactly as pi threads it from `Models.refresh` (`models.ts:527-544`).
///
/// It is an owned, `Clone` value, so a provider that does its work in a spawned task hands that
/// task a clone and the task sees the same credential, stored catalog and publisher. (Until
/// PROV-111 the credential, stored catalog and `publish` travelled through a tokio task-local
/// instead, which did not cross a `tokio::spawn`.)
///
/// `[CYRUP-DELTA]` **the three host-supplied members are optional.** pi's engine is the only
/// caller of `refreshModels` and always provides `credential?`, `stored?` and `publish`. Here two
/// engines call [`Provider::refresh_models`]: the session's extension-provider registry
/// (`cyrup-session-svc`'s `GuestProviderRegistry`), which fills all of them, and
/// [`crate::collection::Models::refresh_with`], whose persisting fetcher is
/// [`crate::remote_catalog::RemoteCatalog`] — it owns its own [`crate::models_store::ModelsStore`]
/// and auth context, so there `credential` and `stored` are `None` and `publisher` is `None`
/// (`publish` then answers `Ok(false)`: nothing was applied). pi's `store` member of the older
/// v0.83.0 shape (`:38`) has no counterpart: v0.99.2 replaced it with `stored` + `publish`.
#[derive(Clone)]
pub struct RefreshModelsContext {
    /// pi `credential?` — the effective credential: the stored one for the cache-only phase, the
    /// provider-auth-resolved one for the network phase (`models.ts:571`, `:575-577`).
    pub credential: Option<Credential>,
    /// pi `stored?` — the provider's persisted entry, snapshotted before this phase.
    pub stored: Option<crate::ModelsStoreEntry>,
    /// The classifier models persisted beside `stored` (the `type: "classifier"` members of pi's
    /// `stored.models`).
    pub stored_classifiers: Vec<ClassifierModel>,
    /// pi `publish` — the generation-checked publisher of this phase. `None` when the engine that
    /// called has no store to publish to; see the type's `[CYRUP-DELTA]`.
    pub publisher: Option<std::sync::Arc<dyn ModelsPublisher>>,
    /// `false` during offline / cache-only initialization (pi `allowNetwork`). A provider MUST
    /// restore its persisted catalog and perform no network I/O.
    pub allow_network: bool,
    /// Bypass provider freshness checks and fetch immediately when network access is allowed
    /// (pi `force`).
    pub force: bool,
    /// pi's `signal: AbortSignal`. **This is not advisory** — an implementation that can
    /// block MUST select on [`RefreshModelsContext::cancelled`] or check
    /// [`RefreshModelsContext::is_aborted`], because that is the only thing that makes
    /// [`crate::collection::ModelsRefreshResult::aborted`] mean anything. `Models::refresh_with`
    /// additionally skips any provider whose turn has not started when the token fires (pi's
    /// `if (options.signal?.aborted) return;`, `models.ts:286`).
    pub cancel: CancelToken,
}

impl std::fmt::Debug for RefreshModelsContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshModelsContext")
            // A credential carries the secret: say whether there is one, never what it is.
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "<redacted>"),
            )
            .field("stored", &self.stored)
            .field("stored_classifiers", &self.stored_classifiers)
            .field("publisher", &self.publisher.as_ref().map(|_| "<publisher>"))
            .field("allow_network", &self.allow_network)
            .field("force", &self.force)
            .field("cancel", &self.cancel)
            .finish()
    }
}

impl Default for RefreshModelsContext {
    /// pi's defaults for a bare `refresh()`: `allowNetwork = options.allowNetwork ?? true`
    /// (`models.ts:277`), `force` undefined ⇒ falsy, no signal.
    fn default() -> Self {
        Self {
            credential: None,
            stored: None,
            stored_classifiers: Vec::new(),
            publisher: None,
            allow_network: true,
            force: false,
            cancel: CancelToken::new(),
        }
    }
}

impl RefreshModelsContext {
    /// The offline posture: restore the persisted catalog, touch no network. This is both pi's
    /// startup call (`agent-session-services.ts:180`, `refresh({ allowNetwork: false })`) and the
    /// shape of its post-failure cache restore (`models.ts:314-319`).
    pub fn cache_only() -> Self {
        Self {
            allow_network: false,
            ..Self::default()
        }
    }

    /// pi `context.publish(publication)` (`models.ts:83`): hand a publication to this phase's
    /// publisher. `Ok(false)` — nothing applied, the caller should stop — when the refresh was
    /// superseded or aborted, and also when the engine supplied no publisher at all.
    ///
    /// # Errors
    ///
    /// The publisher's own failure (a store write that failed, an aborted wait).
    pub async fn publish(&self, publication: ModelsPublication) -> Result<bool, ProviderError> {
        match &self.publisher {
            Some(publisher) => publisher.publish(publication).await,
            None => Ok(false),
        }
    }

    /// Whether the caller has already aborted (pi `options.signal?.aborted`).
    pub fn is_aborted(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Resolves when the caller aborts. Implementations that block on I/O should race this —
    /// `tokio::select! { biased; () = ctx.cancelled() => …, r = fetch => … }`.
    pub async fn cancelled(&self) {
        self.cancel.cancelled().await;
    }
}

/// A runtime unit owning a model catalog, auth, and stream behavior (func-01 §6).
///
/// Slice: `stream` + catalog reads + `stream_simple` lowering + dynamic `refresh_models`. Auth
/// resolution rides on [`Provider::provider_auth`].
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &ProviderId;

    /// Human display name (Pi `Provider.name`, `models.ts:77` @v0.83.0). Provider pickers and
    /// status output show this rather than the machine id. Defaults to the id so an existing
    /// implementation is unchanged (PROV-017).
    fn name(&self) -> &str {
        self.id().as_str()
    }

    /// Provider-level default base URL (Pi `Provider.baseUrl?`, `models.ts:79` @v0.83.0).
    /// `None` = the provider has none and every model carries its own (PROV-017).
    fn base_url(&self) -> Option<&str> {
        None
    }

    /// Provider-level default headers (Pi `Provider.headers?: ProviderHeaders`, `models.ts:80`
    /// @v0.83.0), merged beneath the per-model and per-request overlays (PROV-017).
    fn headers(&self) -> Option<&crate::HeaderMap> {
        None
    }

    /// Last-known catalog; synchronous and non-throwing (func-01 R-01-001).
    fn models(&self) -> &[Model];

    /// Every model the provider lists, of every type (Pi `Provider.getAllModels?`,
    /// `models.ts:174`: "Complete synchronous catalog across every model type"). The default is the
    /// chat catalog wrapped as [`AnyModel::Chat`], which is what pi falls back to at every call
    /// site (`entry.getAllModels?.() ?? entry.getModels()`, `models.ts:451`, `:460`, `:723`). A
    /// provider that lists classifier models overrides this and returns them after its chat
    /// models, as pi's llama provider does (`getAllModels: () => [...models, ...classifiers]`,
    /// `extensions/llama/provider.ts:200`). Returns an owned list because such a catalog can sit
    /// behind a lock.
    fn get_all_models(&self) -> Vec<AnyModel> {
        self.models().iter().cloned().map(AnyModel::Chat).collect()
    }

    /// Optional provider policy for credential-specific model availability (Pi
    /// `Provider.filterModels?`, `models.ts:111` @v0.83.0, documented at `:105-110`).
    ///
    /// [`Provider::models`] remains the complete synchronous catalog; this is applied by
    /// [`crate::collection::Models::get_available`] **after** confirming that provider auth is
    /// configured — pi's exact position, `models.ts:407`. The default returns the catalog unchanged,
    /// matching pi's optional member being absent (PROV-032).
    fn filter_models(&self, models: &[Model], _credential: Option<&Credential>) -> Vec<Model> {
        models.to_vec()
    }

    /// Optional credential-specific availability policy across every model type (Pi
    /// `Provider.filterAllModels?`, `models.ts:196-199` @v1.0.1: "Without it,
    /// `Models.getAllAvailable()` applies `filterModels` to chat models and keeps every other
    /// model"). `None` is the member being absent; `Some` replaces the default policy outright, as
    /// `if (provider.filterAllModels) return provider.filterAllModels(models, credential)` does
    /// (`models.ts:724`). Applied by [`crate::collection::Models::get_all_available`] after auth is
    /// confirmed.
    fn filter_all_models(
        &self,
        _models: &[AnyModel],
        _credential: Option<&Credential>,
    ) -> Option<Vec<AnyModel>> {
        None
    }

    /// The provider's auth strategy (Pi `Provider.auth`). Exposed so a [`crate::collection::Models`]
    /// can resolve request auth against the collection's own credential store + auth context
    /// (Pi `models.ts:getAuth`/`applyAuth`). Default `None` for providers that fully encapsulate
    /// their own auth (the collection then delegates `stream()` without re-resolution). Additive.
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        None
    }

    fn get_model(&self, id: &str) -> Option<&Model> {
        self.models().iter().find(|m| m.id.as_str() == id)
    }

    /// Dynamic providers only: re-fetch and update the model list (Pi `Provider.refreshModels?`,
    /// `models.ts:104` @v0.83.0, documented at `:99-103`; PROV-041 corrected `:63`, which is
    /// `ModelsApiStreamOptions`). Side-effect-free discovery (no loading/downloading). Returns:
    ///
    /// - `None` for a static provider (no dynamic model source) — `Models::refresh` treats this as a
    ///   no-op, exactly as Pi's optional `refreshModels?` being `undefined`.
    /// - `Some(Ok(()))` when the refresh succeeded and the catalog was updated.
    /// - `Some(Err(_))` when the fetch failed; the list stays at its last-known state and a later
    ///   call retries (`Models::refresh(provider)` re-wraps it as a `model_source` error).
    ///
    /// Concurrent calls MUST share one in-flight fetch — an override builds that with
    /// [`crate::utils::refresh::RefreshDedup`]. The default is `None` (static provider). Additive.
    ///
    /// PROV-S05: `ctx` is pi's `RefreshModelsContext` argument (`models.ts:104`, constructed at
    /// `:297-303`). Before this the method took nothing, so `allowNetwork`, `force` and the abort
    /// signal could not reach an implementation at all. An implementation that performs network I/O
    /// **must** honour all three; see [`RefreshModelsContext`].
    async fn refresh_models(
        &self,
        _ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        None
    }

    /// Whether [`Provider::refresh_models`] does anything — pi's `refreshModels !== undefined`
    /// (`packages/ai/src/models.ts:552-554`, identical at v0.99.2 and v1.0.0-25). pi's
    /// engine filters static providers out BEFORE it begins a refresh (`beginProviderRefresh`,
    /// `:559`), because beginning one supersedes whatever refresh of the same id is still in
    /// flight. A Rust trait method cannot be absent, and answering `None` is only knowable by
    /// calling it, so this is the member that carries the filter (SEAM-142).
    ///
    /// **Every provider whose `refresh_models` can answer `Some` MUST override this to `true`**
    /// (and a wrapper must forward it with `refresh_models`): the registry never calls
    /// `refresh_models` on a provider that answers `false`. A provider whose answer depends on its
    /// construction (an optional store) answers for that state.
    fn has_refresh_models(&self) -> bool {
        false
    }

    /// Generate images with one of this provider's image models (pi `Provider.generateImages?`,
    /// `models.ts:220-225`: "Present when the provider supports dedicated image models. Never
    /// rejects.").
    ///
    /// PROV-128 — pi's member is optional and [`crate::collection::Models::generate_images`] turns
    /// its absence into `Provider ${model.provider} does not support image generation`
    /// (`models.ts:956-959`). A Rust trait method cannot be absent, so the default IS that absent
    /// case: an error envelope with that message. A provider with image models overrides this,
    /// typically by delegating to a [`crate::images::ImageApiRegistry`] (pi's
    /// `createProvider({ images })` dispatch on `model.api`, `models.ts:1146-1160`) — which is
    /// what [`crate::wire::WireProvider::with_images`] installs.
    ///
    /// Like [`Provider::classify`], this never fails as a call: every failure is an error
    /// [`AssistantImages`]. The default is `models.ts:955-962` end to end: the `throw` sits inside
    /// pi's `try`, whose `catch` is `imageErrorResult(model, error, options?.signal?.aborted)`, so
    /// the envelope is `aborted` (not `error`) when the request was already cancelled.
    async fn generate_images(
        &self,
        model: &ImageModel,
        _context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages {
        AssistantImages::errored(
            model,
            format!(
                "Provider {} does not support image generation",
                model.provider
            ),
            options.is_aborted(),
        )
    }

    /// Whether [`Provider::generate_images`] is more than the absent-member default (pi's
    /// `if (!provider.generateImages)` presence test, `models.ts:956`).
    ///
    /// [`crate::collection::Models::generate_images`] asks this BEFORE it applies request auth,
    /// because pi throws `does not support image generation` ahead of `applyAuth`
    /// (`models.ts:954-959`): a provider that cannot generate images must neither need a credential
    /// nor trigger an OAuth refresh. The default is "this provider lists an image model" (pi
    /// attaches `generateImages` exactly when the provider was created with image
    /// implementations, `models.ts:1146`). A provider that overrides [`Provider::generate_images`]
    /// without listing image models overrides this too.
    fn supports_image_generation(&self) -> bool {
        self.get_all_models()
            .iter()
            .any(|m| matches!(m, AnyModel::Image(_)))
    }

    /// Classify structured state with one of this provider's classifier models (Pi
    /// `Provider.classify?`, `models.ts:227-232`: "Present when the provider supports structured
    /// classifier models. Never rejects.").
    ///
    /// Pi's member is optional and [`crate::collection::Models::classify`] turns its absence into
    /// `Provider ${model.provider} does not support classification` (`models.ts:974-976`). A Rust
    /// trait method cannot be absent, so the default IS that absent case: an error result with that
    /// message. A provider with classifier models overrides this, typically by delegating to a
    /// [`crate::classifier::ClassifierApiRegistry`] (pi's `createProvider({ classifiers })`
    /// dispatch on `model.api`, `models.ts:1161-1171`).
    ///
    /// Like [`Provider::stream`], this never fails as a call: every failure is an error
    /// [`ClassifierResult`].
    ///
    /// The default is `models.ts:973-980` end to end: the `throw` sits inside pi's `try`, whose
    /// `catch` is `classifierErrorResult(model, error, options?.signal?.aborted)`, so the result is
    /// `aborted` (not `error`) when the request was already cancelled.
    async fn classify(
        &self,
        model: &ClassifierModel,
        _context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> ClassifierResult {
        ClassifierResult::errored(
            model,
            format!(
                "Provider {} does not support classification",
                model.provider
            ),
            options.is_aborted(),
        )
    }

    /// Whether [`Provider::classify`] is more than the absent-member default (pi's
    /// `if (!provider.classify)` presence test, `models.ts:974`).
    ///
    /// [`crate::collection::Models::classify`] asks this BEFORE it applies request auth, because pi
    /// throws `does not support classification` ahead of `applyAuth` (`models.ts:972-977`): a
    /// provider that cannot classify must neither need a credential nor trigger an OAuth refresh.
    /// The default is "this provider lists a classifier model" (pi attaches `classify` exactly when
    /// the provider was created with classifier implementations, `models.ts:1161`). A provider
    /// that overrides [`Provider::classify`] without listing classifier models overrides this too.
    fn supports_classification(&self) -> bool {
        self.get_all_models()
            .iter()
            .any(|m| matches!(m, AnyModel::Classifier(_)))
    }

    /// Construct the response stream. Returns immediately; setup happens behind the stream and
    /// failures are delivered as a terminal `StreamEvent::Error` (func-01 R-01-009/045) — this
    /// method never returns `Err`.
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent>;

    /// Stream with the unified "simple" option surface (Pi `Provider.streamSimple`,
    /// `models.ts:119` @v0.83.0; PROV-041 corrected `:71`, a prose line in the `Provider` docblock).
    ///
    /// Lowers a [`SimpleStreamOptions`] to a concrete [`StreamOptions`] and delegates to
    /// [`Provider::stream`]. The default mirrors Pi's per-API `streamSimple` for the
    /// `openai-completions` family (the only wire protocol present), `api/openai-completions.ts:478`:
    ///
    /// 1. `build_base_options` clamps `max_tokens` to the remaining context window and threads every
    ///    transport-level field (`buildBaseOptions`, simple-options.ts:21).
    /// 2. The unified `reasoning` on-level is clamped to one the model supports via
    ///    [`clamp_thinking_level`] (`clampThinkingLevel(model, options.reasoning)`,
    ///    openai-completions.ts:486); a clamp result of `off` disables reasoning
    ///    (`clampedReasoning === "off" ? undefined`, openai-completions.ts:487).
    ///
    /// Token-budget providers (anthropic-messages / google-generative-ai) override this to split the
    /// budget via `adjust_max_tokens_for_thinking`; they land with their wire protocols. Like
    /// [`Provider::stream`], this never returns `Err` — failures arrive as a terminal
    /// [`StreamEvent::Error`].
    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> EventStream<StreamEvent> {
        // `apiKey || options?.apiKey` — buildBaseOptions already applies this precedence, so pass the
        // option key through (no separate request-key here at the provider edge).
        let mut lowered =
            build_base_options(model, context, options, options.base.api_key.as_deref());
        // `options?.reasoning ? clampThinkingLevel(...) : undefined`; `off` collapses to "disabled".
        if let Some(level) = options.reasoning {
            lowered.reasoning = clamp_thinking_level(model, level.into());
        }
        self.stream(model, context, &lowered)
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
    use crate::model::{Modality, ModelCost};
    use crate::stream::collect_message;
    use crate::utils::simple_options::SimpleStreamOptions;
    use cyrup_core::{AssistantMessage, Message, ModelThinkingLevel, StopReason, ThinkingLevel};
    use std::sync::{Arc, Mutex};
    use tokio_stream::wrappers::ReceiverStream;

    /// A `Provider` whose `stream()` records the exact [`StreamOptions`] it was handed, then yields a
    /// single terminal `Done`. Lets a `stream_simple` test assert the lowering applied to the options.
    struct RecordingProvider {
        id: ProviderId,
        models: Vec<Model>,
        seen: Arc<Mutex<Option<StreamOptions>>>,
    }

    impl Provider for RecordingProvider {
        fn id(&self) -> &ProviderId {
            &self.id
        }
        fn models(&self) -> &[Model] {
            &self.models
        }
        fn stream(
            &self,
            model: &Model,
            _context: &Context,
            options: &StreamOptions,
        ) -> EventStream<StreamEvent> {
            if let Ok(mut g) = self.seen.lock() {
                *g = Some(options.clone());
            }
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let msg = AssistantMessage::errored(
                model.provider.clone(),
                model.id.as_str(),
                Some(model.api.clone()),
                StopReason::Stop,
                "",
            );
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::terminal(msg)).await;
            });
            Box::pin(ReceiverStream::new(rx))
        }
    }

    fn reasoning_model(context_window: u64, max_tokens: u64) -> Model {
        Model {
            id: "m1".into(),
            name: "M1".into(),
            api: "openai-completions".into(),
            provider: "p".into(),
            base_url: String::new(),
            reasoning: true,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window,
            max_tokens,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn ctx() -> Context {
        Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![cyrup_core::Content::Text {
                    text: "hi".into(),
                    text_signature: None,
                }],
                timestamp: 0,
            }],
            tools: Vec::new(),
        }
    }

    fn recorder(model: Model) -> (RecordingProvider, Arc<Mutex<Option<StreamOptions>>>) {
        let seen = Arc::new(Mutex::new(None));
        (
            RecordingProvider {
                id: ProviderId::from("p"),
                models: vec![model],
                seen: seen.clone(),
            },
            seen,
        )
    }

    /// Pi `streamSimple` (openai-completions.ts:486-487): the unified on-level is clamped to a level
    /// the model supports. `xhigh` is unsupported without an explicit map entry, so it walks down to
    /// `high`; `max_tokens` defaults to the model cap then is clamped to fit the window.
    #[tokio::test]
    async fn stream_simple_lowers_reasoning_and_clamps_max_tokens() {
        let model = reasoning_model(100_000, 8_000);
        let (provider, seen) = recorder(model.clone());
        let opts = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Xhigh),
            ..Default::default()
        };
        let _ = collect_message(provider.stream_simple(&model, &ctx(), &opts)).await;
        let lowered = seen.lock().unwrap().clone().expect("stream() saw options");
        // xhigh (no map entry) clamps to high.
        assert_eq!(lowered.reasoning, ModelThinkingLevel::High);
        // defaulted to the model cap, then clamped to fit the (large) window.
        assert_eq!(lowered.max_tokens, Some(8_000));
    }

    /// No unified `reasoning` → the lowered options keep the default `off` (Pi: `reasoning`
    /// `undefined`), and an explicit `max_tokens` is threaded then clamped.
    #[tokio::test]
    async fn stream_simple_without_reasoning_keeps_off_and_threads_max_tokens() {
        let model = reasoning_model(100_000, 8_000);
        let (provider, seen) = recorder(model.clone());
        let mut opts = SimpleStreamOptions::default();
        opts.base.max_tokens = Some(2_000);
        let _ = collect_message(provider.stream_simple(&model, &ctx(), &opts)).await;
        let lowered = seen.lock().unwrap().clone().expect("stream() saw options");
        assert_eq!(lowered.reasoning, ModelThinkingLevel::Off);
        assert_eq!(lowered.max_tokens, Some(2_000));
    }
}
